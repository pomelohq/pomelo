use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::{
    git, run_shell, EventSink, Operation, Outcome, PipelineError, Run, StageResult, StageScope,
    WorkspaceContext,
};

pub const DELETE_STAGES: [&str; 6] = [
    "Stopping services",
    "Releasing IP and slots",
    "Running pre-delete commands",
    "Removing worktrees and databases",
    "Cleaning up folders",
    "Cleaning up agent state and volumes",
];

const STOP: usize = 0;
const RELEASE: usize = 1;
const CLEANUP: usize = 2;
const REMOVE: usize = 3;
const FINALIZE: usize = 4;
const FORGET: usize = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeleteRequest {
    pub branch: String,
    /// 0-based stage to start from when resuming a failed run.
    pub from_stage: usize,
    /// Refuse, changing nothing, while a repo has uncommitted changes or commits no remote has.
    pub keep_unsaved_work: bool,
    /// Repo -> the commit its PR was last pushed at (see [`crate::unsaved_work`]).
    pub pushed_heads: BTreeMap<String, String>,
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
        resumable: true,
    };
    run.execute(|index, scope| {
        if request.branch.trim().is_empty() || request.branch == default_branch {
            return Err(format!("{:?} is not a branch workspace", request.branch));
        }
        if index == STOP && request.keep_unsaved_work {
            let unsaved =
                crate::unsaved_work(context.config, root, &request.branch, &request.pushed_heads);
            if !unsaved.is_empty() {
                return Err(format!("kept: {}", unsaved.join("; ")));
            }
        }
        match index {
            STOP => deletion.stop(scope),
            RELEASE => deletion.release(),
            CLEANUP => deletion.cleanup(scope),
            REMOVE => deletion.remove(scope),
            FINALIZE => deletion.finalize(),
            FORGET => deletion.forget(scope),
            _ => Ok(StageResult::Skipped),
        }
    })
}

impl Deletion<'_> {
    /// Holders of this workspace: its services, agents (any role, side agents included) and terminals. One
    /// started for a workspace records it, which settles it; an older one is matched by name, where a branch
    /// that extends this one (`feat` vs `feat-x`) shares the prefix, so its holders are excluded.
    fn holder_matches(
        &self,
        name: &str,
        group: Option<(String, String)>,
        other_branches: &[String],
    ) -> bool {
        let session = &self.context.config.session;
        if let Some((project, workspace)) = group {
            return project == *session && workspace == self.branch;
        }
        holder_of_branch(name, session, self.branch, other_branches)
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
            .filter(|name| self.holder_matches(name, holders.holder_group(name), &others))
            .collect();
        for name in &doomed {
            match holders.kill_holder(name) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    scope.warn(format!("stop {name}: {error}"));
                }
                _ => scope.progress(format!("stopped {name}")),
            }
        }
        let surviving: Vec<&String> = doomed
            .iter()
            .filter(|name| holders.holder_alive(name))
            .collect();
        if !surviving.is_empty() {
            return Err(format!(
                "still running after stop: {}",
                surviving
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
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
        let state = pom_layout::WorkspaceState::load(&self.folder);
        for repo in &self.repos {
            scope.progress(format!("worktree: {}", repo.name));
            let default = config.default_branch_for(&repo.name);
            // The branch the repo really uses, which may not be the workspace's; detached means none.
            let branch = match pom_layout::read_head(&repo.worktree) {
                Some(pom_layout::Head::Branch(branch)) => Some(branch),
                Some(pom_layout::Head::Detached(_)) => None,
                None => Some(state.repo_branch(&repo.name, self.branch).to_string()),
            };
            if let Err(error) = git::remove_worktree(&repo.main, &repo.worktree) {
                scope.warn(format!("{}: remove worktree: {error}", repo.name));
                continue;
            }
            let Some(branch) = branch else {
                continue;
            };
            match git::delete_branch_if_safe(&repo.main, &branch, default) {
                Ok(true) => scope.warn(kept_branch_note(&repo.name, &branch, &repo.main, default)),
                Ok(false) => {}
                Err(error) => scope.warn(format!("{}: delete branch {branch}: {error}", repo.name)),
            }
        }
        if !config.shared_services.is_empty() {
            if let Err(error) = self.context.runner.drop_all_snapshots(config, &self.folder) {
                scope.warn(format!("drop snapshots: {error}"));
            }
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
        Ok(StageResult::Done)
    }

    /// The agents' sessions, index, leases and log, and the Docker volumes labelled as this workspace's.
    fn forget(&self, scope: &StageScope<'_>) -> Result<StageResult, String> {
        let session = &self.context.config.session;
        for error in pom_agent::forget_workspace(
            self.context.state,
            session,
            self.branch,
            &self.other_branches(),
        ) {
            scope.warn(error);
        }
        let agent_state = self
            .context
            .state
            .path("agents")
            .join(format!("state-{}.json", self.branch.replace('/', "_")));
        if let Err(error) = std::fs::remove_file(agent_state) {
            if error.kind() != std::io::ErrorKind::NotFound {
                scope.warn(format!("remove agent state: {error}"));
            }
        }
        match self.context.runner.remove_workspace_volumes(self.branch) {
            Some(Ok(names)) if !names.is_empty() => {
                scope.progress(format!("removed volumes {}", names.join(", ")))
            }
            Some(Err(error)) => scope.warn(format!("remove volumes: {error}")),
            _ => {}
        }
        Ok(StageResult::Done)
    }
}

/// Whether holder `name` is one of workspace `branch` of project `session` by its name alone: a service
/// (`svc-`), an agent or workspace service (`ws-`) or a terminal (`term-`).
pub(crate) fn holder_of_branch(
    name: &str,
    session: &str,
    branch: &str,
    other_branches: &[String],
) -> bool {
    let session = pom_env::branch_safe(session);
    ["svc", "ws", "term"].iter().any(|kind| {
        let prefix = |branch: &str| format!("{kind}-{session}-{}-", pom_env::branch_safe(branch));
        let own = prefix(branch);
        name.starts_with(&own)
            && !other_branches.iter().any(|other| {
                let other = prefix(other);
                other != own && other.starts_with(&own) && name.starts_with(&other)
            })
    })
}

/// A holder still running for a workspace or project that no longer exists, so nothing in pom reaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanHolder {
    pub name: String,
    pub pid: i32,
    pub project: String,
    /// `None` when only its name ties it to the project and no workspace matches it.
    pub workspace: Option<String>,
    pub reason: String,
}

/// A registered project as orphan detection needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnownProject {
    pub session: String,
    /// Its folder and pom.yml are still there.
    pub present: bool,
    /// The main workspace's branch and every branch workspace's.
    pub branches: Vec<String>,
}

impl KnownProject {
    pub fn read(session: &str, root: &std::path::Path) -> KnownProject {
        let Ok(config) = pom_config::Config::load(&root.join(pom_config::CONFIG_FILE_NAME)) else {
            return KnownProject {
                session: session.to_string(),
                present: false,
                branches: Vec::new(),
            };
        };
        let mut branches = vec![config.global_default_branch().to_string()];
        if let Ok(entries) = std::fs::read_dir(root) {
            branches.extend(
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter_map(|name| {
                        name.strip_prefix(pom_layout::WORKSPACE_PREFIX)
                            .map(str::to_string)
                    }),
            );
        }
        branches.sort();
        branches.dedup();
        KnownProject {
            session: session.to_string(),
            present: true,
            branches,
        }
    }
}

/// Why holder `name` (started for `group`, when it recorded one) is an orphan of one of `projects`; `None`
/// when its workspace exists or it belongs to no registered project, which pom cannot judge.
pub fn orphan_reason(
    name: &str,
    group: Option<(String, String)>,
    projects: &[KnownProject],
) -> Option<(String, Option<String>, String)> {
    const PROJECT_GONE: &str = "its project's folder or pom.yml is gone";
    if let Some((project, workspace)) = group {
        let known = projects.iter().find(|known| known.session == project)?;
        if !known.present {
            return Some((project, Some(workspace), PROJECT_GONE.into()));
        }
        if known.branches.contains(&workspace) {
            return None;
        }
        return Some((
            project,
            Some(workspace),
            "its workspace no longer exists".into(),
        ));
    }
    let candidates: Vec<&KnownProject> = projects
        .iter()
        .filter(|known| {
            let session = pom_env::branch_safe(&known.session);
            ["svc", "ws", "term"]
                .iter()
                .any(|kind| name.starts_with(&format!("{kind}-{session}-")))
        })
        .collect();
    let claimed = candidates.iter().any(|known| {
        known.present
            && known
                .branches
                .iter()
                .any(|branch| holder_of_branch(name, &known.session, branch, &known.branches))
    });
    if claimed {
        return None;
    }
    let owner = candidates
        .into_iter()
        .max_by_key(|known| known.session.len())?;
    let reason = if owner.present {
        format!("no workspace of {} matches its name", owner.session)
    } else {
        PROJECT_GONE.into()
    };
    Some((owner.session.clone(), None, reason))
}

/// Every running holder whose workspace or project is gone, judged against the registered projects.
pub fn orphan_holders(
    state: &pom_paths::StateDir,
    holders: &pom_ptyhost::SocketDir,
) -> Vec<OrphanHolder> {
    let projects: Vec<KnownProject> = pom_sessions::Projects::load(state)
        .projects
        .iter()
        .map(|(session, root)| KnownProject::read(session, std::path::Path::new(root)))
        .collect();
    holders
        .holders()
        .into_iter()
        .filter_map(|(name, pid)| {
            let (project, workspace, reason) =
                orphan_reason(&name, holders.holder_group(&name), &projects)?;
            Some(OrphanHolder {
                name,
                pid,
                project,
                workspace,
                reason,
            })
        })
        .collect()
}

fn kept_branch_note(repo: &str, branch: &str, main: &std::path::Path, default: &str) -> String {
    match git::unpushed_commits(main, branch, default) {
        0 => format!(
            "{repo}: kept local branch {branch} - it has commits that are not pushed or merged"
        ),
        1 => {
            format!("{repo} has 1 commit not on origin; its branch {branch} stays on this machine")
        }
        count => format!(
            "{repo} has {count} commits not on origin; its branch {branch} stays on this machine"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_holder_named_for_the_workspace_is_its_own_and_none_of_a_longer_branch() {
        let others = ["feat/login-x".to_string(), "main".to_string()];
        let owned = |name: &str| holder_of_branch(name, "myproject", "feat/login", &others);
        for name in [
            "svc-myproject-feat_login-api-web",
            "ws-myproject-feat_login-claude-raw",
            "ws-myproject-feat_login-claude-raw-2",
            "ws-myproject-feat_login-fixer-1",
            "ws-myproject-feat_login-side-review-1",
            "ws-myproject-feat_login-side-codex-1",
            "ws-myproject-feat_login-claude-reviewer",
            "ws-myproject-feat_login-agent-codex",
            "ws-myproject-feat_login-gateway",
            "term-myproject-feat_login-3",
        ] {
            assert!(owned(name), "{name}");
        }
        for name in [
            "ws-myproject-feat_login-x-claude-raw",
            "svc-myproject-feat_login-x-api-web",
            "ws-other-feat_login-claude-raw",
            "ws-myproject-main-claude-raw",
            "shr-myproject-postgres",
        ] {
            assert!(!owned(name), "{name}");
        }
    }

    #[test]
    fn a_holder_outliving_its_workspace_or_project_is_an_orphan() {
        let known = |session: &str, present: bool, branches: &[&str]| KnownProject {
            session: session.into(),
            present,
            branches: branches.iter().map(|branch| branch.to_string()).collect(),
        };
        let projects = [
            known("myproject", true, &["feat/login", "main"]),
            known("my", true, &["main"]),
            known("my-project", true, &["main"]),
            known("gone", false, &[]),
        ];
        let group = |project: &str, workspace: &str| Some((project.into(), workspace.into()));
        let judged = |name: &str, group: Option<(String, String)>| {
            orphan_reason(name, group, &projects)
                .map(|(project, workspace, _)| (project, workspace))
        };
        assert_eq!(
            judged(
                "ws-myproject-feat_login-claude-raw",
                group("myproject", "feat/login")
            ),
            None
        );
        assert_eq!(
            judged(
                "ws-myproject-feat_old-fixer",
                group("myproject", "feat/old")
            ),
            Some(("myproject".into(), Some("feat/old".into())))
        );
        assert_eq!(
            judged("ws-myproject-feat_old-claude-reviewer", None),
            Some(("myproject".into(), None)),
            "unlabelled, matched by name"
        );
        assert_eq!(judged("svc-myproject-main-api-web", None), None);
        assert_eq!(
            judged("ws-my-project-main-claude-raw", None),
            None,
            "a project whose name extends another's claims its own holders"
        );
        assert_eq!(
            judged("ws-gone-main-claude-raw", group("gone", "main")),
            Some(("gone".into(), Some("main".into())))
        );
        assert_eq!(
            judged("ws-unknown-main-claude-raw", group("unknown", "main")),
            None,
            "a project this machine never registered is not judged"
        );
        assert_eq!(
            judged("ws-my-project-feat-claude-raw", None),
            Some(("my-project".into(), None))
        );
        assert_eq!(judged("shr-myproject-postgres", None), None);
    }
}
