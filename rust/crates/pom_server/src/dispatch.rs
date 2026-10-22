//! The read and action methods: each checks its parameters, then asks [`LocalSource`] exactly as the CLI's
//! `--json` commands do, so an answer has the same shape whichever way it is read.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_remote::server_api::{
    method, op_state, ActParams, AgentAnswerParams, AgentHolderParams, AgentListing,
    AgentStartParams, AgentStarted, LogsParams, OpParams, OpResult, ProjectParams, TurnParams,
    WatchBranchParams, WatchChannel, WatchParams, WorkspaceParams,
};
use pom_remote::{LocalSource, ProjectSource, ServiceAction, SourceError};
use pom_rpc::{error_code, RemoteError, Request};

use crate::pty::{Caller, Ptys};
use crate::watch::Watches;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// The code of an answer whose command ran and failed (no such workspace, a service that would not start).
pub const FAILED: &str = "failed";
pub const OP_MEMORY: Duration = Duration::from_secs(5 * 60);
const MAX_NAME: usize = 128;
const MAX_BRANCH: usize = 255;

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(error_code::INVALID_PARAMS, message)
}

fn failed(error: SourceError) -> RemoteError {
    RemoteError::new(FAILED, error.to_string())
}

fn to_value(value: impl Serialize) -> Result<Value, RemoteError> {
    serde_json::to_value(value)
        .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))
}

fn params<T: DeserializeOwned + Default>(value: &Value) -> Result<T, RemoteError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value.clone()).map_err(|error| invalid(format!("bad params: {error}")))
}

/// `cfg` must be the pom.yml of a project this user registered here, so a client can never point the
/// server at some other file.
pub fn registered_config(state: &StateDir, cfg: &str) -> Result<PathBuf, RemoteError> {
    let path = Path::new(cfg);
    if cfg.is_empty() || !path.is_absolute() {
        return Err(invalid(
            "cfg must be the absolute path of a project's pom.yml",
        ));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(invalid("cfg must not contain .."));
    }
    let wanted = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    pom_remote::local_projects(state)
        .into_iter()
        .map(|project| project.config)
        .find(|config| {
            config == path || std::fs::canonicalize(config).is_ok_and(|config| config == wanted)
        })
        .ok_or_else(|| {
            invalid(format!(
                "{cfg} is not a project pom knows on this machine (see pom project ls)"
            ))
        })
}

pub fn check_branch(branch: &str) -> Result<(), RemoteError> {
    if branch.len() > MAX_BRANCH {
        return Err(invalid("the branch name is too long"));
    }
    pom_workspace::validate_branch_name(branch).map_err(invalid)
}

/// A service, repo, group or agent role: `name` or `repo/name`.
pub fn check_name(what: &str, name: &str) -> Result<(), RemoteError> {
    let valid = name.len() <= MAX_NAME
        && name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && !name.contains("..")
        && name
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-' | '/'));
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("{name:?} is not a valid {what}")))
    }
}

/// A holder name or request id: letters, digits, '.', '_' or '-'.
fn check_word(what: &str, text: &str) -> Result<(), RemoteError> {
    let valid = !text.is_empty()
        && text.len() <= 255
        && text
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("{text:?} is not a valid {what}")))
    }
}

pub fn check_op_id(op_id: &str) -> Result<(), RemoteError> {
    let valid = !op_id.is_empty()
        && op_id.len() <= MAX_NAME
        && op_id
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(invalid("op_id must be letters, digits, '.', '_' or '-'"))
    }
}

enum Op {
    Running,
    Done {
        result: Result<String, String>,
        finished: Instant,
    },
}

/// Actions by op id, kept for [`OP_MEMORY`] after they finish so a client that lost its connection can
/// learn the outcome instead of running the action again.
pub struct Ops {
    entries: Mutex<HashMap<String, Op>>,
    finished: Condvar,
    memory: Duration,
}

impl Ops {
    pub fn new(memory: Duration) -> Ops {
        Ops {
            entries: Mutex::new(HashMap::new()),
            finished: Condvar::new(),
            memory,
        }
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, Op>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn prune(&self, entries: &mut HashMap<String, Op>) {
        entries.retain(|_, op| match op {
            Op::Running => true,
            Op::Done { finished, .. } => finished.elapsed() < self.memory,
        });
    }

    /// Runs `work` once per `op_id`: a repeat waits for the first run, or gets its remembered result.
    pub fn run(
        &self,
        op_id: &str,
        work: impl FnOnce() -> Result<String, String>,
    ) -> Result<String, String> {
        let mut entries = self.entries();
        self.prune(&mut entries);
        loop {
            match entries.get(op_id) {
                Some(Op::Done { result, .. }) => return result.clone(),
                Some(Op::Running) => {
                    entries = self
                        .finished
                        .wait(entries)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                None => break,
            }
        }
        entries.insert(op_id.to_string(), Op::Running);
        drop(entries);
        let result = work();
        self.entries().insert(
            op_id.to_string(),
            Op::Done {
                result: result.clone(),
                finished: Instant::now(),
            },
        );
        self.finished.notify_all();
        result
    }

    pub fn result(&self, op_id: &str) -> OpResult {
        let mut entries = self.entries();
        self.prune(&mut entries);
        match entries.get(op_id) {
            None => OpResult {
                state: op_state::UNKNOWN.to_string(),
                ..OpResult::default()
            },
            Some(Op::Running) => OpResult {
                state: op_state::RUNNING.to_string(),
                ..OpResult::default()
            },
            Some(Op::Done { result, .. }) => OpResult {
                state: op_state::DONE.to_string(),
                said: result.as_ref().ok().cloned(),
                error: result.as_ref().err().cloned(),
            },
        }
    }
}

/// Answers the project methods for this machine's state and holders.
pub struct Dispatcher {
    state: StateDir,
    holders: SocketDir,
    binary: PathBuf,
    ops: Ops,
    ptys: Ptys,
    watches: Watches,
}

impl Dispatcher {
    pub fn new(state: StateDir, holders: SocketDir, binary: PathBuf, op_memory: Duration) -> Self {
        Dispatcher {
            ptys: Ptys::new(holders.clone(), binary.clone()),
            watches: Watches::new(state.clone()),
            state,
            holders,
            binary,
            ops: Ops::new(op_memory),
        }
    }

    pub fn ptys(&self) -> &Ptys {
        &self.ptys
    }

    pub fn watches(&self) -> &Watches {
        &self.watches
    }

    /// A client went away: its terminals detach and its watches stop.
    pub fn disconnect(&self, connection: u64) {
        self.ptys.disconnect(connection);
        self.watches.disconnect(connection);
    }

    /// The methods that need to know who asks: watches publish on the caller's connection.
    pub fn dispatch_watch(
        &self,
        request: &Request,
        caller: &Caller<'_>,
    ) -> Option<Result<Value, RemoteError>> {
        let raw = &request.params;
        Some(match request.method.as_str() {
            method::WATCH_SUBSCRIBE => params::<WatchParams>(raw).and_then(|params| {
                let source = self.source(&params.cfg)?;
                self.watches.subscribe(source, caller, params)
            }),
            method::WATCH_SET_BRANCH => params::<WatchBranchParams>(raw)
                .and_then(|params| self.watches.set_branch(caller, params)),
            method::WATCH_UNSUBSCRIBE => params::<WatchChannel>(raw)
                .and_then(|params| self.watches.unsubscribe(caller, params)),
            _ => return None,
        })
    }

    pub fn dispatch_pty(
        &self,
        request: &Request,
        caller: &Caller<'_>,
    ) -> Option<Result<Value, RemoteError>> {
        let raw = &request.params;
        let cfg = || {
            let fields: pom_remote::server_api::WorkspaceParams = params(raw)?;
            check_branch(&fields.branch)?;
            self.source(&fields.cfg)
        };
        Some(match request.method.as_str() {
            method::PTY_OPEN => cfg().and_then(|source| self.ptys.open(&source, raw)),
            method::PTY_ATTACH => cfg().and_then(|source| self.ptys.attach(&source, caller, raw)),
            method::PTY_RESIZE => self.ptys.resize(caller, raw),
            method::PTY_PRIMARY => self.ptys.primary(caller, raw),
            method::PTY_DETACH => self.ptys.detach(caller, raw),
            method::PTY_CLOSE => cfg().and_then(|source| self.ptys.close(&source, raw)),
            _ => return None,
        })
    }

    fn source(&self, cfg: &str) -> Result<LocalSource, RemoteError> {
        Ok(LocalSource::new(
            registered_config(&self.state, cfg)?,
            self.state.clone(),
            self.holders.clone(),
            self.binary.clone(),
        ))
    }

    fn workspace_source(&self, params: &WorkspaceParams) -> Result<LocalSource, RemoteError> {
        check_branch(&params.branch)?;
        self.source(&params.cfg)
    }

    pub fn dispatch(&self, request: &Request) -> Result<Value, RemoteError> {
        let raw = &request.params;
        match request.method.as_str() {
            method::PROJECT_LIST => to_value(serde_json::json!({
                "schema": pom_remote::PROJECT_SCHEMA,
                "projects": pom_remote::local_projects(&self.state),
            })),
            method::WS_LIST => {
                let params: ProjectParams = params(raw)?;
                to_value(self.source(&params.cfg)?.workspaces().map_err(failed)?)
            }
            method::WS_STATUS => {
                let params: WorkspaceParams = params(raw)?;
                let source = self.workspace_source(&params)?;
                to_value(source.status(&params.branch).map_err(failed)?)
            }
            method::SVC_LOGS => {
                let params: LogsParams = params(raw)?;
                check_branch(&params.branch)?;
                check_name("service", &params.service)?;
                let source = self.source(&params.cfg)?;
                to_value(
                    source
                        .logs(&params.branch, &params.service)
                        .map_err(failed)?,
                )
            }
            method::AGENT_LIST => {
                let params: WorkspaceParams = params(raw)?;
                let source = self.workspace_source(&params)?;
                to_value(AgentListing {
                    schema: pom_agent::SCHEMA.to_string(),
                    sessions: source.agents(&params.branch).map_err(failed)?,
                })
            }
            method::AGENT_TURN => {
                let params: TurnParams = params(raw)?;
                check_branch(&params.branch)?;
                check_name("agent role", &params.role)?;
                let source = self.source(&params.cfg)?;
                source
                    .last_turn_document(&params.branch, &params.role)
                    .map_err(failed)
            }
            method::SVC_ACT => self.act(params(raw)?),
            method::AGENT_START => {
                let params: AgentStartParams = params(raw)?;
                check_branch(&params.branch)?;
                check_name("agent role", &params.role)?;
                let source = self.source(&params.cfg)?;
                let session = source
                    .start_agent(&params.branch, &params.role, params.person)
                    .map_err(failed)?;
                to_value(AgentStarted {
                    schema: pom_agent::SCHEMA.to_string(),
                    session,
                })
            }
            method::AGENT_CLAIM => {
                let params: TurnParams = params(raw)?;
                check_branch(&params.branch)?;
                check_name("agent role", &params.role)?;
                let source = self.source(&params.cfg)?;
                to_value(
                    source
                        .claim_agent(&params.branch, &params.role)
                        .map_err(failed)?,
                )
            }
            method::AGENT_TAKE_OVER => {
                let params: AgentHolderParams = params(raw)?;
                check_branch(&params.branch)?;
                check_word("holder", &params.holder)?;
                let source = self.source(&params.cfg)?;
                source
                    .take_over_agent(&params.branch, &params.holder)
                    .map_err(failed)?;
                Ok(Value::Null)
            }
            method::AGENT_ANSWER => {
                let params: AgentAnswerParams = params(raw)?;
                check_branch(&params.branch)?;
                check_word("holder", &params.holder)?;
                check_word("request", &params.request)?;
                let source = self.source(&params.cfg)?;
                source
                    .answer_agent(
                        &params.branch,
                        &params.holder,
                        &params.request,
                        params.allow,
                    )
                    .map_err(failed)?;
                Ok(Value::Null)
            }
            method::OP_RESULT => {
                let params: OpParams = params(raw)?;
                check_op_id(&params.op_id)?;
                to_value(self.ops.result(&params.op_id))
            }
            other => Err(RemoteError::new(
                error_code::METHOD_NOT_FOUND,
                format!("no method {other}"),
            )),
        }
    }

    fn act(&self, params: ActParams) -> Result<Value, RemoteError> {
        check_branch(&params.branch)?;
        check_name("target", &params.target)?;
        check_op_id(&params.op_id)?;
        let action = ServiceAction::parse(&params.action)
            .ok_or_else(|| invalid("action must be start, stop or restart"))?;
        let source = self.source(&params.cfg)?;
        let result = self.ops.run(&params.op_id, || {
            source
                .act(&params.branch, action, &params.target)
                .map_err(|error| error.to_string())
        });
        match result {
            Ok(said) => to_value(said),
            Err(message) => Err(RemoteError::new(FAILED, message)),
        }
    }
}
