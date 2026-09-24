//! Opt-in (`POM_GO_ORACLE=1`): the tools both cores offer are declared the same way, so an agent sees
//! the same schemas and hints whichever binary answers.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const POM: &str = env!("CARGO_BIN_EXE_pom");

fn tools(binary: &Path, cwd: &Path, state: &Path) -> Vec<Value> {
    let mut child = Command::new(binary)
        .arg("mcp")
        .current_dir(cwd)
        .env("XDG_STATE_HOME", state)
        .env("POM_SKIP_GLOBAL_HOOK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("mcp");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
        )
        .expect("write");
    }
    let output = child.wait_with_output().expect("output");
    let reply: Value = serde_json::from_slice(
        output
            .stdout
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default(),
    )
    .expect("reply");
    reply["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn ascii(text: &str) -> String {
    text.replace(" \u{2014} ", " - ").replace('\u{2192}', "->")
}

#[test]
fn shared_tools_match_the_previous_core() {
    if std::env::var_os("POM_GO_ORACLE").is_none() {
        return;
    }
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("workspace--main/api/.git")).expect("repo");
    std::fs::write(
        root.join("pom.yml"),
        "session: demo\ndefault_branch: main\nrepos:\n  api:\n    services:\n      web: rails s\n",
    )
    .expect("pom.yml");
    let go_binary = temp.path().join("pom-go");
    let built = Command::new("go")
        .args(["build", "-o"])
        .arg(&go_binary)
        .arg("./cmd/pom")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .status()
        .expect("go build");
    assert!(built.success());
    let state = temp.path().join("state");
    let go = tools(&go_binary, &root, &state);
    let rust = tools(Path::new(POM), &root, &state);
    assert!(!go.is_empty() && !rust.is_empty());
    for ours in &rust {
        let name = ours["name"].as_str().expect("name");
        let theirs = go
            .iter()
            .find(|tool| tool["name"] == ours["name"])
            .unwrap_or_else(|| panic!("{name} is not a tool of the previous core"));
        assert_eq!(ours["inputSchema"], theirs["inputSchema"], "{name} schema");
        assert_eq!(
            ours["annotations"], theirs["annotations"],
            "{name} annotations"
        );
        assert_eq!(
            ours["description"].as_str(),
            Some(ascii(theirs["description"].as_str().unwrap_or_default()).as_str()),
            "{name} description"
        );
    }
}
