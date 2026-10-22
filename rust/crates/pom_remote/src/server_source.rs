//! A project on another machine read through the `pom server` there: one ssh session carries every request.
//! A host whose pom has no server, or speaks another protocol, is read the H5 way instead ([`RemoteSource`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pom_agent::SessionView;
use pom_rpc::pty::{self, PtyProvider};
use pom_rpc::{error_code, CloseReason, Connection, Error, Hello};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::server_api::{
    method, ActParams, AgentListing, LogsParams, ProjectParams, TurnParams, WorkspaceParams,
    REQUIRED,
};
use crate::server_transport::{self, ProxyFailure, ProxyProcess};
use crate::source::{last_turn, ACT_TIMEOUT, READ_TIMEOUT};
use crate::{
    version_mismatch, AgentTurn, Host, HostTransport, ProjectSource, RemoteSource, ServiceAction,
    SourceError, Ssh, StatusReport, TransportInfo, Via, WorkspaceList,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// ssh may take its own connect timeout, then the proxy waits for a server to start.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_WAIT: Duration = Duration::from_secs(2);

struct Link {
    connection: Connection,
    _process: ProxyProcess,
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

struct Inner {
    host: Host,
    config: String,
    ssh: Ssh,
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
        let (transport, process) =
            server_transport::open(&self.ssh, &self.host).map_err(|error| {
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
                let terminals = pom_rpc::pty::METHODS
                    .iter()
                    .all(|method| server.features.iter().any(|feature| feature == method));
                *locked(&self.info) = Some(TransportInfo {
                    via: Via::Server,
                    fallback_reason: None,
                    version_mismatch: version_mismatch(VERSION, &server.pom_version),
                    remote_version: Some(server.pom_version),
                    terminals,
                });
                *link = LinkState::Open(Link {
                    connection: connection.clone(),
                    _process: process,
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
            Err(error) => match process.failure(EXIT_WAIT) {
                ProxyFailure::NoServer(reason) => {
                    self.fall_back(&mut link, format!("pom on {name}: {reason}"));
                    Ok(None)
                }
                ProxyFailure::Unreachable(reason) => Err(SourceError::Unreachable(reason)),
                ProxyFailure::NotInstalled => Err(SourceError::Failed(format!(
                    "pom is not installed on {name}; check it with: pom host test {name}"
                ))),
                ProxyFailure::Other(reason) => Err(SourceError::Unreachable(format!(
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
        locked(&self.inner.info).clone()
    }

    fn pty(&self, branch: &str) -> Option<Arc<dyn PtyProvider>> {
        let info = locked(&self.inner.info).clone()?;
        (info.via == Via::Server && info.terminals).then(|| self.terminals(branch))
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
        pty::Terminal::attach(&connection, params, TERMINAL_TIMEOUT).map_err(|error| match error {
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
