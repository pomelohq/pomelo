//! A repo of a workspace on a branch other than the workspace's: listing and fetching its branches, switching
//! its worktree, and remembering the choice in `.pom-workspace.json`.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use pom_layout::{Head, WorkspaceState};

use crate::git::{self, git, git_timed, resolves};
use crate::validate_branch_name;

const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const BRANCH_FIELDS: &str =
    "%(refname)%00%(authorname)%00%(committerdate:relative)%00%(subject)%00%(symref)";

/// A branch of a repo, as the branch picker lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchInfo {
    /// Without the `origin/` of a remote branch.
    pub name: String,
    pub remote: bool,
    pub author: String,
    pub relative_time: String,
    pub subject: String,
}

/// `git fetch origin --prune`, stopped after a minute.
pub fn fetch_origin(repo_checkout: &Path) -> Result<(), String> {
    git_timed(
        repo_checkout,
        &["fetch", "origin", "--prune", "--quiet"],
        FETCH_TIMEOUT,
    )
    .map(|_| ())
    .map_err(|error| format!("git fetch origin: {error}"))
}

/// Local branches, then `origin`'s that no local branch shares a name with, most recent commit first.
pub fn list_branches(repo_checkout: &Path) -> Result<Vec<BranchInfo>, String> {
    let output = git(
        repo_checkout,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            &format!("--format={BRANCH_FIELDS}"),
            "refs/heads",
            "refs/remotes/origin",
        ],
    )?;
    Ok(parse_branches(&output))
}

fn parse_branches(output: &str) -> Vec<BranchInfo> {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [reference, author, relative_time, subject, symref] = fields.as_slice() else {
            continue;
        };
        if !symref.is_empty() {
            continue;
        }
        let (name, is_remote) = match (
            reference.strip_prefix("refs/heads/"),
            reference.strip_prefix("refs/remotes/origin/"),
        ) {
            (Some(name), _) => (name, false),
            (None, Some(name)) => (name, true),
            (None, None) => continue,
        };
        let info = BranchInfo {
            name: name.to_string(),
            remote: is_remote,
            author: author.to_string(),
            relative_time: relative_time.to_string(),
            subject: subject.to_string(),
        };
        if is_remote {
            remote.push(info);
        } else {
            local.push(info);
        }
    }
    remote.retain(|branch: &BranchInfo| !local.iter().any(|mine| mine.name == branch.name));
    local.extend(remote);
    local
}

/// Remembers the branch the repo's worktree is on now as the one it uses. Returns that branch.
pub fn keep_repo_branch(
    workspace_folder: &Path,
    workspace_branch: &str,
    repo: &str,
) -> Result<String, String> {
    let branch = match pom_layout::read_head(&workspace_folder.join(repo)) {
        Some(Head::Branch(branch)) => branch,
        Some(Head::Detached(commit)) => {
            return Err(format!("{repo} is not on a branch (HEAD is at {commit})"))
        }
        None => return Err(format!("{repo} is not checked out in this workspace")),
    };
    record(workspace_folder, workspace_branch, repo, &branch)?;
    Ok(branch)
}

/// Checks out `target` in the repo's worktree (the local branch, else tracking `origin/<target>`, else a new
/// branch from HEAD) and remembers it. Refuses while the worktree has uncommitted changes.
pub fn switch_repo_branch(
    workspace_folder: &Path,
    workspace_branch: &str,
    repo: &str,
    target: &str,
) -> Result<(), String> {
    validate_branch_name(target)?;
    let checkout = workspace_folder.join(repo);
    if !pom_layout::is_git_repo(&checkout) {
        return Err(format!("{repo} is not checked out in this workspace"));
    }
    let changes = git(
        &checkout,
        &["status", "--porcelain", "--untracked-files=no"],
    )?;
    if !changes.is_empty() {
        return Err(format!(
            "{repo} has uncommitted changes; commit or stash them before switching to {target}"
        ));
    }
    let remote = format!("origin/{target}");
    let args: Vec<&str> = if resolves(&checkout, &format!("refs/heads/{target}")) {
        vec!["checkout", "--quiet", target]
    } else if resolves(&checkout, &format!("refs/remotes/{remote}")) {
        vec!["checkout", "--quiet", "--track", "-b", target, &remote]
    } else {
        vec!["checkout", "--quiet", "-b", target]
    };
    if let Err(output) = git(&checkout, &args) {
        let project_root = workspace_folder.parent().unwrap_or(workspace_folder);
        return Err(match git::checked_out_path(&output) {
            Some(path) => checked_out_message(repo, target, project_root, &path, &output),
            None => format!("{repo}: git checkout {target}: {output}"),
        });
    }
    record(workspace_folder, workspace_branch, repo, target)
}

/// Fetches origin, then switches the repo to `branch` as `switch_repo_branch` does. Without a network the
/// branches already known still work.
pub fn use_another_branch(
    workspace_folder: &Path,
    workspace_branch: &str,
    repo: &str,
    branch: &str,
) -> Result<(), String> {
    let checkout = workspace_folder.join(repo);
    if let Err(error) = fetch_origin(&checkout) {
        let known = resolves(&checkout, &format!("refs/heads/{branch}"))
            || resolves(&checkout, &format!("refs/remotes/origin/{branch}"));
        if !known {
            return Err(format!("{repo}: {error}"));
        }
        eprintln!("workspace: {repo}: {error}");
    }
    switch_repo_branch(workspace_folder, workspace_branch, repo, branch)
}

fn record(
    workspace_folder: &Path,
    workspace_branch: &str,
    repo: &str,
    branch: &str,
) -> Result<(), String> {
    let mut state = WorkspaceState::load(workspace_folder);
    state.set_repo_branch(repo, branch, workspace_branch);
    state
        .save(workspace_folder)
        .map_err(|error| format!("save workspace state: {error}"))
}

/// A checkout named the way the app shows it: `main/web` for `<project>/workspace--main/web`.
pub fn checkout_label(project_root: &Path, path: &Path) -> String {
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (root, path) = (canonical(project_root), canonical(path));
    let Ok(relative) = path.strip_prefix(&root) else {
        return path.display().to_string();
    };
    let parts: Vec<String> = relative
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .enumerate()
        .map(
            |(index, name)| match name.strip_prefix(pom_layout::WORKSPACE_PREFIX) {
                Some(branch) if index == 0 => branch.to_string(),
                _ => name,
            },
        )
        .collect();
    parts.join("/")
}

pub(crate) fn checked_out_message(
    repo: &str,
    branch: &str,
    project_root: &Path,
    path: &Path,
    output: &str,
) -> String {
    format!(
        "{repo}: {branch} is checked out in {}\n{output}",
        checkout_label(project_root, path)
    )
}

/// The other checkout a "branch is checked out elsewhere" failure points at.
pub fn checked_out_elsewhere(message: &str) -> Option<PathBuf> {
    let first = message.lines().next()?;
    if !first.contains(" is checked out in ") {
        return None;
    }
    git::checked_out_path(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branches_list_local_first_and_fold_tracked_remotes() {
        let line = |reference: &str, symref: &str| {
            format!("{reference}\0Ana Lima\02 hours ago\0Validate cart totals\0{symref}")
        };
        let output = [
            line("refs/heads/main", ""),
            line("refs/remotes/origin/HEAD", "refs/remotes/origin/main"),
            line("refs/remotes/origin/main", ""),
            line("refs/remotes/origin/ana/checkout-api", ""),
            "broken line".to_string(),
        ]
        .join("\n");
        let branches = parse_branches(&output);
        let names: Vec<(&str, bool)> = branches
            .iter()
            .map(|branch| (branch.name.as_str(), branch.remote))
            .collect();
        assert_eq!(names, [("main", false), ("ana/checkout-api", true)]);
        assert_eq!(branches[1].author, "Ana Lima");
        assert_eq!(branches[1].relative_time, "2 hours ago");
        assert_eq!(branches[1].subject, "Validate cart totals");
    }

    #[test]
    fn checkouts_are_labeled_by_workspace() {
        let root = Path::new("/nonexistent/project");
        assert_eq!(
            checkout_label(root, &root.join("workspace--main/web")),
            "main/web"
        );
        assert_eq!(checkout_label(root, Path::new("/other/web")), "/other/web");
        let message = "web: ana/checkout-ui is checked out in main/web\nfatal: 'ana/checkout-ui' is already used by worktree at '/p/workspace--main/web'";
        assert_eq!(
            checked_out_elsewhere(message),
            Some(PathBuf::from("/p/workspace--main/web"))
        );
        assert_eq!(checked_out_elsewhere("web: git worktree add: nope"), None);
    }
}
