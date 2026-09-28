use std::path::Path;
use std::process::Command;

use pom_core::{scaffold_session, RepoSpec, ScaffoldRequest};
use pom_paths::StateDir;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .map(|status| status.success());
    assert_eq!(status.ok(), Some(true), "git {args:?}");
}

fn source_repo(root: &Path) -> std::path::PathBuf {
    let repo = root.join("web");
    std::fs::create_dir_all(&repo).ok();
    git(&repo, &["init", "--quiet", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".env.local\n.env.example.local\n").ok();
    std::fs::write(
        repo.join("package.json"),
        r#"{"name":"web","scripts":{"dev":"next dev"},"dependencies":{"next":"14"}}"#,
    )
    .ok();
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "init",
        ],
    );
    std::fs::write(
        repo.join(".env.local"),
        "export API_KEY=\"abc\"\n# note\nDEBUG=1\n",
    )
    .ok();
    std::fs::write(repo.join(".env.example.local"), "IGNORED=1\n").ok();
    std::fs::write(repo.join("notes.txt"), "draft\n").ok();
    repo
}

#[test]
fn scaffold_clones_imports_env_and_registers() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = StateDir::new(temp.path().join("state"));
    let source = source_repo(temp.path());
    let request = ScaffoldRequest {
        name: "demo".into(),
        root: temp.path().join("sessions").to_string_lossy().into_owned(),
        default_branch: "develop".into(),
        repos: vec![RepoSpec {
            path: source.to_string_lossy().into_owned(),
            alias: "fe".into(),
        }],
        skip_secrets: false,
    };
    let session_dir = scaffold_session(&request, &state).expect("scaffold");
    let clone = session_dir.join("workspace--develop/web");
    assert!(clone.join("package.json").exists());
    assert_eq!(
        std::fs::read_to_string(clone.join("notes.txt"))
            .ok()
            .as_deref(),
        Some("draft\n")
    );
    assert!(!clone.join(".env.local").exists());

    let secrets = pom_secrets::SecretStore::new(state.clone(), "demo");
    assert_eq!(
        secrets.get("API_KEY").ok().flatten().as_deref(),
        Some("abc")
    );
    assert_eq!(secrets.get("DEBUG").ok().flatten().as_deref(), Some("1"));
    assert_eq!(secrets.get("IGNORED").ok().flatten(), None);

    let config = std::fs::read_to_string(session_dir.join("pom.yml")).unwrap_or_default();
    assert!(config.contains("default_branch: develop"), "{config}");
    assert!(config.contains("web"), "{config}");
    let sessions = pom_sessions::Sessions::load(&state);
    assert_eq!(
        sessions.get("demo").map(|session| session.path.clone()),
        Some(session_dir.to_string_lossy().into_owned())
    );

    assert!(scaffold_session(&request, &state).is_err());
}

#[test]
fn failed_scaffold_leaves_nothing_behind() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = StateDir::new(temp.path().join("state"));
    let plain = temp.path().join("plain");
    std::fs::create_dir_all(&plain).ok();
    let request = ScaffoldRequest {
        name: "broken".into(),
        root: temp.path().join("sessions").to_string_lossy().into_owned(),
        default_branch: String::new(),
        repos: vec![RepoSpec {
            path: plain.to_string_lossy().into_owned(),
            alias: String::new(),
        }],
        skip_secrets: false,
    };
    assert!(scaffold_session(&request, &state)
        .unwrap_err()
        .contains("not a git repo"));
    assert!(!temp.path().join("sessions/broken").exists());
    for name in ["", "a/b", ".."] {
        let request = ScaffoldRequest {
            name: name.into(),
            ..request.clone()
        };
        assert!(scaffold_session(&request, &state).is_err());
    }
}

#[test]
fn scaffold_reports_each_repo_and_what_the_scan_found() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = StateDir::new(temp.path().join("state"));
    let source = source_repo(temp.path());
    let preview = pom_core::preview_repo(&source).expect("a git repo");
    assert_eq!(preview.name, "web");
    assert_eq!(preview.stack, vec!["Node".to_string()]);
    assert_eq!(preview.env_files, 1);
    assert!(pom_core::preview_repo(temp.path()).is_none());

    let request = ScaffoldRequest {
        name: "demo".into(),
        root: temp.path().join("sessions").to_string_lossy().into_owned(),
        default_branch: "main".into(),
        repos: vec![RepoSpec {
            path: source.to_string_lossy().into_owned(),
            alias: String::new(),
        }],
        skip_secrets: true,
    };
    let mut events = Vec::new();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    pom_core::scaffold_session_with(&request, &state, &mut |event| events.push(event), &cancel)
        .expect("scaffold");
    assert_eq!(
        events[..2],
        [
            pom_core::ScaffoldEvent::Cloning {
                repo: 0,
                percent: 0
            },
            pom_core::ScaffoldEvent::Cloned {
                repo: 0,
                linked: true
            },
        ]
    );
    match events.last() {
        Some(pom_core::ScaffoldEvent::Scanned(scans)) => {
            assert_eq!(scans[0].stack, vec!["Node".to_string()]);
            assert_eq!(scans[0].env_files, 0, "secrets were skipped");
        }
        other => panic!("no scan: {other:?}"),
    }
    let store = pom_secrets::SecretStore::new(state.clone(), "demo");
    assert!(store.names().unwrap_or_default().is_empty());

    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let request = ScaffoldRequest {
        name: "stopped".into(),
        ..request
    };
    assert_eq!(
        pom_core::scaffold_session_with(&request, &state, &mut |_| {}, &cancel),
        Err(pom_core::CANCELLED.to_string())
    );
    assert!(!temp.path().join("sessions/stopped").exists());
}
