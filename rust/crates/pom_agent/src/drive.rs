//! The operations behind `pom agent` and the agent MCP tools, on one workspace's agent sessions: list,
//! start, stop, read and watch. Every operation that touches a session goes through the gate first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pom_paths::StateDir;
use pom_ptyhost::{SocketDir, SpawnRequest};
use serde::Serialize;
use serde_json::{json, Value};

use crate::gate::{Gate, Refusal};
use crate::identity::{holder_role, workspace_prefix};
use crate::launch::{claude_launch_with, fresh_launch, LaunchContext, LaunchOptions};
use crate::sessions::{
    events, now_ms, read_state, workspace_sessions, EventLine, SessionState, SCHEMA,
};
use crate::turns::{read_turns, RecordedTurn};

/// A working state no event refreshed for this long is reported stale.
const STALE_AFTER_MS: u64 = 15 * 60 * 1000;
const HOLDER_COLS: u16 = 160;
const HOLDER_ROWS: u16 = 48;
const MCP_SERVER: &str = "pom";
const WORKSPACE_LOG_KEY: &str = "\u{0}workspace";

#[derive(Debug)]
pub enum DriveError {
    Refused(Refusal),
    /// The prompt was typed but the agent never took it as a turn.
    NotSubmitted(String),
    NotFound(String),
    Invalid(String),
    Failed(String),
}

impl DriveError {
    pub fn exit_code(&self) -> i32 {
        match self {
            DriveError::Refused(_) => crate::gate::REFUSED_EXIT,
            DriveError::NotSubmitted(_) => crate::conversation::NOT_SUBMITTED_EXIT,
            DriveError::NotFound(_) | DriveError::Invalid(_) | DriveError::Failed(_) => 1,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            DriveError::Refused(refusal) => refusal.code(),
            DriveError::NotSubmitted(_) => "not_submitted",
            DriveError::NotFound(_) => "not_found",
            DriveError::Invalid(_) => "invalid",
            DriveError::Failed(_) => "failed",
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"schema": SCHEMA, "error": self.code(), "message": self.to_string()})
    }
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriveError::Refused(refusal) => write!(formatter, "{refusal}"),
            DriveError::NotSubmitted(handle) => write!(
                formatter,
                "the prompt was typed into {handle} but it never started a turn"
            ),
            DriveError::NotFound(message)
            | DriveError::Invalid(message)
            | DriveError::Failed(message) => write!(formatter, "{message}"),
        }
    }
}

impl From<Refusal> for DriveError {
    fn from(refusal: Refusal) -> DriveError {
        DriveError::Refused(refusal)
    }
}

/// One agent session as `ls` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionView {
    pub handle: String,
    pub workspace: String,
    pub role: String,
    pub holder: String,
    pub driver: String,
    pub session_id: String,
    pub alive: bool,
    pub drivable: bool,
    /// `idle`, `thinking`, `tool_use`, `compacting`, `awaiting_input`, `died` or `unknown`.
    pub state: String,
    pub turn: u64,
    pub last_event: String,
    pub last_event_age_s: u64,
    pub stale: bool,
}

impl SessionView {
    pub fn is_working(&self) -> bool {
        matches!(self.state.as_str(), "thinking" | "tool_use" | "compacting")
    }
}

/// What a project's agent sessions need from the machine.
pub struct Drive {
    pub state: StateDir,
    pub holders: SocketDir,
    /// The project's session name.
    pub project: String,
    pub home: PathBuf,
    pub binary: PathBuf,
    pub tool_path: String,
}

fn view_of(record: &SessionState, alive: bool, drivable: bool, now: u64) -> SessionView {
    let age = now.saturating_sub(record.event_ms);
    let working = matches!(
        record.state.as_str(),
        "thinking" | "tool_use" | "compacting"
    );
    let state = if working && !alive {
        "died".to_string()
    } else if !alive && record.last_event == "SessionEnd" {
        "ended".to_string()
    } else {
        record.state.clone()
    };
    SessionView {
        handle: format!("{}/{}", record.branch, record.role),
        workspace: record.branch.clone(),
        role: record.role.clone(),
        holder: record.holder.clone(),
        driver: record.driver.clone(),
        session_id: record.session_id.clone(),
        alive,
        drivable,
        state,
        turn: record.turn,
        last_event: record.last_event.clone(),
        last_event_age_s: age / 1000,
        stale: working && alive && age > STALE_AFTER_MS,
    }
}

/// A handle names a session as `<workspace>/<role>`, or just its role inside the workspace in use.
pub fn split_handle(handle: &str, default_branch: &str) -> (String, String) {
    match handle.rsplit_once('/') {
        Some((branch, role)) if !role.is_empty() => (branch.to_string(), role.to_string()),
        _ => (default_branch.to_string(), handle.to_string()),
    }
}

impl Drive {
    fn alive(&self, holder: &str) -> bool {
        self.holders.holder_alive(holder)
    }

    /// The agent sessions of one workspace: every recorded one, plus live agent holders that have not
    /// reported yet (started before this version).
    pub fn list(&self, branch: &str) -> Vec<SessionView> {
        let now = now_ms();
        let mut views: Vec<SessionView> = Vec::new();
        for entry in workspace_sessions(&self.state, &self.project, branch) {
            let Some(record) = read_state(&self.state, &entry.session_id) else {
                continue;
            };
            let role = holder_role(&record.holder, &self.project, branch);
            let view = view_of(
                &record,
                self.alive(&record.holder),
                role.is_some_and(|role| role.drivable),
                now,
            );
            // A resumed conversation keeps its id; a relaunched role gets a new one: keep the newest per role.
            match views.iter_mut().find(|known| known.role == view.role) {
                Some(known) if known.alive && !view.alive => {}
                Some(known) => *known = view,
                None => views.push(view),
            }
        }
        let prefix = workspace_prefix(&self.project, branch);
        for (holder, _) in self.holders.holders() {
            if !holder.starts_with(&prefix) || views.iter().any(|view| view.holder == holder) {
                continue;
            }
            let Some(role) = holder_role(&holder, &self.project, branch) else {
                continue;
            };
            views.push(SessionView {
                handle: format!("{branch}/{}", role.role),
                workspace: branch.to_string(),
                role: role.role,
                holder,
                driver: role.driver.to_string(),
                session_id: String::new(),
                alive: true,
                drivable: role.drivable,
                state: "unknown".into(),
                turn: 0,
                last_event: String::new(),
                last_event_age_s: 0,
                stale: false,
            });
        }
        views.sort_by(|a, b| a.role.cmp(&b.role));
        views
    }

    /// The session a handle names, inside the gate's workspace.
    pub fn resolve(&self, gate: &Gate<'_>, handle: &str) -> Result<SessionView, DriveError> {
        let workspace = gate.workspace();
        let (branch, role) = split_handle(handle, &workspace.branch);
        let by_holder = handle.starts_with("ws-");
        if !by_holder && branch != workspace.branch {
            return Err(Refusal::OtherWorkspace {
                allowed: workspace.branch.clone(),
            }
            .into());
        }
        if by_holder {
            gate.target(handle)?;
        }
        self.list(&workspace.branch)
            .into_iter()
            .find(|view| {
                if by_holder {
                    view.holder == handle
                } else {
                    view.role == role
                }
            })
            .ok_or_else(|| {
                DriveError::NotFound(format!(
                    "no agent session {handle} in workspace {}",
                    workspace.branch
                ))
            })
    }

    /// Starts a session, or reuses the running one unless `fresh`. A fresh session runs as role `role`
    /// (`claude-<role>`) on a new conversation of its own.
    pub fn start(
        &self,
        gate: &Gate<'_>,
        cwd: &Path,
        is_main: bool,
        role: &str,
        fresh: bool,
        options: &LaunchOptions,
    ) -> Result<SessionView, DriveError> {
        if let Some(extra) = &options.extra_mcp_config {
            check_extra_mcp(extra)?;
        }
        let options = &LaunchOptions {
            isolated: true,
            ..options.clone()
        };
        let workspace = gate.workspace().clone();
        let context = LaunchContext {
            state: &self.state,
            home: &self.home,
            binary: &self.binary,
            tool_path: &self.tool_path,
            session: &self.project,
            branch: &workspace.branch,
            is_main,
            cwd,
        };
        let launch = if fresh {
            if role == "claude" || !role_is_valid(role) {
                return Err(DriveError::Invalid(format!(
                    "a fresh session needs its own --role (lowercase letters, digits, dashes), not {role:?}"
                )));
            }
            fresh_launch(&context, role, options).0
        } else if role == "claude" {
            claude_launch_with(&context, options)
        } else {
            return self.resolve(gate, role);
        };
        gate.target(&launch.holder)?;
        if self.alive(&launch.holder) {
            if fresh {
                return Err(DriveError::Invalid(format!(
                    "{role} is already running in {}; stop it or pick another --role",
                    workspace.branch
                )));
            }
        } else {
            pom_ptyhost::spawn_holder(
                &self.holders,
                &SpawnRequest {
                    binary: &self.binary,
                    name: &launch.holder,
                    cwd: &launch.cwd,
                    cols: HOLDER_COLS,
                    rows: HOLDER_ROWS,
                    argv: &launch.argv,
                    env: &[],
                },
            )
            .map_err(|error| DriveError::Failed(format!("could not start {role}: {error}")))?;
        }
        let holder = launch.holder.clone();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !self.alive(&holder) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let role_name = holder_role(&holder, &self.project, &workspace.branch)
            .map_or_else(|| role.to_string(), |role| role.role);
        Ok(self
            .list(&workspace.branch)
            .into_iter()
            .find(|view| view.holder == holder)
            .unwrap_or(SessionView {
                handle: format!("{}/{role_name}", workspace.branch),
                workspace: workspace.branch.clone(),
                role: role_name,
                holder,
                driver: crate::identity::CLAUDE_DRIVER.into(),
                session_id: String::new(),
                alive: false,
                drivable: true,
                state: "starting".into(),
                turn: 0,
                last_event: String::new(),
                last_event_age_s: 0,
                stale: false,
            }))
    }

    /// Ends a session and its holder.
    pub fn stop(&self, gate: &Gate<'_>, view: &SessionView) -> Result<(), DriveError> {
        gate.target(&view.holder)?;
        if !view.session_id.is_empty() {
            let identity = crate::identity::Identity {
                holder: view.holder.clone(),
                role: view.role.clone(),
                project: self.project.clone(),
                branch: view.workspace.clone(),
                driver: view.driver.clone(),
            };
            let event = crate::sessions::SessionEvent {
                session_id: view.session_id.clone(),
                event: "Killed".into(),
                state: Some(crate::hooks::AgentState::Idle),
                ..crate::sessions::SessionEvent::default()
            };
            if let Err(error) = crate::sessions::record_event(&self.state, &identity, &event) {
                eprintln!("agent: record stop of {}: {error}", view.handle);
            }
        }
        match self.holders.kill_holder(&view.holder) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DriveError::Failed(format!(
                "could not stop {}: {error}",
                view.handle
            ))),
        }
    }

    /// The turns of a session `select` picks.
    pub fn read(
        &self,
        view: &SessionView,
        select: impl Fn(u64) -> bool,
        full: bool,
    ) -> Result<Vec<RecordedTurn>, DriveError> {
        if view.session_id.is_empty() {
            return Err(DriveError::NotFound(format!(
                "{} has not reported a session yet (started before this version, or still starting)",
                view.handle
            )));
        }
        Ok(read_turns(
            &self.state,
            &view.session_id,
            select,
            view.alive,
            full,
        ))
    }

    /// New events of a workspace's sessions since `seen` (event count per session), as `watch` lines.
    pub fn new_events(
        &self,
        branch: &str,
        seen: &mut std::collections::HashMap<String, usize>,
    ) -> Vec<Value> {
        let mut lines = Vec::new();
        for entry in workspace_sessions(&self.state, &self.project, branch) {
            let all: Vec<EventLine> = events(&self.state, &entry.session_id);
            let from = seen.get(&entry.session_id).copied().unwrap_or(0);
            for line in all.iter().skip(from) {
                lines.push(watch_line(branch, &entry.role, &entry.session_id, line));
            }
            seen.insert(entry.session_id.clone(), all.len());
        }
        // Lease changes and messages between sessions belong to the workspace, not to one session.
        let log = crate::lease::log_lines(&self.state, &self.project, branch);
        let from = seen.get(WORKSPACE_LOG_KEY).copied().unwrap_or(0);
        for line in log.iter().skip(from) {
            let mut line = line.clone();
            line["schema"] = json!(SCHEMA);
            lines.push(line);
        }
        seen.insert(WORKSPACE_LOG_KEY.to_string(), log.len());
        lines.sort_by_key(|line| line.get("t_ms").and_then(Value::as_u64).unwrap_or(0));
        lines
    }
}

pub fn watch_line(branch: &str, role: &str, session_id: &str, line: &EventLine) -> Value {
    let mut value = json!({
        "schema": SCHEMA,
        "t_ms": line.t_ms,
        "workspace": branch,
        "role": role,
        "session_id": session_id,
        "event": line.event,
        "state": line.state,
        "turn": line.turn,
    });
    if let Some(tool) = &line.tool {
        value["tool"] = json!(tool);
    }
    if !line.detail.is_null() {
        value["detail"] = line.detail.clone();
    }
    value
}

fn role_is_valid(role: &str) -> bool {
    role.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && role
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !role.starts_with("raw")
}

/// An extra MCP config may add servers, never replace pom's own.
fn check_extra_mcp(extra: &str) -> Result<(), DriveError> {
    let text = if extra.trim_start().starts_with('{') {
        extra.to_string()
    } else {
        std::fs::read_to_string(extra)
            .map_err(|error| DriveError::Invalid(format!("--extra-mcp-config {extra}: {error}")))?
    };
    let config: Value = serde_json::from_str(&text)
        .map_err(|error| DriveError::Invalid(format!("--extra-mcp-config is not JSON: {error}")))?;
    let servers = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or_else(|| DriveError::Invalid("--extra-mcp-config has no mcpServers".into()))?;
    if servers.contains_key(MCP_SERVER) {
        return Err(DriveError::Invalid(format!(
            "--extra-mcp-config may not define a server named {MCP_SERVER:?}: it is Pomelo's own"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::caller::Caller;
    use crate::gate::{Limits, Workspace};
    use crate::hooks::AgentState;
    use crate::identity::Identity;
    use crate::sessions::{record_event, SessionEvent};

    fn drive(temp: &Path) -> Drive {
        Drive {
            state: StateDir::new(temp.join("state")),
            holders: SocketDir::new(temp.join("holders")),
            project: "myproject".into(),
            home: temp.join("home"),
            binary: PathBuf::from("/bin/false"),
            tool_path: "/usr/bin:/bin".into(),
        }
    }

    fn record(
        drive: &Drive,
        branch: &str,
        role: &str,
        holder: &str,
        session: &str,
        event: &str,
        state: AgentState,
    ) {
        let identity = Identity {
            holder: holder.into(),
            role: role.into(),
            project: "myproject".into(),
            branch: branch.into(),
            driver: "claude".into(),
        };
        let event = SessionEvent {
            session_id: session.into(),
            event: event.into(),
            state: Some(state),
            ..SessionEvent::default()
        };
        record_event(&drive.state, &identity, &event).expect("event");
    }

    #[test]
    fn ls_shows_one_workspaces_sessions_with_their_own_state() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        record(
            &drive,
            "feat-login",
            "claude",
            "ws-myproject-feat-login-claude-raw",
            "a",
            "UserPromptSubmit",
            AgentState::Thinking,
        );
        record(
            &drive,
            "feat-login",
            "reviewer",
            "ws-myproject-feat-login-claude-reviewer",
            "b",
            "Stop",
            AgentState::Idle,
        );
        record(
            &drive,
            "feat-signup",
            "claude",
            "ws-myproject-feat-signup-claude-raw",
            "c",
            "Stop",
            AgentState::Idle,
        );
        let views = drive.list("feat-login");
        let summary: Vec<(String, String, bool)> = views
            .iter()
            .map(|view| (view.handle.clone(), view.state.clone(), view.alive))
            .collect();
        assert_eq!(
            summary,
            [
                ("feat-login/claude".to_string(), "died".to_string(), false),
                ("feat-login/reviewer".to_string(), "idle".to_string(), false),
            ],
            "a working session whose holder is gone reads as died; feat-signup's is not listed"
        );
        assert_eq!(views[0].turn, 1);
    }

    #[test]
    fn a_handle_of_another_workspace_or_an_unknown_role_is_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        record(
            &drive,
            "feat-login",
            "claude",
            "ws-myproject-feat-login-claude-raw",
            "a",
            "Stop",
            AgentState::Idle,
        );
        let workspace = Workspace {
            project: "myproject".into(),
            branch: "feat-login".into(),
        };
        let gate =
            Gate::open(&drive.state, Caller::Operator, workspace, Limits::OPERATOR).expect("gate");
        assert_eq!(drive.resolve(&gate, "claude").expect("own").session_id, "a");
        assert_eq!(
            drive
                .resolve(&gate, "feat-login/claude")
                .expect("own")
                .session_id,
            "a"
        );
        assert!(matches!(
            drive.resolve(&gate, "feat-signup/claude"),
            Err(DriveError::Refused(Refusal::OtherWorkspace { .. }))
        ));
        assert!(matches!(
            drive.resolve(&gate, "ws-myproject-feat-signup-claude-raw"),
            Err(DriveError::Refused(Refusal::OtherWorkspace { .. }))
        ));
        assert!(matches!(
            drive.resolve(&gate, "reviewer"),
            Err(DriveError::NotFound(_))
        ));
    }

    #[test]
    fn an_extra_mcp_config_cannot_replace_poms_server() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        let workspace = Workspace {
            project: "myproject".into(),
            branch: "feat-login".into(),
        };
        let gate =
            Gate::open(&drive.state, Caller::Operator, workspace, Limits::OPERATOR).expect("gate");
        let options = LaunchOptions {
            extra_mcp_config: Some(r#"{"mcpServers":{"pom":{"command":"evil"}}}"#.into()),
            ..LaunchOptions::default()
        };
        let refused = drive.start(&gate, temp.path(), false, "reviewer", true, &options);
        assert!(
            matches!(refused, Err(DriveError::Invalid(message)) if message.contains("Pomelo's own"))
        );
        let bad_role = drive.start(
            &gate,
            temp.path(),
            false,
            "claude",
            true,
            &LaunchOptions::default(),
        );
        assert!(matches!(bad_role, Err(DriveError::Invalid(_))));
    }

    #[test]
    fn watch_lines_carry_the_workspace_and_role_and_arrive_once() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        record(
            &drive,
            "feat-login",
            "claude",
            "ws-myproject-feat-login-claude-raw",
            "a",
            "UserPromptSubmit",
            AgentState::Thinking,
        );
        let mut seen = std::collections::HashMap::new();
        let first = drive.new_events("feat-login", &mut seen);
        assert_eq!(first.len(), 1);
        assert_eq!(
            (
                first[0]["workspace"].as_str(),
                first[0]["role"].as_str(),
                first[0]["event"].as_str()
            ),
            (Some("feat-login"), Some("claude"), Some("UserPromptSubmit"))
        );
        record(
            &drive,
            "feat-login",
            "claude",
            "ws-myproject-feat-login-claude-raw",
            "a",
            "PreCompact",
            AgentState::Compacting,
        );
        let next = drive.new_events("feat-login", &mut seen);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0]["state"], "compacting");
        assert!(drive.new_events("feat-login", &mut seen).is_empty());
    }
}
