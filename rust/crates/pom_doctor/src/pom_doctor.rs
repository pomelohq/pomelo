mod disk;
mod git_auth;
mod limits;
mod linux;
mod server;
mod shadow;

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pom_config::Config;
use serde::Serialize;

pub use disk::{
    df_usage, disk_findings, docker_root, filesystems, format_bytes, thin_pools, Filesystem, Place,
    ThinPool, Usage, FULL_PERCENT,
};
pub use git_auth::{git_auth_findings, probe_origin, OriginAnswer, OriginProbe};
pub use limits::{limit_findings, LimitSupport};
pub use linux::{high_load, in_lxc, linux_findings, lxc_docker, stuck_finding, stuck_processes};
pub use server::{server_findings, OtherServer, RunningServer, ServerSurvey, SocketDirState};
pub use shadow::{path_shadow, pom_on_path, pom_version};

const DOCKER_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
    Ok,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fix: String,
    pub agent_fixable: bool,
}

impl Finding {
    fn new(id: impl Into<String>, severity: Severity, title: impl Into<String>) -> Finding {
        Finding {
            id: id.into(),
            severity,
            title: title.into(),
            detail: String::new(),
            fix: String::new(),
            agent_fixable: false,
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Finding {
        self.detail = detail.into();
        self
    }

    fn fix(mut self, fix: impl Into<String>) -> Finding {
        self.fix = fix.into();
        self
    }

    fn agent_fixable(mut self) -> Finding {
        self.agent_fixable = true;
        self
    }
}

/// How the machine answers the tool checks; tests swap in their own.
pub struct Machine<'a> {
    pub has_tool: &'a dyn Fn(&str) -> bool,
    pub docker_running: &'a dyn Fn() -> bool,
    pub compose_installed: &'a dyn Fn() -> bool,
}

const INSTALL_DOCKER: &str = if cfg!(target_os = "macos") {
    "install Docker Desktop / OrbStack"
} else {
    "install Docker Engine and its Compose plugin (Ubuntu: `sudo apt install docker.io docker-compose-v2`; others: docs.docker.com/engine/install)"
};

const START_DOCKER: &str = if cfg!(target_os = "macos") {
    "start Docker"
} else {
    "start it with `sudo systemctl start docker`; if it runs, let your user reach it: `sudo usermod -aG docker $USER`, then log in again"
};

const INSTALL_COMPOSE: &str = if cfg!(target_os = "macos") {
    "update Docker Desktop / OrbStack, which ship Compose"
} else {
    "install the Compose plugin (Ubuntu: `sudo apt install docker-compose-v2`; others: docs.docker.com/compose/install/linux)"
};

pub fn on_path(path: &str, tool: &str) -> bool {
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .any(|dir| Path::new(dir).join(tool).is_file())
}

/// Shared services run as a compose project; Linux packages of Docker often leave the plugin out.
pub fn compose_answers(path: &str) -> bool {
    let mut command = Command::new("docker");
    command.args(["compose", "version"]).env("PATH", path);
    disk::output_within(&mut command, DOCKER_TIMEOUT).is_some()
}

pub fn docker_answers(path: &str) -> bool {
    let Ok(mut child) = Command::new("docker")
        .arg("info")
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + DOCKER_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                if let Err(error) = child.kill() {
                    eprintln!("doctor: stop docker info: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("doctor: reap docker info: {error}");
                }
                return false;
            }
        }
    }
}

fn template_refs(text: &str) -> Vec<&str> {
    let mut refs = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            break;
        };
        refs.push(after[..close].trim());
        rest = &after[close + 2..];
    }
    refs
}

/// Everything standing between the project and a first run, most blocking first; a single `ok` finding when
/// nothing does.
/// The program a shell command runs first, past `KEY=value` assignments and `exec`/`env`; none for a path
/// (a script in the project), which the PATH can't vouch for.
fn command_program(command: &str) -> Option<&str> {
    command
        .split_whitespace()
        .find(|word| !word.contains('=') && *word != "exec" && *word != "env")
        .filter(|program| !program.contains('/'))
}

pub fn diagnose(
    config: Option<&Config>,
    config_path: &Path,
    project_root: &Path,
    secret_names: &[String],
    machine: &Machine<'_>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(config) = config else {
        out.push(
            Finding::new("config.load", Severity::Error, "No project config")
                .detail("pom.yml not found or failed to load")
                .fix("run `pom init` or open a project folder"),
        );
        return out;
    };
    if let Err(error) = config.validate() {
        out.push(
            Finding::new("config.validate", Severity::Error, "Config is invalid")
                .detail(error)
                .fix("fix the reported key (typo'd alias / unknown {{var}})")
                .agent_fixable(),
        );
    }
    if !(machine.has_tool)("git") {
        out.push(Finding::new("tool.git", Severity::Error, "git not found").fix("install git"));
    }
    let shared = config.shared_services.values().any(|def| !def.is_command());
    if !(machine.has_tool)("docker") {
        if shared {
            out.push(
                Finding::new("tool.docker", Severity::Error, "docker not found")
                    .detail("shared services (postgres/redis/...) need Docker")
                    .fix(INSTALL_DOCKER),
            );
        }
    } else if shared && !(machine.docker_running)() {
        out.push(
            Finding::new("docker.down", Severity::Error, "Docker not running")
                .detail("shared services can't start until Docker is up")
                .fix(START_DOCKER),
        );
    } else if shared && !(machine.compose_installed)() {
        out.push(
            Finding::new(
                "docker.compose",
                Severity::Error,
                "Docker Compose not found",
            )
            .detail("shared services run as a Compose project (`docker compose`)")
            .fix(INSTALL_COMPOSE),
        );
    }
    for (name, def) in &config.shared_services {
        let Some(program) = command_program(&def.cmd) else {
            continue;
        };
        if !(machine.has_tool)(program) {
            out.push(
                Finding::new(
                    format!("tool.shared:{name}"),
                    Severity::Error,
                    format!("{program} not found for shared service {name}"),
                )
                .detail(format!(
                    "shared_services.{name}.cmd runs `{program}`, which is not on the PATH"
                ))
                .fix(format!(
                    "install {program}, or change shared_services.{name}.cmd"
                )),
            );
        }
    }
    let default_branch = config.global_default_branch();
    for name in config.repos.keys() {
        let path = pom_layout::repo_worktree(project_root, name, default_branch, true);
        if !path.is_dir() {
            out.push(
                Finding::new(
                    format!("repo.missing:{name}"),
                    Severity::Error,
                    format!("Repo not found: {name}"),
                )
                .detail(path.to_string_lossy())
                .fix("clone/prepare it (run `pom prepare-main`), or fix the repo path in config"),
            );
        }
    }
    let removed = pom_config::maintain::removed_keys_in(config_path);
    if !removed.is_empty() {
        out.push(
            Finding::new(
                "config.removed",
                Severity::Error,
                format!("Removed config keys: {}", removed.join(", ")),
            )
            .detail("these keys were removed from the schema and are ignored")
            .fix("run config_normalize (or delete them)")
            .agent_fixable(),
        );
    }
    let merged = std::fs::read_to_string(config_path).unwrap_or_default();
    let refs = template_refs(&merged);
    let mut missing: Vec<&str> = refs
        .iter()
        .filter_map(|reference| reference.strip_prefix("secret."))
        .filter(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .filter(|name| !secret_names.iter().any(|known| known == name))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    for name in missing {
        out.push(
            Finding::new(
                format!("secret.missing:{name}"),
                Severity::Warn,
                format!("Secret not set: {name}"),
            )
            .detail(format!(
                "config uses {{{{secret.{name}}}}} but no value is stored"
            ))
            .fix("add it in the Services panel > Secrets (the key button)"),
        );
    }
    let mut services: Vec<&String> = config.shared_services.keys().collect();
    services.sort();
    for name in services {
        let prefix = format!("shared.{name}.");
        if !refs.iter().any(|reference| reference.starts_with(&prefix)) {
            out.push(
                Finding::new(
                    format!("shared.unwired:{name}"),
                    Severity::Warn,
                    format!("Shared service not wired: {name}"),
                )
                .detail(format!(
                    "shared_services.{name} is declared but no repo env references {{{{shared.{name}.url}}}} / {{{{shared.{name}.host}}}} / {{{{shared.{name}.port}}}} - its services will start with no connection info"
                ))
                .fix(format!(
                    "add the connection env to the repos that need it (e.g. DATABASE_URL: postgresql://{{{{shared.{name}.url}}}}/{{{{db.main}}}})"
                ))
                .agent_fixable(),
            );
        }
    }
    if out.is_empty() {
        out.push(Finding::new("ok", Severity::Ok, "Ready to run").detail("no blocking gaps found"));
    }
    out
}

/// Someone other than this machine's user can reach the dev proxy (`reach` says how: a non-loopback bind or a
/// public domain): it refuses to start without a token unless the risk was accepted. `refusal` is why it will
/// not start, if it will not.
pub fn proxy_exposure(
    reach: Option<String>,
    trusted_proxies: usize,
    has_token: bool,
    refusal: Option<String>,
) -> Option<Finding> {
    let reach = reach?;
    if let Some(reason) = refusal {
        return Some(
            Finding::new("proxy.refused", Severity::Error, "Dev proxy will not start")
                .detail(reason)
                .fix("`pom proxy token new` (requests from outside then present it), or `pom proxy config reset bind` and `pom proxy config reset domain`"),
        );
    }
    if has_token {
        return None;
    }
    let detail = if trusted_proxies == 0 {
        format!("{reach}, proxy.allow_unauthenticated is on and proxy.trusted_proxies is empty: anyone who reaches the proxy opens every workspace's services, and forwarded headers from a reverse proxy are ignored")
    } else {
        format!("{reach} and proxy.allow_unauthenticated is on: anyone who reaches the proxy opens every workspace's services")
    };
    Some(
        Finding::new("proxy.exposed", Severity::Warn, "Dev proxy reachable from other machines with no token")
            .detail(detail)
            .fix("`pom proxy token new`, or keep the address on a private network (Tailscale, a LAN) or behind an authenticating reverse proxy, or `pom proxy config reset bind`"),
    )
}

/// Peers outside `proxy.trusted_proxies` that the dev proxy saw send forwarded headers lately: most likely a
/// reverse proxy left off the list, whose headers are ignored and whose requests all need the token.
pub fn untrusted_forwarders(peers: &[String]) -> Option<Finding> {
    let first = peers.first()?;
    Some(
        Finding::new(
            "proxy.untrusted_forwarder",
            Severity::Warn,
            "Dev proxy gets forwarded requests from an untrusted peer",
        )
        .detail(format!(
            "{} sent X-Forwarded-For, X-Real-IP or Forwarded but is not in proxy.trusted_proxies: its forwarded headers are ignored and every request it forwards needs the proxy token",
            peers.join(", ")
        ))
        .fix(format!(
            "if it is your reverse proxy, `pom proxy config set trusted_proxies {first}`; otherwise find what on that address forwards to the proxy"
        )),
    )
}

/// Adds `finding`, dropping the lone "ready" finding it contradicts.
pub fn add_finding(findings: &mut Vec<Finding>, finding: Finding) {
    findings.retain(|existing| existing.severity != Severity::Ok);
    findings.push(finding);
}

/// Holders still running for a workspace or project that is gone.
pub fn orphan_holders_finding(count: usize, memory: &str, listed: &[String]) -> Finding {
    Finding::new(
        "holders.orphan",
        Severity::Warn,
        format!("{count} holder(s) outlive their workspace ({memory})"),
    )
    .detail(format!(
        "nothing in pom reaches them any more: {}",
        listed.join(", ")
    ))
    .fix("pom ps --orphans to list them, pom ps --orphans --kill to stop them")
}

/// How long one doctor check took, and whether it gave up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CheckTiming {
    pub name: String,
    pub duration_ms: u64,
    pub timed_out: bool,
}

/// One independent doctor check: a name and the work that yields its findings.
pub type Check = (&'static str, Box<dyn FnOnce() -> Vec<Finding> + Send>);

/// Runs `checks` side by side and waits at most `timeout` for them; a check still running then is reported
/// as timed out and left to finish on its own. Findings keep the checks' order.
pub fn run_checks(checks: Vec<Check>, timeout: Duration) -> (Vec<Vec<Finding>>, Vec<CheckTiming>) {
    let started = Instant::now();
    let deadline = started + timeout;
    let (sender, receiver) = std::sync::mpsc::channel();
    let names: Vec<&'static str> = checks.iter().map(|(name, _)| *name).collect();
    let mut results: Vec<Option<(Vec<Finding>, u64)>> = names.iter().map(|_| None).collect();
    for (index, (name, check)) in checks.into_iter().enumerate() {
        let sender = sender.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("doctor-{name}"))
            .spawn(move || {
                let findings = check();
                let elapsed = started.elapsed().as_millis() as u64;
                if sender.send((index, findings, elapsed)).is_err() {
                    eprintln!("doctor: {name} finished after the report");
                }
            });
        if let Err(error) = spawned {
            results[index] = Some((
                vec![Finding::new(
                    format!("doctor.check:{name}"),
                    Severity::Warn,
                    format!("Could not run the {name} check"),
                )
                .detail(error.to_string())],
                0,
            ));
        }
    }
    drop(sender);
    while results.iter().any(Option::is_none) {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok((index, findings, elapsed)) => {
                if let Some(slot) = results.get_mut(index) {
                    *slot = Some((findings, elapsed));
                }
            }
            Err(_) => break,
        }
    }
    let mut findings = Vec::new();
    let mut timings = Vec::new();
    for (name, result) in names.into_iter().zip(results) {
        match result {
            Some((found, elapsed)) => {
                findings.push(found);
                timings.push(CheckTiming {
                    name: name.to_string(),
                    duration_ms: elapsed,
                    timed_out: false,
                });
            }
            None => {
                findings.push(vec![Finding::new(
                    format!("doctor.timeout:{name}"),
                    Severity::Warn,
                    format!("The {name} check did not finish within {}s", timeout.as_secs()),
                )
                .detail("something it asks (Docker, a disk, a process) is not answering")
                .fix(format!("run `pom doctor` again; if the {name} check keeps timing out, look at what it probes"))]);
                timings.push(CheckTiming {
                    name: name.to_string(),
                    duration_ms: timeout.as_millis() as u64,
                    timed_out: true,
                });
            }
        }
    }
    (findings, timings)
}

/// `{findings, errors, warnings}`, the shape the previous core's doctor endpoint returned.
pub fn report(findings: &[Finding]) -> serde_json::Value {
    let count = |severity: Severity| {
        findings
            .iter()
            .filter(|finding| finding.severity == severity)
            .count()
    };
    serde_json::json!({
        "findings": findings,
        "errors": count(Severity::Error),
        "warnings": count(Severity::Warn),
    })
}

pub fn fix_prompt(findings: &[Finding]) -> String {
    let gaps: Vec<String> = findings
        .iter()
        .filter(|finding| finding.severity != Severity::Ok)
        .map(|finding| {
            let severity = match finding.severity {
                Severity::Error => "error",
                Severity::Warn => "warn",
                Severity::Ok => "ok",
            };
            if finding.detail.is_empty() {
                format!("- [{severity}] {}", finding.title)
            } else {
                format!("- [{severity}] {} ({})", finding.title, finding.detail)
            }
        })
        .collect();
    format!(
        "Make this project runnable. Current config_doctor findings:\n{}\n\nFix them via the pom MCP config tools, then loop config_doctor until it reports no\nerrors. Ask me only for values you can't infer (a real secret value, a repo's clone URL).",
        gaps.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_run_side_by_side_and_a_slow_one_times_out() {
        let finding = |id: &str| Finding::new(id, Severity::Warn, id);
        let checks: Vec<Check> = vec![
            ("quick", Box::new(move || vec![finding("quick")])),
            (
                "slow",
                Box::new(move || {
                    std::thread::sleep(Duration::from_secs(5));
                    vec![finding("slow")]
                }),
            ),
            (
                "medium",
                Box::new(move || {
                    std::thread::sleep(Duration::from_millis(300));
                    vec![finding("medium")]
                }),
            ),
        ];
        let started = Instant::now();
        let (findings, timings) = run_checks(checks, Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_secs(2));
        let ids: Vec<&str> = findings
            .iter()
            .flatten()
            .map(|finding| finding.id.as_str())
            .collect();
        assert_eq!(ids, ["quick", "doctor.timeout:slow", "medium"]);
        let timed_out: Vec<(&str, bool)> = timings
            .iter()
            .map(|timing| (timing.name.as_str(), timing.timed_out))
            .collect();
        assert_eq!(
            timed_out,
            [("quick", false), ("slow", true), ("medium", false)]
        );
        assert!(timings[2].duration_ms >= 300);
    }

    #[test]
    fn untrusted_forwarders_are_named_with_the_fix() {
        assert!(untrusted_forwarders(&[]).is_none());
        let finding =
            untrusted_forwarders(&["127.0.0.1".into(), "10.0.0.5".into()]).expect("finding");
        assert_eq!(finding.id, "proxy.untrusted_forwarder");
        assert_eq!(finding.severity, Severity::Warn);
        assert!(finding.detail.starts_with("127.0.0.1, 10.0.0.5 sent"));
        assert!(finding
            .fix
            .contains("pom proxy config set trusted_proxies 127.0.0.1"));
    }

    #[test]
    fn only_a_proxy_reachable_from_outside_is_reported() {
        let bind = |address: &str| Some(format!("proxy.bind is {address}"));
        assert!(proxy_exposure(None, 0, false, None).is_none());
        assert!(proxy_exposure(bind("100.64.0.10"), 0, true, None).is_none());
        let domain = proxy_exposure(
            Some("proxy.domain is dev.example.net".into()),
            1,
            false,
            None,
        )
        .unwrap();
        assert!(domain
            .detail
            .starts_with("proxy.domain is dev.example.net and"));
        let refused = proxy_exposure(
            bind("0.0.0.0"),
            0,
            false,
            Some("refusing to listen on 0.0.0.0".into()),
        )
        .unwrap();
        assert_eq!(refused.severity, Severity::Error);
        assert_eq!(refused.id, "proxy.refused");
        let finding = proxy_exposure(bind("100.64.0.10"), 0, false, None).unwrap();
        assert_eq!(finding.severity, Severity::Warn);
        assert!(finding.detail.contains("trusted_proxies is empty"));
        let mut findings = vec![Finding::new("ok", Severity::Ok, "Ready to run")];
        add_finding(&mut findings, finding);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "proxy.exposed");
    }

    fn write(root: &Path, yaml: &str) -> std::path::PathBuf {
        let path = root.join("pom.yml");
        if let Err(error) = std::fs::write(&path, yaml) {
            panic!("write pom.yml: {error}");
        }
        path
    }

    fn machine(docker: bool) -> (impl Fn(&str) -> bool, impl Fn() -> bool) {
        (
            move |tool: &str| tool == "git" || (docker && tool == "docker"),
            move || docker,
        )
    }

    #[test]
    fn a_command_shared_service_needs_its_program_not_docker() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = write(
            temp.path(),
            "session: demo\nshared_services:\n  mock-as:\n    cmd: PORT_HINT=1 exec deno run mock.ts\nrepos:\n  api:\n    env:\n      AS: \"{{shared.mock-as.url}}\"\n    services:\n      web: rails s\n",
        );
        std::fs::create_dir_all(temp.path().join("workspace--main/api"))
            .map_err(|error| error.to_string())?;
        let config = Config::load(&path).map_err(|error| error.message)?;
        let (has_tool, docker) = machine(false);
        let findings = diagnose(
            Some(&config),
            &path,
            temp.path(),
            &[],
            &Machine {
                has_tool: &has_tool,
                docker_running: &docker,
                compose_installed: &|| true,
            },
        );
        let ids: Vec<&str> = findings.iter().map(|finding| finding.id.as_str()).collect();
        assert_eq!(
            ids,
            ["tool.shared:mock-as"],
            "no Docker needed for a command"
        );
        assert!(findings[0].title.contains("deno"));
        assert_eq!(command_program("./bin/mock"), None);
        Ok(())
    }

    #[test]
    fn a_clean_project_is_ready_to_run() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = write(
            temp.path(),
            "session: demo\nrepos:\n  api:\n    services:\n      web: rails s\n",
        );
        std::fs::create_dir_all(temp.path().join("workspace--main/api"))
            .map_err(|error| error.to_string())?;
        let config = Config::load(&path).map_err(|error| error.message)?;
        let (has_tool, docker) = machine(true);
        let findings = diagnose(
            Some(&config),
            &path,
            temp.path(),
            &[],
            &Machine {
                has_tool: &has_tool,
                docker_running: &docker,
                compose_installed: &|| true,
            },
        );
        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect::<Vec<_>>(),
            ["ok"]
        );
        assert_eq!(report(&findings)["errors"], 0);
        Ok(())
    }

    #[test]
    fn gaps_are_reported_in_order_with_fixes() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = write(
            temp.path(),
            "session: demo\nshared_services:\n  postgres:\n    type: postgres\n  redis:\n    type: redis\nrepos:\n  api:\n    env:\n      KEY: \"{{secret.API_KEY}}\"\n      TOKEN: \"{{ secret.TOKEN }}\"\n      REDIS: \"{{shared.redis.url}}\"\n    services:\n      web: rails s\nwebhook:\n  port: 1\n",
        );
        let config = Config::load(&path).map_err(|error| error.message)?;
        let (has_tool, docker) = machine(false);
        let findings = diagnose(
            Some(&config),
            &path,
            temp.path(),
            &["TOKEN".to_string()],
            &Machine {
                has_tool: &has_tool,
                docker_running: &docker,
                compose_installed: &|| true,
            },
        );
        let ids: Vec<&str> = findings.iter().map(|finding| finding.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "tool.docker",
                "repo.missing:api",
                "config.removed",
                "secret.missing:API_KEY",
                "shared.unwired:postgres"
            ]
        );
        assert!(findings
            .iter()
            .all(|finding| finding.title.is_ascii() && finding.detail.is_ascii()));
        let prompt = fix_prompt(&findings);
        assert!(prompt.contains("- [warn] Secret not set: API_KEY"));
        assert_eq!(report(&findings)["warnings"], 2);
        Ok(())
    }

    #[test]
    fn no_config_says_so() {
        let (has_tool, docker) = machine(true);
        let findings = diagnose(
            None,
            Path::new("/nonexistent/pom.yml"),
            Path::new("/nonexistent"),
            &[],
            &Machine {
                has_tool: &has_tool,
                docker_running: &docker,
                compose_installed: &|| true,
            },
        );
        assert_eq!(findings[0].id, "config.load");
    }

    #[test]
    fn docker_without_compose_says_how_to_add_it() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = write(
            temp.path(),
            "session: demo\nshared_services:\n  postgres:\n    image: postgres:16\nrepos:\n  api:\n    env:\n      DATABASE_URL: \"{{shared.postgres.url}}\"\n    services:\n      web: rails s\n",
        );
        std::fs::create_dir_all(temp.path().join("workspace--main/api"))
            .map_err(|error| error.to_string())?;
        let config = Config::load(&path).map_err(|error| error.message)?;
        let (has_tool, docker) = machine(true);
        let findings = diagnose(
            Some(&config),
            &path,
            temp.path(),
            &[],
            &Machine {
                has_tool: &has_tool,
                docker_running: &docker,
                compose_installed: &|| false,
            },
        );
        let compose = findings
            .iter()
            .find(|finding| finding.id == "docker.compose")
            .ok_or("no compose finding")?;
        assert_eq!(compose.severity, Severity::Error);
        assert!(compose.fix.contains("Compose"), "{}", compose.fix);
        if !cfg!(target_os = "macos") {
            assert!(!compose.fix.contains("Docker Desktop"), "{}", compose.fix);
        }
        Ok(())
    }
}
