//! The agent MCP tools as an agent inside a workspace sees them: `pom mcp` runs as the agent holder's own
//! process, so the gate knows the caller is that agent whatever it puts in its arguments.

use std::io::Write;
use std::process::{Command, Stdio};

use pom_agent::{record_event, AgentState, Identity, SessionEvent};
use pom_paths::StateDir;
use serde_json::{json, Value};

const POM: &str = env!("CARGO_BIN_EXE_pom");
const CALLER: &str = "ws-demo-feat-claude-raw";

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomam")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::create_dir_all(temp.path().join("s")).expect("sockets");
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\ndefault_branch: main\nrepos:\n  api:\n    services:\n      web: { cmd: \"sleep 100\" }\n",
        )
        .expect("pom.yml");
        let fixture = Fixture { temp };
        fixture.session("feat", "claude", CALLER, "own");
        fixture.session("feat", "reviewer", "ws-demo-feat-claude-reviewer", "rev");
        fixture.session("main", "claude", "ws-demo-main-claude-raw", "other");
        fixture
    }

    fn session(&self, branch: &str, role: &str, holder: &str, id: &str) {
        let identity = Identity {
            holder: holder.into(),
            role: role.into(),
            project: "demo".into(),
            branch: branch.into(),
            driver: "claude".into(),
        };
        let event = SessionEvent {
            session_id: id.into(),
            event: "SessionStart".into(),
            state: Some(AgentState::Idle),
            ..SessionEvent::default()
        };
        record_event(
            &StateDir::new(self.temp.path().join("state/pom")),
            &identity,
            &event,
        )
        .expect("event");
    }

    /// Runs `pom mcp --branch feat` as the feat workspace's agent and calls `calls` on it.
    fn as_agent(&self, calls: &[(&str, Value)]) -> Vec<(Value, bool)> {
        let mut lines = vec![
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        ];
        for (index, (name, arguments)) in calls.iter().enumerate() {
            lines.push(json!({"jsonrpc":"2.0","id":index + 1,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        }
        // The shell becomes `pom mcp` (exec), so the process the pidfile names is the server itself.
        let script = format!("sleep 0.3; exec '{POM}' mcp --branch feat");
        let mut child = Command::new("/bin/sh")
            .args(["-c", &script])
            .current_dir(self.temp.path().join("project/workspace--feat/api"))
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("POM_AGENT_HOLDER", "ws-demo-main-claude-raw")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("sh");
        std::fs::write(
            self.temp.path().join("s/caller.pid"),
            format!("{}\n{CALLER}\n", child.id()),
        )
        .expect("pidfile");
        {
            let mut stdin = child.stdin.take().expect("stdin");
            for line in &lines {
                writeln!(stdin, "{line}").expect("write");
            }
        }
        let output = child.wait_with_output().expect("output");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .skip(1)
            .map(|reply| {
                let result = &reply["result"];
                let text = result["content"][0]["text"].as_str().unwrap_or_default();
                (
                    serde_json::from_str(text).unwrap_or(Value::String(text.to_string())),
                    result["isError"].as_bool().unwrap_or(false),
                )
            })
            .collect()
    }
}

#[test]
fn an_agent_sees_and_reaches_only_its_own_workspaces_sessions() {
    let fixture = Fixture::new();
    let replies = fixture.as_agent(&[
        ("agent_list", json!({})),
        ("agent_read", json!({"role": "main/claude"})),
        ("agent_send", json!({"role": "main/claude", "text": "hi"})),
        ("agent_start", json!({"role": "main/claude"})),
        (
            "agent_approve",
            json!({"role": "main/claude", "request": "abc"}),
        ),
        ("agent_send", json!({"role": "claude", "text": "hi"})),
        ("agent_send", json!({"role": "reviewer", "text": "hi"})),
    ]);
    assert_eq!(replies.len(), 7);
    let (list, failed) = &replies[0];
    assert!(!failed);
    let roles: Vec<&str> = list["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .filter_map(|session| session["role"].as_str())
        .collect();
    assert_eq!(
        roles,
        ["claude", "reviewer"],
        "main's session does not exist for this agent"
    );
    for (index, what) in [(1, "read"), (2, "send"), (4, "approve")] {
        assert!(replies[index].1, "{what} into another workspace fails");
        assert_eq!(
            replies[index].0["error"], "other_workspace",
            "{what}: {:?}",
            replies[index].0
        );
    }
    assert!(replies[3].1, "start a session of another workspace");
    assert_eq!(
        replies[5].0["error"], "self",
        "the agent's env claimed another holder, but ancestry wins"
    );
    assert_eq!(
        replies[6].0["error"], "lease",
        "a person's session is not for an agent to type into"
    );
}
