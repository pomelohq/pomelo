//! Opt-in (`POM_GO_ORACLE=1`): the previous core must read a store written here, and this crate
//! must read what it writes back, sharing one machine key.

use std::path::Path;
use std::process::Command;

use pom_paths::StateDir;
use pom_secrets::SecretStore;

#[test]
fn store_is_readable_across_implementations() {
    if std::env::var_os("POM_GO_ORACLE").is_none() {
        return;
    }
    let temp = tempfile::tempdir().expect("temp dir");
    let session = "interop demo";
    let store = SecretStore::new(StateDir::new(temp.path().join("pom")), session);
    store
        .set("FROM_RUST", "rust value with \"quotes\"")
        .expect("rust set");

    let output = temp.path().join("go-read.json");
    let go_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let status = Command::new("go")
        .args([
            "test",
            "./internal/secrets",
            "-run",
            "^TestOracleInterop$",
            "-count=1",
        ])
        .current_dir(go_root)
        .env("POM_ORACLE_STATE_HOME", temp.path())
        .env("POM_ORACLE_SESSION", session)
        .env("POM_ORACLE_OUT", &output)
        .status()
        .expect("run go test");
    assert!(status.success());

    let seen = std::fs::read_to_string(&output).expect("read go output");
    assert_eq!(seen, r#"{"FROM_RUST":"rust value with \"quotes\""}"#);
    assert_eq!(
        store.get("FROM_GO").expect("rust get").as_deref(),
        Some("go value")
    );
    assert_eq!(store.names().expect("names"), ["FROM_GO", "FROM_RUST"]);
}
