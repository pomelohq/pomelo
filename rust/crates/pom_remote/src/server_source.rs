//! A project on another machine read through the `pom server` there: one ssh session carries every request.
//! A host whose pom has no server, or speaks another protocol, is read the H5 way instead ([`RemoteSource`]).
//! A session that drops is reopened with backoff under the same client id: the same server lets every stream
//! carry on where it was; a restarted one is told apart by its server id.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pom_agent::SessionView;
use pom_rpc::pty::{self, LinkState, PtyProvider, ResumeError, SharedEvents};
use pom_rpc::{
    error_code, CloseReason, Connection, Error, Hello, PushEvent, StreamEvent, StreamReceiver,
    Subscription,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::server_api::{
    method, topic, ActParams, AgentAnswerParams, AgentHolderParams, AgentListing, AgentStartParams,
    AgentStarted, FollowLogsParams, FollowedLogs, JobFollowParams, JobParams, JobRequest,
    JobStartParams, JobStarted, LogsParams, Progress, ProjectParams, ServerInfo, TurnParams,
    WatchBranchParams, WatchParams, WorkspaceParams, AGENTS, FILES_READ, FILES_WRITE, JOBS,
    JOB_TASK, LIVE_LOGS, PROGRESS_TOPIC, PUSH, REQUIRED,
};
use crate::server_api::{
    ServiceChoiceParams, ServiceRunParams, ServicesFacts, SharedLogsParams, SharedRunParams,
};
use crate::server_transport::{self, ProxyFailure, ProxyProcess, SERVER_GONE};
use crate::source::{last_turn, ACT_TIMEOUT, READ_TIMEOUT};
use crate::update::{Release, UpdateStep};
use crate::{
    version_mismatch, AgentTurn, Feed, FeedEvent, Host, HostTransport, JobEvent, JobFeed, LogEvent,
    LogFeed, ProjectSource, RemoteSource, ServiceAction, SourceError, Ssh, StatusReport,
    TransportInfo, Update, Via, WorkspaceList,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// ssh may take its own connect timeout, then the proxy waits for a server to start.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_WAIT: Duration = Duration::from_secs(2);
/// The waits between reconnect attempts; past the last one the source counts as disconnected.
pub const BACKOFF: [Duration; 6] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(15),
];
/// How long an update waits for the new server to answer after the old one stopped.
const UPDATE_RECONNECT: Duration = Duration::from_secs(60);
/// A caller that finds the session down waits this long for the reconnect under way.
const LINK_WAIT: Duration = Duration::from_secs(5);
/// How often the supervisor looks up from a wait to notice a retry, a wake from sleep or its end.
const SUPERVISOR_TICK: Duration = Duration::from_millis(100);
/// A wall clock that ran this much further than the monotonic one means the machine slept.
const SLEPT: Duration = Duration::from_secs(5);

struct Link {
    connection: Connection,
    process: Option<ProxyProcess>,
    /// The server takes every terminal's events on one channel.
    shares_events: bool,
    events: Option<Arc<SharedEvents>>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.connection.close();
    }
}

enum LinkSlot {
    /// Never opened yet: the next caller opens it.
    Closed,
    Open(Link),
    /// The host cannot serve; every read goes over plain ssh from now on.
    Fallback,
}

enum Route<T> {
    Server(T),
    Fallback,
}

pub struct ServerSource {
    inner: Arc<Inner>,
}

/// Opens a byte pipe to the host's server; `true` asks to reconnect to a running one only, and an error saying
/// [`SERVER_GONE`] means none runs.
pub type Opener = Arc<dyn Fn(bool) -> std::io::Result<pom_rpc::Transport> + Send + Sync>;

struct Status {
    link: LinkState,
    /// Times the session came back to another server than the one it left.
    restarts: u64,
    supervised: bool,
}

pub(crate) struct Inner {
    host: Host,
    config: String,
    ssh: Ssh,
    opener: Option<Opener>,
    fallback: RemoteSource,
    link: Mutex<LinkSlot>,
    info: Mutex<Option<TransportInfo>>,
    server_id: Mutex<String>,
    status: Mutex<Status>,
    changed: Condvar,
    kick: AtomicBool,
    backoff: Mutex<Vec<Duration>>,
    client_id: String,
    next_op: AtomicU64,
    next_terminal: AtomicU64,
    unconfirmed_input_limit: AtomicUsize,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What reads `config` on `host`: its server, unless hosts.json picks plain ssh. Its terminals keep up to
/// `unconfirmed_input_limit` bytes of typed input to send again after a drop.
pub fn project_source(
    host: Host,
    config: String,
    ssh: Ssh,
    unconfirmed_input_limit: usize,
) -> Arc<dyn ProjectSource> {
    match host.transport {
        Some(HostTransport::Ssh) => Arc::new(RemoteSource::new(host, config, ssh)),
        Some(HostTransport::Server) | None => {
            let source = ServerSource::new(host, config, ssh);
            source.set_unconfirmed_input_limit(unconfirmed_input_limit);
            Arc::new(source)
        }
    }
}

impl ServerSource {
    pub fn new(host: Host, config: String, ssh: Ssh) -> ServerSource {
        Self::build(host, config, ssh, None)
    }

    pub fn with_opener(host: Host, config: String, opener: Opener) -> ServerSource {
        Self::build(host, config, Ssh::system(), Some(opener))
    }

    fn build(host: Host, config: String, ssh: Ssh, opener: Option<Opener>) -> ServerSource {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        ServerSource {
            inner: Arc::new(Inner {
                fallback: RemoteSource::new(host.clone(), config.clone(), ssh.clone()),
                host,
                config,
                ssh,
                opener,
                link: Mutex::new(LinkSlot::Closed),
                info: Mutex::new(None),
                server_id: Mutex::new(String::new()),
                status: Mutex::new(Status {
                    link: LinkState::Connected,
                    restarts: 0,
                    supervised: false,
                }),
                changed: Condvar::new(),
                kick: AtomicBool::new(false),
                backoff: Mutex::new(BACKOFF.to_vec()),
                client_id: format!("{}-{nanos:x}", std::process::id()),
                next_op: AtomicU64::new(1),
                next_terminal: AtomicU64::new(1),
                unconfirmed_input_limit: AtomicUsize::new(pty::UNCONFIRMED_INPUT_LIMIT),
            }),
        }
    }

    pub fn set_unconfirmed_input_limit(&self, bytes: usize) {
        self.inner
            .unconfirmed_input_limit
            .store(bytes, Ordering::Release);
    }

    /// The waits between reconnect attempts, shorter in tests.
    pub fn set_backoff(&self, steps: Vec<Duration>) {
        *locked(&self.inner.backoff) = steps;
    }

    pub fn terminals(&self, branch: &str) -> Arc<dyn PtyProvider> {
        Arc::new(Terminals {
            inner: self.inner.clone(),
            branch: branch.to_string(),
        })
    }

    /// The id the server knows this app by, the same across reconnects.
    pub fn client_id(&self) -> &str {
        &self.inner.client_id
    }

    /// The process id of the ssh session carrying the connection now, when it runs one.
    pub fn session_pid(&self) -> Option<u32> {
        match &*locked(&self.inner.link) {
            LinkSlot::Open(open) => open.process.as_ref().map(ProxyProcess::pid),
            _ => None,
        }
    }

    /// The id of the server the session reaches now; empty before the first connect.
    pub fn server_id(&self) -> String {
        locked(&self.inner.server_id).clone()
    }
}

/// Why opening a session failed.
enum Failure {
    /// `--reconnect` found no server.
    ServerGone,
    /// The host cannot serve; read it over plain ssh.
    Fallback(String),
    Source(SourceError),
}

struct Dialed {
    connection: Connection,
    server: Hello,
    process: Option<ProxyProcess>,
}

/// A wait of `base` give or take a fifth, so many apps that lost one host do not all come back at once.
fn jittered(base: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();
    let spread = f64::from(nanos % 1000) / 1000.0;
    base.mul_f64(0.8 + 0.4 * spread)
}

impl Inner {
    fn set_link_state(&self, link: LinkState) {
        let mut status = locked(&self.status);
        if status.link != link {
            status.link = link;
        }
        drop(status);
        self.changed.notify_all();
    }

    fn link_state(&self) -> LinkState {
        locked(&self.status).link.clone()
    }

    fn fall_back(&self, link: &mut LinkSlot, reason: String) {
        *link = LinkSlot::Fallback;
        *locked(&self.info) = Some(TransportInfo {
            via: Via::Ssh,
            fallback_reason: Some(reason),
            ..TransportInfo::default()
        });
        self.set_link_state(LinkState::Connected);
    }

    /// Opens a session and says hello; `reconnect` only reaches a server that already runs.
    fn dial(&self, reconnect: bool) -> Result<Dialed, Failure> {
        let name = &self.host.name;
        let opened = match &self.opener {
            Some(open) => open(reconnect).map(|transport| (transport, None)),
            None => server_transport::open(&self.ssh, &self.host, reconnect)
                .map(|(transport, process)| (transport, Some(process))),
        };
        let (transport, process) = opened.map_err(|error| {
            if error.to_string() == SERVER_GONE {
                return Failure::ServerGone;
            }
            Failure::Source(SourceError::Unreachable(match error.kind() {
                std::io::ErrorKind::NotFound => "ssh is not installed".to_string(),
                _ => error.to_string(),
            }))
        })?;
        let mut hello = Hello::new(VERSION);
        hello.client_id = Some(self.client_id.clone());
        let config = pom_rpc::Config {
            handshake_timeout: HANDSHAKE_TIMEOUT,
            ..pom_rpc::Config::default()
        };
        match Connection::builder(transport).config(config).connect(hello) {
            Ok((connection, server)) => {
                if let Some(missing) = REQUIRED
                    .iter()
                    .find(|method| !server.features.iter().any(|feature| feature == *method))
                {
                    connection.close();
                    return Err(Failure::Fallback(format!(
                        "pom server on {name} lacks {missing}; update pom on the VM to get terminals"
                    )));
                }
                Ok(Dialed {
                    connection,
                    server,
                    process,
                })
            }
            Err(Error::Closed(CloseReason::Peer(message)))
                if message.starts_with(error_code::PROTOCOL_MISMATCH) =>
            {
                Err(Failure::Fallback(format!(
                    "pom server on {name} speaks another protocol; update pom on the VM to get terminals"
                )))
            }
            Err(error) => Err(
                match process.as_ref().map(|process| process.failure(EXIT_WAIT)) {
                    None => Failure::Source(SourceError::Unreachable(format!(
                        "pom server on {name} did not answer: {error}"
                    ))),
                    Some(ProxyFailure::ServerGone) => Failure::ServerGone,
                    Some(ProxyFailure::NoServer(reason)) => {
                        Failure::Fallback(format!("pom on {name}: {reason}"))
                    }
                    Some(ProxyFailure::Unreachable(reason)) => {
                        Failure::Source(SourceError::Unreachable(reason))
                    }
                    Some(ProxyFailure::NotInstalled) => {
                        Failure::Source(SourceError::Failed(format!(
                            "pom is not installed on {name}; check it with: pom host test {name}"
                        )))
                    }
                    Some(ProxyFailure::Other(reason)) => {
                        Failure::Source(SourceError::Unreachable(format!(
                            "pom server on {name} did not answer ({error}): {reason}"
                        )))
                    }
                },
            ),
        }
    }

    /// Makes `dialed` the session: what the server offers, and whether it is the server the last one reached.
    fn install(&self, link: &mut LinkSlot, dialed: Dialed) -> Connection {
        let Dialed {
            connection,
            server,
            process,
        } = dialed;
        let offers = |wanted: &[&str]| {
            wanted
                .iter()
                .all(|method| server.features.iter().any(|feature| feature == method))
        };
        let shares_events = offers(&[pty::SHARED_EVENTS]);
        *locked(&self.info) = Some(TransportInfo {
            via: Via::Server,
            fallback_reason: None,
            version_mismatch: version_mismatch(VERSION, &server.pom_version),
            remote_version: Some(server.pom_version.clone()),
            terminals: offers(pom_rpc::pty::METHODS),
            pushes: offers(PUSH),
            agents: offers(AGENTS),
            live_logs: offers(LIVE_LOGS),
            jobs: offers(JOBS),
            files: offers(&[FILES_READ]),
            services: crate::ServiceOffers {
                facts: offers(&[method::SVC_FACTS]),
                run: offers(&[method::SVC_RUN]),
                shared: offers(&[method::SVC_SHARED]),
                mode: offers(&[method::SVC_MODE]),
                env_profile: offers(&[method::SVC_ENV_PROFILE]),
                shared_logs: offers(&[method::SVC_SHARED_LOGS]),
                tasks: offers(&[JOB_TASK]),
            },
            files_write: offers(&[FILES_WRITE]),
        });
        let server_id = server.server_id.unwrap_or_default();
        let previous = std::mem::replace(&mut *locked(&self.server_id), server_id.clone());
        {
            let mut status = locked(&self.status);
            if !previous.is_empty() && previous != server_id {
                status.restarts += 1;
            }
            status.link = LinkState::Connected;
        }
        self.changed.notify_all();
        *link = LinkSlot::Open(Link {
            connection: connection.clone(),
            process,
            shares_events,
            events: None,
        });
        connection
    }

    pub(crate) fn host_name(&self) -> &str {
        &self.host.name
    }

    /// A fresh op id: asking again with it after a reconnect gets the first run's outcome.
    pub(crate) fn next_op_id(&self) -> String {
        format!(
            "{}-{}",
            self.client_id,
            self.next_op.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// The project's pom.yml on the host.
    pub(crate) fn config(&self) -> &str {
        &self.config
    }

    /// The live session, without opening one.
    pub(crate) fn live_connection(&self) -> Option<Connection> {
        match &*locked(&self.link) {
            LinkSlot::Open(open) if open.connection.state() == pom_rpc::ConnectionState::Open => {
                Some(open.connection.clone())
            }
            _ => None,
        }
    }

    /// The open connection, opening the first one; `None` once the host fell back to plain ssh. A session that
    /// dropped is the supervisor's to reopen, so this waits a little for it rather than dialing too.
    pub(crate) fn connection(self: &Arc<Self>) -> Result<Option<Connection>, SourceError> {
        let mut link = locked(&self.link);
        match &*link {
            LinkSlot::Fallback => return Ok(None),
            LinkSlot::Open(open) if open.connection.state() == pom_rpc::ConnectionState::Open => {
                return Ok(Some(open.connection.clone()))
            }
            LinkSlot::Open(_) => {
                drop(link);
                return self.wait_for_link(LINK_WAIT);
            }
            LinkSlot::Closed => {}
        }
        match self.dial(false) {
            Ok(dialed) => {
                let connection = self.install(&mut link, dialed);
                drop(link);
                self.supervise();
                Ok(Some(connection))
            }
            Err(Failure::Fallback(reason)) => {
                self.fall_back(&mut link, reason);
                Ok(None)
            }
            Err(Failure::ServerGone) => Err(SourceError::Unreachable(format!(
                "pom server on {} is gone",
                self.host.name
            ))),
            Err(Failure::Source(error)) => Err(error),
        }
    }

    fn wait_for_link(&self, timeout: Duration) -> Result<Option<Connection>, SourceError> {
        let deadline = Instant::now() + timeout;
        loop {
            if matches!(*locked(&self.link), LinkSlot::Fallback) {
                return Ok(None);
            }
            if let Some(connection) = self.live_connection() {
                return Ok(Some(connection));
            }
            let status = locked(&self.status);
            let reason = match &status.link {
                LinkState::Disconnected { reason } => {
                    return Err(SourceError::Unreachable(reason.clone()))
                }
                _ => format!("reconnecting to {}", self.host.name),
            };
            let now = Instant::now();
            if now >= deadline {
                return Err(SourceError::Unreachable(reason));
            }
            let wait = (deadline - now).min(SUPERVISOR_TICK);
            drop(
                self.changed
                    .wait_timeout(status, wait)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }
    }

    /// Starts the thread that reopens the session whenever it drops, once.
    fn supervise(self: &Arc<Self>) {
        {
            let mut status = locked(&self.status);
            if status.supervised {
                return;
            }
            status.supervised = true;
        }
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("pom-remote-reconnect".into())
            .spawn(move || supervise(&weak));
        if let Err(error) = spawned {
            eprintln!("pom server session: no reconnect thread: {error}");
            locked(&self.status).supervised = false;
        }
    }

    /// Ends the session; the supervisor opens a new one.
    fn forget(&self) {
        if let LinkSlot::Open(open) = &*locked(&self.link) {
            open.connection.close();
        }
    }

    /// Tries to reconnect now: at a retry, a window coming to the front, a wake from sleep.
    fn kick(&self) {
        if self.link_state() != LinkState::Connected {
            self.kick.store(true, Ordering::Release);
        }
        self.changed.notify_all();
    }

    /// Waits `delay`, cut short by a kick or by a wake from sleep; false once nothing else holds the source.
    fn wait_or_kick(self: &Arc<Self>, delay: Option<Duration>) -> bool {
        let started = (Instant::now(), SystemTime::now());
        let mut status = locked(&self.status);
        loop {
            if Arc::strong_count(self) <= 1 {
                return false;
            }
            if self.kick.swap(false, Ordering::AcqRel) {
                return true;
            }
            let monotonic = started.0.elapsed();
            let wall = started.1.elapsed().unwrap_or_default();
            if wall > monotonic + SLEPT {
                return true;
            }
            if delay.is_some_and(|delay| monotonic >= delay) {
                return true;
            }
            status = self
                .changed
                .wait_timeout(status, SUPERVISOR_TICK)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
    }

    /// One reconnect: to the same server when it still runs, else to a fresh one.
    fn redial(&self, link: &mut LinkSlot) -> Result<(), Failure> {
        let dialed = match self.dial(true) {
            Err(Failure::ServerGone) => self.dial(false)?,
            other => other?,
        };
        self.install(link, dialed);
        Ok(())
    }
}

/// Reopens the session each time it drops: attempts spaced by the backoff, then disconnected until kicked.
fn supervise(weak: &Weak<Inner>) {
    loop {
        loop {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let connection = match &*locked(&inner.link) {
                LinkSlot::Open(open) => open.connection.clone(),
                LinkSlot::Fallback => return,
                LinkSlot::Closed => break,
            };
            drop(inner);
            if connection.wait_closed(SUPERVISOR_TICK).is_some() {
                break;
            }
        }
        let mut attempt = 0usize;
        let mut reason = String::new();
        loop {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let steps = locked(&inner.backoff).clone();
            let delay = match steps.get(attempt) {
                Some(step) => jittered(*step),
                None => {
                    inner.set_link_state(LinkState::Disconnected {
                        reason: reason.clone(),
                    });
                    if !inner.wait_or_kick(None) {
                        return;
                    }
                    attempt = 0;
                    Duration::ZERO
                }
            };
            inner.set_link_state(LinkState::Reconnecting {
                attempt: u32::try_from(attempt + 1).unwrap_or(u32::MAX),
                next_in: delay,
            });
            if !inner.wait_or_kick(Some(delay)) {
                return;
            }
            let mut link = locked(&inner.link);
            match inner.redial(&mut link) {
                Ok(()) => break,
                Err(Failure::Fallback(why)) => {
                    inner.fall_back(&mut link, why);
                    return;
                }
                Err(Failure::ServerGone) => reason = "pom server is gone".to_string(),
                Err(Failure::Source(error)) => reason = error.to_string(),
            }
            attempt += 1;
        }
    }
}

impl Inner {
    /// One request; a connection that dropped is waited for and the request asked once more.
    fn request<T: DeserializeOwned>(
        self: &Arc<Self>,
        name: &str,
        params: impl Serialize,
        timeout: Duration,
    ) -> Result<Route<T>, SourceError> {
        let params =
            serde_json::to_value(params).map_err(|error| SourceError::Failed(error.to_string()))?;
        let mut retried = false;
        loop {
            let Some(connection) = self.connection()? else {
                return Ok(Route::Fallback);
            };
            let answer = connection
                .start_request(name, &params)
                .and_then(|mut pending| pending.wait_timeout(timeout));
            match answer {
                Ok(value) => {
                    return serde_json::from_value(value)
                        .map(Route::Server)
                        .map_err(|error| {
                            SourceError::Failed(format!(
                                "{} answered {name} with something pom here cannot read ({error})",
                                self.host.name
                            ))
                        })
                }
                Err(Error::Remote(error)) => return Err(SourceError::Failed(error.message)),
                Err(Error::Closed(_) | Error::Io(_)) if !retried => {
                    retried = true;
                    self.forget();
                }
                Err(Error::Timeout) => {
                    self.forget();
                    return Err(SourceError::Unreachable(format!(
                        "no answer from {} after {}s",
                        self.host.name,
                        timeout.as_secs()
                    )));
                }
                Err(error) => {
                    self.forget();
                    return Err(SourceError::Unreachable(error.to_string()));
                }
            }
        }
    }

    /// The connection's shared events channel, opened on first use; `None` from a server without one.
    fn shared_events(&self, connection: &Connection) -> Result<Option<Arc<SharedEvents>>, Error> {
        let mut link = locked(&self.link);
        let LinkSlot::Open(open) = &mut *link else {
            return Ok(None);
        };
        if !open.shares_events || !open.connection.ptr_eq(connection) {
            return Ok(None);
        }
        if let Some(events) = &open.events {
            return Ok(Some(events.clone()));
        }
        let events = Arc::new(SharedEvents::open(connection)?);
        open.events = Some(events.clone());
        Ok(Some(events))
    }

    fn info(&self) -> Option<TransportInfo> {
        locked(&self.info).clone()
    }

    fn server_only<T>(&self, route: Route<T>, what: &str) -> Result<T, SourceError> {
        match route {
            Route::Server(value) => Ok(value),
            Route::Fallback => Err(SourceError::Failed(format!(
                "{what} needs pom server on {}; it is read over plain ssh",
                self.host.name
            ))),
        }
    }

    /// The open connection when the server offers `wanted`.
    fn offering(
        self: &Arc<Self>,
        wanted: impl Fn(&TransportInfo) -> bool,
    ) -> Result<Option<Connection>, SourceError> {
        let Some(connection) = self.connection()? else {
            return Ok(None);
        };
        Ok(self
            .info()
            .as_ref()
            .is_some_and(wanted)
            .then_some(connection))
    }

    fn stopped_answering(&self, error: Error) -> SourceError {
        match error {
            Error::Remote(error) => SourceError::Failed(error.message),
            other => {
                self.forget();
                SourceError::Unreachable(format!("{} stopped answering: {other}", self.host.name))
            }
        }
    }

    fn op_id(&self) -> String {
        format!(
            "{}-{}",
            self.client_id,
            self.next_op.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Fails with what needs a newer pom when the server does not offer it.
    fn offered(
        self: &Arc<Self>,
        wanted: impl Fn(&crate::ServiceOffers) -> bool,
        what: &str,
    ) -> Result<(), SourceError> {
        if self.connection()?.is_none() {
            return Err(SourceError::Failed(format!(
                "{what} needs pom server on {}; it is read over plain ssh",
                self.host.name
            )));
        }
        if self.info().is_some_and(|info| wanted(&info.services)) {
            return Ok(());
        }
        Err(SourceError::Failed(format!(
            "{what} needs a newer pom on {}; update it there",
            self.host.name
        )))
    }

    fn choose(
        self: &Arc<Self>,
        name: &str,
        branch: &str,
        repo: &str,
        service: &str,
        value: &str,
    ) -> Result<(), SourceError> {
        let params = ServiceChoiceParams {
            cfg: self.config.clone(),
            branch: branch.to_string(),
            repo: repo.to_string(),
            service: service.to_string(),
            value: value.to_string(),
        };
        let route = self.request::<serde_json::Value>(name, params, READ_TIMEOUT)?;
        self.server_only(route, "Changing a service").map(|_| ())
    }

    fn workspace(&self, branch: &str) -> WorkspaceParams {
        WorkspaceParams {
            cfg: self.config.clone(),
            branch: branch.to_string(),
        }
    }

    fn follow(
        &self,
        connection: &Connection,
        branch: &str,
        service: &str,
        since: u64,
    ) -> Result<(StreamReceiver, FollowedLogs), Error> {
        let stream = connection.open_stream()?;
        let params = FollowLogsParams {
            cfg: self.config.clone(),
            branch: branch.to_string(),
            service: service.to_string(),
            since,
            channel: stream.channel(),
        };
        let value = connection
            .start_request(method::LOGS_FOLLOW, params)
            .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT))?;
        let followed = serde_json::from_value(value)
            .map_err(|error| Error::Protocol(format!("logs.follow answer: {error}")))?;
        Ok((stream, followed))
    }
}

impl ProjectSource for ServerSource {
    fn location(&self) -> String {
        format!("{} ({})", self.inner.host.name, self.inner.host.url.alias())
    }

    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        let params = ProjectParams {
            cfg: self.inner.config.clone(),
        };
        match self.inner.request(method::WS_LIST, params, READ_TIMEOUT)? {
            Route::Server(list) => Ok(list),
            Route::Fallback => self.inner.fallback.workspaces(),
        }
    }

    fn status(&self, branch: &str) -> Result<StatusReport, SourceError> {
        match self.inner.request(
            method::WS_STATUS,
            self.inner.workspace(branch),
            READ_TIMEOUT,
        )? {
            Route::Server(report) => Ok(report),
            Route::Fallback => self.inner.fallback.status(branch),
        }
    }

    fn logs(&self, branch: &str, service: &str) -> Result<String, SourceError> {
        let params = LogsParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            service: service.to_string(),
        };
        match self.inner.request(method::SVC_LOGS, params, READ_TIMEOUT)? {
            Route::Server(text) => Ok(text),
            Route::Fallback => self.inner.fallback.logs(branch, service),
        }
    }

    fn agents(&self, branch: &str) -> Result<Vec<SessionView>, SourceError> {
        match self.inner.request::<AgentListing>(
            method::AGENT_LIST,
            self.inner.workspace(branch),
            READ_TIMEOUT,
        )? {
            Route::Server(listing) => Ok(listing.sessions),
            Route::Fallback => self.inner.fallback.agents(branch),
        }
    }

    fn agent_turn(&self, branch: &str, role: &str) -> Result<Option<AgentTurn>, SourceError> {
        let params = TurnParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            role: role.to_string(),
        };
        match self
            .inner
            .request::<serde_json::Value>(method::AGENT_TURN, params, READ_TIMEOUT)?
        {
            Route::Server(document) => Ok(last_turn(&document)),
            Route::Fallback => self.inner.fallback.agent_turn(branch, role),
        }
    }

    /// Asking again with the same op id after a reconnect gets the first run's answer, never a second run.
    fn act(
        &self,
        branch: &str,
        action: ServiceAction,
        target: &str,
    ) -> Result<String, SourceError> {
        let params = ActParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            action: action.verb().to_string(),
            target: target.to_string(),
            op_id: self.inner.op_id(),
        };
        match self.inner.request(method::SVC_ACT, params, ACT_TIMEOUT)? {
            Route::Server(said) => Ok(said),
            Route::Fallback => self.inner.fallback.act(branch, action, target),
        }
    }

    fn transport(&self) -> Option<TransportInfo> {
        self.inner.info()
    }

    fn link(&self) -> Option<crate::LinkReport> {
        let status = locked(&self.inner.status);
        status.supervised.then(|| crate::LinkReport {
            state: status.link.clone(),
            restarts: status.restarts,
        })
    }

    fn reconnect_now(&self) {
        self.inner.kick();
    }

    /// Installs the new pom first, then stops the old server, so the reconnect that follows starts the new one.
    fn update_pom(&self, release: &Release, progress: &dyn Fn(UpdateStep)) -> Result<(), String> {
        let inner = &self.inner;
        crate::update::install_pom(&inner.ssh, &inner.host, release, progress)?;
        progress(UpdateStep::Restart);
        let old = inner.live_connection();
        if let Some(connection) = &old {
            let stopped = connection
                .start_request(method::SERVER_STOP, serde_json::Value::Null)
                .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT));
            match stopped {
                Ok(_) | Err(Error::Closed(_)) => {}
                Err(error) => return Err(format!("stopping the old pom server: {error}")),
            }
        }
        inner.kick();
        let deadline = Instant::now() + UPDATE_RECONNECT;
        loop {
            let running = inner
                .live_connection()
                .filter(|now| old.as_ref().is_none_or(|old| !old.ptr_eq(now)))
                .and_then(|_| inner.info())
                .and_then(|info| info.remote_version);
            if running.as_deref() == Some(release.version.as_str()) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "pom {} is installed on {}, but its server answers with {}",
                    release.version,
                    inner.host.name,
                    running.unwrap_or_else(|| "nothing yet".to_string())
                ));
            }
            inner.kick();
            std::thread::sleep(SUPERVISOR_TICK);
        }
    }

    fn files(&self, branch: &str) -> Option<Arc<dyn files::ProjectFs>> {
        let info = self.inner.info()?;
        (info.via == Via::Server && info.files).then(|| {
            Arc::new(crate::remote_fs::RemoteFs::new(self.inner.clone(), branch))
                as Arc<dyn files::ProjectFs>
        })
    }

    fn dev_proxy_port(&self) -> Option<u16> {
        match self.inner.request::<ServerInfo>(
            method::SERVER_INFO,
            serde_json::Value::Null,
            READ_TIMEOUT,
        ) {
            Ok(Route::Server(info)) => Some(info.proxy_port).filter(|port| *port > 0),
            Ok(Route::Fallback) => None,
            Err(error) => {
                eprintln!("{}: server.info: {error}", self.inner.host.name);
                None
            }
        }
    }

    fn pty(&self, branch: &str) -> Option<Arc<dyn PtyProvider>> {
        let info = self.inner.info()?;
        (info.via == Via::Server && info.terminals).then(|| self.terminals(branch))
    }

    fn events(
        &self,
        branch: &str,
        turn: Option<&str>,
    ) -> Result<Option<Arc<dyn Feed>>, SourceError> {
        let Some(connection) = self.inner.connection()? else {
            return Ok(None);
        };
        if !self.inner.info().is_some_and(|info| info.pushes) {
            return Ok(None);
        }
        let unreachable = |error: Error| {
            SourceError::Unreachable(format!(
                "{} stopped answering: {error}",
                self.inner.host.name
            ))
        };
        let topics = if self.inner.info().is_some_and(|info| info.services.facts) {
            topic::ALL
        } else {
            topic::BEFORE_SERVICES
        };
        let subscription = connection.subscribe().map_err(unreachable)?;
        let params = WatchParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            topics: topics.iter().map(|name| name.to_string()).collect(),
            channel: subscription.channel(),
            turn: turn.map(str::to_string),
        };
        connection
            .start_request(method::WATCH_SUBSCRIBE, params)
            .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT))
            .map_err(|error| match error {
                Error::Remote(error) => SourceError::Failed(error.message),
                other => unreachable(other),
            })?;
        Ok(Some(Arc::new(ServerFeed {
            inner: self.inner.clone(),
            connection,
            channel: subscription.channel(),
            subscription: Mutex::new(subscription),
        })))
    }

    fn follow_logs(
        &self,
        branch: &str,
        service: &str,
    ) -> Result<Option<Box<dyn LogFeed>>, SourceError> {
        let Some(connection) = self.inner.offering(|info| info.live_logs)? else {
            return Ok(None);
        };
        let (stream, followed) = self
            .inner
            .follow(&connection, branch, service, 0)
            .map_err(|error| self.inner.stopped_answering(error))?;
        Ok(Some(Box::new(ServerLogs {
            inner: self.inner.clone(),
            branch: branch.to_string(),
            service: service.to_string(),
            stream: Mutex::new(Some(stream)),
            next: AtomicU64::new(followed.start),
        })))
    }

    fn start_job(&self, request: &JobRequest, op_id: &str) -> Result<String, SourceError> {
        if self.inner.offering(|info| info.jobs)?.is_none() {
            return Err(SourceError::Failed(format!(
                "pom on {} cannot run this; update pom there",
                self.inner.host.name
            )));
        }
        let params = JobStartParams {
            cfg: self.inner.config.clone(),
            op: request.op().to_string(),
            args: request.args(),
            op_id: op_id.to_string(),
        };
        let route = self
            .inner
            .request::<JobStarted>(method::JOB_START, params, READ_TIMEOUT)?;
        self.inner
            .server_only(route, "Running a workspace job")
            .map(|started| started.id)
    }

    fn follow_job(&self, id: &str, since_seq: u64) -> Result<Box<dyn JobFeed>, SourceError> {
        let connection = self.inner.offering(|info| info.jobs)?.ok_or_else(|| {
            SourceError::Failed(format!(
                "pom on {} cannot run this; update pom there",
                self.inner.host.name
            ))
        })?;
        let subscription = connection
            .subscribe()
            .map_err(|error| self.inner.stopped_answering(error))?;
        let params = JobFollowParams {
            id: id.to_string(),
            since_seq,
            channel: subscription.channel(),
        };
        connection
            .start_request(method::JOB_FOLLOW, params)
            .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT))
            .map_err(|error| self.inner.stopped_answering(error))?;
        Ok(Box::new(ServerJob {
            subscription: Mutex::new(subscription),
        }))
    }

    fn cancel_job(&self, id: &str) -> Result<(), SourceError> {
        let params = JobParams { id: id.to_string() };
        let route =
            self.inner
                .request::<serde_json::Value>(method::JOB_CANCEL, params, READ_TIMEOUT)?;
        self.inner
            .server_only(route, "Cancelling a workspace job")
            .map(|_| ())
    }

    fn services_facts(&self, branch: &str) -> Result<ServicesFacts, SourceError> {
        self.inner
            .offered(|offers| offers.facts, "The Services panel")?;
        let route = self.inner.request::<ServicesFacts>(
            method::SVC_FACTS,
            self.inner.workspace(branch),
            READ_TIMEOUT,
        )?;
        self.inner.server_only(route, "The Services panel")
    }

    /// Asking again with the same op id after a reconnect gets the first run's answer, never a second run.
    fn run_service(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        verb: crate::ServiceVerb,
    ) -> Result<(), SourceError> {
        self.inner
            .offered(|offers| offers.run, "Running a service")?;
        let params = ServiceRunParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            repo: repo.to_string(),
            service: service.to_string(),
            action: verb.verb().to_string(),
            op_id: self.inner.op_id(),
        };
        let route =
            self.inner
                .request::<serde_json::Value>(method::SVC_RUN, params, ACT_TIMEOUT)?;
        self.inner
            .server_only(route, "Running a service")
            .map(|_| ())
    }

    fn run_shared(&self, name: &str, verb: crate::ServiceVerb) -> Result<(), SourceError> {
        self.inner
            .offered(|offers| offers.shared, "Running a shared service")?;
        let params = SharedRunParams {
            cfg: self.inner.config.clone(),
            name: name.to_string(),
            action: verb.verb().to_string(),
            op_id: self.inner.op_id(),
        };
        let route =
            self.inner
                .request::<serde_json::Value>(method::SVC_SHARED, params, ACT_TIMEOUT)?;
        self.inner
            .server_only(route, "Running a shared service")
            .map(|_| ())
    }

    fn set_service_mode(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        mode: &str,
    ) -> Result<(), SourceError> {
        self.inner
            .offered(|offers| offers.mode, "Changing a service's mode")?;
        self.inner
            .choose(method::SVC_MODE, branch, repo, service, mode)
    }

    fn set_env_profile(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        profile: &str,
    ) -> Result<(), SourceError> {
        self.inner.offered(
            |offers| offers.env_profile,
            "Changing a service's env profile",
        )?;
        self.inner
            .choose(method::SVC_ENV_PROFILE, branch, repo, service, profile)
    }

    fn shared_logs(&self, name: &str, lines: usize) -> Result<String, SourceError> {
        self.inner
            .offered(|offers| offers.shared_logs, "A shared service's output")?;
        let params = SharedLogsParams {
            cfg: self.inner.config.clone(),
            name: name.to_string(),
            lines,
        };
        let route = self
            .inner
            .request::<String>(method::SVC_SHARED_LOGS, params, READ_TIMEOUT)?;
        self.inner.server_only(route, "A shared service's output")
    }

    fn start_agent(&self, branch: &str, role: &str) -> Result<SessionView, SourceError> {
        let params = AgentStartParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            role: role.to_string(),
            person: true,
        };
        let route = self
            .inner
            .request::<AgentStarted>(method::AGENT_START, params, ACT_TIMEOUT)?;
        self.inner
            .server_only(route, "Starting an agent")
            .map(|started| started.session)
    }

    fn take_over_agent(&self, branch: &str, holder: &str) -> Result<(), SourceError> {
        let params = AgentHolderParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            holder: holder.to_string(),
        };
        let route = self.inner.request::<serde_json::Value>(
            method::AGENT_TAKE_OVER,
            params,
            READ_TIMEOUT,
        )?;
        self.inner
            .server_only(route, "Taking an agent over")
            .map(|_| ())
    }

    fn answer_agent(
        &self,
        branch: &str,
        holder: &str,
        request: &str,
        allow: bool,
    ) -> Result<(), SourceError> {
        let params = AgentAnswerParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            holder: holder.to_string(),
            request: request.to_string(),
            allow,
        };
        let route =
            self.inner
                .request::<serde_json::Value>(method::AGENT_ANSWER, params, READ_TIMEOUT)?;
        self.inner
            .server_only(route, "Answering an agent")
            .map(|_| ())
    }
}

/// A service's output streamed by the server, acknowledged as it is read. A stream that drops with its session
/// follows again from where it was once the session is back.
struct ServerLogs {
    inner: Arc<Inner>,
    branch: String,
    service: String,
    stream: Mutex<Option<StreamReceiver>>,
    /// The stream offset after the last byte read.
    next: AtomicU64,
}

/// How long `next` waits before it asks again while the session is down.
const OFFLINE_WAIT: Duration = Duration::from_millis(250);

impl ServerLogs {
    fn follow_again(&self, stream: &mut Option<StreamReceiver>, timeout: Duration) -> LogEvent {
        let Some(connection) = self.inner.live_connection() else {
            std::thread::sleep(timeout.min(OFFLINE_WAIT));
            return LogEvent::Idle;
        };
        let since = self.next.load(Ordering::Acquire);
        match self
            .inner
            .follow(&connection, &self.branch, &self.service, since)
        {
            Ok((receiver, followed)) => {
                *stream = Some(receiver);
                self.next.store(followed.start, Ordering::Release);
                match followed.start.cmp(&since) {
                    std::cmp::Ordering::Less => LogEvent::StartedOver,
                    std::cmp::Ordering::Equal => LogEvent::Idle,
                    std::cmp::Ordering::Greater => LogEvent::Skipped,
                }
            }
            Err(Error::Remote(error)) => LogEvent::Ended(error.message),
            Err(_) => {
                std::thread::sleep(timeout.min(OFFLINE_WAIT));
                LogEvent::Idle
            }
        }
    }
}

impl LogFeed for ServerLogs {
    fn next(&self, timeout: Duration) -> LogEvent {
        let mut stream = locked(&self.stream);
        let Some(receiver) = stream.as_ref() else {
            return self.follow_again(&mut stream, timeout);
        };
        match receiver.recv_timeout(timeout) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                let end = offset + bytes.len() as u64;
                receiver.ack(end);
                self.next.store(end, Ordering::Release);
                LogEvent::Data(bytes)
            }
            Ok(StreamEvent::Reset { offset }) => {
                self.next.store(offset, Ordering::Release);
                LogEvent::Skipped
            }
            Err(Error::Timeout) => LogEvent::Idle,
            Ok(StreamEvent::End { .. }) | Err(_) => {
                *stream = None;
                LogEvent::Idle
            }
        }
    }
}

/// A job's progress pushed by the server.
struct ServerJob {
    subscription: Mutex<Subscription>,
}

impl JobFeed for ServerJob {
    fn next(&self, timeout: Duration) -> JobEvent {
        match locked(&self.subscription).recv_timeout(timeout) {
            Ok(PushEvent::Value { topic, value }) if topic == PROGRESS_TOPIC => {
                match serde_json::from_value::<Progress>(value) {
                    Ok(progress) => JobEvent::Progress(progress),
                    Err(error) => JobEvent::Ended(format!("unreadable progress: {error}")),
                }
            }
            Ok(PushEvent::Value { .. }) | Err(Error::Timeout) => JobEvent::Idle,
            Ok(PushEvent::End { reason }) => JobEvent::Ended(reason),
            Err(error) => JobEvent::Ended(error.to_string()),
        }
    }
}

/// The values one watch pushes, read off its subscription.
struct ServerFeed {
    inner: Arc<Inner>,
    connection: Connection,
    /// Kept apart from the subscription, which its reader holds while it waits.
    channel: u32,
    subscription: Mutex<Subscription>,
}

impl ServerFeed {
    fn update(name: &str, value: serde_json::Value) -> Option<Update> {
        match name {
            topic::WORKSPACES => serde_json::from_value(value).ok().map(Update::Workspaces),
            topic::STATUS => serde_json::from_value(value).ok().map(Update::Status),
            topic::AGENTS => serde_json::from_value(value).ok().map(Update::Agents),
            topic::TURN => serde_json::from_value(value).ok().map(Update::Turn),
            topic::SERVICES => serde_json::from_value(value).ok().map(Update::Services),
            _ => None,
        }
    }
}

impl Feed for ServerFeed {
    fn next(&self, timeout: Duration) -> FeedEvent {
        match locked(&self.subscription).recv_timeout(timeout) {
            Ok(PushEvent::Value { topic, value }) => match Self::update(&topic, value) {
                Some(update) => FeedEvent::Update(update),
                None => FeedEvent::Idle,
            },
            Ok(PushEvent::End { reason }) => FeedEvent::Ended(reason),
            Err(Error::Timeout) => FeedEvent::Idle,
            Err(error) => FeedEvent::Ended(error.to_string()),
        }
    }

    fn follow(&self, branch: &str, turn: Option<&str>) -> Result<(), SourceError> {
        let params = WatchBranchParams {
            channel: self.channel,
            branch: branch.to_string(),
            turn: turn.map(str::to_string),
        };
        self.connection
            .start_request(method::WATCH_SET_BRANCH, params)
            .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT))
            .map(|_| ())
            .map_err(|error| match error {
                Error::Remote(error) => SourceError::Failed(error.message),
                other => SourceError::Unreachable(format!(
                    "{} stopped answering: {other}",
                    self.inner.host.name
                )),
            })
    }
}

struct Terminals {
    inner: Arc<Inner>,
    branch: String,
}

const TERMINAL_TIMEOUT: Duration = Duration::from_secs(30);

impl Terminals {
    fn connection(&self) -> Result<Connection, String> {
        let connection = self
            .inner
            .connection()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "{} is read over plain ssh, which has no terminals",
                    self.inner.host.name
                )
            })?;
        let supported = locked(&self.inner.info)
            .as_ref()
            .is_some_and(|info| info.terminals);
        if !supported {
            return Err(format!(
                "pom server on {} has no terminals; update pom on the VM to get them",
                self.inner.host.name
            ));
        }
        Ok(connection)
    }

    /// Attaches on `connection`, the terminal tagged with the server it went through.
    fn attach_on(
        &self,
        connection: &Connection,
        params: pty::AttachParams,
    ) -> Result<pty::Terminal, Error> {
        let server = locked(&self.inner.server_id).clone();
        let mut terminal = match self.inner.shared_events(connection)? {
            Some(events) => {
                pty::Terminal::attach_shared(connection, &events, params, TERMINAL_TIMEOUT)?
            }
            None => pty::Terminal::attach(connection, params, TERMINAL_TIMEOUT)?,
        };
        terminal.server = server;
        Ok(terminal)
    }

    fn ask<T: DeserializeOwned>(
        connection: &Connection,
        name: &str,
        params: impl Serialize,
    ) -> Result<T, String> {
        let value = connection
            .start_request(name, params)
            .and_then(|mut pending| pending.wait_timeout(TERMINAL_TIMEOUT))
            .map_err(|error| match error {
                Error::Remote(error) => error.message,
                other => other.to_string(),
            })?;
        serde_json::from_value(value).map_err(|error| format!("{name}: {error}"))
    }
}

impl PtyProvider for Terminals {
    fn unconfirmed_input_limit(&self) -> usize {
        self.inner.unconfirmed_input_limit.load(Ordering::Acquire)
    }

    fn attach(&self, request: &pty::Request) -> Result<pty::Terminal, String> {
        let connection = self.connection()?;
        let open = |kind, role, service, cwd| pty::OpenParams {
            cfg: self.inner.config.clone(),
            branch: self.branch.clone(),
            kind,
            role,
            service,
            cwd,
            cols: request.cols,
            rows: request.rows,
        };
        let holder = match &request.target {
            pty::Target::Holder(holder) => holder.clone(),
            pty::Target::Shell { cwd } => {
                let params = open(pty::Kind::Shell, None, None, cwd.clone());
                Self::ask::<pty::Opened>(&connection, pty::method::OPEN, params)?.holder
            }
            pty::Target::Service(service) => {
                let params = open(pty::Kind::Service, None, Some(service.clone()), None);
                Self::ask::<pty::Opened>(&connection, pty::method::OPEN, params)?.holder
            }
            pty::Target::Agent(role) => {
                let params = open(pty::Kind::Agent, Some(role.clone()), None, None);
                Self::ask::<pty::Opened>(&connection, pty::method::OPEN, params)?.holder
            }
        };
        let params = pty::AttachParams {
            cfg: self.inner.config.clone(),
            branch: self.branch.clone(),
            holder,
            terminal: self
                .inner
                .next_terminal
                .fetch_add(1, Ordering::Relaxed)
                .to_string(),
            mode: request.mode,
            primary: request.primary,
            claim: request.claim.clone(),
            cols: request.cols,
            rows: request.rows,
            ..pty::AttachParams::default()
        };
        self.attach_on(&connection, params)
            .map_err(|error| match error {
                Error::Remote(error) => error.message,
                other => other.to_string(),
            })
    }

    fn resume(&self, resume: &pty::Resume) -> Result<pty::Terminal, ResumeError> {
        let connection = self.inner.live_connection().ok_or_else(|| {
            ResumeError::Offline(format!("reconnecting to {}", self.inner.host.name))
        })?;
        let server = locked(&self.inner.server_id).clone();
        let restarted = !resume.server.is_empty() && resume.server != server;
        let params = pty::AttachParams {
            cfg: self.inner.config.clone(),
            branch: self.branch.clone(),
            holder: resume.holder.clone(),
            terminal: resume.terminal.clone(),
            since: resume.since,
            // A restarted server never wrote the unconfirmed input, and must never get it twice.
            input_from: if restarted { None } else { resume.input_from },
            mode: resume.mode,
            primary: resume.primary,
            cols: resume.cols,
            rows: resume.rows,
            ..pty::AttachParams::default()
        };
        self.attach_on(&connection, params)
            .map_err(|error| match error {
                Error::Remote(error) => ResumeError::Gone(error.message),
                other => ResumeError::Offline(other.to_string()),
            })
    }

    fn link(&self) -> LinkState {
        self.inner.link_state()
    }

    fn close(&self, holder: &str) -> Result<(), String> {
        let connection = self.connection()?;
        let params = pty::CloseParams {
            cfg: self.inner.config.clone(),
            branch: self.branch.clone(),
            holder: holder.to_string(),
        };
        Self::ask::<serde_json::Value>(&connection, pty::method::CLOSE, params).map(|_| ())
    }
}
