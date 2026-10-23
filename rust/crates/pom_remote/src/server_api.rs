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
    pub const LOGS_FOLLOW: &str = "logs.follow";
    pub const JOB_START: &str = "job.start";
    pub const JOB_FOLLOW: &str = "job.follow";
    pub const JOB_CANCEL: &str = "job.cancel";
    pub const FS_WATCH: &str = "fs.watch";
    pub const FS_READ: &str = "fs.read";
    pub const FS_STAT: &str = "fs.stat";
    pub const FS_FILES: &str = "fs.files";
    pub const FS_EXPAND: &str = "fs.expand";
    pub const SVC_FACTS: &str = "svc.facts";
    pub const SVC_RUN: &str = "svc.run";
    pub const SVC_SHARED: &str = "svc.shared";
    pub const SVC_MODE: &str = "svc.mode";
    pub const SVC_ENV_PROFILE: &str = "svc.env_profile";
    pub const SVC_SHARED_LOGS: &str = "svc.shared_logs";
    pub const FS_WRITE: &str = "fs.write";
    pub const FS_CREATE_FILE: &str = "fs.create_file";
    pub const FS_CREATE_DIR: &str = "fs.create_dir";
    pub const FS_RENAME: &str = "fs.rename";
    pub const FS_COPY: &str = "fs.copy";
    pub const FS_REMOVE: &str = "fs.remove";
    pub const FS_TRASH: &str = "fs.trash";
    pub const FS_APPEND_GITIGNORE: &str = "fs.append_gitignore";
    pub const SEARCH_START: &str = "search.start";
    pub const SEARCH_CANCEL: &str = "search.cancel";
    pub const USAGE_GET: &str = "usage.get";
    pub const JIRA_SETTINGS: &str = "jira.settings";
    pub const JIRA_SAVE: &str = "jira.save";
    pub const JIRA_TEST: &str = "jira.test";
    pub const JIRA_SEARCH: &str = "jira.search";
    pub const JIRA_ISSUE: &str = "jira.issue";
    pub const JIRA_BOARDS: &str = "jira.boards";
    pub const JIRA_LIST: &str = "jira.list";
    pub const PASTE_PUT: &str = "paste.put";
    pub const GIT_REPOS: &str = "git.repos";
    pub const GIT_SCAN: &str = "git.scan";
    pub const GIT_WATCH: &str = "git.watch";
    pub const GIT_ACT: &str = "git.act";
    pub const GIT_READ: &str = "git.read";
    pub const GIT_BRANCHES: &str = "git.branches";
    pub const GIT_BRANCH: &str = "git.branch";
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
    method::LOGS_FOLLOW,
    method::JOB_START,
    method::JOB_FOLLOW,
    method::JOB_CANCEL,
    method::FS_WATCH,
    method::FS_READ,
    method::FS_STAT,
    method::FS_FILES,
    method::SVC_FACTS,
    method::SVC_RUN,
    method::SVC_SHARED,
    method::SVC_MODE,
    method::SVC_ENV_PROFILE,
    method::SVC_SHARED_LOGS,
    method::FS_WRITE,
    method::FS_CREATE_FILE,
    method::FS_CREATE_DIR,
    method::FS_RENAME,
    method::FS_COPY,
    method::FS_REMOVE,
    method::FS_TRASH,
    method::FS_APPEND_GITIGNORE,
    method::FS_EXPAND,
    method::SEARCH_START,
    method::SEARCH_CANCEL,
    method::USAGE_GET,
    method::JIRA_SETTINGS,
    method::JIRA_SAVE,
    method::JIRA_TEST,
    method::JIRA_SEARCH,
    method::JIRA_ISSUE,
    method::JIRA_BOARDS,
    method::JIRA_LIST,
    method::PASTE_PUT,
    method::GIT_REPOS,
    method::GIT_SCAN,
    method::GIT_WATCH,
    method::GIT_ACT,
    method::GIT_READ,
    method::GIT_BRANCHES,
    method::GIT_BRANCH,
];

/// What the server's HELLO lists beside its methods.
pub const FEATURES: &[&str] = &[
    pom_rpc::pty::SHARED_EVENTS,
    FILES_READ,
    JOB_TASK,
    FILES_WRITE,
    FILES_SEARCH,
    MODULE_STORE,
    GIT_REMOTE,
];

/// The server runs a repo's pre-written command as a job (`job_op::TASK`).
pub const JOB_TASK: &str = "job.task";

/// The server reads a workspace's files: its tree as it changes, a file, the finder's list.
pub const FILES_READ: &str = "files.read";
/// The server saves a workspace's files and changes its tree: new files and folders, moves, copies, removals.
pub const FILES_WRITE: &str = "files.write";

/// The server runs the node_modules Store page's requests as jobs (`job_op::STORE`).
pub const MODULE_STORE: &str = "module_store";

/// The server runs the Git panel's fetch, pull and push as jobs (`job_op::GIT_REMOTE`).
pub const GIT_REMOTE: &str = "git.remote";

/// The server searches a workspace's files and pushes what it finds.
pub const FILES_SEARCH: &str = "files.search";

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

/// The methods a server must list for the app to follow a service's output instead of asking for it again.
pub const LIVE_LOGS: &[&str] = &[method::LOGS_FOLLOW];

/// The methods a server must list for the app to create, sleep, wake and pull workspaces there.
pub const JOBS: &[&str] = &[method::JOB_START, method::JOB_FOLLOW, method::JOB_CANCEL];

/// The methods a server must list for the app to read Jira with the settings kept on that machine.
pub const JIRA: &[&str] = &[
    method::JIRA_SETTINGS,
    method::JIRA_SAVE,
    method::JIRA_TEST,
    method::JIRA_SEARCH,
    method::JIRA_ISSUE,
    method::JIRA_BOARDS,
    method::JIRA_LIST,
];

/// The methods a server must list for the app to show its repos in the Git panel.
pub const GIT: &[&str] = &[
    method::GIT_REPOS,
    method::GIT_SCAN,
    method::GIT_WATCH,
    method::GIT_ACT,
    method::GIT_READ,
    method::GIT_BRANCHES,
    method::GIT_BRANCH,
];

/// What a watch pushes, each as its full value whenever it changed.
pub mod topic {
    pub const WORKSPACES: &str = "workspaces";
    pub const STATUS: &str = "status";
    pub const AGENTS: &str = "agents";
    /// The last turn of the agent the client follows.
    pub const TURN: &str = "turn";
    /// What the Services panel shows of the workspace, from a server that lists `svc.facts`.
    pub const SERVICES: &str = "services";
    pub const ALL: &[&str] = &[WORKSPACES, STATUS, AGENTS, TURN, SERVICES];
    /// What a server from before `svc.facts` pushes.
    pub const BEFORE_SERVICES: &[&str] = &[WORKSPACES, STATUS, AGENTS, TURN];
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
    /// Of `streams`, those following a service's output.
    pub log_follows: usize,
    /// Jobs still running.
    pub jobs: usize,
    /// Terminals and followed streams of clients that went, kept for them to reconnect.
    pub retained: usize,
    /// Workspace folders whose files are watched for a client, or were until a minute ago.
    pub file_watches: usize,
    pub socket: PathBuf,
    pub log: PathBuf,
    /// The dev proxy port on the server's machine (0 from a server that predates it).
    pub proxy_port: u16,
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

/// `channel` is the stream the client opened for the output; `since` the offset to go on from (0: all of it).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FollowLogsParams {
    pub cfg: String,
    pub branch: String,
    pub service: String,
    pub since: u64,
    pub channel: u32,
}

/// Where the followed output starts and stands: offsets of the stream, which run on across restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FollowedLogs {
    pub holder: String,
    pub start: u64,
    pub end: u64,
    /// The service was running; otherwise the stream starts with what it left and waits for it to start.
    pub running: bool,
}

/// What a job runs.
pub mod job_op {
    pub const WS_CREATE: &str = "ws.create";
    pub const WS_WAKE: &str = "ws.wake";
    pub const WS_SLEEP: &str = "ws.sleep";
    pub const START_WAIT: &str = "start_wait";
    pub const SYNC: &str = "sync";
    /// A repo's pre-written command, run with the workspace's env.
    pub const TASK: &str = "task";
    /// A request of the node_modules Store page, reporting [`module_store::StoreUpdate`]s as `data`.
    pub const STORE: &str = "store";
    /// A fetch, pull or push of one repo, reporting git's progress as lines and its outcome as `data`.
    pub const GIT_REMOTE: &str = "git.remote";
    pub const ALL: &[&str] = &[
        WS_CREATE, WS_WAKE, WS_SLEEP, START_WAIT, SYNC, TASK, STORE, GIT_REMOTE,
    ];
}

/// `ws.create`: an empty `repos` checks out every repo of the project.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateArgs {
    pub branch: String,
    pub repos: Vec<String>,
    pub environment: String,
    pub fresh_databases: bool,
    pub display_name: String,
    /// 0-based stage to resume a stopped run from.
    pub from_stage: usize,
}

/// `ws.wake`, `ws.sleep` (with `force`) and `sync`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BranchArgs {
    pub branch: String,
    pub force: bool,
}

/// `start_wait`: starts `target` (a service, repo or group) and waits until it is ready.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartWaitArgs {
    pub branch: String,
    pub target: String,
    pub timeout_seconds: u64,
}

/// `task`: the command of `repo` whose label (its description, else key, else command) is `label`; the
/// server runs only what the project's config holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskArgs {
    pub branch: String,
    pub repo: String,
    pub label: String,
}

/// `store`: a request of the node_modules Store page, run on the project's store and workspaces there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreArgs {
    pub request: module_store::Request,
}

/// `git.remote`: `command` in repo `repo` of workspace `branch`, with the machine's own git credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRemoteArgs {
    pub branch: String,
    pub repo: String,
    pub command: git::access::RemoteCommand,
}

/// How a `git.remote` job ended: git's output, or its error.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitRemoteOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<git::working_copy::CommandOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<git::working_copy::CommandError>,
}

/// A job as the app asks for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobRequest {
    Create(CreateArgs),
    Wake(BranchArgs),
    Sleep(BranchArgs),
    StartWait(StartWaitArgs),
    Sync(BranchArgs),
    Task(TaskArgs),
    Store(StoreArgs),
    GitRemote(GitRemoteArgs),
}

impl JobRequest {
    pub fn op(&self) -> &'static str {
        match self {
            JobRequest::Create(_) => job_op::WS_CREATE,
            JobRequest::Wake(_) => job_op::WS_WAKE,
            JobRequest::Sleep(_) => job_op::WS_SLEEP,
            JobRequest::StartWait(_) => job_op::START_WAIT,
            JobRequest::Sync(_) => job_op::SYNC,
            JobRequest::Task(_) => job_op::TASK,
            JobRequest::Store(_) => job_op::STORE,
            JobRequest::GitRemote(_) => job_op::GIT_REMOTE,
        }
    }

    pub fn branch(&self) -> &str {
        match self {
            JobRequest::Create(args) => &args.branch,
            JobRequest::StartWait(args) => &args.branch,
            JobRequest::Task(args) => &args.branch,
            JobRequest::Store(_) => "",
            JobRequest::GitRemote(args) => &args.branch,
            JobRequest::Wake(args) | JobRequest::Sleep(args) | JobRequest::Sync(args) => {
                &args.branch
            }
        }
    }

    pub fn args(&self) -> serde_json::Value {
        let value = match self {
            JobRequest::Create(args) => serde_json::to_value(args),
            JobRequest::StartWait(args) => serde_json::to_value(args),
            JobRequest::Task(args) => serde_json::to_value(args),
            JobRequest::Store(args) => serde_json::to_value(args),
            JobRequest::GitRemote(args) => serde_json::to_value(args),
            JobRequest::Wake(args) | JobRequest::Sleep(args) | JobRequest::Sync(args) => {
                serde_json::to_value(args)
            }
        };
        value.unwrap_or_default()
    }
}

/// `op_id` names the start, so asking again (after a reconnect) gets the same job instead of a second one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobStartParams {
    pub cfg: String,
    pub op: String,
    pub args: serde_json::Value,
    pub op_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobStarted {
    pub id: String,
    /// The op id had already started this job.
    pub existing: bool,
}

/// Pushes the job's progress from `since_seq` on, on the client's subscription `channel`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobFollowParams {
    pub id: String,
    pub since_seq: u64,
    pub channel: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobFollowed {
    pub id: String,
    pub op: String,
    /// The seq the next event will have.
    pub next_seq: u64,
    pub done: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobParams {
    pub id: String,
}

/// The topic a job's progress is pushed under.
pub const PROGRESS_TOPIC: &str = "progress";

/// What a progress event says.
pub mod progress_kind {
    /// `stages` lists the job's stages, all pending.
    pub const STAGES: &str = "stages";
    pub const STAGE_STARTED: &str = "stage_started";
    pub const STAGE_DONE: &str = "stage_done";
    pub const STAGE_SKIPPED: &str = "stage_skipped";
    /// `line` is a step of the running stage.
    pub const LINE: &str = "line";
    /// `line` went wrong without stopping the job.
    pub const WARNING: &str = "warning";
    /// Stage `stage` failed with `line`.
    pub const FAILED: &str = "failed";
    /// The last event: `done` with the job's `exit`.
    pub const DONE: &str = "done";
    /// `data` is what the job reports in its own terms.
    pub const DATA: &str = "data";
}

/// One event of a job, numbered from 0 so a client can follow again from the last one it saw.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub seq: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub line: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// The exit of a job stopped by `job.cancel`.
pub const EXIT_CANCELLED: i32 = 130;

/// `channel` is the client's subscription the tree is pushed on, as [`TreeChunk`]s under [`FS_TREE_TOPIC`].
/// With `lazy_ignored` a gitignored folder comes as a lazy entry whose contents follow [`method::FS_EXPAND`];
/// a server from before it sends every folder's contents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsWatchParams {
    pub cfg: String,
    pub branch: String,
    pub channel: u32,
    pub lazy_ignored: bool,
}

/// The workspace folder whose tree is pushed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsWatched {
    pub root: PathBuf,
}

pub const FS_TREE_TOPIC: &str = "fs.tree";

/// A piece of the tree. `reset` starts a full snapshot that replaces what the client has; its chunks run until
/// one with `last`. After that each change comes as a patch, also ending with `last`: `removed` paths go with
/// everything under them, `entries` are added or replaced, `changed` files have new contents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TreeChunk {
    pub scan_id: u64,
    pub reset: bool,
    pub last: bool,
    pub entries: Vec<files::FileEntry>,
    pub removed: Vec<String>,
    pub changed: Vec<files::ChangedFile>,
}

/// `path` is relative to the workspace, '/'-separated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsPathParams {
    pub cfg: String,
    pub branch: String,
    pub path: String,
}

/// `channel` is a stream the client opened: a file larger than the inline limit, or any file asked `raw`,
/// comes over it. `limit` (0: the server's) caps the size read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsReadParams {
    pub cfg: String,
    pub branch: String,
    pub path: String,
    pub channel: u32,
    pub limit: u64,
    pub raw: bool,
}

/// A file as read: inline (`text`, or `binary`), or `streamed` over the channel with `size` bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsRead {
    pub text: Option<String>,
    pub binary: bool,
    pub mtime_ns: u64,
    pub size: u64,
    pub streamed: bool,
}

/// The finder's file list comes over `channel`, one path per line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsFilesParams {
    pub cfg: String,
    pub branch: String,
    pub include_ignored: bool,
    pub channel: u32,
}

/// A save: the text inline, or `size` bytes sent as INPUT on `channel` once the server writes [`WRITE_READY`]
/// there. `expect` is the file as the client last read or saved it; a file that is no longer that one is kept
/// and reported as a conflict. `op_id` makes asking again after a reconnect answer the first save's outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsWriteParams {
    pub cfg: String,
    pub branch: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub channel: u32,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expect: Option<files::FileStat>,
    pub op_id: String,
}

/// What a save did: `written` with the file as it now is, or a `conflict` with the file as found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsWritten {
    pub written: bool,
    pub conflict: bool,
    pub mtime_ns: u64,
    pub size: u64,
}

/// The first bytes the server sends on a save's channel: it is listening, the text may come.
pub const WRITE_READY: &[u8] = b"ready";

/// A change to the tree: `path` alone (new file, new folder, remove, trash, .gitignore) or `from` to `path`
/// (rename, copy). `is_dir` says what `path` is for a .gitignore line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FsChangeParams {
    pub cfg: String,
    pub branch: String,
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_dir: bool,
    pub op_id: String,
}

/// Starts a project search pushed on the client's subscription `channel`: [`SearchFound`] under
/// [`SEARCH_FOUND_TOPIC`] as files turn up, then one [`SearchDone`] under [`SEARCH_DONE_TOPIC`]. A new search from
/// the same `searcher` on the same connection stops the one before.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchStartParams {
    pub cfg: String,
    pub branch: String,
    pub channel: u32,
    pub searcher: String,
    pub search: files::search::SearchRequest,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchCancelParams {
    pub channel: u32,
}

pub const SEARCH_FOUND_TOPIC: &str = "search.found";
pub const SEARCH_DONE_TOPIC: &str = "search.done";

/// Files with matches. A file too large for one push comes in pieces, one after another: each piece but the last
/// is the last file of a push with `continued`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchFound {
    pub files: Vec<files::search::FileResult>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub continued: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchDone {
    pub limited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Error codes of the `fs.*` methods beside the path guard's.
pub mod fs_error {
    pub const TOO_LARGE: &str = "too_large";
    pub const BUSY: &str = "busy";
    /// The text is not something the file may hold, such as a project config that does not load.
    pub const REJECTED: &str = "rejected";
    /// The save's text stopped coming before all of it arrived; asking again sends it again.
    pub const INCOMPLETE: &str = "incomplete";
}

/// How a service stands: `running`, `crashed` (it died and left a record) or `stopped`.
pub mod service_status {
    pub const RUNNING: &str = "running";
    pub const CRASHED: &str = "crashed";
    pub const STOPPED: &str = "stopped";
}

/// How a crashed service died: the line of its output that most likely says why, how it exited, and when
/// (seconds since the epoch).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrashFact {
    pub line: Option<String>,
    pub exit: String,
    pub at: Option<u64>,
}

/// One service of the workspace as the Services panel shows it. `repo` is `_ws` for a workspace-level one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceFact {
    pub repo: String,
    pub service: String,
    pub holder: String,
    pub status: String,
    pub port: Option<u16>,
    pub mode: String,
    pub env_profile: String,
    /// Its dev-proxy address on the machine.
    pub url: Option<String>,
    pub pid: Option<i32>,
    /// When its holder started, in seconds since the epoch.
    pub started_at: Option<u64>,
    pub crash: Option<CrashFact>,
}

/// A shared service as the Services panel shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedFact {
    pub name: String,
    pub running: bool,
    pub host_port: u16,
    /// The holder of a command shared service.
    pub holder: String,
    pub pid: Option<i32>,
    pub started_at: Option<u64>,
    /// What `{{shared.<name>.url}}` gives the workspace.
    pub connection: Option<String>,
    /// The folder a command shared service runs in, as shown.
    pub folder: String,
}

/// Everything the Services panel shows of one workspace, read on the machine that runs it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServicesFacts {
    pub branch: String,
    /// The workspace's folder there.
    pub root: PathBuf,
    /// The project's pom.yml as it reads now.
    pub config: String,
    pub services: Vec<ServiceFact>,
    pub shared: Vec<SharedFact>,
    /// The repos checked out in the workspace.
    pub repos: Vec<String>,
    /// Running services of the project in other workspaces.
    pub others_running: usize,
    /// The dev proxy's addresses reach the machine from elsewhere, not only `localhost` there.
    pub public_urls: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServicesUpdate {
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facts: Option<ServicesFacts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What `svc.run` and `svc.shared` do.
pub mod service_verb {
    pub const START: &str = "start";
    pub const STOP: &str = "stop";
    pub const RESTART: &str = "restart";
    /// Moves a service to a new port and starts it there.
    pub const RELOCATE: &str = "relocate";
}

/// One service of a workspace (`repo` `_ws` for a workspace-level one); `op_id` as for `svc.act`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceRunParams {
    pub cfg: String,
    pub branch: String,
    pub repo: String,
    pub service: String,
    pub action: String,
    pub op_id: String,
}

/// A shared service, or all of them for `*`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedRunParams {
    pub cfg: String,
    pub name: String,
    pub action: String,
    pub op_id: String,
}

/// `value` is the mode or env profile to set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceChoiceParams {
    pub cfg: String,
    pub branch: String,
    pub repo: String,
    pub service: String,
    pub value: String,
}

/// The last `lines` lines of shared service `name`'s output (0: the server's default).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedLogsParams {
    pub cfg: String,
    pub name: String,
    pub lines: usize,
}

/// `since`: when the asking app's day began (unix seconds), so "today" is the person's day, not the host's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageParams {
    pub cfg: String,
    pub since: u64,
}

/// The Claude account the host's agents run under, its plan limits, and what the project's agents cost today
/// there. Nothing secret: the credentials stay on the host.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageReport {
    pub account: Option<agent_usage::Account>,
    pub limits: Option<agent_usage::Limits>,
    pub limits_error: Option<agent_usage::LimitsError>,
    /// When the limits shown were read (unix seconds).
    pub limits_at: Option<u64>,
    pub today: Option<agent_usage::DaySummary>,
}

/// A Jira request about the project `cfg`: `keys` for `jira.search`, `key` for `jira.issue`, `list` (as
/// [`pom_jira::TicketList`] writes it) for `jira.list`, `change` for `jira.save`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JiraParams {
    pub cfg: String,
    pub keys: Vec<String>,
    pub key: String,
    pub list: String,
    pub change: pom_jira::SetupChange,
}

/// Who the Jira settings sign in as.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JiraAccount {
    pub name: String,
    pub email: String,
}

/// The largest file `paste.put` takes.
pub const PASTE_LIMIT: u64 = 20 * 1024 * 1024;

/// A file pasted in a terminal on this Mac (a screenshot), for the program there to read: `size` bytes sent as
/// INPUT on `channel` once the server writes [`WRITE_READY`]. `extension` is the file's type, such as `png`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PasteParams {
    pub extension: String,
    pub channel: u32,
    pub size: u64,
}

/// Where the pasted file now is on the host.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pasted {
    pub path: PathBuf,
}

/// One repo of workspace `branch`, by its name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitRepoParams {
    pub cfg: String,
    pub branch: String,
    pub repo: String,
}

/// `op` on repo `repo` of workspace `branch`; `op_id` makes asking again after a reconnect answer the first run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitActParams {
    #[serde(default)]
    pub cfg: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub repo: String,
    pub op: git::access::GitOp,
    #[serde(default)]
    pub op_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitReadParams {
    #[serde(default)]
    pub cfg: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub repo: String,
    pub read: git::access::GitRead,
}

/// Pushes [`GitScanned`] under [`GIT_SCAN_TOPIC`] on the client's subscription `channel` whenever one of `repos`
/// (names) of workspace `branch` reads differently, and once for each at the start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitWatchParams {
    pub cfg: String,
    pub branch: String,
    pub repos: Vec<String>,
    pub channel: u32,
}

pub const GIT_SCAN_TOPIC: &str = "git.scan";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitScanned {
    pub repo: String,
    pub scan: git::scan::RepoScan,
}

/// What `git.act` did: nothing to say, or git's error with its output.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitActed {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<git::working_copy::CommandError>,
}

/// A repo's branches as the branch picker lists them, read from its checkout in the workspace (else in main).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitBranches {
    /// The branch the main checkout is on, which a new branch starts from.
    pub base: String,
    pub branches: Vec<pom_workspace::BranchInfo>,
}

/// What `git.branch` does to a repo of a workspace.
pub mod branch_action {
    /// Keeps the branch the repo is on now as the one it uses.
    pub const KEEP: &str = "keep";
    /// Checks out `target`.
    pub const SWITCH: &str = "switch";
    /// Fetches origin, then checks out `target`.
    pub const USE: &str = "use";
    /// Fetches origin in the checkout the branches are listed from.
    pub const FETCH: &str = "fetch";
    pub const ALL: &[&str] = &[KEEP, SWITCH, USE, FETCH];
}

/// A change to which branch repo `repo` of workspace `branch` uses; answers what it did, as a sentence.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitBranchParams {
    pub cfg: String,
    pub branch: String,
    pub repo: String,
    pub action: String,
    pub target: String,
    pub op_id: String,
}
