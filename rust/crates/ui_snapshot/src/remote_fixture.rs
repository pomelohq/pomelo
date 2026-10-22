//! A project on another machine, drawn from a stand-in source: connected, disconnected, its local-only panels,
//! and the connect dialog. Nothing reaches ssh.

use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pom_agent::SessionView;
use pom_remote::{
    AgentTurn, Connection, Host, HostUrl, ProjectEntry, ProjectSource, ServiceAction, SourceError,
    StatusReport, WorkspaceList, WorkspaceSummary,
};
use pom_services::{ServiceHealth, SharedHealth};

#[derive(Default)]
struct Board {
    down: AtomicBool,
    shell: Option<Arc<Shell>>,
    /// Pushes who drives the agents and the approval one waits on.
    pushes: bool,
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
    sender.write(output).map_err(failed)?;
    std::thread::spawn(move || {
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
            terminals: self.shell.is_some(),
            pushes: self.pushes,
            agents: self.pushes,
            ..pom_remote::TransportInfo::default()
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

fn connect_modal(on_host: bool) -> remote_ui::ConnectHostModal {
    let list: remote_ui::ListProjects = Arc::new(|host: &Host| {
        Ok(["myproject", "storefront"]
            .iter()
            .map(|name| ProjectEntry {
                name: name.to_string(),
                path: format!("/home/{}/pom/{name}", host.name).into(),
                config: format!("/home/{}/pom/{name}/pom.yml", host.name).into(),
                ..ProjectEntry::default()
            })
            .collect())
    });
    let modal = remote_ui::ConnectHostModal::new(Ok(hosts()), list);
    if !on_host {
        return modal;
    }
    let mut modal = modal.on_host("dev");
    let deadline = Instant::now() + Duration::from_secs(5);
    while workspace::WindowModal::busy(&modal) && Instant::now() < deadline {
        workspace::WindowModal::tick(&mut modal);
        std::thread::sleep(Duration::from_millis(5));
    }
    modal
}

pub fn render(
    mode: &str,
    out: &str,
    (width, height): (f32, f32),
    scale: f32,
    renderer: &mut ui::UiRenderer,
) -> anyhow::Result<()> {
    let agents = mode == "agents";
    let shell = (mode == "terminal" || agents)
        .then(|| pom_rpc::mock::connected(pom_rpc::Config::default(), serve_shell))
        .transpose()?
        .map(|(client, server)| {
            Arc::new(Shell {
                client,
                _server: server,
            })
        });
    let board = Arc::new(Board {
        shell,
        pushes: agents,
        ..Board::default()
    });
    let watch = pom_remote::Watch::start(board.clone(), "feat-login", Arc::new(|| {}));
    watch.set_visible(true);
    wait_until(&watch, |snapshot| {
        snapshot.connection == Connection::Connected && snapshot.status.is_some()
    });
    if agents {
        wait_until(&watch, |snapshot| !snapshot.presence.is_empty());
    }
    if mode == "disconnected" {
        board.down.store(true, Ordering::SeqCst);
        watch.refresh_now();
        wait_until(&watch, |snapshot| {
            matches!(snapshot.connection, Connection::Disconnected { .. })
        });
    }
    let connect = matches!(mode, "connect" | "connect-projects");
    let snapshot = watch.snapshot();
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
                project: (!connect).then(|| project_info(&snapshot)),
                ..Default::default()
            };
            layout.left.width = 256.0;
            layout.right.width = 380.0;
            layout.bottom.collapsed = true;
            workspace::WorkspaceView::new(layout)
        },
    );
    entity.update(app.app_mut(), |view, _| {
        view.set_sessions(sessions, (!connect).then_some(0));
        if connect {
            view.open_window_modal(Box::new(connect_modal(mode == "connect-projects")));
            return;
        }
        if mode == "create" {
            view.set_workspace_ops(vec![creating_on_host()]);
        }
        let mut panel = services_ui::RemoteServicesPanel::new(watch.clone());
        if mode == "logs" {
            panel.show_logs("api/server");
        }
        view.set_side_panels(vec![Box::new(panel)]);
        let (notice, _) = remote_ui::remote_notice(&snapshot, watch.location(), "dev", "myproject");
        view.set_center_notice(Some(notice));
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
    if mode == "logs" {
        wait_until(&watch, |snapshot| snapshot.logs.is_some());
    }
    let live = mode == "terminal" || agents;
    for _ in 0..if live { 40 } else { 4 } {
        app.draw(handle).expect("frame");
        if live {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let frame = app.draw(handle).expect("frame");
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
    renderer.render_frame(ui::theme().background, &layers)?;
    let (w, h, rgba) = renderer.read_rgba()?;
    let file = std::fs::File::create(out)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    println!("wrote {out} ({w}x{h})");
    Ok(())
}
