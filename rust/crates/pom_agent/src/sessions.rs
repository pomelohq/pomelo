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
pub(crate) const SESSIONS_DIR: &str = "agents/sessions";
const INDEX_DIR: &str = "agents/index";
const EVENTS_FILE: &str = "events.ndjson";
pub(crate) const STATE_FILE: &str = "state.json";

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
    /// Why the session is in its state; files written before this field derive it from `last_event`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub const PENDING_APPROVAL: &str = "pending_approval";
pub const IDLE_NOTIFICATION: &str = "idle_notification";
pub const STOP: &str = "stop";
pub const INTERRUPTED: &str = "interrupted";
pub const STALE: &str = "stale";
/// pom saw a sent prompt start its turn before the agent reported working on it.
pub const SUBMITTED: &str = "submitted";
pub const SUBMITTED_EVENT: &str = "Submitted";
/// The agent's idle reminder needs a minute of idling, so one this soon after a prompt is about the idle time
/// before it and must not end the turn the prompt just started.
const IDLE_REMINDER_GRACE_MS: u64 = 10_000;

const AWAITING_INPUT: &str = "awaiting_input";

/// Every `state` a session view reports. The first seven come from the agent and pom's launch; `died`,
/// `ended` and `unknown` only from a view, for a session whose holder is gone or never reported.
pub const STATES: [&str; 10] = [
    STARTING_STATE,
    NEEDS_TRUST_STATE,
    "idle",
    "thinking",
    "tool_use",
    "compacting",
    AWAITING_INPUT,
    "died",
    "ended",
    "unknown",
];
/// Every `reason` a session view reports; `null` when none applies.
pub const REASONS: [&str; 6] = [
    PENDING_APPROVAL,
    IDLE_NOTIFICATION,
    STOP,
    INTERRUPTED,
    STALE,
    SUBMITTED,
];
/// Every `phase`: what a client does next, whatever the exact state.
pub const PHASES: [&str; 4] = ["working", "waiting", "idle", "gone"];

/// Whether a turn is in progress: the agent works on it, or it waits on a permission inside it.
pub fn turn_running(state: &str, reason: Option<&str>) -> bool {
    match state {
        "thinking" | "tool_use" | "compacting" => true,
        AWAITING_INPUT => reason == Some(PENDING_APPROVAL),
        _ => false,
    }
}

/// The coarse phase of a state: `working` (a turn runs), `waiting` (it cannot go on until someone acts or it
/// finishes starting), `idle` (it takes a turn now) or `gone` (its holder ended).
pub fn phase(state: &str) -> &'static str {
    match state {
        "thinking" | "tool_use" | "compacting" => "working",
        "idle" => "idle",
        "died" | "ended" => "gone",
        _ => "waiting",
    }
}

/// Why an event left the session in `state`, for the states a driver has to tell apart.
pub(crate) fn reason_for(event: &str, state: &str) -> Option<&'static str> {
    match event {
        "Stop" => Some(STOP),
        "Interrupted" => Some(INTERRUPTED),
        "permission_request" | "PermissionRequest" => Some(PENDING_APPROVAL),
        "Notification" if state == AWAITING_INPUT => Some(PENDING_APPROVAL),
        "Notification" => Some(IDLE_NOTIFICATION),
        SUBMITTED_EVENT => Some(SUBMITTED),
        _ => None,
    }
}

impl SessionState {
    /// The state and reason a driver acts on. A file from before `reason` existed may say
    /// `awaiting_input` for the idle reminder: only one inside a running turn is a real approval.
    pub fn status(&self) -> (String, Option<String>) {
        if let Some(reason) = &self.reason {
            return (self.state.clone(), Some(reason.clone()));
        }
        let legacy_idle_reminder = self.last_event == "Notification"
            && self.state == AWAITING_INPUT
            && self.turn_state != TurnState::Running;
        if legacy_idle_reminder {
            return (
                AgentState::Idle.as_str().to_string(),
                Some(IDLE_NOTIFICATION.into()),
            );
        }
        (
            self.state.clone(),
            reason_for(&self.last_event, &self.state).map(str::to_string),
        )
    }
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

pub(crate) fn index_path(state: &StateDir, project: &str, branch: &str) -> PathBuf {
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
    record_event_unless(state, identity, event, |_| false)
        .map(|recorded| recorded.unwrap_or_default())
}

/// Once a send saw turn `turn` start: if the session does not show it running (an idle event slipped in after
/// the prompt), records pom's `Submitted` so it reads `thinking` until the agent's next hook. A turn that
/// already ended or moved on is left alone, so a Stop always wins. Whether it recorded.
pub fn mark_submitted(
    state: &StateDir,
    identity: &Identity,
    session_id: &str,
    turn: u64,
) -> std::io::Result<bool> {
    let event = SessionEvent {
        session_id: session_id.to_string(),
        event: SUBMITTED_EVENT.into(),
        state: Some(AgentState::Thinking),
        ..SessionEvent::default()
    };
    let recorded = record_event_unless(state, identity, &event, |current| {
        current.is_none_or(|current| {
            let (status, reason) = current.status();
            current.turn != turn
                || current.turn_state != TurnState::Running
                || turn_running(&status, reason.as_deref())
        })
    })?;
    Ok(recorded.is_some())
}

/// `record_event`, unless `skip` says otherwise of the current state, checked under the session's lock.
fn record_event_unless(
    state: &StateDir,
    identity: &Identity,
    event: &SessionEvent,
    skip: impl FnOnce(Option<&SessionState>) -> bool,
) -> std::io::Result<Option<SessionState>> {
    let dir = session_dir(state, &event.session_id);
    let previous = read_state(state, &event.session_id);
    let record = with_lock(&dir.join(EVENTS_FILE), |file| {
        let current = read_state(state, &event.session_id);
        if skip(current.as_ref()) {
            return Ok(None);
        }
        let lines: Vec<EventLine> = read_all(file)?
            .lines()
            .filter_map(|line| serde_json::from_str::<EventLine>(line).ok())
            .collect();
        let prompts = lines
            .iter()
            .filter(|line| line.event == "UserPromptSubmit")
            .count() as u64;
        let last_prompt_ms = lines
            .iter()
            .rev()
            .find(|line| line.event == "UserPromptSubmit")
            .map(|line| line.t_ms);
        let turn = if event.event == "UserPromptSubmit" {
            prompts + 1
        } else {
            prompts
        };
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
        let late_idle_reminder = event.event == "Notification"
            && event.state == Some(AgentState::Idle)
            && current
                .as_ref()
                .is_some_and(|current| current.turn_state == TurnState::Running)
            && last_prompt_ms.is_some_and(|at| t_ms.saturating_sub(at) < IDLE_REMINDER_GRACE_MS);
        let agent_state = match (&current, late_idle_reminder) {
            (Some(current), true) => current.state.clone(),
            _ => agent_state,
        };
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
        if let (Some(current), true) = (&current, late_idle_reminder) {
            return Ok(Some(current.clone()));
        }
        let reason = reason_for(&event.event, &agent_state).map(str::to_string);
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
            reason,
            transcript,
        };
        let blob = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
        pom_paths::write_atomic(&dir.join(STATE_FILE), &blob, 0o644)?;
        Ok(Some(record))
    })?;
    if previous.is_none() && record.is_some() {
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
    fn a_sent_turn_reads_running_until_its_stop_even_after_an_idle_event_slips_in() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let main = identity("claude", "ws-myproject-feat-login-claude-raw");
        let running = |state: &StateDir| {
            let record = read_state(state, "a").expect("a");
            let (status, reason) = record.status();
            (
                status.clone(),
                reason.clone(),
                turn_running(&status, reason.as_deref()),
            )
        };
        record_event(
            &state,
            &main,
            &event("a", "UserPromptSubmit", AgentState::Thinking),
        )
        .expect("prompt");
        assert!(
            !mark_submitted(&state, &main, "a", 1).expect("mark"),
            "the hook already says it runs"
        );

        let reminder = SessionEvent {
            notification: Some("idle_prompt".into()),
            ..event("a", "Notification", AgentState::Idle)
        };
        record_event(&state, &main, &reminder).expect("late reminder");
        assert_eq!(running(&state), ("thinking".into(), None, true));
        assert_eq!(events(&state, "a").len(), 2, "the reminder is still logged");

        record_event(&state, &main, &event("a", "SessionStart", AgentState::Idle)).expect("start");
        assert!(!running(&state).2);
        assert!(mark_submitted(&state, &main, "a", 1).expect("mark"));
        assert_eq!(
            running(&state),
            ("thinking".into(), Some(SUBMITTED.into()), true)
        );
        assert_eq!(read_state(&state, "a").expect("a").turn, 1);

        record_event(&state, &main, &event("a", "Stop", AgentState::Idle)).expect("stop");
        assert!(!mark_submitted(&state, &main, "a", 1).expect("mark after stop"));
        assert_eq!(running(&state), ("idle".into(), Some(STOP.into()), false));

        record_event(&state, &main, &reminder).expect("reminder after the turn");
        assert_eq!(
            running(&state),
            ("idle".into(), Some(IDLE_NOTIFICATION.into()), false)
        );
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

    #[test]
    fn each_state_says_why_and_a_cleared_question_drops_the_reason() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let main = identity("claude", "ws-myproject-feat-login-claude-raw");
        let step = |name: &str, agent_state: AgentState| {
            record_event(&state, &main, &event("s", name, agent_state))
                .expect("event")
                .status()
        };
        let reason = |status: (String, Option<String>)| (status.0, status.1.unwrap_or_default());
        assert_eq!(
            reason(step("UserPromptSubmit", AgentState::Thinking)),
            ("thinking".into(), String::new())
        );
        assert_eq!(
            reason(step("PermissionRequest", AgentState::AwaitingInput)),
            ("awaiting_input".into(), PENDING_APPROVAL.into())
        );
        assert_eq!(
            reason(step("PostToolUse", AgentState::ToolUse)),
            ("tool_use".into(), String::new())
        );
        assert_eq!(
            reason(step("Stop", AgentState::Idle)),
            ("idle".into(), STOP.into())
        );
        assert_eq!(
            reason(step("Notification", AgentState::Idle)),
            ("idle".into(), IDLE_NOTIFICATION.into())
        );
        assert_eq!(
            read_state(&state, "s").expect("s").reason.as_deref(),
            Some(IDLE_NOTIFICATION)
        );
    }

    #[test]
    fn a_state_file_without_a_reason_derives_it_and_frees_an_old_idle_reminder() {
        let legacy = |last_event: &str, state: &str, turn_state: TurnState| {
            let text = serde_json::json!({
                "schema": SCHEMA, "session_id": "s", "holder": "h", "project": "myproject",
                "branch": "feat-login", "role": "claude", "driver": "claude", "state": state,
                "last_event": last_event, "event_ms": 1, "turn": 3, "turn_state": turn_state,
            })
            .to_string();
            let record: SessionState = serde_json::from_str(&text).expect("old file");
            assert_eq!(record.reason, None);
            record.status()
        };
        let status = |state: &str, reason: &str| (state.to_string(), Some(reason.to_string()));
        assert_eq!(
            legacy("Notification", "awaiting_input", TurnState::Ended),
            status("idle", IDLE_NOTIFICATION),
            "the idle reminder after a turn read as a permission prompt"
        );
        assert_eq!(
            legacy("Notification", "awaiting_input", TurnState::Running),
            status("awaiting_input", PENDING_APPROVAL)
        );
        assert_eq!(
            legacy("permission_request", "awaiting_input", TurnState::Running),
            status("awaiting_input", PENDING_APPROVAL)
        );
        assert_eq!(
            legacy("Stop", "idle", TurnState::Ended),
            status("idle", STOP)
        );
        assert_eq!(
            legacy("Interrupted", "idle", TurnState::Cancelled),
            status("idle", INTERRUPTED)
        );
        assert_eq!(
            legacy("PreToolUse", "tool_use", TurnState::Running),
            ("tool_use".to_string(), None)
        );
    }

    #[test]
    fn every_state_has_a_phase_and_only_work_or_a_pending_approval_runs_a_turn() {
        let phases: Vec<(&str, &str, bool)> = STATES
            .iter()
            .map(|state| {
                let reason = (*state == AWAITING_INPUT).then_some(PENDING_APPROVAL);
                (*state, phase(state), turn_running(state, reason))
            })
            .collect();
        assert_eq!(
            phases,
            [
                ("starting", "waiting", false),
                ("needs_trust", "waiting", false),
                ("idle", "idle", false),
                ("thinking", "working", true),
                ("tool_use", "working", true),
                ("compacting", "working", true),
                ("awaiting_input", "waiting", true),
                ("died", "gone", false),
                ("ended", "gone", false),
                ("unknown", "waiting", false),
            ]
        );
        assert!(!turn_running(AWAITING_INPUT, None));
        assert!(phases.iter().all(|(_, phase, _)| PHASES.contains(phase)));
    }
}
