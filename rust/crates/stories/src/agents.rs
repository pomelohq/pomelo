//! The agent dock: its tab bar with the workspace's Main agent pinned first, and the strip over an agent someone
//! else drives, with a tool call waiting on the person.

use std::sync::Mutex;

use workspace::pane::{Pane, PaneClickIds, TabBarButton, TabBarConfig};

use crate::{Story, StoryDef};

struct AgentTab;

impl workspace::Item for AgentTab {
    fn id(&self) -> Option<String> {
        Some("main".into())
    }

    fn title(&self) -> String {
        "Main".into()
    }

    fn tab_icon(&self) -> Option<ui::IconKind> {
        Some(ui::IconKind::Sparkle)
    }

    fn render(&mut self) -> ui::Node {
        ui::div().into()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TakeOver {
    Done,
    /// Takes ten minutes, so the board stays on its busy state.
    Hangs,
    Refused,
}

/// Who drives the session and what it waits on; Take over and the answers change it as the host would.
struct StandInLease(Mutex<terminal_ui::LeaseState>, TakeOver);

impl terminal_ui::LeaseSource for StandInLease {
    fn state(&self) -> terminal_ui::LeaseState {
        self.0.lock().map(|state| state.clone()).unwrap_or_default()
    }

    fn take_over(&self) -> Result<(), String> {
        match self.1 {
            TakeOver::Done => {}
            TakeOver::Hangs => std::thread::sleep(std::time::Duration::from_secs(600)),
            TakeOver::Refused => {
                return Err("the orchestrator still holds the session's keys".into())
            }
        }
        let mut state = self.0.lock().map_err(|error| error.to_string())?;
        state.driver = None;
        Ok(())
    }

    fn answer(&self, _request: &str, _allow: bool) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|error| error.to_string())?;
        state.pending = None;
        Ok(())
    }
}

const IDS: PaneClickIds = PaneClickIds {
    tab_activate: 100,
    tab_close: 200,
    nav_back: 300,
    nav_forward: 301,
    search: 302,
};

struct AgentDock {
    pane: Pane,
    lease: Option<terminal_ui::LeaseBar>,
    hover: Option<u64>,
    opened: std::time::Instant,
}

impl AgentDock {
    fn config(&self) -> TabBarConfig {
        TabBarConfig {
            show_nav: false,
            buttons: [
                ui::IconKind::Plus,
                ui::IconKind::Clock,
                ui::IconKind::Maximize,
            ]
            .into_iter()
            .enumerate()
            .map(|(index, icon)| TabBarButton {
                icon,
                id: 900 + index as u64,
                selected: false,
                disabled: false,
            })
            .collect(),
            toolbar: Vec::new(),
        }
    }
}

impl Story for AgentDock {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        let config = self.config();
        let (nav, buttons) = workspace::pane::tab_bar_edges(&config);
        let tab_h = workspace::pane::TAB_H;
        let bar = ui::div()
            .col()
            .h_px(tab_h)
            .child(workspace::pane::render_pane(
                &self.pane, &config, IDS, width,
            ))
            .pin(
                ui::Corner::TopLeft,
                nav,
                0.0,
                (width - nav - buttons).max(0.0),
                tab_h,
                workspace::pane::render_tab_strip(&self.pane, IDS, self.hover),
            );
        let mut dock = ui::div().col().w_px(width).h_px(height).child(bar);
        if let Some(strip) = self.lease.as_ref().and_then(|lease| lease.render(width)) {
            dock = dock.child(strip);
        }
        dock.child(
            ui::div()
                .col()
                .flex(1.0)
                .bg(ui::theme().terminal_background),
        )
        .into()
    }

    fn click(&mut self, id: u64) {
        if let Some(lease) = self.lease.as_mut() {
            lease.click(id);
        }
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        let changed = self.hover != id;
        self.hover = id;
        changed
    }

    fn animating(&self) -> bool {
        self.lease.as_ref().is_some_and(terminal_ui::LeaseBar::busy)
    }

    // A refused take over answers at once and stills wait for its reason; one that hangs stays busy.
    fn pending(&mut self) -> bool {
        self.opened.elapsed() < std::time::Duration::from_millis(300) && self.animating()
    }
}

fn build(lease: Option<terminal_ui::LeaseState>) -> anyhow::Result<Box<dyn Story>> {
    build_taking(lease, TakeOver::Done, false)
}

fn build_taking(
    lease: Option<terminal_ui::LeaseState>,
    outcome: TakeOver,
    pressed: bool,
) -> anyhow::Result<Box<dyn Story>> {
    workspace::keymap::set_key_hints(&[
        (workspace::keymap::Action::AgentAllow, "cmd-k y".into()),
        (workspace::keymap::Action::AgentDeny, "cmd-k n".into()),
    ]);
    let mut pane = Pane::new(1);
    pane.add_item(Box::new(AgentTab));
    pane.toggle_pin(0);
    let mut lease = lease.map(|state| {
        terminal_ui::LeaseBar::with_source(Box::new(StandInLease(Mutex::new(state), outcome)))
    });
    if pressed {
        if let Some(lease) = lease.as_mut() {
            lease.press(terminal_ui::LeaseButton::TakeOver);
        }
    }
    Ok(Box::new(AgentDock {
        pane,
        lease,
        hover: None,
        opened: std::time::Instant::now(),
    }))
}

fn watching() -> Option<terminal_ui::LeaseState> {
    Some(terminal_ui::LeaseState {
        driver: Some("an orchestrator".into()),
        pending: None,
    })
}

fn approval() -> pom_agent::PendingApproval {
    pom_agent::PendingApproval {
        request: "1".into(),
        tool: "Bash".into(),
        summary: "bundle exec rspec spec/requests/users_spec.rb".into(),
    }
}

const DOCK: (f32, f32) = (440.0, 240.0);
const WIDE: (f32, f32) = (680.0, 200.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, size, build| StoryDef {
        id,
        page: "Organisms",
        group: "Agent dock",
        title,
        size,
        build,
    };
    vec![
        story("agent-dock-main", "Main pinned", DOCK, || build(None)),
        story("agent-dock-watching", "Someone else drives", DOCK, || {
            build(Some(terminal_ui::LeaseState {
                driver: Some("an orchestrator".into()),
                pending: None,
            }))
        }),
        story("agent-dock-taking-over", "Taking over", DOCK, || {
            build_taking(watching(), TakeOver::Hangs, true)
        }),
        story(
            "agent-dock-take-over-failed",
            "Take over failed",
            DOCK,
            || build_taking(watching(), TakeOver::Refused, true),
        ),
        story("agent-dock-approval", "Waiting for approval", DOCK, || {
            build(Some(terminal_ui::LeaseState {
                driver: Some("an orchestrator".into()),
                pending: Some(approval()),
            }))
        }),
        story(
            "agent-dock-approval-wide",
            "Approval on one line",
            WIDE,
            || {
                build(Some(terminal_ui::LeaseState {
                    driver: Some("an orchestrator".into()),
                    pending: Some(approval()),
                }))
            },
        ),
    ]
}
