use std::collections::BTreeMap;

use pom_paths::StateDir;
use pom_secrets::SecretStore;

#[test]
fn bundles_sealed_by_the_previous_core_open() {
    let data = include_bytes!("fixtures/sealed-by-previous-core.pombundle");
    let contents = pom_bundle::open(data, "hunter2").expect("open");
    assert_eq!(contents.config, "session: demo\nrepos: {}\n");
    assert_eq!(
        contents.secrets,
        BTreeMap::from([
            ("TOK".to_string(), "abc".to_string()),
            ("URL".to_string(), "postgres://u:p@h/db".to_string()),
        ])
    );
}

#[test]
fn export_then_apply_carries_the_merged_config_and_secrets_to_another_project() {
    let home = tempfile::tempdir().expect("temp dir");
    let state = StateDir::new(home.path().join("state"));
    let source = home.path().join("source");
    std::fs::create_dir_all(source.join("pom.d")).expect("pom.d");
    std::fs::write(source.join("pom.yml"), "session: shop\n").expect("pom.yml");
    std::fs::write(
        source.join("pom.d/10-repos.yml"),
        "repos:\n  api:\n    services:\n      server:\n        cmd: run\n",
    )
    .expect("fragment");
    SecretStore::new(state.clone(), "shop")
        .set("API_KEY", "k-1")
        .expect("secret");

    let plain = pom_bundle::export(&source.join("pom.yml"), &state, "shop", None).expect("plain");
    assert_eq!(plain.file_name, "pom-config.yml");
    assert!(!pom_bundle::is_sealed(&plain.data));

    let sealed =
        pom_bundle::export(&source.join("pom.yml"), &state, "shop", Some("pw")).expect("sealed");
    assert_eq!(sealed.file_name, "pom-config.pombundle");
    let contents = pom_bundle::open(&sealed.data, "pw").expect("open");
    assert!(contents.config.contains("server:"), "{}", contents.config);
    assert_eq!(
        contents.secrets.get("API_KEY").map(String::as_str),
        Some("k-1")
    );

    let target = home.path().join("target");
    std::fs::create_dir_all(&target).expect("target");
    std::fs::write(target.join("pom.yml"), "session: old\n").expect("pom.yml");
    let applied = pom_bundle::apply(
        &target.join("pom.yml"),
        &state,
        "shop2",
        Some(&contents.config),
        &contents.secrets,
    )
    .expect("apply");
    assert_eq!(applied.secrets_created, 1);
    assert!(applied.split);
    let loaded = pom_config::Config::load(&target.join("pom.yml")).expect("load");
    assert!(loaded.repos["api"].services.contains_key("server"));
    assert_eq!(
        SecretStore::new(state, "shop2")
            .get("API_KEY")
            .expect("get"),
        Some("k-1".to_string())
    );
}
