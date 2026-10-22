//! `pom host` and `pom -H` through the real binary, with a fake `ssh` first on PATH: it records its argv and
//! runs the remote command line with `sh` in a scratch home whose `~/.local/bin/pom` is this build, against the
//! same temp state. No real SSH is involved.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use pom_agent::{record_event, AgentState, Identity, SessionEvent};
use pom_paths::StateDir;
use serde_json::Value;

const POM: &str = env!("CARGO_BIN_EXE_pom");

const FAKE_SSH: &str = r#"#!/bin/sh
for arg in "$@"; do printf '%s\n' "$arg"; done >> "$FAKE_SSH_LOG"
printf '%s\n' '--end--' >> "$FAKE_SSH_LOG"
while [ $# -gt 0 ]; do
  case "$1" in
    --) shift; break ;;
    -o|-p) shift 2 ;;
    *) shift ;;
  esac
done
case "$1" in
  *down*) echo "ssh: connect to host $1 port 22: Connection refused" >&2; exit 255 ;;
esac
cd "$HOME" || exit 1
exec /bin/sh -c "$2"
"#;

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomr")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::write(
            root.join("pom.yml"),
            r#"session: demo
default_branch: main
repos:
  api:
    services:
      web:
        type: backend
        cmd: "echo web-banner-$PORT; exec nc -lk 127.0.0.1 $PORT"
"#,
        )
        .expect("pom.yml");
        let bin = temp.path().join("bin");
        let home_bin = temp.path().join("home/.local/bin");
        for dir in [
            &bin,
            &home_bin,
            &temp.path().join("s"),
            &temp.path().join("run"),
        ] {
            std::fs::create_dir_all(dir).expect("dir");
        }
        executable(&bin.join("ssh"), FAKE_SSH);
        std::os::unix::fs::symlink(POM, home_bin.join("pom")).expect("remote pom");
        Fixture { temp }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn config(&self) -> String {
        self.root().join("pom.yml").to_string_lossy().into_owned()
    }

    fn log(&self) -> PathBuf {
        self.temp.path().join("ssh.log")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(POM);
        let path = format!(
            "{}:{}",
            self.temp.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        command
            .args(args)
            .current_dir(self.temp.path())
            .env("PATH", path)
            .env("HOME", self.temp.path().join("home"))
            .env("XDG_RUNTIME_DIR", self.temp.path().join("run"))
            .env("POMELO_CONFIG_DIR", self.temp.path().join("config"))
            .env("FAKE_SSH_LOG", self.log())
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env_remove("POM_AGENT_HOLDER")
            .stdin(Stdio::null());
        command
    }

    fn pom(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("pom")
    }

    fn ssh_calls(&self) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(self.log()).unwrap_or_default();
        text.split("--end--\n")
            .filter(|call| !call.is_empty())
            .map(|call| call.lines().map(str::to_string).collect())
            .collect()
    }

    fn add_dev(&self) {
        let added = self.pom(&["host", "add", "dev", "ssh://dev@dev.example.net:2222"]);
        assert!(added.status.success(), "{}", text(&added));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let dir = pom_ptyhost::SocketDir::new(self.temp.path().join("s"));
        let names: Vec<String> = dir.holders().into_iter().map(|(name, _)| name).collect();
        dir.kill_holders_now(&names);
    }
}

fn executable(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, script).expect("script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", text(output)))
}

#[test]
fn hosts_are_added_listed_tested_and_removed() {
    let fixture = Fixture::new();
    fixture.add_dev();
    let file =
        std::fs::read_to_string(fixture.temp.path().join("config/hosts.json")).expect("hosts.json");
    let file: Value = serde_json::from_str(&file).expect("json");
    assert_eq!(file["schema"], "pom.hosts/v1");
    assert_eq!(
        file["hosts"]["dev"]["url"],
        "ssh://dev@dev.example.net:2222"
    );

    let listed = json(&fixture.pom(&["host", "ls", "--json"]));
    assert_eq!(listed["hosts"][0]["name"], "dev");
    assert_eq!(listed["hosts"][0]["destination"], "dev@dev.example.net");
    assert_eq!(listed["hosts"][0]["port"], 2222);

    let tested = fixture.pom(&["host", "test", "dev", "--json"]);
    assert!(tested.status.success(), "{}", text(&tested));
    let tested = json(&tested);
    assert_eq!(tested["reachable"], true);
    assert_eq!(tested["remote_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(tested["version_mismatch"], false);
    let plain = text(&fixture.pom(&["host", "test", "dev"]));
    assert!(plain.contains("dev: connected, pom "), "{plain}");

    let bad = fixture.pom(&["host", "add", "evil", "ssh://-oProxyCommand=x"]);
    assert_eq!(bad.status.code(), Some(1));

    let removed = json(&fixture.pom(&["host", "rm", "dev", "--json"]));
    assert_eq!(removed["removed"], "dev");
    let empty = json(&fixture.pom(&["host", "ls", "--json"]));
    assert_eq!(empty["hosts"], Value::Array(Vec::new()));
}

#[test]
fn an_older_remote_pom_is_flagged_as_a_version_mismatch() {
    let fixture = Fixture::new();
    let old = fixture.temp.path().join("home/old-pom");
    executable(&old, "#!/bin/sh\necho pom 0.1.0\n");
    let added = fixture.pom(&[
        "host",
        "add",
        "old",
        "ssh://192.0.2.10",
        "--pom-path",
        "~/old-pom",
    ]);
    assert!(added.status.success(), "{}", text(&added));
    let tested = fixture.pom(&["host", "test", "old", "--json"]);
    assert!(tested.status.success(), "{}", text(&tested));
    assert_eq!(json(&tested)["version_mismatch"], true);
    assert!(String::from_utf8_lossy(&tested.stderr).contains("warning: old runs pom 0.1.0"));
}

#[test]
fn remote_json_and_exit_codes_come_back_verbatim() {
    let fixture = Fixture::new();
    fixture.add_dev();
    let config = fixture.config();

    let local = fixture.pom(&["-c", &config, "ws", "ls", "--json"]);
    let remote = fixture.pom(&["-H", "dev", "-c", &config, "ws", "ls", "--json"]);
    assert!(local.status.success(), "{}", text(&local));
    assert_eq!(remote.stdout, local.stdout);
    assert_eq!(remote.status.code(), Some(0));
    let listed = json(&remote);
    assert_eq!(listed["schema"], "pom.workspace/v1");
    assert_eq!(listed["workspaces"].as_array().map(Vec::len), Some(2));

    let after = fixture.pom(&[
        "-c",
        &config,
        "--host",
        "dev",
        "get",
        "workspaces",
        "-o",
        "json",
    ]);
    assert_eq!(
        after.stdout,
        fixture
            .pom(&["-c", &config, "get", "workspaces", "-o", "json"])
            .stdout
    );

    let usage = fixture.pom(&["-H", "dev", "frobnicate"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(text(&usage).contains("unknown command frobnicate"));

    let calls = fixture.ssh_calls();
    let call = calls.last().expect("an ssh call");
    for expected in [
        "ControlMaster=auto",
        "ControlPersist=10m",
        "BatchMode=yes",
        "-T",
        "2222",
    ] {
        assert!(
            call.iter().any(|arg| arg == expected),
            "{expected} in {call:?}"
        );
    }
    let control = call
        .iter()
        .find(|arg| arg.starts_with("ControlPath="))
        .expect("control path");
    assert!(control.ends_with("/run/pom-ssh/%C"), "{control}");
    let dashes = call.iter().position(|arg| arg == "--").expect("--");
    assert_eq!(call[dashes + 1], "dev@dev.example.net");
}

#[test]
fn arguments_with_quotes_dollars_and_newlines_reach_the_remote_intact() {
    let fixture = Fixture::new();
    fixture.add_dev();
    let config = fixture.config();
    let name = "it's \"new\" $HOME `id` $(id)\nline two; Tiếng Việt";
    let renamed = fixture.pom(&["-H", "dev", "-c", &config, "ws", "rename", "feat", name]);
    assert!(renamed.status.success(), "{}", text(&renamed));
    let listed = json(&fixture.pom(&["-c", &config, "ws", "ls", "--json"]));
    let feat = listed["workspaces"]
        .as_array()
        .and_then(|list| list.iter().find(|ws| ws["branch"] == "feat"))
        .expect("feat");
    assert_eq!(feat["name"], name);
}

#[test]
fn stdin_stdout_and_any_exit_code_pass_through() {
    let fixture = Fixture::new();
    let echo = fixture.temp.path().join("home/echo-pom");
    executable(
        &echo,
        "#!/bin/sh\necho \"args: $*\"\ncat\necho to-stderr >&2\nexit 42\n",
    );
    let added = fixture.pom(&[
        "host",
        "add",
        "echo",
        "ssh://echo.example.net",
        "--pom-path",
        "~/echo-pom",
    ]);
    assert!(added.status.success(), "{}", text(&added));
    let mut child = fixture
        .command(&["-H", "echo", "agent", "send", "feat/dev", "--file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"from stdin\n")
        .expect("write");
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "args: agent send feat/dev --file -\nfrom stdin\n"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "to-stderr\n");
}

#[test]
fn an_unreachable_host_exits_69_with_a_json_error_when_json_was_asked() {
    let fixture = Fixture::new();
    let added = fixture.pom(&["host", "add", "lab", "ssh://down.example.net"]);
    assert!(added.status.success(), "{}", text(&added));

    let failed = fixture.pom(&["-H", "lab", "status", "-o", "json"]);
    assert_eq!(failed.status.code(), Some(69));
    let error = json(&failed);
    assert_eq!(error["error"]["code"], "unreachable");
    assert_eq!(error["error"]["host"], "lab");
    assert!(error["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("Connection refused")));

    let plain = fixture.pom(&["-H", "lab", "status"]);
    assert_eq!(plain.status.code(), Some(69));
    assert!(plain.stdout.is_empty());
    assert!(text(&plain).contains("could not reach host lab over ssh"));

    let tested = fixture.pom(&["host", "test", "lab", "--json"]);
    assert_eq!(tested.status.code(), Some(69));
    assert_eq!(json(&tested)["reachable"], false);

    let unknown = fixture.pom(&["-H", "ghost", "ws", "ls", "--json"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert_eq!(json(&unknown)["error"]["code"], "unknown_host");
}

#[test]
fn services_start_stop_and_logs_on_the_remote() {
    let fixture = Fixture::new();
    fixture.add_dev();
    let config = fixture.config();
    let remote = |args: &[&str]| {
        let mut full = vec!["-H", "dev", "-c", config.as_str(), "-w", "feat"];
        full.extend_from_slice(args);
        fixture.pom(&full)
    };

    let started = remote(&["start", "web"]);
    assert!(started.status.success(), "{}", text(&started));
    assert!(text(&started).contains("started api/web on port"));

    let status = json(&remote(&["status", "-o", "json"]));
    assert_eq!(status["workspace"], "feat");
    assert_eq!(status["services"][0]["up"], true);

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let logs = text(&remote(&["logs", "web"]));
        if logs.contains("web-banner-") {
            break;
        }
        assert!(Instant::now() < deadline, "no banner in {logs:?}");
        std::thread::sleep(Duration::from_millis(100));
    }

    let stopped = remote(&["stop", "web"]);
    assert_eq!(text(&stopped).trim(), "stopped api/web");
    let status = json(&remote(&["status", "-o", "json"]));
    assert_eq!(status["services"][0]["up"], false);
}

#[test]
fn agent_ls_comes_back_verbatim() {
    let fixture = Fixture::new();
    fixture.add_dev();
    let state = StateDir::new(fixture.temp.path().join("state/pom"));
    let identity = Identity {
        holder: "ws-feat-claude".into(),
        role: "dev".into(),
        project: "demo".into(),
        branch: "feat".into(),
        driver: "claude".into(),
    };
    let event = SessionEvent {
        session_id: "session-1".into(),
        event: "UserPromptSubmit".into(),
        state: Some(AgentState::Thinking),
        ..SessionEvent::default()
    };
    record_event(&state, &identity, &event).expect("event");
    let config = fixture.config();
    let local = fixture.pom(&["-c", &config, "agent", "ls", "feat", "--json"]);
    assert!(local.status.success(), "{}", text(&local));
    let remote = fixture.pom(&["-H", "dev", "-c", &config, "agent", "ls", "feat", "--json"]);
    assert_eq!(remote.stdout, local.stdout);
    assert_eq!(json(&remote)["sessions"][0]["role"], "dev");
}
