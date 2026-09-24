use pom_config::{Config, Dir, Service};

fn service(cmd: &str) -> Service {
    Service {
        cmd: cmd.to_string(),
        ..Service::default()
    }
}

fn repo(alias: &str, services: &[&str]) -> Dir {
    Dir {
        alias: alias.to_string(),
        services: services
            .iter()
            .map(|name| (name.to_string(), service(name)))
            .collect(),
        ..Dir::default()
    }
}

fn two_repo_config() -> Config {
    let mut config = Config {
        session: "myapp".into(),
        ..Config::default()
    };
    config
        .repos
        .insert("myapp-api".into(), repo("api", &["server"]));
    config
        .repos
        .insert("myapp-client".into(), repo("client", &["dev"]));
    config
}

fn found(dir: &str, service: &str) -> Result<(String, String), String> {
    Ok((dir.to_string(), service.to_string()))
}

#[test]
fn default_branches() {
    let mut config = Config::default();
    assert_eq!(config.global_default_branch(), "main");
    config.default_branch = "develop".into();
    config.repos.insert(
        "api".into(),
        Dir {
            default_branch: "staging".into(),
            ..Dir::default()
        },
    );
    config.repos.insert("client".into(), Dir::default());
    assert_eq!(config.default_branch_for("api"), "staging");
    assert_eq!(config.default_branch_for("client"), "develop");
    assert_eq!(config.default_branch_for("unknown"), "develop");
}

#[test]
fn find_service_entry_by_alias_name_or_bare() {
    let config = two_repo_config();
    assert_eq!(
        config.find_service_entry("api/server"),
        found("myapp-api", "server")
    );
    assert_eq!(
        config.find_service_entry("myapp-api/server"),
        found("myapp-api", "server")
    );
    assert_eq!(
        config.find_service_entry("server"),
        found("myapp-api", "server")
    );
    assert_eq!(
        config.find_service_entry("dev"),
        found("myapp-client", "dev")
    );
    assert!(config.find_service_entry("api/unknown").is_err());
    assert!(config.find_service_entry("nonexistent").is_err());
}

#[test]
fn bare_service_in_two_repos_is_ambiguous() {
    let mut config = Config::default();
    config.repos.insert("repo-a".into(), repo("", &["server"]));
    config.repos.insert("repo-b".into(), repo("", &["server"]));
    let error = config
        .find_service_entry("server")
        .err()
        .unwrap_or_default();
    assert!(
        error.contains("ambiguous") && error.contains("repo-a, repo-b"),
        "{error}"
    );
}

#[test]
fn all_workspaces_merges_combinations_and_auto_generates() {
    let mut config = Config::default();
    config.workspaces.insert("dev".into(), vec!["a".into()]);
    config
        .combinations
        .insert("full".into(), vec!["a".into(), "b".into()]);
    config
        .combinations
        .insert("dev".into(), vec!["ignored".into()]);
    let all = config.all_workspaces();
    assert_eq!(all.len(), 2);
    assert_eq!(all["dev"], ["a"]);

    let config = two_repo_config();
    assert_eq!(
        config.all_workspaces()["myapp"],
        ["api/server", "client/dev"]
    );
}

#[test]
fn resolve_services_targets() {
    let mut config = two_repo_config();
    config
        .workspaces
        .insert("both".into(), vec!["api/server".into(), "dev".into()]);
    assert_eq!(
        config.resolve_services("both"),
        Ok(vec![
            ("myapp-api".into(), "server".into()),
            ("myapp-client".into(), "dev".into())
        ])
    );
    assert_eq!(
        config.resolve_services("client"),
        Ok(vec![("myapp-client".into(), "dev".into())])
    );
    assert_eq!(
        config.resolve_services("myapp-api"),
        Ok(vec![("myapp-api".into(), "server".into())])
    );
    assert!(config.resolve_services("nope").is_err());
}

#[test]
fn resolve_service_prefers_main_worktree_and_inherits_repo_fields() -> std::io::Result<()> {
    let temp = tempfile::tempdir()?;
    let mut config = Config::default();
    let mut api = repo("", &[]);
    api.shell_env = ".env".into();
    api.pre_start = "repo start".into();
    api.services.insert(
        "server".into(),
        Service {
            cmd: "go run .".into(),
            dir: "cmd".into(),
            modes: [("debug".to_string(), "dlv debug".to_string())]
                .into_iter()
                .collect(),
            mode: "debug".into(),
            ..Service::default()
        },
    );
    config.repos.insert("api".into(), api);

    let resolved = config
        .resolve_service(temp.path(), "api", "server")
        .map_err(std::io::Error::other)?;
    assert_eq!(resolved.cmd, "dlv debug");
    assert_eq!(resolved.work_dir, temp.path().join("api/cmd"));
    assert_eq!(resolved.shell_env, ".env");
    assert_eq!(resolved.pre_start, "repo start");

    std::fs::create_dir_all(temp.path().join("workspace--main/api"))?;
    let resolved = config
        .resolve_service(temp.path(), "api", "server")
        .map_err(std::io::Error::other)?;
    assert_eq!(
        resolved.work_dir,
        temp.path().join("workspace--main/api/cmd")
    );

    assert!(config
        .resolve_service(temp.path(), "api", "missing")
        .is_err());
    assert!(config
        .resolve_service(temp.path(), "ghost", "server")
        .is_err());
    Ok(())
}

#[test]
fn service_mode_and_port_rules() {
    let mut service = service("run");
    service.modes.insert("b".into(), "run b".into());
    service.modes.insert("a".into(), "run a".into());
    assert_eq!(service.active_cmd(""), "run");
    assert_eq!(service.active_cmd("a"), "run a");
    assert_eq!(service.active_cmd("missing"), "run");
    assert_eq!(service.mode_names(), ["a", "b"]);
    assert!(!service.has_port());
    service.kind = "backend".into();
    assert!(service.has_port());
    service.port = Some(false);
    assert!(!service.has_port());
}

#[test]
fn prepare_main_phases_filter_unknown() {
    let mut config = Config::default();
    assert_eq!(config.prepare_main_phases(), ["reset", "migrate", "seed"]);
    config.prepare_main = vec!["seed".into(), "bogus".into()];
    assert_eq!(config.prepare_main_phases(), ["seed"]);
    config.prepare_main = vec!["bogus".into()];
    assert_eq!(config.prepare_main_phases(), ["reset"]);
}

#[test]
fn validate_environment_lists_available() {
    let mut config = Config::default();
    assert!(config.validate_environment("").is_ok());
    assert!(config
        .validate_environment("staging")
        .is_err_and(|e| e.contains("no environments defined")));
    config
        .environments
        .insert("prod".into(), Default::default());
    assert!(config
        .validate_environment("staging")
        .is_err_and(|e| e.contains("available: prod")));
    assert!(config.validate_environment("prod").is_ok());
}
