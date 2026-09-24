//! The tools an agent gets for its workspace. Descriptions steer the agent, so they stay as the previous
//! core worded them (plain ASCII here).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_core::Project;
use pom_paths::StateDir;
use pom_services::{ServiceRunner, ServiceTarget};
use serde::Serialize;
use serde_json::{json, Value};

use crate::config_files;
use crate::protocol::{Tool, ToolArgs, ToolFn};

const RUN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const DEFAULT_LOG_LINES: usize = 200;
const MAX_LOG_LINES: usize = 2000;
const WORKSPACE_REPO: &str = "_ws";
const DEFAULT_QUERY_LIMIT: usize = 200;

/// The workspace an agent works in. Config and workspaces are read fresh on every call, so edits made
/// through the config tools apply immediately.
pub struct Workspace {
    pub config_path: PathBuf,
    pub state: StateDir,
    pub branch: String,
    pub runner: ServiceRunner,
}

#[derive(Serialize)]
struct EntryService {
    name: String,
    running: bool,
    #[serde(skip_serializing_if = "is_zero")]
    port: u16,
    #[serde(skip_serializing_if = "String::is_empty")]
    mode: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    tmux_window: String,
}

#[derive(Serialize)]
struct EntryShortcut {
    cmd: String,
    desc: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    key: String,
}

#[derive(Serialize)]
struct EntryRepo {
    name: String,
    alias: String,
    path: PathBuf,
    services: Vec<EntryService>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    shortcuts: Vec<EntryShortcut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    setup: Vec<String>,
}

#[derive(Serialize)]
struct Entry {
    branch: String,
    is_main: bool,
    path: PathBuf,
    repos: Vec<EntryRepo>,
    ws_services: Vec<EntryService>,
    running: usize,
    total: usize,
}

fn is_zero(port: &u16) -> bool {
    *port == 0
}

fn pretty(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|error| error.to_string())
}

fn text(args: &ToolArgs, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn positive(args: &ToolArgs, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(Value::as_f64)
        .filter(|number| *number > 0.0)
        .map(|number| number as usize)
}

impl Workspace {
    fn connector<'a>(&'a self, config: &'a Config) -> pom_db::Connector<'a> {
        pom_db::Connector {
            runner: &self.runner,
            config,
            branch: &self.branch,
        }
    }

    fn database(&self, config: &Config, name: &str) -> Result<pom_db::Database, String> {
        if name.is_empty() {
            return Err("db is required (a name from db_list)".into());
        }
        pom_db::list_databases(config, &self.branch)
            .into_iter()
            .find(|database| database.name == name)
            .ok_or_else(|| format!("no database {name:?} in this branch (see db_list)"))
    }

    fn config(&self) -> Result<Config, String> {
        Config::load(&self.config_path).map_err(|error| error.to_string())
    }

    fn project_root(&self) -> PathBuf {
        self.config_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    fn entry(&self, config: &Config) -> Result<Entry, String> {
        let project = Project::open(&self.config_path, &self.state);
        let workspace = project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == self.branch)
            .ok_or_else(|| {
                format!(
                    "no workspace for branch {:?} - create it first",
                    self.branch
                )
            })?;
        let rank = |name: &str| {
            config
                .repos
                .get_index_of(name)
                .unwrap_or(config.repos.len())
        };
        let mut repos: Vec<&pom_layout::Repo> = workspace.repos.iter().collect();
        repos.sort_by(|a, b| {
            rank(&a.name)
                .cmp(&rank(&b.name))
                .then_with(|| a.name.cmp(&b.name))
        });
        let (mut running, mut total) = (0, 0);
        let mut entry_repos = Vec::new();
        for repo in repos {
            let mut entry = EntryRepo {
                name: repo.name.clone(),
                alias: String::new(),
                path: repo.path.clone(),
                services: Vec::new(),
                shortcuts: Vec::new(),
                setup: Vec::new(),
            };
            if let Some(dir) = config.repos.get(&repo.name) {
                entry.alias = if dir.alias.is_empty() {
                    repo.name.clone()
                } else {
                    dir.alias.clone()
                };
                entry.shortcuts = dir
                    .effective_shortcuts()
                    .into_iter()
                    .map(|shortcut| EntryShortcut {
                        cmd: shortcut.cmd,
                        desc: shortcut.desc,
                        key: shortcut.key,
                    })
                    .collect();
                entry.setup = dir.effective_setup();
                for (name, service) in &dir.services {
                    let target = self.target(workspace.is_main, &repo.name, name);
                    let holder = self.runner.holder_name(&target);
                    let is_running = self.runner.holders().holder_alive(&holder);
                    let mode = if service.modes.is_empty() {
                        String::new()
                    } else {
                        let mode = self.runner.mode(&repo.name, name, service);
                        if mode.is_empty() {
                            "default".to_string()
                        } else {
                            mode
                        }
                    };
                    entry.services.push(EntryService {
                        name: name.clone(),
                        running: is_running,
                        port: self.runner.port(config, &target).unwrap_or(0),
                        mode,
                        tmux_window: holder,
                    });
                    total += 1;
                    running += usize::from(is_running);
                }
            }
            entry_repos.push(entry);
        }
        let ws_services = config
            .workspace_services
            .keys()
            .map(|name| {
                let target = self.target(workspace.is_main, WORKSPACE_REPO, name);
                let holder = self.runner.holder_name(&target);
                let is_running = self.runner.holders().holder_alive(&holder);
                total += 1;
                running += usize::from(is_running);
                EntryService {
                    name: name.clone(),
                    running: is_running,
                    port: 0,
                    mode: String::new(),
                    tmux_window: holder,
                }
            })
            .collect();
        Ok(Entry {
            branch: workspace.branch.clone(),
            is_main: workspace.is_main,
            path: workspace.path.clone(),
            repos: entry_repos,
            ws_services,
            running,
            total,
        })
    }

    fn target(&self, is_main: bool, repo: &str, service: &str) -> ServiceTarget {
        ServiceTarget {
            branch: self.branch.clone(),
            is_main,
            repo: repo.to_string(),
            service: service.to_string(),
        }
    }

    /// A service by name, in `repo` (name or alias), or among the workspace-level ones when `repo` is
    /// empty; returns its target and the port it was leased (0 when none).
    fn resolve_service(
        &self,
        entry: &Entry,
        repo: &str,
        service: &str,
    ) -> Result<(ServiceTarget, u16), String> {
        let missing = || format!("no service {service:?} in this workspace");
        if repo.is_empty() || repo == WORKSPACE_REPO {
            return entry
                .ws_services
                .iter()
                .find(|known| known.name == service)
                .map(|_| (self.target(entry.is_main, WORKSPACE_REPO, service), 0))
                .ok_or_else(missing);
        }
        let found = entry
            .repos
            .iter()
            .filter(|known| known.name == repo || known.alias == repo)
            .find_map(|known| {
                known
                    .services
                    .iter()
                    .find(|candidate| candidate.name == service)
                    .map(|candidate| (known.name.clone(), candidate.port))
            })
            .ok_or_else(missing)?;
        Ok((self.target(entry.is_main, &found.0, service), found.1))
    }

    fn service_action(&self, action: &str, args: &ToolArgs) -> Result<String, String> {
        let config = self.config()?;
        let entry = self.entry(&config)?;
        let service = text(args, "service");
        let (target, port) = self.resolve_service(&entry, &text(args, "repo"), &service)?;
        let result = match action {
            "start" => self.runner.start(&config, &target).map(|_| ()),
            "stop" => self.runner.stop(&target).map_err(Into::into),
            _ => self.runner.restart(&config, &target).map(|_| ()),
        };
        result.map_err(|error| error.to_string())?;
        Ok(format!("{action}: {service} (port {port})"))
    }

    /// A shell command in a repo's worktree with the workspace's env; stdout and stderr interleaved.
    fn run_in_env(&self, repo: &str, command: &str) -> Result<(i32, String), String> {
        let config = self.config()?;
        let entry = self.entry(&config)?;
        if entry.is_main {
            return Err("main is read-only - run this in a test branch".into());
        }
        let worktree = pom_layout::repo_worktree(&self.project_root(), repo, &self.branch, false);
        if !worktree.is_dir() {
            return Err(format!(
                "no worktree for repo {repo:?} on {:?}",
                self.branch
            ));
        }
        let env = self.runner.workspace_env(&config, &self.branch);
        env.write_env_files().map_err(|error| error.to_string())?;
        run_captured(&worktree, command, env.repo_env(repo))
    }
}

/// Runs `command` in a login shell, killing it (and whatever it started) after the timeout.
fn run_captured(
    dir: &Path,
    command: &str,
    env: Vec<(String, String)>,
) -> Result<(i32, String), String> {
    use std::os::unix::process::CommandExt;

    let (mut reader, writer) = std::io::pipe().map_err(|error| error.to_string())?;
    let errors = writer.try_clone().map_err(|error| error.to_string())?;
    let mut child = Command::new("zsh")
        .args(["-lc", command])
        .current_dir(dir)
        .env("PATH", pom_services::tool_path())
        .envs(env)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(errors)
        .process_group(0)
        .spawn()
        .map_err(|error| error.to_string())?;
    let collector = std::thread::spawn(move || {
        let mut output = Vec::new();
        if let Err(error) = reader.read_to_end(&mut output) {
            output.extend_from_slice(format!("\n(output read failed: {error})").as_bytes());
        }
        output
    });
    let deadline = Instant::now() + RUN_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                // SAFETY: signalling our own child's process group.
                unsafe {
                    libc::killpg(child.id() as i32, libc::SIGKILL);
                }
                if let Err(error) = child.wait() {
                    eprintln!("mcp: reap timed-out command: {error}");
                }
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(error.to_string()),
        }
    };
    let output = collector.join().unwrap_or_default();
    let mut output = String::from_utf8_lossy(&output).into_owned();
    let code = match status {
        Some(status) => status.code().unwrap_or(-1),
        None => {
            output.push_str("\n(timed out after 5 minutes)");
            -1
        }
    };
    Ok((code, output))
}

/// The last `lines` non-blank lines of terminal output, escape sequences removed.
pub fn tail_lines(raw: &[u8], lines: usize) -> Vec<String> {
    let text = strip_escapes(&String::from_utf8_lossy(raw)).replace('\r', "");
    let kept: Vec<String> = text
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect();
    let skip = kept.len().saturating_sub(lines);
    kept.into_iter().skip(skip).collect()
}

/// Drops OSC (`ESC ] ... BEL|ST`), CSI (`ESC [ ... final`) and two-byte escape sequences.
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some('[') => {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn service_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "service": { "type": "string", "description": "service name" },
            "repo": { "type": "string", "description": "repo name/alias; omit for workspace-level services (Claude Code, editor)" },
        },
        "required": ["service"],
    })
}

fn yaml_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "yaml": { "type": "string" } },
        "required": ["yaml"],
    })
}

fn path_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "path": { "type": "string" } },
        "required": ["path"],
    })
}

pub fn tools(workspace: Rc<Workspace>) -> Vec<Tool> {
    let tool = |name: &'static str,
                description: &'static str,
                schema: Option<Value>,
                read_only: bool,
                run: ToolFn| Tool {
        name,
        description,
        schema,
        read_only,
        destructive: false,
        max_result_chars: 0,
        run,
    };
    let mut tools = Vec::new();

    let ws = workspace.clone();
    tools.push(tool(
        "workspace_info",
        "Overview of THIS workspace: branch, repos, and their services (running state, ports, modes, agent state).",
        None,
        true,
        Box::new(move |_| pretty(&ws.entry(&ws.config()?)?)),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "services",
        "List this workspace's services with running state and allocated port.",
        None,
        true,
        Box::new(move |_| {
            let entry = ws.entry(&ws.config()?)?;
            let rows: Vec<Value> = entry
                .repos
                .iter()
                .flat_map(|repo| {
                    repo.services.iter().map(|service| {
                        json!({
                            "Repo": repo.name,
                            "Service": service.name,
                            "Running": service.running,
                            "Port": service.port,
                        })
                    })
                })
                .collect();
            if rows.is_empty() {
                return Ok("null".into());
            }
            pretty(&rows)
        }),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "ports",
        "Map of this workspace's services to their allocated localhost ports.",
        None,
        true,
        Box::new(move |_| {
            let entry = ws.entry(&ws.config()?)?;
            let ports: std::collections::BTreeMap<String, u16> = entry
                .repos
                .iter()
                .flat_map(|repo| {
                    repo.services
                        .iter()
                        .filter(|service| service.port > 0)
                        .map(|service| (format!("{}/{}", repo.name, service.name), service.port))
                })
                .collect();
            pretty(&ports)
        }),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "service_url",
        "The base URL to reach a service (dev-proxy domain if configured, else http://localhost:<port>). Curl it via run_in_env, e.g. `curl -s $(url)/health`.",
        Some(service_schema()),
        true,
        Box::new(move |args| {
            let config = ws.config()?;
            let entry = ws.entry(&config)?;
            let service = text(args, "service");
            let (target, _) = ws.resolve_service(&entry, &text(args, "repo"), &service)?;
            ws.runner.url(&config, &target).ok_or_else(|| {
                format!("no URL for {service:?} (not running and no remote env?)")
            })
        }),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "databases",
        "This branch's databases with ready-to-use Postgres connection strings. Use these to run migrations/queries against the per-branch DB.",
        None,
        true,
        Box::new(move |_| {
            let config = ws.config()?;
            let postgres = config.shared_services.get("postgres");
            let pick = |value: Option<&String>| {
                value
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .unwrap_or_else(|| "postgres".to_string())
            };
            let user = pick(postgres.map(|def| &def.db_user));
            let password = pick(postgres.map(|def| &def.db_password));
            let port = match ws.runner.shared_host_port("postgres") {
                0 => 5432,
                port => port,
            };
            let databases: Vec<Value> = config
                .repos
                .values()
                .flat_map(|dir| dir.databases.values())
                .map(|template| {
                    let name = format!(
                        "{}_{}",
                        config.session,
                        pom_env::resolve_branch_tokens(template, &ws.branch)
                    );
                    json!({
                        "name": name,
                        "url": format!("postgres://{user}:{password}@localhost:{port}/{name}"),
                    })
                })
                .collect();
            pretty(&databases)
        }),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "db_list",
        "List the databases you can browse in THIS branch, each with `name` (use it as `db` in db_tables/db_columns/db_query), `engine` (postgres|redis), `repo`, and `label`. Includes shared Redis keyspaces. Prefer this + db_query to inspect data over spinning up psql via run_in_env.",
        None,
        true,
        Box::new(move |_| pretty(&pom_db::list_databases(&ws.config()?, &ws.branch))),
    ));

    let db_schema = || {
        json!({
            "type": "object",
            "properties": { "db": { "type": "string", "description": "database name from db_list" } },
            "required": ["db"],
        })
    };
    let ws = workspace.clone();
    tools.push(tool(
        "db_tables",
        "List a database's tables/views (postgres) or keyspaces (redis). `db` is a `name` from db_list.",
        Some(db_schema()),
        true,
        Box::new(move |args| {
            let config = ws.config()?;
            let database = ws.database(&config, &text(args, "db"))?;
            pretty(&ws.connector(&config).tables(&database)?)
        }),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "db_columns",
        "List every column (schema/table/name/type) in a database - use to learn the schema before writing a query. `db` is a `name` from db_list.",
        Some(db_schema()),
        true,
        Box::new(move |args| {
            let config = ws.config()?;
            let database = ws.database(&config, &text(args, "db"))?;
            pretty(&ws.connector(&config).columns(&database)?)
        }),
    ));

    let ws = workspace.clone();
    tools.push(Tool {
        max_result_chars: 12000,
        ..tool(
            "db_query",
            "Run SQL against a branch database (postgres) or a command against Redis, returning columns + rows. `db` is a `name` from db_list. Reads are safe; writes hit the REAL per-branch DB - verify with a SELECT first. Results are capped by `limit` (default 200).",
            Some(json!({
                "type": "object",
                "properties": {
                    "db": { "type": "string", "description": "database name from db_list" },
                    "sql": { "type": "string", "description": "SQL (postgres) or a Redis command (e.g. `GET key`)" },
                    "limit": { "type": "integer", "description": "max rows (default 200)" },
                },
                "required": ["db", "sql"],
            })),
            false,
            Box::new(move |args| {
                let sql = text(args, "sql");
                if sql.trim().is_empty() {
                    return Err("sql is required".into());
                }
                let config = ws.config()?;
                let database = ws.database(&config, &text(args, "db"))?;
                let limit = positive(args, "limit").unwrap_or(DEFAULT_QUERY_LIMIT);
                let result = ws.connector(&config).query(&database, &sql, limit)?;
                let mut out = json!({ "columns": result.columns, "rows": result.rows });
                if result.truncated {
                    out["truncated"] = json!(true);
                }
                if let Some(count) = result.rows_affected.filter(|count| *count > 0) {
                    out["rows_affected"] = json!(count);
                }
                pretty(&out)
            }),
        )
    });

    for action in ["start", "stop", "restart"] {
        let ws = workspace.clone();
        let description: &'static str = match action {
            "start" => "start a service in this workspace and report status. Ports are pre-flighted, so a started service is guaranteed to bind the port pom reports.",
            "stop" => "stop a service in this workspace and report status. Ports are pre-flighted, so a started service is guaranteed to bind the port pom reports.",
            _ => "restart a service in this workspace and report status. Ports are pre-flighted, so a started service is guaranteed to bind the port pom reports.",
        };
        let name: &'static str = match action {
            "start" => "service_start",
            "stop" => "service_stop",
            _ => "service_restart",
        };
        tools.push(tool(
            name,
            description,
            Some(service_schema()),
            false,
            Box::new(move |args| ws.service_action(action, args)),
        ));
    }

    let ws = workspace.clone();
    tools.push(tool(
        "commands",
        "The project's PRE-WRITTEN commands per repo: one-time `setup` steps and `shortcuts` (each with `key`->`desc`->`cmd`), already preset-resolved. ALWAYS check here before hand-writing a shell command - these carry the project's canonical install / generate / migrate / lint / test / build invocations (e.g. `npx prisma generate`, `bundle exec rake db:migrate`). Run one with `run_shortcut` (by `key` when present, else `desc`) or, for a raw setup step, `run_in_env`.",
        None,
        true,
        Box::new(move |_| {
            let entry = ws.entry(&ws.config()?)?;
            let repos: Vec<Value> = entry
                .repos
                .iter()
                .filter(|repo| !repo.setup.is_empty() || !repo.shortcuts.is_empty())
                .map(|repo| {
                    let mut value = json!({ "repo": repo.name, "alias": repo.alias });
                    if !repo.setup.is_empty() {
                        value["setup"] = json!(repo.setup);
                    }
                    if !repo.shortcuts.is_empty() {
                        value["shortcuts"] = json!(repo.shortcuts);
                    }
                    value
                })
                .collect();
            if repos.is_empty() {
                return Ok("No pre-written setup/shortcuts in this project's config.".into());
            }
            pretty(&json!({ "repos": repos }))
        }),
    ));

    let ws = workspace.clone();
    tools.push(Tool {
        max_result_chars: 8000,
        ..tool(
            "run_shortcut",
            "Run one of a repo's PRE-WRITTEN shortcuts, in the repo's worktree with the workspace's resolved env. Address it by `key` (the canonical op - install/generate/migrate/test/lint/build/format; see each shortcut's `key` in `commands`) OR by `desc`. PREFER `key` when it exists - it's stable across projects. PREFER this over `run_in_env` whenever a shortcut exists - it uses the project's exact, tested command. Synchronous.",
            Some(json!({
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "repo name/alias" },
                    "key": { "type": "string", "description": "canonical op: install/generate/migrate/test/lint/build/format" },
                    "desc": { "type": "string", "description": "the shortcut's description, or a unique substring of it (use when there's no key)" },
                },
                "required": ["repo"],
            })),
            false,
            Box::new(move |args| {
                let (repo, key, wanted) = (text(args, "repo"), text(args, "key"), text(args, "desc"));
                if key.is_empty() && wanted.is_empty() {
                    return Err(
                        "provide `key` (e.g. migrate) or `desc` - call `commands` to list them".into(),
                    );
                }
                let entry = ws.entry(&ws.config()?)?;
                let found = entry
                    .repos
                    .iter()
                    .find(|known| known.name == repo || known.alias == repo)
                    .ok_or_else(|| format!("no repo {repo:?} in this workspace"))?;
                let command = if key.is_empty() {
                    let wanted_lower = wanted.trim().to_lowercase();
                    found
                        .shortcuts
                        .iter()
                        .find(|shortcut| shortcut.desc.eq_ignore_ascii_case(&wanted))
                        .or_else(|| {
                            found
                                .shortcuts
                                .iter()
                                .find(|shortcut| shortcut.desc.to_lowercase().contains(&wanted_lower))
                        })
                        .map(|shortcut| shortcut.cmd.clone())
                        .ok_or_else(|| {
                            format!(
                                "no shortcut matching {wanted:?} in {repo} - call `commands` to list them"
                            )
                        })?
                } else {
                    found
                        .shortcuts
                        .iter()
                        .find(|shortcut| shortcut.key.eq_ignore_ascii_case(&key))
                        .map(|shortcut| shortcut.cmd.clone())
                        .ok_or_else(|| {
                            format!("no command {key:?} in {repo} - call `commands` to list keys")
                        })?
                };
                let (code, output) = ws.run_in_env(&found.name, &command)?;
                Ok(format!("$ {command}\nexit {code}\n{output}"))
            }),
        )
    });

    let ws = workspace.clone();
    tools.push(Tool {
        max_result_chars: 8000,
        ..tool(
            "service_logs",
            "Recent terminal output of a service (to check for errors like a port-in-use).",
            Some(json!({
                "type": "object",
                "properties": {
                    "service": { "type": "string" },
                    "repo": { "type": "string" },
                    "lines": { "type": "integer", "description": "how many trailing lines (default 200)" },
                },
                "required": ["service"],
            })),
            true,
            Box::new(move |args| {
                let config = ws.config()?;
                let entry = ws.entry(&config)?;
                let service = text(args, "service");
                let (target, _) = ws
                    .resolve_service(&entry, &text(args, "repo"), &service)
                    .map_err(|_| format!("service {service:?} not found or not started"))?;
                let lines = positive(args, "lines")
                    .unwrap_or(DEFAULT_LOG_LINES)
                    .min(MAX_LOG_LINES);
                let holder = ws.runner.holder_name(&target);
                let holders = ws.runner.holders();
                let raw = if holders.holder_alive(&holder) {
                    pom_ptyhost::snapshot(holders, &holder, Duration::from_secs(2))
                        .map_err(|error| error.to_string())?
                } else {
                    holders
                        .crash_info(&holder)
                        .map(|info| info.output)
                        .unwrap_or_default()
                };
                Ok(tail_lines(&raw, lines).join("\n"))
            }),
        )
    });

    let ws = workspace.clone();
    tools.push(Tool {
        max_result_chars: 8000,
        ..tool(
            "run_in_env",
            "Run a shell command in a repo's worktree with the workspace's resolved env (correct DATABASE_URL/ports). Use to run migrations, tests, seeds and verify against the REAL running stack. Synchronous, 5-min timeout. NOTE: if the project already defines a `setup` step or `shortcut` for this task (call `commands` to check), prefer `run_shortcut` so you use the project's canonical invocation instead of guessing one.",
            Some(json!({
                "type": "object",
                "properties": {
                    "cmd": { "type": "string", "description": "shell command" },
                    "repo": { "type": "string", "description": "repo name/alias to run in" },
                },
                "required": ["cmd", "repo"],
            })),
            false,
            Box::new(move |args| {
                let command = text(args, "cmd");
                if command.trim().is_empty() {
                    return Err("branch + cmd required".into());
                }
                let config = ws.config()?;
                let repo = text(args, "repo");
                let repo = config
                    .repos
                    .iter()
                    .find(|(name, dir)| **name == repo || dir.alias == repo)
                    .map_or(repo.clone(), |(name, _)| name.clone());
                let (code, output) = ws.run_in_env(&repo, &command)?;
                Ok(format!("exit {code}\n{output}"))
            }),
        )
    });

    let ws = workspace.clone();
    tools.push(tool(
        "resolve_port_conflict",
        "Move this workspace to a fresh, fully-free port region and regenerate its env - the self-heal when a service can't bind because something grabbed pom's port. Restart affected services afterward.",
        None,
        false,
        Box::new(move |_| {
            let config = ws.config()?;
            ws.runner
                .relocate_workspace(&config, &ws.branch)
                .map_err(|error| error.to_string())?;
            Ok("Relocated to a clean port region and regenerated env. Restart your services to pick up the new ports; call `ports` to see them.".into())
        }),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "config_get",
        "Read this project's pom.yml (services, repos, shared services, env profiles, databases).",
        None,
        true,
        Box::new(move |_| config_files::merged_text(&ws.config_path)),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "secrets_list",
        "List the NAMES of secrets in the app-local secret store (values are never returned). Onboarding imports a project's gitignored .env values here - wire each into the config env as {{secret.NAME}} (a real secret) or map infra to {{shared.*}} instead.",
        None,
        true,
        Box::new(move |_| {
            let store = pom_secrets::SecretStore::new(ws.state.clone(), ws.runner.session());
            let mut names = store.names().map_err(|error| format!("{error:?}"))?;
            names.sort();
            pretty(&names)
        }),
    ));

    tools.push(tool(
        "config_validate",
        "Dry-run validate a proposed pom.yml (schema + reference checks) WITHOUT writing. Always validate before config_set.",
        Some(yaml_schema()),
        true,
        Box::new(move |args| {
            config_files::validate_text(&text(args, "yaml"))
                .map(|_| "valid".to_string())
                .map_err(|error| format!("invalid: {error}"))
        }),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "config_set",
        "Validate and write a new pom.yml, then reload - adds/edits services, repos, shared services, databases, env. Rejected if invalid (nothing is written). Newly added services get ports allocated automatically.",
        Some(yaml_schema()),
        false,
        Box::new(move |args| {
            config_files::write_root(&ws.config_path, &text(args, "yaml"))
                .map_err(|error| format!("rejected: {error}"))?;
            Ok("Config written, validated, and reloaded.".into())
        }),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "config_files",
        "List the config files: the root pom.yml plus every pom.d/**.yml fragment (when the config is split). Use this first when the config is split - edit the right fragment with config_file_get/config_file_set instead of config_set (which is rejected for split configs).",
        None,
        true,
        Box::new(move |_| pretty(&config_files::list(&ws.config_path))),
    ));

    let ws = workspace.clone();
    tools.push(tool(
        "config_file_get",
        "Read one config file by its absolute path (from config_files) - the root pom.yml or a single pom.d fragment.",
        Some(path_schema()),
        true,
        Box::new(move |args| config_files::read(&ws.config_path, Path::new(&text(args, "path")))),
    ));

    let ws = workspace;
    tools.push(tool(
        "config_file_set",
        "Write one config file (root pom.yml or a pom.d fragment) by absolute path, then reload. The edit is validated against the FULL merged config before it lands - rejected (nothing written) if it breaks the whole. Prefer this over config_set when the config is split across pom.d.",
        Some(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "yaml": { "type": "string" },
            },
            "required": ["path", "yaml"],
        })),
        false,
        Box::new(move |args| {
            config_files::write_file(
                &ws.config_path,
                Path::new(&text(args, "path")),
                &text(args, "yaml"),
            )
            .map_err(|error| format!("rejected: {error}"))?;
            Ok("File written, validated against the merged config, and reloaded.".into())
        }),
    ));

    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_strip_escapes_and_blank_lines() {
        let raw = b"\x1b]0;title\x07one\r\n\x1b[31mtwo\x1b[0m\n\n   \nthree\n";
        assert_eq!(tail_lines(raw, 2), ["two", "three"]);
        assert_eq!(tail_lines(raw, 10), ["one", "two", "three"]);
        assert!(tail_lines(b"", 5).is_empty());
    }
}
