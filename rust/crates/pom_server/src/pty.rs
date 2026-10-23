//! Terminals for a remote app: `pty.open` starts or finds a holder of a workspace, `pty.attach` bridges it onto
//! a stream. The bridge always drains the holder, because a holder drops output for a client that falls behind;
//! when the app stops acknowledging, it detaches instead and resumes from the holder's ring by offset.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader, Read};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pom_ptyhost::{HolderConnection, SocketDir, SpawnRequest};
use pom_remote::server_api::{FollowLogsParams, FollowedLogs};
use pom_remote::{LocalSource, ProjectSource, WorkspaceTerminals};
use pom_rpc::pty::{
    AttachParams, Attached, ChannelParams, CloseParams, Exit, Kind, Mode, OpenParams, Opened,
    ResizeParams, EXIT_TOPIC, MAX_SIZE,
};
use pom_rpc::{
    error_code, Connection, InputReceiver, Publisher, RemoteError, RequestContext, StreamSender,
    MAX_DATA,
};
use serde_json::Value;

const RING: u64 = pom_ptyhost::RING_CAPACITY as u64;
const HOLDER_START: Duration = Duration::from_secs(5);
/// How often a waiting bridge looks up from its wait to notice a detach.
const POLL: Duration = Duration::from_millis(100);
const MAX_CLAIM: usize = 256;
/// macOS can leave a blocked read asleep after the holder half-closes a reader that fell behind; the next read
/// sees the end, so no read waits longer than this.
const END_RECHECK: Duration = Duration::from_secs(1);
/// How long a vanished client's input numbering and followed streams are kept for it to reconnect.
pub const RETENTION: Duration = Duration::from_secs(10 * 60);
const MAX_TERMINAL_ID: usize = 64;

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(error_code::INVALID_PARAMS, message)
}

fn failed(message: impl Into<String>) -> RemoteError {
    RemoteError::new(crate::dispatch::FAILED, message)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn check_size(cols: u16, rows: u16) -> Result<(), RemoteError> {
    if (1..=MAX_SIZE).contains(&cols) && (1..=MAX_SIZE).contains(&rows) {
        Ok(())
    } else {
        Err(invalid(format!(
            "cols and rows must be 1..{MAX_SIZE}, got {cols}x{rows}"
        )))
    }
}

/// A holder this server may touch: one of `workspace`'s shells, services or agents.
pub fn check_holder(workspace: &WorkspaceTerminals, holder: &str) -> Result<(), RemoteError> {
    let (kind, rest) = holder.split_once('-').unwrap_or_default();
    let shaped = matches!(kind, "ws" | "svc" | "term")
        && holder.len() <= 255
        && !rest.is_empty()
        && rest
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'));
    if !shaped {
        return Err(invalid(format!("{holder:?} is not a holder name")));
    }
    if !workspace.owns(holder) {
        return Err(invalid(format!(
            "{holder} is not a terminal of this workspace"
        )));
    }
    Ok(())
}

/// `cwd` must name a directory inside the workspace's worktree, symlinks resolved.
pub fn check_cwd(worktree: &Path, cwd: &str) -> Result<PathBuf, RemoteError> {
    let path = Path::new(cwd);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree.join(path)
    };
    let resolved =
        std::fs::canonicalize(&path).map_err(|error| invalid(format!("cwd {cwd}: {error}")))?;
    let root = std::fs::canonicalize(worktree)
        .map_err(|error| failed(format!("{}: {error}", worktree.display())))?;
    if !resolved.starts_with(&root) || !resolved.is_dir() {
        return Err(invalid(format!(
            "cwd {cwd} is not a folder inside the workspace"
        )));
    }
    Ok(resolved)
}

fn check_text(what: &str, text: &str, limit: usize) -> Result<(), RemoteError> {
    if text.len() > limit || text.chars().any(char::is_control) {
        return Err(invalid(format!("{what} is not valid")));
    }
    Ok(())
}

/// The shell a new terminal runs, and the variables it gets on top of the server's.
#[derive(Clone, Debug)]
pub struct ShellCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl ShellCommand {
    pub fn login() -> ShellCommand {
        let mut env = vec![
            ("TERM_PROGRAM".to_string(), "pomelo".to_string()),
            (
                "TERM_PROGRAM_VERSION".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
            ),
        ];
        if std::env::var_os("LANG").is_none() {
            env.push(("LANG".to_string(), "en_US.UTF-8".to_string()));
        }
        ShellCommand {
            argv: pom_shell::Shell::user().interactive_argv(),
            env,
        }
    }
}

type InputKey = (String, String);

struct InputRecord {
    applied: u64,
    /// Bridges typing under this key right now.
    users: usize,
    /// When the last of them ended; the record goes once that is older than the retention.
    released: Option<Instant>,
}

/// Input already written per (client terminal, holder), so a resent chunk is never typed twice, kept a while
/// after the client went so a reconnect resumes the numbering.
#[derive(Default)]
struct Inputs {
    applied: Mutex<HashMap<InputKey, InputRecord>>,
}

impl Inputs {
    /// Where a terminal's numbering goes on: its own `resume` point, or what the holder already took.
    fn start(&self, key: &InputKey, resume: Option<u64>) -> u64 {
        let mut applied = locked(&self.applied);
        let record = applied.entry(key.clone()).or_insert(InputRecord {
            applied: 0,
            users: 0,
            released: None,
        });
        record.users += 1;
        record.released = None;
        match resume {
            Some(offset) => offset,
            None => record.applied,
        }
    }

    fn release(&self, key: &InputKey) {
        if let Some(record) = locked(&self.applied).get_mut(key) {
            record.users = record.users.saturating_sub(1);
            if record.users == 0 {
                record.released = Some(Instant::now());
            }
        }
    }

    /// The part of `bytes` (numbered from `offset`) not written yet.
    fn fresh<'a>(&self, key: &InputKey, offset: u64, bytes: &'a [u8]) -> &'a [u8] {
        let mut applied = locked(&self.applied);
        let Some(record) = applied.get_mut(key) else {
            return bytes;
        };
        let end = offset.saturating_add(bytes.len() as u64);
        let skip = record
            .applied
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        record.applied = record.applied.max(end);
        bytes.get(skip..).unwrap_or_default()
    }

    fn sweep(&self, retention: Duration) -> usize {
        let mut applied = locked(&self.applied);
        applied.retain(|_, record| {
            record
                .released
                .is_none_or(|released| released.elapsed() < retention)
        });
        applied
            .values()
            .filter(|record| record.released.is_some())
            .count()
    }
}

/// Where a followed service's stream stood when its client went: enough to carry the stream on at the same
/// offsets when the client comes back.
struct FollowRecord {
    base: u64,
    origin: u64,
    identity: Option<i32>,
    /// The stream offset after the last byte sent.
    next: u64,
    released: Instant,
}

/// The holder's output read so far and not yet handed to the stream.
struct Flow {
    buffer: VecDeque<Vec<u8>>,
    buffered: usize,
    /// The holder offset right after the last buffered byte.
    end: u64,
    /// The stream offset of holder offset `origin`: a followed service's stream runs on across its restarts.
    base: u64,
    origin: u64,
    /// The reading connection while attached; `None` while detached for lack of credit.
    reader: Option<HolderConnection>,
    /// Bumped at every detach, so a reader that outlived its connection stops.
    generation: u64,
    holder_gone: bool,
    closing: Option<String>,
}

impl Flow {
    fn new() -> Flow {
        Flow {
            buffer: VecDeque::new(),
            buffered: 0,
            end: 0,
            base: 0,
            origin: 0,
            reader: None,
            generation: 0,
            holder_gone: false,
            closing: None,
        }
    }

    fn stream_offset(&self, holder_offset: u64) -> u64 {
        self.base + holder_offset.saturating_sub(self.origin)
    }
}

/// One holder bridged onto one stream of one connection.
pub struct Bridge {
    holder: String,
    dir: SocketDir,
    window: u64,
    flow: Mutex<Flow>,
    changed: Condvar,
    /// Input, sizes, primary and claim go through a connection of their own, kept while reading detaches;
    /// a followed service's output has none.
    writer: Mutex<Option<HolderConnection>>,
    last_resize: Mutex<u64>,
    events: Option<Arc<Publisher>>,
    /// The client's stream, which names this terminal in its `pty.exit`.
    channel: u32,
    /// The service whose output this follows, from holder to holder as it restarts.
    follow: Option<String>,
    /// The pid of the holder being read, which tells a restarted holder of the same name apart.
    identity: Mutex<Option<i32>>,
}

struct Resumed {
    start: u64,
    end: u64,
    data: Vec<u8>,
    connection: HolderConnection,
    output: BufReader<UnixStream>,
}

impl Bridge {
    fn new(
        holder: &str,
        dir: &SocketDir,
        window: u64,
        writer: Option<HolderConnection>,
        events: Option<Arc<Publisher>>,
        channel: u32,
        follow: Option<String>,
    ) -> Bridge {
        Bridge {
            holder: holder.to_string(),
            dir: dir.clone(),
            window,
            flow: Mutex::new(Flow::new()),
            changed: Condvar::new(),
            writer: Mutex::new(writer),
            last_resize: Mutex::new(0),
            events,
            channel,
            follow,
            identity: Mutex::new(dir.holder_pid(holder)),
        }
    }

    fn flow(&self) -> MutexGuard<'_, Flow> {
        locked(&self.flow)
    }

    fn writer(&self) -> io::Result<MutexGuard<'_, Option<HolderConnection>>> {
        let writer = locked(&self.writer);
        if writer.is_none() {
            return Err(io::Error::other("this stream only reads"));
        }
        Ok(writer)
    }

    /// The holder now behind the name is another than the one this read.
    fn replaced(&self) -> bool {
        let now = self.dir.holder_pid(&self.holder);
        now.is_some() && now != *locked(&self.identity)
    }

    /// Attaches a reader from `since`. A ring that no longer holds `since` replays from its start; replayed
    /// bytes the holder scrubbed (device queries) come back as NULs so offsets stay exact.
    fn open_reader(&self, since: u64, exact: bool) -> io::Result<Resumed> {
        let attached = pom_ptyhost::attach(&self.dir, &self.holder, since)?;
        let end = attached.end;
        let in_ring = since <= end && end - since <= RING;
        let length = attached.snapshot.len() as u64;
        let start = match (in_ring, exact) {
            (true, true) => since,
            _ => end.saturating_sub(RING).max(end.saturating_sub(length)),
        };
        let padding = (end - start).saturating_sub(length) as usize;
        let mut data = vec![0u8; padding];
        data.extend_from_slice(&attached.snapshot);
        Ok(Resumed {
            start,
            end,
            data,
            connection: attached.connection,
            output: attached.output,
        })
    }

    fn adopt(self: &Arc<Self>, resumed: Resumed) -> io::Result<()> {
        let generation = {
            let mut flow = self.flow();
            flow.buffered += resumed.data.len();
            if !resumed.data.is_empty() {
                flow.buffer.push_back(resumed.data);
            }
            flow.end = resumed.end;
            flow.generation += 1;
            flow.reader = Some(resumed.connection);
            flow.generation
        };
        self.changed.notify_all();
        let bridge = self.clone();
        let output = resumed.output;
        std::thread::Builder::new()
            .name("pom-server-pty-read".into())
            .spawn(move || bridge.read(output, generation))
            .map(|_| ())
    }

    fn detach_reader(flow: &mut Flow) {
        flow.generation += 1;
        if let Some(reader) = flow.reader.take() {
            if let Err(error) = reader.shutdown() {
                if error.kind() != io::ErrorKind::NotConnected {
                    eprintln!("pom server: detach from a holder: {error}");
                }
            }
        }
    }

    fn read(self: Arc<Self>, mut output: BufReader<UnixStream>, generation: u64) {
        if let Err(error) = output.get_ref().set_read_timeout(Some(END_RECHECK)) {
            eprintln!("pom server: read {}: {error}", self.holder);
        }
        let mut chunk = vec![0u8; MAX_DATA];
        loop {
            let read = output.read(&mut chunk);
            let mut flow = self.flow();
            if flow.generation != generation {
                return;
            }
            match read {
                Ok(0) => break,
                Ok(count) => {
                    flow.buffer
                        .push_back(chunk.get(..count).unwrap_or_default().to_vec());
                    flow.buffered += count;
                    flow.end += count as u64;
                    if flow.buffered as u64 >= self.window {
                        Self::detach_reader(&mut flow);
                    }
                    drop(flow);
                    self.changed.notify_all();
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
        // The command exited, or the holder stopped the output of a reader that fell behind: resuming tells which.
        let mut flow = self.flow();
        if flow.generation == generation {
            flow.reader = None;
        }
        drop(flow);
        self.changed.notify_all();
    }

    fn close(&self, reason: &str) {
        let mut flow = self.flow();
        if flow.closing.is_none() {
            flow.closing = Some(reason.to_string());
        }
        drop(flow);
        self.changed.notify_all();
    }

    fn shut_down(&self) {
        Self::detach_reader(&mut self.flow());
        if let Some(writer) = locked(&self.writer).as_ref() {
            if let Err(error) = writer.shutdown() {
                if error.kind() != io::ErrorKind::NotConnected {
                    eprintln!("pom server: close a holder writer: {error}");
                }
            }
        }
    }

    fn resize(&self, seq: u64, cols: u16, rows: u16) -> io::Result<()> {
        let mut last = locked(&self.last_resize);
        if seq < *last {
            return Ok(());
        }
        *last = seq;
        match self.writer()?.as_mut() {
            Some(writer) => writer.resize(cols, rows),
            None => Ok(()),
        }
    }

    fn exit(&self) {
        let crashed = self
            .dir
            .crash_info(&self.holder)
            .is_some_and(|info| info.crashed);
        if let Some(events) = &self.events {
            let exit = Exit {
                holder: self.holder.clone(),
                crashed,
                channel: self.channel,
            };
            if let Err(error) = events.publish(EXIT_TOPIC, exit) {
                eprintln!("pom server: pty.exit: {error}");
            }
        }
    }

    /// Hands buffered output to the stream as credit allows, resuming the holder once the app caught up.
    /// Returns why the stream ended and the offset its next byte would have had.
    fn pump(self: Arc<Self>, mut sender: StreamSender) -> (String, u64) {
        let half = self.window - self.window / 2;
        // Tracked here: once the connection is gone the sender no longer knows its offset.
        let mut next = sender.offset();
        loop {
            let mut flow = self.flow();
            while flow.closing.is_none() && flow.buffer.is_empty() && flow.reader.is_some() {
                flow = match self.changed.wait_timeout(flow, POLL) {
                    Ok((flow, _)) => flow,
                    Err(poisoned) => poisoned.into_inner().0,
                };
                // A terminal learns this from its input ending; a follower has no input.
                if sender.is_ended() {
                    flow.closing
                        .get_or_insert_with(|| "client gone".to_string());
                }
            }
            if let Some(reason) = flow.closing.clone() {
                drop(flow);
                sender.finish(&reason);
                return (reason, next);
            }
            if let Some(mut bytes) = flow.buffer.pop_front() {
                drop(flow);
                let written = match sender.try_write(&bytes) {
                    Ok(0) => {
                        if sender.wait_credit(1, POLL).is_err() {
                            self.close("client gone");
                        }
                        0
                    }
                    Ok(written) => written,
                    Err(_) => {
                        self.close("client gone");
                        0
                    }
                };
                next += written as u64;
                let mut flow = self.flow();
                flow.buffered -= written;
                if written < bytes.len() {
                    bytes.drain(..written);
                    flow.buffer.push_front(bytes);
                }
                continue;
            }
            if flow.holder_gone {
                drop(flow);
                if let Some(service) = &self.follow {
                    self.await_successor(&sender, service);
                    continue;
                }
                self.exit();
                sender.finish("exit");
                return ("exit".to_string(), next);
            }
            let from = flow.end;
            drop(flow);
            match sender.wait_credit(half, POLL) {
                Ok(credit) if credit >= half => {}
                Ok(_) => continue,
                Err(_) => {
                    self.close("client gone");
                    continue;
                }
            }
            match self.open_reader(from, true) {
                // A restarted service runs in a new holder of the same name.
                Ok(resumed) if resumed.start < from || self.replaced() => {
                    if let Err(error) = resumed.connection.shutdown() {
                        eprintln!("pom server: leave a replaced holder: {error}");
                    }
                    self.flow().holder_gone = true;
                }
                Ok(resumed) => {
                    let skipped = self.flow().stream_offset(resumed.start);
                    if resumed.start != from {
                        if sender.reset(skipped).is_err() {
                            self.close("client gone");
                            continue;
                        }
                        next = skipped;
                    }
                    if let Err(error) = self.adopt(resumed) {
                        eprintln!("pom server: resume {}: {error}", self.holder);
                        self.close("resume failed");
                    }
                }
                Err(_) => {
                    let gone = !self.dir.holder_alive(&self.holder);
                    if gone {
                        self.flow().holder_gone = true;
                    } else {
                        std::thread::sleep(POLL);
                    }
                }
            }
        }
    }

    /// Waits for the followed service to run again, then goes on reading its new holder after a line that
    /// says so.
    fn await_successor(self: &Arc<Self>, sender: &StreamSender, service: &str) {
        loop {
            if self.flow().closing.is_some() {
                return;
            }
            if sender.is_ended() {
                self.close("client gone");
                return;
            }
            let pid = self
                .dir
                .holder_pid(&self.holder)
                .filter(|_| self.dir.holder_alive(&self.holder));
            let before = *locked(&self.identity);
            if pid.is_some() && pid != before {
                if let Ok(resumed) = self.open_reader(0, false) {
                    *locked(&self.identity) = pid;
                    let verb = if before.is_some() {
                        "restarted"
                    } else {
                        "started"
                    };
                    let separator = format!("\r\n--- {service} {verb} ---\r\n").into_bytes();
                    {
                        let mut flow = self.flow();
                        flow.base = sender.offset() + separator.len() as u64;
                        flow.origin = resumed.start;
                        flow.holder_gone = false;
                        flow.buffered += separator.len();
                        flow.buffer.push_back(separator);
                    }
                    if let Err(error) = self.adopt(resumed) {
                        eprintln!("pom server: follow {}: {error}", self.holder);
                        self.close("resume failed");
                    }
                    return;
                }
            }
            let flow = self.flow();
            drop(
                self.changed
                    .wait_timeout(flow, POLL)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }
    }

    fn forward_input(
        self: Arc<Self>,
        input: InputReceiver,
        inputs: Arc<Inputs>,
        key: (String, String),
        mode: Mode,
    ) {
        while let Ok(chunk) = input.recv() {
            if mode == Mode::ReadOnly {
                continue;
            }
            let fresh = inputs.fresh(&key, chunk.offset, &chunk.bytes);
            if fresh.is_empty() {
                continue;
            }
            let written = self.writer().and_then(|mut writer| match writer.as_mut() {
                Some(writer) => writer.input(fresh),
                None => Ok(()),
            });
            if let Err(error) = written {
                eprintln!("pom server: input to {}: {error}", self.holder);
            }
        }
        self.close("detached");
    }
}

type Streams = Mutex<HashMap<(u64, u32), Arc<Bridge>>>;

/// Every bridged terminal of every connection, the input already typed, and the push channels for exits.
pub struct Ptys {
    holders: SocketDir,
    binary: PathBuf,
    shell: Mutex<ShellCommand>,
    streams: Arc<Streams>,
    inputs: Arc<Inputs>,
    follows: Arc<Follows>,
    retention: Mutex<Duration>,
    publishers: Arc<Publishers>,
    next_shell: AtomicU64,
}

type Follows = Mutex<HashMap<(String, String), FollowRecord>>;

type Publishers = Mutex<HashMap<(u64, u32), Arc<Publisher>>>;

/// Who asks: the connection, the client's id from its HELLO, and the connection itself.
pub struct Caller<'a> {
    pub connection: u64,
    pub client: &'a str,
    pub context: &'a RequestContext,
}

fn params<T: serde::de::DeserializeOwned + Default>(value: &Value) -> Result<T, RemoteError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value.clone()).map_err(|error| invalid(format!("bad params: {error}")))
}

fn to_value(value: impl serde::Serialize) -> Result<Value, RemoteError> {
    serde_json::to_value(value)
        .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))
}

impl Ptys {
    pub fn new(holders: SocketDir, binary: PathBuf) -> Ptys {
        Ptys {
            holders,
            binary,
            shell: Mutex::new(ShellCommand::login()),
            streams: Arc::new(Mutex::new(HashMap::new())),
            inputs: Arc::new(Inputs::default()),
            follows: Arc::new(Mutex::new(HashMap::new())),
            retention: Mutex::new(RETENTION),
            publishers: Arc::new(Mutex::new(HashMap::new())),
            next_shell: AtomicU64::new(1),
        }
    }

    pub fn set_shell(&self, shell: ShellCommand) {
        *locked(&self.shell) = shell;
    }

    pub fn streams(&self) -> usize {
        locked(&self.streams).len()
    }

    /// How long a vanished client's terminals and followed streams can be resumed.
    pub fn set_retention(&self, retention: Duration) {
        *locked(&self.retention) = retention;
    }

    /// Drops what vanished clients left longer ago than the retention; returns what is still kept.
    pub fn sweep(&self) -> usize {
        let retention = *locked(&self.retention);
        let inputs = self.inputs.sweep(retention);
        let mut follows = locked(&self.follows);
        follows.retain(|_, record| record.released.elapsed() < retention);
        inputs + follows.len()
    }

    pub fn log_follows(&self) -> usize {
        locked(&self.streams)
            .values()
            .filter(|bridge| bridge.follow.is_some())
            .count()
    }

    fn stream(&self, connection: u64, channel: u32) -> Result<Arc<Bridge>, RemoteError> {
        locked(&self.streams)
            .get(&(connection, channel))
            .cloned()
            .ok_or_else(|| invalid(format!("no terminal on channel {channel}")))
    }

    pub fn open(&self, source: &LocalSource, raw: &Value) -> Result<Value, RemoteError> {
        let params: OpenParams = params(raw)?;
        check_size(params.cols, params.rows)?;
        let workspace = source
            .workspace_terminals(&params.branch)
            .map_err(|error| failed(error.to_string()))?;
        let holder = match params.kind {
            Kind::Shell => self.open_shell(&workspace, &params)?,
            Kind::Service => {
                let service = params
                    .service
                    .as_deref()
                    .ok_or_else(|| invalid("a service terminal needs service"))?;
                crate::dispatch::check_name("service", service)?;
                let holder = source
                    .service_holder(&params.branch, service)
                    .map_err(|error| failed(error.to_string()))?;
                self.running(&holder, &format!("{service} is not running"))?
            }
            Kind::Agent => {
                let role = params
                    .role
                    .as_deref()
                    .ok_or_else(|| invalid("an agent terminal needs role"))?;
                crate::dispatch::check_name("agent role", role)?;
                let holder = source
                    .agents(&params.branch)
                    .map_err(|error| failed(error.to_string()))?
                    .into_iter()
                    .find(|agent| agent.role == role)
                    .map(|agent| agent.holder)
                    .ok_or_else(|| failed(format!("no agent {}/{role}", params.branch)))?;
                self.running(&holder, &format!("agent {role} is not running"))?
            }
        };
        check_holder(&workspace, &holder)?;
        to_value(Opened { holder })
    }

    fn running(&self, holder: &str, message: &str) -> Result<String, RemoteError> {
        if self.holders.holder_alive(holder) {
            Ok(holder.to_string())
        } else {
            Err(failed(message))
        }
    }

    fn open_shell(
        &self,
        workspace: &WorkspaceTerminals,
        params: &OpenParams,
    ) -> Result<String, RemoteError> {
        let cwd = match &params.cwd {
            Some(cwd) => check_cwd(&workspace.worktree, cwd)?,
            None => workspace.worktree.clone(),
        };
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let holder = format!(
            "{}-{millis}-{}",
            workspace.shell_prefix(),
            self.next_shell.fetch_add(1, Ordering::AcqRel)
        );
        let shell = locked(&self.shell).clone();
        let request = SpawnRequest {
            binary: &self.binary,
            name: &holder,
            cwd: &cwd,
            cols: params.cols,
            rows: params.rows,
            argv: &shell.argv,
            env: &shell.env,
            group: Some(&workspace.group),
        };
        pom_ptyhost::spawn_holder(&self.holders, &request)
            .map_err(|error| failed(format!("start a shell: {error}")))?;
        pom_ptyhost::wait_for_holder(&self.holders, &holder, HOLDER_START)
            .map_err(|error| failed(format!("the shell did not start: {error}")))?;
        Ok(holder)
    }

    fn publisher(
        &self,
        connection: &Connection,
        id: u64,
        channel: u32,
    ) -> Result<Arc<Publisher>, RemoteError> {
        let mut publishers = locked(&self.publishers);
        if let Some(publisher) = publishers.get(&(id, channel)) {
            return Ok(publisher.clone());
        }
        let publisher = Arc::new(
            connection
                .publisher(channel)
                .map_err(|error| invalid(format!("events: {error}")))?,
        );
        publishers.insert((id, channel), publisher.clone());
        Ok(publisher)
    }

    pub fn attach(
        &self,
        source: &LocalSource,
        caller: &Caller<'_>,
        raw: &Value,
    ) -> Result<Value, RemoteError> {
        let params: AttachParams = params(raw)?;
        check_size(params.cols, params.rows)?;
        check_text("terminal", &params.terminal, MAX_TERMINAL_ID)?;
        if let Some(claim) = &params.claim {
            check_text("claim", claim, MAX_CLAIM)?;
        }
        let workspace = source
            .workspace_terminals(&params.branch)
            .map_err(|error| failed(error.to_string()))?;
        check_holder(&workspace, &params.holder)?;
        self.running(&params.holder, &format!("{} is not running", params.holder))?;
        let connection = caller.context.connection();
        let events = params
            .events
            .map(|channel| self.publisher(connection, caller.connection, channel))
            .transpose()?;
        let unreachable = |error: io::Error| failed(format!("{}: {error}", params.holder));
        let mut writer =
            pom_ptyhost::connect_writer(&self.holders, &params.holder).map_err(unreachable)?;
        if let Some(claim) = &params.claim {
            writer.claim(claim).map_err(unreachable)?;
        }
        writer
            .resize(params.cols, params.rows)
            .map_err(unreachable)?;
        if params.primary {
            writer.primary().map_err(unreachable)?;
        }
        let bridge = Arc::new(Bridge::new(
            &params.holder,
            &self.holders,
            connection.config().window,
            Some(writer),
            events,
            params.channel,
            None,
        ));
        let resumed = bridge
            .open_reader(params.since, params.since > 0)
            .map_err(unreachable)?;
        let (start, end) = (resumed.start, resumed.end);
        let mut sender = match connection.attach_stream(params.channel, start) {
            Ok(sender) => sender,
            Err(error) => {
                bridge.shut_down();
                return Err(invalid(format!("channel {}: {error}", params.channel)));
            }
        };
        let input = sender.take_input();
        bridge.adopt(resumed).map_err(unreachable)?;
        let key = (
            format!("{}/{}", caller.client, params.terminal),
            params.holder.clone(),
        );
        self.sweep();
        let input_from = self.inputs.start(&key, params.input_from);
        let stream_key = (caller.connection, params.channel);
        locked(&self.streams).insert(stream_key, bridge.clone());
        if let Some(input) = input {
            let (bridge, inputs, key) = (bridge.clone(), self.inputs.clone(), key.clone());
            let mode = params.mode;
            std::thread::Builder::new()
                .name("pom-server-pty-input".into())
                .spawn(move || bridge.forward_input(input, inputs, key, mode))
                .map_err(|error| failed(error.to_string()))?;
        }
        let (streams, publishers) = (self.streams.clone(), self.publishers.clone());
        let inputs = self.inputs.clone();
        // A shared channel serves the connection's other terminals too, so it stays until the client goes.
        let events_key = params
            .events
            .filter(|_| !params.shared_events)
            .map(|channel| (caller.connection, channel));
        let pumped = bridge.clone();
        std::thread::Builder::new()
            .name("pom-server-pty-pump".into())
            .spawn(move || {
                pumped.clone().pump(sender);
                pumped.shut_down();
                inputs.release(&key);
                locked(&streams).remove(&stream_key);
                if let Some(events_key) = events_key {
                    locked(&publishers).remove(&events_key);
                }
            })
            .map_err(|error| failed(error.to_string()))?;
        to_value(Attached {
            holder: params.holder,
            start,
            end,
            input_from,
        })
    }

    /// Streams a service's output as it prints it, read-only, from holder to holder across restarts; a service
    /// that is not running starts the stream with what it left and the stream waits for it. A client back
    /// within the retention carries its stream on at the offsets it had.
    pub fn follow_logs(
        &self,
        source: &LocalSource,
        caller: &Caller<'_>,
        params: FollowLogsParams,
    ) -> Result<Value, RemoteError> {
        let workspace = source
            .workspace_terminals(&params.branch)
            .map_err(|error| failed(error.to_string()))?;
        let holder = source
            .service_holder(&params.branch, &params.service)
            .map_err(|error| failed(error.to_string()))?;
        check_holder(&workspace, &holder)?;
        let connection = caller.context.connection();
        let bridge = Arc::new(Bridge::new(
            &holder,
            &self.holders,
            connection.config().window,
            None,
            None,
            params.channel,
            Some(params.service.clone()),
        ));
        self.sweep();
        let follow_key = (caller.client.to_string(), holder.clone());
        let retained = locked(&self.follows).remove(&follow_key).filter(|record| {
            params.since > 0 && (record.base..=record.next).contains(&params.since)
        });
        let (start, end, running, resumed) = match retained {
            Some(record) => self.carry_on(&bridge, &record, params.since),
            None => self.start_follow(&bridge, &holder, params.since),
        };
        let sender = match connection.attach_stream(params.channel, start) {
            Ok(sender) => sender,
            Err(error) => {
                bridge.shut_down();
                return Err(invalid(format!("channel {}: {error}", params.channel)));
            }
        };
        if let Some(resumed) = resumed {
            bridge
                .adopt(resumed)
                .map_err(|error| failed(format!("{holder}: {error}")))?;
        }
        let stream_key = (caller.connection, params.channel);
        locked(&self.streams).insert(stream_key, bridge.clone());
        let (streams, follows) = (self.streams.clone(), self.follows.clone());
        std::thread::Builder::new()
            .name("pom-server-logs-pump".into())
            .spawn(move || {
                let (reason, next) = bridge.clone().pump(sender);
                bridge.shut_down();
                locked(&streams).remove(&stream_key);
                if reason == "client gone" {
                    let (base, origin) = {
                        let flow = bridge.flow();
                        (flow.base, flow.origin)
                    };
                    let record = FollowRecord {
                        base,
                        origin,
                        identity: *locked(&bridge.identity),
                        next,
                        released: Instant::now(),
                    };
                    locked(&follows).insert(follow_key, record);
                }
            })
            .map_err(|error| failed(error.to_string()))?;
        to_value(FollowedLogs {
            holder,
            start,
            end,
            running,
        })
    }

    /// A first follow: stream offsets are the holder's own.
    fn start_follow(
        &self,
        bridge: &Arc<Bridge>,
        holder: &str,
        since: u64,
    ) -> (u64, u64, bool, Option<Resumed>) {
        let resumed = self
            .holders
            .holder_alive(holder)
            .then(|| bridge.open_reader(since, since > 0).ok())
            .flatten();
        match resumed {
            Some(resumed) => (resumed.start, resumed.end, true, Some(resumed)),
            None => {
                let left = self
                    .holders
                    .crash_info(holder)
                    .map(|info| info.output)
                    .unwrap_or_default();
                let length = left.len() as u64;
                *locked(&bridge.identity) = None;
                let mut flow = bridge.flow();
                flow.base = length;
                flow.buffered = left.len();
                if !left.is_empty() {
                    flow.buffer.push_back(left);
                }
                flow.holder_gone = true;
                (0, length, false, None)
            }
        }
    }

    /// The stream a client left goes on from `since`: from the same holder when it still runs, else after a
    /// line about the restart, as if the client had never gone.
    fn carry_on(
        &self,
        bridge: &Arc<Bridge>,
        record: &FollowRecord,
        since: u64,
    ) -> (u64, u64, bool, Option<Resumed>) {
        {
            let mut flow = bridge.flow();
            flow.base = record.base;
            flow.origin = record.origin;
        }
        *locked(&bridge.identity) = record.identity;
        let running = self
            .holders
            .holder_pid(&bridge.holder)
            .filter(|_| self.holders.holder_alive(&bridge.holder));
        if running.is_some() && running == record.identity {
            let holder_offset = record.origin + (since - record.base);
            if let Ok(resumed) = bridge.open_reader(holder_offset, true) {
                let flow = bridge.flow();
                let (start, end) = (
                    flow.stream_offset(resumed.start),
                    flow.stream_offset(resumed.end),
                );
                drop(flow);
                return (start, end, true, Some(resumed));
            }
        }
        bridge.flow().holder_gone = true;
        (since, since, running.is_some(), None)
    }

    pub fn resize(&self, caller: &Caller<'_>, raw: &Value) -> Result<Value, RemoteError> {
        let params: ResizeParams = params(raw)?;
        check_size(params.cols, params.rows)?;
        self.stream(caller.connection, params.channel)?
            .resize(params.seq, params.cols, params.rows)
            .map_err(|error| failed(error.to_string()))?;
        Ok(Value::Null)
    }

    pub fn primary(&self, caller: &Caller<'_>, raw: &Value) -> Result<Value, RemoteError> {
        let params: ChannelParams = params(raw)?;
        let bridge = self.stream(caller.connection, params.channel)?;
        bridge
            .writer()
            .and_then(|mut writer| match writer.as_mut() {
                Some(writer) => writer.primary(),
                None => Ok(()),
            })
            .map_err(|error| failed(error.to_string()))?;
        Ok(Value::Null)
    }

    /// Ends the stream; the holder and its command keep running.
    pub fn detach(&self, caller: &Caller<'_>, raw: &Value) -> Result<Value, RemoteError> {
        let params: ChannelParams = params(raw)?;
        self.stream(caller.connection, params.channel)?
            .close("detached");
        Ok(Value::Null)
    }

    pub fn close(&self, source: &LocalSource, raw: &Value) -> Result<Value, RemoteError> {
        let params: CloseParams = params(raw)?;
        let workspace = source
            .workspace_terminals(&params.branch)
            .map_err(|error| failed(error.to_string()))?;
        check_holder(&workspace, &params.holder)?;
        match self.holders.kill_holder(&params.holder) {
            Ok(()) => Ok(Value::Null),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Value::Null),
            Err(error) => Err(failed(error.to_string())),
        }
    }

    /// A client went away: its streams end and their holders keep running.
    pub fn disconnect(&self, connection: u64) {
        let bridges: Vec<Arc<Bridge>> = locked(&self.streams)
            .iter()
            .filter(|((owner, _), _)| *owner == connection)
            .map(|(_, bridge)| bridge.clone())
            .collect();
        for bridge in bridges {
            bridge.close("client gone");
        }
        locked(&self.publishers).retain(|(owner, _), _| *owner != connection);
    }
}
