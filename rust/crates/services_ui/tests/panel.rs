//! The panel against real holders: the tree it shows, starting a service from a row, its console tab, and
//! the context menu.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner};
use services_ui::{ServicesContext, ServicesPanel, Status};
use workspace::{PaneKind, PanelRequest, SidePanelView};

const TIMEOUT: Duration = Duration::from_secs(15);
const ROW_STRIDE: u64 = 16;

/// Stands in for the app binary's `pty run`: the wrapper script re-enters this test binary here.
#[test]
fn holder_entry() {
    let Some(count) = std::env::var("POM_TEST_PTY_ARGC")
        .ok()
        .and_then(|count| count.parse::<usize>().ok())
    else {
        return;
    };
    let mut args = vec!["pomelo".to_string()];
    args.extend(
        (0..count).filter_map(|index| std::env::var(format!("POM_TEST_PTY_ARG_{index}")).ok()),
    );
    std::process::exit(pom_ptyhost::cli::run(&args).unwrap_or(2));
}

struct Fixture {
    _temp: tempfile::TempDir,
    holders: SocketDir,
    panel: ServicesPanel,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("svp")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join("workspace--feat/api")).expect("worktree");
        std::fs::write(
            root.join("pom.yml"),
            r#"session: demo
repos:
  api:
    profiles: [staging]
    services:
      web:
        type: backend
        mode: dev
        modes:
          dev: "echo web-up-$PORT; exec nc -lk 127.0.0.1 $PORT"
          prod: "echo prod; exec nc -lk 127.0.0.1 $PORT"
        cmd: "echo fallback"
      worker:
        cmd: "echo worker-up; exec sleep 60"
"#,
        )
        .expect("pom.yml");
        let config = Config::load(&root.join("pom.yml")).expect("config");
        let holders = SocketDir::new(temp.path().join("s"));
        let runner = ServiceRunner::new(RunnerOptions {
            project_root: root.clone(),
            session: "demo".into(),
            state: StateDir::new(temp.path().join("state")),
            holders: holders.clone(),
            binary: wrapper_script(temp.path()),
            docker: "docker".into(),
        });
        let panel = ServicesPanel::new(
            ServicesContext {
                runner: Arc::new(runner),
                config: Arc::new(RwLock::new(Some(Arc::new(config)))),
                branch: "feat".into(),
                is_main: false,
                waker: Arc::new(|| {}),
            },
            root.join("workspace--feat"),
        );
        Fixture {
            _temp: temp,
            holders,
            panel,
        }
    }

    fn id(&self, row: u64, control: u64) -> u64 {
        workspace::side_panel_base(PaneKind::Services) + row * ROW_STRIDE + control
    }

    fn wait_for(&mut self, what: &str, mut ready: impl FnMut(&mut ServicesPanel) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while !ready(&mut self.panel) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.panel.render(300.0, 400.0);
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let names: Vec<String> = self
            .holders
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        self.holders.kill_holders_now(&names);
    }
}

fn wrapper_script(dir: &Path) -> PathBuf {
    let zdotdir = dir.join("zdot");
    std::fs::create_dir_all(&zdotdir).expect("zdotdir");
    let test_binary = std::env::current_exe().expect("test binary");
    let script = dir.join("pty-host");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ni=0\nfor arg; do export \"POM_TEST_PTY_ARG_$i=$arg\"; i=$((i+1)); done\n\
             export POM_TEST_PTY_ARGC=$i ZDOTDIR='{}'\n\
             exec '{}' holder_entry --exact --nocapture\n",
            zdotdir.display(),
            test_binary.display()
        ),
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

#[test]
fn start_from_a_row_then_open_its_console_and_close_it_without_stopping() {
    let mut fixture = Fixture::new();
    fixture.panel.render(300.0, 400.0);
    assert_eq!(fixture.panel.status("api", "web"), Status::Stopped);

    // Row 0 is the `api` group, row 1 is `web`; control 1 is Start.
    fixture.panel.click(fixture.id(1, 1));
    fixture.wait_for("web to run", |panel| {
        panel.status("api", "web") == Status::Running
    });

    fixture.panel.click(fixture.id(1, 0));
    let requests = fixture.panel.take_requests();
    let Some(PanelRequest::Reveal { id, open }) = requests.into_iter().next() else {
        panic!("clicking a running service opens its console");
    };
    assert_eq!(id, "service:svc-demo-feat-api-web");
    let mut console = open().expect("console item");
    assert_eq!(console.title(), "api/web");
    assert_eq!(
        console.id().as_deref(),
        Some("service:svc-demo-feat-api-web")
    );
    assert!(
        console.serialize().is_none(),
        "consoles are not restored as shells"
    );
    console.closed();
    drop(console);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        fixture.holders.holder_alive("svc-demo-feat-api-web"),
        "closing the console leaves the service running"
    );

    // Control 2 is Stop.
    fixture.panel.click(fixture.id(1, 2));
    fixture.wait_for("web to stop", |panel| {
        panel.status("api", "web") == Status::Stopped
    });
    assert!(!fixture.holders.holder_alive("svc-demo-feat-api-web"));
}

#[test]
fn start_all_starts_every_service_of_the_repo() {
    let mut fixture = Fixture::new();
    fixture.panel.render(300.0, 400.0);
    // Control 5 on the group row is Start all.
    fixture.panel.click(fixture.id(0, 5));
    fixture.wait_for("both services to run", |panel| {
        panel.status("api", "web") == Status::Running
            && panel.status("api", "worker") == Status::Running
    });
}

#[test]
fn the_context_menu_offers_modes_and_profiles_with_the_current_ones_checked() {
    let mut fixture = Fixture::new();
    fixture.panel.render(300.0, 400.0);
    assert!(fixture.panel.open_menu(fixture.id(1, 0)));
    let items: Vec<(String, bool)> = fixture
        .panel
        .menu_items()
        .into_iter()
        .map(|item| (item.label.to_string(), item.checked))
        .collect();
    let labels: Vec<&str> = items.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Start",
            "Use a New Port",
            "Mode: dev",
            "Mode: prod",
            "Env: local",
            "Env: staging"
        ]
    );
    let checked: Vec<&str> = items
        .iter()
        .filter(|(_, checked)| *checked)
        .map(|(label, _)| label.as_str())
        .collect();
    assert_eq!(checked, ["Mode: dev", "Env: local"]);

    let staging = fixture
        .panel
        .menu_items()
        .into_iter()
        .find(|item| item.label == "Env: staging")
        .expect("staging item");
    fixture.panel.menu_action(staging.id);
    assert!(fixture.panel.open_menu(fixture.id(1, 0)));
    let now_checked: Vec<String> = fixture
        .panel
        .menu_items()
        .into_iter()
        .filter(|item| item.checked)
        .map(|item| item.label.to_string())
        .collect();
    assert_eq!(now_checked, ["Mode: dev", "Env: staging"]);
    assert!(
        !fixture.panel.open_menu(fixture.id(0, 0)),
        "a group row has no menu"
    );
}

#[test]
fn collapsing_a_repo_hides_its_services() {
    let mut fixture = Fixture::new();
    fixture.panel.render(300.0, 400.0);
    fixture.panel.click(fixture.id(0, 0));
    fixture.panel.render(300.0, 400.0);
    assert!(
        !fixture.panel.open_menu(fixture.id(1, 0)),
        "no service row is left under the collapsed group"
    );
}

struct SharedFixture {
    _temp: tempfile::TempDir,
    log: PathBuf,
    holders: SocketDir,
    panel: ServicesPanel,
}

fn shared_fixture() -> SharedFixture {
    let temp = tempfile::Builder::new()
        .prefix("svs")
        .tempdir_in("/tmp")
        .expect("temp");
    let root = temp.path().join("project");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(
        root.join("pom.yml"),
        "session: demo\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\nrepos:\n  api:\n    services:\n      web: rails s\n",
    )
    .expect("pom.yml");
    let config = Config::load(&root.join("pom.yml")).expect("config");
    let log = temp.path().join("docker.log");
    let docker = temp.path().join("docker");
    std::fs::write(
        &docker,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n  compose*\" ps \"*) echo postgres ;;\nesac\nexit 0\n",
            log.display()
        ),
    )
    .expect("docker");
    std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    std::fs::write(root.join(pom_services::COMPOSE_FILE), "").expect("compose file");
    let holders = SocketDir::new(temp.path().join("s"));
    let runner = ServiceRunner::new(RunnerOptions {
        project_root: root.clone(),
        session: "demo".into(),
        state: StateDir::new(temp.path().join("state")),
        holders: holders.clone(),
        binary: PathBuf::from("/nonexistent"),
        docker,
    });
    let panel = ServicesPanel::new(
        ServicesContext {
            runner: Arc::new(runner),
            config: Arc::new(RwLock::new(Some(Arc::new(config)))),
            branch: "main".into(),
            is_main: true,
            waker: Arc::new(|| {}),
        },
        root,
    );
    SharedFixture {
        _temp: temp,
        log,
        holders,
        panel,
    }
}

/// Rows: 0 api, 1 web, 2 Shared, 3 postgres, 4 redis.
fn shared_row(row: u64, control: u64) -> u64 {
    workspace::side_panel_base(PaneKind::Services) + row * ROW_STRIDE + control
}

fn wait_until_postgres_runs(fixture: &mut SharedFixture) {
    let deadline = Instant::now() + TIMEOUT;
    // The first container check runs on the poller thread; a running row offers Stop (control 2).
    loop {
        fixture.panel.render(300.0, 400.0);
        fixture.panel.set_hover(Some(shared_row(3, 0)));
        let node = fixture.panel.render(300.0, 400.0);
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, 300.0, 400.0, ui::Rgba::TRANSPARENT),
        );
        if painted.hits.iter().any(|(_, id)| *id == shared_row(3, 2)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "postgres never showed as running"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn docker_calls(fixture: &SharedFixture) -> String {
    std::fs::read_to_string(&fixture.log).unwrap_or_default()
}

#[test]
fn shared_services_show_their_container_state_and_take_actions() {
    let mut fixture = shared_fixture();
    wait_until_postgres_runs(&mut fixture);
    fixture.panel.click(shared_row(3, 2));
    let deadline = Instant::now() + TIMEOUT;
    while !docker_calls(&fixture).contains("stop postgres") {
        assert!(
            Instant::now() < deadline,
            "nothing else runs, so it stops without asking"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    fixture.panel.click(shared_row(4, 0));
    let requests = fixture.panel.take_requests();
    assert!(
        matches!(requests.first(), Some(PanelRequest::Reveal { id, .. }) if id == "shared-log:redis"),
        "clicking a shared service follows its log"
    );
}

#[test]
fn stopping_a_shared_service_asks_while_other_workspaces_run_services() {
    let mut fixture = shared_fixture();
    let other = "svc-demo-feat_x-api-web";
    std::fs::create_dir_all(fixture.holders.root()).expect("sockets");
    std::fs::write(
        fixture.holders.pidfile(other),
        format!("{}\n{other}\n", std::process::id()),
    )
    .expect("pidfile");
    wait_until_postgres_runs(&mut fixture);

    fixture.panel.click(shared_row(3, 2));
    let asked = |panel: &mut ServicesPanel| {
        panel
            .take_requests()
            .into_iter()
            .find_map(|request| match request {
                PanelRequest::Prompt { tag, detail, .. } => Some((tag, detail.unwrap_or_default())),
                _ => None,
            })
    };
    let (tag, detail) = asked(&mut fixture.panel).expect("a confirmation prompt");
    assert!(detail.contains("1 service in other workspaces"), "{detail}");
    fixture.panel.prompt_answered(tag, 1);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !docker_calls(&fixture).contains("stop postgres"),
        "cancel keeps it running"
    );

    fixture.panel.click(shared_row(3, 2));
    let (tag, _) = asked(&mut fixture.panel).expect("asked again");
    fixture.panel.prompt_answered(tag, 0);
    let deadline = Instant::now() + TIMEOUT;
    while !docker_calls(&fixture).contains("stop postgres") {
        assert!(Instant::now() < deadline, "confirming stops it");
        std::thread::sleep(Duration::from_millis(50));
    }
}
