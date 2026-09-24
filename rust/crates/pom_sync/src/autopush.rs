//! Auto-push: every workspace branch goes to origin, and uncommitted work rides along as a snapshot commit on
//! `refs/pom-wip/<branch>` (built in a scratch index, so the working tree and the real index stay untouched).

use std::path::Path;

use crate::git::{self, current_branch, git_with, LOCAL, NETWORK, PUSH};

const WIP_PREFIX: &str = "refs/pom-wip/";

/// Pushes the worktree's branch and, when the tree has uncommitted changes that differ from the last
/// snapshot, a fresh WIP snapshot. Returns what went wrong, if anything.
pub fn push_worktree(worktree: &Path) -> Result<(), String> {
    let Some(branch) = current_branch(worktree) else {
        return Ok(());
    };
    git::git(
        worktree,
        &["push", "origin", &format!("HEAD:refs/heads/{branch}")],
        PUSH,
    )?;
    if let Some(reference) = write_wip_snapshot(worktree, &branch)? {
        git::git(
            worktree,
            &[
                "push",
                "--force",
                "origin",
                &format!("{reference}:{reference}"),
            ],
            PUSH,
        )?;
    }
    Ok(())
}

/// Records the uncommitted work as a commit on `refs/pom-wip/<branch>`; `None` when the tree is clean or
/// the snapshot would not change.
pub fn write_wip_snapshot(worktree: &Path, branch: &str) -> Result<Option<String>, String> {
    let reference = format!("{WIP_PREFIX}{branch}");
    if git::git(worktree, &["status", "--porcelain"], LOCAL)?.is_empty() {
        return Ok(None);
    }
    let scratch = tempfile_dir()?;
    let index = scratch.join("index");
    let index_text = index.to_string_lossy().into_owned();
    let env = [
        ("GIT_INDEX_FILE", index_text.as_str()),
        ("GIT_AUTHOR_NAME", "pom"),
        ("GIT_AUTHOR_EMAIL", "pom@localhost"),
        ("GIT_COMMITTER_NAME", "pom"),
        ("GIT_COMMITTER_EMAIL", "pom@localhost"),
    ];
    let result = (|| {
        git_with(worktree, &["read-tree", "HEAD"], &env, LOCAL)?;
        git_with(worktree, &["add", "-A"], &env, PUSH)?;
        let tree = git_with(worktree, &["write-tree"], &env, LOCAL)?;
        let previous = git::git(
            worktree,
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("{reference}^{{tree}}"),
            ],
            LOCAL,
        );
        if previous.as_deref() == Ok(tree.as_str()) {
            return Ok(None);
        }
        let commit = git_with(
            worktree,
            &["commit-tree", &tree, "-p", "HEAD", "-m", "pom-wip snapshot"],
            &env,
            LOCAL,
        )?;
        git::git(worktree, &["update-ref", &reference, &commit], LOCAL)?;
        Ok(Some(reference.clone()))
    })();
    if let Err(error) = std::fs::remove_dir_all(&scratch) {
        eprintln!("sync: remove the scratch index: {error}");
    }
    result
}

fn tempfile_dir() -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join(format!(
        "pom-wip-idx-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    ));
    std::fs::create_dir_all(&dir).map_err(|error| format!("scratch index: {error}"))?;
    Ok(dir)
}

/// Deletes origin's WIP snapshots of branches no worktree of this repo has any more.
pub fn prune_wip_refs(worktree: &Path) -> Result<(), String> {
    let listing = git::git(worktree, &["worktree", "list", "--porcelain"], LOCAL)?;
    let local: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.strip_prefix("branch refs/heads/"))
        .collect();
    let remote = git::git(
        worktree,
        &["ls-remote", "origin", &format!("{WIP_PREFIX}*")],
        NETWORK,
    )?;
    for line in remote.lines() {
        let Some(at) = line.find(WIP_PREFIX) else {
            continue;
        };
        let branch = &line[at + WIP_PREFIX.len()..];
        if !branch.is_empty() && !local.contains(&branch) {
            git::git(
                worktree,
                &[
                    "push",
                    "origin",
                    "--delete",
                    &format!("{WIP_PREFIX}{branch}"),
                ],
                PUSH,
            )?;
        }
    }
    Ok(())
}

/// Pushes every repo of every workspace but main; with `prune`, also clears stale WIP snapshots (once per
/// repo). Returns the failures, `repo@branch: error`.
pub fn auto_push_once(
    project_root: &Path,
    default_branch: &str,
    repos: &[String],
    prune: bool,
) -> Vec<String> {
    let known: std::collections::HashSet<String> = repos.iter().cloned().collect();
    let mut failures = Vec::new();
    let mut pruned: Vec<String> = Vec::new();
    for workspace in pom_layout::scan(project_root, default_branch, Some(&known)) {
        if workspace.is_main {
            continue;
        }
        for repo in &workspace.repos {
            if let Err(error) = push_worktree(&repo.path) {
                failures.push(format!("{}@{}: {error}", repo.name, workspace.branch));
            }
            if prune && !pruned.contains(&repo.name) {
                pruned.push(repo.name.clone());
                if let Err(error) = prune_wip_refs(&repo.path) {
                    failures.push(format!("{}: prune: {error}", repo.name));
                }
            }
        }
    }
    failures
}
