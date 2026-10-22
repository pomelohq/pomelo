//! A project on another machine read through the `pom server` there: one ssh session carries every request.
//! A host whose pom has no server, or speaks another protocol, is read the H5 way instead ([`RemoteSource`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pom_agent::SessionView;
use pom_rpc::pty::{self, PtyProvider, SharedEvents};
use pom_rpc::{
    error_code, CloseReason, Connection, Error, Hello, PushEvent, StreamEvent, StreamReceiver,
    Subscription,
};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::server_api::{
    method, topic, ActParams, AgentAnswerParams, AgentHolderParams, AgentListing, AgentStartParams,
    AgentStarted, FollowLogsParams, JobFollowParams, JobParams, JobRequest, JobStartParams,
    JobStarted, LogsParams, Progress, ProjectParams, TurnParams, WatchBranchParams, WatchParams,
    WorkspaceParams, AGENTS, JOBS, LIVE_LOGS, PROGRESS_TOPIC, PUSH, REQUIRED,
};
use crate::server_transport::{self, ProxyFailure, ProxyProcess};
use crate::source::{last_turn, ACT_TIMEOUT, READ_TIMEOUT};
use crate::{
    version_mismatch, AgentTurn, Feed, FeedEvent, Host, HostTransport, JobEvent, JobFeed, LogEvent,
    LogFeed, ProjectSource, RemoteSource, ServiceAction, SourceError, Ssh, StatusReport,
    TransportInfo, Update, Via, WorkspaceList,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// ssh may take its own connect timeout, then the proxy waits for a server to start.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_WAIT: Duration = Duration::from_secs(2);

struct Link {
    connection: Connection,
    _process: Option<ProxyProcess>,
    /// The server takes every terminal's events on one channel.
    shares_events: bool,
    events: Option<Arc<SharedEvents>>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.connection.close();
    }
}

enum LinkState {
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

pub type Opener = Arc<dyn Fn() -> std::io::Result<pom_rpc::Transport> + Send + Sync>;

struct Inner {
    host: Host,
    config: String,
    ssh: Ssh,
    opener: Option<Opener>,
    fallback: RemoteSource,
    link: Mutex<LinkState>,
    info: Mutex<Option<TransportInfo>>,
    client_id: String,
    next_op: AtomicU64,
    next_terminal: AtomicU64,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What reads `config` on `host`: its server, unless hosts.json picks plain ssh.
pub fn project_source(host: Host, config: String, ssh: Ssh) -> Arc<dyn ProjectSource> {
    match host.transport {
        Some(HostTransport::Ssh) => Arc::new(RemoteSource::new(host, config, ssh)),
        Some(HostTransport::Server) | None => Arc::new(ServerSource::new(host, config, ssh)),
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
                link: Mutex::new(LinkState::Closed),
                info: Mutex::new(None),
                client_id: format!("{}-{nanos:x}", std::process::id()),
                next_op: AtomicU64::new(1),
                next_terminal: AtomicU64::new(1),
            }),
        }
    }

    pub fn terminals(&self, branch: &str) -> Arc<dyn PtyProvider> {
        Arc::new(Terminals {
            inner: self.inner.clone(),
            branch: branch.to_string(),
        })
    }
}

impl Inner {
    fn fall_back(&self, link: &mut LinkState, reason: String) {
        *link = LinkState::Fallback;
        *locked(&self.info) = Some(TransportInfo {
            via: Via::Ssh,
            fallback_reason: Some(reason),
            ..TransportInfo::default()
        });
    }

    /// The open connection, opening one if there is none; `None` once the host fell back to plain ssh.
    fn connection(&self) -> Result<Option<Connection>, SourceError> {
        let mut link = locked(&self.link);
        match &*link {
            LinkState::Fallback => return Ok(None),
            LinkState::Open(open) if open.connection.state() == pom_rpc::ConnectionState::Open => {
                return Ok(Some(open.connection.clone()))
            }
            _ => *link = LinkState::Closed,
        }
        let name = &self.host.name;
        let opened = match &self.opener {
            Some(open) => open().map(|transport| (transport, None)),
            None => server_transport::open(&self.ssh, &self.host)
                .map(|(transport, process)| (transport, Some(process))),
        };
        let (transport, process) = opened.map_err(|error| {
            SourceError::Unreachable(match error.kind() {
                std::io::ErrorKind::NotFound => "ssh is not installed".to_string(),
                _ => error.to_string(),
            })
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
                    let reason = format!(
                        "pom server on {name} lacks {missing}; update pom on the VM to get terminals"
                    );
                    self.fall_back(&mut link, reason);
                    return Ok(None);
                }
                let offers = |wanted: &[&str]| {
                    wanted
                        .iter()
                        .all(|method| server.features.iter().any(|feature| feature == method))
                };
                let terminals = offers(pom_rpc::pty::METHODS);
                let shares_events = offers(&[pty::SHARED_EVENTS]);
                let (pushes, agents) = (offers(PUSH), offers(AGENTS));
                let (live_logs, jobs) = (offers(LIVE_LOGS), offers(JOBS));
                *locked(&self.info) = Some(TransportInfo {
                    via: Via::Server,
                    fallback_reason: None,
                    version_mismatch: version_mismatch(VERSION, &server.pom_version),
                    remote_version: Some(server.pom_version),
                    terminals,
                    pushes,
                    agents,
                    live_logs,
                    jobs,
                });
                *link = LinkState::Open(Link {
                    connection: connection.clone(),
                    _process: process,
                    shares_events,
                    events: None,
                });
                Ok(Some(connection))
            }
            Err(Error::Closed(CloseReason::Peer(message)))
                if message.starts_with(error_code::PROTOCOL_MISMATCH) =>
            {
                let reason = format!(
                    "pom server on {name} speaks another protocol; update pom on the VM to get terminals"
                );
                self.fall_back(&mut link, reason);
                Ok(None)
            }
            Err(error) => match process.as_ref().map(|process| process.failure(EXIT_WAIT)) {
                None => Err(SourceError::Unreachable(format!(
                    "pom server on {name} did not answer: {error}"
                ))),
                Some(ProxyFailure::NoServer(reason)) => {
                    self.fall_back(&mut link, format!("pom on {name}: {reason}"));
                    Ok(None)
                }
                Some(ProxyFailure::Unreachable(reason)) => Err(SourceError::Unreachable(reason)),
                Some(ProxyFailure::NotInstalled) => Err(SourceError::Failed(format!(
                    "pom is not installed on {name}; check it with: pom host test {name}"
                ))),
                Some(ProxyFailure::Other(reason)) => Err(SourceError::Unreachable(format!(
                    "pom server on {name} did not answer ({error}): {reason}"
                ))),
            },
        }
    }

    fn forget(&self) {
        let mut link = locked(&self.link);
        if matches!(*link, LinkState::Open(_)) {
            *link = LinkState::Closed;
        }
    }

    /// One request; a connection that dropped is reopened from scratch and the request asked once more.
    fn request<T: DeserializeOwned>(
        &self,
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
        let LinkState::Open(open) = &mut *link else {
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
        &self,
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

    fn workspace(&self, branch: &str) -> WorkspaceParams {
        WorkspaceParams {
            cfg: self.config.clone(),
            branch: branch.to_string(),
        }
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
            op_id: format!(
                "{}-{}",
                self.inner.client_id,
                self.inner.next_op.fetch_add(1, Ordering::Relaxed)
            ),
        };
        match self.inner.request(method::SVC_ACT, params, ACT_TIMEOUT)? {
            Route::Server(said) => Ok(said),
            Route::Fallback => self.inner.fallback.act(branch, action, target),
        }
    }

    fn transport(&self) -> Option<TransportInfo> {
        self.inner.info()
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
        let subscription = connection.subscribe().map_err(unreachable)?;
        let params = WatchParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            topics: topic::ALL.iter().map(|name| name.to_string()).collect(),
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
        let stream = connection
            .open_stream()
            .map_err(|error| self.inner.stopped_answering(error))?;
        let params = FollowLogsParams {
            cfg: self.inner.config.clone(),
            branch: branch.to_string(),
            service: service.to_string(),
            since: 0,
            channel: stream.channel(),
        };
        connection
            .start_request(method::LOGS_FOLLOW, params)
            .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT))
            .map_err(|error| self.inner.stopped_answering(error))?;
        Ok(Some(Box::new(ServerLogs {
            stream: Mutex::new(stream),
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

/// A service's output streamed by the server, acknowledged as it is read.
struct ServerLogs {
    stream: Mutex<StreamReceiver>,
}

impl LogFeed for ServerLogs {
    fn next(&self, timeout: Duration) -> LogEvent {
        let stream = locked(&self.stream);
        match stream.recv_timeout(timeout) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                stream.ack(offset + bytes.len() as u64);
                LogEvent::Data(bytes)
            }
            Ok(StreamEvent::Reset { .. }) => LogEvent::Skipped,
            Ok(StreamEvent::End { reason }) => LogEvent::Ended(reason),
            Err(Error::Timeout) => LogEvent::Idle,
            Err(error) => LogEvent::Ended(error.to_string()),
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
        let shared = self
            .inner
            .shared_events(&connection)
            .map_err(|error| error.to_string())?;
        let attached = match shared {
            Some(events) => {
                pty::Terminal::attach_shared(&connection, &events, params, TERMINAL_TIMEOUT)
            }
            None => pty::Terminal::attach(&connection, params, TERMINAL_TIMEOUT),
        };
        attached.map_err(|error| match error {
            Error::Remote(error) => error.message,
            other => other.to_string(),
        })
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
