use std::collections::HashMap;

use indexmap::IndexMap;
use pom_config::{Config, Dir, Service, SharedServiceDef};
use pom_env::{EnvSources, ResolveContext, SlotAllocation};

#[derive(Default)]
struct FakeState {
    shared_ports: HashMap<String, u16>,
    service_ports: HashMap<(String, String), u16>,
    slots: HashMap<(String, String), SlotAllocation>,
    secrets: HashMap<(String, String), String>,
}

impl EnvSources for FakeState {
    fn shared_port(&self, name: &str) -> Option<u16> {
        self.shared_ports.get(name).copied()
    }
    fn service_port(&self, ws_key: &str, service_key: &str) -> Option<u16> {
        self.service_ports
            .get(&(ws_key.to_string(), service_key.to_string()))
            .copied()
    }
    fn slot(&self, shared_name: &str, ws_key: &str) -> Option<SlotAllocation> {
        self.slots
            .get(&(shared_name.to_string(), ws_key.to_string()))
            .copied()
    }
    fn secret(&self, session: &str, name: &str) -> Option<String> {
        self.secrets
            .get(&(session.to_string(), name.to_string()))
            .cloned()
    }
}

const WS: &str = "ws:feat";

fn config() -> Config {
    let mut config = Config {
        session: "acme".into(),
        ..Config::default()
    };
    let mut api = Dir {
        alias: "api".into(),
        proxy_port: Some(4000),
        ..Dir::default()
    };
    api.services.insert("server".into(), Service::default());
    api.services.insert("worker".into(), Service::default());
    config.repos.insert("acme-api".into(), api);
    config.repos.insert(
        "static".into(),
        Dir {
            proxy_port: Some(4100),
            ..Dir::default()
        },
    );
    config.shared_services.insert(
        "pg".into(),
        SharedServiceDef {
            kind: "postgres".into(),
            ..SharedServiceDef::default()
        },
    );
    config.shared_services.insert(
        "redis".into(),
        SharedServiceDef {
            capacity: Some(2),
            ..SharedServiceDef::default()
        },
    );
    config
}

fn state() -> FakeState {
    let mut state = FakeState::default();
    state.shared_ports.insert("pg".into(), 21000);
    state.shared_ports.insert("redis".into(), 22000);
    state
        .service_ports
        .insert((WS.into(), "api~server".into()), 3101);
    state.slots.insert(
        ("redis".into(), WS.into()),
        SlotAllocation {
            instance: 1,
            slot: 3,
        },
    );
    state
        .secrets
        .insert(("acme".into(), "API_KEY".into()), "s3cret".into());
    state
}

fn resolve_all(inputs: &[&str]) -> Vec<String> {
    let config = config();
    let state = state();
    let db_names: IndexMap<String, String> = [("main".to_string(), "acme_feat".to_string())]
        .into_iter()
        .collect();
    let context = ResolveContext {
        config: &config,
        branch: "feat",
        ws_key: WS,
        env_name: "",
        db_names: &db_names,
        sources: &state,
    };
    inputs.iter().map(|input| context.resolve(input)).collect()
}

#[test]
fn leased_ports_slots_instances_and_secrets() {
    let resolved = resolve_all(&[
        "{{api.server.port}}",
        "{{api.worker.port}}",
        "{{shared.redis.port}}",
        "{{shared.redis.slot}}",
        "{{slot.redis}}",
        "{{shared.redis.url}}",
        "{{shared.pg.user}}",
        "{{db.main.url}}",
        "{{secret.API_KEY}}",
        "{{secret.MISSING}}",
    ]);
    assert_eq!(
        resolved,
        [
            "3101",
            "0",
            "22001",
            "3",
            "3",
            "postgres:postgres@127.0.0.1:22001",
            "postgres",
            "postgres://postgres:postgres@127.0.0.1:21000/acme_feat",
            "s3cret",
            "{{secret.MISSING}}",
        ]
    );
}

#[test]
fn unleased_shared_url_falls_back_to_default_postgres_port() {
    let config = config();
    let empty = FakeState::default();
    let db_names = IndexMap::new();
    let context = ResolveContext {
        config: &config,
        branch: "feat",
        ws_key: WS,
        env_name: "",
        db_names: &db_names,
        sources: &empty,
    };
    assert_eq!(
        context.resolve("{{shared.pg.url}} {{shared.pg.port}}"),
        "postgres:postgres@127.0.0.1:5432 0"
    );
}

#[test]
fn resolve_env_sorts_by_key() {
    let config = config();
    let state = state();
    let db_names = IndexMap::new();
    let context = ResolveContext {
        config: &config,
        branch: "feat/x",
        ws_key: WS,
        env_name: "",
        db_names: &db_names,
        sources: &state,
    };
    let env: IndexMap<String, String> = [
        ("B".to_string(), "{{branch.safe}}".to_string()),
        ("A".to_string(), "plain".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        context.resolve_env(&env),
        [
            ("A".to_string(), "plain".to_string()),
            ("B".to_string(), "feat_x".to_string())
        ]
    );
}

#[test]
fn override_host_and_port_come_from_the_url() {
    let mut config = config();
    config.environments.insert(
        "remote".into(),
        [
            (
                "api.server".to_string(),
                "https://api.example.com/v1".to_string(),
            ),
            ("api.worker".to_string(), "http://[::1]:7000".to_string()),
            ("static.site".to_string(), "tcp-thing".to_string()),
        ]
        .into_iter()
        .collect(),
    );
    let mut repo = Dir::default();
    repo.services.insert("site".into(), Service::default());
    config.repos.insert("static".into(), repo);
    let state = state();
    let db_names = IndexMap::new();
    let context = ResolveContext {
        config: &config,
        branch: "feat",
        ws_key: WS,
        env_name: "remote",
        db_names: &db_names,
        sources: &state,
    };
    let resolved: Vec<String> = [
        "{{api.server.host}}",
        "{{api.server.port}}",
        "{{api.server.ws}}",
        "{{api.worker.host}}",
        "{{api.worker.port}}",
        "{{static.site.port}}",
    ]
    .iter()
    .map(|input| context.resolve(input))
    .collect();
    assert_eq!(
        resolved,
        [
            "api.example.com",
            "443",
            "wss://api.example.com/v1",
            "::1",
            "7000",
            "{{static.site.port}}",
        ]
    );
}

#[test]
fn http_to_ws_only_swaps_http_schemes() {
    assert_eq!(pom_env::http_to_ws("http://a:1/x"), "ws://a:1/x");
    assert_eq!(pom_env::http_to_ws("https://a"), "wss://a");
    assert_eq!(pom_env::http_to_ws("tcp://a"), "tcp://a");
}
