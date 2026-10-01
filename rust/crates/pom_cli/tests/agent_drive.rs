//! Driving an agent session end to end through the real `pom` binary: a scripted fake agent runs in a real
//! holder and reports through the same hook command a real agent's hooks call. No real agent runs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use pom_ptyhost::{SocketDir, SpawnRequest};
use serde_json::Value;

const POM: &str = env!("CARGO_BIN_EXE_pom");
const FAKE_AGENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-agent.sh");
const HOLDER: &str = "ws-demo-feat-claude-reviewer";

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    /// A project with a `feat` workspace whose `reviewer` session is the fake agent.
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomdrv")
            .tempdir_in("/tmp")
            .expect("temp");
        let fixture = Fixture { temp };
        let root = fixture.root();
        for repo in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\ndefault_branch: main\nrepos:\n  api:\n    services:\n      web: { cmd: \"sleep 100\" }\n",
        )
        .expect("pom.yml");
        let env: Vec<(String, String)> = [
            (
                "XDG_STATE_HOME",
                fixture
                    .temp
                    .path()
                    .join("state")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("POM", POM.to_string()),
            ("FAKE_SESSION", "s1".to_string()),
            (
                "FAKE_TRANSCRIPT",
                fixture
                    .temp
                    .path()
                    .join("s1.jsonl")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("POM_AGENT_HOLDER", HOLDER.to_string()),
            ("POM_AGENT_ROLE", "reviewer".to_string()),
            ("POM_PROJECT_SESSION", "demo".to_string()),
            ("POM_WORKSPACE", "feat".to_string()),
            ("POM_AGENT_DRIVER", "claude".to_string()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
        let argv = vec!["/bin/sh".to_string(), FAKE_AGENT.to_string()];
        pom_ptyhost::spawn_holder(
            &fixture.holders(),
            &SpawnRequest {
                binary: Path::new(POM),
                name: HOLDER,
                cwd: &root.join("workspace--feat"),
                cols: 120,
                rows: 40,
                argv: &argv,
                env: &env,
            },
        )
        .expect("holder");
        pom_ptyhost::wait_for_holder(&fixture.holders(), HOLDER, Duration::from_secs(5))
            .expect("socket");
        fixture.until("the fake agent to report", |fixture| {
            fixture.json(&["agent", "ls", "feat", "--json"])["sessions"][0]["session_id"] == "s1"
        });
        fixture
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn holders(&self) -> SocketDir {
        SocketDir::new(self.temp.path().join("s"))
    }

    fn pom(&self, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(self.root())
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env_remove("POM_AGENT_HOLDER")
            .output()
            .expect("pom")
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.pom(args);
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    }

    fn until(&self, what: &str, done: impl Fn(&Fixture) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let holders = self.holders();
        let names: Vec<String> = holders
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        holders.kill_holders_now(&names);
    }
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "not JSON: {} / {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap_or(Value::Null)
}

#[test]
fn ask_sends_one_turn_waits_for_its_stop_and_returns_what_the_agent_did() {
    let fixture = Fixture::new();
    let refused = fixture.pom(&["agent", "send", "feat/reviewer", "hello", "--json"]);
    assert_eq!(
        refused.status.code(),
        Some(6),
        "a person's session is not an orchestrator's to type into"
    );
    assert_eq!(stderr_json(&refused)["error"], "lease");

    let output = fixture.pom(&[
        "agent",
        "ask",
        "feat/reviewer",
        "hello",
        "--take",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer = stdout_json(&output);
    assert_eq!(answer["turn"], 1);
    assert_eq!(answer["wait"]["reached"], "turn-end");
    assert_eq!(answer["wait"]["stop_reason"], "end_turn");
    assert_eq!(answer["result"]["prompt"], "hello");
    assert_eq!(answer["result"]["origin"], "orchestrator");
    assert_eq!(answer["result"]["items"][0]["text"], "echo: hello");
    assert_eq!(answer["result"]["usage"]["output_tokens"], 2);

    let second = fixture.pom(&[
        "agent",
        "ask",
        "feat/reviewer",
        "again",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(
        second.status.code(),
        Some(0),
        "the orchestrator keeps its lease"
    );
    assert_eq!(stdout_json(&second)["turn"], 2);
}

#[test]
fn wait_exits_3_when_the_agent_asks_for_permission() {
    let fixture = Fixture::new();
    let sent = fixture.pom(&[
        "agent",
        "send",
        "feat/reviewer",
        "needs-permission",
        "--take",
        "--json",
    ]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let waited = fixture.pom(&[
        "agent",
        "wait",
        "feat/reviewer",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(waited.status.code(), Some(3));
    assert_eq!(stdout_json(&waited)["reached"], "awaiting_input");
}

#[test]
fn wait_exits_4_with_the_cause_when_the_agent_dies_mid_turn() {
    let fixture = Fixture::new();
    let sent = fixture.pom(&[
        "agent",
        "send",
        "feat/reviewer",
        "crash",
        "--take",
        "--json",
    ]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let waited = fixture.pom(&[
        "agent",
        "wait",
        "feat/reviewer",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(waited.status.code(), Some(4));
    let outcome = stdout_json(&waited);
    assert_eq!(outcome["reached"], "died");
    assert_eq!(outcome["cause"], "crashed");
    let listed = fixture.json(&["agent", "ls", "feat", "--json"]);
    assert_eq!(listed["sessions"][0]["state"], "died");
}

#[test]
fn interrupt_cancels_the_running_turn() {
    let fixture = Fixture::new();
    let sent = fixture.pom(&["agent", "send", "feat/reviewer", "long", "--take", "--json"]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let interrupted = fixture.pom(&["agent", "interrupt", "feat/reviewer", "--json"]);
    assert_eq!(
        interrupted.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&interrupted.stderr)
    );
    let waited = fixture.pom(&[
        "agent",
        "wait",
        "feat/reviewer",
        "--timeout",
        "10s",
        "--json",
    ]);
    assert_eq!(waited.status.code(), Some(0));
    assert_eq!(stdout_json(&waited)["stop_reason"], "cancelled");
}

#[test]
fn takeover_and_release_move_the_lease_and_say_so_on_watch() {
    let fixture = Fixture::new();
    let sent = fixture.pom(&[
        "agent",
        "ask",
        "feat/reviewer",
        "hello",
        "--take",
        "--timeout",
        "20s",
        "--json",
    ]);
    assert_eq!(
        sent.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert!(
        fixture.holders().input_lease(HOLDER).is_some(),
        "the holder only takes the orchestrator's keys"
    );

    let taken = fixture.pom(&["agent", "takeover", "feat", "--session-id", "s1", "--json"]);
    assert_eq!(
        taken.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&taken.stderr)
    );
    assert_eq!(
        fixture.holders().input_lease(HOLDER),
        None,
        "a person types freely"
    );
    let refused = fixture.pom(&["agent", "send", "feat/reviewer", "more", "--json"]);
    assert_eq!(
        stderr_json(&refused)["error"],
        "lease",
        "the orchestrator waits while a person has it"
    );

    let released = fixture.pom(&["agent", "release", "feat/reviewer", "--json"]);
    assert_eq!(
        released.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&released.stderr)
    );
    assert_eq!(stdout_json(&released)["lease"], "orchestrator");
    assert!(fixture.holders().input_lease(HOLDER).is_some());

    let watched = fixture.pom(&["agent", "watch", "feat", "--from-start", "--timeout", "1"]);
    let changes: Vec<(String, String)> = String::from_utf8_lossy(&watched.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["event"] == "lease")
        .map(|line| {
            (
                line["from_class"].as_str().unwrap_or_default().to_string(),
                line["to_class"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        changes,
        [
            ("human".to_string(), "orchestrator".to_string()),
            ("orchestrator".to_string(), "human".to_string()),
            ("human".to_string(), "orchestrator".to_string()),
        ]
    );
    let messages = String::from_utf8_lossy(&watched.stdout)
        .lines()
        .filter(|line| line.contains("\"event\":\"message\""))
        .count();
    assert_eq!(messages, 1, "every turn sent is logged once");
}
