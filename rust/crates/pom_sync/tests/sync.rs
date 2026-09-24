//! Refresh-main and auto-push against real repos: a bare origin, main cloned from it, a workspace worktree.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use pom_config::Config;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner};
use pom_sync::{refresh_main, RefreshContext, RepoState};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
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

struct Fixture {
    temp: tempfile::TempDir,
    origin: PathBuf,
    main: PathBuf,
    config: Config,
    runner: ServiceRunner,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::tempdir().expect("temp");
        let origin = temp.path().join("origin.git");
        std::fs::create_dir_all(&origin).expect("origin");
        git(&origin, &["init", "-q", "--bare", "-b", "main"]);
        let seed = temp.path().join("seed");
        std::fs::create_dir_all(&seed).expect("seed");
        git(&seed, &["init", "-q", "-b", "main"]);
        std::fs::write(seed.join("README"), "one\n").expect("readme");
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-q", "-m", "one"]);
        git(
            &seed,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );
        git(&seed, &["push", "-q", "origin", "main"]);

        let root = temp.path().join("project");
        let main = root.join("workspace--main/api");
        std::fs::create_dir_all(main.parent().expect("parent")).expect("main dir");
        git(
            temp.path(),
            &[
                "clone",
                "-q",
                &origin.to_string_lossy(),
                &main.to_string_lossy(),
            ],
        );
        let marker = temp.path().join("migrated");
        std::fs::write(
            root.join("pom.yml"),
            format!(
                "session: demo\nrepos:\n  api:\n    migrate: [\"echo run >> '{}'\"]\n    services:\n      web: sleep 100\n",
                marker.display()
            ),
        )
        .expect("pom.yml");
        let config = Config::load(&root.join("pom.yml")).expect("config");
        let runner = ServiceRunner::new(RunnerOptions {
            project_root: root,
            session: "demo".into(),
            state: StateDir::new(temp.path().join("state")),
            holders: SocketDir::new(temp.path().join("s")),
            binary: PathBuf::from("/nonexistent"),
            docker: PathBuf::from("/nonexistent"),
        });
        Fixture {
            temp,
            origin,
            main,
            config,
            runner,
        }
    }

    fn push_upstream_commit(&self, text: &str) {
        let seed = self.temp.path().join("seed");
        std::fs::write(seed.join("README"), text).expect("readme");
        git(&seed, &["commit", "-q", "-am", text]);
        git(&seed, &["push", "-q", "origin", "main"]);
    }

    fn refresh(&self) -> Result<Vec<(String, RepoState)>, String> {
        let seen = Mutex::new(Vec::new());
        let progress = |repo: &str, state: &RepoState| {
            seen.lock()
                .expect("seen")
                .push((repo.to_string(), state.clone()));
        };
        let lock_dir = self.temp.path().join("locks");
        refresh_main(
            &RefreshContext {
                config: &self.config,
                runner: &self.runner,
                lock_dir: &lock_dir,
            },
            &progress,
        )
    }

    fn migrations(&self) -> usize {
        std::fs::read_to_string(self.temp.path().join("migrated"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

#[test]
fn refresh_pulls_migrates_and_leaves_uncommitted_work_alone() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.refresh(),
        Ok(vec![("api".to_string(), RepoState::NoChange)])
    );
    assert_eq!(fixture.migrations(), 0);

    fixture.push_upstream_commit("two\n");
    assert_eq!(
        fixture.refresh(),
        Ok(vec![("api".to_string(), RepoState::Updated)])
    );
    assert_eq!(
        std::fs::read_to_string(fixture.main.join("README")).expect("readme"),
        "two\n"
    );
    assert_eq!(fixture.migrations(), 1);

    fixture.push_upstream_commit("three\n");
    std::fs::write(fixture.main.join("scratch.txt"), "mine\n").expect("scratch");
    assert_eq!(
        fixture.refresh(),
        Ok(vec![(
            "api".to_string(),
            RepoState::Skipped("uncommitted changes".into())
        )])
    );
    assert!(fixture.main.join("scratch.txt").exists());

    let held = pom_lock::try_acquire(&fixture.temp.path().join("locks"), "demo", "refresh-main")
        .expect("lock")
        .expect("free");
    assert!(fixture.refresh().is_err(), "one refresh at a time");
    drop(held);
}

#[test]
fn auto_push_sends_the_branch_and_a_snapshot_of_uncommitted_work() {
    let fixture = Fixture::new();
    let feature = fixture.temp.path().join("project/workspace--feat/api");
    git(
        &fixture.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            &feature.to_string_lossy(),
        ],
    );
    std::fs::write(feature.join("done.txt"), "done\n").expect("done");
    git(&feature, &["add", "."]);
    git(&feature, &["commit", "-q", "-m", "done"]);
    std::fs::write(feature.join("wip.txt"), "half\n").expect("wip");

    let failures = pom_sync::auto_push_once(
        &fixture.temp.path().join("project"),
        "main",
        &["api".to_string()],
        false,
    );
    assert!(failures.is_empty(), "{failures:?}");
    let refs = git(&fixture.origin, &["for-each-ref", "--format=%(refname)"]);
    assert!(refs.contains("refs/heads/feat"), "{refs}");
    assert!(refs.contains("refs/pom-wip/feat"), "{refs}");
    let snapshot = git(
        &fixture.origin,
        &["ls-tree", "--name-only", "refs/pom-wip/feat"],
    );
    assert!(
        snapshot.contains("wip.txt") && snapshot.contains("done.txt"),
        "{snapshot}"
    );
    assert_eq!(
        git(&feature, &["status", "--porcelain"]),
        "?? wip.txt",
        "the working tree and index are untouched"
    );
    assert_eq!(
        pom_sync::write_wip_snapshot(&feature, "feat"),
        Ok(None),
        "nothing new to snapshot"
    );

    git(
        &fixture.origin,
        &[
            "update-ref",
            "refs/pom-wip/gone",
            &git(&fixture.origin, &["rev-parse", "refs/heads/feat"]),
        ],
    );
    pom_sync::prune_wip_refs(&feature).expect("prune");
    let refs = git(&fixture.origin, &["for-each-ref", "--format=%(refname)"]);
    assert!(!refs.contains("refs/pom-wip/gone"), "{refs}");
    assert!(refs.contains("refs/pom-wip/feat"), "{refs}");
}
