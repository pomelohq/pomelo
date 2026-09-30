//! Workspace layout: a self-managed top bar, a left dock (the workspace-list panel — our addition) and a
//! right dock, both resizable via a divider and collapsing to a fixed icon rail instead of vanishing, plus the
//! content area. Computes rectangles, text runs and hit regions; the app drives input and rendering.

pub mod agent_popover;
mod form;
pub mod keymap;
pub mod pane;
pub mod pane_group;
pub mod pane_group_view;
mod panel;
pub mod persistence;
pub mod search_bar;
pub mod tab_drag;
pub mod text_field;
mod update;
mod usage;
mod welcome;
mod workspace_view;
pub use form::{
    checkbox, is_window_modal_id, modal_button, modal_footer, modal_frame, modal_header,
    modal_section, outlined_button, progress_bar, segmented, status_line, toggle_button_group,
    InputField, ModalResult, WindowModal, WINDOW_MODAL_BASE, WINDOW_MODAL_END,
};
pub use panel::{
    function_bar, function_content, function_dock_body, is_side_panel_id, side_panel_base,
    side_panel_kind, terminal_content, terminal_dock_body, AgentDot, AgentEmptyPanel, AgentFix,
    DockPosition, PaletteEntry, PaneKind, Panel, PanelRequest, ProjectPanel, SidePanelView,
    TerminalPanel, WorkspaceList, WorkspaceRow, SIDE_PANEL_BASE, SIDE_PANEL_SPAN,
};
pub use ui::render_keystroke;
pub use update::{
    UpdateAction, UpdateButton, UpdateInfo, UpdateMenuItem, UpdateTone, UPDATE_BUTTON,
    UPDATE_DISMISS,
};
pub use usage::{
    tone as usage_tone, UsageAccount, UsageInfo, UsageToday, UsageWindow, APP_MENU, USAGE_CHIP,
    USAGE_OPEN, USAGE_REFRESH, USAGE_STATUS,
};
pub use welcome::{
    is_welcome_id, MachineCheck, WELCOME_FIX_BASE, WELCOME_IMPORT_BUNDLE, WELCOME_NEW_PROJECT,
    WELCOME_NEW_PROJECT_CARD, WELCOME_OPEN_PROJECT, WELCOME_OPEN_SETTINGS, WELCOME_RECENT_BASE,
    WELCOME_RECHECK,
};

// Re-exported below where defined: status_bar, status_tooltip, tooltip_above, session_action_tooltip, tooltip.
pub use workspace_view::{
    ParkedWorkspace, ResizeCursor, SessionRequest, TitlebarGesture, WorkspaceEffects,
    WorkspaceRequests, WorkspaceView,
};

/// One selection's piece of copied editor text: its length in chars, whether it was a whole line, and the
/// first line's indentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardSlice {
    pub len: usize,
    pub is_entire_line: bool,
    pub first_line_indent: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopiedText {
    pub text: String,
    pub slices: Vec<ClipboardSlice>,
}

thread_local! {
    // The OS clipboard only holds text, so the slices of our last copy live here and apply while the
    // clipboard still holds that same text.
    static LAST_COPY: std::cell::RefCell<Option<CopiedText>> = const { std::cell::RefCell::new(None) };
}

pub fn remember_copy(copied: &CopiedText) {
    LAST_COPY.with(|last| *last.borrow_mut() = Some(copied.clone()));
}

pub fn slices_for(text: &str) -> Option<Vec<ClipboardSlice>> {
    LAST_COPY.with(|last| {
        last.borrow()
            .as_ref()
            .filter(|copied| copied.text == text)
            .map(|copied| copied.slices.clone())
    })
}

use ui::{div, folder_icon, label, render, theme, Node, Painted, Rect, Rgba, Text};

pub const TOP_BAR_H: f32 = 38.0;
pub const STATUS_BAR_H: f32 = 24.0; // the thin status strip at the very bottom (a component, not a dock)
pub const RAIL_W: f32 = 56.0; // collapsed dock width — the icon rail; the dock never goes narrower than this
pub const FUNC_BASE: u64 = 700; // function-nav click ids (bottom bar): FUNC_BASE + PaneKind index
pub const FUNC_VIEW_BASE: u64 = 10000; // click ids owned by a feature's `FunctionView` (routed to it)
/// Click ids owned by the terminal panel's pane group, above every other range.
pub const TERMINAL_VIEW_BASE: u64 = 1_000_000_000;

pub fn is_terminal_id(id: u64) -> bool {
    (TERMINAL_VIEW_BASE..TERMINAL_VIEW_BASE + pane_group_view::ID_SPAN).contains(&id)
}
/// Click ids owned by the agent dock's pane group, right after the terminal panel's.
pub const AGENT_VIEW_BASE: u64 = TERMINAL_VIEW_BASE + pane_group_view::ID_SPAN;

pub fn is_agent_id(id: u64) -> bool {
    (AGENT_VIEW_BASE..AGENT_VIEW_BASE + pane_group_view::ID_SPAN).contains(&id)
}
pub const FILES_TREE_W: f32 = 260.0; // default width of the Files tree dock (left of the center editor)
/// Every side panel (tree, agent, terminal, tool panels) keeps at least this width, wide enough for the
/// widest of them (a Git repo card's name, branch and action chip on one line).
pub const SIDE_PANEL_MIN: f32 = 320.0;
pub const FILES_TREE_MAX: f32 = 560.0;
pub const DOCK_MIN: f32 = 180.0; // narrowest expanded width
pub const DOCK_MAX: f32 = 520.0;
pub const BOTTOM_MIN: f32 = 100.0; // shortest expanded bottom-dock height
pub const BOTTOM_MAX: f32 = 600.0;
pub const DIVIDER_HIT: f32 = 5.0;
pub const TRAFFIC_INSET: f32 = 82.0; // left space reserved for the macOS traffic lights
const SESSION_LEFT: f32 = TRAFFIC_INSET + 14.0; // extra gap so the session trigger clears the traffic lights
const FULLSCREEN_LEFT: f32 = 8.0;

// Colors resolved from the active theme so the workspace restyles with it. Placeholder content panels keep
// fixed demo colors until real content lands.
fn top_bar_c() -> Rgba {
    theme().title_bar_background
}
fn rail_c() -> Rgba {
    theme().tab_active_background
}
fn dock_c() -> Rgba {
    theme().panel_background
}
fn divider_c() -> Rgba {
    theme().border
}
fn text_c() -> Rgba {
    theme().text
}
fn text_dim_c() -> Rgba {
    theme().text_muted
}
/// A dock: geometry (width/collapsed) plus the `Panel` it hosts (the framework's Dock owns its panels). The interior
/// renders through the panel + element tree; `Layout` still owns divider/toggle hit-testing.
pub struct Dock {
    pub position: DockPosition,
    pub width: f32,
    pub collapsed: bool,
    /// While its divider is dragged: the width following the pointer, down to the rail's.
    pub live: Option<f32>,
    pub panel: Box<dyn Panel>,
}

impl Dock {
    /// The dock's cross-axis size (width for side docks, height for the bottom dock). The WORKSPACES panel
    /// (left) is always present and collapses to a rail; the right + bottom docks (conventional) hide entirely
    /// when closed, toggled from the status bar. `width` doubles as the bottom dock's height.
    fn effective(&self) -> f32 {
        match self.position {
            DockPosition::Left => {
                if let Some(live) = self.live {
                    live
                } else if self.collapsed {
                    RAIL_W
                } else {
                    self.width.clamp(DOCK_MIN, DOCK_MAX)
                }
            }
            DockPosition::Right => {
                if self.collapsed {
                    0.0
                } else {
                    self.width.clamp(DOCK_MIN, DOCK_MAX)
                }
            }
            DockPosition::Bottom => {
                if self.collapsed {
                    0.0
                } else {
                    self.width.clamp(BOTTOM_MIN, BOTTOM_MAX)
                }
            }
        }
    }

    /// Render the dock's panel body into `region`, after syncing it with the current session data.
    pub fn render_body(&mut self, region: Rect, list: &crate::panel::WorkspaceList<'_>) -> Painted {
        render(&self.body(list), region)
    }

    /// The dock's own panel as it last synced, for a panel that needs no list (the agent's empty state).
    pub fn render_panel(&mut self, region: Rect) -> Painted {
        render(&self.panel.render(), region)
    }

    pub fn body(&mut self, list: &crate::panel::WorkspaceList<'_>) -> ui::Node {
        self.panel.sync(list);
        self.panel.render()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub name: String,
    pub path: std::path::PathBuf,
    pub running: bool,
    pub missing: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectInfo {
    pub name: String,
    pub branch: String,
    pub config_path: std::path::PathBuf,
    /// Workspace branches on disk, the main one first.
    pub workspaces: Vec<String>,
    /// The workspace this window works in (its files, terminal, branch).
    pub active: String,
    /// Display names by workspace (same order as `workspaces`; empty shows the branch).
    pub labels: Vec<String>,
    /// Running services by workspace (same order as `workspaces`).
    pub running: Vec<usize>,
    /// Jira ticket status by workspace (same order; empty when none).
    pub tickets: Vec<String>,
    /// Each ticket status's Jira category (`new`, `indeterminate`, `done`; same order).
    pub ticket_categories: Vec<String>,
    pub prs: Vec<Option<PrSummary>>,
    /// Repos of the config main has no clone of (other workspaces pick their repos on purpose: empty).
    pub missing: Vec<Vec<String>>,
    /// Repos whose checked-out branch is not the workspace branch, by workspace (same order).
    pub repo_branches: Vec<Vec<RepoBranch>>,
}

/// A repo of a workspace on a branch other than the workspace's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoBranch {
    pub repo: String,
    /// The branch checked out now.
    pub actual: String,
    /// The branch the workspace means it to be on: its recorded choice, else the workspace branch.
    pub expected: String,
    /// The other branch was chosen on purpose (recorded, and still checked out).
    pub kept: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrSeverity {
    Ok,
    Warn,
    Merged,
    Danger,
}

impl PrSeverity {
    pub fn color(self) -> ui::Rgba {
        let colors = ui::theme();
        match self {
            PrSeverity::Ok => colors.success,
            PrSeverity::Warn => colors.warning,
            PrSeverity::Merged => colors.terminal_ansi[5],
            PrSeverity::Danger => colors.error,
        }
    }
}

/// Why a workspace's PRs need someone; the sidebar names it instead of just colouring the count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrTrouble {
    Pending,
    ChangesRequested,
    ChecksFailed,
    Conflict,
}

impl PrTrouble {
    pub fn label(self) -> &'static str {
        match self {
            PrTrouble::Pending => "Checks pending",
            PrTrouble::ChangesRequested => "Changes requested",
            PrTrouble::ChecksFailed => "CI failed",
            PrTrouble::Conflict => "Conflict",
        }
    }

    pub fn color(self) -> ui::Rgba {
        match self {
            PrTrouble::Pending => ui::theme().warning,
            _ => ui::theme().error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrSummary {
    pub count: usize,
    pub severity: PrSeverity,
    pub trouble: Option<PrTrouble>,
}

impl ProjectInfo {
    /// What the WORKSPACES list calls the workspace at `index`: its display name, else its branch.
    pub fn label(&self, index: usize) -> &str {
        match self.labels.get(index) {
            Some(label) if !label.is_empty() => label,
            _ => self.workspaces.get(index).map_or("", String::as_str),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpStatus {
    #[default]
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageState {
    Pending,
    Running,
    Done,
    Skipped,
    Failed,
}

/// A workspace being created or deleted, as the WORKSPACES panel shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceOp {
    pub id: u64,
    pub branch: String,
    pub title: String,
    pub status: OpStatus,
    pub stages: Vec<(String, StageState)>,
    /// The latest progress line of the running stage.
    pub detail: String,
    pub error: String,
    /// Whether a failed run can resume from its failed stage.
    pub retryable: bool,
    /// Background upkeep (keeping main fresh): shown on main's row rather than as a row of its own.
    pub quiet: bool,
    /// The whole error of a failed run (a command's exit and the tail of its output), `error` being its first line.
    pub log: String,
    /// The checkout the failure happened in, where a manual fix starts; empty when no repo is to blame.
    pub fix_dir: String,
    /// What skipping the failed stage would leave undone, empty when that stage can't be skipped.
    pub skip: String,
}

/// Where a workspace's ticket stands, for the list grouped by ticket status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TicketGroup {
    InProgress,
    InReview,
    Backlog,
    Done,
    /// No ticket, or one we could not read.
    Other,
}

impl TicketGroup {
    pub const ALL: [TicketGroup; 5] = [
        TicketGroup::InProgress,
        TicketGroup::InReview,
        TicketGroup::Backlog,
        TicketGroup::Done,
        TicketGroup::Other,
    ];

    /// The tracker files review under "in progress", so the status name tells review apart.
    pub fn of(status: &str, category: &str) -> TicketGroup {
        match category {
            _ if status.is_empty() => TicketGroup::Other,
            "done" => TicketGroup::Done,
            "new" => TicketGroup::Backlog,
            "indeterminate" if status.to_lowercase().contains("review") => TicketGroup::InReview,
            "indeterminate" => TicketGroup::InProgress,
            _ => TicketGroup::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TicketGroup::InProgress => "In progress",
            TicketGroup::InReview => "In review",
            TicketGroup::Backlog => "Backlog",
            TicketGroup::Done => "Done",
            TicketGroup::Other => "Other",
        }
    }

    /// The name it is saved under.
    pub fn key(self) -> &'static str {
        match self {
            TicketGroup::InProgress => "in_progress",
            TicketGroup::InReview => "in_review",
            TicketGroup::Backlog => "backlog",
            TicketGroup::Done => "done",
            TicketGroup::Other => "other",
        }
    }

    pub fn from_key(key: &str) -> Option<TicketGroup> {
        TicketGroup::ALL
            .into_iter()
            .find(|group| group.key() == key)
    }

    pub fn index(self) -> usize {
        TicketGroup::ALL
            .iter()
            .position(|group| *group == self)
            .unwrap_or(0)
    }

    pub fn color(self) -> Rgba {
        let colors = theme();
        match self {
            TicketGroup::InProgress => colors.text_accent,
            TicketGroup::InReview => colors.warning,
            TicketGroup::Backlog => colors.text_placeholder,
            TicketGroup::Done => colors.success,
            TicketGroup::Other => colors.text_disabled,
        }
    }
}

/// How the grouped list is arranged: the groups in the user's order, and which are folded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grouping {
    pub order: Vec<TicketGroup>,
    pub folded: Vec<TicketGroup>,
}

impl Grouping {
    /// Groups from saved keys, unknown ones dropped and missing ones appended in their usual place.
    pub fn from_keys(order: &[String], folded: &[String]) -> Grouping {
        let mut groups: Vec<TicketGroup> = Vec::new();
        for group in order.iter().filter_map(|key| TicketGroup::from_key(key)) {
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        for group in TicketGroup::ALL {
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        Grouping {
            order: groups,
            folded: folded
                .iter()
                .filter_map(|key| TicketGroup::from_key(key))
                .collect(),
        }
    }

    pub fn keys(&self) -> (Vec<String>, Vec<String>) {
        let keys =
            |groups: &[TicketGroup]| groups.iter().map(|group| group.key().to_string()).collect();
        (keys(&self.order), keys(&self.folded))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpAction {
    Retry,
    Dismiss,
    /// Drop a queued operation before it starts.
    Cancel,
    /// Resume past the failed stage.
    Skip,
    /// Start an agent on the failure, in the checkout it happened in.
    FixWithAgent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAction {
    Rename,
    StopServices,
    Delete,
    UpdateMain,
    PrepareMain,
    OpenTicket,
    /// Clone into main the repos the config names but main lacks.
    AddMissingRepos,
    /// Pick more of the config's repos to check out in this workspace.
    AddRepos,
}

// Header click ids for the session switcher (routed by the app). Kept distinct from dock geometry hits.
pub const SESSION_TRIGGER: u64 = 1;
pub const SESSION_OPEN: u64 = 3;
pub const SESSION_SEARCH: u64 = 4;
/// The status bar's leftmost button toggles the WORKSPACES sidebar (the sidebar toggle).
pub const SIDEBAR_TOGGLE: u64 = 5;
/// The status bar's terminal button toggles the bottom dock.
pub const BOTTOM_TOGGLE: u64 = 6;
/// The right dock's collapse toggle (in a right-docked panel's header).
pub const RIGHT_TOGGLE: u64 = 7;
/// The agent panel button: activates the agent in the right dock (a panel like the others), toggling it.
pub const AGENT_TOGGLE: u64 = 8;
pub const TOAST_ACTION: u64 = 9;
pub const TOAST_CLOSE: u64 = 10;
pub const NOTIFICATION_PRIMARY: u64 = 620;
pub const NOTIFICATION_CLOSE: u64 = 621;
/// A row of the WORKSPACES panel: id = base + index into `ProjectInfo::workspaces`.
pub const SIDE_PANEL_MENU_TARGET: u64 = 1500;

/// What a side agent is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideAgentRole {
    Ask,
    Review,
    SecondOpinion,
    Fix,
}

impl SideAgentRole {
    pub const ALL: [SideAgentRole; 4] = [
        SideAgentRole::Ask,
        SideAgentRole::Review,
        SideAgentRole::SecondOpinion,
        SideAgentRole::Fix,
    ];

    pub fn title(self) -> &'static str {
        match self {
            SideAgentRole::Ask => "Ask",
            SideAgentRole::Review => "Review",
            SideAgentRole::SecondOpinion => "Second opinion",
            SideAgentRole::Fix => "Fix",
        }
    }
}

/// What a side agent starts with; `Auto` lets the app pick from the main session's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideAgentStart {
    Auto,
    Fork,
    Compacted,
    Fresh,
}

impl SideAgentStart {
    pub const ALL: [SideAgentStart; 4] = [
        SideAgentStart::Auto,
        SideAgentStart::Fork,
        SideAgentStart::Compacted,
        SideAgentStart::Fresh,
    ];

    pub fn title(self) -> &'static str {
        match self {
            SideAgentStart::Auto => "Auto",
            SideAgentStart::Fork => "Fork",
            SideAgentStart::Compacted => "Compacted",
            SideAgentStart::Fresh => "Fresh",
        }
    }
}
pub const WORKSPACE_ROW_BASE: u64 = 2000;
pub const WORKSPACE_ROW_END: u64 = 3000;
pub const WORKSPACE_PR_BASE: u64 = 4000;
pub const WORKSPACE_PR_END: u64 = 5000;
pub const WORKSPACE_TICKET_BASE: u64 = 5000;
pub const WORKSPACE_TICKET_END: u64 = 6000;
/// A group header in the WORKSPACES list (or its marker on the rail), by `TicketGroup::index`.
pub const WORKSPACE_GROUP_BASE: u64 = 6000;
pub const WORKSPACE_GROUP_END: u64 = 6010;
/// The WORKSPACES header's new-workspace button.
pub const WORKSPACE_NEW: u64 = 14;
/// The status bar's button for the active workspace's Jira ticket.
pub const STATUS_TICKET: u64 = 15;
/// The session menu's "Edit pom.yml" row.
pub const SESSION_EDIT_CONFIG: u64 = 16;
/// The session menu's "New session..." row.
pub const SESSION_NEW: u64 = 17;
/// A workspace operation card: base + position * stride + part.
pub const WORKSPACE_OP_BASE: u64 = 3000;
pub const WORKSPACE_OP_END: u64 = 4000;
pub const WORKSPACE_OP_STRIDE: u64 = 12;
pub const WORKSPACE_OP_TOGGLE: u64 = 0;
pub const WORKSPACE_OP_RETRY: u64 = 1;
pub const WORKSPACE_OP_DISMISS: u64 = 2;
pub const WORKSPACE_OP_COPY: u64 = 3;
pub const WORKSPACE_OP_CANCEL: u64 = 4;
pub const WORKSPACE_OP_SKIP: u64 = 5;
pub const WORKSPACE_OP_AGENT: u64 = 6;
pub const WORKSPACE_OP_TERMINAL: u64 = 7;
pub const WORKSPACE_OP_CONFIG: u64 = 8;
/// Opens or closes the menu of manual fixes.
pub const WORKSPACE_OP_MANUAL: u64 = 9;
/// The "!" on a rail tile: the failure's details in a popover beside it.
pub const WORKSPACE_OP_POPOVER: u64 = 10;
/// The context menu of a WORKSPACES row, and its items.
pub const WORKSPACE_ROW_MENU_TARGET: u64 = 1600;
pub const MENU_WS_RENAME: u64 = 940;
pub const MENU_WS_STOP: u64 = 941;
pub const MENU_WS_DELETE: u64 = 942;
pub const MENU_WS_UPDATE_MAIN: u64 = 943;
pub const MENU_WS_PREPARE_MAIN: u64 = 944;
pub const MENU_WS_OPEN_TICKET: u64 = 945;
pub const MENU_WS_ADD_MISSING: u64 = 946;
pub const MENU_WS_ADD_REPOS: u64 = 947;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticSummary {
    pub errors: usize,
    pub warnings: usize,
    pub current: Option<String>,
}

pub const CURSOR_POSITION: u64 = 11;
pub const DIAGNOSTIC_MESSAGE: u64 = 12;
/// The status bar's language-server button and the line of server activity beside it.
pub const LANGUAGE_SERVERS_BUTTON: u64 = 23;
pub const LANGUAGE_ACTIVITY: u64 = 24;
/// The status bar's error and warning counts, which open the project diagnostics tab.
pub const DIAGNOSTICS_BUTTON: u64 = 25;
pub const LANGUAGE_SERVERS_MENU_TARGET: u64 = 857;
pub const LANGUAGE_ACTIVITY_MENU_TARGET: u64 = 858;
/// A server's row in the language-server menu, opening its submenu: base + its index.
pub const LANGUAGE_SERVER_SUBMENU_BASE: u64 = 1700;
const LANGUAGE_SERVER_LIMIT: u64 = 16;
/// A server's submenu entries: base + index * stride + which.
pub const LANGUAGE_SERVER_ACTION_BASE: u64 = 1900;
const LANGUAGE_SERVER_ACTION_STRIDE: u64 = 4;
pub const LANGUAGE_SERVERS_RESTART_ALL: u64 = 1890;
pub const LANGUAGE_SERVERS_STOP_ALL: u64 = 1891;
/// Cancel the activity menu's work at this index: base + index.
pub const LANGUAGE_WORK_CANCEL_BASE: u64 = 1970;
/// The status-bar item names this keeps the line of server activity to, before trailing off.
pub const ACTIVITY_MESSAGE_LIMIT: usize = 50;

/// How a language server is doing, by the color its dot shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerHealth {
    Starting,
    Running,
    Stopped,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LanguageServerRow {
    pub name: String,
    pub health: ServerHealth,
    pub message: Option<String>,
    pub version: Option<String>,
    pub memory: Option<String>,
    /// What runs it, for the tooltip of its details.
    pub binary: Option<String>,
    pub can_stop: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityIcon {
    Loading,
    Download,
    Warning,
}

/// The one line of server activity the status bar shows: what it says, and what clicking it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub icon: ActivityIcon,
    pub message: String,
    pub click: ActivityClick,
    /// The work in progress that can be cancelled, by title, when clicking lists it.
    pub cancellable: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityClick {
    ListWork,
    ShowError,
    Dismiss,
}

/// The language servers of the files view's folder, and what they are doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LanguageServers {
    pub folder: String,
    pub servers: Vec<LanguageServerRow>,
    pub activity: Option<Activity>,
    pub can_restart_all: bool,
    pub can_stop_all: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageServerAction {
    ViewMessage(usize),
    Restart(usize),
    Stop(usize),
    RestartAll,
    StopAll,
    CancelWork(usize),
    ShowError,
    DismissActivity,
}

/// What a click in the language-server menus asks for.
pub fn language_server_action(id: u64) -> Option<LanguageServerAction> {
    match id {
        LANGUAGE_SERVERS_RESTART_ALL => return Some(LanguageServerAction::RestartAll),
        LANGUAGE_SERVERS_STOP_ALL => return Some(LanguageServerAction::StopAll),
        _ => {}
    }
    if let Some(index) = id
        .checked_sub(LANGUAGE_WORK_CANCEL_BASE)
        .filter(|index| *index < LANGUAGE_SERVER_LIMIT)
    {
        return Some(LanguageServerAction::CancelWork(index as usize));
    }
    let offset = id.checked_sub(LANGUAGE_SERVER_ACTION_BASE)?;
    let index = (offset / LANGUAGE_SERVER_ACTION_STRIDE) as usize;
    if index as u64 >= LANGUAGE_SERVER_LIMIT {
        return None;
    }
    match offset % LANGUAGE_SERVER_ACTION_STRIDE {
        0 => Some(LanguageServerAction::ViewMessage(index)),
        1 => Some(LanguageServerAction::Restart(index)),
        2 => Some(LanguageServerAction::Stop(index)),
        _ => None,
    }
}

fn language_server_action_id(index: usize, which: u64) -> u64 {
    LANGUAGE_SERVER_ACTION_BASE + index as u64 * LANGUAGE_SERVER_ACTION_STRIDE + which
}

fn health_color(health: ServerHealth) -> Rgba {
    let colors = theme();
    match health {
        ServerHealth::Starting => colors.version_control_modified,
        ServerHealth::Running => colors.success,
        ServerHealth::Stopped => colors.text_disabled,
        ServerHealth::Error => colors.error,
    }
}

fn health_label(health: ServerHealth) -> &'static str {
    match health {
        ServerHealth::Starting => "Starting...",
        ServerHealth::Running => "Running",
        ServerHealth::Stopped => "Stopped",
        ServerHealth::Error => "Error",
    }
}

impl LanguageServers {
    /// The dot on the status-bar button, and what its tooltip says of the servers.
    pub fn health(&self) -> (Option<Rgba>, &'static str) {
        if self
            .servers
            .iter()
            .any(|server| server.health == ServerHealth::Error)
        {
            (Some(theme().error), "Server with errors")
        } else if self.servers.iter().any(|server| server.message.is_some()) {
            (
                Some(theme().version_control_modified),
                "Server with notifications",
            )
        } else {
            (None, "All Servers Operational")
        }
    }

    /// The language-server menu: the folder, each server with its submenu, then restart and stop for all.
    pub fn menu_items(&self) -> Vec<MenuItem> {
        let mut items = vec![MenuItem {
            header: true,
            ..MenuItem::new(0, self.folder.clone())
        }];
        items.extend(
            self.servers
                .iter()
                .enumerate()
                .take(LANGUAGE_SERVER_LIMIT as usize)
                .map(|(index, server)| MenuItem {
                    icon: Some(ui::IconKind::Circle),
                    color: Some(health_color(server.health)),
                    ..MenuItem::new(
                        LANGUAGE_SERVER_SUBMENU_BASE + index as u64,
                        server.name.clone(),
                    )
                }),
        );
        if self.can_stop_all || self.can_restart_all {
            items.push(MenuItem {
                sep: true,
                ..MenuItem::new(LANGUAGE_SERVERS_RESTART_ALL, "Restart All Servers")
            });
        }
        if self.can_stop_all {
            items.push(MenuItem::new(LANGUAGE_SERVERS_STOP_ALL, "Stop All Servers"));
        }
        items
    }

    /// A server's submenu: its message, restart, stop, and a line of how it is.
    pub fn submenu_items(&self, id: u64) -> Vec<MenuItem> {
        let Some(index) = id
            .checked_sub(LANGUAGE_SERVER_SUBMENU_BASE)
            .map(|index| index as usize)
        else {
            return Vec::new();
        };
        let Some(server) = self.servers.get(index) else {
            return Vec::new();
        };
        let mut items = Vec::new();
        if server.message.is_some() {
            items.push(MenuItem::new(
                language_server_action_id(index, 0),
                "View Message",
            ));
        }
        items.push(MenuItem::new(
            language_server_action_id(index, 1),
            "Restart Server",
        ));
        if server.can_stop {
            items.push(MenuItem::new(
                language_server_action_id(index, 2),
                "Stop Server",
            ));
        }
        let details = std::iter::once(health_label(server.health).to_string())
            .chain(server.version.iter().map(|version| format!("v{version}")))
            .chain(server.memory.iter().cloned())
            .collect::<Vec<_>>()
            .join(" - ");
        items.push(MenuItem {
            sep: true,
            disabled: true,
            icon: Some(ui::IconKind::Circle),
            color: Some(health_color(server.health)),
            ..MenuItem::new(language_server_action_id(index, 3), details)
        });
        if let Some(message) = &server.message {
            items.push(MenuItem {
                disabled: true,
                ..MenuItem::new(language_server_action_id(index, 3), message.clone())
            });
        }
        items
    }

    /// The details row a hovered submenu entry stands for: the binary that runs the server.
    pub fn details_tooltip(&self, id: u64) -> Option<String> {
        let offset = id.checked_sub(LANGUAGE_SERVER_ACTION_BASE)?;
        if offset % LANGUAGE_SERVER_ACTION_STRIDE != 3 {
            return None;
        }
        self.servers
            .get((offset / LANGUAGE_SERVER_ACTION_STRIDE) as usize)?
            .binary
            .clone()
    }

    /// The activity menu: the work that can be cancelled.
    pub fn activity_items(&self) -> Vec<MenuItem> {
        self.activity
            .iter()
            .flat_map(|activity| activity.cancellable.iter().enumerate())
            .map(|(index, title)| MenuItem {
                icon: Some(ui::IconKind::Close),
                ..MenuItem::new(
                    LANGUAGE_WORK_CANCEL_BASE + index as u64,
                    format!("Cancel {title}"),
                )
            })
            .collect()
    }
}

/// `text` cut to `limit` characters with a trailing `...` when longer.
pub fn trail_off(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}...")
}
/// Agent right-click menu item ids.
pub const MENU_DOCK_LEFT: u64 = 810;
pub const MENU_DOCK_RIGHT: u64 = 811;
pub const MENU_DOCK_BOTTOM: u64 = 813;
pub const MENU_HIDE: u64 = 812;
pub const MENU_COPY_PATH: u64 = 820;
pub const MENU_COPY_REL_PATH: u64 = 821;
pub const MENU_REVEAL: u64 = 822;
pub const MENU_TREE_OPEN: u64 = 823;
pub const MENU_EDIT_CUT: u64 = 830;
pub const MENU_EDIT_COPY: u64 = 831;
pub const MENU_EDIT_PASTE: u64 = 832;
pub const MENU_EDIT_SELECT_ALL: u64 = 833;
pub const MENU_COPY_NAME: u64 = 824;
pub const TREE_MENU_TARGET: u64 = 850;
pub const EDITOR_MENU_TARGET: u64 = 851;
pub const TERMINAL_MENU_TARGET: u64 = 853;
pub const MENU_TERM_COPY: u64 = 930;
pub const MENU_TERM_PASTE: u64 = 931;
pub const MENU_TERM_SELECT_ALL: u64 = 932;
pub const MENU_TERM_CLEAR: u64 = 933;
pub const MENU_TERM_ADD_TO_AGENT: u64 = 934;
pub const MENU_TERM_ASK_AGENT: u64 = 935;
pub const MENU_TERM_CLOSE: u64 = 936;
pub const MENU_SUBMENU_BASE: u64 = 860;
pub const MENU_SUBMENU_COPY: u64 = 860;
pub const MENU_TREE_NEW_FILE: u64 = 870;
pub const MENU_TREE_NEW_DIR: u64 = 871;
pub const MENU_TREE_OPEN_SYSTEM: u64 = 872;
pub const MENU_TREE_CUT: u64 = 873;
pub const MENU_TREE_COPY: u64 = 874;
pub const MENU_TREE_DUPLICATE: u64 = 875;
pub const MENU_TREE_PASTE: u64 = 876;
pub const MENU_TREE_RESTORE: u64 = 877;
pub const MENU_TREE_GITIGNORE: u64 = 878;
pub const MENU_TREE_RENAME: u64 = 879;
pub const MENU_TREE_TRASH: u64 = 880;
pub const MENU_TREE_DELETE: u64 = 881;
pub const MENU_TREE_EXPAND_ALL: u64 = 882;
pub const MENU_TREE_COLLAPSE_ALL: u64 = 883;
pub const MENU_EDIT_GO_TO_DEFINITION: u64 = 890;
pub const MENU_EDIT_GO_TO_DECLARATION: u64 = 891;
pub const MENU_EDIT_GO_TO_TYPE_DEFINITION: u64 = 892;
pub const MENU_EDIT_GO_TO_IMPLEMENTATION: u64 = 893;
pub const MENU_EDIT_COPY_TRIM: u64 = 894;
pub const MENU_EDIT_REVEAL: u64 = 895;
pub const MENU_TREE_OPEN_TERMINAL: u64 = 884;
pub const MENU_EDIT_OPEN_TERMINAL: u64 = 896;
pub const MENU_EDIT_SPLIT_DIFF: u64 = 897;
pub const MENU_EDIT_MARKDOWN_PREVIEW: u64 = 898;
pub const TAB_MENU_TARGET: u64 = 852;
/// The title bar's app menu (its chevron).
pub const APP_MENU_TARGET: u64 = 854;
/// A menu a tab's own toolbar button opened; its entries and picks belong to that tab.
pub const ITEM_MENU_TARGET: u64 = 856;
pub const MENU_APP_ACCOUNT: u64 = 950;
pub const MENU_APP_UPDATE: u64 = 951;
pub const MENU_APP_SETTINGS: u64 = 952;
pub const MENU_APP_KEYMAP: u64 = 953;
pub const MENU_APP_THEME: u64 = 954;
pub const MENU_APP_USAGE: u64 = 955;
pub const MENU_APP_RELEASE_NOTES: u64 = 956;
/// The app menu's Panel Layout submenu.
pub const MENU_SUBMENU_LAYOUT: u64 = 869;
pub const MENU_TAB_CLOSE: u64 = 900;
pub const MENU_TAB_CLOSE_OTHERS: u64 = 901;
pub const MENU_TAB_CLOSE_LEFT: u64 = 902;
pub const MENU_TAB_CLOSE_RIGHT: u64 = 903;
pub const MENU_TAB_CLOSE_CLEAN: u64 = 904;
pub const MENU_TAB_CLOSE_ALL: u64 = 905;
pub const MENU_TAB_COPY_PATH: u64 = 906;
pub const MENU_TAB_COPY_REL_PATH: u64 = 907;
pub const MENU_TAB_REVEAL: u64 = 908;
pub const MENU_TAB_REVEAL_IN_TREE: u64 = 909;
pub const MENU_TAB_OPEN_TERMINAL: u64 = 910;
pub const MENU_TAB_STOP: u64 = 911;
pub const MENU_TAB_TOGGLE_PIN: u64 = 911;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeAction {
    NewFile,
    NewDirectory,
    OpenWithSystem,
    Cut,
    Copy,
    Duplicate,
    Paste,
    RestoreFile,
    AddToGitignore,
    Rename,
    Trash,
    Delete,
    ExpandAll,
    CollapseAll,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeMenuState {
    pub is_dir: bool,
    pub is_root: bool,
    pub has_git_repo: bool,
    pub has_git_changes: bool,
    pub can_paste: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub token: u64,
    pub message: String,
    pub detail: Option<String>,
    pub buttons: Vec<String>,
}

pub fn is_submenu(id: u64) -> bool {
    (MENU_SUBMENU_BASE..MENU_SUBMENU_BASE + 10).contains(&id)
        || (LANGUAGE_SERVER_SUBMENU_BASE..LANGUAGE_SERVER_SUBMENU_BASE + LANGUAGE_SERVER_LIMIT)
            .contains(&id)
}

pub fn menu_key(id: u64) -> &'static str {
    match id {
        MENU_EDIT_CUT => "cmd-x",
        MENU_EDIT_COPY | MENU_COPY_PATH => "cmd-c",
        MENU_EDIT_PASTE => "cmd-v",
        MENU_EDIT_SELECT_ALL => "cmd-a",
        MENU_COPY_REL_PATH => "cmd-shift-c",
        MENU_REVEAL | MENU_EDIT_REVEAL => "alt-cmd-r",
        MENU_TREE_NEW_FILE => "cmd-n",
        MENU_TREE_NEW_DIR => "alt-cmd-n",
        MENU_TREE_OPEN_SYSTEM => "ctrl-shift-enter",
        MENU_TREE_CUT => "cmd-x",
        MENU_TREE_COPY => "cmd-c",
        MENU_TREE_DUPLICATE => "cmd-d",
        MENU_TREE_PASTE => "cmd-v",
        MENU_TREE_RENAME => "enter",
        MENU_TREE_TRASH => "backspace",
        MENU_TREE_DELETE => "cmd-delete",
        MENU_EDIT_GO_TO_DEFINITION => "f12",
        MENU_EDIT_GO_TO_DECLARATION => "ctrl-f12",
        MENU_EDIT_GO_TO_TYPE_DEFINITION => "cmd-f12",
        MENU_EDIT_GO_TO_IMPLEMENTATION => "shift-f12",
        _ => "",
    }
}
/// A session row: id = `SESSION_ITEM_BASE + original index`.
pub const SESSION_ITEM_BASE: u64 = 100;
/// A session row's delete (x) button: id = `SESSION_DELETE_BASE + original index`.
pub const SESSION_DELETE_BASE: u64 = 200;
/// A session row's "open in new window" (arrow) button: id = base + index.
pub const SESSION_OPENNEW_BASE: u64 = 300;
/// A session row's "open in this window" (window) button: id = base + index.
pub const SESSION_OPENTHIS_BASE: u64 = 400;
/// A session row's "reveal in Finder" (folder) button: id = base + index.
pub const SESSION_REVEAL_BASE: u64 = 500;
// Session menu metrics (design px). The list scrolls smoothly (by pixels) between the fixed search and footer.
const MENU_PAD: f32 = 4.0;
const MENU_SEARCH_H: f32 = 30.0;
const MENU_DIV_H: f32 = 9.0; // menu_divider() = py(4) + 1px line
const MENU_HEADER_H: f32 = 24.0;
const MENU_ITEM_H: f32 = 32.0;
const MENU_ACTION_H: f32 = 32.0;
const MENU_MAX_LIST: f32 = 300.0; // list region caps here, then scrolls

pub struct Layout {
    /// Roughly how many tokens each side agent start would begin with (Auto, Fork, Compacted, Fresh), for
    /// the "+" menu; `None` where it is not known.
    pub side_agent_sizes: [Option<usize>; 4],
    /// The main agent's session size, for the "+" popover.
    pub main_agent_tokens: Option<usize>,
    /// Side agents that were closed, newest first, for the history popover.
    pub archived_agents: Vec<agent_popover::ArchivedAgent>,
    /// The other coding CLI a second opinion runs with, when one is installed.
    pub other_cli: Option<String>,
    pub left: Dock,
    pub right: Dock,
    pub bottom: Dock,
    /// Which side the WORKSPACES sidebar docks on (Left/Right), changed via its status-bar right-click menu.
    pub sidebar_side: DockPosition,
    /// Which dock the agent panel belongs to (Left or Right), changed via its status-bar right-click menu. It is
    /// one of that dock's panels; the docks themselves never move.
    pub agent_side: DockPosition,
    /// Whether the agent's status-bar toggle button is hidden ("Hide Button").
    pub agent_hidden: bool,
    /// Whether the terminal's status-bar toggle button is hidden.
    pub terminal_hidden: bool,
    /// Which content area the terminal renders in (Left = center, Right/Bottom = that dock).
    pub terminal_side: DockPosition,
    /// The active (visible) panel of each content area, indexed by `DockPosition::index()`. Each side tracks
    /// its own active panel independently (conventional: there is no single global active panel).
    pub active_panels: [Option<Shown>; 3],
    /// Which function-nav buttons are hidden (index = `PaneKind::ALL` index).
    pub func_hidden: Vec<bool>,
    /// Which content area each function's content renders in (its dock side), index = `PaneKind::ALL` index.
    pub func_side: Vec<DockPosition>,
    pub sessions: Vec<Session>,
    /// The agents' usage: the title bar chip and the status bar's total.
    pub usage: UsageInfo,
    pub update: UpdateInfo,
    /// The app menu or the usage card is open, so their buttons stay lit.
    pub app_menu_open: bool,
    pub usage_card_open: bool,
    pub usage_today_open: bool,
    /// What the welcome page reports about this Mac.
    pub machine: Vec<MachineCheck>,
    pub current_session: Option<usize>,
    pub project: Option<ProjectInfo>,
    pub session_menu: bool,
    /// Pixel scroll offset of the (scrollable) session menu list.
    pub session_scroll: f32,
    /// The window fills the screen: macOS hides the traffic lights, so the header needs no room for them.
    pub fullscreen: bool,
    pub files_view: Option<Box<dyn FunctionView>>,
    /// The files view's language servers as the status bar shows them, taken each frame.
    pub language_servers: LanguageServers,
    pub terminal_view: Option<Box<dyn TerminalPanelView>>,
    /// Agent sessions, shown in the agent dock rather than among the terminals.
    pub agent_view: Option<Box<dyn TerminalPanelView>>,
    pub side_panels: Vec<Box<dyn SidePanelView>>,
    /// Branch -> what its coding agent last reported.
    pub agent_states: std::collections::HashMap<String, AgentDot>,
    /// A repo service of the active workspace is running (shared services don't count).
    pub services_running: bool,
    pub files_tree_w: f32,
    /// The left panel column (Files, Services, ...) is closed: its active button was clicked again.
    pub panels_collapsed: bool,
}

pub struct TreePanel {
    pub tree: Node,
    pub sticky: Node,
    pub scroll_x: f32,
    pub content_w: f32,
    pub y_offset: f32,
}

pub trait Item: 'static {
    fn id(&self) -> Option<String> {
        None
    }
    fn title(&self) -> String;
    fn icon(&self) -> Option<ui::MaterialIcon> {
        None
    }
    /// A monochrome tab icon, used instead of the file-type icon when set.
    fn tab_icon(&self) -> Option<ui::IconKind> {
        None
    }
    /// What the tab shows in place of its icon and title (counts with their icons, say).
    fn tab_content(&self, _active: bool) -> Option<ui::Node> {
        None
    }
    /// The body's fill, which the active tab's bottom edge matches so the two read as one surface.
    fn body_background(&self) -> ui::Rgba {
        ui::theme().editor_background
    }
    fn searchable(&mut self) -> Option<&mut dyn search_bar::Searchable> {
        None
    }
    /// For back/forward history: the caret offset, its row and the scroll position.
    fn nav_position(&self) -> Option<(usize, usize, (f32, f32))> {
        None
    }
    /// Return to a history position; returns whether anything moved.
    fn navigate_to(&mut self, _cursor: usize, _scroll: (f32, f32)) -> bool {
        false
    }
    /// Items that draw their own body (a terminal) paint it into `body` (logical px) here instead of going
    /// through the text editor's layout.
    fn paint_body(&mut self, _body: ui::Rect, _focused: bool) -> Option<ui::Painted> {
        None
    }
    /// A popover floated over the body, drawn as a separate layer so the body's text stays beneath it.
    fn paint_popover(&mut self, _body: ui::Rect) -> Option<ui::Painted> {
        None
    }
    /// Whether key presses should reach this item raw (`keystroke`) rather than as editor commands.
    fn wants_keystrokes(&self) -> bool {
        false
    }
    fn keystroke(&mut self, _keystroke: &terminal::Keystroke) -> TerminalKeyOutcome {
        TerminalKeyOutcome::Ignored
    }
    /// Pointer input in window coordinates, for items that paint their own body.
    fn pointer_down(
        &mut self,
        _x: f32,
        _y: f32,
        _click_count: u32,
        _modifiers: terminal::Modifiers,
    ) -> bool {
        false
    }
    fn pointer_drag(&mut self, _x: f32, _y: f32, _modifiers: terminal::Modifiers) -> bool {
        false
    }
    fn pointer_move(
        &mut self,
        _x: f32,
        _y: f32,
        _modifiers: terminal::Modifiers,
        _focused: bool,
    ) -> bool {
        false
    }
    fn pointer_up(&mut self, _x: f32, _y: f32, _modifiers: terminal::Modifiers) {}
    /// A sideways scroll (trackpad) over a self-painted body; returns whether it moved.
    fn pointer_scroll_x(&mut self, _x: f32, _y: f32, _delta_x: f32) -> bool {
        false
    }
    fn pointer_scroll(
        &mut self,
        _x: f32,
        _y: f32,
        _delta_y: f32,
        _modifiers: terminal::Modifiers,
    ) -> bool {
        false
    }
    /// Background work to bring in before drawing (a shell's output); `close` asks for the tab to close.
    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        ItemTick::default()
    }
    /// Things the tab asks the workspace to do (open another tab, a file); drained every frame.
    fn take_requests(&mut self) -> Vec<PanelRequest> {
        Vec::new()
    }
    fn take_open_request(&mut self) -> Option<TerminalOpenTarget> {
        None
    }
    fn link_hovered(&self) -> bool {
        false
    }
    fn render(&mut self) -> Node;
    fn cursor_status(&self) -> Option<String> {
        None
    }
    fn language_name(&self) -> Option<&'static str> {
        None
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
    /// The file this item shows, for tab actions like copying its path.
    fn abs_path(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn closed(&mut self) {}
    /// This item's state to restore it from next session; `None` for items that are not restored.
    fn serialize(&self) -> Option<persistence::SerializedItem> {
        None
    }
    fn clone_on_split(&self) -> Option<Box<dyn Item>> {
        None
    }

    fn is_editable(&self) -> bool {
        false
    }
    /// Has a rendered preview (a markdown file); its pane then shows a preview button.
    fn previewable(&self) -> bool {
        false
    }
    /// A diff's layout (`Some(true)` side by side, `Some(false)` unified); `None` for anything else.
    fn diff_split(&self) -> Option<bool> {
        None
    }
    /// Kept first and pinned in its pane (a workspace's main agent).
    fn pinned_at_front(&self) -> bool {
        false
    }
    /// Muted text after the title on its tab (a service's repo).
    fn tab_detail(&self) -> Option<String> {
        None
    }
    /// A status dot drawn before the title on its tab, in this color.
    fn tab_dot(&self) -> Option<ui::Rgba> {
        None
    }
    /// A row the item draws under the tabs (a service's controls and facts), `width` wide; its click ids are the
    /// item's own and come back through `toolbar_click`.
    fn toolbar(&self, _width: f32) -> Option<ui::Node> {
        None
    }
    /// A click on one of the item's toolbar ids; false when the id is not its own.
    fn toolbar_click(&mut self, _id: u64) -> bool {
        false
    }
    /// The menu a toolbar click asked to open under the button it hit, once.
    fn take_menu_request(&mut self) -> Option<Vec<MenuItem>> {
        None
    }
    /// An entry picked from the menu `take_menu_request` opened.
    fn menu_pick(&mut self, _id: u64) {}
    /// Whether a toolbar click asked for the pane's find bar, once.
    fn take_find_request(&mut self) -> bool {
        false
    }
    /// Whether a diff has room to lay out side by side at its current width.
    fn diff_split_room(&self) -> bool {
        true
    }
    /// Lines a diff adds and removes, for its toolbar.
    fn diff_stat(&self) -> Option<(usize, usize)> {
        None
    }
    /// A terminal's selection, recent output and directory, for its context menu; `None` for anything else.
    fn terminal_context(&self) -> Option<TerminalContext> {
        None
    }
    fn terminal_command(&mut self, _command: TerminalCommand) {}
    fn set_focused(&mut self, _focused: bool) {}
    fn input_text(&mut self, _text: &str) {}
    /// An input method's in-progress text; `selected` is its caret inside `text`, in chars.
    fn ime_preedit(&mut self, _text: &str, _selected: Option<std::ops::Range<usize>>) {}
    fn ime_commit(&mut self, text: &str) {
        self.input_text(text);
    }
    fn copy(&self) -> Option<CopiedText> {
        self.selected_text().map(|text| CopiedText {
            text,
            slices: Vec::new(),
        })
    }
    fn copy_trimmed(&self) -> Option<CopiedText> {
        self.copy()
    }
    fn cut(&mut self) -> Option<CopiedText> {
        None
    }
    /// Insert clipboard text verbatim (no auto-closing brackets); `slices` come from our own copy.
    fn paste(&mut self, text: &str, _slices: Option<&[ClipboardSlice]>) {
        self.input_text(text);
    }
    fn input_key(&mut self, _key: EditKey, _shift: bool) {}
    fn place_cursor(&mut self, _local_x: f32, _local_y: f32, _extend: bool) {}
    /// A click with the multi-cursor modifier: adds a caret there, or drops the one already there.
    fn toggle_cursor_at(&mut self, local_x: f32, local_y: f32) {
        self.place_cursor(local_x, local_y, false);
    }
    /// A selection drag to window point `(x, y)` over the body at `body`; items that scroll may autoscroll when
    /// the point is near or past an edge.
    fn drag_select(&mut self, x: f32, y: f32, body: ui::Rect) {
        self.place_cursor((x - body.x).max(0.0), y - body.y, true);
    }
    /// A cmd-click at a body point; returns whether the item took it (go to definition, open a link).
    fn cmd_click(&mut self, _local_x: f32, _local_y: f32) -> bool {
        false
    }
    fn select_word_at(&mut self, _local_x: f32, _local_y: f32) {}
    fn selected_text(&self) -> Option<String> {
        None
    }
    fn set_body_height(&mut self, _h: f32) {}
    fn scroll_by(&mut self, _dy: f32) -> bool {
        false
    }
    /// A small overview of the whole text beside the scrollbar, drawn over the body.
    fn minimap(&mut self, _content: ui::Rect) -> Option<ui::Painted> {
        None
    }
    /// A press at a body point on the minimap: it scrolls there and starts a drag; false off the minimap.
    fn minimap_press(&mut self, _local_x: f32, _local_y: f32) -> bool {
        false
    }
    /// The carets over the body: bars or boxes, a gliding caret's trail, a block caret's letter.
    fn carets(&mut self, _content: ui::Rect) -> ui::Painted {
        ui::Painted::default()
    }
    fn back_rects(&self, _content: ui::Rect) -> Vec<ui::Rect> {
        Vec::new()
    }
    fn selection_tris(&self, _content: ui::Rect) -> Vec<ui::Tri> {
        Vec::new()
    }
    fn scrollbar(&self, _content: ui::Rect) -> Vec<ui::Rect> {
        Vec::new()
    }
    fn body_y_offset(&self) -> f32 {
        0.0
    }

    fn gutter(&mut self, _fold_base: u64) -> Option<Node> {
        None
    }
    fn gutter_w(&self) -> f32 {
        0.0
    }
    fn toggle_fold(&mut self, _line: usize) {}
    /// A popover at the caret (the word menu) and its window position, given the text area.
    /// Documentation of the selected completion, beside the menu; `viewport` is the window size.
    fn completion_aside(
        &self,
        _content: ui::Rect,
        _viewport: (f32, f32),
    ) -> Option<(Node, f32, f32)> {
        None
    }
    fn scroll_completion_aside(&mut self, _dy: f32) -> bool {
        false
    }
    fn completion_popover(&self, _content: ui::Rect) -> Option<(Node, f32, f32)> {
        None
    }
    /// A click on row `row` of the word menu.
    fn click_completion(&mut self, _row: usize) {}
    /// A click on one of this item's popovers (completion rows, hover cards); returns whether it was one.
    fn popover_click(&mut self, _id: u64) -> bool {
        false
    }
    fn hover_completion(&mut self, _id: Option<u64>) {}
    fn pointer_moved(&mut self, _local: Option<(f32, f32)>, _window: (f32, f32)) -> bool {
        false
    }
    fn hover_popovers(&self, _content: ui::Rect) -> Vec<(Node, f32, f32)> {
        Vec::new()
    }
    fn scroll_hover(&mut self, _index: usize, _dy: f32) -> bool {
        false
    }
    fn diagnostic_message(&self) -> Option<String> {
        None
    }
    fn scroll_completion(&mut self, _dy: f32) -> bool {
        false
    }
    /// Expand or collapse the uncommitted change at `line`.
    fn toggle_diff_hunk(&mut self, _line: usize) {}
    /// Whether the pointer is over this item's gutter; returns whether that changed.
    fn set_gutter_hovered(&mut self, _hovered: bool) -> bool {
        false
    }
    fn save(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn is_dirty(&self) -> bool {
        false
    }
    /// The body is text in its own font, sized apart from the UI font: it lays out at a UI scale of 1.
    fn fixed_scale(&self) -> bool {
        false
    }
    /// What the tab's menu calls ending the program behind it, when closing the tab only hides it.
    fn stop_label(&self) -> Option<&'static str> {
        None
    }
    fn stop(&mut self) {}
    fn has_conflict(&self) -> bool {
        false
    }
    fn refresh_disk_state(&mut self) {}
    fn is_busy(&self) -> bool {
        false
    }
    /// Wants every frame (not just the busy tick rate) for an animation of its own.
    fn animating(&self) -> bool {
        false
    }
    fn right_press(&mut self, _local_x: f32, _local_y: f32) {}
    fn buffer_line_at(&self, _local_y: f32) -> Option<usize> {
        None
    }
    fn line_screen_y(&self, _content: ui::Rect, _line: usize) -> Option<f32> {
        None
    }
    fn body_x_offset(&self) -> f32 {
        0.0
    }
    fn set_body_width(&mut self, _w: f32) {}
    fn scroll_by_x(&mut self, _dx: f32) -> bool {
        false
    }
    fn h_scrollbar(&self, _content: ui::Rect) -> Vec<ui::Rect> {
        Vec::new()
    }
    /// Width to keep at the left of a body `body_w` wide for a companion view (the old side of a split
    /// diff); the text body gets the rest.
    fn companion_width(&mut self, _body_w: f32) -> f32 {
        0.0
    }
    /// The companion view, painted into `area` (left of the text body, scrolled with it).
    fn paint_companion(&mut self, _area: ui::Rect) -> Option<ui::Painted> {
        None
    }
    fn footer_height(&mut self, _body_h: f32) -> f32 {
        0.0
    }
    fn paint_footer(&mut self, _area: ui::Rect) -> Option<ui::Painted> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRequest {
    pub text: String,
    pub selection: Option<String>,
    pub caret: usize,
    pub all: bool,
}

pub trait ItemFooter: 'static {
    fn height(&mut self, body_h: f32) -> f32;
    fn paint(&mut self, area: ui::Rect) -> Option<ui::Painted>;
    fn pointer_down(&mut self, x: f32, y: f32, click_count: u32) -> bool;
    fn pointer_drag(&mut self, _x: f32, _y: f32) -> bool {
        false
    }
    fn pointer_up(&mut self) {}
    fn pointer_move(&mut self, _x: f32, _y: f32) -> bool {
        false
    }
    fn scroll(&mut self, _delta_x: f32, _delta_y: f32, _shift: bool) -> bool {
        false
    }
    fn key(&mut self, _key: EditKey, _shift: bool) -> bool {
        false
    }
    fn copy(&self) -> Option<String> {
        None
    }
    fn run(&mut self, request: RunRequest);
    fn take_run(&mut self) -> Option<bool> {
        None
    }
    fn serialize(&self) -> Option<persistence::SerializedItem> {
        None
    }
    fn text_changed(&mut self, _text: &str) {}
    fn tick(&mut self) -> bool {
        false
    }
    fn busy(&self) -> bool {
        false
    }
    fn title(&self) -> Option<String> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
    SubwordLeft,
    SubwordRight,
    /// Home/End ignoring soft wraps.
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteSubwordLeft,
    DeleteSubwordRight,
    DeleteToLineStart,
    DeleteToLineEnd,
    Enter,
    Tab,
    Backtab,
    Outdent,
    Indent,
    ToggleComments,
    DeleteLine,
    DuplicateLineUp,
    DuplicateLineDown,
    MoveLineUp,
    MoveLineDown,
    JoinLines,
    Transpose,
    ToggleGoToLine,
    ToggleCommandPalette,
    ToggleFileFinder,
    DeployProjectSearch,
    /// Open the project diagnostics tab; from the status bar it shows warnings when there are no errors.
    DeployDiagnostics,
    DeployDiagnosticsFromStatus,
    OpenMarkdownPreview,
    OpenMarkdownPreviewToTheSide,
    ToggleIncludeIgnored,
    ToggleOutline,
    NewCenterTerminal,
    TogglePickerPreview,
    SetPickerPreviewRight,
    GoBack,
    GoForward,
    DeploySearch,
    ToggleSearchReplace,
    SelectNextMatch,
    SelectPreviousMatch,
    SelectAllMatchesInSearch,
    ToggleSearchCaseSensitive,
    ToggleSearchWholeWord,
    ToggleSearchRegex,
    UseSelectionForFind,
    ReplaceAll,
    SelectNext,
    SelectAllMatches,
    /// Add a caret on the next buffer line above/below (skipping soft-wrapped rows).
    AddCursorAbove,
    AddCursorBelow,
    /// Add a caret on the next display row above/below.
    AddCursorAboveRow,
    AddCursorBelowRow,
    SelectLargerSyntaxNode,
    SelectSmallerSyntaxNode,
    MoveToEnclosingBracket,
    GoToHunk,
    GoToDiagnostic,
    GoToPreviousDiagnostic,
    GoToDefinition,
    GoToDeclaration,
    GoToTypeDefinition,
    GoToImplementation,
    GoToPreviousHunk,
    /// Switch a diff between one column and two.
    ToggleSplitDiff,
    GitRestore,
    ToggleStaged,
    StageAndNext,
    UnstageAndNext,
    ToggleSelectedDiffHunks,
    ExpandAllDiffHunks,
    ShowCompletions,
    ShowWordCompletions,
    Hover,
    Undo,
    Redo,
    SelectAll,
    Escape,
    ToggleSoftWrap,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DividerAxis {
    Horizontal,
    Vertical,
}

pub struct PanePlacement {
    pub rect: ui::Rect,
    pub node: Node,
    /// The tabs: laid out at the first rect (scrolled), shown only inside the second.
    pub strip: Option<(Node, ui::Rect, ui::Rect)>,
    /// A body the item painted itself (a terminal), drawn clipped to the area below the chrome.
    pub painted: Option<(ui::Painted, ui::Rect)>,
    /// A text body's companion view (the old side of a split diff), drawn over the body's left.
    pub companion: Option<(ui::Painted, ui::Rect)>,
    pub footer: Option<(ui::Painted, ui::Rect)>,
    /// A popover the item floats over its body, drawn as its own layer after the body.
    pub popover: Option<(ui::Painted, ui::Rect)>,
    pub body: Option<PaneBody>,
    pub back: Vec<ui::Rect>,
    pub back_tris: Vec<ui::Tri>,
    pub carets: ui::Painted,
    pub minimap: Option<ui::Painted>,
    pub scrollbar: Vec<ui::Rect>,
    pub h_scrollbar: Vec<ui::Rect>,
}

pub struct PaneBody {
    pub node: Node,
    pub rect: ui::Rect,
    /// The item's fill under the text, so a body looks the same in any dock.
    pub background: ui::Rgba,
    pub y_offset: f32,
    pub text_left: f32,
    pub x_offset: f32,
    pub text_clip: ui::Rect,
    pub gutter: Option<Node>,
    pub gutter_clip: ui::Rect,
    /// The UI text scale the body and gutter lay out at: 1 for the editor, whose text follows its own font.
    pub scale: Option<f32>,
}

pub struct DividerPlacement {
    pub rect: ui::Rect,
    pub id: u64,
    pub axis: DividerAxis,
}

#[derive(Default)]
pub struct EditorLayout {
    pub panes: Vec<PanePlacement>,
    pub dividers: Vec<DividerPlacement>,
}

/// How high a surface floats, which sets its shadow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// Popovers and small floating panels.
    Elevated,
    /// Modals that take over input, such as pickers.
    Modal,
}

pub struct ModalView {
    pub node: Node,
    /// Width in design px.
    pub width: f32,
    pub elevation: Elevation,
}

/// What a key did in the focused terminal. Copy and Paste need the system clipboard, which the workspace owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalKeyOutcome {
    Ignored,
    Handled,
    Copy(String),
    Paste,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalContext {
    pub selection: Option<String>,
    /// The last lines of output, for asking about it when nothing is selected.
    pub recent: String,
    pub cwd: std::path::PathBuf,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalCommand {
    Paste(String),
    SelectAll,
    Clear,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemTick {
    pub changed: bool,
    pub clipboard_store: Option<String>,
    pub close: bool,
}

/// Something a feature view asks the workspace to do that it can't do itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewRequest {
    NewCenterTerminal,
}

/// A cmd-clicked link from the terminal: a URL for the browser, or an existing file (1-based position).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalOpenTarget {
    Url(String),
    Path {
        path: std::path::PathBuf,
        row: Option<u32>,
        column: Option<u32>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalSyncOutcome {
    pub changed: bool,
    pub clipboard_store: Option<String>,
    /// The last terminal exited, so the panel should close.
    pub closed_all: bool,
    /// Text a tab sends to another tab's prompt (a side agent's answer for the main agent): (item id, text).
    pub send: Vec<(String, String)>,
}

/// The terminal panel as the workspace drives it: drawn into whichever area shows the terminal, fed pointer and
/// keyboard input while focused, and synced with its shells on every frame.
pub trait TerminalPanelView: 'static {
    /// The panel's panes; pointer and key input reaches its items through them.
    fn panes(&mut self) -> &mut pane_group_view::PaneGroupView;
    fn panes_ref(&self) -> &pane_group_view::PaneGroupView;
    /// A backdrop for the region (a message when there is nothing to show) and the panes to draw over it.
    fn render(&mut self, region: Rect, focused: bool) -> (ui::Painted, EditorLayout);
    /// The panes and their terminals as saved state, and rebuilding them from it (a new shell in each saved
    /// terminal's directory).
    fn save_panes(&self) -> persistence::SerializedMember;
    fn restore_panes(&mut self, saved: &persistence::SerializedMember) -> bool;
    /// Clicks on ids for which `is_terminal_id` holds.
    fn click(&mut self, id: u64) -> bool;
    /// The hit id under the pointer (tabs reveal their close button while hovered); returns whether to repaint.
    fn set_hover(&mut self, id: Option<u64>) -> bool;
    fn divider_axis(&self, id: u64) -> Option<DividerAxis>;
    fn drag_divider(&mut self, id: u64, x: f32, y: f32) -> bool;
    fn is_tab(&self, id: u64) -> bool;
    fn begin_tab_drag(&mut self, id: u64) -> bool;
    /// `over` is the hit under the pointer with its rect, so a tab under it can take the drop.
    fn update_tab_drag(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool;
    fn drop_tab(&mut self) -> bool;
    fn tab_drag_overlay(&self) -> Option<Rect>;
    fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)>;
    /// Returns false when the command had nothing to act on (no pane in that direction, ...).
    fn pane_command(&mut self, command: pane::PaneCommand) -> bool;
    /// Moving a tab across pane groups: the item being dragged here, taking it out, whether this view takes
    /// such an item, where it would land when the pointer is over this view, and placing it.
    fn dragged_item(&self) -> Option<&dyn Item>;
    fn take_dragged_item(&mut self) -> Option<Box<dyn Item>>;
    fn accepts_item(&self, item: &dyn Item) -> bool;
    fn update_foreign_drop(
        &mut self,
        x: f32,
        y: f32,
        over: Option<(u64, Rect)>,
        item: &dyn Item,
    ) -> bool;
    fn clear_foreign_drop(&mut self);
    fn accept_foreign_item(&mut self, item: Box<dyn Item>);
    fn focus_changed(&mut self, focused: bool);
    fn sync(&mut self, clipboard: &dyn Fn() -> Option<String>) -> TerminalSyncOutcome;
    /// The "+" of the agent dock was pressed: a side agent is wanted, once.
    fn take_new_agent_request(&mut self) -> bool {
        false
    }
    /// The agent dock's history button was pressed, once.
    fn take_history_request(&mut self) -> bool {
        false
    }
    /// Shows the tab `id` and types `text` into its prompt without sending; false when there is no such tab.
    fn paste_into(&mut self, _id: &str, _text: &str) -> bool {
        false
    }
    /// Types `text` into tab `id`'s prompt and sends it; false when there is no such tab.
    fn submit_into(&mut self, _id: &str, _text: &str) -> bool {
        false
    }
    /// Start a shell in `cwd` (the project root when `None`) as a new active tab.
    fn open(&mut self, cwd: Option<std::path::PathBuf>);
    fn is_empty(&self) -> bool;
    fn take_open_request(&mut self) -> Option<TerminalOpenTarget>;
    /// Whether the pointer is over a link that a click would open (pointing-hand cursor).
    fn link_hovered(&self) -> bool;
    /// A new shell as a pane item, for placing outside the panel.
    fn new_item(&mut self, cwd: Option<std::path::PathBuf>) -> Option<Box<dyn Item>>;
    /// A terminal running `argv` in `cwd` with `env` added, titled `title`.
    fn command_item(
        &mut self,
        _title: String,
        _cwd: std::path::PathBuf,
        _argv: Vec<String>,
        _env: Vec<(String, String)>,
    ) -> Option<Box<dyn Item>> {
        None
    }
}

/// Pointer, keyboard and clipboard input for the items of a pane group (the editor area, the terminal panel),
/// and what their bodies show around the caret.
pub trait ItemInput {
    fn editor_key(&mut self, key: EditKey, shift: bool) -> bool;
    fn editor_text(&mut self, text: &str) -> bool;
    fn editor_paste(&mut self, text: &str, slices: Option<&[ClipboardSlice]>) -> bool;
    fn editor_ime_preedit(&mut self, text: &str, selected: Option<std::ops::Range<usize>>) -> bool;
    fn editor_ime_commit(&mut self, text: &str) -> bool;
    fn editor_click(&mut self, x: f32, y: f32, extend: bool) -> bool;
    fn editor_double_click(&mut self, x: f32, y: f32) -> bool;
    /// Extend the selection to a drag point, which may lie outside the pane.
    fn editor_drag(&mut self, x: f32, y: f32) -> bool;
    /// The button went up after an `editor_click`.
    fn editor_release(&mut self) {}
    /// The pointer moved (no button held); returns whether anything hover-dependent changed.
    fn editor_hover(&mut self, x: f32, y: f32) -> bool;
    fn editor_scroll(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool;
    fn editor_copy(&self) -> Option<CopiedText>;
    fn editor_cut(&mut self) -> Option<CopiedText>;
    fn editor_copy_trimmed(&self) -> Option<CopiedText>;
    fn editor_selected_text(&self) -> Option<String>;
    fn editor_right_press(&mut self, x: f32, y: f32) -> bool;
    fn editor_menu_anchor_at(&self, x: f32, y: f32) -> Option<(Vec<usize>, usize)>;
    /// For a diff tab, whether it shows two columns; `None` for anything else.
    fn editor_split_diff(&self) -> Option<bool> {
        None
    }
    fn editor_menu_y(&self, path: &[usize], line: usize) -> Option<f32>;
    fn editor_save(&mut self) -> Option<Result<(), String>>;
    fn editor_focused(&self) -> bool;
    fn cursor_position(&self) -> Option<String>;
    fn active_language(&self) -> Option<&'static str>;
    /// Popovers at the caret (completions) and under the pointer (hover), placed for a window of `viewport`.
    fn editor_popovers(&mut self, viewport: (f32, f32)) -> Vec<(Node, f32, f32)>;
    /// A wheel or trackpad scroll over popover `index`.
    fn popover_scroll(&mut self, index: usize, dy: f32) -> bool;
    fn popover_click(&mut self, id: u64) -> bool;
    /// The hit id under the pointer while it is over one of this group's popovers.
    fn popover_hover(&mut self, id: Option<u64>);
    /// Whether the focused item takes raw key presses (a terminal).
    fn active_wants_keystrokes(&self) -> bool;
    fn item_keystroke(&mut self, keystroke: &terminal::Keystroke) -> TerminalKeyOutcome;
    fn item_text(&mut self, text: &str);
    fn item_paste(&mut self, text: &str);
    fn item_focus_changed(&mut self, focused: bool);
    fn item_pointer_down(
        &mut self,
        x: f32,
        y: f32,
        click_count: u32,
        modifiers: terminal::Modifiers,
    ) -> bool;
    fn item_pointer_drag(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) -> bool;
    fn item_pointer_move(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) -> bool;
    fn item_pointer_up(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers);
    fn item_pointer_scroll(
        &mut self,
        x: f32,
        y: f32,
        delta: (f32, f32),
        modifiers: terminal::Modifiers,
    ) -> bool;
}

/// A command the window adds to the command palette: its name, its binding as keystrokes (`cmd-k`, `cmd-s`)
/// and the id the palette hands back when it is chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtraCommand {
    pub name: String,
    pub keys: Vec<String>,
    pub id: u64,
}

pub trait FunctionView: ItemInput + 'static {
    /// The window's own commands, listed in the command palette next to the editor's.
    fn set_extra_commands(&mut self, _commands: Vec<ExtraCommand>) {}
    /// The editor font changed; open files lay out again.
    fn editor_metrics_changed(&mut self) {}
    /// Opens the palette with only `commands` (a switcher), showing `placeholder` in its query.
    fn open_command_list(&mut self, _commands: Vec<ExtraCommand>, _placeholder: &'static str) {}
    /// The window command picked in the palette, once.
    fn take_extra_command(&mut self) -> Option<u64> {
        None
    }
    /// Once a frame before layout: bring the project's services (language servers, ...) up to date with the
    /// items open here and in `other`, another pane group such as the terminal panel.
    fn sync_items(&mut self, _other: Option<&mut pane_group_view::PaneGroupView>) {}
    fn editor_layout(&mut self, area: ui::Rect) -> EditorLayout;
    fn render_tree(&mut self) -> Option<TreePanel> {
        None
    }
    /// A modal floating over the window (e.g. go to line), given the window size.
    fn modal(&mut self, _viewport: (f32, f32)) -> Option<ModalView> {
        None
    }
    fn dismiss_modal(&mut self) {}
    fn diagnostic_summary(&self) -> Option<DiagnosticSummary> {
        None
    }
    /// The folder's language servers; `details` also measures what each uses, for the open menu.
    fn language_servers(&self, _details: bool) -> Option<LanguageServers> {
        None
    }
    fn language_server_action(&mut self, _action: LanguageServerAction) {}
    fn tree_menu_state(&self, _path: Option<&str>) -> TreeMenuState {
        TreeMenuState::default()
    }
    fn tree_action(&mut self, _path: Option<&str>, _action: TreeAction) -> Option<Prompt> {
        None
    }
    fn prompt_answered(&mut self, _token: u64, _answer: usize) {}
    fn take_toast(&mut self) -> Option<String> {
        None
    }
    fn take_request(&mut self) -> Option<ViewRequest> {
        None
    }
    /// A menu an item's toolbar asked to open, once.
    fn take_menu_request(&mut self) -> Option<Vec<MenuItem>> {
        None
    }
    /// An entry picked from that menu, for the item that opened it.
    fn menu_pick(&mut self, _id: u64) {}
    /// Place an item as a new tab in the focused pane.
    fn add_center_item(&mut self, _item: Box<dyn Item>) {}
    /// Tick every item that works in the background; closes the tabs that asked to close.
    fn tick_items(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        ItemTick::default()
    }
    fn take_item_open_request(&mut self) -> Option<TerminalOpenTarget> {
        None
    }
    fn item_link_hovered(&self) -> bool {
        false
    }
    fn active_file_path(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// Open an absolute path as a diff against `base` (every change expanded), beside its plain tab.
    fn open_diff(&mut self, _path: &std::path::Path, _base: Option<String>) {}
    /// Open an absolute path in the editor, placing the caret at a 1-based row/column when given.
    fn open_file_at(&mut self, _path: &std::path::Path, _row: Option<u32>, _column: Option<u32>) {}
    /// A wheel or trackpad scroll over the open modal; returns whether it moved.
    fn modal_scroll(&mut self, _dy: f32) -> bool {
        false
    }
    fn on_click(&mut self, _id: u64) -> bool {
        false
    }
    fn on_scroll(&mut self, _dx: f32, _dy: f32, _vw: f32, _vh: f32) -> bool {
        false
    }
    fn scroll_offset(&self) -> f32 {
        0.0
    }
    fn content_height(&self) -> f32 {
        0.0
    }
    fn set_viewport(&mut self, _w: f32, _h: f32) {}
    fn divider_axis(&self, _id: u64) -> Option<DividerAxis> {
        None
    }
    fn drag_divider(&mut self, _id: u64, _dx: f32, _dy: f32) -> bool {
        false
    }
    fn set_hover(&mut self, _id: Option<u64>) -> bool {
        false
    }
    fn is_tab(&self, _id: u64) -> bool {
        false
    }
    fn begin_tab_drag(&mut self, _id: u64) -> bool {
        false
    }
    /// `over` is the hit under the pointer with its rect, so a tab under it can take the drop.
    fn update_tab_drag(&mut self, _x: f32, _y: f32, _over: Option<(u64, ui::Rect)>) -> bool {
        false
    }
    fn drop_tab(&mut self) -> bool {
        false
    }
    fn cancel_tab_drag(&mut self) {}
    fn dragging_tab(&self) -> bool {
        false
    }
    fn tab_drag_overlay(&self) -> Option<ui::Rect> {
        None
    }
    fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)> {
        None
    }
    /// Moving a tab across pane groups: the item being dragged here, taking it out, whether this view takes
    /// such an item, where it would land when the pointer is over this view, and placing it.
    /// False while a modal (picker, palette, inline rename) owns the keyboard, so pane keys fall through to it.
    /// The editor area's panes, for actions that work on any pane group (the tab context menu).
    fn pane_group(&self) -> Option<&pane_group_view::PaneGroupView> {
        None
    }

    fn pane_group_mut(&mut self) -> Option<&mut pane_group_view::PaneGroupView> {
        None
    }

    /// Show `path` in the file tree: expand its folders and scroll its row into view.
    fn reveal_in_tree(&mut self, _path: &std::path::Path) {}

    /// Whether the editor area's zoomed pane is showing (it is zoomed and focused).
    fn zoom_shown(&self) -> bool {
        false
    }

    /// Whether this view handles `key` itself wherever focus is (a modal is open, or the key opens one).
    fn claims_key(&self, _key: EditKey) -> bool {
        false
    }

    fn accepts_pane_keys(&self) -> bool {
        false
    }

    /// The editor area's panes and tabs as saved state, and rebuilding them from it.
    fn save_panes(&self) -> Option<persistence::SerializedMember> {
        None
    }

    fn restore_panes(
        &mut self,
        _saved: &persistence::SerializedMember,
        _fallback: &mut dyn FnMut(&persistence::SerializedItem) -> Option<Box<dyn Item>>,
    ) -> bool {
        false
    }

    fn pane_command(&mut self, _command: pane::PaneCommand) -> bool {
        false
    }

    fn dragged_item(&self) -> Option<&dyn Item> {
        None
    }
    fn take_dragged_item(&mut self) -> Option<Box<dyn Item>> {
        None
    }
    fn accepts_item(&self, _item: &dyn Item) -> bool {
        false
    }
    fn update_foreign_drop(
        &mut self,
        _x: f32,
        _y: f32,
        _over: Option<(u64, ui::Rect)>,
        _item: &dyn Item,
    ) -> bool {
        false
    }
    fn clear_foreign_drop(&mut self) {}
    fn accept_foreign_item(&mut self, _item: Box<dyn Item>) {}

    fn row_path(&self, _id: u64) -> Option<(String, bool)> {
        None
    }
    fn root_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn open_path(&mut self, _path: &str) {}
    fn refresh_disk_state(&mut self) {}
    fn is_busy(&self) -> bool {
        false
    }
    fn animating(&self) -> bool {
        false
    }
}

/// The session switcher popover, split so the app can scroll the list smoothly: `fixed` (shadow, panel, search,
/// footer, scrollbar) draws unclipped; `list` (the scrolling rows, already offset by scroll) draws clipped to
/// `clip`. `content_h`/`region_h` (logical px) let the app clamp the scroll.
pub struct SessionMenu {
    pub fixed: Painted,
    pub list: Painted,
    pub clip: (f32, f32, f32, f32),
    pub content_h: f32,
    pub region_h: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            side_agent_sizes: [None; 4],
            main_agent_tokens: None,
            archived_agents: Vec::new(),
            other_cli: None,
            left: Dock {
                position: DockPosition::Left,
                width: 260.0,
                collapsed: false,
                live: None,
                panel: Box::new(ProjectPanel::default()),
            },
            right: Dock {
                position: DockPosition::Right,
                width: 300.0,
                collapsed: true,
                live: None,
                panel: Box::new(AgentEmptyPanel),
            },
            bottom: Dock {
                position: DockPosition::Bottom,
                width: 220.0, // height for the bottom dock
                collapsed: false,
                live: None,
                panel: Box::new(TerminalPanel),
            },
            sessions: Vec::new(),
            machine: Vec::new(),
            usage: UsageInfo::default(),
            update: UpdateInfo::default(),
            app_menu_open: false,
            usage_card_open: false,
            usage_today_open: false,
            project: None,
            sidebar_side: DockPosition::Left,
            agent_side: DockPosition::Right,
            agent_hidden: false,
            terminal_hidden: false,
            terminal_side: DockPosition::Bottom,
            // Left area shows Files, the bottom dock shows the terminal, the right dock is empty by default.
            active_panels: [
                Some(Shown::Func(PaneKind::Files)),
                Some(Shown::Terminal),
                None,
            ],
            func_hidden: vec![false; PaneKind::ALL.len()],
            func_side: vec![DockPosition::Left; PaneKind::ALL.len()],
            current_session: None,
            session_menu: false,
            session_scroll: 0.0,
            fullscreen: false,
            files_view: None,
            language_servers: LanguageServers::default(),
            terminal_view: None,
            agent_view: None,
            services_running: false,
            side_panels: Vec::new(),
            agent_states: std::collections::HashMap::new(),
            files_tree_w: FILES_TREE_W,
            panels_collapsed: false,
        }
    }
}

/// Which panel a content area (center/right/bottom) currently shows: a workspace function, the terminal, or
/// the agent (the right dock's default panel — a panel like the others, not a bare open/close toggle).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shown {
    Func(PaneKind),
    Terminal,
    Agent,
}

impl Layout {
    pub fn left_w(&self) -> f32 {
        self.left.effective()
    }
    pub fn right_w(&self) -> f32 {
        let width = self.right.effective();
        if width <= 0.0 {
            return width;
        }
        width.clamp(SIDE_PANEL_MIN, DOCK_MAX)
    }
    /// The bottom dock's current height (0 when collapsed).
    pub fn bottom_h(&self) -> f32 {
        self.bottom.effective()
    }

    /// Whether the WORKSPACES sidebar docks on the left (default) vs the right.
    pub fn sidebar_left(&self) -> bool {
        self.sidebar_side != DockPosition::Right
    }

    /// Whether the function at `PaneKind::ALL` index `i` has a button and a panel: not hidden by the user, and
    /// meaningful here (main is the base every branch compares against, so it has no Git panel).
    pub fn func_offered(&self, i: usize) -> bool {
        if self.func_hidden.get(i).copied().unwrap_or(false) {
            return false;
        }
        let on_main = self.project.as_ref().is_some_and(|project| {
            project
                .workspaces
                .first()
                .is_some_and(|main| *main == project.active)
        });
        !(on_main && PaneKind::ALL.get(i) == Some(&PaneKind::Git))
    }

    /// The panels assigned to `side`, in a stable order (functions first, then the terminal). These are the
    /// "tabs" of that dock; one of them is the active/visible panel.
    pub fn candidates_on(&self, side: DockPosition) -> Vec<Shown> {
        let mut v = Vec::new();
        for (i, kind) in PaneKind::ALL.iter().enumerate() {
            if self.func_offered(i)
                && self.func_side.get(i).copied().unwrap_or(DockPosition::Left) == side
            {
                v.push(Shown::Func(*kind));
            }
        }
        if !self.terminal_hidden && self.terminal_side == side {
            v.push(Shown::Terminal);
        }
        // Listed last so functions and the terminal take priority when the agent is not the active panel.
        if !self.agent_hidden && side == self.agent_side {
            v.push(Shown::Agent);
        }
        v
    }

    /// What renders on `side`: that side's active panel if it is still a valid candidate there, otherwise the
    /// first candidate. Each side chooses independently (the per-dock `active_panel_index`).
    pub fn shown_on(&self, side: DockPosition) -> Option<Shown> {
        let candidates = self.candidates_on(side);
        if let Some(active) = self.active_panels[side.index()] {
            if candidates.contains(&active) {
                return Some(active);
            }
        }
        candidates.first().copied()
    }

    /// Whether the content area on `side` is currently visible (the center is always open; docks obey collapse).
    pub fn dock_open(&self, side: DockPosition) -> bool {
        match side {
            DockPosition::Left => !self.panels_collapsed,
            DockPosition::Right => !self.right.collapsed,
            DockPosition::Bottom => !self.bottom.collapsed,
        }
    }

    /// Whether function `i` is the visible panel of its side (for the status-bar highlight).
    pub fn func_active(&self, i: usize) -> bool {
        let side = self.func_side.get(i).copied().unwrap_or(DockPosition::Left);
        self.dock_open(side) && self.shown_on(side) == Some(Shown::Func(PaneKind::ALL[i]))
    }

    /// Whether the terminal is currently the visible panel of its (open) side.
    /// The agent dock shows its agent sessions (not the empty placeholder).
    pub fn agent_visible(&self) -> bool {
        self.agent_shown()
            && self
                .agent_view
                .as_ref()
                .is_some_and(|view| !view.is_empty())
    }

    pub fn terminal_visible(&self) -> bool {
        self.dock_open(self.terminal_side)
            && self.shown_on(self.terminal_side) == Some(Shown::Terminal)
    }

    /// Keep dock state consistent: the bottom dock has no default panel, so it collapses when nothing is
    /// docked there (e.g. after the terminal moves to another side).
    pub fn sync_docks(&mut self) {
        if self.candidates_on(DockPosition::Bottom).is_empty() {
            self.bottom.collapsed = true;
        }
        // Likewise the right dock once its last panel (often the agent) has moved away.
        if self.candidates_on(DockPosition::Right).is_empty() {
            self.right.collapsed = true;
        }
    }

    pub fn files_side(&self) -> Option<DockPosition> {
        self.files_view.as_ref()?;
        let side = self
            .func_side
            .first()
            .copied()
            .unwrap_or(DockPosition::Left);
        (self.dock_open(side) && self.shown_on(side) == Some(Shown::Func(PaneKind::Files)))
            .then_some(side)
    }

    pub fn files_tree_active(&self) -> bool {
        self.files_side() == Some(DockPosition::Left)
    }

    pub fn left_column_active(&self) -> bool {
        !self.panels_collapsed
            && self.files_view.is_some()
            && matches!(
                self.shown_on(DockPosition::Left),
                Some(Shown::Func(_) | Shown::Agent)
            )
    }

    /// Whether the agent is the visible panel of its (open) dock, sessions or not.
    pub fn agent_shown(&self) -> bool {
        let side = self.agent_side;
        let open = match side {
            DockPosition::Left => self.left_column_active(),
            _ => self.dock_open(side),
        };
        open && self.shown_on(side) == Some(Shown::Agent)
    }

    /// Where the agent panel draws: the left column or the right dock.
    pub fn agent_region(&self, w: f32, h: f32) -> Rect {
        match self.agent_side {
            DockPosition::Left => self.tree_region(w, h),
            _ => self.right_region(w, h),
        }
    }

    pub fn side_panel_mut(&mut self, kind: PaneKind) -> Option<&mut Box<dyn SidePanelView>> {
        self.side_panels
            .iter_mut()
            .find(|panel| panel.kind() == kind)
    }

    pub fn side_panel_on(&mut self, side: DockPosition) -> Option<&mut Box<dyn SidePanelView>> {
        match self.shown_on(side) {
            Some(Shown::Func(kind)) if kind != PaneKind::Files && self.dock_open(side) => {
                self.side_panel_mut(kind)
            }
            _ => None,
        }
    }

    fn tree_w(&self) -> f32 {
        if self.left_column_active() {
            self.files_tree_w.clamp(self.tree_min(), FILES_TREE_MAX)
        } else {
            0.0
        }
    }

    fn tree_min(&self) -> f32 {
        SIDE_PANEL_MIN.min(FILES_TREE_MAX)
    }

    /// Left edge of the editor area (right of the sidebar when docked left; 0 when the sidebar is on the right).
    fn editor_l(&self) -> f32 {
        if self.sidebar_left() {
            self.left_w()
        } else {
            0.0
        }
    }

    fn block_l(&self) -> f32 {
        self.editor_l() + self.tree_w()
    }
    /// Right edge of the editor area (window edge when the sidebar is left; left of the sidebar when it's right).
    fn editor_r(&self, w: f32) -> f32 {
        if self.sidebar_left() {
            w
        } else {
            w - self.left_w()
        }
    }

    /// Drag the sidebar divider; width from the sidebar's outer edge (left: `x`, right: `w - x`).
    pub fn set_left_divider(&mut self, x: f32, w: f32) {
        let sw = if self.sidebar_left() { x } else { w - x };
        // The edge follows the pointer the whole way; below the list's minimum it shows as the rail.
        let live = sw.clamp(RAIL_W, DOCK_MAX);
        self.left.live = Some(live);
        self.left.collapsed = live < DOCK_MIN;
        if !self.left.collapsed {
            self.left.width = live;
        }
    }

    /// Ends a divider drag: a width between the rail and the list's minimum settles on whichever is nearer.
    pub fn finish_left_drag(&mut self) {
        let Some(live) = self.left.live.take() else {
            return;
        };
        self.left.collapsed = live < (RAIL_W + DOCK_MIN) / 2.0;
        if !self.left.collapsed {
            self.left.width = live.max(DOCK_MIN);
        }
    }

    /// Drag the agent divider; width from the window edge on its side (right: `w - x`, left: `x - left_w`).
    pub fn set_right_divider(&mut self, x: f32, w: f32) {
        let rw = self.editor_r(w) - x;
        if rw < DOCK_MIN - 20.0 {
            self.right.collapsed = true;
        } else {
            self.right.collapsed = false;
            self.right.width = rw.clamp(DOCK_MIN, DOCK_MAX);
        }
    }

    /// Drag the bottom divider; `y` is the pointer y, `h` the window height. Bottom height = content_bottom - y.
    pub fn set_bottom_divider(&mut self, y: f32, h: f32) {
        let bh = self.content_bottom(h) - y;
        if bh < BOTTOM_MIN - 20.0 {
            self.bottom.collapsed = true;
        } else {
            self.bottom.collapsed = false;
            self.bottom.width = bh.clamp(BOTTOM_MIN, BOTTOM_MAX);
        }
    }

    pub fn on_left_divider(&self, x: f32, y: f32, w: f32) -> bool {
        let edge = if self.sidebar_left() {
            self.left_w()
        } else {
            w - self.left_w()
        };
        y > TOP_BAR_H && (x - edge).abs() <= DIVIDER_HIT
    }
    pub fn on_right_divider(&self, x: f32, y: f32, w: f32) -> bool {
        if self.right_w() <= 0.0 {
            return false;
        }
        let edge = self.editor_r(w) - self.right_w();
        y > TOP_BAR_H && (x - edge).abs() <= DIVIDER_HIT
    }

    pub fn on_tree_divider(&self, x: f32, y: f32, _w: f32) -> bool {
        if !self.left_column_active() {
            return false;
        }
        let edge = self.editor_l() + self.files_tree_w.clamp(self.tree_min(), FILES_TREE_MAX);
        y > TOP_BAR_H && (x - edge).abs() <= DIVIDER_HIT
    }

    pub fn set_tree_divider(&mut self, x: f32) {
        self.files_tree_w = (x - self.editor_l()).clamp(self.tree_min(), FILES_TREE_MAX);
    }
    /// The bottom dock's top divider (only over the center x-range, when the dock is open).
    pub fn on_bottom_divider(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let bh = self.bottom_h();
        if bh <= 0.0 {
            return false;
        }
        let top = self.content_bottom(h) - bh;
        x > self.center_l() && x < self.center_r(w) && (y - top).abs() <= DIVIDER_HIT
    }

    pub fn build(&self, w: f32, h: f32) -> (Vec<Rect>, Vec<Text>) {
        let lw = self.left_w();
        let rw = self.right_w();
        let by = TOP_BAR_H;
        // Only the WORKSPACES panel is full height (`dock_h`); the editor-area panels sit above the status strip.
        let dock_h = (h - TOP_BAR_H).max(0.0);
        let content_bottom = h - STATUS_BAR_H;
        let mut out = Painted::default();
        let mut band = |node: Node, rect: Rect| {
            let p = render(&node, rect);
            out.rects.extend(p.rects);
            out.texts.extend(p.texts);
        };
        let editor_h = (content_bottom - by).max(0.0);
        let zl = self.editor_l();
        let zr = self.editor_r(w);

        let mut top = div().items_center().justify_center().bg(top_bar_c());
        if let Some(project) = self.project.as_ref().filter(|_| ui::chrome().branch) {
            top = top.child(label(project.active.clone()).size(14.0).color(text_dim_c()));
        }
        band(
            top.into(),
            Rect::new(0.0, 0.0, w, TOP_BAR_H, Rgba::TRANSPARENT),
        );

        let sx = if self.sidebar_left() { 0.0 } else { w - lw };
        let side_bg = if self.left.collapsed {
            rail_c()
        } else {
            dock_c()
        };
        band(
            div().bg(side_bg).into(),
            Rect::new(sx, by, lw, dock_h, Rgba::TRANSPARENT),
        );

        let tw = self.tree_w();
        if tw > 0.0 {
            band(
                div().bg(dock_c()).into(),
                Rect::new(zl, by, tw, editor_h, Rgba::TRANSPARENT),
            );
        }
        if rw > 0.0 {
            let ax = zr - rw;
            band(
                div().bg(dock_c()).into(),
                Rect::new(ax, by, rw, editor_h, Rgba::TRANSPARENT),
            );
        }

        let cx0 = self.center_l();
        let cw = (self.center_r(w) - cx0).max(0.0);
        let bd_h = self.bottom_h();
        if bd_h > 0.0 {
            band(
                div().bg(dock_c()).into(),
                Rect::new(cx0, content_bottom - bd_h, cw, bd_h, Rgba::TRANSPARENT),
            );
        }

        let sw = (zr - zl).max(0.0);
        band(
            div().bg(top_bar_c()).into(),
            Rect::new(zl, content_bottom, sw, STATUS_BAR_H, Rgba::TRANSPARENT),
        );

        (out.rects, out.texts)
    }

    pub fn border_lines(&self, w: f32, h: f32) -> Vec<Rect> {
        let by = TOP_BAR_H;
        let content_bottom = self.content_bottom(h);
        let dock_h = (h - TOP_BAR_H).max(0.0);
        let editor_h = (content_bottom - by).max(0.0);
        let lw = self.left_w();
        let rw = self.right_w();
        let tw = self.tree_w();
        let zl = self.editor_l();
        let zr = self.editor_r(w);
        let bl = zl + tw;
        let cx0 = self.center_l();
        let cw = (self.center_r(w) - cx0).max(0.0);
        let c = divider_c();
        let mut v = Vec::new();
        let sidebar_edge = if self.sidebar_left() { lw } else { w - lw };
        v.push(Rect::new(sidebar_edge - 1.0, by, 1.0, dock_h, c));
        if tw > 0.0 {
            v.push(Rect::new(bl - 1.0, by, 1.0, editor_h, c));
        }
        if rw > 0.0 {
            let edge = zr - rw;
            v.push(Rect::new(edge - 1.0, by, 1.0, editor_h, c));
        }
        let bd_h = self.bottom_h();
        if bd_h > 0.0 {
            v.push(Rect::new(cx0, content_bottom - bd_h - 1.0, cw, 1.0, c));
        }
        let sw = (zr - zl).max(0.0);
        v.push(Rect::new(zl, content_bottom - 1.0, sw, 1.0, c));
        v
    }

    /// Bottom edge of the center/bottom-dock content (above the fixed status strip).
    fn content_bottom(&self, h: f32) -> f32 {
        h - STATUS_BAR_H
    }

    fn center_l(&self) -> f32 {
        self.block_l()
    }

    /// Right edge of the center: the editor-area right, minus the right dock.
    fn center_r(&self, w: f32) -> f32 {
        self.editor_r(w) - self.right_w()
    }

    /// The WORKSPACES sidebar region — full height on its docked side; the status strip never covers it.
    pub fn left_region(&self, w: f32, h: f32) -> Rect {
        let ch = (h - TOP_BAR_H).max(0.0);
        let x = if self.sidebar_left() {
            0.0
        } else {
            w - self.left_w()
        };
        Rect::new(x, TOP_BAR_H, self.left_w(), ch, Rgba::TRANSPARENT)
    }

    /// The right dock's interior region (editor area, above the status strip).
    pub fn right_region(&self, w: f32, h: f32) -> Rect {
        let rw = self.right_w();
        let ch = (self.content_bottom(h) - TOP_BAR_H).max(0.0);
        let x = self.editor_r(w) - rw;
        Rect::new(x, TOP_BAR_H, rw, ch, Rgba::TRANSPARENT)
    }

    pub fn tree_region(&self, _w: f32, h: f32) -> Rect {
        let ch = (self.content_bottom(h) - TOP_BAR_H).max(0.0);
        Rect::new(
            self.editor_l(),
            TOP_BAR_H,
            self.tree_w(),
            ch,
            Rgba::TRANSPARENT,
        )
    }

    /// The center content region, above the bottom dock and the status strip.
    pub fn center_region(&self, w: f32, h: f32) -> Rect {
        let cl = self.center_l();
        let cr = self.center_r(w);
        let ch = (self.content_bottom(h) - TOP_BAR_H - self.bottom_h()).max(0.0);
        Rect::new(cl, TOP_BAR_H, (cr - cl).max(0.0), ch, Rgba::TRANSPARENT)
    }

    /// The bottom dock's interior region (center width, above the status bar).
    pub fn bottom_region(&self, w: f32, h: f32) -> Rect {
        let cl = self.center_l();
        let cr = self.center_r(w);
        let bh = self.bottom_h();
        Rect::new(
            cl,
            self.content_bottom(h) - bh,
            (cr - cl).max(0.0),
            bh,
            Rgba::TRANSPARENT,
        )
    }

    /// The area docks and the center share (below the header, above the status strip, beside the sidebar);
    /// a zoomed pane covers it.
    pub fn editor_area(&self, w: f32, h: f32) -> Rect {
        let zl = self.editor_l();
        Rect::new(
            zl,
            TOP_BAR_H,
            (self.editor_r(w) - zl).max(0.0),
            (self.content_bottom(h) - TOP_BAR_H).max(0.0),
            Rgba::TRANSPARENT,
        )
    }

    /// The status strip region — the whole editor area (never over the sidebar).
    pub fn status_region(&self, w: f32, h: f32) -> Rect {
        let zl = self.editor_l();
        Rect::new(
            zl,
            h - STATUS_BAR_H,
            (self.editor_r(w) - zl).max(0.0),
            STATUS_BAR_H,
            Rgba::TRANSPARENT,
        )
    }

    /// The interactive session switcher in the header (left, after the traffic lights): just the current
    /// session name as a subtle Button-style trigger, like the reference's project name button.
    /// Where the session switcher starts: past the traffic lights, or near the edge once they are hidden.
    fn session_left(&self) -> f32 {
        if self.fullscreen {
            FULLSCREEN_LEFT
        } else {
            SESSION_LEFT
        }
    }

    pub fn header(&self, w: f32, hovered: Option<u64>) -> Painted {
        let name = self
            .current_session
            .and_then(|index| self.sessions.get(index))
            .map(|s| s.name.as_str())
            .unwrap_or("Open a Project");
        let active = hovered == Some(SESSION_TRIGGER) || self.session_menu;
        let mut trigger = div()
            .row()
            .items_center()
            .px(8.0)
            .h_px(24.0)
            .rounded(6.0)
            .on_click(SESSION_TRIGGER)
            .child(label(name).size(13.0).color(text_c()));
        if active {
            trigger = trigger.bg(theme().ghost_element_hover);
        }
        let mut bar_row = div().row().items_center().h_px(TOP_BAR_H);
        if ui::chrome().session_name {
            bar_row = bar_row.child(trigger);
        }
        let bar: Node = bar_row.into();
        let mut painted = render(
            &bar,
            Rect::new(
                self.session_left(),
                0.0,
                (w - self.session_left()).max(0.0),
                TOP_BAR_H,
                Rgba::TRANSPARENT,
            ),
        );
        let chevron_hot = hovered == Some(APP_MENU) || self.app_menu_open;
        let mut chevron = div()
            .row()
            .w_px(24.0)
            .h_px(24.0)
            .rounded(5.0)
            .items_center()
            .justify_center()
            .on_click(APP_MENU)
            .child(
                ui::icon(ui::IconKind::ChevronDown)
                    .size(12.0)
                    .color(if chevron_hot { text_c() } else { text_dim_c() }),
            );
        if chevron_hot {
            chevron = chevron.bg(theme().ghost_element_hover);
        }
        let mut right = div().row().items_center().gap(4.0).h_px(TOP_BAR_H);
        if let Some(update) = &self.update.button {
            right = right.child(crate::update::button(update, hovered));
        }
        let right: Node = right
            .child(crate::usage::chip(
                &self.usage,
                hovered == Some(USAGE_CHIP) || self.usage_card_open,
            ))
            .child(chevron)
            .into();
        let scale = ui::ui_text_scale();
        let right_w = ui::measure(&right).0 * scale;
        let cluster = render(
            &right,
            Rect::new(
                (w - right_w - 10.0 * scale).max(0.0),
                0.0,
                right_w,
                TOP_BAR_H,
                Rgba::TRANSPARENT,
            ),
        );
        painted.rects.extend(cluster.rects);
        painted.tris.extend(cluster.tris);
        painted.texts.extend(cluster.texts);
        painted.icons.extend(cluster.icons);
        painted.hits.extend(cluster.hits);
        painted
    }

    /// Largest pixel scroll offset for the session list (list content height minus the visible region).
    pub fn session_menu_max_scroll(&self, query: &str) -> f32 {
        let (content_h, region_h) = self.session_list_metrics(query);
        (content_h - region_h).max(0.0)
    }

    /// (content height, visible region height) of the session list, both logical px.
    fn session_list_metrics(&self, query: &str) -> (f32, f32) {
        let entries = self.session_entries(query);
        let mut design = 0.0;
        for (i, (h, _)) in entries.iter().enumerate() {
            design += if h.is_some() {
                MENU_HEADER_H
            } else {
                MENU_ITEM_H
            };
            if i + 1 < entries.len() {
                design += 2.0; // gap
            }
        }
        let scale = ui::ui_text_scale();
        let content_h = design * scale;
        (content_h, content_h.min(MENU_MAX_LIST * scale))
    }

    /// Flattened menu rows: a "This Window" header + the current session, then a "Recent" header + the rest,
    /// filtered by `query`. Each row is `(section header, session index)` -- exactly one is Some.
    fn session_entries(&self, query: &str) -> Vec<(Option<&'static str>, Option<usize>)> {
        let q = query.to_lowercase();
        let matches: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| q.is_empty() || s.name.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        let mut entries = Vec::new();
        if let Some(current) = self.current_session.filter(|c| matches.contains(c)) {
            entries.push((Some("This Window"), None));
            entries.push((None, Some(current)));
        }
        let recent: Vec<usize> = matches
            .iter()
            .copied()
            .filter(|&i| Some(i) != self.current_session)
            .collect();
        if !recent.is_empty() {
            entries.push((Some("Recent"), None));
            for i in recent {
                entries.push((None, Some(i)));
            }
        }
        entries
    }

    /// The session switcher popover. Returns fixed chrome (drawn unclipped) and the scrolling list (drawn
    /// clipped to `clip`), so the app can scroll the list smoothly by pixels between the fixed search and footer.
    pub fn session_menu(
        &self,
        query: &str,
        hovered: Option<u64>,
        search_caret: bool,
    ) -> SessionMenu {
        let entries = self.session_entries(query);
        let scale = ui::ui_text_scale();
        let x = self.session_left();
        let y = TOP_BAR_H + 2.0;
        let menu_w = 300.0 * scale;

        let pad = MENU_PAD * scale;
        let search_h = MENU_SEARCH_H * scale;
        let div_h = MENU_DIV_H * scale;
        let footer_rows = if self.project.is_some() { 3.0 } else { 2.0 };
        let footer_h = (MENU_ACTION_H * footer_rows + 2.0 * (footer_rows - 1.0)) * scale;
        let (content_h, region_h) = self.session_list_metrics(query);
        let max_scroll = (content_h - region_h).max(0.0);
        let scroll = self.session_scroll.clamp(0.0, max_scroll);

        let region_top = y + pad + search_h + div_h;
        let region_bottom = region_top + region_h;
        let menu_h = pad + search_h + div_h + region_h + div_h + footer_h + pad;

        // ---- Fixed chrome (unclipped): shadow, panel, search, dividers, footer, scrollbar. ----
        let mut fixed = Painted::default();
        for i in (1..=6).rev() {
            let sp = i as f32 * 2.0 * scale;
            fixed.rects.push(Rect {
                x: x - sp,
                y: y - sp + 4.0 * scale,
                w: menu_w + 2.0 * sp,
                h: menu_h + 2.0 * sp,
                color: Rgba::new(0.0, 0.0, 0.0, 0.05),
                radius: 8.0 * scale + sp,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            });
        }
        let panel: Node = div()
            .col()
            .bg(theme().elevated_surface_background)
            .rounded(8.0)
            .border(1.0, theme().border)
            .into();
        push_into(
            &mut fixed,
            &panel,
            Rect::new(x, y, menu_w, menu_h, Rgba::TRANSPARENT),
        );

        let mut search = div()
            .row()
            .items_center()
            .gap(6.0)
            .px(8.0)
            .h_px(MENU_SEARCH_H)
            .on_click(SESSION_SEARCH)
            .child(ui::search_icon().size(13.0).color(text_dim_c()));
        if query.is_empty() {
            search = search
                .child(caret(search_caret))
                .child(label("Search sessions...").size(13.0).color(text_dim_c()));
        } else {
            search = search
                .child(label(query).size(13.0).color(text_c()))
                .child(caret(search_caret));
        }
        push_into(
            &mut fixed,
            &search.into(),
            Rect::new(x, y + pad, menu_w, search_h, Rgba::TRANSPARENT),
        );
        push_into(
            &mut fixed,
            &menu_divider(),
            Rect::new(
                x + pad,
                y + pad + search_h,
                menu_w - 2.0 * pad,
                div_h,
                Rgba::TRANSPARENT,
            ),
        );
        push_into(
            &mut fixed,
            &menu_divider(),
            Rect::new(
                x + pad,
                region_bottom,
                menu_w - 2.0 * pad,
                div_h,
                Rgba::TRANSPARENT,
            ),
        );
        let mut footer = div()
            .col()
            .gap(2.0)
            .child(action_row(
                ui::icon(ui::IconKind::Plus)
                    .size(14.0)
                    .color(theme().icon_muted)
                    .into(),
                "New session...",
                SESSION_NEW,
                hovered,
            ))
            .child(action_row(
                folder_icon().into(),
                "Open a session...",
                SESSION_OPEN,
                hovered,
            ));
        if self.project.is_some() {
            footer = footer.child(action_row(
                ui::icon(ui::IconKind::File)
                    .size(14.0)
                    .color(theme().icon_muted)
                    .into(),
                "Edit pom.yml",
                SESSION_EDIT_CONFIG,
                hovered,
            ));
        }
        let footer: Node = footer.into();
        push_into(
            &mut fixed,
            &footer,
            Rect::new(
                x + pad,
                region_bottom + div_h,
                menu_w - 2.0 * pad,
                footer_h,
                Rgba::TRANSPARENT,
            ),
        );
        // ---- Scrolling list (clipped): section headers + session rows, offset by the pixel scroll. ----
        let mut col = div().col().gap(2.0);
        for &(header, item) in &entries {
            if let Some(h) = header {
                col = col.child(
                    div()
                        .px(8.0)
                        .h_px(MENU_HEADER_H)
                        .items_center()
                        .child(label(h).size(12.0).color(text_dim_c())),
                );
            } else if let Some(i) = item {
                if let Some(s) = self.sessions.get(i) {
                    col = col.child(session_row(i, s, Some(i) == self.current_session, hovered));
                }
            }
        }
        let mut list = render(
            &col.into(),
            Rect::new(
                x + pad,
                region_top - scroll,
                menu_w - 2.0 * pad,
                content_h.max(region_h) + scroll,
                Rgba::TRANSPARENT,
            ),
        );
        // Scrollbar thumb drawn on top of the rows (also clipped to the list region).
        if max_scroll > 0.0 {
            let thumb_h = (region_h * region_h / content_h).max(24.0 * scale);
            let travel = region_h - thumb_h;
            let t = scroll / max_scroll;
            list.rects.push(Rect {
                x: x + menu_w - 7.0 * scale,
                y: region_top + travel * t,
                w: 4.0 * scale,
                h: thumb_h,
                color: theme().scrollbar_thumb_background,
                radius: 2.0 * scale,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            });
        }

        SessionMenu {
            fixed,
            list,
            clip: (x, region_top, menu_w, region_h),
            content_h,
            region_h,
        }
    }
}

/// Render `node` into `area` and append the result to `out`.
fn push_into(out: &mut Painted, node: &Node, area: Rect) {
    let p = render(node, area);
    out.rects.extend(p.rects);
    out.tris.extend(p.tris);
    out.texts.extend(p.texts);
    out.icons.extend(p.icons);
    out.hits.extend(p.hits);
}

fn menu_divider() -> Node {
    div()
        .py(4.0)
        .child(div().h_px(1.0).bg(theme().border_variant))
        .into()
}

fn caret(visible: bool) -> Node {
    div()
        .w_px(1.5)
        .h_px(15.0)
        .bg(if visible {
            theme().text
        } else {
            Rgba::TRANSPARENT
        })
        .into()
}

fn action_row(icon: Node, text: &str, id: u64, hovered: Option<u64>) -> Node {
    let mut r = div()
        .row()
        .items_center()
        .gap(8.0)
        .px(8.0)
        .h_px(28.0)
        .rounded(6.0)
        .on_click(id)
        .child(icon)
        .child(label(text).size(13.0).color(text_c()));
    if hovered == Some(id) {
        r = r.bg(theme().element_hover);
    }
    r.into()
}

fn session_row(i: usize, s: &Session, current: bool, hovered: Option<u64>) -> Node {
    let id = SESSION_ITEM_BASE + i as u64;
    let action_ids = [
        SESSION_OPENNEW_BASE + i as u64,
        SESSION_OPENTHIS_BASE + i as u64,
        SESSION_REVEAL_BASE + i as u64,
        SESSION_DELETE_BASE + i as u64,
    ];
    let row_hover = hovered == Some(id) || action_ids.iter().any(|a| hovered == Some(*a));
    // Left group: monitor icon, name, and (on the current session) a check right next to the name -- like the
    // reference's "name check" rather than a far-right tick.
    let mut left = div()
        .row()
        .items_center()
        .gap(8.0)
        .child(ui::monitor_icon().size(16.0).color(text_dim_c()))
        .child(
            label(&s.name)
                .size(13.0)
                .color(if current { text_c() } else { text_dim_c() }),
        );
    if current {
        left = left.child(ui::check_icon().size(15.0));
    }
    if s.missing {
        left = left.child(
            label("missing")
                .label_size(ui::LabelSize::XSmall)
                .color(theme().text_disabled),
        );
    }
    let mut r = div()
        .row()
        .items_center()
        .px(8.0)
        .h_px(28.0)
        .rounded(6.0)
        .on_click(id);
    if row_hover {
        // Hover: row action buttons pinned to the right (open in new window, open in this window, reveal,
        // delete) -- mirroring the reference's on-hover row actions.
        let mut actions = div().row().items_center().gap(2.0);
        if !s.missing {
            actions = actions
                .child(icon_button(
                    ui::arrow_up_right_icon().into(),
                    SESSION_OPENNEW_BASE + i as u64,
                    hovered,
                ))
                .child(icon_button(
                    ui::window_icon().into(),
                    SESSION_OPENTHIS_BASE + i as u64,
                    hovered,
                ))
                .child(icon_button(
                    ui::folder_icon().into(),
                    SESSION_REVEAL_BASE + i as u64,
                    hovered,
                ));
        }
        if !current {
            actions = actions.child(icon_button(
                ui::close_icon().into(),
                SESSION_DELETE_BASE + i as u64,
                hovered,
            ));
        }
        r = r
            .justify_between()
            .bg(theme().element_hover)
            .child(left)
            .child(actions);
    } else {
        r = r.child(left);
    }
    r.into()
}

/// The bottom bar (a component, not a dock): left = the workspace function nav (Files/Services/... via
/// `function_bar`) + terminal toggle; right = diagnostics + cursor position + language (placeholder data).
/// This is where Pomelo puts the function icons — a horizontal strip at the very bottom.
pub fn status_bar(layout: &Layout, hovered: Option<u64>) -> Node {
    let dim = text_dim_c();
    // A dock toggle: the icon turns accent (blue) when its dock is open; the fill only shows on hover/press.
    let badged = |kind: ui::IconKind, id: u64, active: bool, badge: bool| {
        let color = if active { theme().icon_accent } else { dim };
        let mut glyph = div()
            .row()
            .gap(1.0)
            .child(ui::icon(kind).size(13.0).color(color));
        if badge {
            glyph = glyph.child(
                div()
                    .col()
                    .h_px(13.0)
                    .justify_end()
                    .child(div().w_px(6.0).h_px(6.0).rounded(3.0).bg(theme().success)),
            );
        }
        let mut b = div()
            .w_px(26.0)
            .h_px(20.0)
            .rounded(5.0)
            .items_center()
            .justify_center()
            .on_click(id)
            .child(glyph);
        if hovered == Some(id) {
            b = b.bg(theme().element_hover);
        }
        b
    };
    let toggle = |kind: ui::IconKind, id: u64, active: bool| badged(kind, id, active, false);
    // A small vertical divider between groups (a thin), 1px on the theme border color.
    let vsep = || div().w_px(1.0).h_px(14.0).bg(theme().border);

    // One dock's panel buttons (editor's `PanelButtons`): a row of icon buttons — the functions on this side,
    // the terminal if it lives here, and the agent (right dock's default). No dividers between the buttons;
    // the active/visible panel's icon is accented. Returns None when the dock has no buttons.
    let dock_group = |side: DockPosition| -> Option<Node> {
        let mut row = div().row().gap(4.0).items_center();
        let mut has = false;
        let active_ticket = layout.project.as_ref().is_some_and(|project| {
            project
                .workspaces
                .iter()
                .position(|branch| *branch == project.active)
                .and_then(|index| project.tickets.get(index))
                .is_some_and(|ticket| !ticket.is_empty())
        });
        for (i, kind) in PaneKind::ALL.iter().enumerate() {
            if !layout.func_offered(i) {
                continue;
            }
            if layout
                .func_side
                .get(i)
                .copied()
                .unwrap_or(DockPosition::Left)
                != side
            {
                continue;
            }
            row = row.child(badged(
                kind.icon(),
                FUNC_BASE + i as u64,
                layout.func_active(i),
                *kind == PaneKind::Services && layout.services_running,
            ));
            has = true;
        }
        if active_ticket && side == DockPosition::Left {
            row = row.child(toggle(ui::IconKind::Ticket, STATUS_TICKET, false));
            has = true;
        }
        if !layout.terminal_hidden && layout.terminal_side == side {
            row = row.child(toggle(
                ui::IconKind::Terminal,
                BOTTOM_TOGGLE,
                layout.terminal_visible(),
            ));
            has = true;
        }
        if !layout.agent_hidden && side == layout.agent_side {
            let active = layout.agent_shown();
            row = row.child(toggle(ui::IconKind::Sparkle, AGENT_TOGGLE, active));
            has = true;
        }
        has.then_some(row.into())
    };
    // The WORKSPACES sidebar toggle is NOT in this status bar: the sidebar is a special panel with its own
    // footer (drawn in `WorkspaceView`), and the status strip never covers it.
    div()
        .row()
        .px(8.0)
        .gap(6.0)
        .items_center()
        .justify_between()
        // Left tools: the sidebar toggle (docked left), the left dock's buttons, then diagnostics. Each group is
        // followed by a divider (editor's Left-dock rule: divider after the buttons).
        .child({
            let mut row = div().row().gap(6.0).items_center();
            if let Some(g) = dock_group(DockPosition::Left) {
                row = row.child(g).child(vsep());
            }
            let servers = &layout.language_servers;
            if ui::chrome().language_servers && !servers.servers.is_empty() {
                let (dot, _) = servers.health();
                let mut glyph = div().row().gap(1.0).child(
                    ui::icon(ui::IconKind::BoltOutlined)
                        .size(14.0)
                        .color(theme().icon_muted),
                );
                if let Some(dot) = dot {
                    glyph = glyph.child(
                        div()
                            .col()
                            .h_px(14.0)
                            .justify_end()
                            .child(div().w_px(6.0).h_px(6.0).rounded(3.0).bg(dot)),
                    );
                }
                let mut button = div()
                    .w_px(26.0)
                    .h_px(20.0)
                    .rounded(5.0)
                    .items_center()
                    .justify_center()
                    .on_click(LANGUAGE_SERVERS_BUTTON)
                    .child(glyph);
                if hovered == Some(LANGUAGE_SERVERS_BUTTON) {
                    button = button.bg(theme().element_hover);
                }
                row = row.child(button);
            }
            if ui::chrome().diagnostics {
                let summary = layout
                    .files_view
                    .as_ref()
                    .and_then(|v| v.diagnostic_summary())
                    .unwrap_or_default();
                let colors = ui::theme();
                let mut indicator = div().row().gap(4.0).items_center();
                if summary.errors == 0 && summary.warnings == 0 {
                    indicator = indicator
                        .child(ui::icon(ui::IconKind::Check).size(14.0).color(colors.text));
                } else {
                    if summary.errors > 0 {
                        indicator = indicator
                            .child(
                                ui::icon(ui::IconKind::XCircle)
                                    .size(14.0)
                                    .color(colors.error),
                            )
                            .child(
                                label(summary.errors.to_string())
                                    .size(12.0)
                                    .color(colors.text),
                            );
                    }
                    if summary.warnings > 0 {
                        indicator = indicator
                            .child(
                                ui::icon(ui::IconKind::Warning)
                                    .size(14.0)
                                    .color(colors.warning),
                            )
                            .child(
                                label(summary.warnings.to_string())
                                    .size(12.0)
                                    .color(colors.text),
                            );
                    }
                }
                let mut button = div()
                    .row()
                    .h_px(20.0)
                    .px(4.0)
                    .rounded(4.0)
                    .items_center()
                    .on_click(DIAGNOSTICS_BUTTON)
                    .child(indicator);
                if hovered == Some(DIAGNOSTICS_BUTTON) {
                    button = button.bg(colors.ghost_element_hover);
                }
                row = row.child(button);
                if let Some(message) = summary.current {
                    let mut button = div()
                        .h_px(20.0)
                        .px(4.0)
                        .rounded(4.0)
                        .items_center()
                        .on_click(DIAGNOSTIC_MESSAGE)
                        .child(label(message).size(12.0).color(colors.text));
                    if hovered == Some(DIAGNOSTIC_MESSAGE) {
                        button = button.bg(colors.ghost_element_hover);
                    }
                    row = row.child(button);
                }
            }
            if let Some(activity) = &layout.language_servers.activity {
                let icon = match activity.icon {
                    ActivityIcon::Loading => ui::IconKind::LoadCircle,
                    ActivityIcon::Download => ui::IconKind::Download,
                    ActivityIcon::Warning => ui::IconKind::Warning,
                };
                let mut button = div()
                    .row()
                    .h_px(20.0)
                    .px(4.0)
                    .gap(4.0)
                    .rounded(4.0)
                    .items_center()
                    .on_click(LANGUAGE_ACTIVITY)
                    .child(ui::icon(icon).size(14.0).color(theme().icon_muted))
                    .child(
                        label(trail_off(&activity.message, ACTIVITY_MESSAGE_LIMIT))
                            .size(12.0)
                            .color(theme().text),
                    );
                if hovered == Some(LANGUAGE_ACTIVITY) {
                    button = button.bg(theme().ghost_element_hover);
                }
                row = row.child(button);
            }
            row
        })
        .child({
            let ch = ui::chrome();
            let mut row = div().row().gap(6.0).items_center();
            let position = layout
                .files_view
                .as_ref()
                .and_then(|v| v.cursor_position())
                .filter(|_| ch.cursor_position);
            let shows_position = position.is_some();
            if let Some(text) = position {
                let mut button = div()
                    .h_px(20.0)
                    .px(4.0)
                    .rounded(4.0)
                    .items_center()
                    .on_click(CURSOR_POSITION)
                    .child(label(text).size(12.0).color(theme().text));
                if hovered == Some(CURSOR_POSITION) {
                    button = button.bg(theme().ghost_element_hover);
                }
                row = row.child(button);
            }
            let language = layout
                .files_view
                .as_ref()
                .and_then(|v| v.active_language())
                .filter(|_| ch.language);
            if let Some(language) = language {
                if shows_position {
                    row = row.child(vsep());
                }
                row = row.child(label(language).size(12.0).color(dim));
            }
            if let Some(today) = &layout.usage.today {
                row = row.child(vsep()).child(crate::usage::status_item(
                    today,
                    hovered == Some(USAGE_STATUS) || layout.usage_today_open,
                ));
            }
            if let Some(g) = dock_group(DockPosition::Bottom) {
                row = row.child(vsep()).child(g);
            }
            if let Some(g) = dock_group(DockPosition::Right) {
                row = row.child(vsep()).child(g);
            }
            row
        })
        .into()
}

/// One row of a context menu: click `id`, a `label`, a `checked` mark, and a `sep`arator line above it.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub id: u64,
    pub label: std::borrow::Cow<'static, str>,
    pub checked: bool,
    pub sep: bool,
    pub disabled: bool,
    /// Loses data: drawn in the error color.
    pub danger: bool,
    /// Shown before the label when the item is not checked.
    pub icon: Option<ui::IconKind>,
    /// Muted text at the right: its key, or what a submenu is set to.
    pub hint: Option<std::borrow::Cow<'static, str>>,
    /// The icon's color, over the default for its kind.
    pub color: Option<Rgba>,
    /// A group's title: small, muted and not clickable.
    pub header: bool,
}

impl MenuItem {
    pub fn new(id: u64, label: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        MenuItem {
            id,
            label: label.into(),
            checked: false,
            sep: false,
            disabled: false,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        }
    }
}

/// A right-click context menu anchored above `(ax, ay)`, clamped to stay inside `viewport_w`.
#[allow(clippy::too_many_arguments)]
pub fn context_menu(
    ax: f32,
    atop: f32,
    abottom: f32,
    viewport_w: f32,
    viewport_h: f32,
    items: &[MenuItem],
    hovered: Option<u64>,
    flip_left_at: Option<f32>,
) -> Painted {
    let scale = ui::ui_text_scale();
    let row_h = 26.0_f32;
    let pad = 4.0_f32;
    let wght = ui::ui_font_weight();
    let mut content_w = 0.0_f32;
    for it in items {
        let label_w = ui::measure_text_width(&it.label, 13.0, false, wght);
        let right = if is_submenu(it.id) {
            14.0
        } else {
            let k = it.hint.as_deref().unwrap_or_else(|| menu_key(it.id));
            if k.is_empty() {
                0.0
            } else {
                ui::measure(&crate::render_keystroke(k, 12.0)).0 + 24.0
            }
        } + if is_submenu(it.id) {
            it.hint.as_deref().map_or(0.0, |hint| {
                ui::measure_text_width(hint, 12.0, false, wght) + 12.0
            })
        } else {
            0.0
        };
        content_w = content_w.max(label_w + right);
    }
    let mw = (16.0 + 6.0 + content_w + 16.0 + 2.0 * pad).max(200.0);
    // Count only the separators actually drawn (a leading `sep` on the first item is skipped).
    let seps = items
        .iter()
        .enumerate()
        .filter(|(i, it)| it.sep && *i > 0)
        .count() as f32;
    let mh = pad * 2.0 + row_h * items.len() as f32 + 5.0 * seps;
    let margin = 6.0 * scale;
    let mwpx = mw * scale;
    let x = match flip_left_at {
        Some(left) if ax + mwpx + margin > viewport_w => (left - mwpx).max(margin),
        _ => ax.min(viewport_w - mwpx - margin).max(margin),
    };
    // Open below the anchored button when there is room; flip above if it would spill off the bottom (editor).
    let gap = 4.0 * scale;
    let mhpx = mh * scale;
    let y = if abottom + gap + mhpx + 6.0 * scale <= viewport_h {
        abottom + gap
    } else {
        (atop - gap - mhpx).max(6.0 * scale)
    };

    let mut col = div()
        .col()
        .p(pad)
        .rounded(8.0)
        .bg(theme().elevated_surface_background)
        .border(1.0, theme().border);
    for (i, item) in items.iter().enumerate() {
        if item.sep && i > 0 {
            col = col.child(
                div()
                    .h_px(5.0)
                    .py(2.0)
                    .child(div().h_px(1.0).w_px(mw - 2.0 * pad).bg(theme().border)),
            );
        }
        if item.header {
            col = col.child(
                div().row().h_px(row_h).px(8.0).items_center().child(
                    label(item.label.to_string())
                        .size(12.0)
                        .color(theme().text_muted),
                ),
            );
            continue;
        }
        let mut row = div()
            .row()
            .h_px(row_h)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .rounded(5.0)
            .on_click(item.id);
        if hovered == Some(item.id) && !item.disabled {
            row = row.bg(theme().element_hover);
        }
        let label_color = if item.disabled {
            theme().text_disabled
        } else if item.danger {
            theme().error
        } else {
            text_c()
        };
        let mark = match (item.checked, item.icon) {
            (true, _) => ui::check_icon().size(12.0),
            (false, Some(kind)) => {
                ui::icon(kind)
                    .size(12.0)
                    .color(if let Some(color) = item.color {
                        color
                    } else if item.disabled {
                        theme().text_disabled
                    } else if item.danger {
                        theme().error
                    } else if kind == ui::IconKind::Sparkle {
                        theme().icon_accent
                    } else {
                        theme().icon_muted
                    })
            }
            (false, None) => ui::icon(ui::IconKind::Check)
                .size(12.0)
                .color(Rgba::TRANSPARENT),
        };
        row = row
            .child(mark)
            .child(label(item.label.to_string()).size(13.0).color(label_color));
        if is_submenu(item.id) {
            row = row.child(div().flex(1.0));
            if let Some(hint) = &item.hint {
                row = row.child(label(hint.to_string()).size(12.0).color(theme().text_muted));
            }
            row = row.child(
                ui::icon(ui::IconKind::ChevronRight)
                    .size(12.0)
                    .color(theme().icon_muted),
            );
        } else {
            let key = item.hint.as_deref().unwrap_or_else(|| menu_key(item.id));
            if !key.is_empty() {
                row = row
                    .child(div().flex(1.0))
                    .child(crate::render_keystroke(key, 12.0));
            }
        }
        col = col.child(row);
    }
    let mut out = Painted::default();
    out.rects.push(Rect {
        x: x - 2.0 * scale,
        y: y - 1.0 * scale,
        w: mw * scale + 4.0 * scale,
        h: mh * scale + 4.0 * scale,
        color: Rgba::new(0.0, 0.0, 0.0, 0.14),
        radius: 10.0 * scale,
        border: 0.0,
        border_color: Rgba::TRANSPARENT,
    });
    push_into(
        &mut out,
        &col.into(),
        Rect::new(x, y, mw * scale, mh * scale, Rgba::TRANSPARENT),
    );
    out
}

/// The tooltip of a hovered status-bar item (function nav / dock toggles): its title, and the action whose
/// binding it shows (or a fixed key for the ones that are not keymap actions).
pub fn status_tooltip(id: u64) -> Option<(String, StatusKey)> {
    use crate::keymap::Action;
    let func = |kind: PaneKind| match kind {
        PaneKind::Files => StatusKey::Action(Action::FocusFiles),
        PaneKind::Services => StatusKey::Action(Action::FocusServices),
        PaneKind::Git => StatusKey::Action(Action::FocusGit),
        PaneKind::Database => StatusKey::Action(Action::FocusDatabase),
        _ => StatusKey::None,
    };
    if id == BOTTOM_TOGGLE {
        Some(("Terminal".into(), StatusKey::Action(Action::ToggleTerminal)))
    } else if id == AGENT_TOGGLE || id == RIGHT_TOGGLE {
        Some(("Agent".into(), StatusKey::Action(Action::ToggleAgent)))
    } else if id == CURSOR_POSITION {
        Some(("Go to Line/Column".into(), StatusKey::Fixed("ctrl-g")))
    } else if id == DIAGNOSTIC_MESSAGE {
        Some(("Next Diagnostic".into(), StatusKey::Fixed("f8")))
    } else if id == LANGUAGE_SERVERS_BUTTON {
        Some(("Language Servers".into(), StatusKey::None))
    } else if id == DIAGNOSTICS_BUTTON {
        Some((
            "Project Diagnostics".into(),
            StatusKey::Action(Action::ProjectDiagnostics),
        ))
    } else if id == STATUS_TICKET {
        Some(("Jira Ticket".into(), StatusKey::None))
    } else if (FUNC_BASE..FUNC_BASE + PaneKind::ALL.len() as u64).contains(&id) {
        let kind = PaneKind::ALL[(id - FUNC_BASE) as usize];
        Some((kind.title().to_string(), func(kind)))
    } else {
        None
    }
}

/// The key a status-bar tooltip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKey {
    None,
    Action(crate::keymap::Action),
    Fixed(&'static str),
}

/// A tooltip bubble anchored ABOVE `anchor` (for the status bar, which sits at the very bottom).
pub fn tooltip_above(anchor: Rect, text: &str, keys: Option<&str>, viewport_w: f32) -> Painted {
    let scale = ui::ui_text_scale();
    let mut content = div()
        .row()
        .gap(8.0)
        .items_center()
        .child(label(text).size(12.0).color(text_c()));
    if let Some(keys) = keys.filter(|keys| !keys.is_empty()) {
        let mut strokes = div().row().gap(4.0).items_center();
        for stroke in keys.split_whitespace() {
            strokes = strokes.child(crate::render_keystroke(stroke, 12.0));
        }
        content = content.child(strokes);
    }
    let content: Node = content.into();
    let w = (ui::measure(&content).0 + 16.0) * scale;
    let h = 24.0 * scale;
    let mut x = anchor.x;
    if x + w > viewport_w - 6.0 * scale {
        x = (viewport_w - 6.0 * scale - w).max(6.0 * scale);
    }
    let y = anchor.y - h - 5.0 * scale;
    let node: Node = div()
        .row()
        .items_center()
        .px(8.0)
        .h_px(24.0)
        .rounded(6.0)
        .bg(theme().elevated_surface_background)
        .border(1.0, theme().border)
        .child(content)
        .into();
    let mut out = Painted::default();
    out.rects.push(Rect {
        x: x - 2.0 * scale,
        y: y - 1.0 * scale,
        w: w + 4.0 * scale,
        h: h + 4.0 * scale,
        color: Rgba::new(0.0, 0.0, 0.0, 0.12),
        radius: 8.0 * scale,
        border: 0.0,
        border_color: Rgba::TRANSPARENT,
    });
    push_into(&mut out, &node, Rect::new(x, y, w, h, Rgba::TRANSPARENT));
    out
}

/// The tooltip text for a hovered session-row action id, if any.
pub fn session_action_tooltip(id: u64) -> Option<&'static str> {
    if (SESSION_OPENNEW_BASE..SESSION_OPENNEW_BASE + 100).contains(&id) {
        Some("Open in New Window")
    } else if (SESSION_OPENTHIS_BASE..SESSION_OPENTHIS_BASE + 100).contains(&id) {
        Some("Open in This Window")
    } else if (SESSION_REVEAL_BASE..SESSION_REVEAL_BASE + 100).contains(&id) {
        Some("Reveal in Finder")
    } else if (SESSION_DELETE_BASE..SESSION_DELETE_BASE + 100).contains(&id) {
        Some("Remove from List")
    } else {
        None
    }
}

/// A small tooltip bubble anchored below `anchor` (logical px), clamped within `viewport_w`. Drawn in the
/// renderer's top (unclipped) layer so it floats above the clipped menu list.
pub fn tooltip(anchor: Rect, text: &str, viewport_w: f32) -> Painted {
    let scale = ui::ui_text_scale();
    let tw = ui::measure_text_width(text, 12.0, false, ui::ui_font_weight());
    let pad = 8.0 * scale;
    let w = tw + 2.0 * pad;
    let h = 24.0 * scale;
    let mut x = anchor.x;
    if x + w > viewport_w - 6.0 * scale {
        x = (viewport_w - 6.0 * scale - w).max(6.0 * scale);
    }
    let y = anchor.y + anchor.h + 5.0 * scale;
    let node: Node = div()
        .row()
        .items_center()
        .px(8.0)
        .h_px(24.0)
        .rounded(6.0)
        .bg(theme().elevated_surface_background)
        .border(1.0, theme().border)
        .child(label(text).size(12.0).color(text_c()))
        .into();
    let mut out = Painted::default();
    // A faint shadow under the bubble.
    out.rects.push(Rect {
        x: x - 2.0 * scale,
        y: y - 1.0 * scale,
        w: w + 4.0 * scale,
        h: h + 4.0 * scale,
        color: Rgba::new(0.0, 0.0, 0.0, 0.12),
        radius: 8.0 * scale,
        border: 0.0,
        border_color: Rgba::TRANSPARENT,
    });
    push_into(&mut out, &node, Rect::new(x, y, w, h, Rgba::TRANSPARENT));
    out
}

/// A small square icon button used in session-row hover actions; highlights when it is the hovered id.
fn icon_button(icon: Node, id: u64, hovered: Option<u64>) -> Node {
    let mut b = div()
        .items_center()
        .justify_center()
        .w_px(20.0)
        .h_px(20.0)
        .rounded(4.0)
        .on_click(id)
        .child(icon);
    if hovered == Some(id) {
        b = b.bg(theme().ghost_element_hover);
    }
    b.into()
}

/// The syntax palette matching the active UI theme's light/dark appearance, so highlighting (and selection
/// tints in text fields) stay in sync with the app theme.
pub fn syntax_theme() -> editor::Theme {
    let colors = ui::theme();
    let mut theme = if colors.appearance == ui::Appearance::Light {
        editor::Theme::one_light()
    } else {
        editor::Theme::one_dark()
    };
    let rgb = |rgba: ui::Rgba| {
        let [r, g, b, _] = rgba.to_u8();
        editor::theme::Color::rgb(r, g, b)
    };
    theme.foreground = rgb(colors.editor_foreground);
    for (capture, color) in ui::theme_file::syntax_colors() {
        theme.syntax.insert(capture, rgb(color));
    }
    theme
}
