//! A project on another machine, drawn from a stand-in source: connected, reconnecting, disconnected, back from
//! a server restart, its local-only panels, and the connect dialog. Nothing reaches ssh.

use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pom_agent::SessionView;
use pom_remote::{
    AgentTurn, Connection, Host, HostUrl, ProjectEntry, ProjectSource, ServiceAction, SourceError,
    StatusReport, WorkspaceList, WorkspaceSummary,
};
use pom_services::{ServiceHealth, SharedHealth};

type SharedLink = Arc<Mutex<pom_rpc::pty::LinkState>>;

fn link_of(link: &SharedLink) -> pom_rpc::pty::LinkState {
    link.lock().map(|state| state.clone()).unwrap_or_default()
}

#[derive(Default)]
struct Board {
    down: AtomicBool,
    shell: Option<Arc<Shell>>,
    /// Pushes who drives the agents and the approval one waits on.
    pushes: bool,
    /// The session's state, once the scene drops it.
    link: SharedLink,
    restarts: u64,
    /// The host's pom, when it is on another minor version than the app.
    older_pom: Option<String>,
    /// Its server opens terminals, whether or not the scene opens one.
    serves_terminals: bool,
    /// What its server tells the Services panel.
    services: Option<pom_remote::server_api::ServicesFacts>,
    /// Its server reads the workspace's files.
    serves_files: bool,
}

/// The workspace folder the boards show on the host.
const WORKTREE: &str = "/home/dev/pom/myproject/workspace--feat-login";
const CONTROLLER: &str = "api/app/controllers/users_controller.rb";

/// The boards' files in a scratch folder, read as if they were on the host; `partial` stops the tree's walk
/// after its first folders, as a large tree still being read.
struct HostFiles {
    local: files::LocalFs,
    partial: bool,
}

struct PartialFeed {
    entries: Option<Vec<files::FileEntry>>,
}

impl files::TreeFeed for PartialFeed {
    fn poll(&mut self) -> Vec<files::TreeEvent> {
        self.entries
            .take()
            .map(|entries| files::TreeEvent::Snapshot {
                entries,
                complete: false,
            })
            .into_iter()
            .collect()
    }
}

impl files::ProjectFs for HostFiles {
    fn local_root(&self) -> Option<&std::path::Path> {
        None
    }

    fn watch(&self, wake: Arc<dyn Fn() + Send + Sync>) -> Box<dyn files::TreeFeed> {
        let Some(root) = files::ProjectFs::local_root(&self.local) else {
            return Box::new(PartialFeed { entries: None });
        };
        if self.partial {
            let entries = files::list(root)
                .into_iter()
                .filter(|entry| {
                    entry.path.starts_with("api") && entry.path.matches('/').count() < 2
                })
                .collect();
            return Box::new(PartialFeed {
                entries: Some(entries),
            });
        }
        self.local.watch(wake)
    }

    fn stat(&self, path: &str) -> std::io::Result<files::FileStat> {
        self.local.stat(path)
    }

    fn read(&self, path: &str) -> std::io::Result<files::FileResult> {
        self.local.read(path)
    }

    fn read_bytes(&self, path: &str, limit: u64) -> std::io::Result<Vec<u8>> {
        self.local.read_bytes(path, limit)
    }

    fn project_files(&self, include_ignored: bool) -> std::io::Result<Vec<String>> {
        self.local.project_files(include_ignored)
    }
}

/// The file tree and editor of the host's workspace, with the controller open when the tree is whole.
fn host_files(partial: bool) -> anyhow::Result<files_ui::FilesView> {
    host_files_opening(partial, !partial)
}

/// The host's files, `open` with the controller open, else with no tab.
fn host_files_opening(partial: bool, open: bool) -> anyhow::Result<files_ui::FilesView> {
    let root = crate::fixture_dir().join("remote");
    crate::editor_fixture::create(&root)?;
    let fs = Arc::new(HostFiles {
        local: files::LocalFs::new(root),
        partial,
    });
    let mut files = files_ui::FilesView::new(fs, WORKTREE.into()).read_only(true);
    let file = std::path::Path::new(WORKTREE).join(CONTROLLER);
    if open {
        workspace::FunctionView::open_file_at(&mut files, &file, None, None);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        workspace::FunctionView::tick_items(&mut files, &|| None);
        let ready = if partial {
            files.scanning()
        } else {
            !files.scanning()
                && (!open || workspace::FunctionView::active_file_path(&files).is_some())
        };
        if ready {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let shown = if partial {
        std::path::Path::new(WORKTREE).join("api/README.md")
    } else {
        file
    };
    workspace::FunctionView::reveal_in_tree(&mut files, &shown);
    Ok(files)
}

const DEV_AGENT: &str = "ws-myproject-feat-login-claude-raw";

const AGENT_OUTPUT: &[u8] = b"\x1b[1m> Fix the failing login spec\x1b[0m\r\n\r\n\x1b[38;2;215;119;87m*\x1b[0m Reading spec/requests/login_spec.rb\r\n\x1b[38;2;215;119;87m*\x1b[0m The spec expects a redirect to /dashboard after sign in.\r\n\x1b[38;2;215;119;87m*\x1b[0m Running the spec again with the fix applied\r\n\r\n  bin/rspec spec/requests/login_spec.rb\r\n";

/// Pushes one agents update, then nothing changes.
struct BoardFeed {
    sent: AtomicBool,
}

impl pom_remote::Feed for BoardFeed {
    fn next(&self, timeout: Duration) -> pom_remote::FeedEvent {
        if self.sent.swap(true, Ordering::SeqCst) {
            std::thread::sleep(timeout);
            return pom_remote::FeedEvent::Idle;
        }
        let sessions = Board::sessions("feat-login");
        let presence = vec![pom_remote::server_api::AgentPresence {
            holder: DEV_AGENT.into(),
            driven_by: Some("an orchestrator".into()),
            pending: Some(pom_agent::PendingApproval {
                request: "r1".into(),
                tool: "Bash".into(),
                summary: "bin/rspec spec/requests/login_spec.rb".into(),
            }),
        }];
        pom_remote::FeedEvent::Update(pom_remote::Update::Agents(
            pom_remote::server_api::AgentsUpdate {
                branch: "feat-login".into(),
                sessions,
                presence,
                error: None,
            },
        ))
    }

    fn follow(&self, _branch: &str, _turn: Option<&str>) -> Result<(), SourceError> {
        Ok(())
    }
}

const SHELL_OUTPUT: &[u8] = b"\x1b[32mdev@vm\x1b[0m:\x1b[34m~/pom/myproject/workspace--feat-login\x1b[0m$ ls\r\napi  web  pom.yml\r\n\x1b[32mdev@vm\x1b[0m:\x1b[34m~/pom/myproject/workspace--feat-login\x1b[0m$ pom status\r\napi/server   running  :3104\r\napi/jobs     running\r\nweb/app      running  :5276\r\nweb/storybook  crashed\r\n\x1b[32mdev@vm\x1b[0m:\x1b[34m~/pom/myproject/workspace--feat-login\x1b[0m$ ";

fn serve_shell(
    request: pom_rpc::Request,
    context: &pom_rpc::RequestContext,
) -> Result<serde_json::Value, pom_rpc::RemoteError> {
    let failed = |error: pom_rpc::Error| pom_rpc::RemoteError::new("failed", error.to_string());
    let params: pom_rpc::pty::AttachParams = serde_json::from_value(request.params)
        .map_err(|error| pom_rpc::RemoteError::new("invalid_params", error.to_string()))?;
    let mut sender = context
        .connection()
        .attach_stream(params.channel, 0)
        .map_err(failed)?;
    let output = if params.holder.starts_with("ws-") {
        AGENT_OUTPUT
    } else {
        SHELL_OUTPUT
    };
    // Written once the dock has laid out and sized the terminal, so no line scrolls off a smaller first size.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        if let Err(error) = sender.write(output) {
            eprintln!("remote fixture: {error}");
        }
        std::thread::sleep(Duration::from_secs(30));
        drop(sender);
    });
    serde_json::to_value(pom_rpc::pty::Attached {
        holder: "term-myproject-feat-login-1-1".into(),
        ..pom_rpc::pty::Attached::default()
    })
    .map_err(|error| pom_rpc::RemoteError::new("internal", error.to_string()))
}

struct Shell {
    client: pom_rpc::Connection,
    _server: pom_rpc::Connection,
    link: SharedLink,
}

impl pom_rpc::pty::PtyProvider for Shell {
    fn attach(&self, request: &pom_rpc::pty::Request) -> Result<pom_rpc::pty::Terminal, String> {
        let holder = match &request.target {
            pom_rpc::pty::Target::Agent(_) => DEV_AGENT.to_string(),
            _ => String::new(),
        };
        let params = pom_rpc::pty::AttachParams {
            holder,
            cols: request.cols,
            rows: request.rows,
            ..pom_rpc::pty::AttachParams::default()
        };
        pom_rpc::pty::Terminal::attach(&self.client, params, Duration::from_secs(5))
            .map_err(|error| error.to_string())
    }

    fn close(&self, _holder: &str) -> Result<(), String> {
        Ok(())
    }

    fn link(&self) -> pom_rpc::pty::LinkState {
        link_of(&self.link)
    }
}

impl Board {
    fn sessions(branch: &str) -> Vec<SessionView> {
        let agent = |role: &str, holder: &str, state: &str, turn: u64| SessionView {
            handle: format!("{branch}/{role}"),
            workspace: branch.into(),
            role: role.into(),
            holder: holder.into(),
            state: state.into(),
            turn,
            alive: true,
            ..SessionView::default()
        };
        vec![
            agent("dev", DEV_AGENT, "tool_use", 7),
            agent(
                "review",
                "ws-myproject-feat-login-claude-review",
                "awaiting_input",
                2,
            ),
        ]
    }

    fn check(&self) -> Result<(), SourceError> {
        if self.down.load(Ordering::SeqCst) {
            return Err(SourceError::Unreachable(
                "ssh: connect to host dev.example.net port 22: Operation timed out".into(),
            ));
        }
        Ok(())
    }
}

fn workspace(branch: &str, name: &str, running: usize, is_main: bool) -> WorkspaceSummary {
    WorkspaceSummary {
        branch: branch.into(),
        is_main,
        path: format!("/home/dev/pom/myproject/workspace--{branch}").into(),
        name: name.into(),
        repos: vec!["api".into(), "web".into()],
        running,
        ..WorkspaceSummary::default()
    }
}

fn service(repo: &str, name: &str, up: bool, port: Option<u16>, mode: &str) -> ServiceHealth {
    ServiceHealth {
        repo: repo.into(),
        service: name.into(),
        up,
        ready: up,
        port,
        mode: mode.into(),
        crashed: !up && name == "storybook",
        ..ServiceHealth::default()
    }
}

impl ProjectSource for Board {
    fn location(&self) -> String {
        "dev (dev@dev.example.net)".into()
    }

    fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
        self.check()?;
        Ok(WorkspaceList {
            schema: pom_remote::WORKSPACE_SCHEMA.into(),
            project: "myproject".into(),
            root: "/home/dev/pom/myproject".into(),
            workspaces: vec![
                workspace("main", "", 0, true),
                workspace("feat-login", "Login page", 4, false),
                workspace("proj-101-payments", "PROJ-101 Payments crash", 0, false),
            ],
        })
    }

    fn status(&self, branch: &str) -> Result<StatusReport, SourceError> {
        self.check()?;
        Ok(StatusReport {
            workspace: branch.into(),
            services: vec![
                service("api", "server", true, Some(3104), "dev"),
                service("api", "jobs", true, None, "dev"),
                service("web", "app", true, Some(5276), "hmr"),
                service("web", "storybook", false, None, ""),
            ],
            shared: vec![
                SharedHealth {
                    name: "postgres".into(),
                    up: true,
                    ready: true,
                    port: 5432,
                },
                SharedHealth {
                    name: "redis".into(),
                    up: true,
                    ready: true,
                    port: 6379,
                },
            ],
        })
    }

    fn logs(&self, _branch: &str, service: &str) -> Result<String, SourceError> {
        self.check()?;
        Ok(format!(
            "=> Booting Puma\n=> Rails 7.1 application starting in development\n* Listening on http://127.0.0.1:3104\nStarted GET \"/users\" for 127.0.0.1\nCompleted 200 OK in 12ms ({service})\n"
        ))
    }

    fn agents(&self, branch: &str) -> Result<Vec<SessionView>, SourceError> {
        self.check()?;
        Ok(Self::sessions(branch))
    }

    fn agent_turn(&self, _: &str, _: &str) -> Result<Option<AgentTurn>, SourceError> {
        self.check()?;
        Ok(Some(AgentTurn {
            turn: 7,
            stop_reason: "end_turn".into(),
            prompt: "Fix the failing login spec".into(),
            text: "The spec expected a redirect to /dashboard; the controller now sends it there."
                .into(),
        }))
    }

    fn act(&self, _: &str, action: ServiceAction, target: &str) -> Result<String, SourceError> {
        self.check()?;
        Ok(format!("{} {target}", action.verb()))
    }

    fn transport(&self) -> Option<pom_remote::TransportInfo> {
        Some(pom_remote::TransportInfo {
            via: pom_remote::Via::Server,
            terminals: self.serves_terminals,
            files: self.serves_files,
            services: if self.services.is_some() {
                pom_remote::ServiceOffers {
                    facts: true,
                    run: true,
                    shared: true,
                    mode: true,
                    env_profile: true,
                    shared_logs: true,
                    tasks: true,
                }
            } else {
                pom_remote::ServiceOffers::default()
            },
            pushes: self.pushes,
            agents: self.pushes,
            version_mismatch: self
                .older_pom
                .as_ref()
                .is_some_and(|version| version != pom_remote::VERSION),
            remote_version: self.older_pom.clone(),
            ..pom_remote::TransportInfo::default()
        })
    }

    fn services_facts(
        &self,
        _branch: &str,
    ) -> Result<pom_remote::server_api::ServicesFacts, SourceError> {
        self.check()?;
        self.services
            .clone()
            .ok_or_else(|| SourceError::Failed("no services".into()))
    }

    fn link(&self) -> Option<pom_remote::LinkReport> {
        Some(pom_remote::LinkReport {
            state: link_of(&self.link),
            restarts: self.restarts,
        })
    }

    fn pty(&self, _branch: &str) -> Option<Arc<dyn pom_rpc::pty::PtyProvider>> {
        self.shell
            .clone()
            .map(|shell| shell as Arc<dyn pom_rpc::pty::PtyProvider>)
    }

    fn events(
        &self,
        _branch: &str,
        _turn: Option<&str>,
    ) -> Result<Option<Arc<dyn pom_remote::Feed>>, SourceError> {
        Ok(self.pushes.then(|| {
            Arc::new(BoardFeed {
                sent: AtomicBool::new(false),
            }) as Arc<dyn pom_remote::Feed>
        }))
    }
}

fn wait_until(watch: &pom_remote::Watch, done: impl Fn(&pom_remote::Snapshot) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&watch.snapshot()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn project_info(snapshot: &pom_remote::Snapshot) -> workspace::ProjectInfo {
    let list = snapshot
        .workspaces
        .as_ref()
        .map(|list| list.workspaces.clone())
        .unwrap_or_default();
    let count = list.len();
    workspace::ProjectInfo {
        states: Vec::new(),
        name: "myproject".into(),
        branch: "main".into(),
        config_path: "/home/dev/pom/myproject/pom.yml".into(),
        host: Some("dev".into()),
        workspaces: list.iter().map(|ws| ws.branch.clone()).collect(),
        active: snapshot.branch.clone(),
        labels: list.iter().map(|ws| ws.name.clone()).collect(),
        running: list.iter().map(|ws| ws.running).collect(),
        tickets: vec![String::new(); count],
        ticket_categories: vec![String::new(); count],
        prs: vec![None; count],
        missing: vec![Vec::new(); count],
        repo_branches: vec![Vec::new(); count],
    }
}

/// A workspace being created on the host, its card fed the progress a job pushes: worktrees made, setup running.
fn creating_on_host() -> workspace::WorkspaceOp {
    use pom_remote::server_api::{progress_kind, Progress};
    let mut op = workspace::WorkspaceOp {
        id: 1,
        branch: "feat-search".into(),
        title: "Search page".into(),
        status: workspace::OpStatus::Queued,
        retryable: true,
        remote: true,
        ..workspace::WorkspaceOp::default()
    };
    let mut events = vec![Progress {
        kind: progress_kind::STAGES.into(),
        stages: pom_workspace::CREATE_STAGES
            .iter()
            .map(|stage| stage.to_string())
            .collect(),
        ..Progress::default()
    }];
    for stage in 0..5 {
        for kind in [progress_kind::STAGE_STARTED, progress_kind::STAGE_DONE] {
            events.push(Progress {
                kind: kind.into(),
                stage: Some(stage),
                ..Progress::default()
            });
        }
    }
    events.push(Progress {
        kind: progress_kind::STAGE_STARTED.into(),
        stage: Some(5),
        ..Progress::default()
    });
    events.push(Progress {
        kind: progress_kind::LINE.into(),
        stage: Some(5),
        line: "api: npm install".into(),
        ..Progress::default()
    });
    for (seq, mut event) in events.into_iter().enumerate() {
        event.seq = seq as u64;
        workspaces_ui::apply_progress(&mut op, &event);
    }
    op
}

fn hosts() -> Vec<Host> {
    [
        ("dev", "ssh://dev@dev.example.net"),
        ("lab", "ssh://me@100.64.0.10:2222"),
    ]
    .iter()
    .map(|(name, url)| Host {
        name: name.to_string(),
        url: HostUrl::parse(url).unwrap_or_else(|error| panic!("{url}: {error}")),
        pom_path: None,
        transport: None,
    })
    .collect()
}

/// Connect to Host in the state a board shows: `hosts`, `empty`, `loading`, `error` or `projects`.
pub fn connect_modal(step: &str) -> Option<remote_ui::ConnectHostModal> {
    let list: remote_ui::ListProjects = match step {
        "loading" => Arc::new(|_: &Host| {
            std::thread::sleep(Duration::from_secs(60));
            Ok(Vec::new())
        }),
        "error" => Arc::new(|_: &Host| {
            Err(SourceError::Unreachable(
                "dev.example.net port 22: Operation timed out".into(),
            ))
        }),
        _ => Arc::new(|host: &Host| {
            Ok(["myproject", "storefront"]
                .iter()
                .map(|name| ProjectEntry {
                    name: name.to_string(),
                    path: format!("/home/{}/pom/{name}", host.name).into(),
                    config: format!("/home/{}/pom/{name}/pom.yml", host.name).into(),
                    ..ProjectEntry::default()
                })
                .collect())
        }),
    };
    let modal = match step {
        "hosts" => return Some(remote_ui::ConnectHostModal::new(Ok(hosts()), list)),
        "empty" => return Some(remote_ui::ConnectHostModal::new(Ok(Vec::new()), list)),
        "loading" => {
            return Some(remote_ui::ConnectHostModal::new(Ok(hosts()), list).on_host("dev"))
        }
        "error" | "projects" => remote_ui::ConnectHostModal::new(Ok(hosts()), list),
        _ => return None,
    };
    let mut modal = modal.on_host("dev");
    let deadline = Instant::now() + Duration::from_secs(5);
    while workspace::WindowModal::busy(&modal) && Instant::now() < deadline {
        workspace::WindowModal::tick(&mut modal);
        std::thread::sleep(Duration::from_millis(5));
    }
    Some(modal)
}

pub fn render(
    mode: &str,
    out: &str,
    (width, height): (f32, f32),
    scale: f32,
    renderer: &mut ui::UiRenderer,
    crop: Option<(u32, u32, u32, u32)>,
) -> anyhow::Result<()> {
    let agents = mode == "agents";
    let dropped = match mode {
        "reconnecting" => Some(pom_rpc::pty::LinkState::Reconnecting {
            attempt: 3,
            next_in: Duration::from_secs(2),
        }),
        "terminal-disconnected" => Some(pom_rpc::pty::LinkState::Disconnected {
            reason: "lost dev: ssh: connect to host dev.example.net port 22: Operation timed out"
                .into(),
        }),
        _ => None,
    };
    let terminal = matches!(mode, "terminal" | "restarted") || dropped.is_some();
    let link = SharedLink::default();
    let shell = (terminal || agents)
        .then(|| pom_rpc::mock::connected(pom_rpc::Config::default(), serve_shell))
        .transpose()?
        .map(|(client, server)| {
            Arc::new(Shell {
                client,
                _server: server,
                link: link.clone(),
            })
        });
    let board = Arc::new(Board {
        shell,
        pushes: agents,
        link: link.clone(),
        restarts: u64::from(mode == "restarted"),
        older_pom: matches!(mode, "version" | "version-commands" | "version-updating")
            .then(|| "0.1.0".to_string())
            .or_else(|| (mode == "files-old-server").then(|| pom_remote::VERSION.to_string())),
        serves_files: mode != "files-old-server",
        serves_terminals: mode != "disconnected",
        services: Some(window_facts()),
        ..Board::default()
    });
    let watch = pom_remote::Watch::start(board.clone(), "feat-login", Arc::new(|| {}));
    watch.set_visible(true);
    wait_until(&watch, |snapshot| {
        snapshot.connection == Connection::Connected
            && snapshot.status.is_some()
            && snapshot.services.is_some()
    });
    if agents {
        wait_until(&watch, |snapshot| !snapshot.presence.is_empty());
    }
    if let Some(state) = dropped {
        if let Ok(mut current) = link.lock() {
            *current = state;
        }
        board.down.store(true, Ordering::SeqCst);
        wait_until(&watch, |snapshot| {
            matches!(
                snapshot.connection,
                Connection::Reconnecting { .. } | Connection::Disconnected { .. }
            )
        });
    }
    if mode == "disconnected" {
        board.down.store(true, Ordering::SeqCst);
        watch.refresh_now();
        wait_until(&watch, |snapshot| {
            matches!(snapshot.connection, Connection::Disconnected { .. })
        });
    }
    let snapshot = watch.snapshot();
    let mut files_view = match mode {
        "files-scanning" => Some(host_files(true)?),
        "files-open" => Some(host_files(false)?),
        "connected" => Some(host_files_opening(false, false)?),
        _ => None,
    };
    let sessions = vec![
        workspace::Session {
            name: "myproject".into(),
            path: "/home/dev/pom/myproject".into(),
            host: Some("dev".into()),
            ..workspace::Session::default()
        },
        workspace::Session {
            name: "storefront".into(),
            path: "/Users/me/code/storefront".into(),
            ..workspace::Session::default()
        },
    ];
    terminal_ui::set_terminal_defaults(15.0, "/bin/sh", 10_000);
    terminal_ui::set_agent_font_size(15.0);
    let mut app = ui::Application::new();
    let (handle, entity) = app.open_raw_window(
        ui::WindowOptions {
            width,
            height,
            scale,
            ..Default::default()
        },
        |_| {
            let mut layout = workspace::Layout {
                project: Some(project_info(&snapshot)),
                ..Default::default()
            };
            layout.left.width = 256.0;
            if mode == "connected" {
                layout.active_panels[workspace::DockPosition::Left.index()] =
                    Some(workspace::Shown::Func(workspace::PaneKind::Services));
            }
            layout.right.width = 380.0;
            layout.bottom.collapsed = true;
            layout.bottom.width = 360.0;
            workspace::WorkspaceView::new(layout)
        },
    );
    entity.update(app.app_mut(), |view, _| {
        view.set_sessions(sessions, Some(0));
        view.set_usage(crate::window_usage());
        view.set_host_link(remote_ui::host_link(&snapshot.connection));
        if mode == "create" {
            view.set_workspace_ops(vec![creating_on_host()]);
        }
        view.set_side_panels(vec![Box::new(remote_panel(&watch))]);
        let update = remote_ui::PomUpdate {
            can_install: mode != "version-commands",
            running: (mode == "version-updating")
                .then_some(pom_remote::update::UpdateStep::Download),
            error: None,
        };
        let (notice, _) =
            remote_ui::remote_notice(&snapshot, watch.location(), "dev", "myproject", &update);
        view.set_center_notice(Some(notice));
        if let Some(files) = files_view.take() {
            view.set_files_view(Some(Box::new(files)));
        }
        if mode == "restarted" {
            view.show_toast(remote_ui::restart_notice("dev"), None);
        }
        if agents {
            let root = std::path::PathBuf::from("/home/dev/pom/myproject/workspace--feat-login");
            let dock = terminal_ui::TerminalPanel::remote_agent_dock(
                root,
                Arc::new(|| {}),
                Some(Box::new(|| {})),
            );
            view.set_terminal_views(None, Some(Box::new(dock)));
            let item_id = services_ui::agent_item_id("feat-login", "dev");
            view.open_agent_item(&item_id, false, || {
                services_ui::remote_agent_item(&watch, "dev", 1)
            });
            return;
        }
        if let Some(provider) = watch.terminals() {
            let root = std::path::PathBuf::from("/home/dev/pom/myproject/workspace--feat-login");
            let panel = terminal_ui::TerminalPanel::with_holders(
                root.clone(),
                Arc::new(|| {}),
                terminal_ui::HolderScope {
                    place: terminal_ui::HolderPlace::Remote(provider),
                    binary: std::path::PathBuf::new(),
                    prefix: "term-myproject-feat-login".into(),
                    group: None,
                },
            );
            view.set_terminal_views(Some(Box::new(panel)), None);
            view.open_terminal_in(root);
        }
    });
    let live = terminal || agents || mode.starts_with("files");
    for _ in 0..if live { 40 } else { 4 } {
        app.draw(handle).expect("frame");
        if live {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let frame = app.draw(handle).expect("frame");
    let lights = crate::scene_lights();
    let mut layers: Vec<ui::Layer> = vec![(
        frame.base.rects.as_slice(),
        frame.base.tris.as_slice(),
        frame.base.texts.as_slice(),
        frame.base.icons.as_slice(),
        None,
    )];
    for overlay in &frame.overlays {
        layers.push((
            overlay.painted.rects.as_slice(),
            overlay.painted.tris.as_slice(),
            overlay.painted.texts.as_slice(),
            overlay.painted.icons.as_slice(),
            overlay.clip.map(|clip| (clip.x, clip.y, clip.w, clip.h)),
        ));
    }
    layers.push((lights.as_slice(), &[], &[], &[], None));
    renderer.render_frame(ui::theme().background, &layers)?;
    let (w, h, rgba) = renderer.read_rgba()?;
    let (w, h, rgba) = match crop {
        Some(rect) => crate::crop_rgba(w, &rgba, rect),
        None => (w, h, rgba),
    };
    let file = std::fs::File::create(out)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    println!("wrote {out} ({w}x{h})");
    Ok(())
}

const ROOT: &str = "/home/dev/pom/myproject/workspace--feat-login";

/// One service as the host's server reports it to the Services panel.
struct Row<'a> {
    repo: &'a str,
    service: &'a str,
    status: &'a str,
    port: Option<u16>,
    mode: &'a str,
    pid: Option<i32>,
    up_for: Option<u64>,
    crash: Option<(&'a str, u64)>,
}

impl Row<'_> {
    fn new<'a>(repo: &'a str, service: &'a str, status: &'a str) -> Row<'a> {
        Row {
            repo,
            service,
            status,
            port: None,
            mode: "",
            pid: None,
            up_for: None,
            crash: None,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The facts a server sends for workspace feat-login of `config`, with `rows` its services' state.
fn facts(config: &str, rows: &[Row<'_>], shared: &[&str]) -> pom_remote::server_api::ServicesFacts {
    use pom_remote::server_api::{CrashFact, ServiceFact, ServicesFacts, SharedFact};
    let parsed =
        pom_config::Config::from_text(config, std::path::Path::new("pom.yml")).unwrap_or_default();
    let now = now_secs();
    let services = rows
        .iter()
        .map(|row| {
            let target = pom_services::ServiceTarget {
                branch: "feat-login".into(),
                is_main: false,
                repo: row.repo.into(),
                service: row.service.into(),
            };
            let url = row.port.and_then(|_| {
                let alias = parsed.repos.get(row.repo).map(|dir| {
                    if dir.alias.is_empty() {
                        row.repo.to_string()
                    } else {
                        dir.alias.clone()
                    }
                })?;
                Some(format!(
                    "http://{}.{alias}.feat-login.localhost:8767",
                    row.service
                ))
            });
            ServiceFact {
                repo: row.repo.into(),
                service: row.service.into(),
                holder: pom_services::service_holder_name("myproject", &target),
                status: row.status.into(),
                port: row.port,
                mode: row.mode.into(),
                env_profile: if target.is_workspace_level() {
                    String::new()
                } else {
                    "local".into()
                },
                url,
                pid: row.pid,
                started_at: row.up_for.map(|seconds| now - seconds),
                crash: row.crash.map(|(line, ago)| CrashFact {
                    line: Some(line.into()),
                    exit: "exit 1".into(),
                    at: Some(now - ago),
                }),
            }
        })
        .collect();
    let shared = parsed
        .shared_services
        .iter()
        .map(|(name, def)| SharedFact {
            name: name.clone(),
            running: shared.contains(&name.as_str()),
            host_port: match name.as_str() {
                "postgres" => 5432,
                "redis" => 6379,
                _ => def.port.unwrap_or_default(),
            },
            holder: format!("shared-myproject-{name}"),
            folder: if def.is_command() {
                def.repo.clone()
            } else {
                String::new()
            },
            ..SharedFact::default()
        })
        .collect();
    ServicesFacts {
        branch: "feat-login".into(),
        root: ROOT.into(),
        config: config.to_string(),
        services,
        shared,
        repos: parsed.repos.keys().cloned().collect(),
        others_running: 0,
        public_urls: false,
    }
}

const WINDOW_YML: &str = "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\nrepos:\n  api:\n    shared_services: [postgres, redis]\n    services:\n      server:\n        type: backend\n        port: true\n        mode: dev\n        modes:\n          dev: bin/rails server -p $PORT\n      jobs:\n        mode: dev\n        modes:\n          dev: bundle exec sidekiq\n  web:\n    services:\n      app:\n        type: frontend\n        port: true\n        mode: hmr\n        modes:\n          hmr: npm run dev -- --port $PORT\n      storybook:\n        cmd: npm run storybook -- -p $PORT\n";

/// The host's services as the remote window's boards show them.
fn window_facts() -> pom_remote::server_api::ServicesFacts {
    facts(
        WINDOW_YML,
        &[
            Row {
                port: Some(3104),
                mode: "dev",
                pid: Some(48213),
                up_for: Some(14 * 60),
                ..Row::new("api", "server", "running")
            },
            Row {
                mode: "dev",
                pid: Some(48214),
                up_for: Some(14 * 60),
                ..Row::new("api", "jobs", "running")
            },
            Row {
                port: Some(5276),
                mode: "hmr",
                pid: Some(48230),
                up_for: Some(11 * 60),
                ..Row::new("web", "app", "running")
            },
            Row {
                crash: Some((
                    "Error: listen EADDRINUSE: address already in use :::6006",
                    130,
                )),
                ..Row::new("web", "storybook", "crashed")
            },
        ],
        &["postgres", "redis"],
    )
}

/// The Services panel of the window, over the stand-in host.
fn remote_panel(watch: &pom_remote::Watch) -> services_ui::ServicesPanel {
    let source = Arc::new(services_ui::RemoteServices::new(
        watch.clone(),
        "feat-login",
        "dev",
    ));
    services_ui::ServicesPanel::with_access(
        source,
        services_ui::WorkspaceInfo {
            branch: "feat-login".into(),
            ticket: "PROJ-101".into(),
            is_main: false,
        },
        Arc::new(|| {}),
        ROOT.into(),
    )
}

/// The panel of the services scenes over a host whose server reports what the local scenes show: `config`,
/// api/server up 12 minutes, api/jobs crashed, web/app up (or stopped as `stopped` says), three shared up.
pub fn services_panel(
    config: &str,
    stopped: Option<&str>,
) -> anyhow::Result<services_ui::ServicesPanel> {
    let state = |which: &str| {
        if stopped == Some(which) {
            "stopped"
        } else {
            "running"
        }
    };
    let rows = [
        Row::new("_ws", "gateway", "running"),
        Row {
            port: Some(3000),
            mode: "dev",
            pid: Some(48213).filter(|_| state("api/server") == "running"),
            up_for: Some(12 * 60 + 5).filter(|_| state("api/server") == "running"),
            ..Row::new("api", "server", state("api/server"))
        },
        Row {
            mode: "dev",
            crash: Some(("KeyError: key not found: \"REDIS_URL\"", 130)),
            ..Row::new("api", "jobs", "crashed")
        },
        Row::new("api", "mailer", "stopped"),
        Row {
            port: Some(5173),
            mode: "hmr",
            ..Row::new("web", "app", state("web/app"))
        },
        Row::new("web", "storybook", "stopped"),
    ];
    let board = Arc::new(Board {
        services: Some(facts(config, &rows, &["postgres", "redis", "files"])),
        ..Board::default()
    });
    let watch = pom_remote::Watch::start(board, "feat-login", Arc::new(|| {}));
    watch.set_visible(true);
    wait_until(&watch, |snapshot| snapshot.services.is_some());
    if watch.services().0.is_none() {
        anyhow::bail!("the stand-in host never reported its services");
    }
    Ok(remote_panel(&watch))
}
