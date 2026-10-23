//! The agent's own permission prompts, answered from outside its terminal. The `PermissionRequest` hook
//! records each prompt under an id of its own. While an orchestrator or an agent holds the session's lease
//! nobody sits at its terminal, so the hook itself waits for an answer and hands it to the agent; once a
//! person drives, or the wait runs out, the agent draws its dialog and an answer is typed into it.

use std::time::{Duration, Instant};

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::hooks::AgentState;
use crate::identity::Identity;
use crate::lease::{read_lease, LeaseClass};
use crate::policy::{answer_path, request_id, take_answer, Answer};
use crate::sessions::{events, now_ms, record_event, EventLine, SessionEvent};

pub(crate) const REQUEST_EVENT: &str = "PermissionRequest";
pub(crate) const DECISION_EVENT: &str = "permission_decision";
pub(crate) const RELEASED_EVENT: &str = "permission_released";
const POLICY_REQUEST_EVENT: &str = "permission_request";
/// How long the hook holds a prompt for an answer before the agent draws its dialog.
pub(crate) const HOOK_WAIT: Duration = Duration::from_secs(30 * 60);
const HOOK_POLL: Duration = Duration::from_millis(200);

/// Who answers a pending prompt.
pub const ANSWER_BY_HOOK: &str = "hook";
pub const ANSWER_BY_TERMINAL: &str = "terminal";
pub const ANSWER_BY_POLICY: &str = "policy";

/// A tool call that waits for someone to allow or deny it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionPrompt {
    /// Stable for this one prompt; an answer naming another id never reaches it.
    pub id: String,
    /// `agent`: the agent's own prompt; `policy`: the workspace policy answered ask.
    pub kind: String,
    pub tool_name: String,
    pub tool_input: Value,
    /// The input, short, on one line.
    pub summary: String,
    pub since_ms: u64,
    /// `hook` (pom holds the prompt for an answer), `terminal` (the agent's dialog is drawn) or `policy`.
    pub answer_by: String,
}

/// The key that tells one tool call from another inside a session, the same for its prompt and its result.
pub(crate) fn call_key(tool: &Value, input: &Value) -> String {
    request_id("", tool, input)
}

/// What a `PermissionRequest` hook records, with a fresh id and who will answer it.
pub(crate) fn request_detail(state: &StateDir, identity: Option<&Identity>, body: &Value) -> Value {
    let field = |key: &str| body.get(key).cloned().unwrap_or(Value::Null);
    let session = body
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let unattended = identity
        .is_some_and(|identity| read_lease(state, &identity.holder).class != LeaseClass::Human);
    json!({
        "request": request_id(session, &field("tool_name"), &json!([field("tool_input"), now_ms()])),
        "call": call_key(&field("tool_name"), &field("tool_input")),
        "tool": field("tool_name"),
        "input": field("tool_input"),
        "answer_by": if unattended { ANSWER_BY_HOOK } else { ANSWER_BY_TERMINAL },
    })
}

fn summary(input: &Value) -> String {
    let text = input
        .get("command")
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("url"))
        .and_then(Value::as_str)
        .map_or_else(|| input.to_string(), str::to_string);
    let mut line: String = text.lines().next().unwrap_or_default().to_string();
    if line.chars().count() > 80 {
        line = line.chars().take(80).collect::<String>() + "...";
    }
    line
}

/// Whether `line`, after a prompt was asked, means that prompt is no longer waiting.
fn settles(line: &EventLine, request: &str, call: &str) -> bool {
    match line.event.as_str() {
        "Notification" | RELEASED_EVENT => false,
        DECISION_EVENT => line.detail["request"] == request,
        // Parallel tool calls report their own results while this one still waits.
        "PreToolUse" | "PostToolUse" => line.detail["call"] == call,
        _ => true,
    }
}

fn agent_prompt(lines: &[EventLine]) -> Option<(u64, PermissionPrompt)> {
    let at = lines
        .iter()
        .rposition(|line| line.event == REQUEST_EVENT && line.detail["request"].is_string())?;
    let asked = &lines[at];
    let request = asked.detail["request"].as_str().unwrap_or_default();
    let call = asked.detail["call"].as_str().unwrap_or_default();
    let later = &lines[at + 1..];
    if later.iter().any(|line| settles(line, request, call)) {
        return None;
    }
    let released = later
        .iter()
        .any(|line| line.event == RELEASED_EVENT && line.detail["request"] == request);
    let answer_by = match asked.detail["answer_by"].as_str() {
        Some(ANSWER_BY_HOOK) if !released => ANSWER_BY_HOOK,
        _ => ANSWER_BY_TERMINAL,
    };
    Some((
        asked.t_ms,
        PermissionPrompt {
            id: request.to_string(),
            kind: "agent".into(),
            tool_name: asked.detail["tool"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            tool_input: asked.detail["input"].clone(),
            summary: summary(&asked.detail["input"]),
            since_ms: asked.t_ms,
            answer_by: answer_by.into(),
        },
    ))
}

fn policy_prompt(
    state: &StateDir,
    session_id: &str,
    lines: &[EventLine],
) -> Option<(u64, PermissionPrompt)> {
    let pending = crate::presence::pending_approval(state, session_id)?;
    let asked = lines.iter().rev().find(|line| {
        line.event == POLICY_REQUEST_EVENT && line.detail["request"] == pending.request.as_str()
    })?;
    Some((
        asked.t_ms,
        PermissionPrompt {
            id: pending.request,
            kind: "policy".into(),
            tool_name: pending.tool,
            tool_input: asked.detail["input"].clone(),
            summary: pending.summary,
            since_ms: asked.t_ms,
            answer_by: ANSWER_BY_POLICY.into(),
        },
    ))
}

/// The prompt a session waits on now, the newest of its own and its policy's.
pub fn pending_permission(state: &StateDir, session_id: &str) -> Option<PermissionPrompt> {
    let lines = events(state, session_id);
    let agent = agent_prompt(&lines);
    let policy = policy_prompt(state, session_id, &lines);
    match (agent, policy) {
        (Some(agent), Some(policy)) => Some(if policy.0 > agent.0 {
            policy.1
        } else {
            agent.1
        }),
        (one, other) => one.or(other).map(|(_, prompt)| prompt),
    }
}

/// Whether the session ever asked the workspace policy for `request`.
pub(crate) fn is_policy_request(state: &StateDir, session_id: &str, request: &str) -> bool {
    events(state, session_id)
        .iter()
        .any(|line| line.event == POLICY_REQUEST_EVENT && line.detail["request"] == request)
}

fn record(
    state: &StateDir,
    identity: &Identity,
    session_id: &str,
    event: &str,
    agent_state: AgentState,
    detail: Value,
) {
    let event = SessionEvent {
        session_id: session_id.to_string(),
        event: event.into(),
        state: Some(agent_state),
        detail,
        ..SessionEvent::default()
    };
    if let Err(error) = record_event(state, identity, &event) {
        eprintln!("claude-hook: {error}");
    }
}

/// The decision a held prompt's hook prints, or none to let the agent draw its dialog. Waits for an answer
/// while nobody sits at the terminal; a person taking the session over, or the wait running out, lets go.
pub(crate) fn await_answer(
    state: &StateDir,
    identity: &Identity,
    session_id: &str,
    request: &str,
    wait: Duration,
) -> Option<String> {
    let started = Instant::now();
    loop {
        if let Some(answer) = take_answer(state, session_id, request) {
            record(
                state,
                identity,
                session_id,
                DECISION_EVENT,
                if answer.allow {
                    AgentState::ToolUse
                } else {
                    AgentState::Thinking
                },
                json!({"request": request, "allow": answer.allow, "decided_by": answer.decided_by, "via": ANSWER_BY_HOOK}),
            );
            return Some(decision_output(&answer));
        }
        let why = if read_lease(state, &identity.holder).class == LeaseClass::Human {
            "person"
        } else if started.elapsed() >= wait {
            "timeout"
        } else {
            std::thread::sleep(HOOK_POLL);
            continue;
        };
        record(
            state,
            identity,
            session_id,
            RELEASED_EVENT,
            AgentState::AwaitingInput,
            json!({"request": request, "why": why}),
        );
        return None;
    }
}

/// What the hook prints so the agent applies an answer.
pub(crate) fn decision_output(answer: &Answer) -> String {
    let decision = if answer.allow {
        json!({"behavior": "allow"})
    } else {
        let message = if answer.reason.is_empty() {
            format!("{} denied this tool call", answer.decided_by)
        } else {
            answer.reason.clone()
        };
        json!({"behavior": "deny", "message": message})
    };
    json!({"hookSpecificOutput": {"hookEventName": REQUEST_EVENT, "decision": decision}})
        .to_string()
}

/// Whether the hook took `request`'s answer, within `wait`.
pub(crate) fn answer_taken(
    state: &StateDir,
    session_id: &str,
    request: &str,
    wait: Duration,
) -> bool {
    let started = Instant::now();
    loop {
        let taken = events(state, session_id)
            .iter()
            .any(|line| line.event == DECISION_EVENT && line.detail["request"] == request);
        if taken {
            return true;
        }
        if started.elapsed() >= wait {
            if let Err(error) = std::fs::remove_file(answer_path(state, session_id, request)) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("agent: {error}");
                }
            }
            return false;
        }
        std::thread::sleep(HOOK_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::policy::write_answer;

    fn identity() -> Identity {
        Identity {
            holder: "ws-myproject-feat-login-claude-reviewer".into(),
            role: "reviewer".into(),
            project: "myproject".into(),
            branch: "feat-login".into(),
            driver: "claude".into(),
        }
    }

    fn ask(state: &StateDir, tool: &str, input: Value) -> PermissionPrompt {
        let body = json!({"session_id": "s", "hook_event_name": REQUEST_EVENT, "tool_name": tool, "tool_input": input});
        let detail = request_detail(state, Some(&identity()), &body);
        record(
            state,
            &identity(),
            "s",
            REQUEST_EVENT,
            AgentState::AwaitingInput,
            detail,
        );
        pending_permission(state, "s").expect("pending")
    }

    fn tool_event(state: &StateDir, event: &str, tool: &str, input: Value) {
        record(
            state,
            &identity(),
            "s",
            event,
            AgentState::ToolUse,
            json!({"call": call_key(&json!(tool), &input)}),
        );
    }

    #[test]
    fn a_prompt_waits_until_its_own_call_moves_on_and_each_prompt_gets_its_own_id() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let input = json!({"command": "rm -rf build", "description": "clean"});
        let first = ask(&state, "Bash", input.clone());
        assert_eq!(
            (
                first.kind.as_str(),
                first.tool_name.as_str(),
                first.summary.as_str(),
                first.answer_by.as_str()
            ),
            ("agent", "Bash", "rm -rf build", ANSWER_BY_TERMINAL),
            "a person's session draws the dialog"
        );
        record(
            &state,
            &identity(),
            "s",
            "Notification",
            AgentState::AwaitingInput,
            Value::Null,
        );
        tool_event(&state, "PostToolUse", "Read", json!({"file_path": "/a"}));
        assert_eq!(
            pending_permission(&state, "s").map(|prompt| prompt.id),
            Some(first.id.clone()),
            "another call's result and the reminder leave it waiting"
        );
        tool_event(&state, "PostToolUse", "Bash", input.clone());
        assert_eq!(pending_permission(&state, "s"), None);

        std::thread::sleep(Duration::from_millis(2));
        let second = ask(&state, "Bash", input);
        assert_ne!(
            second.id, first.id,
            "the same call asked again is another prompt"
        );
        record(
            &state,
            &identity(),
            "s",
            "Stop",
            AgentState::Idle,
            Value::Null,
        );
        assert_eq!(pending_permission(&state, "s"), None);
    }

    #[test]
    fn an_unattended_prompt_is_held_for_an_answer_and_let_go_when_a_person_drives() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let holders = pom_ptyhost::SocketDir::new(temp.path().join("s"));
        let identity = identity();
        crate::lease::set_lease(
            &state,
            &holders,
            "myproject",
            "feat-login",
            &identity.holder,
            "reviewer",
            crate::lease::Lease::for_caller(&crate::caller::Caller::Operator),
        )
        .expect("lease");
        let prompt = ask(&state, "Bash", json!({"command": "make"}));
        assert_eq!(prompt.answer_by, ANSWER_BY_HOOK);
        let answer = Answer {
            allow: false,
            decided_by: "operator".into(),
            reason: "not now".into(),
            always: false,
        };
        write_answer(&state, "s", &prompt.id, &answer).expect("answer");
        let output = await_answer(&state, &identity, "s", &prompt.id, Duration::from_secs(5))
            .expect("decided");
        let output: Value = serde_json::from_str(&output).expect("json");
        assert_eq!(output["hookSpecificOutput"]["hookEventName"], REQUEST_EVENT);
        assert_eq!(output["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert_eq!(
            output["hookSpecificOutput"]["decision"]["message"],
            "not now"
        );
        assert_eq!(pending_permission(&state, "s"), None);
        assert!(answer_taken(&state, "s", &prompt.id, Duration::ZERO));

        let held = ask(&state, "Bash", json!({"command": "make test"}));
        let mut person = crate::lease::Lease::person();
        person.previous = Some(Box::new(read_lease(&state, &identity.holder)));
        crate::lease::set_lease(
            &state,
            &holders,
            "myproject",
            "feat-login",
            &identity.holder,
            "reviewer",
            person,
        )
        .expect("takeover");
        assert_eq!(
            await_answer(&state, &identity, "s", &held.id, Duration::from_secs(5)),
            None
        );
        let after = pending_permission(&state, "s").expect("still pending");
        assert_eq!(
            (after.id, after.answer_by.as_str()),
            (held.id, ANSWER_BY_TERMINAL)
        );
    }
}
