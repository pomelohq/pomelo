//! A workspace's agent policy: before every tool call, the agent's `PreToolUse` hook asks the command
//! `pom.yml` names under `agents.policy`. It answers allow, deny or ask; ask waits for `pom agent approve` or
//! `deny`. A policy that fails, times out or is never answered denies the call: a guard that gives up must
//! not let things through.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::hooks::AgentState;
use crate::identity::Identity;
use crate::sessions::{now_ms, record_event, session_dir, SessionEvent};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_ASK_TIMEOUT: Duration = Duration::from_secs(120);
const DECISION_POLL: Duration = Duration::from_millis(200);
const WORKSPACE_PREFIX: &str = "workspace--";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub command: String,
    pub timeout: Duration,
    pub ask_timeout: Duration,
    /// The workspace folder the command runs in.
    pub workspace_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    Deny,
    Ask,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub verdict: Verdict,
    pub reason: String,
}

impl Decision {
    fn deny(reason: impl Into<String>) -> Decision {
        Decision {
            verdict: Verdict::Deny,
            reason: reason.into(),
        }
    }
}

/// An answer to an `ask`, written by `pom agent approve|deny` into the session's folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub allow: bool,
    pub decided_by: String,
    #[serde(default)]
    pub reason: String,
}

/// The policy of the workspace a session runs in, from its project's `pom.yml`.
pub fn workspace_policy(cwd: &Path) -> Option<Policy> {
    let mut root = PathBuf::new();
    let mut workspace_dir = None;
    for component in cwd.components() {
        if let Component::Normal(name) = component {
            if name.to_string_lossy().starts_with(WORKSPACE_PREFIX) {
                workspace_dir = Some(root.join(name));
                break;
            }
        }
        root.push(component);
    }
    let workspace_dir = workspace_dir?;
    let config = pom_config::Config::load(&root.join("pom.yml")).ok()?;
    let agents = config.agents?;
    let seconds = |value: i64, default: Duration| {
        u64::try_from(value)
            .ok()
            .filter(|seconds| *seconds > 0)
            .map_or(default, Duration::from_secs)
    };
    (!agents.policy.trim().is_empty()).then(|| Policy {
        command: agents.policy,
        timeout: seconds(agents.policy_timeout_sec, DEFAULT_TIMEOUT),
        ask_timeout: seconds(agents.ask_timeout_sec, DEFAULT_ASK_TIMEOUT),
        workspace_dir,
    })
}

/// Runs the policy command on one tool call. It is the user's own configured command, run like a service's.
pub fn ask_policy(policy: &Policy, input: &Value) -> Decision {
    let child = Command::new("/bin/sh")
        .args(["-c", &policy.command])
        .current_dir(&policy.workspace_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => return Decision::deny(format!("the agent policy could not run: {error}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(input.to_string().as_bytes()) {
            eprintln!("agent policy: stdin: {error}");
        }
    }
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stdout) = stdout.as_mut() {
            if let Err(error) = stdout.read_to_string(&mut text) {
                eprintln!("agent policy: stdout: {error}");
            }
        }
        text
    });
    let deadline = Instant::now() + policy.timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    eprintln!("agent policy: kill: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("agent policy: wait: {error}");
                }
                return Decision::deny(format!(
                    "the agent policy took longer than {} s",
                    policy.timeout.as_secs()
                ));
            }
            Err(error) => return Decision::deny(format!("the agent policy failed: {error}")),
        }
    }
    let text = reader.join().unwrap_or_default();
    let Ok(answer) = serde_json::from_str::<Value>(text.trim()) else {
        return Decision::deny("the agent policy did not answer with JSON");
    };
    let reason = answer
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match answer.get("decision").and_then(Value::as_str) {
        Some("allow") => Decision {
            verdict: Verdict::Allow,
            reason,
        },
        Some("ask") => Decision {
            verdict: Verdict::Ask,
            reason,
        },
        Some("deny") => Decision {
            verdict: Verdict::Deny,
            reason,
        },
        _ => Decision::deny("the agent policy answered no decision"),
    }
}

pub fn answer_path(state: &StateDir, session_id: &str, request: &str) -> PathBuf {
    session_dir(state, session_id)
        .join("answers")
        .join(format!("{request}.json"))
}

/// Writes the answer to a pending `ask`.
pub fn write_answer(
    state: &StateDir,
    session_id: &str,
    request: &str,
    answer: &Answer,
) -> std::io::Result<()> {
    let blob = serde_json::to_vec(answer).map_err(std::io::Error::other)?;
    pom_paths::write_atomic(&answer_path(state, session_id, request), &blob, 0o644)
}

/// Waits for the answer to `request`, denying when none comes in time.
pub fn wait_for_answer(
    state: &StateDir,
    session_id: &str,
    request: &str,
    timeout: Duration,
) -> (Decision, String) {
    let path = answer_path(state, session_id, request);
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(answer) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Answer>(&text).ok())
        {
            let verdict = if answer.allow {
                Verdict::Allow
            } else {
                Verdict::Deny
            };
            return (
                Decision {
                    verdict,
                    reason: answer.reason,
                },
                answer.decided_by,
            );
        }
        if Instant::now() >= deadline {
            return (
                Decision::deny(format!(
                    "nobody answered the permission request within {} s",
                    timeout.as_secs()
                )),
                String::new(),
            );
        }
        std::thread::sleep(DECISION_POLL);
    }
}

/// The policy's decision on one `PreToolUse` event, with `ask` resolved through an answer.
pub fn decide(
    state: &StateDir,
    identity: Option<&Identity>,
    body: &Value,
    policy: &Policy,
) -> Decision {
    let field = |key: &str| body.get(key).cloned().unwrap_or(Value::Null);
    let session_id = body
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let input = json!({
        "tool_name": field("tool_name"),
        "tool_input": field("tool_input"),
        "session_id": session_id,
        "role": identity.map(|identity| identity.role.clone()),
        "workspace": identity.map(|identity| identity.branch.clone()),
        "origin": "human",
        "driven_by": Value::Null,
    });
    let decision = ask_policy(policy, &input);
    if decision.verdict != Verdict::Ask {
        return decision;
    }
    let request = format!("{}-{}", now_ms(), std::process::id());
    let record = |event: &str, state_now: AgentState, detail: Value| {
        if let (Some(identity), false) = (identity, session_id.is_empty()) {
            let event = SessionEvent {
                session_id: session_id.clone(),
                event: event.into(),
                state: Some(state_now),
                detail,
                ..SessionEvent::default()
            };
            if let Err(error) = record_event(state, identity, &event) {
                eprintln!("agent policy: {error}");
            }
        }
    };
    record(
        "permission_request",
        AgentState::AwaitingInput,
        json!({"request": request, "tool": field("tool_name"), "input": field("tool_input"), "reason": decision.reason}),
    );
    let (answer, decided_by) = wait_for_answer(state, &session_id, &request, policy.ask_timeout);
    record(
        "permission_decision",
        AgentState::ToolUse,
        json!({"request": request, "decision": answer.verdict, "decided_by": decided_by, "reason": answer.reason}),
    );
    answer
}

/// What the hook prints so the agent applies the decision.
pub fn hook_output(decision: &Decision) -> String {
    let verdict = match decision.verdict {
        Verdict::Allow => "allow",
        Verdict::Deny => "deny",
        Verdict::Ask => "ask",
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": verdict,
            "permissionDecisionReason": decision.reason,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(dir: &Path, command: &str, timeout: Duration) -> Policy {
        Policy {
            command: command.into(),
            timeout,
            ask_timeout: Duration::from_secs(2),
            workspace_dir: dir.to_path_buf(),
        }
    }

    fn call() -> Value {
        json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}})
    }

    #[test]
    fn the_policy_reads_the_call_and_its_answer_is_applied() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let allow = policy(
            temp.path(),
            r#"grep -q '"tool_name":"Bash"' && echo '{"decision":"allow","reason":"ok"}'"#,
            Duration::from_secs(5),
        );
        assert_eq!(
            decide(&state, None, &call(), &allow).verdict,
            Verdict::Allow
        );
        let deny = policy(
            temp.path(),
            r#"echo '{"decision":"deny","reason":"no rm"}'"#,
            Duration::from_secs(5),
        );
        let decision = decide(&state, None, &call(), &deny);
        assert_eq!(
            (decision.verdict, decision.reason.as_str()),
            (Verdict::Deny, "no rm")
        );
        let output: Value = serde_json::from_str(&hook_output(&decision)).expect("json");
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    }

    #[test]
    fn a_policy_that_times_out_or_breaks_denies() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let slow = policy(temp.path(), "sleep 5", Duration::from_millis(300));
        let started = Instant::now();
        let decision = decide(&state, None, &call(), &slow);
        assert_eq!(decision.verdict, Verdict::Deny);
        assert!(
            decision.reason.contains("longer than"),
            "{}",
            decision.reason
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        let garbage = policy(temp.path(), "echo maybe", Duration::from_secs(5));
        assert_eq!(
            decide(&state, None, &call(), &garbage).verdict,
            Verdict::Deny
        );
        let failing = policy(temp.path(), "exit 3", Duration::from_secs(5));
        assert_eq!(
            decide(&state, None, &call(), &failing).verdict,
            Verdict::Deny
        );
    }

    #[test]
    fn an_ask_nobody_answers_denies_and_an_answer_decides() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let mut ask = policy(
            temp.path(),
            r#"echo '{"decision":"ask","reason":"deploys"}'"#,
            Duration::from_secs(5),
        );
        ask.ask_timeout = Duration::from_millis(400);
        let identity = Identity {
            holder: "ws-myproject-feat-login-claude-raw".into(),
            role: "claude".into(),
            project: "myproject".into(),
            branch: "feat-login".into(),
            driver: "claude".into(),
        };
        let decision = decide(&state, Some(&identity), &call(), &ask);
        assert_eq!(decision.verdict, Verdict::Deny);
        let events = crate::sessions::events(&state, "s1");
        assert_eq!(events[0].event, "permission_request");
        assert_eq!(events[0].state, "awaiting_input");
        let request = events[0].detail["request"]
            .as_str()
            .expect("request")
            .to_string();
        assert_eq!(events[1].event, "permission_decision");

        ask.ask_timeout = Duration::from_secs(5);
        let answering = {
            let state = state.clone();
            std::thread::spawn(move || {
                for _ in 0..50 {
                    let pending: Vec<String> = crate::sessions::events(&state, "s1")
                        .into_iter()
                        .filter(|line| line.event == "permission_request")
                        .filter_map(|line| line.detail["request"].as_str().map(str::to_string))
                        .collect();
                    if let Some(latest) = pending.last().filter(|latest| **latest != request) {
                        let answer = Answer {
                            allow: true,
                            decided_by: "operator".into(),
                            reason: String::new(),
                        };
                        write_answer(&state, "s1", latest, &answer).expect("answer");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            })
        };
        assert_eq!(
            decide(&state, Some(&identity), &call(), &ask).verdict,
            Verdict::Allow
        );
        answering.join().expect("answer thread");
        let last = crate::sessions::events(&state, "s1")
            .pop()
            .expect("decision");
        assert_eq!(last.detail["decided_by"], "operator");
    }

    #[test]
    fn the_policy_comes_from_the_project_of_the_workspace_folder() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("myproject");
        let workspace = root.join("workspace--feat-login").join("api");
        std::fs::create_dir_all(&workspace).expect("dirs");
        std::fs::write(
            root.join("pom.yml"),
            "session: myproject\nagents:\n  policy: ./policy.sh\n  policy_timeout_sec: 9\n",
        )
        .expect("pom.yml");
        let found = workspace_policy(&workspace).expect("policy");
        assert_eq!(found.command, "./policy.sh");
        assert_eq!(found.timeout, Duration::from_secs(9));
        assert_eq!(found.ask_timeout, DEFAULT_ASK_TIMEOUT);
        assert_eq!(found.workspace_dir, root.join("workspace--feat-login"));
        assert_eq!(workspace_policy(temp.path()), None, "outside any workspace");
    }
}
