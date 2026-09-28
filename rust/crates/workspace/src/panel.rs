//! Docked panels, reactive-entity style but adapted to our element tree. A `Panel` is a piece of dockable UI that knows
//! its dock side + icon and renders its body as an element-tree `Node`; the `Dock` geometry (width/collapsed)
//! stays in `workspace`. This is the first slice of the structure-first workspace migration: the left dock's
//! interior (the workspace list) now renders through `ProjectPanel` instead of hand-placed rects.

use ui::{div, icon, label, theme, Corner, Div, IconKind, Node, Rgba};

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
    /// Start the coding agent in `cwd` on `prompt`.
    FixWithAgent(AgentFix),
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
    /// Keep `repo` on `branch` in this workspace instead of the workspace's branch.
    KeepBranch {
        repo: String,
        branch: String,
    },
    /// Check the workspace's branch out again in `repo`.
    SwitchBranch {
        repo: String,
    },
    /// Choose another branch for `repo` in this workspace.
    PickBranch {
        repo: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentFix {
    pub prompt: String,
    pub cwd: std::path::PathBuf,
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
    /// The hit id under the pointer, for hover backgrounds.
    pub hovered: Option<u64>,
    /// Main's background update just succeeded; its row says so for a moment.
    pub upkeep_done: bool,
    /// The panel's width in layout units, for content that wraps.
    pub width: f32,
    /// The failed operation whose menu of manual fixes is open.
    pub manual: Option<u64>,
    /// Grouped by ticket status, in this arrangement; flat when `None`.
    pub grouping: Option<&'a crate::Grouping>,
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
    hovered: Option<u64>,
    upkeep_done: bool,
    width: f32,
    manual: Option<u64>,
    grouping: Option<crate::Grouping>,
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
        self.hovered = list.hovered;
        self.upkeep_done = list.upkeep_done;
        self.width = list.width;
        self.manual = list.manual;
        self.grouping = list.grouping.cloned();
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
        let upkeep = upkeep_of(&self.ops, &self.expanded, self.upkeep_done);
        // Panel padding and a row's own insets.
        let inner_w = (self.width - 20.0 - 26.0).max(120.0);
        let row_node = |row: &WorkspaceRow, upkeep: Upkeep<'_>| {
            let id = crate::WORKSPACE_ROW_BASE + row.index as u64;
            workspace_row(
                row,
                row.index == self.current,
                self.hovered == Some(id),
                upkeep,
                inner_w,
                self.manual,
            )
        };
        if let Some(main) = self.rows.iter().find(|row| row.index == 0) {
            col = col.child(row_node(main, upkeep));
            let mut ahead = 0;
            for (position, op) in self.ops.iter().enumerate().filter(|(_, op)| !op.quiet) {
                let expanded = self.expanded.contains(&op.id);
                let manual_open = self.manual == Some(op.id);
                col = col.child(op_row(op, position, expanded, ahead, inner_w, manual_open));
                if op.status != crate::OpStatus::Failed {
                    ahead += 1;
                }
            }
        }
        for section in sections(&self.rows, self.grouping.as_ref()) {
            col = col.child(match section {
                Section::Row(row) => row_node(row, Upkeep::None),
                Section::Header {
                    group,
                    count,
                    folded,
                } => group_header(group, count, folded, self.hovered),
            });
        }
        col.into()
    }
}

enum Section<'r> {
    Header {
        group: crate::TicketGroup,
        count: usize,
        folded: bool,
    },
    Row(&'r WorkspaceRow),
}

/// Every workspace but main, in the shared order: as is, or under a header per ticket status with a folded
/// group's rows left out. Groups nothing falls into get no header.
fn sections<'r>(rows: &'r [WorkspaceRow], grouping: Option<&crate::Grouping>) -> Vec<Section<'r>> {
    let rest = rows.iter().filter(|row| row.index != 0);
    let Some(grouping) = grouping else {
        return rest.map(Section::Row).collect();
    };
    let mut out = Vec::new();
    for group in &grouping.order {
        let members: Vec<&WorkspaceRow> = rest
            .clone()
            .filter(|row| crate::TicketGroup::of(&row.ticket, &row.ticket_category) == *group)
            .collect();
        if members.is_empty() {
            continue;
        }
        let folded = grouping.folded.contains(group);
        out.push(Section::Header {
            group: *group,
            count: members.len(),
            folded,
        });
        if !folded {
            out.extend(members.into_iter().map(Section::Row));
        }
    }
    out
}

fn group_header(
    group: crate::TicketGroup,
    count: usize,
    folded: bool,
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let id = crate::WORKSPACE_GROUP_BASE + group.index() as u64;
    let mut header = div()
        .row()
        .h_px(24.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .rounded(5.0)
        .on_click(id)
        .child(
            icon(if folded {
                IconKind::ChevronRight
            } else {
                IconKind::ChevronDown
            })
            .size(10.0)
            .color(colors.text_placeholder),
        )
        .child(
            label(group.label())
                .size(11.0)
                .color(colors.text_placeholder),
        )
        .child(div().row().flex(1.0))
        .child(
            label(count.to_string())
                .size(11.0)
                .color(colors.text_placeholder),
        );
    if hovered == Some(id) {
        header = header.bg(colors.element_hover);
    }
    div().col().pt(8.0).child(header).into()
}

/// A group on the rail: a rule in its colour either side of its count; dimmer and on a chip when folded.
fn group_marker(
    group: crate::TicketGroup,
    count: usize,
    folded: bool,
    cell_w: f32,
    hovered: bool,
) -> Node {
    let color = group.color();
    let rule_alpha = if folded { 0.35 } else { 0.7 };
    let rule = || {
        div()
            .row()
            .flex(1.0)
            .h_px(2.0)
            .rounded(1.0)
            .bg(with_alpha(color, rule_alpha))
    };
    let mut marker = div()
        .row()
        .w_px(36.0)
        .gap(4.0)
        .items_center()
        .child(rule())
        .child(label(count.to_string()).size(9.5).weight(600).color(color))
        .child(rule());
    // The padding is always there so hovering or folding only changes the fill, never the rail's layout.
    marker = marker.py(3.0).rounded(6.0);
    if hovered {
        marker = marker.bg(theme().element_hover);
    } else if folded {
        marker = marker.bg(with_alpha(theme().text, 0.04));
    }
    div()
        .row()
        .w_px(cell_w)
        .pt(4.0)
        .justify_center()
        .on_click(crate::WORKSPACE_GROUP_BASE + group.index() as u64)
        .child(marker)
        .into()
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
    let upkeep = upkeep_of(list.ops, list.expanded, list.upkeep_done);
    let cell = |row: &WorkspaceRow, upkeep: Upkeep<'_>| {
        let id = crate::WORKSPACE_ROW_BASE + row.index as u64;
        rail_cell(
            row,
            row.index == list.current,
            list.hovered == Some(id),
            cell_w,
            upkeep,
        )
    };
    if let Some(main) = list.rows.iter().find(|row| row.index == 0) {
        column = column.child(cell(main, upkeep));
        for (position, op) in list.ops.iter().enumerate().filter(|(_, op)| !op.quiet) {
            column = column.child(op_tile(op, position, cell_w));
        }
        column = column.child(div().w_px(20.0).h_px(1.0).bg(theme().border_variant));
    }
    for section in sections(list.rows, list.grouping) {
        column = column.child(match section {
            Section::Row(row) => cell(row, Upkeep::None),
            Section::Header {
                group,
                count,
                folded,
            } => group_marker(
                group,
                count,
                folded,
                cell_w,
                list.hovered == Some(crate::WORKSPACE_GROUP_BASE + group.index() as u64),
            ),
        });
    }
    column.into()
}

/// A workspace's ticket key, taken from its branch (`proj-101-login` -> `PROJ-101`) when it has a ticket.
pub fn ticket_key(row: &WorkspaceRow) -> Option<String> {
    if row.ticket.is_empty() {
        return None;
    }
    branch_key(&row.branch)
}

/// The row's name without a leading ticket key, which the second line already shows.
fn title(row: &WorkspaceRow, key: Option<&str>) -> String {
    match key {
        Some(key) => strip_key(&row.label, key),
        None => row.label.clone(),
    }
}

fn strip_key(name: &str, key: &str) -> String {
    let rest = name
        .get(key.len()..)
        .filter(|_| name.to_uppercase().starts_with(key));
    match rest.map(|rest| rest.trim_start_matches([' ', '-', ':', '_']).trim()) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => name.to_string(),
    }
}

fn branch_key(branch: &str) -> Option<String> {
    let mut parts = branch
        .split(['-', '_', '/'])
        .filter(|part| !part.is_empty());
    let project = parts.next()?;
    let number = parts
        .next()
        .filter(|part| part.chars().all(|c| c.is_ascii_digit()))?;
    Some(format!("{}-{number}", project.to_uppercase()))
}

/// A creation has no ticket yet, so its key counts only when its title leads with it.
fn op_ticket_key(op: &crate::WorkspaceOp) -> Option<String> {
    branch_key(&op.branch).filter(|key| op.title.to_uppercase().starts_with(key.as_str()))
}

fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba::new(color.r, color.g, color.b, alpha)
}

fn trouble_icon(trouble: crate::PrTrouble) -> IconKind {
    match trouble {
        crate::PrTrouble::Pending => IconKind::Clock,
        crate::PrTrouble::ChangesRequested => IconKind::Undo,
        crate::PrTrouble::ChecksFailed => IconKind::Close,
        crate::PrTrouble::Conflict => IconKind::Warning,
    }
}

fn trouble_badge(trouble: crate::PrTrouble) -> IconKind {
    match trouble {
        crate::PrTrouble::ChecksFailed => IconKind::BadgeClose,
        crate::PrTrouble::Conflict => IconKind::BadgeAlert,
        other => trouble_icon(other),
    }
}

/// A PR shown on the default branch says nothing (every PR targets it), so main never carries one.
fn shown_pr(row: &WorkspaceRow) -> Option<crate::PrSummary> {
    row.pr.filter(|_| row.index != 0)
}

fn category_color(category: &str) -> Rgba {
    let colors = theme();
    match category {
        "done" => colors.success,
        "indeterminate" => colors.text_accent,
        _ => colors.text_placeholder,
    }
}

/// One workspace folded into a tile that only raises what needs you: a ring while its agent works or waits,
/// a pill with the reason and count when its PRs are in trouble, a strip along the bottom while services run.
fn rail_cell(
    row: &WorkspaceRow,
    current: bool,
    hovered: bool,
    cell_w: f32,
    upkeep: Upkeep<'_>,
) -> Node {
    const TILE: f32 = 36.0;
    let colors = theme();
    let (top, bottom) = rail_label(row);
    let name = if bottom.is_empty() { top } else { bottom };
    let size = if name.chars().count() > 4 { 9.5 } else { 11.0 };
    let mut tile = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(TILE)
        .h_px(TILE)
        .rounded(8.0)
        .bg(if current {
            colors.element_selected
        } else if hovered {
            colors.element_hover
        } else {
            with_alpha(colors.text, 0.04)
        })
        .child(
            label(name)
                .size(size)
                .medium()
                .color(if current || hovered {
                    colors.text
                } else {
                    colors.text_muted
                }),
        );
    if let Some(agent) = row.agent.filter(|agent| *agent != AgentDot::Idle) {
        tile = tile.border(1.5, agent.color());
        if agent == AgentDot::AwaitingInput && !current {
            tile = tile.bg(with_alpha(agent.color(), 0.12));
        }
    }
    if row.running > 0 {
        tile = tile.pin(
            Corner::BottomLeft,
            8.0,
            -3.0,
            TILE - 16.0,
            2.0,
            div().rounded(1.0).bg(colors.success),
        );
    }
    if let Some((pr, trouble)) =
        shown_pr(row).and_then(|pr| pr.trouble.map(|trouble| (pr, trouble)))
    {
        tile = pin_pill(
            tile,
            Some(trouble_badge(trouble)),
            Some(pr.count.to_string()),
            trouble.color(),
        );
    }
    // Main never shows PRs, so its corner carries its background update instead.
    tile = match upkeep {
        Upkeep::Running(_) => pin_pill(tile, Some(IconKind::RotateCw), None, colors.text_accent),
        Upkeep::Done => pin_pill(tile, Some(IconKind::BadgeCheck), None, colors.success),
        Upkeep::Failed { position, .. } => pin_pill_with(
            tile,
            Some(IconKind::BadgeAlert),
            None,
            colors.error,
            Some(op_base(position) + crate::WORKSPACE_OP_POPOVER),
        ),
        Upkeep::None => tile,
    };
    let mut cell = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(cell_w)
        .h_px(TILE + 6.0)
        .on_click(crate::WORKSPACE_ROW_BASE + row.index as u64)
        .child(tile);
    if current {
        cell = cell.pin(
            Corner::TopLeft,
            0.0,
            12.0,
            3.0,
            TILE - 18.0,
            div().rounded(1.5).bg(colors.text_accent),
        );
    }
    cell.into()
}

/// Hangs a pill off a tile's top-right corner: a glyph, a count, or both, on `bg` with a ring of the panel colour
/// cutting it from the tile. A lone glyph makes a circle; a count stretches it into a capsule.
fn pin_pill(tile: Div, glyph: Option<IconKind>, text: Option<String>, bg: Rgba) -> Div {
    pin_pill_with(tile, glyph, text, bg, None)
}

fn pin_pill_with(
    tile: Div,
    glyph: Option<IconKind>,
    text: Option<String>,
    bg: Rgba,
    click: Option<u64>,
) -> Div {
    const INNER_H: f32 = 15.0;
    const GLYPH: f32 = 9.0;
    const RING: f32 = 2.0;
    let colors = theme();
    let ink = colors.editor_background;
    let mut pill = div()
        .row()
        .items_center()
        .justify_center()
        .gap(1.0)
        .rounded(INNER_H / 2.0 + RING)
        .bg(bg)
        .border(RING, colors.panel_background);
    let mut inner_w = 0.0;
    if let Some(glyph) = glyph {
        pill = pill.child(icon(glyph).size(GLYPH).color(ink));
        inner_w += GLYPH;
    }
    if let Some(text) = text {
        inner_w += ui::measure_text_width(&text, 9.5, false, 700)
            + if glyph.is_some() { 1.0 } else { 0.0 };
        pill = pill.child(label(text).size(9.5).weight(700).color(ink));
    }
    let inner_w = if glyph.is_some() && inner_w > GLYPH {
        inner_w + 3.0 + 4.0
    } else {
        INNER_H
    };
    if let Some(id) = click {
        pill = pill.on_click(id);
    }
    let (w, h) = (inner_w.max(INNER_H) + 2.0 * RING, INNER_H + 2.0 * RING);
    tile.pin(Corner::TopRight, 8.0 + RING, -6.0 - RING, w, h, pill)
}

/// A workspace being created, folded into an ordinary tile with a dim name: a progress strip while it runs, a
/// clock while it waits, a red corner when it failed.
fn op_tile(op: &crate::WorkspaceOp, position: usize, cell_w: f32) -> Node {
    use crate::OpStatus;
    const TILE: f32 = 36.0;
    let colors = theme();
    let mut parts = op
        .branch
        .split(['-', '_', '/'])
        .filter(|part| !part.is_empty());
    let number = parts
        .nth(1)
        .filter(|part| part.chars().all(|c| c.is_ascii_digit()));
    let name: String = match number {
        Some(number) => number.to_string(),
        None => op
            .title
            .chars()
            .filter(|c| !c.is_whitespace())
            .take(4)
            .collect(),
    };
    let mut tile = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(TILE)
        .h_px(TILE)
        .rounded(8.0)
        .bg(with_alpha(colors.text, 0.03))
        .child(
            label(name)
                .size(11.0)
                .medium()
                .color(colors.text_placeholder),
        );
    match op.status {
        OpStatus::Running => {
            let done = op_progress(op).clamp(0.02, 1.0);
            let strip = div()
                .row()
                .rounded(1.0)
                .bg(with_alpha(colors.text_accent, 0.25))
                .child(
                    div()
                        .h_px(2.0)
                        .flex(done)
                        .rounded(1.0)
                        .bg(colors.text_accent),
                )
                .child(div().h_px(2.0).flex(1.0 - done));
            tile = tile.pin(Corner::BottomLeft, 8.0, -3.0, TILE - 16.0, 2.0, strip);
        }
        OpStatus::Queued => {
            tile = pin_pill(tile, Some(IconKind::Clock), None, colors.text_placeholder);
        }
        OpStatus::Failed => {
            tile = pin_pill(tile, Some(IconKind::BadgeAlert), None, colors.error);
        }
        OpStatus::Done => {}
    }
    let mut cell = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(cell_w)
        .h_px(TILE + 6.0)
        .child(tile);
    // Only a failure has something to open; a running or queued tile is just there to be seen.
    if op.status == OpStatus::Failed {
        cell = cell.on_click(op_base(position) + crate::WORKSPACE_OP_POPOVER);
    }
    cell.into()
}

fn op_base(position: usize) -> u64 {
    crate::WORKSPACE_OP_BASE + position as u64 * crate::WORKSPACE_OP_STRIDE
}

/// A failure's details beside its rail tile, for when the list is folded away.
pub fn failure_popover(op: &crate::WorkspaceOp, position: usize, manual_open: bool) -> Node {
    const WIDTH: f32 = 290.0;
    let colors = theme();
    let title = if op.quiet {
        "main".to_string()
    } else {
        op.title.clone()
    };
    div()
        .col()
        .w_px(WIDTH)
        .p(10.0)
        .rounded(8.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border)
        .child(label(title).medium().color(colors.text).truncate())
        .child(div().row().h_px(16.0).items_center().child(status_line(
            IconKind::Warning,
            colors.error,
            op.error.clone(),
        )))
        .child(op_details(
            op,
            position,
            WIDTH - 20.0,
            !op.quiet,
            manual_open,
        ))
        .into()
}

/// One workspace in the list. The name has the first line to itself (the PR count at its end); the second line
/// carries the ticket key, its status, why a PR needs you and how many services run. An agent at work is a dot
/// at the left edge.
fn workspace_row(
    row: &WorkspaceRow,
    current: bool,
    hovered: bool,
    upkeep: Upkeep<'_>,
    inner_w: f32,
    manual: Option<u64>,
) -> Node {
    let colors = theme();
    let key = ticket_key(row);
    let pr = shown_pr(row);
    let trouble = pr.and_then(|pr| pr.trouble);
    let name_color = if current || hovered {
        colors.text
    } else {
        colors.text_muted
    };
    let mut name = label(title(row, key.as_deref()))
        .truncate()
        .color(name_color);
    if current {
        name = name.medium();
    }
    let mut first = div()
        .row()
        .h_px(18.0)
        .gap(6.0)
        .items_center()
        .child(div().row().flex(1.0).items_center().child(name));
    if let Some(pr) = pr {
        first = first.child(pr_pill(row.index, pr));
    }

    let mut details = div().row().h_px(16.0).gap(8.0).items_center();
    let mut has_details = false;
    // Main's update takes its second line while it runs, has just finished, or failed.
    let upkeep_line = match upkeep {
        Upkeep::None => None,
        Upkeep::Running(op) => Some(status_line(
            IconKind::RotateCw,
            colors.text_accent,
            format!("Updating - {}", op_step(op)),
        )),
        Upkeep::Done => Some(status_line(
            IconKind::Check,
            colors.success,
            "Up to date".into(),
        )),
        Upkeep::Failed {
            op,
            position,
            expanded,
        } => {
            let base = crate::WORKSPACE_OP_BASE + position as u64 * crate::WORKSPACE_OP_STRIDE;
            Some(failure_line(
                format!("Update failed - {}", op.error),
                base,
                expanded,
            ))
        }
    };
    if let Some(line) = upkeep_line {
        details = details.child(line);
        has_details = true;
    } else if let Some(key) = &key {
        details = details.child(
            label(key.clone())
                .size(10.5)
                .mono()
                .color(colors.text_placeholder),
        );
        has_details = true;
    }
    if matches!(upkeep, Upkeep::None) && !row.missing.is_empty() {
        details = details.child(
            div().row().flex(1.0).items_center().child(
                label(format!("not cloned: {}", row.missing.join(", ")))
                    .size(11.0)
                    .color(colors.warning)
                    .truncate(),
            ),
        );
        has_details = true;
    } else if matches!(upkeep, Upkeep::None) && !row.ticket.is_empty() {
        // A PR problem outranks the status name; the status keeps its colour square either way.
        let square = div()
            .w_px(6.0)
            .h_px(6.0)
            .rounded(2.0)
            .bg(category_color(&row.ticket_category));
        let mut status = div()
            .row()
            .gap(5.0)
            .items_center()
            .on_click(crate::WORKSPACE_TICKET_BASE + row.index as u64)
            .child(square);
        if trouble.is_none() {
            status = status.flex(1.0).child(
                label(row.ticket.clone())
                    .size(11.0)
                    .color(colors.text_muted)
                    .truncate(),
            );
        }
        details = details.child(status);
        has_details = true;
    } else if matches!(upkeep, Upkeep::None) && row.branch != row.label && row.running > 0 {
        details = details.child(
            label(row.branch.clone())
                .size(10.5)
                .mono()
                .color(colors.text_placeholder)
                .truncate(),
        );
        has_details = true;
    }
    if let Some(trouble) = trouble {
        details = details.child(
            div().row().flex(1.0).items_center().child(
                label(trouble.label())
                    .size(11.0)
                    .color(trouble.color())
                    .truncate(),
            ),
        );
        has_details = true;
    } else if has_details && row.ticket.is_empty() && matches!(upkeep, Upkeep::None) {
        // Push the running count to the end only when something sits before it.
        details = details.child(div().row().flex(1.0));
    }
    if row.running > 0 && !matches!(upkeep, Upkeep::Failed { .. }) {
        let text = if key.is_some() {
            row.running.to_string()
        } else {
            format!("{} running", row.running)
        };
        details = details.child(
            div()
                .row()
                .gap(4.0)
                .items_center()
                .child(div().w_px(5.0).h_px(5.0).rounded(2.5).bg(colors.success))
                .child(label(text).size(11.0).color(colors.text_placeholder)),
        );
        has_details = true;
    }

    let open_failure = match upkeep {
        Upkeep::Failed {
            op,
            position,
            expanded: true,
        } => Some((op, position)),
        _ => None,
    };
    let height = if has_details { 42.0 } else { 30.0 };
    let mut item = div().col();
    if open_failure.is_none() {
        item = item.h_px(height);
    }
    let mut item = item
        .gap(2.0)
        .pl(18.0)
        .pr(8.0)
        .py(if has_details { 4.0 } else { 6.0 })
        .rounded(6.0)
        .on_click(crate::WORKSPACE_ROW_BASE + row.index as u64)
        .child(first);
    if has_details {
        item = item.child(details);
    }
    if let Some((op, position)) = open_failure {
        let manual_open = manual == Some(op.id);
        item = item
            .pb(8.0)
            .child(op_details(op, position, inner_w, false, manual_open));
    }
    if current {
        item = item.bg(colors.element_selected).pin_left_edge(
            0.0,
            9.0,
            2.0,
            div().rounded(1.0).bg(colors.text_accent),
        );
    } else if hovered {
        item = item.bg(colors.element_hover);
    }
    if let Some(agent) = row.agent {
        let dot = div().w_px(6.0).h_px(6.0).rounded(3.0).bg(agent.color());
        if agent != AgentDot::Idle {
            item = item.pin(
                Corner::TopLeft,
                4.0,
                7.0,
                12.0,
                12.0,
                div().rounded(6.0).bg(with_alpha(agent.color(), 0.2)),
            );
        }
        item = item.pin(Corner::TopLeft, 7.0, 10.0, 6.0, 6.0, dot);
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
        .child(workspace_row(row, true, false, Upkeep::None, 240.0, None))
        .into()
}

/// A group header lifted off the list while it is dragged, with its colour so it reads as the whole group.
pub fn group_ghost(group: crate::TicketGroup, count: usize) -> Node {
    let colors = theme();
    div()
        .row()
        .h_px(30.0)
        .px(10.0)
        .gap(6.0)
        .items_center()
        .rounded(6.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border)
        .child(div().w_px(6.0).h_px(6.0).rounded(2.0).bg(group.color()))
        .child(label(group.label()).size(12.0).color(colors.text))
        .child(div().row().flex(1.0))
        .child(label(count.to_string()).size(11.0).color(colors.text_muted))
        .into()
}

/// The PR count: a coloured icon and a muted number while all is well; on a red tint with the reason's icon when a
/// PR is failing, in conflict or sent back.
fn pr_pill(index: usize, pr: crate::PrSummary) -> Node {
    let colors = theme();
    let failing = pr
        .trouble
        .filter(|trouble| *trouble != crate::PrTrouble::Pending);
    let (glyph, glyph_color, text_color, bg) = match failing {
        Some(trouble) => (
            trouble_icon(trouble),
            colors.error,
            colors.error,
            with_alpha(colors.error, 0.12),
        ),
        None => (
            IconKind::PullRequest,
            pr.severity.color(),
            colors.text_muted,
            Rgba::TRANSPARENT,
        ),
    };
    div()
        .row()
        .h_px(18.0)
        .px(5.0)
        .gap(3.0)
        .items_center()
        .rounded(4.0)
        .bg(bg)
        .on_click(crate::WORKSPACE_PR_BASE + index as u64)
        .child(icon(glyph).size(11.0).color(glyph_color))
        .child(label(pr.count.to_string()).size(11.0).color(text_color))
        .into()
}

/// Main's background update as its row shows it.
#[derive(Clone, Copy)]
enum Upkeep<'a> {
    None,
    Running(&'a crate::WorkspaceOp),
    Done,
    Failed {
        op: &'a crate::WorkspaceOp,
        position: usize,
        expanded: bool,
    },
}

fn upkeep_of<'a>(ops: &'a [crate::WorkspaceOp], expanded: &[u64], done: bool) -> Upkeep<'a> {
    use crate::OpStatus;
    match ops.iter().enumerate().find(|(_, op)| op.quiet) {
        Some((position, op)) if op.status == OpStatus::Failed => Upkeep::Failed {
            op,
            position,
            expanded: expanded.contains(&op.id),
        },
        Some((_, op)) if matches!(op.status, OpStatus::Queued | OpStatus::Running) => {
            Upkeep::Running(op)
        }
        _ if done => Upkeep::Done,
        _ => Upkeep::None,
    }
}

/// The step an operation is on: its running stage and that stage's latest line.
fn op_step(op: &crate::WorkspaceOp) -> String {
    let stage = op
        .stages
        .iter()
        .position(|(_, state)| *state == crate::StageState::Running);
    match stage.and_then(|at| op.stages.get(at).map(|(name, _)| (at, name))) {
        Some((_, name)) if !op.detail.is_empty() => format!("{name}: {}", op.detail),
        Some((at, name)) => format!("{name} ({} of {})", at + 1, op.stages.len()),
        None => "starting".to_string(),
    }
}

fn op_progress(op: &crate::WorkspaceOp) -> f32 {
    use crate::StageState;
    let total = op.stages.len().max(1) as f32;
    let finished = op
        .stages
        .iter()
        .filter(|(_, state)| matches!(state, StageState::Done | StageState::Skipped))
        .count() as f32;
    finished / total
}

/// An icon and a line of coloured text, the text truncating: the second line of an operation's row.
fn status_line(glyph: IconKind, color: Rgba, text: String) -> Node {
    div()
        .row()
        .flex(1.0)
        .gap(5.0)
        .items_center()
        .child(icon(glyph).size(11.0).color(color))
        .child(
            div()
                .row()
                .flex(1.0)
                .items_center()
                .child(label(text).size(11.0).color(color).truncate()),
        )
        .into()
}

fn small_button(text: &str, id: u64) -> Node {
    let colors = theme();
    div()
        .row()
        .items_center()
        .h_px(18.0)
        .px(7.0)
        .rounded(4.0)
        .bg(with_alpha(colors.text, 0.07))
        .border(1.0, colors.border)
        .on_click(id)
        .child(label(text.to_string()).size(10.5).color(colors.text))
        .into()
}

fn icon_button(kind: IconKind, id: u64) -> Node {
    div()
        .row()
        .items_center()
        .justify_center()
        .w_px(22.0)
        .h_px(22.0)
        .rounded(4.0)
        .on_click(id)
        .child(icon(kind).size(13.0).color(theme().text_muted))
        .into()
}

/// A failure's second line: the error, and a toggle for its details; the whole line opens them.
fn failure_line(text: String, base: u64, expanded: bool) -> Node {
    let colors = theme();
    div()
        .row()
        .flex(1.0)
        .gap(8.0)
        .items_center()
        .on_click(base + crate::WORKSPACE_OP_TOGGLE)
        .child(status_line(IconKind::Warning, colors.error, text))
        .child(
            label(if expanded { "Hide" } else { "Details" })
                .size(10.5)
                .color(colors.text_muted)
                .underline(colors.text_muted),
        )
        .into()
}

/// A failure opened up: the stages as chips that wrap to the width, the error in full, and what to do about it.
/// Main's update can't be dismissed, only retried: it would just fail again on the next refresh.
fn op_details(
    op: &crate::WorkspaceOp,
    position: usize,
    width: f32,
    dismissable: bool,
    manual_open: bool,
) -> Node {
    use crate::StageState;
    const CHIP_GAP: f32 = 10.0;
    const CHIP_ICON: f32 = 12.0;
    let colors = theme();
    let base = crate::WORKSPACE_OP_BASE + position as u64 * crate::WORKSPACE_OP_STRIDE;
    let mut lines: Vec<Vec<Node>> = vec![Vec::new()];
    let mut used = 0.0;
    for (name, state) in &op.stages {
        let (kind, color) = match state {
            StageState::Done => (Some(IconKind::Check), colors.success),
            StageState::Failed => (Some(IconKind::Close), colors.error),
            StageState::Running => (Some(IconKind::RotateCw), colors.text_accent),
            StageState::Pending | StageState::Skipped => (None, colors.text_disabled),
        };
        let chip_w = ui::measure_text_width(name, 10.5, false, 400)
            + if kind.is_some() { CHIP_ICON } else { 0.0 };
        if used > 0.0 && used + CHIP_GAP + chip_w > width {
            lines.push(Vec::new());
            used = 0.0;
        }
        used += if used > 0.0 { CHIP_GAP } else { 0.0 } + chip_w;
        let mut chip = div().row().gap(3.0).items_center();
        if let Some(kind) = kind {
            chip = chip.child(icon(kind).size(9.0).color(color));
        }
        chip = chip.child(label(name.clone()).size(10.5).color(color));
        if let Some(line) = lines.last_mut() {
            line.push(chip.into());
        }
    }
    let mut stages = div().col().gap(4.0);
    for line in lines {
        let mut row = div().row().gap(CHIP_GAP).items_center();
        for chip in line {
            row = row.child(chip);
        }
        stages = stages.child(row);
    }
    let log = if op.log.is_empty() {
        &op.error
    } else {
        &op.log
    };
    let error = div()
        .col()
        .px(8.0)
        .py(7.0)
        .rounded(5.0)
        .bg(colors.editor_background)
        .border(1.0, colors.border_variant)
        .child(
            label(log.clone())
                .size(10.0)
                .mono()
                .color(colors.text)
                .wrap((width - 18.0).max(60.0)),
        );
    let mut fixes = div().row().gap(6.0).items_center();
    let mut fixes_w = 0.0;
    if op.retryable {
        fixes = fixes.child(small_button("Retry", base + crate::WORKSPACE_OP_RETRY));
        fixes_w += button_width("Retry", 0.0) + 6.0;
    }
    fixes = fixes.child(agent_button(base + crate::WORKSPACE_OP_AGENT));
    fixes_w += button_width(AGENT_LABEL, 15.0);
    // Its menu floats over the list: the view drops it from this button.
    let mut manual = div()
        .row()
        .items_center()
        .gap(3.0)
        .h_px(18.0)
        .px(7.0)
        .rounded(4.0)
        .border(1.0, colors.border)
        .on_click(base + crate::WORKSPACE_OP_MANUAL)
        .child(label("Fix manually").size(10.5).color(colors.text_muted))
        .child(
            icon(IconKind::ChevronDown)
                .size(8.0)
                .color(colors.text_placeholder),
        );
    if manual_open {
        manual = manual.bg(colors.element_selected);
    }
    let manual: Node = manual.into();
    let manual_w = button_width("Fix manually", 11.0);
    let mut icons = div()
        .row()
        .gap(2.0)
        .items_center()
        .child(icon_button(IconKind::Copy, base + crate::WORKSPACE_OP_COPY));
    let mut icons_w = 22.0;
    if dismissable {
        icons = icons.child(icon_button(
            IconKind::Close,
            base + crate::WORKSPACE_OP_DISMISS,
        ));
        icons_w += 24.0;
    }
    // One line while everything fits; otherwise the manual routes and the icons move to a second line.
    let actions: Node = if fixes_w + 6.0 + manual_w + 6.0 + icons_w <= width {
        fixes
            .child(manual)
            .child(div().row().flex(1.0))
            .child(icons)
            .into()
    } else {
        div()
            .col()
            .gap(6.0)
            .child(fixes)
            .child(
                div()
                    .row()
                    .items_center()
                    .child(manual)
                    .child(div().row().flex(1.0))
                    .child(icons),
            )
            .into()
    };
    let body = div()
        .col()
        .gap(6.0)
        .pt(6.0)
        .child(stages)
        .child(error)
        .child(actions);
    body.into()
}

const AGENT_LABEL: &str = "Fix with Claude";

/// A small button's width: its label, padding and border, plus `extra` for an icon beside the label.
fn button_width(text: &str, extra: f32) -> f32 {
    ui::measure_text_width(text, 10.5, false, 400) + 14.0 + 2.0 + extra
}

fn agent_button(id: u64) -> Node {
    let tint = theme().terminal_ansi[5];
    div()
        .row()
        .items_center()
        .gap(4.0)
        .h_px(18.0)
        .px(7.0)
        .rounded(4.0)
        .bg(with_alpha(tint, 0.12))
        .border(1.0, with_alpha(tint, 0.55))
        .on_click(id)
        .child(icon(IconKind::Sparkle).size(11.0).color(tint))
        .child(label(AGENT_LABEL).size(10.5).color(theme().text))
        .into()
}

/// The routes to fix a failure by hand: a terminal in the checkout it names, the config, or doing without the step.
pub fn manual_menu(op: &crate::WorkspaceOp, position: usize, width: f32) -> Node {
    let base = op_base(position);
    let colors = theme();
    let item = |title: String, note: String, id: u64| -> Node {
        div()
            .col()
            .gap(1.0)
            .px(8.0)
            .py(6.0)
            .rounded(5.0)
            .on_click(id)
            .child(label(title).size(12.0).color(colors.text))
            .child(
                label(note)
                    .size(10.5)
                    .mono()
                    .color(colors.text_placeholder)
                    .wrap((width - 34.0).max(60.0)),
            )
            .into()
    };
    let mut menu = div()
        .col()
        .w_px(width)
        .p(4.0)
        .rounded(7.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border);
    if !op.fix_dir.is_empty() {
        let path = std::path::Path::new(&op.fix_dir);
        let repo = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent = path
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        menu = menu.child(item(
            format!("Open terminal in {repo}/"),
            format!("{parent}/{repo}"),
            base + crate::WORKSPACE_OP_TERMINAL,
        ));
    }
    menu = menu.child(item(
        "Edit pom.yml".into(),
        "the project config, then Retry".into(),
        base + crate::WORKSPACE_OP_CONFIG,
    ));
    if !op.skip.is_empty() {
        menu = menu.child(item(
            "Skip this step and finish".into(),
            op.skip.clone(),
            base + crate::WORKSPACE_OP_SKIP,
        ));
    }
    menu.into()
}

/// A workspace being created or deleted, as a row where it will land: its name, then its ticket key and the step
/// it is on with a thin progress bar, its place in the queue, or the error with a toggle to open the details.
fn op_row(
    op: &crate::WorkspaceOp,
    position: usize,
    expanded: bool,
    ahead: usize,
    width: f32,
    manual_open: bool,
) -> Node {
    use crate::OpStatus;
    let colors = theme();
    let base = crate::WORKSPACE_OP_BASE + position as u64 * crate::WORKSPACE_OP_STRIDE;
    let key = op_ticket_key(op);
    let line = match op.status {
        OpStatus::Running => status_line(IconKind::RotateCw, colors.text_accent, op_step(op)),
        OpStatus::Queued => status_line(
            IconKind::Clock,
            colors.text_placeholder,
            if ahead > 1 {
                format!("Queued - {ahead} ahead")
            } else {
                "Queued - next".into()
            },
        ),
        OpStatus::Done => status_line(IconKind::Check, colors.success, "Done".into()),
        OpStatus::Failed => failure_line(op.error.clone(), base, expanded),
    };
    let mut second = div().row().h_px(16.0).gap(8.0).items_center();
    if let Some(key) = &key {
        second = second.child(
            label(key.clone())
                .size(10.5)
                .mono()
                .color(colors.text_placeholder),
        );
    }
    second = second.child(line);
    if op.status == OpStatus::Queued {
        second = second.child(
            div()
                .row()
                .items_center()
                .h_px(16.0)
                .px(6.0)
                .rounded(4.0)
                .on_click(base + crate::WORKSPACE_OP_CANCEL)
                .child(label("Cancel").size(10.5).color(colors.text_muted)),
        );
    }
    let title_color = if op.status == OpStatus::Queued {
        colors.text_placeholder
    } else {
        colors.text_muted
    };
    let name = match &key {
        Some(key) => strip_key(&op.title, key),
        None => op.title.clone(),
    };
    let mut row = div()
        .col()
        .gap(2.0)
        .pl(18.0)
        .pr(8.0)
        .py(4.0)
        .rounded(6.0)
        .child(
            div()
                .row()
                .h_px(18.0)
                .items_center()
                .child(label(name).truncate().color(title_color)),
        )
        .child(second);
    if expanded && op.status == OpStatus::Failed {
        row = row
            .pb(8.0)
            .bg(with_alpha(colors.error, 0.06))
            .child(op_details(op, position, width, true, manual_open));
    } else {
        row = row.h_px(if op.status == OpStatus::Running {
            46.0
        } else {
            42.0
        });
    }
    if op.status == OpStatus::Running {
        // Two growing halves split the width by progress, so the bar follows the row at any sidebar width.
        let done = op_progress(op).clamp(0.02, 1.0);
        let bar = div()
            .row()
            .h_px(2.0)
            .rounded(1.0)
            .bg(colors.border_variant)
            .child(
                div()
                    .h_px(2.0)
                    .flex(done)
                    .rounded(1.0)
                    .bg(colors.text_accent),
            )
            .child(div().h_px(2.0).flex(1.0 - done));
        row = row.child(bar);
    }
    row.into()
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
                label("No agent open in this dock.")
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
            hovered: None,
            upkeep_done: false,
            width: 272.0,
            manual: None,
            grouping: None,
        }
    }

    #[test]
    fn background_upkeep_lives_on_the_main_row_and_a_failure_stays() {
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
            ..Default::default()
        };
        let rows = [row(0, "main", None)];
        let text_of = |op: &crate::WorkspaceOp| {
            let mut p = ProjectPanel::default();
            p.sync(&list(&rows, std::slice::from_ref(op)));
            let painted = ui::render(
                &p.render(),
                ui::Rect::new(0.0, 0.0, 600.0, 600.0, ui::Rgba::TRANSPARENT),
            );
            let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
            let text: Vec<String> = painted.texts.iter().map(|t| t.text.clone()).collect();
            (text, ids)
        };
        let (text, ids) = text_of(&op);
        assert!(
            text.iter().any(|t| t == "Updating - api (1 of 1)"),
            "{text:?}"
        );
        assert!(
            !ids.iter().any(|id| *id >= crate::WORKSPACE_OP_BASE
                && *id < crate::WORKSPACE_OP_BASE + crate::WORKSPACE_OP_STRIDE),
            "no controls while it runs"
        );
        op.status = crate::OpStatus::Failed;
        op.error = "migrate failed".into();
        let (text, ids) = text_of(&op);
        assert!(
            text.iter().any(|t| t == "Update failed - migrate failed"),
            "{text:?}"
        );
        assert!(ids.contains(&(crate::WORKSPACE_OP_BASE + crate::WORKSPACE_OP_TOGGLE)));
    }

    #[test]
    fn review_is_told_apart_from_progress_by_the_status_name() {
        use crate::TicketGroup;
        assert_eq!(
            TicketGroup::of("Code review", "indeterminate"),
            TicketGroup::InReview
        );
        assert_eq!(
            TicketGroup::of("QA In Progress", "indeterminate"),
            TicketGroup::InProgress
        );
        assert_eq!(TicketGroup::of("To Do", "new"), TicketGroup::Backlog);
        assert_eq!(TicketGroup::of("Closed", "done"), TicketGroup::Done);
        assert_eq!(TicketGroup::of("", ""), TicketGroup::Other);
        let grouping =
            crate::Grouping::from_keys(&["done".into(), "bogus".into(), "done".into()], &[]);
        assert_eq!(grouping.order[0], TicketGroup::Done);
        assert_eq!(
            grouping.order.len(),
            TicketGroup::ALL.len(),
            "missing groups come back"
        );
    }

    #[test]
    fn grouped_rows_sit_under_their_status_and_a_folded_group_keeps_only_its_header() {
        let ticket = |index: usize, status: &str, category: &str| WorkspaceRow {
            ticket: status.into(),
            ticket_category: category.into(),
            ..row(index, &format!("proj-{index}"), None)
        };
        let rows = [
            row(0, "main", None),
            ticket(1, "In Progress", "indeterminate"),
            ticket(2, "Done", "done"),
            ticket(3, "Code review", "indeterminate"),
            row(4, "spike", None),
            ticket(5, "In Progress", "indeterminate"),
        ];
        let grouping = crate::Grouping::from_keys(&["other".into()], &["done".into()]);
        let shape: Vec<String> = sections(&rows, Some(&grouping))
            .into_iter()
            .map(|section| match section {
                Section::Header { group, count, .. } => format!("{}:{count}", group.key()),
                Section::Row(row) => row.index.to_string(),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "other:1",
                "4",
                "in_progress:2",
                "1",
                "5",
                "in_review:1",
                "3",
                "done:1"
            ]
        );
        let flat: Vec<usize> = sections(&rows, None)
            .into_iter()
            .filter_map(|section| match section {
                Section::Row(row) => Some(row.index),
                Section::Header { .. } => None,
            })
            .collect();
        assert_eq!(flat, [1, 2, 3, 4, 5], "one shared order, main apart");
    }

    #[test]
    fn the_manual_fixes_offer_a_terminal_where_it_broke_and_skipping_when_allowed() {
        let op = crate::WorkspaceOp {
            id: 7,
            title: "feat-x".into(),
            status: crate::OpStatus::Failed,
            stages: vec![("databases".into(), crate::StageState::Failed)],
            error: "web: boom".into(),
            retryable: true,
            fix_dir: "/work/myproject/feat-x/web".into(),
            skip: "no databases until they start".into(),
            ..Default::default()
        };
        let painted = ui::render(
            &manual_menu(&op, 0, 260.0),
            ui::Rect::new(0.0, 0.0, 600.0, 900.0, ui::Rgba::TRANSPARENT),
        );
        let text: Vec<String> = painted.texts.iter().map(|t| t.text.clone()).collect();
        for expected in [
            "Open terminal in web/",
            "feat-x/web",
            "Edit pom.yml",
            "Skip this step and finish",
        ] {
            assert!(text.iter().any(|t| t == expected), "{expected} in {text:?}");
        }
        let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
        for part in [crate::WORKSPACE_OP_TERMINAL, crate::WORKSPACE_OP_SKIP] {
            assert!(ids.contains(&(crate::WORKSPACE_OP_BASE + part)), "{part}");
        }
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
            ..Default::default()
        };
        let mut p = ProjectPanel::default();
        let expanded = [op.id];
        let rows = [row(0, "main", None)];
        p.sync(&WorkspaceList {
            expanded: &expanded,
            ..list(&rows, std::slice::from_ref(&op))
        });
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
    fn a_ticket_row_shows_its_key_under_the_name_and_why_its_pr_fails() {
        let mut ticket = row(1, "PROJ-101 Email open tracking", None);
        ticket.branch = "proj-101-email-open-tracking".into();
        ticket.ticket = "QA In Progress".into();
        ticket.pr = Some(crate::PrSummary {
            count: 3,
            severity: crate::PrSeverity::Danger,
            trouble: Some(crate::PrTrouble::ChecksFailed),
        });
        assert_eq!(ticket_key(&ticket).as_deref(), Some("PROJ-101"));
        let rows = [row(0, "main", None), ticket];
        let mut p = ProjectPanel::default();
        p.sync(&list(&rows, &[]));
        let painted = ui::render(
            &p.render(),
            ui::Rect::new(0.0, 0.0, 280.0, 600.0, ui::Rgba::TRANSPARENT),
        );
        let text: Vec<String> = painted.texts.iter().map(|t| t.text.clone()).collect();
        for expected in ["Email open tracking", "PROJ-101", "CI failed", "3"] {
            assert!(
                text.contains(&expected.to_string()),
                "{expected} in {text:?}"
            );
        }
        // The failure takes the status name's place on the second line.
        assert!(!text.contains(&"QA In Progress".to_string()), "{text:?}");
    }

    #[test]
    fn main_never_shows_a_pr() {
        let mut main = row(0, "main", None);
        main.pr = Some(crate::PrSummary {
            count: 2,
            severity: crate::PrSeverity::Merged,
            trouble: None,
        });
        let rows = [main];
        let mut p = ProjectPanel::default();
        p.sync(&list(&rows, &[]));
        let painted = ui::render(
            &p.render(),
            ui::Rect::new(0.0, 0.0, 280.0, 600.0, ui::Rgba::TRANSPARENT),
        );
        let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
        assert!(!ids.contains(&crate::WORKSPACE_PR_BASE));
    }

    #[test]
    fn a_ticketless_branch_has_no_key() {
        assert_eq!(ticket_key(&row(1, "investigate-0917-5fwg", None)), None);
    }

    #[test]
    fn a_workspace_with_prs_shows_a_pill_that_opens_its_git_panel() {
        let mut p = ProjectPanel::default();
        let mut with_prs = row(1, "web", None);
        with_prs.pr = Some(crate::PrSummary {
            count: 3,
            severity: crate::PrSeverity::Danger,
            trouble: Some(crate::PrTrouble::ChecksFailed),
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
