use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::*;

const CONFIG: &str = "session: demo
shared_services:
  postgres:
    image: postgres:16
repos:
  api:
    alias: api
    setup: [SETUP]
    migrate: [bin/migrate]
    databases:
      main: main
    env:
      DATABASE_URL: postgresql://{{shared.postgres.url}}/{{db.main}}
      PLAIN: value
    services:
      web:
        cmd: bin/web -p $PORT
      jobs:
        cmd: bin/jobs
        port: false
";

struct Fake {
    states: HashMap<String, BootState>,
    started: RefCell<Vec<String>>,
    stopped: RefCell<Vec<String>>,
}

impl Fake {
    fn new(states: &[(&str, BootState)]) -> Fake {
        Fake {
            states: states
                .iter()
                .map(|(name, state)| (name.to_string(), state.clone()))
                .collect(),
            started: RefCell::new(Vec::new()),
            stopped: RefCell::new(Vec::new()),
        }
    }
}

impl Services for Fake {
    fn prepare(&self) -> Result<(), String> {
        Ok(())
    }

    fn repo_env(&self, _repo: &str) -> Vec<(String, String)> {
        Vec::new()
    }

    fn start(&self, target: &ServiceTarget) -> Result<(), String> {
        self.started.borrow_mut().push(target.service.clone());
        Ok(())
    }

    fn state(&self, target: &ServiceTarget) -> BootState {
        self.states
            .get(&target.service)
            .cloned()
            .unwrap_or(BootState::Starting)
    }

    fn has_port(&self, target: &ServiceTarget) -> bool {
        target.service != "jobs"
    }

    fn stop(&self, target: &ServiceTarget) {
        self.stopped.borrow_mut().push(target.service.clone());
    }
}

struct Project {
    _temp: tempfile::TempDir,
    root: PathBuf,
    config: Config,
}

fn project(setup: &str) -> Project {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let checkout = root.join("workspace--main/api");
    std::fs::create_dir_all(checkout.join("bin")).expect("checkout");
    std::fs::write(checkout.join("bin/migrate"), "#!/bin/sh\necho migrated\n").expect("migrate");
    let status = std::process::Command::new("chmod")
        .args(["+x", "bin/migrate"])
        .current_dir(&checkout)
        .status()
        .expect("chmod");
    assert!(status.success());
    let text = CONFIG.replace("SETUP", setup);
    std::fs::write(root.join("pom.yml"), &text).expect("config");
    let config = Config::load(&root.join("pom.yml")).expect("load");
    Project {
        _temp: temp,
        root,
        config,
    }
}

fn run(
    project: &Project,
    services: &Fake,
    installed: &HashMap<String, String>,
    skipped: &[(String, String)],
) -> (Vec<Check>, Vec<Check>) {
    let has_tool = |_: &str| true;
    let docker = || true;
    let machine = pom_doctor::Machine {
        has_tool: &has_tool,
        docker_running: &docker,
    };
    let input = VerifyInput {
        config: &project.config,
        config_path: &project.root.join("pom.yml"),
        root: &project.root,
        secret_names: &[],
        machine: &machine,
        installed,
        skipped,
        boot_timeout: Duration::from_millis(600),
        worker_grace: Duration::ZERO,
    };
    let mut reported = Vec::new();
    let done = verify(
        &input,
        services,
        &mut |check| reported.push(check.clone()),
        &AtomicBool::new(false),
    );
    (done, reported)
}

fn statuses(checks: &[Check]) -> Vec<(String, CheckStatus)> {
    checks
        .iter()
        .map(|check| (check.kind.title(), check.status))
        .collect()
}

#[test]
fn a_clean_project_installs_and_boots_every_service_then_stops_them() {
    let project = project("echo installed");
    let services = Fake::new(&[
        ("web", BootState::Listening(31022)),
        ("jobs", BootState::Running),
    ]);
    let (done, reported) = run(&project, &services, &HashMap::new(), &[]);
    assert_eq!(
        statuses(&done),
        vec![
            ("config doctor".to_string(), CheckStatus::Passed),
            ("install api".to_string(), CheckStatus::Passed),
            ("boot api > web".to_string(), CheckStatus::Passed),
            ("boot api > jobs".to_string(), CheckStatus::Passed),
        ],
        "{done:#?}"
    );
    assert!(done[1].detail.starts_with("echo installed - "));
    assert_eq!(done[1].command, "echo installed && bin/migrate");
    assert_eq!(done[2].detail, "listening on :31022");
    assert!(done[3].detail.starts_with("running after"));
    assert_eq!(*services.stopped.borrow(), vec!["web", "jobs"]);
    assert_eq!(
        reported.len(),
        8,
        "each check reports its start and its end"
    );
}

#[test]
fn a_failed_install_stops_the_run_with_its_output() {
    let project = project("echo 'Could not find gem rails' >&2; exit 7");
    let services = Fake::new(&[]);
    let (done, _) = run(&project, &services, &HashMap::new(), &[]);
    let last = done.last().expect("a check");
    assert_eq!(last.kind.title(), "install api");
    assert_eq!(last.status, CheckStatus::Failed);
    assert_eq!(last.detail, "Could not find gem rails");
    assert!(last.output.starts_with("exit 7"));
    assert!(services.started.borrow().is_empty());
}

#[test]
fn an_unchanged_install_is_not_run_again_and_a_skipped_service_is_not_booted() {
    let project = project("exit 1");
    let installed = HashMap::from([("api".to_string(), "exit 1 && bin/migrate".to_string())]);
    let services = Fake::new(&[(
        "web",
        BootState::Exited {
            line: "PG::ConnectionBad: database does not exist".into(),
            output: "boot\nPG::ConnectionBad: database does not exist".into(),
        },
    )]);
    let skipped = [("api".to_string(), "jobs".to_string())];
    let (done, _) = run(&project, &services, &installed, &skipped);
    assert_eq!(done[1].status, CheckStatus::Passed);
    assert!(done[1].detail.ends_with("done before"));
    let last = done.last().expect("a check");
    assert_eq!(last.kind.title(), "boot api > web");
    assert_eq!(last.detail, "PG::ConnectionBad: database does not exist");
    assert_eq!(*services.started.borrow(), vec!["web"]);
    assert_eq!(
        *services.stopped.borrow(),
        vec!["web"],
        "a crashed boot is cleaned up"
    );
}

#[test]
fn a_service_that_never_answers_times_out() {
    let project = project("true");
    let services = Fake::new(&[]);
    let (done, _) = run(&project, &services, &HashMap::new(), &[]);
    let last = done.last().expect("a check");
    assert_eq!(last.kind.title(), "boot api > web");
    assert_eq!(last.status, CheckStatus::Failed);
    assert!(last.detail.starts_with("did not answer on its port"));
}

#[test]
fn the_summary_names_services_setup_and_wired_env() {
    let project = project("bundle install");
    let text: Vec<String> = summary_lines(&project.config)
        .iter()
        .map(|line| line.iter().map(|segment| segment.text.as_str()).collect())
        .collect();
    assert_eq!(
        text,
        vec![
            "api: services web (bin/web -p $PORT), jobs (bin/jobs)",
            "api: setup bundle install - migrate bin/migrate",
            "api: DATABASE_URL -> postgresql://{{shared.postgres.url}}/{{db.main}}",
        ]
    );
    assert_eq!(
        counts(&project.config),
        Counts {
            repos: 1,
            services: 2,
            shared: 1,
            databases: 1
        }
    );
}
