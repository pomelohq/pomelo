//! `pom ws export` on one "machine" and `pom ws import` on another, through the real binary. Each machine is its
//! own state dir (so its own secret key and port leases) and project folder; both clone the same bare origin.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use pom_paths::StateDir;
use pom_secrets::SecretStore;
use serde_json::Value;

const POM: &str = env!("CARGO_BIN_EXE_pom");
const PASSPHRASE: &str = "correct horse battery";

const CONFIG: &str = r#"session: demo
default_branch: main
repos:
  api:
    env:
      API_KEY: "{{secret.API_KEY}}"
      WEB_PORT: "{{api.web.port}}"
    services:
      web:
        cmd: "sleep 1000"
        port: true
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", text(output)))
}

struct Machine {
    home: PathBuf,
}

impl Machine {
    /// A project folder with `api` cloned from `origin` on main.
    fn new(home: PathBuf, origin: &Path) -> Machine {
        let root = home.join("project");
        std::fs::create_dir_all(root.join("workspace--main")).expect("project");
        std::fs::create_dir_all(home.join("zdot")).expect("zdot");
        git(
            &root.join("workspace--main"),
            &["clone", "-q", &origin.to_string_lossy(), "api"],
        );
        std::fs::write(root.join("pom.yml"), CONFIG).expect("pom.yml");
        Machine { home }
    }

    fn root(&self) -> PathBuf {
        self.home.join("project")
    }

    fn state(&self) -> StateDir {
        StateDir::new(self.home.join("state/pom"))
    }

    fn worktree(&self, branch: &str) -> PathBuf {
        self.root().join(format!("workspace--{branch}/api"))
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(POM);
        command
            .args(args)
            .current_dir(self.root())
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("POMELO_CONFIG_DIR", self.home.join("config"))
            .env("POM_PTY_SOCK_DIR", self.home.join("s"))
            .env("ZDOTDIR", self.home.join("zdot"))
            .env("POM_NO_PROXY", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env_remove(pom_passphrase_env())
            .stdin(Stdio::null());
        // The compose plugin lives under the real home's docker config.
        if let Some(home) = std::env::var_os("HOME") {
            let docker = PathBuf::from(home).join(".docker");
            if docker.is_dir() && std::env::var_os("DOCKER_CONFIG").is_none() {
                command.env("DOCKER_CONFIG", docker);
            }
        }
        command
    }

    fn pom(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("pom")
    }

    fn pom_with_passphrase(&self, args: &[&str]) -> Output {
        self.command(args)
            .env(pom_passphrase_env(), PASSPHRASE)
            .output()
            .expect("pom")
    }

    fn env_value(&self, branch: &str, key: &str) -> String {
        let path = self.worktree(branch).join(".env.local");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            let listing: Vec<_> = std::fs::read_dir(self.worktree(branch))
                .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
                .unwrap_or_default();
            panic!("{}: {error} {listing:?}", path.display())
        });
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .unwrap_or_default()
            .trim_matches('"')
            .to_string()
    }
}

fn pom_passphrase_env() -> &'static str {
    "POM_BUNDLE_PASSPHRASE"
}

/// A bare origin with one commit on main that ignores `*.log`.
fn origin(temp: &Path) -> PathBuf {
    let seed = temp.join("seed");
    std::fs::create_dir_all(&seed).expect("seed");
    git(&seed, &["init", "-q", "-b", "main"]);
    std::fs::write(seed.join("README"), "hello\n").expect("readme");
    std::fs::write(seed.join(".gitignore"), "*.log\n.env.local\n").expect("ignore");
    std::fs::write(seed.join(".env"), "MODE=dev\n").expect("env");
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "init"]);
    let origin = temp.join("origin/api.git");
    git(
        temp,
        &[
            "clone",
            "-q",
            "--bare",
            &seed.to_string_lossy(),
            &origin.to_string_lossy(),
        ],
    );
    origin
}

#[test]
fn a_half_done_workspace_opens_on_another_machine_as_it_was() {
    let temp = tempfile::Builder::new()
        .prefix("pomx")
        .tempdir_in("/tmp")
        .expect("temp");
    let origin = origin(temp.path());
    let mac = Machine::new(temp.path().join("mac"), &origin);
    let vm = Machine::new(temp.path().join("vm"), &origin);
    SecretStore::new(mac.state(), "demo")
        .set("API_KEY", "k-123")
        .expect("secret");
    SecretStore::new(mac.state(), "demo")
        .set("UNUSED", "never-travels")
        .expect("secret");

    let created = mac.pom(&["ws", "create", "feat-x"]);
    assert!(created.status.success(), "{}", text(&created));
    let source = mac.worktree("feat-x");
    for (file, message) in [("one.txt", "first"), ("two.txt", "second")] {
        std::fs::write(source.join(file), format!("{message}\n")).expect("file");
        git(&source, &["add", file]);
        git(&source, &["commit", "-q", "-m", message]);
    }
    std::fs::write(source.join("README"), "hello\nchanged\n").expect("modify");
    std::fs::write(source.join("staged.txt"), "staged\n").expect("staged");
    git(&source, &["add", "staged.txt"]);
    std::fs::write(source.join("notes.md"), "draft\n").expect("untracked");
    std::fs::write(source.join("debug.log"), "noise\n").expect("ignored");

    let archive = temp.path().join("feat-x.pomws");
    let exported = mac.pom_with_passphrase(&[
        "ws",
        "export",
        "feat-x",
        "-o",
        &archive.to_string_lossy(),
        "--no-db",
        "--json",
    ]);
    assert!(exported.status.success(), "{}", text(&exported));
    let report = json(&exported);
    assert_eq!(report["schema"], "pom.ws-transfer/v1");
    assert_eq!(report["repos"][0]["unpushed_commits"], 2);
    assert_eq!(report["repos"][0]["uncommitted_changes"], true);
    assert_eq!(report["repos"][0]["untracked_files"], 1);
    assert_eq!(report["secrets"], serde_json::json!(["API_KEY"]));
    let bytes = std::fs::read(&archive).expect("archive");
    assert!(!bytes.windows(5).any(|window| window == b"k-123"));

    let wrong = vm
        .command(&["ws", "import", &archive.to_string_lossy()])
        .env(pom_passphrase_env(), "wrong")
        .output()
        .expect("pom");
    assert_eq!(wrong.status.code(), Some(1));
    assert!(text(&wrong).contains("wrong password"), "{}", text(&wrong));

    let imported = vm.pom_with_passphrase(&["ws", "import", &archive.to_string_lossy(), "--json"]);
    assert!(imported.status.success(), "{}", text(&imported));
    let report = json(&imported);
    assert_eq!(report["action"], "import");
    assert_eq!(report["branch"], "feat-x");

    let target = vm.worktree("feat-x");
    assert_eq!(
        git(&target, &["rev-parse", "HEAD"]),
        git(&source, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        git(&target, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feat-x"
    );
    assert_eq!(
        git(&target, &["diff", "HEAD"]),
        git(&source, &["diff", "HEAD"])
    );
    assert_eq!(
        git(&target, &["diff", "--cached", "--name-only"]),
        "staged.txt"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("notes.md")).expect("untracked"),
        "draft\n"
    );
    assert!(!target.join("debug.log").exists());

    let vm_store = SecretStore::new(vm.state(), "demo");
    assert_eq!(
        vm_store.get("API_KEY").expect("get").as_deref(),
        Some("k-123")
    );
    assert_eq!(vm_store.get("UNUSED").expect("get"), None);
    assert_ne!(
        std::fs::read(mac.state().path("secret.key")).expect("mac key"),
        std::fs::read(vm.state().path("secret.key")).expect("vm key")
    );
    assert_eq!(vm.env_value("feat-x", "API_KEY"), "k-123");
    let (mac_port, vm_port) = (
        mac.env_value("feat-x", "WEB_PORT"),
        vm.env_value("feat-x", "WEB_PORT"),
    );
    assert!(!vm_port.is_empty());
    assert_ne!(mac_port, vm_port);

    let again = vm.pom_with_passphrase(&["ws", "import", &archive.to_string_lossy()]);
    assert_eq!(again.status.code(), Some(1));
    assert!(text(&again).contains("already exists"), "{}", text(&again));

    let renamed = vm.pom_with_passphrase(&[
        "ws",
        "import",
        &archive.to_string_lossy(),
        "--branch",
        "feat-x-copy",
    ]);
    assert!(renamed.status.success(), "{}", text(&renamed));
    assert_eq!(
        git(
            &vm.worktree("feat-x-copy"),
            &["rev-parse", "--abbrev-ref", "HEAD"]
        ),
        "feat-x-copy"
    );
}

#[test]
fn an_archive_from_a_newer_pom_is_refused() {
    let temp = tempfile::Builder::new()
        .prefix("pomx")
        .tempdir_in("/tmp")
        .expect("temp");
    let origin = origin(temp.path());
    let vm = Machine::new(temp.path().join("vm"), &origin);
    let staging = temp.path().join("empty");
    std::fs::create_dir_all(&staging).expect("staging");
    let manifest = pom_transfer::Manifest {
        schema: "pom.wsbundle/v2".into(),
        project: "demo".into(),
        branch: "feat-x".into(),
        ..pom_transfer::Manifest::default()
    };
    let archive = temp.path().join("future.pomws");
    let mut file = std::fs::File::create(&archive).expect("archive");
    pom_transfer::pack(&manifest, &staging, &mut file).expect("pack");

    let refused = vm.pom(&["ws", "import", &archive.to_string_lossy()]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(text(&refused).contains("newer pom"), "{}", text(&refused));
    assert!(!vm.root().join("workspace--feat-x").exists());

    let unknown = Machine::new(temp.path().join("other"), &origin);
    std::fs::write(
        unknown.root().join("pom.yml"),
        CONFIG.replace("session: demo", "session: shop"),
    )
    .expect("pom.yml");
    let manifest = pom_transfer::Manifest {
        schema: pom_transfer::SCHEMA.into(),
        ..manifest
    };
    let mut file = std::fs::File::create(&archive).expect("archive");
    pom_transfer::pack(&manifest, &staging, &mut file).expect("pack");
    let missing = unknown.pom(&["ws", "import", &archive.to_string_lossy()]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        text(&missing).contains("pom project import"),
        "{}",
        text(&missing)
    );
}

const DB_CONFIG: &str = r#"session: SESSION
default_branch: main
shared_services:
  postgres:
    image: postgres:16-alpine
    ports: ["PORT:5432"]
    db_user: postgres
    db_password: postgres
repos:
  api:
    databases:
      main: "api_{{branch.safe}}"
    env:
      DATABASE: "{{db.main}}"
"#;

/// Drops a throwaway shared stack and its volumes.
struct Teardown(PathBuf, String);

impl Drop for Teardown {
    fn drop(&mut self) {
        let status = Command::new("docker")
            .env("PATH", pom_services::tool_path())
            .args(["compose", "-f"])
            .arg(&self.0)
            .args(["-p", &format!("{}-shared", self.1), "down", "-v"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Err(error) = status {
            eprintln!("teardown: {error}");
        }
    }
}

fn psql(session: &str, database: &str, sql: &str) -> String {
    let output = Command::new("docker")
        .env("PATH", pom_services::tool_path())
        .args([
            "exec",
            &format!("{session}-shared-postgres-1"),
            "psql",
            "-U",
            "postgres",
            "-d",
            database,
            "-tAc",
            sql,
        ])
        .output()
        .expect("docker exec psql");
    assert!(
        output.status.success(),
        "psql {sql}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Opt-in (`POM_DOCKER_TEST=1`): a real shared Postgres per machine, each its own session prefix.
#[test]
fn a_workspace_database_travels_with_its_rows() {
    if std::env::var_os("POM_DOCKER_TEST").is_none() {
        return;
    }
    let temp = tempfile::Builder::new()
        .prefix("pomx")
        .tempdir_in("/tmp")
        .expect("temp");
    let origin = origin(temp.path());
    let machines = [("pomxfera", 25446), ("pomxferb", 25447)].map(|(session, port)| {
        let machine = Machine::new(temp.path().join(session), &origin);
        std::fs::write(
            machine.root().join("pom.yml"),
            DB_CONFIG
                .replace("SESSION", session)
                .replace("PORT", &port.to_string()),
        )
        .expect("pom.yml");
        let teardown = Teardown(
            machine.root().join("docker-compose.shared.yml"),
            session.to_string(),
        );
        (machine, teardown)
    });
    let [(mac, _mac_down), (vm, _vm_down)] = &machines;

    let created = mac.pom(&["ws", "create", "feat-db"]);
    assert!(created.status.success(), "{}", text(&created));
    psql(
        "pomxfera",
        "pomxfera_api_feat-db",
        "CREATE TABLE orders (id int, note text); INSERT INTO orders VALUES (1, 'first'), (2, 'second')",
    );

    let archive = temp.path().join("feat-db.pomws");
    let exported = mac.pom(&[
        "ws",
        "export",
        "feat-db",
        "-o",
        &archive.to_string_lossy(),
        "--json",
    ]);
    assert!(exported.status.success(), "{}", text(&exported));
    assert_eq!(
        json(&exported)["databases"][0]["name"],
        "pomxfera_api_feat-db"
    );

    let imported = vm.pom(&[
        "ws",
        "import",
        &archive.to_string_lossy(),
        "--project",
        "pomxferb",
        "--json",
    ]);
    assert!(imported.status.success(), "{}", text(&imported));
    let report = json(&imported);
    assert_eq!(report["databases"][0]["from"], "pomxfera_api_feat-db");
    assert_eq!(report["databases"][0]["name"], "pomxferb_api_feat-db");
    assert_eq!(
        psql(
            "pomxferb",
            "pomxferb_api_feat-db",
            "SELECT string_agg(note, ',' ORDER BY id) FROM orders"
        ),
        "first,second"
    );
    assert_eq!(vm.env_value("feat-db", "DATABASE"), "pomxferb_api_feat-db");
}
