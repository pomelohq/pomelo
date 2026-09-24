//! `pom mcp` as an agent would drive it: JSON-RPC lines on stdin, answers on stdout.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const POM: &str = env!("CARGO_BIN_EXE_pom");

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomm")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for workspace in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(workspace).join(".git")).expect("worktree");
        }
        std::fs::create_dir_all(temp.path().join("zdot")).expect("zdot");
        std::fs::write(
            root.join("pom.yml"),
            r#"session: demo
default_branch: main
repos:
  api:
    env:
      GREETING: "hi-{{branch.safe}}"
    commands:
      test: "echo tests-pass-$GREETING"
    services:
      web:
        type: backend
        cmd: "echo web-banner-$PORT; exec nc -lk 127.0.0.1 $PORT"
"#,
        )
        .expect("pom.yml");
        Fixture { temp }
    }

    /// Sends `calls` (tool name, arguments) after the handshake from `cwd`; returns each call's text and
    /// error flag.
    fn call(&self, cwd: &Path, calls: &[(&str, Value)]) -> Vec<(String, bool)> {
        let mut lines = vec![
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        ];
        for (index, (name, arguments)) in calls.iter().enumerate() {
            lines.push(json!({
                "jsonrpc": "2.0",
                "id": index + 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            }));
        }
        let replies = self.exchange(cwd, &lines);
        replies[1..]
            .iter()
            .map(|reply| {
                let result = &reply["result"];
                (
                    result["content"][0]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    result["isError"].as_bool().unwrap_or(false),
                )
            })
            .collect()
    }

    fn exchange(&self, cwd: &Path, lines: &[Value]) -> Vec<Value> {
        let mut child = Command::new(POM)
            .arg("mcp")
            .current_dir(cwd)
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("ZDOTDIR", self.temp.path().join("zdot"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("pom mcp");
        {
            let mut stdin = child.stdin.take().expect("stdin");
            for line in lines {
                writeln!(stdin, "{line}").expect("write");
            }
        }
        let output = child.wait_with_output().expect("output");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| serde_json::from_str(line).expect("json reply"))
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

#[test]
fn an_agent_in_a_workspace_sees_and_drives_it() {
    let fixture = Fixture::new();
    let feat = fixture.temp.path().join("project/workspace--feat/api");

    let listed = fixture.exchange(
        &feat,
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let names: Vec<&str> = listed[0]["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(
        names.contains(&"workspace_info") && names.contains(&"run_in_env"),
        "{names:?}"
    );

    let replies = fixture.call(
        &feat,
        &[
            ("service_start", json!({"service": "web", "repo": "api"})),
            ("services", json!({})),
            ("run_shortcut", json!({"repo": "api", "key": "test"})),
            (
                "run_in_env",
                json!({"repo": "api", "cmd": "echo env-$GREETING; exit 3"}),
            ),
            ("config_validate", json!({"yaml": "session: [broken"})),
            ("service_url", json!({"service": "nope"})),
            ("workspace_info", json!({})),
        ],
    );
    assert!(!replies[0].1, "{}", replies[0].0);
    assert!(
        replies[0].0.starts_with("start: web (port "),
        "{}",
        replies[0].0
    );
    let services: Value = serde_json::from_str(&replies[1].0).expect("services json");
    assert_eq!(services[0]["Service"], "web");
    assert_eq!(services[0]["Running"], true);
    assert!(
        replies[2]
            .0
            .starts_with("$ echo tests-pass-$GREETING\nexit 0\ntests-pass-hi-feat"),
        "{}",
        replies[2].0
    );
    assert!(
        replies[3].0.starts_with("exit 3\nenv-hi-feat"),
        "{}",
        replies[3].0
    );
    assert!(
        replies[4].1 && replies[4].0.starts_with("invalid: yaml parse error"),
        "{:?}",
        replies[4]
    );
    assert!(
        replies[5].1 && replies[5].0.contains("no service \"nope\""),
        "{:?}",
        replies[5]
    );
    let info: Value = serde_json::from_str(&replies[6].0).expect("info json");
    assert_eq!(info["branch"], "feat");
    assert_eq!(
        info["repos"][0]["services"][0]["tmux_window"],
        "svc-demo-feat-api-web"
    );
}

#[test]
fn main_is_read_only_for_commands_and_no_project_means_no_tools() {
    let fixture = Fixture::new();
    let main = fixture.temp.path().join("project");
    let replies = fixture.call(
        &main,
        &[("run_in_env", json!({"repo": "api", "cmd": "true"}))],
    );
    assert!(
        replies[0].1 && replies[0].0.contains("main is read-only"),
        "{:?}",
        replies[0]
    );

    let nowhere = fixture.exchange(
        fixture.temp.path(),
        &[json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    assert_eq!(nowhere[0]["result"]["tools"], json!([]));
}
