//! Claude Code reports what it is doing through hooks: every event runs `claude-hook`, which maps it to
//! a state and writes `agents/state-<branch>.json` for the workspace the session runs in. The app
//! watches that folder. File names, contents and the event mapping are the previous core's.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use pom_paths::StateDir;
use serde_json::{json, Map, Value};

use crate::claude::{read_object, write_object, write_wrapper, ClaudeHome, InstallError};

const HOOK_WRAPPER: &str = "claude-hook";
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
    let notification = if event == "Notification" {
        let raw = String::from_utf8_lossy(input).to_lowercase();
        if raw.contains("permission") || raw.contains("elicitation") {
            "permission"
        } else {
            "idle"
        }
    } else {
        field("notification_type")
    };
    let Some(agent_state) = event_state(event, notification) else {
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

/// Handles `<binary> claude-hook`: the event arrives on stdin. Never fails the hook: a broken hook
/// would show up as an error inside the user's Claude session.
pub fn run(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some(HOOK_WRAPPER) {
        return None;
    }
    let mut input = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("claude-hook: {error}");
        return Some(0);
    }
    if let Err(error) = record_hook(&StateDir::from_env(), &input) {
        eprintln!("claude-hook: {error}");
    }
    Some(0)
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

/// Adds pom's hook to every tracked Claude Code event in `~/.claude/settings.json`, replacing an older
/// pom hook and keeping everyone else's. Rewrites only when something changed.
pub fn install_hooks(
    claude: &ClaudeHome,
    state: &StateDir,
    binary: &Path,
) -> Result<bool, InstallError> {
    let wrapper = write_wrapper(state, HOOK_WRAPPER, binary, HOOK_WRAPPER)?;
    let command = format!("sh {}", wrapper.to_string_lossy());
    let path = claude.home.join(".claude/settings.json");
    let original = read_object(&path)?;
    let mut root = original.clone();
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    let Some(hooks) = hooks.as_object_mut() else {
        return Ok(false);
    };
    let ours = json!({ "matcher": "", "hooks": [{ "type": "command", "command": command }] });
    for event in HOOK_EVENTS {
        let list = hooks
            .get(event)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut kept = without_our_hooks(list);
        kept.push(ours.clone());
        hooks.insert(event.to_string(), Value::Array(kept));
    }
    if root == original {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_object(&path, root)?;
    Ok(true)
}

/// Drops pom's hook commands from a hook list, and entries left with no command.
fn without_our_hooks(list: Vec<Value>) -> Vec<Value> {
    list.into_iter()
        .filter_map(|entry| {
            let Value::Object(mut entry) = entry else {
                return Some(entry);
            };
            let inner: Vec<Value> = entry
                .get("hooks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|hook| {
                    !hook
                        .get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|command| command.contains(HOOK_WRAPPER))
                })
                .collect();
            if inner.is_empty() {
                return None;
            }
            entry.insert("hooks".into(), Value::Array(inner));
            Some(Value::Object(entry))
        })
        .collect()
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
    fn hooks_install_next_to_other_tools_hooks_and_only_once() {
        let (temp, state) = state();
        let claude = ClaudeHome {
            home: temp.path().join("home"),
        };
        let settings = claude.home.join(".claude/settings.json");
        std::fs::create_dir_all(settings.parent().expect("parent")).expect("dir");
        std::fs::write(
            &settings,
            r#"{"theme":"dark","hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"say done"},{"type":"command","command":"sh /old/claude-hook"}]}]}}"#,
        )
        .expect("settings");
        let binary = Path::new("/Apps/Pomelo.app/Contents/MacOS/pomelo");
        assert!(install_hooks(&claude, &state, binary).expect("install"));
        assert!(!install_hooks(&claude, &state, binary).expect("again"));
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).expect("read")).expect("json");
        assert_eq!(written["theme"], "dark");
        let command = format!("sh {}", state.path("claude-hook").display());
        assert_eq!(
            written["hooks"]["Stop"],
            json!([
                {"matcher": "", "hooks": [{"type": "command", "command": "say done"}]},
                {"matcher": "", "hooks": [{"type": "command", "command": command}]},
            ])
        );
        for event in HOOK_EVENTS {
            assert!(
                written["hooks"][event]
                    .as_array()
                    .is_some_and(|list| !list.is_empty()),
                "{event}"
            );
        }
        assert!(std::fs::read_to_string(state.path("claude-hook"))
            .expect("wrapper")
            .ends_with("exec \"$BIN\" claude-hook\n"));
    }
}
