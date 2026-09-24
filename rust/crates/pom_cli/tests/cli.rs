//! The real `pom` binary against a throwaway project, state dir and holder dir.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const POM: &str = env!("CARGO_BIN_EXE_pom");

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomc")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for workspace in ["workspace--main/api", "workspace--feat/api"] {
            std::fs::create_dir_all(root.join(workspace)).expect("worktree");
        }
        std::fs::create_dir_all(temp.path().join("zdot")).expect("zdot");
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
        Fixture { temp }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn pom(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(cwd)
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("ZDOTDIR", self.temp.path().join("zdot"))
            .output()
            .expect("pom")
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

#[test]
fn start_status_logs_url_and_stop_in_the_workspace_of_the_current_directory() {
    let fixture = Fixture::new();
    let feat = fixture.root().join("workspace--feat/api");

    let started = fixture.pom(&feat, &["start", "web"]);
    assert!(started.status.success(), "{}", text(&started));
    let said = text(&started);
    let port: u16 = said
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("a port in {said:?}"));
    assert_eq!(said.trim(), format!("started api/web on port {port}"));

    let status = text(&fixture.pom(&feat, &["status"]));
    assert!(status.starts_with("demo  workspace feat\n"), "{status}");
    assert!(
        status.contains(&format!("web  running   :{port}")),
        "{status}"
    );

    let main_status = text(&fixture.pom(&fixture.root(), &["status"]));
    assert!(main_status.contains("workspace main"), "{main_status}");
    assert!(main_status.contains("web  stopped"), "{main_status}");

    let url = text(&fixture.pom(&feat, &["url", "api/web"]));
    let mut lines = url.lines();
    assert_eq!(
        lines.next(),
        Some(format!("direct  http://127.0.0.1:{port}").as_str())
    );
    assert_eq!(
        lines.next(),
        Some("proxy   http://web.api.feat.localhost:3  (proxy not running: start the app or `pom proxy`)")
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let logs = text(&fixture.pom(&feat, &["logs", "web"]));
        if logs.contains(&format!("web-banner-{port}")) {
            break;
        }
        assert!(Instant::now() < deadline, "no banner in {logs:?}");
        std::thread::sleep(Duration::from_millis(100));
    }

    let ports = text(&fixture.pom(&feat, &["ports"]));
    assert!(
        ports.contains(&format!("{port}")) && ports.contains("ws-feat api~web"),
        "{ports}"
    );

    let again = text(&fixture.pom(&fixture.root(), &["-w", "feat", "start", "web"]));
    assert_eq!(again.trim(), "api/web is already running");

    let stopped = text(&fixture.pom(&feat, &["stop"]));
    assert_eq!(stopped.trim(), "stopped api/web");
    let status = text(&fixture.pom(&feat, &["status"]));
    assert!(status.contains("web  stopped"), "{status}");
}

#[test]
fn mistakes_are_reported_plainly() {
    let fixture = Fixture::new();
    let unknown = fixture.pom(&fixture.root(), &["-w", "nope", "status"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(
        text(&unknown).contains("no workspace nope (have: "),
        "{}",
        text(&unknown)
    );

    let missing = fixture.pom(&fixture.root(), &["logs", "ghost"]);
    assert!(
        text(&missing).contains("service 'ghost' not found"),
        "{}",
        text(&missing)
    );

    let not_running = fixture.pom(&fixture.root(), &["attach", "web"]);
    assert!(text(&not_running).contains("web is not running"));

    let usage = fixture.pom(&fixture.root(), &["frobnicate"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(text(&usage).contains("usage: pom"));

    let nowhere = fixture.pom(fixture.temp.path(), &["status"]);
    assert!(text(&nowhere).contains("no pom.yml here or in any parent"));
}
