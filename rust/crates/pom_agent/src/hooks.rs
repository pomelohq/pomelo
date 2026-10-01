//! Claude Code reports what it is doing through hooks: every event runs `claude-hook`, which maps it to
//! a state and writes `agents/state-<branch>.json` for the workspace the session runs in. The app
//! watches that folder. File names, contents and the event mapping are the previous core's.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use pom_paths::StateDir;
use serde_json::{json, Map, Value};

use crate::claude::write_wrapper;
use crate::identity::Identity;
use crate::policy::{
    decide, hook_output, workspace_policy, Decision, Driver, PolicyLookup, Verdict,
};
use crate::sessions::{record_event as record_session_event, SessionEvent};

const HOOK_WRAPPER: &str = "claude-hook";
const SESSION_WRAPPER: &str = "claude-session-hook";
const POLICY_WRAPPER: &str = "claude-policy-hook";
const SESSION_FLAG: &str = "--session";
const HOOK_EVENTS: [&str; 8] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PreCompact",
    "Stop",
    "SessionEnd",
    "Notification",
];
const AGENTS_DIR: &str = "agents";
const WORKSPACE_PREFIX: &str = "workspace--";
/// A working state not refreshed for this long belongs to a session that died without saying so.
const STALE_WORKING: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentState {
    Idle,
    Thinking,
    ToolUse,
    Compacting,
    AwaitingInput,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Thinking => "thinking",
            AgentState::ToolUse => "tool_use",
            AgentState::Compacting => "compacting",
            AgentState::AwaitingInput => "awaiting_input",
        }
    }

    pub fn parse(text: &str) -> Option<AgentState> {
        Some(match text {
            "idle" | "stopped" => AgentState::Idle,
            "thinking" => AgentState::Thinking,
            "tool_use" => AgentState::ToolUse,
            "compacting" => AgentState::Compacting,
            "awaiting_input" => AgentState::AwaitingInput,
            _ => return None,
        })
    }

    pub fn is_working(self) -> bool {
        matches!(self, AgentState::Thinking | AgentState::ToolUse)
    }
}

/// The state a hook event means. Only a permission or elicitation prompt really blocks on the user;
/// Claude's idle reminder is also a notification and must not light every idle agent up.
pub fn event_state(event: &str, notification: &str) -> Option<AgentState> {
    Some(match event {
        "SessionStart" | "Stop" | "SessionEnd" => AgentState::Idle,
        "UserPromptSubmit" => AgentState::Thinking,
        "PreToolUse" | "PostToolUse" | "PostToolBatch" => AgentState::ToolUse,
        "PreCompact" => AgentState::Compacting,
        "Notification" if notification == "permission" => AgentState::AwaitingInput,
        "Notification" => AgentState::Idle,
        _ => return None,
    })
}

/// The branch of the `workspace--<branch>` folder a session runs in.
pub fn branch_from_cwd(cwd: &Path) -> Option<String> {
    cwd.components().find_map(|component| match component {
        Component::Normal(name) => name
            .to_str()?
            .strip_prefix(WORKSPACE_PREFIX)
            .filter(|branch| !branch.is_empty())
            .map(str::to_string),
        _ => None,
    })
}

fn state_file(state: &StateDir, branch: &str) -> PathBuf {
    state
        .path(AGENTS_DIR)
        .join(format!("state-{}.json", branch.replace('/', "_")))
}

fn notification_kind(event: &str, input: &[u8], body: &Map<String, Value>) -> String {
    if event == "Notification" {
        let raw = String::from_utf8_lossy(input).to_lowercase();
        if raw.contains("permission") || raw.contains("elicitation") {
            "permission".into()
        } else {
            "idle".into()
        }
    } else {
        body.get("notification_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The session-log event one hook invocation's stdin carries; `None` without a session id.
pub fn session_event(input: &[u8]) -> Option<SessionEvent> {
    let Ok(Value::Object(body)) = serde_json::from_slice::<Value>(input) else {
        return None;
    };
    let field = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or_default();
    let session_id = field("session_id");
    if session_id.is_empty() {
        return None;
    }
    let event = field("hook_event_name");
    let notification = notification_kind(event, input, &body);
    let tool = body
        .get("tool_name")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(SessionEvent {
        session_id: session_id.to_string(),
        event: event.to_string(),
        state: event_state(event, &notification),
        transcript: field("transcript_path").to_string(),
        tool,
        notification: (event == "Notification").then_some(notification),
        detail: match event {
            "UserPromptSubmit" => json!({ "prompt_chars": field("prompt").chars().count() }),
            "Stop" => json!({ "stop_reason": "end_turn" }),
            "PreCompact" => json!({ "trigger": field("trigger") }),
            "SessionEnd" => json!({ "reason": field("reason") }),
            _ => Value::Null,
        },
    })
}

/// What one hook invocation's stdin means for its workspace; a session outside any workspace, or an
/// event without a state, writes nothing.
pub fn record_hook(
    state: &StateDir,
    input: &[u8],
) -> std::io::Result<Option<(String, AgentState)>> {
    let Ok(Value::Object(body)) = serde_json::from_slice::<Value>(input) else {
        return Ok(None);
    };
    let field = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or_default();
    let Some(branch) = branch_from_cwd(Path::new(field("cwd"))) else {
        return Ok(None);
    };
    let event = field("hook_event_name");
    let notification = notification_kind(event, input, &body);
    let Some(agent_state) = event_state(event, &notification) else {
        return Ok(None);
    };
    let path = state_file(state, &branch);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let blob = json!({ "branch": branch, "state": agent_state.as_str() }).to_string();
    pom_paths::write_atomic(&path, blob.as_bytes(), 0o644)?;
    Ok(Some((branch, agent_state)))
}

/// Handles `<binary> claude-hook [--session]`: the event arrives on stdin. Recording never fails the hook (a
/// broken hook shows up as an error inside the user's Claude session); the policy fails closed.
///
/// `--session` is the hook a launch attaches through its own settings. Without it the call comes from a
/// hook an older version installed in the user's settings: it only serves sessions that carry no identity,
/// so a session Pomelo launched is never recorded twice.
pub fn run(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some(HOOK_WRAPPER) {
        return None;
    }
    let attached = args.get(2).map(String::as_str) == Some(SESSION_FLAG);
    let mut input = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("claude-hook: {error}");
        return Some(if attached && is_pre_tool_use(&input) {
            2
        } else {
            0
        });
    }
    let state = StateDir::from_env();
    let identity = Identity::from_env();
    if identity.is_some() && !attached {
        return Some(0);
    }
    // A side agent works next to the main one; only the main agent's state is the workspace's dot.
    if std::env::var_os(crate::SIDE_AGENT_ENV).is_none() {
        if let Err(error) = record_hook(&state, &input) {
            eprintln!("claude-hook: {error}");
        }
    }
    if let (Some(identity), Some(event)) = (&identity, session_event(&input)) {
        if event.state.is_some() {
            if let Err(error) = record_session_event(&state, identity, &event) {
                eprintln!("claude-hook: {error}");
            }
        }
    }
    let decided = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pre_tool_use_decision(&state, identity.as_ref(), &input)
    }));
    match decided {
        Ok(Some(output)) => println!("{output}"),
        Ok(None) => {}
        Err(_) => {
            println!(
                "{}",
                hook_output(&Decision {
                    verdict: Verdict::Deny,
                    reason: "the agent policy check failed".into(),
                })
            );
        }
    }
    Some(0)
}

fn is_pre_tool_use(input: &[u8]) -> bool {
    serde_json::from_slice::<Value>(input)
        .is_ok_and(|body| body.get("hook_event_name").and_then(Value::as_str) == Some("PreToolUse"))
}

/// The hooks a launched session gets through its `--settings`, never written into the user's settings.
/// `PreToolUse` runs through a wrapper that blocks the call when pom itself cannot run or fails, because
/// a hook that merely errors lets the tool run.
pub fn session_hooks(state: &StateDir, binary: &Path) -> Option<Value> {
    let quoted = |path: PathBuf| format!("sh '{}'", path.to_string_lossy().replace('\'', r"'\''"));
    let recorder = write_wrapper(
        state,
        SESSION_WRAPPER,
        binary,
        &format!("{HOOK_WRAPPER} {SESSION_FLAG}"),
    )
    .map_err(|error| eprintln!("agent: hook wrapper: {error}"))
    .ok()?;
    let guard = write_policy_wrapper(state, binary)
        .map_err(|error| eprintln!("agent: policy wrapper: {error}"))
        .ok()?;
    let mut hooks = Map::new();
    for event in HOOK_EVENTS {
        let command = if event == "PreToolUse" {
            quoted(guard.clone())
        } else {
            quoted(recorder.clone())
        };
        hooks.insert(
            event.to_string(),
            json!([{ "matcher": "", "hooks": [{ "type": "command", "command": command }] }]),
        );
    }
    Some(Value::Object(hooks))
}

fn write_policy_wrapper(state: &StateDir, binary: &Path) -> std::io::Result<PathBuf> {
    let path = state.path(POLICY_WRAPPER);
    let quoted = binary.to_string_lossy().replace('\'', r"'\''");
    let body = format!(
        "#!/bin/sh\nBIN='{quoted}'\nif [ ! -x \"$BIN\" ]; then echo 'Pomelo is not reachable to check this tool call' >&2; exit 2; fi\n\"$BIN\" {HOOK_WRAPPER} {SESSION_FLAG} || {{ echo 'the agent policy check failed' >&2; exit 2; }}\n"
    );
    if std::fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        pom_paths::write_atomic(&path, body.as_bytes(), 0o755)?;
    }
    Ok(path)
}

/// The workspace policy's answer to a tool call, as the hook's output; `None` for any other event or a
/// workspace without a policy.
fn pre_tool_use_decision(
    state: &StateDir,
    identity: Option<&Identity>,
    input: &[u8],
) -> Option<String> {
    let body: Value = serde_json::from_slice(input).ok()?;
    if body.get("hook_event_name").and_then(Value::as_str) != Some("PreToolUse") {
        return None;
    }
    let cwd = body.get("cwd").and_then(Value::as_str)?;
    let decision = match workspace_policy(Path::new(cwd)) {
        PolicyLookup::None => return None,
        PolicyLookup::Unreadable(reason) => Decision::deny(reason),
        PolicyLookup::Found(policy) => {
            let driven = identity.is_some_and(|identity| {
                crate::lease::read_lease(state, &identity.holder).class
                    != crate::lease::LeaseClass::Human
            });
            let driver = if driven {
                Driver::Orchestrator
            } else {
                Driver::Person
            };
            decide(state, identity, &body, &policy, driver)
        }
    };
    Some(hook_output(&decision))
}

/// One workspace's agent as last reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStatus {
    pub branch: String,
    pub state: AgentState,
}

/// Every workspace's reported agent state. A working state gone quiet past the stale limit reads as
/// idle; a pending question stays until answered.
pub fn read_states(state: &StateDir) -> Vec<AgentStatus> {
    let Ok(entries) = std::fs::read_dir(state.path(AGENTS_DIR)) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut out: Vec<AgentStatus> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("state-") && name.ends_with(".json")
        })
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let value: Value = serde_json::from_str(&text).ok()?;
            let branch = value.get("branch")?.as_str()?.to_string();
            let mut agent_state = AgentState::parse(value.get("state")?.as_str()?)?;
            let age = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .unwrap_or_default();
            if agent_state.is_working() && age > STALE_WORKING {
                agent_state = AgentState::Idle;
            }
            Some(AgentStatus {
                branch,
                state: agent_state,
            })
        })
        .collect();
    out.sort_by(|a, b| a.branch.cmp(&b.branch));
    out
}

/// What the user hears about a change, as (title, event); `None` when it is not worth a notification.
pub fn notification_for(
    before: Option<AgentState>,
    after: AgentState,
) -> Option<(&'static str, &'static str)> {
    let was_working = before.is_some_and(AgentState::is_working);
    let was_idle = before.is_none_or(|state| state == AgentState::Idle);
    match after {
        AgentState::AwaitingInput if before != Some(AgentState::AwaitingInput) => {
            Some(("Claude needs your input", "needs_input"))
        }
        AgentState::Compacting if before != Some(AgentState::Compacting) => {
            Some(("Claude is compacting", "compacting"))
        }
        AgentState::Idle if was_working => Some(("Claude finished", "finished")),
        AgentState::Thinking | AgentState::ToolUse if was_idle => {
            Some(("Claude is working", "working"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (tempfile::TempDir, StateDir) {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        (temp, state)
    }

    #[test]
    fn hook_events_become_workspace_states() {
        let (_temp, state) = state();
        let hook = |body: Value| record_hook(&state, body.to_string().as_bytes()).expect("hook");
        assert_eq!(
            hook(json!({"cwd": "/p/workspace--feat-x/api", "hook_event_name": "UserPromptSubmit"})),
            Some(("feat-x".to_string(), AgentState::Thinking))
        );
        assert_eq!(
            hook(
                json!({"cwd": "/p/workspace--feat-x/api", "hook_event_name": "Notification", "message": "Claude needs your permission to use Bash"})
            ),
            Some(("feat-x".to_string(), AgentState::AwaitingInput))
        );
        assert_eq!(
            hook(
                json!({"cwd": "/p/workspace--main", "hook_event_name": "Notification", "message": "Claude is waiting for your input"})
            ),
            Some(("main".to_string(), AgentState::Idle))
        );
        assert_eq!(
            hook(json!({"cwd": "/elsewhere", "hook_event_name": "Stop"})),
            None
        );
        assert_eq!(
            hook(json!({"cwd": "/p/workspace--main", "hook_event_name": "Unknown"})),
            None
        );
        assert_eq!(
            std::fs::read_to_string(state.path("agents/state-feat-x.json")).expect("file"),
            r#"{"branch":"feat-x","state":"awaiting_input"}"#
        );
        assert_eq!(
            read_states(&state),
            [
                AgentStatus {
                    branch: "feat-x".into(),
                    state: AgentState::AwaitingInput
                },
                AgentStatus {
                    branch: "main".into(),
                    state: AgentState::Idle
                },
            ]
        );
    }

    #[test]
    fn a_hook_payload_names_its_session_turn_boundary_and_transcript() {
        let event = |body: Value| session_event(body.to_string().as_bytes());
        let prompt = event(json!({"session_id": "s1", "hook_event_name": "UserPromptSubmit", "transcript_path": "/t/s1.jsonl", "prompt": "list the files"}))
            .expect("event");
        assert_eq!(prompt.state, Some(AgentState::Thinking));
        assert_eq!(prompt.transcript, "/t/s1.jsonl");
        assert_eq!(prompt.detail["prompt_chars"], 14);
        let ask = event(json!({"session_id": "s1", "hook_event_name": "Notification", "message": "Claude needs your permission to use Bash"}))
            .expect("event");
        assert_eq!(ask.state, Some(AgentState::AwaitingInput));
        assert_eq!(ask.notification.as_deref(), Some("permission"));
        let tool = event(
            json!({"session_id": "s1", "hook_event_name": "PreToolUse", "tool_name": "Read"}),
        )
        .expect("event");
        assert_eq!(tool.tool.as_deref(), Some("Read"));
        assert_eq!(
            event(json!({"hook_event_name": "Stop"})),
            None,
            "no session id"
        );
    }

    #[test]
    fn transitions_worth_telling() {
        use AgentState::*;
        assert_eq!(
            notification_for(Some(Thinking), AwaitingInput).map(|n| n.1),
            Some("needs_input")
        );
        assert_eq!(
            notification_for(Some(ToolUse), Idle).map(|n| n.1),
            Some("finished")
        );
        assert_eq!(
            notification_for(None, Thinking).map(|n| n.1),
            Some("working")
        );
        assert_eq!(notification_for(Some(Thinking), ToolUse), None);
        assert_eq!(notification_for(Some(AwaitingInput), AwaitingInput), None);
        assert_eq!(notification_for(Some(Idle), Idle), None);
    }

    #[test]
    fn launched_sessions_get_every_hook_and_the_policy_hook_fails_closed() {
        let (temp, state) = state();
        let hooks = session_hooks(&state, Path::new("/nonexistent/pomelo")).expect("hooks");
        for event in HOOK_EVENTS {
            let command = hooks[event][0]["hooks"][0]["command"]
                .as_str()
                .expect("command");
            let wrapper = if event == "PreToolUse" {
                POLICY_WRAPPER
            } else {
                SESSION_WRAPPER
            };
            assert!(command.contains(wrapper), "{event}: {command}");
        }
        let guard = state.path(POLICY_WRAPPER);
        let run = |path: &Path| {
            std::process::Command::new("/bin/sh")
                .arg(path)
                .stdin(std::process::Stdio::null())
                .output()
                .expect("sh")
                .status
                .code()
        };
        assert_eq!(run(&guard), Some(2), "pom missing: the call is blocked");
        let failing = temp.path().join("failing");
        std::fs::write(&failing, "#!/bin/sh\nexit 101\n").expect("script");
        std::fs::set_permissions(
            &failing,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("chmod");
        session_hooks(&state, &failing).expect("hooks");
        assert_eq!(run(&guard), Some(2), "pom crashing: the call is blocked");
        assert!(std::fs::read_to_string(state.path(SESSION_WRAPPER))
            .expect("recorder")
            .contains("claude-hook --session"));
        assert!(
            !temp.path().join("home/.claude/settings.json").exists(),
            "nothing is written to the user's settings"
        );
    }
}
