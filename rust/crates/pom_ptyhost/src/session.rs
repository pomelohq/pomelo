use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::pty;
use crate::queries::answer_queries;

/// Output kept for reattaching clients. Larger than the previous core's 256 KB so a long build log isn't
/// cut; clients of either core read it fine since snapshots are chunked.
pub const RING_CAPACITY: usize = 1024 * 1024;
/// Chunks a slow client may fall behind by before it misses live output (it still resumes from the ring).
const SUBSCRIBER_BACKLOG: usize = 256;
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

pub type ExitHook = Box<dyn FnOnce(&[u8], &ExitStatus) + Send>;

pub struct StartOptions {
    pub argv: Vec<String>,
    pub dir: PathBuf,
    /// The complete environment of the command.
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    pub on_exit: Option<ExitHook>,
}

/// What a subscriber gets: the scrollback after its resume point, the absolute offset at the end of it, and
/// the live output from there on (the channel closes when the command exits).
pub struct Subscription {
    pub snapshot: Vec<u8>,
    pub end: u64,
    pub output: Receiver<Arc<[u8]>>,
    id: u64,
}

impl Subscription {
    pub fn id(&self) -> u64 {
        self.id
    }
}

#[derive(Default)]
struct Output {
    ring: VecDeque<u8>,
    total: u64,
    subscribers: HashMap<u64, SyncSender<Arc<[u8]>>>,
    next_subscriber: u64,
}

#[derive(Clone, Copy, Default)]
struct ClientSize {
    cols: u16,
    rows: u16,
    primary: bool,
}

struct Clients {
    sizes: HashMap<u64, ClientSize>,
    next: u64,
    applied: (u16, u16),
}

struct Inner {
    master: File,
    pid: u32,
    output: Mutex<Output>,
    clients: Mutex<Clients>,
    exit: Mutex<Option<ExitStatus>>,
    exited: Condvar,
}

/// A command on a PTY with its scrollback and any number of attached clients.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

impl Session {
    pub fn start(mut options: StartOptions) -> io::Result<Session> {
        let cols = if options.cols == 0 {
            DEFAULT_COLS
        } else {
            options.cols
        };
        let rows = if options.rows == 0 {
            DEFAULT_ROWS
        } else {
            options.rows
        };
        ensure_terminal_env(&mut options.env);
        let pty::Pty { master, mut child } =
            pty::spawn(&options.argv, &options.dir, &options.env, cols, rows)?;
        let reader = master.try_clone()?;
        let inner = Arc::new(Inner {
            master,
            pid: child.id(),
            output: Mutex::new(Output::default()),
            clients: Mutex::new(Clients {
                sizes: HashMap::new(),
                next: 0,
                applied: (cols, rows),
            }),
            exit: Mutex::new(None),
            exited: Condvar::new(),
        });
        let (read_done, read_finished) = std::sync::mpsc::channel::<()>();
        let reading = inner.clone();
        std::thread::Builder::new()
            .name("ptyhost-read".into())
            .spawn(move || {
                read_loop(&reading, reader);
                drop(read_done);
            })?;
        let waiting = inner.clone();
        let on_exit = options.on_exit.take();
        std::thread::Builder::new()
            .name("ptyhost-wait".into())
            .spawn(move || {
                let status = match child.wait() {
                    Ok(status) => status,
                    Err(error) => {
                        eprintln!("ptyhost: wait failed: {error}");
                        return;
                    }
                };
                // Let the reader drain what the command printed last, but don't hang on a grandchild
                // that still holds the terminal open.
                let _drained = read_finished.recv_timeout(Duration::from_millis(500));
                if let Some(on_exit) = on_exit {
                    on_exit(&waiting.scrollback(), &status);
                }
                if let Ok(mut output) = waiting.output.lock() {
                    output.subscribers.clear();
                }
                if let Ok(mut exit) = waiting.exit.lock() {
                    *exit = Some(status);
                }
                waiting.exited.notify_all();
            })?;
        Ok(Session { inner })
    }

    pub fn pid(&self) -> u32 {
        self.inner.pid
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        (&self.inner.master).write_all(bytes)
    }

    pub fn scrollback(&self) -> Vec<u8> {
        self.inner.scrollback()
    }

    /// Subscribe from absolute offset `since`: a stale or zero offset replays the whole ring.
    pub fn subscribe_since(&self, since: u64) -> Subscription {
        let (sender, output) = std::sync::mpsc::sync_channel(SUBSCRIBER_BACKLOG);
        let Ok(mut state) = self.inner.output.lock() else {
            return Subscription {
                snapshot: Vec::new(),
                end: 0,
                output,
                id: u64::MAX,
            };
        };
        let end = state.total;
        let base = end - state.ring.len() as u64;
        let skip = if since > base && since <= end {
            (since - base) as usize
        } else {
            0
        };
        let snapshot = state.ring.iter().skip(skip).copied().collect();
        state.next_subscriber += 1;
        let id = state.next_subscriber;
        if self.exit_status().is_none() {
            state.subscribers.insert(id, sender);
        }
        Subscription {
            snapshot,
            end,
            output,
            id,
        }
    }

    pub fn unsubscribe(&self, subscription: &Subscription) {
        self.unsubscribe_id(subscription.id);
    }

    /// Ends a subscription from another thread; its receiver sees the channel close.
    pub fn unsubscribe_id(&self, id: u64) {
        if let Ok(mut state) = self.inner.output.lock() {
            state.subscribers.remove(&id);
        }
    }

    pub fn add_client(&self) -> u64 {
        let Ok(mut clients) = self.inner.clients.lock() else {
            return 0;
        };
        clients.next += 1;
        let id = clients.next;
        clients.sizes.insert(id, ClientSize::default());
        id
    }

    pub fn remove_client(&self, id: u64) {
        self.update_clients(|clients| {
            clients.remove(&id);
        });
    }

    pub fn client_size(&self, id: u64, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        self.update_clients(|clients| {
            let size = clients.entry(id).or_default();
            size.cols = cols;
            size.rows = rows;
        });
    }

    /// The primary client (the app's visible tab) decides the size alone while it has one.
    pub fn set_primary(&self, id: u64) {
        self.update_clients(|clients| clients.entry(id).or_default().primary = true);
    }

    pub fn size(&self) -> (u16, u16) {
        self.inner
            .clients
            .lock()
            .map_or((DEFAULT_COLS, DEFAULT_ROWS), |clients| clients.applied)
    }

    fn update_clients(&self, change: impl FnOnce(&mut HashMap<u64, ClientSize>)) {
        let Ok(mut clients) = self.inner.clients.lock() else {
            return;
        };
        change(&mut clients.sizes);
        let Some(size) = smallest_size(&clients.sizes) else {
            return;
        };
        if size != clients.applied {
            clients.applied = size;
            if let Err(error) = pty::resize(&self.inner.master, size.0, size.1) {
                eprintln!("ptyhost: resize failed: {error}");
            }
        }
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.inner.exit.lock().ok().and_then(|exit| *exit)
    }

    pub fn wait(&self) -> Option<ExitStatus> {
        let mut exit = self.inner.exit.lock().ok()?;
        while exit.is_none() {
            exit = self.inner.exited.wait(exit).ok()?;
        }
        *exit
    }

    pub fn wait_timeout(&self, timeout: Duration) -> Option<ExitStatus> {
        let exit = self.inner.exit.lock().ok()?;
        let (exit, _) = self
            .inner
            .exited
            .wait_timeout_while(exit, timeout, |exit| exit.is_none())
            .ok()?;
        *exit
    }

    pub fn kill(&self) {
        // SAFETY: signalling our own child's pid.
        unsafe {
            libc::kill(self.inner.pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

impl Inner {
    fn scrollback(&self) -> Vec<u8> {
        self.output
            .lock()
            .map(|output| output.ring.iter().copied().collect())
            .unwrap_or_default()
    }

    fn publish(&self, chunk: &[u8]) {
        let Ok(mut state) = self.output.lock() else {
            return;
        };
        state.total += chunk.len() as u64;
        state.ring.extend(chunk);
        let excess = state.ring.len().saturating_sub(RING_CAPACITY);
        state.ring.drain(..excess);
        let shared: Arc<[u8]> = Arc::from(chunk);
        state
            .subscribers
            .retain(|_, sender| match sender.try_send(shared.clone()) {
                Ok(()) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            });
    }
}

fn read_loop(inner: &Inner, mut reader: File) {
    let mut buffer = vec![0u8; 32 * 1024];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => {
                carry.extend_from_slice(&buffer[..read]);
                let (output, reply, rest) = answer_queries(&carry);
                carry = rest;
                if !reply.is_empty() {
                    if let Err(error) = (&inner.master).write_all(&reply) {
                        eprintln!("ptyhost: query reply failed: {error}");
                    }
                }
                if !output.is_empty() {
                    inner.publish(&output);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            // The master reads EIO once the last process holding the terminal closes it.
            Err(_) => return,
        }
    }
}

fn smallest_size(clients: &HashMap<u64, ClientSize>) -> Option<(u16, u16)> {
    let sized = |size: &&ClientSize| size.cols > 0 && size.rows > 0;
    let primary_only = clients.values().filter(sized).any(|size| size.primary);
    let (cols, rows) = clients
        .values()
        .filter(sized)
        .filter(|size| !primary_only || size.primary)
        .fold((0u16, 0u16), |(cols, rows), size| {
            (
                if cols == 0 {
                    size.cols
                } else {
                    cols.min(size.cols)
                },
                if rows == 0 {
                    size.rows
                } else {
                    rows.min(size.rows)
                },
            )
        });
    (cols > 0 && rows > 0).then_some((cols, rows))
}

fn ensure_terminal_env(env: &mut Vec<(String, String)>) {
    for (key, value) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")] {
        if !env.iter().any(|(existing, _)| existing == key) {
            env.push((key.to_string(), value.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn options(argv: &[&str]) -> StartOptions {
        StartOptions {
            argv: argv.iter().map(|part| part.to_string()).collect(),
            dir: PathBuf::from("/"),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cols: 80,
            rows: 24,
            on_exit: None,
        }
    }

    fn wait_for(subscription: &Subscription, want: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = subscription.snapshot.clone();
        while Instant::now() < deadline {
            if String::from_utf8_lossy(&seen).contains(want) {
                return true;
            }
            match subscription.output.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => seen.extend_from_slice(&chunk),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return String::from_utf8_lossy(&seen).contains(want);
                }
            }
        }
        false
    }

    #[test]
    fn runs_to_completion_and_reports_exit() -> io::Result<()> {
        let exited = Arc::new(Mutex::new(Vec::new()));
        let record = exited.clone();
        let mut opts = options(&["sh", "-c", "printf hello-pty"]);
        opts.on_exit = Some(Box::new(move |scrollback, _| {
            if let Ok(mut out) = record.lock() {
                out.extend_from_slice(scrollback);
            }
        }));
        let session = Session::start(opts)?;
        let subscription = session.subscribe_since(0);
        assert!(wait_for(&subscription, "hello-pty"));
        assert!(session.wait().is_some_and(|status| status.success()));
        let scrollback = exited.lock().map(|out| out.clone()).unwrap_or_default();
        assert!(String::from_utf8_lossy(&scrollback).contains("hello-pty"));
        Ok(())
    }

    #[test]
    fn echoes_input_to_every_subscriber() -> io::Result<()> {
        let session = Session::start(options(&["cat"]))?;
        let first = session.subscribe_since(0);
        let second = session.subscribe_since(0);
        session.write(b"fan-out\n")?;
        assert!(wait_for(&first, "fan-out"));
        assert!(wait_for(&second, "fan-out"));
        session.kill();
        assert!(session.wait_timeout(Duration::from_secs(5)).is_some());
        Ok(())
    }

    #[test]
    fn snapshot_holds_earlier_output_and_resumes_by_offset() -> io::Result<()> {
        let session = Session::start(options(&["sh", "-c", "printf marker-42; sleep 3"]))?;
        let first = session.subscribe_since(0);
        assert!(wait_for(&first, "marker-42"));
        let late = session.subscribe_since(0);
        assert!(String::from_utf8_lossy(&late.snapshot).contains("marker-42"));
        let resumed = session.subscribe_since(late.end);
        assert!(resumed.snapshot.is_empty(), "nothing new since the offset");
        session.kill();
        Ok(())
    }

    #[test]
    fn smallest_client_wins_and_primary_decides_alone() -> io::Result<()> {
        let session = Session::start(options(&["cat"]))?;
        let big = session.add_client();
        let small = session.add_client();
        session.client_size(big, 200, 60);
        session.client_size(small, 100, 40);
        assert_eq!(session.size(), (100, 40));
        session.remove_client(small);
        assert_eq!(session.size(), (200, 60));
        let other = session.add_client();
        session.client_size(other, 90, 20);
        assert_eq!(session.size(), (90, 20));
        session.set_primary(big);
        assert_eq!(session.size(), (200, 60));
        session.kill();
        Ok(())
    }

    #[test]
    fn ring_keeps_only_the_newest_output() -> io::Result<()> {
        let script = format!(
            "head -c {} /dev/zero | tr '\\0' a; printf END",
            RING_CAPACITY + 50_000
        );
        let session = Session::start(options(&["sh", "-c", &script]))?;
        assert!(session.wait_timeout(Duration::from_secs(10)).is_some());
        let subscription = session.subscribe_since(0);
        assert_eq!(subscription.snapshot.len(), RING_CAPACITY);
        assert!(subscription.snapshot.ends_with(b"END"));
        assert!(subscription.end > RING_CAPACITY as u64);
        Ok(())
    }
}
