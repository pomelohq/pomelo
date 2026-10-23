use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use git::access::{
    apply, check_path, check_ref, inside, run_remote, sign_in_failed, GitAccess, GitOp, GitRead,
    LocalGit, RemoteCommand, RepoSource,
};
use git::scan::RepoScan;
use git::working_copy::{CommandOutput, PushMode};

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Dev")
        .env("GIT_AUTHOR_EMAIL", "dev@example.com")
        .env("GIT_COMMITTER_NAME", "Dev")
        .env("GIT_COMMITTER_EMAIL", "dev@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

fn repo(root: &Path) -> RepoSource {
    git(root, &["init", "--quiet", "-b", "main"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("a.txt"), "one\n").expect("write");
    git(root, &["add", "."]);
    git(root, &["commit", "--quiet", "-m", "first"]);
    RepoSource {
        name: "api".into(),
        root: root.to_path_buf(),
        default_branch: "main".into(),
        ..RepoSource::default()
    }
}

#[test]
fn paths_and_refs_that_could_leave_the_repo_or_pass_as_options_are_refused() {
    for good in [
        "a.txt",
        "src/main.rs",
        "dir/",
        "./a.txt",
        ".gitignore",
        "-dash.txt",
    ] {
        assert!(check_path(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "../x",
        "a/../../x",
        "/etc/passwd",
        ".git/config",
        "a\0b",
        "a\\b",
        "line\nbreak",
    ] {
        assert!(check_path(bad).is_err(), "{bad:?}");
    }
    for good in [
        "HEAD",
        "main",
        "origin",
        "feat-login",
        "abc123",
        "feature/x",
        "HEAD^",
    ] {
        assert!(check_ref("ref", good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "-x",
        "--output=/tmp/x",
        "a..b",
        "HEAD:a.txt",
        "a b",
        "a\nb",
    ] {
        assert!(check_ref("ref", bad).is_err(), "{bad:?}");
    }
    let read = GitRead::FileAt {
        revision: "--output=/tmp/x".into(),
        path: "a.txt".into(),
    };
    assert!(read.check().is_err());
    let push = RemoteCommand::Push {
        remote: "--receive-pack=evil".into(),
        branch: "main".into(),
        remote_branch: "main".into(),
        mode: PushMode::Normal,
    };
    assert!(push.check().is_err());
}

#[test]
fn a_symlink_out_of_the_repo_is_not_followed() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let root = dir.path().join("api");
    std::fs::create_dir(&root)?;
    let source = repo(&root);
    std::fs::write(outside.path().join("keep.txt"), "mine\n")?;
    std::os::unix::fs::symlink(outside.path(), root.join("out"))?;
    assert!(inside(&root, "out/keep.txt").is_err());
    let refused = apply(
        &source.root,
        &GitOp::Discard {
            path: "out/keep.txt".into(),
        },
    );
    assert!(refused.is_err());
    assert_eq!(
        std::fs::read_to_string(outside.path().join("keep.txt"))?,
        "mine\n"
    );
    assert!(
        inside(&root, "out").is_ok(),
        "the link itself sits in the repo"
    );
    assert!(inside(&root, "new/file.txt").is_ok());
    Ok(())
}

#[test]
fn local_git_reads_and_changes_the_repo_as_the_panel_did() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let source = repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "two\n")?;
    std::fs::write(dir.path().join("b.txt"), "new\n")?;
    let access = LocalGit;
    let scan = access.scan(&source);
    assert_eq!(scan, RepoScan::read(dir.path(), "main"));
    assert_eq!(scan.status.len(), 2);
    access
        .act(
            &source,
            GitOp::Stage {
                paths: vec!["a.txt".into()],
            },
        )
        .map_err(|error| std::io::Error::other(error.message))?;
    assert_eq!(access.scan(&source).staged_count(), 1);
    assert_eq!(
        access.file_at(&source, "HEAD", "a.txt").as_deref(),
        Some("one\n")
    );
    access
        .act(
            &source,
            GitOp::Commit {
                message: "second".into(),
                options: Default::default(),
            },
        )
        .map_err(|error| std::io::Error::other(error.message))?;
    assert_eq!(access.head_message(&source).as_deref(), Some("second"));
    std::fs::write(
        dir.path().join("b.txt"),
        "<<<<<<< ours\nx\n=======\ny\n>>>>>>> theirs\n",
    )?;
    let left = access.act(
        &source,
        GitOp::MarkResolved {
            path: "b.txt".into(),
        },
    );
    assert_eq!(
        left.map_err(|error| error.message),
        Err("b.txt still has 1 conflict; resolve them first".to_string())
    );
    access
        .act(
            &source,
            GitOp::Discard {
                path: "b.txt".into(),
            },
        )
        .map_err(|error| std::io::Error::other(error.message))?;
    assert!(!dir.path().join("b.txt").exists());
    Ok(())
}

#[test]
fn a_push_tells_its_progress_and_leaves_it_out_of_the_output() -> std::io::Result<()> {
    let dir = tempfile::tempdir()?;
    let origin = dir.path().join("origin.git");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work)?;
    git(dir.path(), &["init", "--quiet", "--bare", "origin.git"]);
    repo(&work);
    git(
        &work,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    let heard = Mutex::new(Vec::<String>::new());
    let tell = |line: &str| {
        if let Ok(mut heard) = heard.lock() {
            heard.push(line.to_string());
        }
    };
    let output = run_remote(
        &work,
        &RemoteCommand::Push {
            remote: "origin".into(),
            branch: "main".into(),
            remote_branch: "main".into(),
            mode: PushMode::SetUpstream,
        },
        Some(&tell),
    )
    .map_err(|error| std::io::Error::other(error.message))?;
    let heard = heard.into_inner().unwrap_or_default();
    assert!(
        heard.iter().any(|line| line.starts_with("Writing objects")),
        "{heard:?}"
    );
    assert!(
        !output.stderr.contains("Writing objects"),
        "{}",
        output.stderr
    );
    assert!(output.stderr.contains("[new branch]"), "{}", output.stderr);
    Ok(())
}

#[test]
fn a_refused_sign_in_is_recognised() {
    let said = |stderr: &str| CommandOutput {
        stdout: String::new(),
        stderr: stderr.into(),
    };
    assert!(sign_in_failed(&said(
        "fatal: could not read Username for 'https://github.com': terminal prompts disabled\n"
    )));
    assert!(sign_in_failed(&said(
        "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.\n"
    )));
    assert!(!sign_in_failed(&said(
        "! [rejected] main -> main (fetch first)\n"
    )));
}
