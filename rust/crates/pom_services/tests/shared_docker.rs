//! Opt-in (`POM_DOCKER_TEST=1`): the shared stack against the real Docker daemon, torn down afterwards.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, SharedAction};

const SESSION: &str = "pomsharedtest";

struct Teardown(PathBuf);

impl Drop for Teardown {
    fn drop(&mut self) {
        let status = Command::new("docker")
            .env("PATH", pom_services::tool_path())
            .args(["compose", "-f"])
            .arg(&self.0)
            .args(["-p", &format!("{SESSION}-shared"), "down", "-v"])
            .status();
        if let Err(error) = status {
            eprintln!("teardown: {error}");
        }
    }
}

#[test]
fn shared_stack_comes_up_creates_databases_and_stops() {
    if std::env::var_os("POM_DOCKER_TEST").is_none() {
        return;
    }
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("project");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(
        root.join("pom.yml"),
        format!(
            "session: {SESSION}\nshared_services:\n  postgres:\n    image: postgres:16-alpine\n    ports: [\"25432:5432\"]\n    db_user: postgres\n    db_password: postgres\n  redis:\n    image: redis:7-alpine\n    ports: [\"26379:6379\"]\nrepos:\n  api:\n    databases:\n      main: \"api_{{{{branch.safe}}}}\"\n    services:\n      server: echo\n"
        ),
    )
    .expect("pom.yml");
    let config = Config::load(&root.join("pom.yml")).expect("config");
    let runner = ServiceRunner::new(RunnerOptions {
        project_root: root.clone(),
        session: SESSION.into(),
        state: StateDir::new(temp.path().join("state")),
        holders: SocketDir::new(temp.path().join("s")),
        binary: PathBuf::from("/nonexistent"),
        docker: "docker".into(),
    });
    let _teardown = Teardown(runner.compose_file());
    runner.ensure_shared(&config).expect("up");
    assert_eq!(
        runner.shared_running(),
        ["postgres", "redis"]
            .map(str::to_string)
            .into_iter()
            .collect()
    );

    // Postgres accepts connections a moment after its container starts.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match runner.ensure_databases(&config, "feat/x") {
            Ok(()) => break,
            Err(error) if Instant::now() < deadline => {
                eprintln!("waiting for postgres: {error}");
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(error) => panic!("databases: {error}"),
        }
    }
    let port = runner.shared_host_port("postgres");
    let listed = Command::new("docker")
        .env("PATH", pom_services::tool_path())
        .args(["ps", "-q", "--filter", &format!("publish={port}")])
        .output()
        .expect("ps");
    let container = String::from_utf8_lossy(&listed.stdout).trim().to_string();
    let databases = Command::new("docker")
        .env("PATH", pom_services::tool_path())
        .args([
            "exec",
            &container,
            "psql",
            "-U",
            "postgres",
            "-tAc",
            "SELECT datname FROM pg_database",
        ])
        .output()
        .expect("psql");
    let names = String::from_utf8_lossy(&databases.stdout);
    assert!(
        names.lines().any(|name| name == "pomsharedtest_api_feat_x"),
        "{names}"
    );
    runner
        .ensure_databases(&config, "feat/x")
        .expect("creating again is a no-op");

    runner
        .shared_action(&config, "redis", SharedAction::Stop)
        .expect("stop redis");
    assert!(!runner.shared_running().contains("redis"));
}
