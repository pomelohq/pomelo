//! Deleting a workspace stops every holder it has, agents included, and `pom ps --orphans` finds the ones a
//! workspace or project left behind. The agent is a scripted stand-in found first on the PATH; no real agent runs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use pom_paths::StateDir;
use pom_ptyhost::{HolderGroup, Limits, SocketDir, SpawnRequest};
use serde_json::Value;

const POM: &str = env!("CARGO_BIN_EXE_pom");
const FAKE_CLAUDE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude-trust.sh");

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomhold")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in [
            "workspace--main/api",
            "workspace--feat/api",
            "workspace--feat-x/api",
        ] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        for dir in ["s", "home", "transcripts", "bin"] {
            std::fs::create_dir_all(temp.path().join(dir)).expect("dir");
        }
        std::os::unix::fs::symlink(FAKE_CLAUDE, temp.path().join("bin/claude"))
            .expect("fake claude");
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

    fn holders(&self) -> SocketDir {
        SocketDir::new(self.temp.path().join("s"))
    }

    fn state(&self) -> StateDir {
        StateDir::new(self.temp.path().join("state/pom"))
    }

    fn pom(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:/usr/bin:/bin:/usr/sbin:/sbin",
            self.temp.path().join("bin").display()
        );
        Command::new(POM)
            .args(args)
            .current_dir(self.root())
            .env("PATH", path)
            .env("HOME", self.temp.path().join("home"))
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POMELO_CONFIG_DIR", self.temp.path().join("config"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("FAKE_POM", POM)
            .env("FAKE_ENV_DUMP", self.temp.path().join("env.txt"))
            .env("FAKE_TRANSCRIPTS", self.temp.path().join("transcripts"))
            .env_remove("POM_AGENT_HOLDER")
            .output()
            .expect("pom")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.pom(args);
        assert!(output.status.success(), "{args:?}: {}", text(&output));
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A stand-in holder named like a workspace's agent, recording `group` when given.
    fn spawn(&self, name: &str, group: Option<(&str, &str)>) {
        let argv = vec!["sleep".to_string(), "100".to_string()];
        let group = group
            .map(|(project, workspace)| HolderGroup::new(project, workspace, Limits::default()));
        pom_ptyhost::spawn_holder(
            &self.holders(),
            &SpawnRequest {
                group: group.as_ref(),
                binary: Path::new(POM),
                name,
                cwd: &self.root(),
                cols: 80,
                rows: 24,
                argv: &argv,
                env: &[],
            },
        )
        .expect("spawn holder");
        pom_ptyhost::wait_for_holder(&self.holders(), name, Duration::from_secs(5))
            .expect("holder socket");
    }

    fn alive(&self) -> Vec<String> {
        self.holders()
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
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

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn deleting_a_workspace_stops_its_agents_and_services_and_nothing_else() {
    let fixture = Fixture::new();
    let started = fixture.pom(&[
        "agent", "start", "feat", "--fresh", "--role", "reviewer", "--json",
    ]);
    assert_eq!(started.status.code(), Some(7), "{}", text(&started));
    assert!(
        fixture.temp.path().join("env.txt").is_file(),
        "the stand-in agent ran"
    );
    fixture.ok(&["-w", "feat", "start", "api/web"]);
    fixture.spawn("ws-demo-feat-claude-raw-2", Some(("demo", "feat")));
    fixture.spawn("ws-demo-feat-side-codex-1", None);
    fixture.spawn("ws-demo-feat-x-claude-raw", Some(("demo", "feat-x")));
    fixture.spawn("ws-demo-feat-x-fixer", None);
    let before = fixture.alive();
    for name in [
        "ws-demo-feat-claude-reviewer",
        "svc-demo-feat-api-web",
        "ws-demo-feat-claude-raw-2",
        "ws-demo-feat-side-codex-1",
    ] {
        assert!(before.contains(&name.to_string()), "{name} in {before:?}");
    }

    fixture.ok(&["ws", "delete", "feat"]);
    let after = fixture.alive();
    assert!(
        !after
            .iter()
            .any(|name| name.starts_with("ws-demo-feat-") && !name.starts_with("ws-demo-feat-x-")),
        "an agent of feat survived: {after:?}"
    );
    assert!(!after.contains(&"svc-demo-feat-api-web".to_string()));
    assert!(
        after.contains(&"ws-demo-feat-x-claude-raw".to_string())
            && after.contains(&"ws-demo-feat-x-fixer".to_string()),
        "feat-x keeps its holders: {after:?}"
    );
    let orphans: Value =
        serde_json::from_str(&fixture.ok(&["ps", "--orphans", "--json"])).expect("orphans json");
    assert_eq!(orphans["orphans"], serde_json::json!([]), "{orphans}");
}

#[test]
fn holders_left_by_a_gone_workspace_are_marked_listed_and_stopped() {
    let fixture = Fixture::new();
    pom_sessions::Projects::register(&fixture.state(), "demo", &fixture.root()).expect("register");
    fixture.spawn("ws-demo-old-claude-raw", Some(("demo", "old")));
    fixture.spawn("ws-demo-feat-claude-raw", Some(("demo", "feat")));
    fixture.spawn("ws-other-main-claude-raw", Some(("other", "main")));

    let listed: Value = serde_json::from_str(&fixture.ok(&["ps", "--json"])).expect("ps json");
    let orphan_of = |name: &str| {
        listed["holders"]
            .as_array()
            .and_then(|holders| holders.iter().find(|holder| holder["name"] == name))
            .map(|holder| holder["orphan"].clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(orphan_of("ws-demo-old-claude-raw"), true, "{listed}");
    assert_eq!(orphan_of("ws-demo-feat-claude-raw"), false);
    assert_eq!(
        orphan_of("ws-other-main-claude-raw"),
        false,
        "a project this machine does not know is not judged"
    );

    let doctor: Value =
        serde_json::from_slice(&fixture.pom(&["doctor", "--quick", "--json"]).stdout)
            .expect("doctor json");
    let finding = doctor["findings"]
        .as_array()
        .and_then(|findings| {
            findings
                .iter()
                .find(|finding| finding["id"] == "holders.orphan")
        })
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        finding["fix"]
            .as_str()
            .unwrap_or_default()
            .contains("pom ps --orphans --kill"),
        "{doctor}"
    );

    let killed: Value = serde_json::from_str(&fixture.ok(&["ps", "--orphans", "--kill", "--json"]))
        .expect("kill json");
    assert_eq!(killed["schema"], "pom.orphans/v1");
    assert_eq!(killed["orphans"][0]["name"], "ws-demo-old-claude-raw");
    assert_eq!(killed["orphans"][0]["workspace"], "old");
    assert_eq!(killed["orphans"][0]["killed"], true);
    let alive = fixture.alive();
    assert!(!alive.contains(&"ws-demo-old-claude-raw".to_string()));
    assert!(alive.contains(&"ws-demo-feat-claude-raw".to_string()));
    assert!(alive.contains(&"ws-other-main-claude-raw".to_string()));
}
