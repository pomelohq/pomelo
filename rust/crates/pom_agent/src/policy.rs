//! A workspace's agent policy: before every tool call, the agent's `PreToolUse` hook asks the command
//! `pom.yml` names under `agents.policy`. It answers allow, deny or ask. The check fails closed: a policy
//! that errors, times out, answers nonsense or cannot be read denies the call, because a hook that merely
//! fails lets the tool run.
//!
//! `ask` shows the agent's own permission prompt to the person at the session. A session an orchestrator
//! drives has nobody at its prompt, so there `ask` denies with `pending approval <id>` until an approval
//! for that exact call is recorded; the orchestrator then retries.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::hooks::AgentState;
use crate::identity::Identity;
use crate::sessions::{record_event, session_dir, SessionEvent};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const WORKSPACE_PREFIX: &str = "workspace--";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub command: String,
    pub timeout: Duration,
    /// The workspace folder the command runs in.
    pub workspace_dir: PathBuf,
}

/// What the project of a session's folder says about its policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyLookup {
    /// Outside a workspace, or the project sets no policy.
    None,
    Found(Policy),
    /// The project's config exists but cannot be read: its policy cannot be known.
    Unreadable(String),
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
    pub fn deny(reason: impl Into<String>) -> Decision {
        Decision {
            verdict: Verdict::Deny,
            reason: reason.into(),
        }
    }
}

/// An answer to a pending approval, written by `pom agent approve|deny`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub allow: bool,
    pub decided_by: String,
    #[serde(default)]
    pub reason: String,
    /// Stays for every later identical call instead of letting one through.
    #[serde(default)]
    pub always: bool,
}

/// The policy of the workspace a session runs in, from its project's `pom.yml`.
pub fn workspace_policy(cwd: &Path) -> PolicyLookup {
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
    let Some(workspace_dir) = workspace_dir else {
        return PolicyLookup::None;
    };
    let path = root.join("pom.yml");
    if !path.is_file() {
        return PolicyLookup::None;
    }
    let config = match pom_config::Config::load(&path) {
        Ok(config) => config,
        Err(error) => {
            return PolicyLookup::Unreadable(format!(
                "the project config could not be read, so its agent policy is unknown: {error}"
            ))
        }
    };
    let Some(agents) = config
        .agents
        .filter(|agents| !agents.policy.trim().is_empty())
    else {
        return PolicyLookup::None;
    };
    PolicyLookup::Found(Policy {
        command: agents.policy,
        timeout: u64::try_from(agents.policy_timeout_sec)
            .ok()
            .filter(|seconds| *seconds > 0)
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs),
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
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
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
    };
    let text = reader.join().unwrap_or_default();
    if !status.success() {
        return Decision::deny(format!("the agent policy failed ({status})"));
    }
    let Ok(answer) = serde_json::from_str::<Value>(text.trim()) else {
        return Decision::deny("the agent policy did not answer with JSON");
    };
    let reason = answer
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let verdict = match answer.get("decision").and_then(Value::as_str) {
        Some("allow") => Verdict::Allow,
        Some("ask") => Verdict::Ask,
        Some("deny") => Verdict::Deny,
        _ => return Decision::deny("the agent policy answered no decision"),
    };
    Decision { verdict, reason }
}

/// The id of one tool call, the same when an orchestrator retries it.
pub fn request_id(session_id: &str, tool: &Value, input: &Value) -> String {
    let digest = Sha1::digest(format!("{session_id}\0{tool}\0{input}").as_bytes());
    digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn answer_path(state: &StateDir, session_id: &str, request: &str) -> PathBuf {
    session_dir(state, session_id)
        .join("answers")
        .join(format!("{request}.json"))
}

/// Records the answer to a pending approval; the next try of that call reads it once.
pub fn write_answer(
    state: &StateDir,
    session_id: &str,
    request: &str,
    answer: &Answer,
) -> std::io::Result<()> {
    let blob = serde_json::to_vec(answer).map_err(std::io::Error::other)?;
    pom_paths::write_atomic(&answer_path(state, session_id, request), &blob, 0o644)
}

/// The recorded answer for `request`, removed as it is read so one approval lets one call through.
pub fn take_answer(state: &StateDir, session_id: &str, request: &str) -> Option<Answer> {
    let path = answer_path(state, session_id, request);
    let answer: Answer = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    if !answer.always {
        if let Err(error) = std::fs::remove_file(&path) {
            eprintln!("agent policy: {}: {error}", path.display());
        }
    }
    Some(answer)
}

/// Who holds a session's prompt while its policy runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    /// A person is (or may be) at the session's prompt.
    Person,
    /// An orchestrator drives it; nobody answers a prompt there.
    Orchestrator,
}

/// The policy's decision on one `PreToolUse` event.
pub fn decide(
    state: &StateDir,
    identity: Option<&Identity>,
    body: &Value,
    policy: &Policy,
    driver: Driver,
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
        "origin": match driver { Driver::Person => "human", Driver::Orchestrator => "orchestrator" },
        "driven_by": Value::Null,
    });
    let decision = ask_policy(policy, &input);
    if decision.verdict != Verdict::Ask {
        return decision;
    }
    let request = request_id(&session_id, &field("tool_name"), &field("tool_input"));
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
    if driver == Driver::Orchestrator {
        if let Some(answer) = take_answer(state, &session_id, &request) {
            record(
                "permission_decision",
                AgentState::ToolUse,
                json!({"request": request, "allow": answer.allow, "decided_by": answer.decided_by}),
            );
            return Decision {
                verdict: if answer.allow {
                    Verdict::Allow
                } else {
                    Verdict::Deny
                },
                reason: answer.reason,
            };
        }
    }
    record(
        "permission_request",
        AgentState::AwaitingInput,
        json!({"request": request, "tool": field("tool_name"), "input": field("tool_input"), "reason": decision.reason}),
    );
    match driver {
        Driver::Person => decision,
        Driver::Orchestrator => Decision::deny(format!("pending approval {request}")),
    }
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
            workspace_dir: dir.to_path_buf(),
        }
    }

    fn call() -> Value {
        json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}})
    }

    fn identity() -> Identity {
        Identity {
            holder: "ws-myproject-feat-login-claude-raw".into(),
            role: "claude".into(),
            project: "myproject".into(),
            branch: "feat-login".into(),
            driver: "claude".into(),
        }
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
            decide(&state, None, &call(), &allow, Driver::Person).verdict,
            Verdict::Allow
        );
        let deny = policy(
            temp.path(),
            r#"echo '{"decision":"deny","reason":"no rm"}'"#,
            Duration::from_secs(5),
        );
        let decision = decide(&state, None, &call(), &deny, Driver::Person);
        assert_eq!(
            (decision.verdict, decision.reason.as_str()),
            (Verdict::Deny, "no rm")
        );
        let output: Value = serde_json::from_str(&hook_output(&decision)).expect("json");
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    }

    #[test]
    fn a_policy_that_times_out_errors_or_cannot_run_denies() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let slow = policy(temp.path(), "sleep 5", Duration::from_millis(300));
        let started = Instant::now();
        let decision = decide(&state, None, &call(), &slow, Driver::Person);
        assert_eq!(decision.verdict, Verdict::Deny);
        assert!(
            decision.reason.contains("longer than"),
            "{}",
            decision.reason
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        for command in [
            "echo maybe",
            "exit 3",
            "echo '{\"decision\":\"allow\"}'; exit 1",
            "/nonexistent/policy",
        ] {
            let broken = policy(temp.path(), command, Duration::from_secs(5));
            assert_eq!(
                decide(&state, None, &call(), &broken, Driver::Person).verdict,
                Verdict::Deny,
                "{command}"
            );
        }
        let gone = policy(
            &temp.path().join("missing-workspace"),
            "echo '{\"decision\":\"allow\"}'",
            Duration::from_secs(5),
        );
        assert_eq!(
            decide(&state, None, &call(), &gone, Driver::Person).verdict,
            Verdict::Deny,
            "the policy cannot be reached"
        );
    }

    #[test]
    fn ask_shows_the_prompt_to_a_person_and_denies_pending_approval_under_an_orchestrator() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let ask = policy(
            temp.path(),
            r#"echo '{"decision":"ask","reason":"deploys"}'"#,
            Duration::from_secs(5),
        );
        assert_eq!(
            decide(&state, Some(&identity()), &call(), &ask, Driver::Person).verdict,
            Verdict::Ask
        );

        let pending = decide(
            &state,
            Some(&identity()),
            &call(),
            &ask,
            Driver::Orchestrator,
        );
        assert_eq!(pending.verdict, Verdict::Deny);
        let request = request_id("s1", &json!("Bash"), &json!({"command": "rm -rf build"}));
        assert_eq!(pending.reason, format!("pending approval {request}"));
        let events = crate::sessions::events(&state, "s1");
        assert!(events.iter().all(|line| line.event == "permission_request"));
        assert_eq!(
            events.last().map(|line| line.detail["request"].clone()),
            Some(json!(request))
        );

        let approval = Answer {
            allow: true,
            decided_by: "operator".into(),
            reason: String::new(),
            always: false,
        };
        write_answer(&state, "s1", &request, &approval).expect("approve");
        assert_eq!(
            decide(
                &state,
                Some(&identity()),
                &call(),
                &ask,
                Driver::Orchestrator
            )
            .verdict,
            Verdict::Allow,
            "the retry goes through"
        );
        assert_eq!(
            decide(
                &state,
                Some(&identity()),
                &call(),
                &ask,
                Driver::Orchestrator
            )
            .verdict,
            Verdict::Deny,
            "one approval, one call"
        );
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
        let PolicyLookup::Found(found) = workspace_policy(&workspace) else {
            panic!("policy");
        };
        assert_eq!(found.command, "./policy.sh");
        assert_eq!(found.timeout, Duration::from_secs(9));
        assert_eq!(found.workspace_dir, root.join("workspace--feat-login"));
        assert_eq!(
            workspace_policy(temp.path()),
            PolicyLookup::None,
            "outside any workspace"
        );
        std::fs::write(root.join("pom.yml"), "session: [unclosed\n").expect("broken");
        assert!(
            matches!(workspace_policy(&workspace), PolicyLookup::Unreadable(_)),
            "a broken config denies"
        );
    }
}
