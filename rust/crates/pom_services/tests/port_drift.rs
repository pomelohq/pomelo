//! Real services in real holders, seen from more than one process: the app, an agent's MCP server and the
//! CLI each open their own runner over the same state, and must all agree on where a service listens.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};

const TIMEOUT: Duration = Duration::from_secs(15);
const LEASE_KEY: &str = "ws-feat_x\u{1f}api~web";

static SERIAL: Mutex<()> = Mutex::new(());

/// Stands in for the app binary's `pty run`: the wrapper script below re-enters this test binary here.
#[test]
fn holder_entry() {
    let Some(count) = std::env::var("POM_TEST_PTY_ARGC")
        .ok()
        .and_then(|count| count.parse::<usize>().ok())
    else {
        return;
    };
    let mut args = vec!["pomelo".to_string()];
    args.extend(
        (0..count).filter_map(|index| std::env::var(format!("POM_TEST_PTY_ARG_{index}")).ok()),
    );
    std::process::exit(pom_ptyhost::cli::run(&args).unwrap_or(2));
}

struct Fixture {
    temp: tempfile::TempDir,
    config: Config,
    target: ServiceTarget,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Fixture {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let temp = tempfile::Builder::new()
            .prefix("drf")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join("workspace--feat/x").join("api")).expect("worktree");
        std::fs::write(
            root.join("pom.yml"),
            r#"session: demo
repos:
  api:
    services:
      web:
        type: backend
        cmd: "exec nc -lk 127.0.0.1 $PORT"
"#,
        )
        .expect("pom.yml");
        let config = Config::load(&root.join("pom.yml")).expect("config");
        wrapper_script(temp.path());
        Fixture {
            temp,
            config,
            target: ServiceTarget {
                branch: "feat/x".into(),
                is_main: false,
                repo: "api".into(),
                service: "web".into(),
            },
            _serial: serial,
        }
    }

    /// One process's view: the app, an agent's `pom mcp`, or a CLI run.
    fn process(&self) -> ServiceRunner {
        ServiceRunner::new(RunnerOptions {
            project_root: self.temp.path().join("project"),
            session: "demo".into(),
            state: StateDir::new(self.temp.path().join("state")),
            holders: SocketDir::new(self.temp.path().join("s")),
            binary: self.temp.path().join("pty-host"),
            docker: "docker".into(),
        })
    }

    fn lease_files(&self) -> Vec<u16> {
        pom_ports::scan_leases(&self.temp.path().join("state/ports.d"))
            .into_iter()
            .filter(|lease| lease.key == LEASE_KEY)
            .map(|lease| lease.port)
            .collect()
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

fn wrapper_script(dir: &Path) -> PathBuf {
    let zdotdir = dir.join("zdot");
    std::fs::create_dir_all(&zdotdir).expect("zdotdir");
    let test_binary = std::env::current_exe().expect("test binary");
    let script = dir.join("pty-host");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ni=0\nfor arg; do export \"POM_TEST_PTY_ARG_$i=$arg\"; i=$((i+1)); done\n\
             export POM_TEST_PTY_ARGC=$i ZDOTDIR='{}'\n\
             exec '{}' holder_entry --exact --nocapture\n",
            zdotdir.display(),
            test_binary.display()
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

fn listens(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_agent_opened_before_the_start_sees_the_apps_port() {
    let fixture = Fixture::new();
    let app = fixture.process();
    let agent = fixture.process();
    app.start(&fixture.config, &fixture.target).expect("start");
    let port = app.port(&fixture.config, &fixture.target).expect("port");
    wait_for("the service to listen", || listens(port));

    assert_eq!(
        agent.port(&fixture.config, &fixture.target),
        Some(port),
        "the agent reports where the service really is"
    );
    assert_eq!(
        agent.ports().acquire(LEASE_KEY),
        Some(port),
        "writing env from the agent must not mint a second port"
    );
    assert_eq!(
        fixture.lease_files(),
        vec![port],
        "one lease for the service"
    );
}

#[test]
fn the_cli_and_the_app_agree_after_either_restarts_the_service() {
    let fixture = Fixture::new();
    let app = fixture.process();
    app.start(&fixture.config, &fixture.target).expect("start");
    let first = app.port(&fixture.config, &fixture.target).expect("port");
    wait_for("the service to listen", || listens(first));

    let cli = fixture.process();
    cli.restart(&fixture.config, &fixture.target)
        .expect("restart");
    let port = cli.port(&fixture.config, &fixture.target).expect("port");
    wait_for("the restarted service to listen", || listens(port));
    assert_eq!(app.port(&fixture.config, &fixture.target), Some(port));
    assert_eq!(fixture.lease_files(), vec![port]);
}

#[test]
fn resolving_a_port_conflict_never_strands_a_running_service() {
    let fixture = Fixture::new();
    let app = fixture.process();
    app.start(&fixture.config, &fixture.target).expect("start");
    let before = app.port(&fixture.config, &fixture.target).expect("port");
    wait_for("the service to listen", || listens(before));

    let agent = fixture.process();
    agent
        .relocate_workspace(&fixture.config, &fixture.target.branch, false)
        .expect("relocate");
    let after = agent.port(&fixture.config, &fixture.target).expect("port");
    wait_for("the service to listen where its lease says", || {
        listens(after)
    });
    assert_eq!(
        app.port(&fixture.config, &fixture.target),
        Some(after),
        "the app follows the move"
    );
}
