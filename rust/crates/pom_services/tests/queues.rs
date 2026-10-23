//! Opt-in (`POM_DOCKER_TEST=1`, also the Linux CI job): a worker's queue in a real Redis, counted and waited on.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use pom_config::Config;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};

const SESSION: &str = "pomqueuetest";
const PORT: u16 = 26381;

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

fn redis(args: &[&str]) {
    let status = Command::new("docker")
        .env("PATH", pom_services::tool_path())
        .args([
            "run",
            "--rm",
            "--add-host=host.docker.internal:host-gateway",
            "redis:7-alpine",
            "redis-cli",
            "-h",
            "host.docker.internal",
            "-p",
            &PORT.to_string(),
        ])
        .args(args)
        .status()
        .expect("redis-cli");
    assert!(status.success(), "redis-cli {args:?}");
}

fn target(service: &str) -> ServiceTarget {
    ServiceTarget {
        branch: "feat-login".into(),
        is_main: false,
        repo: "api".into(),
        service: service.into(),
    }
}

#[test]
fn a_real_queue_is_counted_and_waited_on_until_it_drains() {
    if std::env::var_os("POM_DOCKER_TEST").is_none() {
        return;
    }
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("project");
    std::fs::create_dir_all(root.join("workspace--feat-login/api")).expect("worktree");
    std::fs::write(
        root.join("pom.yml"),
        format!(
            "session: {SESSION}\nshared_services:\n  redis:\n    image: redis:7-alpine\n    ports: [\"{PORT}:6379\"]\nrepos:\n  api:\n    env:\n      REDIS_URL: \"redis://{{{{shared.redis.host}}}}:{{{{shared.redis.port}}}}/0\"\n    services:\n      worker:\n        cmd: run\n        queue: {{ kind: sidekiq }}\n      mailer:\n        cmd: run\n        queue: {{ kind: bullmq }}\n"
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

    redis(&["SADD", "queues", "default"]);
    redis(&["LPUSH", "queue:default", "a", "b", "c"]);
    let counts = runner
        .queue_snapshot(&config, &target("worker"), None)
        .expect("counts")
        .counts;
    assert_eq!(counts.waiting["default"], 3, "{counts:?}");

    let still_busy = runner
        .wait_queue_idle(&config, &target("worker"), None, Duration::from_secs(1))
        .expect("wait");
    assert!(still_busy.is_err(), "three jobs are still waiting");

    let drain = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(1));
        redis(&["DEL", "queue:default"]);
    });
    let idle = runner
        .wait_queue_idle(&config, &target("worker"), None, Duration::from_secs(20))
        .expect("wait");
    assert!(idle.is_ok(), "{idle:?}");
    assert!(drain.join().is_ok());

    redis(&["SET", "bull:legacy:id", "1"]);
    redis(&["LPUSH", "bull:legacy:wait", "1"]);
    redis(&["ZADD", "bull:emails:repeat", "99999999999999", "daily"]);
    redis(&[
        "ZADD",
        "bull:emails:delayed",
        "409599999999995904",
        "repeat:daily:99999999999999",
    ]);
    let snapshot = runner
        .queue_snapshot(&config, &target("mailer"), None)
        .expect("counts");
    assert_eq!(snapshot.redis.name, "redis");
    assert_eq!(snapshot.redis.reason, "env REDIS_URL");
    let counts = snapshot.counts;
    assert_eq!(
        counts.waiting["legacy"], 1,
        "a Bull v4 queue has no meta key: {counts:?}"
    );
    assert_eq!(
        (counts.repeat_scheduled, counts.total()),
        (1, 1),
        "{counts:?}"
    );
}
