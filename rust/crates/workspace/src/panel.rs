//! Docked panels, reactive-entity style but adapted to our element tree. A `Panel` is a piece of dockable UI that knows
//! its dock side + icon and renders its body as an element-tree `Node`; the `Dock` geometry (width/collapsed)
//! stays in `workspace`. This is the first slice of the structure-first workspace migration: the left dock's
//! interior (the workspace list) now renders through `ProjectPanel` instead of hand-placed rects.

use ui::{div, icon, label, theme, IconKind, Node, Rgba};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DockPosition {
    Left,
    Bottom,
    Right,
}

impl DockPosition {
    pub fn index(self) -> usize {
        match self {
            DockPosition::Left => 0,
            DockPosition::Bottom => 1,
            DockPosition::Right => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DockPosition::Left => "left",
            DockPosition::Bottom => "bottom",
            DockPosition::Right => "right",
        }
    }

    /// Parse a persisted side string, defaulting to `Left` for unknown values.
    pub fn from_side(s: &str) -> DockPosition {
        match s {
            "right" => DockPosition::Right,
            "bottom" => DockPosition::Bottom,
            _ => DockPosition::Left,
        }
    }
}

/// The functions available inside a workspace (Pomelo's `PaneKind`, minus the agent which is the right dock).
/// Selected via the function rail (the second left panel); the center shows the active one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneKind {
    Files,
    Services,
    Git,
    Jira,
    Database,
    Review,
}

impl PaneKind {
    /// The functions shown in the bottom bar. Review is temporarily hidden.
    pub const ALL: [PaneKind; 4] = [
        PaneKind::Files,
        PaneKind::Services,
        PaneKind::Git,
        PaneKind::Database,
    ];

    pub fn index(self) -> usize {
        match self {
            PaneKind::Files => 0,
            PaneKind::Services => 1,
            PaneKind::Git => 2,
            PaneKind::Jira => 3,
            PaneKind::Database => 4,
            PaneKind::Review => 5,
        }
    }

    pub fn from_index(index: usize) -> Option<PaneKind> {
        [
            PaneKind::Files,
            PaneKind::Services,
            PaneKind::Git,
            PaneKind::Jira,
            PaneKind::Database,
            PaneKind::Review,
        ]
        .get(index)
        .copied()
    }

    pub fn icon(self) -> IconKind {
        match self {
            PaneKind::Files => IconKind::Folder,
            PaneKind::Services => IconKind::Server,
            PaneKind::Git => IconKind::Branch,
            PaneKind::Jira => IconKind::Diamond,
            PaneKind::Database => IconKind::Cylinder,
            PaneKind::Review => IconKind::Search,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PaneKind::Files => "Files",
            PaneKind::Services => "Services",
            PaneKind::Git => "Git",
            PaneKind::Jira => "Jira",
            PaneKind::Database => "Database",
            PaneKind::Review => "Review",
        }
    }
}

/// The workspace function nav: a horizontal icon row for the bottom bar, one button per function whose dock
/// side matches `want` (and isn't hidden), the active one highlighted and the hovered one lit. Click id =
/// `base_id + function index`.
pub fn function_bar(
    highlighted: &[bool],
    base_id: u64,
    hovered: Option<u64>,
    hidden: &[bool],
    sides: &[DockPosition],
    want: DockPosition,
) -> Node {
    let mut row = div().row().gap(2.0).items_center();
    for (i, kind) in PaneKind::ALL.iter().enumerate() {
        if hidden.get(i).copied().unwrap_or(false) {
            continue;
        }
        if sides.get(i).copied().unwrap_or(DockPosition::Left) != want {
            continue;
        }
        let id = base_id + i as u64;
        let on = highlighted.get(i).copied().unwrap_or(false);
        let hot = hovered == Some(id);
        let cell = div()
            .w_px(26.0)
            .h_px(20.0)
            .rounded(5.0)
            .items_center()
            .justify_center()
            .on_click(id)
            .bg(if hot {
                theme().element_hover
            } else {
                Rgba::TRANSPARENT
            })
            .child(icon(kind.icon()).size(13.0).color(if on {
                theme().icon_accent
            } else {
                theme().icon_muted
            }));
        row = row.child(cell);
    }
    row.into()
}

fn function_rows(active: PaneKind) -> &'static [&'static str] {
    match active {
        PaneKind::Files => &["src/", "Cargo.toml", "README.md", "main.rs"],
        PaneKind::Services => &["api", "web", "worker", "gateway"],
        PaneKind::Git => &["#146 review comments", "#145 fix build", "#144 add tests"],
        PaneKind::Database => &["users", "sessions", "workspaces", "events"],
        PaneKind::Jira => &["PROJ-1 open", "PROJ-2 in progress"],
        PaneKind::Review => &["No review"],
    }
}

fn function_row_list(active: PaneKind) -> Node {
    let mut col = div().col().gap(4.0);
    for r in function_rows(active) {
        col = col.child(
            div()
                .row()
                .h_px(24.0)
                .px(4.0)
                .items_center()
                .child(label(r.to_string()).color(theme().text_muted)),
        );
    }
    col.into()
}

/// The active function's panel in the center (main) area: a plain title header plus its rows.
pub fn function_content(active: PaneKind) -> Node {
    div()
        .col()
        .bg(theme().editor_background)
        .px(10.0)
        .py(10.0)
        .gap(4.0)
        .child(label(active.title()).size(12.0).color(theme().text_muted))
        .child(function_row_list(active))
        .into()
}

/// The active function's panel when it lives in a side/bottom dock: a title header plus the function's rows.
pub fn function_dock_body(active: PaneKind) -> Node {
    div()
        .col()
        .px(10.0)
        .py(10.0)
        .gap(4.0)
        .child(panel_header(active.title()))
        .child(function_row_list(active))
        .into()
}

/// the header carries no collapse control.
pub fn panel_header(title: &str) -> Node {
    div()
        .row()
        .h_px(24.0)
        .items_center()
        .child(
            label(title.to_string())
                .size(12.0)
                .color(theme().text_muted),
        )
        .into()
}

pub const SIDE_PANEL_BASE: u64 = 800_000_000;
pub const SIDE_PANEL_SPAN: u64 = 100_000_000;

const SIDE_PANEL_SLICE: u64 = 10_000_000;

pub fn is_side_panel_id(id: u64) -> bool {
    (SIDE_PANEL_BASE..SIDE_PANEL_BASE + SIDE_PANEL_SPAN).contains(&id)
}

pub fn side_panel_base(kind: PaneKind) -> u64 {
    SIDE_PANEL_BASE + kind.index() as u64 * SIDE_PANEL_SLICE
}

pub fn side_panel_kind(id: u64) -> Option<PaneKind> {
    if !is_side_panel_id(id) {
        return None;
    }
    PaneKind::from_index(((id - SIDE_PANEL_BASE) / SIDE_PANEL_SLICE) as usize)
}

pub enum PanelRequest {
    OpenItem(Box<dyn crate::Item>),
    /// Open a file in the editor.
    OpenFile(std::path::PathBuf),
    Reveal {
        id: String,
        open: Box<dyn FnOnce() -> Option<Box<dyn crate::Item>>>,
    },
    OpenUrl(String),
    Copy(String),
    Toast(String),
    /// Ask before acting; the answer comes back through `prompt_answered` with the same `tag`.
    Prompt {
        tag: u64,
        message: String,
        detail: Option<String>,
        buttons: Vec<String>,
    },
}

pub trait SidePanelView: 'static {
    fn kind(&self) -> PaneKind;
    fn render(&mut self, width: f32, height: f32) -> Node;
    fn click(&mut self, id: u64);
    fn set_hover(&mut self, id: Option<u64>) -> bool;
    fn scroll(&mut self, dy: f32) -> bool;
    fn open_menu(&mut self, id: u64) -> bool;
    fn menu_items(&self) -> Vec<crate::MenuItem>;
    fn menu_action(&mut self, item: u64);
    fn take_requests(&mut self) -> Vec<PanelRequest>;
    /// The button (index into the prompt's buttons) picked for the prompt asked with `tag`.
    fn prompt_answered(&mut self, _tag: u64, _answer: usize) {}
}

/// What a workspace's coding agent is doing, as the dot on its row shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentDot {
    Idle,
    Thinking,
    ToolUse,
    Compacting,
    AwaitingInput,
}

impl AgentDot {
    fn color(self) -> Rgba {
        match self {
            AgentDot::Idle => theme().success,
            AgentDot::Thinking => theme().warning,
            AgentDot::ToolUse => theme().info,
            AgentDot::Compacting => theme().text_accent,
            AgentDot::AwaitingInput => theme().error,
        }
    }
}

/// A row of the WORKSPACES list: the branch and its agent, if one reported.
pub type WorkspaceRow = (String, Option<AgentDot>);

/// A dockable piece of UI. Mirrors the framework's `Panel` (position + icon + render), trimmed to what we draw now.
pub trait Panel: 'static {
    fn position(&self) -> DockPosition;
    /// The icon shown on the collapsed icon rail / dock tab.
    fn icon(&self) -> IconKind;
    /// The panel's title (dock header).
    fn title(&self) -> &str;
    fn sync(&mut self, _rows: &[WorkspaceRow], _current: usize) {}
    /// The panel body as an element tree, laid into the dock region by the caller.
    fn render(&mut self) -> Node;
}

#[derive(Default)]
pub struct ProjectPanel {
    rows: Vec<WorkspaceRow>,
    current: usize,
}

impl Panel for ProjectPanel {
    fn position(&self) -> DockPosition {
        DockPosition::Left
    }

    fn icon(&self) -> IconKind {
        IconKind::Folder
    }

    fn title(&self) -> &str {
        "WORKSPACES"
    }

    fn sync(&mut self, rows: &[WorkspaceRow], current: usize) {
        self.rows = rows.to_vec();
        self.current = current;
    }

    fn render(&mut self) -> Node {
        let mut col = div()
            .col()
            .px(10.0)
            .py(10.0)
            .gap(2.0)
            .child(panel_header(self.title()));
        for (i, (name, agent)) in self.rows.iter().enumerate() {
            let dot = agent.map_or(theme().icon_muted, AgentDot::color);
            let row = div()
                .row()
                .h_px(28.0)
                .px(4.0)
                .gap(8.0)
                .items_center()
                .rounded(4.0)
                .on_click(crate::WORKSPACE_ROW_BASE + i as u64)
                .bg(if i == self.current {
                    theme().element_selected
                } else {
                    Rgba::TRANSPARENT
                })
                .child(div().w_px(6.0).h_px(6.0).rounded(3.0).bg(dot))
                .child(label(name.clone()).truncate().color(if i == self.current {
                    theme().text
                } else {
                    theme().text_muted
                }));
            col = col.child(row);
        }
        col.into()
    }
}

/// The right dock's outline placeholder: a header plus a muted "nothing here yet" line. A stand-in until a real
/// outline/symbols panel lands; it exercises the right dock through the same `Panel`/element-tree path.
#[derive(Default)]
pub struct OutlinePanel;

impl Panel for OutlinePanel {
    fn position(&self) -> DockPosition {
        DockPosition::Right
    }

    fn icon(&self) -> IconKind {
        IconKind::Monitor
    }

    fn title(&self) -> &str {
        "OUTLINE"
    }

    fn render(&mut self) -> Node {
        div()
            .col()
            .px(10.0)
            .py(10.0)
            .gap(6.0)
            .child(panel_header(self.title()))
            .child(label("No symbols").size(13.0).color(theme().text_muted))
            .into()
    }
}

/// The bottom dock's terminal placeholder: a header plus a shell prompt line. Stand-in until a real terminal
/// lands; exercises the bottom dock through the same `Panel`/element-tree path.
#[derive(Default)]
pub struct TerminalPanel;

impl Panel for TerminalPanel {
    fn position(&self) -> DockPosition {
        DockPosition::Bottom
    }

    fn icon(&self) -> IconKind {
        IconKind::Monitor
    }

    fn title(&self) -> &str {
        "TERMINAL"
    }

    fn render(&mut self) -> Node {
        terminal_dock_body()
    }
}

/// The terminal panel body when docked (side/bottom): a title header plus a shell prompt line.
pub fn terminal_dock_body() -> Node {
    div()
        .col()
        .px(12.0)
        .py(8.0)
        .gap(4.0)
        .child(panel_header("TERMINAL"))
        .child(label("pomelo % ").size(13.0).mono().color(theme().text))
        .into()
}

/// The terminal panel body in the center (main) area: a plain title header (no dock collapse) plus the prompt.
pub fn terminal_content() -> Node {
    div()
        .col()
        .bg(theme().editor_background)
        .px(12.0)
        .py(8.0)
        .gap(4.0)
        .child(label("TERMINAL").size(12.0).color(theme().text_muted))
        .child(label("pomelo % ").size(13.0).mono().color(theme().text))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_panel_lists_workspaces() {
        let mut p = ProjectPanel::default();
        p.sync(
            &[
                ("api".into(), Some(AgentDot::Thinking)),
                ("web".into(), None),
            ],
            0,
        );
        assert_eq!(p.position(), DockPosition::Left);
        let node = p.render();
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, 240.0, 600.0, ui::Rgba::TRANSPARENT),
        );
        let text: String = painted.texts.iter().map(|t| t.text.clone()).collect();
        assert!(text.contains("WORKSPACES"));
        assert!(text.contains("api"));
        assert!(text.contains("web"));
        assert!(painted
            .hits
            .iter()
            .any(|(_, id)| *id == crate::WORKSPACE_ROW_BASE + 1));
    }

    #[test]
    fn long_workspace_names_stay_inside_the_panel() {
        let mut p = ProjectPanel::default();
        let long = "proj-101-a-very-long-branch-name-that-does-not-fit-in-the-dock".to_string();
        p.sync(&[("main".into(), None), (long.clone(), None)], 0);
        let painted = ui::render(
            &p.render(),
            ui::Rect::new(0.0, 0.0, 240.0, 600.0, ui::Rgba::TRANSPARENT),
        );
        let row = painted
            .texts
            .iter()
            .find(|t| t.text.starts_with("proj-101"))
            .expect("long row");
        assert!(row.text.ends_with("...") && row.text.len() < long.len());
        let width = ui::measure_text_width(&row.text, row.size, false, row.weight);
        assert!(row.x + width <= 240.0, "row text ends at {}", row.x + width);
    }
}
