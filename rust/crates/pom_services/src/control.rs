//! Starting and stopping a workspace's services. Each runs in a detached PTY holder named after the
//! session, workspace, repo and service, so it keeps running when the app quits and any client can
//! reattach to its console.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use pom_config::{Config, Dir, Service};
use pom_env::{EnvSources, SlotAllocation, BIND_IP};
use pom_layout::WorkspaceState;
use pom_paths::StateDir;
use pom_ports::{PortManager, PortState, Probe, SystemProbe};
use pom_ptyhost::{SocketDir, SpawnRequest};
use pom_secrets::SecretStore;

use crate::env_files::{alias, WorkspaceEnv};
use crate::slots::SlotStore;
use crate::tool_path::tool_path;

/// A repo named like this (or empty) addresses a workspace-level service.
const WORKSPACE_REPO: &str = "_ws";
const LOCAL_PROFILE: &str = "local";
const RELOCATE_ATTEMPTS: usize = 3;
/// A just-started Postgres takes a few seconds before it accepts connections.
const DATABASE_WAIT: Duration = Duration::from_secs(30);
/// Start returns once the holder accepts clients, so its console can be opened right away.
const HOLDER_START_TIMEOUT: Duration = Duration::from_secs(5);

/// One service in one workspace. An empty `repo` means a workspace-level service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceTarget {
    pub branch: String,
    pub is_main: bool,
    pub repo: String,
    pub service: String,
}

impl ServiceTarget {
    pub fn is_workspace_level(&self) -> bool {
        self.repo.is_empty() || self.repo == WORKSPACE_REPO
    }
}

#[derive(Debug)]
pub enum ServiceError {
    UnknownRepo(String),
    UnknownService(String),
    NoFreePort,
    PortBusy { port: u16, owner: Option<String> },
    Io(std::io::Error),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::UnknownRepo(repo) => write!(formatter, "unknown repo: {repo}"),
            ServiceError::UnknownService(service) => {
                write!(formatter, "unknown or blank service: {service}")
            }
            ServiceError::NoFreePort => write!(
                formatter,
                "no free port in {}-{} - free some and retry",
                pom_ports::PORT_LOW,
                pom_ports::PORT_HIGH
            ),
            ServiceError::PortBusy {
                port,
                owner: Some(owner),
            } => write!(
                formatter,
                "port {port} is already held by {owner} - stop it if it is not this service, or use a new port"
            ),
            ServiceError::PortBusy { port, owner: None } => write!(
                formatter,
                "port {port} is already in use - free it or use a new port"
            ),
            ServiceError::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<std::io::Error> for ServiceError {
    fn from(error: std::io::Error) -> ServiceError {
        ServiceError::Io(error)
    }
}

pub struct RunnerOptions {
    pub project_root: PathBuf,
    pub session: String,
    pub state: StateDir,
    pub holders: SocketDir,
    /// The binary that hosts holders (`<binary> pty run ...`); the app passes itself.
    pub binary: PathBuf,
    /// The Docker CLI that runs shared services (usually just `docker`, found on the tool PATH).
    pub docker: PathBuf,
}

/// Runs one project's services.
pub struct ServiceRunner {
    pub(crate) project_root: PathBuf,
    pub(crate) session: String,
    pub(crate) ports: PortManager,
    pub(crate) slots: SlotStore,
    secrets: SecretStore,
    holders: SocketDir,
    binary: PathBuf,
    pub(crate) docker: PathBuf,
    /// `repo~service` -> mode picked for this app run.
    modes: Mutex<HashMap<String, String>>,
}

impl ServiceRunner {
    pub fn new(options: RunnerOptions) -> ServiceRunner {
        let holders = options.holders.clone();
        let probe = SystemProbe::new(Box::new(move |name| holders.holder_alive(name)));
        ServiceRunner::with_probe(options, Box::new(probe))
    }

    pub fn with_probe(options: RunnerOptions, probe: Box<dyn Probe>) -> ServiceRunner {
        ServiceRunner {
            ports: PortManager::open(&options.state, &options.session, probe),
            slots: SlotStore::new(options.state.clone()),
            secrets: SecretStore::new(options.state, &options.session),
            project_root: options.project_root,
            session: options.session,
            holders: options.holders,
            binary: options.binary,
            docker: options.docker,
            modes: Mutex::new(HashMap::new()),
        }
    }

    pub fn ports(&self) -> &PortManager {
        &self.ports
    }

    pub fn holders(&self) -> &SocketDir {
        &self.holders
    }

    pub fn holder_name(&self, target: &ServiceTarget) -> String {
        let session = pom_env::branch_safe(&self.session);
        let branch = pom_env::branch_safe(&target.branch);
        if target.is_workspace_level() {
            format!("ws-{session}-{branch}-{}", target.service)
        } else {
            format!("svc-{session}-{branch}-{}-{}", target.repo, target.service)
        }
    }

    /// Running service holders of this project, in every workspace.
    pub fn running_holders(&self) -> Vec<String> {
        let session = pom_env::branch_safe(&self.session);
        let prefixes = [format!("svc-{session}-"), format!("ws-{session}-")];
        self.holders
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| prefixes.iter().any(|prefix| name.starts_with(prefix)))
            .collect()
    }

    pub fn is_running(&self, target: &ServiceTarget) -> bool {
        self.holders.holder_alive(&self.holder_name(target))
    }

    /// Starts the service unless it already runs; returns its holder name.
    pub fn start(&self, config: &Config, target: &ServiceTarget) -> Result<String, ServiceError> {
        let holder = self.holder_name(target);
        if self.holders.holder_alive(&holder) {
            return Ok(holder);
        }
        if target.is_workspace_level() {
            self.start_workspace_service(config, target, &holder)?;
        } else {
            self.prepare_shared(config, &target.branch);
            self.start_repo_service(config, target, &holder)?;
        }
        Ok(holder)
    }

    /// Shared services and the workspace's databases come up before a repo service needs them. A failure
    /// here is not fatal: the service may not use them, and its console shows what went wrong if it does.
    fn prepare_shared(&self, config: &Config, branch: &str) {
        if config.shared_services.is_empty() {
            return;
        }
        if let Err(error) = self.ensure_shared(config) {
            eprintln!("services: shared services: {error}");
            return;
        }
        let deadline = std::time::Instant::now() + DATABASE_WAIT;
        loop {
            match self.ensure_databases(config, branch) {
                Ok(()) => return,
                Err(error) if still_starting(&error) && std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(error) => {
                    eprintln!("services: {error}");
                    return;
                }
            }
        }
    }

    /// Stops the service's whole process tree. Its port lease stays, so a restart reuses the port;
    /// the reaper reclaims it once the service has stayed down.
    pub fn stop(&self, target: &ServiceTarget) -> std::io::Result<()> {
        match self.holders.kill_holder(&self.holder_name(target)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    pub fn restart(&self, config: &Config, target: &ServiceTarget) -> Result<String, ServiceError> {
        self.stop(target)?;
        self.start(config, target)
    }

    /// Restarts on a freshly leased port, for when the old one is taken by something else.
    pub fn relocate(
        &self,
        config: &Config,
        target: &ServiceTarget,
    ) -> Result<String, ServiceError> {
        self.stop(target)?;
        if let Some(key) = self.lease_key(config, target) {
            self.ports.release(&key);
        }
        self.start(config, target)
    }

    /// Drops every port lease of the workspace and leases fresh ones, rewriting its env files: the way
    /// out when something outside took the workspace's ports.
    pub fn relocate_workspace(&self, config: &Config, branch: &str) -> Result<(), ServiceError> {
        let ws_key = pom_env::port_ws_key(branch);
        self.ports.release_workspace(&ws_key);
        self.acquire_workspace_ports(config, &ws_key);
        self.workspace_env(config, branch).write_env_files()?;
        Ok(())
    }

    /// The workspace's env, resolved against this runner's leases, slots and secrets.
    pub fn workspace_env<'a>(&'a self, config: &'a Config, branch: &'a str) -> WorkspaceEnv<'a> {
        WorkspaceEnv {
            config,
            project_root: &self.project_root,
            branch,
            sources: self,
        }
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn set_mode(
        &self,
        config: &Config,
        repo: &str,
        service: &str,
        mode: &str,
    ) -> Result<(), ServiceError> {
        let dir = config
            .repos
            .get(repo)
            .ok_or_else(|| ServiceError::UnknownRepo(repo.to_string()))?;
        match dir.services.get(service) {
            Some(own) if !own.modes.is_empty() => {}
            _ => return Err(ServiceError::UnknownService(service.to_string())),
        }
        if let Ok(mut modes) = self.modes.lock() {
            modes.insert(format!("{repo}~{service}"), mode.to_string());
        }
        Ok(())
    }

    pub fn mode(&self, repo: &str, service_name: &str, service: &Service) -> String {
        self.modes
            .lock()
            .ok()
            .and_then(|modes| modes.get(&format!("{repo}~{service_name}")).cloned())
            .filter(|mode| !mode.is_empty())
            .unwrap_or_else(|| service.mode.clone())
    }

    pub fn env_profile(&self, config: &Config, target: &ServiceTarget) -> String {
        let Some(dir) = config.repos.get(&target.repo) else {
            return String::new();
        };
        let state = WorkspaceState::load(&pom_layout::workspace_folder(
            &self.project_root,
            &target.branch,
        ));
        let profile =
            state.service_env(&format!("{}/{}", alias(&target.repo, dir), target.service));
        if profile.is_empty() {
            LOCAL_PROFILE.to_string()
        } else {
            profile.to_string()
        }
    }

    pub fn set_env_profile(
        &self,
        config: &Config,
        target: &ServiceTarget,
        profile: &str,
    ) -> Result<(), ServiceError> {
        let dir = config
            .repos
            .get(&target.repo)
            .ok_or_else(|| ServiceError::UnknownRepo(target.repo.clone()))?;
        let folder = pom_layout::workspace_folder(&self.project_root, &target.branch);
        let mut state = WorkspaceState::load(&folder);
        let key = format!("{}/{}", alias(&target.repo, dir), target.service);
        if profile.is_empty() || profile == LOCAL_PROFILE {
            state.service_envs.shift_remove(&key);
        } else {
            state.service_envs.insert(key, profile.to_string());
        }
        std::fs::create_dir_all(&folder)?;
        state.save(&folder)?;
        Ok(())
    }

    /// The port leased to a repo service with a port, once it has one.
    pub fn port(&self, config: &Config, target: &ServiceTarget) -> Option<u16> {
        self.ports.port_of(&self.lease_key(config, target)?)
    }

    pub fn url(&self, config: &Config, target: &ServiceTarget) -> Option<String> {
        self.port(config, target)
            .map(|port| format!("http://{BIND_IP}:{port}"))
    }

    fn lease_key(&self, config: &Config, target: &ServiceTarget) -> Option<String> {
        let dir = config.repos.get(&target.repo)?;
        dir.services
            .get(&target.service)
            .filter(|service| service.has_port())?;
        Some(pom_ports::service_key(
            &pom_env::port_ws_key(&target.branch),
            &format!("{}~{}", alias(&target.repo, dir), target.service),
        ))
    }

    fn start_workspace_service(
        &self,
        config: &Config,
        target: &ServiceTarget,
        holder: &str,
    ) -> Result<(), ServiceError> {
        let service = config
            .workspace_services
            .get(&target.service)
            .filter(|service| !service.cmd.is_empty())
            .ok_or_else(|| ServiceError::UnknownService(target.service.clone()))?;
        let cwd = pom_layout::workspace_root(&self.project_root, &target.branch, target.is_main);
        self.spawn(holder, &cwd, &service.cmd, Vec::new())
    }

    fn start_repo_service(
        &self,
        config: &Config,
        target: &ServiceTarget,
        holder: &str,
    ) -> Result<(), ServiceError> {
        let dir = config
            .repos
            .get(&target.repo)
            .ok_or_else(|| ServiceError::UnknownRepo(target.repo.clone()))?;
        let service = dir
            .services
            .get(&target.service)
            .filter(|service| !service.active_cmd("").is_empty())
            .ok_or_else(|| ServiceError::UnknownService(target.service.clone()))?;
        let worktree = pom_layout::repo_worktree(
            &self.project_root,
            &target.repo,
            &target.branch,
            target.is_main,
        );
        let lease = self.lease_key(config, target);
        let port = match &lease {
            Some(key) => Some(self.preflight_port(key)?),
            None => None,
        };
        let mode = self.mode(&target.repo, &target.service, service);
        let command = service_command(&worktree, dir, service, port, &mode);
        let ws_key = pom_env::port_ws_key(&target.branch);
        self.allocate_slots(config, &ws_key)?;
        self.acquire_workspace_ports(config, &ws_key);
        let env = WorkspaceEnv {
            config,
            project_root: &self.project_root,
            branch: &target.branch,
            sources: self,
        };
        env.write_env_files()?;
        let service_env = env.service_env(&target.repo, &target.service);
        if let Some(key) = &lease {
            self.ports.set_holder(key, holder);
        }
        self.spawn(holder, &worktree, &command, service_env)
    }

    /// The service's port, moving to a new one when something outside our leases already listens on
    /// the old one.
    fn preflight_port(&self, key: &str) -> Result<u16, ServiceError> {
        let mut port = self.lease_port(key)?;
        // A fresh port can be taken by another process before we look again, so try a few.
        for _ in 0..RELOCATE_ATTEMPTS {
            if self.ports.bindable(port) {
                break;
            }
            self.ports.release(key);
            port = self.lease_port(key)?;
        }
        if !self.ports.bindable(port) {
            return Err(ServiceError::PortBusy {
                port,
                owner: port_owner(port),
            });
        }
        self.ports.mark(key, PortState::Starting);
        Ok(port)
    }

    fn lease_port(&self, key: &str) -> Result<u16, ServiceError> {
        self.ports.acquire(key).ok_or(ServiceError::NoFreePort)
    }

    /// Every service with a port gets its lease up front, so cross-service refs resolve to real ports
    /// even for services not started yet.
    fn acquire_workspace_ports(&self, config: &Config, ws_key: &str) {
        for (repo, dir) in &config.repos {
            for (name, service) in &dir.services {
                if service.has_port() {
                    let key = format!("{}~{name}", alias(repo, dir));
                    if self
                        .ports
                        .acquire(&pom_ports::service_key(ws_key, &key))
                        .is_none()
                    {
                        eprintln!("services: no free port for {key}");
                    }
                }
            }
        }
    }

    fn allocate_slots(&self, config: &Config, ws_key: &str) -> std::io::Result<()> {
        let mut wanted: Vec<String> = Vec::new();
        for dir in config.repos.values() {
            wanted.extend(dir.shared_refs.iter().map(|shared| shared.name.clone()));
            for value in dir.env.values() {
                wanted.extend(pom_env::template::slot_refs(value));
            }
        }
        let mut done: Vec<&str> = Vec::new();
        for name in &wanted {
            if done.contains(&name.as_str()) {
                continue;
            }
            if let Some(capacity) = config
                .shared_services
                .get(name)
                .and_then(|shared| shared.capacity)
            {
                self.slots.allocate(name, ws_key, capacity)?;
                done.push(name);
            }
        }
        Ok(())
    }

    fn spawn(
        &self,
        holder: &str,
        cwd: &Path,
        command: &str,
        env: Vec<(String, String)>,
    ) -> Result<(), ServiceError> {
        let mut full_env = vec![("PATH".to_string(), tool_path().to_string())];
        full_env.extend(env);
        let argv = login_shell(command);
        pom_ptyhost::spawn_holder(
            &self.holders,
            &SpawnRequest {
                binary: &self.binary,
                name: holder,
                cwd,
                cols: 0,
                rows: 0,
                argv: &argv,
                env: &full_env,
            },
        )?;
        pom_ptyhost::wait_for_holder(&self.holders, holder, HOLDER_START_TIMEOUT)?;
        Ok(())
    }
}

impl EnvSources for ServiceRunner {
    fn shared_port(&self, name: &str) -> Option<u16> {
        self.ports
            .port_of(&pom_ports::shared_key(name, 0))
            .or_else(|| Some(pom_env::stable_shared_port(&self.session, name)))
    }

    fn service_port(&self, ws_key: &str, service_key: &str) -> Option<u16> {
        self.ports
            .port_of(&pom_ports::service_key(ws_key, service_key))
    }

    fn slot(&self, shared_name: &str, ws_key: &str) -> Option<SlotAllocation> {
        self.slots.get(shared_name, ws_key)
    }

    fn secret(&self, _session: &str, name: &str) -> Option<String> {
        match self.secrets.get(name) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("services: secret {name}: {error:?}");
                None
            }
        }
    }
}

fn still_starting(error: &ServiceError) -> bool {
    let text = error.to_string();
    [
        "connection to server",
        "the database system is starting up",
        "Connection refused",
    ]
    .iter()
    .any(|sign| text.contains(sign))
}

/// `cd <dir> && <pre_start> && export PORT BIND_IP && <cmd>`, prefixed by the shell env assignments.
pub fn service_command(
    worktree: &Path,
    dir: &Dir,
    service: &Service,
    port: Option<u16>,
    mode: &str,
) -> String {
    let service_dir = if service.dir.is_empty() {
        worktree.to_path_buf()
    } else {
        worktree.join(&service.dir)
    };
    let mut command = format!("cd {}", shell_quote(&service_dir.to_string_lossy()));
    let pre_start = if service.pre_start.is_empty() {
        &dir.pre_start
    } else {
        &service.pre_start
    };
    if !pre_start.is_empty() {
        command.push_str(" && ");
        command.push_str(pre_start);
    }
    if let Some(port) = port {
        command.push_str(&format!(" && export PORT={port} BIND_IP={BIND_IP}"));
    }
    command.push_str(" && ");
    command.push_str(service.active_cmd(mode));
    let shell_env = if service.shell_env.is_empty() {
        &dir.shell_env
    } else {
        &service.shell_env
    };
    if shell_env.is_empty() {
        command
    } else {
        format!("{shell_env} {command}")
    }
}

/// A login shell so the user's profile (tool versions, exports) applies to the command.
pub fn login_shell(command: &str) -> Vec<String> {
    vec!["zsh".into(), "-lc".into(), command.to_string()]
}

pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `command (pid N)` of whatever listens on the port, for telling the user what is in the way.
fn port_owner(port: u16) -> Option<String> {
    let output = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let field = |tag: char| {
        text.lines()
            .find_map(|line| line.strip_prefix(tag))
            .filter(|value| !value.is_empty())
    };
    match (field('c'), field('p')) {
        (Some(command), Some(pid)) => Some(format!("{command} (pid {pid})")),
        (Some(command), None) => Some(command.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_chains_dir_pre_start_port_and_shell_env() {
        let mut dir = Dir {
            pre_start: "nvm use".into(),
            shell_env: "FOO=1".into(),
            ..Dir::default()
        };
        let service = Service {
            cmd: "pnpm dev".into(),
            dir: "apps/web".into(),
            modes: [("prod".to_string(), "pnpm start".to_string())]
                .into_iter()
                .collect(),
            ..Service::default()
        };
        let command = service_command(Path::new("/w/it's"), &dir, &service, Some(12345), "");
        assert_eq!(
            command,
            r"FOO=1 cd '/w/it'\''s/apps/web' && nvm use && export PORT=12345 BIND_IP=127.0.0.1 && pnpm dev"
        );
        dir.shell_env.clear();
        dir.pre_start.clear();
        let command = service_command(Path::new("/w"), &dir, &service, None, "prod");
        assert_eq!(command, "cd '/w/apps/web' && pnpm start");
    }
}
