//! A Store page request run in place, as a window with a project open here runs it: the overview it sends,
//! keeping an install, moving a workspace onto the shared copy, and refusing a path outside the worktrees.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use module_store::{
    Holding, Overview, RepoState, RepoWorktrees, Request, Store, StoreUpdate, Worktree,
};
use pom_workspace::{run_store_job, SizeCache, StoreJob};

fn install(worktree: &Path) {
    let package = worktree.join("node_modules/left-pad");
    std::fs::create_dir_all(&package).expect("node_modules");
    std::fs::write(package.join("index.js"), "module.exports = 1;\n").expect("package");
    std::fs::write(worktree.join("package.json"), r#"{"name":"api"}"#).expect("package.json");
    std::fs::write(
        worktree.join("package-lock.json"),
        r#"{"name":"api","lockfileVersion":3}"#,
    )
    .expect("lockfile");
}

fn repos(main: &Path, feat: &Path) -> Vec<RepoWorktrees> {
    vec![RepoWorktrees {
        repo: "api".into(),
        worktrees: vec![
            Worktree {
                workspace: "main".into(),
                is_main: true,
                path: main.to_path_buf(),
            },
            Worktree {
                workspace: "feat".into(),
                is_main: false,
                path: feat.to_path_buf(),
            },
        ],
        node: Some("v22.1.0".into()),
        setup: Vec::new(),
    }]
}

fn run(store: &Store, request: Request, repos: Vec<RepoWorktrees>) -> Vec<StoreUpdate> {
    let updates = Mutex::new(Vec::new());
    let job = StoreJob {
        request,
        repos,
        previous: None,
        services: None,
    };
    run_store_job(store, job, &SizeCache::default(), &|update| {
        updates.lock().expect("updates").push(update);
        true
    });
    updates.into_inner().expect("updates")
}

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

fn holding(overview: &Overview, workspace: &str) -> Holding {
    let Some(RepoState::Versions { versions, .. }) = overview.repos.first().map(|repo| &repo.state)
    else {
        panic!("no versions: {overview:?}");
    };
    versions
        .iter()
        .flat_map(|version| &version.users)
        .find(|user| user.workspace == workspace)
        .map(|user| user.holding.clone())
        .expect("workspace")
}

#[test]
fn a_local_request_keeps_relinks_and_reports_like_before() {
    let temp = tempfile::tempdir().expect("temp");
    let main = temp.path().join("workspace--main/api");
    let feat = temp.path().join("workspace--feat/api");
    install(&main);
    install(&feat);
    let store = Store::at(temp.path().join("nm-store"));

    let updates = run(&store, Request::Refresh, repos(&main, &feat));
    assert!(!updates
        .iter()
        .any(|update| matches!(update, StoreUpdate::Place { .. })));
    let (overview, message, failed) = outcome(&updates);
    assert!(!failed && message.is_none());
    assert!(matches!(holding(&overview, "feat"), Holding::Own { .. }));

    let keep = Request::Keep {
        repo: "api".into(),
        path: main.clone(),
    };
    let (overview, message, failed) = outcome(&run(&store, keep, repos(&main, &feat)));
    assert!(!failed, "{message:?}");
    assert_eq!(holding(&overview, "main"), Holding::Shared);

    let relink = Request::Relink {
        repo: "api".into(),
        workspace: "feat".into(),
        path: feat.clone(),
    };
    let (overview, message, failed) = outcome(&run(&store, relink, repos(&main, &feat)));
    assert!(!failed, "{message:?}");
    assert_eq!(holding(&overview, "feat"), Holding::Shared);
    assert!(feat.join("node_modules/left-pad/index.js").is_file());

    let elsewhere = Request::Keep {
        repo: "api".into(),
        path: PathBuf::from("/"),
    };
    let (_, message, failed) = outcome(&run(&store, elsewhere, repos(&main, &feat)));
    assert!(failed);
    assert!(message
        .unwrap_or_default()
        .contains("not a worktree of api"));
}
