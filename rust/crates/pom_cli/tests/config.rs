//! `pom config export|import` through the real binary.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const POM: &str = env!("CARGO_BIN_EXE_pom");

fn pom(home: &Path, cwd: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(POM)
        .args(args)
        .current_dir(cwd)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("POM_NO_PROXY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("pom");
    if let Some(mut input) = child.stdin.take() {
        input.write_all(stdin.as_bytes()).expect("stdin");
    }
    child.wait_with_output().expect("pom")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_sealed_export_imports_into_another_project_with_its_secrets() {
    let home = tempfile::tempdir().expect("temp");
    let source = home.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(
        source.join("pom.yml"),
        "session: shop\nrepos:\n  api:\n    services:\n      web:\n        cmd: run\n",
    )
    .expect("pom.yml");
    let state = pom_paths::StateDir::new(home.path().join("state").join("pom"));
    pom_secrets::SecretStore::new(state.clone(), "shop")
        .set("TOKEN", "t-1")
        .expect("secret");

    let plain = pom(home.path(), &source, &["config", "export"], "");
    assert!(plain.status.success(), "{}", text(&plain.stderr));
    assert!(text(&plain.stdout).contains("session: shop"));

    let binary = pom(
        home.path(),
        &source,
        &["config", "export", "--secrets"],
        "pw\n",
    );
    assert!(!binary.status.success());
    assert!(text(&binary.stderr).contains("-o <file>"));

    let bundle = home.path().join("shop.pombundle");
    let sealed = pom(
        home.path(),
        &source,
        &[
            "config",
            "export",
            "--secrets",
            "-o",
            bundle.to_str().expect("path"),
        ],
        "pw\n",
    );
    assert!(sealed.status.success(), "{}", text(&sealed.stderr));
    assert!(pom_bundle::is_sealed(
        &std::fs::read(&bundle).expect("bundle")
    ));

    let target = home.path().join("target");
    std::fs::create_dir_all(&target).expect("target");
    std::fs::write(target.join("pom.yml"), "session: fresh\nrepos: {}\n").expect("pom.yml");
    let wrong = pom(
        home.path(),
        &target,
        &["config", "import", bundle.to_str().expect("path")],
        "nope\n",
    );
    assert!(text(&wrong.stderr).contains("wrong password"));

    let imported = pom(
        home.path(),
        &target,
        &[
            "config",
            "import",
            bundle.to_str().expect("path"),
            "--secrets-only",
        ],
        "pw\n",
    );
    assert!(imported.status.success(), "{}", text(&imported.stderr));
    assert_eq!(text(&imported.stdout).trim(), "stored 1 secret(s)");
    assert_eq!(
        std::fs::read_to_string(target.join("pom.yml")).expect("pom.yml"),
        "session: fresh\nrepos: {}\n"
    );
    assert_eq!(
        pom_secrets::SecretStore::new(state, "fresh")
            .get("TOKEN")
            .expect("get"),
        Some("t-1".to_string())
    );

    let replaced = pom(
        home.path(),
        &target,
        &[
            "config",
            "import",
            bundle.to_str().expect("path"),
            "--config-only",
        ],
        "pw\n",
    );
    assert!(replaced.status.success(), "{}", text(&replaced.stderr));
    assert!(target.join("pom.yml.bak").is_file());
    let loaded = pom_config::Config::load(&target.join("pom.yml")).expect("load");
    assert_eq!(loaded.session, "shop");
}
