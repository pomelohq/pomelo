//! `workspaces.max_running` and idle sleep through the real binary and the scheduler's tick, on a fake clock.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use pom_agent::{record_event, AgentState, Identity, SessionEvent};
use pom_paths::StateDir;
use pom_ptyhost::{SocketDir, SpawnRequest};
use pom_workspace::{Machine, PowerSettings};

const POM: &str = env!("CARGO_BIN_EXE_pom");
const AGENT: &str = "ws-demo-b-claude-raw";

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new(max_running: u64) -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomsc")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for branch in ["main", "a", "b"] {
            std::fs::create_dir_all(root.join(format!("workspace--{branch}/api/.git")))
                .expect("worktree");
        }
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\ndefault_branch: main\nrepos:\n  api:\n    services:\n      web: { cmd: \"sleep 100\" }\n",
        )
        .expect("pom.yml");
        let config = temp.path().join("config");
        std::fs::create_dir_all(&config).expect("config");
        std::fs::write(
            config.join("settings.json"),
            format!(r#"{{"workspaces": {{"max_running": {max_running}}}}}"#),
        )
        .expect("settings");
        Fixture { temp }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn state(&self) -> StateDir {
        StateDir::new(self.temp.path().join("state/pom"))
    }

    fn holders(&self) -> SocketDir {
        SocketDir::new(self.temp.path().join("s"))
    }

    fn pom(&self, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(self.root())
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POMELO_CONFIG_DIR", self.temp.path().join("config"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("HOME", self.temp.path())
            .env_remove("POM_AGENT_HOLDER")
            .output()
            .expect("pom")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.pom(args);
        assert!(output.status.success(), "{args:?}: {}", text(&output));
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn state_of(&self, branch: &str) -> String {
        let listed: serde_json::Value =
            serde_json::from_str(&self.ok(&["ws", "ls", "--json"])).expect("json");
        listed["workspaces"]
            .as_array()
            .and_then(|list| list.iter().find(|workspace| workspace["branch"] == branch))
            .and_then(|workspace| workspace["state"].as_str())
            .unwrap_or_default()
            .to_string()
    }

    fn running(&self, branch: &str) -> bool {
        self.holders()
            .holder_alive(&format!("svc-demo-{branch}-api-web"))
    }

    /// An agent session of workspace b, alive in a holder and in `state`.
    fn agent(&self, state: AgentState) {
        if !self.holders().holder_alive(AGENT) {
            let argv = vec!["sleep".to_string(), "100".to_string()];
            pom_ptyhost::spawn_holder(
                &self.holders(),
                &SpawnRequest {
                    group: None,
                    binary: Path::new(POM),
                    name: AGENT,
                    cwd: &self.root().join("workspace--b"),
                    cols: 80,
                    rows: 24,
                    argv: &argv,
                    env: &[],
                },
            )
            .expect("agent holder");
            pom_ptyhost::wait_for_holder(&self.holders(), AGENT, Duration::from_secs(5))
                .expect("agent socket");
        }
        let identity = Identity {
            holder: AGENT.into(),
            role: "claude".into(),
            project: "demo".into(),
            branch: "b".into(),
            driver: "claude".into(),
        };
        let event = SessionEvent {
            session_id: "s-b".into(),
            event: "UserPromptSubmit".into(),
            state: Some(state),
            ..SessionEvent::default()
        };
        record_event(&self.state(), &identity, &event).expect("event");
    }

    fn machine(&self) -> Machine {
        Machine {
            state: self.state(),
            holders: self.holders(),
            binary: PathBuf::from(POM),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let dir = self.holders();
        let names: Vec<String> = dir.holders().into_iter().map(|(name, _)| name).collect();
        dir.kill_holders_now(&names);
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn max_running_sleeps_the_idle_one_queues_behind_a_busy_one_and_starts_it_when_a_slot_frees() {
    let fixture = Fixture::new(1);
    fixture.ok(&["-w", "a", "start", "api/web"]);
    assert!(fixture.running("a"));

    let started = fixture.ok(&["-w", "b", "start", "api/web"]);
    assert!(started.contains("put demo/a to sleep"), "{started}");
    assert!(fixture.running("b") && !fixture.running("a"));
    assert_eq!(fixture.state_of("a"), "sleeping");

    fixture.agent(AgentState::Thinking);
    let queued = fixture.ok(&["ws", "wake", "a"]);
    assert!(queued.contains("a is queued"), "{queued}");
    assert_eq!(fixture.state_of("a"), "queued");
    assert!(!fixture.running("a"));
    let refused = fixture.pom(&["-w", "main", "start", "api/web"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert!(
        text(&refused).contains("main is queued"),
        "{}",
        text(&refused)
    );

    let settings = PowerSettings {
        idle_sleep_minutes: 0,
        max_running: 1,
    };
    let now = pom_workspace::now_ms();
    assert!(
        fixture.machine().tick(&settings, now).is_empty(),
        "no slot while b works"
    );
    fixture.ok(&["-w", "b", "stop"]);
    fixture
        .holders()
        .kill_holder(AGENT)
        .expect("stop the agent");
    let done = fixture.machine().tick(&settings, now);
    assert!(
        done.iter().any(|line| line.contains("started demo/a")),
        "{done:?}"
    );
    assert_eq!(done.len(), 1, "one slot, for the longest waiting: {done:?}");
    assert_eq!(fixture.state_of("main"), "queued");
    assert!(fixture.running("a"), "a got the slot back with what it ran");
    assert_eq!(fixture.state_of("a"), "running");
}

#[test]
fn an_idle_workspace_sleeps_once_the_clock_passes_its_minutes() {
    let fixture = Fixture::new(0);
    fixture.ok(&["-w", "a", "start", "api/web"]);
    fixture.ok(&["-w", "b", "start", "api/web"]);
    fixture.agent(AgentState::Thinking);
    let settings = PowerSettings {
        idle_sleep_minutes: 30,
        max_running: 0,
    };
    let now = pom_workspace::now_ms();
    assert!(fixture
        .machine()
        .tick(&settings, now + 29 * 60_000)
        .is_empty());
    let done = fixture.machine().tick(&settings, now + 31 * 60_000);
    assert_eq!(done, ["slept demo/a (idle)"], "b's agent is working");
    assert_eq!(fixture.state_of("a"), "sleeping");
    assert!(!fixture.running("a") && fixture.running("b"));
}
