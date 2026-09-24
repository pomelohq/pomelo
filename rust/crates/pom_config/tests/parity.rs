//! Each fixture's loaded config is projected to JSON and compared with a golden file captured
//! from the previous implementation. `POM_GO_ORACLE=1` re-runs that implementation live and
//! compares against it instead; add `POM_ORACLE_BLESS=1` to rewrite the goldens from it.

use std::path::{Path, PathBuf};
use std::process::Command;

use indexmap::IndexMap;
use pom_config::{Config, Shortcut};
use serde_json::{json, Map, Value};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(fixtures_root())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.join("pom.yml").is_file())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

fn string_map(map: &IndexMap<String, String>) -> Value {
    Value::Object(
        map.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
}

fn shortcuts(list: &[Shortcut]) -> Value {
    list.iter()
        .map(|s| json!({"cmd": s.cmd, "desc": s.desc, "key": s.key}))
        .collect()
}

fn project(config: &Config) -> Value {
    let repos: Vec<Value> = config
        .repos
        .iter()
        .map(|(name, dir)| {
            let services: Vec<Value> = dir
                .services
                .iter()
                .map(|(service_name, s)| {
                    json!({
                        "name": service_name, "type": s.kind, "cmd": s.cmd, "dir": s.dir,
                        "shell_env": s.shell_env, "env": string_map(&s.env),
                        "pre_start": s.pre_start, "proxy_port": s.proxy_port,
                        "shortcuts": shortcuts(&s.shortcuts), "depends_on": s.depends_on,
                        "has_port": s.has_port(), "modes": string_map(&s.modes), "mode": s.mode,
                        "profiles": s.profiles,
                    })
                })
                .collect();
            let env_files: Vec<Value> = dir
                .env_file_entries()
                .iter()
                .map(|e| json!({"file": e.file, "env": string_map(&e.env)}))
                .collect();
            let refs: Vec<Value> = dir
                .shared_refs
                .iter()
                .map(|r| json!({"name": r.name, "db_name": r.db_name}))
                .collect();
            json!({
                "name": name, "alias": dir.alias, "pre_start": dir.pre_start,
                "shell_env": dir.shell_env, "default_branch": dir.default_branch,
                "services": services, "proxy_port": dir.proxy_port, "profiles": dir.profiles,
                "copy": dir.copy, "env": string_map(&dir.env), "own_env": string_map(&dir.own_env),
                "env_files": env_files, "shared_refs": refs,
                "databases": string_map(&dir.databases), "presets": dir.presets,
                "setup": dir.setup, "migrate": dir.migrate, "seed": dir.seed,
                "seed_from_main": dir.seed_from_main, "commands": string_map(&dir.commands),
                "pre_delete": dir.pre_delete, "effective_setup": dir.effective_setup(),
                "effective_migrate": dir.effective_migrate(),
                "effective_shortcuts": shortcuts(&dir.effective_shortcuts()),
                "has_worktree_config": dir.has_worktree_config(),
            })
        })
        .collect();
    let shared: Map<String, Value> = config
        .shared_services
        .iter()
        .map(|(name, s)| {
            let mut entry = json!({
                "type": s.kind, "image": s.image, "host": s.host, "ports": s.ports,
                "environment": string_map(&s.environment), "volumes": s.volumes,
                "command": s.command, "db_user": s.db_user, "db_password": s.db_password,
                "capacity": s.capacity,
            });
            if let (Some(health), Value::Object(fields)) = (&s.healthcheck, &mut entry) {
                fields.insert(
                    "healthcheck".into(),
                    json!({"interval": health.interval, "timeout": health.timeout, "retries": health.retries}),
                );
            }
            (name.clone(), entry)
        })
        .collect();
    let workspaces: Map<String, Value> = config
        .all_workspaces()
        .into_iter()
        .map(|(name, entries)| (name, json!(entries)))
        .collect();
    let environments: Map<String, Value> = config
        .environments
        .iter()
        .map(|(name, values)| (name.clone(), string_map(values)))
        .collect();
    let workspace_services: Vec<Value> = config
        .workspace_services
        .iter()
        .map(|(name, s)| json!({"name": name, "cmd": s.cmd}))
        .collect();
    json!({
        "session": config.session, "default_branch": config.global_default_branch(),
        "repos": repos, "shared_services": shared, "workspaces": workspaces,
        "environments": environments, "seed": config.seed,
        "prepare_main": config.prepare_main_phases(), "workspace_services": workspace_services,
        "valid": config.validate().is_ok(),
    })
}

fn rust_projection(fixture: &Path) -> Value {
    match Config::load(&fixture.join("pom.yml")) {
        Ok(config) => normalize(project(&config)),
        Err(_) => json!({"load_error": true}),
    }
}

/// Absent, null, empty and zero values are one state on the old side (nil vs empty slices,
/// omitted pointers), so they are dropped before comparing.
fn normalize(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k, normalize(v)))
                .filter(|(_, v)| !is_empty(v))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        other => other,
    }
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(flag) => !flag,
        Value::Number(number) => number.as_f64() == Some(0.0),
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(fields) => fields.is_empty(),
    }
}

fn oracle_projection(fixture: &Path) -> Value {
    let go_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let output = std::env::temp_dir().join(format!(
        "pom-config-oracle-{}-{}.json",
        std::process::id(),
        fixture
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    let status = Command::new("go")
        .args([
            "test",
            "./internal/config",
            "-run",
            "^TestOracleDump$",
            "-count=1",
        ])
        .current_dir(&go_root)
        .env("POM_ORACLE_CONFIG", fixture.join("pom.yml"))
        .env("POM_ORACLE_OUT", &output)
        .status()
        .expect("run go test");
    assert!(
        status.success(),
        "oracle dump failed for {}",
        fixture.display()
    );
    let text = std::fs::read_to_string(&output).expect("read oracle output");
    if let Err(error) = std::fs::remove_file(&output) {
        eprintln!("leaving {}: {error}", output.display());
    }
    normalize(serde_json::from_str(&text).expect("oracle json"))
}

#[test]
fn fixtures_match_previous_implementation() {
    let fixtures = fixture_dirs();
    assert!(
        !fixtures.is_empty(),
        "no fixtures under {}",
        fixtures_root().display()
    );
    let live = std::env::var_os("POM_GO_ORACLE").is_some();
    let bless = std::env::var_os("POM_ORACLE_BLESS").is_some();
    let mut failures = Vec::new();
    for fixture in fixtures {
        let actual = rust_projection(&fixture);
        let golden_path = fixture.join("expected.json");
        let expected = if live {
            let oracle = oracle_projection(&fixture);
            if bless {
                let pretty = serde_json::to_string_pretty(&oracle).expect("serialize golden");
                std::fs::write(&golden_path, pretty + "\n").expect("write golden");
            }
            oracle
        } else {
            let text = std::fs::read_to_string(&golden_path).unwrap_or_else(|error| {
                panic!(
                    "missing golden {} ({error}); run with POM_GO_ORACLE=1 POM_ORACLE_BLESS=1",
                    golden_path.display()
                )
            });
            serde_json::from_str(&text).expect("golden json")
        };
        if actual != expected {
            failures.push(format!(
                "{}\n  expected: {}\n  actual:   {}",
                fixture.display(),
                expected,
                actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "parity mismatches:\n{}",
        failures.join("\n")
    );
}
