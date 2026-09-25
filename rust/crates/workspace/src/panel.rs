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
    /// Open a file as a diff against `base` (its earlier text; `None` when it is new).
    OpenDiff {
        path: std::path::PathBuf,
        base: Option<String>,
    },
    Reveal {
        id: String,
        open: Box<dyn FnOnce() -> Option<Box<dyn crate::Item>>>,
    },
    OpenUrl(String),
    Copy(String),
    Toast(String),
    ToastAction {
        message: String,
        action: String,
        then: Box<PanelRequest>,
    },
    OpenMenu,
    /// Run a command in a new terminal tab (`argv` spawned in `cwd`), titled `title`.
    RunCommand {
        title: String,
        cwd: std::path::PathBuf,
        argv: Vec<String>,
    },
    /// Ask before acting; the answer comes back through `prompt_answered` with the same `tag`.
    Prompt {
        tag: u64,
        message: String,
        detail: Option<String>,
        buttons: Vec<String>,
    },
}

/// A command a side panel offers in the command palette; `id` comes back through `run_palette_entry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    pub label: String,
    pub id: u64,
}

pub trait SidePanelView: 'static {
    fn kind(&self) -> PaneKind;
    fn palette_entries(&self) -> Vec<PaletteEntry> {
        Vec::new()
    }
    fn run_palette_entry(&mut self, _id: u64) {}
    fn render(&mut self, width: f32, height: f32) -> Node;
    fn click(&mut self, id: u64);
    fn click_at(&mut self, id: u64, _x: f32, _y: f32) {
        self.click(id);
    }
    fn set_hover(&mut self, id: Option<u64>) -> bool;
    fn scroll(&mut self, dy: f32) -> bool;
    fn open_menu(&mut self, id: u64) -> bool;
    fn menu_items(&self) -> Vec<crate::MenuItem>;
    fn menu_action(&mut self, item: u64);
    fn take_requests(&mut self) -> Vec<PanelRequest>;
    /// The button (index into the prompt's buttons) picked for the prompt asked with `tag`.
    fn prompt_answered(&mut self, _tag: u64, _answer: usize) {}
    fn text_focused(&self) -> bool {
        false
    }
    fn text(&mut self, _text: &str) -> bool {
        false
    }
    fn key(&mut self, _key: crate::EditKey, _shift: bool) -> bool {
        false
    }
    fn blur(&mut self) {}
    fn restore_item(
        &mut self,
        _item: &crate::persistence::SerializedItem,
    ) -> Option<Box<dyn crate::Item>> {
        None
    }
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

    pub fn label(self) -> &'static str {
        match self {
            AgentDot::Idle => "Idle",
            AgentDot::Thinking => "Thinking",
            AgentDot::ToolUse => "Using tools",
            AgentDot::Compacting => "Compacting",
            AgentDot::AwaitingInput => "Awaiting input",
        }
    }
}

/// A row of the WORKSPACES list: its index in `ProjectInfo::workspaces`, what it is called and its agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub index: usize,
    pub label: String,
    pub branch: String,
    pub agent: Option<AgentDot>,
    /// The workspace's Jira ticket status, when it has one.
    pub ticket: String,
    /// That status's Jira category: `new`, `indeterminate` or `done`.
    pub ticket_category: String,
    /// How many of its services run.
    pub running: usize,
    pub pr: Option<crate::PrSummary>,
    /// Repos of the config main has no clone of (only main has any).
    pub missing: Vec<String>,
}

/// What the WORKSPACES panel shows: the workspaces, which one is current, and creations/deletions in flight.
pub struct WorkspaceList<'a> {
    pub rows: &'a [WorkspaceRow],
    pub current: usize,
    pub ops: &'a [crate::WorkspaceOp],
    pub expanded: &'a [u64],
}

/// A dockable piece of UI. Mirrors the framework's `Panel` (position + icon + render), trimmed to what we draw now.
pub trait Panel: 'static {
    fn position(&self) -> DockPosition;
    /// The icon shown on the collapsed icon rail / dock tab.
    fn icon(&self) -> IconKind;
    /// The panel's title (dock header).
    fn title(&self) -> &str;
    fn sync(&mut self, _list: &WorkspaceList<'_>) {}
    /// The panel body as an element tree, laid into the dock region by the caller.
    fn render(&mut self) -> Node;
}

#[derive(Default)]
pub struct ProjectPanel {
    rows: Vec<WorkspaceRow>,
    current: usize,
    ops: Vec<crate::WorkspaceOp>,
    expanded: Vec<u64>,
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

    fn sync(&mut self, list: &WorkspaceList<'_>) {
        self.rows = list.rows.to_vec();
        self.current = list.current;
        self.ops = list.ops.to_vec();
        self.expanded = list.expanded.to_vec();
    }

    fn render(&mut self) -> Node {
        let header = div()
            .row()
            .items_center()
            .justify_between()
            .child(panel_header(self.title()))
            .child(
                div()
                    .row()
                    .items_center()
                    .justify_center()
                    .w_px(22.0)
                    .h_px(22.0)
                    .rounded(4.0)
                    .on_click(crate::WORKSPACE_NEW)
                    .child(icon(IconKind::Plus).size(14.0).color(theme().icon_muted)),
            );
        let mut col = div().col().px(10.0).py(10.0).gap(2.0).child(header);
        for (position, op) in self.ops.iter().enumerate() {
            if op.quiet {
                if let Some(line) = quiet_op_line(op) {
                    col = col.child(line);
                }
                continue;
            }
            let expanded = self.expanded.contains(&op.id);
            col = col.child(op_row(op, position, expanded));
        }
        for row in &self.rows {
            col = col.child(workspace_row(row, row.index == self.current));
        }
        col.into()
    }
}

/// What a rail cell calls a workspace, as two short lines. A workspace with a Jira ticket shows its project
/// key over its number (`CRM` / `1299`); any other shows the start of its name over the last part of its
/// branch (`inv` / `5fwg` for `investigate-0917-5fwg`); main is just `main`.
pub fn rail_label(row: &WorkspaceRow) -> (String, String) {
    let branch = row.branch.as_str();
    let parts: Vec<&str> = branch
        .split(|c: char| matches!(c, '-' | '_' | '/') || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    let ticket_number = parts
        .get(1)
        .filter(|part| part.chars().all(|c| c.is_ascii_digit()));
    if let (false, Some(project), Some(number)) =
        (row.ticket.is_empty(), parts.first(), ticket_number)
    {
        return (
            project.to_uppercase().chars().take(4).collect(),
            number.to_string(),
        );
    }
    let name = if row.label.is_empty() {
        branch
    } else {
        &row.label
    };
    let word = name
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '/'))
        .find(|word| !word.is_empty())
        .unwrap_or(name);
    match parts.as_slice() {
        [] | [_] => (word.chars().take(5).collect(), String::new()),
        [.., last] => (
            word.chars().take(3).collect(),
            last.chars().take(5).collect(),
        ),
    }
}

/// The WORKSPACES panel folded to a rail: a new-workspace button, then one cell per workspace with its short
/// label and, under it, its agent and running-services dots. Cells click like the rows they stand for.
pub fn workspace_rail(list: &WorkspaceList<'_>, width: f32) -> Node {
    let colors = theme();
    let cell_w = (width - 8.0).max(24.0);
    let mut column = div()
        .col()
        .w_px(width)
        .py(8.0)
        .gap(4.0)
        .items_center()
        .child(
            div()
                .row()
                .items_center()
                .justify_center()
                .w_px(cell_w)
                .h_px(26.0)
                .rounded(4.0)
                .on_click(crate::WORKSPACE_NEW)
                .child(icon(IconKind::Plus).size(14.0).color(colors.icon_muted)),
        );
    for row in list.rows {
        column = column.child(rail_cell(row, row.index == list.current, cell_w));
    }
    column.into()
}

/// One workspace folded into a single badge: the ring is its agent (colored by what it is doing, faint when
/// none runs), the number inside is colored by its ticket status, and the strip under it counts running
/// services and carries a pull-request mark.
fn rail_cell(row: &WorkspaceRow, current: bool, cell_w: f32) -> Node {
    const BADGE: f32 = 32.0;
    let colors = theme();
    let (top, bottom) = rail_label(row);
    let (caption, name) = if bottom.is_empty() {
        (None, top)
    } else {
        (Some(top), bottom)
    };
    let name_color = if row.ticket.is_empty() {
        if current {
            colors.text
        } else {
            colors.text_muted
        }
    } else {
        match row.ticket_category.as_str() {
            "done" => colors.success,
            "indeterminate" => colors.text_accent,
            _ => colors.text_muted,
        }
    };
    let (ring_width, ring) = match row.agent {
        Some(agent) => (2.0, agent.color()),
        None => (1.0, colors.border_variant),
    };
    let mut badge = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(BADGE)
        .h_px(BADGE)
        .rounded(BADGE / 2.0)
        .border(ring_width, ring)
        .child(label(name).size(10.5).color(name_color).truncate());
    if current {
        badge = badge.bg(colors.element_selected);
    }
    let mut strip = div()
        .row()
        .h_px(8.0)
        .gap(2.0)
        .items_center()
        .justify_center();
    for _ in 0..row.running.min(4) {
        strip = strip.child(div().w_px(3.0).h_px(3.0).rounded(1.5).bg(colors.success));
    }
    if let Some(pr) = row.pr {
        strip = strip.child(
            icon(IconKind::PullRequest)
                .size(8.0)
                .color(pr.severity.color()),
        );
    }
    let mut cell = div()
        .col()
        .items_center()
        .gap(2.0)
        .w_px(cell_w)
        .py(3.0)
        .rounded(6.0)
        .on_click(crate::WORKSPACE_ROW_BASE + row.index as u64);
    if let Some(caption) = caption {
        cell = cell.child(
            div().row().justify_center().w_px(cell_w).child(
                label(caption)
                    .size(8.5)
                    .color(colors.text_placeholder)
                    .truncate(),
            ),
        );
    }
    cell.child(badge).child(strip).into()
}

/// One workspace: its agent (when one runs), name and pull requests, and below, when there is any, the
/// ticket status colored by its category and how many services run.
fn workspace_row(row: &WorkspaceRow, current: bool) -> Node {
    let colors = theme();
    let marker: Node = match row.agent {
        Some(agent) => div()
            .w_px(6.0)
            .h_px(6.0)
            .rounded(3.0)
            .bg(agent.color())
            .into(),
        None => div().w_px(6.0).h_px(6.0).into(),
    };
    let mut first = div()
        .row()
        .h_px(20.0)
        .gap(8.0)
        .items_center()
        .child(marker)
        .child(div().row().flex(1.0).items_center().child(
            label(row.label.clone()).truncate().color(if current {
                colors.text
            } else {
                colors.text_muted
            }),
        ));
    if let Some(pr) = row.pr {
        first = first.child(pr_pill(row.index, pr));
    }
    let mut details = div().row().h_px(16.0).gap(10.0).items_center().pl(14.0);
    let mut has_details = false;
    if !row.missing.is_empty() {
        details = details.child(
            div().row().flex(1.0).items_center().child(
                label(format!("not cloned: {}", row.missing.join(", ")))
                    .size(11.0)
                    .color(colors.warning)
                    .truncate(),
            ),
        );
        has_details = true;
    }
    if let Some(agent) = row.agent {
        let color = match agent {
            AgentDot::Idle => colors.text_muted,
            _ => agent.color(),
        };
        details = details.child(label(agent.label()).size(11.0).color(color));
        has_details = true;
    }
    if row.running > 0 {
        details = details.child(
            div()
                .row()
                .gap(4.0)
                .items_center()
                .child(div().w_px(5.0).h_px(5.0).rounded(2.5).bg(colors.success))
                .child(
                    label(format!("{} running", row.running))
                        .size(11.0)
                        .color(colors.text_muted),
                ),
        );
        has_details = true;
    }
    if !row.ticket.is_empty() {
        let color = match row.ticket_category.as_str() {
            "done" => colors.success,
            "indeterminate" => colors.text_accent,
            _ => colors.text_muted,
        };
        details = details.child(
            div()
                .row()
                .flex(1.0)
                .items_center()
                .on_click(crate::WORKSPACE_TICKET_BASE + row.index as u64)
                .child(label(row.ticket.clone()).size(11.0).color(color).truncate()),
        );
        has_details = true;
    }
    let mut item = div()
        .col()
        .px(6.0)
        .py(4.0)
        .rounded(4.0)
        .on_click(crate::WORKSPACE_ROW_BASE + row.index as u64)
        .child(first);
    if has_details {
        item = item.child(details);
    }
    if current {
        item = item.bg(colors.element_selected);
    }
    item.into()
}

/// The row lifted off the list while it is dragged: the same row on an elevated card.
pub fn workspace_row_ghost(row: &WorkspaceRow) -> Node {
    div()
        .col()
        .rounded(6.0)
        .bg(theme().elevated_surface_background)
        .border(1.0, theme().border)
        .child(workspace_row(row, true))
        .into()
}

fn pr_pill(index: usize, pr: crate::PrSummary) -> Node {
    let color = pr.severity.color();
    div()
        .row()
        .h_px(18.0)
        .px(5.0)
        .gap(3.0)
        .items_center()
        .rounded(9.0)
        .bg(Rgba::new(color.r, color.g, color.b, 0.16))
        .on_click(crate::WORKSPACE_PR_BASE + index as u64)
        .child(icon(IconKind::PullRequest).size(11.0).color(color))
        .child(label(pr.count.to_string()).size(11.0).color(color))
        .into()
}

/// A creation or deletion in flight: its title, the stage it is on and a progress bar; expanded, every stage.
/// A failed one shows its error with Retry (when it can resume) and a dismiss button.
/// Background upkeep in flight as a single muted line: what it is and the step it is on.
fn quiet_op_line(op: &crate::WorkspaceOp) -> Option<Node> {
    use crate::{OpStatus, StageState};
    if !matches!(op.status, OpStatus::Queued | OpStatus::Running) {
        return None;
    }
    let colors = theme();
    let step = op
        .stages
        .iter()
        .find(|(_, state)| *state == StageState::Running)
        .map(|(name, _)| name.clone())
        .filter(|name| !name.is_empty());
    let text = match step {
        Some(step) => format!("{} - {step}", op.title),
        None => op.title.clone(),
    };
    Some(
        div()
            .row()
            .items_center()
            .gap(6.0)
            .h_px(20.0)
            .px(4.0)
            .child(icon(IconKind::RotateCw).size(11.0).color(colors.icon_muted))
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .items_center()
                    .child(label(text).size(11.0).color(colors.text_muted).truncate()),
            )
            .into(),
    )
}

fn op_row(op: &crate::WorkspaceOp, position: usize, expanded: bool) -> Node {
    use crate::{OpStatus, StageState};
    let colors = theme();
    let base = crate::WORKSPACE_OP_BASE + position as u64 * crate::WORKSPACE_OP_STRIDE;
    let (status_icon, status_color) = match op.status {
        OpStatus::Queued => (IconKind::ChevronRight, colors.icon_muted),
        OpStatus::Running => (IconKind::RotateCw, colors.icon_muted),
        OpStatus::Done => (IconKind::Check, colors.success),
        OpStatus::Failed => (IconKind::Warning, colors.error),
    };
    let subtitle = match op.status {
        OpStatus::Queued => "queued".to_string(),
        OpStatus::Done => "done".to_string(),
        OpStatus::Failed => op.error.clone(),
        OpStatus::Running => op
            .stages
            .iter()
            .find(|(_, state)| *state == StageState::Running)
            .map_or_else(|| "starting".to_string(), |(name, _)| name.clone()),
    };
    let mut title_row = div()
        .row()
        .items_center()
        .gap(6.0)
        .child(icon(status_icon).size(12.0).color(status_color))
        .child(
            div().row().flex(1.0).items_center().child(
                label(op.title.clone())
                    .size(13.0)
                    .color(colors.text)
                    .truncate(),
            ),
        );
    if op.status == OpStatus::Failed {
        if op.retryable {
            title_row = title_row.child(
                div()
                    .row()
                    .items_center()
                    .h_px(18.0)
                    .px(4.0)
                    .rounded(4.0)
                    .border(1.0, colors.border_variant)
                    .on_click(base + crate::WORKSPACE_OP_RETRY)
                    .child(label("Retry").size(12.0).color(colors.text)),
            );
        }
        title_row = title_row.child(
            div()
                .row()
                .items_center()
                .justify_center()
                .w_px(18.0)
                .h_px(18.0)
                .rounded(4.0)
                .on_click(base + crate::WORKSPACE_OP_DISMISS)
                .child(icon(IconKind::Close).size(12.0).color(colors.icon_muted)),
        );
    }
    let subtitle_color = if op.status == OpStatus::Failed {
        colors.error
    } else {
        colors.text_muted
    };
    let mut card = div()
        .col()
        .gap(4.0)
        .p(6.0)
        .rounded(4.0)
        .border(1.0, colors.border_variant)
        .on_click(base + crate::WORKSPACE_OP_TOGGLE)
        .child(title_row)
        .child(
            div()
                .row()
                .child(label(subtitle).size(12.0).color(subtitle_color).truncate()),
        );
    if op.status == OpStatus::Running || op.status == OpStatus::Queued {
        let total = op.stages.len().max(1) as f32;
        let finished = op
            .stages
            .iter()
            .filter(|(_, state)| matches!(state, StageState::Done | StageState::Skipped))
            .count() as f32;
        card = card.child(crate::form::progress_bar(finished / total));
    }
    if expanded {
        for (name, state) in &op.stages {
            let (kind, color) = match state {
                StageState::Pending => (IconKind::ChevronRight, colors.text_disabled),
                StageState::Running => (IconKind::RotateCw, colors.icon_muted),
                StageState::Done => (IconKind::Check, colors.success),
                StageState::Skipped => (IconKind::SquareMinus, colors.text_disabled),
                StageState::Failed => (IconKind::XCircle, colors.error),
            };
            card = card.child(
                div()
                    .row()
                    .items_center()
                    .gap(6.0)
                    .child(icon(kind).size(12.0).color(color))
                    .child(
                        label(name.clone())
                            .size(12.0)
                            .color(colors.text_muted)
                            .truncate(),
                    ),
            );
            if *state == StageState::Running && !op.detail.is_empty() {
                card = card.child(
                    div().row().pl(18.0).child(
                        label(op.detail.clone())
                            .size(11.0)
                            .mono()
                            .color(colors.text_muted)
                            .truncate(),
                    ),
                );
            }
        }
    }
    card.into()
}

/// What the agent dock shows while no agent session runs in the workspace: a way to start one.
#[derive(Default)]
pub struct AgentEmptyPanel;

impl Panel for AgentEmptyPanel {
    fn position(&self) -> DockPosition {
        DockPosition::Right
    }

    fn icon(&self) -> IconKind {
        IconKind::Monitor
    }

    fn title(&self) -> &str {
        "AGENT"
    }

    fn render(&mut self) -> Node {
        div()
            .col()
            .px(10.0)
            .py(10.0)
            .gap(8.0)
            .child(panel_header(self.title()))
            .child(
                label("No agent running in this workspace.")
                    .size(13.0)
                    .color(theme().text_muted),
            )
            .child(div().row().child(crate::outlined_button(
                crate::AGENT_TOGGLE,
                Some(IconKind::Sparkle),
                "Start Agent",
                true,
            )))
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

    fn row(index: usize, label: &str, agent: Option<AgentDot>) -> WorkspaceRow {
        WorkspaceRow {
            index,
            label: label.into(),
            branch: label.into(),
            agent,
            ticket: String::new(),
            ticket_category: String::new(),
            running: 0,
            pr: None,
            missing: Vec::new(),
        }
    }

    fn list<'a>(rows: &'a [WorkspaceRow], ops: &'a [crate::WorkspaceOp]) -> WorkspaceList<'a> {
        WorkspaceList {
            rows,
            current: 0,
            ops,
            expanded: &[],
        }
    }

    #[test]
    fn background_upkeep_is_one_quiet_line_and_hides_when_it_ends() {
        let mut op = crate::WorkspaceOp {
            id: 3,
            branch: String::new(),
            title: "Updating main".into(),
            status: crate::OpStatus::Running,
            stages: vec![("api".into(), crate::StageState::Running)],
            detail: String::new(),
            error: String::new(),
            retryable: true,
            quiet: true,
        };
        let text_of = |op: &crate::WorkspaceOp| {
            let mut p = ProjectPanel::default();
            p.sync(&list(&[], std::slice::from_ref(op)));
            let painted = ui::render(
                &p.render(),
                ui::Rect::new(0.0, 0.0, 240.0, 600.0, ui::Rgba::TRANSPARENT),
            );
            let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
            let text: Vec<String> = painted.texts.iter().map(|t| t.text.clone()).collect();
            (text, ids)
        };
        let (text, ids) = text_of(&op);
        assert!(text.iter().any(|t| t == "Updating main - api"), "{text:?}");
        assert!(
            !ids.iter().any(|id| *id >= crate::WORKSPACE_OP_BASE
                && *id < crate::WORKSPACE_OP_BASE + crate::WORKSPACE_OP_STRIDE),
            "no card controls"
        );
        op.status = crate::OpStatus::Failed;
        op.error = "migrate failed".into();
        let (text, _) = text_of(&op);
        assert!(
            !text.iter().any(|t| t.contains("Updating main")),
            "a failure goes to a toast"
        );
    }

    #[test]
    fn a_failed_creation_offers_retry_and_dismiss() {
        let op = crate::WorkspaceOp {
            id: 7,
            branch: "feat-x".into(),
            title: "feat-x".into(),
            status: crate::OpStatus::Failed,
            stages: vec![
                ("Validating".into(), crate::StageState::Done),
                ("Creating git worktrees".into(), crate::StageState::Failed),
            ],
            detail: String::new(),
            error: "web: git worktree add failed".into(),
            retryable: true,
            quiet: false,
        };
        let mut p = ProjectPanel::default();
        p.sync(&list(&[], std::slice::from_ref(&op)));
        let painted = ui::render(
            &p.render(),
            ui::Rect::new(0.0, 0.0, 240.0, 600.0, ui::Rgba::TRANSPARENT),
        );
        let text: String = painted.texts.iter().map(|t| t.text.clone()).collect();
        assert!(text.contains("feat-x") && text.contains("Retry"), "{text}");
        let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
        for part in [
            crate::WORKSPACE_OP_RETRY,
            crate::WORKSPACE_OP_DISMISS,
            crate::WORKSPACE_NEW,
        ] {
            let id = if part == crate::WORKSPACE_NEW {
                part
            } else {
                crate::WORKSPACE_OP_BASE + part
            };
            assert!(ids.contains(&id), "{id} in {ids:?}");
        }
    }

    #[test]
    fn project_panel_lists_workspaces() {
        let mut p = ProjectPanel::default();
        let rows = [row(0, "api", Some(AgentDot::Thinking)), row(1, "web", None)];
        p.sync(&list(&rows, &[]));
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
    fn a_workspace_with_prs_shows_a_pill_that_opens_its_git_panel() {
        let mut p = ProjectPanel::default();
        let mut with_prs = row(1, "web", None);
        with_prs.pr = Some(crate::PrSummary {
            count: 3,
            severity: crate::PrSeverity::Danger,
        });
        let rows = [row(0, "api", None), with_prs];
        p.sync(&list(&rows, &[]));
        let painted = ui::render(
            &p.render(),
            ui::Rect::new(0.0, 0.0, 240.0, 600.0, ui::Rgba::TRANSPARENT),
        );
        let text: Vec<String> = painted.texts.iter().map(|t| t.text.clone()).collect();
        assert!(text.contains(&"3".to_string()), "{text:?}");
        let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
        assert!(ids.contains(&(crate::WORKSPACE_PR_BASE + 1)));
        assert!(!ids.contains(&crate::WORKSPACE_PR_BASE));
    }

    #[test]
    fn the_rail_names_workspaces_by_ticket_number_and_clicks_like_rows() {
        let mut ticket = row(1, "Login page", Some(AgentDot::Thinking));
        ticket.branch = "proj-101-login".into();
        ticket.ticket = "In Progress".into();
        ticket.running = 2;
        let mut plain = row(2, "", None);
        plain.branch = "investigate-0917-5fwg".into();
        let mut untracked = row(3, "", None);
        untracked.branch = "proj-102-no-status".into();
        assert_eq!(rail_label(&ticket), ("PROJ".into(), "101".into()));
        assert_eq!(rail_label(&plain), ("inv".into(), "5fwg".into()));
        assert_eq!(rail_label(&untracked), ("pro".into(), "statu".into()));
        assert_eq!(
            rail_label(&row(0, "main", None)),
            ("main".into(), String::new())
        );
        let rows = [row(0, "main", None), ticket, plain];
        let painted = ui::render(
            &workspace_rail(&list(&rows, &[]), crate::RAIL_W),
            ui::Rect::new(0.0, 0.0, crate::RAIL_W, 600.0, ui::Rgba::TRANSPARENT),
        );
        let texts: Vec<String> = painted.texts.iter().map(|t| t.text.clone()).collect();
        assert!(texts.contains(&"101".to_string()), "{texts:?}");
        assert!(texts.contains(&"5fwg".to_string()), "{texts:?}");
        let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
        assert!(ids.contains(&crate::WORKSPACE_NEW));
        assert!(ids.contains(&(crate::WORKSPACE_ROW_BASE + 2)));
        assert!(painted
            .rects
            .iter()
            .all(|rect| rect.x + rect.w <= crate::RAIL_W + 0.5));
    }

    #[test]
    fn a_ticket_status_opens_the_ticket() {
        let mut p = ProjectPanel::default();
        let mut with_ticket = row(1, "web", None);
        with_ticket.ticket = "In Progress".into();
        with_ticket.ticket_category = "indeterminate".into();
        let rows = [row(0, "api", None), with_ticket];
        p.sync(&list(&rows, &[]));
        let painted = ui::render(
            &p.render(),
            ui::Rect::new(0.0, 0.0, 240.0, 600.0, ui::Rgba::TRANSPARENT),
        );
        let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
        assert!(ids.contains(&(crate::WORKSPACE_TICKET_BASE + 1)));
        assert!(!ids.contains(&crate::WORKSPACE_TICKET_BASE));
    }

    #[test]
    fn long_workspace_names_stay_inside_the_panel() {
        let mut p = ProjectPanel::default();
        let long = "proj-101-a-very-long-branch-name-that-does-not-fit-in-the-dock".to_string();
        let rows = [row(0, "main", None), row(1, &long, None)];
        p.sync(&list(&rows, &[]));
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
