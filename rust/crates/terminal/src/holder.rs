//! The terminal's grid fed from a detached PTY holder instead of a PTY this process owns, so the shell keeps
//! running across app restarts. Output bytes go through the same VT parser into the same grid as the local
//! backend; input, resizes and replies go back to the holder as frames.

use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as BackendEvent, EventListener, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::Processor;
use pom_ptyhost::{HolderConnection, SocketDir, SpawnRequest};

const HOLDER_START_TIMEOUT: Duration = Duration::from_secs(5);

/// Where a terminal's shell lives: holder `name` in `dir`, started by re-executing `binary` when it isn't
/// running yet.
#[derive(Clone, Debug)]
pub struct HolderOptions {
    pub dir: SocketDir,
    pub name: String,
    pub binary: PathBuf,
    pub attach_only: bool,
    /// The workspace a new holder joins, capped by its limits.
    pub group: Option<pom_ptyhost::HolderGroup>,
}

pub(crate) struct HolderBackend {
    pub(crate) options: HolderOptions,
    connection: HolderConnection,
    /// The command the holder runs (the shell), for the tab title's foreground process.
    pub(crate) shell_pid: Option<u32>,
    detached: Arc<AtomicBool>,
}

impl HolderBackend {
    /// Attaches to the holder (starting it first when needed) and feeds its scrollback and live output into
    /// `term` on a reader thread.
    pub(crate) fn attach<L: EventListener + Send + 'static>(
        options: HolderOptions,
        spawn: SpawnRequest<'_>,
        term: Arc<FairMutex<Term<L>>>,
        listener: L,
    ) -> io::Result<HolderBackend> {
        if !options.dir.holder_alive(&options.name) {
            if options.attach_only {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} is not running", options.name),
                ));
            }
            pom_ptyhost::spawn_holder(&options.dir, &spawn)?;
        }
        pom_ptyhost::wait_for_holder(&options.dir, &options.name, HOLDER_START_TIMEOUT)?;
        let attached = pom_ptyhost::attach(&options.dir, &options.name, 0)?;
        let mut processor: Processor = Processor::new();
        processor.advance(&mut *term.lock(), &attached.snapshot);
        listener.send_event(BackendEvent::Wakeup);
        let shell_pid = options
            .dir
            .holder_pid(&options.name)
            .and_then(first_child)
            .map(|pid| pid as u32);
        let detached = Arc::new(AtomicBool::new(false));
        let reading = Reading {
            dir: options.dir.clone(),
            name: options.name.clone(),
            offset: attached.end,
            detached: detached.clone(),
        };
        let output = attached.output;
        std::thread::Builder::new()
            .name("terminal-holder".into())
            .spawn(move || read_output(output, processor, &term, &listener, reading))?;
        Ok(HolderBackend {
            options,
            connection: attached.connection,
            shell_pid,
            detached,
        })
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) {
        if let Err(error) = self.connection.input(bytes) {
            eprintln!("terminal holder input: {error}");
        }
    }

    pub(crate) fn resize(&mut self, size: WindowSize) {
        if let Err(error) = self.connection.resize(size.num_cols, size.num_lines) {
            eprintln!("terminal holder resize: {error}");
        }
    }

    /// This client's size wins over other clients of the same holder while it is visible.
    pub(crate) fn make_primary(&mut self) {
        if let Err(error) = self.connection.primary() {
            eprintln!("terminal holder primary: {error}");
        }
    }

    pub(crate) fn detach(&self) {
        self.detached.store(true, Ordering::Release);
        if let Err(error) = self.connection.shutdown() {
            if error.kind() != io::ErrorKind::NotConnected {
                eprintln!("terminal holder detach: {error}");
            }
        }
    }
}

/// macOS can leave a blocked read asleep after the holder half-closes a reader that fell behind; the next read
/// sees the end, so no read waits longer than this.
const END_RECHECK: Duration = Duration::from_secs(1);

struct Reading {
    dir: SocketDir,
    name: String,
    offset: u64,
    detached: Arc<AtomicBool>,
}

fn read_output<L: EventListener>(
    mut output: std::io::BufReader<std::os::unix::net::UnixStream>,
    mut processor: Processor,
    term: &FairMutex<Term<L>>,
    listener: &L,
    mut reading: Reading,
) {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut _resumed: Option<HolderConnection> = None;
    loop {
        // A synchronized update holds frames until it ends or times out; wake up in time to end it.
        let sync_deadline = processor.sync_timeout().sync_timeout();
        let timeout = sync_deadline.map_or(END_RECHECK, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .clamp(Duration::from_millis(1), END_RECHECK)
        });
        if let Err(error) = output.get_ref().set_read_timeout(Some(timeout)) {
            eprintln!("terminal holder: {error}");
        }
        match output.read(&mut buffer) {
            Ok(0) => {
                if reading.detached.load(Ordering::Acquire) {
                    break;
                }
                let Ok(attached) = pom_ptyhost::attach(&reading.dir, &reading.name, reading.offset)
                else {
                    break;
                };
                processor.advance(&mut *term.lock(), &attached.snapshot);
                listener.send_event(BackendEvent::Wakeup);
                reading.offset = attached.end;
                output = attached.output;
                _resumed = Some(attached.connection);
            }
            Ok(read) => {
                processor.advance(&mut *term.lock(), &buffer[..read]);
                reading.offset += read as u64;
                listener.send_event(BackendEvent::Wakeup);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if sync_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    processor.stop_sync(&mut *term.lock());
                    listener.send_event(BackendEvent::Wakeup);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    // The connection ends when the command exits (or we detached). Only an exit reports a status; the
    // exiting holder removes its pidfile a moment after closing the connection.
    let (dir, name) = (&reading.dir, reading.name.as_str());
    let deadline = Instant::now() + Duration::from_secs(1);
    while dir.holder_alive(name) {
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let failed = dir.crash_info(name).is_some_and(|info| info.crashed);
    let status = ExitStatus::from_raw(if failed { 1 << 8 } else { 0 });
    listener.send_event(BackendEvent::ChildExit(status));
}

fn first_child(pid: i32) -> Option<i32> {
    pom_ptyhost::descendants(pid).get(1).copied()
}
