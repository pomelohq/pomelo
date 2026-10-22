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
        match self.command(&["server", "stop"]).output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => eprintln!("pom server stop: {}", text(&output)),
            Err(error) => eprintln!("pom server stop: {error}"),
        }
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

impl Fixture {
    fn envs(&self) -> Vec<(String, String)> {
        let path = |relative: &str| {
            self.temp
                .path()
                .join(relative)
                .to_string_lossy()
                .into_owned()
        };
        vec![
            ("HOME".into(), path("home")),
            (
                "FAKE_SSH_LOG".into(),
                self.log().to_string_lossy().into_owned(),
            ),
            ("XDG_STATE_HOME".into(), path("state")),
            ("XDG_RUNTIME_DIR".into(), path("run")),
            ("POMELO_CONFIG_DIR".into(), path("config")),
            ("POM_NO_PROXY".into(), "1".into()),
            ("POM_WEB_PORT".into(), "1".into()),
            ("POM_PTY_SOCK_DIR".into(), path("s")),
            ("SHELL".into(), "/bin/sh".into()),
            ("FAKE_POM".into(), POM.into()),
            ("FAKE_TRANSCRIPTS".into(), path("transcripts")),
        ]
    }

    fn remote_source(&self) -> pom_remote::RemoteSource {
        let ssh = pom_remote::Ssh::custom(self.temp.path().join("bin/ssh"), None, self.envs());
        let host = pom_remote::Hosts::load(&self.temp.path().join("config/hosts.json"))
            .and_then(|hosts| hosts.get("dev"))
            .expect("host dev");
        pom_remote::RemoteSource::new(host, self.config(), ssh)
    }

    fn local_source(&self) -> pom_remote::LocalSource {
        pom_remote::LocalSource::new(
            self.root().join("pom.yml"),
            StateDir::new(self.temp.path().join("state/pom")),
            pom_ptyhost::SocketDir::new(self.temp.path().join("s")),
            PathBuf::from(POM),
        )
    }
}

#[test]
fn the_remote_source_reads_what_the_local_one_does() {
    use pom_remote::{ProjectSource, ServiceAction};
    let fixture = Fixture::new();
    fixture.add_dev();
    let remote = fixture.remote_source();
    let local = fixture.local_source();
    assert_eq!(remote.location(), "dev (dev@dev.example.net:2222)");

    let started = remote
        .act("feat", ServiceAction::Start, "web")
        .expect("start over ssh");
    assert!(started.contains("started api/web"), "{started}");

    assert_eq!(
        remote.workspaces().expect("remote"),
        local.workspaces().expect("local")
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let logs = remote.logs("feat", "api/web").expect("logs");
        if logs.contains("web-banner-") {
            assert!(local
                .logs("feat", "api/web")
                .expect("local logs")
                .contains("web-banner-"));
            break;
        }
        assert!(Instant::now() < deadline, "no banner in {logs:?}");
        std::thread::sleep(Duration::from_millis(100));
    }

    let ready = Instant::now() + Duration::from_secs(15);
    while !remote.status("feat").expect("status").services[0].ready {
        assert!(Instant::now() < ready, "web never became ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    let status = remote.status("feat").expect("status");
    assert_eq!(status, local.status("feat").expect("local status"));
    assert!(status.services[0].up);

    assert_eq!(
        remote.agents("feat").expect("agents"),
        local.agents("feat").expect("local")
    );

    remote
        .act("feat", ServiceAction::Stop, "web")
        .expect("stop");
    assert!(!remote.status("feat").expect("status").services[0].up);
    match remote.status("nope") {
        Err(pom_remote::SourceError::Failed(message)) => {
            assert!(message.contains("no workspace nope"), "{message}")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_connect_flow_lists_the_remote_projects_and_reports_an_unreachable_host() {
    let fixture = Fixture::new();
    fixture.add_dev();
    let state = StateDir::new(fixture.temp.path().join("state/pom"));
    let mut sessions = pom_sessions::Sessions::load(&state);
    sessions.touch("demo", &fixture.root().to_string_lossy(), 10);
    sessions.touch_remote("elsewhere", "lab", "/srv/elsewhere", 20);
    sessions.save(&state).expect("sessions");

    let listed = json(&fixture.pom(&["project", "ls", "--json"]));
    assert_eq!(listed["schema"], "pom.project/v1");
    assert_eq!(listed["projects"].as_array().map(Vec::len), Some(1));

    let ssh = pom_remote::Ssh::custom(fixture.temp.path().join("bin/ssh"), None, fixture.envs());
    let hosts =
        pom_remote::Hosts::load(&fixture.temp.path().join("config/hosts.json")).expect("hosts");
    let projects =
        pom_remote::remote_projects(&ssh, &hosts.get("dev").expect("dev")).expect("projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "demo");
    assert_eq!(projects[0].config, fixture.root().join("pom.yml"));
    assert!(!projects[0].missing);

    let down = pom_remote::Host {
        name: "lab".into(),
        url: pom_remote::HostUrl::parse("ssh://down.example.net").expect("url"),
        pom_path: None,
        transport: None,
    };
    match pom_remote::remote_projects(&ssh, &down) {
        Err(pom_remote::SourceError::Unreachable(reason)) => {
            assert!(reason.contains("Connection refused"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
}

impl Fixture {
    fn register_project(&self) {
        let state = StateDir::new(self.temp.path().join("state/pom"));
        let mut sessions = pom_sessions::Sessions::load(&state);
        sessions.touch("demo", &self.root().to_string_lossy(), 10);
        sessions.save(&state).expect("sessions");
    }

    fn server_source(&self, host: &str) -> pom_remote::ServerSource {
        let ssh = pom_remote::Ssh::custom(self.temp.path().join("bin/ssh"), None, self.envs());
        let host = pom_remote::Hosts::load(&self.temp.path().join("config/hosts.json"))
            .and_then(|hosts| hosts.get(host))
            .expect("host");
        pom_remote::ServerSource::new(host, self.config(), ssh)
    }
}

#[test]
fn the_server_source_reads_what_the_local_one_does() {
    use pom_remote::{ProjectSource, ServiceAction, Via};
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let server = fixture.server_source("dev");
    let local = fixture.local_source();
    assert_eq!(server.transport(), None);

    assert_eq!(
        server.workspaces().expect("server"),
        local.workspaces().expect("local")
    );
    let transport = server.transport().expect("transport");
    assert_eq!(transport.via, Via::Server);
    assert_eq!(
        transport.remote_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(!transport.version_mismatch);
    let calls = fixture.ssh_calls();
    let call = calls.last().expect("an ssh call");
    assert!(call.iter().any(|arg| arg == "IPQoS=lowdelay"), "{call:?}");
    assert!(call.iter().any(|arg| arg == "-T"), "{call:?}");
    assert!(
        call.last()
            .is_some_and(|line| line.ends_with(" server proxy")),
        "{call:?}"
    );

    let started = server
        .act("feat", ServiceAction::Start, "web")
        .expect("start through the server");
    assert!(started.contains("started api/web"), "{started}");

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let logs = server.logs("feat", "api/web").expect("logs");
        if logs.contains("web-banner-") {
            assert_eq!(logs, local.logs("feat", "api/web").expect("local logs"));
            break;
        }
        assert!(Instant::now() < deadline, "no banner in {logs:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let ready = Instant::now() + Duration::from_secs(15);
    while !server.status("feat").expect("status").services[0].ready {
        assert!(Instant::now() < ready, "web never became ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        server.status("feat").expect("status"),
        local.status("feat").expect("local status")
    );
    assert_eq!(
        server.agents("feat").expect("agents"),
        local.agents("feat").expect("local agents")
    );
    match server.agent_turn("feat", "dev") {
        Err(pom_remote::SourceError::Failed(message)) => {
            assert!(message.contains("no agent feat/dev"), "{message}")
        }
        other => panic!("{other:?}"),
    }

    let status = json(&fixture.pom(&["server", "status", "--json"]));
    assert_eq!(status["schema"], "pom.server/v1");
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert!(status["connections"]
        .as_u64()
        .is_some_and(|count| count >= 2));

    server
        .act("feat", ServiceAction::Stop, "web")
        .expect("stop through the server");
    assert!(!server.status("feat").expect("status").services[0].up);
    match server.status("nope") {
        Err(pom_remote::SourceError::Failed(message)) => {
            assert!(message.contains("no workspace nope"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fixture.ssh_calls().len(),
        calls.len(),
        "one ssh session for every read"
    );
}

#[test]
fn a_shell_on_the_server_runs_what_is_typed() {
    use pom_remote::ProjectSource;
    use pom_rpc::pty::{Mode, Request, Target};
    use pom_rpc::StreamEvent;
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let server = fixture.server_source("dev");
    assert!(
        server.pty("feat").is_none(),
        "unknown before the server answered"
    );
    server.workspaces().expect("connect");
    assert!(server.transport().expect("transport").terminals);
    let terminals = server.pty("feat").expect("terminals");
    let request = Request {
        target: Target::Shell { cwd: None },
        cols: 80,
        rows: 24,
        mode: Mode::ReadWrite,
        primary: true,
        claim: None,
    };
    let terminal = terminals.attach(&request).expect("a shell");
    let holder = terminal.attached.holder.clone();
    assert!(holder.starts_with("term-demo-feat-"), "{holder}");
    terminal
        .control
        .input(b"echo hi-$((20 + 1))\n")
        .expect("type");
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !String::from_utf8_lossy(&seen).contains("hi-21") {
        assert!(
            Instant::now() < deadline,
            "{:?}",
            String::from_utf8_lossy(&seen)
        );
        match terminal.output.recv_timeout(Duration::from_millis(100)) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                terminal.control.ack(offset + bytes.len() as u64);
                seen.extend_from_slice(&bytes);
            }
            Ok(StreamEvent::Reset { .. }) | Err(pom_rpc::Error::Timeout) => {}
            other => panic!("{other:?}"),
        }
    }
    let holders = pom_ptyhost::SocketDir::new(fixture.temp.path().join("s"));
    assert!(holders.holder_alive(&holder));
    terminals.close(&holder).expect("close");
    assert!(!holders.holder_alive(&holder));
}

const FAKE_AGENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-agent.sh");

impl Fixture {
    /// A source whose server finds a stand-in for the agent CLI first on its PATH, so no real agent ever
    /// starts: it runs the scripted agent on the conversation id pom hands it.
    fn server_source_with_agent_cli(&self) -> pom_remote::ServerSource {
        std::fs::create_dir_all(self.temp.path().join("transcripts")).expect("transcripts");
        let bin = self.temp.path().join("agent-bin");
        std::fs::create_dir_all(&bin).expect("agent bin");
        executable(
            &bin.join("claude"),
            &format!(
                "#!/bin/sh\nsession=\"\"\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in --session-id|--resume) session=\"$2\"; shift ;; esac\n  shift\ndone\nFAKE_SESSION=\"$session\" FAKE_TRANSCRIPT=\"$FAKE_TRANSCRIPTS/$session.jsonl\" POM=\"$FAKE_POM\" exec /bin/sh '{FAKE_AGENT}'\n"
            ),
        );
        let mut envs = self.envs();
        envs.push((
            "PATH".into(),
            format!(
                "{}:{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
                bin.display(),
                self.temp.path().join("bin").display()
            ),
        ));
        let ssh = pom_remote::Ssh::custom(self.temp.path().join("bin/ssh"), None, envs);
        let host = pom_remote::Hosts::load(&self.temp.path().join("config/hosts.json"))
            .and_then(|hosts| hosts.get("dev"))
            .expect("host");
        pom_remote::ServerSource::new(host, self.config(), ssh)
    }
}

fn wait_snapshot(
    watch: &pom_remote::Watch,
    what: &str,
    done: impl Fn(&pom_remote::Snapshot) -> bool,
) -> pom_remote::Snapshot {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = watch.snapshot();
        if done(&snapshot) {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "no {what}: {snapshot:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_server_starts_an_agent_pushes_its_state_and_takes_what_is_typed() {
    use pom_remote::ProjectSource;
    use pom_rpc::pty::{Mode, Request, Target};
    use pom_rpc::StreamEvent;
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let source = std::sync::Arc::new(fixture.server_source_with_agent_cli());
    let watch = pom_remote::Watch::start(source.clone(), "feat", std::sync::Arc::new(|| {}));
    watch.set_visible(true);
    wait_snapshot(&watch, "pushes", |snapshot| snapshot.pushing);
    assert!(source.transport().is_some_and(|info| info.agents));

    watch.start_agent("claude");
    let deadline = Instant::now() + Duration::from_secs(30);
    let started = loop {
        if let Some(view) = watch.take_started().pop() {
            break view;
        }
        let message = watch.snapshot().message.unwrap_or_default();
        assert!(!message.starts_with("could not"), "{message}");
        assert!(Instant::now() < deadline, "the agent did not start");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(started.role, "claude");
    let screen = pom_ptyhost::snapshot(
        &pom_ptyhost::SocketDir::new(fixture.temp.path().join("s")),
        &started.holder,
        Duration::from_secs(5),
    )
    .unwrap_or_default();
    assert!(
        !String::from_utf8_lossy(&screen).contains("Welcome"),
        "the stand-in runs, not a real agent"
    );
    let holder = started.holder.clone();
    // Whoever starts a session drives it, so it reports to an orchestrator until the server hands it over.
    wait_snapshot(
        &watch,
        "the agent pushed, driven by the person",
        |snapshot| {
            snapshot
                .agents
                .iter()
                .any(|agent| agent.role == "claude" && agent.state == "idle")
                && snapshot
                    .presence
                    .iter()
                    .any(|presence| presence.holder == holder && presence.driven_by.is_none())
        },
    );

    watch.show_turn(Some("claude".into()));
    let terminal = source
        .pty("feat")
        .expect("terminals")
        .attach(&Request {
            target: Target::Agent("claude".into()),
            cols: 80,
            rows: 24,
            mode: Mode::ReadWrite,
            primary: true,
            claim: None,
        })
        .expect("attach the agent");
    assert_eq!(terminal.attached.holder, holder);
    terminal.control.input(b"hello-agent\n").expect("type");
    let snapshot = wait_snapshot(&watch, "the turn pushed", |snapshot| {
        matches!(&snapshot.turn, Some((_, Ok(Some(turn)))) if turn.text.contains("echo: hello-agent"))
            && snapshot.agents.iter().any(|agent| agent.turn == 1)
    });
    assert!(snapshot.pushing);
    while let Ok(event) = terminal.output.recv_timeout(Duration::from_millis(50)) {
        if let StreamEvent::Data { offset, bytes } = event {
            terminal.control.ack(offset + bytes.len() as u64);
        }
    }
    source
        .pty("feat")
        .expect("terminals")
        .close(&holder)
        .expect("stop the agent");
}

#[test]
fn a_dropped_server_session_is_reopened_from_scratch() {
    use pom_remote::ProjectSource;
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let server = fixture.server_source("dev");
    let first = server.workspaces().expect("first");
    let stopped = fixture.pom(&["server", "stop"]);
    assert!(stopped.status.success(), "{}", text(&stopped));
    assert_eq!(server.workspaces().expect("after the server went"), first);
    assert_eq!(fixture.ssh_calls().len(), 2);
}

#[test]
fn an_older_remote_pom_without_a_server_is_read_over_plain_ssh() {
    use pom_remote::{ProjectSource, Via};
    let fixture = Fixture::new();
    fixture.register_project();
    let old = fixture.temp.path().join("home/old-pom");
    executable(
        &old,
        &format!(
            "#!/bin/sh\ncase \"$1\" in server) echo 'unknown command server' >&2; echo 'usage: pom ...' >&2; exit 2 ;; esac\nexec '{POM}' \"$@\"\n"
        ),
    );
    let added = fixture.pom(&[
        "host",
        "add",
        "old",
        "ssh://192.0.2.10",
        "--pom-path",
        "~/old-pom",
    ]);
    assert!(added.status.success(), "{}", text(&added));
    let server = fixture.server_source("old");
    assert_eq!(
        server.workspaces().expect("over ssh"),
        fixture.local_source().workspaces().expect("local")
    );
    let transport = server.transport().expect("transport");
    assert_eq!(transport.via, Via::Ssh);
    let reason = transport.fallback_reason.expect("a reason");
    assert!(reason.contains("update pom on the VM"), "{reason}");
    assert!(server.status("feat").is_ok());
}

#[test]
fn an_unreachable_host_is_unreachable_through_the_server_source_too() {
    use pom_remote::ProjectSource;
    let fixture = Fixture::new();
    let added = fixture.pom(&["host", "add", "lab", "ssh://down.example.net"]);
    assert!(added.status.success(), "{}", text(&added));
    match fixture.server_source("lab").workspaces() {
        Err(pom_remote::SourceError::Unreachable(reason)) => {
            assert!(reason.contains("Connection refused"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_server_proxy_starts_a_server_and_reconnect_never_does() {
    let fixture = Fixture::new();
    let gone = fixture.pom(&["server", "proxy", "--reconnect"]);
    assert_eq!(gone.status.code(), Some(70));
    assert_eq!(
        String::from_utf8_lossy(&gone.stderr).trim(),
        r#"{"error":"server_gone"}"#
    );
    let missing = fixture.pom(&["server", "status"]);
    assert_eq!(missing.status.code(), Some(3));

    let mut child = fixture
        .command(&["server", "proxy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("proxy");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let transport = pom_rpc::Transport::new(stdout, stdin);
    let (connection, hello) = pom_rpc::Connection::builder(transport)
        .connect(pom_rpc::Hello::new(env!("CARGO_PKG_VERSION")))
        .expect("handshake through the proxy");
    assert_eq!(hello.pom_version, env!("CARGO_PKG_VERSION"));
    let info = connection
        .request("server.info", Value::Null)
        .expect("server.info");
    assert_eq!(info["schema"], "pom.server/v1");
    let pid = info["pid"].as_u64().expect("pid");
    assert_ne!(pid, u64::from(child.id()), "the server outlives the proxy");
    connection.close();
    let exited = child.wait().expect("proxy exit");
    assert!(exited.success());

    let status = json(&fixture.pom(&["server", "status", "--json"]));
    assert_eq!(status["pid"].as_u64(), Some(pid));
    let socket = fixture
        .temp
        .path()
        .join(format!("run/pom-server/{}.sock", env!("CARGO_PKG_VERSION")));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&socket)
            .expect("socket")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let resumed = fixture
        .command(&["server", "proxy", "--reconnect"])
        .output()
        .expect("reconnect");
    assert!(resumed.status.success(), "{}", text(&resumed));

    let stopped = fixture.pom(&["server", "stop"]);
    assert!(stopped.status.success(), "{}", text(&stopped));
    assert!(text(&stopped).contains(&format!("(pid {pid})")));
    assert!(!socket.exists());
    let log = std::fs::read_to_string(fixture.temp.path().join(format!(
        "state/pom/server/{}.log",
        env!("CARGO_PKG_VERSION")
    )))
    .expect("server log");
    assert!(log.contains("method=server.info"), "{log}");
    assert!(log.contains("exit: stopped"), "{log}");
}

#[test]
fn a_real_sshd_keystroke_round_trip() {
    use pom_remote::ProjectSource;
    use pom_rpc::pty::{Mode, Request, Target};
    use pom_rpc::StreamEvent;
    if std::env::var_os("POM_REAL_SSH").is_none() {
        return;
    }
    let fixture = Fixture::new();
    let state = StateDir::from_env();
    let mut sessions = pom_sessions::Sessions::load(&state);
    sessions.touch("demo", &fixture.root().to_string_lossy(), 10);
    sessions.save(&state).expect("sessions");
    let host = pom_remote::Host {
        name: "localhost".into(),
        url: pom_remote::HostUrl::parse("ssh://localhost").expect("url"),
        pom_path: Some(POM.into()),
        transport: None,
    };
    let server = pom_remote::ServerSource::new(
        host,
        fixture.config(),
        pom_remote::Ssh::custom("ssh".into(), None, Vec::new()),
    );
    server.workspaces().expect("through sshd");
    let terminals = server.pty("feat").expect("terminals");
    let request = Request {
        target: Target::Shell { cwd: None },
        cols: 80,
        rows: 24,
        mode: Mode::ReadWrite,
        primary: true,
        claim: None,
    };
    let terminal = terminals.attach(&request).expect("a shell");
    terminal.control.input(b"exec cat\n").expect("type");
    std::thread::sleep(Duration::from_millis(500));
    let mut times = Vec::new();
    for index in 0..200u8 {
        let key = b'a' + index % 26;
        let started = Instant::now();
        terminal.control.input(&[key]).expect("key");
        loop {
            match terminal.output.recv_timeout(Duration::from_secs(5)) {
                Ok(StreamEvent::Data { offset, bytes }) => {
                    terminal.control.ack(offset + bytes.len() as u64);
                    if bytes.contains(&key) {
                        break;
                    }
                }
                other => panic!("{other:?}"),
            }
        }
        times.push(started.elapsed());
    }
    times.sort();
    let (p50, p99) = (times[100], times[197]);
    eprintln!("real sshd keystroke round trip: p50 {p50:?} p99 {p99:?}");
    if let Err(error) = terminals.close(&terminal.attached.holder) {
        eprintln!("close: {error}");
    }
    let stopped = Command::new(POM).args(["server", "stop"]).output();
    if let Err(error) = stopped {
        eprintln!("pom server stop: {error}");
    }
    assert!(p50 < Duration::from_millis(60), "p50 {p50:?}");
    assert!(p99 < Duration::from_millis(120), "p99 {p99:?}");
}
