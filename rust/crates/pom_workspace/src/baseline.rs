//! A workspace's baseline: fresh data in (main's baseline or one of its own snapshots), the branch's migrations
//! run, then `ws__baseline` saved again, so every test starts from the same rows.

use std::path::Path;
use std::time::Instant;

use pom_services::{DbOutcome, SnapshotReport, MAIN_BASELINE, WORKSPACE_BASELINE};
use serde::Serialize;

use crate::{run_shell, WorkspaceContext};

/// Where a reseed takes its data from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReseedSource {
    MainBaseline,
    Snapshot(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MigrationOutcome {
    pub repo: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BaselineReport {
    pub schema: &'static str,
    pub workspace: String,
    pub action: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub databases: Vec<DbOutcome>,
    pub migrations: Vec<MigrationOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<SnapshotReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub services_restarted: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub total_ms: u64,
}

impl BaselineReport {
    fn new(workspace: &str, action: &'static str) -> BaselineReport {
        BaselineReport {
            schema: "pom.db/v1",
            workspace: workspace.to_string(),
            action,
            ..BaselineReport::default()
        }
    }

    /// Every database copied and the baseline saved; a failed migration is reported but does not fail it.
    pub fn ok(&self) -> bool {
        self.databases.iter().all(|outcome| outcome.ok)
            && self.baseline.as_ref().is_none_or(SnapshotReport::ok)
    }
}

/// Runs each checked-out repo's migrations in the workspace, with its env.
pub fn migrate(
    context: &WorkspaceContext<'_>,
    branch: &str,
    folder: &Path,
) -> Vec<MigrationOutcome> {
    let config = context.config;
    let env = context.runner.workspace_env(config, branch);
    let mut outcomes = Vec::new();
    for (repo, dir) in &config.repos {
        let worktree = folder.join(repo);
        let steps = dir.effective_migrate();
        if !worktree.is_dir() || !dir.has_worktree_config() || steps.is_empty() {
            continue;
        }
        let result = run_shell(true, &steps.join(" && "), &worktree, &env.repo_env(repo));
        outcomes.push(MigrationOutcome {
            repo: repo.clone(),
            ok: result.is_ok(),
            error: result.err(),
        });
    }
    outcomes
}

fn owned(context: &WorkspaceContext<'_>, branch: &str, folder: &Path) -> Vec<String> {
    pom_services::owned_database_names(context.config, branch, |repo| folder.join(repo).is_dir())
}

/// Runs the branch's migrations, then saves `ws__baseline` again.
pub fn rebaseline(
    context: &WorkspaceContext<'_>,
    branch: &str,
    folder: &Path,
) -> Result<BaselineReport, String> {
    let started = Instant::now();
    let names = owned(context, branch, folder);
    if names.is_empty() {
        return Err(format!("workspace {branch} has no databases"));
    }
    let mut report = BaselineReport::new(branch, "baseline");
    report.migrations = migrate(context, branch, folder);
    report.baseline = Some(context.runner.snapshot_workspace(
        context.config,
        branch,
        folder,
        &names,
        WORKSPACE_BASELINE,
        true,
    )?);
    report.total_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// Replaces the workspace's data from `source`, runs the branch's migrations and saves `ws__baseline` again. The
/// workspace's running services are stopped for it and started again afterwards. Main is copied from its own
/// `main__baseline` and keeps that snapshot.
pub fn reseed(
    context: &WorkspaceContext<'_>,
    branch: &str,
    is_main: bool,
    folder: &Path,
    source: &ReseedSource,
) -> Result<BaselineReport, String> {
    let started = Instant::now();
    let config = context.config;
    let runner = context.runner;
    let names = owned(context, branch, folder);
    if names.is_empty() {
        return Err(format!("workspace {branch} has no databases"));
    }
    let mut report = BaselineReport::new(branch, "reseed");
    let main_folder =
        pom_layout::workspace_root(runner.project_root(), config.global_default_branch(), true);
    let copies: Vec<(String, String)> = match source {
        ReseedSource::Snapshot(name) => {
            report.source = name.clone();
            Vec::new()
        }
        ReseedSource::MainBaseline if is_main => {
            report.source = MAIN_BASELINE.to_string();
            Vec::new()
        }
        ReseedSource::MainBaseline => {
            report.source = MAIN_BASELINE.to_string();
            let pairs =
                pom_services::main_counterparts(config, branch, |repo| folder.join(repo).is_dir());
            let mut copies = Vec::new();
            for db in &names {
                let snapshot = pairs
                    .iter()
                    .find(|(workspace, _)| workspace == db)
                    .and_then(|(_, main)| {
                        pom_services::snapshot_copy(&main_folder, MAIN_BASELINE, main)
                    });
                match snapshot {
                    Some(snapshot) => copies.push((db.clone(), snapshot)),
                    None => {
                        return Err(format!(
                            "main__baseline has no copy for {db}: run `pom prepare-main` to take it"
                        ))
                    }
                }
            }
            copies
        }
    };
    let running = runner.stop_running_services(config, branch, is_main)?;
    match source {
        ReseedSource::Snapshot(name) => {
            let restored =
                runner.restore_workspace(config, branch, is_main, folder, &names, name, false)?;
            report.databases = restored.databases;
        }
        ReseedSource::MainBaseline if is_main => {
            let restored = runner.restore_workspace(
                config,
                branch,
                is_main,
                folder,
                &names,
                MAIN_BASELINE,
                false,
            )?;
            report.databases = restored.databases;
        }
        ReseedSource::MainBaseline => {
            for (db, snapshot) in &copies {
                let begin = Instant::now();
                let result = runner.clone_from_snapshot(config, snapshot, db);
                let failed = result.is_err();
                report.databases.push(DbOutcome {
                    db: db.clone(),
                    snapshot_db: snapshot.clone(),
                    ms: begin.elapsed().as_millis() as u64,
                    ok: !failed,
                    error: result.err(),
                    ..DbOutcome::default()
                });
                if failed {
                    break;
                }
            }
        }
    }
    if report.databases.iter().all(|outcome| outcome.ok) {
        report.migrations = migrate(context, branch, folder);
        if !is_main {
            report.baseline = Some(runner.snapshot_workspace(
                config,
                branch,
                folder,
                &names,
                WORKSPACE_BASELINE,
                true,
            )?);
        }
    }
    report.services_restarted = runner.start_services(config, &running, &mut report.warnings);
    report.total_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}
