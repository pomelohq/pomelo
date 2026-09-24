//! `pom`: a workspace's services from a terminal. It drives the same holders, leases and compose project as
//! the app, so a service started here shows up in the app's Services panel and the other way round.

mod workspaces;

use std::io::Write;
use std::path::{Path, PathBuf};

use pom_config::{Config, DepGraph};
use pom_core::Project;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};
use workspaces::WorkspaceCommand;

const USAGE: &str = "usage: pom [-w <workspace>] [-c <pom.yml>] <command> [args]

commands:
  start <target>     start a service, a repo's services, or a `workspaces:` group
  stop [target]      stop them; with no target, every service of the workspace
  restart <target>   stop, then start
  status             services of the workspace and whether they run
  logs <service>     recent output of a service
  attach <service>   attach this terminal to a running service (detach: close the terminal)
  ports              every leased port
  url <service>      where a service with a port listens
  mcp [--branch b]   MCP server on stdio for a coding agent working in this workspace

  ws create <branch> [--repos a,b] [--env name] [--no-seed] [--from-stage n]
                     new workspace: worktrees, databases, env files, setup and seed
  ws delete <branch> [--from-stage n]
                     stop it and remove its worktrees, databases and folder; the local
                     branch goes too only when it is pushed or merged
  ws rename <branch> [name]   set or clear the workspace's display name
  ws list            workspaces, their repos and running services
  prepare-main [--no-seed]    reset main's databases, migrate and seed (new workspaces copy them)
  doctor             what keeps the project from running, and how to fix it
  version

The workspace is the one the current directory is in, else the main one; -w picks another.
A service is `name` or `repo/name`.";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Start(String),
    Stop(Option<String>),
    Restart(String),
    Status,
    Logs(String),
    Attach(String),
    Ports,
    Url(String),
    Workspace(WorkspaceCommand),
    Doctor,
    Version,
    Help,
}

#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    workspace: Option<String>,
    config: Option<PathBuf>,
    command: Command,
}

/// Runs `args` (the program name first) as if typed in `cwd`; returns the exit code.
pub fn run(args: &[String], cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let invocation = match parse(args.get(1..).unwrap_or_default()) {
        Ok(invocation) => invocation,
        Err(message) => {
            report(err, &format!("{message}\n\n{USAGE}"));
            return 2;
        }
    };
    let result =
        match invocation.command {
            Command::Help => writeln!(out, "{USAGE}").map_err(|error| error.to_string()),
            Command::Version => writeln!(out, "pom {}", env!("CARGO_PKG_VERSION"))
                .map_err(|error| error.to_string()),
            Command::Ports => ports(&StateDir::from_env(), out),
            Command::Doctor => doctor(invocation.config.as_deref(), cwd, out),
            ref command => Session::open(&invocation, cwd)
                .and_then(|session| session.execute(command, out, err)),
        };
    match result {
        Ok(()) => 0,
        Err(message) => {
            report(err, &format!("error: {message}"));
            1
        }
    }
}

fn report(err: &mut dyn Write, message: &str) {
    if let Err(error) = writeln!(err, "{message}") {
        eprintln!("{message} ({error})");
    }
}

fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut workspace = None;
    let mut config = None;
    let mut words: Vec<&str> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        // Workspace commands have flags of their own.
        let help = matches!(arg.as_str(), "-h" | "--help");
        if !help && matches!(words.first(), Some(&("ws" | "workspace" | "prepare-main"))) {
            words.push(arg);
            continue;
        }
        match arg.as_str() {
            "-w" | "--workspace" => {
                workspace = Some(iter.next().ok_or("-w needs a workspace")?.clone());
            }
            "-c" | "--config" => {
                config = Some(PathBuf::from(iter.next().ok_or("-c needs a path")?));
            }
            "-h" | "--help" | "help" => words.insert(0, "help"),
            flag if flag.starts_with('-') => return Err(format!("unknown flag {flag}")),
            word => words.push(word),
        }
    }
    let (name, rest) = words.split_first().ok_or("missing command")?;
    let one = |what: &str| -> Result<String, String> {
        match rest {
            [only] => Ok(only.to_string()),
            [] => Err(format!("{name} needs {what}")),
            _ => Err(format!("{name} takes one {what}")),
        }
    };
    let none = || -> Result<(), String> {
        if rest.is_empty() {
            Ok(())
        } else {
            Err(format!("{name} takes no arguments"))
        }
    };
    let command = match *name {
        "start" => Command::Start(one("a target")?),
        "stop" => match rest {
            [] => Command::Stop(None),
            _ => Command::Stop(Some(one("target")?)),
        },
        "restart" => Command::Restart(one("a target")?),
        "status" => none().map(|_| Command::Status)?,
        "logs" => Command::Logs(one("a service")?),
        "attach" => Command::Attach(one("a service")?),
        "ports" => none().map(|_| Command::Ports)?,
        "url" => Command::Url(one("a service")?),
        "ws" | "workspace" => Command::Workspace(workspaces::parse(rest, false)?),
        "prepare-main" => Command::Workspace(workspaces::parse(rest, true)?),
        "doctor" => none().map(|_| Command::Doctor)?,
        "version" => Command::Version,
        "help" => Command::Help,
        other => return Err(format!("unknown command {other}")),
    };
    Ok(Invocation {
        workspace,
        config,
        command,
    })
}

fn doctor(explicit: Option<&Path>, cwd: &Path, out: &mut dyn Write) -> Result<(), String> {
    let config_path = find_config(explicit, cwd).unwrap_or_else(|_| cwd.join("pom.yml"));
    let config = Config::load(&config_path).ok();
    let root = config_path.parent().unwrap_or(cwd);
    let names = config
        .as_ref()
        .map(|config| {
            pom_secrets::SecretStore::new(StateDir::from_env(), &config.session)
                .names()
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let path = pom_services::tool_path();
    let has_tool = |tool: &str| pom_doctor::on_path(path, tool);
    let docker_running = || pom_doctor::docker_answers(path);
    let findings = pom_doctor::diagnose(
        config.as_ref(),
        &config_path,
        root,
        &names,
        &pom_doctor::Machine {
            has_tool: &has_tool,
            docker_running: &docker_running,
        },
    );
    let write = |out: &mut dyn Write, line: String| {
        writeln!(out, "{line}").map_err(|error| error.to_string())
    };
    for finding in &findings {
        let mark = match finding.severity {
            pom_doctor::Severity::Error => "error",
            pom_doctor::Severity::Warn => "warn ",
            pom_doctor::Severity::Ok => "ok   ",
        };
        write(out, format!("{mark}  {}", finding.title))?;
        if !finding.detail.is_empty() {
            write(out, format!("       {}", finding.detail))?;
        }
        if !finding.fix.is_empty() {
            write(out, format!("       fix: {}", finding.fix))?;
        }
    }
    let errors = findings
        .iter()
        .filter(|finding| finding.severity == pom_doctor::Severity::Error)
        .count();
    if errors > 0 {
        return Err(format!("{errors} blocking problem(s)"));
    }
    Ok(())
}

/// The project `cwd` is in (its `pom.yml` is here or in a parent), or the explicit one.
fn find_config(explicit: Option<&Path>, cwd: &Path) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return path
            .is_file()
            .then(|| path.to_path_buf())
            .ok_or_else(|| format!("{} not found", path.display()));
    }
    cwd.ancestors()
        .find_map(pom_core::config_in)
        .ok_or_else(|| "no pom.yml here or in any parent directory (use -c)".to_string())
}

struct Session {
    project: Project,
    state: StateDir,
    config: Config,
    runner: ServiceRunner,
    branch: String,
    is_main: bool,
}

impl Session {
    fn open(invocation: &Invocation, cwd: &Path) -> Result<Session, String> {
        let state = StateDir::from_env();
        let config_path = find_config(invocation.config.as_deref(), cwd)?;
        let project = Project::open(&config_path, &state);
        let config = match (&project.config, &project.error) {
            (Some(config), None) => config.clone(),
            (_, Some(problem)) => return Err(problem.message.clone()),
            (None, None) => return Err("pom.yml could not be loaded".into()),
        };
        let (branch, is_main) = pick_workspace(&project, invocation.workspace.as_deref(), cwd)?;
        let binary = std::env::current_exe().map_err(|error| error.to_string())?;
        let runner = ServiceRunner::new(RunnerOptions {
            project_root: project.root.clone(),
            session: config.session.clone(),
            state: state.clone(),
            holders: SocketDir::from_env(),
            binary,
            docker: "docker".into(),
        });
        Ok(Session {
            project,
            state,
            config,
            runner,
            branch,
            is_main,
        })
    }

    fn target(&self, repo: &str, service: &str) -> ServiceTarget {
        ServiceTarget {
            branch: self.branch.clone(),
            is_main: self.is_main,
            repo: repo.to_string(),
            service: service.to_string(),
        }
    }

    fn execute(
        &self,
        command: &Command,
        out: &mut dyn Write,
        err: &mut dyn Write,
    ) -> Result<(), String> {
        match command {
            Command::Start(target) => self.start(target, out, err),
            Command::Stop(target) => self.stop(target.as_deref(), out),
            Command::Restart(target) => {
                self.stop(Some(target), out)?;
                self.start(target, out, err)
            }
            Command::Status => self.status(out),
            Command::Logs(service) => self.logs(service, out),
            Command::Attach(service) => self.attach(service),
            Command::Url(service) => self.url(service, out),
            Command::Workspace(command) => self.workspace(command, out),
            Command::Ports | Command::Doctor | Command::Version | Command::Help => Ok(()),
        }
    }

    /// `(repo, service)` pairs of `target`, each repo's in dependency order (reversed for stopping).
    fn ordered(&self, target: &str, stopping: bool) -> Result<Vec<(String, String)>, String> {
        let pairs = self.config.resolve_services(target)?;
        let mut repos: Vec<(String, Vec<String>)> = Vec::new();
        for (repo, service) in pairs {
            match repos.iter_mut().find(|(name, _)| *name == repo) {
                Some((_, services)) => services.push(service),
                None => repos.push((repo, vec![service])),
            }
        }
        let mut ordered = Vec::new();
        for (repo, services) in repos {
            let services = match self.config.repos.get(&repo) {
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

    fn start(&self, target: &str, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), String> {
        let mut failed = 0;
        for (repo, service) in self.ordered(target, false)? {
            let target = self.target(&repo, &service);
            if self.runner.is_running(&target) {
                say(out, &format!("{repo}/{service} is already running"))?;
                continue;
            }
            match self.runner.start(&self.config, &target) {
                Ok(_) => {
                    let port = self
                        .runner
                        .port(&self.config, &target)
                        .map(|port| format!(" on port {port}"))
                        .unwrap_or_default();
                    say(out, &format!("started {repo}/{service}{port}"))?;
                }
                Err(error) => {
                    failed += 1;
                    say(err, &format!("could not start {repo}/{service}: {error}"))?;
                }
            }
        }
        if failed > 0 {
            return Err(format!("{failed} service(s) did not start"));
        }
        Ok(())
    }

    fn stop(&self, target: Option<&str>, out: &mut dyn Write) -> Result<(), String> {
        let pairs = match target {
            Some(target) => self.ordered(target, true)?,
            None => self
                .config
                .repos
                .iter()
                .flat_map(|(repo, dir)| {
                    dir.services
                        .keys()
                        .map(move |service| (repo.clone(), service.clone()))
                })
                .collect(),
        };
        let mut stopped = 0;
        for (repo, service) in pairs {
            let target = self.target(&repo, &service);
            if !self.runner.is_running(&target) {
                continue;
            }
            self.runner
                .stop(&target)
                .map_err(|error| format!("{repo}/{service}: {error}"))?;
            say(out, &format!("stopped {repo}/{service}"))?;
            stopped += 1;
        }
        if stopped == 0 {
            say(out, "nothing was running")?;
        }
        Ok(())
    }

    fn status(&self, out: &mut dyn Write) -> Result<(), String> {
        say(
            out,
            &format!("{}  workspace {}", self.config.session, self.branch),
        )?;
        let width = self
            .config
            .repos
            .values()
            .flat_map(|dir| dir.services.keys())
            .chain(self.config.shared_services.keys())
            .map(String::len)
            .max()
            .unwrap_or(0);
        for (repo, dir) in &self.config.repos {
            if dir.services.is_empty() {
                continue;
            }
            say(out, &format!("\n{repo}"))?;
            for (name, service) in &dir.services {
                let target = self.target(repo, name);
                let holder = self.runner.holder_name(&target);
                let holders = self.runner.holders();
                let state = if holders.holder_alive(&holder) {
                    "running"
                } else if holders.crash_info(&holder).is_some_and(|info| info.crashed) {
                    "crashed"
                } else {
                    "stopped"
                };
                let mut line = format!("  {name:width$}  {state:8}");
                if let Some(port) = self.runner.port(&self.config, &target) {
                    line.push_str(&format!("  :{port}"));
                }
                let mode = self.runner.mode(repo, name, service);
                if !mode.is_empty() {
                    line.push_str(&format!("  {mode}"));
                }
                say(out, line.trim_end())?;
            }
        }
        if !self.config.shared_services.is_empty() {
            let running = self.runner.shared_running();
            say(out, "\nshared (all workspaces)")?;
            for name in self.config.shared_services.keys() {
                let state = if running.contains(name) {
                    "running"
                } else {
                    "stopped"
                };
                let port = self.runner.shared_host_port(name);
                say(out, &format!("  {name:width$}  {state:8}  :{port}"))?;
            }
        }
        Ok(())
    }

    fn service(&self, entry: &str) -> Result<ServiceTarget, String> {
        let (repo, service) = self.config.find_service_entry(entry)?;
        Ok(self.target(&repo, &service))
    }

    /// A running service's scrollback, or what a crashed one left.
    fn logs(&self, entry: &str, out: &mut dyn Write) -> Result<(), String> {
        let target = self.service(entry)?;
        let holder = self.runner.holder_name(&target);
        let holders = self.runner.holders();
        let bytes = if holders.holder_alive(&holder) {
            pom_ptyhost::snapshot(holders, &holder, std::time::Duration::from_secs(5))
                .map_err(|error| error.to_string())?
        } else if let Some(info) = holders.crash_info(&holder) {
            info.output
        } else {
            return Err(format!("{entry} is not running"));
        };
        out.write_all(&bytes).map_err(|error| error.to_string())
    }

    fn attach(&self, entry: &str) -> Result<(), String> {
        let target = self.service(entry)?;
        let holder = self.runner.holder_name(&target);
        if !self.runner.holders().holder_alive(&holder) {
            return Err(format!("{entry} is not running"));
        }
        let args = ["pom", "pty", "attach", &holder].map(str::to_string);
        match pom_ptyhost::cli::run(&args) {
            Some(0) => Ok(()),
            _ => Err(format!("could not attach to {entry}")),
        }
    }

    fn url(&self, entry: &str, out: &mut dyn Write) -> Result<(), String> {
        let target = self.service(entry)?;
        let has_port = self
            .config
            .repos
            .get(&target.repo)
            .and_then(|dir| dir.services.get(&target.service))
            .is_some_and(|service| service.has_port());
        if !has_port {
            return Err(format!("{entry} has no port"));
        }
        match self.runner.url(&self.config, &target) {
            Some(url) => say(out, &url),
            None => Err(format!("{entry} has no port yet; start it first")),
        }
    }
}

/// `-w` names the workspace; otherwise the one whose folder holds `cwd`, else the main one.
fn pick_workspace(
    project: &Project,
    explicit: Option<&str>,
    cwd: &Path,
) -> Result<(String, bool), String> {
    if let Some(branch) = explicit {
        return project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
            .map(|workspace| (workspace.branch.clone(), workspace.is_main))
            .ok_or_else(|| {
                let known: Vec<&str> = project
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.branch.as_str())
                    .collect();
                format!("no workspace {branch} (have: {})", known.join(", "))
            });
    }
    let cwd = canonical(cwd);
    let inside = project
        .workspaces
        .iter()
        .filter(|workspace| workspace.path != project.root)
        .filter(|workspace| cwd.starts_with(canonical(&workspace.path)))
        .max_by_key(|workspace| workspace.path.as_os_str().len());
    if let Some(workspace) = inside {
        return Ok((workspace.branch.clone(), workspace.is_main));
    }
    Ok((project.branch().to_string(), true))
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn say(out: &mut dyn Write, line: &str) -> Result<(), String> {
    writeln!(out, "{line}").map_err(|error| error.to_string())
}

fn ports(state: &StateDir, out: &mut dyn Write) -> Result<(), String> {
    let leases = pom_ports::scan_leases(&state.path("ports.d"));
    if leases.is_empty() {
        return say(out, "no ports leased");
    }
    say(out, "PORT   STATE     SESSION         SERVICE")?;
    let mut leases = leases;
    leases.sort_by_key(|lease| lease.port);
    for lease in leases {
        let service = lease.key.replace('\u{1f}', " ");
        say(
            out,
            &format!(
                "{:<6} {:<9} {:<15} {service}",
                lease.port,
                format!("{:?}", lease.state).to_lowercase(),
                lease.session
            ),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Invocation, String> {
        let args: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        parse(&args)
    }

    #[test]
    fn commands_and_flags_parse() {
        assert_eq!(
            parsed("-w feat/x start api").map(|i| (i.workspace, i.command)),
            Ok((Some("feat/x".into()), Command::Start("api".into())))
        );
        assert_eq!(parsed("stop").map(|i| i.command), Ok(Command::Stop(None)));
        assert_eq!(
            parsed("logs api/web -c /p/pom.yml").map(|i| (i.config, i.command)),
            Ok((
                Some(PathBuf::from("/p/pom.yml")),
                Command::Logs("api/web".into())
            ))
        );
        assert_eq!(parsed("start --help").map(|i| i.command), Ok(Command::Help));
        assert!(parsed("start").is_err());
        assert!(parsed("status extra").is_err());
        assert!(parsed("frobnicate").is_err());
        assert!(parsed("--nope status").is_err());
    }
}
