//! The node_modules Store page of a remote app through a real server: the overview of the host's store and
//! worktrees, keeping an install, moving a workspace onto the shared copy, and refusing a path that is not a
//! worktree of the project.

#[path = "fixture/terminals.rs"]
mod terminals;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use module_store::{Holding, Overview, RepoState, Request, StoreUpdate};
use pom_remote::server_api::{progress_kind, JobRequest, StoreArgs};
use pom_remote::{Host, HostUrl, JobEvent, ProjectSource, ServerSource};
use terminals::{Remote, TIMEOUT};

fn source(remote: &Remote) -> ServerSource {
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

fn install(remote: &Remote, workspace: &str) {
    let worktree = remote
        .temp
        .path()
        .join(format!("project/workspace--{workspace}/api"));
    std::fs::write(worktree.join("package.json"), r#"{"name":"api"}"#).expect("package.json");
    std::fs::write(
        worktree.join("package-lock.json"),
        r#"{"name":"api","lockfileVersion":3}"#,
    )
    .expect("lockfile");
    let package = worktree.join("node_modules/left-pad");
    std::fs::create_dir_all(&package).expect("node_modules");
    std::fs::write(package.join("index.js"), "module.exports = 1;\n").expect("package");
}

fn run(source: &ServerSource, request: Request, op_id: &str) -> Vec<StoreUpdate> {
    let id = source
        .start_job(&JobRequest::Store(StoreArgs { request }), op_id)
        .expect("start");
    let feed = source.follow_job(&id, 0).expect("follow");
    let deadline = Instant::now() + TIMEOUT;
    let mut updates = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "only {updates:?}");
        match feed.next(Duration::from_millis(100)) {
            JobEvent::Progress(progress) => {
                if progress.kind == progress_kind::DATA {
                    let data = progress.data.clone().expect("data");
                    updates.push(serde_json::from_value(data).expect("an update"));
                }
                if progress.done {
                    assert_eq!(progress.exit, Some(0));
                    return updates;
                }
            }
            JobEvent::Ended(_) => return updates,
            JobEvent::Idle => {}
        }
    }
}

/// The overview after the request, and its result line.
fn outcome(updates: &[StoreUpdate]) -> (Overview, Option<String>, bool) {
    updates
        .iter()
        .find_map(|update| match update {
            StoreUpdate::Overview {
                overview,
                message,
                failed,
                ..
            } => Some((overview.clone(), message.clone(), *failed)),
            _ => None,
        })
        .expect("an overview")
}

fn holdings(overview: &Overview) -> Vec<(String, Holding, PathBuf)> {
    let Some(RepoState::Versions { versions, .. }) = overview
        .repos
        .iter()
        .find(|repo| repo.repo == "api")
        .map(|repo| &repo.state)
    else {
        panic!("api has no versions: {overview:?}");
    };
    let mut users: Vec<_> = versions
        .iter()
        .flat_map(|version| &version.users)
        .map(|user| {
            (
                user.workspace.clone(),
                user.holding.clone(),
                user.path.clone(),
            )
        })
        .collect();
    users.sort_by(|left, right| left.0.cmp(&right.0));
    users
}

fn path_of(users: &[(String, Holding, PathBuf)], workspace: &str) -> PathBuf {
    users
        .iter()
        .find(|user| user.0 == workspace)
        .map(|user| user.2.clone())
        .expect("workspace")
}

#[test]
fn the_page_reads_and_changes_the_hosts_store() {
    let remote = Remote::start(&["/bin/sh"]);
    install(&remote, "main");
    install(&remote, "feat");
    let source = source(&remote);
    assert!(source.transport().expect("transport").module_store);

    let updates = run(&source, Request::Refresh, "op-store-1");
    let Some(StoreUpdate::Place { root, method }) = updates.first() else {
        panic!("no place first: {updates:?}");
    };
    assert!(root.starts_with(&remote.temp.path().join("state").display().to_string()));
    assert!(!method.is_empty());
    let (overview, _, failed) = outcome(&updates);
    assert!(!failed);
    let users = holdings(&overview);
    assert!(
        users
            .iter()
            .all(|user| matches!(user.1, Holding::Own { .. })),
        "{users:?}"
    );
    assert!(updates
        .iter()
        .any(|update| matches!(update, StoreUpdate::Measured { .. })));

    let main = path_of(&users, "main");
    let feat = path_of(&users, "feat");
    let updates = run(
        &source,
        Request::Keep {
            repo: "api".into(),
            path: main,
        },
        "op-store-2",
    );
    let (overview, message, failed) = outcome(&updates);
    assert!(!failed, "{message:?}");
    assert!(message
        .unwrap_or_default()
        .starts_with("Kept api's install"));
    assert!(holdings(&overview)
        .iter()
        .any(|user| user.0 == "main" && user.1 == Holding::Shared));

    let updates = run(
        &source,
        Request::Relink {
            repo: "api".into(),
            workspace: "feat".into(),
            path: feat.clone(),
        },
        "op-store-3",
    );
    let (overview, message, failed) = outcome(&updates);
    assert!(!failed, "{message:?}");
    assert!(holdings(&overview)
        .iter()
        .any(|user| user.0 == "feat" && user.1 == Holding::Shared));
    assert!(feat.join("node_modules/left-pad/index.js").is_file());

    let elsewhere = remote.temp.path().join("state");
    let updates = run(
        &source,
        Request::Keep {
            repo: "api".into(),
            path: elsewhere,
        },
        "op-store-4",
    );
    let (_, message, failed) = outcome(&updates);
    assert!(failed);
    assert!(message
        .unwrap_or_default()
        .contains("not a worktree of api"));

    let outside = JobRequest::Store(StoreArgs {
        request: Request::Keep {
            repo: "api".into(),
            path: PathBuf::from("../escape"),
        },
    });
    assert!(source.start_job(&outside, "op-store-5").is_err());
}
