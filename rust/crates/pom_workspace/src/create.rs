use std::path::PathBuf;
use std::sync::Mutex;

use pom_layout::WorkspaceState;

use crate::{
    git, node_modules, repo_alias, run_shell, EventSink, Operation, Outcome, PipelineError, Run,
    StageResult, StageScope, WorkspaceContext,
};

pub const CREATE_STAGES: [&str; 7] = [
    "Validating config and hosts",
    "Provisioning workspace",
    "Starting shared services and databases",
    "Creating git worktrees (parallel)",
    "Configuring repos (parallel)",
    "Running setup commands (parallel)",
    "Seeding databases (parallel)",
];

const VALIDATE: usize = 0;
const PROVISION: usize = 1;
const INFRA: usize = 2;
const SOURCE: usize = 3;
const CONFIGURE: usize = 4;
const SETUP: usize = 5;
const SEED: usize = 6;

const PNPM_LOCK: &str = "pnpm-lock.yaml";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreateRequest {
    pub branch: String,
    /// Repos to check out; empty means every repo whose main checkout exists.
    pub repos: Vec<String>,
    /// Environment profile for every repo of the workspace; empty keeps `local`.
    pub environment: String,
    pub skip_seed: bool,
    /// 0-based stage to start from when resuming a failed run.
    pub from_stage: usize,
}

struct RepoPlan {
    name: String,
    main: PathBuf,
    worktree: PathBuf,
    base: String,
}

struct Creation<'a> {
    context: &'a WorkspaceContext<'a>,
    request: &'a CreateRequest,
    folder: PathBuf,
    default_branch: &'a str,
    repos: Vec<RepoPlan>,
}

pub fn create(
    context: &WorkspaceContext<'_>,
    request: &CreateRequest,
    sink: EventSink<'_>,
) -> Result<Outcome, PipelineError> {
    let project_root = context.runner.project_root();
    let folder = pom_layout::workspace_folder(project_root, &request.branch);
    let run = Run {
        operation: Operation::Create,
        branch: &request.branch,
        workspace: &folder,
        labels: &CREATE_STAGES,
        from_stage: request.from_stage,
        state: context.state,
        sink,
        resumable: true,
    };
    let default_branch = context.config.global_default_branch();
    let planned = plan_repos(context, request, default_branch);
    let creation = planned.as_ref().ok().map(|repos| Creation {
        context,
        request,
        folder: folder.clone(),
        default_branch,
        repos: repos
            .iter()
            .map(|(name, main)| RepoPlan {
                name: name.clone(),
                worktree: folder.join(name),
                base: git::current_branch(main),
                main: main.clone(),
            })
            .collect(),
    });
    run.execute(|index, scope| {
        let creation = match (&creation, &planned) {
            (Some(creation), _) => creation,
            (None, Err(error)) => return Err(error.clone()),
            (None, Ok(_)) => return Err("no repos".into()),
        };
        match index {
            VALIDATE => creation.validate(),
            PROVISION => creation.provision(),
            INFRA => creation.infra(scope),
            SOURCE => creation.source(scope),
            CONFIGURE => creation.configure(),
            SETUP => creation.setup(scope),
            SEED => creation.seed(scope),
            _ => Ok(StageResult::Skipped),
        }
    })
}

/// The selected repos with their main checkouts.
fn plan_repos(
    context: &WorkspaceContext<'_>,
    request: &CreateRequest,
    default_branch: &str,
) -> Result<Vec<(String, PathBuf)>, String> {
    let root = context.runner.project_root();
    let main_checkout = |repo: &str| pom_layout::repo_worktree(root, repo, default_branch, true);
    if request.repos.is_empty() {
        let repos: Vec<(String, PathBuf)> = context
            .config
            .repos
            .keys()
            .map(|repo| (repo.clone(), main_checkout(repo)))
            .filter(|(_, main)| pom_layout::is_git_repo(main))
            .collect();
        if repos.is_empty() {
            return Err("no repo of the project is checked out on the main branch".into());
        }
        return Ok(repos);
    }
    request
        .repos
        .iter()
        .map(|repo| {
            if !context.config.repos.contains_key(repo) {
                return Err(format!("unknown repo: {repo}"));
            }
            let main = main_checkout(repo);
            if pom_layout::is_git_repo(&main) {
                Ok((repo.clone(), main))
            } else {
                Err(format!("{repo} is not checked out at {}", main.display()))
            }
        })
        .collect()
}

pub(crate) fn validate_branch_name(branch: &str) -> Result<(), String> {
    if branch.trim().is_empty() {
        return Err("the workspace needs a branch name".into());
    }
    if branch.contains("..")
        || branch.starts_with(['/', '~', '-', '.'])
        || branch.ends_with(['/', '.'])
        || branch.ends_with(".lock")
        || branch.contains("//")
        || branch
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
    {
        return Err(format!("{branch:?} is not a valid branch name"));
    }
    Ok(())
}

impl Creation<'_> {
    fn branch(&self) -> &str {
        &self.request.branch
    }

    fn ws_key(&self) -> String {
        pom_env::port_ws_key(self.branch())
    }

    fn validate(&self) -> Result<StageResult, String> {
        validate_branch_name(self.branch())?;
        if self.branch() == self.default_branch {
            return Err(format!(
                "{} is the main workspace; pick another branch",
                self.default_branch
            ));
        }
        if let Some(repo) = self.repos.iter().find(|repo| repo.worktree.exists()) {
            return Err(format!(
                "{} already exists; delete the workspace first or resume its failed run",
                repo.worktree.display()
            ));
        }
        Ok(StageResult::Done)
    }

    fn provision(&self) -> Result<StageResult, String> {
        let runner = self.context.runner;
        std::fs::create_dir_all(&self.folder)
            .map_err(|error| format!("create {}: {error}", self.folder.display()))?;
        runner
            .allocate_slots(self.context.config, &self.ws_key())
            .map_err(|error| format!("shared-service slots: {error}"))?;
        runner.acquire_workspace_ports(self.context.config, &self.ws_key());
        if !self.request.environment.is_empty() {
            let mut state = WorkspaceState::load(&self.folder);
            for repo in &self.repos {
                state.service_envs.insert(
                    repo_alias(&repo.name, self.context.config).to_string(),
                    self.request.environment.clone(),
                );
            }
            state
                .save(&self.folder)
                .map_err(|error| format!("save workspace state: {error}"))?;
        }
        Ok(StageResult::Done)
    }

    fn infra(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let config = self.context.config;
        if config.shared_services.is_empty() {
            return Ok(StageResult::Skipped);
        }
        let runner = self.context.runner;
        let names: Vec<&str> = config.shared_services.keys().map(String::as_str).collect();
        scope.progress(format!("starting: {}", names.join(", ")));
        runner
            .ensure_shared(config)
            .map_err(|error| format!("shared services: {error}"))?;
        let (fresh, clones) = self.databases();
        if fresh.is_empty() && clones.is_empty() {
            return Ok(StageResult::Done);
        }
        scope.progress("creating databases");
        runner
            .create_databases_when_ready(config, &fresh)
            .map_err(|error| error.to_string())?;
        for (target, template) in clones {
            if runner.database_exists(config, &template) {
                match runner.clone_database(config, &template, &target) {
                    Ok(()) => {
                        scope.progress(format!("{target} copied from {template}"));
                        continue;
                    }
                    Err(error) => scope.warn(format!(
                        "could not copy {template} into {target}, starting it empty: {error}"
                    )),
                }
            }
            runner
                .create_databases_when_ready(config, std::slice::from_ref(&target))
                .map_err(|error| error.to_string())?;
        }
        Ok(StageResult::Done)
    }

    /// Databases to create empty, and `(workspace, main)` pairs to copy for repos seeded from main.
    fn databases(&self) -> (Vec<String>, Vec<(String, String)>) {
        let config = self.context.config;
        let mut fresh = Vec::new();
        let mut clones = Vec::new();
        for repo in &self.repos {
            let Some(dir) = config.repos.get(&repo.name) else {
                continue;
            };
            if !dir.has_worktree_config() {
                continue;
            }
            let shared = dir
                .shared_refs
                .iter()
                .filter(|shared| !shared.db_name.is_empty())
                .map(|shared| (shared.db_name.as_str(), ""));
            let own = dir
                .databases
                .values()
                .map(|template| (template.as_str(), config.session.as_str()));
            for (template, session) in shared.chain(own) {
                let name = |branch: &str| {
                    let resolved = pom_env::resolve_branch_tokens(template, branch);
                    if session.is_empty() {
                        resolved
                    } else {
                        format!("{session}_{resolved}")
                    }
                };
                let (workspace, main) = (name(self.branch()), name(self.default_branch));
                if dir.seed_from_main && workspace != main {
                    clones.push((workspace, main));
                } else {
                    fresh.push(workspace);
                }
            }
        }
        (fresh, clones)
    }

    fn source(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let created: Mutex<Vec<&RepoPlan>> = Mutex::new(Vec::new());
        let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
        std::thread::scope(|threads| {
            for repo in &self.repos {
                let (created, errors) = (&created, &errors);
                threads.spawn(move || {
                    if pom_layout::is_git_repo(&repo.worktree) {
                        scope.progress(format!("{}: worktree already there", repo.name));
                        return;
                    }
                    scope.progress(format!("worktree: {}", repo.name));
                    let copy = self
                        .context
                        .config
                        .repos
                        .get(&repo.name)
                        .filter(|dir| dir.has_worktree_config())
                        .map_or(&[][..], |dir| dir.copy.as_slice());
                    let added = git::add_worktree(
                        &repo.main,
                        &repo.worktree,
                        self.branch(),
                        &repo.base,
                        copy,
                    );
                    match added {
                        Ok(()) => push(created, repo),
                        Err(error) => push(errors, format!("{}: {error}", repo.name)),
                    }
                });
            }
        });
        let errors = errors.into_inner().unwrap_or_default();
        let Some(first) = errors.into_iter().next() else {
            return Ok(StageResult::Done);
        };
        for repo in created.into_inner().unwrap_or_default() {
            let default = self.context.config.default_branch_for(&repo.name);
            if let Err(error) =
                git::remove_worktree(&repo.main, &repo.worktree, self.branch(), default)
            {
                scope.warn(format!("{}: roll back worktree: {error}", repo.name));
            }
        }
        Err(first)
    }

    fn configure(&self) -> Result<StageResult, String> {
        self.context
            .runner
            .workspace_env(self.context.config, self.branch())
            .write_env_files()
            .map_err(|error| format!("write env files: {error}"))?;
        Ok(StageResult::Done)
    }

    fn setup(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let config = self.context.config;
        let env = self.context.runner.workspace_env(config, self.branch());
        std::thread::scope(|threads| {
            for repo in self.repos.iter().filter(|repo| repo.worktree.is_dir()) {
                let repo_env = env.repo_env(&repo.name);
                threads.spawn(move || {
                    let from_store = !repo.worktree.join(PNPM_LOCK).exists()
                        && node_modules::restore(
                            self.context.state,
                            &repo.name,
                            &repo.worktree,
                            &repo.main,
                        );
                    if from_store {
                        scope
                            .progress(format!("{}: node_modules from the shared store", repo.name));
                    }
                    let Some(dir) = config
                        .repos
                        .get(&repo.name)
                        .filter(|dir| dir.has_worktree_config())
                    else {
                        return;
                    };
                    let steps = dir.effective_setup();
                    if steps.is_empty() {
                        return;
                    }
                    scope.progress(format!("{}: setup", repo.name));
                    match run_shell(true, &steps.join(" && "), &repo.worktree, &repo_env) {
                        Ok(()) => {
                            // A failed install may leave a partial node_modules; only a good one is shared.
                            if !from_store {
                                node_modules::snapshot(
                                    self.context.state,
                                    &repo.name,
                                    &repo.worktree,
                                );
                            }
                            scope.progress(format!("{}: setup done", repo.name));
                        }
                        Err(error) => scope.warn(format!("{}: setup failed ({error})", repo.name)),
                    }
                });
            }
        });
        Ok(StageResult::Done)
    }

    fn seed(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let config = self.context.config;
        if self.request.skip_seed {
            return Ok(StageResult::Skipped);
        }
        if !config.seed.is_empty() {
            scope.progress("workspace: seeding");
            if let Err(error) = run_shell(true, &config.seed.join(" && "), &self.folder, &[]) {
                scope.warn(format!("workspace: seed failed ({error})"));
            }
        }
        let env = self.context.runner.workspace_env(config, self.branch());
        std::thread::scope(|threads| {
            for repo in self.repos.iter().filter(|repo| repo.worktree.is_dir()) {
                let Some(dir) = config.repos.get(&repo.name) else {
                    continue;
                };
                if dir.seed.is_empty() || dir.seed_from_main {
                    continue;
                }
                let repo_env = env.repo_env(&repo.name);
                threads.spawn(move || {
                    scope.progress(format!("{}: seeding", repo.name));
                    match run_shell(true, &dir.seed.join(" && "), &repo.worktree, &repo_env) {
                        Ok(()) => scope.progress(format!("{}: seed done", repo.name)),
                        Err(error) => scope.warn(format!("{}: seed failed ({error})", repo.name)),
                    }
                });
            }
        });
        Ok(StageResult::Done)
    }
}

fn push<T>(list: &Mutex<Vec<T>>, item: T) {
    match list.lock() {
        Ok(mut list) => list.push(item),
        Err(poisoned) => poisoned.into_inner().push(item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_follow_git_rules() {
        for good in ["feat-x", "feat/login", "PROJ-101_fix", "v1.2"] {
            assert!(validate_branch_name(good).is_ok(), "{good}");
        }
        for bad in [
            "", " ", "a..b", "/x", "~x", "-x", "x/", "x.lock", "a b", "a:b", "a//b", "x.",
        ] {
            assert!(validate_branch_name(bad).is_err(), "{bad:?}");
        }
    }
}
