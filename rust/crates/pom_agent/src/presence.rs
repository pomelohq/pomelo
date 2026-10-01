//! What the app shows about agent sessions it did not start itself: who drives one, what it waits to have
//! approved, and the one state a workspace's dot shows for all of its sessions.

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use serde_json::{json, Value};

use crate::hooks::AgentState;
use crate::lease::{log, read_lease, set_lease, Lease, LeaseClass};
use crate::policy::{answer_path, write_answer, Answer};
use crate::sessions::{events, read_state, workspace_sessions, SessionState};

/// The session a holder runs, as last recorded.
pub fn session_for_holder(state: &StateDir, holder: &str) -> Option<SessionState> {
    let entries = std::fs::read_dir(state.path("agents/sessions")).ok()?;
    entries
        .flatten()
        .filter_map(|entry| read_state(state, &entry.file_name().to_string_lossy()))
        .filter(|record| record.holder == holder)
        .max_by_key(|record| record.event_ms)
}

/// A tool call that waits for someone to approve it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub request: String,
    pub tool: String,
    /// The call's input, short, for a one-line banner.
    pub summary: String,
}

/// The latest approval a session waits for, if nobody answered it yet.
pub fn pending_approval(state: &StateDir, session_id: &str) -> Option<PendingApproval> {
    let lines = events(state, session_id);
    let asked = lines
        .iter()
        .rev()
        .find(|line| line.event == "permission_request")?;
    let request = asked.detail["request"].as_str()?.to_string();
    let decided = lines.iter().any(|line| {
        line.event == "permission_decision"
            && line.detail["request"] == request.as_str()
            && line.t_ms >= asked.t_ms
    });
    if decided || answer_path(state, session_id, &request).exists() {
        return None;
    }
    let input = &asked.detail["input"];
    let summary = input
        .get("command")
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("url"))
        .and_then(Value::as_str)
        .map_or_else(|| input.to_string(), str::to_string);
    let mut summary: String = summary.lines().next().unwrap_or_default().to_string();
    if summary.chars().count() > 80 {
        summary = summary.chars().take(80).collect::<String>() + "...";
    }
    Some(PendingApproval {
        request,
        tool: asked.detail["tool"]
            .as_str()
            .unwrap_or("a tool")
            .to_string(),
        summary,
    })
}

/// Who drives the session `holder` runs, in words, when it is not the person using the app.
pub fn driven_by(state: &StateDir, holder: &str) -> Option<String> {
    let lease = read_lease(state, holder);
    match lease.class {
        LeaseClass::Human => None,
        LeaseClass::Orchestrator => Some("an orchestrator".into()),
        LeaseClass::Agent => Some(format!("the agent in {}", lease.by)),
    }
}

/// The person at the app takes the session over; whoever drove it gets it back on release.
pub fn take_over(
    state: &StateDir,
    holders: &SocketDir,
    record: &SessionState,
) -> std::io::Result<()> {
    let before = read_lease(state, &record.holder);
    if before.class == LeaseClass::Human {
        return Ok(());
    }
    let mut person = Lease::person();
    person.previous = Some(Box::new(before));
    set_lease(
        state,
        holders,
        &record.project,
        &record.branch,
        &record.holder,
        &record.role,
        person,
    )
    .map(|_| ())
}

/// The person at the app answers a pending approval.
pub fn answer_as_person(
    state: &StateDir,
    record: &SessionState,
    request: &str,
    allow: bool,
) -> std::io::Result<()> {
    let answer = Answer {
        allow,
        decided_by: "person".into(),
        reason: String::new(),
        always: false,
    };
    write_answer(state, &record.session_id, request, &answer)?;
    log(
        state,
        &record.project,
        &record.branch,
        json!({"event": "permission_answer", "role": record.role, "request": request, "scope": if allow { "once" } else { "deny" }, "decided_by": "person"}),
    )
}

fn urgency(state: AgentState) -> u8 {
    match state {
        AgentState::AwaitingInput => 4,
        AgentState::ToolUse | AgentState::Thinking => 3,
        AgentState::Compacting => 2,
        AgentState::Idle => 1,
    }
}

/// The most urgent state of a workspace's live agent sessions (a question beats work beats idle); `None`
/// when no recorded session of it runs.
pub fn workspace_agent_state(
    state: &StateDir,
    holders: &SocketDir,
    project: &str,
    branch: &str,
) -> Option<AgentState> {
    workspace_sessions(state, project, branch)
        .into_iter()
        .filter(|entry| holders.holder_alive(&entry.holder))
        .filter_map(|entry| read_state(state, &entry.session_id))
        .filter_map(|record| AgentState::parse(&record.state))
        .max_by_key(|state| urgency(*state))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::identity::Identity;
    use crate::sessions::{record_event, SessionEvent};

    fn record(
        state: &StateDir,
        holder: &str,
        role: &str,
        session: &str,
        event: &str,
        agent_state: AgentState,
        detail: Value,
    ) {
        let identity = Identity {
            holder: holder.into(),
            role: role.into(),
            project: "myproject".into(),
            branch: "feat-login".into(),
            driver: "claude".into(),
        };
        let event = SessionEvent {
            session_id: session.into(),
            event: event.into(),
            state: Some(agent_state),
            detail,
            ..SessionEvent::default()
        };
        record_event(state, &identity, &event).expect("event");
    }

    #[test]
    fn the_dot_shows_the_most_urgent_live_session_of_the_workspace() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let holders = SocketDir::new(temp.path().join("s"));
        std::fs::create_dir_all(temp.path().join("s")).expect("dir");
        let me = std::process::id();
        for (name, role, session, agent_state) in [
            (
                "ws-myproject-feat-login-claude-raw",
                "claude",
                "a",
                AgentState::Idle,
            ),
            (
                "ws-myproject-feat-login-claude-reviewer",
                "reviewer",
                "b",
                AgentState::AwaitingInput,
            ),
        ] {
            record(
                &state,
                name,
                role,
                session,
                "SessionStart",
                agent_state,
                Value::Null,
            );
            std::fs::write(holders.pidfile(name), format!("{me}\n{name}\n")).expect("pidfile");
        }
        assert_eq!(
            workspace_agent_state(&state, &holders, "myproject", "feat-login"),
            Some(AgentState::AwaitingInput),
            "the reviewer's question shows though the main agent is idle"
        );
        std::fs::remove_file(holders.pidfile("ws-myproject-feat-login-claude-reviewer"))
            .expect("gone");
        assert_eq!(
            workspace_agent_state(&state, &holders, "myproject", "feat-login"),
            Some(AgentState::Idle)
        );
        assert_eq!(
            workspace_agent_state(&state, &holders, "myproject", "main"),
            None
        );
    }

    #[test]
    fn a_pending_approval_shows_until_someone_answers_it() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let holder = "ws-myproject-feat-login-claude-reviewer";
        record(
            &state,
            holder,
            "reviewer",
            "b",
            "permission_request",
            AgentState::AwaitingInput,
            json!({"request": "r1", "tool": "Bash", "input": {"command": "rm -rf build\nmore"}}),
        );
        let pending = pending_approval(&state, "b").expect("pending");
        assert_eq!(
            (
                pending.request.as_str(),
                pending.tool.as_str(),
                pending.summary.as_str()
            ),
            ("r1", "Bash", "rm -rf build")
        );
        let session = session_for_holder(&state, holder).expect("session");
        answer_as_person(&state, &session, "r1", true).expect("answer");
        assert_eq!(pending_approval(&state, "b"), None);
    }

    #[test]
    fn taking_over_from_the_app_hands_the_session_to_the_person() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let holders = SocketDir::new(temp.path().join("s"));
        let holder = "ws-myproject-feat-login-claude-reviewer";
        record(
            &state,
            holder,
            "reviewer",
            "b",
            "SessionStart",
            AgentState::Idle,
            Value::Null,
        );
        let session = session_for_holder(&state, holder).expect("session");
        assert_eq!(driven_by(&state, holder), None);
        set_lease(
            &state,
            &holders,
            "myproject",
            "feat-login",
            holder,
            "reviewer",
            Lease::for_caller(&crate::caller::Caller::Operator),
        )
        .expect("lease");
        assert_eq!(
            driven_by(&state, holder).as_deref(),
            Some("an orchestrator")
        );
        take_over(&state, &holders, &session).expect("take over");
        assert_eq!(driven_by(&state, holder), None);
        assert_eq!(holders.input_lease(holder), None);
    }
}
