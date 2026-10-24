//! `pom`: a workspace's services from a terminal. It drives the same holders, leases and compose project as
//! the app, so a service started here shows up in the app's Services panel and the other way round.

mod agent;
mod args;
mod completion;
mod config;
mod db;
mod env;
mod gc;
mod lifecycle;
mod machine;
mod marks;
mod modules;
mod onboard;
mod proxy;
mod push;
mod remote;
mod run;
mod self_update;
mod server;
mod transfer;
mod workspaces;

use std::io::Write;
use std::path::{Path, PathBuf};

use config::ConfigCommand;
use db::DbCommand;
use env::EnvCommand;
use lifecycle::LifecycleCommand;
use onboard::OnboardCommand;
use pom_config::Config;
use pom_core::Project;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner, ServiceTarget};
use run::RunCommand;
use workspaces::WorkspaceCommand;

pub const USAGE: &str = "usage: pom [-H <host>] [-w <workspace>] [-c <pom.yml>] <command> [args]

services
  start <target> [--wait [--timeout 120s]]
                     start a service, a repo's services, or a `workspaces:` group; --wait blocks until
                     each is ready (its healthcheck, else its port) and the shared services answer
                     on their host port: exit 0 ready, 2 timeout, 3 stopped
  stop [target]      stop them; with no target, every service of the workspace
  restart <target>   stop, then start
  status [-o json]   services of the workspace and whether they run (json: up, ready, healthy;
                     shared: up, ready)
  logs <service> [--since <mark>] [--raw] [-o json]
                     recent output of a service, or only what it printed since a mark
  logs --mark <name> record where every service's output has got to
  mark <name> [-w b] one mark for a test step: log offsets and query counters
  queue wait-idle <service> [--timeout 60s] [--redis <name>] [-o json]
                     wait until the service's Sidekiq/Bull/BullMQ queue (pom.yml `queue:`) has no
                     waiting, active, due or prioritized job (crons and later delays do not count):
                     exit 0 idle, 2 still busy
  queue counts <service> [--redis <name>] [-o json]
                     the queue's jobs by kind, and the shared Redis it was read from
  attach <service>   attach this terminal to a running service (detach: close the terminal)
  ports              every leased port
  url <service>      where a service with a port listens, directly and through the dev proxy
  proxy fault add <service> [--path /x] [--status 503] [--delay 2s] [--rate 0.3] [--ttl 10m] [--json]
                     make the dev proxy fail or slow down this workspace's requests to a service
  proxy fault ls [--all] [--json] | rm <id> [--json] | clear [--json]
                     the fault rules in force; remove one, or every one of the workspace
                     (add, rm and clear return once the running proxy applies the change)
  proxy              serve the dev proxy and webhook relay for every project (`pom start` runs
                     one in the background when neither the app nor another proxy does)
  proxy config [-o json] | set <key> <value> | reset <key>
                     how other machines reach the proxy: bind, domain, public_scheme,
                     public_port, trusted_proxies, token_env, allow_unauthenticated
                     (settings.json `proxy`); a new domain, scheme or port rewrites every
                     workspace's env files
  proxy token [new | rm]
                     the token other machines present when proxy.bind is not loopback
                     ($POM_PROXY_TOKEN, or one pom keeps encrypted); without one such a
                     bind refuses to start
  run [--profile test|dev] <name|\"cmd\"> [repo]
                     a command from the config (or any shell command) in the repo's worktree,
                     with the workspace's env; the test profile layers the repo's .env.test on
                     top (connection values keep the workspace's), and `run test` uses it when
                     the repo has a `test:` block
  run --explain-env [--profile test] [--json] <name|\"cmd\"> [repo]
                     where each variable the command would get comes from, secrets masked
  commands [-o json] the named commands of every repo (lifecycle.commands, keyed tasks, the test
                     block, service tasks); json: pom.commands/v1 with name, repo, command, source
  refresh            stop every running service of the project (frees their ports)
  release [--disk] [--worktrees] [--yes]
                     stop services and remove the shared containers; --disk also drops their
                     volumes (database data), --worktrees deletes the branch workspaces

workspaces
  ws create <branch> [--repos a,b] [--env name] [--no-seed] [--fresh-db] [--from-stage n]
                     new workspace: worktrees, databases, env files, setup and seed;
                     --fresh-db starts every database empty instead of copying main's
  ws delete <branch> [--from-stage n]
                     stop every service, agent and terminal of it, then remove its worktrees,
                     databases and folder; the local branch goes too only when it is pushed
                     or merged
  ws rename <branch> [name]   set or clear the workspace's display name
  ws export <branch> [-o file.pomws | --to <host>] [--no-db] [--json]
                     the workspace as one archive another machine can open as it was: commits
                     origin lacks, uncommitted and untracked files (not gitignored ones), its
                     databases (pg_dump) and the secrets its config uses, sealed with a
                     passphrase ($POM_BUNDLE_PASSPHRASE, else asked); -o - writes to stdout;
                     --to streams it over ssh into `pom ws import -` on a host of `pom host`
                     (secrets sealed with a one-time passphrase sent ahead of it)
  ws import <file.pomws | -> [--project name] [--branch new] [--json]
                     open an exported workspace here: worktrees at the same commits with the
                     same changes, databases restored, fresh ports, env files rewritten, setup
                     run; a stopped import resumes when run again
                     with --json (or -o json) stdout is one document, an error with its code on
                     failure, and stderr carries progress as one JSON event per line; the same
                     goes for project export and import
  ws gc [--merged] [--closed] [--dry-run] [--yes] [--force] [--json]
                     delete the workspaces whose pull requests (in every repo that has one) are
                     merged, or closed with --closed; asks for each on a terminal, else needs
                     --yes; a workspace with uncommitted or unpushed work is kept unless --force
  ws ls [--json]     workspaces, their repos, running services, and whether each is sleeping or
                     queued (ws list works too)
  ws sleep <branch> [--force] [--json]
                     stop its services and agent sessions, keeping its databases, worktrees and
                     ports; refused while an agent is working or waits for an approval, unless --force
  ws wake <branch> [--json]
                     start what ran when it went to sleep (the dev proxy also wakes a sleeping
                     workspace on a request); with settings.json workspaces.max_running, starting
                     or waking one more first sleeps the least recently used idle workspace, or
                     queues it until one sleeps when the others' agents are at work
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
  doctor [--quick] [--json]
                     what keeps the project from running, and how to fix it; on a machine that
                     runs `pom server`, how the server stands; checks run side by side, each with
                     a timeout, and --json reports how long each took; --quick runs only the
                     cheap local ones (config, tools on the PATH, the pom on the PATH, orphan
                     holders) in under 2 s
  github webhook setup [--rotate] [--json]
                     the payload URL for a GitHub webhook on the relay's /_pom/github, and the
                     project's webhook secret (made once and shown once; --rotate replaces it)
  sync status [--limit n] [--json]
                     recent GitHub push deliveries and what each did to the workspaces
                     (pulled, notified, left alone because of local changes); see sync.on_push

projects and machine
  project ls [--json]         the projects this machine knows: name, folder, whether its pom.yml is there
  project export [-o file.pomproj | --to <host>] [--json]
                     pom.yml, every secret of the project (sealed with a passphrase) and each
                     repo's origin, for setting the project up on another machine
  project import <file.pomproj | -> [--dir path] [--json]
                     write pom.yml (into ~/pom/<project> unless --dir), store the secrets under
                     this machine's key and clone the repos on main
  init [name] [--ai]          a new project from the git repo you are in (--ai: Claude finishes pom.yml)
  onboard [session] [--new name --repo path... [--branch b]] [--no-ai]
                     Claude writes a runnable pom.yml with you, in this terminal
  ps [--watch | --json] [--orphans [--kill]]
                     CPU and memory of every holder Pomelo started, and of each workspace together
                     (from its cgroup when it has limits on Linux, with its caps); a holder whose
                     workspace or project is gone is marked orphan; --orphans lists only those,
                     --kill stops them
  disk [--json]      free space where Pomelo writes (home, its state, Docker's data, LVM thin
                     pools; warns past 80% with what to run) and the projects' node_modules
  modules [list|prune|clear]
                     the shared node_modules store: its copies, drop unused ones, or empty it
  mcp [--branch b]   MCP server on stdio for a coding agent working in this workspace

agents (one workspace at a time; --json for stable output)
  agent ls [workspace] [--all-workspaces]
                     the workspace's agent sessions: role, state, reason, running, phase, turn,
                     holder (states and reasons: see driving agents in the docs)
  agent start [workspace] [--role r] [--fresh] [--trust] [--prompt text | --prompt-file f]
              [--system-prompt-file f] [--tools t] [--allowed-tools t] [--disallowed-tools t]
              [--permission-mode m] [--model m] [--extra-mcp-config f]
                     start (or reuse) a session; --fresh starts role r on a new conversation;
                     --trust marks the folder trusted in the agent's own config before it starts
                     (else exit 7 at its trust prompt)
  agent read <workspace/role> [--turn n | --since n] [--full] [--live]
                     what the agent did in a turn (the last by default), from its transcript;
                     --live adds the reply it is still writing
  agent watch [workspace] [--role r] [--live] [--until turn-end] [--from-start] [--timeout d]
                     stream the workspace's agent events as NDJSON; --role keeps one session's,
                     --live adds live_text lines as its reply is written, --until turn-end stops
                     once that session's turn ends (picked as wait does)
  agent stop <workspace/role>
                     end a session
  agent send <workspace/role> <text | --file f> [--queue] [--take | --as-person] [--timeout d]
                     submit one turn (refused while working or pending approval; --queue waits until
                     idle, 30 min unless --timeout; --take takes the session over; --as-person types
                     as the person who drives it, from another client, and leaves the lease theirs)
  agent wait <workspace/role> [--until idle|awaiting_input|turn-end] [--turn n] [--timeout d]
                     exit 0 reached, 2 timeout, 3 awaiting input, 4 died; turn-end waits for exactly
                     turn n, else the running turn, else the one the last send started, else the next
  agent ask <workspace/role> <text | --file f> [--take | --as-person] [--timeout d]
                     send, wait for the turn to end, and read it
  agent interrupt <workspace/role> [--as-person]
                     stop the current turn
  agent allow|deny <workspace/role> [<id> | --id id] [--reason r] [--as-person]
                     answer the permission prompt the session waits on (pending_permission in
                     agent ls --json), the agent's own or its workspace policy's; an id that no
                     longer waits is refused (stale); a dialog the agent drew (answer_by terminal)
                     is only denied, with Esc, never allowed and never while a person drives it
  agent approve <workspace/role> <request> [--always]
                     approve a pending approval of the workspace policy
  agent takeover [workspace] --session-id <id> [--settings f] [--mcp-config f]
  agent release <workspace/role>
                     a person takes a session over, and hands it back
  completion bash|zsh|fish
  self-update [--channel stable|nightly] [--version <v>] [--check]
                     replace this pom (and the Linux app beside it) with the newest build of the
                     channel (settings.json update_channel, else the build's own), or an exact
                     version, after checking its signature; only a pom install.sh installed
  version, --version

remote machines
  host add <name> ssh://user@host[:port] [--pom-path p] [--transport server|ssh] [--json]
                     name a machine pom runs on; ssh uses your own config and keys; the app
                     reads it through `pom server` there unless --transport ssh
  host ls [--json]   the named machines (~/.config/pomelo/hosts.json)
  host rm <name> [--json]
                     forget one
  host test <name> [--json]
                     connect and compare the remote pom's version with this one
  host ports <name> [--json]
                     the ports the app (and pom -H <name> forward) forwards from that machine
  -H <host> <command>
                     run the command on that machine over ssh (--host works too, anywhere on the
                     line): output, input and exit code pass through; exit 69 when ssh cannot
                     connect (with a JSON error object when the command asked for JSON)
  -H <host> forward <port>[:<local>] [--json]
                     open the machine's port <port> on 127.0.0.1 here (the same number when free,
                     else the next free one; exactly <local> when given) until Ctrl-C
  server run [--foreground]
                     start this machine's pom server in the background (the app starts it on
                     demand over ssh; it listens on a private Unix socket, never the network,
                     and exits after server.idle_exit_minutes without a client, 30 by default)
  server status [--json]
                     the running server's pid, version, uptime and connections: exit 3 when none runs
  server stop        stop it
  server proxy [--reconnect]
                     what the app runs over ssh: pass bytes between stdin/stdout and the server,
                     starting it when it is not running; --reconnect never starts one and exits
                     70 when it is gone
  server install-unit [--json]
                     Linux: write ~/.config/systemd/user/pom-server.service (restarted on failure)
                     and enable it, so the server starts with your user manager
  server uninstall-unit [--json]
                     disable and remove that unit

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
    ProxyConfig(proxy::ProxyConfigCommand),
    ProxyToken(proxy::TokenCommand),
    Attach(String),
    Ports,
    Url(String),
    Proxy,
    Workspace(WorkspaceCommand),
    Transfer(transfer::TransferCommand),
    Config(ConfigCommand),
    Env(EnvCommand),
    Db(DbCommand),
    Run(RunCommand),
    Commands { json: bool },
    Lifecycle(LifecycleCommand),
    Onboard(OnboardCommand),
    Ps(machine::PsArgs),
    Disk { json: bool },
    Modules(modules::ModulesCommand),
    Completion(String),
    Doctor { json: bool, quick: bool },
    Agent(agent::AgentCommand),
    Host(remote::HostCommand),
    Server(server::ServerCommand),
    Projects { json: bool },
    SelfUpdate(self_update::SelfUpdate),
    Github(push::WebhookSetup),
    SyncStatus(push::SyncStatus),
    Version,
    Help,
}

#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    workspace: Option<String>,
    config: Option<PathBuf>,
    command: Command,
}

/// [`run`] on the process's own stdout and stderr.
pub fn run_on_std(args: &[String], cwd: &Path) -> i32 {
    let (mut out, mut err) = std_handles();
    run(args, cwd, &mut out, &mut err)
}

/// Unlocked: a stage on another thread prints its warnings to stderr while the command streams, and a lock
/// held for the whole command would leave that thread waiting forever.
fn std_handles() -> (std::io::Stdout, std::io::Stderr) {
    (std::io::stdout(), std::io::stderr())
}

/// Runs `args` (the program name first) as if typed in `cwd`; returns the exit code.
pub fn run(args: &[String], cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let args = match remote::extract_host(args.get(1..).unwrap_or_default()) {
        Ok((Some(host), rest)) if rest.first().map(String::as_str) == Some("forward") => {
            return remote::hold_forward(&host, &rest[1..], out, err)
        }
        Ok((Some(host), rest)) => return remote::forward(&host, &rest, out, err),
        Ok((None, rest)) => rest,
        Err(message) => {
            report(err, &format!("{message}\n\n{USAGE}"));
            return 2;
        }
    };
    let invocation = match parse(&args) {
        Ok(invocation) => Invocation {
            config: invocation.config.map(|path| absolute_in(cwd, &path)),
            ..invocation
        },
        Err(message) if transfer::wants_json_errors(&args) => {
            let document = transfer::error_document(transfer::USAGE, &message);
            if let Err(error) = writeln!(out, "{document}") {
                report(err, &format!("{message} ({error})"));
            }
            return 2;
        }
        Err(message) => {
            report(err, &format!("{message}\n\n{USAGE}"));
            return 2;
        }
    };
    let result = match invocation.command {
        Command::Help => writeln!(out, "{USAGE}").map_err(|error| error.to_string()),
        Command::Version => writeln!(out, "pom {VERSION}").map_err(|error| error.to_string()),
        Command::Ports => ports(&StateDir::from_env(), out),
        Command::Doctor { json, quick } => {
            doctor(invocation.config.as_deref(), cwd, json, quick, out)
        }
        Command::Proxy => proxy::serve(&StateDir::from_env(), out),
        Command::ProxyConfig(ref command) => {
            proxy::config_command(command, &StateDir::from_env(), out)
        }
        Command::ProxyToken(ref command) => {
            proxy::token_command(command, &StateDir::from_env(), out)
        }
        Command::Config(ref command) if !command.needs_session() => {
            find_config(invocation.config.as_deref(), cwd)
                .and_then(|path| config::execute(command, &path, out))
        }
        Command::Env(ref command) if !command.needs_session() => {
            find_config(invocation.config.as_deref(), cwd)
                .and_then(|path| env::edit(command, &path, out))
        }
        Command::Onboard(ref command) => onboard::execute(command, cwd, out),
        Command::Ps(ref request) => machine::ps(request, out),
        Command::Disk { json } => machine::disk(&StateDir::from_env(), json, out),
        Command::Modules(command) => modules::execute(command, &StateDir::from_env(), out),
        Command::Completion(ref shell) => completion::print(shell, out),
        Command::Agent(ref command) => {
            return agent::execute(command, &invocation, cwd, out, err);
        }
        Command::Host(ref command) => return remote::execute(command, out, err),
        Command::Server(ref command) => return server::execute(command, out, err),
        Command::Transfer(ref command) => {
            return transfer::execute(command, &invocation, cwd, out, err);
        }
        Command::Projects { json } => projects(&StateDir::from_env(), json, out),
        Command::SelfUpdate(ref request) => self_update::execute(request, out),
        Command::SyncStatus(ref request) => push::status(&StateDir::from_env(), request, out),
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
        ref command => {
            Session::open(&invocation, cwd).and_then(|session| session.execute(command, out, err))
        }
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
    /// The shared Redis to look in, when the config and env leave it ambiguous.
    redis: Option<String>,
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
    let args = args::Args::parse(rest, &["--timeout", "--redis", "-o", "--output"])?;
    args.allow(&["--timeout", "--redis", "-o", "--output"])?;
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
        redis: args.value(&["--redis"]),
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
                        | "commands"
                        | "shortcuts"
                        | "get"
                        | "describe"
                        | "apply"
                        | "release"
                        | "ps"
                        | "init"
                        | "onboard"
                        | "agent"
                        | "doctor"
                        | "disk"
                        | "start"
                        | "status"
                        | "logs"
                        | "mark"
                        | "queue"
                        | "proxy"
                        | "host"
                        | "server"
                        | "project"
                        | "github"
                        | "self-update"
                        | "sync")
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
            "-V" | "--version" => words.insert(0, "version"),
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
        "proxy" if rest.first() == Some(&"config") => {
            Command::ProxyConfig(proxy::parse_config(&rest[1..])?)
        }
        "proxy" if rest.first() == Some(&"token") => {
            Command::ProxyToken(proxy::parse_token(&rest[1..])?)
        }
        "proxy" => Command::ProxyFault(proxy::parse_fault(rest)?),
        "ws" | "workspace" if matches!(rest.first(), Some(&("export" | "import"))) => {
            Command::Transfer(transfer::parse(rest, false)?)
        }
        "ws" | "workspace" => Command::Workspace(workspaces::parse(rest, false)?),
        "prepare-main" => Command::Workspace(workspaces::parse(rest, true)?),
        "config" => Command::Config(config::parse(rest)?),
        "env" => Command::Env(env::parse(rest)?),
        "db" => Command::Db(db::parse(rest)?),
        "run" => Command::Run(run::parse(rest)?),
        "commands" | "shortcuts" => Command::Commands {
            json: run::parse_commands(rest)?,
        },
        "refresh" | "release" | "get" | "describe" | "apply" => {
            Command::Lifecycle(lifecycle::parse(name, rest)?)
        }
        "init" | "onboard" => Command::Onboard(onboard::parse(name, rest)?),
        "ps" => Command::Ps(machine::parse_ps(rest)?),
        "disk" => Command::Disk {
            json: match rest {
                [] | ["-o" | "--output", "table"] => false,
                ["--json"] | ["-o" | "--output", "json"] => true,
                _ => return Err("usage: pom disk [--json]".into()),
            },
        },
        "modules" => Command::Modules(modules::parse(rest)?),
        "completion" => Command::Completion(one("a shell (bash, zsh or fish)")?),
        "doctor" => {
            let args = args::Args::parse(rest, &["-o", "--output"])?;
            args.allow(&["--json", "-o", "--output", "--quick"])
                .map_err(|error| format!("{error}\nusage: pom doctor [--quick] [--json]"))?;
            args.at_most(0, "doctor")?;
            Command::Doctor {
                json: args.has("--json") || args.json()?,
                quick: args.has("--quick"),
            }
        }
        "agent" => {
            let (command, named) = agent::parse(rest)?;
            if workspace.is_none() {
                workspace = named;
            }
            Command::Agent(command)
        }
        "host" => Command::Host(remote::parse(rest)?),
        "server" => Command::Server(server::parse(rest)?),
        "project" | "projects" if matches!(rest.first(), Some(&("export" | "import"))) => {
            Command::Transfer(transfer::parse(rest, true)?)
        }
        "project" | "projects" => {
            let (verb, flags) = rest.split_first().ok_or("usage: pom project ls [--json]")?;
            if !matches!(*verb, "ls" | "list") {
                return Err(format!(
                    "unknown project command {verb} (ls, export, import)"
                ));
            }
            let args = args::Args::parse(flags, &["-o", "--output"])?;
            args.allow(&["--json", "-o", "--output"])?;
            args.at_most(0, "project ls")?;
            Command::Projects {
                json: args.has("--json") || args.json()?,
            }
        }
        "github" => Command::Github(push::parse_github(rest)?),
        "self-update" => Command::SelfUpdate(self_update::parse(rest)?),
        "sync" => Command::SyncStatus(push::parse_sync(rest)?),
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

/// What `pom --version` reports: the workspace version for a release build, with `-dev+<commit>` otherwise.
pub const VERSION: &str = env!("POM_VERSION");

const DOCTOR_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const QUICK_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

pub(crate) fn doctor(
    explicit: Option<&Path>,
    cwd: &Path,
    json: bool,
    quick: bool,
    out: &mut dyn Write,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let config_path = find_config(explicit, cwd).unwrap_or_else(|_| cwd.join("pom.yml"));
    let config = Config::load(&config_path).ok();
    let root = config_path.parent().unwrap_or(cwd).to_path_buf();
    let path = pom_services::tool_path();
    let backend = pom_ptyhost::limits::Backend::from_env();
    let support = pom_doctor::LimitSupport {
        linux: cfg!(target_os = "linux"),
        backend: backend.name().to_string(),
        controllers: pom_ptyhost::limits::available_controllers(&backend),
        linger: pom_ptyhost::limits::lingering(),
    };
    let linger = support.linger;
    let configured = config
        .as_ref()
        .is_some_and(|config| !config.limits.is_empty());
    let mut checks: Vec<pom_doctor::Check> = Vec::new();
    {
        let config = config.clone();
        let config_path = config_path.clone();
        let root = root.clone();
        checks.push((
            "config",
            Box::new(move || {
                let names = config
                    .as_ref()
                    .map(|config| {
                        pom_secrets::SecretStore::new(StateDir::from_env(), &config.session)
                            .names()
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let has_tool = |tool: &str| pom_doctor::on_path(path, tool);
                // Quick mode never waits on Docker: it trusts a docker found on the PATH.
                let docker_running = || quick || pom_doctor::docker_answers(path);
                let compose_installed = || quick || pom_doctor::compose_answers(path);
                pom_doctor::diagnose(
                    config.as_ref(),
                    &config_path,
                    &root,
                    &names,
                    &pom_doctor::Machine {
                        has_tool: &has_tool,
                        docker_running: &docker_running,
                        compose_installed: &compose_installed,
                    },
                )
            }),
        ));
    }
    checks.push((
        "proxy",
        Box::new(|| {
            let network = pom_env::dev_network();
            let token = pom_proxy::proxy_token(&network, &StateDir::from_env());
            pom_doctor::proxy_exposure(
                network.reach(),
                network.trusted_proxies.len(),
                token.is_some(),
                pom_proxy::refusal(&network, token.as_ref()),
            )
            .into_iter()
            .collect()
        }),
    ));
    checks.push((
        "limits",
        Box::new(move || pom_doctor::limit_findings(configured, &support)),
    ));
    checks.push((
        "path",
        Box::new(move || path_shadow_finding(path).into_iter().collect()),
    ));
    checks.push((
        "holders",
        Box::new(|| {
            machine::orphan_finding(&StateDir::from_env(), &SocketDir::from_env())
                .into_iter()
                .collect()
        }),
    ));
    if !quick {
        checks.push((
            "linux",
            Box::new(move || pom_doctor::linux_findings(path, pom_doctor::on_path(path, "docker"))),
        ));
        checks.push((
            "disk",
            Box::new(|| machine::disk_check(&StateDir::from_env()).findings),
        ));
        checks.push((
            "server",
            Box::new(move || pom_doctor::server_findings(&server::survey(linger))),
        ));
        if let Some(config) = config.clone() {
            checks.push((
                "git",
                Box::new(move || {
                    let default_branch = config.global_default_branch();
                    let probes = std::thread::scope(|threads| {
                        let handles: Vec<_> = config
                            .repos
                            .keys()
                            .map(|name| {
                                let checkout =
                                    pom_layout::repo_worktree(&root, name, default_branch, true);
                                threads.spawn(move || {
                                    checkout
                                        .is_dir()
                                        .then(|| pom_doctor::probe_origin(name, &checkout, path))
                                })
                            })
                            .collect();
                        handles
                            .into_iter()
                            .filter_map(|handle| handle.join().ok().flatten())
                            .collect::<Vec<_>>()
                    });
                    pom_doctor::git_auth_findings(&probes)
                }),
            ));
        }
    }
    let timeout = if quick {
        QUICK_CHECK_TIMEOUT
    } else {
        DOCTOR_CHECK_TIMEOUT
    };
    let (results, timings) = pom_doctor::run_checks(checks, timeout);
    let mut results = results.into_iter();
    let mut findings = results.next().unwrap_or_default();
    let mut gathered: Vec<pom_doctor::Finding> = results.flatten().collect();
    let limits_linger = gathered.iter().any(|finding| finding.id == "limits.linger");
    gathered.retain(|finding| !(limits_linger && finding.id == "server.linger"));
    let (status, problems): (Vec<_>, Vec<_>) = gathered.into_iter().partition(|finding| {
        finding.id.starts_with("server.") && finding.severity == pom_doctor::Severity::Ok
    });
    for finding in problems {
        pom_doctor::add_finding(&mut findings, finding);
    }
    findings.extend(status);
    let write = |out: &mut dyn Write, line: String| {
        writeln!(out, "{line}").map_err(|error| error.to_string())
    };
    let errors = findings
        .iter()
        .filter(|finding| finding.severity == pom_doctor::Severity::Error)
        .count();
    if json {
        let mut report = pom_doctor::report(&findings);
        report["quick"] = quick.into();
        report["version"] = VERSION.into();
        report["duration_ms"] = (started.elapsed().as_millis() as u64).into();
        report["checks"] = serde_json::to_value(&timings).map_err(|error| error.to_string())?;
        let report = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
        write(out, report)?;
        return if errors > 0 {
            Err(format!("{errors} blocking problem(s)"))
        } else {
            Ok(())
        };
    }
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
    if errors > 0 {
        return Err(format!("{errors} blocking problem(s)"));
    }
    Ok(())
}

/// Whether the `pom` a terminal runs is another build than this one.
fn path_shadow_finding(tool_path: &str) -> Option<pom_doctor::Finding> {
    let own = std::env::current_exe().ok()?;
    let path_env = std::env::var("PATH")
        .ok()
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| tool_path.to_string());
    pom_doctor::path_shadow(&path_env, &own, VERSION, &pom_doctor::pom_version)
}

/// A project only ever used from the CLI must still show in `project ls` and the app's host picker.
pub(crate) fn register_project(state: &StateDir, session: &str, root: &Path) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    if let Err(error) = pom_core::ensure_registered(state, session, root, now) {
        eprintln!("pom: list the project {session}: {error}");
    }
}

/// `path` as seen from `cwd`, so a project root derived from a bare `-c pom.yml` is never empty.
fn absolute_in(cwd: &Path, path: &Path) -> PathBuf {
    cwd.join(path)
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect()
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
        register_project(&state, &config.session, &project.root);
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
            Command::Run(request) => self.run_command(request, out),
            Command::Commands { json } => self.list_commands(*json, out),
            Command::Lifecycle(command) => self.lifecycle_command(command, out),
            Command::Github(request) => {
                push::setup(&self.state, &self.config.session, request, out)
            }
            Command::Ports
            | Command::Proxy
            | Command::ProxyConfig(_)
            | Command::ProxyToken(_)
            | Command::Onboard(_)
            | Command::Ps(_)
            | Command::Disk { .. }
            | Command::Modules(_)
            | Command::Completion(_)
            | Command::Doctor { .. }
            | Command::Agent(_)
            | Command::Host(_)
            | Command::Server(_)
            | Command::Transfer(_)
            | Command::Projects { .. }
            | Command::SelfUpdate(_)
            | Command::SyncStatus(_)
            | Command::Queue(_)
            | Command::Version
            | Command::Help => Ok(()),
        }
    }

    fn ordered(&self, target: &str, stopping: bool) -> Result<Vec<(String, String)>, String> {
        pom_remote::ordered_services(&self.config, target, stopping)
    }

    /// Under `workspaces.max_running`, a workspace that does not run yet first needs a slot; queued, it does not
    /// start now.
    fn admit(&self, target: &str, out: &mut dyn Write) -> Result<(), String> {
        let settings = pom_workspace::PowerSettings::from_settings_file();
        let Some(workspace) = self
            .project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == self.branch)
        else {
            return Ok(());
        };
        let mut state = pom_layout::WorkspaceState::load(&workspace.path);
        let up = ServiceRunner::service_targets(&self.config, &self.branch, self.is_main)
            .iter()
            .any(|target| self.runner.is_running(target));
        if settings.max_running > 0 && !(up && state.state.is_running()) {
            let services: Vec<String> = self
                .ordered(target, false)?
                .into_iter()
                .map(|(repo, service)| format!("{repo}/{service}"))
                .collect();
            let machine = pom_workspace::Machine::of(&self.context());
            match machine.admit(
                &self.project.root,
                workspace,
                &services,
                &settings,
                pom_workspace::now_ms(),
            )? {
                pom_workspace::Admitted::Run => {}
                pom_workspace::Admitted::MadeRoom(slept) => say(
                    out,
                    &format!("put {slept} to sleep to make room (workspaces.max_running)"),
                )?,
                pom_workspace::Admitted::Queued { running } => {
                    return Err(format!(
                        "{} is queued: {running} workspaces run with agents at work and workspaces.max_running is {}; it starts once one sleeps",
                        self.branch, settings.max_running
                    ));
                }
            }
        }
        if !state.state.is_running() {
            state.state = pom_layout::RunState::Running;
            state.asleep = None;
            state.queued_ms = None;
            state
                .save(&workspace.path)
                .map_err(|error| format!("save {}: {error}", workspace.path.display()))?;
        }
        Ok(())
    }

    fn start(&self, target: &str, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), String> {
        self.admit(target, out)?;
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
        // The dev proxy routes only the projects it knows, and one run from the CLI alone was never opened in the app.
        if let Err(error) =
            pom_sessions::Projects::register(&self.state, &self.config.session, &self.project.root)
        {
            say(err, &format!("dev proxy: register the project: {error}"))?;
        }
        if let Err(error) = proxy::ensure_running(out) {
            say(err, &format!("dev proxy: {error}"))?;
        }
        let store = module_store::Store::new(&self.state);
        match store.prune_if_due(&module_store::Options::from_settings_file()) {
            Ok(Some(pruned)) if pruned.removed > 0 => say(
                out,
                &format!(
                    "node_modules store: removed {} unused copies, freed {}",
                    pruned.removed,
                    module_store::format_size(pruned.freed)
                ),
            )?,
            Err(error) => say(err, &format!("node_modules store: prune: {error}"))?,
            _ => {}
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
                let state = if !running.contains(name) {
                    "stopped"
                } else if self.runner.shared_ready(&self.config, name) {
                    "running"
                } else {
                    "starting"
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
        let started = std::time::Instant::now();
        if targets.iter().any(|target| !target.is_workspace_level()) {
            let names: Vec<String> = self.config.shared_services.keys().cloned().collect();
            if let Err(silent) = self.runner.wait_shared_ready(&self.config, &names, timeout) {
                for name in &silent {
                    report(err, &format!("  shared/{name}  not ready"));
                }
                report(
                    err,
                    &format!("error: not ready after {}s", timeout.as_secs()),
                );
                return WAIT_TIMED_OUT;
            }
        }
        let timeout = timeout.saturating_sub(started.elapsed());
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
        let print = |snapshot: &pom_services::QueueSnapshot, idle: bool, out: &mut dyn Write| {
            let counts = &snapshot.counts;
            if queue.json {
                let document = serde_json::json!({
                    "schema": "pom.queue/v2",
                    "service": format!("{}/{}", target.repo, target.service),
                    "idle": idle,
                    "total": counts.total(),
                    "counts": counts,
                    "redis": snapshot.redis,
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
                        "{}/{}: waiting [{}], active {}, delayed due {}, prioritized {}; not counted: delayed later {}, repeat scheduled {} (redis {} db {})",
                        target.repo,
                        target.service,
                        waiting.join(" "),
                        counts.active,
                        counts.delayed_due,
                        counts.prioritized,
                        counts.delayed_future,
                        counts.repeat_scheduled,
                        snapshot.redis.name,
                        snapshot.redis.database,
                    ),
                );
            }
        };
        let redis = queue.redis.as_deref();
        if !queue.wait {
            return match self.runner.queue_snapshot(&self.config, &target, redis) {
                Ok(snapshot) => {
                    print(&snapshot, snapshot.counts.total() == 0, out);
                    0
                }
                Err(message) => {
                    report(err, &format!("error: {message}"));
                    1
                }
            };
        }
        let timeout = queue.timeout.unwrap_or(DEFAULT_QUEUE_WAIT);
        match self
            .runner
            .wait_queue_idle(&self.config, &target, redis, timeout)
        {
            Ok(Ok(snapshot)) => {
                print(&snapshot, true, out);
                0
            }
            Ok(Err(snapshot)) => {
                print(&snapshot, false, err);
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
        let report =
            pom_remote::status_report(&self.config, &self.runner, &self.branch, self.is_main);
        let mut document = serde_json::to_value(&report).map_err(|error| error.to_string())?;
        document["schema"] = "pom.status/v1".into();
        say(out, &document.to_string())
    }

    fn service(&self, entry: &str) -> Result<ServiceTarget, String> {
        let (repo, service) = self.config.find_service_entry(entry)?;
        Ok(self.target(&repo, &service))
    }

    /// A running service's scrollback, or what a crashed one left.
    fn logs(&self, entry: &str, out: &mut dyn Write) -> Result<(), String> {
        let target = self.service(entry)?;
        let bytes = pom_remote::service_output(&self.runner, &target, entry)?;
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

/// `project ls`: the local projects of sessions.json, most recently used first.
fn projects(state: &StateDir, json: bool, out: &mut dyn Write) -> Result<(), String> {
    let entries = pom_remote::local_projects(state);
    if json {
        let document =
            serde_json::json!({"schema": pom_remote::PROJECT_SCHEMA, "projects": entries});
        return say(out, &document.to_string());
    }
    if entries.is_empty() {
        return say(out, "no projects");
    }
    let width = entries
        .iter()
        .map(|entry| entry.name.len())
        .max()
        .unwrap_or(0);
    for entry in &entries {
        let missing = if entry.missing { "  (missing)" } else { "" };
        say(
            out,
            &format!("{:width$}  {}{missing}", entry.name, entry.path.display()),
        )?;
    }
    Ok(())
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

    #[test]
    fn a_build_reports_the_workspace_version_or_a_dev_version_of_it() {
        let release = env!("CARGO_PKG_VERSION");
        let dev = VERSION
            .strip_prefix(release)
            .ok_or(VERSION)
            .map(|rest| rest.is_empty() || rest.starts_with("-dev"));
        assert_eq!(dev, Ok(true), "{VERSION} vs {release}");
        assert!(
            !pom_remote::version_mismatch(VERSION, release),
            "a dev build speaks the same version as its release"
        );
    }

    #[test]
    fn another_thread_can_warn_while_the_command_holds_its_output() {
        let (mut out, mut err) = std_handles();
        writeln!(err, "streaming").ok();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            eprintln!("a warning from a stage thread");
            done.send(()).ok();
        });
        assert!(finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .is_ok());
        writeln!(out, "still streaming").ok();
    }

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
                redis: None,
            }))
        );
        assert_eq!(
            parsed("queue counts api/worker --redis cache").map(|i| i.command),
            Ok(Command::Queue(QueueArgs {
                service: "api/worker".into(),
                redis: Some("cache".into()),
                ..QueueArgs::default()
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
