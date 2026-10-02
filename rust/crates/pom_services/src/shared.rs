//! Shared services (a Postgres, a Redis, ...) run once per project in Docker, not per workspace: one
//! compose project `<session>-shared` from a generated `docker-compose.shared.yml` at the project root,
//! the same file and names the previous core used so either can manage the running stack.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use pom_config::{Config, SharedServiceDef};

use crate::control::{ServiceError, ServiceRunner};

pub const COMPOSE_FILE: &str = "docker-compose.shared.yml";
pub const SHARED_NETWORK: &str = "pomelo-shared";
/// Shared services keep their usual host port when it is free, searching this far above it.
const PREFERRED_SPAN: u16 = 100;
const SLOT_RESET_ATTEMPTS: usize = 5;
const SLOT_RESET_PAUSE: Duration = Duration::from_millis(500);
const POSTGRES: &str = "postgres";
/// A Docker that stopped answering must not hold up the panel or an agent asking what runs.
const SHARED_PS_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_PG_PORT: u16 = 5432;
const DEFAULT_REDIS_PORT: u16 = 6379;

/// Where a shared database server listens and the login to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}
/// The `index`-th `"host:container"` port of `service` in a compose file we wrote.
fn composed_port(text: &str, service: &str, index: usize) -> Option<u16> {
    let header = format!("  {service}:");
    let mut lines = text.lines().skip_while(|line| *line != header).skip(1);
    lines
        .by_ref()
        .take_while(|line| line.starts_with("   ") || line.is_empty())
        .filter_map(|line| line.trim().strip_prefix("- \""))
        .filter_map(|mapping| mapping.trim_end_matches('"').split_once(':'))
        .nth(index)
        .and_then(|(host, _)| host.parse().ok())
}

/// A running container publishing a host port, and the compose project it belongs to (empty outside compose).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortOwner {
    pub container: String,
    pub project: String,
}

/// The first `name\tproject` line of `docker ps --format`.
fn parse_port_owner(stdout: &str) -> Option<PortOwner> {
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let (container, project) = line.split_once('\t').unwrap_or((line, ""));
    Some(PortOwner {
        container: container.trim().to_string(),
        project: project.trim().to_string(),
    })
}

const PORT_OWNER_TIMEOUT: Duration = Duration::from_secs(5);

const OUTPUT_TAIL_LINES: usize = 20;

/// Runs `command` without stdin, killing it past `timeout`; its stdout, or why it failed with the tail of its
/// output.
pub fn run_within(command: &mut Command, timeout: Duration) -> Result<String, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start: {error}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                if let Err(error) = pipe.read_to_string(&mut text) {
                    eprintln!("services: read output: {error}");
                }
            }
            text
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    eprintln!("services: stop a command past its deadline: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("services: reap a command: {error}");
                }
                break Err(format!("gave no answer within {}s", timeout.as_secs()));
            }
            Err(error) => break Err(error.to_string()),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    let status = status?;
    if status.success() {
        return Ok(stdout);
    }
    let combined = stdout + &stderr;
    let lines: Vec<&str> = combined
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(OUTPUT_TAIL_LINES)..].join("\n");
    Err(match status.code() {
        Some(code) => format!("exit {code}\n{tail}"),
        None => format!("killed\n{tail}"),
    })
}

/// Used to reach a Postgres not published by one of our containers.
const PSQL_IMAGE: &str = "postgres:16-alpine";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedAction {
    Start,
    Stop,
    Restart,
}

impl ServiceRunner {
    pub fn compose_project(&self) -> String {
        format!("{}-shared", self.session)
    }

    pub fn compose_file(&self) -> PathBuf {
        self.project_root.join(COMPOSE_FILE)
    }

    /// The host port the compose file on disk publishes for a shared service's `index`-th port. A running
    /// container keeps that port even when its lease was reclaimed (Docker paused, the laptop slept).
    pub(crate) fn composed_port(&self, name: &str, index: usize) -> Option<u16> {
        let text = std::fs::read_to_string(self.compose_file()).ok()?;
        composed_port(&text, name, index)
    }

    /// The host port a shared service is reached on: its lease, else the stable fallback.
    pub fn shared_host_port(&self, name: &str) -> u16 {
        pom_env::EnvSources::shared_port(self, name).unwrap_or(0)
    }

    /// Leases the shared services' ports, makes sure the shared network exists and rewrites the
    /// compose file.
    pub fn write_shared_compose(&self, config: &Config) -> Result<PathBuf, ServiceError> {
        self.ensure_network()?;
        self.reserve_shared_ports(config);
        let text = self.shared_compose(config);
        let path = self.compose_file();
        std::fs::write(&path, text)?;
        Ok(path)
    }

    /// The compose file's contents: one service per instance (a capacity service runs as many as its
    /// slots need), each on its leased host port plus the instance index.
    pub fn shared_compose(&self, config: &Config) -> String {
        let mut out = String::from("# Auto-generated by pom. Do not edit.\n");
        out.push_str(&format!("name: {}\n\nservices:\n", self.compose_project()));
        let mut volumes: Vec<String> = Vec::new();
        for (name, def) in &config.shared_services {
            if def.is_command() {
                continue;
            }
            let instances = if def.capacity.is_some() {
                self.slots.instance_count(name).max(1)
            } else {
                1
            };
            for instance in 0..instances {
                let service = if instance == 0 {
                    name.clone()
                } else {
                    format!("{name}-{}", instance + 1)
                };
                out.push_str(&format!("  {service}:\n    image: {}\n", def.image));
                if !def.command.is_empty() {
                    out.push_str(&format!("    command: {}\n", def.command));
                }
                if !def.ports.is_empty() {
                    out.push_str("    ports:\n");
                    for (index, mapping) in def.ports.iter().enumerate() {
                        let host = self.shared_port_at(name, index) + instance;
                        out.push_str(&format!("      - \"{host}:{}\"\n", container_port(mapping)));
                    }
                }
                if !def.environment.is_empty() {
                    out.push_str("    environment:\n");
                    for (key, value) in &def.environment {
                        out.push_str(&format!("      {key}: \"{value}\"\n"));
                    }
                }
                if !def.volumes.is_empty() {
                    out.push_str("    volumes:\n");
                    for volume in &def.volumes {
                        let volume = instance_volume(volume, instance);
                        out.push_str(&format!("      - {volume}\n"));
                        if let Some((source, _)) = volume.split_once(':') {
                            let named = !source.starts_with('.') && !source.starts_with('/');
                            if named && !volumes.iter().any(|known| known == source) {
                                volumes.push(source.to_string());
                            }
                        }
                    }
                }
                if let Some(check) = &def.healthcheck {
                    let or = |value: &str, fallback: &str| {
                        if value.is_empty() {
                            fallback.to_string()
                        } else {
                            value.to_string()
                        }
                    };
                    let retries = if check.retries == 0 { 3 } else { check.retries };
                    out.push_str(&format!(
                        "    healthcheck:\n      interval: {}\n      timeout: {}\n      retries: {retries}\n",
                        or(&check.interval, "10s"),
                        or(&check.timeout, "3s"),
                    ));
                }
                out.push_str(&format!(
                    "    networks:\n      - default\n      - {SHARED_NETWORK}\n    restart: unless-stopped\n\n"
                ));
            }
        }
        out.push_str(&format!(
            "networks:\n  {SHARED_NETWORK}:\n    external: true\n"
        ));
        if !volumes.is_empty() {
            out.push_str("\nvolumes:\n");
            for volume in volumes {
                out.push_str(&format!("  {volume}:\n    driver: local\n"));
            }
        }
        out
    }

    /// Brings the whole shared stack up, containers and commands (a no-op for services already running).
    pub fn ensure_shared(&self, config: &Config) -> Result<(), ServiceError> {
        if config.shared_services.is_empty() {
            return Ok(());
        }
        let mut result = Ok(());
        if config.shared_services.values().any(|def| !def.is_command()) {
            let file = self.write_shared_compose(config)?;
            // Instances beyond what the slots need, left from a time more workspaces existed, go away.
            result = self.compose(&file, &["up", "-d", "--remove-orphans"]);
            if result.is_ok() {
                if let Err(error) = self.reconcile_slots(config) {
                    eprintln!("services: shared slots: {error}");
                    result = Err(error);
                }
            }
        } else {
            self.reserve_shared_ports(config);
        }
        for (name, def) in &config.shared_services {
            if def.is_command() {
                if let Err(error) = self.start_shared_command(config, name) {
                    eprintln!("services: shared {name}: {error}");
                    if result.is_ok() {
                        result = Err(error);
                    }
                }
            }
        }
        result
    }

    /// Brings slot bookkeeping in line with the workspaces on disk: adopts this project's slots from the
    /// file every project used to share, gives back the slots of workspaces whose folder is gone, and empties
    /// every slot waiting for its reset.
    pub fn reconcile_slots(&self, config: &Config) -> Result<(), ServiceError> {
        self.reconcile_slots_with(config, true)
    }

    /// Like `reconcile_slots`, for app launch: slots whose container is down wait for the next start.
    pub fn reclaim_slots(&self, config: &Config) -> Result<(), ServiceError> {
        self.reconcile_slots_with(config, false)
    }

    fn reconcile_slots_with(
        &self,
        config: &Config,
        require_running: bool,
    ) -> Result<(), ServiceError> {
        let mut workspaces = workspace_keys_on_disk(&self.project_root);
        self.slots.adopt_legacy(&workspaces)?;
        // An unreadable project folder must not look like every workspace was deleted.
        let scanned = !workspaces.is_empty();
        workspaces.insert(pom_env::port_ws_key(config.global_default_branch()));
        for (name, def) in &config.shared_services {
            if def.capacity.is_none() || !scanned {
                continue;
            }
            for ws_key in self.slots.holders(name) {
                if !workspaces.contains(&ws_key) {
                    self.slots.release(name, &ws_key)?;
                }
            }
        }
        self.reset_pending_slots(config, require_running)
    }

    /// Empties every slot waiting for it. A fresh slot whose reset fails is swapped for another, so a
    /// workspace never gets one that may hold someone else's data. Without `require_running`, a slot whose
    /// container is not up yet just keeps waiting: the next start of the shared services empties it.
    pub(crate) fn reset_pending_slots(
        &self,
        config: &Config,
        require_running: bool,
    ) -> Result<(), ServiceError> {
        let mut failures: Vec<String> = Vec::new();
        for (name, def) in &config.shared_services {
            let Some(capacity) = def.capacity else {
                continue;
            };
            // A second pass empties the replacements of fresh slots that failed in the first.
            for _ in 0..2 {
                let mut swapped = false;
                for pending in self.slots.pending(name) {
                    let at = pending.allocation;
                    match self.reset_slot(config, name, def, at) {
                        Ok(true) => self.slots.cleared(name, at)?,
                        Ok(false) if !require_running => {}
                        Ok(false) => failures.push(format!(
                            "{name} instance {} slot {}: its container is not running",
                            at.instance, at.slot
                        )),
                        Err(error) => {
                            failures.push(format!(
                                "{name} instance {} slot {}: {error}",
                                at.instance, at.slot
                            ));
                            if let Some(owner) = pending.owner {
                                self.slots.reject(name, &owner)?;
                                self.slots.allocate(name, &owner, capacity)?;
                                swapped = true;
                            }
                        }
                    }
                }
                if !swapped {
                    break;
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ServiceError::Io(std::io::Error::other(format!(
                "could not empty shared slots (they stay out of use): {}",
                failures.join("; ")
            ))))
        }
    }

    /// Runs the service's `slot_reset` for one slot; `false` when its container is not running.
    fn reset_slot(
        &self,
        config: &Config,
        name: &str,
        def: &SharedServiceDef,
        allocation: pom_env::SlotAllocation,
    ) -> Result<bool, ServiceError> {
        if def.slot_reset.is_empty() {
            return Ok(true);
        }
        let service = if allocation.instance == 0 {
            name.to_string()
        } else {
            format!("{name}-{}", allocation.instance + 1)
        };
        let file = self.write_shared_compose(config)?;
        let running =
            self.docker(&self.compose_args(&file, &["ps", "--status", "running", "-q", &service]))?;
        if !running.status.success() || String::from_utf8_lossy(&running.stdout).trim().is_empty() {
            return Ok(false);
        }
        let command = def
            .slot_reset
            .replace("{{slot}}", &allocation.slot.to_string());
        let args = self.compose_args(&file, &["exec", "-T", &service, "sh", "-c", &command]);
        let mut last = String::new();
        // A container just brought up may still be loading its data.
        for attempt in 0..SLOT_RESET_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(SLOT_RESET_PAUSE);
            }
            let output = self.docker(&args)?;
            if output.status.success() {
                return Ok(true);
            }
            last = String::from_utf8_lossy(&output.stderr).trim().to_string();
        }
        Err(ServiceError::Io(std::io::Error::other(
            if last.is_empty() {
                format!("`{command}` failed in {service}")
            } else {
                format!("`{command}` failed in {service}: {last}")
            },
        )))
    }

    pub fn shared_action(
        &self,
        config: &Config,
        name: &str,
        action: SharedAction,
    ) -> Result<(), ServiceError> {
        let Some(def) = config.shared_services.get(name) else {
            return Err(ServiceError::UnknownService(name.to_string()));
        };
        if def.is_command() {
            self.reserve_shared_ports(config);
            return match action {
                SharedAction::Start => self.start_shared_command(config, name).map(|_| ()),
                SharedAction::Stop => self.stop_shared_command(name).map_err(Into::into),
                SharedAction::Restart => {
                    self.stop_shared_command(name)?;
                    self.start_shared_command(config, name).map(|_| ())
                }
            };
        }
        let file = self.write_shared_compose(config)?;
        match action {
            SharedAction::Start => self.compose(&file, &["up", "-d", name]),
            SharedAction::Restart => self.compose(&file, &["up", "-d", "--force-recreate", name]),
            SharedAction::Stop => self.compose(&file, &["stop", name]),
        }
    }

    pub fn stop_shared(&self) -> Result<(), ServiceError> {
        let commands = self.shared_command_holders();
        self.holders().kill_holders_now(&commands);
        if !self.compose_file().is_file() {
            return Ok(());
        }
        self.compose(&self.compose_file(), &["stop"])
    }

    /// The running holders of command shared services, named as `shared_holder` names them.
    fn shared_command_holders(&self) -> Vec<String> {
        let prefix = self.shared_holder("");
        self.holders()
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| name.starts_with(&prefix) && self.holders().holder_alive(name))
            .collect()
    }

    /// Shared services that run right now: commands by their holder, containers as Docker reports them (none
    /// when Docker is unreachable).
    pub fn shared_running(&self) -> HashSet<String> {
        let prefix = self.shared_holder("");
        let mut running: HashSet<String> = self
            .shared_command_holders()
            .iter()
            .filter_map(|holder| holder.strip_prefix(&prefix).map(str::to_string))
            .collect();
        let file = self.compose_file();
        if !file.is_file() {
            return running;
        }
        let args = self.compose_args(&file, &["ps", "--services", "--filter", "status=running"]);
        if let Ok(stdout) = self.docker_within(&args, SHARED_PS_TIMEOUT) {
            running.extend(
                stdout
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string),
            );
        }
        running
    }

    /// Creates the workspace's databases in the shared Postgres; existing ones are left alone.
    pub fn ensure_databases(&self, config: &Config, branch: &str) -> Result<(), ServiceError> {
        self.create_databases(config, &database_names(config, branch))
    }

    /// Where the shared Postgres listens and how to log in (the service with a `db_user`, else the defaults).
    pub fn postgres_endpoint(&self, config: &Config) -> Endpoint {
        let def = config
            .shared_services
            .values()
            .find(|def| !def.db_user.is_empty());
        let pick = |value: Option<&String>, fallback: &str| {
            value
                .filter(|value| !value.is_empty())
                .cloned()
                .unwrap_or_else(|| fallback.to_string())
        };
        Endpoint {
            host: pick(def.map(|def| &def.host), "localhost"),
            port: match self.shared_host_port(POSTGRES) {
                0 => DEFAULT_PG_PORT,
                port => port,
            },
            user: pick(def.map(|def| &def.db_user), POSTGRES),
            password: pick(def.map(|def| &def.db_password), POSTGRES),
        }
    }

    /// The Redis URL a workspace uses for shared service `name`: its instance's port and its slot's database.
    pub fn redis_url(&self, name: &str, branch: &str) -> String {
        let slot = self.slots.get(name, &pom_env::port_ws_key(branch));
        let base = match self.shared_host_port(name) {
            0 => DEFAULT_REDIS_PORT,
            port => port,
        };
        let port = base + slot.map_or(0, |slot| slot.instance);
        format!(
            "redis://localhost:{port}/{}",
            slot.map_or(0, |slot| slot.slot)
        )
    }

    /// The shared Postgres: its connection and the container publishing it, if one of ours does.
    pub(crate) fn postgres(&self, config: &Config) -> Postgres {
        let endpoint = self.postgres_endpoint(config);
        Postgres {
            container: self.published_container(endpoint.port),
            host: endpoint.host,
            user: endpoint.user,
            password: endpoint.password,
            port: endpoint.port,
        }
    }

    /// Creates each database unless it exists (pg_stat_statements enabled in new ones).
    pub fn create_databases(&self, config: &Config, names: &[String]) -> Result<(), ServiceError> {
        if names.is_empty() {
            return Ok(());
        }
        let postgres = self.postgres(config);
        let mut failures = Vec::new();
        for name in names {
            let output = self.psql(&postgres, POSTGRES, &format!("CREATE DATABASE \"{name}\""))?;
            let text = output_text(&output);
            if !output.status.success() && !text.contains("already exists") {
                failures.push(format!("{name}: {}", text.trim()));
                continue;
            }
            let extension = "CREATE EXTENSION IF NOT EXISTS pg_stat_statements";
            if let Err(error) = self.psql(&postgres, name, extension) {
                eprintln!("services: pg_stat_statements on {name}: {error}");
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ServiceError::Io(std::io::Error::other(format!(
                "could not create databases: {}",
                failures.join("; ")
            ))))
        }
    }

    /// Every user database in the shared Postgres.
    pub fn list_databases(&self, config: &Config) -> Result<Vec<String>, ServiceError> {
        let postgres = self.postgres(config);
        let sql =
            "SELECT datname FROM pg_database WHERE NOT datistemplate AND datname <> 'postgres'";
        let output = self.docker(&postgres.psql_args(POSTGRES, &["-tAc", sql]))?;
        if !output.status.success() {
            return Err(docker_failure(&output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub fn database_exists(&self, config: &Config, name: &str) -> bool {
        let postgres = self.postgres(config);
        let query = format!(
            "SELECT 1 FROM pg_database WHERE datname = '{}'",
            name.replace('\'', "''")
        );
        self.psql_rows(&postgres, &query)
            .is_ok_and(|rows| rows.trim() == "1")
    }

    /// Replaces `target` with a copy of `template` (a workspace starting from main's data).
    pub fn clone_database(
        &self,
        config: &Config,
        template: &str,
        target: &str,
    ) -> Result<(), ServiceError> {
        let postgres = self.postgres(config);
        self.terminate(&postgres, target);
        self.drop_one(&postgres, target)?;
        // Postgres refuses to copy a database anyone is connected to.
        self.terminate(&postgres, template);
        let output = self.psql(
            &postgres,
            POSTGRES,
            &format!("CREATE DATABASE \"{target}\" TEMPLATE \"{template}\""),
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(ServiceError::Io(std::io::Error::other(format!(
                "clone {target} from {template}: {}",
                output_text(&output).trim()
            ))))
        }
    }

    /// Drops the databases, disconnecting whoever uses them.
    pub fn drop_databases(&self, config: &Config, names: &[String]) -> Result<(), ServiceError> {
        if names.is_empty() {
            return Ok(());
        }
        let postgres = self.postgres(config);
        let mut failures = Vec::new();
        for name in names {
            self.terminate(&postgres, name);
            if let Err(error) = self.drop_one(&postgres, name) {
                failures.push(error.to_string());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ServiceError::Io(std::io::Error::other(failures.join("; "))))
        }
    }

    fn drop_one(&self, postgres: &Postgres, name: &str) -> Result<(), ServiceError> {
        let output = self.psql(
            postgres,
            POSTGRES,
            &format!("DROP DATABASE IF EXISTS \"{name}\""),
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(ServiceError::Io(std::io::Error::other(format!(
                "drop {name}: {}",
                output_text(&output).trim()
            ))))
        }
    }

    pub(crate) fn terminate(&self, postgres: &Postgres, name: &str) {
        let sql = format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}' AND pid <> pg_backend_pid()",
            name.replace('\'', "''")
        );
        if let Err(error) = self.psql(postgres, POSTGRES, &sql) {
            eprintln!("services: disconnect {name}: {error}");
        }
    }

    fn ensure_network(&self) -> Result<(), ServiceError> {
        let output = self.docker(&["network".into(), "create".into(), SHARED_NETWORK.into()])?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() || stderr.contains("already exists") {
            Ok(())
        } else {
            Err(docker_failure(&output))
        }
    }

    fn reserve_shared_ports(&self, config: &Config) {
        let mut names: Vec<&String> = config.shared_services.keys().collect();
        names.sort();
        for name in names {
            let Some(def) = config.shared_services.get(name) else {
                continue;
            };
            for index in 0..def.ports.len().max(1) {
                let base = if def.is_command() {
                    def.port.unwrap_or(0)
                } else {
                    host_port_base(def, index)
                };
                let key = pom_ports::shared_key(name, index);
                // A cmd's own port is the one it is told: searching past it would point refs elsewhere.
                let span = if def.is_command() && def.port.is_some() {
                    0
                } else {
                    PREFERRED_SPAN
                };
                if self.ports.acquire_preferred(&key, base, span).is_none() {
                    eprintln!("services: no free port for shared {name}");
                }
            }
        }
    }

    fn shared_port_at(&self, name: &str, index: usize) -> u16 {
        self.ports
            .port_of(&pom_ports::shared_key(name, index))
            .or_else(|| self.composed_port(name, index))
            .unwrap_or_else(|| {
                let lease_name = if index == 0 {
                    name.to_string()
                } else {
                    format!("{name}#{index}")
                };
                pom_env::stable_shared_port(&self.session, &lease_name)
            })
    }

    fn published_container(&self, port: u16) -> Option<String> {
        let args = [
            "ps",
            "-q",
            "--filter",
            &format!("publish={port}"),
            "--filter",
            "status=running",
        ]
        .map(str::to_string);
        let output = self.docker(&args).ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }

    /// The running container publishing host `port`, if Docker answers within a few seconds.
    pub fn port_owner(&self, port: u16) -> Option<PortOwner> {
        let args = [
            "ps".to_string(),
            "--filter".into(),
            format!("publish={port}"),
            "--filter".into(),
            "status=running".into(),
            "--format".into(),
            "{{.Names}}\t{{.Label \"com.docker.compose.project\"}}".into(),
        ];
        match self.docker_within(&args, PORT_OWNER_TIMEOUT) {
            Ok(stdout) => parse_port_owner(&stdout),
            Err(error) => {
                eprintln!("services: who publishes {port}: {error}");
                None
            }
        }
    }

    /// Whether `owner` is a container of this project's shared stack.
    pub fn owns(&self, owner: &PortOwner) -> bool {
        owner.project == self.compose_project()
    }

    fn docker_within(&self, args: &[String], timeout: Duration) -> Result<String, String> {
        let mut command = Command::new(&self.docker);
        command
            .args(args)
            .current_dir(&self.project_root)
            .env("PATH", crate::tool_path());
        run_within(&mut command, timeout)
    }

    pub(crate) fn psql(
        &self,
        postgres: &Postgres,
        database: &str,
        sql: &str,
    ) -> Result<Output, ServiceError> {
        self.docker(&postgres.psql_args(database, &["-c", sql]))
    }

    /// A query's rows as unaligned text.
    pub(crate) fn psql_rows(&self, postgres: &Postgres, sql: &str) -> Result<String, ServiceError> {
        let output = self.docker(&postgres.psql_args(POSTGRES, &["-tAc", sql]))?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn compose_args(&self, file: &Path, rest: &[&str]) -> Vec<String> {
        let mut args = vec![
            "compose".to_string(),
            "-f".to_string(),
            file.to_string_lossy().into_owned(),
            "-p".to_string(),
            self.compose_project(),
        ];
        args.extend(rest.iter().map(|arg| arg.to_string()));
        args
    }

    fn compose(&self, file: &Path, rest: &[&str]) -> Result<(), ServiceError> {
        let output = self.docker(&self.compose_args(file, rest))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(docker_failure(&output))
        }
    }

    pub(crate) fn docker(&self, args: &[String]) -> Result<Output, ServiceError> {
        Command::new(&self.docker)
            .args(args)
            .current_dir(&self.project_root)
            .env("PATH", crate::tool_path())
            .output()
            .map_err(|error| {
                ServiceError::Io(std::io::Error::new(
                    error.kind(),
                    format!("docker: {error}"),
                ))
            })
    }
}

/// The slot keys of every workspace folder on disk. A branch with `/` nests (`workspace--feat/login`), so
/// every level counts: keeping a stale slot is harmless, emptying a live one is not.
fn workspace_keys_on_disk(project_root: &Path) -> HashSet<String> {
    fn walk(folder: &Path, branch: &str, depth: usize, keys: &mut HashSet<String>) {
        keys.insert(pom_env::port_ws_key(branch));
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && entry.path().is_dir() {
                walk(&entry.path(), &format!("{branch}/{name}"), depth - 1, keys);
            }
        }
    }
    let mut keys = HashSet::new();
    let Ok(entries) = std::fs::read_dir(project_root) else {
        return keys;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(branch) = name.strip_prefix("workspace--") {
            if entry.path().is_dir() {
                walk(&entry.path(), branch, 3, &mut keys);
            }
        }
    }
    keys
}

pub(crate) struct Postgres {
    pub(crate) container: Option<String>,
    host: String,
    port: u16,
    user: String,
    password: String,
}

impl Postgres {
    /// Docker arguments running `psql` on `database`: inside our container, or a throwaway client.
    fn psql_args(&self, database: &str, rest: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = match &self.container {
            Some(container) => ["exec", container, "psql", "-U", &self.user, "-d", database]
                .map(str::to_string)
                .to_vec(),
            None => vec![
                "run".into(),
                "--rm".into(),
                format!("--add-host={}:host-gateway", self.host),
                PSQL_IMAGE.into(),
                "psql".into(),
                format!(
                    "postgresql://{}:{}@{}:{}/{database}",
                    self.user, self.password, self.host, self.port
                ),
            ],
        };
        args.extend(rest.iter().map(|arg| arg.to_string()));
        args
    }
}

pub(crate) fn output_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr)
}

/// Database names of a workspace: shared-service refs as named, and `databases:` session-prefixed.
pub fn database_names(config: &Config, branch: &str) -> Vec<String> {
    database_names_where(config, branch, |_| true)
}

/// `database_names` limited to the repos `keep` accepts (the ones checked out in the workspace).
pub fn database_names_where(
    config: &Config,
    branch: &str,
    keep: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut names = Vec::new();
    for (repo, dir) in &config.repos {
        if !dir.has_worktree_config() || !keep(repo) {
            continue;
        }
        for shared in &dir.shared_refs {
            if !shared.db_name.is_empty() {
                names.push(pom_env::resolve_branch_tokens(&shared.db_name, branch));
            }
        }
        for template in dir.databases.values() {
            names.push(format!(
                "{}_{}",
                config.session,
                pom_env::resolve_branch_tokens(template, branch)
            ));
        }
    }
    names
}

fn docker_failure(output: &Output) -> ServiceError {
    let mut text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if text.is_empty() {
        text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    }
    ServiceError::Io(std::io::Error::other(format!("docker: {text}")))
}

/// `host:container` -> `container`; a bare port is both.
fn container_port(mapping: &str) -> &str {
    mapping
        .split_once(':')
        .map_or(mapping, |(_, container)| container)
}

/// The usual host port of the `index`th mapping, tried first when leasing.
fn host_port_base(def: &SharedServiceDef, index: usize) -> u16 {
    def.ports
        .get(index)
        .and_then(|mapping| mapping.split([':', '/']).next())
        .and_then(|port| port.trim().parse().ok())
        .unwrap_or(0)
}

/// Extra instances get their own named volume (`data-2:/var/lib/...`); bind mounts are shared.
fn instance_volume(volume: &str, instance: u16) -> String {
    if instance == 0 {
        return volume.to_string();
    }
    match volume.split_once(':') {
        Some((source, target)) if !source.starts_with('.') && !source.starts_with('/') => {
            format!("{source}-{}:{target}", instance + 1)
        }
        _ => volume.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reclaimed_lease_still_finds_the_port_the_compose_file_publishes() {
        let text = "name: demo-shared\n\nservices:\n  postgres:\n    image: postgres:16\n    ports:\n      - \"5434:5432\"\n    environment:\n      POSTGRES_USER: \"postgres\"\n\n  minio:\n    ports:\n      - \"9000:9000\"\n      - \"9001:9001\"\n";
        assert_eq!(composed_port(text, "postgres", 0), Some(5434));
        assert_eq!(composed_port(text, "minio", 1), Some(9001));
        assert_eq!(composed_port(text, "postgres", 1), None);
        assert_eq!(composed_port(text, "redis", 0), None);
    }

    #[test]
    fn the_port_owner_is_the_first_container_listed() {
        assert_eq!(
            parse_port_owner("other-shared-postgres-1\tother-shared\nsecond\tx\n"),
            Some(PortOwner {
                container: "other-shared-postgres-1".into(),
                project: "other-shared".into(),
            })
        );
        assert_eq!(
            parse_port_owner("loose\n"),
            Some(PortOwner {
                container: "loose".into(),
                project: String::new(),
            })
        );
        assert_eq!(parse_port_owner("\n"), None);
    }

    #[test]
    fn volumes_and_ports_per_instance() {
        assert_eq!(
            instance_volume("pgdata:/var/lib/pg", 0),
            "pgdata:/var/lib/pg"
        );
        assert_eq!(
            instance_volume("pgdata:/var/lib/pg", 1),
            "pgdata-2:/var/lib/pg"
        );
        assert_eq!(instance_volume("./init:/docker", 2), "./init:/docker");
        assert_eq!(container_port("5432:5432"), "5432");
        assert_eq!(container_port("6379"), "6379");
    }
}
