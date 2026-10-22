//! A project on another machine read through the `pom server` there: one ssh session carries every request.
//! A host whose pom has no server, or speaks another protocol, is read the H5 way instead ([`RemoteSource`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pom_agent::SessionView;
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
    host: Host,
    config: String,
    ssh: Ssh,
    fallback: RemoteSource,
    link: Mutex<LinkState>,
    info: Mutex<Option<TransportInfo>>,
    client_id: String,
    next_op: AtomicU64,
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
            fallback: RemoteSource::new(host.clone(), config.clone(), ssh.clone()),
            host,
            config,
            ssh,
            link: Mutex::new(LinkState::Closed),
            info: Mutex::new(None),
            client_id: format!("{}-{nanos:x}", std::process::id()),
            next_op: AtomicU64::new(1),
        }
    }

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
                *locked(&self.info) = Some(TransportInfo {
                    via: Via::Server,
                    fallback_reason: None,
                    version_mismatch: version_mismatch(VERSION, &server.pom_version),
                    remote_version: Some(server.pom_version),
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
        format!("{} ({})", self.host.name, self.host.url.alias())
    }

    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        let params = ProjectParams {
            cfg: self.config.clone(),
        };
        match self.request(method::WS_LIST, params, READ_TIMEOUT)? {
            Route::Server(list) => Ok(list),
            Route::Fallback => self.fallback.workspaces(),
        }
    }

    fn status(&self, branch: &str) -> Result<StatusReport, SourceError> {
        match self.request(method::WS_STATUS, self.workspace(branch), READ_TIMEOUT)? {
            Route::Server(report) => Ok(report),
            Route::Fallback => self.fallback.status(branch),
        }
    }

    fn logs(&self, branch: &str, service: &str) -> Result<String, SourceError> {
        let params = LogsParams {
            cfg: self.config.clone(),
            branch: branch.to_string(),
            service: service.to_string(),
        };
        match self.request(method::SVC_LOGS, params, READ_TIMEOUT)? {
            Route::Server(text) => Ok(text),
            Route::Fallback => self.fallback.logs(branch, service),
        }
    }

    fn agents(&self, branch: &str) -> Result<Vec<SessionView>, SourceError> {
        match self.request::<AgentListing>(
            method::AGENT_LIST,
            self.workspace(branch),
            READ_TIMEOUT,
        )? {
            Route::Server(listing) => Ok(listing.sessions),
            Route::Fallback => self.fallback.agents(branch),
        }
    }

    fn agent_turn(&self, branch: &str, role: &str) -> Result<Option<AgentTurn>, SourceError> {
        let params = TurnParams {
            cfg: self.config.clone(),
            branch: branch.to_string(),
            role: role.to_string(),
        };
        match self.request::<serde_json::Value>(method::AGENT_TURN, params, READ_TIMEOUT)? {
            Route::Server(document) => Ok(last_turn(&document)),
            Route::Fallback => self.fallback.agent_turn(branch, role),
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
            cfg: self.config.clone(),
            branch: branch.to_string(),
            action: action.verb().to_string(),
            target: target.to_string(),
            op_id: format!(
                "{}-{}",
                self.client_id,
                self.next_op.fetch_add(1, Ordering::Relaxed)
            ),
        };
        match self.request(method::SVC_ACT, params, ACT_TIMEOUT)? {
            Route::Server(said) => Ok(said),
            Route::Fallback => self.fallback.act(branch, action, target),
        }
    }

    fn transport(&self) -> Option<TransportInfo> {
        locked(&self.info).clone()
    }
}
