//! Every agent session's own state, keyed by the agent's session id: an append-only event log, the
//! current state, and a per-workspace index. Several sessions share a workspace and several hooks of one
//! session run at once, so every read-modify-write here holds a lock on the file it changes.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hooks::AgentState;
use crate::identity::Identity;

pub const SCHEMA: &str = "pom.agent/v1";
const SESSIONS_DIR: &str = "agents/sessions";
const INDEX_DIR: &str = "agents/index";
const EVENTS_FILE: &str = "events.ndjson";
const STATE_FILE: &str = "state.json";

/// Where a turn stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    #[default]
    None,
    Running,
    Ended,
    Cancelled,
}

/// One line of a session's event log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLine {
    pub t_ms: u64,
    pub event: String,
    pub state: String,
    pub turn: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<String>,
    /// How long the transcript was when the event fired, so a turn is the slice between two events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_bytes: Option<u64>,
    /// Extra fields of the event, such as a permission request's id and input.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub detail: Value,
}

/// A session's current state, `sessions/<id>/state.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub schema: String,
    pub session_id: String,
    pub holder: String,
    pub project: String,
    pub branch: String,
    pub role: String,
    pub driver: String,
    pub state: String,
    pub last_event: String,
    pub event_ms: u64,
    pub turn: u64,
    pub turn_state: TurnState,
    #[serde(default)]
    pub transcript: String,
}

/// One session in a workspace's index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub session_id: String,
    pub role: String,
    pub holder: String,
    pub driver: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceIndex {
    schema: String,
    project: String,
    branch: String,
    sessions: Vec<IndexEntry>,
}

/// pom's own event for a session it just launched, before the agent ran any hook.
pub const LAUNCHED_EVENT: &str = "Launched";
/// A launched session the agent has not reported on yet; nothing may be sent to it.
pub const STARTING_STATE: &str = "starting";
/// A launched session stuck at the agent's "do you trust this folder" prompt.
pub const NEEDS_TRUST_STATE: &str = "needs_trust";
pub const NEEDS_TRUST_EVENT: &str = "NeedsTrust";

/// A hook event, reduced to what the session log keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionEvent {
    pub session_id: String,
    pub event: String,
    pub state: Option<AgentState>,
    pub transcript: String,
    pub tool: Option<String>,
    pub notification: Option<String>,
    pub detail: Value,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn session_dir(state: &StateDir, session_id: &str) -> PathBuf {
    state.path(SESSIONS_DIR).join(safe(session_id))
}

fn index_path(state: &StateDir, project: &str, branch: &str) -> PathBuf {
    state
        .path(INDEX_DIR)
        .join(safe(project))
        .join(format!("{}.json", safe(branch)))
}

/// Runs `body` while holding an exclusive lock on `path`, which it creates.
pub fn with_lock<T>(
    path: &Path,
    body: impl FnOnce(&mut std::fs::File) -> std::io::Result<T>,
) -> std::io::Result<T> {
    use std::os::fd::AsRawFd;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)?;
    // SAFETY: flock on a descriptor we own; it is released when the file closes.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    body(&mut file)
}

fn read_all(file: &mut std::fs::File) -> std::io::Result<String> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut text)?;
    Ok(text)
}

/// Every event line of a session, oldest first.
pub fn events(state: &StateDir, session_id: &str) -> Vec<EventLine> {
    std::fs::read_to_string(session_dir(state, session_id).join(EVENTS_FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

pub fn events_path(state: &StateDir, session_id: &str) -> PathBuf {
    session_dir(state, session_id).join(EVENTS_FILE)
}

pub fn read_state(state: &StateDir, session_id: &str) -> Option<SessionState> {
    let text = std::fs::read_to_string(session_dir(state, session_id).join(STATE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Appends one event to a session and moves its state on: `UserPromptSubmit` opens the next turn, `Stop`
/// ends it, `Interrupted` (written by pom, since an interrupt fires no hook) cancels it.
pub fn record_event(
    state: &StateDir,
    identity: &Identity,
    event: &SessionEvent,
) -> std::io::Result<SessionState> {
    let dir = session_dir(state, &event.session_id);
    let previous = read_state(state, &event.session_id);
    let record = with_lock(&dir.join(EVENTS_FILE), |file| {
        let prompts = read_all(file)?
            .lines()
            .filter_map(|line| serde_json::from_str::<EventLine>(line).ok())
            .filter(|line| line.event == "UserPromptSubmit")
            .count() as u64;
        let turn = if event.event == "UserPromptSubmit" {
            prompts + 1
        } else {
            prompts
        };
        let current = read_state(state, &event.session_id);
        let turn_state = match event.event.as_str() {
            "UserPromptSubmit" => TurnState::Running,
            "Stop" => TurnState::Ended,
            "Interrupted" => TurnState::Cancelled,
            _ => current
                .as_ref()
                .map_or(TurnState::None, |current| current.turn_state),
        };
        let agent_state = if event.event == LAUNCHED_EVENT {
            STARTING_STATE.to_string()
        } else if event.event == NEEDS_TRUST_EVENT {
            NEEDS_TRUST_STATE.to_string()
        } else {
            event
                .state
                .map(AgentState::as_str)
                .map(str::to_string)
                .or_else(|| current.as_ref().map(|current| current.state.clone()))
                .unwrap_or_else(|| AgentState::Idle.as_str().to_string())
        };
        let t_ms = now_ms();
        let transcript = if event.transcript.is_empty() {
            current
                .as_ref()
                .map(|current| current.transcript.clone())
                .unwrap_or_default()
        } else {
            event.transcript.clone()
        };
        let transcript_bytes = (!transcript.is_empty())
            .then(|| std::fs::metadata(&transcript).ok().map(|meta| meta.len()))
            .flatten();
        let line = EventLine {
            t_ms,
            event: event.event.clone(),
            state: agent_state.clone(),
            turn,
            tool: event.tool.clone(),
            notification: event.notification.clone(),
            transcript_bytes,
            detail: event.detail.clone(),
        };
        let mut text = serde_json::to_string(&line).map_err(std::io::Error::other)?;
        text.push('\n');
        file.write_all(text.as_bytes())?;
        let record = SessionState {
            schema: SCHEMA.into(),
            session_id: event.session_id.clone(),
            holder: identity.holder.clone(),
            project: identity.project.clone(),
            branch: identity.branch.clone(),
            role: identity.role.clone(),
            driver: identity.driver.clone(),
            state: agent_state,
            last_event: event.event.clone(),
            event_ms: t_ms,
            turn,
            turn_state,
            transcript,
        };
        let blob = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
        pom_paths::write_atomic(&dir.join(STATE_FILE), &blob, 0o644)?;
        Ok(record)
    })?;
    if previous.is_none() {
        index_session(state, identity, &event.session_id)?;
    }
    Ok(record)
}

fn index_session(state: &StateDir, identity: &Identity, session_id: &str) -> std::io::Result<()> {
    let path = index_path(state, &identity.project, &identity.branch);
    with_lock(&path.with_extension("lock"), |_| {
        let mut index: WorkspaceIndex = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let entry = IndexEntry {
            session_id: session_id.to_string(),
            role: identity.role.clone(),
            holder: identity.holder.clone(),
            driver: identity.driver.clone(),
        };
        if index.sessions.contains(&entry) {
            return Ok(());
        }
        index
            .sessions
            .retain(|known| known.session_id != session_id);
        index.sessions.push(entry);
        index.schema = SCHEMA.into();
        index.project = identity.project.clone();
        index.branch = identity.branch.clone();
        let blob = serde_json::to_vec(&index).map_err(std::io::Error::other)?;
        pom_paths::write_atomic(&path, &blob, 0o644)
    })
}

/// The sessions a workspace of `project` has recorded, oldest first.
pub fn workspace_sessions(state: &StateDir, project: &str, branch: &str) -> Vec<IndexEntry> {
    std::fs::read_to_string(index_path(state, project, branch))
        .ok()
        .and_then(|text| serde_json::from_str::<WorkspaceIndex>(&text).ok())
        .map(|index| index.sessions)
        .unwrap_or_default()
}

/// Every session of every workspace of `project` (read-only overviews).
pub fn project_sessions(state: &StateDir, project: &str) -> Vec<(String, IndexEntry)> {
    let Ok(entries) = std::fs::read_dir(state.path(INDEX_DIR).join(safe(project))) else {
        return Vec::new();
    };
    let mut out: Vec<(String, IndexEntry)> = entries
        .flatten()
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let index: WorkspaceIndex = serde_json::from_str(&text).ok()?;
            Some(
                index
                    .sessions
                    .into_iter()
                    .map(move |session| (index.branch.clone(), session)),
            )
        })
        .flatten()
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(role: &str, holder: &str) -> Identity {
        Identity {
            holder: holder.into(),
            role: role.into(),
            project: "myproject".into(),
            branch: "feat-login".into(),
            driver: "claude".into(),
        }
    }

    fn event(session: &str, name: &str, state: AgentState) -> SessionEvent {
        SessionEvent {
            session_id: session.into(),
            event: name.into(),
            state: Some(state),
            transcript: "/t.jsonl".into(),
            ..SessionEvent::default()
        }
    }

    #[test]
    fn two_sessions_of_one_workspace_keep_their_own_state_and_turns() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let main = identity("claude", "ws-myproject-feat-login-claude-raw");
        let reviewer = identity("reviewer", "ws-myproject-feat-login-claude-reviewer");
        record_event(
            &state,
            &main,
            &event("a", "UserPromptSubmit", AgentState::Thinking),
        )
        .expect("a1");
        record_event(&state, &main, &event("a", "Stop", AgentState::Idle)).expect("a1 end");
        record_event(
            &state,
            &main,
            &event("a", "UserPromptSubmit", AgentState::Thinking),
        )
        .expect("a2");
        let review = record_event(
            &state,
            &reviewer,
            &event("b", "UserPromptSubmit", AgentState::Thinking),
        )
        .expect("b1");
        let main_state = read_state(&state, "a").expect("a");
        assert_eq!(
            (
                main_state.turn,
                main_state.turn_state,
                main_state.state.as_str()
            ),
            (2, TurnState::Running, "thinking")
        );
        assert_eq!((review.turn, review.role.as_str()), (1, "reviewer"));
        let roles: Vec<String> = workspace_sessions(&state, "myproject", "feat-login")
            .into_iter()
            .map(|entry| entry.role)
            .collect();
        assert_eq!(roles, ["claude", "reviewer"]);
        assert_eq!(events(&state, "a").len(), 3);
    }

    #[test]
    fn the_same_branch_in_two_projects_is_two_workspaces() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let mut first = identity("claude", "ws-myproject-main-claude-raw");
        first.branch = "main".into();
        let mut second = first.clone();
        second.project = "other".into();
        second.holder = "ws-other-main-claude-raw".into();
        record_event(
            &state,
            &first,
            &event("one", "SessionStart", AgentState::Idle),
        )
        .expect("one");
        record_event(
            &state,
            &second,
            &event("two", "SessionStart", AgentState::Idle),
        )
        .expect("two");
        assert_eq!(workspace_sessions(&state, "myproject", "main").len(), 1);
        assert_eq!(
            workspace_sessions(&state, "other", "main")[0].session_id,
            "two"
        );
    }

    #[test]
    fn concurrent_hooks_never_lose_or_double_count_a_turn() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let main = identity("claude", "ws-myproject-feat-login-claude-raw");
        record_event(
            &state,
            &main,
            &event("s", "UserPromptSubmit", AgentState::Thinking),
        )
        .expect("turn");
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let (state, main) = (state.clone(), main.clone());
                std::thread::spawn(move || {
                    record_event(
                        &state,
                        &main,
                        &event("s", "PreToolUse", AgentState::ToolUse),
                    )
                })
            })
            .collect();
        for handle in handles {
            assert!(handle.join().is_ok_and(|result| result.is_ok()));
        }
        let lines = events(&state, "s");
        assert_eq!(lines.len(), 17);
        assert!(lines.iter().all(|line| line.turn == 1));
        record_event(&state, &main, &event("s", "Interrupted", AgentState::Idle)).expect("cancel");
        assert_eq!(
            read_state(&state, "s").expect("s").turn_state,
            TurnState::Cancelled
        );
    }
}
