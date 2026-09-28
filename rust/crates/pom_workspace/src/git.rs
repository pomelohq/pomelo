//! The git side of a workspace: one worktree per repo, each on the workspace branch or its own.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
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

/// `git -C dir args` that is stopped after `timeout`, for commands that reach the network.
pub(crate) fn git_timed(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git: {error}"))?;
    let drain = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut stream) = stream {
                if let Err(error) = stream.read_to_string(&mut text) {
                    eprintln!("workspace: read git output: {error}");
                }
            }
            text
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    eprintln!("workspace: stop git: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("workspace: reap git: {error}");
                }
                return Err(format!("git {} timed out", args.first().unwrap_or(&"")));
            }
            Err(error) => return Err(format!("git: {error}")),
        }
    };
    let (out, errors) = (
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    );
    if status.success() {
        Ok(out.trim().to_string())
    } else {
        Err(errors.trim().to_string())
    }
}

pub(crate) fn resolves(dir: &Path, reference: &str) -> bool {
    git(dir, &["rev-parse", "--verify", "--quiet", reference]).is_ok()
}

/// The branch a checkout is on (`main` when git cannot tell).
pub fn current_branch(dir: &Path) -> String {
    git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|branch| !branch.is_empty())
        .unwrap_or_else(|| "main".to_string())
}

/// Where a worktree's branch came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchOrigin {
    Local,
    TrackedOrigin,
    /// Made by this run from the base branch.
    Created,
}

/// Why a worktree could not be added.
#[derive(Debug)]
pub enum AddError {
    /// The branch is checked out in another worktree (git allows one).
    CheckedOutElsewhere {
        path: PathBuf,
        output: String,
    },
    Other(String),
}

/// The other checkout git names when it refuses a branch that is already checked out.
pub(crate) fn checked_out_path(output: &str) -> Option<PathBuf> {
    ["used by worktree at '", "checked out at '"]
        .iter()
        .find_map(|marker| output.split_once(marker))
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(path, _)| PathBuf::from(path))
}

/// Adds a worktree for `branch` at `target`: the local branch when it exists, else tracking
/// `origin/<branch>`, else a new branch from `base`. Copies the repo's `copy:` files in.
pub fn add_worktree(
    repo: &Path,
    target: &Path,
    branch: &str,
    base: &str,
    copy: &[String],
) -> Result<BranchOrigin, AddError> {
    if target.exists() {
        return Err(AddError::Other(format!(
            "{} already exists",
            target.display()
        )));
    }
    if let Err(error) = git(repo, &["worktree", "prune"]) {
        eprintln!("workspace: git worktree prune: {error}");
    }
    let target_text = target.to_string_lossy().into_owned();
    let remote = format!("origin/{branch}");
    let (origin, args): (BranchOrigin, Vec<&str>) =
        if resolves(repo, &format!("refs/heads/{branch}")) {
            (
                BranchOrigin::Local,
                vec!["worktree", "add", &target_text, branch],
            )
        } else if resolves(repo, &remote) {
            (
                BranchOrigin::TrackedOrigin,
                vec![
                    "worktree",
                    "add",
                    "--track",
                    "-b",
                    branch,
                    &target_text,
                    &remote,
                ],
            )
        } else {
            (
                BranchOrigin::Created,
                vec!["worktree", "add", "-b", branch, &target_text, base],
            )
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
    if let Err(error) = result {
        return Err(match checked_out_path(&error) {
            Some(path) => AddError::CheckedOutElsewhere {
                path,
                output: error,
            },
            None => AddError::Other(format!("git worktree add: {error}")),
        });
    }
    copy_files(repo, target, copy);
    Ok(origin)
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

/// Removes a worktree, falling back to deleting its folder when git no longer knows it.
pub fn remove_worktree(repo: &Path, worktree: &Path) -> Result<(), String> {
    let worktree_text = worktree.to_string_lossy().into_owned();
    if let Err(error) = git(repo, &["worktree", "remove", "--force", &worktree_text]) {
        if worktree.exists() {
            std::fs::remove_dir_all(worktree).map_err(|remove| format!("{error}; {remove}"))?;
        }
    }
    if let Err(error) = git(repo, &["worktree", "prune"]) {
        eprintln!("workspace: git worktree prune: {error}");
    }
    Ok(())
}

/// Deletes a local branch only when nothing on it would be lost; never the default branch. Returns
/// whether the branch was kept.
pub fn delete_branch_if_safe(repo: &Path, branch: &str, default: &str) -> Result<bool, String> {
    if branch.is_empty() || branch == default {
        return Ok(false);
    }
    if !branch_is_safe_to_delete(repo, branch, default) {
        return Ok(true);
    }
    if resolves(repo, &format!("refs/heads/{branch}")) {
        git(repo, &["branch", "-D", branch])?;
    }
    Ok(false)
}

/// Commits on the local branch that neither an `origin/*` branch nor the default branch has.
pub fn unpushed_commits(repo: &Path, branch: &str, default: &str) -> usize {
    let local = format!("refs/heads/{branch}");
    let mut args = vec!["rev-list", "--count", &local, "--not", "--remotes=origin"];
    let default_ref = format!("refs/heads/{default}");
    if resolves(repo, &default_ref) {
        args.push(&default_ref);
    }
    git(repo, &args)
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}
