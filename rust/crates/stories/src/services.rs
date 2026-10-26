//! The Services panel and its tabs over one project, on this machine and on a host.

use workspace::SidePanelView;

use crate::fixtures::Scratch;
use crate::{Story, StoryDef};

/// The project the services stories draw, local and remote alike.
pub const SERVICES_YML: &str = "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\n  files:\n    image: minio/minio\n  mail:\n    image: axllent/mailpit\n  mock-as:\n    cmd: node scripts/mock-as.js\n    repo: api\nworkspace_services:\n  gateway:\n    cmd: caddy run\n    port: 8080\nrepos:\n  api:\n    env:\n      .env.development:\n        RAILS_ENV: development\n    shared_services: [postgres, redis, files, mock-as]\n    services:\n      server:\n        type: backend\n        port: true\n        mode: dev\n        modes:\n          dev: bin/rails server -p $PORT\n      jobs:\n        mode: dev\n        modes:\n          dev: bundle exec sidekiq\n      mailer: bin/mailer\n  web:\n    services:\n      app:\n        type: frontend\n        port: true\n        mode: hmr\n        modes:\n          hmr: npm run dev -- --port $PORT\n      storybook:\n        type: frontend\n        cmd: npm run storybook -- -p $PORT\n";

/// A panel over the services project; its scratch folder goes when it does.
pub struct ServicesFixture {
    pub panel: services_ui::ServicesPanel,
    _scratch: Option<Scratch>,
}

/// The panel with api/server up 12 minutes, api/jobs crashed, web/storybook kept off its port and three shared
/// services up; `stopped` ("repo/service") is stopped instead. On a host the same facts come from its server.
pub fn fixture(on_host: bool, stopped: Option<&str>) -> anyhow::Result<ServicesFixture> {
    if on_host {
        return Ok(ServicesFixture {
            panel: crate::fixtures::host::services_panel(SERVICES_YML, stopped)?,
            _scratch: None,
        });
    }
    let scratch = Scratch::new("svc")?;
    let dir = scratch.path().to_path_buf();
    std::fs::create_dir_all(dir.join("workspace--feat-login"))?;
    std::fs::write(dir.join("pom.yml"), SERVICES_YML)?;
    let config = pom_config::Config::load(&dir.join("pom.yml"))?;
    let runner = std::sync::Arc::new(pom_services::ServiceRunner::new(
        pom_services::RunnerOptions {
            project_root: dir.clone(),
            session: "myproject".into(),
            state: pom_paths::StateDir::new(dir.join("state")),
            holders: pom_ptyhost::SocketDir::new(dir.join("s")),
            binary: "/nonexistent".into(),
            docker: "/nonexistent".into(),
        },
    ));
    let mut panel = services_ui::ServicesPanel::new(
        services_ui::ServicesContext {
            runner: runner.clone(),
            config: std::sync::Arc::new(std::sync::RwLock::new(Some(std::sync::Arc::new(config)))),
            branch: "feat-login".into(),
            ticket: "PROJ-101".into(),
            is_main: false,
            waker: std::sync::Arc::new(|| {}),
        },
        dir.join("workspace--feat-login"),
    );
    // The facts the boards show: leased ports behind the proxy, and api/server's holder up for 12 minutes.
    for (repo, service, port) in [("api", "server", 3000), ("web", "app", 5173)] {
        let key = pom_ports::service_key(
            &pom_env::port_ws_key("feat-login"),
            &format!("{repo}~{service}"),
        );
        runner.ports().acquire_preferred(&key, port, 1);
    }
    let server = runner.holder_name(&pom_services::ServiceTarget {
        branch: "feat-login".into(),
        is_main: false,
        repo: "api".into(),
        service: "server".into(),
    });
    let pidfile = runner.holders().pidfile(&server);
    if let Some(parent) = pidfile.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pidfile, "48213\n")?;
    std::fs::File::options()
        .write(true)
        .open(&pidfile)?
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(12 * 60 + 5))?;
    use services_ui::Status;
    panel.show_status("_ws", "gateway", Status::Running, None, None);
    panel.show_status("api", "server", Status::Running, None, None);
    panel.show_status(
        "api",
        "jobs",
        Status::Crashed,
        None,
        Some(services_ui::Crash {
            line: Some("KeyError: key not found: \"REDIS_URL\"".into()),
            exit: "exit 1".into(),
            at: Some(std::time::SystemTime::now() - std::time::Duration::from_secs(130)),
        }),
    );
    panel.show_status("web", "app", Status::Running, None, None);
    panel.show_status(
        "web",
        "storybook",
        Status::Stopped,
        Some("port 6006 is already in use by node (pid 48213)".into()),
        None,
    );
    for name in ["postgres", "redis", "files"] {
        panel.show_shared_running(name);
    }
    if let Some((repo, service)) = stopped.and_then(|which| which.split_once('/')) {
        panel.show_status(repo, service, Status::Stopped, None, None);
    }
    Ok(ServicesFixture {
        panel,
        _scratch: Some(scratch),
    })
}

const SERVICE_LOG: &[&str] = &[
    "[vite] hmr update /src/pages/Login.tsx",
    "[vite] page reload src/main.tsx",
    "Warning: something is deprecated",
    "[vite] page reload src/main.tsx",
    "Error: could not resolve 'vite-plugin-svgr'",
    "[vite] hmr update /src/components/Button.tsx",
];

const SHARED_LOG: &[&str] = &[
    "LOG:  database system is ready to accept connections",
    "LOG:  checkpoint starting: time",
    "ERROR:  relation \"accounts\" does not exist at character 15",
    "LOG:  checkpoint complete: wrote 3 buffers",
];

#[derive(Clone, Copy)]
enum View {
    Panel,
    /// The panel with the context menu of the row named so open under it.
    Menu(&'static str),
    Tab(&'static str, &'static str),
    Shared(&'static str),
}

struct ServicesStory {
    fixture: ServicesFixture,
    view: View,
}

impl Story for ServicesStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        let panel = &mut self.fixture.panel;
        match self.view {
            View::Panel => panel.render(width, height),
            View::Menu(_) => beside_editor(panel, height),
            View::Tab(repo, service) => {
                panel.tab_preview(repo, service, SERVICE_LOG, "", width, height)
            }
            View::Shared(name) => panel.shared_tab_preview(name, SHARED_LOG, width, height),
        }
    }

    fn click(&mut self, id: u64) {
        if matches!(self.view, View::Panel | View::Menu(_)) {
            self.fixture.panel.click(id);
        }
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        matches!(self.view, View::Panel | View::Menu(_)) && self.fixture.panel.set_hover(id)
    }

    fn scroll(&mut self, dy: f32) -> bool {
        matches!(self.view, View::Panel | View::Menu(_)) && self.fixture.panel.scroll(dy)
    }

    fn overlay(&mut self, area: ui::Rect) -> Option<ui::Painted> {
        let View::Menu(name) = self.view else {
            return None;
        };
        let panel = &mut self.fixture.panel;
        let drawn = ui::render_interactive(
            &beside_editor(panel, area.h),
            area,
            ui::Interaction::default(),
        );
        let id = panel.row_id(name)?;
        let row = drawn.hits.iter().find(|(_, hit)| *hit == id)?.0;
        if !panel.open_menu(id) {
            return None;
        }
        let items = panel.menu_items();
        let (card, menu_w, menu_h) = workspace::context_menu_card(&items, None);
        let x = (row.x + 48.0)
            .min(area.x + area.w - menu_w - 6.0)
            .max(area.x + 6.0);
        let below = row.y + row.h + 2.0;
        let y = if below + menu_h + 6.0 <= area.y + area.h {
            below
        } else {
            (row.y - menu_h - 2.0).max(area.y + 6.0)
        };
        Some(ui::render(
            &card,
            ui::Rect::new(x, y, menu_w, menu_h, ui::Rgba::TRANSPARENT),
        ))
    }

    fn context_menu(&mut self, id: u64) -> Vec<components::MenuItem> {
        let panel = &mut self.fixture.panel;
        if matches!(self.view, View::Panel | View::Menu(_)) && panel.open_menu(id) {
            panel.menu_items()
        } else {
            Vec::new()
        }
    }

    fn submenu(&self, id: u64) -> Vec<components::MenuItem> {
        self.fixture.panel.submenu_items(id)
    }

    fn menu_action(&mut self, item: u64) {
        self.fixture.panel.menu_action(item);
    }

    fn text(&mut self, text: &str) -> bool {
        matches!(self.view, View::Panel | View::Menu(_)) && self.fixture.panel.text(text)
    }

    fn key(&mut self, key: workspace::EditKey, shift: bool) -> bool {
        matches!(self.view, View::Panel | View::Menu(_)) && self.fixture.panel.key(key, shift)
    }
}

/// The panel in its dock with the editor's background beside it, so an open menu has room to spread.
fn beside_editor(panel: &mut services_ui::ServicesPanel, height: f32) -> ui::Node {
    let colors = ui::theme();
    ui::div()
        .row()
        .h_px(height)
        .child(ui::div().w_px(PANEL.0).child(panel.render(PANEL.0, height)))
        .child(ui::div().w_px(1.0).bg(colors.border))
        .child(ui::div().flex(1.0).bg(colors.background))
        .into()
}

fn build(on_host: bool, stopped: Option<&str>, view: View) -> anyhow::Result<Box<dyn Story>> {
    Ok(Box::new(ServicesStory {
        fixture: fixture(on_host, stopped)?,
        view,
    }))
}

const PANEL: (f32, f32) = (320.0, 900.0);
const TAB: (f32, f32) = (1000.0, 520.0);
const WITH_MENU: (f32, f32) = (PANEL.0 + 280.0, PANEL.1);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, group, title, size, build| StoryDef {
        id,
        page: "Organisms",
        group,
        title,
        size,
        build,
    };
    vec![
        story(
            "services-panel",
            "Services panel",
            "This machine",
            PANEL,
            || build(false, None, View::Panel),
        ),
        story(
            "services-panel-service-menu",
            "Services panel",
            "Service menu",
            WITH_MENU,
            || build(false, None, View::Menu("jobs")),
        ),
        story(
            "services-panel-repo-menu",
            "Services panel",
            "Repo menu",
            WITH_MENU,
            || build(false, None, View::Menu("api")),
        ),
        story(
            "services-panel-shared-menu",
            "Services panel",
            "Shared service menu",
            WITH_MENU,
            || build(false, None, View::Menu("postgres")),
        ),
        story(
            "services-panel-stopped",
            "Services panel",
            "One service stopped",
            PANEL,
            || build(false, Some("web/app"), View::Panel),
        ),
        story(
            "services-panel-host",
            "Services panel",
            "On a host",
            PANEL,
            || build(true, None, View::Panel),
        ),
        story(
            "services-tab-running",
            "Service tab",
            "Running",
            TAB,
            || build(false, None, View::Tab("api", "server")),
        ),
        story(
            "services-tab-stopped",
            "Service tab",
            "Stopped",
            TAB,
            || build(false, Some("web/app"), View::Tab("web", "app")),
        ),
        story(
            "services-tab-crashed",
            "Service tab",
            "Crashed",
            TAB,
            || build(false, None, View::Tab("api", "jobs")),
        ),
        story(
            "services-tab-port",
            "Service tab",
            "Port in use",
            TAB,
            || build(false, None, View::Tab("web", "storybook")),
        ),
        story(
            "services-tab-shared",
            "Service tab",
            "Shared service",
            TAB,
            || build(false, None, View::Shared("postgres")),
        ),
        story(
            "services-tab-host-crashed",
            "Service tab",
            "Crashed on a host",
            TAB,
            || build(true, None, View::Tab("api", "jobs")),
        ),
    ]
}
