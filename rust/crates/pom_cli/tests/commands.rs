//! The config, env, run, workspace-status and completion commands through the real `pom` binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const POM: &str = env!("CARGO_BIN_EXE_pom");

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomcmd")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        for repo in [
            "workspace--main/api",
            "workspace--main/web",
            "workspace--feat/api",
        ] {
            std::fs::create_dir_all(root.join(repo).join(".git")).expect("worktree");
        }
        std::fs::create_dir_all(temp.path().join("zdot")).expect("zdot");
        std::fs::write(
            root.join("pom.yml"),
            r#"session: demo
default_branch: main
repos:
  api:
    alias: be
    databases:
      main: "{{branch.safe}}"
    env:
      GREETING: hello
      TOKEN: "{{secret.TOKEN}}"
    lifecycle:
      commands:
        hello: "printf ran-$GREETING > ran.txt"
    services:
      web:
        cmd: "exec nc -lk 127.0.0.1 $PORT"
        type: backend
  web:
    env:
      MODE: dev
    services:
      app:
        cmd: "sleep 100"
"#,
        )
        .expect("pom.yml");
        Fixture { temp }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn pom(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(cwd)
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", "1")
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .env("ZDOTDIR", self.temp.path().join("zdot"))
            // `config edit` must never open a real editor window from a test.
            .env("VISUAL", "true")
            .output()
            .expect("pom")
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.pom(cwd, args);
        assert!(output.status.success(), "{args:?}: {}", text(&output));
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let dir = pom_ptyhost::SocketDir::new(self.temp.path().join("s"));
        let names: Vec<String> = dir.holders().into_iter().map(|(name, _)| name).collect();
        dir.kill_holders_now(&names);
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn config_path_and_explain() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let path = PathBuf::from(fixture.ok(&root, &["config", "path"]).trim());
    assert_eq!(
        std::fs::canonicalize(path).expect("path"),
        std::fs::canonicalize(root.join("pom.yml")).expect("pom.yml")
    );
    let explained = fixture.ok(&root, &["config", "explain", "--branch", "feat/x"]);
    assert!(explained.contains("Branch  feat/x"), "{explained}");
    assert!(
        explained.contains("{{db.main}}  demo_feat_x"),
        "{explained}"
    );

    let service = fixture.ok(&root, &["config", "explain", "be/web"]);
    assert!(service.contains("api/web  (alias be)"), "{service}");
    assert!(service.contains("GREETING"), "{service}");
    assert!(!service.contains("{{secret"), "{service}");

    let json = fixture.ok(&root, &["config", "explain", "-o", "json"]);
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(value["branch"], "main");

    let edited = fixture.ok(&root, &["config", "edit"]);
    assert!(edited.contains("is valid"), "{edited}");
}

#[test]
fn env_set_get_list_and_unset() {
    let fixture = Fixture::new();
    let root = fixture.root();
    fixture.ok(&root, &["env", "set", "be", "COLOR=blue", "GREETING=hi"]);
    assert_eq!(
        fixture.ok(&root, &["env", "get", "api", "COLOR"]).trim(),
        "blue"
    );
    assert_eq!(
        fixture
            .ok(&root, &["env", "get", "api/web", "GREETING"])
            .trim(),
        "hi"
    );
    let listed = fixture.ok(&root, &["env", "ls", "api"]);
    assert!(
        listed.contains("COLOR     blue") || listed.contains("COLOR  blue"),
        "{listed}"
    );
    fixture.ok(&root, &["env", "unset", "api", "COLOR"]);
    let missing = fixture.pom(&root, &["env", "get", "api", "COLOR"]);
    assert!(!missing.status.success());
    assert!(text(&missing).contains("no env var COLOR"));
    let bad = fixture.pom(&root, &["env", "set", "api", "NOEQUALS"]);
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn run_uses_named_commands_and_the_workspace_env() {
    let fixture = Fixture::new();
    let feat = fixture.root().join("workspace--feat/api");
    fixture.ok(&feat, &["run", "hello"]);
    assert_eq!(
        std::fs::read_to_string(feat.join("ran.txt")).expect("ran.txt"),
        "ran-hello"
    );
    fixture.ok(
        &fixture.root(),
        &["run", "printf direct > direct.txt", "be"],
    );
    assert!(fixture
        .root()
        .join("workspace--main/api/direct.txt")
        .is_file());
    let failed = fixture.pom(&feat, &["run", "exit 3"]);
    assert!(text(&failed).contains("status 3"), "{}", text(&failed));
    let listed = fixture.ok(&feat, &["commands"]);
    assert!(
        listed.contains("api (be)") && listed.contains("hello"),
        "{listed}"
    );
}

#[test]
fn workspace_status_drift_and_refresh() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let listed = fixture.ok(&root, &["get", "workspaces"]);
    assert!(
        listed.contains("feat") && listed.contains("main (main)"),
        "{listed}"
    );
    assert!(listed.contains("0/1") && listed.contains("0/2"), "{listed}");

    let described = fixture.ok(&root, &["describe", "workspace", "feat"]);
    assert!(
        described.contains("not checked out here: web"),
        "{described}"
    );
    let plan = fixture.ok(&root, &["apply"]);
    assert!(
        plan.contains("feat: + web") && plan.contains("dry run"),
        "{plan}"
    );

    let json = fixture.ok(&root, &["get", "ws", "-o", "json"]);
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(value.as_array().map(Vec::len), Some(2));

    fixture.ok(&root.join("workspace--feat/api"), &["start", "api/web"]);
    let refreshed = fixture.ok(&root, &["refresh"]);
    assert!(refreshed.contains("stopped 1 service"), "{refreshed}");
    assert!(fixture
        .ok(&root, &["refresh"])
        .contains("no running services"));
}

#[test]
fn completion_and_argument_errors() {
    let fixture = Fixture::new();
    let root = fixture.root();
    assert!(fixture
        .ok(&root, &["completion", "zsh"])
        .starts_with("#compdef pom"));
    for bad in [
        vec!["db", "nuke"],
        vec!["get", "pods"],
        vec!["onboard", "--repo", "x"],
        vec!["ps", "--forever"],
    ] {
        assert_eq!(fixture.pom(&root, &bad).status.code(), Some(2), "{bad:?}");
    }
}

#[test]
fn init_turns_the_repo_you_are_in_into_a_project() {
    let fixture = Fixture::new();
    let repo = fixture.temp.path().join("shop");
    std::fs::create_dir_all(&repo).expect("repo");
    std::fs::write(
        repo.join("package.json"),
        r#"{"name":"shop","scripts":{"dev":"vite"}}"#,
    )
    .expect("package.json");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"]);

    let sessions = fixture.temp.path().join("sessions");
    let output = Command::new(POM)
        .args(["init", "shopdemo"])
        .current_dir(&repo)
        .env("XDG_STATE_HOME", fixture.temp.path().join("state"))
        .env("POM_SESSIONS_ROOT", &sessions)
        .output()
        .expect("pom");
    assert!(output.status.success(), "{}", text(&output));
    let config = sessions.join("shopdemo/pom.yml");
    assert!(config.is_file(), "{}", text(&output));
    assert!(sessions
        .join("shopdemo/workspace--main/shop/package.json")
        .is_file());
    let loaded = std::fs::read_to_string(&config).expect("pom.yml");
    assert!(loaded.contains("session: shopdemo"), "{loaded}");
}
