//! One server per (user, version): a Unix socket in a private dir, a lock so a second copy refuses to start,
//! and a loop that exits when idle or when its socket file is gone.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pom_remote::server_api::{method, ServerInfo, FEATURES, METHODS, SERVER_SCHEMA};
use pom_rpc::{Config, Connection, Hello, RemoteError, Request, RequestContext, Transport};
use serde_json::Value;

use crate::dispatch::Dispatcher;
use crate::log::{RequestRecord, ServerLog, ROTATE_AT};
use crate::paths::ServerPaths;
use crate::peer;
use crate::pty::Caller;
use pom_paths::ensure_private_dir;

pub const DEFAULT_IDLE_EXIT_MINUTES: u64 = 30;
pub const SOCKET_CHECK_EVERY: Duration = Duration::from_secs(30);
/// A client behind ssh can stall for a while (a laptop lid, a slow link) before it counts as gone.
pub const DEAD_AFTER: Duration = Duration::from_secs(60);

/// Reads the user id at the other end of an accepted connection; tests swap it to fake another user.
pub type PeerCheck = Arc<dyn Fn(&UnixStream) -> io::Result<u32> + Send + Sync>;

pub struct ServerOptions {
    pub paths: ServerPaths,
    pub version: String,
    /// `None` never exits for lack of clients.
    pub idle_exit: Option<Duration>,
    pub socket_check_every: Duration,
    pub dead_after: Duration,
    pub peer_uid: PeerCheck,
    pub log_rotate_at: u64,
    pub dispatcher: Dispatcher,
}

impl ServerOptions {
    pub fn new(paths: ServerPaths, version: &str, dispatcher: Dispatcher) -> ServerOptions {
        ServerOptions {
            paths,
            version: version.to_string(),
            idle_exit: Some(Duration::from_secs(DEFAULT_IDLE_EXIT_MINUTES * 60)),
            socket_check_every: SOCKET_CHECK_EVERY,
            dead_after: DEAD_AFTER,
            peer_uid: Arc::new(peer::peer_uid),
            log_rotate_at: ROTATE_AT,
            dispatcher,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    /// Another server of this version holds the lock; carries its pid when the pid file says.
    AlreadyRunning(Option<u32>),
    Io(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::AlreadyRunning(Some(pid)) => {
                write!(
                    formatter,
                    "a pom server of this version already runs (pid {pid})"
                )
            }
            StartError::AlreadyRunning(None) => {
                write!(formatter, "a pom server of this version already runs")
            }
            StartError::Io(message) => formatter.write_str(message),
        }
    }
}

fn io_error(what: &str, path: &std::path::Path, error: io::Error) -> StartError {
    StartError::Io(format!("{what} {}: {error}", path.display()))
}

struct Shared {
    paths: ServerPaths,
    version: String,
    server_id: String,
    started: Instant,
    dead_after: Duration,
    peer_uid: PeerCheck,
    uid: u32,
    log: ServerLog,
    dispatcher: Dispatcher,
    stop: AtomicBool,
    connections: AtomicUsize,
    next_connection: AtomicU64,
    idle_since: Mutex<Instant>,
    live: Mutex<HashMap<u64, Connection>>,
}

impl Shared {
    fn live(&self) -> MutexGuard<'_, HashMap<u64, Connection>> {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn idle_since(&self) -> MutexGuard<'_, Instant> {
        self.idle_since
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn info(&self) -> ServerInfo {
        ServerInfo {
            schema: SERVER_SCHEMA.to_string(),
            pid: std::process::id(),
            version: self.version.clone(),
            uptime_seconds: self.started.elapsed().as_secs(),
            connections: self.connections.load(Ordering::Acquire),
            streams: self.dispatcher.ptys().streams(),
            log_follows: self.dispatcher.ptys().log_follows(),
            jobs: self.dispatcher.jobs().running(),
            retained: self.dispatcher.ptys().sweep(),
            file_watches: self.dispatcher.files().watched(),
            socket: self.paths.socket.clone(),
            log: self.log.path().to_path_buf(),
            proxy_port: pom_env::dev_ports().proxy,
        }
    }

    fn answer(&self, request: &Request, caller: &Caller<'_>) -> Result<Value, RemoteError> {
        if let Some(answer) = self.dispatcher.dispatch_pty(request, caller) {
            return answer;
        }
        if let Some(answer) = self.dispatcher.dispatch_watch(request, caller) {
            return answer;
        }
        if let Some(answer) = self.dispatcher.dispatch_jobs(request, caller) {
            return answer;
        }
        if let Some(answer) = self.dispatcher.dispatch_fs(request, caller) {
            return answer;
        }
        match request.method.as_str() {
            method::SERVER_INFO => serde_json::to_value(self.info()).map_err(|error| {
                RemoteError::new(pom_rpc::error_code::INTERNAL, error.to_string())
            }),
            method::SERVER_STOP => {
                self.stop.store(true, Ordering::Release);
                Ok(Value::Null)
            }
            _ => self.dispatcher.dispatch(request),
        }
    }

    fn handle(
        &self,
        connection: u64,
        client: &str,
        request: Request,
        context: &RequestContext,
    ) -> Result<Value, RemoteError> {
        let started = Instant::now();
        let bytes_in = serde_json::to_vec(&request.params)
            .map(|bytes| bytes.len())
            .unwrap_or_default();
        let caller = Caller {
            connection,
            client,
            context,
        };
        let result = self.answer(&request, &caller);
        let bytes_out = match &result {
            Ok(value) => serde_json::to_vec(value)
                .map(|bytes| bytes.len())
                .unwrap_or_default(),
            Err(error) => error.message.len(),
        };
        let known = METHODS
            .iter()
            .find(|name| **name == request.method)
            .copied()
            .unwrap_or("unknown");
        self.log.request(&RequestRecord {
            connection,
            method: known,
            id: context.id(),
            bytes_in,
            bytes_out,
            duration: started.elapsed(),
            result: match &result {
                Ok(_) => "ok",
                Err(error) => &error.code,
            },
        });
        result
    }

    fn serve(self: &Arc<Self>, stream: UnixStream, id: u64) {
        match (self.peer_uid)(&stream) {
            Ok(uid) if uid == self.uid => {}
            Ok(uid) => {
                self.log.line(&format!("conn={id} refused: peer uid {uid}"));
                return;
            }
            Err(error) => {
                self.log
                    .line(&format!("conn={id} refused: peer unknown ({error})"));
                return;
            }
        }
        let transport = match Transport::unix(stream) {
            Ok(transport) => transport,
            Err(error) => {
                self.log.line(&format!("conn={id} failed: {error}"));
                return;
            }
        };
        let client_id = Arc::new(Mutex::new(String::new()));
        let handler = {
            let (shared, client_id) = (self.clone(), client_id.clone());
            move |request: Request, context: &RequestContext| {
                let client = locked(&client_id).clone();
                shared.handle(id, &client, request, context)
            }
        };
        let config = Config {
            dead_after: self.dead_after,
            ..Config::default()
        };
        let (version, server_id) = (self.version.clone(), self.server_id.clone());
        let accepted = Connection::builder(transport)
            .config(config)
            .handler(handler)
            .accept(move |client| {
                *locked(&client_id) = sanitized(client.client_id.as_deref().unwrap_or_default());
                let mut hello = Hello::new(version);
                hello.server_id = Some(server_id);
                hello.client_id = client.client_id.clone();
                hello.features = METHODS
                    .iter()
                    .chain(FEATURES)
                    .map(|name| name.to_string())
                    .collect();
                Ok(hello)
            });
        let connection = match accepted {
            Ok((connection, client)) => {
                self.log.line(&format!(
                    "conn={id} open client_version={}",
                    sanitized(&client.pom_version)
                ));
                connection
            }
            Err(error) => {
                self.log
                    .line(&format!("conn={id} handshake failed: {error}"));
                return;
            }
        };
        self.live().insert(id, connection.clone());
        let reason = loop {
            if let Some(reason) = connection.wait_closed(Duration::from_secs(3600)) {
                break reason;
            }
        };
        self.live().remove(&id);
        self.dispatcher.disconnect(id);
        self.log.line(&format!("conn={id} closed: {reason}"));
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A version string from a client goes into the log, so only its harmless characters do.
fn sanitized(text: &str) -> String {
    text.chars()
        .filter(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '-' | '+'))
        .take(32)
        .collect()
}

/// A handle on a running server, for tests and status.
#[derive(Clone)]
pub struct ServerHandle {
    shared: Arc<Shared>,
}

impl ServerHandle {
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }

    pub fn info(&self) -> ServerInfo {
        self.shared.info()
    }
}

pub struct Server {
    listener: UnixListener,
    _lock: File,
    socket_inode: u64,
    idle_exit: Option<Duration>,
    socket_check_every: Duration,
    shared: Arc<Shared>,
}

/// Takes the version's lock, then binds its socket (0600) and writes its pid.
pub fn bind(options: ServerOptions) -> Result<Server, StartError> {
    let paths = options.paths.clone();
    let uid = peer::current_uid();
    ensure_private_dir(&paths.dir, uid)
        .map_err(|error| io_error("socket dir", &paths.dir, error))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&paths.lock)
        .map_err(|error| io_error("lock", &paths.lock, error))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            let pid = std::fs::read_to_string(&paths.pid)
                .ok()
                .and_then(|text| text.trim().parse().ok());
            return Err(StartError::AlreadyRunning(pid));
        }
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(io_error("lock", &paths.lock, error))
        }
    }
    match std::fs::remove_file(&paths.socket) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("stale socket", &paths.socket, error)),
    }
    let listener = UnixListener::bind(&paths.socket)
        .map_err(|error| io_error("bind", &paths.socket, error))?;
    std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| io_error("socket mode", &paths.socket, error))?;
    let socket_inode = std::fs::metadata(&paths.socket)
        .map_err(|error| io_error("socket", &paths.socket, error))?
        .ino();
    pom_paths::write_atomic(
        &paths.pid,
        format!("{}\n", std::process::id()).as_bytes(),
        0o600,
    )
    .map_err(|error| io_error("pid file", &paths.pid, error))?;
    let log = ServerLog::open(&paths.log, options.log_rotate_at)
        .map_err(|error| io_error("log", &paths.log, error))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let shared = Arc::new(Shared {
        server_id: format!("{}-{nanos:x}", std::process::id()),
        paths,
        version: options.version,
        started: Instant::now(),
        dead_after: options.dead_after,
        peer_uid: options.peer_uid,
        uid,
        log,
        dispatcher: options.dispatcher,
        stop: AtomicBool::new(false),
        connections: AtomicUsize::new(0),
        next_connection: AtomicU64::new(1),
        idle_since: Mutex::new(Instant::now()),
        live: Mutex::new(HashMap::new()),
    });
    shared.log.line(&format!(
        "start pid={} version={}",
        std::process::id(),
        shared.version
    ));
    Ok(Server {
        listener,
        _lock: lock,
        socket_inode,
        idle_exit: options.idle_exit,
        socket_check_every: options.socket_check_every,
        shared,
    })
}

/// Whether the listener has a connection waiting, within `timeout`.
fn readable(listener: &UnixListener, timeout: Duration) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one valid pollfd for the duration of the call.
    let result = unsafe { libc::poll(&mut descriptor, 1, millis) };
    match result {
        -1 => {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                Ok(false)
            } else {
                Err(error)
            }
        }
        0 => Ok(false),
        _ => Ok(true),
    }
}

impl Server {
    pub fn handle(&self) -> ServerHandle {
        ServerHandle {
            shared: self.shared.clone(),
        }
    }

    fn tick(&self) -> Duration {
        let mut tick = Duration::from_millis(500).min(self.socket_check_every / 2);
        if let Some(idle) = self.idle_exit {
            tick = tick.min(idle / 4);
        }
        tick.max(Duration::from_millis(10))
    }

    fn socket_is_ours(&self) -> bool {
        std::fs::metadata(&self.shared.paths.socket)
            .is_ok_and(|metadata| metadata.ino() == self.socket_inode)
    }

    fn accept(&self) {
        let stream = match self.listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) => {
                self.shared.log.line(&format!("accept failed: {error}"));
                return;
            }
        };
        let id = self.shared.next_connection.fetch_add(1, Ordering::AcqRel);
        self.shared.connections.fetch_add(1, Ordering::AcqRel);
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("pom-server-conn-{id}"))
            .spawn(move || {
                shared.serve(stream, id);
                if shared.connections.fetch_sub(1, Ordering::AcqRel) == 1 {
                    *shared.idle_since() = Instant::now();
                }
            });
        if let Err(error) = spawned {
            self.shared.connections.fetch_sub(1, Ordering::AcqRel);
            self.shared
                .log
                .line(&format!("conn={id} no thread: {error}"));
        }
    }

    /// Serves until stopped, idle past `idle_exit`, or the socket file is removed or replaced.
    pub fn run(self) -> String {
        let tick = self.tick();
        let mut checked = Instant::now();
        let reason = loop {
            if self.shared.stop.load(Ordering::Acquire) {
                break "stopped".to_string();
            }
            if checked.elapsed() >= self.socket_check_every {
                checked = Instant::now();
                if !self.socket_is_ours() {
                    break "socket removed".to_string();
                }
            }
            if let Some(idle) = self.idle_exit {
                let idle_for = self.shared.idle_since().elapsed();
                if self.shared.connections.load(Ordering::Acquire) == 0 && idle_for >= idle {
                    break format!("idle for {}s", idle_for.as_secs());
                }
            }
            match readable(&self.listener, tick) {
                Ok(true) => self.accept(),
                Ok(false) => {}
                Err(error) => break format!("poll failed: {error}"),
            }
        };
        let live: Vec<Connection> = self.shared.live().values().cloned().collect();
        for connection in live {
            connection.close();
        }
        if self.socket_is_ours() {
            if let Err(error) = std::fs::remove_file(&self.shared.paths.socket) {
                self.shared.log.line(&format!("remove socket: {error}"));
            }
        }
        if let Err(error) = std::fs::remove_file(&self.shared.paths.pid) {
            self.shared.log.line(&format!("remove pid file: {error}"));
        }
        self.shared.log.line(&format!("exit: {reason}"));
        reason
    }
}
