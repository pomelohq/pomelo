//! The requests `pom server` answers and their parameters, shared by the server and [`crate::ServerSource`].
//! Answers are the types `--json` already prints, so both sides read them with the same structs.

use std::path::PathBuf;

use pom_agent::SessionView;
use serde::{Deserialize, Serialize};

pub const SERVER_SCHEMA: &str = "pom.server/v1";

pub mod method {
    pub const SERVER_INFO: &str = "server.info";
    pub const SERVER_STOP: &str = "server.stop";
    pub const PROJECT_LIST: &str = "project.list";
    pub const WS_LIST: &str = "ws.list";
    pub const WS_STATUS: &str = "ws.status";
    pub const SVC_LOGS: &str = "svc.logs";
    pub const AGENT_LIST: &str = "agent.list";
    pub const AGENT_TURN: &str = "agent.turn";
    pub const SVC_ACT: &str = "svc.act";
    pub const OP_RESULT: &str = "op.result";
    pub use pom_rpc::pty::method::{
        ATTACH as PTY_ATTACH, CLOSE as PTY_CLOSE, DETACH as PTY_DETACH, OPEN as PTY_OPEN,
        PRIMARY as PTY_PRIMARY, RESIZE as PTY_RESIZE,
    };
}

/// Every method this build serves; the server's HELLO lists them as its features.
pub const METHODS: &[&str] = &[
    method::SERVER_INFO,
    method::SERVER_STOP,
    method::PROJECT_LIST,
    method::WS_LIST,
    method::WS_STATUS,
    method::SVC_LOGS,
    method::AGENT_LIST,
    method::AGENT_TURN,
    method::SVC_ACT,
    method::OP_RESULT,
    method::PTY_OPEN,
    method::PTY_ATTACH,
    method::PTY_RESIZE,
    method::PTY_PRIMARY,
    method::PTY_DETACH,
    method::PTY_CLOSE,
];

pub const REQUIRED: &[&str] = &[
    method::WS_LIST,
    method::WS_STATUS,
    method::SVC_LOGS,
    method::AGENT_LIST,
    method::AGENT_TURN,
    method::SVC_ACT,
    method::OP_RESULT,
];

/// `cfg` is the project's pom.yml on the server's machine, as `pom project ls --json` lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectParams {
    pub cfg: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceParams {
    pub cfg: String,
    pub branch: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogsParams {
    pub cfg: String,
    pub branch: String,
    pub service: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnParams {
    pub cfg: String,
    pub branch: String,
    pub role: String,
}

/// `op_id` names the action so that asking again (after a reconnect) never runs it twice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActParams {
    pub cfg: String,
    pub branch: String,
    pub action: String,
    pub target: String,
    pub op_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpParams {
    pub op_id: String,
}

pub mod op_state {
    pub const RUNNING: &str = "running";
    pub const DONE: &str = "done";
    pub const UNKNOWN: &str = "unknown";
}

/// What became of an action: still `running`, `done` with what it said or its error, or `unknown` (never
/// seen, or forgotten after five minutes).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpResult {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentListing {
    pub schema: String,
    pub sessions: Vec<SessionView>,
}

/// `pom server status --json` and the `server.info` answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerInfo {
    pub schema: String,
    pub pid: u32,
    pub version: String,
    pub uptime_seconds: u64,
    pub connections: usize,
    pub streams: usize,
    pub socket: PathBuf,
    pub log: PathBuf,
}
