//! Putting a workspace to sleep and waking it: its services and agents stop, and start again later, while its
//! worktrees, databases and leased ports stay as they are.

use std::path::Path;

use pom_layout::{RunState, SleepRecord, Workspace, WorkspaceState};
use pom_services::ServiceTarget;

use crate::WorkspaceContext;

/// One agent session of a workspace, as sleeping needs to see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentSession {
    pub role: String,
    pub holder: String,
    pub alive: bool,
    /// In a turn (thinking, a tool, compacting) or waiting on an approval.
    pub busy: bool,
    /// When it last reported anything (ms since the epoch).
    pub active_ms: u64,
}

/// The agent sessions of a workspace: listed to decide whether it may sleep, restarted when it wakes.
pub trait Agents {
    fn sessions(&self, branch: &str) -> Vec<AgentSession>;
    fn restart(&self, workspace: &Workspace, role: &str) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SleepError {
    /// These agents are working; `--force` puts the workspace to sleep anyway.
    AgentsBusy(Vec<String>),
    Failed(String),
}

impl std::fmt::Display for SleepError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SleepError::AgentsBusy(roles) => write!(
                formatter,
                "an agent is working ({}); wait for it, or sleep with --force",
                roles.join(", ")
            ),
            SleepError::Failed(message) => write!(formatter, "{message}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SleepReport {
    pub already: bool,
    pub services: Vec<String>,
    pub agents: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WakeReport {
    pub already: bool,
    pub started: Vec<String>,
    pub agents: Vec<String>,
    pub failed: Vec<String>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn service_label(target: &ServiceTarget) -> String {
    if target.is_workspace_level() {
        format!("/{}", target.service)
    } else {
        format!("{}/{}", target.repo, target.service)
    }
}

fn save(workspace: &Workspace, state: &WorkspaceState) -> Result<(), String> {
    state
        .save(&workspace.path)
        .map_err(|error| format!("save {}: {error}", workspace.path.display()))
}

/// The agents of `branch` that may not be stopped: alive and in a turn or waiting on an approval.
pub fn busy_agents(agents: &dyn Agents, branch: &str) -> Vec<String> {
    agents
        .sessions(branch)
        .into_iter()
        .filter(|session| session.alive && session.busy)
        .map(|session| session.role)
        .collect()
}

/// Stops the workspace's services and agent sessions and records them, so `wake` starts the same set.
pub fn sleep(
    context: &WorkspaceContext<'_>,
    agents: &dyn Agents,
    workspace: &Workspace,
    force: bool,
) -> Result<SleepReport, SleepError> {
    let mut state = WorkspaceState::load(&workspace.path);
    if state.state == RunState::Sleeping {
        return Ok(SleepReport {
            already: true,
            ..SleepReport::default()
        });
    }
    let busy = busy_agents(agents, &workspace.branch);
    if !busy.is_empty() && !force {
        return Err(SleepError::AgentsBusy(busy));
    }
    let running: Vec<ServiceTarget> = pom_services::ServiceRunner::service_targets(
        context.config,
        &workspace.branch,
        workspace.is_main,
    )
    .into_iter()
    .filter(|target| context.runner.is_running(target))
    .collect();
    let sessions: Vec<AgentSession> = agents
        .sessions(&workspace.branch)
        .into_iter()
        .filter(|session| session.alive)
        .collect();
    let report = SleepReport {
        already: false,
        services: running.iter().map(service_label).collect(),
        agents: sessions
            .iter()
            .map(|session| session.role.clone())
            .collect(),
    };
    // Recorded before anything stops, so the dev proxy offers to wake it rather than calling it stopped.
    state.state = RunState::Sleeping;
    state.asleep = Some(SleepRecord {
        since_ms: now_ms(),
        services: report.services.clone(),
        agents: report.agents.clone(),
    });
    save(workspace, &state).map_err(SleepError::Failed)?;
    let mut failures = Vec::new();
    for session in &sessions {
        if let Err(error) = context.runner.holders().kill_holder(&session.holder) {
            if error.kind() != std::io::ErrorKind::NotFound {
                failures.push(format!("{}: {error}", session.role));
            }
        }
    }
    for target in &running {
        if let Err(error) = context.runner.stop(target) {
            failures.push(format!("{}: {error}", service_label(target)));
        }
    }
    if !failures.is_empty() {
        return Err(SleepError::Failed(format!(
            "could not stop {}",
            failures.join("; ")
        )));
    }
    Ok(report)
}

/// Starts what ran when the workspace went to sleep, services in dependency order, then its agent sessions.
pub fn wake(
    context: &WorkspaceContext<'_>,
    agents: &dyn Agents,
    workspace: &Workspace,
) -> Result<WakeReport, String> {
    let mut state = WorkspaceState::load(&workspace.path);
    if state.state == RunState::Running {
        return Ok(WakeReport {
            already: true,
            ..WakeReport::default()
        });
    }
    let record = state.asleep.take().unwrap_or_default();
    state.state = RunState::Running;
    state.queued_ms = None;
    save(workspace, &state)?;
    let mut report = WakeReport::default();
    let targets = ordered_targets(context, workspace, &record.services);
    if targets.iter().any(|target| !target.is_workspace_level())
        && !context.config.shared_services.is_empty()
    {
        if let Err(error) = context.runner.ensure_shared(context.config) {
            report.failed.push(format!("shared services: {error}"));
        }
    }
    for target in targets {
        let label = service_label(&target);
        match context.runner.start(context.config, &target) {
            Ok(_) => report.started.push(label),
            Err(error) => report.failed.push(format!("{label}: {error}")),
        }
    }
    for role in &record.agents {
        match agents.restart(workspace, role) {
            Ok(()) => report.agents.push(role.clone()),
            Err(error) => report.failed.push(format!("agent {role}: {error}")),
        }
    }
    Ok(report)
}

/// The recorded services that the config still has, each repo's in the order its dependencies need.
fn ordered_targets(
    context: &WorkspaceContext<'_>,
    workspace: &Workspace,
    recorded: &[String],
) -> Vec<ServiceTarget> {
    let target = |repo: &str, service: &str| ServiceTarget {
        branch: workspace.branch.clone(),
        is_main: workspace.is_main,
        repo: repo.to_string(),
        service: service.to_string(),
    };
    let mut repos: Vec<(String, Vec<String>)> = Vec::new();
    let mut out = Vec::new();
    for label in recorded {
        let Some((repo, service)) = label.split_once('/') else {
            continue;
        };
        if repo.is_empty() {
            if context.config.workspace_services.contains_key(service) {
                out.push(target("", service));
            }
            continue;
        }
        let known = context
            .config
            .repos
            .get(repo)
            .is_some_and(|dir| dir.services.contains_key(service));
        if !known {
            continue;
        }
        match repos.iter_mut().find(|(name, _)| name == repo) {
            Some((_, services)) => services.push(service.to_string()),
            None => repos.push((repo.to_string(), vec![service.to_string()])),
        }
    }
    for (repo, services) in repos {
        let ordered = context
            .config
            .repos
            .get(&repo)
            .and_then(|dir| pom_config::DepGraph::build(dir).start_order(&services).ok())
            .unwrap_or(services);
        out.extend(ordered.iter().map(|service| target(&repo, service)));
    }
    out
}

/// The workspace of `branch` in the project at `root`, if it is on disk.
pub fn find_workspace(root: &Path, config: &pom_config::Config, branch: &str) -> Option<Workspace> {
    let known: std::collections::HashSet<String> = config.repos.keys().cloned().collect();
    pom_layout::scan(root, config.global_default_branch(), Some(&known))
        .into_iter()
        .find(|workspace| workspace.branch == branch)
}

/// The project's agent sessions, through the same driver `pom agent` uses.
pub struct DriveAgents {
    pub drive: pom_agent::Drive,
}

impl DriveAgents {
    pub fn new(context: &WorkspaceContext<'_>) -> DriveAgents {
        DriveAgents {
            drive: pom_agent::Drive {
                state: context.state.clone(),
                holders: context.runner.holders().clone(),
                project: context.config.session.clone(),
                home: std::env::var_os("HOME").map(Into::into).unwrap_or_default(),
                binary: context.runner.binary().to_path_buf(),
                tool_path: pom_services::tool_path().to_string(),
                limits: context.config.limits.clone(),
            },
        }
    }
}

/// Only the workspace's own `claude` session comes back by itself: another role was started for one task.
const RESTARTED_ROLE: &str = "claude";

impl Agents for DriveAgents {
    fn sessions(&self, branch: &str) -> Vec<AgentSession> {
        self.drive
            .list(branch)
            .into_iter()
            .map(|view| AgentSession {
                active_ms: now_ms().saturating_sub(view.last_event_age_s * 1000),
                busy: view.is_working()
                    || view.reason.as_deref() == Some(pom_agent::PENDING_APPROVAL),
                role: view.role,
                holder: view.holder,
                alive: view.alive,
            })
            .collect()
    }

    fn restart(&self, workspace: &Workspace, role: &str) -> Result<(), String> {
        if role != RESTARTED_ROLE {
            return Err(format!(
                "not restarted; `pom agent start --role {role}` starts it again"
            ));
        }
        let gate = pom_agent::Gate::open(
            &self.drive.state,
            pom_agent::Caller::Operator,
            pom_agent::Workspace {
                project: self.drive.project.clone(),
                branch: workspace.branch.clone(),
            },
            pom_agent::Limits::OPERATOR,
        )
        .map_err(|refusal| refusal.to_string())?;
        self.drive
            .start(
                &gate,
                &workspace.path,
                workspace.is_main,
                role,
                false,
                &pom_agent::LaunchOptions::default(),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Wakes `branch` of the project at `root` from its pom.yml alone, for a process that has no session open (the
/// dev proxy on a request).
pub fn wake_at(
    root: &Path,
    state: &pom_paths::StateDir,
    holders: &pom_ptyhost::SocketDir,
    binary: &Path,
    branch: &str,
) -> Result<crate::schedule::WakeOutcome, String> {
    let config = pom_config::Config::load(&root.join(pom_config::CONFIG_FILE_NAME))
        .map_err(|error| error.to_string())?;
    let runner = pom_services::ServiceRunner::new(pom_services::RunnerOptions {
        project_root: root.to_path_buf(),
        session: config.session.clone(),
        state: state.clone(),
        holders: holders.clone(),
        binary: binary.to_path_buf(),
        docker: "docker".into(),
    });
    let context = WorkspaceContext {
        config: &config,
        runner: &runner,
        state,
    };
    let workspace =
        find_workspace(root, &config, branch).ok_or_else(|| format!("no workspace {branch}"))?;
    crate::schedule::wake_with_room(
        &context,
        &DriveAgents::new(&context),
        &workspace,
        &crate::schedule::PowerSettings::from_settings_file(),
    )
}

/// Wakes `branch` of the project at a root; the dev proxy's hook.
pub type Waker = std::sync::Arc<dyn Fn(&Path, &str) + Send + Sync>;

/// What the dev proxy calls to wake a workspace a request reached while it slept.
pub fn waker(
    state: pom_paths::StateDir,
    holders: pom_ptyhost::SocketDir,
    binary: std::path::PathBuf,
) -> Waker {
    std::sync::Arc::new(move |root: &Path, branch: &str| {
        match wake_at(root, &state, &holders, &binary, branch) {
            Ok(crate::schedule::WakeOutcome::Woke { report, .. }) if !report.failed.is_empty() => {
                eprintln!(
                    "dev proxy: woke {branch}, but could not start {}",
                    report.failed.join("; ")
                )
            }
            Ok(crate::schedule::WakeOutcome::Queued { running }) => eprintln!(
                "dev proxy: {branch} is queued: {running} workspaces run, the most workspaces.max_running allows"
            ),
            Ok(_) => {}
            Err(error) => eprintln!("dev proxy: wake {branch}: {error}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeAgents {
        sessions: Vec<AgentSession>,
        restarted: std::sync::Mutex<Vec<String>>,
    }

    impl Agents for FakeAgents {
        fn sessions(&self, _branch: &str) -> Vec<AgentSession> {
            self.sessions.clone()
        }
        fn restart(&self, _workspace: &Workspace, role: &str) -> Result<(), String> {
            if let Ok(mut restarted) = self.restarted.lock() {
                restarted.push(role.to_string());
            }
            Ok(())
        }
    }

    fn session(role: &str, alive: bool, busy: bool) -> AgentSession {
        AgentSession {
            role: role.into(),
            holder: format!("ws-demo-feat-claude-{role}"),
            alive,
            busy,
            active_ms: 0,
        }
    }

    #[test]
    fn a_working_agent_keeps_the_workspace_awake_unless_forced() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("project");
        let folder = root.join("workspace--feat");
        std::fs::create_dir_all(&folder).expect("folder");
        let config = pom_config::Config {
            session: "demo".into(),
            ..pom_config::Config::default()
        };
        let state = pom_paths::StateDir::new(temp.path().join("state"));
        let runner = pom_services::ServiceRunner::new(pom_services::RunnerOptions {
            project_root: root,
            session: "demo".into(),
            state: state.clone(),
            holders: pom_ptyhost::SocketDir::new(temp.path().join("s")),
            binary: "/nonexistent".into(),
            docker: "docker".into(),
        });
        let context = WorkspaceContext {
            config: &config,
            runner: &runner,
            state: &state,
        };
        let workspace = Workspace {
            branch: "feat".into(),
            is_main: false,
            path: folder.clone(),
            repos: Vec::new(),
        };
        let agents = FakeAgents {
            sessions: vec![
                session("claude", true, false),
                session("reviewer", true, true),
                session("old", false, true),
            ],
            restarted: std::sync::Mutex::new(Vec::new()),
        };
        assert_eq!(
            sleep(&context, &agents, &workspace, false),
            Err(SleepError::AgentsBusy(vec!["reviewer".into()]))
        );
        assert_eq!(WorkspaceState::load(&folder).state, RunState::Running);

        let report = sleep(&context, &agents, &workspace, true).expect("forced");
        assert_eq!(report.agents, ["claude", "reviewer"]);
        let state = WorkspaceState::load(&folder);
        assert_eq!(state.state, RunState::Sleeping);
        assert_eq!(
            state.asleep.map(|record| record.agents),
            Some(vec!["claude".to_string(), "reviewer".to_string()])
        );

        let woken = wake(&context, &agents, &workspace).expect("wake");
        assert_eq!(woken.agents, ["claude", "reviewer"]);
        assert_eq!(
            *agents.restarted.lock().expect("restarted"),
            ["claude", "reviewer"]
        );
        assert_eq!(WorkspaceState::load(&folder), WorkspaceState::default());
    }
}
