//! `pom ws` and `pom prepare-main` through the real binary, on a throwaway git project.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const POM: &str = env!("CARGO_BIN_EXE_pom");

struct Fixture {
    temp: tempfile::TempDir,
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
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomw")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        let api = root.join("workspace--main/api");
        std::fs::create_dir_all(&api).expect("repo");
        git(&api, &["init", "-q", "-b", "main"]);
        std::fs::write(api.join("README"), "x\n").expect("readme");
        git(&api, &["add", "."]);
        git(&api, &["commit", "-q", "-m", "init"]);
        std::fs::create_dir_all(temp.path().join("zdot")).expect("zdot");
        std::fs::write(
            root.join("pom.yml"),
            r#"session: demo
default_branch: main
repos:
  api:
    setup: ["touch set-up"]
    migrate: ["touch migrated"]
    seed: ["touch seeded"]
    services:
      web:
        cmd: "sleep 1000"
"#,
        )
        .expect("pom.yml");
        Fixture { temp }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn pom(&self, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(self.root())
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("ZDOTDIR", self.temp.path().join("zdot"))
            .output()
            .expect("pom")
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
fn create_list_rename_and_delete_a_workspace() {
    let fixture = Fixture::new();
    let created = fixture.pom(&["ws", "create", "feat-x"]);
    let said = text(&created);
    assert!(created.status.success(), "{said}");
    assert!(
        said.contains(">>> [4/7] Creating git worktrees (parallel)"),
        "{said}"
    );
    assert!(said.contains("Workspace feat-x ready"), "{said}");
    let worktree = fixture.root().join("workspace--feat-x/api");
    assert!(worktree.join("set-up").exists());
    assert!(worktree.join("seeded").exists());

    let renamed = fixture.pom(&["workspace", "rename", "feat-x", "Login page"]);
    assert!(renamed.status.success(), "{}", text(&renamed));
    let listed = text(&fixture.pom(&["ws", "list"]));
    let line = listed
        .lines()
        .find(|line| line.starts_with("feat-x"))
        .unwrap_or_default();
    assert!(
        line.contains("api") && line.contains("\"Login page\""),
        "{listed}"
    );
    assert!(
        listed.lines().any(|line| line.starts_with("main ")),
        "{listed}"
    );

    let deleted = fixture.pom(&["ws", "delete", "feat-x"]);
    assert!(deleted.status.success(), "{}", text(&deleted));
    assert!(text(&deleted).contains("Workspace feat-x deleted"));
    assert!(!fixture.root().join("workspace--feat-x").exists());
    let main_api = fixture.root().join("workspace--main/api");
    assert_eq!(
        git(&main_api, &["branch", "--format=%(refname:short)"]),
        "main"
    );
}

#[test]
fn a_failed_create_says_how_to_resume() {
    let fixture = Fixture::new();
    let failed = fixture.pom(&["ws", "create", "main"]);
    assert_eq!(failed.status.code(), Some(1));
    let said = text(&failed);
    assert!(
        said.contains("stage 1 (Validating config and hosts) failed"),
        "{said}"
    );
    assert!(said.contains("pom ws create main --from-stage 1"), "{said}");

    let usage = fixture.pom(&["ws", "create", "x", "--repos", "api:other"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(text(&usage).contains("every repo checks out the workspace branch"));
}

#[test]
fn prepare_main_migrates_and_seeds_main() {
    let fixture = Fixture::new();
    let prepared = fixture.pom(&["prepare-main"]);
    let said = text(&prepared);
    assert!(prepared.status.success(), "{said}");
    assert!(
        said.contains("skipped: Resetting main's databases"),
        "{said}"
    );
    let api = fixture.root().join("workspace--main/api");
    assert!(api.join("migrated").exists() && api.join("seeded").exists());

    std::fs::remove_file(api.join("seeded")).expect("rm");
    let prepared = fixture.pom(&["prepare-main", "--no-seed"]);
    assert!(prepared.status.success(), "{}", text(&prepared));
    assert!(!api.join("seeded").exists());
}
