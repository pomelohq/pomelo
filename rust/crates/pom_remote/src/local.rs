//! A project on this machine, read through the crates directly. The CLI prints the same builders as JSON, which
//! is what a remote machine answers with.

use std::path::{Path, PathBuf};

use pom_agent::{Caller, Drive, DriveError, Gate, Lease, LeaseClass, Limits, SessionView};
use pom_config::{Config, DepGraph};
use pom_core::Project;
use pom_layout::WorkspaceState;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};

use crate::server_api::{AgentClaim, AgentPresence};
use crate::source::{
    last_turn, AgentTurn, FailedRun, LastSync, ProjectEntry, ProjectSource, ServiceAction,
    SourceError, StatusReport, WorkspaceList, WorkspaceSummary, WORKSPACE_SCHEMA,
};

fn target(branch: &str, is_main: bool, repo: &str, service: &str) -> ServiceTarget {
    ServiceTarget {
        branch: branch.to_string(),
        is_main,
        repo: repo.to_string(),
        service: service.to_string(),
    }
}

pub fn local_projects(state: &StateDir) -> Vec<ProjectEntry> {
    pom_core::session_entries(state)
        .into_iter()
        .filter(|entry| entry.host.is_none())
        .map(|entry| ProjectEntry {
            config: entry.path.join(pom_config::CONFIG_FILE_NAME),
            name: entry.name,
            path: entry.path,
            missing: entry.missing,
            last_used: entry.last_used,
        })
        .collect()
}

/// Every workspace of `project` with its repos, display name, running services and any unfinished run.
pub fn workspace_list(
    project: &Project,
    config: &Config,
    runner: &ServiceRunner,
    state: &StateDir,
) -> WorkspaceList {
    let last_syncs = pom_push::last_syncs(state, &config.session);
    let proxy_requests = pom_workspace::proxy_requests(state);
    let context = pom_workspace::WorkspaceContext {
        config,
        runner,
        state,
    };
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
                        pom_workspace::Operation::Import => "import",
                        _ => "create",
                    }
                    .to_string(),
                    stage: failed.failed_stage + 1,
                });
            let workspace_state = WorkspaceState::load(&workspace.path);
            let request = proxy_requests
                .get(&(project.root.clone(), workspace.branch.clone()))
                .copied()
                .unwrap_or_default();
            let activity = pom_workspace::workspace_activity(&context, workspace, request);
            WorkspaceSummary {
                branch: workspace.branch.clone(),
                is_main: workspace.is_main,
                path: workspace.path.clone(),
                state: workspace_state.state.name().to_string(),
                name: workspace_state.display_name,
                repos: workspace
                    .repos
                    .iter()
                    .map(|repo| repo.name.clone())
                    .collect(),
                running,
                in_progress: pom_workspace::active_run(state, &workspace.branch),
                failed,
                remote_ahead: pom_push::remote_ahead(workspace),
                last_sync: last_syncs.get(&workspace.branch).map(|record| LastSync {
                    time: record.time.clone(),
                    outcome: record.outcome.name().to_string(),
                    repo: record.repo.clone(),
                    commits: record.commits,
                    restarted: record.restarted.clone(),
                    detail: record.detail.clone(),
                }),
                last_activity_at: activity.map(|activity| pom_push::utc_text(activity.at_ms)),
                last_activity_source: activity.map(|activity| activity.source.to_string()),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceTerminals {
    pub session: String,
    pub branch: String,
    pub worktree: PathBuf,
    pub group: pom_ptyhost::HolderGroup,
}

impl WorkspaceTerminals {
    pub fn shell_prefix(&self) -> String {
        format!("term-{}-{}", self.session, self.branch)
    }

    pub fn owns(&self, name: &str) -> bool {
        ["term", "svc", "ws"].iter().any(|kind| {
            name.strip_prefix(&format!("{kind}-{}-{}-", self.session, self.branch))
                .is_some_and(|rest| !rest.is_empty())
        })
    }
}

/// A project on this machine.
pub struct LocalSource {
    config_path: PathBuf,
    state: StateDir,
    holders: SocketDir,
    binary: PathBuf,
}

/// A project read from disk, with the runner of its services.
pub struct OpenedProject {
    pub project: Project,
    pub config: Config,
    pub runner: ServiceRunner,
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

    pub fn open_project(&self) -> Result<OpenedProject, SourceError> {
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
        Ok(OpenedProject {
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

    pub fn state(&self) -> &StateDir {
        &self.state
    }

    pub fn holders(&self) -> &SocketDir {
        &self.holders
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub fn workspace_terminals(&self, branch: &str) -> Result<WorkspaceTerminals, SourceError> {
        let opened = self.open_project()?;
        let worktree = opened
            .project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
            .map(|workspace| workspace.path.clone())
            .ok_or_else(|| SourceError::Failed(format!("no workspace {branch}")))?;
        Ok(WorkspaceTerminals {
            session: pom_env::branch_safe(&opened.config.session),
            branch: pom_env::branch_safe(branch),
            worktree,
            group: pom_services::holder_group(&opened.config, branch),
        })
    }

    pub fn service_holder(&self, branch: &str, service: &str) -> Result<String, SourceError> {
        let opened = self.open_project()?;
        let is_main = Self::is_main(&opened.project, branch)?;
        let (repo, name) = opened
            .config
            .find_service_entry(service)
            .map_err(SourceError::Failed)?;
        Ok(opened
            .runner
            .holder_name(&target(branch, is_main, &repo, &name)))
    }

    /// The machine's own processes act as an orchestrator; a remote person says so with `person`.
    fn gate<'a>(&'a self, config: &Config, branch: &str) -> Result<Gate<'a>, SourceError> {
        Gate::open(
            &self.state,
            Caller::Operator,
            pom_agent::Workspace {
                project: config.session.clone(),
                branch: branch.to_string(),
            },
            Limits::OPERATOR,
        )
        .map_err(|refusal| SourceError::Failed(refusal.to_string()))
    }

    fn agent_view(
        &self,
        drive: &Drive,
        branch: &str,
        holder: &str,
    ) -> Result<SessionView, SourceError> {
        drive
            .list(branch)
            .into_iter()
            .find(|view| view.holder == holder)
            .ok_or_else(|| SourceError::Failed(format!("no agent {holder} in {branch}")))
    }

    /// Who drives each session, and the approval it waits for.
    pub fn agent_presence(&self, sessions: &[SessionView]) -> Vec<AgentPresence> {
        sessions
            .iter()
            .map(|view| AgentPresence {
                holder: view.holder.clone(),
                driven_by: pom_agent::driven_by(&self.state, &view.holder),
                pending: (!view.session_id.is_empty())
                    .then(|| pom_agent::pending_approval(&self.state, &view.session_id))
                    .flatten(),
            })
            .collect()
    }

    /// Starts the workspace's main agent (`claude`) or a fresh one as `role`, the way `pom agent start` does.
    /// A person who starts one drives it, and answers its folder trust prompt in its terminal.
    pub fn start_agent(
        &self,
        branch: &str,
        role: &str,
        person: bool,
    ) -> Result<SessionView, SourceError> {
        let opened = self.open_project()?;
        let is_main = Self::is_main(&opened.project, branch)?;
        let drive = self.drive(&opened.config);
        let gate = self.gate(&opened.config, branch)?;
        let cwd = pom_layout::workspace_root(&opened.project.root, branch, is_main);
        let fresh = role != pom_agent::CLAUDE_DRIVER;
        let started = drive.start(
            &gate,
            &cwd,
            is_main,
            role,
            fresh,
            &pom_agent::LaunchOptions::default(),
        );
        let view = match started {
            Ok(view) => view,
            Err(DriveError::NeedsTrust(_)) if person => drive
                .list(branch)
                .into_iter()
                .find(|view| view.role == role)
                .ok_or_else(|| SourceError::Failed(format!("{role} did not start")))?,
            Err(error) => return Err(SourceError::Failed(error.to_string())),
        };
        if person {
            pom_agent::set_lease(
                &self.state,
                &self.holders,
                &opened.config.session,
                branch,
                &view.holder,
                &view.role,
                Lease::person(),
            )
            .map_err(|error| SourceError::Failed(format!("could not hand {role} over: {error}")))?;
        }
        Ok(view)
    }

    /// Takes the lease of `role` as an orchestrator, as `pom agent send --take` does.
    pub fn claim_agent(&self, branch: &str, role: &str) -> Result<AgentClaim, SourceError> {
        let opened = self.open_project()?;
        let drive = self.drive(&opened.config);
        let gate = self.gate(&opened.config, branch)?;
        let view = drive
            .resolve(&gate, role)
            .map_err(|error| SourceError::Failed(error.to_string()))?;
        let lease = drive
            .take_lease(&gate, &view)
            .map_err(|error| SourceError::Failed(error.to_string()))?;
        Ok(AgentClaim {
            holder: view.holder,
            token: lease.token,
        })
    }

    /// The person takes `holder` over; whoever drove it gets it back on release.
    pub fn take_over_agent(&self, branch: &str, holder: &str) -> Result<(), SourceError> {
        let opened = self.open_project()?;
        let view = self.agent_view(&self.drive(&opened.config), branch, holder)?;
        let before = pom_agent::read_lease(&self.state, holder);
        if before.class == LeaseClass::Human {
            return Ok(());
        }
        let mut person = Lease::person();
        person.previous = Some(Box::new(before));
        pom_agent::set_lease(
            &self.state,
            &self.holders,
            &opened.config.session,
            branch,
            holder,
            &view.role,
            person,
        )
        .map(|_| ())
        .map_err(|error| SourceError::Failed(error.to_string()))
    }

    /// The person answers the approval `holder`'s session waits for.
    pub fn answer_agent(
        &self,
        branch: &str,
        holder: &str,
        request: &str,
        allow: bool,
    ) -> Result<(), SourceError> {
        let opened = self.open_project()?;
        self.agent_view(&self.drive(&opened.config), branch, holder)?;
        let session = pom_agent::session_for_holder(&self.state, holder)
            .ok_or_else(|| SourceError::Failed(format!("{holder} has no session yet")))?;
        if pom_agent::pending_approval(&self.state, &session.session_id)
            .is_none_or(|pending| pending.request != request)
        {
            return Err(SourceError::Failed(format!(
                "{holder} does not wait for {request}"
            )));
        }
        pom_agent::answer_as_person(&self.state, &session, request, allow)
            .map_err(|error| SourceError::Failed(error.to_string()))
    }

    pub fn last_turn_document(
        &self,
        branch: &str,
        role: &str,
    ) -> Result<serde_json::Value, SourceError> {
        let opened = self.open_project()?;
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
        Ok(serde_json::json!({
            "schema": pom_agent::SCHEMA,
            "handle": view.handle,
            "session_id": view.session_id,
            "turns": turns,
        }))
    }
}

impl ProjectSource for LocalSource {
    fn location(&self) -> String {
        "this machine".to_string()
    }

    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        let opened = self.open_project()?;
        Ok(workspace_list(
            &opened.project,
            &opened.config,
            &opened.runner,
            &self.state,
        ))
    }

    fn status(&self, branch: &str) -> Result<StatusReport, SourceError> {
        let opened = self.open_project()?;
        let is_main = Self::is_main(&opened.project, branch)?;
        Ok(status_report(
            &opened.config,
            &opened.runner,
            branch,
            is_main,
        ))
    }

    fn logs(&self, branch: &str, service: &str) -> Result<String, SourceError> {
        let opened = self.open_project()?;
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
        let opened = self.open_project()?;
        Ok(self.drive(&opened.config).list(branch))
    }

    fn agent_turn(&self, branch: &str, role: &str) -> Result<Option<AgentTurn>, SourceError> {
        Ok(last_turn(&self.last_turn_document(branch, role)?))
    }

    fn act(
        &self,
        branch: &str,
        action: ServiceAction,
        target_name: &str,
    ) -> Result<String, SourceError> {
        let opened = self.open_project()?;
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
