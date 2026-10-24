use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
            .filter(|session| !session.is_remote())
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
    let network = pom_env::dev_network();
    let ports = Ports::from_env();
    if pom_proxy::listening(ports.proxy) {
        return Err(format!(
            "port {} is already served (the app or another `pom proxy`)",
            ports.proxy
        ));
    }
    let holders = pom_ptyhost::SocketDir::from_env();
    let binary = std::env::current_exe().map_err(|error| error.to_string())?;
    let schedule = pom_workspace::Machine {
        state: state.clone(),
        holders: holders.clone(),
        binary: binary.clone(),
    };
    if let Err(error) =
        pom_workspace::run_in_background(schedule, |line| println!("workspaces: {line}"))
    {
        eprintln!("workspaces: idle sleep and max_running are off: {error}");
    }
    let machine = SystemMachine {
        state: state.clone(),
        holders: holders.clone(),
        waker: Some(pom_workspace::waker(
            state.clone(),
            holders.clone(),
            binary.clone(),
        )),
    };
    let push = pom_push::PushSync::start(pom_push::Machine {
        state: state.clone(),
        holders,
        binary,
    });
    let proxy = DevProxy::start_with(
        Box::new(machine),
        network.clone(),
        pom_proxy::Serve::default(),
        pom_proxy::Initial {
            projects: registered_routes(state),
            github: Some(push.hook()),
            token: pom_proxy::proxy_token(&network, state),
        },
    )
    .map_err(|error| format!("dev proxy: {error}"))?;
    if !proxy.proxy_running() {
        return Err(format!("could not listen on port {}", ports.proxy));
    }
    let (proxy_where, relay_host) = if network.public().is_local() && !network.exposed() {
        (
            format!("http://*.localhost:{}", ports.proxy),
            "127.0.0.1".to_string(),
        )
    } else {
        let listen = SocketAddr::new(network.bind, ports.proxy);
        (
            format!(
                "{listen} for {}",
                network.public().url(&format!("*.{}", network.domain))
            ),
            SocketAddr::new(network.bind, ports.webhook)
                .ip()
                .to_string(),
        )
    };
    writeln!(
        out,
        "dev proxy on {proxy_where}  webhook relay on http://{relay_host}:{}{}",
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
    let network = pom_env::dev_network();
    let token = pom_proxy::proxy_token(&network, &state);
    if let Some(reason) = pom_proxy::refusal(&network, token.as_ref()) {
        return writeln!(out, "dev proxy not started: {reason}").map_err(|error| error.to_string());
    }
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
    let public = pom_env::dev_network().public();
    public.url(&pom_env::service_host(
        service,
        alias,
        branch,
        &public.domain,
    ))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProxyConfigCommand {
    Show { json: bool },
    Set { key: String, value: String },
    Reset { key: String },
}

const PROXY_KEYS: [&str; 7] = [
    "bind",
    "domain",
    "public_scheme",
    "public_port",
    "trusted_proxies",
    "token_env",
    "allow_unauthenticated",
];

pub(crate) fn parse_config(rest: &[&str]) -> Result<ProxyConfigCommand, String> {
    let known = |key: &str| -> Result<String, String> {
        let key = key.strip_prefix("proxy.").unwrap_or(key);
        PROXY_KEYS
            .contains(&key)
            .then(|| key.to_string())
            .ok_or_else(|| format!("unknown proxy setting {key} ({})", PROXY_KEYS.join(", ")))
    };
    match rest {
        [] => Ok(ProxyConfigCommand::Show { json: false }),
        ["-o" | "--output", "json"] => Ok(ProxyConfigCommand::Show { json: true }),
        ["set", key, values @ ..] if !values.is_empty() || *key == "trusted_proxies" => {
            Ok(ProxyConfigCommand::Set {
                key: known(key)?,
                value: values.join(" "),
            })
        }
        ["set", ..] => Err("usage: pom proxy config set <key> <value>".into()),
        ["reset", key] => Ok(ProxyConfigCommand::Reset { key: known(key)? }),
        _ => Err("usage: pom proxy config [-o json] | set <key> <value> | reset <key>".into()),
    }
}

/// The proxy block with one key changed, refusing a value the proxy could not use.
fn apply_proxy_setting(
    mut proxy: settings::ProxySettings,
    key: &str,
    value: Option<&str>,
) -> Result<settings::ProxySettings, String> {
    let defaults = settings::ProxySettings::default();
    match (key, value) {
        ("bind", Some(value)) => proxy.bind = settings::parse_bind(value)?.to_string(),
        ("bind", None) => proxy.bind = defaults.bind,
        ("domain", Some(value)) => proxy.domain = settings::parse_domain(value)?,
        ("domain", None) => proxy.domain = defaults.domain,
        ("public_scheme", Some(value)) => {
            proxy.public_scheme = settings::parse_scheme(value)?.to_string()
        }
        ("public_scheme", None) => proxy.public_scheme = defaults.public_scheme,
        ("public_port", Some(value)) => {
            proxy.public_port = value
                .trim()
                .parse()
                .map_err(|_| format!("{value} is not a port (0 uses the proxy's own)"))?
        }
        ("public_port", None) => proxy.public_port = defaults.public_port,
        ("trusted_proxies", Some(value)) => {
            proxy.trusted_proxies = value
                .split([',', ' '])
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(|entry| settings::Cidr::parse(entry).map(|range| range.to_string()))
                .collect::<Result<_, _>>()?
        }
        ("trusted_proxies", None) => proxy.trusted_proxies = defaults.trusted_proxies,
        ("token_env", Some(value)) => proxy.token_env = settings::parse_env_name(value)?,
        ("token_env", None) => proxy.token_env = defaults.token_env,
        ("allow_unauthenticated", Some(value)) => {
            proxy.allow_unauthenticated = match value.trim() {
                "true" => true,
                "false" => false,
                _ => return Err(format!("{value}: allow_unauthenticated is true or false")),
            }
        }
        ("allow_unauthenticated", None) => {
            proxy.allow_unauthenticated = defaults.allow_unauthenticated
        }
        (other, _) => return Err(format!("unknown proxy setting {other}")),
    }
    Ok(proxy)
}

fn read_settings_file(path: &Path) -> Result<serde_json::Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(serde_json::Value::Object(serde_json::Map::new()))
        }
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn show_network(
    network: &pom_env::DevNetwork,
    json: bool,
    out: &mut dyn Write,
) -> Result<(), String> {
    let public = network.public();
    let example = public.url(&format!("<service>.<repo>.<workspace>.{}", public.domain));
    let trusted: Vec<String> = network
        .trusted_proxies
        .iter()
        .map(ToString::to_string)
        .collect();
    if json {
        let document = serde_json::json!({
            "schema": "pom.proxy/v1",
            "config": {
                "bind": network.bind.to_string(),
                "domain": network.domain,
                "public_scheme": if network.https { "https" } else { "http" },
                "public_port": network.public_port,
                "trusted_proxies": trusted,
                "token_env": network.token_env,
                "allow_unauthenticated": network.allow_unauthenticated,
            },
            "proxy_port": network.ports.proxy,
            "webhook_port": network.ports.webhook,
            "exposed": network.exposed(),
            "service_url": example,
        });
        return crate::say(out, &document.to_string());
    }
    let lines = [
        format!("bind             {}", network.bind),
        format!("domain           {}", network.domain),
        format!(
            "public_scheme    {}",
            if network.https { "https" } else { "http" }
        ),
        format!("public_port      {}", network.public_port),
        format!(
            "trusted_proxies  {}",
            if trusted.is_empty() {
                "-".to_string()
            } else {
                trusted.join(", ")
            }
        ),
        format!("token_env        {}", network.token_env),
        format!("allow_unauthenticated  {}", network.allow_unauthenticated),
        format!("service URLs     {example}"),
    ];
    for line in lines {
        crate::say(out, &line)?;
    }
    let token = pom_proxy::proxy_token(network, &StateDir::from_env());
    if let Some(reason) = pom_proxy::refusal(network, token.as_ref()) {
        crate::say(out, &format!("error: {reason}"))?;
    } else if network.reach().is_some() && token.is_none() {
        crate::say(
            out,
            &format!("warning: {}", pom_proxy::exposure_warning(network)),
        )?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TokenCommand {
    Show,
    New,
    Remove,
}

pub(crate) fn parse_token(rest: &[&str]) -> Result<TokenCommand, String> {
    match rest {
        [] => Ok(TokenCommand::Show),
        ["new"] => Ok(TokenCommand::New),
        ["rm"] => Ok(TokenCommand::Remove),
        _ => Err("usage: pom proxy token [new | rm]".into()),
    }
}

/// `pom proxy token`: the token other machines present to a non-loopback proxy.
pub(crate) fn token_command(
    command: &TokenCommand,
    state: &StateDir,
    out: &mut dyn Write,
) -> Result<(), String> {
    let network = pom_env::dev_network();
    let example = network.public().url(&format!(
        "<service>.<repo>.<workspace>.{}",
        network.public().domain
    ));
    let usage = |token: &str| {
        format!(
            "open a link once with ?{}={token} (for example {example}/?{}={token}); scripts send the {} header; webhook URLs from another machine add ?{}={token}",
            pom_proxy::TOKEN_PARAM,
            pom_proxy::TOKEN_PARAM,
            "X-Pom-Token",
            pom_proxy::TOKEN_PARAM,
        )
    };
    match command {
        TokenCommand::Show => match pom_proxy::proxy_token(&network, state) {
            Some(token) => {
                let from = match &token.source {
                    pom_proxy::TokenSource::Env(name) => format!("from ${name}"),
                    pom_proxy::TokenSource::Secret => "kept by pom (pom proxy token new)".into(),
                };
                crate::say(out, &format!("{}  {from}", token.value))?;
                crate::say(out, &usage(&token.value))
            }
            None => crate::say(
                out,
                &format!(
                    "no proxy token: `pom proxy token new` makes one, or set ${}",
                    network.token_env
                ),
            ),
        },
        TokenCommand::New => {
            let token = pom_proxy::new_proxy_token(state)?;
            crate::say(out, &token)?;
            if std::env::var_os(&network.token_env).is_some() {
                crate::say(
                    out,
                    &format!("${} is set and wins over this one", network.token_env),
                )?;
            }
            crate::say(out, &usage(&token))?;
            if pom_proxy::listening(network.ports.proxy) {
                crate::say(
                    out,
                    "restart the running proxy to use it (the app restarts its own; stop `pom proxy` and run it again)",
                )?;
            }
            Ok(())
        }
        TokenCommand::Remove => {
            pom_proxy::remove_proxy_token(state)?;
            crate::say(out, "removed the proxy token pom kept")
        }
    }
}

/// `pom proxy config`: shows or changes how the dev proxy is reached. A new public address rewrites every
/// workspace's env files, since service URLs in them change.
pub(crate) fn config_command(
    command: &ProxyConfigCommand,
    state: &StateDir,
    out: &mut dyn Write,
) -> Result<(), String> {
    let (key, value) = match command {
        ProxyConfigCommand::Show { json } => {
            return show_network(&pom_env::dev_network(), *json, out)
        }
        ProxyConfigCommand::Set { key, value } => (key.as_str(), Some(value.as_str())),
        ProxyConfigCommand::Reset { key } => (key.as_str(), None),
    };
    let path = settings::Settings::path().ok_or("no config directory (HOME is not set)")?;
    let mut document = read_settings_file(&path)?;
    let current = document
        .get(pom_env::PROXY_KEY)
        .cloned()
        .map(serde_json::from_value::<settings::ProxySettings>)
        .transpose()
        .map_err(|error| format!("{}: proxy: {error}", path.display()))?
        .unwrap_or_default();
    let before = pom_env::dev_network();
    let changed = apply_proxy_setting(current, key, value)?;
    let fields = document
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;
    fields.insert(
        pom_env::PROXY_KEY.to_string(),
        serde_json::to_value(&changed).map_err(|error| error.to_string())?,
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    std::fs::write(&path, format!("{text}\n"))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    pom_env::forget_dev_network();
    let after = pom_env::dev_network();
    show_network(&after, false, out)?;
    if after.public() != before.public() {
        refresh_env_files(state, out)?;
    }
    if (
        after.bind,
        &after.trusted_proxies,
        &after.domain,
        &after.token_env,
        after.allow_unauthenticated,
    ) != (
        before.bind,
        &before.trusted_proxies,
        &before.domain,
        &before.token_env,
        before.allow_unauthenticated,
    ) && pom_proxy::listening(after.ports.proxy)
    {
        crate::say(
            out,
            "restart the running proxy to apply this (the app restarts its own; stop `pom proxy` and run it again)",
        )?;
    }
    Ok(())
}

/// Rewrites every registered workspace's env files and lists the running services still on the old URLs.
fn refresh_env_files(state: &StateDir, out: &mut dyn Write) -> Result<(), String> {
    let binary = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut stale = Vec::new();
    for route in registered_routes(state) {
        let Some(config_path) = pom_core::config_in(&route.root) else {
            continue;
        };
        let project = pom_core::Project::open(&config_path, state);
        let Some(config) = project.config.as_ref().filter(|_| project.error.is_none()) else {
            continue;
        };
        let runner = pom_services::ServiceRunner::new(pom_services::RunnerOptions {
            project_root: project.root.clone(),
            session: config.session.clone(),
            state: state.clone(),
            holders: pom_ptyhost::SocketDir::from_env(),
            binary: binary.clone(),
            docker: "docker".into(),
        });
        for workspace in &project.workspaces {
            if let Err(error) = runner.refresh_workspace_env(config, &workspace.branch) {
                crate::say(out, &format!("env for {}: {error}", workspace.branch))?;
            }
            let targets = pom_services::ServiceRunner::service_targets(
                config,
                &workspace.branch,
                workspace.is_main,
            );
            stale.extend(
                runner
                    .stale_services(config, &targets)
                    .into_iter()
                    .map(|target| {
                        let name = if target.repo.is_empty() {
                            target.service
                        } else {
                            format!("{}/{}", target.repo, target.service)
                        };
                        format!("{name} ({}, {})", config.session, target.branch)
                    }),
            );
        }
    }
    crate::say(
        out,
        "rewrote the workspaces' env files with the new service URLs",
    )?;
    if !stale.is_empty() {
        crate::say(
            out,
            &format!(
                "restart these running services to use them: {}",
                stale.join(", ")
            ),
        )?;
    }
    Ok(())
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
    fn proxy_settings_are_validated_before_they_are_written() {
        let proxy = settings::ProxySettings::default();
        let set = |key: &str, value: &str| apply_proxy_setting(proxy.clone(), key, Some(value));
        assert_eq!(set("bind", "[::]").map(|p| p.bind), Ok("::".into()));
        assert!(set("bind", "my-vm").is_err());
        assert_eq!(
            set("domain", "Dev.Example.NET").map(|p| p.domain),
            Ok("dev.example.net".into())
        );
        assert!(set("public_scheme", "ftp").is_err());
        assert_eq!(set("public_port", "443").map(|p| p.public_port), Ok(443));
        assert_eq!(
            set("trusted_proxies", "127.0.0.1, 100.64.0.0/10").map(|p| p.trusted_proxies),
            Ok(vec!["127.0.0.1/32".into(), "100.64.0.0/10".into()])
        );
        assert!(set("trusted_proxies", "everyone").is_err());
        let changed = set("domain", "dev.example.net").unwrap();
        assert_eq!(
            apply_proxy_setting(changed, "domain", None),
            Ok(settings::ProxySettings::default())
        );
        assert_eq!(
            parse_config(&["set", "proxy.domain", "dev.example.net"]),
            Ok(ProxyConfigCommand::Set {
                key: "domain".into(),
                value: "dev.example.net".into()
            })
        );
        assert_eq!(
            parse_config(&["-o", "json"]),
            Ok(ProxyConfigCommand::Show { json: true })
        );
        assert!(parse_config(&["set", "port", "1"]).is_err());
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
    Remove {
        id: String,
        json: bool,
    },
    Clear {
        json: bool,
    },
}

const DEFAULT_FAULT_TTL: Duration = Duration::from_secs(600);
/// How long `proxy fault add|rm|clear` waits for the running proxy to confirm it re-read the rules.
const FAULT_APPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// A proxy too old to confirm re-reads its rules at most this long after they change.
const UNCONFIRMED_FAULT_RELOAD: Duration = Duration::from_millis(1100);

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
    let json = || -> Result<bool, String> { Ok(args.has("--json") || args.json()?) };
    match *verb {
        "add" => {
            let mut allowed = valued.to_vec();
            allowed.push("--json");
            args.allow(&allowed)?;
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
                json: json()?,
            })
        }
        "ls" => {
            args.allow(&["--all", "--json", "-o", "--output"])?;
            args.at_most(0, "proxy fault ls")?;
            Ok(FaultCommand::List {
                all: args.has("--all"),
                json: json()?,
            })
        }
        "rm" => {
            args.allow(&["--json", "-o", "--output"])?;
            let [id] = args.positional.as_slice() else {
                return Err("proxy fault rm needs a rule id (from proxy fault ls)".into());
            };
            Ok(FaultCommand::Remove {
                id: id.clone(),
                json: json()?,
            })
        }
        "clear" => {
            args.allow(&["--json", "-o", "--output"])?;
            args.at_most(0, "proxy fault clear")?;
            Ok(FaultCommand::Clear { json: json()? })
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
                let applied = applied_by_proxy(state, |ids| ids.contains(&rule.id));
                if *json {
                    let mut document =
                        serde_json::to_value(&rule).map_err(|error| error.to_string())?;
                    applied.annotate(&mut document);
                    return crate::say(out, &document.to_string());
                }
                crate::say(
                    out,
                    &format!(
                        "fault {} on {repo}/{name}{} in workspace {} for {}s{}",
                        rule.id,
                        if rule.path.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", rule.path)
                        },
                        self.branch,
                        ttl.as_secs(),
                        applied.suffix()
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
            FaultCommand::Remove { id, json } => {
                if !pom_proxy::remove_fault(state, id)? {
                    return Err(format!("no fault {id}"));
                }
                let applied = applied_by_proxy(state, |ids| !ids.contains(id));
                if *json {
                    let mut document =
                        serde_json::json!({ "schema": "pom.proxy/v1", "removed": id });
                    applied.annotate(&mut document);
                    return crate::say(out, &document.to_string());
                }
                crate::say(out, &format!("removed fault {id}{}", applied.suffix()))
            }
            FaultCommand::Clear { json } => {
                let workspace_ids: Vec<String> = pom_proxy::load_faults(state)
                    .into_iter()
                    .filter(|rule| {
                        rule.session == self.config.session && rule.branch == self.branch
                    })
                    .map(|rule| rule.id)
                    .collect();
                let cleared = pom_proxy::clear_faults(state, &self.config.session, &self.branch)?;
                let applied = applied_by_proxy(state, |ids| {
                    !ids.iter().any(|id| workspace_ids.contains(id))
                });
                if *json {
                    let mut document = serde_json::json!({
                        "schema": "pom.proxy/v1",
                        "workspace": self.branch,
                        "cleared": cleared,
                    });
                    applied.annotate(&mut document);
                    return crate::say(out, &document.to_string());
                }
                crate::say(
                    out,
                    &format!(
                        "cleared {cleared} fault rule(s) of workspace {}{}",
                        self.branch,
                        applied.suffix()
                    ),
                )
            }
        }
    }
}

/// Whether the running proxy obeys the rules as just written, and why not when it does not.
#[derive(Debug, PartialEq, Eq)]
struct Applied {
    applied: bool,
    reason: Option<String>,
}

impl Applied {
    fn annotate(&self, document: &mut serde_json::Value) {
        document["applied"] = serde_json::json!(self.applied);
        if let Some(reason) = &self.reason {
            document["reason"] = serde_json::json!(reason);
        }
    }

    fn suffix(&self) -> String {
        match (&self.reason, self.applied) {
            (None, true) => " (applied by the running proxy)".into(),
            (Some(reason), _) => format!(" ({reason})"),
            (None, false) => String::new(),
        }
    }
}

/// Has the proxy on this machine re-read the rules and waits until `obeys` holds for the ids it loaded, so a
/// test's next request already meets the change.
fn applied_by_proxy(state: &StateDir, obeys: impl Fn(&[String]) -> bool) -> Applied {
    let port = Ports::from_env().proxy;
    let deadline = std::time::Instant::now() + FAULT_APPLY_TIMEOUT;
    let not_applied = |reason: String| Applied {
        applied: false,
        reason: Some(reason),
    };
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match pom_proxy::reload_running_faults(port, remaining.max(Duration::from_millis(100))) {
            pom_proxy::ReloadReply::NotRunning => {
                return not_applied("no proxy running".into());
            }
            pom_proxy::ReloadReply::Unanswered(why) => {
                std::thread::sleep(UNCONFIRMED_FAULT_RELOAD);
                return Applied {
                    applied: true,
                    reason: Some(format!(
                        "the proxy on port {port} did not confirm ({why}); waited out its reload"
                    )),
                };
            }
            pom_proxy::ReloadReply::Loaded(loaded) => {
                let ours = pom_proxy::faults_path(state);
                if let Some(source) = loaded.source.filter(|source| *source != ours) {
                    return not_applied(format!(
                        "the proxy on port {port} reads {}, not {}",
                        source.display(),
                        ours.display()
                    ));
                }
                if obeys(&loaded.ids) {
                    return Applied {
                        applied: true,
                        reason: None,
                    };
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return not_applied(format!(
                "the proxy on port {port} did not pick up the change within {} s",
                FAULT_APPLY_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
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
            Ok(FaultCommand::Remove {
                id: "f1a2b3".into(),
                json: false
            })
        );
        assert_eq!(
            parse_fault(&["fault", "clear", "--json"]),
            Ok(FaultCommand::Clear { json: true })
        );
        assert!(matches!(
            parse_fault(&["fault", "add", "api/server", "--status", "503", "--json"]),
            Ok(FaultCommand::Add { json: true, .. })
        ));
        assert!(matches!(
            parse_fault(&["fault", "ls", "--all"]),
            Ok(FaultCommand::List { all: true, .. })
        ));
        assert!(parse_fault(&["restart"]).is_err());
    }
}
