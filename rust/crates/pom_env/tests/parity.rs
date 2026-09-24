//! Resolver and branch-helper outputs compared with a golden captured from the previous
//! implementation. `POM_GO_ORACLE=1` runs it live; add `POM_ORACLE_BLESS=1` to rewrite the golden.

use std::path::{Path, PathBuf};
use std::process::Command;

use indexmap::IndexMap;
use pom_config::Config;
use pom_env::{EnvSources, ResolveContext, SlotAllocation};
use serde_json::{json, Value};

/// Empty machine state: nothing leased, so shared services sit on their stable hash port.
struct EmptyState {
    session: String,
}

impl EnvSources for EmptyState {
    fn shared_port(&self, name: &str) -> Option<u16> {
        Some(pom_env::stable_shared_port(&self.session, name))
    }
    fn service_port(&self, _ws_key: &str, _service_key: &str) -> Option<u16> {
        None
    }
    fn slot(&self, _shared_name: &str, _ws_key: &str) -> Option<SlotAllocation> {
        None
    }
    fn secret(&self, _session: &str, _name: &str) -> Option<String> {
        None
    }
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resolve")
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn rust_output(cases: &Value) -> Value {
    let config =
        Config::load(&fixture().join(text(&cases["config"]))).expect("load fixture config");
    let state = EmptyState {
        session: config.session.clone(),
    };
    let empty = Vec::new();
    let resolved: Vec<Value> = cases["contexts"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|context| {
            let db_names: IndexMap<String, String> = context["db_names"]
                .as_object()
                .map(|names| {
                    names
                        .iter()
                        .map(|(k, v)| (k.clone(), text(v).to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let resolver = ResolveContext {
                config: &config,
                branch: text(&context["branch"]),
                ws_key: text(&context["ws_key"]),
                env_name: text(&context["env_name"]),
                db_names: &db_names,
                sources: &state,
            };
            context["inputs"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .map(|input| json!(resolver.resolve(text(input))))
                .collect()
        })
        .collect();
    let branches: Vec<Value> = cases["branches"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|branch| {
            let branch = text(branch);
            json!({
                "safe": pom_env::branch_safe(branch), "hash": pom_env::branch_hash(branch),
                "host": pom_env::branch_host(branch), "label": pom_env::workspace_label(branch),
                "port_ws_key": pom_env::port_ws_key(branch),
            })
        })
        .collect();
    let ports: Vec<Value> = cases["shared_ports"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|pair| json!(pom_env::stable_shared_port(text(&pair[0]), text(&pair[1]))))
        .collect();
    json!({"resolved": resolved, "branches": branches, "stable_ports": ports})
}

fn oracle_output(cases: &Value) -> Value {
    let go_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let scratch = std::env::temp_dir().join(format!("pom-env-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let mut go_cases = cases.clone();
    go_cases["config"] = json!(fixture().join(text(&cases["config"])));
    let cases_path = scratch.join("cases.json");
    let output_path = scratch.join("out.json");
    std::fs::write(&cases_path, go_cases.to_string()).expect("write cases");
    let status = Command::new("go")
        .args([
            "test",
            "./internal/services",
            "-run",
            "^TestOracleResolve$",
            "-count=1",
        ])
        .current_dir(&go_root)
        .env("POM_ORACLE_CASES", &cases_path)
        .env("POM_ORACLE_OUT", &output_path)
        .status()
        .expect("run go test");
    assert!(status.success(), "oracle resolve failed");
    let output = std::fs::read_to_string(&output_path).expect("read oracle output");
    if let Err(error) = std::fs::remove_dir_all(&scratch) {
        eprintln!("leaving {}: {error}", scratch.display());
    }
    serde_json::from_str(&output).expect("oracle json")
}

/// Deliberate behavior fixes are listed in the cases file so the golden stays the untouched
/// output of the previous implementation.
fn apply_intentional_divergences(cases: &Value, mut expected: Value) -> Value {
    let empty = Vec::new();
    for divergence in cases["diverges"].as_array().unwrap_or(&empty) {
        let index = divergence["context"].as_u64().unwrap_or_default() as usize;
        let inputs = cases["contexts"][index]["inputs"]
            .as_array()
            .unwrap_or(&empty);
        for (position, input) in inputs.iter().enumerate() {
            if input == &divergence["input"] {
                expected["resolved"][index][position] = divergence["value"].clone();
            }
        }
    }
    expected
}

#[test]
fn resolver_matches_previous_implementation() {
    let cases: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture().join("cases.json")).expect("read cases"),
    )
    .expect("cases json");
    let golden_path = fixture().join("expected.json");
    let expected = if std::env::var_os("POM_GO_ORACLE").is_some() {
        let oracle = oracle_output(&cases);
        if std::env::var_os("POM_ORACLE_BLESS").is_some() {
            let pretty = serde_json::to_string_pretty(&oracle).expect("serialize golden");
            std::fs::write(&golden_path, pretty + "\n").expect("write golden");
        }
        oracle
    } else {
        serde_json::from_str(
            &std::fs::read_to_string(&golden_path).unwrap_or_else(|error| {
                panic!(
                    "missing golden {} ({error}); run with POM_GO_ORACLE=1 POM_ORACLE_BLESS=1",
                    golden_path.display()
                )
            }),
        )
        .expect("golden json")
    };
    let expected = apply_intentional_divergences(&cases, expected);
    let actual = rust_output(&cases);
    for key in ["resolved", "branches", "stable_ports"] {
        assert_eq!(actual[key], expected[key], "{key} differs");
    }
}
