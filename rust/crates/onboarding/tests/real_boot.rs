//! Boots real services through PTY holders hosted by the app binary. Opt-in: run with
//! `POM_APP_BINARY=target/debug/pomelo cargo test -p onboarding --test real_boot -- --ignored`.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use onboarding::{CheckStatus, RunnerServices, VerifyInput};

#[test]
#[ignore]
fn a_real_server_and_worker_boot_and_a_crashing_one_is_caught() {
    let Some(binary) = std::env::var_os("POM_APP_BINARY") else {
        return;
    };
    let binary = std::fs::canonicalize(binary).expect("binary");
    let temp = tempfile::Builder::new()
        .prefix("pob")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let root = temp.path().join("p");
    std::fs::create_dir_all(root.join("workspace--main/web")).expect("checkout");
    std::fs::write(
        root.join("pom.yml"),
        "session: obtest\nrepos:\n  web:\n    setup: [echo installed]\n    services:\n      server:\n        type: backend\n        cmd: python3 -m http.server $PORT --bind 127.0.0.1\n      worker:\n        cmd: sleep 30\n        port: false\n      broken:\n        cmd: \"echo 'Error: cannot find module express'; exit 3\"\n        port: false\n",
    )
    .expect("config");
    let config = pom_config::Config::load(&root.join("pom.yml")).expect("load");
    let runner = pom_services::ServiceRunner::new(pom_services::RunnerOptions {
        project_root: root.clone(),
        session: "obtest".into(),
        state: pom_paths::StateDir::new(temp.path().join("state")),
        holders: pom_ptyhost::SocketDir::new(temp.path().join("s")),
        binary,
        docker: "docker".into(),
    });
    let has_tool = |_: &str| true;
    let docker = || true;
    let machine = pom_doctor::Machine {
        has_tool: &has_tool,
        docker_running: &docker,
    };
    let installed = HashMap::new();
    let input = VerifyInput {
        config: &config,
        config_path: &root.join("pom.yml"),
        root: &root,
        secret_names: &[],
        machine: &machine,
        installed: &installed,
        skipped: &[],
        boot_timeout: Duration::from_secs(30),
        worker_grace: Duration::from_secs(2),
    };
    let services = RunnerServices {
        runner: &runner,
        config: &config,
    };
    let checks = onboarding::verify(&input, &services, &mut |_| {}, &AtomicBool::new(false));
    let summary: Vec<(String, CheckStatus, String)> = checks
        .iter()
        .map(|check| (check.kind.title(), check.status, check.detail.clone()))
        .collect();
    eprintln!("{summary:#?}");
    assert_eq!(summary[1].1, CheckStatus::Passed, "install");
    assert_eq!(summary[2].1, CheckStatus::Passed, "server");
    assert!(summary[2].2.starts_with("listening on :"));
    assert_eq!(summary[3].1, CheckStatus::Passed, "worker");
    assert_eq!(summary[4].1, CheckStatus::Failed, "broken");
    assert!(
        summary[4].2.contains("cannot find module"),
        "{}",
        summary[4].2
    );
    assert!(
        runner.running_holders().is_empty(),
        "every boot was stopped"
    );
}
