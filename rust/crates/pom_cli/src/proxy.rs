use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use pom_config::Config;
use pom_paths::StateDir;
use pom_proxy::{DevProxy, Ports, ProjectRoute, SystemMachine};

const RELOAD_EVERY: Duration = Duration::from_secs(2);

/// Every project the app or the CLI registered, with its config as it is on disk now.
pub(crate) fn registered_routes(state: &StateDir) -> Vec<ProjectRoute> {
    let mut roots: Vec<PathBuf> = pom_sessions::Projects::load(state)
        .projects
        .into_values()
        .map(PathBuf::from)
        .collect();
    roots.extend(
        pom_sessions::Sessions::load(state)
            .sessions
            .into_iter()
            .map(|session| PathBuf::from(session.path)),
    );
    roots.sort();
    roots.dedup();
    roots
        .into_iter()
        .filter_map(|root| {
            let config = Config::load(&pom_core::config_in(&root)?).ok()?;
            Some(ProjectRoute {
                root,
                config: Arc::new(RwLock::new(Some(Arc::new(config)))),
            })
        })
        .collect()
}

/// `pom proxy`: the dev proxy and webhook relay in the foreground, re-reading the projects every few seconds.
pub(crate) fn serve(state: &StateDir, out: &mut dyn Write) -> Result<(), String> {
    let ports = Ports::from_env();
    if pom_proxy::listening(ports.proxy) {
        return Err(format!(
            "port {} is already served (the app or another `pom proxy`)",
            ports.proxy
        ));
    }
    let machine = SystemMachine {
        state: state.clone(),
        holders: pom_ptyhost::SocketDir::from_env(),
    };
    let proxy = DevProxy::start(Box::new(machine), ports, pom_proxy::Serve::default())
        .map_err(|error| error.to_string())?;
    if !proxy.proxy_running() {
        return Err(format!("could not listen on port {}", ports.proxy));
    }
    writeln!(
        out,
        "dev proxy on http://*.localhost:{}  webhook relay on http://127.0.0.1:{}{}",
        ports.proxy,
        ports.webhook,
        if proxy.webhook_running() {
            ""
        } else {
            " (port taken, relay off)"
        }
    )
    .map_err(|error| error.to_string())?;
    out.flush().map_err(|error| error.to_string())?;
    loop {
        proxy.set_projects(registered_routes(state));
        std::thread::sleep(RELOAD_EVERY);
    }
}

/// After `pom start`: services reach each other through the dev proxy, so start one in the background when
/// nothing serves its port. Only the `pom` binary does this, and `POM_NO_PROXY` turns it off (tests).
pub(crate) fn ensure_running(out: &mut dyn Write) -> Result<(), String> {
    let ports = Ports::from_env();
    if std::env::var_os("POM_NO_PROXY").is_some() || pom_proxy::listening(ports.proxy) {
        return Ok(());
    }
    let Ok(exe) = std::env::current_exe() else {
        return Ok(());
    };
    if exe.file_name().is_none_or(|name| name != "pom") {
        return Ok(());
    }
    let state = StateDir::from_env();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.path("proxy.log"))
        .map_err(|error| format!("proxy log: {error}"))?;
    let errors = log.try_clone().map_err(|error| error.to_string())?;
    use std::os::unix::process::CommandExt;
    std::process::Command::new(&exe)
        .arg("proxy")
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(errors)
        .process_group(0)
        .spawn()
        .map_err(|error| format!("start the dev proxy: {error}"))?;
    writeln!(
        out,
        "started the dev proxy on port {} (pom proxy, log in {})",
        ports.proxy,
        state.path("proxy.log").display()
    )
    .map_err(|error| error.to_string())
}

/// Where the dev proxy serves a repo service for a workspace.
pub(crate) fn service_url(config: &Config, branch: &str, repo: &str, service: &str) -> String {
    let alias = config
        .repos
        .get(repo)
        .map(|dir| dir.alias.as_str())
        .filter(|alias| !alias.is_empty())
        .unwrap_or(repo);
    format!(
        "http://{service}.{alias}.{}.localhost:{}",
        pom_env::workspace_label(branch),
        Ports::from_env().proxy
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_projects_and_sessions_become_routes() {
        let home = tempfile::tempdir().expect("temp dir");
        let state = StateDir::new(home.path().join("state"));
        let project = home.path().join("shop");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(project.join("pom.yml"), "session: shop\nrepos: {}\n").expect("pom.yml");
        pom_sessions::Projects::register(&state, "shop", &project).expect("register");
        pom_sessions::Projects::register(&state, "gone", &home.path().join("gone"))
            .expect("register");

        let routes = registered_routes(&state);
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].root,
            std::path::absolute(&project).expect("absolute")
        );
    }

    #[test]
    fn service_urls_use_the_alias_and_workspace_label() {
        let config: Config = serde_yaml_from(
            "session: shop\nrepos:\n  backend:\n    alias: api\n    services:\n      server:\n        cmd: run\n        port: true\n",
        );
        let url = service_url(&config, "feat/login", "backend", "server");
        assert!(url.starts_with("http://server.api."), "{url}");
        assert!(url.contains(".localhost:"), "{url}");
    }

    fn serde_yaml_from(text: &str) -> Config {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("pom.yml");
        std::fs::write(&path, text).expect("pom.yml");
        Config::load(&path).expect("config")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FaultCommand {
    Add {
        service: String,
        path: String,
        status: Option<u16>,
        delay: Duration,
        /// The share of requests it applies to, in millionths (the command stays comparable).
        rate_millionths: u32,
        ttl: Duration,
        json: bool,
    },
    List {
        all: bool,
        json: bool,
    },
    Remove(String),
    Clear,
}

const DEFAULT_FAULT_TTL: Duration = Duration::from_secs(600);

/// `500ms`, `2s`, `1m`.
fn delay(text: &str) -> Result<Duration, String> {
    let unreadable = || format!("--delay {text}: use a duration like 500ms or 2s");
    let (number, unit) = text
        .find(|c: char| !c.is_ascii_digit())
        .map_or((text, ""), |at| text.split_at(at));
    let value: u64 = number.parse().map_err(|_| unreadable())?;
    match unit {
        "ms" => Ok(Duration::from_millis(value)),
        "" | "s" => Ok(Duration::from_secs(value)),
        "m" => Ok(Duration::from_secs(value * 60)),
        _ => Err(unreadable()),
    }
}

pub(crate) fn parse_fault(rest: &[&str]) -> Result<FaultCommand, String> {
    let ["fault", verb, rest @ ..] = rest else {
        return Err("proxy takes `fault add|ls|rm|clear`, or nothing to serve".into());
    };
    let valued = [
        "--path", "--status", "--delay", "--rate", "--ttl", "-o", "--output",
    ];
    let args = crate::args::Args::parse(rest, &valued)?;
    match *verb {
        "add" => {
            args.allow(&valued)?;
            let [service] = args.positional.as_slice() else {
                return Err("proxy fault add needs one service".into());
            };
            let status = args
                .value(&["--status"])
                .map(|text| {
                    text.parse::<u16>()
                        .map_err(|_| format!("--status {text} is not a number"))
                })
                .transpose()?;
            let rate = args
                .value(&["--rate"])
                .map(|text| {
                    text.parse::<f64>()
                        .map_err(|_| format!("--rate {text} is not a number"))
                })
                .transpose()?
                .unwrap_or(1.0);
            if !(0.0..=1.0).contains(&rate) {
                return Err(format!("--rate {rate} must be between 0 and 1"));
            }
            Ok(FaultCommand::Add {
                service: service.clone(),
                path: args.value(&["--path"]).unwrap_or_default(),
                status,
                delay: args
                    .value(&["--delay"])
                    .map(|text| delay(&text))
                    .transpose()?
                    .unwrap_or_default(),
                rate_millionths: (rate * 1_000_000.0).round() as u32,
                ttl: args
                    .value(&["--ttl"])
                    .map(|text| crate::agent::parse_duration(&text))
                    .transpose()?
                    .unwrap_or(DEFAULT_FAULT_TTL),
                json: args.json()?,
            })
        }
        "ls" => {
            args.allow(&["--all", "-o", "--output"])?;
            args.at_most(0, "proxy fault ls")?;
            Ok(FaultCommand::List {
                all: args.has("--all"),
                json: args.json()?,
            })
        }
        "rm" => {
            args.allow(&[])?;
            let [id] = args.positional.as_slice() else {
                return Err("proxy fault rm needs a rule id (from proxy fault ls)".into());
            };
            Ok(FaultCommand::Remove(id.clone()))
        }
        "clear" => {
            args.allow(&[])?;
            args.at_most(0, "proxy fault clear")?;
            Ok(FaultCommand::Clear)
        }
        other => Err(format!(
            "unknown proxy fault subcommand {other} (add, ls, rm, clear)"
        )),
    }
}

impl crate::Session {
    /// Fault rules for this workspace's requests through the dev proxy.
    pub(crate) fn fault_command(
        &self,
        command: &FaultCommand,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let state = &self.state;
        match command {
            FaultCommand::Add {
                service,
                path,
                status,
                delay,
                rate_millionths,
                ttl,
                json,
            } => {
                let (repo, name) = self.config.find_service_entry(service)?;
                let alias = self
                    .config
                    .repos
                    .get(&repo)
                    .map(|dir| dir.alias.clone())
                    .unwrap_or_default();
                let path = if path.is_empty() || path.starts_with('/') {
                    path.clone()
                } else {
                    format!("/{path}")
                };
                let rule = pom_proxy::add_fault(
                    state,
                    pom_proxy::FaultRule {
                        session: self.config.session.clone(),
                        branch: self.branch.clone(),
                        repo: repo.clone(),
                        alias,
                        service: name.clone(),
                        path,
                        status: *status,
                        delay_ms: delay.as_millis() as u64,
                        rate: f64::from(*rate_millionths) / 1_000_000.0,
                        expires_ms: pom_proxy::faults_now() + ttl.as_millis() as u64,
                        ..pom_proxy::FaultRule::default()
                    },
                )?;
                if *json {
                    let text = serde_json::to_string(&rule).map_err(|error| error.to_string())?;
                    return crate::say(out, &text);
                }
                crate::say(
                    out,
                    &format!(
                        "fault {} on {repo}/{name}{} in workspace {} for {}s",
                        rule.id,
                        if rule.path.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", rule.path)
                        },
                        self.branch,
                        ttl.as_secs()
                    ),
                )
            }
            FaultCommand::List { all, json } => {
                let rules: Vec<pom_proxy::FaultRule> = pom_proxy::load_faults(state)
                    .into_iter()
                    .filter(|rule| {
                        *all || (rule.session == self.config.session && rule.branch == self.branch)
                    })
                    .collect();
                if *json {
                    let document = serde_json::json!({ "schema": "pom.proxy/v1", "faults": rules });
                    return crate::say(out, &document.to_string());
                }
                if rules.is_empty() {
                    return crate::say(out, "no fault rules");
                }
                let now = pom_proxy::faults_now();
                for rule in &rules {
                    let effect = match (rule.status, rule.delay_ms) {
                        (Some(status), 0) => format!("status {status}"),
                        (Some(status), delay) => format!("status {status} after {delay} ms"),
                        (None, delay) => format!("delay {delay} ms"),
                    };
                    crate::say(
                        out,
                        &format!(
                            "{}  {}/{}{}  {}/{}  {effect}  rate {}  {}s left",
                            rule.id,
                            rule.repo,
                            rule.service,
                            if rule.path.is_empty() {
                                String::new()
                            } else {
                                format!(" {}", rule.path)
                            },
                            rule.session,
                            rule.branch,
                            rule.rate,
                            rule.expires_ms.saturating_sub(now) / 1000
                        ),
                    )?;
                }
                Ok(())
            }
            FaultCommand::Remove(id) => {
                if pom_proxy::remove_fault(state, id)? {
                    crate::say(out, &format!("removed fault {id}"))
                } else {
                    Err(format!("no fault {id}"))
                }
            }
            FaultCommand::Clear => {
                let cleared = pom_proxy::clear_faults(state, &self.config.session, &self.branch)?;
                crate::say(
                    out,
                    &format!(
                        "cleared {cleared} fault rule(s) of workspace {}",
                        self.branch
                    ),
                )
            }
        }
    }
}

#[cfg(test)]
mod fault_tests {
    use super::*;

    #[test]
    fn fault_commands_parse() {
        let words = [
            "fault",
            "add",
            "api/server",
            "--path",
            "/v1/orders",
            "--status",
            "503",
            "--delay",
            "500ms",
            "--rate",
            "0.3",
            "--ttl",
            "2m",
            "-o",
            "json",
        ];
        assert_eq!(
            parse_fault(&words),
            Ok(FaultCommand::Add {
                service: "api/server".into(),
                path: "/v1/orders".into(),
                status: Some(503),
                delay: Duration::from_millis(500),
                rate_millionths: 300_000,
                ttl: Duration::from_secs(120),
                json: true,
            })
        );
        assert!(parse_fault(&["fault", "add", "api/server", "--delay", "soon"]).is_err());
        assert!(parse_fault(&[
            "fault",
            "add",
            "api/server",
            "--status",
            "503",
            "--rate",
            "2"
        ])
        .is_err());
        assert_eq!(
            parse_fault(&["fault", "rm", "f1a2b3"]),
            Ok(FaultCommand::Remove("f1a2b3".into()))
        );
        assert_eq!(parse_fault(&["fault", "clear"]), Ok(FaultCommand::Clear));
        assert!(matches!(
            parse_fault(&["fault", "ls", "--all"]),
            Ok(FaultCommand::List { all: true, .. })
        ));
        assert!(parse_fault(&["restart"]).is_err());
    }
}
