//! `pom`: a workspace's services from a terminal. It drives the same holders, leases and compose project as
//! the app, so a service started here shows up in the app's Services panel and the other way round.

mod agent;
mod args;
mod completion;
mod config;
mod db;
mod env;
mod lifecycle;
mod machine;
mod marks;
mod modules;
mod onboard;
mod proxy;
mod run;
mod workspaces;

use std::io::Write;
use std::path::{Path, PathBuf};

use config::ConfigCommand;
use db::DbCommand;
use env::EnvCommand;
use lifecycle::LifecycleCommand;
use onboard::OnboardCommand;
use pom_config::{Config, DepGraph};
use pom_core::Project;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};
use run::RunCommand;
use workspaces::WorkspaceCommand;

pub const USAGE: &str = "usage: pom [-w <workspace>] [-c <pom.yml>] <command> [args]

services
  start <target> [--wait [--timeout 120s]]
                     start a service, a repo's services, or a `workspaces:` group; --wait blocks until
                     each is ready (its healthcheck, else its port): exit 0 ready, 2 timeout, 3 stopped
  stop [target]      stop them; with no target, every service of the workspace
  restart <target>   stop, then start
  status [-o json]   services of the workspace and whether they run (json: up, ready, healthy)
  logs <service> [--since <mark>] [--raw] [-o json]
                     recent output of a service, or only what it printed since a mark
  logs --mark <name> record where every service's output has got to
  mark <name> [-w b] one mark for a test step: log offsets and query counters
  queue wait-idle <service> [--timeout 60s] [-o json]
                     wait until the service's Sidekiq/BullMQ queue (pom.yml `queue:`) has no work:
                     exit 0 idle, 2 still busy
  queue counts <service> [-o json]   the queue's waiting, active, due and delayed jobs
  attach <service>   attach this terminal to a running service (detach: close the terminal)
  ports              every leased port
  url <service>      where a service with a port listens, directly and through the dev proxy
  proxy fault add <service> [--path /x] [--status 503] [--delay 2s] [--rate 0.3] [--ttl 10m]
                     make the dev proxy fail or slow down this workspace's requests to a service
  proxy fault ls [--all] [-o json] | rm <id> | clear
                     the fault rules in force; remove one, or every one of the workspace
  proxy              serve the dev proxy and webhook relay for every project (`pom start` runs
                     one in the background when neither the app nor another proxy does)
  run <name|\"cmd\"> [repo]
                     a command from the config (or any shell command) in the repo's worktree,
                     with the workspace's env
  commands           the commands the config defines, per repo
  refresh            stop every running service of the project (frees their ports)
  release [--disk] [--worktrees] [--yes]
                     stop services and remove the shared containers; --disk also drops their
                     volumes (database data), --worktrees deletes the branch workspaces

workspaces
  ws create <branch> [--repos a,b] [--env name] [--no-seed] [--fresh-db] [--from-stage n]
                     new workspace: worktrees, databases, env files, setup and seed;
                     --fresh-db starts every database empty instead of copying main's
  ws delete <branch> [--from-stage n]
                     stop it and remove its worktrees, databases and folder; the local
                     branch goes too only when it is pushed or merged
  ws rename <branch> [name]   set or clear the workspace's display name
  ws list            workspaces, their repos and running services
  get workspaces [-o json]    each workspace's readiness and phase
  describe workspace <branch> [-o json]
                     its services, ports and repos missing from it
  apply [branch] [--yes]      check out repos the config added but a workspace lacks
  prepare-main [--no-seed]    reset main's databases, migrate and seed (new workspaces copy them)
  db create|drop|reset [branch]
                     the workspace's databases in the shared Postgres
  db clean [--dry-run] [--yes]
                     drop this project's databases (and snapshots) no workspace uses
  db snapshot <name> [-w b] [--replace] [-o json]
                     copy every database of the workspace into snapshot <name>
  db restore <name> [-w b] [--no-restart] [--main] [-o json]
                     put the snapshot back: stops the workspace's services, restarts them after
  db snapshots [-w b] [-o json]      the workspace's snapshots and their sizes
  db snapshot drop <name> [-w b]     drop a snapshot
  db baseline [-w b] [-o json]       run the branch's migrations, then save ws__baseline again
  db mark <name> [-w b]              save the workspace's query counters (pom mark saves logs too)
  db stats --since <mark> [--repeated n] [-w b] [-o json]
                     the queries the workspace ran since the mark: totals, slowest, repeated (N+1)
  db reseed [-w b] [--from main__baseline | --snapshot <name>] [--main] [-o json]
                     replace the workspace's data, migrate, save ws__baseline; services restart

config
  config path        the config file in use
  config edit        open it in $EDITOR, then check it still loads
  config explain [repo | repo/service] [--branch b] [--env name] [-o json]
                     what the config resolves to and where each value comes from
  config normalize [--dry-run]
                     drop removed keys, migrate old tokens
  config export [--secrets] [-o file]
                     the config as YAML; --secrets seals the session's secrets with it
                     into a bundle (password read from stdin)
  config import <file> [--config-only|--secrets-only]
                     replace pom.yml with a YAML file or bundle (old one kept as pom.yml.bak)
                     and store the bundle's secrets
  env ls <repo[/service]> [--branch b] [--env name] [--show-secrets]
  env get <repo[/service]> <KEY>
  env set <repo> KEY=VALUE...
  env unset <repo> KEY...
  doctor             what keeps the project from running, and how to fix it

projects and machine
  init [name] [--ai]          a new project from the git repo you are in (--ai: Claude finishes pom.yml)
  onboard [session] [--new name --repo path... [--branch b]] [--no-ai]
                     Claude writes a runnable pom.yml with you, in this terminal
  ps [--watch]       CPU and memory of every holder Pomelo started
  disk               disk used by the registered projects
  modules [list|prune|clear]
                     the shared node_modules store: its copies, drop unused ones, or empty it
  mcp [--branch b]   MCP server on stdio for a coding agent working in this workspace

agents (one workspace at a time; --json for stable output)
  agent ls [workspace] [--all-workspaces]
                     the workspace's agent sessions: role, state, turn, holder
  agent start [workspace] [--role r] [--fresh] [--trust] [--prompt text | --prompt-file f]
              [--system-prompt-file f] [--tools t] [--allowed-tools t] [--disallowed-tools t]
              [--permission-mode m] [--model m] [--extra-mcp-config f]
                     start (or reuse) a session; --fresh starts role r on a new conversation;
                     --trust says yes to the agent's prompt to trust the folder (else exit 7)
  agent read <workspace/role> [--turn n | --since n] [--full]
                     what the agent did in a turn (the last by default), from its transcript
  agent watch [workspace] [--from-start] [--timeout d]
                     stream the workspace's agent events as NDJSON
  agent stop <workspace/role>
                     end a session
  agent send <workspace/role> <text | --file f> [--queue] [--take]
                     submit one turn (refused unless idle; --queue waits, --take takes the session over)
  agent wait <workspace/role> [--until idle|awaiting_input|turn-end] [--turn n] [--timeout d]
                     exit 0 reached, 2 timeout, 3 awaiting input, 4 died
  agent ask <workspace/role> <text | --file f> [--take] [--timeout d]
                     send, wait for the turn to end, and read it
  agent interrupt <workspace/role>
                     stop the current turn
  agent approve|deny <workspace/role> <request> [--always]
                     answer a pending approval of the workspace policy
  agent takeover [workspace] --session-id <id> [--settings f] [--mcp-config f]
  agent release <workspace/role>
                     a person takes a session over, and hands it back
  completion bash|zsh|fish
  version

The workspace is the one the current directory is in, else the main one; -w picks another.
A service is `name` or `repo/name`.";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Start(StartArgs),
    Stop(Option<String>),
    Restart(String),
    Status { json: bool },
    Logs(marks::LogsArgs),
    Mark(marks::MarkArgs),
    Queue(QueueArgs),
    ProxyFault(proxy::FaultCommand),
    Attach(String),
    Ports,
    Url(String),
    Proxy,
    Workspace(WorkspaceCommand),
    Config(ConfigCommand),
    Env(EnvCommand),
    Db(DbCommand),
    Run(RunCommand),
    Commands,
    Lifecycle(LifecycleCommand),
    Onboard(OnboardCommand),
    Ps { watch: bool },
    Disk,
    Modules(modules::ModulesCommand),
    Completion(String),
    Doctor,
    Agent(agent::AgentCommand),
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
            Command::Proxy => proxy::serve(&StateDir::from_env(), out),
            Command::Config(ref command) if !command.needs_session() => {
                find_config(invocation.config.as_deref(), cwd)
                    .and_then(|path| config::execute(command, &path, out))
            }
            Command::Env(ref command) if !command.needs_session() => {
                find_config(invocation.config.as_deref(), cwd)
                    .and_then(|path| env::edit(command, &path, out))
            }
            Command::Onboard(ref command) => onboard::execute(command, cwd, out),
            Command::Ps { watch } => machine::ps(watch, out),
            Command::Disk => machine::disk(&StateDir::from_env(), out),
            Command::Modules(command) => modules::execute(command, &StateDir::from_env(), out),
            Command::Completion(ref shell) => completion::print(shell, out),
            Command::Agent(ref command) => {
                return agent::execute(command, &invocation, cwd, out, err);
            }
            Command::Queue(ref queue) => {
                return match Session::open(&invocation, cwd) {
                    Ok(session) => session.queue_command(queue, out, err),
                    Err(message) => {
                        report(err, &format!("error: {message}"));
                        1
                    }
                };
            }
            Command::Start(ref start) if start.wait => {
                return match Session::open(&invocation, cwd) {
                    Ok(session) => session.start_and_wait(start, out, err),
                    Err(message) => {
                        report(err, &format!("error: {message}"));
                        1
                    }
                };
            }
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

#[derive(Debug, Default, PartialEq, Eq)]
struct StartArgs {
    target: String,
    wait: bool,
    timeout: Option<std::time::Duration>,
}

/// Exit codes of `pom start --wait`.
const WAIT_TIMED_OUT: i32 = 2;
const WAIT_CRASHED: i32 = 3;
const DEFAULT_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Debug, Default, PartialEq, Eq)]
struct QueueArgs {
    service: String,
    /// Wait until idle; otherwise print the counts once.
    wait: bool,
    timeout: Option<std::time::Duration>,
    json: bool,
}

const DEFAULT_QUEUE_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

fn parse_queue(rest: &[&str]) -> Result<QueueArgs, String> {
    let (verb, rest) = rest
        .split_first()
        .ok_or("queue needs a subcommand (wait-idle, counts)")?;
    let wait = match *verb {
        "wait-idle" => true,
        "counts" => false,
        other => {
            return Err(format!(
                "unknown queue subcommand {other} (wait-idle, counts)"
            ))
        }
    };
    let args = args::Args::parse(rest, &["--timeout", "-o", "--output"])?;
    args.allow(&["--timeout", "-o", "--output"])?;
    let [service] = args.positional.as_slice() else {
        return Err(format!("queue {verb} needs one service"));
    };
    let timeout = args
        .value(&["--timeout"])
        .map(|text| agent::parse_duration(&text))
        .transpose()?;
    if timeout.is_some() && !wait {
        return Err("--timeout goes with wait-idle".into());
    }
    Ok(QueueArgs {
        service: service.clone(),
        wait,
        timeout,
        json: args.json()?,
    })
}

fn parse_start(rest: &[&str]) -> Result<StartArgs, String> {
    let args = args::Args::parse(rest, &["--timeout"])?;
    args.allow(&["--wait", "--timeout"])?;
    let [target] = args.positional.as_slice() else {
        return Err("start needs one target".into());
    };
    let wait = args.has("--wait");
    let timeout = args
        .value(&["--timeout"])
        .map(|text| agent::parse_duration(&text))
        .transpose()?;
    if timeout.is_some() && !wait {
        return Err("--timeout goes with --wait".into());
    }
    Ok(StartArgs {
        target: target.clone(),
        wait,
        timeout,
    })
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
        // No subcommand takes -c, so the config path works after any command.
        if matches!(arg.as_str(), "-c" | "--config") {
            config = Some(PathBuf::from(iter.next().ok_or("-c needs a path")?));
            continue;
        }
        // Workspace commands have flags of their own.
        let help = matches!(arg.as_str(), "-h" | "--help");
        if !help
            && matches!(
                words.first(),
                Some(
                    &("ws"
                        | "workspace"
                        | "prepare-main"
                        | "config"
                        | "env"
                        | "db"
                        | "run"
                        | "get"
                        | "describe"
                        | "apply"
                        | "release"
                        | "ps"
                        | "init"
                        | "onboard"
                        | "agent"
                        | "start"
                        | "status"
                        | "logs"
                        | "mark"
                        | "queue"
                        | "proxy")
                )
            )
        {
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
        "start" => Command::Start(parse_start(rest)?),
        "stop" => match rest {
            [] => Command::Stop(None),
            _ => Command::Stop(Some(one("target")?)),
        },
        "restart" => Command::Restart(one("a target")?),
        "status" => {
            let args = args::Args::parse(rest, &["-o", "--output"])?;
            args.allow(&["-o", "--output"])?;
            args.at_most(0, "status")?;
            Command::Status { json: args.json()? }
        }
        "logs" => Command::Logs(marks::parse_logs(rest)?),
        "mark" => Command::Mark(marks::parse_mark(rest)?),
        "queue" => Command::Queue(parse_queue(rest)?),
        "attach" => Command::Attach(one("a service")?),
        "ports" => none().map(|_| Command::Ports)?,
        "url" => Command::Url(one("a service")?),
        "proxy" if rest.is_empty() => Command::Proxy,
        "proxy" => Command::ProxyFault(proxy::parse_fault(rest)?),
        "ws" | "workspace" => Command::Workspace(workspaces::parse(rest, false)?),
        "prepare-main" => Command::Workspace(workspaces::parse(rest, true)?),
        "config" => Command::Config(config::parse(rest)?),
        "env" => Command::Env(env::parse(rest)?),
        "db" => Command::Db(db::parse(rest)?),
        "run" => Command::Run(run::parse(rest)?),
        "commands" | "shortcuts" => none().map(|_| Command::Commands)?,
        "refresh" | "release" | "get" | "describe" | "apply" => {
            Command::Lifecycle(lifecycle::parse(name, rest)?)
        }
        "init" | "onboard" => Command::Onboard(onboard::parse(name, rest)?),
        "ps" => match rest {
            [] => Command::Ps { watch: false },
            ["-w" | "--watch"] => Command::Ps { watch: true },
            _ => return Err("usage: pom ps [--watch]".into()),
        },
        "disk" => none().map(|_| Command::Disk)?,
        "modules" => Command::Modules(modules::parse(rest)?),
        "completion" => Command::Completion(one("a shell (bash, zsh or fish)")?),
        "doctor" => none().map(|_| Command::Doctor)?,
        "agent" => {
            let (command, named) = agent::parse(rest)?;
            if workspace.is_none() {
                workspace = named;
            }
            Command::Agent(command)
        }
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

pub(crate) fn doctor(
    explicit: Option<&Path>,
    cwd: &Path,
    out: &mut dyn Write,
) -> Result<(), String> {
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
            Command::Start(start) => self.start(&start.target, out, err),
            Command::Stop(target) => self.stop(target.as_deref(), out),
            Command::Restart(target) => {
                self.stop(Some(target), out)?;
                self.start(target, out, err)
            }
            Command::Status { json: false } => self.status(out),
            Command::Status { json: true } => self.status_json(out),
            Command::Logs(args) => self.logs_command(args, out),
            Command::Mark(args) => self.mark_command(args, out),
            Command::ProxyFault(command) => self.fault_command(command, out),
            Command::Attach(service) => self.attach(service),
            Command::Url(service) => self.url(service, out),
            Command::Workspace(command) => self.workspace(command, out),
            Command::Config(command) => self.config_command(command, out),
            Command::Env(command) => self.env_command(command, out),
            Command::Db(command) => self.db_command(command, out),
            Command::Run(request) => self.run_command(request),
            Command::Commands => self.list_commands(out),
            Command::Lifecycle(command) => self.lifecycle_command(command, out),
            Command::Ports
            | Command::Proxy
            | Command::Onboard(_)
            | Command::Ps { .. }
            | Command::Disk
            | Command::Modules(_)
            | Command::Completion(_)
            | Command::Doctor
            | Command::Agent(_)
            | Command::Queue(_)
            | Command::Version
            | Command::Help => Ok(()),
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
        if let Err(error) = proxy::ensure_running(out) {
            say(err, &format!("dev proxy: {error}"))?;
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

    /// `start`, then waits until every started service is ready: 0 ready, 2 timed out, 3 one stopped.
    fn start_and_wait(&self, start: &StartArgs, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
        if let Err(message) = self.start(&start.target, out, err) {
            report(err, &format!("error: {message}"));
            return 1;
        }
        let targets: Vec<ServiceTarget> = match self.ordered(&start.target, false) {
            Ok(pairs) => pairs
                .iter()
                .map(|(repo, service)| self.target(repo, service))
                .collect(),
            Err(message) => {
                report(err, &format!("error: {message}"));
                return 1;
            }
        };
        let timeout = start.timeout.unwrap_or(DEFAULT_WAIT);
        let describe = |health: &pom_services::ServiceHealth| {
            let state = if health.ready {
                "ready"
            } else if health.up {
                "not ready"
            } else {
                "stopped"
            };
            let port = health
                .port
                .map(|port| format!("  :{port}"))
                .unwrap_or_default();
            format!("  {}/{}  {state}{port}", health.repo, health.service)
        };
        let lines = |services: &[pom_services::ServiceHealth], out: &mut dyn Write| {
            for health in services {
                report(out, &describe(health));
            }
        };
        match self.runner.wait_ready(&self.config, &targets, timeout) {
            pom_services::WaitOutcome::Ready(services) => {
                lines(&services, out);
                report(out, "ready");
                0
            }
            pom_services::WaitOutcome::TimedOut(services) => {
                lines(&services, err);
                report(
                    err,
                    &format!("error: not ready after {}s", timeout.as_secs()),
                );
                WAIT_TIMED_OUT
            }
            pom_services::WaitOutcome::Crashed {
                service,
                tail,
                services,
            } => {
                lines(&services, err);
                report(err, &format!("error: {service} stopped while starting"));
                if !tail.is_empty() {
                    report(err, &tail);
                }
                WAIT_CRASHED
            }
        }
    }

    /// `queue counts` or `queue wait-idle`: 0 idle (or counted), 2 still busy when the timeout passed.
    fn queue_command(&self, queue: &QueueArgs, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
        let target = match self.service(&queue.service) {
            Ok(target) => target,
            Err(message) => {
                report(err, &format!("error: {message}"));
                return 1;
            }
        };
        let print = |counts: &pom_services::QueueCounts, idle: bool, out: &mut dyn Write| {
            if queue.json {
                let document = serde_json::json!({
                    "schema": "pom.queue/v1",
                    "service": format!("{}/{}", target.repo, target.service),
                    "idle": idle,
                    "total": counts.total(),
                    "counts": counts,
                });
                report(out, &document.to_string());
            } else {
                let waiting: Vec<String> = counts
                    .waiting
                    .iter()
                    .map(|(name, count)| format!("{name}={count}"))
                    .collect();
                report(
                    out,
                    &format!(
                        "{}/{}: waiting [{}], active {}, due {}, delayed {}",
                        target.repo,
                        target.service,
                        waiting.join(" "),
                        counts.active,
                        counts.due,
                        counts.delayed
                    ),
                );
            }
        };
        if !queue.wait {
            return match self.runner.queue_counts(&self.config, &target) {
                Ok(counts) => {
                    print(&counts, counts.total() == 0, out);
                    0
                }
                Err(message) => {
                    report(err, &format!("error: {message}"));
                    1
                }
            };
        }
        let timeout = queue.timeout.unwrap_or(DEFAULT_QUEUE_WAIT);
        match self.runner.wait_queue_idle(&self.config, &target, timeout) {
            Ok(Ok(counts)) => {
                print(&counts, true, out);
                0
            }
            Ok(Err(counts)) => {
                print(&counts, false, err);
                report(
                    err,
                    &format!("error: still busy after {}s", timeout.as_secs()),
                );
                WAIT_TIMED_OUT
            }
            Err(message) => {
                report(err, &format!("error: {message}"));
                1
            }
        }
    }

    /// `status -o json`: every repo service's up / ready / healthy, and the shared services.
    fn status_json(&self, out: &mut dyn Write) -> Result<(), String> {
        let mut services = Vec::new();
        for (repo, dir) in &self.config.repos {
            for name in dir.services.keys() {
                services.push(
                    self.runner
                        .service_health(&self.config, &self.target(repo, name)),
                );
            }
        }
        let running = self.runner.shared_running();
        let shared: Vec<serde_json::Value> = self
            .config
            .shared_services
            .keys()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "up": running.contains(name),
                    "port": self.runner.shared_host_port(name),
                })
            })
            .collect();
        let document = serde_json::json!({
            "schema": "pom.status/v1",
            "workspace": self.branch,
            "services": services,
            "shared": shared,
        });
        say(out, &document.to_string())
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
        let direct = self
            .runner
            .url(&self.config, &target)
            .ok_or_else(|| format!("{entry} has no port yet; start it first"))?;
        let proxied = proxy::service_url(&self.config, &self.branch, &target.repo, &target.service);
        let status = if pom_proxy::listening(pom_proxy::Ports::from_env().proxy) {
            ""
        } else {
            "  (proxy not running: start the app or `pom proxy`)"
        };
        say(out, &format!("direct  {direct}"))?;
        say(out, &format!("proxy   {proxied}{status}"))
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
            Ok((
                Some("feat/x".into()),
                Command::Start(StartArgs {
                    target: "api".into(),
                    ..StartArgs::default()
                })
            ))
        );
        assert_eq!(parsed("stop").map(|i| i.command), Ok(Command::Stop(None)));
        assert_eq!(
            parsed("logs api/web -c /p/pom.yml").map(|i| (i.config, i.command)),
            Ok((
                Some(PathBuf::from("/p/pom.yml")),
                Command::Logs(marks::LogsArgs {
                    service: Some("api/web".into()),
                    ..marks::LogsArgs::default()
                })
            ))
        );
        assert_eq!(parsed("start --help").map(|i| i.command), Ok(Command::Help));
        assert!(parsed("start").is_err());
        assert!(parsed("status extra").is_err());
        assert_eq!(
            parsed("start api --wait --timeout 30s").map(|i| i.command),
            Ok(Command::Start(StartArgs {
                target: "api".into(),
                wait: true,
                timeout: Some(std::time::Duration::from_secs(30)),
            }))
        );
        assert!(
            parsed("start api --timeout 30s").is_err(),
            "--timeout needs --wait"
        );
        assert_eq!(
            parsed("queue wait-idle api/worker --timeout 2m -o json").map(|i| i.command),
            Ok(Command::Queue(QueueArgs {
                service: "api/worker".into(),
                wait: true,
                timeout: Some(std::time::Duration::from_secs(120)),
                json: true,
            }))
        );
        assert!(parsed("queue counts api/worker --timeout 1s").is_err());
        assert!(parsed("queue drain api/worker").is_err());
        assert_eq!(
            parsed("status -o json").map(|i| i.command),
            Ok(Command::Status { json: true })
        );
        assert!(parsed("frobnicate").is_err());
        assert!(parsed("--nope status").is_err());
    }
}
