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
    let proxy = DevProxy::start(Box::new(machine), ports).map_err(|error| error.to_string())?;
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
