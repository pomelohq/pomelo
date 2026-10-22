//! Where a project's state comes from: this machine's crates, or `pom ... --json` on another machine over
//! ssh. Both answer with the same types, so a view never knows which one it reads.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pom_agent::SessionView;
use pom_services::{ServiceHealth, SharedHealth};
use serde::{Deserialize, Serialize};

use crate::server_api::{
    AgentsUpdate, JobRequest, Progress, StatusUpdate, TurnUpdate, WorkspacesUpdate,
};
use crate::{Host, Ssh, EXIT_UNREACHABLE};

pub const WORKSPACE_SCHEMA: &str = "pom.workspace/v1";
pub const PROJECT_SCHEMA: &str = "pom.project/v1";

/// A create or delete that stopped part-way, and the stage to resume from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FailedRun {
    pub operation: String,
    pub stage: usize,
}

/// One workspace as `pom ws ls --json` lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceSummary {
    pub branch: String,
    pub is_main: bool,
    pub path: PathBuf,
    pub name: String,
    pub repos: Vec<String>,
    pub running: usize,
    /// `running`, `sleeping` or `queued`; empty from a pom that predates sleeping.
    pub state: String,
    pub in_progress: Option<String>,
    pub failed: Option<FailedRun>,
    /// Commits origin has on the workspace's branches that its worktrees do not, as of the last fetch.
    pub remote_ahead: u64,
    /// What the last GitHub push for the workspace did.
    pub last_sync: Option<LastSync>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LastSync {
    pub time: String,
    /// `pulled`, `notified`, `dirty`, `diverged`, `up_to_date`, `deleted`, `failed`...
    pub outcome: String,
    pub repo: String,
    pub commits: u64,
    pub restarted: Vec<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceList {
    pub schema: String,
    pub project: String,
    pub root: PathBuf,
    pub workspaces: Vec<WorkspaceSummary>,
}

/// A workspace's services and the shared ones, as `pom status -o json` reports them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusReport {
    pub workspace: String,
    pub services: Vec<ServiceHealth>,
    pub shared: Vec<SharedHealth>,
}

/// A project a machine knows, as `pom project ls --json` lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectEntry {
    pub name: String,
    pub path: PathBuf,
    pub config: PathBuf,
    pub missing: bool,
    pub last_used: i64,
}

/// The last turn of an agent session: what it was asked and what it answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentTurn {
    pub turn: u64,
    pub stop_reason: String,
    pub prompt: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

impl ServiceAction {
    pub fn parse(verb: &str) -> Option<ServiceAction> {
        match verb {
            "start" => Some(ServiceAction::Start),
            "stop" => Some(ServiceAction::Stop),
            "restart" => Some(ServiceAction::Restart),
            _ => None,
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            ServiceAction::Start => "start",
            ServiceAction::Stop => "stop",
            ServiceAction::Restart => "restart",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// The machine did not answer (ssh failed, timed out or is missing).
    Unreachable(String),
    /// It answered, and the command failed.
    Failed(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::Unreachable(message) | SourceError::Failed(message) => {
                formatter.write_str(message)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Via {
    Server,
    #[default]
    Ssh,
}

impl Via {
    pub fn name(self) -> &'static str {
        match self {
            Via::Server => "server",
            Via::Ssh => "ssh",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransportInfo {
    pub via: Via,
    pub fallback_reason: Option<String>,
    pub remote_version: Option<String>,
    pub version_mismatch: bool,
    pub terminals: bool,
    /// The server pushes changes, so nothing needs asking again and again.
    pub pushes: bool,
    /// The server starts agents and moves their leases.
    pub agents: bool,
    /// The server streams a service's output as it prints it.
    pub live_logs: bool,
    /// The server runs workspace jobs (create, sleep, wake, pull) and reports their progress.
    pub jobs: bool,
}

/// The long-lived session's state, and how many times it came back to a restarted server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkReport {
    pub state: pom_rpc::pty::LinkState,
    pub restarts: u64,
}

/// One value a machine pushed because it changed.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Workspaces(WorkspacesUpdate),
    Status(StatusUpdate),
    Agents(AgentsUpdate),
    Turn(TurnUpdate),
}

#[derive(Clone, Debug, PartialEq)]
pub enum FeedEvent {
    Update(Update),
    /// Nothing changed meanwhile.
    Idle,
    /// The pushes stopped (the connection went); ask again to know where things stand.
    Ended(String),
}

/// What a machine pushes about one workspace of a project.
pub trait Feed: Send + Sync {
    fn next(&self, timeout: Duration) -> FeedEvent;
    /// Follows another workspace, and the last turn of agent `turn`.
    fn follow(&self, branch: &str, turn: Option<&str>) -> Result<(), SourceError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogEvent {
    Data(Vec<u8>),
    /// Output was skipped: what follows does not carry on from what came before.
    Skipped,
    /// The output starts over from an earlier point: drop what was shown.
    StartedOver,
    Idle,
    Ended(String),
}

/// A service's output as the machine streams it.
pub trait LogFeed: Send + Sync {
    fn next(&self, timeout: Duration) -> LogEvent;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEvent {
    Progress(Progress),
    Idle,
    /// The pushes stopped: after the job's last event, or because the connection went.
    Ended(String),
}

/// A job's progress as the machine pushes it.
pub trait JobFeed: Send + Sync {
    fn next(&self, timeout: Duration) -> JobEvent;
}

fn unsupported(what: &str) -> SourceError {
    SourceError::Failed(format!(
        "{what} needs pom server on the project's machine; update pom there"
    ))
}

/// A project's state and controls, read on whichever machine runs it.
pub trait ProjectSource: Send + Sync {
    /// Where it runs, for the connection banner ("dev (dev@dev.example.net)").
    fn location(&self) -> String;
    fn workspaces(&self) -> Result<WorkspaceList, SourceError>;
    fn status(&self, branch: &str) -> Result<StatusReport, SourceError>;
    /// What the service printed lately (its scrollback, or what a crashed one left).
    fn logs(&self, branch: &str, service: &str) -> Result<String, SourceError>;
    fn agents(&self, branch: &str) -> Result<Vec<SessionView>, SourceError>;
    fn agent_turn(&self, branch: &str, role: &str) -> Result<Option<AgentTurn>, SourceError>;
    /// Starts, stops or restarts `target` (a service, a repo or a group); returns what pom said.
    fn act(&self, branch: &str, action: ServiceAction, target: &str)
        -> Result<String, SourceError>;
    fn transport(&self) -> Option<TransportInfo> {
        None
    }
    /// How the long-lived session to the machine stands, for a source that keeps one.
    fn link(&self) -> Option<LinkReport> {
        None
    }
    /// Tries to reconnect now rather than at the next scheduled attempt.
    fn reconnect_now(&self) {}
    /// Installs `release`'s pom on the machine and moves the session to its server.
    fn update_pom(
        &self,
        _release: &crate::update::Release,
        _progress: &dyn Fn(crate::update::UpdateStep),
    ) -> Result<(), String> {
        Err("updating pom needs pom server on the project's machine".to_string())
    }
    fn pty(&self, _branch: &str) -> Option<Arc<dyn pom_rpc::pty::PtyProvider>> {
        None
    }
    /// Pushes about `branch` (empty: only the workspace list) when the machine sends them; `None` means ask.
    fn events(
        &self,
        _branch: &str,
        _turn: Option<&str>,
    ) -> Result<Option<Arc<dyn Feed>>, SourceError> {
        Ok(None)
    }
    /// Starts agent `role` (`claude` for the workspace's main one) for the person here to drive.
    fn start_agent(&self, _branch: &str, _role: &str) -> Result<SessionView, SourceError> {
        Err(unsupported("Starting an agent"))
    }
    /// The person here takes the agent in `holder` over from whoever drives it.
    fn take_over_agent(&self, _branch: &str, _holder: &str) -> Result<(), SourceError> {
        Err(unsupported("Taking an agent over"))
    }
    fn answer_agent(
        &self,
        _branch: &str,
        _holder: &str,
        _request: &str,
        _allow: bool,
    ) -> Result<(), SourceError> {
        Err(unsupported("Answering an agent"))
    }
    /// The service's output as it prints it, when the machine streams it; `None` means ask with `logs`.
    fn follow_logs(
        &self,
        _branch: &str,
        _service: &str,
    ) -> Result<Option<Box<dyn LogFeed>>, SourceError> {
        Ok(None)
    }
    /// Starts a job once per `op_id` and returns its id.
    fn start_job(&self, _request: &JobRequest, _op_id: &str) -> Result<String, SourceError> {
        Err(unsupported("Running a workspace job"))
    }
    /// The job's progress from event `since_seq` on.
    fn follow_job(&self, _id: &str, _since_seq: u64) -> Result<Box<dyn JobFeed>, SourceError> {
        Err(unsupported("Following a workspace job"))
    }
    fn cancel_job(&self, _id: &str) -> Result<(), SourceError> {
        Err(unsupported("Cancelling a workspace job"))
    }
}

/// The last turn out of `agent read --json`, or the JSON of the turns read locally.
pub(crate) fn last_turn(document: &serde_json::Value) -> Option<AgentTurn> {
    let turn = document["turns"].as_array()?.last()?;
    let text: Vec<&str> = turn["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item["kind"] == "text")
                .filter_map(|item| item["text"].as_str())
                .collect()
        })
        .unwrap_or_default();
    Some(AgentTurn {
        turn: turn["turn"].as_u64().unwrap_or_default(),
        stop_reason: turn["stop_reason"].as_str().unwrap_or_default().to_string(),
        prompt: turn["prompt"].as_str().unwrap_or_default().to_string(),
        text: text.join("\n\n"),
    })
}

/// A project on another machine, read with `pom -c <its pom.yml> ... --json` over ssh.
pub struct RemoteSource {
    pub host: Host,
    /// The project's pom.yml, as a path on the remote machine.
    pub config: String,
    ssh: Ssh,
}

pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// `start` waits for nothing but can spend a while spawning holders and shared containers.
pub(crate) const ACT_TIMEOUT: Duration = Duration::from_secs(180);

impl RemoteSource {
    pub fn new(host: Host, config: String, ssh: Ssh) -> RemoteSource {
        RemoteSource { host, config, ssh }
    }

    fn run(&self, args: &[&str], timeout: Duration) -> Result<String, SourceError> {
        let mut full = vec!["-c".to_string(), self.config.clone()];
        full.extend(args.iter().map(|arg| arg.to_string()));
        run_pom(&self.ssh, &self.host, &full, timeout)
    }

    fn json<T: for<'de> Deserialize<'de>>(&self, args: &[&str]) -> Result<T, SourceError> {
        let text = self.run(args, READ_TIMEOUT)?;
        serde_json::from_str(&text).map_err(|error| {
            SourceError::Failed(format!(
                "{} answered something pom here cannot read ({error}); check the versions with: pom host test {}",
                self.host.name, self.host.name
            ))
        })
    }
}

/// pom with `args` on `host`; its stdout when it exits 0.
pub fn run_pom(
    ssh: &Ssh,
    host: &Host,
    args: &[String],
    timeout: Duration,
) -> Result<String, SourceError> {
    let output = ssh.output(host, args, timeout).map_err(|error| {
        SourceError::Unreachable(match error.kind() {
            std::io::ErrorKind::NotFound => "ssh is not installed".to_string(),
            _ => error.to_string(),
        })
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.strip_prefix("error: ").unwrap_or(line).to_string());
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Some(255) | Some(EXIT_UNREACHABLE) | None => {
            Err(SourceError::Unreachable(reason.unwrap_or_else(|| {
                format!("could not reach {}", host.name)
            })))
        }
        Some(127) => Err(SourceError::Failed(format!(
            "pom is not installed on {}; check it with: pom host test {}",
            host.name, host.name
        ))),
        Some(code) => {
            Err(SourceError::Failed(reason.unwrap_or_else(|| {
                format!("pom exited {code} on {}", host.name)
            })))
        }
    }
}

/// The projects `host` knows (`pom project ls --json` there).
pub fn remote_projects(ssh: &Ssh, host: &Host) -> Result<Vec<ProjectEntry>, SourceError> {
    #[derive(Deserialize)]
    struct Listing {
        #[serde(default)]
        projects: Vec<ProjectEntry>,
    }
    let args = ["project", "ls", "--json"].map(str::to_string);
    let text = run_pom(ssh, host, &args, READ_TIMEOUT)?;
    serde_json::from_str::<Listing>(&text)
        .map(|listing| listing.projects)
        .map_err(|error| {
            SourceError::Failed(format!(
                "{} did not list its projects ({error}); it may run an older pom, check with: pom host test {}",
                host.name, host.name
            ))
        })
}

impl ProjectSource for RemoteSource {
    fn location(&self) -> String {
        format!("{} ({})", self.host.name, self.host.url.alias())
    }

    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        self.json(&["ws", "ls", "--json"])
    }

    fn status(&self, branch: &str) -> Result<StatusReport, SourceError> {
        self.json(&["-w", branch, "status", "-o", "json"])
    }

    fn logs(&self, branch: &str, service: &str) -> Result<String, SourceError> {
        self.run(&["-w", branch, "logs", service], READ_TIMEOUT)
    }

    fn agents(&self, branch: &str) -> Result<Vec<SessionView>, SourceError> {
        #[derive(Deserialize)]
        struct Listing {
            #[serde(default)]
            sessions: Vec<SessionView>,
        }
        self.json::<Listing>(&["agent", "ls", branch, "--json"])
            .map(|listing| listing.sessions)
    }

    fn agent_turn(&self, branch: &str, role: &str) -> Result<Option<AgentTurn>, SourceError> {
        let handle = format!("{branch}/{role}");
        let document: serde_json::Value = self.json(&["agent", "read", &handle, "--json"])?;
        Ok(last_turn(&document))
    }

    fn act(
        &self,
        branch: &str,
        action: ServiceAction,
        target: &str,
    ) -> Result<String, SourceError> {
        self.run(&["-w", branch, action.verb(), target], ACT_TIMEOUT)
    }

    fn transport(&self) -> Option<TransportInfo> {
        Some(TransportInfo::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_turn_reads_prompt_and_text_items() {
        let document = serde_json::json!({
            "schema": "pom.agent/v1",
            "turns": [
                {"turn": 1, "stop_reason": "end_turn", "prompt": "first", "items": []},
                {"turn": 2, "stop_reason": "end_turn", "prompt": "fix the login test",
                 "items": [{"kind": "text", "text": "Looking."}, {"kind": "tool_call", "name": "Bash", "text": "ls"},
                           {"kind": "text", "text": "Fixed."}]}
            ]
        });
        let turn = last_turn(&document).expect("turn");
        assert_eq!(turn.turn, 2);
        assert_eq!(turn.prompt, "fix the login test");
        assert_eq!(turn.text, "Looking.\n\nFixed.");
        assert_eq!(last_turn(&serde_json::json!({"turns": []})), None);
    }

    #[test]
    fn listings_from_a_newer_pom_still_read() {
        let list: WorkspaceList = serde_json::from_str(
            r#"{"schema":"pom.workspace/v1","project":"myproject","root":"/home/dev/pom/myproject",
                "workspaces":[{"branch":"main","is_main":true,"path":"/home/dev/pom/myproject/workspace--main",
                "repos":["api"],"running":2,"future_field":1}],"later":true}"#,
        )
        .expect("list");
        assert_eq!(list.workspaces[0].running, 2);
        let status: StatusReport = serde_json::from_str(
            r#"{"workspace":"main","services":[{"repo":"api","service":"web","up":true,"ready":true,"healthy":null,"port":4000,"crashed":false}],"shared":[]}"#,
        )
        .expect("status");
        assert_eq!(status.services[0].port, Some(4000));
    }
}
