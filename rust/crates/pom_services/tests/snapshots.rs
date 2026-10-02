//! Workspace database snapshots: the exact statements against a recording `docker`, and (opt-in,
//! `POM_DOCKER_TEST=1`) a real Postgres where a restore brings every database of one workspace back and leaves
//! another workspace alone.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use pom_config::Config;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, SnapshotIndex, MAIN_BASELINE};

const CONFIG: &str = "session: myproject\ndefault_branch: main\nshared_services:\n  postgres:\n    image: postgres:16-alpine\n    ports: [\"PORT:5432\"]\n    db_user: postgres\n    db_password: postgres\nrepos:\n  api:\n    databases:\n      main: \"api_{{branch.safe}}\"\n      events: \"events_{{branch.safe}}\"\n    services:\n      server: echo\n";

struct Project {
    _temp: tempfile::TempDir,
    root: PathBuf,
    config: Config,
    runner: ServiceRunner,
}

fn project(session: &str, port: u16, docker: PathBuf) -> Project {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("project");
    for branch in ["main", "feat-login", "feat-search"] {
        std::fs::create_dir_all(root.join(format!("workspace--{branch}/api"))).expect("worktree");
    }
    std::fs::write(
        root.join("pom.yml"),
        CONFIG
            .replace("myproject", session)
            .replace("PORT", &port.to_string()),
    )
    .expect("pom.yml");
    let config = Config::load(&root.join("pom.yml")).expect("config");
    let runner = ServiceRunner::new(RunnerOptions {
        project_root: root.clone(),
        session: session.into(),
        state: StateDir::new(temp.path().join("state")),
        holders: SocketDir::new(temp.path().join("s")),
        binary: PathBuf::from("/nonexistent"),
        docker,
    });
    Project {
        _temp: temp,
        root,
        config,
        runner,
    }
}

impl Project {
    fn folder(&self, branch: &str) -> PathBuf {
        self.root.join(format!("workspace--{branch}"))
    }

    fn databases(&self, branch: &str) -> Vec<String> {
        pom_services::owned_database_names(&self.config, branch, |_| true)
    }
}

/// A `docker` that records its arguments, one call per line, and answers the few queries snapshots make.
fn recording_docker(dir: &Path) -> (PathBuf, PathBuf) {
    let log = dir.join("docker.log");
    let script = dir.join("docker");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$*\" in\n  *'ps -q'*) echo pgcontainer ;;\n  *server_version_num*) echo 160002 ;;\n  *pg_database_size*) echo 4096 ;;\n  *'df -Pk'*) printf 'Filesystem 1024-blocks Used Available Capacity Mounted\\n/dev/vda 100 50 999999 50%% /data\\n' ;;\nesac\nexit 0\n",
            log = log.display()
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    (script, log)
}

fn sql_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(" -c ").map(|(_, sql)| sql.to_string()))
        .collect()
}

#[test]
fn a_restore_drops_and_recreates_only_the_workspaces_databases_from_its_snapshot() {
    let tools = tempfile::tempdir().expect("tools");
    let (docker, log) = recording_docker(tools.path());
    let project = project("myproject", 25433, docker);
    let branch = "feat-login";
    let dbs = project.databases(branch);
    assert_eq!(
        dbs,
        ["myproject_api_feat-login", "myproject_events_feat-login"]
    );

    let report = project
        .runner
        .snapshot_workspace(
            &project.config,
            branch,
            &project.folder(branch),
            &dbs,
            "before-checkout",
            false,
        )
        .expect("snapshot");
    assert!(report.ok(), "{report:?}");
    assert_eq!(report.databases.len(), 2);
    assert_eq!(report.databases[0].bytes, 4096);
    let index = SnapshotIndex::load(&project.folder(branch));
    assert_eq!(
        index.snapshots["before-checkout"].databases[1].snapshot_db,
        "myproject_events_feat-login__snap__before-checkout"
    );
    let again = project.runner.snapshot_workspace(
        &project.config,
        branch,
        &project.folder(branch),
        &dbs,
        "before-checkout",
        false,
    );
    assert!(again.is_err(), "an existing snapshot needs --replace");

    std::fs::write(&log, "").expect("clear log");
    let report = project
        .runner
        .restore_workspace(
            &project.config,
            branch,
            false,
            &project.folder(branch),
            &dbs,
            "before-checkout",
            true,
        )
        .expect("restore");
    assert!(report.ok(), "{report:?}");
    let statements: Vec<String> = sql_lines(&log)
        .into_iter()
        .filter(|sql| !sql.starts_with("SELECT"))
        .collect();
    assert_eq!(
        statements,
        [
            "DROP DATABASE IF EXISTS \"myproject_api_feat-login\" WITH (FORCE)",
            "CREATE DATABASE \"myproject_api_feat-login\" TEMPLATE \"myproject_api_feat-login__snap__before-checkout\" STRATEGY FILE_COPY",
            "DROP DATABASE IF EXISTS \"myproject_events_feat-login\" WITH (FORCE)",
            "CREATE DATABASE \"myproject_events_feat-login\" TEMPLATE \"myproject_events_feat-login__snap__before-checkout\" STRATEGY FILE_COPY",
        ]
    );
    let everything = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !everything.contains("feat-search"),
        "another workspace is never touched"
    );
    assert!(!everything.contains("_main"), "main is never touched");

    let missing = project.runner.restore_workspace(
        &project.config,
        branch,
        false,
        &project.folder(branch),
        &dbs,
        "never-taken",
        true,
    );
    assert!(missing.is_err());

    let dropped = project
        .runner
        .drop_workspace_snapshot(
            &project.config,
            branch,
            &project.folder(branch),
            "before-checkout",
        )
        .expect("drop");
    assert!(dropped.ok());
    assert!(SnapshotIndex::load(&project.folder(branch))
        .snapshots
        .is_empty());
}

fn psql(port: u16, database: &str, sql: &str) -> String {
    let output = Command::new("docker")
        .env("PATH", pom_services::tool_path())
        .args([
            "run",
            "--rm",
            "--add-host=host.docker.internal:host-gateway",
            "postgres:16-alpine",
            "psql",
            &format!("postgresql://postgres:postgres@host.docker.internal:{port}/{database}"),
            "-tAc",
            sql,
        ])
        .output()
        .expect("psql");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

struct Teardown(PathBuf, String);

impl Drop for Teardown {
    fn drop(&mut self) {
        let status = Command::new("docker")
            .env("PATH", pom_services::tool_path())
            .args(["compose", "-f"])
            .arg(&self.0)
            .args(["-p", &format!("{}-shared", self.1), "down", "-v"])
            .status();
        if let Err(error) = status {
            eprintln!("teardown: {error}");
        }
    }
}

#[test]
fn a_real_restore_brings_back_every_database_of_one_workspace_only() {
    if std::env::var_os("POM_DOCKER_TEST").is_none() {
        return;
    }
    let session = "pomsnaptest";
    let port = 25434;
    let project = project(session, port, "docker".into());
    let _teardown = Teardown(project.runner.compose_file(), session.to_string());
    project.runner.ensure_shared(&project.config).expect("up");
    for branch in ["feat-login", "feat-search"] {
        let dbs = project.databases(branch);
        project
            .runner
            .create_databases_when_ready(&project.config, &dbs)
            .expect("create");
        for db in &dbs {
            psql(
                port,
                db,
                "CREATE TABLE rows (value text); INSERT INTO rows VALUES ('seed')",
            );
        }
    }
    let login = project.databases("feat-login");
    let search = project.databases("feat-search");
    let report = project
        .runner
        .snapshot_workspace(
            &project.config,
            "feat-login",
            &project.folder("feat-login"),
            &login,
            "seeded",
            false,
        )
        .expect("snapshot");
    assert!(report.ok(), "{report:?}");
    for db in login.iter().chain(&search) {
        psql(port, db, "INSERT INTO rows VALUES ('changed')");
    }
    let report = project
        .runner
        .restore_workspace(
            &project.config,
            "feat-login",
            false,
            &project.folder("feat-login"),
            &login,
            "seeded",
            true,
        )
        .expect("restore");
    assert!(report.ok(), "{report:?}");
    for db in &login {
        assert_eq!(
            psql(port, db, "SELECT string_agg(value, ',') FROM rows"),
            "seed",
            "{db}"
        );
    }
    for db in &search {
        assert_eq!(
            psql(
                port,
                db,
                "SELECT string_agg(value, ',' ORDER BY value) FROM rows"
            ),
            "changed,seed",
            "{db} keeps its own change"
        );
    }
    project
        .runner
        .drop_workspace_snapshot(
            &project.config,
            "feat-login",
            &project.folder("feat-login"),
            "seeded",
        )
        .expect("drop");
    assert!(project
        .runner
        .snapshot_databases_present(&project.config)
        .expect("list")
        .is_empty());
}

/// Kills the long-lived psql client container even when an assertion fails.
struct Client(std::process::Child, String);

impl Drop for Client {
    fn drop(&mut self) {
        if let Err(error) = self.0.kill() {
            eprintln!("client: {error}");
        }
        let status = Command::new("docker")
            .env("PATH", pom_services::tool_path())
            .args(["rm", "-f", &self.1])
            .status();
        if let Err(error) = status {
            eprintln!("client: {error}");
        }
    }
}

#[test]
fn a_real_copy_from_main_baseline_keeps_mains_connections_open() {
    if std::env::var_os("POM_DOCKER_TEST").is_none() {
        return;
    }
    let session = "pombasetest";
    let port = 25435;
    let project = project(session, port, "docker".into());
    let _teardown = Teardown(project.runner.compose_file(), session.to_string());
    project.runner.ensure_shared(&project.config).expect("up");
    let main_dbs = project.databases("main");
    project
        .runner
        .create_databases_when_ready(&project.config, &main_dbs)
        .expect("create main");
    for db in &main_dbs {
        psql(
            port,
            db,
            "CREATE TABLE rows (value text); INSERT INTO rows VALUES ('seed')",
        );
    }
    let report = project
        .runner
        .snapshot_workspace(
            &project.config,
            "main",
            &project.folder("main"),
            &main_dbs,
            MAIN_BASELINE,
            true,
        )
        .expect("baseline");
    assert!(report.ok(), "{report:?}");

    let watched = &main_dbs[0];
    let name = format!("{session}-holder");
    let child = Command::new("docker")
        .env("PATH", pom_services::tool_path())
        .args([
            "run",
            "--rm",
            "--name",
            &name,
            "--add-host=host.docker.internal:host-gateway",
            "postgres:16-alpine",
            "psql",
            &format!("postgresql://postgres:postgres@host.docker.internal:{port}/{watched}"),
            "-c",
            "SELECT pg_sleep(60)",
        ])
        .spawn()
        .expect("client");
    let mut client = Client(child, name);
    let connected = format!(
        "SELECT count(*) FROM pg_stat_activity WHERE datname = '{watched}' AND query LIKE '%pg_sleep%'"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while psql(port, "postgres", &connected) != "1" {
        assert!(
            std::time::Instant::now() < deadline,
            "the client never connected"
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    let pairs = pom_services::main_counterparts(&project.config, "feat-login", |_| true);
    assert_eq!(pairs.len(), 2);
    for (workspace, main) in &pairs {
        let snapshot = pom_services::snapshot_copy(&project.folder("main"), MAIN_BASELINE, main)
            .expect("baseline copy");
        project
            .runner
            .clone_from_snapshot(&project.config, &snapshot, workspace)
            .expect("clone");
        assert_eq!(
            psql(port, workspace, "SELECT string_agg(value, ',') FROM rows"),
            "seed"
        );
    }
    assert_eq!(
        psql(port, "postgres", &connected),
        "1",
        "main's client is still connected"
    );
    assert!(
        client.0.try_wait().expect("client state").is_none(),
        "main's client is still running"
    );
}
