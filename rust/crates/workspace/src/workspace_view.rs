//! `WorkspaceView`: the main window as a reactive `RawView`. It owns the `Layout` plus the header session-menu
//! interaction state, composes the layered `Frame` (body + header base, then the session menu's fixed chrome,
//! clipped scrolling list, and tooltip overlays), and handles input via mouse/scroll/key methods. Dock-divider
//! drag and dock persistence are surfaced through `WorkspaceEffects` for the shell (winit + persisted settings).

use crate::{
    context_menu, function_content, function_dock_body, is_submenu, session_action_tooltip,
    status_bar, status_tooltip, terminal_dock_body, tooltip, tooltip_above, DividerAxis,
    DockPosition, EditKey, Layout, MenuItem, PaneKind, Shown, AGENT_TOGGLE, BOTTOM_TOGGLE,
    EDITOR_MENU_TARGET, FUNC_BASE, FUNC_VIEW_BASE, MENU_COPY_NAME, MENU_COPY_PATH,
    MENU_COPY_REL_PATH, MENU_DOCK_BOTTOM, MENU_DOCK_LEFT, MENU_DOCK_RIGHT, MENU_EDIT_COPY,
    MENU_EDIT_CUT, MENU_EDIT_PASTE, MENU_EDIT_SELECT_ALL, MENU_HIDE, MENU_REVEAL,
    MENU_SUBMENU_COPY, MENU_TREE_OPEN, RIGHT_TOGGLE, SESSION_DELETE_BASE, SESSION_ITEM_BASE,
    SESSION_OPEN, SESSION_OPENNEW_BASE, SESSION_OPENTHIS_BASE, SESSION_REVEAL_BASE, SESSION_SEARCH,
    SESSION_TRIGGER, SIDEBAR_TOGGLE, TREE_MENU_TARGET,
};
use std::time::{Duration, Instant};
use ui::{Context, Frame, IconKind, Overlay, Painted, RawView, Rect, Rgba, Window};

use crate::{
    TreeAction, MENU_EDIT_COPY_TRIM, MENU_EDIT_GO_TO_DECLARATION, MENU_EDIT_GO_TO_DEFINITION,
    MENU_EDIT_GO_TO_IMPLEMENTATION, MENU_EDIT_GO_TO_TYPE_DEFINITION, MENU_EDIT_REVEAL,
    MENU_TREE_COLLAPSE_ALL, MENU_TREE_COPY, MENU_TREE_CUT, MENU_TREE_DELETE, MENU_TREE_DUPLICATE,
    MENU_TREE_EXPAND_ALL, MENU_TREE_GITIGNORE, MENU_TREE_NEW_DIR, MENU_TREE_NEW_FILE,
    MENU_TREE_OPEN_SYSTEM, MENU_TREE_PASTE, MENU_TREE_RENAME, MENU_TREE_RESTORE, MENU_TREE_TRASH,
};
use crate::{TOAST_ACTION, TOAST_CLOSE};

const TOAST_DISMISS: Duration = Duration::from_secs(10);
const UPKEEP_DONE_SHOWN: Duration = Duration::from_secs(4);
/// How long after the panes first change they are written, so a burst of changes costs one write.
const PANES_SAVE_THROTTLE: Duration = Duration::from_millis(200);
/// Prompt tokens the view hands out itself, above the ones features number from 1.
const CLOSE_PROMPT_TOKENS: u64 = 1 << 40;
const FORGET_PROMPT_TOKENS: u64 = 1 << 41;
const PANEL_PROMPT_TOKENS: u64 = 1 << 42;
const DELETE_WORKSPACE_PROMPT_TOKENS: u64 = 1 << 43;
const PREPARE_MAIN_PROMPT_TOKEN: u64 = 1 << 44;
/// The gap a zoomed view leaves around it (on its dock's inner side only, for a dock panel).
const ZOOM_PADDING: f32 = 8.0;
/// Where tabs' own toolbar ids start (a service's header, a side agent's bar).
const PAGE_TAB: u64 = 1150;
const PAGE_TAB_CLOSE: u64 = 1151;

const TOAST_ANIM: Duration = Duration::from_millis(160);
const MODAL_TOP: f32 = 80.0;

struct Toast {
    message: String,
    action: Option<String>,
    shown_at: Instant,
    deadline: Instant,
    hovered: bool,
    remaining: Duration,
    rect: Rect,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceEffects {
    pub persist: bool,
    pub open_new_window: Option<usize>,
    pub session: Option<SessionRequest>,
    pub open_settings: bool,
    /// Work in this workspace (an index into `ProjectInfo::workspaces`) in this window.
    pub activate_workspace: Option<usize>,
    /// Open (or focus) the workspace's coding agent.
    pub open_agent: bool,
    /// Start a side agent next to the main one.
    pub side_agent: Option<(crate::SideAgentRole, crate::SideAgentStart)>,
    /// Reopen an archived side agent (an index into `Layout::archived_agents`).
    pub reopen_agent: Option<usize>,
    pub fix_setup: bool,
    /// Restart the services still running with the previous config.
    pub restart_stale: bool,
    pub switch_language_server: Option<crate::ServerSwitch>,
    /// A command palette pick the app runs (settings, projects, themes...).
    pub action: Option<crate::keymap::Action>,
    /// The usage card's Refresh Now.
    pub refresh_usage: bool,
    pub update: Option<crate::UpdateAction>,
    pub titlebar: Option<TitlebarGesture>,
}

/// What a press on an empty part of the title bar asks the window to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarGesture {
    Move,
    /// Whatever the system's "double-click a window's title bar" setting says.
    DoubleClick,
}

/// What the WORKSPACES panel asked for: the new-workspace form, an action on a row (an index into
/// `ProjectInfo::workspaces`) or a button on an operation card (by operation id).
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceRequests {
    pub new_workspace: bool,
    pub row: Option<(usize, crate::RowAction)>,
    /// Move the workspace at the first index to the second.
    pub reorder: Option<(usize, usize)>,
    pub op: Option<(u64, crate::OpAction)>,
    /// A group was folded, opened or moved; `workspace_grouping` has the new arrangement to save.
    pub grouping_changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRequest {
    Switch(usize),
    Forget(usize),
    Reveal(usize),
    ChooseFolder,
    NewProject,
    ImportBundle,
    CheckMachine,
    /// The fix of the welcome page's machine check at this index (start Docker).
    FixMachine(usize),
}

/// A workspace's views and dock visibility while another workspace has the window.
pub struct ParkedWorkspace {
    files: Option<Box<dyn crate::FunctionView>>,
    terminal: Option<Box<dyn crate::TerminalPanelView>>,
    agent: Option<Box<dyn crate::TerminalPanelView>>,
    side_panels: Vec<Box<dyn crate::SidePanelView>>,
    active_panels: [Option<Shown>; 3],
    right_collapsed: bool,
    bottom_collapsed: bool,
}

fn clipped_hits(painted: &Painted, clip: Rect) -> Vec<(Rect, u64)> {
    painted
        .hits
        .iter()
        .filter_map(|(r, id)| {
            let x0 = r.x.max(clip.x);
            let y0 = r.y.max(clip.y);
            let x1 = (r.x + r.w).min(clip.x + clip.w);
            let y1 = (r.y + r.h).min(clip.y + clip.h);
            (x1 > x0 && y1 > y0)
                .then(|| (Rect::new(x0, y0, x1 - x0, y1 - y0, Rgba::TRANSPARENT), *id))
        })
        .collect()
}

const SETUP_TITLE: &str = "Project setup needs attention";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Notification {
    title: String,
    message: String,
    primary: Option<String>,
    action: Option<NotificationAction>,
    level: Option<crate::NoticeLevel>,
    buttons: Vec<String>,
    copy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NotificationAction {
    OpenFile(std::path::PathBuf, Option<u32>),
    FixSetup,
    RestartStale,
    ReleaseNotes,
    Server(crate::ServerNotice),
    ReopenTabs,
    RestartUpdate,
}

/// Whether the next restore brings the saved tabs back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TabRestore {
    #[default]
    Restore,
    /// Start empty once and save from there (the setting says not to restore).
    SkipOnce,
    /// Start empty and leave the saved tabs as they are until they are reopened (they may be what crashed).
    Hold,
}

/// A zoomed group drawn over the workspace: its frame (with the border on `sides`) and the content inside it.
#[derive(Clone, Copy)]
struct Zoom {
    group: InputGroup,
    outer: Rect,
    inner: Rect,
    /// Top, right, bottom, left.
    sides: [bool; 4],
}

/// Which pane group input goes to: the editor area's or the terminal panel's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InputGroup {
    Center,
    Panel,
    Agent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    TerminalDivider(u64),
    TerminalTab,
    /// A press on an item that paints itself (a terminal); its drag and release go to the same item.
    ItemPointer(InputGroup),
    Left,
    Right,
    Bottom,
    Tree,
    Center(u64),
    Tab,
    EditorSel(InputGroup),
    WorkspaceRow,
    /// A group header of the WORKSPACES list, moving its whole group.
    WorkspaceGroup,
    /// A tab of the agent dock, moved within that dock.
    AgentTab,
}

/// A group header pressed in the grouped list: a click folds it, a drag moves the group.
#[derive(Clone, Copy)]
struct GroupDrag {
    from: crate::TicketGroup,
    press: (f32, f32),
    /// The pressed header's rect, so the lifted group keeps the grab point under the pointer.
    source: Rect,
    to: Option<crate::TicketGroup>,
    pointer: (f32, f32),
}

/// A WORKSPACES row being dragged to a new place: its index, where the press was, and the row it would take.
#[derive(Clone, Copy)]
struct RowDrag {
    from: usize,
    press: (f32, f32),
    /// The pressed row's rect, so the ghost keeps the grab point under the pointer.
    source: Rect,
    to: Option<usize>,
    pointer: (f32, f32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResizeCursor {
    Horizontal,
    Vertical,
}

pub struct WorkspaceView {
    /// Where the open modal was drawn last frame, for dismissing it on outside presses.
    modal_rect: Option<Rect>,
    /// A form over the whole window; it takes the keyboard while open.
    window_modal: Option<Box<dyn crate::WindowModal>>,
    /// How the last window modal ended, until the app takes it.
    modal_result: Option<crate::ModalResult>,
    workspace_ops: Vec<crate::WorkspaceOp>,
    /// Operation cards showing their stages.
    expanded_ops: Vec<u64>,
    /// The failed operation whose menu of manual fixes is open.
    manual_menu: Option<u64>,
    /// The failed operation shown in a popover beside its rail tile, and where that popover was drawn.
    rail_popover: Option<u64>,
    rail_popover_rect: Option<Rect>,
    manual_menu_rect: Option<Rect>,
    /// The agent dock's open popover and the button it drops from.
    agent_popover: Option<(crate::agent_popover::AgentPopoverKind, Rect)>,
    agent_popover_rect: Option<Rect>,
    /// The usage card open under the title bar chip (`false`) or over the status item (`true`), and its anchor.
    usage_popover: Option<(bool, Rect)>,
    usage_popover_rect: Option<Rect>,
    /// The list grouped by ticket status, when that setting is on.
    grouping: Option<crate::Grouping>,
    group_drag: Option<GroupDrag>,
    /// The WORKSPACES row a context menu was opened on.
    menu_workspace: Option<usize>,
    workspace_requests: WorkspaceRequests,
    /// Where the caret popover was drawn last frame, so scrolling over it scrolls it.
    popover_rects: Vec<Rect>,
    /// For each drawn popover, the pane group it belongs to and its index there.
    popover_groups: Vec<(InputGroup, usize)>,
    /// The project's saved panes were loaded (done once, before the first frame).
    panes_restored: bool,
    tab_restore: TabRestore,
    /// The panes as last written, and when the pending write is due.
    saved_panes: Option<String>,
    panes_write_at: Option<Instant>,
    /// When the panes were last compared with what is saved; a frame inside the throttle leaves a check owed.
    panes_checked_at: Option<Instant>,
    panes_check_owed: bool,
    /// Input arrived since the panes were last compared with what is saved.
    panes_input: bool,
    /// The zoom showing this frame, for drawing and routing input.
    zoom: Option<Zoom>,
    pending_prompt: Option<crate::Prompt>,
    agent_fix: Option<(crate::AgentFix, crate::SideAgentRole)>,
    /// The side panel prompt waiting for an answer: its token, panel and the panel's tag.
    panel_prompt: Option<(u64, PaneKind, u64)>,
    /// Branch changes the Git panel asked for, which the app carries out.
    branch_requests: Vec<crate::PanelRequest>,
    /// A tab close waiting on the save prompt: its token, pane group and request.
    close_prompt: Option<(u64, InputGroup, crate::pane_group_view::CloseRequest)>,
    next_prompt_token: u64,
    terminal_focused: bool,
    agent_focused: bool,
    /// The confirmation on screen and its highlighted button; it takes all input until answered.
    prompt_shown: Option<(crate::Prompt, usize)>,
    bindings: Vec<(crate::keymap::Action, String)>,
    pointer: (f32, f32),
    press: (f32, f32),
    show_after_switch: Option<PaneKind>,
    toast_then: Option<(PaneKind, crate::PanelRequest)>,
    /// When main's background update last succeeded; its row says "Up to date" for a moment after.
    upkeep_done_at: Option<Instant>,
    /// Time, place and count of the last press in the terminal grid, for double/triple-click selection.
    terminal_click: Option<(Instant, f32, f32, u32)>,
    titlebar_press: bool,
    titlebar_click: Option<(Instant, f32, f32)>,
    layout: Layout,
    session_menu_hover: Option<u64>,
    session_search_query: String,
    viewport: (f32, f32),
    header_hits: Vec<(Rect, u64)>,
    dragging: Drag,
    pending_tab: Option<(u64, f32, f32)>,
    row_drag: Option<RowDrag>,
    tab_ghost_at: Option<(f32, f32)>,
    last_click: Option<(Instant, f32, f32)>,
    /// An open right-click context menu: `(anchor_x, anchor_top, anchor_bottom, target button id)`.
    menu: Option<(f32, f32, f32, u64)>,
    /// The entries of the menu a tab's toolbar button opened.
    item_menu: Vec<MenuItem>,
    menu_path: Option<(String, bool)>,
    submenu: Option<(f32, f32, f32, u64)>,
    menu_editor_anchor: Option<(Vec<usize>, usize)>,
    /// The tab a tab context menu was opened on: its group, pane id and index.
    menu_tab: Option<(InputGroup, u64, usize)>,
    /// The pane group an editor context menu was opened in.
    menu_group: InputGroup,
    /// The terminal a terminal context menu was opened on: its group, pane path and what it showed then.
    menu_terminal: Option<(InputGroup, Vec<usize>, crate::TerminalContext)>,
    toast: Option<Toast>,
    notification: Option<Notification>,
    server_notices: std::collections::VecDeque<crate::ServerNotice>,
    shown_problem: Option<String>,
    /// How far the WORKSPACES list (or rail) is scrolled, and how far it can go.
    workspace_scroll: f32,
    workspace_scroll_max: f32,
    /// The coding agent is installed: setup problems offer "Fix with Claude", else point at pom.yml.
    ai_available: bool,
    dismissed_setup: Option<String>,
    pending: WorkspaceEffects,
    /// A page tab (onboarding) opened before this window had a pane group to hold it: drawn on its own in the
    /// editor area while there is no project, moved into the panes once there are some.
    page: Option<crate::pane::Pane>,
    page_body: Option<Rect>,
}

impl WorkspaceView {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            session_menu_hover: None,
            session_search_query: String::new(),
            viewport: (0.0, 0.0),
            header_hits: Vec::new(),
            dragging: Drag::None,
            pending_tab: None,
            row_drag: None,
            tab_ghost_at: None,
            last_click: None,
            menu: None,
            item_menu: Vec::new(),
            menu_path: None,
            submenu: None,
            menu_editor_anchor: None,
            menu_tab: None,
            menu_group: InputGroup::Center,
            menu_terminal: None,
            modal_rect: None,
            window_modal: None,
            modal_result: None,
            workspace_ops: Vec::new(),
            expanded_ops: Vec::new(),
            manual_menu: None,
            rail_popover: None,
            rail_popover_rect: None,
            manual_menu_rect: None,
            agent_popover: None,
            agent_popover_rect: None,
            usage_popover: None,
            usage_popover_rect: None,
            grouping: None,
            group_drag: None,
            menu_workspace: None,
            workspace_requests: WorkspaceRequests::default(),
            popover_rects: Vec::new(),
            popover_groups: Vec::new(),
            panes_restored: false,
            tab_restore: TabRestore::default(),
            saved_panes: None,
            panes_write_at: None,
            panes_checked_at: None,
            panes_check_owed: false,
            panes_input: true,
            zoom: None,
            pending_prompt: None,
            agent_fix: None,
            panel_prompt: None,
            branch_requests: Vec::new(),
            close_prompt: None,
            next_prompt_token: CLOSE_PROMPT_TOKENS,
            terminal_focused: false,
            agent_focused: false,
            prompt_shown: None,
            bindings: Vec::new(),
            pointer: (0.0, 0.0),
            press: (0.0, 0.0),
            show_after_switch: None,
            toast_then: None,
            upkeep_done_at: None,
            terminal_click: None,
            titlebar_press: false,
            titlebar_click: None,
            toast: None,
            notification: None,
            server_notices: std::collections::VecDeque::new(),
            shown_problem: None,
            workspace_scroll: 0.0,
            workspace_scroll_max: 0.0,
            ai_available: true,
            dismissed_setup: None,
            pending: WorkspaceEffects::default(),
            page: None,
            page_body: None,
        }
    }

    pub fn set_project(
        &mut self,
        project: Option<crate::ProjectInfo>,
        files_view: Option<Box<dyn crate::FunctionView>>,
        terminal_view: Option<Box<dyn crate::TerminalPanelView>>,
        agent_view: Option<Box<dyn crate::TerminalPanelView>>,
    ) {
        self.persist_panes(true);
        self.layout.project = project;
        self.layout.files_view = files_view;
        self.layout.terminal_view = terminal_view;
        self.layout.agent_view = agent_view;
        self.layout.side_panels.clear();
        self.layout.session_menu = false;
        self.panes_restored = false;
        self.saved_panes = None;
        self.panes_write_at = None;
        self.panes_check_owed = false;
        self.panes_input = true;
        self.terminal_focused = false;
        self.agent_focused = false;
        self.zoom = None;
        self.menu = None;
        self.notification = None;
        self.shown_problem = None;
    }

    /// The side panel's base layer; its own overlays (tooltips) go to `overlays` so they draw above its text.
    fn function_panel_painted(
        &mut self,
        side: DockPosition,
        region: Rect,
        overlays: &mut Vec<Overlay>,
    ) -> Option<Painted> {
        let Some(Shown::Func(kind)) = self.layout.shown_on(side) else {
            return None;
        };
        let scale = ui::ui_text_scale();
        let node = match self.layout.side_panel_on(side) {
            Some(panel) => panel.render(region.w / scale, region.h / scale),
            None => function_dock_body(kind),
        };
        let frame = ui::paint_frame(&node, region);
        overlays.extend(frame.overlays.into_iter().map(|overlay| Overlay {
            painted: overlay.painted,
            clip: Some(overlay.clip.unwrap_or(region)),
        }));
        Some(Painted {
            hits: frame.hits,
            ..frame.base
        })
    }

    /// Take this workspace's live views and dock visibility out of the window, to bring back later with
    /// `resume` exactly as they were (open tabs, running terminals, which docks showed what).
    pub fn park(&mut self) -> ParkedWorkspace {
        self.persist_panes(true);
        ParkedWorkspace {
            files: self.layout.files_view.take(),
            terminal: self.layout.terminal_view.take(),
            agent: self.layout.agent_view.take(),
            side_panels: std::mem::take(&mut self.layout.side_panels),
            active_panels: self.layout.active_panels,
            right_collapsed: self.layout.right.collapsed,
            bottom_collapsed: self.layout.bottom.collapsed,
        }
    }

    /// Put back a parked workspace: its views are live, so nothing is restored from disk.
    pub fn resume(&mut self, project: crate::ProjectInfo, parked: ParkedWorkspace) {
        self.set_project(Some(project), parked.files, parked.terminal, parked.agent);
        self.layout.side_panels = parked.side_panels;
        self.layout.active_panels = parked.active_panels;
        self.layout.right.collapsed = parked.right_collapsed;
        self.layout.bottom.collapsed = parked.bottom_collapsed;
        self.panes_restored = true;
        self.saved_panes = self.panes_state().map(|(_, json)| json);
        if let Some(kind) = self.show_after_switch.take() {
            self.show_function(kind);
        }
    }

    pub fn set_side_panels(&mut self, panels: Vec<Box<dyn crate::SidePanelView>>) {
        self.layout.side_panels = panels;
        if let Some(kind) = self.show_after_switch.take() {
            self.show_function(kind);
        }
    }

    fn show_function(&mut self, kind: PaneKind) {
        let Some(index) = PaneKind::ALL.iter().position(|each| *each == kind) else {
            return;
        };
        let side = self
            .layout
            .func_side
            .get(index)
            .copied()
            .unwrap_or(DockPosition::Left);
        self.layout.active_panels[side.index()] = Some(Shown::Func(kind));
        self.toggle_side(side, false);
        self.pending.persist = true;
    }

    /// Runs a keymap or palette action that belongs to the window itself; false for the ones the app runs
    /// (settings, projects, themes) and when a modal or prompt holds the keyboard.
    pub fn run_action(&mut self, action: crate::keymap::Action) -> bool {
        use crate::keymap::Action;
        if self.prompt_shown.is_some() || self.window_modal.is_some() {
            return false;
        }
        let function = |kind: PaneKind| {
            PaneKind::ALL
                .iter()
                .position(|each| *each == kind)
                .map(|index| FUNC_BASE + index as u64)
        };
        match action {
            Action::ToggleLeftDock => self.header_click(SIDEBAR_TOGGLE),
            Action::ToggleRightDock => self.header_click(RIGHT_TOGGLE),
            Action::ToggleBottomDock => {
                self.layout.bottom.collapsed = !self.layout.bottom.collapsed;
                self.pending.persist = true;
            }
            Action::FocusFiles
            | Action::FocusGit
            | Action::FocusServices
            | Action::FocusDatabase
            | Action::FocusPullRequests => {
                let kind = match action {
                    Action::FocusFiles => PaneKind::Files,
                    Action::FocusServices => PaneKind::Services,
                    Action::FocusDatabase => PaneKind::Database,
                    _ => PaneKind::Git,
                };
                if action == Action::FocusPullRequests {
                    self.show_function(kind);
                } else if let Some(id) = function(kind) {
                    self.header_click(id);
                }
            }
            Action::ToggleAgent => self.header_click(AGENT_TOGGLE),
            Action::ToggleTerminal => self.toggle_terminal(),
            Action::NewTerminal => self.open_terminal_at(None),
            Action::CloseActiveItem | Action::CloseAllItems => {
                let command = if action == Action::CloseActiveItem {
                    crate::pane::PaneCommand::CloseActiveItem
                } else {
                    crate::pane::PaneCommand::CloseAllItems
                };
                let group = self.focused_group();
                let handled = self
                    .group_view_mut(group)
                    .is_some_and(|panes| panes.pane_command(command));
                self.ask_about_pending_close();
                return handled;
            }
            Action::SwitchWorkspace => {
                let Some(project) = self.layout.project.as_ref() else {
                    return false;
                };
                let entries: Vec<crate::ExtraCommand> = project
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(index, branch)| {
                        let label = project.label(index);
                        crate::ExtraCommand {
                            name: if label == branch.as_str() {
                                branch.clone()
                            } else {
                                format!("{label}  {branch}")
                            },
                            keys: Vec::new(),
                            id: PALETTE_WORKSPACE_BASE + index as u64,
                        }
                    })
                    .collect();
                self.set_terminal_focus(false);
                self.set_agent_focus(false);
                match self.layout.files_view.as_mut() {
                    Some(files) => files.open_command_list(entries, "Switch to workspace..."),
                    None => return false,
                }
            }
            Action::FileFinder => self.open_file_finder(),
            Action::ProjectSearch => self.deploy_project_search(),
            Action::ProjectDiagnostics => self.deploy_project_diagnostics(false),
            Action::SelectLanguage => {
                self.set_terminal_focus(false);
                return self
                    .layout
                    .files_view
                    .as_mut()
                    .is_some_and(|files| files.editor_key(EditKey::ToggleLanguageSelector, false));
            }
            Action::MarkdownPreview | Action::MarkdownPreviewToTheSide => {
                let key = if action == Action::MarkdownPreview {
                    EditKey::OpenMarkdownPreview
                } else {
                    EditKey::OpenMarkdownPreviewToTheSide
                };
                self.set_terminal_focus(false);
                return self
                    .layout
                    .files_view
                    .as_mut()
                    .is_some_and(|files| files.editor_key(key, false));
            }
            Action::ActivateTab(index) => {
                return self
                    .pane_command(crate::pane::PaneCommand::ActivateItem(usize::from(index)))
            }
            Action::ActivateLastTab => {
                return self.pane_command(crate::pane::PaneCommand::ActivateLastItem)
            }
            Action::ActivatePreviousTab => {
                return self.pane_command(crate::pane::PaneCommand::ActivatePreviousItem)
            }
            Action::ActivateNextTab => {
                return self.pane_command(crate::pane::PaneCommand::ActivateNextItem)
            }
            _ => return false,
        }
        true
    }

    /// Show a form over the window (replacing any open one).
    /// Shows a page tab (by its id): brought forward when already open, else opened in the panes, or on its own
    /// while the window has none.
    pub fn open_page(&mut self, item: Box<dyn crate::Item>) {
        let id = item.id().unwrap_or_default();
        if self
            .page
            .as_ref()
            .is_some_and(|page| page.index_of_id(&id).is_some())
        {
            return;
        }
        if self.panes_restored {
            if let Some(files) = self.layout.files_view.as_mut() {
                let revealed = files
                    .pane_group_mut()
                    .is_some_and(|group| group.reveal_item(&id));
                if !revealed {
                    files.add_center_item(item);
                }
                self.set_terminal_focus(false);
                self.panes_input = true;
                return;
            }
        }
        let mut pane = crate::pane::Pane::new(0);
        pane.add_item(item);
        self.page = Some(pane);
        self.set_terminal_focus(false);
    }

    pub fn close_page(&mut self, id: &str) {
        if self
            .page
            .as_ref()
            .is_some_and(|page| page.index_of_id(id).is_some())
        {
            if let Some(mut page) = self.page.take() {
                page.close_tab(0);
            }
            return;
        }
        if let Some(group) = self
            .layout
            .files_view
            .as_mut()
            .and_then(|files| files.pane_group_mut())
        {
            group.close_item(id);
        }
    }

    /// The page drawn on its own, when the window has no panes to hold it.
    fn standalone_page(&mut self) -> Option<&mut Box<dyn crate::Item>> {
        if self.layout.files_view.is_some() {
            return None;
        }
        self.page.as_mut()?.open.first_mut()
    }

    fn page_body_at(&self, x: f32, y: f32) -> bool {
        self.layout.files_view.is_none()
            && self
                .page_body
                .is_some_and(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
    }

    pub fn open_window_modal(&mut self, modal: Box<dyn crate::WindowModal>) {
        self.menu = None;
        self.layout.session_menu = false;
        self.window_modal = Some(modal);
    }

    pub fn window_modal_open(&self) -> bool {
        self.window_modal.is_some()
    }

    /// How the last window modal ended (submitted or cancelled), once.
    pub fn take_modal_result(&mut self) -> Option<crate::ModalResult> {
        self.modal_result.take()
    }

    /// Polls the open modal's background work; returns whether it needs a redraw.
    pub fn tick_window_modal(&mut self) -> bool {
        let changed = self.window_modal.as_mut().is_some_and(|modal| modal.tick());
        self.settle_window_modal();
        changed
    }

    fn settle_window_modal(&mut self) {
        let result = self
            .window_modal
            .as_mut()
            .and_then(|modal| modal.take_result());
        if let Some(result) = result {
            self.window_modal = None;
            self.modal_result = Some(result);
        }
    }

    fn cancel_window_modal(&mut self) {
        if self.window_modal.take().is_some() {
            self.modal_result = Some(crate::ModalResult::Cancelled);
        }
    }

    /// Groups the WORKSPACES list by ticket status in this arrangement, or lists it flat with `None`.
    pub fn set_workspace_grouping(&mut self, grouping: Option<crate::Grouping>) {
        self.grouping = grouping;
    }

    pub fn workspace_grouping(&self) -> Option<&crate::Grouping> {
        self.grouping.as_ref()
    }

    /// The ticket group of the workspace at `index`.
    fn row_group(&self, index: usize) -> crate::TicketGroup {
        let project = self.layout.project.as_ref();
        let field = |values: Option<&Vec<String>>| {
            values
                .and_then(|values| values.get(index))
                .cloned()
                .unwrap_or_default()
        };
        crate::TicketGroup::of(
            &field(project.map(|project| &project.tickets)),
            &field(project.map(|project| &project.ticket_categories)),
        )
    }

    fn toggle_group_fold(&mut self, group: crate::TicketGroup) {
        if let Some(grouping) = self.grouping.as_mut() {
            match grouping.folded.iter().position(|folded| *folded == group) {
                Some(at) => {
                    grouping.folded.remove(at);
                }
                None => grouping.folded.push(group),
            }
            self.workspace_requests.grouping_changed = true;
        }
    }

    fn move_group(&mut self, from: crate::TicketGroup, to: crate::TicketGroup) {
        if let Some(grouping) = self.grouping.as_mut() {
            let (Some(at), Some(target)) = (
                grouping.order.iter().position(|group| *group == from),
                grouping.order.iter().position(|group| *group == to),
            ) else {
                return;
            };
            let group = grouping.order.remove(at);
            grouping.order.insert(target, group);
            self.workspace_requests.grouping_changed = true;
        }
    }

    fn update_group_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(mut drag) = self.group_drag else {
            return false;
        };
        drag.to = self
            .header_hits
            .iter()
            .filter(|(rect, id)| {
                (crate::WORKSPACE_GROUP_BASE..crate::WORKSPACE_GROUP_END).contains(id)
                    && x >= rect.x
                    && x < rect.x + rect.w
                    && y >= rect.y
                    && y < rect.y + rect.h
            })
            .filter_map(|(_, id)| {
                crate::TicketGroup::ALL
                    .get((id - crate::WORKSPACE_GROUP_BASE) as usize)
                    .copied()
            })
            .next_back()
            .filter(|group| *group != drag.from);
        drag.pointer = (x, y);
        self.group_drag = Some(drag);
        true
    }

    /// The group header a dragged group would take, tinted with a bar on the side it lands.
    fn group_drop_painted(&self, hits: &[(Rect, u64)]) -> Option<Painted> {
        let drag = self
            .group_drag
            .filter(|_| self.dragging == Drag::WorkspaceGroup)?;
        let to = drag.to?;
        let order = &self.grouping.as_ref()?.order;
        let before = order.iter().position(|group| *group == to)
            < order.iter().position(|group| *group == drag.from);
        let (rect, _) = hits
            .iter()
            .find(|(_, id)| *id == crate::WORKSPACE_GROUP_BASE + to.index() as u64)?;
        let mut drop = Painted::default();
        drop.rects.push(Rect::new(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            ui::theme().text_accent.alpha(0.22),
        ));
        let bar_y = if before {
            rect.y
        } else {
            rect.y + rect.h - 2.0
        };
        drop.rects.push(Rect::new(
            rect.x,
            bar_y,
            rect.w,
            2.0,
            ui::theme().border_focused,
        ));
        Some(drop)
    }

    /// The open menu of manual fixes, dropped from its button (in the list or a rail popover), or lifted above
    /// it when there is no room below.
    fn manual_menu_painted(&self, hits: &[(Rect, u64)], w: f32, h: f32) -> Option<Painted> {
        let op_id = self.manual_menu?;
        let (position, op) = self
            .workspace_ops
            .iter()
            .enumerate()
            .find(|(_, op)| op.id == op_id)?;
        let target = crate::WORKSPACE_OP_BASE
            + position as u64 * crate::WORKSPACE_OP_STRIDE
            + crate::WORKSPACE_OP_MANUAL;
        let (button, _) = hits.iter().rev().find(|(_, id)| *id == target)?;
        let scale = ui::ui_text_scale();
        // As wide as the details it drops from: from the button to the card's right edge.
        let right = match self
            .rail_popover_rect
            .filter(|_| self.rail_popover == Some(op_id))
        {
            Some(card) => card.x + card.w - 10.0 * scale,
            None => {
                let region = self.layout.left_region(w, h);
                region.x + region.w - 18.0 * scale
            }
        };
        let width = ((right - button.x) / scale).clamp(160.0, 280.0);
        let node: ui::Node = ui::div()
            .col()
            .child(
                ui::div()
                    .row()
                    .child(crate::panel::manual_menu(op, position, width)),
            )
            .into();
        let place = |y: f32| {
            ui::render(
                &node,
                Rect::new(button.x, y, w - button.x, h, Rgba::TRANSPARENT),
            )
        };
        let below = button.y + button.h + 4.0 * scale;
        let painted = place(below);
        let bottom = painted
            .rects
            .iter()
            .map(|r| r.y + r.h)
            .fold(below, f32::max);
        if bottom <= h - 8.0 * scale {
            return Some(painted);
        }
        let height = bottom - below;
        Some(place((button.y - 4.0 * scale - height).max(8.0 * scale)))
    }

    /// Opens a failure's menu of manual fixes (as its button does).
    pub fn toggle_manual_fixes(&mut self, op_id: u64) {
        self.manual_menu = (self.manual_menu != Some(op_id)).then_some(op_id);
    }

    /// Opens a failure's details beside its rail tile (as its "!" does).
    pub fn toggle_failure_popover(&mut self, op_id: u64) {
        self.rail_popover = (self.rail_popover != Some(op_id)).then_some(op_id);
    }

    /// Shows or hides an operation card's stages.
    pub fn toggle_workspace_op(&mut self, op_id: u64) {
        match self.expanded_ops.iter().position(|id| *id == op_id) {
            Some(at) => {
                self.expanded_ops.remove(at);
            }
            None => self.expanded_ops.push(op_id),
        }
    }

    pub fn take_workspace_requests(&mut self) -> WorkspaceRequests {
        std::mem::take(&mut self.workspace_requests)
    }

    /// The `KeepBranch`, `SwitchBranch` and `PickBranch` requests side panels made since the last call.
    pub fn take_branch_requests(&mut self) -> Vec<crate::PanelRequest> {
        std::mem::take(&mut self.branch_requests)
    }

    pub fn set_workspace_ops(&mut self, ops: Vec<crate::WorkspaceOp>) {
        self.expanded_ops
            .retain(|id| ops.iter().any(|op| op.id == *id));
        let still_failed = |id: &u64| {
            ops.iter()
                .any(|op| op.id == *id && op.status == crate::OpStatus::Failed)
        };
        if !self.manual_menu.as_ref().is_some_and(still_failed) {
            self.manual_menu = None;
        }
        if !self.rail_popover.as_ref().is_some_and(still_failed) {
            self.rail_popover = None;
        }
        // A background update that ran and is gone without failing succeeded (failures stay in the queue).
        let succeeded = self.workspace_ops.iter().any(|old| {
            old.quiet
                && matches!(
                    old.status,
                    crate::OpStatus::Queued | crate::OpStatus::Running
                )
                && !ops.iter().any(|op| op.id == old.id)
        });
        if succeeded {
            self.upkeep_done_at = Some(Instant::now());
        }
        if ops.iter().any(|op| op.quiet) {
            self.upkeep_done_at = None;
        }
        self.workspace_ops = ops;
    }

    pub fn set_agent_states(&mut self, states: std::collections::HashMap<String, crate::AgentDot>) {
        self.layout.agent_states = states;
    }

    /// What the agent dock's popovers show: start sizes, the main session's, the history, a second CLI.
    pub fn set_side_agents(
        &mut self,
        sizes: [Option<usize>; 4],
        main_tokens: Option<usize>,
        archived: Vec<crate::agent_popover::ArchivedAgent>,
        other_cli: Option<String>,
    ) {
        self.layout.side_agent_sizes = sizes;
        self.layout.main_agent_tokens = main_tokens;
        self.layout.archived_agents = archived;
        self.layout.other_cli = other_cli;
    }

    /// The active editor's file, else the workspace folder: what an external editor should open.
    pub fn external_target(&self) -> Option<std::path::PathBuf> {
        let files = self.layout.files_view.as_ref()?;
        files.active_file_path().or_else(|| files.root_dir())
    }

    pub fn open_file(&mut self, path: &std::path::Path) {
        if let Some(files) = self.layout.files_view.as_mut() {
            files.open_file_at(path, None, None);
            self.set_terminal_focus(false);
            self.set_agent_focus(false);
            self.panes_input = true;
        }
    }

    pub fn editor_metrics_changed(&mut self) {
        if let Some(files) = self.layout.files_view.as_mut() {
            files.editor_metrics_changed();
        }
    }

    /// Returns whether the badge changed.
    pub fn set_services_running(&mut self, running: bool) -> bool {
        std::mem::replace(&mut self.layout.services_running, running) != running
    }

    pub fn update_project(&mut self, project: crate::ProjectInfo) {
        self.layout.project = Some(project);
    }

    pub fn set_machine_checks(&mut self, checks: Vec<crate::MachineCheck>) {
        self.layout.machine = checks;
    }

    pub fn set_sessions(&mut self, sessions: Vec<crate::Session>, current: Option<usize>) {
        self.layout.sessions = sessions;
        self.layout.current_session = current;
    }

    pub fn set_config_problem(
        &mut self,
        problem: Option<(String, Option<u32>)>,
        config_path: &std::path::Path,
    ) {
        let Some((message, line)) = problem else {
            if self.shown_problem.take().is_some() {
                self.notification = None;
            }
            return;
        };
        if self.shown_problem.as_deref() == Some(message.as_str()) {
            return;
        }
        self.shown_problem = Some(message.clone());
        self.notification = Some(Notification {
            title: "Invalid pom.yml".into(),
            message,
            primary: Some("Open pom.yml".into()),
            action: Some(NotificationAction::OpenFile(
                config_path.to_path_buf(),
                line,
            )),
            ..Notification::default()
        });
    }

    /// Running services started with the previous config: offer to restart them (none clears the offer).
    /// Returns whether the header has to move.
    /// Which status bar buttons show; answers whether any changed.
    pub fn set_buttons_hidden(
        &mut self,
        agent_hidden: bool,
        terminal_hidden: bool,
        func_hidden: &[bool],
    ) -> bool {
        let mut changed = self.layout.agent_hidden != agent_hidden
            || self.layout.terminal_hidden != terminal_hidden;
        self.layout.agent_hidden = agent_hidden;
        self.layout.terminal_hidden = terminal_hidden;
        for (slot, hidden) in self.layout.func_hidden.iter_mut().zip(func_hidden) {
            changed |= *slot != *hidden;
            *slot = *hidden;
        }
        changed
    }

    pub fn set_fullscreen(&mut self, fullscreen: bool) -> bool {
        let changed = self.layout.fullscreen != fullscreen;
        self.layout.fullscreen = fullscreen;
        changed
    }

    pub fn set_stale_services(&mut self, names: &[String]) {
        let showing = matches!(
            self.notification.as_ref().and_then(|n| n.action.as_ref()),
            Some(NotificationAction::RestartStale)
        );
        if names.is_empty() {
            if showing {
                self.notification = None;
            }
            return;
        }
        let noun = if names.len() == 1 {
            "service"
        } else {
            "services"
        };
        self.notification = Some(Notification {
            title: format!("{} {noun} still run the old config", names.len()),
            message: names.join(", "),
            primary: Some("Restart".into()),
            action: Some(NotificationAction::RestartStale),
            ..Notification::default()
        });
    }

    /// Leave the saved tabs closed and untouched; `reopen_saved_tabs` brings them back.
    pub fn hold_saved_tabs(&mut self) {
        self.tab_restore = TabRestore::Hold;
    }

    /// Open the project without its saved tabs, saving what is open from then on.
    pub fn skip_saved_tabs_once(&mut self) {
        self.tab_restore = TabRestore::SkipOnce;
    }

    pub fn reopen_saved_tabs(&mut self) {
        self.tab_restore = TabRestore::Restore;
        self.panes_restored = false;
    }

    /// After a crash at startup: the tabs stayed closed, and the button reopens them.
    pub fn notify_recovery(&mut self, title: String, message: String) {
        self.notification = Some(Notification {
            title,
            message,
            primary: Some("Reopen Tabs".into()),
            action: Some(NotificationAction::ReopenTabs),
            level: Some(crate::NoticeLevel::Warning),
            ..Notification::default()
        });
    }

    /// A release that may fix the crash is downloaded; the button restarts into it.
    pub fn notify_update_ready(&mut self, title: String, message: String) {
        self.notification = Some(Notification {
            title,
            message,
            primary: Some("Restart to Update".into()),
            action: Some(NotificationAction::RestartUpdate),
            ..Notification::default()
        });
    }

    pub fn notify_release_notes(&mut self, title: String, message: String, button: &str) {
        self.notification = Some(Notification {
            title,
            message,
            primary: Some(button.into()),
            action: Some(NotificationAction::ReleaseNotes),
            ..Notification::default()
        });
    }

    pub fn open_markdown_preview(&mut self, path: &std::path::Path) -> bool {
        if self.layout.files_view.is_none() {
            return false;
        }
        self.open_file(path);
        self.layout
            .files_view
            .as_mut()
            .is_some_and(|files| files.editor_key(EditKey::OpenMarkdownPreview, false))
    }

    pub fn set_ai_available(&mut self, available: bool) {
        self.ai_available = available;
    }

    pub fn dismiss_notification(&mut self, title: &str) {
        if self
            .notification
            .as_ref()
            .is_some_and(|notification| notification.title == title)
        {
            self.notification = None;
        }
    }

    /// A notification whose button opens `path` (at `line`), e.g. the drafted config of a new project.
    pub fn notify_with_file(
        &mut self,
        title: &str,
        message: String,
        button: &str,
        path: std::path::PathBuf,
        line: Option<u32>,
    ) {
        self.notification = Some(Notification {
            title: title.into(),
            message,
            primary: Some(button.into()),
            action: Some(NotificationAction::OpenFile(path, line)),
            ..Notification::default()
        });
    }

    pub fn set_setup_problems(&mut self, summary: Option<String>) {
        if self.shown_problem.is_some() {
            return;
        }
        let showing = self
            .notification
            .as_ref()
            .is_some_and(|notification| notification.title == SETUP_TITLE);
        match summary {
            Some(message) => {
                if showing
                    && self
                        .notification
                        .as_ref()
                        .is_some_and(|notification| notification.message == message)
                {
                    return;
                }
                if self.dismissed_setup.as_deref() == Some(message.as_str()) {
                    return;
                }
                let config_path = self
                    .layout
                    .project
                    .as_ref()
                    .map(|project| project.config_path.clone());
                let (primary, action) = match config_path.filter(|_| !self.ai_available) {
                    Some(path) => ("Open pom.yml", NotificationAction::OpenFile(path, None)),
                    None => ("Fix with Claude", NotificationAction::FixSetup),
                };
                self.notification = Some(Notification {
                    title: SETUP_TITLE.into(),
                    message,
                    primary: Some(primary.into()),
                    action: Some(action),
                    ..Notification::default()
                });
            }
            None if showing => self.notification = None,
            None => {}
        }
    }

    pub fn notification_text(&self) -> Option<(&str, &str)> {
        self.notification
            .as_ref()
            .map(|n| (n.title.as_str(), n.message.as_str()))
    }

    pub fn show_toast(&mut self, message: impl Into<String>, action: Option<String>) {
        let now = Instant::now();
        self.toast = Some(Toast {
            message: message.into(),
            action,
            shown_at: now,
            deadline: now + TOAST_DISMISS,
            hovered: false,
            remaining: TOAST_DISMISS,
            rect: Rect::new(0.0, 0.0, 0.0, 0.0, Rgba::TRANSPARENT),
        });
    }

    /// An item on screen runs an animation and wants every frame.
    pub fn animating(&self) -> bool {
        self.page
            .as_ref()
            .and_then(|page| page.active_item())
            .is_some_and(|item| item.animating())
            || self
                .layout
                .files_view
                .as_ref()
                .is_some_and(|view| view.animating())
    }

    pub fn ticking(&self) -> bool {
        self.toast.is_some()
            || self
                .page
                .as_ref()
                .and_then(|page| page.active_item())
                .is_some_and(|item| item.is_busy())
            || self.upkeep_done_at.is_some()
            || self.window_modal.as_ref().is_some_and(|modal| modal.busy())
            || self.panes_write_at.is_some()
            || self.panes_check_owed
            || self.layout.files_view.as_ref().is_some_and(|v| v.is_busy())
            || self
                .layout
                .terminal_view
                .as_ref()
                .is_some_and(|view| view.panes_ref().is_busy())
            || self
                .layout
                .agent_view
                .as_ref()
                .is_some_and(|view| view.panes_ref().is_busy())
    }

    /// Bring back the project's saved panes, once.
    fn restore_saved_panes(&mut self) {
        if std::mem::replace(&mut self.panes_restored, true) {
            return;
        }
        match self.tab_restore {
            TabRestore::Hold => return,
            TabRestore::SkipOnce => {
                self.tab_restore = TabRestore::Restore;
                self.saved_panes = self.panes_state().map(|(_, json)| json);
                return;
            }
            TabRestore::Restore => {}
        }
        let Some(root) = self.layout.files_view.as_ref().and_then(|v| v.root_dir()) else {
            return;
        };
        if let Some(saved) = crate::persistence::load_workspace(&root) {
            if let (Some(center), Some(files)) = (&saved.center, self.layout.files_view.as_mut()) {
                let panels = &mut self.layout.side_panels;
                files.restore_panes(center, &mut |item| {
                    panels.iter_mut().find_map(|panel| panel.restore_item(item))
                });
            }
            if let (Some(panel), Some(view)) = (&saved.panel, self.layout.terminal_view.as_mut()) {
                if view.restore_panes(panel) && saved.panel_zoomed {
                    view.panes().toggle_zoom();
                }
            }
        }
        self.saved_panes = self.panes_state().map(|(_, json)| json);
    }

    fn panes_state(&self) -> Option<(std::path::PathBuf, String)> {
        let files = self.layout.files_view.as_ref()?;
        let root = files.root_dir()?;
        let state = crate::persistence::SerializedWorkspace {
            root: root.clone(),
            center: files.save_panes(),
            panel: self
                .layout
                .terminal_view
                .as_ref()
                .map(|view| view.save_panes()),
            panel_zoomed: self
                .layout
                .terminal_view
                .as_ref()
                .is_some_and(|view| view.panes_ref().is_zoomed()),
        };
        Some((root, serde_json::to_string_pretty(&state).ok()?))
    }

    /// Save the panes 200ms after they first change, taking in whatever else changed meanwhile; `flush` writes
    /// any change right away (on quit).
    pub fn persist_panes(&mut self, flush: bool) {
        if !self.panes_restored || self.tab_restore == TabRestore::Hold {
            return;
        }
        let now = Instant::now();
        // Saved state may carry whole unsaved files, so it is rebuilt at most once per throttle window.
        let recent = self
            .panes_checked_at
            .is_some_and(|at| now.duration_since(at) < PANES_SAVE_THROTTLE);
        if recent && !flush {
            self.panes_check_owed = true;
            return;
        }
        self.panes_checked_at = Some(now);
        self.panes_check_owed = false;
        self.panes_input = false;
        let Some((root, json)) = self.panes_state() else {
            return;
        };
        if self.saved_panes.as_deref() == Some(json.as_str()) {
            self.panes_write_at = None;
            return;
        }
        let due = *self.panes_write_at.get_or_insert(now + PANES_SAVE_THROTTLE);
        if !flush && now < due {
            return;
        }
        self.panes_write_at = None;
        match crate::persistence::save_workspace(&json, &root) {
            Ok(()) => self.saved_panes = Some(json),
            Err(error) => eprintln!("failed to save the workspace panes: {error}"),
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Called by the shell after every input it routes here, which is also when the panes may have changed.
    pub fn take_effects(&mut self) -> WorkspaceEffects {
        self.panes_input = true;
        self.run_picked_command();
        std::mem::take(&mut self.pending)
    }

    /// The keymap's bindings, shown next to the window commands in the palette.
    pub fn set_bindings(&mut self, bindings: Vec<(crate::keymap::Action, String)>) {
        self.bindings = bindings;
    }

    /// The window's commands for the palette: its actions, then what each side panel offers.
    fn palette_commands(&self) -> Vec<crate::ExtraCommand> {
        use crate::keymap::Action;
        let mut commands: Vec<crate::ExtraCommand> = Action::ALL
            .iter()
            .enumerate()
            .filter(|(_, action)| **action != Action::CommandPalette)
            .map(|(index, action)| crate::ExtraCommand {
                name: format!("workspace: {}", action.label().to_lowercase()),
                keys: self
                    .bindings
                    .iter()
                    .find(|(bound, _)| bound == action)
                    .map(|(_, keys)| keys.split_whitespace().map(str::to_string).collect())
                    .unwrap_or_default(),
                id: PALETTE_ACTION_BASE + index as u64,
            })
            .collect();
        for panel in &self.layout.side_panels {
            let base = PALETTE_PANEL_BASE + panel.kind() as u64 * PALETTE_PANEL_STRIDE;
            commands.extend(panel.palette_entries().into_iter().filter_map(|entry| {
                (entry.id < PALETTE_PANEL_STRIDE).then(|| crate::ExtraCommand {
                    name: entry.label,
                    keys: Vec::new(),
                    id: base + entry.id,
                })
            }));
        }
        commands
    }

    /// Runs the window command the palette handed back: actions this view owns here, the rest for the app,
    /// panel entries by their panel.
    fn run_picked_command(&mut self) {
        let Some(id) = self
            .layout
            .files_view
            .as_mut()
            .and_then(|view| view.take_extra_command())
        else {
            return;
        };
        if let Some(index) = id
            .checked_sub(PALETTE_WORKSPACE_BASE)
            .filter(|index| *index < PALETTE_PANEL_BASE - PALETTE_WORKSPACE_BASE)
        {
            self.pending.activate_workspace = Some(index as usize);
            return;
        }
        if let Some(index) = id
            .checked_sub(PALETTE_ACTION_BASE)
            .filter(|index| *index < PALETTE_WORKSPACE_BASE - PALETTE_ACTION_BASE)
        {
            let Some(action) = crate::keymap::Action::ALL.get(index as usize).copied() else {
                return;
            };
            if !self.run_action(action) {
                self.pending.action = Some(action);
            }
            return;
        }
        let Some(offset) = id.checked_sub(PALETTE_PANEL_BASE) else {
            return;
        };
        let kind = PaneKind::from_index((offset / PALETTE_PANEL_STRIDE) as usize);
        if let Some(panel) = kind.and_then(|kind| self.layout.side_panel_mut(kind)) {
            panel.run_palette_entry(offset % PALETTE_PANEL_STRIDE);
        }
        self.apply_panel_requests();
    }

    fn width(&self) -> f32 {
        self.viewport.0
    }

    fn build(&mut self, window: &Window) -> Frame {
        let (w, h) = (window.width, window.height);
        self.viewport = (w, h);
        self.restore_saved_panes();
        self.layout.app_menu_open = self
            .menu
            .is_some_and(|menu| menu.3 == crate::APP_MENU_TARGET);
        self.layout.usage_card_open = self.usage_popover.is_some_and(|(status, _)| !status);
        self.layout.usage_today_open = self.usage_popover.is_some_and(|(status, _)| status);
        if self.layout.files_view.is_some() {
            if let Some(mut page) = self.page.take() {
                if let Some(item) = page.open.pop() {
                    self.open_page(item);
                }
            }
        }
        self.apply_panel_requests();
        // Comparing panes with what is saved serializes every tab, so it runs only after input (or while a
        // write is pending), never on frames that nothing but a timer asked for.
        if self.panes_input || self.panes_check_owed || self.panes_write_at.is_some() {
            self.persist_panes(false);
        }
        self.sync_terminals();
        self.zoom = self.shown_zoom(w, h);
        let mut zoom_overlays: Vec<Overlay> = Vec::new();
        let mut zoom_hits: Vec<(Rect, u64)> = Vec::new();
        self.layout.sync_docks();
        let (mut rects, mut texts) = self.layout.build(w, h);
        let mut tris: Vec<ui::Tri> = Vec::new();
        let mut icons: Vec<ui::IconQuad> = Vec::new();

        let mut blit = |p: Painted| {
            rects.extend(p.rects);
            tris.extend(p.tris);
            texts.extend(p.texts);
            icons.extend(p.icons);
        };

        let mut panel_hits: Vec<(Rect, u64)> = Vec::new();
        // the center (main) area, Right in the right dock, Bottom in the bottom dock. The status-bar clusters
        let center_region = self.layout.center_region(w, h);
        let mut center_overlays: Vec<Overlay> = Vec::new();
        let mut scrollbar_overlay: Option<Painted> = None;
        let cr = center_region;
        let clamp = 4000.0; // slack so horizontally-scrolled rows still lay out fully
        let files_side = self.layout.files_side();
        let editor_in_center = self.layout.files_view.is_some()
            && (files_side.is_some()
                || self.layout.shown_on(DockPosition::Left) != Some(Shown::Terminal));
        if editor_in_center {
            {
                let tree_region = match files_side {
                    Some(DockPosition::Right) => self.layout.right_region(w, h),
                    Some(DockPosition::Bottom) => self.layout.bottom_region(w, h),
                    _ => self.layout.tree_region(w, h),
                };
                if let Some(files) = self.layout.files_view.as_mut() {
                    let panel = self.layout.terminal_view.as_mut().map(|view| view.panes());
                    files.sync_items(panel);
                }
                let (tp, mut editor) = {
                    let v = self.layout.files_view.as_mut().unwrap();
                    v.set_viewport(tree_region.w, tree_region.h);
                    let editor_area = match self.zoom {
                        Some(zoom) if zoom.group == InputGroup::Center => zoom.inner,
                        _ => cr,
                    };
                    let tree = files_side.and_then(|_| v.render_tree());
                    (tree, v.editor_layout(editor_area))
                };
                let (scroll, ch) = {
                    let v = self.layout.files_view.as_ref().unwrap();
                    (v.scroll_offset(), v.content_height())
                };
                blit(ui::render(
                    &ui::div().bg(ui::theme().editor_background).into(),
                    cr,
                ));
                if let Some(sr) = tp {
                    blit(ui::render(
                        &ui::div().bg(ui::theme().panel_background).into(),
                        Rect::new(
                            tree_region.x,
                            tree_region.y,
                            (tree_region.w - 1.0).max(0.0),
                            tree_region.h,
                            Rgba::TRANSPARENT,
                        ),
                    ));
                    let tree_area = Rect::new(
                        tree_region.x - sr.scroll_x,
                        tree_region.y + sr.y_offset,
                        tree_region.w + sr.scroll_x + clamp,
                        tree_region.h - sr.y_offset,
                        Rgba::TRANSPARENT,
                    );
                    let sticky_area = Rect::new(
                        tree_region.x - sr.scroll_x,
                        tree_region.y,
                        tree_region.w + sr.scroll_x + clamp,
                        tree_region.h,
                        Rgba::TRANSPARENT,
                    );
                    for (node, area) in [(&sr.tree, tree_area), (&sr.sticky, sticky_area)] {
                        let p = ui::render(node, area);
                        panel_hits.extend(clipped_hits(&p, tree_region));
                        center_overlays.push(Overlay {
                            painted: p,
                            clip: Some(tree_region),
                        });
                    }
                    let mut bar = Painted::default();
                    let thumb = ui::theme().scrollbar_thumb_background;
                    let tr = tree_region;
                    if ch > tr.h + 1.0 {
                        let thumb_h = (tr.h * tr.h / ch).max(28.0);
                        let t = scroll / (ch - tr.h);
                        bar.rects.push(Rect {
                            x: tr.x + tr.w - 7.0,
                            y: tr.y + t * (tr.h - thumb_h),
                            w: 4.0,
                            h: thumb_h,
                            color: thumb,
                            radius: 2.0,
                            border: 0.0,
                            border_color: Rgba::TRANSPARENT,
                        });
                    }
                    if sr.content_w > tr.w + 1.0 {
                        let thumb_w = (tr.w * tr.w / sr.content_w).max(28.0);
                        let t = sr.scroll_x / (sr.content_w - tr.w);
                        bar.rects.push(Rect {
                            x: tr.x + t * (tr.w - thumb_w),
                            y: tr.y + tr.h - 7.0,
                            w: thumb_w,
                            h: 4.0,
                            color: thumb,
                            radius: 2.0,
                            border: 0.0,
                            border_color: Rgba::TRANSPARENT,
                        });
                    }
                    if !bar.rects.is_empty() {
                        scrollbar_overlay = Some(bar);
                    }
                }
                // The left column shows its own panel whenever the file tree lives in another dock.
                if files_side != Some(DockPosition::Left) && self.layout.left_column_active() {
                    let region = self.layout.tree_region(w, h);
                    blit(ui::render(
                        &ui::div().bg(ui::theme().panel_background).into(),
                        Rect::new(
                            region.x,
                            region.y,
                            (region.w - 1.0).max(0.0),
                            region.h,
                            Rgba::TRANSPARENT,
                        ),
                    ));
                    let agent_here = self.layout.shown_on(DockPosition::Left) == Some(Shown::Agent);
                    if agent_here && self.layout.agent_visible() {
                        let p =
                            self.agent_painted(region, true, &mut center_overlays, &mut panel_hits);
                        panel_hits.extend(p.hits.iter().copied());
                        blit(p);
                    } else if agent_here {
                        let p = self.layout.right.render_panel(region);
                        panel_hits.extend(p.hits.iter().copied());
                        blit(p);
                    } else {
                        let mut above = Vec::new();
                        if let Some(p) =
                            self.function_panel_painted(DockPosition::Left, region, &mut above)
                        {
                            panel_hits.extend(clipped_hits(&p, region));
                            center_overlays.push(Overlay {
                                painted: p,
                                clip: Some(region),
                            });
                            center_overlays.append(&mut above);
                        }
                    }
                }
                match self.zoom {
                    Some(zoom) if zoom.group == InputGroup::Center => {
                        push_pane_group(&mut editor, zoom.inner, &mut zoom_overlays, &mut zoom_hits)
                    }
                    _ => push_pane_group(&mut editor, cr, &mut center_overlays, &mut panel_hits),
                }
                if let Some(hl) = self
                    .layout
                    .files_view
                    .as_ref()
                    .and_then(|v| v.tab_drag_overlay())
                {
                    let mut prev = Painted::default();
                    prev.rects.push(Rect {
                        x: hl.x,
                        y: hl.y,
                        w: hl.w,
                        h: hl.h,
                        color: ui::theme().text_accent.alpha(0.22),
                        radius: 0.0,
                        border: 0.0,
                        border_color: Rgba::TRANSPARENT,
                    });
                    center_overlays.push(Overlay {
                        painted: prev,
                        clip: Some(cr),
                    });
                }
            }
        } else if self.page.is_some() {
            let strip = self.page.as_ref().map(|page| {
                crate::pane::render_tab_strip(
                    page,
                    crate::pane::PaneClickIds {
                        tab_activate: PAGE_TAB,
                        tab_close: PAGE_TAB_CLOSE,
                        nav_back: PAGE_TAB,
                        nav_forward: PAGE_TAB,
                        search: PAGE_TAB,
                    },
                    self.session_menu_hover,
                )
            });
            let scale = ui::ui_text_scale();
            let bar_h = crate::pane::TAB_H * scale;
            let bar = ui::div()
                .row()
                .w_px(cr.w / scale)
                .bg(ui::theme().tab_bar_background)
                .children(strip)
                .child(
                    ui::div()
                        .col()
                        .flex(1.0)
                        .h_px(crate::pane::TAB_H)
                        .child(ui::div().flex(1.0))
                        .child(ui::div().h_px(1.0).bg(ui::theme().border)),
                );
            let p = ui::render(
                &bar.into(),
                Rect::new(cr.x, cr.y, cr.w, bar_h, Rgba::TRANSPARENT),
            );
            panel_hits.extend(p.hits.iter().copied());
            blit(p);
            let body = Rect::new(
                cr.x,
                cr.y + bar_h,
                cr.w,
                (cr.h - bar_h).max(0.0),
                Rgba::TRANSPARENT,
            );
            self.page_body = Some(body);
            let painted = self.page.as_mut().and_then(|page| {
                let item = page.open.first_mut()?;
                item.tick(&Self::clip_get);
                let painted = item.paint_body(body, true)?;
                Some((painted, item.paint_popover(body)))
            });
            if let Some((painted, popover)) = painted {
                center_overlays.push(Overlay {
                    painted,
                    clip: Some(body),
                });
                if let Some(popover) = popover {
                    center_overlays.push(Overlay {
                        painted: popover,
                        clip: Some(body),
                    });
                }
            }
        } else if self.layout.project.is_none() {
            let page = crate::welcome::welcome_page(
                &self.layout.sessions,
                &self.layout.machine,
                cr.w / ui::ui_text_scale(),
                self.session_menu_hover,
            );
            let p = ui::render(&page, cr);
            panel_hits.extend(p.hits.iter().copied());
            blit(p);
        } else {
            match self.layout.shown_on(DockPosition::Left) {
                Some(Shown::Terminal) => {
                    let p = self.terminal_painted(cr, true, &mut center_overlays, &mut panel_hits);
                    panel_hits.extend(p.hits.iter().copied());
                    blit(p)
                }
                Some(Shown::Func(k)) => blit(ui::render(&function_content(k), cr)),
                _ => blit(ui::render(
                    &ui::div().bg(ui::theme().editor_background).into(),
                    cr,
                )),
            }
        }
        let busy: Vec<&str> = self
            .workspace_ops
            .iter()
            .filter(|op| {
                matches!(
                    op.status,
                    crate::OpStatus::Queued | crate::OpStatus::Running
                )
            })
            .map(|op| op.branch.as_str())
            .collect();
        let workspaces: Vec<crate::panel::WorkspaceRow> = self
            .layout
            .project
            .iter()
            .flat_map(|project| {
                project
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(move |(index, branch)| (project, index, branch))
            })
            .filter(|(_, _, branch)| !busy.contains(&branch.as_str()))
            .map(|(project, index, branch)| crate::panel::WorkspaceRow {
                index,
                label: project.label(index).to_string(),
                branch: branch.clone(),
                agent: self.layout.agent_states.get(branch).copied(),
                ticket: project.tickets.get(index).cloned().unwrap_or_default(),
                ticket_category: project
                    .ticket_categories
                    .get(index)
                    .cloned()
                    .unwrap_or_default(),
                running: project.running.get(index).copied().unwrap_or(0),
                pr: project.prs.get(index).copied().flatten(),
                missing: project.missing.get(index).cloned().unwrap_or_default(),
            })
            .collect();
        let current = self
            .layout
            .project
            .as_ref()
            .and_then(|project| project.workspaces.iter().position(|b| *b == project.active))
            .unwrap_or(0);
        let row_ghost = self
            .row_drag
            .filter(|_| self.dragging == Drag::WorkspaceRow)
            .and_then(|drag| {
                let row = workspaces.iter().find(|row| row.index == drag.from)?;
                let width = if self.layout.left.collapsed {
                    200.0 * ui::ui_text_scale()
                } else {
                    drag.source.w
                };
                let area = Rect::new(
                    drag.source.x + drag.pointer.0 - drag.press.0,
                    drag.source.y + drag.pointer.1 - drag.press.1,
                    width,
                    drag.source.h,
                    Rgba::TRANSPARENT,
                );
                Some(lifted(&crate::panel::workspace_row_ghost(row), area))
            })
            .or_else(|| {
                let drag = self
                    .group_drag
                    .filter(|_| self.dragging == Drag::WorkspaceGroup)?;
                let count = workspaces
                    .iter()
                    .filter(|row| row.index != 0 && self.row_group(row.index) == drag.from)
                    .count();
                let scale = ui::ui_text_scale();
                let width = if self.layout.left.collapsed {
                    160.0 * scale
                } else {
                    drag.source.w
                };
                let area = Rect::new(
                    drag.source.x + drag.pointer.0 - drag.press.0,
                    drag.source.y + drag.pointer.1 - drag.press.1,
                    width,
                    30.0 * scale,
                    Rgba::TRANSPARENT,
                );
                Some(lifted(&crate::panel::group_ghost(drag.from, count), area))
            });
        if self
            .upkeep_done_at
            .is_some_and(|at| at.elapsed() >= UPKEEP_DONE_SHOWN)
        {
            self.upkeep_done_at = None;
        }
        let workspace_ops = self.workspace_ops.clone();
        let expanded_ops = self.expanded_ops.clone();
        let list = crate::panel::WorkspaceList {
            rows: &workspaces,
            current,
            ops: &workspace_ops,
            expanded: &expanded_ops,
            hovered: self.session_menu_hover,
            upkeep_done: self.upkeep_done_at.is_some(),
            width: self.layout.left_region(w, h).w / ui::ui_text_scale(),
            manual: self.manual_menu,
            grouping: self.grouping.as_ref(),
        };
        let mut rail_tip = None;
        let mut rail_popover = None;
        {
            let region = self.layout.left_region(w, h);
            // The list scrolls under the fixed footer strip: laid out from its scroll offset, then clipped.
            let visible = Rect::new(
                region.x,
                region.y,
                region.w,
                (region.h - crate::STATUS_BAR_H).max(0.0),
                Rgba::TRANSPARENT,
            );
            let node = if self.layout.left.collapsed {
                crate::panel::workspace_rail(&list, region.w / ui::ui_text_scale())
            } else {
                self.layout.left.body(&list)
            };
            let top = visible.y - self.workspace_scroll;
            let mut p = ui::render(
                &node,
                Rect::new(
                    visible.x,
                    top,
                    visible.w,
                    visible.h + self.workspace_scroll,
                    Rgba::TRANSPARENT,
                ),
            );
            let bottom = p
                .hits
                .iter()
                .map(|(rect, _)| rect.y + rect.h)
                .chain(p.rects.iter().map(|rect| rect.y + rect.h))
                .fold(top, f32::max);
            self.workspace_scroll_max = (bottom - top + 8.0 - visible.h).max(0.0);
            if self.workspace_scroll > self.workspace_scroll_max {
                self.workspace_scroll = self.workspace_scroll_max;
            }
            if self.layout.left.collapsed {
                let group_tip = self.session_menu_hover.and_then(|hovered| {
                    let group = crate::TicketGroup::ALL
                        .get(hovered.checked_sub(crate::WORKSPACE_GROUP_BASE)? as usize)
                        .copied()?;
                    let (cell, _) = p.hits.iter().find(|(_, id)| *id == hovered)?;
                    let count = workspaces
                        .iter()
                        .filter(|row| row.index != 0 && self.row_group(row.index) == group)
                        .count();
                    let folded = self
                        .grouping
                        .as_ref()
                        .is_some_and(|grouping| grouping.folded.contains(&group));
                    let noun = if count == 1 {
                        "workspace"
                    } else {
                        "workspaces"
                    };
                    let hint = if folded {
                        "folded - click to open"
                    } else {
                        "click to fold, drag to move the group"
                    };
                    let scale = ui::ui_text_scale();
                    let anchor = Rect::new(
                        cell.x + cell.w + 6.0 * scale,
                        cell.y + cell.h / 2.0 - 17.0 * scale,
                        0.0,
                        0.0,
                        Rgba::TRANSPARENT,
                    );
                    let text = format!("{} - {count} {noun} - {hint}", group.label());
                    Some(tooltip(anchor, &text, w))
                });
                rail_tip = group_tip.or_else(|| {
                    self.session_menu_hover.and_then(|hovered| {
                        let index = hovered.checked_sub(crate::WORKSPACE_ROW_BASE)? as usize;
                        let row = workspaces.iter().find(|row| row.index == index)?;
                        let (cell, _) = p.hits.iter().find(|(_, id)| *id == hovered)?;
                        let scale = ui::ui_text_scale();
                        // Beside the cell, centred on it, rather than under it where the next cells are.
                        let anchor = Rect::new(
                            cell.x + cell.w + 6.0 * scale,
                            cell.y + cell.h / 2.0 - 17.0 * scale,
                            0.0,
                            0.0,
                            Rgba::TRANSPARENT,
                        );
                        // The tile only raises what needs you; the tooltip carries the rest.
                        let mut text = row.label.clone();
                        if !row.ticket.is_empty() {
                            match crate::panel::ticket_key(row) {
                                Some(key) if !row.label.to_uppercase().starts_with(&key) => {
                                    text.push_str(&format!(" - {key} {}", row.ticket))
                                }
                                _ => text.push_str(&format!(" - {}", row.ticket)),
                            }
                        }
                        if let Some(agent) = row.agent {
                            text.push_str(&format!(" - Agent: {}", agent.label()));
                        }
                        if row.running > 0 {
                            text.push_str(&format!(" - {} running", row.running));
                        }
                        if let Some(pr) = row.pr.filter(|_| row.index != 0) {
                            let noun = if pr.count == 1 { "PR" } else { "PRs" };
                            match pr.trouble {
                                Some(trouble) => text.push_str(&format!(
                                    " - {} {noun}: {}",
                                    pr.count,
                                    trouble.label()
                                )),
                                None => text.push_str(&format!(" - {} {noun}", pr.count)),
                            }
                        }
                        Some(tooltip(anchor, &text, w))
                    })
                });
                rail_popover = self.rail_popover.and_then(|op_id| {
                    let (position, op) = self
                        .workspace_ops
                        .iter()
                        .enumerate()
                        .find(|(_, op)| op.id == op_id)?;
                    let target = crate::WORKSPACE_OP_BASE
                        + position as u64 * crate::WORKSPACE_OP_STRIDE
                        + crate::WORKSPACE_OP_POPOVER;
                    let (cell, _) = p.hits.iter().find(|(_, id)| *id == target)?;
                    let scale = ui::ui_text_scale();
                    let node = crate::panel::failure_popover(
                        op,
                        position,
                        self.manual_menu == Some(op_id),
                    );
                    let x = region.x + region.w + 6.0 * scale;
                    // Wrapped so the card keeps its own size instead of filling the area it is laid into.
                    let node: ui::Node = ui::div().col().child(ui::div().row().child(node)).into();
                    let place =
                        |y: f32| ui::render(&node, Rect::new(x, y, w - x, h, Rgba::TRANSPARENT));
                    let mut y = (cell.y - 4.0 * scale).max(8.0 * scale);
                    let mut painted = place(y);
                    let bottom = painted.rects.iter().map(|r| r.y + r.h).fold(y, f32::max);
                    // Lifted to stay on screen when the tile sits low.
                    if bottom > h - 8.0 * scale {
                        y = (y - (bottom - (h - 8.0 * scale))).max(8.0 * scale);
                        painted = place(y);
                    }
                    Some(painted)
                });
            }
            if let Some(drop) = self.row_drop_painted(&p.hits) {
                p.rects.extend(drop.rects);
            }
            if let Some(drop) = self.group_drop_painted(&p.hits) {
                p.rects.extend(drop.rects);
            }
            panel_hits.extend(clipped_hits(&p, visible));
            center_overlays.push(Overlay {
                painted: p,
                clip: Some(visible),
            });
        }
        // not in the main status bar (which never covers this special sidebar) and not in the header. Always
        // shown (even in the collapsed rail), accented while the sidebar is open.
        {
            let region = self.layout.left_region(w, h);
            let strip_top = region.y + region.h - crate::STATUS_BAR_H;
            // The footer strip: same background + top border as the status bar so the two read as one continuous
            blit(ui::render(
                &ui::div().bg(ui::theme().title_bar_background).into(),
                Rect::new(
                    region.x,
                    strip_top,
                    region.w,
                    crate::STATUS_BAR_H,
                    Rgba::TRANSPARENT,
                ),
            ));
            // Top border at `content_bottom - 1`, exactly matching the status bar's own top border (1px higher
            // than the fill) so the two strips line up pixel-for-pixel.
            blit(ui::render(
                &ui::div().bg(ui::theme().border).into(),
                Rect::new(region.x, strip_top - 1.0, region.w, 1.0, Rgba::TRANSPARENT),
            ));
            // Vertical divider at the sidebar's inner edge (the footer fill would otherwise cover the full-height
            // one from `Layout::build`), so the sidebar footer is separated from the status bar.
            let edge_x = if self.layout.sidebar_left() {
                region.x + region.w - 1.0
            } else {
                region.x
            };
            blit(ui::render(
                &ui::div().bg(ui::theme().border).into(),
                Rect::new(
                    edge_x,
                    strip_top,
                    1.0,
                    crate::STATUS_BAR_H,
                    Rgba::TRANSPARENT,
                ),
            ));
            let kind = if self.layout.sidebar_left() {
                IconKind::Sidebar
            } else {
                IconKind::PanelRight
            };
            let color = if self.layout.left.collapsed {
                ui::theme().icon_muted
            } else {
                ui::theme().icon_accent
            };
            let btn = ui::div()
                .w_px(26.0)
                .h_px(20.0)
                .rounded(5.0)
                .items_center()
                .justify_center()
                .on_click(SIDEBAR_TOGGLE)
                .child(ui::icon(kind).size(13.0).color(color));
            // Sit in the same bottom strip as the status bar, vertically centered like its icons.
            let strip = crate::STATUS_BAR_H;
            let btn_h = 20.0;
            let btn_rect = Rect::new(
                region.x + 8.0,
                region.y + region.h - strip + (strip - btn_h) / 2.0,
                26.0,
                btn_h,
                Rgba::TRANSPARENT,
            );
            let bp = ui::render(&btn.into(), btn_rect);
            panel_hits.extend(bp.hits.iter().copied());
            blit(bp);
        }
        if !self.layout.right.collapsed {
            let region = self.layout.right_region(w, h);
            let p = match self.layout.shown_on(DockPosition::Right) {
                Some(Shown::Terminal) => {
                    self.terminal_painted(region, true, &mut center_overlays, &mut panel_hits)
                }
                Some(Shown::Func(PaneKind::Files)) => Painted::default(),
                Some(Shown::Func(_)) => self
                    .function_panel_painted(DockPosition::Right, region, &mut center_overlays)
                    .unwrap_or_default(),
                Some(Shown::Agent) if self.layout.agent_visible() => {
                    self.agent_painted(region, true, &mut center_overlays, &mut panel_hits)
                }
                // With no agent session open (and when nothing is docked) the dock shows the AgentEmptyPanel.
                Some(Shown::Agent) | None => self.layout.right.render_body(region, &list),
            };
            panel_hits.extend(p.hits.iter().copied());
            blit(p);
        }
        if !self.layout.bottom.collapsed {
            let region = self.layout.bottom_region(w, h);
            // The bottom dock has no default panel: it only shows whatever is docked there (terminal/function).
            let p = match self.layout.shown_on(DockPosition::Bottom) {
                Some(Shown::Terminal) => {
                    self.terminal_painted(region, true, &mut center_overlays, &mut panel_hits)
                }
                Some(Shown::Func(PaneKind::Files)) => Painted::default(),
                Some(Shown::Func(_)) => self
                    .function_panel_painted(DockPosition::Bottom, region, &mut center_overlays)
                    .unwrap_or_default(),
                _ => ui::render(&ui::div().bg(ui::theme().panel_background).into(), region),
            };
            panel_hits.extend(p.hits.iter().copied());
            blit(p);
        }
        self.pull_server_notices();
        self.layout.language_servers = self
            .layout
            .files_view
            .as_ref()
            .and_then(|view| view.language_servers(false))
            .unwrap_or_default();
        let status = ui::render(
            &status_bar(&self.layout, self.session_menu_hover),
            self.layout.status_region(w, h),
        );
        let status_hits = status.hits.clone();
        blit(status);
        let status_tip = self.session_menu_hover.and_then(|hv| {
            let (text, key) = status_tooltip(hv).or_else(|| {
                let activity = self.layout.language_servers.activity.as_ref()?;
                (hv == crate::LANGUAGE_ACTIVITY
                    && activity.message.chars().count() > crate::ACTIVITY_MESSAGE_LIMIT)
                    .then(|| (activity.message.clone(), crate::StatusKey::None))
            })?;
            let rect = status_hits
                .iter()
                .find(|(_, id)| *id == hv)
                .map(|(r, _)| *r)?;
            let keys = match key {
                crate::StatusKey::Action(action) => self
                    .bindings
                    .iter()
                    .find(|(bound, _)| *bound == action)
                    .map(|(_, keys)| keys.clone()),
                crate::StatusKey::Fixed(keys) => Some(keys.to_string()),
                crate::StatusKey::None => None,
            };
            Some(tooltip_above(rect, &text, keys.as_deref(), w))
        });

        let header = self.layout.header(w, self.session_menu_hover);
        let update_tip = self.session_menu_hover.and_then(|hovered| {
            let text = crate::update::tooltip(self.layout.update.button.as_ref()?, hovered)?;
            let (rect, _) = header.hits.iter().find(|(_, id)| *id == hovered)?;
            Some(tooltip(*rect, text, w))
        });
        let mut header_hits = header.hits.clone();
        header_hits.extend(panel_hits);
        header_hits.extend(status_hits);
        if let Some(zoom) = self.zoom {
            let rect = zoom.outer;
            if zoom.group == InputGroup::Panel || zoom.group == InputGroup::Agent {
                let backdrop = if zoom.group == InputGroup::Agent {
                    self.agent_painted(zoom.inner, false, &mut zoom_overlays, &mut zoom_hits)
                } else {
                    self.terminal_painted(zoom.inner, false, &mut zoom_overlays, &mut zoom_hits)
                };
                zoom_overlays.insert(
                    0,
                    Overlay {
                        painted: backdrop,
                        clip: Some(zoom.inner),
                    },
                );
            }
            // The zoomed view occludes what is under it, so only its own hits stay live there.
            header_hits.retain(|(hit, _)| {
                let (cx, cy) = (hit.x + hit.w / 2.0, hit.y + hit.h / 2.0);
                !(cx >= rect.x && cx < rect.x + rect.w && cy >= rect.y && cy < rect.y + rect.h)
            });
            header_hits.extend(zoom_hits);
        }
        rects.extend(header.rects);
        tris.extend(header.tris);
        texts.extend(header.texts);
        icons.extend(header.icons);

        let base = Painted {
            rects,
            tris,
            texts,
            icons,
            hits: Vec::new(),
            debug_bounds: Vec::new(),
        };

        let mut overlays: Vec<Overlay> = Vec::new();
        overlays.append(&mut center_overlays);
        {
            let bp = Painted {
                rects: self.layout.border_lines(w, h),
                ..Painted::default()
            };
            overlays.push(Overlay {
                painted: bp,
                clip: None,
            });
        }
        if let Some(bar) = scrollbar_overlay {
            overlays.push(Overlay {
                painted: bar,
                clip: None,
            });
        }
        // Above the docks' borders and scrollbars, below drags, toasts, popovers and menus.
        if let Some(zoom) = self.zoom {
            let rect = zoom.outer;
            let mut frame = Painted::default();
            frame.rects.extend(box_shadow(rect, &LARGE_SHADOW, 0.0));
            frame.rects.push(Rect::new(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                ui::theme().background,
            ));
            let [top, right, bottom, left] = zoom.sides;
            let border = ui::theme().border;
            let edges = [
                (top, Rect::new(rect.x, rect.y, rect.w, 1.0, border)),
                (
                    right,
                    Rect::new(rect.x + rect.w - 1.0, rect.y, 1.0, rect.h, border),
                ),
                (
                    bottom,
                    Rect::new(rect.x, rect.y + rect.h - 1.0, rect.w, 1.0, border),
                ),
                (left, Rect::new(rect.x, rect.y, 1.0, rect.h, border)),
            ];
            frame.rects.extend(
                edges
                    .into_iter()
                    .filter(|(on, _)| *on)
                    .map(|(_, edge)| edge),
            );
            overlays.push(Overlay {
                painted: frame,
                clip: None,
            });
            overlays.append(&mut zoom_overlays);
        }
        {
            let mut drag = Painted::default();
            let panel_preview = self
                .layout
                .terminal_view
                .as_ref()
                .filter(|_| self.layout.terminal_visible())
                .and_then(|v| v.tab_drag_overlay())
                .or_else(|| {
                    self.layout
                        .agent_view
                        .as_ref()
                        .and_then(|v| v.tab_drag_overlay())
                });
            if let Some(preview) = panel_preview {
                drag.rects.push(Rect::new(
                    preview.x,
                    preview.y,
                    preview.w,
                    preview.h,
                    ui::theme().text_accent.alpha(0.22),
                ));
            }
            let ghost = match self.dragging {
                Drag::TerminalTab => self
                    .layout
                    .terminal_view
                    .as_ref()
                    .and_then(|v| v.tab_drag_ghost()),
                Drag::AgentTab => self
                    .layout
                    .agent_view
                    .as_ref()
                    .and_then(|v| v.tab_drag_ghost()),
                Drag::Tab => self
                    .layout
                    .files_view
                    .as_ref()
                    .and_then(|v| v.tab_drag_ghost()),
                _ => None,
            };
            if let Some(ghost) = row_ghost {
                drag.rects.extend(ghost.rects);
                drag.texts.extend(ghost.texts);
                drag.icons.extend(ghost.icons);
            }
            // Drawn after both groups so the ghost stays on top while crossing between them.
            if let (Some((gx, gy)), Some((node, gw, gh))) = (self.tab_ghost_at, ghost) {
                let area = Rect::new(gx - 14.0, gy - gh / 2.0, gw, gh, Rgba::TRANSPARENT);
                let ghost = ui::render(&node, area);
                drag.rects.extend(ghost.rects);
                drag.texts.extend(ghost.texts);
                drag.icons.extend(ghost.icons);
            }
            if !drag.rects.is_empty() {
                overlays.push(Overlay {
                    painted: drag,
                    clip: None,
                });
            }
        }
        let now = Instant::now();
        let expired = self
            .toast
            .as_ref()
            .is_some_and(|t| !t.hovered && now >= t.deadline);
        if expired {
            self.toast = None;
        }
        if let Some(t) = &mut self.toast {
            let progress =
                ((now - t.shown_at).as_secs_f32() / TOAST_ANIM.as_secs_f32()).clamp(0.0, 1.0);
            let slide = (1.0 - progress) * 12.0;
            let weight = ui::ui_font_weight();
            let msg_w = ui::measure_text_width(&t.message, 13.0, false, weight);
            let action_w = t
                .action
                .as_ref()
                .map(|a| ui::measure_text_width(a, 13.0, false, weight) + 20.0)
                .unwrap_or(0.0);
            let (pad, gap, close_w, toast_h) = (12.0, 10.0, 18.0, 34.0);
            let action_span = if action_w > 0.0 { gap + action_w } else { 0.0 };
            let toast_w = pad + msg_w + action_span + gap + close_w + pad;
            let x = ((w - toast_w) / 2.0).max(8.0);
            let y = h - crate::STATUS_BAR_H - toast_h - 14.0 + slide;
            let rect = Rect::new(x, y, toast_w, toast_h, Rgba::TRANSPARENT);
            t.rect = rect;
            let mut row = ui::div()
                .row()
                .items_center()
                .px(pad)
                .gap(gap)
                .rounded(8.0)
                .bg(ui::theme().elevated_surface_background)
                .border(1.0, ui::theme().border)
                .child(
                    ui::label(t.message.clone())
                        .size(13.0)
                        .color(ui::theme().text),
                );
            if let Some(a) = &t.action {
                row = row.child(
                    ui::div()
                        .px(8.0)
                        .h_px(22.0)
                        .rounded(6.0)
                        .items_center()
                        .justify_center()
                        .bg(ui::theme().element_hover)
                        .on_click(TOAST_ACTION)
                        .child(ui::label(a.clone()).size(13.0).color(ui::theme().text)),
                );
            }
            row = row.child(
                ui::div()
                    .w_px(close_w)
                    .h_px(close_w)
                    .rounded(4.0)
                    .items_center()
                    .justify_center()
                    .on_click(TOAST_CLOSE)
                    .child(
                        ui::icon(IconKind::Close)
                            .size(11.0)
                            .color(ui::theme().icon_muted),
                    ),
            );
            let mut painted = Painted::default();
            painted.rects.push(Rect {
                x: x - 2.0,
                y: y - 1.0,
                w: toast_w + 4.0,
                h: toast_h + 4.0,
                color: Rgba::new(0.0, 0.0, 0.0, 0.14),
                radius: 10.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            });
            let p = ui::render(&row.into(), rect);
            header_hits.extend(p.hits.iter().copied());
            painted.rects.extend(p.rects);
            painted.tris.extend(p.tris);
            painted.texts.extend(p.texts);
            painted.icons.extend(p.icons);
            overlays.push(Overlay {
                painted,
                clip: None,
            });
        }
        if let Some(notification) = &self.notification {
            let painted = notification_card(notification, w, h, self.session_menu_hover);
            header_hits.extend(painted.hits.iter().copied());
            overlays.push(Overlay {
                painted,
                clip: None,
            });
        }
        if self.layout.session_menu {
            let caret = ui::caret_phase();
            let menu = self.layout.session_menu(
                &self.session_search_query,
                self.session_menu_hover,
                caret,
            );
            header_hits.extend(menu.fixed.hits.iter().copied());
            let (_, cy, _, ch) = menu.clip;
            header_hits.extend(
                menu.list
                    .hits
                    .iter()
                    .filter(|(r, _)| r.y + r.h > cy && r.y < cy + ch)
                    .copied(),
            );
            let tip = self.session_menu_hover.and_then(|hv| {
                let text = session_action_tooltip(hv)?;
                let rect = menu
                    .list
                    .hits
                    .iter()
                    .find(|(_, id)| *id == hv)
                    .map(|(r, _)| *r)?;
                Some(tooltip(rect, text, w))
            });
            overlays.push(Overlay {
                painted: menu.fixed,
                clip: None,
            });
            let (cx, cy, cw, chh) = menu.clip;
            overlays.push(Overlay {
                painted: menu.list,
                clip: Some(Rect::new(cx, cy, cw, chh, Rgba::TRANSPARENT)),
            });
            if let Some(t) = tip {
                overlays.push(Overlay {
                    painted: t,
                    clip: None,
                });
            }
        }

        for t in [status_tip, rail_tip, update_tip].into_iter().flatten() {
            overlays.push(Overlay {
                painted: t,
                clip: None,
            });
        }
        self.rail_popover_rect =
            rail_popover.map(|p| push_card(p, &mut header_hits, &mut overlays));
        self.manual_menu_rect = self
            .manual_menu_painted(&header_hits, w, h)
            .map(|p| push_card(p, &mut header_hits, &mut overlays));
        self.agent_popover_rect = self
            .agent_popover_painted(w)
            .map(|p| push_card(p, &mut header_hits, &mut overlays));
        self.usage_popover_rect = self
            .usage_popover_painted(w)
            .map(|p| push_card(p, &mut header_hits, &mut overlays));

        self.popover_rects.clear();
        self.popover_groups.clear();
        let viewport = self.viewport;
        let mut popovers = Vec::new();
        for group in self.visible_groups() {
            let group_popovers = self
                .input(group)
                .map(|input| input.editor_popovers(viewport))
                .unwrap_or_default();
            for (index, popover) in group_popovers.into_iter().enumerate() {
                popovers.push(popover);
                self.popover_groups.push((group, index));
            }
        }
        for (node, px, py) in popovers {
            let area = Rect::new(px, py, w - px, h - py, Rgba::TRANSPARENT);
            let p = ui::render(&ui::div().col().child(node).into(), area);
            header_hits.extend(p.hits.iter().copied());
            let right = p.rects.iter().map(|r| r.x + r.w).fold(px, f32::max);
            let bottom = p.rects.iter().map(|r| r.y + r.h).fold(py, f32::max);
            let mut painted = Painted::default();
            let popover = Rect::new(px, py, right - px, bottom - py, Rgba::TRANSPARENT);
            self.popover_rects.push(popover);
            painted
                .rects
                .extend(elevation_shadow(popover, crate::Elevation::Elevated));
            painted.rects.extend(p.rects);
            painted.tris.extend(p.tris);
            painted.texts.extend(p.texts);
            painted.icons.extend(p.icons);
            overlays.push(Overlay {
                painted,
                clip: None,
            });
        }

        self.modal_rect = None;
        let viewport = (w, h);
        let window_modal = self.window_modal.as_mut().map(|modal| crate::ModalView {
            width: modal.width(),
            node: modal.render(),
            elevation: crate::Elevation::Modal,
        });
        let modal = match window_modal {
            Some(modal) => Some(modal),
            None => self
                .layout
                .files_view
                .as_mut()
                .and_then(|v| v.modal(viewport)),
        };
        if let Some(modal) = modal {
            // Modal widths are design px; the tree scales them with the UI text size.
            let modal_w = modal.width * ui::ui_text_scale();
            let x = ((w - modal_w) / 2.0).max(8.0);
            let area = Rect::new(x, MODAL_TOP, modal_w, h - MODAL_TOP, Rgba::TRANSPARENT);
            // Wrapped in a column so the modal keeps its content height instead of filling the area.
            let frame = ui::paint_frame(&ui::div().col().child(modal.node).into(), area);
            let bottom = frame
                .base
                .rects
                .iter()
                .map(|r| r.y + r.h)
                .fold(MODAL_TOP, f32::max);
            let rect = Rect::new(x, MODAL_TOP, modal_w, bottom - MODAL_TOP, Rgba::TRANSPARENT);
            self.modal_rect = Some(frame.overlays.iter().fold(rect, |rect, overlay| {
                overlay.painted.rects.iter().fold(rect, union_rect)
            }));
            header_hits.extend(frame.hits.iter().copied());
            let mut painted = Painted::default();
            painted
                .rects
                .extend(elevation_shadow(rect, modal.elevation));
            painted.rects.extend(frame.base.rects);
            painted.tris.extend(frame.base.tris);
            painted.texts.extend(frame.base.texts);
            painted.icons.extend(frame.base.icons);
            overlays.push(Overlay {
                painted,
                clip: None,
            });
            // A popover inside the modal is its own layer, so the form's text never shows through it.
            overlays.extend(frame.overlays);
        }

        // The right-click context menu (topmost overlay; its hits win in `hit`).
        if let Some((mx, mtop, mbottom, target)) = self.menu {
            let (mtop, mbottom, keep) = if target == EDITOR_MENU_TARGET {
                match self.menu_editor_anchor.clone() {
                    Some((path, line)) => match self
                        .input_ref(self.menu_group)
                        .and_then(|input| input.editor_menu_y(&path, line))
                    {
                        Some(ly) => (ly, ly + 20.0, true),
                        None => (mtop, mbottom, false),
                    },
                    None => (mtop, mbottom, true),
                }
            } else {
                (mtop, mbottom, true)
            };
            if !keep {
                self.menu = None;
                self.submenu = None;
                self.menu_editor_anchor = None;
                self.header_hits = header_hits.clone();
                return Frame {
                    base,
                    overlays,
                    hits: header_hits,
                };
            }
            let items = self.menu_items(target);
            let painted = context_menu(
                mx,
                mtop,
                mbottom,
                w,
                h,
                &items,
                self.session_menu_hover,
                None,
            );
            header_hits.extend(painted.hits.iter().copied());
            overlays.push(Overlay {
                painted,
                clip: None,
            });
            if let Some((sx, stop, sleft, parent)) = self.submenu {
                let sub = self.menu_items(parent);
                let painted = context_menu(
                    sx,
                    stop,
                    stop,
                    w,
                    h,
                    &sub,
                    self.session_menu_hover,
                    Some(sleft),
                );
                header_hits.extend(painted.hits.iter().copied());
                overlays.push(Overlay {
                    painted,
                    clip: None,
                });
            }
        }

        if self.prompt_shown.is_none() {
            self.prompt_shown = self.pending_prompt.take().map(|prompt| (prompt, 0));
        }
        if let Some((prompt, active)) = &self.prompt_shown {
            let painted = prompt_dialog(prompt, *active, w, h);
            header_hits.extend(painted.hits.iter().copied());
            overlays.push(Overlay {
                painted,
                clip: None,
            });
        }

        self.header_hits = header_hits.clone();
        Frame {
            base,
            overlays,
            hits: header_hits,
        }
    }

    fn answer_prompt(&mut self, index: usize) {
        if let Some((prompt, _)) = self.prompt_shown.take() {
            self.prompt_answered(prompt.token, index);
        }
    }

    /// Keys while a confirmation is up, after the reference's prompt: Enter takes the highlighted button,
    /// Escape the one named Cancel, arrows and Tab move the highlight.
    fn prompt_key(&mut self, key: EditKey, shift: bool) -> bool {
        let Some((prompt, active)) = self.prompt_shown.as_mut() else {
            return false;
        };
        let count = prompt.buttons.len().max(1);
        match key {
            EditKey::Enter => {
                let index = *active;
                self.answer_prompt(index);
            }
            EditKey::Escape => {
                if let Some(cancel) = prompt.buttons.iter().position(|button| button == "Cancel") {
                    self.answer_prompt(cancel);
                }
            }
            EditKey::Down | EditKey::Right => *active = (*active + 1) % count,
            EditKey::Up | EditKey::Left => *active = (*active + count - 1) % count,
            EditKey::Tab | EditKey::Backtab => {
                *active = if shift || key == EditKey::Backtab {
                    (*active + count - 1) % count
                } else {
                    (*active + 1) % count
                };
            }
            _ => {}
        }
        true
    }

    fn hit(&self, x: f32, y: f32) -> Option<u64> {
        self.hit_with_rect(x, y).map(|(id, _)| id)
    }

    fn hit_with_rect(&self, x: f32, y: f32) -> Option<(u64, Rect)> {
        self.header_hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|(r, id)| (*id, *r))
    }

    /// Whether `(x, y)` is over a clickable region (for the shell to show a pointer cursor).
    pub fn hit_at(&self, x: f32, y: f32) -> Option<u64> {
        let hit = self.hit(x, y)?;
        match self.window_modal.as_ref() {
            Some(modal) if crate::is_window_modal_id(hit) && !modal.is_pointer(hit) => None,
            _ => Some(hit),
        }
    }

    pub fn resize_cursor_at(&self, x: f32, y: f32) -> Option<ResizeCursor> {
        match self.dragging {
            Drag::Left | Drag::Right | Drag::Tree => return Some(ResizeCursor::Horizontal),
            Drag::Bottom => return Some(ResizeCursor::Vertical),
            Drag::Center(id) | Drag::TerminalDivider(id) => return self.center_divider_cursor(id),
            Drag::Tab
            | Drag::EditorSel(_)
            | Drag::TerminalTab
            | Drag::AgentTab
            | Drag::ItemPointer(_)
            | Drag::WorkspaceRow
            | Drag::WorkspaceGroup => return None,
            Drag::None => {}
        }
        let (w, h) = self.viewport;
        if self.layout.on_left_divider(x, y, w)
            || self.layout.on_tree_divider(x, y, w)
            || self.layout.on_right_divider(x, y, w)
        {
            Some(ResizeCursor::Horizontal)
        } else if self.layout.on_bottom_divider(x, y, w, h) {
            Some(ResizeCursor::Vertical)
        } else if let Some(id) = self.hit(x, y) {
            self.center_divider_cursor(id)
        } else {
            None
        }
    }

    fn center_divider_cursor(&self, id: u64) -> Option<ResizeCursor> {
        let axis = if crate::is_agent_id(id) {
            self.layout.agent_view.as_ref()?.divider_axis(id)?
        } else if crate::is_terminal_id(id) {
            self.layout.terminal_view.as_ref()?.divider_axis(id)?
        } else {
            self.layout.files_view.as_ref()?.divider_axis(id)?
        };
        match axis {
            DividerAxis::Horizontal => Some(ResizeCursor::Horizontal),
            DividerAxis::Vertical => Some(ResizeCursor::Vertical),
        }
    }

    /// Whether a status-bar button id can be right-clicked for a context menu.
    /// The tab a click id names in either pane group: its group, pane id and index.
    fn tab_under(&self, id: u64) -> Option<(InputGroup, u64, usize)> {
        if crate::is_agent_id(id) {
            let panes = self.layout.agent_view.as_ref()?.panes_ref();
            let (pane, index) = panes.tab_at_id(id)?;
            return Some((InputGroup::Agent, pane, index));
        }
        if crate::is_terminal_id(id) {
            let panes = self.layout.terminal_view.as_ref()?.panes_ref();
            let (pane, index) = panes.tab_at_id(id)?;
            return Some((InputGroup::Panel, pane, index));
        }
        let panes = self.layout.files_view.as_ref()?.pane_group()?;
        let (pane, index) = panes.tab_at_id(id)?;
        Some((InputGroup::Center, pane, index))
    }

    /// The terminal under the pointer in `group`, with its pane's path and what it shows.
    fn terminal_at(
        &self,
        group: InputGroup,
        x: f32,
        y: f32,
    ) -> Option<(InputGroup, Vec<usize>, crate::TerminalContext)> {
        let view = self.group_view(group)?;
        let (path, _, _) = view.body_point(x, y)?;
        let context = view.pane_at(&path)?.active_item()?.terminal_context()?;
        Some((group, path, context))
    }

    fn terminal_menu_items(&self) -> Vec<MenuItem> {
        let Some((group, _, context)) = self.menu_terminal.as_ref() else {
            return Vec::new();
        };
        let entry = |id: u64,
                     label: &'static str,
                     sep: bool,
                     disabled: bool,
                     hint: Option<&'static str>| {
            MenuItem {
                id,
                label: label.into(),
                checked: false,
                sep,
                disabled,
                danger: false,
                icon: None,
                hint: hint.map(Into::into),
                color: None,
                header: false,
            }
        };
        let selected = context.selection.is_some();
        let mut items = vec![
            entry(
                crate::MENU_TERM_COPY,
                "Copy",
                false,
                !selected,
                Some("cmd-c"),
            ),
            entry(crate::MENU_TERM_PASTE, "Paste", false, false, Some("cmd-v")),
            entry(
                crate::MENU_TERM_SELECT_ALL,
                "Select All",
                false,
                false,
                Some("cmd-a"),
            ),
            entry(crate::MENU_TERM_CLEAR, "Clear", false, false, Some("cmd-k")),
        ];
        // An agent's own tab is not sent to an agent.
        if *group != InputGroup::Agent {
            let mut add = entry(
                crate::MENU_TERM_ADD_TO_AGENT,
                "Add Selection to Agent",
                true,
                !selected,
                None,
            );
            add.icon = Some(ui::IconKind::Sparkle);
            items.push(add);
            let mut ask = entry(
                crate::MENU_TERM_ASK_AGENT,
                if selected {
                    "Ask Agent about Selection"
                } else {
                    "Ask Agent about This Output"
                },
                false,
                context.selection.is_none() && context.recent.trim().is_empty(),
                None,
            );
            ask.icon = Some(ui::IconKind::HelpCircle);
            items.push(ask);
        }
        items.push(entry(
            crate::MENU_TERM_CLOSE,
            "Close Terminal Tab",
            true,
            false,
            None,
        ));
        items
    }

    fn terminal_menu_click(&mut self, item: u64) {
        let Some((group, path, context)) = self.menu_terminal.take() else {
            return;
        };
        let with_item = |view: &mut Self, command: crate::TerminalCommand| {
            if let Some(active) = view
                .group_view_mut(group)
                .and_then(|panes| panes.item_at_path_mut(&path))
            {
                active.terminal_command(command);
            }
        };
        match item {
            crate::MENU_TERM_COPY => {
                if let Some(text) = &context.selection {
                    Self::clip_set(text);
                }
            }
            crate::MENU_TERM_PASTE => {
                if let Some(text) = Self::clip_get() {
                    with_item(self, crate::TerminalCommand::Paste(text));
                }
            }
            crate::MENU_TERM_SELECT_ALL => with_item(self, crate::TerminalCommand::SelectAll),
            crate::MENU_TERM_CLEAR => with_item(self, crate::TerminalCommand::Clear),
            crate::MENU_TERM_ADD_TO_AGENT => {
                let Some(text) = context.selection else {
                    return;
                };
                let quoted = format!("```\n{}\n```\n", text.trim_end());
                let main = self.main_agent_id();
                let sent = main.is_some_and(|id| {
                    self.layout
                        .agent_view
                        .as_mut()
                        .is_some_and(|view| view.paste_into(&id, &quoted))
                });
                if sent {
                    self.focus_group(InputGroup::Agent);
                } else {
                    self.show_toast("Start the agent first to add the selection to it", None);
                }
            }
            crate::MENU_TERM_ASK_AGENT => {
                let (what, text) = match &context.selection {
                    Some(text) => ("this from the terminal", text.clone()),
                    None => ("the latest output of the terminal", context.recent.clone()),
                };
                let prompt = format!(
                    "Explain {what} ({}) and what to do about it:\n```\n{}\n```",
                    context.title,
                    text.trim_end()
                );
                self.agent_fix = Some((
                    crate::AgentFix {
                        prompt,
                        cwd: context.cwd,
                    },
                    crate::SideAgentRole::Ask,
                ));
            }
            crate::MENU_TERM_CLOSE => {
                if let Some(panes) = self.group_view_mut(group) {
                    let index = panes.pane_at(&path).and_then(|pane| pane.active);
                    if let Some(index) = index {
                        panes.close_tab(&path, index);
                    }
                }
                self.panes_input = true;
            }
            _ => {}
        }
    }

    /// The workspace's main agent tab, pinned first in the agent dock.
    fn main_agent_id(&mut self) -> Option<String> {
        let view = self.layout.agent_view.as_mut()?;
        let mut found = None;
        view.panes().for_each_item_mut(&mut |item| {
            if found.is_none() && item.pinned_at_front() {
                found = item.id();
            }
        });
        found
    }

    fn group_view(&self, group: InputGroup) -> Option<&crate::pane_group_view::PaneGroupView> {
        match group {
            InputGroup::Center => self.layout.files_view.as_ref()?.pane_group(),
            InputGroup::Panel => Some(self.layout.terminal_view.as_ref()?.panes_ref()),
            InputGroup::Agent => Some(self.layout.agent_view.as_ref()?.panes_ref()),
        }
    }

    fn group_view_mut(
        &mut self,
        group: InputGroup,
    ) -> Option<&mut crate::pane_group_view::PaneGroupView> {
        match group {
            InputGroup::Center => self.layout.files_view.as_mut()?.pane_group_mut(),
            InputGroup::Panel => Some(self.layout.terminal_view.as_mut()?.panes()),
            InputGroup::Agent => Some(self.layout.agent_view.as_mut()?.panes()),
        }
    }

    /// The tab context menu, after the reference's: closes, then the tab's file (paths, Finder, the tree, a
    /// terminal there).
    fn tab_menu_items(&self) -> Vec<MenuItem> {
        let Some(state) = self
            .menu_tab
            .and_then(|(group, pane, index)| self.group_view(group)?.tab_menu_state(pane, index))
        else {
            return Vec::new();
        };
        let entry = |id: u64, label: &'static str, sep: bool, disabled: bool| MenuItem {
            id,
            label: label.into(),
            checked: false,
            sep,
            disabled,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        };
        let mut items = vec![
            entry(crate::MENU_TAB_CLOSE, "Close", false, false),
            entry(
                crate::MENU_TAB_CLOSE_OTHERS,
                "Close Others",
                false,
                !state.has_others,
            ),
            entry(
                crate::MENU_TAB_CLOSE_LEFT,
                "Close Left",
                true,
                !state.has_left,
            ),
            entry(
                crate::MENU_TAB_CLOSE_RIGHT,
                "Close Right",
                false,
                !state.has_right,
            ),
            entry(
                crate::MENU_TAB_CLOSE_CLEAN,
                "Close Clean",
                true,
                !state.has_clean,
            ),
            entry(crate::MENU_TAB_CLOSE_ALL, "Close All", false, false),
        ];
        let pin_label = if state.pinned { "Unpin Tab" } else { "Pin Tab" };
        let pin = entry(crate::MENU_TAB_TOGGLE_PIN, pin_label, true, false);
        if let Some(path) = state.path {
            let in_project = self.relative_to_root(&path).is_some();
            items.push(entry(crate::MENU_TAB_COPY_PATH, "Copy Path", true, false));
            if in_project {
                items.push(entry(
                    crate::MENU_TAB_COPY_REL_PATH,
                    "Copy Relative Path",
                    false,
                    false,
                ));
            }
            items.push(entry(
                crate::MENU_TAB_REVEAL,
                "Reveal in Finder",
                true,
                false,
            ));
            items.push(pin);
            if in_project {
                items.push(entry(
                    crate::MENU_TAB_REVEAL_IN_TREE,
                    "Reveal In Project Panel",
                    false,
                    false,
                ));
            }
            if path.parent().is_some() {
                items.push(entry(
                    crate::MENU_TAB_OPEN_TERMINAL,
                    "Open in Terminal",
                    false,
                    false,
                ));
            }
        } else {
            items.push(pin);
        }
        if let Some(stop) = state.stop_label {
            items.push(MenuItem {
                danger: true,
                ..entry(crate::MENU_TAB_STOP, stop, true, false)
            });
        }
        items
    }

    fn active_file_in(&self, group: InputGroup) -> Option<std::path::PathBuf> {
        self.group_view(group)?.active_item()?.abs_path()
    }

    fn relative_to_root(&self, path: &std::path::Path) -> Option<String> {
        let root = self.layout.files_view.as_ref()?.root_dir()?;
        let relative = path.strip_prefix(root).ok()?;
        Some(relative.to_string_lossy().into_owned())
    }

    fn apply_tab_menu(&mut self, item: u64) {
        let Some((group, pane, index)) = self.menu_tab.take() else {
            return;
        };
        let close = match item {
            crate::MENU_TAB_CLOSE => Some(crate::pane_group_view::CloseTabs::This),
            crate::MENU_TAB_CLOSE_OTHERS => Some(crate::pane_group_view::CloseTabs::Others),
            crate::MENU_TAB_CLOSE_LEFT => Some(crate::pane_group_view::CloseTabs::Left),
            crate::MENU_TAB_CLOSE_RIGHT => Some(crate::pane_group_view::CloseTabs::Right),
            crate::MENU_TAB_CLOSE_CLEAN => Some(crate::pane_group_view::CloseTabs::Clean),
            crate::MENU_TAB_CLOSE_ALL => Some(crate::pane_group_view::CloseTabs::All),
            _ => None,
        };
        if item == crate::MENU_TAB_TOGGLE_PIN {
            if let Some(panes) = self.group_view_mut(group) {
                panes.toggle_pin(pane, index);
            }
            return;
        }
        if item == crate::MENU_TAB_STOP {
            if let Some(panes) = self.group_view_mut(group) {
                panes.stop_tab(pane, index);
            }
            return;
        }
        if let Some(which) = close {
            if let Some(panes) = self.group_view_mut(group) {
                panes.close_tabs(pane, index, which);
            }
            self.ask_about_pending_close();
            return;
        }
        let Some(path) = self
            .group_view(group)
            .and_then(|panes| panes.tab_menu_state(pane, index))
            .and_then(|state| state.path)
        else {
            return;
        };
        match item {
            crate::MENU_TAB_COPY_PATH => Self::clip_set(&path.to_string_lossy()),
            crate::MENU_TAB_COPY_REL_PATH => {
                if let Some(relative) = self.relative_to_root(&path) {
                    Self::clip_set(&relative);
                }
            }
            crate::MENU_TAB_REVEAL => {
                if let Err(error) = std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .spawn()
                {
                    eprintln!("reveal in Finder: {error}");
                }
            }
            crate::MENU_TAB_REVEAL_IN_TREE => {
                self.show_files_tree();
                if let Some(files) = self.layout.files_view.as_mut() {
                    files.reveal_in_tree(&path);
                }
            }
            crate::MENU_TAB_OPEN_TERMINAL => {
                self.open_terminal_at(path.parent().map(std::path::Path::to_path_buf));
            }
            _ => {}
        }
    }

    fn workspace_row_menu_items(&self) -> Vec<MenuItem> {
        let (Some(project), Some(index)) = (self.layout.project.as_ref(), self.menu_workspace)
        else {
            return Vec::new();
        };
        let item = |id: u64, label: &'static str, sep: bool| MenuItem {
            id,
            label: label.into(),
            checked: false,
            sep,
            disabled: false,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        };
        let mut items = vec![item(crate::MENU_WS_RENAME, "Rename...", false)];
        if project
            .tickets
            .get(index)
            .is_some_and(|ticket| !ticket.is_empty())
        {
            items.push(item(crate::MENU_WS_OPEN_TICKET, "Open Ticket", false));
        }
        if project
            .running
            .get(index)
            .is_some_and(|running| *running > 0)
        {
            items.push(item(crate::MENU_WS_STOP, "Stop All Services", false));
        }
        let is_main = project.workspaces.get(index) == Some(&project.branch);
        if is_main
            && project
                .missing
                .get(index)
                .is_some_and(|missing| !missing.is_empty())
        {
            items.push(item(
                crate::MENU_WS_ADD_MISSING,
                "Clone Missing Repos...",
                false,
            ));
        }
        if !is_main {
            items.push(item(crate::MENU_WS_ADD_REPOS, "Add Repos...", false));
        }
        if is_main {
            items.push(item(
                crate::MENU_WS_UPDATE_MAIN,
                "Update Main from Origin",
                true,
            ));
            items.push(item(crate::MENU_WS_PREPARE_MAIN, "Prepare Main...", false));
        } else {
            items.push(item(crate::MENU_WS_DELETE, "Delete Workspace", true));
        }
        items
    }

    fn apply_workspace_row_menu(&mut self, item: u64) {
        let Some(index) = self.menu_workspace.take() else {
            return;
        };
        match item {
            crate::MENU_WS_RENAME => {
                self.workspace_requests.row = Some((index, crate::RowAction::Rename));
            }
            crate::MENU_WS_STOP => {
                self.workspace_requests.row = Some((index, crate::RowAction::StopServices));
            }
            crate::MENU_WS_OPEN_TICKET => {
                self.workspace_requests.row = Some((index, crate::RowAction::OpenTicket));
            }
            crate::MENU_WS_ADD_MISSING => {
                self.workspace_requests.row = Some((index, crate::RowAction::AddMissingRepos));
            }
            crate::MENU_WS_ADD_REPOS => {
                self.workspace_requests.row = Some((index, crate::RowAction::AddRepos));
            }
            crate::MENU_WS_DELETE => self.ask_to_delete_workspace(index),
            crate::MENU_WS_UPDATE_MAIN => {
                self.workspace_requests.row = Some((index, crate::RowAction::UpdateMain));
            }
            crate::MENU_WS_PREPARE_MAIN => {
                self.pending_prompt = Some(crate::Prompt {
                    token: PREPARE_MAIN_PROMPT_TOKEN + index as u64,
                    message: "Reset main's databases?".into(),
                    detail: Some(
                        "Drops and recreates main's databases, runs each repo's migrations, then seeds. \
                         New workspaces copy these databases. Data in them now is lost."
                            .into(),
                    ),
                    buttons: vec!["Prepare Main".into(), "Cancel".into()],
                });
            }
            _ => {}
        }
    }

    /// The reference's destructive-confirmation shape: the question names the target, the detail says what
    /// goes, and the confirming answer comes first.
    fn ask_to_delete_workspace(&mut self, index: usize) {
        let Some(project) = self.layout.project.as_ref() else {
            return;
        };
        self.pending_prompt = Some(crate::Prompt {
            token: DELETE_WORKSPACE_PROMPT_TOKENS + index as u64,
            message: format!("Delete workspace \"{}\"?", project.label(index)),
            detail: Some(
                "Stops its services and removes its worktrees, databases and folder. Local branches \
                 with unpushed commits are kept. This cannot be undone."
                    .into(),
            ),
            buttons: vec!["Delete".into(), "Cancel".into()],
        });
    }

    fn menuable(id: u64) -> bool {
        id == SIDEBAR_TOGGLE
            || id == AGENT_TOGGLE
            || id == BOTTOM_TOGGLE
            || (FUNC_BASE..FUNC_BASE + PaneKind::ALL.len() as u64).contains(&id)
    }

    /// The context-menu items for a given status-bar button (dock positions valid for it + Hide Button).
    fn menu_items(&self, target: u64) -> Vec<MenuItem> {
        if target == crate::ITEM_MENU_TARGET {
            return self.item_menu.clone();
        }
        if target == crate::APP_MENU_TARGET {
            return self.app_menu_items();
        }
        if target == crate::MENU_SUBMENU_LAYOUT {
            return self.panel_layout_items();
        }
        if target == crate::WORKSPACE_ROW_MENU_TARGET {
            return self.workspace_row_menu_items();
        }
        if target == crate::TAB_MENU_TARGET {
            return self.tab_menu_items();
        }
        if target == crate::LANGUAGE_SERVERS_MENU_TARGET
            || target == crate::LANGUAGE_ACTIVITY_MENU_TARGET
            || (crate::LANGUAGE_SERVER_SUBMENU_BASE..crate::LANGUAGE_SERVER_ACTION_BASE)
                .contains(&target)
        {
            let servers = self
                .layout
                .files_view
                .as_ref()
                .and_then(|view| view.language_servers(true))
                .unwrap_or_default();
            return match target {
                crate::LANGUAGE_SERVERS_MENU_TARGET => servers.menu_items(),
                crate::LANGUAGE_ACTIVITY_MENU_TARGET => servers.activity_items(),
                submenu => servers.submenu_items(submenu),
            };
        }
        if is_submenu(target) {
            if let Some(items) = self
                .layout
                .side_panels
                .iter()
                .map(|panel| panel.submenu_items(target))
                .find(|items| !items.is_empty())
            {
                return items;
            }
        }
        if let Some(kind) = Self::side_menu_kind(target) {
            return self
                .layout
                .side_panels
                .iter()
                .find(|panel| panel.kind() == kind)
                .map(|panel| panel.menu_items())
                .unwrap_or_default();
        }
        let hide = MenuItem {
            id: MENU_HIDE,
            label: "Hide Button".into(),
            checked: false,
            sep: true,
            disabled: false,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        };
        let item = |id: u64, label: &'static str, sep: bool| MenuItem {
            id,
            label: label.into(),
            checked: false,
            sep,
            disabled: false,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        };
        let disabled = |id: u64, label: &'static str, sep: bool, disabled: bool| MenuItem {
            id,
            label: label.into(),
            checked: false,
            sep,
            disabled,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        };
        if target == MENU_SUBMENU_COPY {
            return vec![
                item(MENU_COPY_PATH, "Copy Path", false),
                item(MENU_COPY_REL_PATH, "Copy Relative Path", false),
                item(MENU_COPY_NAME, "Copy File Name", false),
            ];
        }
        if target == TREE_MENU_TARGET {
            let path = self
                .menu_path
                .as_ref()
                .map(|(p, _)| p.as_str())
                .filter(|p| !p.is_empty());
            let state = self
                .layout
                .files_view
                .as_ref()
                .map(|v| v.tree_menu_state(path))
                .unwrap_or_default();
            let mut items = vec![
                item(MENU_TREE_NEW_FILE, "New File", false),
                item(MENU_TREE_NEW_DIR, "New Folder", false),
                item(MENU_REVEAL, "Reveal in Finder", true),
                item(MENU_TREE_OPEN_SYSTEM, "Open in Default App", false),
                item(crate::MENU_TREE_OPEN_TERMINAL, "Open in Terminal", false),
                item(MENU_TREE_CUT, "Cut", true),
                item(MENU_TREE_COPY, "Copy", false),
                item(MENU_TREE_DUPLICATE, "Duplicate", false),
                disabled(MENU_TREE_PASTE, "Paste", false, !state.can_paste),
                item(MENU_COPY_PATH, "Copy Path", true),
                item(MENU_COPY_REL_PATH, "Copy Relative Path", false),
            ];
            if state.has_git_repo {
                let restore = !state.is_dir && state.has_git_changes;
                if restore {
                    items.push(item(MENU_TREE_RESTORE, "Restore File", true));
                }
                items.push(item(MENU_TREE_GITIGNORE, "Add to .gitignore", !restore));
            }
            if !state.is_root {
                items.push(item(MENU_TREE_RENAME, "Rename", true));
                items.push(item(MENU_TREE_TRASH, "Trash", false));
                items.push(item(MENU_TREE_DELETE, "Delete", false));
            }
            if state.is_dir {
                items.push(item(MENU_TREE_EXPAND_ALL, "Expand All", true));
                items.push(item(MENU_TREE_COLLAPSE_ALL, "Collapse All", false));
            }
            return items;
        }
        if target == crate::TERMINAL_MENU_TARGET {
            return self.terminal_menu_items();
        }
        if target == EDITOR_MENU_TARGET {
            return vec![
                item(MENU_EDIT_GO_TO_DEFINITION, "Go to Definition", false),
                item(MENU_EDIT_GO_TO_DECLARATION, "Go to Declaration", false),
                item(
                    MENU_EDIT_GO_TO_TYPE_DEFINITION,
                    "Go to Type Definition",
                    false,
                ),
                item(
                    MENU_EDIT_GO_TO_IMPLEMENTATION,
                    "Go to Implementation",
                    false,
                ),
                item(MENU_EDIT_CUT, "Cut", true),
                item(MENU_EDIT_COPY, "Copy", false),
                item(MENU_EDIT_COPY_TRIM, "Copy and Trim", false),
                item(MENU_EDIT_PASTE, "Paste", false),
                item(MENU_EDIT_REVEAL, "Reveal in Finder", true),
                item(crate::MENU_EDIT_OPEN_TERMINAL, "Open in Terminal", false),
            ]
            .into_iter()
            .chain(
                self.group_view(self.menu_group)
                    .and_then(|group| group.active_item())
                    .filter(|item| item.previewable())
                    .map(|_| {
                        item(
                            crate::MENU_EDIT_MARKDOWN_PREVIEW,
                            "Open Markdown Preview",
                            true,
                        )
                    }),
            )
            .chain(
                self.input_ref(self.menu_group)
                    .and_then(|input| input.editor_split_diff())
                    .map(|split| MenuItem {
                        id: crate::MENU_EDIT_SPLIT_DIFF,
                        label: "Split Diff".into(),
                        checked: split,
                        sep: true,
                        disabled: false,
                        danger: false,
                        icon: None,
                        hint: None,
                        color: None,
                        header: false,
                    }),
            )
            .collect();
        }
        if target == SIDEBAR_TOGGLE {
            let left = self.layout.sidebar_left();
            // The sidebar is always present (toggled via its button), so no Hide item.
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left".into(),
                    checked: left,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right".into(),
                    checked: !left,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
            ]
        } else if target == AGENT_TOGGLE {
            let left = self.layout.agent_side == DockPosition::Left;
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left".into(),
                    checked: left,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right".into(),
                    checked: !left,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
                hide,
            ]
        } else if target == BOTTOM_TOGGLE {
            let side = self.layout.terminal_side;
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left".into(),
                    checked: side == DockPosition::Left,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right".into(),
                    checked: side == DockPosition::Right,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
                MenuItem {
                    id: MENU_DOCK_BOTTOM,
                    label: "Dock Bottom".into(),
                    checked: side == DockPosition::Bottom,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
                hide,
            ]
        } else if let Some(i) = target.checked_sub(FUNC_BASE) {
            let side = self
                .layout
                .func_side
                .get(i as usize)
                .copied()
                .unwrap_or(DockPosition::Left);
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left".into(),
                    checked: side == DockPosition::Left,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right".into(),
                    checked: side == DockPosition::Right,
                    sep: false,
                    disabled: false,
                    danger: false,
                    icon: None,
                    hint: None,
                    color: None,
                    header: false,
                },
                hide,
            ]
        } else {
            vec![hide]
        }
    }

    fn dock_item_side(item: u64) -> DockPosition {
        match item {
            MENU_DOCK_RIGHT => DockPosition::Right,
            MENU_DOCK_BOTTOM => DockPosition::Bottom,
            _ => DockPosition::Left,
        }
    }

    /// Open the content area on `side` (the center is always open).
    fn open_side(&mut self, side: DockPosition) {
        match side {
            DockPosition::Right => self.layout.right.collapsed = false,
            DockPosition::Bottom => self.layout.bottom.collapsed = false,
            DockPosition::Left => self.layout.panels_collapsed = false,
        }
    }

    /// The agent joins `side`'s dock as its active panel; the dock it left keeps its other panels.
    fn move_agent_to(&mut self, side: DockPosition) {
        let from = self.layout.agent_side;
        self.layout.agent_side = side;
        if from != side && self.layout.active_panels[from.index()] == Some(Shown::Agent) {
            self.layout.active_panels[from.index()] = None;
        }
        self.show_agent_panel();
    }

    fn show_agent_panel(&mut self) {
        let side = self.layout.agent_side;
        self.layout.active_panels[side.index()] = Some(Shown::Agent);
        match side {
            DockPosition::Right => self.layout.right.collapsed = false,
            DockPosition::Left => self.layout.panels_collapsed = false,
            DockPosition::Bottom => {}
        }
    }

    fn toggle_side(&mut self, side: DockPosition, was_visible: bool) {
        match side {
            DockPosition::Right => self.layout.right.collapsed = was_visible,
            DockPosition::Bottom => self.layout.bottom.collapsed = was_visible,
            DockPosition::Left => self.layout.panels_collapsed = was_visible,
        }
    }

    fn side_menu_kind(target: u64) -> Option<PaneKind> {
        let index = target.checked_sub(crate::SIDE_PANEL_MENU_TARGET)?;
        (index < 10).then(|| PaneKind::from_index(index as usize))?
    }

    fn apply_panel_requests(&mut self) {
        let requests: Vec<(PaneKind, crate::PanelRequest)> = self
            .layout
            .side_panels
            .iter_mut()
            .flat_map(|panel| {
                let kind = panel.kind();
                panel
                    .take_requests()
                    .into_iter()
                    .map(move |request| (kind, request))
            })
            .collect();
        for (kind, request) in requests {
            self.apply_request(kind, request);
        }
        let mut from_tabs = Vec::new();
        if let Some(group) = self
            .layout
            .files_view
            .as_mut()
            .and_then(|view| view.pane_group_mut())
        {
            group.for_each_item_mut(&mut |item| from_tabs.extend(item.take_requests()));
        }
        for request in from_tabs {
            self.apply_request(PaneKind::Database, request);
        }
    }

    fn apply_request(&mut self, kind: PaneKind, request: crate::PanelRequest) {
        match request {
            crate::PanelRequest::Prompt {
                tag,
                message,
                detail,
                buttons,
            } => {
                let token = PANEL_PROMPT_TOKENS + self.next_prompt_token;
                self.next_prompt_token += 1;
                self.panel_prompt = Some((token, kind, tag));
                self.pending_prompt = Some(crate::Prompt {
                    token,
                    message,
                    detail,
                    buttons,
                });
            }
            crate::PanelRequest::OpenItem(item) => {
                if let Some(files) = self.layout.files_view.as_mut() {
                    files.add_center_item(item);
                    self.set_terminal_focus(false);
                    self.panes_input = true;
                }
            }
            crate::PanelRequest::Reveal { id, open } => {
                let Some(files) = self.layout.files_view.as_mut() else {
                    return;
                };
                let revealed = files.pane_group_mut().is_some_and(|group| {
                    let found = group.reveal_item(&id);
                    if found {
                        group.keep_preview_of(&id);
                    }
                    found
                });
                if !revealed {
                    if let Some(item) = open() {
                        files.add_center_item(item);
                    }
                }
                self.set_terminal_focus(false);
                self.panes_input = true;
            }
            crate::PanelRequest::RevealPreview { id, open } => {
                let Some(files) = self.layout.files_view.as_mut() else {
                    return;
                };
                let Some(group) = files.pane_group_mut() else {
                    return;
                };
                if !group.reveal_item(&id) {
                    if let Some(item) = open() {
                        group.add_preview_item(item);
                    }
                }
                self.set_terminal_focus(false);
                self.panes_input = true;
            }
            crate::PanelRequest::OpenDiff { path, base } => {
                if let Some(files) = self.layout.files_view.as_mut() {
                    files.open_diff(&path, base);
                    self.set_terminal_focus(false);
                    self.panes_input = true;
                }
            }
            crate::PanelRequest::OpenFile(path) => {
                if let Some(files) = self.layout.files_view.as_mut() {
                    files.open_file_at(&path, None, None);
                    self.set_terminal_focus(false);
                    self.panes_input = true;
                }
            }
            crate::PanelRequest::OpenUrl(url) => {
                if let Err(error) = std::process::Command::new("open").arg(&url).spawn() {
                    self.show_toast(format!("Failed to open {url}: {error}"), None);
                }
            }
            crate::PanelRequest::Copy(text) => Self::clip_set(&text),
            crate::PanelRequest::FixWithAgent(fix) => {
                self.agent_fix = Some((fix, crate::SideAgentRole::Fix));
            }
            crate::PanelRequest::AskAgent(fix) => {
                self.agent_fix = Some((fix, crate::SideAgentRole::Ask));
            }
            crate::PanelRequest::Toast(message) => {
                self.toast_then = None;
                self.show_toast(message, None);
            }
            crate::PanelRequest::ToastAction {
                message,
                action,
                then,
            } => {
                self.show_toast(message, Some(action));
                self.toast_then = Some((kind, *then));
            }
            crate::PanelRequest::RunCommand {
                title,
                cwd,
                argv,
                env,
            } => {
                let Some(view) = self.layout.terminal_view.as_mut() else {
                    return;
                };
                match view.command_item(title, cwd, argv, env) {
                    Some(item) => {
                        view.accept_foreign_item(item);
                        self.show_terminal();
                        self.set_terminal_focus(true);
                        self.panes_input = true;
                    }
                    None => self.show_toast("Could not start the command", None),
                }
            }
            request @ (crate::PanelRequest::KeepBranch { .. }
            | crate::PanelRequest::SwitchBranch { .. }
            | crate::PanelRequest::PickBranch { .. }) => self.branch_requests.push(request),
            crate::PanelRequest::OpenMenu => {
                let (x, y) = self.press;
                // A tab's own button is not among the window's hits; the menu then drops from the press.
                let anchor = self
                    .hit_with_rect(x, y)
                    .map(|(_, rect)| rect)
                    .or(Some(Rect::new(x, y, 0.0, 0.0, Rgba::TRANSPARENT)));
                if let Some(rect) = anchor {
                    self.menu = Some((
                        rect.x,
                        rect.y,
                        rect.y + rect.h,
                        crate::SIDE_PANEL_MENU_TARGET + kind.index() as u64,
                    ));
                    self.menu_path = None;
                    self.submenu = None;
                    self.menu_editor_anchor = None;
                }
            }
        }
    }

    fn clip_set(text: &str) {
        if let Ok(mut c) = arboard::Clipboard::new() {
            let _ = c.set_text(text.to_string());
        }
    }

    fn clip_get() -> Option<String> {
        arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_text().ok())
    }

    /// Apply a context-menu item to its target button.
    /// Runs the open menu's item whose key hint is `key` (a menu shows "S", "R", "Enter" for its items).
    fn menu_shortcut(&mut self, key: &str) -> bool {
        let Some((_, _, _, target)) = self.menu else {
            return false;
        };
        let item = self
            .menu_items(target)
            .into_iter()
            .find(|item| !item.disabled && item.hint.as_deref() == Some(key));
        let Some(item) = item else {
            return false;
        };
        self.close_menu();
        self.apply_menu(target, item.id);
        true
    }

    fn close_menu(&mut self) {
        self.menu = None;
        self.submenu = None;
        self.menu_path = None;
        self.menu_editor_anchor = None;
    }

    /// Opens the agent dock's popover under the pressed button, or closes it when that one is already open.
    fn toggle_agent_popover(&mut self, kind: crate::agent_popover::AgentPopoverKind) {
        let same = self.agent_popover.is_some_and(|(open, _)| {
            std::mem::discriminant(&open) == std::mem::discriminant(&kind)
        });
        if same {
            self.agent_popover = None;
            return;
        }
        let (x, y) = self.press;
        self.agent_popover = self.hit_with_rect(x, y).map(|(_, rect)| (kind, rect));
    }

    fn agent_popover_click(&mut self, id: u64) {
        use crate::agent_popover::{
            AgentPopoverKind, HISTORY_BASE, NEW_WORKSPACE, ROLE_BASE, START_BASE, START_BUTTON,
        };
        let Some((kind, anchor)) = self.agent_popover else {
            return;
        };
        let (x, y) = self.press;
        let double = self.click_count(x, y) >= 2;
        match kind {
            AgentPopoverKind::New { role, start } => {
                if id == NEW_WORKSPACE {
                    self.agent_popover = None;
                    self.workspace_requests.new_workspace = true;
                } else if id == START_BUTTON {
                    self.agent_popover = None;
                    self.pending.side_agent = Some((role, start));
                } else if let Some(picked) = id
                    .checked_sub(ROLE_BASE)
                    .and_then(|offset| crate::SideAgentRole::ALL.get(offset as usize))
                    .filter(|_| id < START_BASE)
                {
                    let start = if *picked == crate::SideAgentRole::SecondOpinion {
                        crate::SideAgentStart::Fresh
                    } else {
                        start
                    };
                    self.agent_popover = Some((
                        AgentPopoverKind::New {
                            role: *picked,
                            start,
                        },
                        anchor,
                    ));
                } else if let Some(picked) = id
                    .checked_sub(START_BASE)
                    .and_then(|offset| crate::SideAgentStart::ALL.get(offset as usize))
                    .filter(|_| id < START_BUTTON)
                {
                    if double {
                        self.agent_popover = None;
                        self.pending.side_agent = Some((role, *picked));
                    } else {
                        self.agent_popover = Some((
                            AgentPopoverKind::New {
                                role,
                                start: *picked,
                            },
                            anchor,
                        ));
                    }
                }
            }
            AgentPopoverKind::History => {
                if let Some(index) = id.checked_sub(HISTORY_BASE) {
                    self.agent_popover = None;
                    self.pending.reopen_agent = Some(index as usize);
                }
            }
        }
    }

    /// The agent dock's popover, dropped from its button and kept inside the window.
    fn agent_popover_painted(&self, w: f32) -> Option<Painted> {
        let (kind, anchor) = self.agent_popover?;
        let branch = self
            .layout
            .project
            .as_ref()
            .map(|project| project.active.clone())
            .unwrap_or_default();
        let (node, width) = match kind {
            crate::agent_popover::AgentPopoverKind::New { role, start } => (
                crate::agent_popover::new_agent(
                    &branch,
                    role,
                    start,
                    self.layout.side_agent_sizes,
                    self.layout.main_agent_tokens,
                    self.layout.other_cli.as_deref(),
                    self.session_menu_hover,
                ),
                crate::agent_popover::WIDTH,
            ),
            crate::agent_popover::AgentPopoverKind::History => (
                crate::agent_popover::history(
                    &self.layout.archived_agents,
                    self.session_menu_hover,
                ),
                crate::agent_popover::WIDTH - 40.0,
            ),
        };
        let scale = ui::ui_text_scale();
        let height = ui::measure(&node).1;
        let x = (anchor.x + anchor.w - width * scale).clamp(
            8.0 * scale,
            (w - width * scale - 8.0 * scale).max(8.0 * scale),
        );
        let y = anchor.y + anchor.h + 4.0 * scale;
        Some(ui::render(
            &node,
            Rect::new(x, y, width * scale, height * scale, Rgba::TRANSPARENT),
        ))
    }

    /// Opens the usage card under the chip, or the app menu, as a click on them would (for snapshots).
    pub fn show_usage_card(&mut self, over_status: bool, anchor: Rect) {
        self.usage_popover = Some((over_status, anchor));
    }

    /// Open (or close, when it is open) a menu over the status-bar item just pressed, left-aligned to it.
    fn toggle_status_menu(&mut self, target: u64) {
        let open = self.menu.is_some_and(|menu| menu.3 == target);
        self.menu = None;
        self.submenu = None;
        self.usage_popover = None;
        if open {
            return;
        }
        let (x, y) = self.press;
        if let Some((_, rect)) = self.hit_with_rect(x, y) {
            self.menu = Some((rect.x, rect.y, rect.y + rect.h, target));
            self.menu_path = None;
            self.menu_editor_anchor = None;
        }
    }

    pub fn show_app_menu(&mut self, anchor: Rect) {
        self.menu = Some((
            anchor.x + anchor.w,
            anchor.y,
            anchor.y + anchor.h,
            crate::APP_MENU_TARGET,
        ));
    }

    pub fn set_update(&mut self, update: crate::UpdateInfo) -> bool {
        if self.layout.update == update {
            return false;
        }
        self.layout.update = update;
        true
    }

    pub fn set_usage(&mut self, usage: crate::UsageInfo) -> bool {
        if self.layout.usage == usage {
            return false;
        }
        self.layout.usage = usage;
        true
    }

    fn toggle_usage_popover(&mut self, over_status: bool) {
        if self
            .usage_popover
            .is_some_and(|(status, _)| status == over_status)
        {
            self.usage_popover = None;
            return;
        }
        let (x, y) = self.press;
        self.usage_popover = self
            .hit_with_rect(x, y)
            .map(|(_, rect)| (over_status, rect));
    }

    fn usage_popover_painted(&self, w: f32) -> Option<Painted> {
        let (over_status, anchor) = self.usage_popover?;
        let (node, width) = if over_status {
            (
                crate::usage::today_card(&self.layout.usage, self.session_menu_hover),
                crate::usage::TODAY_WIDTH,
            )
        } else {
            (
                crate::usage::chip_card(&self.layout.usage, self.session_menu_hover),
                crate::usage::CARD_WIDTH,
            )
        };
        let scale = ui::ui_text_scale();
        let height = ui::measure(&node).1 * scale;
        let x = (anchor.x + anchor.w - width * scale).clamp(
            8.0 * scale,
            (w - width * scale - 8.0 * scale).max(8.0 * scale),
        );
        let y = if over_status {
            anchor.y - height - 4.0 * scale
        } else {
            anchor.y + anchor.h + 4.0 * scale
        };
        Some(ui::render(
            &node,
            Rect::new(x, y, width * scale, height, Rgba::TRANSPARENT),
        ))
    }

    fn app_menu_items(&self) -> Vec<MenuItem> {
        let entry = |id: u64, label: String, sep: bool, hint: Option<&'static str>| MenuItem {
            id,
            label: label.into(),
            checked: false,
            sep,
            disabled: false,
            danger: false,
            icon: None,
            hint: hint.map(Into::into),
            color: None,
            header: false,
        };
        let mut items = Vec::new();
        if let Some(account) = &self.layout.usage.account {
            let mut item = entry(crate::MENU_APP_ACCOUNT, account.name.clone(), false, None);
            item.icon = Some(ui::IconKind::Sparkle);
            item.hint = (!account.plan.is_empty()).then(|| {
                let mut plan = account.plan.clone();
                if let Some(first) = plan.get(0..1) {
                    plan = first.to_uppercase() + &plan[1..];
                }
                plan.into()
            });
            items.push(item);
        }
        if let Some(update) = &self.layout.update.menu {
            let mut item = entry(
                crate::MENU_APP_UPDATE,
                update.label.clone(),
                !items.is_empty(),
                None,
            );
            item.disabled = !update.enabled;
            items.push(item);
        }
        let first = !items.is_empty();
        items.push(entry(
            crate::MENU_APP_SETTINGS,
            "Settings".into(),
            first,
            Some("cmd-,"),
        ));
        items.push(entry(
            crate::MENU_APP_KEYMAP,
            "Keymap".into(),
            false,
            Some("cmd-k cmd-s"),
        ));
        items.push(entry(
            crate::MENU_APP_THEME,
            "Next Theme".into(),
            false,
            Some("cmd-k cmd-t"),
        ));
        items.push(entry(
            crate::MENU_APP_USAGE,
            "Agent Usage".into(),
            true,
            Some("cmd-shift-u"),
        ));
        items.push(entry(
            crate::MENU_SUBMENU_LAYOUT,
            "Panel Layout".into(),
            false,
            None,
        ));
        items.push(entry(
            crate::MENU_APP_RELEASE_NOTES,
            "Release Notes".into(),
            true,
            None,
        ));
        items
    }

    fn panel_layout_items(&self) -> Vec<MenuItem> {
        let left = self.layout.agent_side == DockPosition::Left;
        let entry = |id: u64, label: &'static str, checked: bool| MenuItem {
            id,
            label: label.into(),
            checked,
            sep: false,
            disabled: false,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        };
        vec![
            entry(MENU_DOCK_RIGHT, "Agent on the Right", !left),
            entry(MENU_DOCK_LEFT, "Agent on the Left", left),
        ]
    }

    /// Whether a submenu's items are applied under the submenu's own id rather than the menu it opened from.
    fn menu_owns(&self, target: u64) -> bool {
        target == crate::MENU_SUBMENU_LAYOUT
    }

    fn apply_menu(&mut self, target: u64, item: u64) {
        if target == crate::LANGUAGE_SERVERS_MENU_TARGET
            || target == crate::LANGUAGE_ACTIVITY_MENU_TARGET
        {
            if let (Some(action), Some(view)) = (
                crate::language_server_action(item),
                self.layout.files_view.as_mut(),
            ) {
                view.language_server_action(action);
            }
            return;
        }
        if target == crate::ITEM_MENU_TARGET {
            if let Some(view) = self.layout.files_view.as_mut() {
                view.menu_pick(item);
            }
            return;
        }
        if target == crate::APP_MENU_TARGET {
            match item {
                crate::MENU_APP_ACCOUNT | crate::MENU_APP_USAGE => {
                    self.pending.action = Some(crate::keymap::Action::OpenAgentUsage)
                }
                crate::MENU_APP_SETTINGS => self.pending.open_settings = true,
                crate::MENU_APP_KEYMAP => {
                    self.pending.action = Some(crate::keymap::Action::OpenKeymap)
                }
                crate::MENU_APP_THEME => {
                    self.pending.action = Some(crate::keymap::Action::CycleTheme)
                }
                crate::MENU_APP_UPDATE => {
                    self.pending.update = self
                        .layout
                        .update
                        .menu
                        .as_ref()
                        .filter(|item| item.enabled)
                        .map(|item| item.action)
                }
                crate::MENU_APP_RELEASE_NOTES => {
                    self.pending.update = Some(crate::UpdateAction::ReleaseNotes)
                }
                _ => {}
            }
            return;
        }
        if target == crate::MENU_SUBMENU_LAYOUT {
            if matches!(item, MENU_DOCK_LEFT | MENU_DOCK_RIGHT) {
                self.move_agent_to(Self::dock_item_side(item));
                self.pending.persist = true;
            }
            return;
        }
        if target == crate::WORKSPACE_ROW_MENU_TARGET {
            self.apply_workspace_row_menu(item);
            return;
        }
        if target == crate::TAB_MENU_TARGET {
            self.apply_tab_menu(item);
            return;
        }
        if let Some(kind) = Self::side_menu_kind(target) {
            if let Some(panel) = self.layout.side_panel_mut(kind) {
                panel.menu_action(item);
            }
            self.apply_panel_requests();
            return;
        }
        if target == TREE_MENU_TARGET {
            let Some((rel, _)) = self.menu_path.clone() else {
                return;
            };
            let abs = self
                .layout
                .files_view
                .as_ref()
                .and_then(|v| v.root_dir())
                .map(|r| r.join(&rel));
            if item == crate::MENU_TREE_OPEN_TERMINAL {
                let directory = abs.map(|path| {
                    if path.is_dir() {
                        path
                    } else {
                        path.parent()
                            .map(|parent| parent.to_path_buf())
                            .unwrap_or(path)
                    }
                });
                self.open_terminal_at(directory);
                return;
            }
            let tree_action = match item {
                MENU_TREE_NEW_FILE => Some(TreeAction::NewFile),
                MENU_TREE_NEW_DIR => Some(TreeAction::NewDirectory),
                MENU_TREE_OPEN_SYSTEM => Some(TreeAction::OpenWithSystem),
                MENU_TREE_CUT => Some(TreeAction::Cut),
                MENU_TREE_COPY => Some(TreeAction::Copy),
                MENU_TREE_DUPLICATE => Some(TreeAction::Duplicate),
                MENU_TREE_PASTE => Some(TreeAction::Paste),
                MENU_TREE_RESTORE => Some(TreeAction::RestoreFile),
                MENU_TREE_GITIGNORE => Some(TreeAction::AddToGitignore),
                MENU_TREE_RENAME => Some(TreeAction::Rename),
                MENU_TREE_TRASH => Some(TreeAction::Trash),
                MENU_TREE_DELETE => Some(TreeAction::Delete),
                MENU_TREE_EXPAND_ALL => Some(TreeAction::ExpandAll),
                MENU_TREE_COLLAPSE_ALL => Some(TreeAction::CollapseAll),
                _ => None,
            };
            if let Some(action) = tree_action {
                let path = (!rel.is_empty()).then_some(rel.as_str());
                let prompt = self
                    .layout
                    .files_view
                    .as_mut()
                    .and_then(|v| v.tree_action(path, action));
                if prompt.is_some() {
                    self.pending_prompt = prompt;
                }
                self.show_view_toast();
                return;
            }
            match item {
                MENU_TREE_OPEN => {
                    if let Some(v) = self.layout.files_view.as_mut() {
                        v.open_path(&rel);
                    }
                }
                MENU_COPY_PATH => {
                    if let Some(p) = abs {
                        Self::clip_set(&p.to_string_lossy());
                    }
                }
                MENU_COPY_REL_PATH => Self::clip_set(&rel),
                MENU_COPY_NAME => {
                    let name = rel.rsplit('/').next().unwrap_or(&rel);
                    Self::clip_set(name);
                }
                MENU_REVEAL => {
                    if let Some(p) = abs {
                        let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
                    }
                }
                _ => {}
            }
            return;
        }
        if target == crate::TERMINAL_MENU_TARGET {
            self.terminal_menu_click(item);
            return;
        }
        if target == EDITOR_MENU_TARGET {
            let group = self.menu_group;
            match item {
                MENU_EDIT_COPY => {
                    self.editor_copy_to_clipboard();
                }
                MENU_EDIT_CUT => {
                    self.editor_cut_to_clipboard();
                }
                MENU_EDIT_PASTE => {
                    self.editor_paste_from_clipboard();
                }
                MENU_EDIT_SELECT_ALL => {
                    if let Some(input) = self.input(group) {
                        input.editor_key(EditKey::SelectAll, false);
                    }
                }
                MENU_EDIT_COPY_TRIM => {
                    let copied = self
                        .input_ref(group)
                        .and_then(|input| input.editor_copy_trimmed());
                    if let Some(copied) = copied {
                        Self::clip_set(&copied.text);
                        crate::remember_copy(&copied);
                    }
                }
                MENU_EDIT_GO_TO_DEFINITION
                | MENU_EDIT_GO_TO_DECLARATION
                | MENU_EDIT_GO_TO_TYPE_DEFINITION
                | MENU_EDIT_GO_TO_IMPLEMENTATION => {
                    let key = match item {
                        MENU_EDIT_GO_TO_DEFINITION => EditKey::GoToDefinition,
                        MENU_EDIT_GO_TO_DECLARATION => EditKey::GoToDeclaration,
                        MENU_EDIT_GO_TO_TYPE_DEFINITION => EditKey::GoToTypeDefinition,
                        _ => EditKey::GoToImplementation,
                    };
                    if let Some(input) = self.input(group) {
                        input.editor_key(key, false);
                    }
                }
                crate::MENU_EDIT_SPLIT_DIFF => {
                    if let Some(input) = self.input(group) {
                        input.editor_key(EditKey::ToggleSplitDiff, false);
                    }
                }
                crate::MENU_EDIT_MARKDOWN_PREVIEW => {
                    if let Some(files) = self.layout.files_view.as_mut() {
                        files.editor_key(EditKey::OpenMarkdownPreview, false);
                    }
                }
                crate::MENU_EDIT_OPEN_TERMINAL => {
                    let directory = self
                        .active_file_in(group)
                        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
                    self.open_terminal_at(directory);
                }
                MENU_EDIT_REVEAL => {
                    let path = self.active_file_in(group);
                    if let Some(path) = path {
                        if let Err(error) = std::process::Command::new("open")
                            .arg("-R")
                            .arg(path)
                            .spawn()
                        {
                            eprintln!("reveal in Finder: {error}");
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        // Any dock move/hide changes the persisted layout.
        self.pending.persist = true;
        // A function: Dock Left/Right/Bottom moves its content to that area and makes it that area's active panel.
        if let Some(i) = target.checked_sub(FUNC_BASE) {
            let i = i as usize;
            match item {
                MENU_DOCK_LEFT | MENU_DOCK_RIGHT | MENU_DOCK_BOTTOM => {
                    let side = Self::dock_item_side(item);
                    if let Some(slot) = self.layout.func_side.get_mut(i) {
                        *slot = side;
                    }
                    self.layout.active_panels[side.index()] = Some(Shown::Func(PaneKind::ALL[i]));
                    self.open_side(side);
                }
                MENU_HIDE => {
                    if let Some(h) = self.layout.func_hidden.get_mut(i) {
                        *h = true;
                    }
                }
                _ => {}
            }
            return;
        }
        // The terminal: Dock Left/Right/Bottom moves it to that area and makes it that area's active panel.
        if target == BOTTOM_TOGGLE {
            match item {
                MENU_DOCK_LEFT | MENU_DOCK_RIGHT | MENU_DOCK_BOTTOM => {
                    let side = Self::dock_item_side(item);
                    self.layout.terminal_side = side;
                    self.layout.active_panels[side.index()] = Some(Shown::Terminal);
                    self.open_side(side);
                }
                MENU_HIDE => self.layout.terminal_hidden = true,
                _ => {}
            }
            return;
        }
        // The remaining targets are the sidebar toggle and the agent button.
        if target == SIDEBAR_TOGGLE {
            match item {
                MENU_DOCK_LEFT => self.layout.sidebar_side = DockPosition::Left,
                MENU_DOCK_RIGHT => self.layout.sidebar_side = DockPosition::Right,
                _ => {}
            }
            return;
        }
        if target == AGENT_TOGGLE {
            match item {
                MENU_DOCK_LEFT | MENU_DOCK_RIGHT => {
                    self.move_agent_to(Self::dock_item_side(item));
                }
                MENU_HIDE => self.layout.agent_hidden = true,
                _ => {}
            }
        }
    }

    /// Right-click: open the context menu for a status-bar button; elsewhere closes any menu. Returns true if
    /// something changed (repaint).
    pub fn right_click(&mut self, x: f32, y: f32) -> bool {
        if self.window_modal.is_some() {
            return false;
        }
        let row = self
            .hit(x, y)
            .filter(|id| (crate::WORKSPACE_ROW_BASE..crate::WORKSPACE_ROW_END).contains(id));
        if let Some(id) = row {
            self.menu = Some((x, y, y, crate::WORKSPACE_ROW_MENU_TARGET));
            self.menu_workspace = Some((id - crate::WORKSPACE_ROW_BASE) as usize);
            self.menu_path = None;
            self.submenu = None;
            self.menu_editor_anchor = None;
            return true;
        }
        if let Some(id) = self.hit(x, y).filter(|id| crate::is_side_panel_id(*id)) {
            let kind = crate::side_panel_kind(id);
            let opened = kind
                .and_then(|kind| self.layout.side_panel_mut(kind))
                .is_some_and(|panel| panel.open_menu(id));
            if let (true, Some(kind)) = (opened, kind) {
                self.menu = Some((x, y, y, crate::SIDE_PANEL_MENU_TARGET + kind.index() as u64));
                self.menu_path = None;
                self.submenu = None;
                self.menu_editor_anchor = None;
                return true;
            }
            if kind != Some(PaneKind::Files) {
                let had = self.menu.take().is_some();
                self.submenu = None;
                self.menu_editor_anchor = None;
                return had;
            }
        }
        if let Some(tab) = self.hit(x, y).and_then(|id| self.tab_under(id)) {
            self.menu = Some((x, y, y, crate::TAB_MENU_TARGET));
            self.menu_tab = Some(tab);
            self.menu_path = None;
            self.submenu = None;
            self.menu_editor_anchor = None;
            return true;
        }
        if let Some(id) = self.hit(x, y).filter(|id| Self::menuable(*id)) {
            // Anchor at the actual button clicked (the rect under the cursor), not just the first with this id.
            let anchor = self
                .header_hits
                .iter()
                .find(|(r, hid)| {
                    *hid == id && x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
                })
                .or_else(|| self.header_hits.iter().find(|(_, hid)| *hid == id))
                .map(|(r, _)| (r.x, r.y, r.y + r.h));
            if let Some((ax, atop, abottom)) = anchor {
                self.menu = Some((ax, atop, abottom, id));
                self.menu_path = None;
                self.submenu = None;
                self.menu_editor_anchor = None;
                return true;
            }
        }
        if let Some(path) = self
            .hit(x, y)
            .and_then(|id| self.layout.files_view.as_ref().and_then(|v| v.row_path(id)))
        {
            self.menu = Some((x, y, y, TREE_MENU_TARGET));
            self.menu_path = Some(path);
            self.submenu = None;
            self.menu_editor_anchor = None;
            return true;
        }
        let (vw, vh) = self.viewport;
        let tree = self.layout.tree_region(vw, vh);
        let in_tree = x >= tree.x && x < tree.x + tree.w && y >= tree.y && y < tree.y + tree.h;
        let files_shown =
            self.layout.shown_on(DockPosition::Left) == Some(Shown::Func(PaneKind::Files));
        if in_tree && files_shown && self.layout.files_view.is_some() {
            self.menu = Some((x, y, y, TREE_MENU_TARGET));
            self.menu_path = Some((String::new(), true));
            self.submenu = None;
            self.menu_editor_anchor = None;
            return true;
        }
        let cr = self.layout.center_region(vw, vh);
        let in_center = x >= cr.x && x < cr.x + cr.w && y >= cr.y && y < cr.y + cr.h;
        let group = self
            .zoom_group_at(x, y)
            .or_else(|| self.dock_body_group_at(x, y))
            .or_else(|| in_center.then_some(InputGroup::Center));
        if let Some((group, path, context)) = group.and_then(|group| self.terminal_at(group, x, y))
        {
            self.focus_group(group);
            self.menu_group = group;
            self.menu_terminal = Some((group, path, context));
            self.menu = Some((x, y, y, crate::TERMINAL_MENU_TARGET));
            self.menu_path = None;
            self.submenu = None;
            self.menu_editor_anchor = None;
            return true;
        }
        let pressed = group.filter(|group| {
            self.input(*group)
                .is_some_and(|input| input.editor_right_press(x, y))
        });
        if let Some(group) = pressed {
            self.focus_group(group);
            self.menu_group = group;
            self.menu = Some((x, y, y, EDITOR_MENU_TARGET));
            self.menu_path = None;
            self.submenu = None;
            self.menu_editor_anchor = self
                .input_ref(group)
                .and_then(|input| input.editor_menu_anchor_at(x, y));
            return true;
        }
        let had = self.menu.take().is_some();
        self.submenu = None;
        self.menu_editor_anchor = None;
        had
    }

    /// Cursor move: drag a divider, or hover the header/menu. Returns true if a repaint is warranted.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        if std::mem::take(&mut self.titlebar_press) {
            self.titlebar_click = None;
            self.pending.titlebar = Some(TitlebarGesture::Move);
        }
        if self.page_body_at(x, y) {
            let modifiers = terminal_modifiers();
            return self
                .standalone_page()
                .is_some_and(|page| page.pointer_move(x, y, modifiers, true));
        }
        let hit = self.hit(x, y);
        let modifiers = terminal_modifiers();
        let modal_hover = hit.filter(|id| crate::is_window_modal_id(*id));
        let mut repaint = self
            .window_modal
            .as_mut()
            .is_some_and(|modal| modal.hover(modal_hover));
        repaint |= self
            .layout
            .terminal_view
            .as_mut()
            .is_some_and(|view| view.set_hover(hit));
        repaint |= self
            .layout
            .agent_view
            .as_mut()
            .is_some_and(|view| view.set_hover(hit));
        if let Drag::ItemPointer(group) = self.dragging {
            return repaint
                | self
                    .input(group)
                    .is_some_and(|input| input.item_pointer_drag(x, y, modifiers));
        }
        if self.dragging == Drag::None {
            for group in self.visible_groups() {
                repaint |= self
                    .input(group)
                    .is_some_and(|input| input.item_pointer_move(x, y, modifiers));
            }
        }
        let over_popover = self.popover_group_at(x, y);
        for group in self.visible_groups() {
            let popover_hit = hit.filter(|_| over_popover == Some(group));
            if let Some(input) = self.input(group) {
                input.popover_hover(popover_hit);
            }
        }
        repaint | self.mouse_move_inner(x, y)
    }

    /// A terminal-panel tab dragged over the center previews where it would land there.
    fn update_center_foreign_drop(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) {
        let source = if self.dragging == Drag::AgentTab {
            self.layout.agent_view.as_ref()
        } else {
            self.layout.terminal_view.as_ref()
        };
        if let (Some(files), Some(panel)) = (self.layout.files_view.as_mut(), source) {
            match panel.dragged_item() {
                Some(item) => {
                    files.update_foreign_drop(x, y, over, item);
                }
                None => files.clear_foreign_drop(),
            }
        }
    }

    /// A center tab dragged over the terminal panel previews there, when the panel can host the item.
    fn update_panel_foreign_drop(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) {
        let visible = self.layout.terminal_visible();
        if let (Some(files), Some(panel)) = (
            self.layout.files_view.as_ref(),
            self.layout.terminal_view.as_mut(),
        ) {
            match files
                .dragged_item()
                .filter(|item| visible && panel.accepts_item(*item))
            {
                Some(item) => {
                    panel.update_foreign_drop(x, y, over, item);
                }
                None => panel.clear_foreign_drop(),
            }
        }
    }

    fn finish_center_tab_drag(&mut self) {
        let foreign = self
            .layout
            .terminal_view
            .as_ref()
            .and_then(|panel| panel.tab_drag_overlay())
            .is_some();
        let moved = if foreign {
            self.layout
                .files_view
                .as_mut()
                .and_then(|files| files.take_dragged_item())
        } else {
            None
        };
        match moved {
            Some(item) => {
                if let Some(panel) = self.layout.terminal_view.as_mut() {
                    panel.accept_foreign_item(item);
                }
                self.set_terminal_focus(true);
            }
            None => {
                if let Some(files) = self.layout.files_view.as_mut() {
                    files.drop_tab();
                }
            }
        }
        if let Some(panel) = self.layout.terminal_view.as_mut() {
            panel.clear_foreign_drop();
        }
    }

    /// An agent tab dropped over the editor area moves there, next to the code; elsewhere it stays in its dock.
    fn finish_agent_tab_drag(&mut self) {
        let over_center = self
            .layout
            .files_view
            .as_ref()
            .and_then(|files| files.tab_drag_overlay())
            .is_some();
        let moved = if over_center {
            self.layout
                .agent_view
                .as_mut()
                .and_then(|panel| panel.take_dragged_item())
        } else {
            None
        };
        match moved {
            Some(item) => {
                if let Some(files) = self.layout.files_view.as_mut() {
                    files.accept_foreign_item(item);
                }
                self.set_agent_focus(false);
                let dock_empty = self
                    .layout
                    .agent_view
                    .as_ref()
                    .is_none_or(|view| view.is_empty());
                if dock_empty {
                    self.layout.right.collapsed = true;
                    self.pending.persist = true;
                }
            }
            None => {
                if let Some(panel) = self.layout.agent_view.as_mut() {
                    panel.drop_tab();
                }
            }
        }
        if let Some(files) = self.layout.files_view.as_mut() {
            files.clear_foreign_drop();
        }
    }

    fn finish_panel_tab_drag(&mut self) {
        let foreign = self
            .layout
            .files_view
            .as_ref()
            .and_then(|files| files.tab_drag_overlay())
            .is_some();
        let moved = if foreign {
            self.layout
                .terminal_view
                .as_mut()
                .and_then(|panel| panel.take_dragged_item())
        } else {
            None
        };
        match moved {
            Some(item) => {
                if let Some(files) = self.layout.files_view.as_mut() {
                    files.accept_foreign_item(item);
                }
                self.set_terminal_focus(false);
            }
            None => {
                if let Some(panel) = self.layout.terminal_view.as_mut() {
                    panel.drop_tab();
                }
            }
        }
        if let Some(files) = self.layout.files_view.as_mut() {
            files.clear_foreign_drop();
        }
    }

    fn mouse_move_inner(&mut self, x: f32, y: f32) -> bool {
        let over = self.hit_with_rect(x, y);
        match self.dragging {
            Drag::ItemPointer(group) => self
                .input(group)
                .is_some_and(|input| input.item_pointer_drag(x, y, terminal_modifiers())),
            Drag::TerminalTab => {
                self.tab_ghost_at = Some((x, y));
                let own = self
                    .layout
                    .terminal_view
                    .as_mut()
                    .is_some_and(|v| v.update_tab_drag(x, y, over));
                self.update_center_foreign_drop(x, y, over);
                own
            }
            Drag::AgentTab => {
                self.tab_ghost_at = Some((x, y));
                let own = self
                    .layout
                    .agent_view
                    .as_mut()
                    .is_some_and(|v| v.update_tab_drag(x, y, over));
                self.update_center_foreign_drop(x, y, over);
                own
            }
            Drag::TerminalDivider(id) => self
                .layout
                .terminal_view
                .as_mut()
                .is_some_and(|v| v.drag_divider(id, x, y)),
            Drag::Left => {
                self.layout.set_left_divider(x, self.width());
                true
            }
            Drag::Right => {
                self.layout.set_right_divider(x, self.width());
                true
            }
            Drag::Bottom => {
                self.layout.set_bottom_divider(y, self.viewport.1);
                true
            }
            Drag::Tree => {
                self.layout.set_tree_divider(x);
                true
            }
            Drag::Center(id) => self
                .layout
                .files_view
                .as_mut()
                .map(|v| v.drag_divider(id, x, y))
                .unwrap_or(false),
            Drag::Tab => {
                self.tab_ghost_at = Some((x, y));
                let own = self
                    .layout
                    .files_view
                    .as_mut()
                    .map(|v| v.update_tab_drag(x, y, over))
                    .unwrap_or(false);
                self.update_panel_foreign_drop(x, y, over);
                own
            }
            Drag::EditorSel(group) => self
                .input(group)
                .is_some_and(|input| input.editor_drag(x, y)),
            Drag::WorkspaceRow => self.update_row_drag(x, y),
            Drag::WorkspaceGroup => self.update_group_drag(x, y),
            Drag::None => {
                if let Some(drag) = self.group_drag {
                    if (x - drag.press.0).abs() > 5.0 || (y - drag.press.1).abs() > 5.0 {
                        self.dragging = Drag::WorkspaceGroup;
                        return self.update_group_drag(x, y);
                    }
                }
                if let Some(drag) = self.row_drag {
                    if (x - drag.press.0).abs() > 5.0 || (y - drag.press.1).abs() > 5.0 {
                        self.dragging = Drag::WorkspaceRow;
                        return self.update_row_drag(x, y);
                    }
                }
                if let Some((id, px, py)) = self.pending_tab {
                    if (x - px).abs() > 5.0 || (y - py).abs() > 5.0 {
                        self.pending_tab = None;
                        if crate::is_agent_id(id) {
                            if let Some(v) = self.layout.agent_view.as_mut() {
                                if v.begin_tab_drag(id) {
                                    self.dragging = Drag::AgentTab;
                                    v.update_tab_drag(x, y, over);
                                    self.tab_ghost_at = Some((x, y));
                                    return true;
                                }
                            }
                        } else if crate::is_terminal_id(id) {
                            if let Some(v) = self.layout.terminal_view.as_mut() {
                                if v.begin_tab_drag(id) {
                                    self.dragging = Drag::TerminalTab;
                                    v.update_tab_drag(x, y, over);
                                    self.tab_ghost_at = Some((x, y));
                                    return true;
                                }
                            }
                        } else if let Some(v) = self.layout.files_view.as_mut() {
                            if v.begin_tab_drag(id) {
                                self.dragging = Drag::Tab;
                                v.update_tab_drag(x, y, over);
                                self.tab_ghost_at = Some((x, y));
                                return true;
                            }
                        }
                    }
                }
                let mut changed = false;
                if let Some(t) = &mut self.toast {
                    let over = x >= t.rect.x
                        && x < t.rect.x + t.rect.w
                        && y >= t.rect.y
                        && y < t.rect.y + t.rect.h;
                    if over != t.hovered {
                        let now = Instant::now();
                        if over {
                            t.remaining = t.deadline.saturating_duration_since(now);
                        } else {
                            t.deadline = now + t.remaining;
                        }
                        t.hovered = over;
                        changed = true;
                    }
                }
                for group in self.visible_groups() {
                    changed |= self
                        .input(group)
                        .is_some_and(|input| input.editor_hover(x, y));
                }
                let hovered = self.hit(x, y);
                if hovered != self.session_menu_hover {
                    self.session_menu_hover = hovered;
                    changed = true;
                }
                if self.menu.is_some() {
                    if let Some(hid) = hovered {
                        if is_submenu(hid) {
                            if self.submenu.map(|s| s.3) != Some(hid) {
                                if let Some((r, _)) =
                                    self.header_hits.iter().find(|(_, id)| *id == hid)
                                {
                                    self.submenu = Some((r.x + r.w, r.y, r.x, hid));
                                    changed = true;
                                }
                            }
                        } else if let Some((_, _, _, parent)) = self.submenu {
                            let is_child = self.menu_items(parent).iter().any(|it| it.id == hid);
                            if !is_child {
                                self.submenu = None;
                                changed = true;
                            }
                        }
                    }
                }
                if let Some(v) = self.layout.files_view.as_mut() {
                    changed |= v.set_hover(hovered);
                }
                for panel in self.layout.side_panels.iter_mut() {
                    changed |= panel.set_hover(hovered);
                }
                changed
            }
        }
    }

    /// Left-button press: route a header/menu click, close the menu, toggle a dock, or begin a divider drag.
    pub fn mouse_down(&mut self, x: f32, y: f32) {
        self.press = (x, y);
        self.titlebar_press = y < crate::TOP_BAR_H
            && self.hit(x, y).is_none()
            && self.prompt_shown.is_none()
            && self.window_modal.is_none();
        if self.titlebar_press {
            let now = Instant::now();
            let second = self.titlebar_click.is_some_and(|(at, px, py)| {
                now.duration_since(at) < Duration::from_millis(400)
                    && (x - px).abs() < 4.0
                    && (y - py).abs() < 4.0
            });
            if second {
                self.titlebar_click = None;
                self.titlebar_press = false;
                self.pending.titlebar = Some(TitlebarGesture::DoubleClick);
            } else {
                self.titlebar_click = Some((now, x, y));
            }
        }
        if self.prompt_shown.is_some() {
            if let Some(index) = self
                .hit(x, y)
                .and_then(|id| id.checked_sub(PROMPT_BUTTON_BASE))
                .filter(|index| *index < PROMPT_BUTTON_SPAN)
            {
                self.answer_prompt(index as usize);
            }
            return;
        }
        if self.window_modal.is_none() && self.page_body_at(x, y) {
            let count = self.click_count(x, y);
            if let Some(page) = self.standalone_page() {
                page.pointer_down(x, y, count, terminal_modifiers());
            }
            return;
        }
        let within = |rect: Option<Rect>| {
            rect.is_some_and(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
        };
        let on_part = |part: u64| {
            self.hit(x, y).is_some_and(|id| {
                (crate::WORKSPACE_OP_BASE..crate::WORKSPACE_OP_END).contains(&id)
                    && (id - crate::WORKSPACE_OP_BASE) % crate::WORKSPACE_OP_STRIDE == part
            })
        };
        if self.manual_menu.is_some()
            && !within(self.manual_menu_rect)
            && !on_part(crate::WORKSPACE_OP_MANUAL)
        {
            self.manual_menu = None;
        }
        if let Some((_, anchor)) = self.usage_popover {
            let on_anchor = x >= anchor.x
                && x < anchor.x + anchor.w
                && y >= anchor.y
                && y < anchor.y + anchor.h;
            if !within(self.usage_popover_rect) && !on_anchor {
                self.usage_popover = None;
            }
        }
        if let Some((_, anchor)) = self.agent_popover {
            let on_anchor = x >= anchor.x
                && x < anchor.x + anchor.w
                && y >= anchor.y
                && y < anchor.y + anchor.h;
            if !within(self.agent_popover_rect) && !on_anchor {
                self.agent_popover = None;
            }
        }
        if self.rail_popover.is_some() {
            let inside = within(self.rail_popover_rect) || within(self.manual_menu_rect);
            let on_toggle = self.hit(x, y).is_some_and(|id| {
                (crate::WORKSPACE_OP_BASE..crate::WORKSPACE_OP_END).contains(&id)
                    && (id - crate::WORKSPACE_OP_BASE) % crate::WORKSPACE_OP_STRIDE
                        == crate::WORKSPACE_OP_POPOVER
            });
            if !inside && !on_toggle {
                self.rail_popover = None;
                self.manual_menu = None;
            }
        }
        let pressed_panel = self.hit(x, y).and_then(crate::side_panel_kind);
        for panel in self.layout.side_panels.iter_mut() {
            if pressed_panel != Some(panel.kind()) {
                panel.blur();
            }
        }
        if let Some(rect) = self.modal_rect.take() {
            let inside = x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
            if !inside {
                match self.window_modal.as_ref() {
                    Some(modal) if modal.dismissable() => self.cancel_window_modal(),
                    Some(_) => self.modal_rect = Some(rect),
                    None => {
                        if let Some(v) = self.layout.files_view.as_mut() {
                            v.dismiss_modal();
                        }
                    }
                }
                return;
            }
            self.modal_rect = Some(rect);
            if self.window_modal.is_some() {
                if let Some(id) = self.hit(x, y).filter(|id| crate::is_window_modal_id(*id)) {
                    if let Some(modal) = self.window_modal.as_mut() {
                        modal.click(id);
                    }
                    self.settle_window_modal();
                }
                return;
            }
        }
        // A click while a context menu is open either picks an item or dismisses it.
        if let Some((_, _, _, target)) = self.menu {
            let hit = self.hit(x, y);
            if hit.is_some_and(is_submenu) {
                return;
            }
            // A submenu's item belongs to the submenu: its own menus (Panel Layout) take it by the submenu's id.
            let mut rows: Vec<(u64, MenuItem)> = self
                .menu_items(target)
                .into_iter()
                .map(|row| (target, row))
                .collect();
            if let Some(parent) = self.submenu {
                rows.splice(
                    0..0,
                    self.menu_items(parent.3)
                        .into_iter()
                        .map(|row| (parent.3, row)),
                );
            }
            let row = hit.and_then(|id| rows.iter().find(|(_, row)| row.id == id));
            if let Some((owner, row)) = row {
                if row.disabled {
                    return;
                }
                let before = self.menu;
                let owner = if self.menu_owns(*owner) {
                    *owner
                } else {
                    target
                };
                self.apply_menu(owner, row.id);
                // A panel item can open a follow-up menu (a picker); keep that one.
                if self.menu != before {
                    return;
                }
            }
            self.menu = None;
            self.submenu = None;
            self.menu_path = None;
            self.menu_editor_anchor = None;
            return;
        }
        if let Some(group) = self.popover_group_at(x, y) {
            if let Some(id) = self.hit(x, y) {
                if let Some(input) = self.input(group) {
                    input.popover_click(id);
                }
            }
            return;
        }
        let w = self.width();
        let in_zoom = self.zoom_group_at(x, y);
        let dock_edges = in_zoom.is_none();
        if dock_edges && self.layout.on_left_divider(x, y, w) {
            self.dragging = Drag::Left;
        } else if dock_edges && self.layout.on_tree_divider(x, y, w) {
            self.dragging = Drag::Tree;
        } else if dock_edges && self.layout.on_right_divider(x, y, w) {
            self.dragging = Drag::Right;
        } else if dock_edges && self.layout.on_bottom_divider(x, y, w, self.viewport.1) {
            self.dragging = Drag::Bottom;
        } else if let Some(id) = self.hit(x, y) {
            if self
                .layout
                .terminal_view
                .as_ref()
                .and_then(|v| v.divider_axis(id))
                .is_some()
            {
                self.dragging = Drag::TerminalDivider(id);
            } else if self
                .layout
                .files_view
                .as_ref()
                .and_then(|v| v.divider_axis(id))
                .is_some()
            {
                self.dragging = Drag::Center(id);
            } else if self
                .layout
                .files_view
                .as_ref()
                .is_some_and(|v| v.is_tab(id))
            {
                self.pending_tab = Some((id, x, y));
            } else if self
                .layout
                .terminal_view
                .as_ref()
                .is_some_and(|v| v.is_tab(id))
            {
                self.set_terminal_focus(true);
                self.pending_tab = Some((id, x, y));
            } else if self
                .layout
                .agent_view
                .as_ref()
                .is_some_and(|v| v.is_tab(id))
            {
                self.focus_group(InputGroup::Agent);
                self.pending_tab = Some((id, x, y));
            } else {
                let source = self
                    .header_hits
                    .iter()
                    .rev()
                    .find(|(rect, hit)| {
                        *hit == id
                            && x >= rect.x
                            && x < rect.x + rect.w
                            && y >= rect.y
                            && y < rect.y + rect.h
                    })
                    .map(|(rect, _)| *rect);
                self.group_drag = (crate::WORKSPACE_GROUP_BASE..crate::WORKSPACE_GROUP_END)
                    .contains(&id)
                    .then(|| {
                        crate::TicketGroup::ALL
                            .get((id - crate::WORKSPACE_GROUP_BASE) as usize)
                            .copied()
                    })
                    .flatten()
                    .zip(source)
                    .map(|(from, source)| GroupDrag {
                        from,
                        press: (x, y),
                        source,
                        to: None,
                        pointer: (x, y),
                    });
                self.row_drag = id
                    .checked_sub(crate::WORKSPACE_ROW_BASE)
                    .filter(|_| id < crate::WORKSPACE_ROW_END)
                    .map(|index| index as usize)
                    .filter(|index| *index > 0)
                    .zip(source)
                    .map(|(from, source)| RowDrag {
                        from,
                        press: (x, y),
                        source,
                        to: None,
                        pointer: (x, y),
                    });
                self.header_click(id);
            }
        } else {
            let (vw, vh) = self.viewport;
            let cr = self.layout.center_region(vw, vh);
            let in_center = x >= cr.x && x < cr.x + cr.w && y >= cr.y && y < cr.y + cr.h;
            let group = if in_zoom.is_some() {
                in_zoom
            } else {
                self.dock_body_group_at(x, y)
                    .or_else(|| in_center.then_some(InputGroup::Center))
            };
            match group {
                Some(group) => self.focus_group(group),
                None => self.set_terminal_focus(false),
            }
            let mut consumed = false;
            if let Some(group) = group {
                let count = self.click_count(x, y);
                let modifiers = terminal_modifiers();
                let pressed = self
                    .input(group)
                    .is_some_and(|input| input.item_pointer_down(x, y, count, modifiers));
                if pressed {
                    self.dragging = Drag::ItemPointer(group);
                    consumed = true;
                } else {
                    let now = Instant::now();
                    let double = self
                        .last_click
                        .map(|(t, px, py)| {
                            now.duration_since(t) < Duration::from_millis(400)
                                && (x - px).abs() < 4.0
                                && (y - py).abs() < 4.0
                        })
                        .unwrap_or(false);
                    self.last_click = Some((now, x, y));
                    consumed = if double {
                        self.input(group)
                            .is_some_and(|input| input.editor_double_click(x, y))
                    } else {
                        // Shift alone extends the selection to the click, as the reference does.
                        let extend =
                            modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.cmd;
                        let placed = self
                            .input(group)
                            .is_some_and(|input| input.editor_click(x, y, extend));
                        if placed {
                            self.dragging = Drag::EditorSel(group);
                        }
                        placed
                    };
                }
            }
            if !consumed && self.layout.session_menu {
                self.layout.session_menu = false;
                self.session_search_query.clear();
            }
        }
    }

    pub fn editor_copy_to_clipboard(&mut self) -> bool {
        if let Some(modal) = self.window_modal.as_ref() {
            if let Some(text) = modal.copy() {
                Self::clip_set(&text);
            }
            return true;
        }
        let copied = self
            .input_ref(self.focused_group())
            .and_then(|v| v.editor_copy());
        match copied {
            Some(copied) => {
                Self::clip_set(&copied.text);
                crate::remember_copy(&copied);
                true
            }
            None => false,
        }
    }

    pub fn editor_cut_to_clipboard(&mut self) -> bool {
        if let Some(modal) = self.window_modal.as_mut() {
            let cut = modal.cut();
            if let Some(text) = &cut {
                Self::clip_set(text);
            }
            return cut.is_some();
        }
        let copied = self
            .input(self.focused_group())
            .and_then(|v| v.editor_cut());
        match copied {
            Some(copied) => {
                Self::clip_set(&copied.text);
                crate::remember_copy(&copied);
                true
            }
            None => false,
        }
    }

    pub fn editor_paste_from_clipboard(&mut self) -> bool {
        let Some(text) = Self::clip_get().filter(|t| !t.is_empty()) else {
            return false;
        };
        if let Some(modal) = self.window_modal.as_mut() {
            return modal.paste(&text);
        }
        if let Some(panel) = self.panel_with_text() {
            return panel.text(&text);
        }
        let slices = crate::slices_for(&text);
        let group = self.text_group();
        self.input(group)
            .map(|v| v.editor_paste(&text, slices.as_deref()))
            .unwrap_or(false)
    }

    pub fn editor_ime_preedit(
        &mut self,
        text: &str,
        selected: Option<std::ops::Range<usize>>,
    ) -> bool {
        if self.window_modal.is_some() {
            return false;
        }
        let group = self.text_group();
        self.input(group)
            .map(|v| v.editor_ime_preedit(text, selected))
            .unwrap_or(false)
    }

    pub fn editor_ime_commit(&mut self, text: &str) -> bool {
        if let Some(modal) = self.window_modal.as_mut() {
            return modal.text(text);
        }
        if !self.editor_modal_open() {
            if let Some(panel) = self.panel_with_text() {
                return panel.text(text);
            }
        }
        let group = self.text_group();
        self.input(group)
            .map(|v| v.editor_ime_commit(text))
            .unwrap_or(false)
    }

    pub fn editor_text(&mut self, text: &str) -> bool {
        if self.prompt_shown.is_some() {
            return false;
        }
        if self.menu.is_some() {
            return self.menu_shortcut(&text.trim().to_uppercase());
        }
        if let Some(modal) = self.window_modal.as_mut() {
            return modal.text(text);
        }
        if !self.editor_modal_open() {
            if let Some(panel) = self.panel_with_text() {
                return panel.text(text);
            }
        }
        let group = self.text_group();
        self.input(group)
            .map(|v| v.editor_text(text))
            .unwrap_or(false)
    }

    fn panel_text_kind(&self) -> Option<crate::PaneKind> {
        if self.window_modal.is_some() {
            return None;
        }
        [
            DockPosition::Left,
            DockPosition::Right,
            DockPosition::Bottom,
        ]
        .into_iter()
        .find_map(|side| match self.layout.shown_on(side) {
            Some(Shown::Func(kind)) if self.layout.dock_open(side) => self
                .layout
                .side_panels
                .iter()
                .find(|panel| panel.kind() == kind && panel.text_focused())
                .map(|panel| panel.kind()),
            _ => None,
        })
    }

    fn panel_with_text(&mut self) -> Option<&mut Box<dyn crate::SidePanelView>> {
        let kind = self.panel_text_kind()?;
        self.layout.side_panel_mut(kind)
    }

    /// The editor area shows a palette, finder or other popup that must get the keyboard over any panel.
    fn editor_modal_open(&self) -> bool {
        self.layout
            .files_view
            .as_ref()
            .is_some_and(|view| !view.accepts_pane_keys())
    }

    /// Where typed text goes: an open modal of the editor area takes it even while the panel has focus.
    fn text_group(&self) -> InputGroup {
        if self.editor_modal_open() {
            InputGroup::Center
        } else {
            self.focused_group()
        }
    }

    pub fn editor_key(&mut self, key: EditKey, shift: bool) -> bool {
        if self.prompt_shown.is_some() {
            return self.prompt_key(key, shift);
        }
        if self.usage_popover.is_some() && key == EditKey::Escape {
            self.usage_popover = None;
            return true;
        }
        if self.agent_popover.is_some() && key == EditKey::Escape {
            self.agent_popover = None;
            return true;
        }
        if self.menu.is_some() {
            match key {
                EditKey::Enter if self.menu_shortcut("Enter") => return true,
                EditKey::Left if self.menu_shortcut("Left") => return true,
                EditKey::Right if self.menu_shortcut("Right") => return true,
                EditKey::Escape => {
                    self.close_menu();
                    return true;
                }
                _ => {}
            }
        }
        if key == EditKey::ToggleCommandPalette && self.window_modal.is_none() {
            let commands = self.palette_commands();
            self.set_terminal_focus(false);
            self.set_agent_focus(false);
            for panel in self.layout.side_panels.iter_mut() {
                panel.blur();
            }
            return self.layout.files_view.as_mut().is_some_and(|files| {
                files.set_extra_commands(commands);
                files.editor_key(key, shift)
            });
        }
        if let Some(modal) = self.window_modal.as_mut() {
            let changed = modal.key(key, shift);
            self.settle_window_modal();
            return changed || self.window_modal.is_none();
        }
        if !self.editor_modal_open() {
            if let Some(panel) = self.panel_with_text() {
                let changed = panel.key(key, shift);
                self.apply_panel_requests();
                return changed;
            }
        }
        let claimed = self
            .layout
            .files_view
            .as_ref()
            .is_some_and(|view| view.claims_key(key));
        let group = if claimed {
            InputGroup::Center
        } else {
            self.focused_group()
        };
        let changed = self
            .input(group)
            .map(|v| v.editor_key(key, shift))
            .unwrap_or(false);
        self.show_view_toast();
        changed
    }

    pub fn editor_save(&mut self) -> Option<Result<(), String>> {
        if self.window_modal.is_some() {
            return None;
        }
        self.input(self.focused_group())
            .and_then(|v| v.editor_save())
    }

    pub fn refresh_disk_state(&mut self) {
        if let Some(v) = self.layout.files_view.as_mut() {
            v.refresh_disk_state();
        }
        if let Some(view) = self.layout.terminal_view.as_mut() {
            view.panes().refresh_disk_state();
        }
    }

    /// Bring the item `id` forward in the editor area, or add the one `open` makes.
    pub fn reveal_center_item(
        &mut self,
        id: &str,
        open: impl FnOnce() -> Option<Box<dyn crate::Item>>,
    ) {
        let Some(files) = self.layout.files_view.as_mut() else {
            return;
        };
        let revealed = files
            .pane_group_mut()
            .is_some_and(|group| group.reveal_item(id));
        if !revealed {
            if let Some(item) = open() {
                files.add_center_item(item);
            }
        }
        self.set_terminal_focus(false);
        self.panes_input = true;
    }

    pub fn open_file_finder(&mut self) {
        if self.window_modal.is_some() {
            return;
        }
        self.set_terminal_focus(false);
        if let Some(files) = self.layout.files_view.as_mut() {
            files.editor_key(EditKey::ToggleFileFinder, false);
        }
    }

    /// Open the project diagnostics tab; `from_status` is the status bar's click, which shows warnings when
    /// there are no errors.
    pub fn deploy_project_diagnostics(&mut self, from_status: bool) {
        if self.window_modal.is_some() {
            return;
        }
        self.set_terminal_focus(false);
        if let Some(files) = self.layout.files_view.as_mut() {
            let key = if from_status {
                EditKey::DeployDiagnosticsFromStatus
            } else {
                EditKey::DeployDiagnostics
            };
            files.editor_key(key, false);
        }
    }

    pub fn deploy_project_search(&mut self) {
        if self.window_modal.is_some() {
            return;
        }
        self.set_terminal_focus(false);
        if let Some(files) = self.layout.files_view.as_mut() {
            files.editor_key(EditKey::DeployProjectSearch, false);
        }
    }

    pub fn editor_focused(&self) -> bool {
        if self.prompt_shown.is_some()
            || self.window_modal.is_some()
            || self.panel_text_kind().is_some()
            || self.editor_modal_open()
        {
            return true;
        }
        self.input_ref(self.focused_group())
            .is_some_and(|input| input.editor_focused())
    }

    pub fn editor_selected_text(&self) -> Option<String> {
        self.input_ref(self.focused_group())
            .and_then(|v| v.editor_selected_text())
    }

    pub fn mouse_up(&mut self) {
        self.titlebar_press = false;
        if let Drag::EditorSel(group) = self.dragging {
            if let Some(input) = self.input(group) {
                input.editor_release();
            }
        }
        if let Drag::ItemPointer(group) = self.dragging {
            let (x, y) = self.pointer;
            if let Some(input) = self.input(group) {
                input.item_pointer_up(x, y, terminal_modifiers());
            }
            let request = match group {
                InputGroup::Center => self
                    .layout
                    .files_view
                    .as_mut()
                    .and_then(|view| view.take_item_open_request()),
                InputGroup::Panel => self
                    .layout
                    .terminal_view
                    .as_mut()
                    .and_then(|view| view.take_open_request()),
                InputGroup::Agent => self
                    .layout
                    .agent_view
                    .as_mut()
                    .and_then(|view| view.take_open_request()),
            };
            if let Some(request) = request {
                self.open_terminal_target(request);
            }
        } else if self.dragging == Drag::WorkspaceRow {
            if let Some(RowDrag {
                from, to: Some(to), ..
            }) = self.row_drag
            {
                self.workspace_requests.reorder = Some((from, to));
            }
        } else if self.dragging == Drag::WorkspaceGroup {
            if let Some(GroupDrag {
                from, to: Some(to), ..
            }) = self.group_drag
            {
                self.move_group(from, to);
            }
        } else if let Some(drag) = self.group_drag.filter(|_| self.dragging == Drag::None) {
            self.toggle_group_fold(drag.from);
        } else if self.dragging == Drag::Tab {
            self.finish_center_tab_drag();
        } else if self.dragging == Drag::AgentTab {
            self.finish_agent_tab_drag();
        } else if self.dragging == Drag::TerminalTab {
            self.finish_panel_tab_drag();
        } else if self.dragging != Drag::None {
            if self.dragging == Drag::Left {
                self.layout.finish_left_drag();
            }
            self.pending.persist = true;
        } else if let Some((id, _, _)) = self.pending_tab.take() {
            if crate::is_terminal_id(id) || crate::is_agent_id(id) {
                self.header_click(id);
            } else if let Some(v) = self.layout.files_view.as_mut() {
                v.on_click(id);
            }
        }
        self.pending_tab = None;
        self.row_drag = None;
        self.group_drag = None;
        self.tab_ghost_at = None;
        self.dragging = Drag::None;
    }

    /// Like a tab drop: the target row (or rail cell) is tinted, with a bar on the side the dragged one lands.
    fn row_drop_painted(&self, hits: &[(Rect, u64)]) -> Option<Painted> {
        let drag = self
            .row_drag
            .filter(|_| self.dragging == Drag::WorkspaceRow)?;
        let to = drag.to?;
        let (rect, _) = hits
            .iter()
            .find(|(_, id)| *id == crate::WORKSPACE_ROW_BASE + to as u64)?;
        let mut drop = Painted::default();
        drop.rects.push(Rect::new(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            ui::theme().text_accent.alpha(0.22),
        ));
        let bar_y = if to < drag.from {
            rect.y
        } else {
            rect.y + rect.h - 2.0
        };
        drop.rects.push(Rect::new(
            rect.x,
            bar_y,
            rect.w,
            2.0,
            ui::theme().border_focused,
        ));
        Some(drop)
    }

    /// The row under the pointer becomes the drop target; main (the first row) never moves.
    fn update_row_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(mut drag) = self.row_drag else {
            return false;
        };
        let to = self
            .header_hits
            .iter()
            .filter(|(rect, id)| {
                (crate::WORKSPACE_ROW_BASE..crate::WORKSPACE_ROW_END).contains(id)
                    && x >= rect.x
                    && x < rect.x + rect.w
                    && y >= rect.y
                    && y < rect.y + rect.h
            })
            .map(|(_, id)| (id - crate::WORKSPACE_ROW_BASE) as usize)
            .next_back()
            .filter(|index| *index > 0 && *index != drag.from)
            // Grouped, a row moves only among its own group; its place in the shared order follows.
            .filter(|index| {
                self.grouping.is_none() || self.row_group(*index) == self.row_group(drag.from)
            });
        drag.to = to;
        drag.pointer = (x, y);
        self.row_drag = Some(drag);
        true
    }

    /// The terminal panel in `region`: a backdrop to blit, with its panes pushed onto `overlays`. In its dock
    /// (`in_dock`) while the panel is zoomed, only the dock's background shows there.
    fn terminal_painted(
        &mut self,
        region: Rect,
        in_dock: bool,
        overlays: &mut Vec<Overlay>,
        hits: &mut Vec<(Rect, u64)>,
    ) -> Painted {
        let focused = self.terminal_focused;
        let zoomed = self
            .zoom
            .is_some_and(|zoom| zoom.group == InputGroup::Panel);
        if in_dock && zoomed {
            return ui::render(&ui::div().bg(ui::theme().panel_background).into(), region);
        }
        match self.layout.terminal_view.as_mut() {
            Some(view) => {
                if view.is_empty() {
                    view.open(None);
                }
                let (backdrop, mut panes) = view.render(region, focused);
                push_pane_group(&mut panes, region, overlays, hits);
                backdrop
            }
            None => ui::render(&terminal_dock_body(), region),
        }
    }

    fn agent_painted(
        &mut self,
        region: Rect,
        in_dock: bool,
        overlays: &mut Vec<Overlay>,
        hits: &mut Vec<(Rect, u64)>,
    ) -> Painted {
        let zoomed = self
            .zoom
            .is_some_and(|zoom| zoom.group == InputGroup::Agent);
        if in_dock && zoomed {
            return ui::render(&ui::div().bg(ui::theme().panel_background).into(), region);
        }
        let focused = self.agent_focused;
        match self.layout.agent_view.as_mut() {
            Some(view) => {
                let (backdrop, mut panes) = view.render(region, focused);
                push_pane_group(&mut panes, region, overlays, hits);
                backdrop
            }
            None => Painted::default(),
        }
    }

    /// Bring every shell's output in before drawing; closes the panel once its last shell exits.
    fn sync_terminals(&mut self) {
        let items = self
            .layout
            .files_view
            .as_mut()
            .map(|v| v.tick_items(&Self::clip_get));
        if let Some(text) = items.and_then(|tick| tick.clipboard_store) {
            Self::clip_set(&text);
        }
        if let Some(agent) = self.layout.agent_view.as_mut() {
            let outcome = agent.sync(&Self::clip_get);
            if let Some(text) = outcome.clipboard_store {
                Self::clip_set(&text);
            }
            let delivered: Vec<bool> = outcome
                .send
                .iter()
                .map(|(target, text)| agent.paste_into(target, text))
                .collect();
            if delivered.contains(&true) {
                self.focus_group(InputGroup::Agent);
            }
            if delivered.contains(&false) {
                self.show_toast("The main agent is not open; start it first", None);
            }
            if outcome.closed_all {
                self.set_agent_focus(false);
            }
        }
        let Some(view) = self.layout.terminal_view.as_mut() else {
            return;
        };
        let outcome = view.sync(&Self::clip_get);
        if let Some(text) = outcome.clipboard_store {
            Self::clip_set(&text);
        }
        if outcome.closed_all {
            let side = self.layout.terminal_side;
            if self.layout.terminal_visible() {
                self.toggle_side(side, true);
            }
            self.set_terminal_focus(false);
        }
    }

    /// Over the body (below the chrome) of a pane in the visible terminal panel.
    fn panel_body_at(&self, x: f32, y: f32) -> bool {
        self.layout.terminal_visible()
            && self.layout.terminal_view.as_ref().is_some_and(|view| {
                view.panes_ref()
                    .body_point(x, y)
                    .and_then(|(path, _, _)| view.panes_ref().pane_at(&path))
                    .is_some_and(|pane| !pane.open.is_empty())
            })
    }

    /// The focused group's zoom, if it shows, and the rect it covers: the editor area inset on every side for a
    /// pane, or leaving a strip on the dock's inner side for the terminal panel.
    fn shown_zoom(&self, w: f32, h: f32) -> Option<Zoom> {
        let group = self.focused_group();
        let shown = match group {
            InputGroup::Center => self
                .layout
                .files_view
                .as_ref()
                .is_some_and(|view| view.zoom_shown()),
            InputGroup::Panel => {
                self.layout.terminal_visible()
                    && self
                        .layout
                        .terminal_view
                        .as_ref()
                        .is_some_and(|view| view.panes_ref().zoom_shown())
            }
            InputGroup::Agent => {
                self.layout.agent_visible()
                    && self
                        .layout
                        .agent_view
                        .as_ref()
                        .is_some_and(|view| view.panes_ref().zoom_shown())
            }
        };
        if !shown {
            return None;
        }
        let area = self.layout.editor_area(w, h);
        let (mut x, mut y, mut rw, mut rh) = (area.x, area.y, area.w, area.h);
        let side = if group == InputGroup::Agent {
            self.layout.agent_side
        } else {
            self.layout.terminal_side
        };
        let (top, right, bottom, left) = match group {
            InputGroup::Center => (true, true, true, true),
            InputGroup::Panel | InputGroup::Agent => (
                side == DockPosition::Bottom,
                side == DockPosition::Left,
                false,
                side == DockPosition::Right,
            ),
        };
        if top {
            y += ZOOM_PADDING;
            rh -= ZOOM_PADDING;
        }
        if bottom {
            rh -= ZOOM_PADDING;
        }
        if left {
            x += ZOOM_PADDING;
            rw -= ZOOM_PADDING;
        }
        if right {
            rw -= ZOOM_PADDING;
        }
        let outer = Rect::new(x, y, rw.max(0.0), rh.max(0.0), Rgba::TRANSPARENT);
        // The border sits on the sides that face the rest of the workspace; content stays inside it.
        let px = |on: bool| if on { 1.0 } else { 0.0 };
        let inner = Rect::new(
            outer.x + px(left),
            outer.y + px(top),
            (outer.w - px(left) - px(right)).max(0.0),
            (outer.h - px(top) - px(bottom)).max(0.0),
            Rgba::TRANSPARENT,
        );
        Some(Zoom {
            group,
            outer,
            inner,
            sides: [top, right, bottom, left],
        })
    }

    fn zoom_group_at(&self, x: f32, y: f32) -> Option<InputGroup> {
        let zoom = self.zoom?;
        let rect = zoom.outer;
        (x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h)
            .then_some(zoom.group)
    }

    fn input(&mut self, group: InputGroup) -> Option<&mut dyn crate::ItemInput> {
        match group {
            InputGroup::Center => self
                .layout
                .files_view
                .as_mut()
                .map(|view| &mut **view as &mut dyn crate::ItemInput),
            InputGroup::Panel => self
                .layout
                .terminal_view
                .as_mut()
                .map(|view| view.panes() as &mut dyn crate::ItemInput),
            InputGroup::Agent => self
                .layout
                .agent_view
                .as_mut()
                .map(|view| view.panes() as &mut dyn crate::ItemInput),
        }
    }

    fn input_ref(&self, group: InputGroup) -> Option<&dyn crate::ItemInput> {
        match group {
            InputGroup::Center => self
                .layout
                .files_view
                .as_ref()
                .map(|view| &**view as &dyn crate::ItemInput),
            InputGroup::Panel => self
                .layout
                .terminal_view
                .as_ref()
                .map(|view| view.panes_ref() as &dyn crate::ItemInput),
            InputGroup::Agent => self
                .layout
                .agent_view
                .as_ref()
                .map(|view| view.panes_ref() as &dyn crate::ItemInput),
        }
    }

    /// The group whose items take keyboard input: the terminal panel's while it has focus, else the editor's.
    fn focused_group(&self) -> InputGroup {
        if self.agent_focused && self.layout.agent_visible() {
            InputGroup::Agent
        } else if self.panel_has_focus() {
            InputGroup::Panel
        } else {
            InputGroup::Center
        }
    }

    fn popover_group_at(&self, x: f32, y: f32) -> Option<InputGroup> {
        let at = self
            .popover_rects
            .iter()
            .position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)?;
        self.popover_groups.get(at).map(|(group, _)| *group)
    }

    fn visible_groups(&self) -> Vec<InputGroup> {
        let mut groups = vec![InputGroup::Center];
        if self.layout.terminal_visible() {
            groups.push(InputGroup::Panel);
        }
        if self.layout.agent_visible() {
            groups.push(InputGroup::Agent);
        }
        groups
    }

    fn set_terminal_focus(&mut self, focused: bool) {
        if focused {
            self.set_agent_focus(false);
        }
        if self.terminal_focused == focused {
            return;
        }
        self.terminal_focused = focused;
        if let Some(view) = self.layout.terminal_view.as_mut() {
            view.focus_changed(focused);
        }
    }

    fn set_agent_focus(&mut self, focused: bool) {
        if self.agent_focused == focused {
            return;
        }
        self.agent_focused = focused;
        if let Some(view) = self.layout.agent_view.as_mut() {
            view.focus_changed(focused);
        }
    }

    /// Keyboard focus to `group`'s items.
    fn focus_group(&mut self, group: InputGroup) {
        match group {
            InputGroup::Center => {
                self.set_terminal_focus(false);
                self.set_agent_focus(false);
            }
            InputGroup::Panel => self.set_terminal_focus(true),
            InputGroup::Agent => {
                self.set_terminal_focus(false);
                self.set_agent_focus(true);
            }
        }
    }

    /// Over the body of a pane in a visible dock group (the terminal panel or the agent dock).
    fn dock_body_group_at(&self, x: f32, y: f32) -> Option<InputGroup> {
        if self.panel_body_at(x, y) {
            return Some(InputGroup::Panel);
        }
        let over_agent = self.layout.agent_visible()
            && self.layout.agent_view.as_ref().is_some_and(|view| {
                view.panes_ref()
                    .body_point(x, y)
                    .and_then(|(path, _, _)| view.panes_ref().pane_at(&path))
                    .is_some_and(|pane| !pane.open.is_empty())
            });
        over_agent.then_some(InputGroup::Agent)
    }

    /// Whether key presses go raw to a terminal: the focused group's active item takes raw keystrokes.
    pub fn terminal_focused(&self) -> bool {
        if self.prompt_shown.is_some()
            || self.window_modal.is_some()
            || self.panel_text_kind().is_some()
        {
            return false;
        }
        if self.layout.files_view.is_none() && self.page.is_some() {
            return self.focused_group() == InputGroup::Center;
        }
        self.input_ref(self.focused_group())
            .is_some_and(|input| input.active_wants_keystrokes())
    }

    /// Presses within 400ms and 4px of the last one count up to a triple click (then wrap to single).
    fn click_count(&mut self, x: f32, y: f32) -> u32 {
        let now = Instant::now();
        let count = match self.terminal_click {
            Some((at, px, py, count))
                if now.duration_since(at) < Duration::from_millis(400)
                    && (x - px).abs() < 4.0
                    && (y - py).abs() < 4.0 =>
            {
                count % 3 + 1
            }
            _ => 1,
        };
        self.terminal_click = Some((now, x, y, count));
        count
    }

    /// Run a pane key on the focused pane group. Moving focus past the group's edge crosses between the center
    /// and the terminal panel when the panel sits on that side. Returns false when the key should fall through.
    pub fn pane_command(&mut self, command: crate::pane::PaneCommand) -> bool {
        if self.window_modal.is_some() {
            return false;
        }
        use crate::pane::PaneCommand;
        use crate::pane_group::SplitDirection;
        let toward_panel = match self.layout.terminal_side {
            DockPosition::Left => SplitDirection::Left,
            DockPosition::Right => SplitDirection::Right,
            DockPosition::Bottom => SplitDirection::Down,
        };
        if self.focused_group() == InputGroup::Agent {
            return self
                .layout
                .agent_view
                .as_mut()
                .is_some_and(|panel| panel.pane_command(command));
        }
        if self.panel_has_focus() {
            let handled = self
                .layout
                .terminal_view
                .as_mut()
                .is_some_and(|panel| panel.pane_command(command));
            if !handled && command == PaneCommand::ActivatePane(toward_panel.opposite()) {
                self.set_terminal_focus(false);
                return true;
            }
            return handled;
        }
        let Some(files) = self.layout.files_view.as_mut() else {
            return false;
        };
        if !files.accepts_pane_keys() {
            return false;
        }
        let handled = files.pane_command(command);
        if !handled
            && command == PaneCommand::ActivatePane(toward_panel)
            && self.layout.terminal_visible()
        {
            self.set_terminal_focus(true);
            return true;
        }
        handled
    }

    fn panel_has_focus(&self) -> bool {
        self.terminal_focused && self.layout.terminal_visible()
    }

    /// Returns whether the key was consumed; unconsumed keys arrive next as typed text.
    pub fn terminal_key(&mut self, keystroke: &terminal::Keystroke) -> bool {
        let group = self.focused_group();
        if group == InputGroup::Center {
            if let Some(page) = self.standalone_page() {
                return match page.keystroke(keystroke) {
                    crate::TerminalKeyOutcome::Ignored => false,
                    crate::TerminalKeyOutcome::Handled => true,
                    crate::TerminalKeyOutcome::Copy(text) => {
                        Self::clip_set(&text);
                        true
                    }
                    crate::TerminalKeyOutcome::Paste => {
                        if let Some(text) = Self::clip_get() {
                            if let Some(page) = self.standalone_page() {
                                page.paste(&text, None);
                            }
                        }
                        true
                    }
                };
            }
        }
        let Some(input) = self.input(group) else {
            return false;
        };
        match input.item_keystroke(keystroke) {
            crate::TerminalKeyOutcome::Ignored => false,
            crate::TerminalKeyOutcome::Handled => true,
            crate::TerminalKeyOutcome::Copy(text) => {
                Self::clip_set(&text);
                true
            }
            crate::TerminalKeyOutcome::Paste => {
                if let Some(text) = Self::clip_get() {
                    input.item_paste(&text);
                }
                true
            }
        }
    }

    pub fn terminal_text(&mut self, text: &str) {
        let group = self.focused_group();
        if group == InputGroup::Center {
            if let Some(page) = self.standalone_page() {
                page.input_text(text);
                return;
            }
        }
        if let Some(input) = self.input(group) {
            input.item_text(text);
        }
    }

    fn new_center_terminal(&mut self) {
        let Some(item) = self
            .layout
            .terminal_view
            .as_mut()
            .and_then(|view| view.new_item(None))
        else {
            return;
        };
        self.set_terminal_focus(false);
        if let Some(view) = self.layout.files_view.as_mut() {
            view.add_center_item(item);
        }
    }

    fn open_terminal_target(&mut self, target: crate::TerminalOpenTarget) {
        match target {
            crate::TerminalOpenTarget::Url(url) => {
                if let Err(error) = std::process::Command::new("open").arg(&url).spawn() {
                    self.show_toast(format!("Failed to open {url}: {error}"), None);
                }
            }
            crate::TerminalOpenTarget::Path { path, row, column } => {
                if let Some(view) = self.layout.files_view.as_mut() {
                    view.open_file_at(&path, row, column);
                    self.set_terminal_focus(false);
                }
            }
        }
    }

    /// Over the terminal grid: `Some(true)` on a link a click would open, `Some(false)` elsewhere (text).
    pub fn terminal_pointer_at(&self, x: f32, y: f32) -> Option<bool> {
        if self
            .layout
            .files_view
            .as_ref()
            .is_some_and(|v| v.item_link_hovered())
        {
            return Some(true);
        }
        match self.dock_body_group_at(x, y)? {
            InputGroup::Agent => self
                .layout
                .agent_view
                .as_ref()
                .map(|view| view.link_hovered()),
            _ => self
                .layout
                .terminal_view
                .as_ref()
                .map(|view| view.link_hovered()),
        }
    }

    /// Make the file tree the visible panel of its dock.
    fn show_files_tree(&mut self) {
        let side = self
            .layout
            .func_side
            .first()
            .copied()
            .unwrap_or(DockPosition::Left);
        self.layout.active_panels[side.index()] = Some(Shown::Func(PaneKind::Files));
        match side {
            DockPosition::Right => self.layout.right.collapsed = false,
            DockPosition::Bottom => self.layout.bottom.collapsed = false,
            DockPosition::Left => self.layout.panels_collapsed = false,
        }
        self.pending.persist = true;
    }

    fn show_terminal(&mut self) {
        let side = self.layout.terminal_side;
        self.layout.active_panels[side.index()] = Some(Shown::Terminal);
        match side {
            DockPosition::Right => self.layout.right.collapsed = false,
            DockPosition::Bottom => self.layout.bottom.collapsed = false,
            DockPosition::Left => self.layout.panels_collapsed = false,
        }
        self.pending.persist = true;
    }

    /// Focus the terminal tab whose item has `id`, or add the item `make` builds; shows the terminal.
    pub fn open_terminal_item(
        &mut self,
        id: &str,
        make: impl FnOnce() -> Option<Box<dyn crate::Item>>,
    ) {
        let Some(view) = self.layout.terminal_view.as_mut() else {
            return;
        };
        if !view.panes().reveal_item(id) {
            let Some(item) = make() else {
                return;
            };
            view.accept_foreign_item(item);
        }
        self.show_terminal();
        self.set_terminal_focus(true);
        self.panes_input = true;
    }

    /// A new shell in `dir`, in the terminal panel.
    pub fn open_terminal_in(&mut self, dir: std::path::PathBuf) {
        let Some(view) = self.layout.terminal_view.as_mut() else {
            return;
        };
        view.open(Some(dir));
        self.show_terminal();
        self.set_terminal_focus(true);
        self.panes_input = true;
    }

    /// Focus the agent session whose item has `id`, or add the one `make` builds; shows it in the agent dock.
    /// Shows the agent `id` wherever it already is, else opens it: next to the code when `in_center`,
    /// otherwise in the agent dock.
    pub fn open_agent_item(
        &mut self,
        id: &str,
        in_center: bool,
        make: impl FnOnce() -> Option<Box<dyn crate::Item>>,
    ) {
        // Moved next to the code, the agent is shown there rather than opened a second time in the dock.
        let shown_in_center = self
            .layout
            .files_view
            .as_mut()
            .and_then(|files| files.pane_group_mut())
            .is_some_and(|group| group.reveal_item(id));
        if shown_in_center {
            self.focus_center_agent();
            return;
        }
        let in_dock = self
            .layout
            .agent_view
            .as_mut()
            .is_some_and(|view| view.panes().reveal_item(id));
        if in_center && !in_dock {
            if let Some(group) = self
                .layout
                .files_view
                .as_mut()
                .and_then(|files| files.pane_group_mut())
            {
                let Some(item) = make() else {
                    return;
                };
                group.accept_foreign_item(item);
                self.focus_center_agent();
                self.pending.persist = true;
                return;
            }
        }
        let Some(view) = self.layout.agent_view.as_mut() else {
            return;
        };
        if !in_dock {
            let Some(item) = make() else {
                return;
            };
            view.accept_foreign_item(item);
        }
        self.layout.agent_hidden = false;
        self.show_agent_panel();
        self.focus_group(InputGroup::Agent);
        self.panes_input = true;
        self.pending.persist = true;
    }

    fn focus_center_agent(&mut self) {
        self.set_terminal_focus(false);
        self.set_agent_focus(false);
        self.focus_group(InputGroup::Center);
        self.panes_input = true;
    }

    /// Whether the agent dock is showing its sessions right now.
    pub fn agent_dock_visible(&self) -> bool {
        self.layout.agent_visible()
    }

    pub fn hide_agent_dock(&mut self) {
        if !self.layout.agent_visible() {
            return;
        }
        match self.layout.agent_side {
            DockPosition::Right => self.layout.right.collapsed = true,
            side => self.layout.active_panels[side.index()] = None,
        }
        self.set_agent_focus(false);
        self.pending.persist = true;
    }

    /// Whether an agent tab `id` is open (in the dock or moved next to the code).
    pub fn agent_item_open(&mut self, id: &str) -> bool {
        let in_dock = self
            .layout
            .agent_view
            .as_mut()
            .is_some_and(|view| view.panes().index_path_of(id).is_some());
        in_dock
            || self
                .layout
                .files_view
                .as_mut()
                .and_then(|files| files.pane_group_mut())
                .is_some_and(|group| group.has_item(id))
    }

    /// Sends `text` to the agent tab `id` as a message; false when it is not open.
    pub fn send_to_agent(&mut self, id: &str, text: &str) -> bool {
        self.layout
            .agent_view
            .as_mut()
            .is_some_and(|view| view.submit_into(id, text))
    }

    pub fn close_agent_item(&mut self, id: &str) {
        if let Some(view) = self.layout.agent_view.as_mut() {
            view.panes().close_item(id);
        }
        if let Some(group) = self
            .layout
            .files_view
            .as_mut()
            .and_then(|files| files.pane_group_mut())
        {
            group.close_item(id);
        }
    }

    /// The terminal toggle: focus the terminal (showing it first), or hide it when it already has focus.
    pub fn toggle_terminal(&mut self) {
        if self.terminal_focused() {
            let side = self.layout.terminal_side;
            self.toggle_side(side, true);
            self.set_terminal_focus(false);
            self.pending.persist = true;
        } else {
            self.show_terminal();
            self.set_terminal_focus(true);
        }
    }

    fn open_terminal_at(&mut self, directory: Option<std::path::PathBuf>) {
        self.show_terminal();
        if let Some(view) = self.layout.terminal_view.as_mut() {
            view.open(directory);
        }
        self.set_terminal_focus(true);
    }

    /// Puts a confirmation up; its answer comes back through `prompt_answered`.
    pub fn ask(&mut self, prompt: crate::Prompt) {
        self.pending_prompt = Some(prompt);
    }

    pub fn take_prompt(&mut self) -> Option<crate::Prompt> {
        self.pending_prompt.take()
    }

    /// A panel's request to start the coding agent on a problem it found.
    pub fn take_agent_fix(&mut self) -> Option<(crate::AgentFix, crate::SideAgentRole)> {
        self.agent_fix.take()
    }

    pub fn prompt_answered(&mut self, token: u64, answer: usize) {
        if token >= PREPARE_MAIN_PROMPT_TOKEN {
            if answer == 0 {
                let index = (token - PREPARE_MAIN_PROMPT_TOKEN) as usize;
                self.workspace_requests.row = Some((index, crate::RowAction::PrepareMain));
            }
            return;
        }
        if token >= DELETE_WORKSPACE_PROMPT_TOKENS {
            if answer == 0 {
                let index = (token - DELETE_WORKSPACE_PROMPT_TOKENS) as usize;
                self.workspace_requests.row = Some((index, crate::RowAction::Delete));
            }
            return;
        }
        if token >= PANEL_PROMPT_TOKENS {
            if let Some((asked, kind, tag)) = self.panel_prompt.take() {
                if asked == token {
                    if let Some(panel) = self.layout.side_panel_mut(kind) {
                        panel.prompt_answered(tag, answer);
                    }
                    self.apply_panel_requests();
                }
            }
            return;
        }
        if token >= FORGET_PROMPT_TOKENS {
            if answer == 0 {
                let index = (token - FORGET_PROMPT_TOKENS) as usize;
                self.pending.session = Some(SessionRequest::Forget(index));
            }
            return;
        }
        if let Some((pending, group, request)) = self.close_prompt.take() {
            if pending == token {
                // Save, Don't Save, Cancel (or Save all, Discard all, Cancel).
                if answer <= 1 {
                    let errors = self
                        .group_view_mut(group)
                        .map(|panes| panes.finish_close(request, answer == 0))
                        .unwrap_or_default();
                    if let Some(error) = errors.into_iter().next() {
                        self.show_toast(error, None);
                    }
                }
                return;
            }
            self.close_prompt = Some((pending, group, request));
        }
        if let Some(v) = self.layout.files_view.as_mut() {
            v.prompt_answered(token, answer);
        }
        self.show_view_toast();
    }

    /// Ask whether to save the dirty tabs a close left pending, after the reference: one file names it, several
    /// list their names. Tabs also open in the other pane group close without asking.
    fn ask_about_pending_close(&mut self) {
        for group in [InputGroup::Center, InputGroup::Panel, InputGroup::Agent] {
            let Some(mut request) = self
                .group_view_mut(group)
                .and_then(|panes| panes.take_close_request())
            else {
                continue;
            };
            let other = match group {
                InputGroup::Center => InputGroup::Panel,
                InputGroup::Panel | InputGroup::Agent => InputGroup::Center,
            };
            if let Some(panes) = self.group_view(other) {
                request.dirty.retain(|dirty| !panes.has_item(&dirty.id));
            }
            if request.dirty.is_empty() {
                if let Some(panes) = self.group_view_mut(group) {
                    panes.finish_close(request, false);
                }
                continue;
            }
            let prompt = self.close_prompt_for(&request);
            self.close_prompt = Some((prompt.token, group, request));
            self.pending_prompt = Some(prompt);
            return;
        }
    }

    fn close_prompt_for(
        &mut self,
        request: &crate::pane_group_view::CloseRequest,
    ) -> crate::Prompt {
        let token = self.next_prompt_token;
        self.next_prompt_token += 1;
        let buttons = |labels: [&str; 3]| labels.iter().map(|label| label.to_string()).collect();
        if let [dirty] = request.dirty.as_slice() {
            let path = dirty
                .path
                .as_deref()
                .and_then(|path| self.relative_to_root(path))
                .filter(|path| !path.is_empty());
            let message = match path {
                Some(path) => format!(
                    "`{}` contains unsaved edits. Do you want to save it?",
                    truncate_front(&path, 80)
                ),
                None => "This buffer contains unsaved edits. Do you want to save it?".to_string(),
            };
            return crate::Prompt {
                token,
                message,
                detail: None,
                buttons: buttons(["Save", "Don't Save", "Cancel"]),
            };
        }
        let mut names: Vec<String> = request
            .dirty
            .iter()
            .map(|dirty| {
                dirty
                    .path
                    .as_deref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "untitled".to_string())
            })
            .collect();
        names.sort();
        names.dedup();
        let detail = if names.len() > 6 {
            let shown: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
            format!("{}\n.. and {} more", shown.join("\n"), names.len() - 5)
        } else {
            names.join("\n")
        };
        crate::Prompt {
            token,
            message: "Do you want to save changes to the following files?".to_string(),
            detail: Some(detail),
            buttons: buttons(["Save all", "Discard all", "Cancel"]),
        }
    }

    fn pull_server_notices(&mut self) {
        if let Some(view) = self.layout.files_view.as_mut() {
            self.server_notices.extend(view.take_server_notices());
            let withdrawn = view.take_withdrawn_notices();
            if let Some(message) = view.take_toast() {
                self.show_toast(message, None);
            }
            if !withdrawn.is_empty() {
                self.withdraw_server_notices(&withdrawn);
            }
        }
        if self.notification.is_some() {
            return;
        }
        if let Some(notice) = self.server_notices.pop_front() {
            self.notification = Some(server_notification(notice));
        }
    }

    fn withdraw_server_notices(&mut self, tokens: &[u64]) {
        self.server_notices
            .retain(|notice| !tokens.contains(&notice.token));
        let showing = matches!(
            self.notification.as_ref().and_then(|notification| notification.action.as_ref()),
            Some(NotificationAction::Server(notice)) if tokens.contains(&notice.token)
        );
        if showing {
            self.notification = None;
        }
    }

    fn answer_server_notice(&mut self, notice: &crate::ServerNotice, action: Option<usize>) {
        if let Some(view) = self.layout.files_view.as_mut() {
            view.answer_server_notice(notice.token, action);
        }
    }

    fn show_view_toast(&mut self) {
        let message = self.layout.files_view.as_mut().and_then(|v| v.take_toast());
        if let Some(message) = message {
            self.show_toast(message, None);
        }
        let request = self
            .layout
            .files_view
            .as_mut()
            .and_then(|v| v.take_request());
        if request == Some(crate::ViewRequest::NewCenterTerminal) {
            self.new_center_terminal();
        }
    }

    pub fn dragging(&self) -> bool {
        self.dragging != Drag::None
    }

    pub fn scroll(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        if self.window_modal.is_none() && self.page_body_at(x, y) {
            let modifiers = terminal_modifiers();
            return self
                .standalone_page()
                .is_some_and(|page| page.pointer_scroll(x, y, dy, modifiers));
        }
        let over_popover = self
            .popover_rects
            .iter()
            .position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h);
        if let Some(&(group, index)) = over_popover.and_then(|at| self.popover_groups.get(at)) {
            return self
                .input(group)
                .is_some_and(|input| input.popover_scroll(index, dy));
        }
        // An open modal swallows scrolling over it so the editor underneath stays put.
        if let Some(rect) = self.modal_rect {
            if self.window_modal.is_some() {
                return false;
            }
            if x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h {
                return self
                    .layout
                    .files_view
                    .as_mut()
                    .is_some_and(|v| v.modal_scroll(dy));
            }
        }
        if let Some(group) = self.zoom_group_at(x, y) {
            let modifiers = terminal_modifiers();
            return self.input(group).is_some_and(|input| {
                input.item_pointer_scroll(x, y, (dx, dy), modifiers)
                    || input.editor_scroll(x, y, dx, dy)
            });
        }
        let sidebar = self.layout.left_region(self.viewport.0, self.viewport.1);
        let over_sidebar = x >= sidebar.x
            && x < sidebar.x + sidebar.w
            && y >= sidebar.y
            && y < sidebar.y + sidebar.h - crate::STATUS_BAR_H;
        if over_sidebar && !self.layout.session_menu {
            let next = (self.workspace_scroll - dy).clamp(0.0, self.workspace_scroll_max);
            if (next - self.workspace_scroll).abs() <= 0.01 {
                return false;
            }
            self.workspace_scroll = next;
            return true;
        }
        if let Some(group) = self
            .dock_body_group_at(x, y)
            .filter(|_| !self.layout.session_menu)
        {
            let modifiers = terminal_modifiers();
            return self.input(group).is_some_and(|input| {
                input.item_pointer_scroll(x, y, (dx, dy), modifiers)
                    || input.editor_scroll(x, y, dx, dy)
            });
        }
        if !self.layout.session_menu {
            let (w, h) = self.viewport;
            let cr = self.layout.center_region(w, h);
            let over_center = x >= cr.x && x < cr.x + cr.w && y >= cr.y && y < cr.y + cr.h;
            // Scrolling goes only to what is under the pointer: an editor at its end doesn't hand the rest of
            // the gesture to the file tree.
            if over_center {
                return self.layout.files_view.as_mut().is_some_and(|view| {
                    view.item_pointer_scroll(x, y, (dx, dy), terminal_modifiers())
                        || view.editor_scroll(x, y, dx, dy)
                });
            }
            for side in [
                DockPosition::Left,
                DockPosition::Right,
                DockPosition::Bottom,
            ] {
                let region = match side {
                    DockPosition::Left if self.layout.left_column_active() => {
                        self.layout.tree_region(w, h)
                    }
                    DockPosition::Left => continue,
                    DockPosition::Right => self.layout.right_region(w, h),
                    DockPosition::Bottom => self.layout.bottom_region(w, h),
                };
                let over = x >= region.x
                    && x < region.x + region.w
                    && y >= region.y
                    && y < region.y + region.h;
                if let (true, Some(panel)) = (over, self.layout.side_panel_on(side)) {
                    return panel.scroll(dy);
                }
            }
            if let Some(side) = self.layout.files_side() {
                let region = match side {
                    DockPosition::Left => self.layout.tree_region(w, h),
                    DockPosition::Right => self.layout.right_region(w, h),
                    DockPosition::Bottom => self.layout.bottom_region(w, h),
                };
                let over_tree = x >= region.x
                    && x < region.x + region.w
                    && y >= region.y
                    && y < region.y + region.h;
                if let (true, Some(view)) = (over_tree, self.layout.files_view.as_mut()) {
                    return view.on_scroll(dx, dy, region.w, region.h);
                }
            }
            return false;
        }
        let max = self
            .layout
            .session_menu_max_scroll(&self.session_search_query);
        let next = (self.layout.session_scroll - dy).clamp(0.0, max);
        if (next - self.layout.session_scroll).abs() <= 0.01 {
            return false;
        }
        self.layout.session_scroll = next;
        true
    }

    /// Whether the session menu is open (the shell caret-blinks only then).
    pub fn menu_open(&self) -> bool {
        self.layout.session_menu
    }

    pub fn key_escape(&mut self) -> bool {
        if self.layout.session_menu {
            self.layout.session_menu = false;
            self.session_search_query.clear();
            true
        } else {
            false
        }
    }

    pub fn key_backspace(&mut self) -> bool {
        if self.layout.session_menu {
            self.session_search_query.pop();
            self.session_menu_hover = None;
            self.layout.session_scroll = 0.0;
            true
        } else {
            false
        }
    }

    pub fn key_text(&mut self, text: &str) -> bool {
        if !self.layout.session_menu {
            return false;
        }
        let add: String = text.chars().filter(|c| !c.is_control()).collect();
        if add.is_empty() {
            return false;
        }
        self.session_search_query.push_str(&add);
        self.session_menu_hover = None;
        self.layout.session_scroll = 0.0;
        true
    }

    fn close_session_menu(&mut self) {
        self.layout.session_menu = false;
        self.session_search_query.clear();
    }

    fn ask_to_forget_session(&mut self, index: usize) {
        let Some(session) = self.layout.sessions.get(index) else {
            return;
        };
        if Some(index) == self.layout.current_session {
            return;
        }
        self.pending_prompt = Some(crate::Prompt {
            token: FORGET_PROMPT_TOKENS + index as u64,
            message: format!("Remove \"{}\" from the session list?", session.name),
            detail: Some("Files on disk are left untouched.".into()),
            buttons: vec!["Remove".into(), "Cancel".into()],
        });
    }

    fn header_click(&mut self, id: u64) {
        if id == TOAST_CLOSE || id == TOAST_ACTION {
            self.toast = None;
            let then = self.toast_then.take();
            if let (TOAST_ACTION, Some((kind, then))) = (id, then) {
                self.apply_request(kind, then);
            }
            return;
        }
        if crate::is_terminal_id(id) {
            self.set_terminal_focus(true);
            if let Some(view) = self.layout.terminal_view.as_mut() {
                view.click(id);
            }
            self.ask_about_pending_close();
            return;
        }
        if (crate::agent_popover::POPOVER_BASE..crate::agent_popover::POPOVER_END).contains(&id) {
            self.agent_popover_click(id);
            return;
        }
        // A tab's own toolbar: its ids sit above every range, so ask the docks' tabs before the editor's.
        if id >= crate::ITEM_TOOLBAR_IDS {
            if self
                .layout
                .agent_view
                .as_mut()
                .is_some_and(|view| view.click(id))
            {
                self.focus_group(InputGroup::Agent);
                return;
            }
            if self
                .layout
                .terminal_view
                .as_mut()
                .is_some_and(|view| view.click(id))
            {
                return;
            }
        }
        if crate::is_agent_id(id) {
            self.focus_group(InputGroup::Agent);
            let (wants_new, wants_history) =
                self.layout
                    .agent_view
                    .as_mut()
                    .map_or((false, false), |view| {
                        view.click(id);
                        (view.take_new_agent_request(), view.take_history_request())
                    });
            if wants_new || wants_history {
                self.toggle_agent_popover(if wants_new {
                    crate::agent_popover::AgentPopoverKind::New {
                        role: crate::SideAgentRole::Ask,
                        start: crate::SideAgentStart::Auto,
                    }
                } else {
                    crate::agent_popover::AgentPopoverKind::History
                });
            }
            self.ask_about_pending_close();
            return;
        }
        if let Some(kind) = crate::side_panel_kind(id) {
            let (x, y) = self.press;
            let scale = ui::ui_text_scale();
            let (local_x, local_y) = self
                .hit_with_rect(x, y)
                .filter(|(hit, _)| *hit == id)
                .map_or((0.0, 0.0), |(_, rect)| {
                    ((x - rect.x) / scale, (y - rect.y) / scale)
                });
            if let Some(panel) = self.layout.side_panel_mut(kind) {
                panel.click_at(id, local_x, local_y);
            }
            self.apply_panel_requests();
            return;
        }
        self.set_terminal_focus(false);
        if id >= FUNC_VIEW_BASE {
            let menu = self.layout.files_view.as_mut().and_then(|view| {
                view.on_click(id);
                view.take_menu_request()
            });
            if let Some(entries) = menu {
                // Under the button it came from, as the reference opens its toolbar menus.
                let (x, top, bottom) = self
                    .header_hits
                    .iter()
                    .rev()
                    .find(|(_, hit)| *hit == id)
                    .map_or(
                        (self.pointer.0, self.pointer.1, self.pointer.1),
                        |(rect, _)| (rect.x, rect.y, rect.y + rect.h),
                    );
                self.item_menu = entries;
                self.menu = Some((x, top, bottom, crate::ITEM_MENU_TARGET));
                self.menu_path = None;
                self.submenu = None;
                self.menu_editor_anchor = None;
            }
            self.ask_about_pending_close();
            return;
        }
        if (FUNC_BASE..FUNC_BASE + PaneKind::ALL.len() as u64).contains(&id) {
            let i = (id - FUNC_BASE) as usize;
            // A panel button: activate this function on its side; if it was already the visible panel, toggle
            let side = self
                .layout
                .func_side
                .get(i)
                .copied()
                .unwrap_or(DockPosition::Left);
            let was_visible = self.layout.dock_open(side)
                && self.layout.shown_on(side) == Some(Shown::Func(PaneKind::ALL[i]));
            self.layout.active_panels[side.index()] = Some(Shown::Func(PaneKind::ALL[i]));
            self.toggle_side(side, was_visible);
            self.pending.persist = true;
        } else if id == crate::DIAGNOSTIC_MESSAGE {
            if let Some(v) = self.layout.files_view.as_mut() {
                v.editor_key(EditKey::GoToDiagnostic, false);
            }
        } else if id == crate::CURSOR_POSITION {
            if let Some(v) = self.layout.files_view.as_mut() {
                v.editor_key(EditKey::ToggleGoToLine, false);
            }
        } else if id == crate::ACTIVE_LANGUAGE {
            if let Some(v) = self.layout.files_view.as_mut() {
                v.editor_key(EditKey::ToggleLanguageSelector, false);
            }
        } else if id == BOTTOM_TOGGLE {
            // The terminal button: activate the terminal on its side, toggling the dock if already visible.
            let side = self.layout.terminal_side;
            let was_visible = self.layout.terminal_visible();
            self.layout.active_panels[side.index()] = Some(Shown::Terminal);
            self.toggle_side(side, was_visible);
            self.pending.persist = true;
        } else if id == AGENT_TOGGLE {
            if self.layout.agent_visible() && self.agent_focused {
                // The right dock closes; the left column has no closed state, so it goes back to its other panels.
                match self.layout.agent_side {
                    DockPosition::Right => self.layout.right.collapsed = true,
                    side => self.layout.active_panels[side.index()] = None,
                }
                self.set_agent_focus(false);
                self.pending.persist = true;
            } else {
                self.pending.open_agent = true;
            }
        } else if id == RIGHT_TOGGLE {
            self.layout.right.collapsed = !self.layout.right.collapsed;
            self.pending.persist = true;
        } else if id == SIDEBAR_TOGGLE {
            self.layout.left.collapsed = !self.layout.left.collapsed;
            self.pending.persist = true;
        } else if id == SESSION_TRIGGER {
            self.layout.session_menu = !self.layout.session_menu;
            self.session_search_query.clear();
            self.layout.session_scroll = 0.0;
            self.session_menu_hover = None;
        } else if id == SESSION_SEARCH {
            // Clicking the search field keeps the menu open (it is always the focus while open).
        } else if id == SESSION_OPEN {
            self.close_session_menu();
            self.pending.session = Some(SessionRequest::ChooseFolder);
        } else if id == crate::SESSION_NEW {
            self.close_session_menu();
            self.pending.session = Some(SessionRequest::NewProject);
        } else if id == crate::SESSION_EDIT_CONFIG {
            self.close_session_menu();
            self.pending.action = Some(crate::keymap::Action::OpenProjectConfig);
        } else if let Some(index) = session_index(id, SESSION_REVEAL_BASE) {
            self.close_session_menu();
            if index < self.layout.sessions.len() {
                self.pending.session = Some(SessionRequest::Reveal(index));
            }
        } else if let Some(index) = session_index(id, SESSION_OPENNEW_BASE) {
            if index < self.layout.sessions.len() {
                self.pending.open_new_window = Some(index);
            }
            self.close_session_menu();
        } else if let Some(index) = session_index(id, SESSION_DELETE_BASE) {
            self.close_session_menu();
            self.ask_to_forget_session(index);
        } else if let Some(index) = session_index(id, SESSION_OPENTHIS_BASE)
            .or_else(|| session_index(id, SESSION_ITEM_BASE))
        {
            self.close_session_menu();
            let openable = self.layout.sessions.get(index).is_some_and(|s| !s.missing);
            if openable && Some(index) != self.layout.current_session {
                self.pending.session = Some(SessionRequest::Switch(index));
            }
        } else if id == crate::UPDATE_BUTTON {
            self.pending.update = self
                .layout
                .update
                .button
                .as_ref()
                .and_then(|button| button.click);
        } else if id == crate::UPDATE_DISMISS {
            self.pending.update = Some(crate::UpdateAction::Dismiss);
        } else if id == crate::USAGE_CHIP {
            self.toggle_usage_popover(false);
        } else if id == crate::USAGE_STATUS {
            self.toggle_usage_popover(true);
        } else if id == crate::DIAGNOSTICS_BUTTON {
            self.deploy_project_diagnostics(true);
        } else if id == crate::LANGUAGE_SERVERS_BUTTON {
            self.toggle_status_menu(crate::LANGUAGE_SERVERS_MENU_TARGET);
        } else if id == crate::LANGUAGE_ACTIVITY {
            let click = self
                .layout
                .language_servers
                .activity
                .as_ref()
                .map(|activity| (activity.click, activity.cancellable.is_empty()));
            match click {
                Some((crate::ActivityClick::ListWork, false)) => {
                    self.toggle_status_menu(crate::LANGUAGE_ACTIVITY_MENU_TARGET)
                }
                Some((crate::ActivityClick::ShowError, _)) => {
                    if let Some(view) = self.layout.files_view.as_mut() {
                        view.language_server_action(crate::LanguageServerAction::ShowError);
                    }
                }
                Some((crate::ActivityClick::Dismiss, _)) => {
                    if let Some(view) = self.layout.files_view.as_mut() {
                        view.language_server_action(crate::LanguageServerAction::DismissActivity);
                    }
                }
                _ => {}
            }
        } else if id == crate::USAGE_OPEN {
            self.usage_popover = None;
            self.pending.action = Some(crate::keymap::Action::OpenAgentUsage);
        } else if id == crate::USAGE_REFRESH {
            self.pending.refresh_usage = true;
        } else if id == crate::APP_MENU {
            let open = self
                .menu
                .is_some_and(|menu| menu.3 == crate::APP_MENU_TARGET);
            self.menu = None;
            self.submenu = None;
            self.usage_popover = None;
            if !open {
                let (x, y) = self.press;
                if let Some((_, rect)) = self.hit_with_rect(x, y) {
                    self.menu = Some((
                        rect.x + rect.w,
                        rect.y,
                        rect.y + rect.h,
                        crate::APP_MENU_TARGET,
                    ));
                    self.menu_path = None;
                    self.menu_editor_anchor = None;
                }
            }
        } else if id == PAGE_TAB_CLOSE {
            if let Some(mut page) = self.page.take() {
                page.close_tab(0);
            }
        } else if id == PAGE_TAB {
        } else if id == crate::WELCOME_OPEN_PROJECT {
            self.pending.session = Some(SessionRequest::ChooseFolder);
        } else if id == crate::WELCOME_NEW_PROJECT || id == crate::WELCOME_NEW_PROJECT_CARD {
            self.pending.session = Some(SessionRequest::NewProject);
        } else if id == crate::WELCOME_IMPORT_BUNDLE {
            self.pending.session = Some(SessionRequest::ImportBundle);
        } else if id == crate::WELCOME_RECHECK {
            self.pending.session = Some(SessionRequest::CheckMachine);
        } else if let Some(index) = id
            .checked_sub(crate::WELCOME_FIX_BASE)
            .filter(|index| *index < 10)
        {
            self.pending.session = Some(SessionRequest::FixMachine(index as usize));
        } else if id == crate::WELCOME_OPEN_SETTINGS {
            self.pending.open_settings = true;
        } else if crate::is_welcome_id(id) {
            let index = (id - crate::WELCOME_RECENT_BASE) as usize;
            if self.layout.sessions.get(index).is_some_and(|s| !s.missing) {
                self.pending.session = Some(SessionRequest::Switch(index));
            }
        } else if id == crate::WORKSPACE_NEW {
            self.workspace_requests.new_workspace = true;
        } else if id == crate::STATUS_TICKET {
            let active = self.layout.project.as_ref().and_then(|project| {
                project
                    .workspaces
                    .iter()
                    .position(|branch| *branch == project.active)
            });
            if let Some(index) = active {
                self.workspace_requests.row = Some((index, crate::RowAction::OpenTicket));
            }
        } else if (crate::WORKSPACE_OP_BASE..crate::WORKSPACE_OP_END).contains(&id) {
            let offset = id - crate::WORKSPACE_OP_BASE;
            let position = (offset / crate::WORKSPACE_OP_STRIDE) as usize;
            let Some(op) = self.workspace_ops.get(position) else {
                return;
            };
            match offset % crate::WORKSPACE_OP_STRIDE {
                crate::WORKSPACE_OP_RETRY => {
                    self.workspace_requests.op = Some((op.id, crate::OpAction::Retry));
                }
                crate::WORKSPACE_OP_DISMISS => {
                    self.workspace_requests.op = Some((op.id, crate::OpAction::Dismiss));
                }
                crate::WORKSPACE_OP_COPY => Self::clip_set(if op.log.is_empty() {
                    &op.error
                } else {
                    &op.log
                }),
                crate::WORKSPACE_OP_CANCEL => {
                    self.workspace_requests.op = Some((op.id, crate::OpAction::Cancel));
                }
                crate::WORKSPACE_OP_SKIP => {
                    self.manual_menu = None;
                    self.workspace_requests.op = Some((op.id, crate::OpAction::Skip));
                }
                crate::WORKSPACE_OP_AGENT => {
                    self.rail_popover = None;
                    self.workspace_requests.op = Some((op.id, crate::OpAction::FixWithAgent));
                }
                crate::WORKSPACE_OP_TERMINAL => {
                    let dir = std::path::PathBuf::from(&op.fix_dir);
                    self.manual_menu = None;
                    self.rail_popover = None;
                    self.open_terminal_at(Some(dir));
                }
                crate::WORKSPACE_OP_CONFIG => {
                    let config = self
                        .layout
                        .project
                        .as_ref()
                        .map(|project| project.config_path.clone());
                    self.manual_menu = None;
                    self.rail_popover = None;
                    if let Some(config) = config {
                        self.open_file(&config);
                    }
                }
                crate::WORKSPACE_OP_MANUAL => {
                    let op_id = op.id;
                    self.manual_menu = (self.manual_menu != Some(op_id)).then_some(op_id);
                }
                crate::WORKSPACE_OP_POPOVER => {
                    let op_id = op.id;
                    self.manual_menu = None;
                    self.rail_popover = (self.rail_popover != Some(op_id)).then_some(op_id);
                }
                _ => {
                    let op_id = op.id;
                    self.toggle_workspace_op(op_id);
                }
            }
        } else if (crate::WORKSPACE_PR_BASE..crate::WORKSPACE_PR_END).contains(&id) {
            let index = (id - crate::WORKSPACE_PR_BASE) as usize;
            let switch = self.layout.project.as_ref().is_some_and(|project| {
                project
                    .workspaces
                    .get(index)
                    .is_some_and(|branch| *branch != project.active)
            });
            if switch {
                self.pending.activate_workspace = Some(index);
                self.show_after_switch = Some(PaneKind::Git);
            } else {
                self.show_function(PaneKind::Git);
            }
        } else if (crate::WORKSPACE_TICKET_BASE..crate::WORKSPACE_TICKET_END).contains(&id) {
            let index = (id - crate::WORKSPACE_TICKET_BASE) as usize;
            self.workspace_requests.row = Some((index, crate::RowAction::OpenTicket));
        } else if (crate::WORKSPACE_ROW_BASE..crate::WORKSPACE_ROW_END).contains(&id) {
            let index = (id - crate::WORKSPACE_ROW_BASE) as usize;
            let switch = self.layout.project.as_ref().is_some_and(|project| {
                project
                    .workspaces
                    .get(index)
                    .is_some_and(|branch| *branch != project.active)
            });
            if switch {
                self.pending.activate_workspace = Some(index);
            }
        } else if id == crate::NOTIFICATION_CLOSE {
            if let Some(closed) = self.notification.take() {
                if closed.title == SETUP_TITLE {
                    self.dismissed_setup = Some(closed.message);
                }
                if let Some(NotificationAction::Server(notice)) = &closed.action {
                    self.answer_server_notice(notice, None);
                }
            }
        } else if id == crate::NOTIFICATION_COPY {
            if let Some(notification) = &self.notification {
                Self::clip_set(&notification.message);
            }
        } else if (crate::NOTIFICATION_BUTTON_BASE..crate::NOTIFICATION_BUTTON_END).contains(&id) {
            let index = (id - crate::NOTIFICATION_BUTTON_BASE) as usize;
            if let Some(NotificationAction::Server(notice)) = self
                .notification
                .take()
                .and_then(|notification| notification.action)
            {
                let switch_buttons = usize::from(notice.switch.is_some());
                if index < switch_buttons {
                    self.pending.switch_language_server = notice.switch;
                    self.answer_server_notice(&notice, None);
                } else {
                    self.answer_server_notice(&notice, Some(index - switch_buttons));
                }
            }
        } else if id == crate::NOTIFICATION_PRIMARY {
            match self
                .notification
                .take()
                .and_then(|notification| notification.action)
            {
                Some(NotificationAction::OpenFile(path, line)) => {
                    if let Some(files) = self.layout.files_view.as_mut() {
                        files.open_file_at(&path, line, line.map(|_| 1));
                    }
                }
                Some(NotificationAction::FixSetup) => self.pending.fix_setup = true,
                Some(NotificationAction::RestartStale) => self.pending.restart_stale = true,
                Some(NotificationAction::ReleaseNotes) => {
                    self.pending.update = Some(crate::UpdateAction::ReleaseNotes)
                }
                Some(NotificationAction::ReopenTabs) => self.reopen_saved_tabs(),
                Some(NotificationAction::RestartUpdate) => {
                    self.pending.update = Some(crate::UpdateAction::Restart)
                }
                Some(NotificationAction::Server(_)) | None => {}
            }
        }
    }
}

impl RawView for WorkspaceView {
    fn render_frame(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> Frame {
        self.build(window)
    }
}

const PALETTE_ACTION_BASE: u64 = 1;
const PALETTE_WORKSPACE_BASE: u64 = 500;
const PALETTE_PANEL_BASE: u64 = 1_000;
const PALETTE_PANEL_STRIDE: u64 = 100_000;
const PROMPT_BUTTON_BASE: u64 = 999_000_000;
const PROMPT_BUTTON_SPAN: u64 = 16;
const PROMPT_BACKDROP: u64 = PROMPT_BUTTON_BASE + PROMPT_BUTTON_SPAN;
/// The reference prompt's `w_80`.
const PROMPT_W: f32 = 320.0;

/// A confirmation over a dimmed window, after the reference's in-app prompt: message, muted detail, and one
/// full-width button per answer with the highlighted one tinted.
fn prompt_dialog(prompt: &crate::Prompt, active: usize, w: f32, h: f32) -> Painted {
    let colors = ui::theme();
    let scale = ui::ui_text_scale();
    let text_w = PROMPT_W - 32.0;
    let mut buttons = ui::div().col().gap(4.0);
    for (index, text) in prompt.buttons.iter().enumerate() {
        let style = if index == active {
            ui::ButtonStyle::TintedAccent
        } else {
            ui::ButtonStyle::Outlined
        };
        buttons = buttons.child(
            ui::button_sized(
                PROMPT_BUTTON_BASE + index as u64,
                text.clone(),
                style,
                ui::ButtonSize::Medium,
            )
            .justify_center(),
        );
    }
    let mut dialog = ui::div()
        .col()
        .w_px(PROMPT_W)
        .p(16.0)
        .gap(16.0)
        .rounded(8.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border_variant)
        .child(
            ui::label(prompt.message.clone())
                .color(colors.text)
                .wrap(text_w),
        );
    if let Some(detail) = prompt.detail.as_ref().filter(|detail| !detail.is_empty()) {
        dialog = dialog.child(
            ui::label(detail.clone())
                .label_size(ui::LabelSize::Small)
                .color(colors.text_muted)
                .wrap(text_w),
        );
    }
    let dialog: ui::Node = dialog.child(buttons).into();
    let measured = ui::render(
        &ui::div().col().child(dialog.clone()).into(),
        Rect::new(0.0, 0.0, PROMPT_W * scale, h, Rgba::TRANSPARENT),
    );
    let dialog_h = measured.rects.iter().map(|r| r.y + r.h).fold(0.0, f32::max);
    let dialog_w = PROMPT_W * scale;
    let rect = Rect::new(
        ((w - dialog_w) / 2.0).max(8.0),
        ((h - dialog_h) / 2.0).max(8.0),
        dialog_w,
        dialog_h,
        Rgba::TRANSPARENT,
    );
    let mut painted = Painted::default();
    painted
        .rects
        .push(Rect::new(0.0, 0.0, w, h, Rgba::new(0.0, 0.0, 0.0, 0.2)));
    painted.hits.push((
        Rect::new(0.0, 0.0, w, h, Rgba::TRANSPARENT),
        PROMPT_BACKDROP,
    ));
    painted
        .rects
        .extend(elevation_shadow(rect, crate::Elevation::Modal));
    let p = ui::render(&ui::div().col().child(dialog).into(), rect);
    painted.rects.extend(p.rects);
    painted.tris.extend(p.tris);
    painted.texts.extend(p.texts);
    painted.icons.extend(p.icons);
    painted.hits.extend(p.hits);
    painted
}

/// Layered shadows under a floating surface: (y offset, alpha, blur) per elevation. The renderer has no blur,
/// so each blurred layer is a stack of rects growing across the blur width, the alpha split between them.
const SHADOW_MAX_LAYERS: f32 = 48.0;

fn elevation_shadow(rect: Rect, elevation: crate::Elevation) -> Vec<Rect> {
    let light = ui::theme().appearance == ui::Appearance::Light;
    let shadows: &[(f32, f32, f32)] = match (elevation, light) {
        (crate::Elevation::Elevated, true) => &[(2.0, 0.12, 3.0), (1.0, 0.03, 0.0)],
        (crate::Elevation::Elevated, false) => &[(2.0, 0.12, 3.0), (1.0, 0.06, 0.0)],
        (crate::Elevation::Modal, true) => &[
            (2.0, 0.06, 3.0),
            (3.0, 0.06, 6.0),
            (6.0, 0.04, 12.0),
            (1.0, 0.04, 0.0),
        ],
        (crate::Elevation::Modal, false) => &[
            (2.0, 0.12, 3.0),
            (3.0, 0.08, 6.0),
            (6.0, 0.04, 12.0),
            (1.0, 0.12, 0.0),
        ],
    };
    let layers: Vec<(f32, f32, f32, f32)> = shadows
        .iter()
        .map(|&(offset, alpha, blur)| (offset, alpha, blur, 0.0))
        .collect();
    box_shadow(rect, &layers, 8.0)
}

/// The large drop shadow a zoomed view casts: offset down, blurred, pulled in by a negative spread.
const LARGE_SHADOW: [(f32, f32, f32, f32); 2] = [(10.0, 0.1, 15.0, -3.0), (4.0, 0.1, 6.0, -4.0)];

/// Rects approximating CSS box shadows under `rect`, each `(offset_y, alpha, blur, spread)` in black.
fn box_shadow(rect: Rect, shadows: &[(f32, f32, f32, f32)], radius: f32) -> Vec<Rect> {
    let mut rects = Vec::new();
    for &(offset, alpha, blur, grow) in shadows {
        let rect = Rect::new(
            rect.x - grow,
            rect.y - grow,
            (rect.w + grow * 2.0).max(0.0),
            (rect.h + grow * 2.0).max(0.0),
            Rgba::TRANSPARENT,
        );
        // A gaussian blur with sigma = blur fades out within 3 sigma of the edge, reaching above the box even
        // though the shadow drops down; nested rects weighted by the gaussian sum to that falloff.
        let layers: Vec<(f32, f32)> = if blur <= 0.0 {
            vec![(0.0, 1.0)]
        } else {
            let extent = 3.0 * blur;
            let steps = (2.0 * extent).ceil().min(SHADOW_MAX_LAYERS) as usize;
            let step = 2.0 * extent / steps as f32;
            (0..steps)
                .map(|index| {
                    let spread = -extent + (index as f32 + 0.5) * step;
                    let density = (-0.5 * (spread / blur).powi(2)).exp()
                        / (blur * (2.0 * std::f32::consts::PI).sqrt());
                    (spread, density * step)
                })
                .collect()
        };
        for (spread, weight) in layers {
            rects.push(Rect {
                x: rect.x - spread,
                y: rect.y + offset - spread,
                w: rect.w + spread * 2.0,
                h: rect.h + spread * 2.0,
                color: Rgba::new(0.0, 0.0, 0.0, alpha * weight),
                radius: (radius + spread).max(0.0),
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            });
        }
    }
    rects
}

/// `text` cut to at most `max` characters by dropping its start, marked with a leading "...".
fn truncate_front(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - max.saturating_sub(3)).collect();
    format!("...{tail}")
}

fn terminal_modifiers() -> terminal::Modifiers {
    let held = ui::modifiers();
    terminal::Modifiers {
        shift: held.shift,
        alt: held.alt,
        ctrl: held.ctrl,
        cmd: held.cmd,
    }
}

/// Draw a pane group's layout over `region`: each pane's body (painted by the item, or text with its selections,
/// gutter, carets and scrollbars, each clipped to its area), its chrome, and the dividers. Collects the hit regions
/// that land inside each pane.
fn push_pane_group(
    layout: &mut crate::EditorLayout,
    region: Rect,
    overlays: &mut Vec<Overlay>,
    hits: &mut Vec<(Rect, u64)>,
) {
    for pane in &mut layout.panes {
        if let Some((painted, clip)) = pane.painted.take() {
            overlays.push(Overlay {
                painted,
                clip: Some(clip),
            });
        }
        if let Some((painted, clip)) = pane.popover.take() {
            overlays.push(Overlay {
                painted,
                clip: Some(clip),
            });
        }
        let chrome = ui::render(&pane.node, pane.rect);
        for (r, id) in chrome.hits.iter().copied() {
            if r.x + r.w > pane.rect.x
                && r.x < pane.rect.x + pane.rect.w
                && r.y + r.h > pane.rect.y
                && r.y < pane.rect.y + pane.rect.h
            {
                hits.push((r, id));
            }
        }
        overlays.push(Overlay {
            painted: chrome,
            clip: Some(pane.rect),
        });
        if let Some((node, at, view)) = pane.strip.take() {
            let strip = ui::render(&node, at);
            for (r, id) in strip.hits.iter().copied() {
                if r.x + r.w > view.x && r.x < view.x + view.w {
                    hits.push((r, id));
                }
            }
            overlays.push(Overlay {
                painted: strip,
                clip: Some(view),
            });
        }
        if let Some(b) = &pane.body {
            let text_clip = b.text_clip;
            let mut fill = Painted::default();
            fill.rects.push(Rect::new(
                b.rect.x,
                b.rect.y,
                b.rect.w,
                b.rect.h,
                b.background,
            ));
            overlays.push(Overlay {
                painted: fill,
                clip: Some(b.rect),
            });
            if let Some((painted, clip)) = pane.companion.take() {
                overlays.push(Overlay {
                    painted,
                    clip: Some(clip),
                });
            }
            if let Some((painted, clip)) = pane.footer.take() {
                overlays.push(Overlay {
                    painted,
                    clip: Some(clip),
                });
            }
            if !pane.back.is_empty() || !pane.back_tris.is_empty() {
                let mut sp = Painted::default();
                sp.rects.extend(pane.back.iter().copied());
                sp.tris.extend(pane.back_tris.iter().copied());
                // The same layer again under the line numbers: change strips and the caret's line reach into
                // the gutter, which the text clip leaves out.
                let mut gutter_back = Painted::default();
                gutter_back.rects.extend(pane.back.iter().copied());
                overlays.push(Overlay {
                    painted: gutter_back,
                    clip: Some(b.gutter_clip),
                });
                overlays.push(Overlay {
                    painted: sp,
                    clip: Some(text_clip),
                });
            }
            let area = Rect::new(
                b.text_left - b.x_offset,
                b.rect.y + b.y_offset,
                b.rect.w + b.x_offset + 64.0,
                b.rect.h - b.y_offset + 64.0,
                Rgba::TRANSPARENT,
            );
            let render = |node: &ui::Node, area: Rect| match b.scale {
                Some(scale) => ui::with_ui_scale(scale, || ui::render(node, area)),
                None => ui::render(node, area),
            };
            overlays.push(Overlay {
                painted: render(&b.node, area),
                clip: Some(text_clip),
            });
            if let Some(g) = &b.gutter {
                let garea = Rect::new(
                    b.rect.x,
                    b.rect.y + b.y_offset,
                    b.gutter_clip.w,
                    b.rect.h - b.y_offset + 64.0,
                    Rgba::TRANSPARENT,
                );
                let gpainted = render(g, garea);
                for (r, id) in gpainted.hits.iter().copied() {
                    if r.y + r.h > b.gutter_clip.y && r.y < b.gutter_clip.y + b.gutter_clip.h {
                        hits.push((r, id));
                    }
                }
                overlays.push(Overlay {
                    painted: gpainted,
                    clip: Some(b.gutter_clip),
                });
            }
            if !pane.carets.rects.is_empty() || !pane.carets.tris.is_empty() {
                overlays.push(Overlay {
                    painted: pane.carets.clone(),
                    clip: Some(text_clip),
                });
            }
            if let Some(minimap) = pane.minimap.take() {
                overlays.push(Overlay {
                    painted: minimap,
                    clip: Some(b.rect),
                });
            }
            for bar in [&pane.scrollbar, &pane.h_scrollbar] {
                if bar.is_empty() {
                    continue;
                }
                let mut bp = Painted::default();
                bp.rects.extend(bar.iter().copied());
                overlays.push(Overlay {
                    painted: bp,
                    clip: Some(b.rect),
                });
            }
        }
    }
    let mut dv = Painted::default();
    for d in &layout.dividers {
        let line = match d.axis {
            DividerAxis::Horizontal => Rect {
                x: d.rect.x + (d.rect.w - 1.0) / 2.0,
                y: d.rect.y,
                w: 1.0,
                h: d.rect.h,
                color: ui::theme().border,
                radius: 0.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            },
            DividerAxis::Vertical => Rect {
                x: d.rect.x,
                y: d.rect.y + (d.rect.h - 1.0) / 2.0,
                w: d.rect.w,
                h: 1.0,
                color: ui::theme().border,
                radius: 0.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            },
        };
        dv.rects.push(line);
        hits.push((d.rect, d.id));
    }
    if !dv.rects.is_empty() {
        overlays.push(Overlay {
            painted: dv,
            clip: Some(region),
        });
    }
}

const NOTIFICATION_W: f32 = 448.0;
const NOTIFICATION_MARGIN: f32 = 12.0;
const NOTIFICATION_PAD: f32 = 12.0;
const NOTIFICATION_MAX_LINES: usize = 12;

fn server_notification(notice: crate::ServerNotice) -> Notification {
    let mut message = notice.message.clone();
    let mut buttons = Vec::new();
    if let Some(switch) = &notice.switch {
        message.push_str(&format!(
            "\n\n{} can index this project instead. It keeps its index in a .{} folder in the project.",
            switch.to, switch.to
        ));
        buttons.push(format!("Use {}", switch.to));
    }
    buttons.extend(notice.actions.iter().cloned());
    Notification {
        title: notice.server.clone(),
        message,
        level: Some(notice.level),
        buttons,
        copy: true,
        action: Some(NotificationAction::Server(notice)),
        ..Notification::default()
    }
}

fn notice_button(id: u64, text: &str, hovered: Option<u64>) -> ui::Div {
    let mut button = ui::div()
        .h_px(22.0)
        .px(4.0)
        .items_center()
        .rounded(4.0)
        .on_click(id)
        .child(
            ui::label(text.to_string())
                .label_size(ui::LabelSize::Small)
                .color(ui::theme().text),
        );
    if hovered == Some(id) {
        button = button.bg(ui::theme().ghost_element_hover);
    }
    button
}

fn notification_card(notification: &Notification, w: f32, h: f32, hovered: Option<u64>) -> Painted {
    let scale = ui::ui_text_scale();
    let card_w = (NOTIFICATION_W * scale)
        .min(w - 2.0 * NOTIFICATION_MARGIN)
        .max(0.0);
    let close_w = 20.0;
    let text_w = (card_w / scale - 2.0 * NOTIFICATION_PAD - close_w - 16.0).max(40.0);
    let mut message = ui::div().col().gap(2.0);
    let lines: Vec<&str> = notification.message.lines().collect();
    for line in lines.iter().take(NOTIFICATION_MAX_LINES) {
        message = message.child(
            ui::label(line.to_string())
                .label_size(ui::LabelSize::Small)
                .color(ui::theme().text_muted)
                .wrap(text_w),
        );
    }
    if lines.len() > NOTIFICATION_MAX_LINES {
        message = message.child(
            ui::label("...")
                .label_size(ui::LabelSize::Small)
                .color(ui::theme().text_muted),
        );
    }
    let mut close = ui::div()
        .w_px(close_w)
        .h_px(close_w)
        .rounded(4.0)
        .items_center()
        .justify_center()
        .on_click(crate::NOTIFICATION_CLOSE)
        .child(
            ui::icon(IconKind::Close)
                .size(12.0)
                .color(ui::theme().icon_muted),
        );
    if hovered == Some(crate::NOTIFICATION_CLOSE) {
        close = close.bg(ui::theme().ghost_element_hover);
    }
    let mut title = ui::div().row().items_center().gap(8.0);
    if let Some(level) = notification.level {
        let (kind, color) = match level {
            crate::NoticeLevel::Error => (IconKind::XCircle, ui::theme().error),
            crate::NoticeLevel::Warning => (IconKind::Warning, ui::theme().warning),
            crate::NoticeLevel::Info => (IconKind::Info, ui::theme().icon_muted),
        };
        title = title.child(ui::icon(kind).size(14.0).color(color));
    }
    let title = title.child(
        ui::label(notification.title.clone())
            .label_size(ui::LabelSize::Default)
            .color(ui::theme().text),
    );
    let mut controls = ui::div().row().gap(4.0);
    if notification.copy {
        let mut copy = ui::div()
            .w_px(close_w)
            .h_px(close_w)
            .rounded(4.0)
            .items_center()
            .justify_center()
            .on_click(crate::NOTIFICATION_COPY)
            .child(
                ui::icon(IconKind::Copy)
                    .size(12.0)
                    .color(ui::theme().icon_muted),
            );
        if hovered == Some(crate::NOTIFICATION_COPY) {
            copy = copy.bg(ui::theme().ghost_element_hover);
        }
        controls = controls.child(copy);
    }
    let header = ui::div()
        .row()
        .justify_between()
        .gap(16.0)
        .child(ui::div().col().gap(2.0).child(title).child(message))
        .child(controls.child(close));
    let mut card = ui::div()
        .col()
        .p(NOTIFICATION_PAD)
        .gap(8.0)
        .rounded(8.0)
        .bg(ui::theme().elevated_surface_background)
        .border(1.0, ui::theme().border)
        .child(header);
    if let Some(primary) = &notification.primary {
        card = card.child(ui::div().row().child(notice_button(
            crate::NOTIFICATION_PRIMARY,
            primary,
            hovered,
        )));
    }
    if !notification.buttons.is_empty() {
        let mut row = ui::div().row().gap(4.0);
        for (index, text) in notification.buttons.iter().enumerate() {
            let id = crate::NOTIFICATION_BUTTON_BASE + index as u64;
            if id < crate::NOTIFICATION_BUTTON_END {
                row = row.child(notice_button(id, text, hovered));
            }
        }
        card = card.child(row);
    }
    let node: ui::Node = ui::div().col().child(card).into();
    let probe = ui::render(&node, Rect::new(0.0, 0.0, card_w, h, Rgba::TRANSPARENT));
    let card_h = probe.rects.iter().map(|r| r.y + r.h).fold(0.0, f32::max);
    let x = w - NOTIFICATION_MARGIN - card_w;
    let y = (h - crate::STATUS_BAR_H - NOTIFICATION_MARGIN - card_h).max(crate::TOP_BAR_H);
    let rect = Rect::new(x, y, card_w, card_h, Rgba::TRANSPARENT);
    let mut painted = Painted::default();
    painted
        .rects
        .extend(elevation_shadow(rect, crate::Elevation::Modal));
    let content = ui::render(&node, Rect::new(x, y, card_w, card_h, Rgba::TRANSPARENT));
    painted.rects.extend(content.rects);
    painted.tris.extend(content.tris);
    painted.texts.extend(content.texts);
    painted.icons.extend(content.icons);
    painted.hits.extend(content.hits);
    painted
}

fn session_index(id: u64, base: u64) -> Option<usize> {
    (base..base + 100)
        .contains(&id)
        .then(|| (id - base) as usize)
}

/// Something picked up and carried by the pointer: the card with a shadow under it.
fn lifted(node: &ui::Node, area: Rect) -> Painted {
    let card = ui::render(node, area);
    let right = card.rects.iter().map(|r| r.x + r.w).fold(area.x, f32::max);
    let bottom = card.rects.iter().map(|r| r.y + r.h).fold(area.y, f32::max);
    let bounds = Rect::new(
        area.x,
        area.y,
        right - area.x,
        bottom - area.y,
        Rgba::TRANSPARENT,
    );
    let mut painted = Painted::default();
    painted
        .rects
        .extend(elevation_shadow(bounds, crate::Elevation::Elevated));
    painted.rects.extend(card.rects);
    painted.texts.extend(card.texts);
    painted.icons.extend(card.icons);
    painted
}

/// Draws a floating card (a popover or menu) with its shadow on top, clickable; returns where it landed.
fn push_card(card: Painted, hits: &mut Vec<(Rect, u64)>, overlays: &mut Vec<Overlay>) -> Rect {
    let left = card.rects.iter().map(|r| r.x).fold(f32::MAX, f32::min);
    let top = card.rects.iter().map(|r| r.y).fold(f32::MAX, f32::min);
    let right = card.rects.iter().map(|r| r.x + r.w).fold(left, f32::max);
    let bottom = card.rects.iter().map(|r| r.y + r.h).fold(top, f32::max);
    let rect = Rect::new(left, top, right - left, bottom - top, Rgba::TRANSPARENT);
    hits.extend(card.hits.iter().copied());
    let mut painted = Painted::default();
    painted
        .rects
        .extend(elevation_shadow(rect, crate::Elevation::Elevated));
    painted.rects.extend(card.rects);
    painted.tris.extend(card.tris);
    painted.texts.extend(card.texts);
    painted.icons.extend(card.icons);
    overlays.push(Overlay {
        painted,
        clip: None,
    });
    rect
}

fn union_rect(a: Rect, b: &Rect) -> Rect {
    let (left, top) = (a.x.min(b.x), a.y.min(b.y));
    let (right, bottom) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
    Rect::new(left, top, right - left, bottom - top, Rgba::TRANSPARENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui::Application;

    #[test]
    fn a_withdrawn_notice_leaves_the_queue_and_the_card() {
        let mut view = WorkspaceView::new(Layout::default());
        let notice = |token: u64| crate::ServerNotice {
            token,
            server: format!("Language {token} is available for this file"),
            level: crate::NoticeLevel::Info,
            message: String::new(),
            actions: Vec::new(),
            switch: None,
        };
        view.server_notices
            .extend([notice(1), notice(2), notice(3)]);
        view.pull_server_notices();
        view.withdraw_server_notices(&[1, 3]);
        assert!(view.notification.is_none());
        view.pull_server_notices();
        assert_eq!(
            view.notification_text().map(|(title, _)| title.to_string()),
            Some("Language 2 is available for this file".to_string())
        );
        assert!(view.server_notices.is_empty());
    }

    #[test]
    fn held_tabs_stay_closed_until_reopened_and_a_fix_restarts() {
        let mut view = WorkspaceView::new(Layout::default());
        view.hold_saved_tabs();
        view.restore_saved_panes();
        assert!(view.panes_restored);
        assert_eq!(
            view.tab_restore,
            TabRestore::Hold,
            "nothing restored, nothing saved over"
        );
        view.notify_recovery("Pomelo quit unexpectedly last time".into(), "tabs".into());
        view.header_click(crate::NOTIFICATION_PRIMARY);
        assert_eq!(view.tab_restore, TabRestore::Restore);
        assert!(
            !view.panes_restored,
            "the next frame restores the saved tabs"
        );
        view.notify_update_ready("A fixed version is ready".into(), "0.8.2".into());
        view.header_click(crate::NOTIFICATION_PRIMARY);
        assert_eq!(
            view.take_effects().update,
            Some(crate::UpdateAction::Restart)
        );
    }

    #[test]
    fn skipping_tabs_once_saves_what_is_open_from_then_on() {
        let mut view = WorkspaceView::new(Layout::default());
        view.skip_saved_tabs_once();
        view.restore_saved_panes();
        assert_eq!(view.tab_restore, TabRestore::Restore);
        assert!(view.panes_restored);
    }

    #[test]
    fn server_notices_wait_their_turn_and_offer_a_switch() {
        let mut view = WorkspaceView::new(Layout::default());
        let switch = crate::ServerSwitch {
            language: "Ruby",
            from: "solargraph",
            to: "ruby-lsp",
        };
        view.server_notices.extend([
            crate::ServerNotice {
                token: 0,
                server: "solargraph".into(),
                level: crate::NoticeLevel::Warning,
                message: "The workspace is too large to index".into(),
                actions: Vec::new(),
                switch: Some(switch),
            },
            crate::ServerNotice {
                token: 1,
                server: "vtsls".into(),
                level: crate::NoticeLevel::Info,
                message: "Ready".into(),
                actions: vec!["OK".into()],
                switch: None,
            },
        ]);
        view.pull_server_notices();
        let shown = view.notification.clone().expect("first notice");
        assert_eq!(shown.title, "solargraph");
        assert_eq!(shown.buttons, ["Use ruby-lsp"]);
        view.header_click(crate::NOTIFICATION_BUTTON_BASE);
        assert_eq!(view.take_effects().switch_language_server, Some(switch));
        view.pull_server_notices();
        assert_eq!(view.notification_text(), Some(("vtsls", "Ready")));
        view.header_click(crate::NOTIFICATION_CLOSE);
        view.pull_server_notices();
        assert!(view.notification_text().is_none());
    }

    #[test]
    fn setup_problems_offer_a_fix_and_stay_closed_once_dismissed() {
        let mut view = WorkspaceView::new(Layout::default());
        view.set_setup_problems(Some("1 error: Docker not running".into()));
        assert_eq!(
            view.notification_text(),
            Some((
                "Project setup needs attention",
                "1 error: Docker not running"
            ))
        );
        view.header_click(crate::NOTIFICATION_PRIMARY);
        assert!(view.take_effects().fix_setup);
        assert!(view.notification_text().is_none());

        view.set_setup_problems(Some("1 error: Docker not running".into()));
        view.header_click(crate::NOTIFICATION_CLOSE);
        view.set_setup_problems(Some("1 error: Docker not running".into()));
        assert!(view.notification_text().is_none(), "closed stays closed");
        view.set_setup_problems(Some(
            "2 errors: Docker not running; Repo not found: api".into(),
        ));
        assert!(
            view.notification_text().is_some(),
            "new findings show again"
        );
        view.set_setup_problems(None);
        assert!(view.notification_text().is_none());

        view.set_config_problem(
            Some(("bad key".into(), Some(3))),
            std::path::Path::new("/p/pom.yml"),
        );
        view.set_setup_problems(Some("1 warning: Secret not set: X".into()));
        assert_eq!(
            view.notification_text().map(|(title, _)| title),
            Some("Invalid pom.yml")
        );
    }

    #[test]
    fn modal_shadow_reaches_above_the_box_and_fades() {
        let rect = Rect::new(100.0, 100.0, 200.0, 100.0, Rgba::TRANSPARENT);
        let rects = elevation_shadow(rect, crate::Elevation::Modal);
        let top = rects.iter().map(|r| r.y).fold(f32::MAX, f32::min);
        assert!(top < 80.0, "reaches {top}");
        let coverage = |y: f32| -> f32 {
            rects
                .iter()
                .filter(|r| r.y <= y && y <= r.y + r.h)
                .map(|r| r.color.a)
                .sum()
        };
        assert!(coverage(98.0) > coverage(90.0));
        assert!(coverage(90.0) > 0.0);
    }

    fn sample_sessions() -> Vec<crate::Session> {
        ["alpha", "beta", "gone"]
            .iter()
            .map(|name| crate::Session {
                name: name.to_string(),
                path: std::path::PathBuf::from(format!("/projects/{name}")),
                running: false,
                missing: *name == "gone",
            })
            .collect()
    }

    fn sample_project() -> crate::ProjectInfo {
        crate::ProjectInfo {
            name: "alpha".into(),
            branch: "trunk".into(),
            config_path: std::path::PathBuf::from("/projects/alpha/pom.yml"),
            workspaces: vec!["trunk".into(), "feat-login".into()],
            active: "feat-login".into(),
            ..Default::default()
        }
    }

    fn open_with(
        project: Option<crate::ProjectInfo>,
    ) -> (Application, ui::WindowHandle, ui::Entity<WorkspaceView>) {
        let mut app = Application::new();
        let current = project.as_ref().map(|_| 0);
        let (h, e) = app.open_raw_window(
            ui::WindowOptions {
                width: 1200.0,
                height: 800.0,
                scale: 2.0,
                ..Default::default()
            },
            move |_| {
                let mut view = WorkspaceView::new(Layout {
                    project,
                    ..Layout::default()
                });
                view.set_sessions(sample_sessions(), current);
                view
            },
        );
        (app, h, e)
    }

    fn open() -> (Application, ui::WindowHandle, ui::Entity<WorkspaceView>) {
        open_with(Some(sample_project()))
    }

    fn frame_text(frame: &ui::Frame) -> String {
        std::iter::once(&frame.base)
            .chain(frame.overlays.iter().map(|overlay| &overlay.painted))
            .flat_map(|painted| painted.texts.iter())
            .map(|text| text.text.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn a_submenu_flipped_to_the_left_still_takes_clicks() {
        let (mut app, h, e) = open();
        app.draw(h);
        e.update(app.app_mut(), |view, _| {
            view.show_app_menu(Rect::new(1170.0, 4.0, 24.0, 24.0, Rgba::TRANSPARENT))
        });
        app.draw(h);
        let parent = app
            .window(h)
            .and_then(|w| w.center_of(crate::MENU_SUBMENU_LAYOUT))
            .expect("submenu row laid out");
        e.update(app.app_mut(), |v, _| v.mouse_move(parent.0, parent.1));
        app.draw(h);
        let item = app
            .window(h)
            .and_then(|w| w.rect_of(MENU_DOCK_LEFT))
            .expect("submenu open");
        assert!(
            item.x + item.w <= parent.0,
            "flipped to the left of its row"
        );
        let (x, y) = (item.x + item.w / 2.0, item.y + item.h / 2.0);
        let side = e.update(app.app_mut(), |v, _| {
            v.mouse_move(parent.0 - 30.0, parent.1);
            v.mouse_move(x, y);
            v.mouse_down(x, y);
            v.mouse_up();
            v.layout.agent_side
        });
        assert_eq!(side, DockPosition::Left);
    }

    #[test]
    fn dragging_a_workspace_row_onto_another_asks_to_move_it_but_main_stays() {
        let mut project = sample_project();
        project.workspaces.push("feat-signup".into());
        let (mut app, h, e) = open_with(Some(project));
        app.draw(h);
        let center = |app: &Application, index: u64| {
            app.window(h)
                .and_then(|w| w.center_of(crate::WORKSPACE_ROW_BASE + index))
                .expect("row laid out")
        };
        let (last, second, main) = (center(&app, 2), center(&app, 1), center(&app, 0));
        e.update(app.app_mut(), |v, _| {
            v.mouse_down(last.0, last.1);
            v.mouse_move(second.0, second.1);
            v.mouse_up();
        });
        let requests = e.update(app.app_mut(), |v, _| v.take_workspace_requests());
        assert_eq!(requests.reorder, Some((2, 1)));

        e.update(app.app_mut(), |v, _| {
            v.mouse_down(last.0, last.1);
            v.mouse_move(main.0, main.1);
            v.mouse_up();
            v.mouse_down(main.0, main.1);
            v.mouse_move(last.0, last.1);
            v.mouse_up();
        });
        let requests = e.update(app.app_mut(), |v, _| v.take_workspace_requests());
        assert_eq!(
            requests.reorder, None,
            "main neither moves nor is displaced"
        );
    }

    #[test]
    fn grouped_rows_move_only_within_their_group_and_a_header_click_folds_it() {
        let mut project = sample_project();
        project.workspaces = vec![
            "trunk".into(),
            "proj-1-a".into(),
            "proj-2-b".into(),
            "proj-3-c".into(),
        ];
        project.tickets = vec![
            String::new(),
            "In Progress".into(),
            "Done".into(),
            "In Progress".into(),
        ];
        project.ticket_categories = vec![
            String::new(),
            "indeterminate".into(),
            "done".into(),
            "indeterminate".into(),
        ];
        let (mut app, h, e) = open_with(Some(project));
        e.update(app.app_mut(), |v, _| {
            v.set_workspace_grouping(Some(crate::Grouping::from_keys(&[], &[])))
        });
        app.draw(h);
        let center = |app: &Application, id: u64| {
            app.window(h)
                .and_then(|w| w.center_of(id))
                .expect("laid out")
        };
        let (first, other_group, same_group) = (
            center(&app, crate::WORKSPACE_ROW_BASE + 1),
            center(&app, crate::WORKSPACE_ROW_BASE + 2),
            center(&app, crate::WORKSPACE_ROW_BASE + 3),
        );
        let across = e.update(app.app_mut(), |v, _| {
            v.mouse_down(first.0, first.1);
            v.mouse_move(other_group.0, other_group.1);
            v.mouse_up();
            v.take_workspace_requests().reorder
        });
        assert_eq!(across, None, "a Done row is another group");
        let within = e.update(app.app_mut(), |v, _| {
            v.mouse_down(first.0, first.1);
            v.mouse_move(same_group.0, same_group.1);
            v.mouse_up();
            v.take_workspace_requests().reorder
        });
        assert_eq!(within, Some((1, 3)));

        let header = center(
            &app,
            crate::WORKSPACE_GROUP_BASE + crate::TicketGroup::Done.index() as u64,
        );
        let folded = e.update(app.app_mut(), |v, _| {
            v.mouse_down(header.0, header.1);
            v.mouse_up();
            let changed = v.take_workspace_requests().grouping_changed;
            (changed, v.workspace_grouping().map(|g| g.folded.clone()))
        });
        assert_eq!(folded, (true, Some(vec![crate::TicketGroup::Done])));
    }

    #[test]
    fn docking_the_agent_left_moves_only_the_agent() {
        let (mut app, _h, e) = open();
        let (left, right, region, right_x) = e.update(app.app_mut(), |v, _| {
            let git = PaneKind::ALL
                .iter()
                .position(|kind| *kind == PaneKind::Git)
                .expect("git panel");
            v.layout.func_side[git] = DockPosition::Right;
            v.move_agent_to(DockPosition::Left);
            let (w, h) = (1200.0, 800.0);
            (
                v.layout.candidates_on(DockPosition::Left),
                v.layout.candidates_on(DockPosition::Right),
                v.layout.agent_region(w, h),
                v.layout.right_region(w, h).x,
            )
        });
        assert!(left.contains(&Shown::Agent), "a panel of the left dock");
        assert!(!right.contains(&Shown::Agent));
        assert!(
            right.contains(&Shown::Func(PaneKind::Git)),
            "the right dock keeps its panels"
        );
        assert!(
            region.x < right_x,
            "drawn in the left column, not over the right dock"
        );
    }

    #[test]
    fn a_long_workspace_list_scrolls_under_the_footer() {
        let mut project = sample_project();
        project.workspaces = (0..60).map(|index| format!("feat-{index}")).collect();
        project.workspaces.insert(0, "trunk".into());
        let (mut app, h, e) = open_with(Some(project));
        app.draw(h);
        let last = crate::WORKSPACE_ROW_BASE + 60;
        assert!(
            app.window(h).and_then(|w| w.center_of(last)).is_none(),
            "clipped below"
        );
        let first = app
            .window(h)
            .and_then(|w| w.center_of(crate::WORKSPACE_ROW_BASE))
            .expect("first row");
        e.update(app.app_mut(), |v, _| {
            v.scroll(first.0, first.1, 0.0, -100_000.0)
        });
        app.draw(h);
        assert!(
            app.window(h).and_then(|w| w.center_of(last)).is_some(),
            "scrolled into view"
        );
    }

    #[test]
    fn the_session_menu_edits_the_project_config() {
        let (mut app, h, e) = open();
        app.draw(h);
        let effects = e.update(app.app_mut(), |v, _| {
            v.header_click(crate::SESSION_EDIT_CONFIG);
            v.take_effects()
        });
        assert_eq!(
            effects.action,
            Some(crate::keymap::Action::OpenProjectConfig)
        );
        let effects = e.update(app.app_mut(), |v, _| {
            v.header_click(crate::SESSION_NEW);
            v.take_effects()
        });
        assert_eq!(effects.session, Some(SessionRequest::NewProject));
    }

    #[test]
    fn an_empty_title_bar_moves_the_window_and_double_click_zooms_it() {
        let (mut app, h, e) = open();
        app.draw(h);
        let trigger = app
            .window(h)
            .and_then(|w| w.center_of(SESSION_TRIGGER))
            .expect("trigger");
        let y = crate::TOP_BAR_H / 2.0;
        let empty = e.update(app.app_mut(), |v, _| {
            (0..60)
                .map(|step| trigger.0 + 40.0 + step as f32 * 10.0)
                .find(|x| v.hit_at(*x, y).is_none())
        });
        let x = empty.expect("an empty spot in the title bar");
        let gesture = |app: &mut Application, act: &dyn Fn(&mut WorkspaceView)| {
            e.update(app.app_mut(), |v, _| {
                act(v);
                v.take_effects().titlebar
            })
        };
        assert_eq!(
            gesture(&mut app, &|v| {
                v.mouse_down(x, y);
                v.mouse_up();
            }),
            None
        );
        assert_eq!(
            gesture(&mut app, &|v| v.mouse_down(x, y)),
            Some(TitlebarGesture::DoubleClick)
        );
        gesture(&mut app, &|v| v.mouse_up());
        std::thread::sleep(Duration::from_millis(450));
        assert_eq!(
            gesture(&mut app, &|v| {
                v.mouse_down(x, y);
                v.mouse_move(x + 12.0, y);
            }),
            Some(TitlebarGesture::Move)
        );
        gesture(&mut app, &|v| v.mouse_up());
        assert_eq!(
            gesture(&mut app, &|v| {
                v.mouse_down(trigger.0, trigger.1);
                v.mouse_move(trigger.0 + 12.0, trigger.1);
            }),
            None,
            "a control in the title bar keeps its own click"
        );
    }

    #[test]
    fn fullscreen_moves_the_project_name_to_the_edge() {
        let (mut app, h, e) = open();
        app.draw(h);
        let windowed = app
            .window(h)
            .and_then(|w| w.center_of(SESSION_TRIGGER))
            .expect("trigger");
        assert!(e.update(app.app_mut(), |v, _| v.set_fullscreen(true)));
        app.draw(h);
        let full = app
            .window(h)
            .and_then(|w| w.center_of(SESSION_TRIGGER))
            .expect("trigger");
        assert!(full.0 < windowed.0 - 50.0, "{full:?} vs {windowed:?}");
    }

    #[test]
    fn the_title_bar_shows_usage_and_its_menus_run_their_actions() {
        let (mut app, h, e) = open();
        e.update(app.app_mut(), |view, _| {
            view.set_usage(crate::UsageInfo {
                account: Some(crate::UsageAccount {
                    name: "dev".into(),
                    email: "dev@example.com".into(),
                    plan: "max".into(),
                    organization: String::new(),
                }),
                session: Some(crate::UsageWindow {
                    used: 23.0,
                    resets: "in 2h".into(),
                }),
                weekly: Some(crate::UsageWindow {
                    used: 74.0,
                    resets: "Sat 17:00".into(),
                }),
                ..crate::UsageInfo::default()
            })
        });
        let frame = app.draw(h).expect("frame");
        let text = frame_text(&frame);
        assert!(text.contains("23%") && text.contains("74%"), "{text}");
        let chip = app
            .window(h)
            .and_then(|w| w.center_of(crate::USAGE_CHIP))
            .expect("chip laid out");
        e.update(app.app_mut(), |v, _| v.mouse_down(chip.0, chip.1));
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(
            text.contains("dev@example.com") && text.contains("Refresh Now"),
            "{text}"
        );
        let items: Vec<(u64, String)> = e.update(app.app_mut(), |view, _| {
            view.app_menu_items()
                .into_iter()
                .map(|item| (item.id, item.label.to_string()))
                .collect()
        });
        assert_eq!(items[0], (crate::MENU_APP_ACCOUNT, "dev".to_string()));
        assert!(items
            .iter()
            .any(|item| item.0 == crate::MENU_SUBMENU_LAYOUT));
        let effects = e.update(app.app_mut(), |view, _| {
            view.apply_menu(crate::APP_MENU_TARGET, crate::MENU_APP_USAGE);
            view.take_effects()
        });
        assert_eq!(effects.action, Some(crate::keymap::Action::OpenAgentUsage));
        let side = e.update(app.app_mut(), |view, _| {
            view.apply_menu(crate::MENU_SUBMENU_LAYOUT, MENU_DOCK_LEFT);
            view.layout.agent_side
        });
        assert_eq!(side, DockPosition::Left);
    }

    #[test]
    fn a_terminal_menu_asks_a_side_agent_about_the_selection_or_the_output() {
        let (mut app, _, e) = open();
        let context = |selection: Option<&str>| crate::TerminalContext {
            selection: selection.map(str::to_string),
            recent: "npm ERR! missing script: dev".into(),
            cwd: "/work/web".into(),
            title: "zsh".into(),
        };
        let (items, fix) = e.update(app.app_mut(), |view, _| {
            view.menu_terminal = Some((InputGroup::Panel, vec![], context(None)));
            let items: Vec<(u64, String, bool)> = view
                .terminal_menu_items()
                .into_iter()
                .map(|item| (item.id, item.label.to_string(), item.disabled))
                .collect();
            view.terminal_menu_click(crate::MENU_TERM_ASK_AGENT);
            (items, view.take_agent_fix())
        });
        let disabled = |id: u64| items.iter().find(|item| item.0 == id).map(|item| item.2);
        assert_eq!(
            disabled(crate::MENU_TERM_COPY),
            Some(true),
            "nothing selected"
        );
        assert_eq!(disabled(crate::MENU_TERM_ADD_TO_AGENT), Some(true));
        assert!(items
            .iter()
            .any(|item| item.1 == "Ask Agent about This Output"));
        let (fix, role) = fix.expect("a side agent is asked");
        assert_eq!(role, crate::SideAgentRole::Ask);
        assert!(
            fix.prompt.contains("npm ERR! missing script: dev"),
            "{}",
            fix.prompt
        );
        assert_eq!(fix.cwd, std::path::PathBuf::from("/work/web"));

        let (items, fix) = e.update(app.app_mut(), |view, _| {
            view.menu_terminal = Some((InputGroup::Panel, vec![], context(Some("panic: failed"))));
            let items = view.terminal_menu_items();
            view.terminal_menu_click(crate::MENU_TERM_ASK_AGENT);
            (items, view.take_agent_fix())
        });
        assert!(items
            .iter()
            .any(|item| item.label == "Ask Agent about Selection" && !item.disabled));
        let prompt = fix.map(|(fix, _)| fix.prompt).unwrap_or_default();
        assert!(
            prompt.contains("panic: failed") && !prompt.contains("npm ERR!"),
            "{prompt}"
        );

        let items = e.update(app.app_mut(), |view, _| {
            view.menu_terminal = Some((InputGroup::Agent, vec![], context(Some("x"))));
            view.terminal_menu_items()
        });
        assert!(
            !items
                .iter()
                .any(|item| item.id == crate::MENU_TERM_ASK_AGENT),
            "an agent's own tab is not sent to an agent"
        );
    }

    #[test]
    fn trigger_toggles_the_session_menu() {
        let (mut app, h, e) = open();
        app.draw(h);
        let trigger = app
            .window(h)
            .and_then(|w| w.center_of(SESSION_TRIGGER))
            .expect("header trigger laid out");
        e.update(app.app_mut(), |v, _| v.mouse_down(trigger.0, trigger.1));
        assert!(e.read(app.app()).menu_open(), "menu opened");
        let frame = app.draw(h).expect("frame");
        assert!(!frame.overlays.is_empty(), "menu overlays present");
        let text = frame_text(&frame);
        assert!(text.contains("beta") && text.contains("missing"), "{text}");
        assert!(text.contains("New session..."), "{text}");
    }

    #[test]
    fn open_in_new_window_flags_an_effect() {
        let (mut app, h, e) = open();
        app.draw(h);
        let trigger = app
            .window(h)
            .and_then(|w| w.center_of(SESSION_TRIGGER))
            .expect("header trigger laid out");
        e.update(app.app_mut(), |v, _| v.mouse_down(trigger.0, trigger.1));
        e.update(app.app_mut(), |v, _| {
            v.header_click(SESSION_OPENNEW_BASE + 1)
        });
        let effects = e.update(app.app_mut(), |v, _| v.take_effects());
        assert_eq!(effects.open_new_window, Some(1));
        assert!(!e.read(app.app()).menu_open(), "menu closed after action");
    }

    #[test]
    fn an_idle_window_stops_redrawing() {
        let (mut app, h, e) = open();
        app.draw(h);
        std::thread::sleep(Duration::from_millis(50));
        app.draw(h);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut frames = 0;
        while e.read(app.app()).ticking() && Instant::now() < deadline {
            app.draw(h);
            frames += 1;
            std::thread::sleep(Duration::from_millis(33));
        }
        assert!(
            !e.read(app.app()).ticking(),
            "still redrawing after {frames} idle frames"
        );
    }

    #[test]
    fn a_parked_workspace_comes_back_with_its_docks() {
        let (mut app, h, e) = open();
        app.draw(h);
        let parked = e.update(app.app_mut(), |v, _| {
            v.layout.bottom.collapsed = true;
            v.layout.right.collapsed = false;
            v.park()
        });
        e.update(app.app_mut(), |v, _| {
            v.set_project(Some(sample_project()), None, None, None);
            v.layout.bottom.collapsed = false;
            v.layout.right.collapsed = true;
        });
        e.update(app.app_mut(), |v, _| v.resume(sample_project(), parked));
        let view = e.read(app.app());
        assert!(
            view.layout.bottom.collapsed,
            "bottom dock stays as that workspace left it"
        );
        assert!(!view.layout.right.collapsed);
        assert!(
            view.panes_restored,
            "live views are not restored again from disk"
        );
    }

    #[test]
    fn clicking_a_workspace_row_asks_to_activate_it() {
        let (mut app, h, e) = open();
        app.draw(h);
        let row = app
            .window(h)
            .and_then(|w| w.center_of(crate::WORKSPACE_ROW_BASE))
            .expect("trunk row laid out");
        e.update(app.app_mut(), |v, _| v.mouse_down(row.0, row.1));
        let effects = e.update(app.app_mut(), |v, _| v.take_effects());
        assert_eq!(effects.activate_workspace, Some(0));

        let active = e.update(app.app_mut(), |v, _| {
            v.header_click(crate::WORKSPACE_ROW_BASE + 1);
            v.take_effects().activate_workspace
        });
        assert_eq!(active, None, "the active workspace is already open");
    }

    #[test]
    fn chrome_shows_the_project_branch_and_workspaces() {
        let (mut app, h, _) = open();
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(
            text.contains("trunk") && text.contains("feat-login"),
            "{text}"
        );
        assert!(
            text.contains("WORKSPACES") && text.contains("alpha"),
            "{text}"
        );
        assert!(!text.contains("Welcome to Pomelo"), "{text}");
    }

    #[test]
    fn no_project_shows_the_welcome_page_with_recent_sessions() {
        let (mut app, h, e) = open_with(None);
        let frame = app.draw(h).expect("frame");
        let text = frame_text(&frame);
        for expected in [
            "Welcome to Pomelo",
            "GET STARTED",
            "Open a project folder",
            "RECENT",
            "alpha",
            "beta",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(
            !text.contains("gone"),
            "missing sessions are not offered: {text}"
        );
        assert!(text.contains("Open a Project"), "{text}");

        let open_project = app
            .window(h)
            .and_then(|w| w.center_of(crate::WELCOME_OPEN_PROJECT))
            .expect("open project button");
        e.update(app.app_mut(), |v, _| {
            v.mouse_down(open_project.0, open_project.1)
        });
        let effects = e.update(app.app_mut(), |v, _| v.take_effects());
        assert_eq!(effects.session, Some(SessionRequest::ChooseFolder));

        e.update(app.app_mut(), |v, _| {
            v.header_click(crate::WELCOME_RECENT_BASE + 1)
        });
        let effects = e.update(app.app_mut(), |v, _| v.take_effects());
        assert_eq!(effects.session, Some(SessionRequest::Switch(1)));

        e.update(app.app_mut(), |v, _| {
            v.header_click(crate::WELCOME_OPEN_SETTINGS)
        });
        assert!(
            e.update(app.app_mut(), |v, _| v.take_effects())
                .open_settings
        );
    }

    #[test]
    fn switching_skips_the_current_and_missing_sessions() {
        let (mut app, _, e) = open();
        let mut switch = |id: u64| {
            e.update(app.app_mut(), |v, _| {
                v.header_click(id);
                v.take_effects().session
            })
        };
        assert_eq!(
            switch(SESSION_ITEM_BASE + 1),
            Some(SessionRequest::Switch(1))
        );
        assert_eq!(
            switch(SESSION_OPENTHIS_BASE + 1),
            Some(SessionRequest::Switch(1))
        );
        assert_eq!(switch(SESSION_ITEM_BASE), None);
        assert_eq!(switch(SESSION_ITEM_BASE + 2), None);
        assert_eq!(
            switch(SESSION_REVEAL_BASE + 1),
            Some(SessionRequest::Reveal(1))
        );
        assert_eq!(switch(SESSION_OPEN), Some(SessionRequest::ChooseFolder));
    }

    #[test]
    fn confirmations_show_in_the_window_and_take_keys_and_clicks() {
        let (mut app, h, e) = open();
        e.update(app.app_mut(), |v, _| {
            v.header_click(SESSION_DELETE_BASE + 2)
        });
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(text.contains("Remove"), "{text}");
        assert!(text.contains("Cancel"), "{text}");
        let (focused, cancelled) = e.update(app.app_mut(), |v, _| {
            let focused = v.editor_focused() && !v.terminal_focused();
            v.editor_key(EditKey::Escape, false);
            (focused, v.take_effects().session)
        });
        assert!(focused, "the prompt takes the keyboard");
        assert_eq!(cancelled, None, "Escape picks Cancel");
        assert!(!frame_text(&app.draw(h).expect("frame")).contains("Remove"));

        e.update(app.app_mut(), |v, _| {
            v.header_click(SESSION_DELETE_BASE + 2)
        });
        app.draw(h);
        let remove = app
            .window(h)
            .and_then(|w| w.center_of(PROMPT_BUTTON_BASE))
            .expect("first button laid out");
        let confirmed = e.update(app.app_mut(), |v, _| {
            v.mouse_down(remove.0, remove.1);
            v.take_effects().session
        });
        assert_eq!(confirmed, Some(SessionRequest::Forget(2)));

        e.update(app.app_mut(), |v, _| {
            v.header_click(SESSION_DELETE_BASE + 2)
        });
        app.draw(h);
        let moved = e.update(app.app_mut(), |v, _| {
            v.editor_key(EditKey::Down, false);
            v.editor_key(EditKey::Enter, false);
            v.take_effects().session
        });
        assert_eq!(moved, None, "Down moves the highlight to Cancel");
    }

    #[test]
    fn clicking_the_open_panels_button_again_closes_the_column() {
        let (mut app, h, e) = open();
        let services = FUNC_BASE + 1;
        let open_now = |app: &mut Application| {
            e.update(app.app_mut(), |v, _| {
                (
                    v.layout.dock_open(DockPosition::Left),
                    v.layout.func_active(1),
                )
            })
        };
        e.update(app.app_mut(), |v, _| v.header_click(services));
        app.draw(h);
        assert_eq!(open_now(&mut app), (true, true));
        e.update(app.app_mut(), |v, _| v.header_click(services));
        app.draw(h);
        assert_eq!(
            open_now(&mut app),
            (false, false),
            "a second click closes it"
        );
        e.update(app.app_mut(), |v, _| v.header_click(FUNC_BASE));
        app.draw(h);
        assert_eq!(
            e.update(app.app_mut(), |v, _| (
                v.layout.dock_open(DockPosition::Left),
                v.layout.func_active(0)
            )),
            (true, true),
            "another panel's button opens the column again"
        );
    }

    #[test]
    fn the_services_button_shows_a_dot_while_a_service_runs() {
        let (mut app, h, e) = open();
        let services = FUNC_BASE + 1;
        let dots = |app: &mut Application| {
            let frame = app.draw(h).expect("frame");
            let button = app
                .window(h)
                .and_then(|w| w.rect_of(services))
                .expect("services button laid out");
            std::iter::once(&frame.base)
                .chain(frame.overlays.iter().map(|overlay| &overlay.painted))
                .flat_map(|painted| painted.rects.iter())
                .filter(|r| {
                    r.color == ui::theme().success
                        && r.x >= button.x
                        && r.x + r.w <= button.x + button.w
                        && r.y >= button.y
                        && r.y + r.h <= button.y + button.h
                })
                .count()
        };
        assert_eq!(dots(&mut app), 0);
        assert!(e.update(app.app_mut(), |v, _| v.set_services_running(true)));
        assert_eq!(dots(&mut app), 1);
        assert!(
            !e.update(app.app_mut(), |v, _| v.set_services_running(true)),
            "no change, no repaint"
        );
        e.update(app.app_mut(), |v, _| v.set_services_running(false));
        assert_eq!(dots(&mut app), 0);
    }

    #[test]
    fn the_folded_sidebar_is_a_rail_that_names_its_cells_on_hover() {
        let (mut app, h, e) = open();
        e.update(app.app_mut(), |v, _| v.layout.left.collapsed = true);
        app.draw(h);
        let cell = app
            .window(h)
            .and_then(|w| w.center_of(crate::WORKSPACE_ROW_BASE + 1))
            .expect("the feat-login cell is laid out");
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(text.contains("feat"), "{text}");
        e.update(app.app_mut(), |v, _| v.mouse_move(cell.0, cell.1));
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(text.contains("feat-login"), "hover names it: {text}");
    }

    #[test]
    fn the_status_bar_opens_the_active_workspaces_ticket() {
        let mut project = sample_project();
        project.tickets = vec![String::new(), "In Progress".into()];
        let (mut app, h, e) = open_with(Some(project));
        app.draw(h);
        let button = app
            .window(h)
            .and_then(|w| w.center_of(crate::STATUS_TICKET))
            .expect("the ticket button is in the status bar");
        let request = e.update(app.app_mut(), |v, _| {
            v.mouse_down(button.0, button.1);
            v.take_workspace_requests().row
        });
        assert_eq!(request, Some((1, crate::RowAction::OpenTicket)));
    }

    #[test]
    fn a_ticket_status_click_asks_to_open_the_ticket() {
        let (mut app, _h, e) = open();
        let request = e.update(app.app_mut(), |v, _| {
            v.header_click(crate::WORKSPACE_TICKET_BASE + 1);
            v.take_workspace_requests().row
        });
        assert_eq!(request, Some((1, crate::RowAction::OpenTicket)));
    }

    #[test]
    fn a_failed_background_update_stays_on_main_and_retries_from_its_details() {
        let (mut app, h, e) = open();
        let failed = crate::WorkspaceOp {
            id: 9,
            branch: String::new(),
            title: "Updating main".into(),
            status: crate::OpStatus::Failed,
            stages: Vec::new(),
            detail: String::new(),
            error: "api: migrate failed".into(),
            retryable: true,
            quiet: true,
            ..Default::default()
        };
        let toast = e.update(app.app_mut(), |v, _| {
            v.set_workspace_ops(vec![failed]);
            v.header_click(crate::WORKSPACE_OP_BASE + crate::WORKSPACE_OP_TOGGLE);
            v.toast.is_some()
        });
        assert!(!toast, "a failure stays in the sidebar, not a toast");
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(text.contains("Update failed"), "{text}");
        assert!(text.contains("Retry"), "{text}");
        let request = e.update(app.app_mut(), |v, _| {
            v.header_click(crate::WORKSPACE_OP_BASE + crate::WORKSPACE_OP_RETRY);
            v.take_workspace_requests().op
        });
        assert_eq!(request, Some((9, crate::OpAction::Retry)));
    }

    #[test]
    fn a_background_update_that_finishes_says_up_to_date() {
        let (mut app, h, e) = open();
        let running = crate::WorkspaceOp {
            id: 9,
            branch: String::new(),
            title: "Updating main".into(),
            status: crate::OpStatus::Running,
            stages: Vec::new(),
            detail: String::new(),
            error: String::new(),
            retryable: true,
            quiet: true,
            ..Default::default()
        };
        e.update(app.app_mut(), |v, _| {
            v.set_workspace_ops(vec![running]);
            v.set_workspace_ops(Vec::new());
        });
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(text.contains("Up to date"), "{text}");
    }

    #[test]
    fn removing_a_session_asks_first() {
        let (mut app, _, e) = open();
        let prompt = e.update(app.app_mut(), |v, _| {
            v.header_click(SESSION_DELETE_BASE + 2);
            v.take_prompt()
        });
        let prompt = prompt.expect("confirmation prompt");
        assert!(prompt.message.contains("gone"));
        assert_eq!(prompt.buttons, ["Remove", "Cancel"]);

        let cancelled = e.update(app.app_mut(), |v, _| {
            v.prompt_answered(prompt.token, 1);
            v.take_effects().session
        });
        assert_eq!(cancelled, None);
        let confirmed = e.update(app.app_mut(), |v, _| {
            v.prompt_answered(prompt.token, 0);
            v.take_effects().session
        });
        assert_eq!(confirmed, Some(SessionRequest::Forget(2)));

        let current = e.update(app.app_mut(), |v, _| {
            v.header_click(SESSION_DELETE_BASE);
            v.take_prompt()
        });
        assert!(current.is_none(), "the open session can't be removed");
    }

    #[test]
    fn config_problem_notification_shows_dismisses_and_clears() {
        let (mut app, h, e) = open();
        let config = std::path::PathBuf::from("/projects/alpha/pom.yml");
        let problem = |message: &str| Some((message.to_string(), Some(4)));
        e.update(app.app_mut(), |v, _| {
            v.set_config_problem(problem("line 4: cannot unmarshal"), &config)
        });
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(text.contains("Invalid pom.yml"), "{text}");
        assert!(text.contains("line 4: cannot unmarshal"), "{text}");
        assert!(text.contains("Open pom.yml"), "{text}");

        let close = app
            .window(h)
            .and_then(|w| w.center_of(crate::NOTIFICATION_CLOSE))
            .expect("close button");
        e.update(app.app_mut(), |v, _| v.mouse_down(close.0, close.1));
        assert!(e.read(app.app()).notification_text().is_none());

        e.update(app.app_mut(), |v, _| {
            v.set_config_problem(problem("line 4: cannot unmarshal"), &config)
        });
        assert!(
            e.read(app.app()).notification_text().is_none(),
            "a dismissed problem stays hidden while unchanged"
        );
        e.update(app.app_mut(), |v, _| {
            v.set_config_problem(problem("line 9: other"), &config)
        });
        assert_eq!(
            e.read(app.app())
                .notification_text()
                .map(|(_, m)| m.to_string()),
            Some("line 9: other".to_string())
        );
        e.update(app.app_mut(), |v, _| v.set_config_problem(None, &config));
        assert!(e.read(app.app()).notification_text().is_none());
        let text = frame_text(&app.draw(h).expect("frame"));
        assert!(!text.contains("Invalid pom.yml"), "{text}");
    }
}
