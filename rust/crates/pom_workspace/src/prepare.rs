//! Refreshing main's data, the source every new workspace copies its databases from: drop and recreate
//! main's databases, run each repo's migrations, then seed.

use std::path::PathBuf;

use crate::{
    run_shell, EventSink, Operation, Outcome, PipelineError, Run, StageResult, StageScope,
    WorkspaceContext,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrepareRequest {
    pub skip_seed: bool,
}

fn phase_label(phase: &str) -> &'static str {
    match phase {
        "reset" => "Resetting main's databases",
        "migrate" => "Running migrations",
        _ => "Seeding databases",
    }
}

struct Preparation<'a> {
    context: &'a WorkspaceContext<'a>,
    branch: &'a str,
    folder: PathBuf,
    /// Repos checked out in main, in config order.
    repos: Vec<(String, PathBuf)>,
}

pub fn prepare_main(
    context: &WorkspaceContext<'_>,
    request: &PrepareRequest,
    sink: EventSink<'_>,
) -> Result<Outcome, PipelineError> {
    let root = context.runner.project_root();
    let branch = context.config.global_default_branch();
    let folder = pom_layout::workspace_root(root, branch, true);
    let repos = context
        .config
        .repos
        .keys()
        .map(|name| {
            (
                name.clone(),
                pom_layout::repo_worktree(root, name, branch, true),
            )
        })
        .filter(|(_, path)| path.is_dir())
        .collect();
    let phases = context.config.prepare_main_phases();
    let labels: Vec<&'static str> = phases.iter().map(|phase| phase_label(phase)).collect();
    let preparation = Preparation {
        context,
        branch,
        folder: folder.clone(),
        repos,
    };
    let run = Run {
        operation: Operation::PrepareMain,
        branch,
        workspace: &folder,
        labels: &labels,
        from_stage: 0,
        state: context.state,
        sink,
        resumable: false,
    };
    run.execute(|index, scope| {
        if index == 0 {
            preparation.write_env()?;
        }
        match phases[index].as_str() {
            "reset" => preparation.reset(scope),
            "migrate" => Ok(preparation.migrate(scope)),
            _ if request.skip_seed => Ok(StageResult::Skipped),
            _ => Ok(preparation.seed(scope)),
        }
    })
}

impl Preparation<'_> {
    fn write_env(&self) -> Result<(), String> {
        let runner = self.context.runner;
        let ws_key = pom_env::port_ws_key(self.branch);
        runner
            .allocate_slots(self.context.config, &ws_key)
            .map_err(|error| format!("shared-service slots: {error}"))?;
        runner.acquire_workspace_ports(self.context.config, &ws_key);
        runner
            .workspace_env(self.context.config, self.branch)
            .write_env_files()
            .map_err(|error| format!("write env files: {error}"))
    }

    fn databases(&self) -> Vec<String> {
        let config = self.context.config;
        let mut names = Vec::new();
        for (repo, _) in &self.repos {
            let Some(dir) = config
                .repos
                .get(repo)
                .filter(|dir| dir.has_worktree_config() && !dir.databases.is_empty())
            else {
                continue;
            };
            let shared = dir
                .shared_refs
                .iter()
                .filter(|shared| !shared.db_name.is_empty())
                .map(|shared| pom_env::resolve_branch_tokens(&shared.db_name, self.branch));
            let own = dir.databases.values().map(|template| {
                format!(
                    "{}_{}",
                    config.session,
                    pom_env::resolve_branch_tokens(template, self.branch)
                )
            });
            for name in shared.chain(own) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    fn reset(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let config = self.context.config;
        let names = self.databases();
        if config.shared_services.is_empty() || names.is_empty() {
            return Ok(StageResult::Skipped);
        }
        let runner = self.context.runner;
        runner
            .ensure_shared(config)
            .map_err(|error| format!("shared services: {error}"))?;
        scope.progress(format!("dropping {}", names.join(", ")));
        runner
            .drop_databases(config, &names)
            .map_err(|error| error.to_string())?;
        scope.progress(format!("creating {}", names.join(", ")));
        runner
            .create_databases_when_ready(config, &names)
            .map_err(|error| error.to_string())?;
        Ok(StageResult::Done)
    }

    /// Runs `commands(repo)` in each repo of main that has any; a failure is a warning.
    fn each_repo(
        &self,
        scope: &StageScope<'_>,
        verb: &str,
        commands: impl Fn(&pom_config::Dir) -> Vec<String>,
    ) {
        let config = self.context.config;
        let env = self.context.runner.workspace_env(config, self.branch);
        for (repo, path) in &self.repos {
            let Some(steps) = config
                .repos
                .get(repo)
                .map(&commands)
                .filter(|steps| !steps.is_empty())
            else {
                continue;
            };
            scope.progress(format!("{repo}: {verb}"));
            if let Err(error) = run_shell(true, &steps.join(" && "), path, &env.repo_env(repo)) {
                scope.warn(format!("{repo}: {verb} failed ({error})"));
            }
        }
    }

    fn migrate(&self, scope: &StageScope<'_>) -> StageResult {
        self.each_repo(scope, "migrate", |dir| dir.effective_migrate());
        StageResult::Done
    }

    fn seed(&self, scope: &StageScope<'_>) -> StageResult {
        let seed = &self.context.config.seed;
        if !seed.is_empty() {
            scope.progress("workspace: seed");
            if let Err(error) = run_shell(true, &seed.join(" && "), &self.folder, &[]) {
                scope.warn(format!("workspace: seed failed ({error})"));
            }
        }
        self.each_repo(scope, "seed", |dir| dir.seed.clone());
        StageResult::Done
    }
}
