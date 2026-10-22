//! The requests `pom server` answers and their parameters, shared by the server and [`crate::ServerSource`].
//! Answers are the types `--json` already prints, so both sides read them with the same structs.

use std::path::PathBuf;

use pom_agent::{PendingApproval, SessionView};
use serde::{Deserialize, Serialize};

use crate::source::{StatusReport, WorkspaceList};

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
    pub const WATCH_SUBSCRIBE: &str = "watch.subscribe";
    pub const WATCH_SET_BRANCH: &str = "watch.set_branch";
    pub const WATCH_UNSUBSCRIBE: &str = "watch.unsubscribe";
    pub const AGENT_START: &str = "agent.start";
    pub const AGENT_CLAIM: &str = "agent.claim";
    pub const AGENT_TAKE_OVER: &str = "agent.take_over";
    pub const AGENT_ANSWER: &str = "agent.answer";
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
    method::WATCH_SUBSCRIBE,
    method::WATCH_SET_BRANCH,
    method::WATCH_UNSUBSCRIBE,
    method::AGENT_START,
    method::AGENT_CLAIM,
    method::AGENT_TAKE_OVER,
    method::AGENT_ANSWER,
];

/// What the server's HELLO lists beside its methods.
pub const FEATURES: &[&str] = &[pom_rpc::pty::SHARED_EVENTS];

/// The methods a server must list for the app to take pushes instead of asking again and again.
pub const PUSH: &[&str] = &[
    method::WATCH_SUBSCRIBE,
    method::WATCH_SET_BRANCH,
    method::WATCH_UNSUBSCRIBE,
];

/// The methods a server must list for the app to start agents and drive their leases there.
pub const AGENTS: &[&str] = &[
    method::AGENT_START,
    method::AGENT_TAKE_OVER,
    method::AGENT_ANSWER,
];

/// What a watch pushes, each as its full value whenever it changed.
pub mod topic {
    pub const WORKSPACES: &str = "workspaces";
    pub const STATUS: &str = "status";
    pub const AGENTS: &str = "agents";
    /// The last turn of the agent the client follows.
    pub const TURN: &str = "turn";
    pub const ALL: &[&str] = &[WORKSPACES, STATUS, AGENTS, TURN];
}

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

/// `channel` is the client's subscription the values are pushed on; `branch` may be empty until the client
/// knows which workspace it shows, and then only the workspace list is pushed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchParams {
    pub cfg: String,
    pub branch: String,
    pub topics: Vec<String>,
    pub channel: u32,
    /// The agent role whose last turn is pushed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchBranchParams {
    pub channel: u32,
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchChannel {
    pub channel: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspacesUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<WorkspaceList>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusUpdate {
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<StatusReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Who drives an agent session when it is not the person at the app, and the tool call it waits on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentPresence {
    pub holder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driven_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingApproval>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsUpdate {
    pub branch: String,
    pub sessions: Vec<SessionView>,
    pub presence: Vec<AgentPresence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The followed agent's last turn as `agent.turn` answers it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnUpdate {
    pub branch: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `person`: the person at the app starts it and drives it, rather than the caller as an orchestrator.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentStartParams {
    pub cfg: String,
    pub branch: String,
    pub role: String,
    pub person: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentStarted {
    pub schema: String,
    pub session: SessionView,
}

/// The lease an orchestrator took: attach with `token` as the claim to type into the session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentClaim {
    pub holder: String,
    pub token: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentHolderParams {
    pub cfg: String,
    pub branch: String,
    pub holder: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentAnswerParams {
    pub cfg: String,
    pub branch: String,
    pub holder: String,
    pub request: String,
    pub allow: bool,
}
