//! `pom ws sleep` and `pom ws wake` through the real binary: what ran comes back on the same ports, and nothing
//! else starts.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const POM: &str = env!("CARGO_BIN_EXE_pom");

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomsl")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::write(root.join("workspace--feat/api/notes.txt"), "kept\n").expect("notes");
        std::fs::create_dir_all(temp.path().join("zdot")).expect("zdot");
        std::fs::write(
            root.join("pom.yml"),
            r#"session: demo
default_branch: main
repos:
  api:
    services:
      web:
        cmd: "exec nc -lk 127.0.0.1 $PORT"
        type: backend
      worker:
        cmd: "sleep 100"
"#,
        )
        .expect("pom.yml");
        Fixture { temp }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn pom(&self, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(self.root())
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("ZDOTDIR", self.temp.path().join("zdot"))
            .env("HOME", self.temp.path())
            .output()
            .expect("pom")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.pom(args);
        assert!(output.status.success(), "{args:?}: {}", text(&output));
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.ok(args)).expect("json")
    }

    /// `repo/service` -> (up, port) in workspace feat.
    fn services(&self) -> Vec<(String, bool, Option<u64>)> {
        let status = self.json(&["-w", "feat", "status", "-o", "json"]);
        status["services"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|service| {
                (
                    format!(
                        "{}/{}",
                        service["repo"].as_str().unwrap_or_default(),
                        service["service"].as_str().unwrap_or_default()
                    ),
                    service["up"].as_bool().unwrap_or(false),
                    service["port"].as_u64(),
                )
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let dir = pom_ptyhost::SocketDir::new(self.temp.path().join("s"));
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

fn state_file(root: &Path) -> String {
    std::fs::read_to_string(root.join("workspace--feat/.pom-workspace.json")).unwrap_or_default()
}

#[test]
fn sleep_stops_what_runs_and_wake_brings_back_only_that_on_the_same_ports() {
    let fixture = Fixture::new();
    let root = fixture.root();
    fixture.ok(&["-w", "feat", "start", "api/web"]);
    let before = fixture.services();
    let web_port = before
        .iter()
        .find(|(name, _, _)| name == "api/web")
        .and_then(|(_, up, port)| up.then_some(*port)?)
        .unwrap_or_else(|| panic!("api/web runs with a port: {before:?}"));

    let slept = fixture.json(&["ws", "sleep", "feat", "--json"]);
    assert_eq!(slept["stopped_services"], serde_json::json!(["api/web"]));
    assert_eq!(slept["state"], "sleeping");
    assert!(
        state_file(&root).contains(r#""state": "sleeping""#),
        "{}",
        state_file(&root)
    );
    assert!(fixture.services().iter().all(|(_, up, _)| !up));
    let listed = fixture.json(&["ws", "ls", "--json"]);
    let feat = listed["workspaces"]
        .as_array()
        .and_then(|list| list.iter().find(|workspace| workspace["branch"] == "feat"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(feat["state"], "sleeping", "{listed}");
    assert!(fixture.ok(&["ws", "ls"]).contains("sleeping"));
    assert!(
        fixture.ok(&["ports"]).contains(&web_port.to_string()),
        "the lease stays"
    );
    assert!(fixture
        .ok(&["ws", "sleep", "feat"])
        .contains("already asleep"));

    let woken = fixture.json(&["ws", "wake", "feat", "--json"]);
    assert_eq!(woken["started_services"], serde_json::json!(["api/web"]));
    assert_eq!(woken["failed"], serde_json::json!([]));
    let after = fixture.services();
    assert!(
        after.contains(&("api/web".to_string(), true, Some(web_port))),
        "{after:?}"
    );
    assert!(
        after.contains(&("api/worker".to_string(), false, None)),
        "only what ran comes back: {after:?}"
    );
    assert!(
        !state_file(&root).contains("sleeping"),
        "{}",
        state_file(&root)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("workspace--feat/api/notes.txt")).expect("notes"),
        "kept\n"
    );
    assert!(fixture.ok(&["ws", "wake", "feat"]).contains("not asleep"));
}
