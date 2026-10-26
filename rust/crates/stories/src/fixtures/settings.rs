//! What the Settings window shows of a project and of the host a remote window is on.

/// The project page's two repos; `remote` puts the project on the host `dev`.
pub fn project_page(remote: bool) -> settings_ui::ProjectPage {
    settings_ui::ProjectPage {
        session: "myproject".into(),
        repos: vec![
            settings_ui::RepoSummary {
                name: "api".into(),
                alias: "be".into(),
                services: 2,
                cloned: true,
                present: 3,
                workspaces: 4,
            },
            settings_ui::RepoSummary {
                name: "web".into(),
                alias: "web".into(),
                services: 1,
                cloned: false,
                present: 0,
                workspaces: 4,
            },
        ],
        host: remote.then(|| "dev".to_string()),
    }
}

/// The host tab of a window on `dev`, shown or behind This Mac; `state` is reading, unreachable, outdated or
/// refused, and anything else reads its settings.
pub fn host_page(shown: bool, state: &str) -> settings_ui::HostPage {
    let mut host = settings_ui::HostPage::new("dev", env!("CARGO_PKG_VERSION"));
    host.shown = shown;
    let mut values = settings::Settings::default();
    values.workspaces.idle_sleep_minutes = 60;
    values.workspaces.max_running = 3;
    values.agent_command = "claude --model opus".into();
    values.terminal_shell = "/usr/bin/zsh".into();
    values.modules_size_limit_gb = 40;
    let answer = settings_ui::HostAnswer {
        values: serde_json::to_value(values).unwrap_or_default(),
        keys: settings::HOST_KEYS
            .iter()
            .map(|key| key.to_string())
            .collect(),
        path: "/home/dev/.config/pomelo/settings.json".into(),
    };
    match state {
        "reading" => {}
        "unreachable" => {
            host.load = settings_ui::HostLoad::Unreachable(
                "ssh: connect to host dev.example.net port 22: Operation timed out".into(),
            );
            host.link = settings_ui::HostLink::Disconnected;
        }
        "outdated" => {
            host.version = "0.9.7".into();
            host.load = settings_ui::HostLoad::Outdated(
                "Changing the settings of dev needs a newer pom on dev; update it there".into(),
            );
        }
        "refused" => {
            host.take_answer(answer);
            host.errors.push((
                "proxy.bind".into(),
                "10.0.0.300 is not an IP address (like 127.0.0.1 or 100.64.0.10)".into(),
            ));
        }
        _ => {
            host.take_answer(answer);
        }
    }
    host
}
