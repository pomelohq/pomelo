use std::path::PathBuf;

use crate::{
    git, run_shell, EventSink, Operation, Outcome, PipelineError, Run, StageResult, StageScope,
    WorkspaceContext,
};

pub const DELETE_STAGES: [&str; 5] = [
    "Stopping services",
    "Releasing IP and slots",
    "Running pre-delete commands",
    "Removing worktrees and databases",
    "Cleaning up folders",
];

const STOP: usize = 0;
const RELEASE: usize = 1;
const CLEANUP: usize = 2;
const REMOVE: usize = 3;
const FINALIZE: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeleteRequest {
    pub branch: String,
    /// 0-based stage to start from when resuming a failed run.
    pub from_stage: usize,
}

struct CheckedOutRepo {
    name: String,
    main: PathBuf,
    worktree: PathBuf,
}

struct Deletion<'a> {
    context: &'a WorkspaceContext<'a>,
    branch: &'a str,
    folder: PathBuf,
    repos: Vec<CheckedOutRepo>,
}

pub fn delete(
    context: &WorkspaceContext<'_>,
    request: &DeleteRequest,
    sink: EventSink<'_>,
) -> Result<Outcome, PipelineError> {
    let root = context.runner.project_root();
    let folder = pom_layout::workspace_folder(root, &request.branch);
    let default_branch = context.config.global_default_branch();
    let repos = context
        .config
        .repos
        .keys()
        .map(|name| CheckedOutRepo {
            name: name.clone(),
            main: pom_layout::repo_worktree(root, name, default_branch, true),
            worktree: folder.join(name),
        })
        .filter(|repo| repo.worktree.exists())
        .collect();
    let deletion = Deletion {
        context,
        branch: &request.branch,
        folder: folder.clone(),
        repos,
    };
    let run = Run {
        operation: Operation::Delete,
        branch: &request.branch,
        workspace: &folder,
        labels: &DELETE_STAGES,
        from_stage: request.from_stage,
        state: context.state,
        sink,
    };
    run.execute(|index, scope| {
        if request.branch.trim().is_empty() || request.branch == default_branch {
            return Err(format!("{:?} is not a branch workspace", request.branch));
        }
        match index {
            STOP => deletion.stop(scope),
            RELEASE => deletion.release(),
            CLEANUP => deletion.cleanup(scope),
            REMOVE => deletion.remove(scope),
            FINALIZE => deletion.finalize(),
            _ => Ok(StageResult::Skipped),
        }
    })
}

impl Deletion<'_> {
    /// Holders of this workspace: its services, its agent and its terminals. A branch whose name
    /// extends this one (`feat` vs `feat-x`) shares the terminal prefix, so its holders are excluded.
    fn holder_matches(&self, name: &str, other_branches: &[String]) -> bool {
        let config = self.context.config;
        let session = pom_env::branch_safe(&config.session);
        let branch = pom_env::branch_safe(self.branch);
        let service_holder = config.repos.iter().any(|(repo, dir)| {
            dir.services
                .keys()
                .any(|service| name == format!("svc-{session}-{branch}-{repo}-{service}"))
        });
        let workspace_holder = config
            .workspace_services
            .keys()
            .map(String::as_str)
            .chain(["claude-raw"])
            .any(|service| name == format!("ws-{session}-{branch}-{service}"));
        if service_holder || workspace_holder {
            return true;
        }
        let terminal = |branch: &str| format!("term-{session}-{branch}-");
        let own = terminal(&branch);
        name.starts_with(&own)
            && !other_branches.iter().any(|other| {
                let other = terminal(&pom_env::branch_safe(other));
                other != own && other.starts_with(&own) && name.starts_with(&other)
            })
    }

    fn other_branches(&self) -> Vec<String> {
        let root = self.context.runner.project_root();
        std::fs::read_dir(root)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter_map(|name| {
                        name.strip_prefix(pom_layout::WORKSPACE_PREFIX)
                            .map(str::to_string)
                    })
                    .filter(|branch| branch != self.branch)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn stop(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let holders = self.context.runner.holders();
        let others = self.other_branches();
        let doomed: Vec<String> = holders
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| self.holder_matches(name, &others))
            .collect();
        for name in &doomed {
            match holders.kill_holder(name) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    scope.warn(format!("stop {name}: {error}"));
                }
                _ => scope.progress(format!("stopped {name}")),
            }
        }
        Ok(StageResult::Done)
    }

    fn release(&self) -> Result<StageResult, String> {
        self.context
            .runner
            .release_workspace(self.context.config, self.branch)
            .map_err(|error| format!("release slots: {error}"))?;
        Ok(StageResult::Done)
    }

    fn cleanup(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let config = self.context.config;
        let env = self.context.runner.workspace_env(config, self.branch);
        for repo in self.repos.iter().filter(|repo| repo.worktree.is_dir()) {
            let Some(dir) = config
                .repos
                .get(&repo.name)
                .filter(|dir| dir.has_worktree_config() && !dir.pre_delete.is_empty())
            else {
                continue;
            };
            scope.progress(format!("{}: pre-delete", repo.name));
            let repo_env = env.repo_env(&repo.name);
            if let Err(error) = run_shell(
                false,
                &dir.pre_delete.join(" && "),
                &repo.worktree,
                &repo_env,
            ) {
                scope.warn(format!("{}: pre-delete failed ({error})", repo.name));
            }
        }
        Ok(StageResult::Done)
    }

    fn remove(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let config = self.context.config;
        for repo in &self.repos {
            scope.progress(format!("worktree: {}", repo.name));
            let default = config.default_branch_for(&repo.name);
            match git::remove_worktree(&repo.main, &repo.worktree, self.branch, default) {
                Ok(true) => scope.warn(format!(
                    "{}: kept local branch {} - it has commits that are not pushed or merged",
                    repo.name, self.branch
                )),
                Ok(false) => {}
                Err(error) => scope.warn(format!("{}: remove worktree: {error}", repo.name)),
            }
        }
        if !config.shared_services.is_empty() {
            let names = self.databases();
            if !names.is_empty() {
                scope.progress(format!("dropping {}", names.join(", ")));
                if let Err(error) = self.context.runner.drop_databases(config, &names) {
                    scope.warn(format!("drop databases: {error}"));
                }
            }
        }
        Ok(StageResult::Done)
    }

    /// The workspace's own databases. A name that does not change with the branch is main's database
    /// too, and is never dropped.
    fn databases(&self) -> Vec<String> {
        let config = self.context.config;
        let default_branch = config.global_default_branch();
        let mut names = Vec::new();
        for repo in &self.repos {
            let Some(dir) = config
                .repos
                .get(&repo.name)
                .filter(|dir| dir.has_worktree_config())
            else {
                continue;
            };
            let shared = dir
                .shared_refs
                .iter()
                .filter(|shared| !shared.db_name.is_empty())
                .map(|shared| (shared.db_name.clone(), String::new()));
            let own = dir
                .databases
                .values()
                .map(|template| (template.clone(), format!("{}_", config.session)));
            for (template, prefix) in shared.chain(own) {
                let name = |branch: &str| {
                    format!(
                        "{prefix}{}",
                        pom_env::resolve_branch_tokens(&template, branch)
                    )
                };
                let (workspace, main) = (name(self.branch), name(default_branch));
                if workspace != main && !names.contains(&workspace) {
                    names.push(workspace);
                }
            }
        }
        names
    }

    fn finalize(&self) -> Result<StageResult, String> {
        match std::fs::remove_dir_all(&self.folder) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("remove {}: {error}", self.folder.display()));
            }
            _ => {}
        }
        let agent_state = self
            .context
            .state
            .path("agents")
            .join(format!("state-{}.json", self.branch.replace('/', "_")));
        if let Err(error) = std::fs::remove_file(agent_state) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("workspace: remove agent state: {error}");
            }
        }
        Ok(StageResult::Done)
    }
}
