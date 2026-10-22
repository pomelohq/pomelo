//! A project on this machine, read through the crates directly. The CLI prints the same builders as JSON, which
//! is what a remote machine answers with.

use std::path::{Path, PathBuf};

use pom_agent::{Drive, SessionView};
use pom_config::{Config, DepGraph};
use pom_core::Project;
use pom_layout::WorkspaceState;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};

use crate::source::{
    last_turn, AgentTurn, FailedRun, ProjectSource, ServiceAction, SourceError, StatusReport,
    WorkspaceList, WorkspaceSummary, WORKSPACE_SCHEMA,
};

fn target(branch: &str, is_main: bool, repo: &str, service: &str) -> ServiceTarget {
    ServiceTarget {
        branch: branch.to_string(),
        is_main,
        repo: repo.to_string(),
        service: service.to_string(),
    }
}

/// Every workspace of `project` with its repos, display name, running services and any unfinished run.
pub fn workspace_list(
    project: &Project,
    config: &Config,
    runner: &ServiceRunner,
    state: &StateDir,
) -> WorkspaceList {
    let workspaces = project
        .workspaces
        .iter()
        .map(|workspace| {
            let running = config
                .repos
                .iter()
                .flat_map(|(repo, dir)| dir.services.keys().map(move |service| (repo, service)))
                .filter(|(repo, service)| {
                    runner.is_running(&target(&workspace.branch, workspace.is_main, repo, service))
                })
                .count();
            let failed =
                pom_workspace::failed_run(state, &workspace.branch).map(|failed| FailedRun {
                    operation: match failed.operation {
                        pom_workspace::Operation::Delete => "delete",
                        _ => "create",
                    }
                    .to_string(),
                    stage: failed.failed_stage + 1,
                });
            WorkspaceSummary {
                branch: workspace.branch.clone(),
                is_main: workspace.is_main,
                path: workspace.path.clone(),
                name: WorkspaceState::load(&workspace.path).display_name,
                repos: workspace
                    .repos
                    .iter()
                    .map(|repo| repo.name.clone())
                    .collect(),
                running,
                in_progress: pom_workspace::active_run(state, &workspace.branch),
                failed,
            }
        })
        .collect();
    WorkspaceList {
        schema: WORKSPACE_SCHEMA.to_string(),
        project: config.session.clone(),
        root: project.root.clone(),
        workspaces,
    }
}

/// Every repo service of one workspace (up, ready, healthy) and the shared services.
pub fn status_report(
    config: &Config,
    runner: &ServiceRunner,
    branch: &str,
    is_main: bool,
) -> StatusReport {
    let mut services = Vec::new();
    for (repo, dir) in &config.repos {
        for name in dir.services.keys() {
            services.push(runner.service_health(config, &target(branch, is_main, repo, name)));
        }
    }
    StatusReport {
        workspace: branch.to_string(),
        services,
        shared: runner.shared_health(config),
    }
}

/// A running service's scrollback, or what a crashed one left.
pub fn service_output(
    runner: &ServiceRunner,
    target: &ServiceTarget,
    label: &str,
) -> Result<Vec<u8>, String> {
    let holder = runner.holder_name(target);
    let holders = runner.holders();
    if holders.holder_alive(&holder) {
        return pom_ptyhost::snapshot(holders, &holder, std::time::Duration::from_secs(5))
            .map_err(|error| error.to_string());
    }
    match holders.crash_info(&holder) {
        Some(info) => Ok(info.output),
        None => Err(format!("{label} is not running")),
    }
}

/// `(repo, service)` pairs of `target`, each repo's in dependency order (reversed for stopping).
pub fn ordered_services(
    config: &Config,
    target: &str,
    stopping: bool,
) -> Result<Vec<(String, String)>, String> {
    let pairs = config.resolve_services(target)?;
    let mut repos: Vec<(String, Vec<String>)> = Vec::new();
    for (repo, service) in pairs {
        match repos.iter_mut().find(|(name, _)| *name == repo) {
            Some((_, services)) => services.push(service),
            None => repos.push((repo, vec![service])),
        }
    }
    let mut ordered = Vec::new();
    for (repo, services) in repos {
        let services = match config.repos.get(&repo) {
            Some(dir) => {
                let graph = DepGraph::build(dir);
                let order = if stopping {
                    graph.stop_order(&services)
                } else {
                    graph.start_order(&services)
                };
                order.map_err(|error| format!("{repo}: {error}"))?
            }
            None => services,
        };
        ordered.extend(services.into_iter().map(|service| (repo.clone(), service)));
    }
    Ok(ordered)
}

/// A project on this machine.
pub struct LocalSource {
    config_path: PathBuf,
    state: StateDir,
    holders: SocketDir,
    binary: PathBuf,
}

struct Opened {
    project: Project,
    config: Config,
    runner: ServiceRunner,
}

impl LocalSource {
    pub fn new(
        config_path: PathBuf,
        state: StateDir,
        holders: SocketDir,
        binary: PathBuf,
    ) -> LocalSource {
        LocalSource {
            config_path,
            state,
            holders,
            binary,
        }
    }

    fn open(&self) -> Result<Opened, SourceError> {
        let project = Project::open(&self.config_path, &self.state);
        let config = match (&project.config, &project.error) {
            (Some(config), None) => config.clone(),
            (_, Some(problem)) => return Err(SourceError::Failed(problem.message.clone())),
            (None, None) => return Err(SourceError::Failed("pom.yml could not be loaded".into())),
        };
        let runner = ServiceRunner::new(RunnerOptions {
            project_root: project.root.clone(),
            session: config.session.clone(),
            state: self.state.clone(),
            holders: self.holders.clone(),
            binary: self.binary.clone(),
            docker: "docker".into(),
        });
        Ok(Opened {
            project,
            config,
            runner,
        })
    }

    fn is_main(project: &Project, branch: &str) -> Result<bool, SourceError> {
        project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
            .map(|workspace| workspace.is_main)
            .ok_or_else(|| SourceError::Failed(format!("no workspace {branch}")))
    }

    fn drive(&self, config: &pom_config::Config) -> Drive {
        Drive {
            limits: config.limits.clone(),
            state: self.state.clone(),
            holders: self.holders.clone(),
            project: config.session.clone(),
            home: std::env::var_os("HOME").map(Into::into).unwrap_or_default(),
            binary: self.binary.clone(),
            tool_path: pom_services::tool_path().to_string(),
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }
}

impl ProjectSource for LocalSource {
    fn location(&self) -> String {
        "this machine".to_string()
    }

    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        let opened = self.open()?;
        Ok(workspace_list(
            &opened.project,
            &opened.config,
            &opened.runner,
            &self.state,
        ))
    }

    fn status(&self, branch: &str) -> Result<StatusReport, SourceError> {
        let opened = self.open()?;
        let is_main = Self::is_main(&opened.project, branch)?;
        Ok(status_report(
            &opened.config,
            &opened.runner,
            branch,
            is_main,
        ))
    }

    fn logs(&self, branch: &str, service: &str) -> Result<String, SourceError> {
        let opened = self.open()?;
        let is_main = Self::is_main(&opened.project, branch)?;
        let (repo, name) = opened
            .config
            .find_service_entry(service)
            .map_err(SourceError::Failed)?;
        let bytes = service_output(
            &opened.runner,
            &target(branch, is_main, &repo, &name),
            service,
        )
        .map_err(SourceError::Failed)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn agents(&self, branch: &str) -> Result<Vec<SessionView>, SourceError> {
        let opened = self.open()?;
        Ok(self.drive(&opened.config).list(branch))
    }

    fn agent_turn(&self, branch: &str, role: &str) -> Result<Option<AgentTurn>, SourceError> {
        let opened = self.open()?;
        let drive = self.drive(&opened.config);
        let Some(view) = drive
            .list(branch)
            .into_iter()
            .find(|view| view.role == role)
        else {
            return Err(SourceError::Failed(format!("no agent {branch}/{role}")));
        };
        let last = view.turn;
        let turns = drive
            .read(&view, |turn| turn == last, false)
            .map_err(|error| SourceError::Failed(error.to_string()))?;
        Ok(last_turn(&serde_json::json!({ "turns": turns })))
    }

    fn act(
        &self,
        branch: &str,
        action: ServiceAction,
        target_name: &str,
    ) -> Result<String, SourceError> {
        let opened = self.open()?;
        let is_main = Self::is_main(&opened.project, branch)?;
        let mut said = Vec::new();
        if action != ServiceAction::Start {
            for (repo, service) in
                ordered_services(&opened.config, target_name, true).map_err(SourceError::Failed)?
            {
                let target = target(branch, is_main, &repo, &service);
                if opened.runner.is_running(&target) {
                    opened.runner.stop(&target).map_err(|error| {
                        SourceError::Failed(format!("{repo}/{service}: {error}"))
                    })?;
                    said.push(format!("stopped {repo}/{service}"));
                }
            }
        }
        if action != ServiceAction::Stop {
            for (repo, service) in
                ordered_services(&opened.config, target_name, false).map_err(SourceError::Failed)?
            {
                let target = target(branch, is_main, &repo, &service);
                if opened.runner.is_running(&target) {
                    continue;
                }
                opened
                    .runner
                    .start(&opened.config, &target)
                    .map_err(|error| SourceError::Failed(format!("{repo}/{service}: {error}")))?;
                said.push(format!("started {repo}/{service}"));
            }
        }
        Ok(said.join("\n"))
    }
}
