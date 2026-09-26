//! A real service in a real holder: env files written, env injected, port leased and listened on,
//! console output readable, and stop takes it down.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};

const TIMEOUT: Duration = Duration::from_secs(15);

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
    runner: ServiceRunner,
    config: Config,
    target: ServiceTarget,
}

impl Fixture {
    fn new() -> Fixture {
        // Short path: Unix socket paths are limited to 104 bytes.
        let temp = tempfile::Builder::new()
            .prefix("svc")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        let worktree = root.join("workspace--feat/x").join("api");
        std::fs::create_dir_all(&worktree).expect("worktree");
        std::fs::write(
            root.join("pom.yml"),
            r#"session: demo
repos:
  api:
    env:
      GREETING: "hi-{{branch.safe}}"
    services:
      web:
        type: backend
        env:
          SELF_PORT: "{{api.web.port}}"
        cmd: "echo started-$PORT-$GREETING-$SELF_PORT; exec nc -lk 127.0.0.1 $PORT"
"#,
        )
        .expect("pom.yml");
        let config = Config::load(&root.join("pom.yml")).expect("config");
        let binary = wrapper_script(temp.path());
        let runner = ServiceRunner::new(RunnerOptions {
            project_root: root,
            session: "demo".into(),
            state: StateDir::new(temp.path().join("state")),
            holders: SocketDir::new(temp.path().join("s")),
            binary,
            docker: "docker".into(),
        });
        Fixture {
            temp,
            runner,
            config,
            target: ServiceTarget {
                branch: "feat/x".into(),
                is_main: false,
                repo: "api".into(),
                service: "web".into(),
            },
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let names: Vec<String> = self
            .runner
            .holders()
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        self.runner.holders().kill_holders_now(&names);
    }
}

/// Passes `pty run ...` through to `holder_entry`, with an empty zsh config dir so the user's
/// profile never runs inside the test.
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

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn start_runs_the_service_on_its_leased_port_and_stop_ends_it() {
    let fixture = Fixture::new();
    let holder = fixture
        .runner
        .start(&fixture.config, &fixture.target)
        .expect("start");
    assert_eq!(holder, "svc-demo-feat_x-api-web");
    let port = fixture
        .runner
        .port(&fixture.config, &fixture.target)
        .expect("leased port");
    assert!(fixture.runner.is_running(&fixture.target));

    let banner = format!("started-{port}-hi-feat_x-{port}");
    wait_for("the service banner", || {
        pom_ptyhost::snapshot(fixture.runner.holders(), &holder, TIMEOUT)
            .is_ok_and(|output| String::from_utf8_lossy(&output).contains(&banner))
    });
    wait_for("the service to listen", || {
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    });

    let env_file = fixture
        .temp
        .path()
        .join("project/workspace--feat/x/api/.env.local");
    let written = std::fs::read_to_string(&env_file).expect("env file");
    assert_eq!(
        written,
        format!("# Auto-generated by pom. Do not commit.\nGREETING=hi-feat_x\nSELF_PORT={port}\n")
    );

    let again = fixture
        .runner
        .start(&fixture.config, &fixture.target)
        .expect("start again");
    assert_eq!(again, holder, "starting a running service is a no-op");

    fixture.runner.stop(&fixture.target).expect("stop");
    assert!(!fixture.runner.is_running(&fixture.target));
    wait_for("the port to free up", || {
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
    });
    assert_eq!(
        fixture.runner.port(&fixture.config, &fixture.target),
        Some(port),
        "the lease survives a stop so a restart keeps the port"
    );
}

#[test]
fn a_busy_port_moves_the_service_to_a_new_one() {
    let fixture = Fixture::new();
    let first = fixture
        .runner
        .ports()
        .acquire("ws-feat_x\u{1f}api~web")
        .expect("lease");
    let squatter = std::net::TcpListener::bind(("127.0.0.1", first)).expect("squat");
    fixture
        .runner
        .start(&fixture.config, &fixture.target)
        .expect("start");
    let moved = fixture
        .runner
        .port(&fixture.config, &fixture.target)
        .expect("port");
    assert_ne!(moved, first);
    drop(squatter);
}

#[test]
fn unknown_services_are_refused() {
    let fixture = Fixture::new();
    let mut target = fixture.target.clone();
    target.service = "nope".into();
    let error = fixture
        .runner
        .start(&fixture.config, &target)
        .expect_err("unknown");
    assert!(error.to_string().contains("nope"), "{error}");
    assert!(fixture.runner.holders().holders().is_empty());
}

#[test]
fn a_running_service_is_stale_once_the_config_changes_its_env() {
    let fixture = Fixture::new();
    fixture
        .runner
        .start(&fixture.config, &fixture.target)
        .expect("start");
    let targets = ServiceRunner::service_targets(&fixture.config, "feat/x", false);
    assert_eq!(targets, vec![fixture.target.clone()]);
    assert!(fixture
        .runner
        .stale_services(&fixture.config, &targets)
        .is_empty());

    let root = fixture.temp.path().join("project");
    let edited = std::fs::read_to_string(root.join("pom.yml"))
        .expect("read")
        .replace("hi-{{branch.safe}}", "hello-{{branch.safe}}");
    std::fs::write(root.join("pom.yml"), edited).expect("write");
    let changed = Config::load(&root.join("pom.yml")).expect("config");
    fixture
        .runner
        .refresh_workspace_env(&changed, "feat/x")
        .expect("refresh env");
    assert_eq!(
        fixture.runner.stale_services(&changed, &targets),
        vec![fixture.target.clone()]
    );
}
