//! `pom agent` through the real binary, over recorded session state and a fixture transcript. No agent runs:
//! the sessions are written the way their hooks would write them.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use pom_agent::{record_event, AgentState, Identity, SessionEvent};
use pom_paths::StateDir;
use serde_json::Value;

const POM: &str = env!("CARGO_BIN_EXE_pom");
const TRANSCRIPT: &str = include_str!("../../pom_agent/tests/fixtures/transcript.jsonl");

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomagent")
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
        Fixture { temp }
    }

    fn state(&self) -> StateDir {
        StateDir::new(self.temp.path().join("state/pom"))
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(cwd)
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env_remove("POM_AGENT_HOLDER");
        command
    }

    fn pom(&self, args: &[&str]) -> Output {
        self.command(POM, &self.root())
            .args(args)
            .output()
            .expect("pom")
    }

    fn session(
        &self,
        branch: &str,
        role: &str,
        holder: &str,
        id: &str,
        steps: &[(&str, AgentState, usize)],
    ) {
        let transcript = self.temp.path().join(format!("{id}.jsonl"));
        let identity = Identity {
            holder: holder.into(),
            role: role.into(),
            project: "demo".into(),
            branch: branch.into(),
            driver: "claude".into(),
        };
        for (event, state, upto) in steps {
            std::fs::write(&transcript, &TRANSCRIPT[..*upto]).expect("transcript");
            let event = SessionEvent {
                session_id: id.into(),
                event: (*event).into(),
                state: Some(*state),
                transcript: transcript.to_string_lossy().into_owned(),
                ..SessionEvent::default()
            };
            record_event(&self.state(), &identity, &event).expect("event");
        }
    }
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json")
}

fn second_prompt() -> usize {
    TRANSCRIPT
        .find("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"run")
        .expect("second prompt")
}

#[test]
fn ls_and_read_report_one_workspaces_sessions_from_their_transcripts() {
    let fixture = Fixture::new();
    let first = TRANSCRIPT
        .find("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"list")
        .expect("first");
    fixture.session(
        "feat",
        "claude",
        "ws-demo-feat-claude-raw",
        "s1",
        &[
            ("SessionStart", AgentState::Idle, first),
            ("UserPromptSubmit", AgentState::Thinking, first),
            ("Stop", AgentState::Idle, second_prompt()),
        ],
    );
    fixture.session(
        "main",
        "claude",
        "ws-demo-main-claude-raw",
        "s2",
        &[("SessionStart", AgentState::Idle, 0)],
    );

    let listed = json(&fixture.pom(&["agent", "ls", "feat", "--json"]));
    assert_eq!(listed["schema"], "pom.agent/v1");
    assert_eq!(listed["workspace"], "feat");
    let sessions = listed["sessions"].as_array().expect("sessions");
    assert_eq!(sessions.len(), 1, "main's session is not feat's");
    assert_eq!(sessions[0]["handle"], "feat/claude");
    assert_eq!(sessions[0]["turn"], 1);
    assert_eq!(sessions[0]["state"], "idle");

    let read = json(&fixture.pom(&["agent", "read", "feat/claude", "--json"]));
    let turn = &read["turns"][0];
    assert_eq!(turn["turn"], 1);
    assert_eq!(turn["prompt"], "list the files in src");
    assert_eq!(turn["stop_reason"], "end_turn");
    assert_eq!(turn["origin"], "human");
    assert_eq!(turn["items"][0]["kind"], "tool_call");
    assert_eq!(turn["items"][0]["name"], "Bash");
    assert_eq!(turn["items"][0]["result"], "main.rs\nlib.rs");
    assert_eq!(turn["usage"]["output_tokens"], 25);

    let missing = fixture.pom(&["agent", "read", "feat/reviewer", "--json"]);
    assert_eq!(missing.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&missing.stderr).expect("error json");
    assert_eq!(error["error"], "not_found");
}

#[test]
fn an_agent_cannot_read_or_list_a_session_of_another_workspace() {
    let fixture = Fixture::new();
    fixture.session(
        "main",
        "claude",
        "ws-demo-main-claude-raw",
        "s2",
        &[("SessionStart", AgentState::Idle, 0)],
    );
    let args = ["agent", "read", "main/claude", "--json"];
    // The shell below stands in for the feat workspace's agent: pom runs as its child.
    let script = format!("sleep 0.3; exec '{POM}' {}", args.join(" "));
    let agent = fixture
        .command("/bin/sh", &fixture.root())
        .args(["-c", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sh");
    let pid = agent.id();
    std::fs::write(
        fixture.temp.path().join("s").join("holder.pid"),
        format!("{pid}\nws-demo-feat-claude-raw\n"),
    )
    .expect("pidfile");
    let output = agent.wait_with_output().expect("wait");
    assert_eq!(
        output.status.code(),
        Some(6),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let error: Value = serde_json::from_slice(&output.stderr).expect("error json");
    assert_eq!(error["error"], "other_workspace");

    let operator = fixture.pom(&args);
    assert!(
        operator.status.success(),
        "a person may pick any workspace per command"
    );
}

#[test]
fn watch_streams_the_workspaces_events_as_ndjson() {
    let fixture = Fixture::new();
    fixture.session(
        "feat",
        "reviewer",
        "ws-demo-feat-claude-reviewer",
        "s3",
        &[
            ("SessionStart", AgentState::Idle, 0),
            ("PreCompact", AgentState::Compacting, 0),
        ],
    );
    let output = fixture.pom(&["agent", "watch", "feat", "--from-start", "--timeout", "1"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("ndjson"))
        .collect();
    let events: Vec<&str> = lines
        .iter()
        .filter_map(|line| line["event"].as_str())
        .collect();
    assert_eq!(events, ["SessionStart", "PreCompact"]);
    assert!(lines
        .iter()
        .all(|line| line["workspace"] == "feat" && line["role"] == "reviewer"));
}
