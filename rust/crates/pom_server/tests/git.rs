//! The Git panel through a real server: a repo reads as a local read of the same folder does, changes and
//! reads are checked before git runs, a watch pushes a scan when the repo changes, and a push runs as a job
//! that tells git's progress.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use git::access::{GitAccess, GitOp, GitRead, LocalGit, RemoteCommand, RepoSource};
use git::working_copy::PushMode;
use pom_remote::{Host, HostUrl, ProjectSource, ServerSource};
use terminals::Remote;

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

/// The feat workspace's api as a real repo on branch feat, one commit past main, with an origin to push to.
fn real_repo(remote: &Remote) -> std::path::PathBuf {
    let root = remote.worktree().join("api");
    std::fs::remove_dir_all(root.join(".git")).expect("fake .git");
    git(&root, &["init", "--quiet", "-b", "main"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["config", "user.name", "Dev"]);
    git(&root, &["config", "user.email", "dev@example.com"]);
    std::fs::write(root.join("a.txt"), "one\n").expect("write");
    git(&root, &["add", "."]);
    git(&root, &["commit", "--quiet", "-m", "first"]);
    git(&root, &["checkout", "--quiet", "-b", "feat"]);
    std::fs::write(root.join("b.txt"), "two\n").expect("write");
    git(&root, &["add", "."]);
    git(&root, &["commit", "--quiet", "-m", "second"]);
    let origin = remote.temp.path().join("origin.git");
    git(
        remote.temp.path(),
        &["init", "--quiet", "--bare", &origin.to_string_lossy()],
    );
    git(
        &root,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    root
}

fn remote_git(remote: &Remote) -> (ServerSource, Arc<dyn GitAccess>) {
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
    assert!(source.transport().expect("transport").git);
    let access = source.git("feat").expect("git over the server");
    (source, access)
}

fn api(source: &ServerSource) -> RepoSource {
    let repos = source.git_repos("feat").expect("repos");
    let api = repos
        .into_iter()
        .find(|repo| repo.name == "api")
        .expect("api");
    assert_eq!(api.expected_branch, "feat");
    api
}

#[test]
fn a_remote_repo_reads_and_changes_as_a_local_one() -> Result<(), String> {
    let remote = Remote::start(&["/bin/sh"]);
    let root = real_repo(&remote);
    let (source, access) = remote_git(&remote);
    let repo = api(&source);
    assert_eq!(
        repo.root
            .canonicalize()
            .map_err(|error| error.to_string())?,
        root.canonicalize().map_err(|error| error.to_string())?
    );
    assert!(repo
        .origin
        .as_deref()
        .is_some_and(|origin| origin.ends_with("origin.git")));
    std::fs::write(root.join("a.txt"), "changed\n").map_err(|error| error.to_string())?;
    std::fs::write(root.join("c.txt"), "new\n").map_err(|error| error.to_string())?;

    let local = LocalGit.scan(&repo);
    let scanned = access.scan(&repo);
    assert_eq!(scanned, local);
    assert_eq!(scanned.history.len(), 1, "{:?}", scanned.history);
    assert_eq!(scanned.status.len(), 2);

    access
        .act(
            &repo,
            GitOp::Stage {
                paths: vec!["a.txt".into()],
            },
        )
        .map_err(|error| error.message)?;
    assert_eq!(access.scan(&repo).staged_count(), 1);
    assert_eq!(
        access.file_at(&repo, "HEAD", "a.txt").as_deref(),
        Some("one\n")
    );
    let head = access.head_state(&repo).ok_or("no head")?;
    assert_eq!(head.branch.as_deref(), Some("feat"));
    let commit = scanned.history.first().ok_or("no commit")?;
    let files = access.commit_files(&repo, &commit.sha);
    assert_eq!(files.len(), 1);
    access
        .act(
            &repo,
            GitOp::Commit {
                message: "third".into(),
                options: Default::default(),
            },
        )
        .map_err(|error| error.message)?;
    assert_eq!(access.head_message(&repo).as_deref(), Some("third"));
    let failed = access.act(
        &repo,
        GitOp::Commit {
            message: "nothing".into(),
            options: Default::default(),
        },
    );
    assert!(failed.is_err(), "nothing staged: git refuses, and says why");
    access
        .act(
            &repo,
            GitOp::Discard {
                path: "c.txt".into(),
            },
        )
        .map_err(|error| error.message)?;
    assert!(!root.join("c.txt").exists());
    Ok(())
}

#[test]
fn requests_that_leave_the_repo_or_pass_options_are_refused_before_git_runs() -> Result<(), String>
{
    let remote = Remote::start(&["/bin/sh"]);
    let root = real_repo(&remote);
    let outside = remote.temp.path().join("outside.txt");
    std::fs::write(&outside, "keep\n").map_err(|error| error.to_string())?;
    let (source, access) = remote_git(&remote);
    let repo = api(&source);
    for op in [
        GitOp::Discard {
            path: "../../../outside.txt".into(),
        },
        GitOp::Stage {
            paths: vec!["/etc/passwd".into()],
        },
        GitOp::StageDirectory {
            path: ".git/hooks".into(),
        },
        GitOp::MarkResolved {
            path: "a/../../x".into(),
        },
    ] {
        let refused = access.act(&repo, op.clone());
        assert!(refused.is_err(), "{op:?} ran");
    }
    assert!(outside.exists());
    for read in [
        GitRead::FileAt {
            revision: "--output=/tmp/pwned".into(),
            path: "a.txt".into(),
        },
        GitRead::Patch {
            against: "-p".into(),
        },
        GitRead::CommitFiles {
            sha: "HEAD:a.txt".into(),
        },
    ] {
        assert!(access.read(&repo, read.clone()).is_err(), "{read:?} ran");
    }
    let elsewhere = RepoSource {
        name: "../api".into(),
        ..repo.clone()
    };
    let scan = access.scan(&elsewhere);
    assert!(
        scan.branch.error.is_some(),
        "a repo is named, never a folder"
    );
    let unknown = RepoSource {
        name: "web".into(),
        root: root.clone(),
        ..repo.clone()
    };
    assert!(access
        .act(&unknown, GitOp::StageAll)
        .is_err_and(|error| error.message.contains("no repo web")));
    let push = RemoteCommand::Push {
        remote: "--receive-pack=touch /tmp/pwned".into(),
        branch: "feat".into(),
        remote_branch: "feat".into(),
        mode: PushMode::Normal,
    };
    assert!(access.remote(&repo, push, &|_| {}).is_err());
    Ok(())
}

#[test]
fn the_editor_reads_bases_and_blame_and_stages_lines_through_the_server() -> Result<(), String> {
    let remote = Remote::start(&["/bin/sh"]);
    let root = real_repo(&remote);
    let (source, access) = remote_git(&remote);
    assert!(source.transport().ok_or("no transport")?.git_editor);
    let repo = api(&source);
    std::fs::write(root.join("a.txt"), "one\ntwo\n").map_err(|error| error.to_string())?;
    let local = git::LocalFile(root.join("a.txt"));
    let files = git::access::RepoFiles::new(access.clone(), vec![repo.clone()]);
    let file = files.file(&repo.root.join("a.txt")).ok_or("no file")?;
    use git::FileGit;
    assert_eq!(file.bases(), local.bases());
    assert_eq!(
        file.bases().and_then(|bases| bases.head).as_deref(),
        Some("one\n")
    );
    let blamed = file.blame("one\ntwo\n").ok_or("no blame")?;
    assert_eq!(Some(blamed), local.blame("one\ntwo\n"));

    file.write_index(Some("one\ntwo\n"))?;
    assert_eq!(
        file.bases().and_then(|bases| bases.index).as_deref(),
        Some("one\ntwo\n")
    );
    assert_eq!(access.scan(&repo).staged_count(), 1);
    file.write_index(Some("one\n"))?;
    assert_eq!(access.scan(&repo).staged_count(), 0);

    assert!(files
        .file(&remote.temp.path().join("outside.txt"))
        .is_none());
    for read in [
        GitRead::Bases {
            path: "../../outside.txt".into(),
        },
        GitRead::Blame {
            path: "a.txt".into(),
            contents: "x".repeat(git::access::MAX_EDITOR_TEXT + 1),
        },
    ] {
        assert!(access.read(&repo, read.clone()).is_err(), "{read:?} ran");
    }
    let write = GitOp::WriteIndex {
        path: ".git/config".into(),
        text: Some("[core]\n".into()),
    };
    assert!(access.act(&repo, write).is_err());
    Ok(())
}

#[test]
fn a_watch_pushes_the_scan_when_the_repo_changes() -> Result<(), String> {
    let remote = Remote::start(&["/bin/sh"]);
    let root = real_repo(&remote);
    let (source, access) = remote_git(&remote);
    let repo = api(&source);
    let heard: Arc<Mutex<Vec<(usize, git::scan::RepoScan)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    let watching = access
        .watch(
            std::slice::from_ref(&repo),
            Arc::new(move |index, scan| {
                if let Ok(mut heard) = sink.lock() {
                    heard.push((index, scan));
                }
            }),
        )
        .ok_or("no watch over the server")?;
    let wait_for = |count: usize| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if heard.lock().map(|heard| heard.len()).unwrap_or(0) >= count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    };
    assert!(wait_for(1), "the first scan comes at once");
    std::fs::write(root.join("d.txt"), "new\n").map_err(|error| error.to_string())?;
    assert!(wait_for(2), "a change is pushed");
    let heard = heard.lock().map_err(|error| error.to_string())?.clone();
    let (index, last) = heard.last().ok_or("nothing heard")?;
    assert_eq!(*index, 0);
    assert!(last.status.iter().any(|entry| entry.path == "d.txt"));
    drop(watching);
    Ok(())
}

#[test]
fn a_push_runs_as_a_job_and_tells_its_progress() -> Result<(), String> {
    let remote = Remote::start(&["/bin/sh"]);
    real_repo(&remote);
    let (source, access) = remote_git(&remote);
    let repo = api(&source);
    let heard = Mutex::new(Vec::<String>::new());
    let tell = |line: &str| {
        if let Ok(mut heard) = heard.lock() {
            heard.push(line.to_string());
        }
    };
    let output = access
        .remote(
            &repo,
            RemoteCommand::Push {
                remote: "origin".into(),
                branch: "feat".into(),
                remote_branch: "feat".into(),
                mode: PushMode::SetUpstream,
            },
            &tell,
        )
        .map_err(|error| error.message)?;
    assert!(output.stderr.contains("[new branch]"), "{}", output.stderr);
    assert!(!output.stderr.contains("Writing objects"));
    let heard = heard.into_inner().unwrap_or_default();
    assert!(!heard.is_empty(), "git's progress is told while it runs");
    let pulled = access
        .remote(
            &repo,
            RemoteCommand::Pull {
                remote: "origin".into(),
                branch: None,
                rebase: false,
            },
            &|_| {},
        )
        .map_err(|error| error.message)?;
    assert!(
        pulled.stdout.contains("Already up to date"),
        "{}",
        pulled.stdout
    );
    let missing = access.remote(
        &repo,
        RemoteCommand::Fetch {
            remote: Some("nowhere".into()),
        },
        &|_| {},
    );
    assert!(missing.is_err_and(|error| !error.output.log().is_empty()));
    Ok(())
}

#[test]
fn the_branch_menu_lists_keeps_and_switches_on_the_host() -> Result<(), String> {
    let remote = Remote::start(&["/bin/sh"]);
    let root = real_repo(&remote);
    let (source, _) = remote_git(&remote);
    let listed = source
        .git_branches("feat", "api")
        .map_err(|error| error.to_string())?;
    let names: Vec<&str> = listed
        .branches
        .iter()
        .map(|branch| branch.name.as_str())
        .collect();
    assert!(
        names.contains(&"main") && names.contains(&"feat"),
        "{names:?}"
    );
    assert_eq!(listed.base, "feat");
    let said = source
        .change_git_branch("feat", "api", "switch", "main")
        .map_err(|error| error.to_string())?;
    assert_eq!(said, "Switched api to main");
    let head =
        std::fs::read_to_string(root.join(".git/HEAD")).map_err(|error| error.to_string())?;
    assert!(head.contains("refs/heads/main"), "{head}");
    let kept = source
        .change_git_branch("feat", "api", "keep", "")
        .map_err(|error| error.to_string())?;
    assert_eq!(kept, "api stays on main in this workspace");
    assert!(source
        .change_git_branch("feat", "api", "switch", "-x")
        .is_err());
    assert!(source
        .change_git_branch("feat", "api", "delete", "main")
        .is_err());
    assert!(source
        .change_git_branch("feat", "../api", "keep", "")
        .is_err());
    Ok(())
}
