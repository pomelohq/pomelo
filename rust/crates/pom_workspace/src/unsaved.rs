//! Work a workspace holds that deleting it would lose: uncommitted changes and commits no remote has.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use pom_config::Config;

use crate::git;

/// Why the workspace on `branch` is not safe to delete, one line per repo; empty when nothing would be lost.
/// `pushed_heads` maps a repo to the commit its PR was last pushed at: a local branch at or behind it is pushed
/// even after the remote branch is gone.
pub fn unsaved_work(
    config: &Config,
    project_root: &Path,
    branch: &str,
    pushed_heads: &BTreeMap<String, String>,
) -> Vec<String> {
    let folder = pom_layout::workspace_folder(project_root, branch);
    let mut reasons = Vec::new();
    for (name, dir) in &config.repos {
        let worktree = folder.join(name);
        if !worktree.is_dir() {
            continue;
        }
        match uncommitted(&worktree, &dir.copy) {
            Ok(0) => {}
            Ok(1) => reasons.push(format!("{name} has 1 uncommitted change")),
            Ok(count) => reasons.push(format!("{name} has {count} uncommitted changes")),
            Err(error) => reasons.push(format!("{name}: git status failed ({error})")),
        }
        let Some(pom_layout::Head::Branch(local)) = pom_layout::read_head(&worktree) else {
            continue;
        };
        let pushed = pushed_heads.get(name).is_some_and(|head| {
            git::git(&worktree, &["merge-base", "--is-ancestor", "HEAD", head]).is_ok()
        });
        if pushed {
            continue;
        }
        let main =
            pom_layout::repo_worktree(project_root, name, config.global_default_branch(), true);
        match git::unpushed_commits(&main, &local, config.default_branch_for(name)) {
            0 => {}
            1 => reasons.push(format!("{name} has 1 commit not pushed")),
            count => reasons.push(format!("{name} has {count} commits not pushed")),
        }
    }
    reasons
}

/// Changed tracked files plus untracked ones, leaving out what pom itself put there (copied files, generated
/// env files).
fn uncommitted(worktree: &Path, copied: &[String]) -> Result<usize, String> {
    let status = git::git(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    Ok(status
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| match line.strip_prefix("?? ") {
            Some(path) => !placed_by_pom(worktree, path, copied),
            None => true,
        })
        .count())
}

/// Untracked files pom put in a worktree itself: `copy:` files and generated env files.
pub fn placed_by_pom(worktree: &Path, path: &str, copied: &[String]) -> bool {
    copied.iter().any(|pattern| matches_copy(pattern, path)) || generated(&worktree.join(path))
}

fn matches_copy(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches("./");
    if !pattern.contains('*') {
        return pattern == path;
    }
    let (folder, name) = pattern.rsplit_once('/').unwrap_or(("", pattern));
    let (path_folder, file) = path.rsplit_once('/').unwrap_or(("", path));
    let (prefix, suffix) = name.split_once('*').unwrap_or((name, ""));
    folder == path_folder
        && file.starts_with(prefix)
        && file.ends_with(suffix)
        && file.len() >= prefix.len() + suffix.len()
}

fn generated(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut first = String::new();
    BufReader::new(file).read_line(&mut first).is_ok()
        && first.trim_end() == pom_services::GENERATED_ENV_HEADER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_files_match_their_pattern_only() {
        assert!(matches_copy(".env", ".env"));
        assert!(matches_copy("config/*.yml", "config/app.yml"));
        assert!(!matches_copy("config/*.yml", "config/deep/app.yml"));
        assert!(!matches_copy("config/*.yml", "app.yml"));
        assert!(!matches_copy(".env", "src/main.rs"));
    }
}
