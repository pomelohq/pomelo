//! The git side of a workspace: one worktree per repo, on the workspace branch.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn resolves(dir: &Path, reference: &str) -> bool {
    git(dir, &["rev-parse", "--verify", "--quiet", reference]).is_ok()
}

/// The branch a checkout is on (`main` when git cannot tell).
pub fn current_branch(dir: &Path) -> String {
    git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|branch| !branch.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

/// Adds a worktree for `branch` at `target`: the local branch when it exists, else tracking
/// `origin/<branch>`, else a new branch from `base`. Copies the repo's `copy:` files in.
pub fn add_worktree(
    repo: &Path,
    target: &Path,
    branch: &str,
    base: &str,
    copy: &[String],
) -> Result<(), String> {
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    if let Err(error) = git(repo, &["worktree", "prune"]) {
        eprintln!("workspace: git worktree prune: {error}");
    }
    let target_text = target.to_string_lossy().into_owned();
    let remote = format!("origin/{branch}");
    let args: Vec<&str> = if resolves(repo, &format!("refs/heads/{branch}")) {
        vec!["worktree", "add", &target_text, branch]
    } else if resolves(repo, &remote) {
        vec![
            "worktree",
            "add",
            "--track",
            "-b",
            branch,
            &target_text,
            &remote,
        ]
    } else {
        vec!["worktree", "add", "-b", branch, &target_text, base]
    };
    let mut result = git(repo, &args);
    // A worktree left registered by a crash holds the branch; clear it and try once more.
    if result.as_ref().is_err_and(|error| {
        error.contains("already used by worktree") || error.contains("is already checked out")
    }) {
        if let Err(error) = git(repo, &["worktree", "prune"]) {
            eprintln!("workspace: git worktree prune: {error}");
        }
        result = git(repo, &args);
    }
    result.map_err(|error| format!("git worktree add: {error}"))?;
    copy_files(repo, target, copy);
    Ok(())
}

/// Copies files (glob patterns allowed) from the main checkout into a new worktree, keeping any the
/// worktree already has.
fn copy_files(repo: &Path, target: &Path, patterns: &[String]) {
    for pattern in patterns {
        for relative in matching(repo, pattern) {
            let (source, destination) = (repo.join(&relative), target.join(&relative));
            if !source.is_file() || destination.exists() {
                continue;
            }
            let copied = destination
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| std::fs::copy(&source, &destination).map(|_| ()));
            if let Err(error) = copied {
                eprintln!("workspace: copy {}: {error}", relative.display());
            }
        }
    }
}

/// Paths under `root` matching a pattern with `*` in its last component (or the literal path).
fn matching(root: &Path, pattern: &str) -> Vec<PathBuf> {
    if !pattern.contains('*') {
        return vec![PathBuf::from(pattern)];
    }
    let (folder, name) = pattern.rsplit_once('/').unwrap_or(("", pattern));
    if folder.contains('*') {
        return Vec::new();
    }
    let (prefix, suffix) = name.split_once('*').unwrap_or((name, ""));
    let Ok(entries) = std::fs::read_dir(root.join(folder)) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|file| {
            file.starts_with(prefix)
                && file.ends_with(suffix)
                && file.len() >= prefix.len() + suffix.len()
        })
        .map(|file| Path::new(folder).join(file))
        .collect();
    found.sort();
    found
}

/// Whether deleting `branch` loses nothing: every commit is on its remote copy or in `default`.
pub fn branch_is_safe_to_delete(repo: &Path, branch: &str, default: &str) -> bool {
    let local = format!("refs/heads/{branch}");
    if !resolves(repo, &local) {
        return true;
    }
    let remote = format!("origin/{branch}");
    if resolves(repo, &remote)
        && git(
            repo,
            &["rev-list", "--count", &format!("{remote}..{local}")],
        )
        .as_deref()
            == Ok("0")
    {
        return true;
    }
    [format!("origin/{default}"), default.to_string()]
        .iter()
        .filter(|base| resolves(repo, base))
        .any(|base| git(repo, &["merge-base", "--is-ancestor", &local, base]).is_ok())
}

/// Removes a worktree; the branch goes too only when nothing on it would be lost. Returns whether
/// the branch was kept.
pub fn remove_worktree(
    repo: &Path,
    worktree: &Path,
    branch: &str,
    default: &str,
) -> Result<bool, String> {
    let worktree_text = worktree.to_string_lossy().into_owned();
    if let Err(error) = git(repo, &["worktree", "remove", "--force", &worktree_text]) {
        if worktree.exists() {
            std::fs::remove_dir_all(worktree).map_err(|remove| format!("{error}; {remove}"))?;
        }
    }
    if let Err(error) = git(repo, &["worktree", "prune"]) {
        eprintln!("workspace: git worktree prune: {error}");
    }
    if branch_is_safe_to_delete(repo, branch, default) {
        if resolves(repo, &format!("refs/heads/{branch}")) {
            git(repo, &["branch", "-D", branch])?;
        }
        Ok(false)
    } else {
        Ok(true)
    }
}
