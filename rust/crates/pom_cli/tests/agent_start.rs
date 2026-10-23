//! `pom agent start` through the real binary with a scripted stand-in for the agent CLI that, like the real one
//! in a folder it has never seen, asks to trust the folder before it reports anything. No real agent runs.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use pom_ptyhost::SocketDir;
use serde_json::Value;

const POM: &str = env!("CARGO_BIN_EXE_pom");
const FAKE_CLAUDE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude-trust.sh");

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomstart")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::create_dir_all(temp.path().join("s")).expect("sockets");
        std::fs::create_dir_all(temp.path().join("home")).expect("home");
        std::fs::create_dir_all(temp.path().join("transcripts")).expect("transcripts");
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        std::os::unix::fs::symlink(FAKE_CLAUDE, bin.join("claude")).expect("fake claude");
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\ndefault_branch: main\nrepos:\n  api:\n    services:\n      web: { cmd: \"sleep 100\" }\n",
        )
        .expect("pom.yml");
        Fixture { temp }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn pom(&self, args: &[&str]) -> Output {
        self.pom_with(args, &[])
    }

    fn pom_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let path = format!(
            "{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
            self.temp.path().join("bin").display()
        );
        Command::new(POM)
            .args(args)
            .current_dir(self.root())
            .env("PATH", path)
            .env("HOME", self.temp.path().join("home"))
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("FAKE_POM", POM)
            .env("FAKE_ENV_DUMP", self.temp.path().join("env.txt"))
            .env("FAKE_TRANSCRIPTS", self.temp.path().join("transcripts"))
            // What a coding agent sets for the commands it runs; a session pom starts must not inherit them.
            .env("CLAUDECODE", "1")
            .env("CLAUDE_CODE_CHILD_SESSION", "1")
            .env("CLAUDE_CODE_MESSAGING_TOKEN", "parent-token")
            .env("FAKE_TRUST_ANSWER", self.trust_answer())
            .envs(env.iter().copied())
            .env_remove("POM_AGENT_HOLDER")
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .expect("pom")
    }

    fn workspace_log(&self, event: &str) -> Vec<Value> {
        let watched = self.pom(&["agent", "watch", "feat", "--from-start", "--timeout", "1"]);
        String::from_utf8_lossy(&watched.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["event"] == event)
            .collect()
    }

    fn trust_answer(&self) -> PathBuf {
        self.temp.path().join("trust-answer.txt")
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_slice(&self.pom(args).stdout).unwrap_or(Value::Null)
    }

    fn reviewer(&self) -> Value {
        self.json(&["agent", "ls", "feat", "--json"])["sessions"]
            .as_array()
            .and_then(|sessions| {
                sessions
                    .iter()
                    .find(|session| session["role"] == "reviewer")
                    .cloned()
            })
            .unwrap_or(Value::Null)
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
        let holders = SocketDir::new(self.temp.path().join("s"));
        let names: Vec<String> = holders
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        holders.kill_holders_now(&names);
    }
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

#[test]
fn a_session_waiting_to_trust_its_folder_says_so_and_can_be_taken_over() {
    let fixture = Fixture::new();
    let started = fixture.pom(&[
        "agent", "start", "feat", "--fresh", "--role", "reviewer", "--json",
    ]);
    assert_eq!(started.status.code(), Some(7), "{started:?}");
    let error: Value = serde_json::from_slice(&started.stderr).unwrap_or(Value::Null);
    assert_eq!(error["error"], "needs_trust", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--trust"),
        "{error}"
    );

    let reviewer = fixture.reviewer();
    assert_eq!(reviewer["state"], "needs_trust", "{reviewer}");
    let session_id = reviewer["session_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !session_id.is_empty(),
        "the session launched is known before any hook: {reviewer}"
    );

    let taken = fixture.pom(&[
        "agent",
        "takeover",
        "feat",
        "--session-id",
        &session_id,
        "--json",
    ]);
    assert_eq!(taken.status.code(), Some(0), "{taken:?}");
    assert_eq!(stdout_json(&taken)["lease"], "person");
}

#[test]
fn trust_answers_the_prompt_and_the_session_comes_up_without_the_parent_agent_markers() {
    let fixture = Fixture::new();
    let started = fixture.pom(&[
        "agent", "start", "feat", "--fresh", "--role", "reviewer", "--trust", "--json",
    ]);
    assert_eq!(started.status.code(), Some(0), "{started:?}");
    fixture.until("the agent's first hook", |fixture| {
        fixture.reviewer()["state"] == "idle"
    });
    assert_eq!(fixture.reviewer()["last_event"], "SessionStart");
    assert_eq!(
        std::fs::read_to_string(fixture.trust_answer()).unwrap_or_default(),
        "yes\n",
        "the stand-in, which draws the prompt again with no highlighted after the first key, took a yes"
    );

    let env = std::fs::read_to_string(fixture.temp.path().join("env.txt")).expect("env dump");
    for marker in [
        "CLAUDECODE=",
        "CLAUDE_CODE_CHILD_SESSION=",
        "CLAUDE_CODE_MESSAGING_TOKEN=",
    ] {
        assert!(
            !env.lines().any(|line| line.starts_with(marker)),
            "{marker} reached the session"
        );
    }
    assert!(env.lines().any(|line| line == "POM_AGENT_ROLE=reviewer"));

    let trusted = fixture.workspace_log("folder_trusted");
    assert_eq!(trusted.len(), 1, "{trusted:?}");
    assert_eq!(trusted[0]["via"], "agent_config");
    let typed = fixture.workspace_log("screen_fallback");
    assert!(!typed.is_empty(), "the screen answer is logged");
    assert!(typed.iter().all(|line| line["action"] == "trust"));
    assert_eq!(
        typed
            .iter()
            .filter_map(|line| line["sent"].as_str())
            .next_back(),
        Some("enter"),
        "{typed:?}"
    );
}

#[test]
fn trust_goes_through_the_agents_config_and_no_key_is_typed() {
    let fixture = Fixture::new();
    let started = fixture.pom_with(
        &[
            "agent", "start", "feat", "--fresh", "--role", "reviewer", "--trust", "--json",
        ],
        &[("FAKE_TRUST_READS_CONFIG", "1")],
    );
    assert_eq!(started.status.code(), Some(0), "{started:?}");
    fixture.until("the agent's first hook", |fixture| {
        fixture.reviewer()["state"] == "idle"
    });
    assert!(
        !fixture.trust_answer().exists(),
        "the prompt was never drawn, so nothing answered it"
    );
    assert!(fixture.workspace_log("screen_fallback").is_empty());
    let folder = std::fs::canonicalize(fixture.root().join("workspace--feat")).expect("folder");
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.temp.path().join("home/.claude.json")).expect("config"),
    )
    .expect("json");
    assert_eq!(
        config["projects"][folder.to_string_lossy().as_ref()]["hasTrustDialogAccepted"],
        true,
        "{config}"
    );
}

#[test]
fn without_trust_the_agents_config_is_left_alone() {
    let fixture = Fixture::new();
    let started = fixture.pom(&[
        "agent", "start", "feat", "--fresh", "--role", "reviewer", "--json",
    ]);
    assert_eq!(started.status.code(), Some(7), "{started:?}");
    assert!(!fixture.temp.path().join("home/.claude.json").exists());
    assert!(fixture.workspace_log("folder_trusted").is_empty());
}

#[test]
fn trust_never_confirms_no_when_every_arrow_lands_twice() {
    let fixture = Fixture::new();
    let started = fixture.pom_with(
        &[
            "agent", "start", "feat", "--fresh", "--role", "reviewer", "--trust", "--json",
        ],
        &[("FAKE_TRUST_DOUBLE_ARROW", "1")],
    );
    assert!(
        fixture.temp.path().join("env.txt").exists(),
        "the stand-in agent ran"
    );
    assert_eq!(started.status.code(), Some(7), "{started:?}");
    let error: Value = serde_json::from_slice(&started.stderr).unwrap_or(Value::Null);
    assert_eq!(error["error"], "needs_trust", "{error}");
    assert!(
        !fixture.trust_answer().exists(),
        "Enter was pressed with no highlighted"
    );
    assert_eq!(fixture.reviewer()["state"], "needs_trust");
}
