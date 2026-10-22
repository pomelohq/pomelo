//! Syncing one repo of one workspace to what GitHub now has: fast-forward only, from a clean worktree, then
//! setup, migrate and restarts for what the change touched.

use std::path::Path;

use pom_config::{Config, OnPush};
use pom_layout::{Head, Workspace, WorkspaceState};
use pom_services::{ServiceRunner, ServiceTarget};
use pom_sync::git::{git, LOCAL, NETWORK};

use crate::journal::{Outcome, Record};

/// Files whose change means the repo's dependencies need installing again.
const SETUP_FILES: [&str; 26] = [
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "Gemfile",
    "Gemfile.lock",
    "go.mod",
    "go.sum",
    "Cargo.lock",
    "pyproject.toml",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "requirements.txt",
    "composer.json",
    "composer.lock",
    "mix.lock",
    ".nvmrc",
    ".node-version",
    ".ruby-version",
    ".python-version",
    ".tool-versions",
    "pom.yml",
];

const SCHEMA_FILES: [&str; 3] = ["schema.rb", "structure.sql", "schema.prisma"];

pub(crate) fn is_setup_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    SETUP_FILES.contains(&name)
}

pub(crate) fn is_migration(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    SCHEMA_FILES.contains(&name)
        || path
            .split('/')
            .rev()
            .skip(1)
            .any(|folder| folder.starts_with("migrat"))
}

/// Whether a change to `path` reaches a service that runs in `dir` (the repo root when empty).
pub(crate) fn touches(dir: &str, path: &str) -> bool {
    let dir = dir.trim_start_matches("./").trim_end_matches('/');
    dir.is_empty() || dir == "." || path == dir || path.starts_with(&format!("{dir}/"))
}

/// Changed tracked files and untracked ones, leaving out what pom put there itself.
fn uncommitted(worktree: &Path, copied: &[String]) -> Result<usize, String> {
    let status = git(
        worktree,
        &["status", "--porcelain", "--untracked-files=all"],
        LOCAL,
    )?;
    Ok(status
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| match line.strip_prefix("?? ") {
            Some(path) => !pom_workspace::placed_by_pom(worktree, path, copied),
            None => true,
        })
        .count())
}

fn count(worktree: &Path, range: &str) -> u64 {
    git(worktree, &["rev-list", "--count", range], LOCAL)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// Commits `refs/remotes/origin/<branch>` has that the worktrees of `workspace` do not, as of the last fetch.
pub fn remote_ahead(workspace: &Workspace) -> u64 {
    workspace
        .repos
        .iter()
        .filter_map(|repo| match pom_layout::read_head(&repo.path) {
            Some(Head::Branch(branch)) => Some((repo, branch)),
            _ => None,
        })
        .map(|(repo, branch)| count(&repo.path, &format!("HEAD..refs/remotes/origin/{branch}")))
        .sum()
}

pub(crate) struct Job<'a> {
    pub config: &'a Config,
    pub runner: &'a ServiceRunner,
    pub workspace: &'a Workspace,
    pub mode: OnPush,
}

impl Job<'_> {
    /// Brings `repo` of the workspace up to origin's `branch`, filling in `record`'s outcome.
    pub fn sync(&self, repo: &str, branch: &str, record: &mut Record) {
        record.workspace = self.workspace.branch.clone();
        record.repo = repo.to_string();
        record.branch = branch.to_string();
        if let Err(error) = self.sync_steps(repo, branch, record) {
            record.outcome = Outcome::Failed;
            record.detail = last_line(&error);
        }
    }

    fn sync_steps(&self, repo: &str, branch: &str, record: &mut Record) -> Result<(), String> {
        let worktree = self
            .workspace
            .repos
            .iter()
            .find(|entry| entry.name == repo)
            .map(|entry| entry.path.clone())
            .ok_or_else(|| format!("{repo} is not in workspace {}", self.workspace.branch))?;
        match pom_layout::read_head(&worktree) {
            Some(Head::Branch(name)) if name == branch => {}
            other => {
                return Err(format!(
                    "{repo} is on {} now, not {branch}",
                    other.as_ref().map_or("nothing", Head::name)
                ))
            }
        }
        git(&worktree, &["fetch", "origin", branch], NETWORK)?;
        let old = git(&worktree, &["rev-parse", "HEAD"], LOCAL)?;
        let new = git(&worktree, &["rev-parse", "FETCH_HEAD"], LOCAL)?;
        record.after = new.clone();
        let behind = count(&worktree, &format!("{old}..{new}"));
        let local = count(&worktree, &format!("{new}..{old}"));
        if behind == 0 {
            record.outcome = Outcome::UpToDate;
            return Ok(());
        }
        let copied = self
            .config
            .repos
            .get(repo)
            .map(|dir| dir.copy.clone())
            .unwrap_or_default();
        let dirty = uncommitted(&worktree, &copied)?;
        record.commits = behind;
        record.can_pull = dirty == 0 && local == 0;
        match self.mode {
            OnPush::Off => {
                record.outcome = Outcome::Off;
                return Ok(());
            }
            OnPush::Notify => {
                record.outcome = Outcome::Notified;
                return Ok(());
            }
            OnPush::Pull => {}
        }
        if dirty > 0 {
            record.outcome = Outcome::Dirty;
            record.detail = format!("{dirty} uncommitted changes; the worktree was left as it is");
            return Ok(());
        }
        if local > 0 {
            record.outcome = Outcome::Diverged;
            record.detail = format!(
                "{local} local commits GitHub does not have; the worktree was left as it is"
            );
            return Ok(());
        }
        git(&worktree, &["merge", "--ff-only", "FETCH_HEAD"], LOCAL)?;
        record.outcome = Outcome::Pulled;
        let changed: Vec<String> = git(&worktree, &["diff", "--name-only", &old, &new], LOCAL)?
            .lines()
            .map(str::to_string)
            .collect();
        if !WorkspaceState::load(&self.workspace.path)
            .state
            .is_running()
        {
            record.detail = "the workspace sleeps: nothing was set up or restarted".into();
            return Ok(());
        }
        self.after_pull(repo, &worktree, &changed, record);
        Ok(())
    }

    fn after_pull(&self, repo: &str, worktree: &Path, changed: &[String], record: &mut Record) {
        let Some(dir) = self.config.repos.get(repo) else {
            return;
        };
        let env = self
            .runner
            .workspace_env(self.config, &self.workspace.branch);
        let mut problems = Vec::new();
        let setup = dir.effective_setup();
        if !setup.is_empty() && changed.iter().any(|path| is_setup_file(path)) {
            record.setup = true;
            if let Err(error) =
                pom_workspace::run_shell(true, &setup.join(" && "), worktree, &env.repo_env(repo))
            {
                problems.push(format!("setup failed: {}", last_line(&error)));
            }
        }
        let migrate = dir.effective_migrate();
        if !migrate.is_empty() && changed.iter().any(|path| is_migration(path)) {
            record.migrated = true;
            if let Err(error) =
                pom_workspace::run_shell(true, &migrate.join(" && "), worktree, &env.repo_env(repo))
            {
                problems.push(format!("migrate failed: {}", last_line(&error)));
            }
        }
        for (name, service) in &dir.services {
            let target = ServiceTarget {
                branch: self.workspace.branch.clone(),
                is_main: self.workspace.is_main,
                repo: repo.to_string(),
                service: name.clone(),
            };
            let affected = record.setup || changed.iter().any(|path| touches(&service.dir, path));
            if !affected || !self.runner.is_running(&target) {
                continue;
            }
            match self.runner.restart(self.config, &target) {
                Ok(_) => record.restarted.push(format!("{repo}/{name}")),
                Err(error) => problems.push(format!("restart {repo}/{name}: {error}")),
            }
        }
        record.detail = problems.join("; ");
    }
}

fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockfiles_and_migrations_are_recognised() {
        assert!(is_setup_file("package-lock.json"));
        assert!(is_setup_file("web/yarn.lock"));
        assert!(!is_setup_file("src/lock.rs"));
        assert!(is_migration("db/migrate/20260101_add_users.rb"));
        assert!(is_migration("prisma/migrations/1/migration.sql"));
        assert!(is_migration("db/schema.rb"));
        assert!(!is_migration("src/migrate.rs"));
        assert!(!is_migration("app/models/user.rb"));
    }

    #[test]
    fn a_service_is_touched_by_changes_under_its_dir() {
        assert!(touches("", "anything.txt"));
        assert!(touches("web", "web/index.js"));
        assert!(touches("./web/", "web/index.js"));
        assert!(!touches("web", "worker/main.js"));
        assert!(!touches("web", "webhooks/x.js"));
    }
}
