//! Where the Services panel and a service's tab read their facts and send their actions: this machine's
//! runner, or a project's server on another machine. Both answer the same questions, so the panel and tab
//! draw and behave alike whichever one they are given.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pom_config::{Config, Service, SharedServiceDef};
use pom_services::{ServiceRunner, ServiceTarget, SharedAction};
use terminal::{HolderOptions, TerminalOptions, Waker};
use workspace::PanelRequest;

use crate::model::{Action, Crash, SharedConfig, Status, ALL_SHARED};

/// A control that a source may lack, such as a host whose pom predates it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceControl {
    /// Moving a service to a new port.
    Relocate,
    /// Starting, stopping and restarting the shared services.
    Shared,
    Mode,
    EnvProfile,
    /// A repo's pre-written commands.
    Tasks,
    /// A side agent asked about or fixing a service.
    SideAgents,
}

/// One of a repo's pre-written commands (the config's `shortcuts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoCommand {
    pub repo: String,
    pub label: String,
    pub cmd: String,
}

/// What a tab's console follows: a service's holder, or a shared service.
pub struct ConsoleRequest<'a> {
    pub target: &'a ServiceTarget,
    /// The holder whose process runs (a service's own, or a command shared service's).
    pub process_holder: &'a str,
    /// The id the tab's console is known by.
    pub item_id: &'a str,
    /// A shared service that runs as a container rather than in a holder.
    pub container: Option<&'a str>,
    /// Follow the live output rather than what the last run left.
    pub live: bool,
    pub root: &'a Path,
    /// The item id of the console a running service's tab types into.
    pub console_id: u64,
    pub waker: Waker,
}

/// What a tab lists, and the console it types into while the service runs.
#[derive(Default)]
pub struct Console {
    pub lines: Option<LogLines>,
    pub shell: Option<terminal_ui::TerminalItem>,
}

/// A tab's output: a terminal that runs off screen, or lines streamed from another machine.
pub enum LogLines {
    Terminal(Box<terminal::Terminal>),
    Stream(crate::remote::LineStream),
}

impl LogLines {
    pub fn output_lines(&self) -> Vec<String> {
        match self {
            LogLines::Terminal(terminal) => terminal.output_lines(),
            LogLines::Stream(stream) => stream.lines(),
        }
    }

    pub fn clear(&mut self) {
        match self {
            LogLines::Terminal(terminal) => terminal.clear(),
            LogLines::Stream(stream) => stream.clear(),
        }
    }

    /// Takes in what arrived; whether anything changed.
    pub fn sync(&mut self, host: &dyn terminal::TerminalHost) -> bool {
        match self {
            LogLines::Terminal(terminal) => {
                let outcome = terminal.sync(host);
                outcome.changed || outcome.title_changed
            }
            LogLines::Stream(stream) => stream.take_changed(),
        }
    }
}

pub trait ServicesAccess: Send + Sync {
    fn config(&self) -> Option<Arc<Config>>;
    fn holder_name(&self, target: &ServiceTarget) -> String;
    fn status_of(&self, target: &ServiceTarget) -> Status;
    /// How a crashed service died, read when it is first seen crashed.
    fn crash(&self, holder: &str) -> Option<Crash>;
    /// Where the full output of a crashed service is, when it left one on this machine.
    fn crash_log(&self, holder: &str) -> Option<PathBuf>;
    fn shared_running(&self) -> HashSet<String>;
    /// Running services of the project in workspaces other than the ones in `own`.
    fn others_running(&self, own: &HashSet<String>) -> usize;
    fn port(&self, config: &Config, target: &ServiceTarget) -> Option<u16>;
    /// The service's dev-proxy address.
    fn url(&self, config: &Config, target: &ServiceTarget) -> Option<String>;
    /// The address a browser here opens, or why it cannot.
    fn browser_url(&self, config: &Config, target: &ServiceTarget) -> Result<String, String> {
        self.url(config, target)
            .ok_or_else(|| "the service has no address".to_string())
    }
    fn mode(&self, target: &ServiceTarget, service: &Service) -> String;
    fn set_mode(&self, config: &Config, target: &ServiceTarget, mode: &str) -> Result<(), String>;
    fn env_profile(&self, config: &Config, target: &ServiceTarget) -> String;
    fn set_env_profile(
        &self,
        config: &Config,
        target: &ServiceTarget,
        profile: &str,
    ) -> Result<(), String>;
    /// How long the holder has run: "12m", "2h 5m".
    fn uptime(&self, holder: &str) -> Option<String>;
    fn pid(&self, holder: &str) -> Option<i32>;
    fn shared_host_port(&self, name: &str) -> u16;
    fn shared_holder(&self, name: &str) -> String;
    /// What `{{shared.<name>.url}}` gives workspace `branch`.
    fn shared_connection(&self, config: &Config, branch: &str, name: &str) -> Option<String>;
    /// The folder a command shared service runs in, relative to the project when it is inside it.
    fn shared_folder(&self, config: &Config, def: &SharedServiceDef) -> String;
    /// Runs `action` on `target` and waits for it.
    fn run(&self, config: &Config, action: Action, target: &ServiceTarget) -> Result<(), String>;
    /// Runs `action` on shared service `name`, or on all of them for [`ALL_SHARED`].
    fn run_shared(&self, config: &Config, name: &str, action: Action) -> Result<(), String>;
    /// Whether `repo` is checked out in the workspace at `root`.
    fn checked_out(&self, root: &Path, repo: &str) -> bool;
    /// Runs a repo command with the workspace's env; the requests that show it.
    fn run_command(
        &self,
        config: &Config,
        branch: &str,
        root: &Path,
        command: &RepoCommand,
    ) -> Vec<PanelRequest>;
    fn console(&self, request: ConsoleRequest<'_>) -> Console;
    /// Why `control` is not offered, when it is not.
    fn missing(&self, _control: ServiceControl) -> Option<String> {
        None
    }
    /// Why nothing can be shown yet, when nothing can.
    fn unavailable(&self) -> Option<String> {
        None
    }
    /// What went wrong since the last call, to say as toasts.
    fn take_notes(&self) -> Vec<String> {
        Vec::new()
    }
}

/// This machine: the project's runner, its holders and its config.
pub struct LocalServices {
    pub runner: Arc<ServiceRunner>,
    pub config: SharedConfig,
}

impl ServicesAccess for LocalServices {
    fn config(&self) -> Option<Arc<Config>> {
        self.config.read().ok().and_then(|config| config.clone())
    }

    fn holder_name(&self, target: &ServiceTarget) -> String {
        self.runner.holder_name(target)
    }

    fn status_of(&self, target: &ServiceTarget) -> Status {
        let holders = self.runner.holders();
        let name = self.runner.holder_name(target);
        if holders.holder_alive(&name) {
            Status::Running
        } else if holders.crash_info(&name).is_some_and(|info| info.crashed) {
            Status::Crashed
        } else {
            Status::Stopped
        }
    }

    fn crash(&self, holder: &str) -> Option<Crash> {
        let holders = self.runner.holders();
        let info = holders.crash_info(holder).filter(|info| info.crashed)?;
        let at = std::fs::metadata(holders.crash_log(holder))
            .and_then(|meta| meta.modified())
            .ok();
        Some(crate::model::crash_from(&info.header, &info.output, at))
    }

    fn crash_log(&self, holder: &str) -> Option<PathBuf> {
        Some(self.runner.holders().crash_log(holder)).filter(|log| log.is_file())
    }

    fn shared_running(&self) -> HashSet<String> {
        self.runner.shared_running()
    }

    fn others_running(&self, own: &HashSet<String>) -> usize {
        self.runner
            .running_holders()
            .into_iter()
            .filter(|holder| !own.contains(holder))
            .count()
    }

    fn port(&self, config: &Config, target: &ServiceTarget) -> Option<u16> {
        self.runner.port(config, target)
    }

    fn url(&self, config: &Config, target: &ServiceTarget) -> Option<String> {
        self.runner.proxy_url(config, target)
    }

    fn mode(&self, target: &ServiceTarget, service: &Service) -> String {
        self.runner.mode(&target.repo, &target.service, service)
    }

    fn set_mode(&self, config: &Config, target: &ServiceTarget, mode: &str) -> Result<(), String> {
        self.runner
            .set_mode(config, &target.repo, &target.service, mode)
            .map_err(|error| error.to_string())
    }

    fn env_profile(&self, config: &Config, target: &ServiceTarget) -> String {
        self.runner.env_profile(config, target)
    }

    fn set_env_profile(
        &self,
        config: &Config,
        target: &ServiceTarget,
        profile: &str,
    ) -> Result<(), String> {
        self.runner
            .set_env_profile(config, target, profile)
            .map_err(|error| error.to_string())
    }

    fn uptime(&self, holder: &str) -> Option<String> {
        crate::view::uptime(&self.runner.holders().pidfile(holder))
    }

    fn pid(&self, holder: &str) -> Option<i32> {
        self.runner.holders().holder_pid(holder)
    }

    fn shared_host_port(&self, name: &str) -> u16 {
        self.runner.shared_host_port(name)
    }

    fn shared_holder(&self, name: &str) -> String {
        self.runner.shared_holder(name)
    }

    fn shared_connection(&self, config: &Config, branch: &str, name: &str) -> Option<String> {
        self.runner.workspace_env(config, branch).shared_url(name)
    }

    fn shared_folder(&self, config: &Config, def: &SharedServiceDef) -> String {
        let folder = self.runner.shared_command_dir(config, def);
        folder
            .strip_prefix(self.runner.project_root())
            .ok()
            .map(|relative| relative.to_string_lossy().into_owned())
            .filter(|relative| !relative.is_empty())
            .unwrap_or_else(|| "project folder".to_string())
    }

    fn run(&self, config: &Config, action: Action, target: &ServiceTarget) -> Result<(), String> {
        let runner = &self.runner;
        let result = match action {
            Action::Start => runner.start(config, target).map(|_| ()),
            Action::Restart => runner.restart(config, target).map(|_| ()),
            Action::Relocate => runner.relocate(config, target).map(|_| ()),
            Action::Stop => runner.stop(target).map_err(Into::into),
        };
        result.map_err(|error| error.to_string())
    }

    fn run_shared(&self, config: &Config, name: &str, action: Action) -> Result<(), String> {
        let runner = &self.runner;
        let result = match (name, action) {
            (ALL_SHARED, Action::Stop) => runner.stop_shared(),
            (ALL_SHARED, _) => runner.ensure_shared(config),
            (name, Action::Stop) => runner.shared_action(config, name, SharedAction::Stop),
            (name, Action::Start) => runner.shared_action(config, name, SharedAction::Start),
            (name, _) => runner.shared_action(config, name, SharedAction::Restart),
        };
        result.map_err(|error| error.to_string())
    }

    fn checked_out(&self, root: &Path, repo: &str) -> bool {
        root.join(repo).is_dir()
    }

    fn run_command(
        &self,
        config: &Config,
        branch: &str,
        root: &Path,
        command: &RepoCommand,
    ) -> Vec<PanelRequest> {
        let mut requests = Vec::new();
        let env = self.runner.workspace_env(config, branch);
        if let Err(error) = env.write_env_files() {
            requests.push(PanelRequest::Toast(format!("Env files: {error}")));
        }
        let exports: String = env
            .repo_env(&command.repo)
            .into_iter()
            .map(|(key, value)| format!("export {key}={}; ", pom_services::shell_quote(&value)))
            .collect();
        let worktree = root.join(&command.repo);
        let script = format!(
            "export PATH={path}; {exports}cd {dir} && {cmd}; status=$?; printf '\\n[exited with %s]\\n' \"$status\"",
            path = pom_services::shell_quote(pom_services::tool_path()),
            dir = pom_services::shell_quote(&worktree.to_string_lossy()),
            cmd = command.cmd,
        );
        requests.push(PanelRequest::RunCommand {
            title: format!("{}: {}", command.repo, command.label),
            cwd: worktree,
            argv: pom_shell::Shell::user().login_argv(&script),
            env: Vec::new(),
        });
        requests
    }

    fn console(&self, request: ConsoleRequest<'_>) -> Console {
        let options = self.console_options(&request);
        let waker = request.waker.clone();
        let shell = match (request.live, &options.holder) {
            (true, Some(holder)) => terminal_ui::TerminalItem::service_console(
                request.console_id,
                request.root.to_path_buf(),
                request.item_id.to_string(),
                request.target.service.clone(),
                holder.clone(),
                waker.clone(),
            )
            .map_err(|error| eprintln!("services: console: {error}"))
            .ok(),
            _ => None,
        };
        let lines = match terminal::Terminal::spawn(options, waker) {
            Ok(mut terminal) => {
                terminal.set_size(terminal::TerminalBounds {
                    cell_width: 8.0,
                    line_height: 16.0,
                    width: 8.0 * 240.0,
                    height: 16.0 * 60.0,
                });
                Some(LogLines::Terminal(Box::new(terminal)))
            }
            Err(error) => {
                eprintln!("services: console: {error}");
                None
            }
        };
        Console { lines, shell }
    }
}

impl LocalServices {
    fn console_options(&self, request: &ConsoleRequest<'_>) -> TerminalOptions {
        let runner = &self.runner;
        if let Some(name) = request.container {
            let mut args = vec![
                format!("PATH={}", pom_services::tool_path()),
                "docker".to_string(),
                "compose".to_string(),
                "-f".to_string(),
                runner.compose_file().to_string_lossy().into_owned(),
                "-p".to_string(),
                runner.compose_project(),
                "logs".to_string(),
            ];
            if request.live {
                args.push("-f".to_string());
            }
            args.extend(["--tail".to_string(), "200".to_string(), name.to_string()]);
            return TerminalOptions {
                shell: Some(("/usr/bin/env".to_string(), args)),
                working_directory: Some(request.root.to_path_buf()),
                ..TerminalOptions::default()
            };
        }
        let holders = runner.holders().clone();
        let process = request.process_holder.to_string();
        if request.live {
            return TerminalOptions {
                working_directory: Some(request.root.to_path_buf()),
                holder: Some(HolderOptions {
                    dir: holders,
                    name: process,
                    binary: std::env::current_exe().unwrap_or_default(),
                    attach_only: true,
                    group: None,
                }),
                ..TerminalOptions::default()
            };
        }
        let log = holders.crash_log(&process);
        let shell = if log.is_file() {
            (
                "/usr/bin/tail".to_string(),
                vec![
                    "-n".to_string(),
                    "+2".to_string(),
                    log.to_string_lossy().into_owned(),
                ],
            )
        } else {
            (
                "/bin/echo".to_string(),
                vec!["Not running. Start it to follow its output here.".to_string()],
            )
        };
        TerminalOptions {
            shell: Some(shell),
            working_directory: Some(request.root.to_path_buf()),
            ..TerminalOptions::default()
        }
    }
}
