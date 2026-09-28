//! Refresh-main: bring each repo of the main workspace to its default branch as origin has it, and migrate the
//! ones that moved, so new workspaces start from current code and data.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_services::ServiceRunner;

use crate::git::{self, current_branch, is_dirty, origin_default_branch, LOCAL, NETWORK};

const MIGRATE_TIMEOUT: Duration = Duration::from_secs(300);
const REFRESH_LOCK: &str = "refresh-main";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoState {
    Pending,
    Pulling,
    /// Left alone, and why (uncommitted work is never thrown away).
    Skipped(String),
    NoChange,
    Migrating,
    Updated,
    Failed(String),
}

pub struct RefreshContext<'a> {
    pub config: &'a Config,
    pub runner: &'a ServiceRunner,
    /// Where the session's lock files live (`/tmp/pom`, shared with the previous core).
    pub lock_dir: &'a Path,
}

/// Refreshes every repo checked out in main, reporting each repo's state as it changes. `Err` when another
/// refresh of the session is already running.
pub fn refresh_main(
    context: &RefreshContext<'_>,
    progress: &dyn Fn(&str, &RepoState),
) -> Result<Vec<(String, RepoState)>, String> {
    let session = context.runner.session();
    let _guard = pom_lock::try_acquire(context.lock_dir, session, REFRESH_LOCK)
        .map_err(|error| format!("refresh lock: {error}"))?
        .ok_or("main is already being refreshed")?;
    let config = context.config;
    let root = context.runner.project_root();
    let main_branch = config.global_default_branch();
    let repos: Vec<(String, std::path::PathBuf)> = config
        .repos
        .keys()
        .map(|repo| {
            (
                repo.clone(),
                pom_layout::repo_worktree(root, repo, main_branch, true),
            )
        })
        .filter(|(_, worktree)| pom_layout::is_git_repo(worktree))
        .collect();
    for (repo, _) in &repos {
        progress(repo, &RepoState::Pending);
    }
    let mut results = Vec::new();
    for (repo, worktree) in repos {
        let state = refresh_repo(context, &repo, &worktree, progress);
        progress(&repo, &state);
        results.push((repo, state));
    }
    Ok(results)
}

fn refresh_repo(
    context: &RefreshContext<'_>,
    repo: &str,
    worktree: &Path,
    progress: &dyn Fn(&str, &RepoState),
) -> RepoState {
    let default = origin_default_branch(worktree)
        .unwrap_or_else(|| context.config.default_branch_for(repo).to_string());
    progress(repo, &RepoState::Pulling);
    match is_dirty(worktree) {
        Ok(true) => return RepoState::Skipped("uncommitted changes".into()),
        Ok(false) => {}
        Err(error) => return RepoState::Failed(error),
    }
    let before = git::git(worktree, &["rev-parse", "HEAD"], LOCAL).unwrap_or_default();
    let steps: Result<(), String> = (|| {
        if current_branch(worktree).as_deref() != Some(default.as_str()) {
            git::git(worktree, &["checkout", &default], LOCAL)?;
        }
        git::git(worktree, &["fetch", "origin", &default], NETWORK)?;
        // Main mirrors origin: a rewritten upstream or a stray local commit resolves instead of failing ff-only.
        git::git(worktree, &["reset", "--hard", "FETCH_HEAD"], LOCAL)?;
        Ok(())
    })();
    if let Err(error) = steps {
        return RepoState::Failed(last_lines(&error, 3));
    }
    let after = git::git(worktree, &["rev-parse", "HEAD"], LOCAL).unwrap_or_default();
    if before == after {
        return RepoState::NoChange;
    }
    let Some(dir) = context.config.repos.get(repo) else {
        return RepoState::Updated;
    };
    let steps = dir.effective_migrate();
    if steps.is_empty() {
        return RepoState::Updated;
    }
    progress(repo, &RepoState::Migrating);
    match migrate(context, repo, worktree, &steps.join(" && ")) {
        Ok(()) => RepoState::Updated,
        Err(error) => RepoState::Failed(format!(
            "pulled, but migrate failed: {}",
            last_lines(&error, 3)
        )),
    }
}

fn migrate(
    context: &RefreshContext<'_>,
    repo: &str,
    worktree: &Path,
    command: &str,
) -> Result<(), String> {
    let branch = context.config.global_default_branch();
    let env = context.runner.workspace_env(context.config, branch);
    env.write_env_files()
        .map_err(|error| format!("write env files: {error}"))?;
    let mut child = Command::new("zsh")
        .args(["-lc", command])
        .current_dir(worktree)
        .env("PATH", pom_services::tool_path())
        .envs(env.repo_env(repo))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("zsh: {error}"))?;
    let deadline = Instant::now() + MIGRATE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut errors = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    if let Err(error) = std::io::Read::read_to_string(&mut stderr, &mut errors) {
                        eprintln!("sync: read migrate output: {error}");
                    }
                }
                return Err(format!("{status}: {}", errors.trim()));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    eprintln!("sync: stop migrate: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("sync: reap migrate: {error}");
                }
                return Err("timed out after 5 minutes".into());
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}
