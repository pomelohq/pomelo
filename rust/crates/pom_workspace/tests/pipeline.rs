//! Create and delete pipelines against real git repos and a stand-in `docker` that records its calls.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use pom_config::Config;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner};
use pom_workspace::{
    create, delete, failed_run, CreateRequest, DeleteRequest, Event, StageStatus, WorkspaceContext,
    CREATE_STAGES,
};

const CONFIG: &str = r#"session: demo
shared_services:
  postgres:
    image: postgres:16
    ports: ["5432:5432"]
    db_user: app
    db_password: secret
repos:
  api:
    copy: [".env", "config/*.yml"]
    databases:
      main: "api_{{branch.safe}}"
    seed_from_main: true
    env:
      DATABASE: "{{db.main}}"
    setup: ["echo installed > setup.txt"]
    seed: ["touch seeded.txt"]
    pre_delete: ["touch ../../pre-delete-ran"]
    services:
      server: rails s
  web:
    databases:
      main: "web_{{branch.safe}}"
    setup: ["exit 3"]
    seed: ["touch seeded.txt"]
    services:
      dev: pnpm dev
"#;

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    config: Config,
    runner: ServiceRunner,
    state: StateDir,
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn main_repo(path: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(path).expect("repo dir");
    git(path, &["init", "-q", "-b", "main"]);
    std::fs::write(path.join("README"), "x\n").expect("readme");
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "init"]);
    for (name, text) in files {
        let file = path.join(name);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        std::fs::write(file, text).expect("untracked file");
    }
}

fn fake_docker(dir: &Path) -> PathBuf {
    let script = dir.join("docker");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
echo "$*" >> '{log}'
case "$*" in
  "ps -q"*) echo pg-container ;;
  *"-tAc"*"SELECT 1"*) echo 1 ;;
esac
exit 0
"#,
            log = dir.join("docker.log").display()
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("project");
        main_repo(
            &root.join("workspace--main/api"),
            &[
                (".env", "SECRET=1\n"),
                ("config/app.yml", "a: 1\n"),
                ("config/skip.txt", ""),
            ],
        );
        main_repo(&root.join("workspace--main/web"), &[]);
        std::fs::write(root.join("pom.yml"), CONFIG).expect("pom.yml");
        let config = Config::load(&root.join("pom.yml")).expect("config");
        let state = StateDir::new(temp.path().join("state"));
        let runner = ServiceRunner::new(RunnerOptions {
            project_root: root.clone(),
            session: "demo".into(),
            state: state.clone(),
            holders: SocketDir::new(temp.path().join("s")),
            binary: PathBuf::from("/nonexistent"),
            docker: fake_docker(temp.path()),
        });
        Fixture {
            temp,
            root,
            config,
            runner,
            state,
        }
    }

    fn context(&self) -> WorkspaceContext<'_> {
        WorkspaceContext {
            config: &self.config,
            runner: &self.runner,
            state: &self.state,
        }
    }

    fn docker_calls(&self) -> String {
        std::fs::read_to_string(self.temp.path().join("docker.log")).unwrap_or_default()
    }

    fn create(
        &self,
        request: &CreateRequest,
    ) -> (
        Result<pom_workspace::Outcome, pom_workspace::PipelineError>,
        Vec<Event>,
    ) {
        let events = Mutex::new(Vec::new());
        let sink = |event: Event| events.lock().expect("events").push(event);
        let result = create(&self.context(), request, &sink);
        (result, events.into_inner().expect("events"))
    }

    fn delete(
        &self,
        branch: &str,
    ) -> (
        Result<pom_workspace::Outcome, pom_workspace::PipelineError>,
        Vec<Event>,
    ) {
        let events = Mutex::new(Vec::new());
        let sink = |event: Event| events.lock().expect("events").push(event);
        let request = DeleteRequest {
            branch: branch.into(),
            from_stage: 0,
        };
        let result = delete(&self.context(), &request, &sink);
        (result, events.into_inner().expect("events"))
    }

    fn workspace(&self, branch: &str) -> PathBuf {
        self.root.join(format!("workspace--{branch}"))
    }
}

fn request(branch: &str) -> CreateRequest {
    CreateRequest {
        branch: branch.into(),
        ..CreateRequest::default()
    }
}

fn branches(repo: &Path) -> Vec<String> {
    git(repo, &["branch", "--format=%(refname:short)"])
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn create_then_delete_round_trip() {
    let fixture = Fixture::new();
    let (result, events) = fixture.create(&request("feat-x"));
    let outcome = result.expect("create");
    let workspace = fixture.workspace("feat-x");
    let api = workspace.join("api");

    assert_eq!(git(&api, &["rev-parse", "--abbrev-ref", "HEAD"]), "feat-x");
    assert_eq!(
        std::fs::read_to_string(api.join(".env")).expect(".env"),
        "SECRET=1\n"
    );
    assert!(api.join("config/app.yml").is_file());
    assert!(
        !api.join("config/skip.txt").exists(),
        "only the glob's matches are copied"
    );
    let env_file = std::fs::read_to_string(api.join(".env.local")).expect(".env.local");
    assert!(env_file.contains("DATABASE=demo_api_feat-x"), "{env_file}");
    assert_eq!(
        std::fs::read_to_string(api.join("setup.txt")).expect("setup"),
        "installed\n"
    );

    assert!(
        !api.join("seeded.txt").exists(),
        "seed_from_main repos are cloned, not seeded"
    );
    assert!(workspace.join("web/seeded.txt").is_file());
    assert_eq!(outcome.warnings.len(), 1, "{:?}", outcome.warnings);
    assert!(
        outcome.warnings[0].starts_with("web: setup failed (exit 3"),
        "{:?}",
        outcome.warnings
    );

    let docker = fixture.docker_calls();
    assert!(
        docker.contains("CREATE DATABASE \"demo_web_feat-x\""),
        "{docker}"
    );
    assert!(
        docker.contains("CREATE DATABASE \"demo_api_feat-x\" TEMPLATE \"demo_api_main\""),
        "{docker}"
    );
    assert!(
        matches!(events.first(), Some(Event::Started { stages, .. }) if stages.len() == CREATE_STAGES.len())
    );
    assert!(matches!(events.last(), Some(Event::Completed { .. })));
    assert!(failed_run(&fixture.state, "feat-x").is_none());

    let (result, _) = fixture.delete("feat-x");
    let outcome = result.expect("delete");
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert!(fixture.root.join("pre-delete-ran").exists());
    assert!(!workspace.exists());
    let main_api = fixture.root.join("workspace--main/api");
    assert_eq!(branches(&main_api), ["main"], "a merged branch is deleted");
    let docker = fixture.docker_calls();
    assert!(
        docker.contains("DROP DATABASE IF EXISTS \"demo_api_feat-x\""),
        "{docker}"
    );
    assert!(
        !docker.contains("DROP DATABASE IF EXISTS \"demo_api_main\""),
        "{docker}"
    );
}

#[test]
fn delete_keeps_a_branch_with_unpushed_commits() {
    let fixture = Fixture::new();
    let only_web = CreateRequest {
        repos: vec!["web".into()],
        skip_seed: true,
        ..request("wip")
    };
    fixture.create(&only_web).0.expect("create");
    let web = fixture.workspace("wip").join("web");
    assert!(!fixture.workspace("wip").join("api").exists());
    std::fs::write(web.join("work.txt"), "w\n").expect("file");
    git(&web, &["add", "."]);
    git(&web, &["commit", "-q", "-m", "work"]);

    let (result, _) = fixture.delete("wip");
    let outcome = result.expect("delete");
    assert_eq!(outcome.warnings.len(), 1);
    assert!(
        outcome.warnings[0].contains("kept local branch wip"),
        "{:?}",
        outcome.warnings
    );
    let main_web = fixture.root.join("workspace--main/web");
    assert_eq!(branches(&main_web), ["main", "wip"]);
    assert!(!fixture.workspace("wip").exists());
}

#[test]
fn a_failed_worktree_rolls_back_and_the_run_resumes() {
    let fixture = Fixture::new();
    let main_web = fixture.root.join("workspace--main/web");
    let elsewhere = fixture.temp.path().join("elsewhere");
    git(
        &main_web,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat-y",
            &elsewhere.to_string_lossy(),
        ],
    );

    let (result, events) = fixture.create(&request("feat-y"));
    let error = result.expect_err("web's branch is checked out elsewhere");
    assert_eq!(error.stage, 3);
    assert!(
        error.message.starts_with("web: git worktree add"),
        "{error}"
    );
    assert!(matches!(
        events.last(),
        Some(Event::Failed { index: 3, .. })
    ));
    assert!(
        !fixture.workspace("feat-y").join("api").exists(),
        "the api worktree is rolled back"
    );
    let saved = failed_run(&fixture.state, "feat-y").expect("saved run");
    assert_eq!(saved.failed_stage, 3);
    let statuses: Vec<StageStatus> = saved.stages.iter().map(|stage| stage.status).collect();
    assert_eq!(
        statuses,
        [
            StageStatus::Completed,
            StageStatus::Completed,
            StageStatus::Completed,
            StageStatus::Failed,
            StageStatus::Pending,
            StageStatus::Pending,
            StageStatus::Pending,
        ]
    );
    let text = std::fs::read_to_string(fixture.state.path("pipeline-feat-y.json")).expect("file");
    assert!(
        text.contains("\"operation\": \"CreateWorkspace\""),
        "{text}"
    );

    git(
        &main_web,
        &[
            "worktree",
            "remove",
            "--force",
            &elsewhere.to_string_lossy(),
        ],
    );
    let resume = CreateRequest {
        from_stage: saved.failed_stage,
        skip_seed: true,
        ..request("feat-y")
    };
    let (result, events) = fixture.create(&resume);
    result.expect("resumed");
    assert!(matches!(events[1], Event::StageSkipped { index: 0 }));
    assert!(fixture
        .workspace("feat-y")
        .join("web")
        .join(".git")
        .exists());
    assert!(failed_run(&fixture.state, "feat-y").is_none());
}

#[test]
fn create_rejects_bad_requests_before_touching_anything() {
    let fixture = Fixture::new();
    for (bad, message) in [
        (request("main"), "main workspace"),
        (request("a b"), "not a valid branch name"),
        (
            CreateRequest {
                repos: vec!["nope".into()],
                ..request("ok")
            },
            "unknown repo: nope",
        ),
    ] {
        let error = fixture.create(&bad).0.expect_err("rejected");
        assert_eq!(error.stage, 0);
        assert!(error.message.contains(message), "{error}");
    }
    assert!(fixture.docker_calls().is_empty());
    assert!(!fixture.workspace("ok").exists());

    fixture.create(&request("dup")).0.expect("create");
    let error = fixture
        .create(&request("dup"))
        .0
        .expect_err("already there");
    assert!(error.message.contains("already exists"), "{error}");
}
