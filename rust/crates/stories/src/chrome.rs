//! The main window's chrome around the panes: title bar, session menu, status bar, workspace rail and list,
//! the pane tab bar, toasts, the welcome page, a confirmation and both switchers.

use ui::{div, Node};
use workspace::pane::{Pane, PaneClickIds, TabBarButton, TabBarConfig};
use workspace::{
    AgentDot, Layout, Panel, PrSeverity, PrSummary, PrTrouble, ProjectInfo, Session, UsageAccount,
    UsageInfo, UsageToday, UsageWindow, WorkspaceList, WorkspaceRow,
};

use crate::{still, StoryDef};

fn sessions() -> Vec<Session> {
    let session = |name: &str, running: bool| Session {
        name: name.into(),
        path: format!("~/code/{name}").into(),
        running,
        ..Session::default()
    };
    vec![
        session("myproject", true),
        session("billing", false),
        session("docs-site", false),
        Session {
            host: Some("dev-box".into()),
            ..session("platform", true)
        },
    ]
}

fn project(host: Option<&str>) -> ProjectInfo {
    ProjectInfo {
        name: "myproject".into(),
        branch: "main".into(),
        config_path: "~/code/myproject/pom.yml".into(),
        host: host.map(Into::into),
        workspaces: vec!["main".into(), "proj-101-login".into(), "fix-cache".into()],
        active: "proj-101-login".into(),
        labels: vec![String::new(), "Login page".into(), String::new()],
        running: vec![3, 2, 0],
        ..ProjectInfo::default()
    }
}

fn usage() -> UsageInfo {
    UsageInfo {
        account: Some(UsageAccount {
            name: "Sam Lee".into(),
            email: "sam@example.com".into(),
            plan: "Max".into(),
            organization: "Example".into(),
        }),
        session: Some(UsageWindow {
            used: 42.0,
            resets: "in 2h 31m".into(),
        }),
        weekly: Some(UsageWindow {
            used: 18.0,
            resets: "Sat 17:00".into(),
        }),
        note: "Updated 12s ago".into(),
        today: Some(UsageToday {
            total: "$17.25".into(),
            sessions: 4,
            by_workspace: vec![("Login page".into(), "$11.40".into())],
        }),
        hidden: false,
    }
}

fn layout() -> Layout {
    Layout {
        sessions: sessions(),
        current_session: Some(0),
        project: Some(project(None)),
        usage: usage(),
        fullscreen: true,
        ..Layout::default()
    }
}

fn header(layout: &Layout, width: f32) -> Node {
    div()
        .col()
        .w_px(width)
        .h_px(workspace::TOP_BAR_H)
        .bg(ui::theme().title_bar_background)
        .painted(layout.header(width, None))
        .into()
}

fn title_bar(width: f32, _: f32) -> Node {
    header(&layout(), width)
}

fn title_bar_remote(width: f32, _: f32) -> Node {
    let layout = Layout {
        current_session: Some(3),
        project: Some(project(Some("dev-box"))),
        ..layout()
    };
    header(&layout, width)
}

fn title_bar_waiting(width: f32, _: f32) -> Node {
    let mut layout = layout();
    layout
        .agent_states
        .insert("fix-cache".into(), AgentDot::AwaitingInput);
    layout.attention_keys = Some("cmd-shift-j".into());
    header(&layout, width)
}

fn title_bar_update(width: f32, _: f32) -> Node {
    let mut layout = layout();
    layout.update.button = Some(workspace::UpdateButton {
        label: "Restart to Update".into(),
        icon: ui::IconKind::Download,
        progress: None,
        tooltip: None,
        tone: workspace::UpdateTone::Ready,
        dismissable: true,
        click: Some(workspace::UpdateAction::Restart),
    });
    header(&layout, width)
}

fn title_bar_linux(width: f32, _: f32) -> Node {
    let layout = Layout {
        fullscreen: false,
        window_controls: true,
        ..layout()
    };
    header(&layout, width)
}

fn session_menu(width: f32, height: f32) -> Node {
    let layout = Layout {
        session_menu: true,
        ..layout()
    };
    let mut painted = layout.header(width, None);
    let menu = layout.session_menu("", None, true);
    painted.append(menu.fixed);
    painted.append(menu.list);
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .child(
            div()
                .w_px(width)
                .h_px(workspace::TOP_BAR_H)
                .bg(ui::theme().title_bar_background),
        )
        .painted(painted)
        .into()
}

fn status_bar(width: f32, _: f32) -> Node {
    div()
        .col()
        .w_px(width)
        .child(workspace::status_bar(&layout(), None))
        .into()
}

fn row(index: usize, label: &str, branch: &str) -> WorkspaceRow {
    WorkspaceRow {
        index,
        label: label.into(),
        branch: branch.into(),
        agent: None,
        ticket: String::new(),
        ticket_category: String::new(),
        running: 0,
        state: String::new(),
        pr: None,
        missing: Vec::new(),
    }
}

fn rows() -> Vec<WorkspaceRow> {
    let main = WorkspaceRow {
        running: 3,
        ..row(0, "main", "main")
    };
    let login = WorkspaceRow {
        agent: Some(AgentDot::Thinking),
        ticket: "In Progress".into(),
        ticket_category: "indeterminate".into(),
        running: 2,
        pr: Some(PrSummary {
            count: 1,
            severity: PrSeverity::Ok,
            trouble: None,
        }),
        ..row(1, "Login page", "proj-101-login")
    };
    let review = WorkspaceRow {
        agent: Some(AgentDot::AwaitingInput),
        ticket: "In Review".into(),
        ticket_category: "indeterminate".into(),
        pr: Some(PrSummary {
            count: 1,
            severity: PrSeverity::Danger,
            trouble: Some(PrTrouble::ChecksFailed),
        }),
        ..row(2, "Billing export", "proj-102-export")
    };
    let sleeping = WorkspaceRow {
        state: "sleeping".into(),
        ..row(3, "fix-cache", "fix-cache")
    };
    vec![main, login, review, sleeping]
}

fn list(rows: &[WorkspaceRow], width: f32) -> WorkspaceList<'_> {
    WorkspaceList {
        rows,
        current: 1,
        ops: &[],
        expanded: &[],
        hovered: None,
        upkeep_done: false,
        width,
        manual: None,
        grouping: None,
    }
}

fn dock(width: f32, height: f32, content: impl Into<Node>) -> Node {
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .bg(ui::theme().panel_background)
        .child(content.into())
        .into()
}

fn rail(width: f32, height: f32) -> Node {
    let rows = rows();
    dock(
        width,
        height,
        workspace::workspace_rail(&list(&rows, workspace::RAIL_W), workspace::RAIL_W),
    )
}

fn workspaces(width: f32, height: f32) -> Node {
    let rows = rows();
    let mut panel = workspace::ProjectPanel::default();
    panel.sync(&list(&rows, width));
    dock(width, height, panel.render())
}

struct FileTab(&'static str, ui::MaterialIcon);

impl workspace::Item for FileTab {
    fn id(&self) -> Option<String> {
        Some(self.0.into())
    }

    fn title(&self) -> String {
        self.0.into()
    }

    fn icon(&self) -> Option<ui::MaterialIcon> {
        Some(self.1)
    }

    fn render(&mut self) -> Node {
        div().into()
    }
}

const PANE_IDS: PaneClickIds = PaneClickIds {
    tab_activate: 100,
    tab_close: 200,
    nav_back: 300,
    nav_forward: 301,
    search: 302,
};

fn pane_tab_bar(width: f32, height: f32) -> Node {
    let mut pane = Pane::new(1);
    pane.add_item(Box::new(FileTab("pom.yml", ui::MaterialIcon::Yaml)));
    pane.toggle_pin(0);
    pane.add_item(Box::new(FileTab("server.rs", ui::MaterialIcon::Rust)));
    pane.add_item(Box::new(FileTab("login.tsx", ui::MaterialIcon::React)));
    pane.add_preview_item(Box::new(FileTab("README.md", ui::MaterialIcon::Markdown)));
    pane.activate_user(2);
    let config = TabBarConfig {
        show_nav: true,
        buttons: [
            ui::IconKind::PanelRight,
            ui::IconKind::PanelBottom,
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
    };
    let (nav, buttons) = workspace::pane::tab_bar_edges(&config);
    let tab_h = workspace::pane::TAB_H;
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .child(
            div()
                .col()
                .h_px(tab_h)
                .child(workspace::pane::render_pane(
                    &pane, &config, PANE_IDS, width,
                ))
                .pin(
                    ui::Corner::TopLeft,
                    nav,
                    0.0,
                    (width - nav - buttons).max(0.0),
                    tab_h,
                    workspace::pane::render_tab_strip(&pane, PANE_IDS, None),
                ),
        )
        .child(div().col().flex(1.0).bg(ui::theme().editor_background))
        .into()
}

fn centered(width: f32, height: f32, node: Node, node_w: f32) -> Node {
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .items_center()
        .justify_center()
        .child(
            div()
                .row()
                .justify_center()
                .child(div().w_px(node_w).child(node)),
        )
        .into()
}

fn toast(width: f32, height: f32) -> Node {
    let (node, node_w, _) =
        workspace::toast_preview("Pulled 3 commits into Login page", Some("Open workspace"));
    centered(width, height, node, node_w)
}

fn notification(which: &str, width: f32, height: f32) -> Node {
    match workspace::notification_preview(which) {
        Some((node, node_w)) => centered(width, height, node, node_w),
        None => div().into(),
    }
}

fn update_ready(width: f32, height: f32) -> Node {
    notification("update-ready", width, height)
}

fn invalid_config(width: f32, height: f32) -> Node {
    notification("invalid-config", width, height)
}

fn checks(docker: bool) -> Vec<workspace::MachineCheck> {
    vec![
        workspace::MachineCheck {
            name: "Docker".into(),
            ok: docker,
            detail: if docker {
                "Running".into()
            } else {
                "Docker is not running. Shared services and databases start once it is.".into()
            },
            fix: (!docker).then(|| "Start Docker".to_string()),
        },
        workspace::MachineCheck {
            name: "git".into(),
            ok: true,
            detail: "git version 2.50.1".into(),
            fix: None,
        },
    ]
}

fn welcome_on(width: f32, height: f32, sessions: &[Session], docker: bool) -> Node {
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .bg(ui::theme().editor_background)
        .child(workspace::welcome_page(
            sessions,
            &checks(docker),
            width,
            None,
        ))
        .into()
}

fn welcome(width: f32, height: f32) -> Node {
    welcome_on(width, height, &sessions(), true)
}

fn welcome_first(width: f32, height: f32) -> Node {
    welcome_on(width, height, &[], false)
}

fn prompt_on(prompt: workspace::Prompt, width: f32, height: f32) -> Node {
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .bg(ui::theme().editor_background)
        .painted(workspace::prompt_preview(&prompt, width, height))
        .into()
}

fn prompt_delete(width: f32, height: f32) -> Node {
    prompt_on(
        workspace::Prompt {
            token: 1,
            message: "Delete Workspace".into(),
            detail: Some(workspace::delete_workspace_detail(
                "Login page",
                "proj-101-login",
            )),
            buttons: vec!["Delete".into(), "Cancel".into()],
            warning: workspace::unpushed_warning(&[("web".into(), 2)]),
        },
        width,
        height,
    )
}

fn prompt_save(width: f32, height: f32) -> Node {
    prompt_on(
        workspace::Prompt {
            token: 2,
            message: "Do you want to save changes to the following files?".into(),
            detail: Some("src/server.rs\nweb/login.tsx".into()),
            buttons: vec!["Save all".into(), "Discard all".into(), "Cancel".into()],
            warning: None,
        },
        width,
        height,
    )
}

fn tab_switcher(width: f32, height: f32) -> Node {
    let entry =
        |index: usize, title: &str, material, preview| workspace::pane_group_view::TabEntry {
            index,
            id: Some(title.into()),
            title: title.into(),
            preview,
            icon: None,
            material,
        };
    let entries = [
        entry(0, "login.tsx", ui::MaterialIcon::React, false),
        entry(1, "server.rs", ui::MaterialIcon::Rust, false),
        entry(2, "README.md", ui::MaterialIcon::Markdown, true),
        entry(3, "pom.yml", ui::MaterialIcon::Yaml, false),
    ];
    let mut terminal = entry(4, "zsh", ui::MaterialIcon::Console, false);
    terminal.icon = Some(ui::IconKind::Terminal);
    let entries: Vec<_> = entries.into_iter().chain([terminal]).collect();
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .justify_center()
        .child(
            div()
                .row()
                .justify_center()
                .child(workspace::tab_switcher_preview(&entries, Some(1), None)),
        )
        .into()
}

fn workspace_switcher(width: f32, height: f32) -> Node {
    let rows = rows();
    let entries = workspace::switcher_order(&rows, None)
        .into_iter()
        .map(|(section, row)| workspace::SwitcherEntry {
            row: row.clone(),
            section,
            id: 500 + row.index as u64,
        })
        .collect();
    let switcher = workspace::WorkspaceSwitcher {
        entries,
        current: 1,
        new_workspace: workspace::ExtraCommand {
            name: "New workspace".into(),
            keys: vec!["cmd-n".into()],
            id: 1,
        },
    };
    div()
        .col()
        .w_px(width)
        .h_px(height)
        .items_center()
        .pt(ui::Space::S24)
        .child(
            div()
                .row()
                .justify_center()
                .child(files_ui::workspace_switcher_preview(switcher, "")),
        )
        .into()
}

const BAR: (f32, f32) = (900.0, 40.0);
const STATUS: (f32, f32) = (900.0, 28.0);
const FLOAT: (f32, f32) = (560.0, 160.0);

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
            "chrome-title-bar",
            "Title bar",
            "Local project",
            BAR,
            || still(title_bar),
        ),
        story(
            "chrome-title-bar-remote",
            "Title bar",
            "Project on a host",
            BAR,
            || still(title_bar_remote),
        ),
        story(
            "chrome-title-bar-waiting",
            "Title bar",
            "A workspace needs you",
            BAR,
            || still(title_bar_waiting),
        ),
        story(
            "chrome-title-bar-update",
            "Title bar",
            "Update ready",
            BAR,
            || still(title_bar_update),
        ),
        story(
            "chrome-title-bar-linux",
            "Title bar",
            "Window controls on Linux",
            BAR,
            || still(title_bar_linux),
        ),
        story(
            "chrome-session-menu",
            "Session menu",
            "Open",
            (520.0, 480.0),
            || still(session_menu),
        ),
        story(
            "chrome-status-bar",
            "Status bar",
            "Workspace",
            STATUS,
            || still(status_bar),
        ),
        story(
            "chrome-rail",
            "Workspace rail",
            "Collapsed dock",
            (56.0, 360.0),
            || still(rail),
        ),
        story(
            "chrome-workspaces",
            "Workspace rail",
            "Workspaces list",
            (280.0, 360.0),
            || still(workspaces),
        ),
        story(
            "chrome-pane-tab-bar",
            "Pane tab bar",
            "Pinned, active and preview tabs",
            (720.0, 120.0),
            || still(pane_tab_bar),
        ),
        story("chrome-toast", "Toasts", "With an action", FLOAT, || {
            still(toast)
        }),
        story(
            "chrome-notification-update",
            "Toasts",
            "Update ready",
            (560.0, 220.0),
            || still(update_ready),
        ),
        story(
            "chrome-notification-config",
            "Toasts",
            "Invalid config",
            (560.0, 240.0),
            || still(invalid_config),
        ),
        story(
            "chrome-welcome",
            "Welcome",
            "Recent projects",
            (900.0, 720.0),
            || still(welcome),
        ),
        story(
            "chrome-welcome-first",
            "Welcome",
            "First run, Docker stopped",
            (900.0, 720.0),
            || still(welcome_first),
        ),
        story(
            "chrome-prompt-delete",
            "Prompt",
            "Delete with unpushed commits",
            (720.0, 420.0),
            || still(prompt_delete),
        ),
        story(
            "chrome-prompt-save",
            "Prompt",
            "Save changes",
            (720.0, 360.0),
            || still(prompt_save),
        ),
        story(
            "chrome-tab-switcher",
            "Tab switcher",
            "Second tab highlighted",
            (560.0, 260.0),
            || still(tab_switcher),
        ),
        story(
            "chrome-workspace-switcher",
            "Workspace switcher",
            "Open",
            (720.0, 440.0),
            || still(workspace_switcher),
        ),
    ]
}
