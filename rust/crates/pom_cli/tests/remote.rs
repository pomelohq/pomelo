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
exec /bin/sh -c "exec $2"
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
    assert_eq!(tested["remote_version"], tested["local_version"]);
    assert!(tested["remote_version"]
        .as_str()
        .unwrap_or_default()
        .starts_with(env!("CARGO_PKG_VERSION")));
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
fn the_server_shows_a_workspace_tree_and_reads_its_files() {
    use pom_remote::ProjectSource;
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let source_dir = fixture.root().join("workspace--feat/api/src");
    std::fs::create_dir_all(&source_dir).expect("src");
    std::fs::write(source_dir.join("main.rs"), "fn main() {}\n").expect("main.rs");
    let server = fixture.server_source("dev");
    assert!(
        server.files("feat").is_none(),
        "unknown before the server answered"
    );
    server.workspaces().expect("connect");
    let fs = server.files("feat").expect("files over the server");
    let mut feed = fs.watch(std::sync::Arc::new(|| {}));
    let deadline = Instant::now() + Duration::from_secs(15);
    let entries = loop {
        let complete = feed.poll().into_iter().find_map(|event| match event {
            files::TreeEvent::Snapshot {
                entries,
                complete: true,
            } => Some(entries),
            _ => None,
        });
        if let Some(entries) = complete {
            break entries;
        }
        assert!(Instant::now() < deadline, "no tree");
        std::thread::sleep(Duration::from_millis(20));
    };
    let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["api", "api/src", "api/src/main.rs"]);
    let read = fs.read("api/src/main.rs").expect("read");
    assert_eq!(read.text.as_deref(), Some("fn main() {}\n"));
    assert!(fs.read("../pom.yml").is_err());
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
    /// The server's environment with a stand-in for the agent CLI first on an explicit PATH. The tool path pom
    /// builds puts some well-known folders in front of PATH unless PATH has them, and reads version-manager
    /// folders from the environment, so those are named here too: no real agent CLI can come first.
    fn agent_cli_envs(&self) -> Vec<(String, String)> {
        std::fs::create_dir_all(self.temp.path().join("transcripts")).expect("transcripts");
        let bin = self.temp.path().join("agent-bin");
        std::fs::create_dir_all(&bin).expect("agent bin");
        executable(
            &bin.join("claude"),
            &format!(
                "#!/bin/sh\necho ran >>\"$FAKE_AGENT_RUNS\"\nsession=\"\"\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in --session-id|--resume) session=\"$2\"; shift ;; esac\n  shift\ndone\nFAKE_SESSION=\"$session\" FAKE_TRANSCRIPT=\"$FAKE_TRANSCRIPTS/$session.jsonl\" POM=\"$FAKE_POM\" exec /bin/sh '{FAKE_AGENT}'\n"
            ),
        );
        let nowhere = self
            .temp
            .path()
            .join("no-tools")
            .to_string_lossy()
            .into_owned();
        let mut envs = self.envs();
        envs.push((
            "PATH".into(),
            format!(
                "{}:{}:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin",
                bin.display(),
                self.temp.path().join("bin").display()
            ),
        ));
        for key in [
            "PNPM_HOME",
            "VOLTA_HOME",
            "XDG_DATA_HOME",
            "ASDF_DATA_DIR",
            "RBENV_ROOT",
            "PYENV_ROOT",
            "NVM_DIR",
        ] {
            envs.push((key.into(), nowhere.clone()));
        }
        envs.push((
            "FAKE_AGENT_RUNS".into(),
            self.agent_runs_file().to_string_lossy().into_owned(),
        ));
        envs
    }

    fn agent_runs_file(&self) -> PathBuf {
        self.temp.path().join("agent-runs")
    }

    /// How many times the stand-in agent CLI started.
    fn agent_runs(&self) -> usize {
        std::fs::read_to_string(self.agent_runs_file())
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn server_source_with_agent_cli(&self) -> pom_remote::ServerSource {
        self.server_source_for(&self.config())
    }

    fn server_source_for(&self, config: &str) -> pom_remote::ServerSource {
        let ssh = pom_remote::Ssh::custom(
            self.temp.path().join("bin/ssh"),
            None,
            self.agent_cli_envs(),
        );
        let host = pom_remote::Hosts::load(&self.temp.path().join("config/hosts.json"))
            .and_then(|hosts| hosts.get("dev"))
            .expect("host");
        pom_remote::ServerSource::new(host, config.to_string(), ssh)
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
    assert_eq!(fixture.agent_runs(), 1, "the stand-in ran");
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
fn a_stopped_server_is_reconnected_to_from_scratch_and_counted_as_restarted() {
    use pom_remote::ProjectSource;
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let server = fixture.server_source("dev");
    let first = server.workspaces().expect("first");
    let before = server.server_id();
    let stopped = fixture.pom(&["server", "stop"]);
    assert!(stopped.status.success(), "{}", text(&stopped));
    assert_eq!(server.workspaces().expect("after the server went"), first);
    let calls = fixture.ssh_calls();
    let proxies: Vec<&String> = calls.iter().filter_map(|call| call.last()).collect();
    assert_eq!(proxies.len(), 3, "{proxies:?}");
    assert!(
        proxies[1].ends_with(" server proxy --reconnect"),
        "{proxies:?}"
    );
    assert!(proxies[2].ends_with(" server proxy"), "{proxies:?}");
    assert_ne!(server.server_id(), before);
    assert_eq!(server.link().map(|report| report.restarts), Some(1));
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

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?}: {}", text(&output));
}

impl Fixture {
    /// A second project, `jobs`, whose main checkout is a real git repo so workspaces can be created there;
    /// its setup takes a moment, long enough to cancel a create during it.
    fn git_project(&self) -> String {
        let root = self.temp.path().join("jobs");
        let api = root.join("workspace--main/api");
        std::fs::create_dir_all(&api).expect("repo");
        git(&api, &["init", "-q", "-b", "main"]);
        std::fs::write(api.join("README"), "x\n").expect("readme");
        git(&api, &["add", "."]);
        git(&api, &["commit", "-q", "-m", "init"]);
        std::fs::write(
            root.join("pom.yml"),
            "session: jobs\ndefault_branch: main\nrepos:\n  api:\n    setup: [\"sleep 2\"]\n    seed: [\"touch seeded\"]\n    services:\n      web:\n        cmd: \"exec sleep 1000\"\n",
        )
        .expect("pom.yml");
        let state = StateDir::new(self.temp.path().join("state/pom"));
        let mut sessions = pom_sessions::Sessions::load(&state);
        sessions.touch("jobs", &root.to_string_lossy(), 20);
        sessions.save(&state).expect("sessions");
        root.join("pom.yml").to_string_lossy().into_owned()
    }
}

/// Follows job `id` from `since` until it ends, or until `stop` says so.
fn job_events(
    source: &dyn pom_remote::ProjectSource,
    id: &str,
    since: u64,
    stop: impl Fn(&pom_remote::server_api::Progress) -> bool,
) -> Vec<pom_remote::server_api::Progress> {
    use pom_remote::JobEvent;
    let feed = source.follow_job(id, since).expect("follow the job");
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "the job never ended: {events:?}");
        match feed.next(Duration::from_millis(200)) {
            JobEvent::Progress(progress) => {
                let done = progress.done || stop(&progress);
                events.push(progress);
                if done {
                    return events;
                }
            }
            JobEvent::Idle => {}
            JobEvent::Ended(reason) => panic!("pushes ended ({reason}) at {events:?}"),
        }
    }
}

fn started_stages(events: &[pom_remote::server_api::Progress]) -> Vec<usize> {
    events
        .iter()
        .filter(|event| event.kind == pom_remote::server_api::progress_kind::STAGE_STARTED)
        .filter_map(|event| event.stage)
        .collect()
}

#[test]
fn the_server_creates_a_workspace_as_a_job_and_a_cancelled_create_resumes() {
    use pom_remote::server_api::{progress_kind, CreateArgs, JobRequest, EXIT_CANCELLED};
    use pom_remote::ProjectSource;
    let fixture = Fixture::new();
    fixture.add_dev();
    let config = fixture.git_project();
    let source = fixture.server_source_for(&config);
    source.workspaces().expect("connect");
    assert!(source
        .transport()
        .is_some_and(|info| info.jobs && info.live_logs));

    let create = |branch: &str, from_stage: usize| {
        JobRequest::Create(CreateArgs {
            branch: branch.into(),
            display_name: "Login page".into(),
            from_stage,
            ..CreateArgs::default()
        })
    };
    let id = source
        .start_job(&create("feat-x", 0), "app-1-create-feat-x")
        .expect("start the create");
    let again = source
        .start_job(&create("feat-x", 0), "app-1-create-feat-x")
        .expect("ask again");
    assert_eq!(again, id, "the op id starts it once");
    let events = job_events(&source, &id, 0, |_| false);
    assert_eq!(events[0].kind, progress_kind::STAGES);
    assert_eq!(events[0].stages, pom_workspace::CREATE_STAGES);
    assert_eq!(
        started_stages(&events),
        (0..8).collect::<Vec<_>>(),
        "in order"
    );
    assert_eq!(events.last().and_then(|event| event.exit), Some(0));
    let folder = fixture.temp.path().join("jobs/workspace--feat-x");
    assert!(folder.join("api/seeded").exists());
    let listed = source.workspaces().expect("list");
    let created = listed
        .workspaces
        .iter()
        .find(|workspace| workspace.branch == "feat-x")
        .expect("listed");
    assert_eq!(created.name, "Login page");

    let replay = job_events(&source, &id, 3, |_| false);
    assert_eq!(
        replay.first().map(|event| event.seq),
        Some(3),
        "a re-follow replays from since_seq"
    );

    let id = source
        .start_job(&create("feat-y", 0), "app-1-create-feat-y")
        .expect("start another");
    let head = job_events(&source, &id, 0, |event| {
        event.kind == progress_kind::STAGE_STARTED && event.stage == Some(5)
    });
    source.cancel_job(&id).expect("cancel");
    let since = head.last().map_or(0, |event| event.seq + 1);
    let rest = job_events(&source, &id, since, |_| false);
    let failed = rest
        .iter()
        .find(|event| event.kind == progress_kind::FAILED)
        .expect("a stopped stage");
    assert_eq!(
        failed.stage,
        Some(6),
        "the running stage finishes, the next never starts"
    );
    assert_eq!(failed.line, "cancelled");
    assert_eq!(
        rest.last().and_then(|event| event.exit),
        Some(EXIT_CANCELLED)
    );
    let listed = source.workspaces().expect("list");
    let stopped = listed
        .workspaces
        .iter()
        .find(|workspace| workspace.branch == "feat-y")
        .expect("listed");
    assert_eq!(
        stopped
            .failed
            .as_ref()
            .map(|failed| (failed.operation.as_str(), failed.stage)),
        Some(("create", 7)),
        "it resumes like a failed local create"
    );
    assert!(!fixture
        .temp
        .path()
        .join("jobs/workspace--feat-y/api/seeded")
        .exists());

    let id = source
        .start_job(&create("feat-y", 6), "app-1-create-feat-y-resume")
        .expect("resume");
    let events = job_events(&source, &id, 0, |_| false);
    assert_eq!(events.last().and_then(|event| event.exit), Some(0));
    assert!(fixture
        .temp
        .path()
        .join("jobs/workspace--feat-y/api/seeded")
        .exists());
    assert_eq!(fixture.agent_runs(), 0, "no agent in a create");
}

#[test]
fn the_server_puts_a_workspace_to_sleep_and_wakes_it_with_its_agent_as_jobs() {
    use pom_remote::server_api::{BranchArgs, JobRequest};
    use pom_remote::{ProjectSource, ServiceAction};
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let source = fixture.server_source_with_agent_cli();
    source
        .act("feat", ServiceAction::Start, "web")
        .expect("start web");
    let agent = source
        .start_agent("feat", "claude")
        .expect("start the agent");
    let holders = pom_ptyhost::SocketDir::new(fixture.temp.path().join("s"));
    assert!(holders.holder_alive(&agent.holder));
    let deadline = Instant::now() + Duration::from_secs(30);
    while fixture.agent_runs() < 1 {
        assert!(Instant::now() < deadline, "the stand-in never ran");
        std::thread::sleep(Duration::from_millis(50));
    }

    let branch = BranchArgs {
        branch: "feat".into(),
        force: false,
    };
    let id = source
        .start_job(&JobRequest::Sleep(branch.clone()), "app-2-sleep")
        .expect("sleep");
    let events = job_events(&source, &id, 0, |_| false);
    assert_eq!(
        events.last().and_then(|event| event.exit),
        Some(0),
        "{events:?}"
    );
    let lines: Vec<&str> = events.iter().map(|event| event.line.as_str()).collect();
    assert!(lines.contains(&"stopped api/web"), "{lines:?}");
    assert!(lines.contains(&"stopped agent claude"), "{lines:?}");
    assert!(!source.status("feat").expect("status").services[0].up);
    assert!(!holders.holder_alive(&agent.holder));

    let id = source
        .start_job(&JobRequest::Wake(branch), "app-2-wake")
        .expect("wake");
    let events = job_events(&source, &id, 0, |_| false);
    assert_eq!(
        events.last().and_then(|event| event.exit),
        Some(0),
        "{events:?}"
    );
    let lines: Vec<&str> = events.iter().map(|event| event.line.as_str()).collect();
    assert!(lines.contains(&"started api/web"), "{lines:?}");
    assert!(lines.contains(&"started agent claude"), "{lines:?}");
    assert!(source.status("feat").expect("status").services[0].up);
    let deadline = Instant::now() + Duration::from_secs(30);
    while fixture.agent_runs() < 2 {
        assert!(Instant::now() < deadline, "the stand-in never ran again");
        std::thread::sleep(Duration::from_millis(50));
    }
    let woken = source
        .agents("feat")
        .expect("agents")
        .into_iter()
        .find(|view| view.role == "claude")
        .expect("the agent is back");
    source
        .pty("feat")
        .expect("terminals")
        .close(&woken.holder)
        .expect("stop the agent");
    source
        .act("feat", ServiceAction::Stop, "web")
        .expect("stop web");
}

#[test]
fn the_server_streams_a_service_output_and_an_older_server_is_asked_instead() {
    use pom_remote::{LogEvent, ProjectSource, ServiceAction};
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let source = fixture.server_source("dev");
    source
        .act("feat", ServiceAction::Start, "web")
        .expect("start web");
    let feed = source
        .follow_logs("feat", "api/web")
        .expect("follow")
        .expect("a server that streams");
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !seen.contains("web-banner-") {
        assert!(Instant::now() < deadline, "{seen:?}");
        match feed.next(Duration::from_millis(100)) {
            LogEvent::Data(bytes) => seen.push_str(&String::from_utf8_lossy(&bytes)),
            LogEvent::Idle | LogEvent::Skipped => {}
            LogEvent::StartedOver => seen.clear(),
            LogEvent::Ended(reason) => panic!("{reason}"),
        }
    }
    source
        .act("feat", ServiceAction::Restart, "web")
        .expect("restart web");
    while !seen.contains("--- api/web restarted ---") {
        assert!(
            Instant::now() < deadline + Duration::from_secs(15),
            "{seen:?}"
        );
        if let LogEvent::Data(bytes) = feed.next(Duration::from_millis(100)) {
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    let status = json(&fixture.pom(&["server", "status", "--json"]));
    assert_eq!(status["log_follows"], 1, "{status}");
    drop(feed);
    let log = std::fs::read_to_string(fixture.temp.path().join(format!(
        "state/pom/server/{}.log",
        env!("CARGO_PKG_VERSION")
    )))
    .expect("server log");
    assert!(log.contains("method=logs.follow"), "{log}");
    assert!(
        !log.contains("web-banner"),
        "the log keeps sizes, never output: {log}"
    );

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
    let older = std::sync::Arc::new(fixture.server_source("old"));
    assert!(older
        .follow_logs("feat", "api/web")
        .expect("asked")
        .is_none());
    let watch = pom_remote::Watch::start(older, "feat", std::sync::Arc::new(|| {}));
    watch.set_visible(true);
    watch.show_logs(Some("api/web".into()));
    let snapshot = wait_snapshot(
        &watch,
        "the output asked for",
        |snapshot| matches!(&snapshot.logs, Some((_, Ok(text))) if text.contains("web-banner-")),
    );
    assert!(!snapshot.logs_live, "an older pom is asked, not streamed");
    source
        .act("feat", ServiceAction::Stop, "web")
        .expect("stop web");
}

/// SIGKILLs the process carrying `source`'s session, as a dropped network or a killed ssh would end it.
fn kill_session(source: &pom_remote::ServerSource) {
    let pid = source.session_pid().expect("a session process");
    kill(pid);
}

fn kill(pid: u32) {
    let killed = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output()
        .expect("kill");
    assert!(killed.status.success(), "{}", text(&killed));
}

fn connected(source: &pom_remote::ServerSource) -> bool {
    use pom_remote::ProjectSource;
    source
        .link()
        .is_some_and(|report| report.state == pom_remote::LinkState::Connected)
        && source.session_pid().is_some()
}

/// A terminal's output placed by offset: offsets must follow on, except across a reset.
struct Screen {
    base: u64,
    bytes: Vec<u8>,
    received: usize,
    resets: usize,
    resumes: Vec<bool>,
    exited: bool,
}

impl Screen {
    fn new(start: u64) -> Screen {
        Screen {
            base: start,
            bytes: Vec::new(),
            received: 0,
            resets: 0,
            resumes: Vec::new(),
            exited: false,
        }
    }

    fn end(&self) -> u64 {
        self.base + self.bytes.len() as u64
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    fn step(&mut self, output: &mut pom_rpc::pty::ResumingOutput) {
        use pom_rpc::pty::TerminalEvent;
        match output.next(Duration::from_millis(20)) {
            TerminalEvent::Data { offset, bytes } => {
                assert_eq!(offset, self.end(), "output must follow on");
                self.received += bytes.len();
                self.bytes.extend_from_slice(&bytes);
                output.ack(self.end());
            }
            TerminalEvent::Reset { offset } => {
                self.resets += 1;
                self.base = offset;
                self.bytes.clear();
            }
            TerminalEvent::Resumed { restarted } => self.resumes.push(restarted),
            TerminalEvent::Exit { .. } => self.exited = true,
            TerminalEvent::Idle | TerminalEvent::Reconnecting(_) => {}
            TerminalEvent::Ended(reason) => panic!("ended: {reason}"),
        }
    }

    fn until(
        &mut self,
        output: &mut pom_rpc::pty::ResumingOutput,
        what: &str,
        done: impl Fn(&Screen) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "no {what}: {:?}",
                self.text().chars().rev().take(300).collect::<String>()
            );
            self.step(output);
        }
    }

    /// How many times `needle` shows in the output after `after`.
    fn count_after(&self, after: &str, needle: &str) -> usize {
        let text = self.text();
        let tail = text.split_once(after).map(|(_, tail)| tail).unwrap_or("");
        tail.matches(needle).count()
    }
}

#[test]
fn chaos_a_proxy_killed_mid_stream_loses_and_repeats_nothing() {
    use pom_remote::ProjectSource;
    use pom_rpc::pty::{Mode, Request, ResumingTerminal, Target};
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let source = fixture.server_source("dev");
    source.set_backoff(vec![Duration::from_millis(50); 6]);
    source.workspaces().expect("connect");
    let provider = source.pty("feat").expect("terminals");
    let request = Request {
        target: Target::Shell { cwd: None },
        cols: 120,
        rows: 40,
        mode: Mode::ReadWrite,
        primary: true,
        claim: None,
    };
    let (terminal, mut output, attached) =
        ResumingTerminal::attach(provider, &request).expect("a shell");
    let holder = attached.holder.clone();
    let holders = pom_ptyhost::SocketDir::new(fixture.temp.path().join("s"));
    let mut screen = Screen::new(attached.start);
    terminal
        .input(b"stty -echo -opost; seq 1 300000; echo SEQ-DONE; exec cat\n")
        .expect("type");

    let mut kills = 0;
    let mut kill_at = 100_000;
    let deadline = Instant::now() + Duration::from_secs(120);
    while !(kills == 5 && screen.resumes.len() == 5 && screen.text().contains("SEQ-DONE\n")) {
        assert!(
            Instant::now() < deadline,
            "kills {kills}, resumes {:?}",
            screen.resumes
        );
        if kills < 5
            && screen.resumes.len() == kills
            && screen.received >= kill_at
            && connected(&source)
        {
            kill_session(&source);
            kills += 1;
            kill_at = screen.received + 200_000;
        }
        screen.step(&mut output);
    }
    assert!(
        screen.resumes.iter().all(|restarted| !restarted),
        "the same server"
    );
    let (ring, end) =
        pom_ptyhost::read_since(&holders, &holder, 0, Duration::from_secs(5)).expect("ring");
    assert_eq!(screen.end(), end, "the client has all the holder printed");
    let ring_start = end - ring.len() as u64;
    assert!(screen.base <= ring_start, "the client holds the whole ring");
    let held = &screen.bytes[(ring_start - screen.base) as usize..];
    assert!(
        held == ring.as_slice(),
        "the client's output is the holder's"
    );
    let text = screen.text();
    // After a reset the output starts mid-line, so its first line may be a fragment.
    let skip = usize::from(screen.resets > 0);
    let numbers: Vec<u64> = text
        .lines()
        .skip(skip)
        .filter_map(|line| line.trim().parse().ok())
        .collect();
    let broken = numbers.windows(2).find(|pair| pair[1] != pair[0] + 1);
    assert!(broken.is_none(), "no line lost or repeated: {broken:?}");
    assert_eq!(numbers.last(), Some(&300_000));
    eprintln!(
        "chaos: {kills} kills, {} resets, {} bytes received, {} lines kept",
        screen.resets,
        screen.received,
        numbers.len()
    );

    // Typed the moment the session dies: the keys reach the holder once, whether or not they left before.
    terminal.input(b"abc\n").expect("type");
    kill_session(&source);
    screen.until(&mut output, "a resume", |screen| screen.resumes.len() == 6);
    screen.until(&mut output, "abc", |screen| {
        screen.count_after("SEQ-DONE", "abc") >= 1
    });
    terminal.input(b"after-kill\n").expect("type");
    screen.until(&mut output, "after-kill", |screen| {
        screen.count_after("SEQ-DONE", "after-kill") >= 1
    });
    assert_eq!(screen.count_after("SEQ-DONE", "abc"), 1, "typed once");

    // The server itself dies: a fresh one serves, and nothing unconfirmed is typed twice.
    terminal.input(b"pre\n").expect("type");
    screen.until(&mut output, "pre", |screen| {
        screen.count_after("after-kill", "pre") >= 1
    });
    let status = json(&fixture.pom(&["server", "status", "--json"]));
    let server_pid = status["pid"].as_u64().expect("server pid");
    kill(u32::try_from(server_pid).expect("pid"));
    screen.until(&mut output, "a resume through a new server", |screen| {
        screen.resumes.len() == 7
    });
    assert_eq!(screen.resumes.last(), Some(&true), "a restarted server");
    assert_eq!(source.link().map(|report| report.restarts), Some(1));
    terminal.input(b"post\n").expect("type");
    screen.until(&mut output, "post", |screen| {
        screen.count_after("after-kill", "post") >= 1
    });
    assert_eq!(screen.count_after("after-kill", "pre"), 1, "typed once");

    holders.kill_holder(&holder).expect("kill the holder");
    screen.until(&mut output, "the exit", |screen| screen.exited);
}

#[test]
fn a_job_is_followed_again_from_its_last_event_after_the_session_dies() {
    use pom_remote::server_api::{CreateArgs, JobRequest};
    use pom_remote::{JobEvent, ProjectSource};
    let fixture = Fixture::new();
    fixture.add_dev();
    let config = fixture.git_project();
    let source = fixture.server_source_for(&config);
    source.set_backoff(vec![Duration::from_millis(50); 6]);
    source.workspaces().expect("connect");
    let id = source
        .start_job(
            &JobRequest::Create(CreateArgs {
                branch: "feat-y".into(),
                ..CreateArgs::default()
            }),
            "app-9-create-feat-y",
        )
        .expect("start");
    let first = job_events(&source, &id, 0, |event| event.seq >= 2);
    let next = first.last().map(|event| event.seq + 1).expect("events");
    kill_session(&source);
    let feed = loop {
        match source.follow_job(&id, next) {
            Ok(feed) => break feed,
            Err(pom_remote::SourceError::Unreachable(_)) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(error) => panic!("{error}"),
        }
    };
    let mut rest = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "the job never ended: {rest:?}");
        match feed.next(Duration::from_millis(200)) {
            JobEvent::Progress(progress) => {
                let done = progress.done;
                rest.push(progress);
                if done {
                    break;
                }
            }
            JobEvent::Idle => {}
            JobEvent::Ended(reason) => panic!("{reason}"),
        }
    }
    assert_eq!(
        rest.first().map(|event| event.seq),
        Some(next),
        "from where it was"
    );
    assert_eq!(rest.last().and_then(|event| event.exit), Some(0));
}

#[test]
fn a_real_sshd_session_killed_mid_terminal_reconnects() {
    use pom_remote::ProjectSource;
    use pom_rpc::pty::{Mode, Request, ResumingTerminal, Target};
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
    let source = pom_remote::ServerSource::new(
        host,
        fixture.config(),
        pom_remote::Ssh::custom("ssh".into(), None, Vec::new()),
    );
    source.set_backoff(vec![Duration::from_millis(200); 6]);
    source.workspaces().expect("through sshd");
    let provider = source.pty("feat").expect("terminals");
    let request = Request {
        target: Target::Shell { cwd: None },
        cols: 80,
        rows: 24,
        mode: Mode::ReadWrite,
        primary: true,
        claim: None,
    };
    let (terminal, mut output, attached) =
        ResumingTerminal::attach(provider.clone(), &request).expect("a shell");
    let mut screen = Screen::new(attached.start);
    terminal
        .input(b"stty -echo; echo ready-$((40 + 2)); exec cat\n")
        .expect("type");
    screen.until(&mut output, "ready", |screen| {
        screen.text().contains("ready-42")
    });
    for round in 0..3 {
        let line = format!("typed-{round}");
        terminal
            .input(format!("{line}\n").as_bytes())
            .expect("type");
        kill_session(&source);
        screen.until(&mut output, "a resume", |screen| {
            screen.resumes.len() == round + 1
        });
        screen.until(&mut output, &line, |screen| screen.text().contains(&line));
        assert_eq!(screen.text().matches(&line).count(), 1, "{line} typed once");
    }
    assert!(screen.resumes.iter().all(|restarted| !restarted));
    if let Err(error) = provider.close(&attached.holder) {
        eprintln!("close: {error}");
    }
    let stopped = Command::new(POM).args(["server", "stop"]).output();
    if let Err(error) = stopped {
        eprintln!("pom server stop: {error}");
    }
}

impl Fixture {
    /// A release folder as GitHub serves it: a pom tarball whose install.sh records how it ran and puts in
    /// place a pom that is this build, and the checksums list, `tamper` making the tarball's entry wrong.
    fn fake_release(&self, version: &str, tamper: bool) -> pom_remote::update::Release {
        let release = self
            .temp
            .path()
            .join(if tamper { "release-bad" } else { "release" });
        let stage = self.temp.path().join(format!("stage/pom-{version}"));
        std::fs::create_dir_all(stage.join("bin")).expect("stage");
        executable(
            &stage.join("bin/pom"),
            &format!("#!/bin/sh\n# installed by the update\nexec '{POM}' \"$@\"\n"),
        );
        executable(
            &stage.join("install.sh"),
            "#!/bin/sh\nset -eu\necho \"$@\" > \"$HOME/install-args\"\nhere=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nrm -f \"$HOME/.local/bin/pom\"\ncp \"$here/bin/pom\" \"$HOME/.local/bin/pom\"\n",
        );
        std::fs::create_dir_all(&release).expect("release");
        let tarball = format!("pom-{version}-linux-x86_64.tar.gz");
        let packed = Command::new("tar")
            .arg("-C")
            .arg(self.temp.path().join("stage"))
            .arg("-czf")
            .arg(release.join(&tarball))
            .arg(format!("pom-{version}"))
            .output()
            .expect("tar");
        assert!(packed.status.success(), "{}", text(&packed));
        let bytes = std::fs::read(release.join(&tarball)).expect("tarball");
        let hash = if tamper {
            "0".repeat(64)
        } else {
            pom_remote::update::sha256_hex(&bytes)
        };
        std::fs::write(
            release.join(pom_remote::update::CHECKSUMS),
            format!(
                "{hash}  {tarball}\n{}  pom-{version}-linux-aarch64.tar.gz\n",
                "1".repeat(64)
            ),
        )
        .expect("checksums");
        pom_remote::update::Release {
            version: version.to_string(),
            base: format!("file://{}", release.display()),
        }
    }

    /// A server source whose ssh finds a `uname` that says Linux, as the hosts pom is published for do.
    fn linux_server_source(&self) -> pom_remote::ServerSource {
        let tools = self.temp.path().join("linux-tools");
        std::fs::create_dir_all(&tools).expect("tools");
        executable(&tools.join("uname"), "#!/bin/sh\necho Linux x86_64\n");
        let mut envs = self.envs();
        envs.push((
            "PATH".into(),
            format!(
                "{}:{}",
                tools.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        ));
        let ssh = pom_remote::Ssh::custom(self.temp.path().join("bin/ssh"), None, envs);
        let host = pom_remote::Hosts::load(&self.temp.path().join("config/hosts.json"))
            .and_then(|hosts| hosts.get("dev"))
            .expect("host");
        pom_remote::ServerSource::new(host, self.config(), ssh)
    }
}

#[test]
fn updating_pom_installs_the_checked_release_tarball_and_moves_to_its_server() {
    use pom_remote::update::UpdateStep;
    use pom_remote::ProjectSource;
    let fixture = Fixture::new();
    fixture.add_dev();
    fixture.register_project();
    let installed = fixture.temp.path().join("home/.local/bin/pom");
    std::fs::remove_file(&installed).expect("drop the symlink");
    executable(&installed, &format!("#!/bin/sh\nexec '{POM}' \"$@\"\n"));
    let source = fixture.linux_server_source();
    source.set_backoff(vec![Duration::from_millis(50); 6]);
    source.workspaces().expect("connect");
    let version = env!("CARGO_PKG_VERSION");

    let refused = source.update_pom(&fixture.fake_release(version, true), &|_| {});
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.contains("does not match the release's checksum")),
        "{refused:?}"
    );
    assert!(
        !std::fs::read_to_string(&installed)
            .unwrap_or_default()
            .contains("installed by the update"),
        "a tarball that fails its checksum is never installed"
    );
    assert!(!fixture.temp.path().join("home/install-args").exists());
    assert_eq!(source.link().map(|report| report.restarts), Some(0));

    let steps = std::sync::Mutex::new(Vec::new());
    source
        .update_pom(&fixture.fake_release(version, false), &|step| {
            if let Ok(mut steps) = steps.lock() {
                steps.push(step);
            }
        })
        .expect("update");
    assert_eq!(
        steps.lock().map(|steps| steps.clone()).unwrap_or_default(),
        vec![
            UpdateStep::Machine,
            UpdateStep::Download,
            UpdateStep::Verify,
            UpdateStep::Install,
            UpdateStep::Restart
        ]
    );
    assert!(std::fs::read_to_string(&installed)
        .unwrap_or_default()
        .contains("installed by the update"));
    assert_eq!(
        std::fs::read_to_string(fixture.temp.path().join("home/install-args"))
            .unwrap_or_default()
            .trim(),
        "--cli-only"
    );
    assert_eq!(
        source.link().map(|report| report.restarts),
        Some(1),
        "the old server stopped and the session moved to a new one"
    );
    assert!(source.workspaces().is_ok());
}

fn doctor_finding(report: &Value, id: &str) -> Option<Value> {
    report["findings"]
        .as_array()?
        .iter()
        .find(|finding| finding["id"] == id)
        .cloned()
}

#[test]
fn doctor_reports_how_the_server_stands() {
    let fixture = Fixture::new();
    // Only the fixture's tools: the doctor runs `pom --version` of whatever pom the PATH holds.
    let doctor = || -> Value {
        let output = fixture
            .command(&["doctor", "--json"])
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin",
                    fixture.temp.path().join("bin").display()
                ),
            )
            .output()
            .expect("pom doctor");
        serde_json::from_slice(&output.stdout).expect("doctor json")
    };
    let report = doctor();
    assert!(
        doctor_finding(&report, "server.stopped").is_none(),
        "no server section before a server ever ran: {report}"
    );

    let started = fixture.pom(&["server", "run"]);
    assert!(started.status.success(), "{}", text(&started));
    let report = doctor();
    let running = doctor_finding(&report, "server.running").expect("server.running");
    assert_eq!(running["severity"], "ok");
    let detail = running["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("0 connection(s)"), "{detail}");
    assert!(detail.contains("0 stream(s)"), "{detail}");
    assert!(detail.contains("0 kept for apps that dropped"), "{detail}");

    let dir = fixture.temp.path().join("run/pom-server");
    let old = std::fs::File::create(dir.join("0.0.1.lock")).expect("old lock");
    old.lock().expect("hold the old version's lock");
    std::fs::write(dir.join("0.0.1.pid"), "4242\n").expect("old pid");
    let report = doctor();
    let stale = doctor_finding(&report, "server.old_version:0.0.1").expect("old version");
    assert_eq!(stale["severity"], "warn");
    assert!(stale["fix"]
        .as_str()
        .unwrap_or_default()
        .contains("kill 4242"));
    drop(old);

    let stopped = fixture.pom(&["server", "stop"]);
    assert!(stopped.status.success(), "{}", text(&stopped));
    let report = doctor();
    assert!(
        doctor_finding(&report, "server.stopped").is_some(),
        "{report}"
    );
    assert!(doctor_finding(&report, "server.old_version:0.0.1").is_none());
}

const FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
echo "$*" >> "$FAKE_SYSTEMCTL_LOG"
"#;

#[test]
fn install_unit_writes_a_user_unit_on_linux_only() {
    let fixture = Fixture::new();
    executable(&fixture.temp.path().join("bin/systemctl"), FAKE_SYSTEMCTL);
    let config = fixture.temp.path().join("xdg");
    let calls = fixture.temp.path().join("systemctl.log");
    let run = |args: &[&str]| {
        fixture
            .command(args)
            .env("XDG_CONFIG_HOME", &config)
            .env("FAKE_SYSTEMCTL_LOG", &calls)
            .output()
            .expect("pom")
    };
    let unit = config.join("systemd/user/pom-server.service");
    let installed = run(&["server", "install-unit", "--json"]);
    if !cfg!(target_os = "linux") {
        assert_eq!(installed.status.code(), Some(1));
        assert!(
            text(&installed).contains("Linux only"),
            "{}",
            text(&installed)
        );
        assert!(!unit.exists());
        assert!(!calls.exists(), "systemctl is never run off Linux");
        return;
    }
    assert!(installed.status.success(), "{}", text(&installed));
    let report = json(&installed);
    assert_eq!(report["schema"], "pom.server/v1");
    assert_eq!(report["enabled"], true);
    assert_eq!(report["unit"], unit.to_string_lossy().as_ref());
    let written = std::fs::read_to_string(&unit).expect("unit");
    assert!(written.contains(" server run --foreground\n"), "{written}");
    assert!(written.contains("Restart=on-failure\n"), "{written}");
    let removed = run(&["server", "uninstall-unit"]);
    assert!(removed.status.success(), "{}", text(&removed));
    assert!(!unit.exists());
    assert_eq!(
        std::fs::read_to_string(&calls).expect("calls"),
        "--user daemon-reload\n--user enable pom-server.service\n--user disable pom-server.service\n--user daemon-reload\n"
    );
}
