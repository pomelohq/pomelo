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

use crate::server_api::{
    service_status, AgentClaim, AgentPresence, CrashFact, ServiceFact, ServicesFacts, SharedFact,
};
use crate::source::{
    last_turn, AgentTurn, FailedRun, LastSync, ProjectEntry, ProjectSource, ServiceAction,
    ServiceVerb, SourceError, StatusReport, WorkspaceList, WorkspaceSummary, WORKSPACE_SCHEMA,
};

/// The repo of a workspace-level service, as the Services panel names it.
pub const WORKSPACE_REPO: &str = "_ws";
/// How many lines of a shared service's output are read when none is asked.
pub const SHARED_LOG_LINES: usize = 200;

/// Modes picked for services (`repo~service` to mode), kept for as long as whoever holds them runs: the app
/// for this machine, the server for its clients.
pub type SharedModes = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>;

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
    modes: SharedModes,
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
            modes: SharedModes::default(),
        }
    }

    /// Runs services in the modes `modes` holds, and keeps the ones picked through it there.
    pub fn with_modes(mut self, modes: SharedModes) -> LocalSource {
        self.modes = modes;
        self
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
        let modes = self
            .modes
            .lock()
            .map(|modes| modes.clone())
            .unwrap_or_default();
        for (key, mode) in modes {
            if let Some((repo, service)) = key.split_once('~') {
                if let Err(error) = runner.set_mode(&config, repo, service, &mode) {
                    eprintln!("mode {key}: {error}");
                }
            }
        }
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
            agents: config.agents.clone().unwrap_or_default(),
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

    /// The folder of workspace `branch` and whether it is the main one, read even while pom.yml is broken so
    /// that it can still be fixed from afar.
    pub fn workspace_folder(&self, branch: &str) -> Result<(PathBuf, bool), SourceError> {
        let project = Project::open(&self.config_path, &self.state);
        project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
            .map(|workspace| (workspace.path.clone(), workspace.is_main))
            .ok_or_else(|| SourceError::Failed(format!("no workspace {branch}")))
    }

    /// The repos of workspace `branch` as the Git panel lists them, with their `origin`; read even while
    /// pom.yml is broken.
    pub fn git_repos(&self, branch: &str) -> Result<Vec<git::access::RepoSource>, SourceError> {
        let project = Project::open(&self.config_path, &self.state);
        if !project
            .workspaces
            .iter()
            .any(|workspace| workspace.branch == branch)
        {
            return Err(SourceError::Failed(format!("no workspace {branch}")));
        }
        Ok(crate::repo_sources(&project, branch)
            .into_iter()
            .map(|repo| git::access::RepoSource {
                origin: git::working_copy::remote_url(&repo.root, "origin"),
                ..repo
            })
            .collect())
    }

    /// Where repo `repo`'s branches are listed from: its checkout in workspace `branch`, else main's.
    pub fn branch_checkout(&self, branch: &str, repo: &str) -> Result<PathBuf, SourceError> {
        let project = Project::open(&self.config_path, &self.state);
        let checkout = |workspace: &pom_layout::Workspace| {
            workspace
                .repos
                .iter()
                .find(|found| found.name == repo)
                .map(|found| found.path.clone())
        };
        let shown = project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
            .ok_or_else(|| SourceError::Failed(format!("no workspace {branch}")))?;
        let worktree = checkout(shown).unwrap_or_else(|| shown.path.join(repo));
        if pom_layout::is_git_repo(&worktree) {
            return Ok(worktree);
        }
        project
            .workspaces
            .iter()
            .find(|workspace| workspace.is_main)
            .and_then(checkout)
            .ok_or_else(|| {
                SourceError::Failed(format!("{repo} has no checkout to list branches from"))
            })
    }

    /// The holder of `service` (`repo/name`, `name`, or `_ws/name` for a workspace-level one).
    pub fn service_holder(&self, branch: &str, service: &str) -> Result<String, SourceError> {
        let opened = self.open_project()?;
        let is_main = Self::is_main(&opened.project, branch)?;
        let (repo, name) = Self::service_entry(&opened.config, service)?;
        Ok(opened
            .runner
            .holder_name(&target(branch, is_main, &repo, &name)))
    }

    fn service_entry(config: &Config, service: &str) -> Result<(String, String), SourceError> {
        if let Some(name) = service.strip_prefix("_ws/") {
            return config
                .workspace_services
                .get(name)
                .filter(|def| !def.cmd.is_empty())
                .map(|_| (WORKSPACE_REPO.to_string(), name.to_string()))
                .ok_or_else(|| SourceError::Failed(format!("service '{service}' not found")));
        }
        config
            .find_service_entry(service)
            .map_err(SourceError::Failed)
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
        let mut extra_args = Vec::new();
        if person && !fresh {
            match pom_agent::AgentCommand::parse(&host_setting("agent_command")) {
                pom_agent::AgentCommand::Claude { extra_args: words } => extra_args = words,
                pom_agent::AgentCommand::Cli { name, command } => {
                    return drive
                        .start_cli(&gate, &cwd, is_main, &name, &command)
                        .map_err(|error| SourceError::Failed(error.to_string()));
                }
            }
        }
        let started = drive.start(
            &gate,
            &cwd,
            is_main,
            role,
            fresh,
            &pom_agent::LaunchOptions {
                for_person: person,
                extra_args,
                ..pom_agent::LaunchOptions::default()
            },
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

/// A file's modification time in seconds since the epoch.
fn modified_secs(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs())
}

/// Every service of a workspace as the Services panel lists them: workspace-level ones first, then each
/// repo's in config order.
pub fn panel_targets(config: &Config, branch: &str, is_main: bool) -> Vec<ServiceTarget> {
    let workspace = config
        .workspace_services
        .iter()
        .filter(|(_, service)| !service.cmd.is_empty())
        .map(|(name, _)| target(branch, is_main, WORKSPACE_REPO, name));
    let repos = config.repos.iter().flat_map(|(repo, dir)| {
        dir.services
            .keys()
            .map(move |service| target(branch, is_main, repo, service))
    });
    workspace.chain(repos).collect()
}

/// The service's definition, workspace-level or a repo's.
fn service_def<'a>(config: &'a Config, target: &ServiceTarget) -> Option<&'a pom_config::Service> {
    if target.is_workspace_level() {
        return config.workspace_services.get(&target.service);
    }
    config
        .repos
        .get(&target.repo)?
        .services
        .get(&target.service)
}

impl LocalSource {
    fn workspace(
        &self,
        opened: &OpenedProject,
        branch: &str,
    ) -> Result<pom_layout::Workspace, SourceError> {
        opened
            .project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
            .cloned()
            .ok_or_else(|| SourceError::Failed(format!("no workspace {branch}")))
    }

    /// The service `repo`/`service` of `branch`, checked against the config.
    fn service_target(
        &self,
        opened: &OpenedProject,
        branch: &str,
        repo: &str,
        service: &str,
    ) -> Result<ServiceTarget, SourceError> {
        let is_main = Self::is_main(&opened.project, branch)?;
        let wanted = target(branch, is_main, repo, service);
        if service_def(&opened.config, &wanted).is_none() {
            return Err(SourceError::Failed(format!(
                "service '{repo}/{service}' not found"
            )));
        }
        Ok(wanted)
    }

    /// What the Services panel shows of workspace `branch`, read as the panel reads it here.
    pub fn services_facts(&self, branch: &str) -> Result<ServicesFacts, SourceError> {
        let opened = self.open_project()?;
        let workspace = self.workspace(&opened, branch)?;
        let config = &opened.config;
        let runner = &opened.runner;
        let holders = runner.holders();
        let text = std::fs::read_to_string(&self.config_path)
            .map_err(|error| SourceError::Failed(format!("reading pom.yml: {error}")))?;
        let targets = panel_targets(config, branch, workspace.is_main);
        let mut own = std::collections::HashSet::new();
        let services = targets
            .iter()
            .map(|target| {
                let holder = runner.holder_name(target);
                own.insert(holder.clone());
                let alive = holders.holder_alive(&holder);
                let crash = (!alive)
                    .then(|| holders.crash_info(&holder))
                    .flatten()
                    .filter(|info| info.crashed)
                    .map(|info| CrashFact {
                        line: pom_services::crash_line(&info.output),
                        exit: pom_services::crash_exit(&info.header),
                        at: modified_secs(&holders.crash_log(&holder)),
                    });
                let status = match (alive, &crash) {
                    (true, _) => service_status::RUNNING,
                    (false, Some(_)) => service_status::CRASHED,
                    (false, None) => service_status::STOPPED,
                };
                let mode = service_def(config, target)
                    .map(|service| runner.mode(&target.repo, &target.service, service))
                    .unwrap_or_default();
                ServiceFact {
                    repo: target.repo.clone(),
                    service: target.service.clone(),
                    status: status.to_string(),
                    port: runner.port(config, target),
                    mode,
                    env_profile: runner.env_profile(config, target),
                    url: runner.proxy_url(config, target),
                    pid: alive.then(|| holders.holder_pid(&holder)).flatten(),
                    started_at: alive
                        .then(|| modified_secs(&holders.pidfile(&holder)))
                        .flatten(),
                    crash,
                    holder,
                }
            })
            .collect();
        let running = runner.shared_running();
        let env = runner.workspace_env(config, branch);
        let shared = config
            .shared_services
            .iter()
            .map(|(name, def)| {
                let holder = runner.shared_holder(name);
                let alive = def.is_command() && holders.holder_alive(&holder);
                let folder = if def.is_command() {
                    runner
                        .shared_command_dir(config, def)
                        .strip_prefix(runner.project_root())
                        .ok()
                        .map(|relative| relative.to_string_lossy().into_owned())
                        .filter(|relative| !relative.is_empty())
                        .unwrap_or_else(|| "project folder".to_string())
                } else {
                    String::new()
                };
                SharedFact {
                    name: name.clone(),
                    running: running.contains(name),
                    host_port: runner.shared_host_port(name),
                    pid: alive.then(|| holders.holder_pid(&holder)).flatten(),
                    started_at: alive
                        .then(|| modified_secs(&holders.pidfile(&holder)))
                        .flatten(),
                    holder,
                    connection: env.shared_url(name),
                    folder,
                }
            })
            .collect();
        let others_running = runner
            .running_holders()
            .into_iter()
            .filter(|holder| !own.contains(holder))
            .count();
        Ok(ServicesFacts {
            branch: branch.to_string(),
            root: workspace.path.clone(),
            config: text,
            services,
            shared,
            repos: workspace
                .repos
                .iter()
                .map(|repo| repo.name.clone())
                .collect(),
            others_running,
            public_urls: !pom_env::dev_network().public().is_local(),
        })
    }

    /// Runs `verb` on one service and waits for it, as the Services panel does here.
    pub fn run_one(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        verb: ServiceVerb,
    ) -> Result<(), SourceError> {
        let opened = self.open_project()?;
        let target = self.service_target(&opened, branch, repo, service)?;
        let runner = &opened.runner;
        let result = match verb {
            ServiceVerb::Start => runner.start(&opened.config, &target).map(|_| ()),
            ServiceVerb::Restart => runner.restart(&opened.config, &target).map(|_| ()),
            ServiceVerb::Relocate => runner.relocate(&opened.config, &target).map(|_| ()),
            ServiceVerb::Stop => runner.stop(&target).map_err(Into::into),
        };
        result.map_err(|error| SourceError::Failed(error.to_string()))
    }

    /// Runs `verb` on shared service `name`, or on all of them for `*`.
    pub fn run_shared_service(&self, name: &str, verb: ServiceVerb) -> Result<(), SourceError> {
        use pom_services::SharedAction;
        let opened = self.open_project()?;
        let config = &opened.config;
        let runner = &opened.runner;
        if name != "*" && !config.shared_services.contains_key(name) {
            return Err(SourceError::Failed(format!("no shared service {name}")));
        }
        let result = match (name, verb) {
            ("*", ServiceVerb::Stop) => runner.stop_shared(),
            ("*", _) => runner.ensure_shared(config),
            (name, ServiceVerb::Stop) => runner.shared_action(config, name, SharedAction::Stop),
            (name, ServiceVerb::Start) => runner.shared_action(config, name, SharedAction::Start),
            (name, _) => runner.shared_action(config, name, SharedAction::Restart),
        };
        result.map_err(|error| SourceError::Failed(error.to_string()))
    }

    /// Picks `mode` for the service: it applies from its next start.
    pub fn pick_mode(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        mode: &str,
    ) -> Result<(), SourceError> {
        let opened = self.open_project()?;
        let target = self.service_target(&opened, branch, repo, service)?;
        let known = service_def(&opened.config, &target)
            .is_some_and(|def| def.mode_names().iter().any(|name| name == mode));
        if !known {
            return Err(SourceError::Failed(format!(
                "{repo}/{service} has no mode {mode}"
            )));
        }
        opened
            .runner
            .set_mode(&opened.config, repo, service, mode)
            .map_err(|error| SourceError::Failed(error.to_string()))?;
        if let Ok(mut modes) = self.modes.lock() {
            modes.insert(format!("{repo}~{service}"), mode.to_string());
        }
        Ok(())
    }

    /// Picks env profile `profile` for the service: it applies from its next start.
    pub fn pick_env_profile(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        profile: &str,
    ) -> Result<(), SourceError> {
        let opened = self.open_project()?;
        let target = self.service_target(&opened, branch, repo, service)?;
        let known = opened.config.repos.get(repo).is_some_and(|dir| {
            dir.env_profiles(service_def(&opened.config, &target))
                .iter()
                .any(|name| name == profile)
        });
        if !known {
            return Err(SourceError::Failed(format!(
                "{repo}/{service} has no env profile {profile}"
            )));
        }
        opened
            .runner
            .set_env_profile(&opened.config, &target, profile)
            .map_err(|error| SourceError::Failed(error.to_string()))
    }

    pub fn shared_output(&self, name: &str, lines: usize) -> Result<String, SourceError> {
        let opened = self.open_project()?;
        let lines = if lines == 0 { SHARED_LOG_LINES } else { lines };
        opened
            .runner
            .shared_logs(&opened.config, name, lines)
            .map_err(SourceError::Failed)
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

    fn services_facts(&self, branch: &str) -> Result<ServicesFacts, SourceError> {
        LocalSource::services_facts(self, branch)
    }

    fn run_service(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        verb: ServiceVerb,
    ) -> Result<(), SourceError> {
        self.run_one(branch, repo, service, verb)
    }

    fn run_shared(&self, name: &str, verb: ServiceVerb) -> Result<(), SourceError> {
        self.run_shared_service(name, verb)
    }

    fn set_service_mode(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        mode: &str,
    ) -> Result<(), SourceError> {
        self.pick_mode(branch, repo, service, mode)
    }

    fn set_env_profile(
        &self,
        branch: &str,
        repo: &str,
        service: &str,
        profile: &str,
    ) -> Result<(), SourceError> {
        self.pick_env_profile(branch, repo, service, profile)
    }

    fn shared_logs(&self, name: &str, lines: usize) -> Result<String, SourceError> {
        self.shared_output(name, lines)
    }

    fn host_settings(&self) -> Result<crate::server_api::HostSettings, SourceError> {
        let path = settings_file()?;
        settings::read_host_settings(&path)
            .map(crate::server_api::HostSettings::new)
            .map_err(|error| SourceError::Failed(error.to_string()))
    }

    fn set_host_settings(
        &self,
        values: serde_json::Value,
    ) -> Result<crate::server_api::HostSettings, SourceError> {
        if pom_agent::Caller::current(&self.holders).is_agent() {
            return Err(SourceError::Failed(
                "only a person can change this machine's settings".into(),
            ));
        }
        let path = settings_file()?;
        settings::write_host_settings(&path, &values)
            .map(crate::server_api::HostSettings::new)
            .map_err(|error| SourceError::Failed(error.to_string()))
    }
}

/// A host-scoped setting of this machine's settings.json as text, its default when unset or unreadable.
pub fn host_setting(key: &str) -> String {
    let values = settings::Settings::path()
        .and_then(|path| settings::read_host_settings(&path).ok())
        .unwrap_or_else(|| settings::host_values(&serde_json::Value::Null));
    key.split('.')
        .try_fold(&values, |value, part| value.get(part))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn settings_file() -> Result<PathBuf, SourceError> {
    settings::Settings::path()
        .ok_or_else(|| SourceError::Failed("no home folder to keep settings.json in".into()))
}
