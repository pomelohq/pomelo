//! The Services panel of a remote app through a real server and the app's source: the facts it shows, a mode
//! and env profile picked, a service run and its output followed, the shared services, the facts pushed as
//! they change, and a repo command run as a job.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::sync::Arc;
use std::time::{Duration, Instant};

use pom_remote::server_api::{progress_kind, service_status, JobRequest, ServicesFacts, TaskArgs};
use pom_remote::{
    FeedEvent, Host, HostUrl, JobEvent, LogEvent, ProjectSource, ServerSource, ServiceVerb, Update,
};
use terminals::{Remote, TIMEOUT};

const CONFIG: &str = r#"session: demo
default_branch: main
environments:
  staging:
    api.web: https://web.staging.example
shared_services:
  mock:
    cmd: "echo mock-up; exec sleep 60"
repos:
  api:
    profiles: [staging]
    shortcuts:
      - { key: migrate, desc: Run migrations, cmd: "echo migrated in $(basename $PWD)" }
      - { key: broken, cmd: "echo going wrong; exit 3" }
    services:
      web:
        type: backend
        mode: dev
        modes:
          dev: "echo web-dev-up; exec sleep 60"
          prod: "echo web-prod-up; exec sleep 60"
"#;

fn source(remote: &Remote) -> ServerSource {
    std::fs::write(remote.temp.path().join("project/pom.yml"), CONFIG).expect("pom.yml");
    let socket = remote.paths.socket.clone();
    let opener: pom_remote::Opener = Arc::new(move |_reconnect| {
        let stream = std::os::unix::net::UnixStream::connect(&socket)?;
        pom_rpc::Transport::unix(stream)
    });
    let host = Host {
        name: "dev".into(),
        url: HostUrl::parse("ssh://dev@dev.example.net").expect("url"),
        pom_path: None,
        transport: None,
    };
    let source = ServerSource::with_opener(host, remote.cfg(), opener);
    source.workspaces().expect("connect");
    source
}

fn facts_until(source: &ServerSource, done: impl Fn(&ServicesFacts) -> bool) -> ServicesFacts {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let facts = source.services_facts("feat").expect("facts");
        if done(&facts) {
            return facts;
        }
        assert!(Instant::now() < deadline, "never came to pass: {facts:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn web(facts: &ServicesFacts) -> &pom_remote::server_api::ServiceFact {
    facts
        .services
        .iter()
        .find(|fact| fact.repo == "api" && fact.service == "web")
        .expect("api/web")
}

fn shared_running(facts: &ServicesFacts) -> bool {
    facts
        .shared
        .iter()
        .any(|fact| fact.name == "mock" && fact.running)
}

#[test]
fn the_panel_reads_and_drives_a_workspace_through_the_server() {
    let remote = Remote::start(&["/bin/sh"]);
    let source = source(&remote);
    let offers = source.transport().expect("transport").services;
    assert!(offers.facts && offers.run && offers.shared && offers.mode);
    assert!(offers.env_profile && offers.shared_logs && offers.tasks);

    let facts = source.services_facts("feat").expect("facts");
    assert_eq!(facts.config, CONFIG);
    assert_eq!(facts.root, remote.worktree());
    assert_eq!(facts.repos, ["api"]);
    let service = web(&facts);
    assert_eq!(service.status, service_status::STOPPED);
    assert_eq!(
        (service.mode.as_str(), service.env_profile.as_str()),
        ("dev", "local")
    );
    assert!(service.holder.starts_with("svc-demo-feat-api-web"));
    assert!(!shared_running(&facts));

    source
        .set_env_profile("feat", "api", "web", "staging")
        .expect("env profile");
    assert_eq!(
        web(&source.services_facts("feat").expect("facts")).env_profile,
        "staging"
    );
    assert!(source
        .set_env_profile("feat", "api", "web", "nowhere")
        .is_err());
    source
        .set_service_mode("feat", "api", "web", "prod")
        .expect("mode");
    assert_eq!(
        web(&source.services_facts("feat").expect("facts")).mode,
        "prod"
    );

    source
        .run_service("feat", "api", "web", ServiceVerb::Start)
        .expect("start");
    let facts = facts_until(&source, |facts| {
        web(facts).status == service_status::RUNNING
    });
    assert!(web(&facts).pid.is_some() && web(&facts).started_at.is_some());
    let feed = source
        .follow_logs("feat", "api/web")
        .expect("follow")
        .expect("a stream");
    let deadline = Instant::now() + TIMEOUT;
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("web-prod-up") {
        assert!(
            Instant::now() < deadline,
            "only {:?}",
            String::from_utf8_lossy(&seen)
        );
        if let LogEvent::Data(bytes) = feed.next(Duration::from_millis(100)) {
            seen.extend(bytes);
        }
    }

    source
        .run_shared("mock", ServiceVerb::Start)
        .expect("shared start");
    facts_until(&source, shared_running);
    let deadline = Instant::now() + TIMEOUT;
    while !source
        .shared_logs("mock", 0)
        .expect("shared logs")
        .contains("mock-up")
    {
        assert!(
            Instant::now() < deadline,
            "the shared service never printed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    source
        .run_shared("mock", ServiceVerb::Stop)
        .expect("shared stop");
    facts_until(&source, |facts| !shared_running(facts));

    source
        .run_service("feat", "api", "web", ServiceVerb::Stop)
        .expect("stop");
    assert_eq!(
        web(&source.services_facts("feat").expect("facts")).status,
        service_status::STOPPED
    );
    assert!(source
        .run_service("feat", "api", "nope", ServiceVerb::Start)
        .is_err());
}

#[test]
fn the_facts_are_pushed_as_they_change() {
    let remote = Remote::start(&["/bin/sh"]);
    let source = source(&remote);
    let feed = source
        .events("feat", None)
        .expect("events")
        .expect("pushes");
    let next_facts = |want: &dyn Fn(&ServicesFacts) -> bool| {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            assert!(Instant::now() < deadline, "no such push");
            match feed.next(Duration::from_millis(100)) {
                FeedEvent::Update(Update::Services(update)) => {
                    if let Some(facts) = update.facts.filter(|facts| want(facts)) {
                        return facts;
                    }
                }
                FeedEvent::Ended(reason) => panic!("pushes ended: {reason}"),
                _ => {}
            }
        }
    };
    let first = next_facts(&|facts| web(facts).status == service_status::STOPPED);
    assert_eq!(first.branch, "feat");
    source
        .run_service("feat", "api", "web", ServiceVerb::Start)
        .expect("start");
    next_facts(&|facts| web(facts).status == service_status::RUNNING);
    source
        .run_service("feat", "api", "web", ServiceVerb::Stop)
        .expect("stop");
    next_facts(&|facts| web(facts).status == service_status::STOPPED);
}

fn task_events(
    source: &ServerSource,
    label: &str,
    op_id: &str,
) -> Vec<pom_remote::server_api::Progress> {
    let request = JobRequest::Task(TaskArgs {
        branch: "feat".into(),
        repo: "api".into(),
        label: label.into(),
    });
    let id = source.start_job(&request, op_id).expect("task");
    let feed = source.follow_job(&id, 0).expect("follow");
    let deadline = Instant::now() + TIMEOUT;
    let mut events = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "only {events:?}");
        match feed.next(Duration::from_millis(100)) {
            JobEvent::Progress(progress) => {
                let done = progress.done;
                events.push(progress);
                if done {
                    return events;
                }
            }
            JobEvent::Ended(_) => return events,
            JobEvent::Idle => {}
        }
    }
}

#[test]
fn a_repo_command_runs_as_a_job_in_its_worktree() {
    let remote = Remote::start(&["/bin/sh"]);
    let source = source(&remote);
    let events = task_events(&source, "Run migrations", "op-task-1");
    assert_eq!(events[0].stages, ["api: Run migrations"]);
    assert!(
        events
            .iter()
            .any(|event| event.kind == progress_kind::LINE && event.line == "migrated in api"),
        "{events:?}"
    );
    assert_eq!(events.last().and_then(|event| event.exit), Some(0));

    let events = task_events(&source, "broken", "op-task-2");
    assert!(events.iter().any(|event| event.line == "going wrong"));
    assert_eq!(events.last().and_then(|event| event.exit), Some(3));

    let unknown = JobRequest::Task(TaskArgs {
        branch: "feat".into(),
        repo: "api".into(),
        label: "rm -rf /".into(),
    });
    assert!(source.start_job(&unknown, "op-task-3").is_err());
}
