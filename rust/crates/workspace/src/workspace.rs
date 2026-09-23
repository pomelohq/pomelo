//! Workspace layout: a self-managed top bar, a left dock (the workspace-list panel — our addition) and a
//! right dock, both resizable via a divider and collapsing to a fixed icon rail instead of vanishing, plus the
//! content area. Computes rectangles, text runs and hit regions; the app drives input and rendering.

pub mod pane;
pub mod pane_group;
mod panel;
pub mod search_bar;
pub mod tab_drag;
pub mod text_field;
mod workspace_view;
pub use panel::{
    function_bar, function_content, function_dock_body, terminal_content, terminal_dock_body,
    DockPosition, OutlinePanel, PaneKind, Panel, ProjectPanel, TerminalPanel,
};

// Re-exported below where defined: status_bar, status_tooltip, tooltip_above, session_action_tooltip, tooltip.
pub use workspace_view::{ResizeCursor, WorkspaceEffects, WorkspaceView};

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

use ui::{div, folder_icon, label, plus_icon, render, theme, Node, Painted, Rect, Rgba, Text};

pub const TOP_BAR_H: f32 = 38.0;
pub const STATUS_BAR_H: f32 = 24.0; // the thin status strip at the very bottom (a component, not a dock)
pub const RAIL_W: f32 = 48.0; // collapsed dock width — the icon rail; the dock never goes narrower than this
pub const FUNC_BASE: u64 = 700; // function-nav click ids (bottom bar): FUNC_BASE + PaneKind index
pub const FUNC_VIEW_BASE: u64 = 10000; // click ids owned by a feature's `FunctionView` (routed to it)
pub const TERMINAL_VIEW_BASE: u64 = 5000; // click ids owned by the terminal panel, up to FUNC_VIEW_BASE
pub const FILES_TREE_W: f32 = 260.0; // default width of the Files tree dock (left of the center editor)
pub const FILES_TREE_MIN: f32 = 160.0;
pub const FILES_TREE_MAX: f32 = 560.0;
pub const DOCK_MIN: f32 = 180.0; // narrowest expanded width
pub const DOCK_MAX: f32 = 520.0;
pub const BOTTOM_MIN: f32 = 100.0; // shortest expanded bottom-dock height
pub const BOTTOM_MAX: f32 = 600.0;
pub const DIVIDER_HIT: f32 = 5.0;
pub const TRAFFIC_INSET: f32 = 82.0; // left space reserved for the macOS traffic lights
const SESSION_LEFT: f32 = TRAFFIC_INSET + 14.0; // extra gap so the session trigger clears the traffic lights

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
    pub panel: Box<dyn Panel>,
}

impl Dock {
    /// The dock's cross-axis size (width for side docks, height for the bottom dock). The WORKSPACES panel
    /// (left) is always present and collapses to a rail; the right + bottom docks (conventional) hide entirely
    /// when closed, toggled from the status bar. `width` doubles as the bottom dock's height.
    fn effective(&self) -> f32 {
        match self.position {
            DockPosition::Left => {
                if self.collapsed {
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
    pub fn render_body(
        &mut self,
        region: Rect,
        sessions: &[(String, bool)],
        current: usize,
    ) -> Painted {
        self.panel.sync(sessions, current);
        render(&self.panel.render(), region)
    }
}

/// A dev session (a project workspace). `running` drives the status dot; `missing` marks a session whose files
/// are gone (shown struck-through with a delete affordance), mirroring the app's session switcher.
pub struct Session {
    pub name: String,
    pub running: bool,
    pub missing: bool,
}

impl Session {
    fn new(name: &str, running: bool) -> Self {
        Self {
            name: name.into(),
            running,
            missing: false,
        }
    }
}

// Header click ids for the session switcher (routed by the app). Kept distinct from dock geometry hits.
pub const SESSION_TRIGGER: u64 = 1;
pub const SESSION_NEW: u64 = 2;
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticSummary {
    pub errors: usize,
    pub warnings: usize,
    pub current: Option<String>,
}

pub const CURSOR_POSITION: u64 = 11;
pub const DIAGNOSTIC_MESSAGE: u64 = 12;
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
}

pub fn menu_key(id: u64) -> &'static str {
    match id {
        MENU_EDIT_CUT => "⌘X",
        MENU_EDIT_COPY | MENU_COPY_PATH => "⌘C",
        MENU_EDIT_PASTE => "⌘V",
        MENU_EDIT_SELECT_ALL => "⌘A",
        MENU_COPY_REL_PATH => "⌘⇧C",
        MENU_REVEAL | MENU_EDIT_REVEAL => "⌘⌥R",
        MENU_TREE_NEW_FILE => "⌘N",
        MENU_TREE_NEW_DIR => "⌥⌘N",
        MENU_TREE_OPEN_SYSTEM => "^⇧↩",
        MENU_TREE_CUT => "⌘X",
        MENU_TREE_COPY => "⌘C",
        MENU_TREE_DUPLICATE => "⌘D",
        MENU_TREE_PASTE => "⌘V",
        MENU_TREE_RENAME => "↩",
        MENU_TREE_TRASH => "⌫",
        MENU_TREE_DELETE => "⌘⌦",
        MENU_EDIT_GO_TO_DEFINITION => "F12",
        MENU_EDIT_GO_TO_DECLARATION => "^F12",
        MENU_EDIT_GO_TO_TYPE_DEFINITION => "⌘F12",
        MENU_EDIT_GO_TO_IMPLEMENTATION => "⇧F12",
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
    pub left: Dock,
    pub right: Dock,
    pub bottom: Dock,
    /// Which side the WORKSPACES sidebar docks on (Left/Right), changed via its status-bar right-click menu.
    pub sidebar_side: DockPosition,
    /// Which side of the editor area the right (agent) dock renders on — changed via its status-bar right-click
    /// menu (Dock Left / Dock Right).
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
    pub current_session: usize,
    pub session_menu: bool,
    /// Pixel scroll offset of the (scrollable) session menu list.
    pub session_scroll: f32,
    pub files_view: Option<Box<dyn FunctionView>>,
    pub terminal_view: Option<Box<dyn TerminalPanelView>>,
    pub files_tree_w: f32,
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
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
    fn clone_on_split(&self) -> Option<Box<dyn Item>> {
        None
    }

    fn is_editable(&self) -> bool {
        false
    }
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
    fn select_word_at(&mut self, _local_x: f32, _local_y: f32) {}
    fn selected_text(&self) -> Option<String> {
        None
    }
    fn set_body_height(&mut self, _h: f32) {}
    fn scroll_by(&mut self, _dy: f32) -> bool {
        false
    }
    fn carets(&self, _content: ui::Rect) -> Vec<ui::Rect> {
        Vec::new()
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
    fn has_conflict(&self) -> bool {
        false
    }
    fn refresh_disk_state(&mut self) {}
    fn is_busy(&self) -> bool {
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
    /// A body the item painted itself (a terminal), drawn clipped to the area below the chrome.
    pub painted: Option<(ui::Painted, ui::Rect)>,
    pub body: Option<PaneBody>,
    pub back: Vec<ui::Rect>,
    pub back_tris: Vec<ui::Tri>,
    pub carets: Vec<ui::Rect>,
    pub scrollbar: Vec<ui::Rect>,
    pub h_scrollbar: Vec<ui::Rect>,
}

pub struct PaneBody {
    pub node: Node,
    pub rect: ui::Rect,
    pub y_offset: f32,
    pub text_left: f32,
    pub x_offset: f32,
    pub text_clip: ui::Rect,
    pub gutter: Option<Node>,
    pub gutter_clip: ui::Rect,
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
}

/// The terminal panel as the workspace drives it: drawn into whichever area shows the terminal, fed pointer and
/// keyboard input while focused, and synced with its shells on every frame.
pub trait TerminalPanelView: 'static {
    fn render(&mut self, region: Rect, focused: bool) -> ui::Painted;
    /// Tab bar clicks (ids from `TERMINAL_VIEW_BASE`).
    fn click(&mut self, id: u64) -> bool;
    /// The hit id under the pointer (tabs reveal their close button while hovered); returns whether to repaint.
    fn set_hover(&mut self, id: Option<u64>) -> bool;
    fn grid_contains(&self, x: f32, y: f32) -> bool;
    fn divider_axis(&self, id: u64) -> Option<DividerAxis>;
    fn drag_divider(&mut self, id: u64, x: f32, y: f32) -> bool;
    fn is_tab(&self, id: u64) -> bool;
    fn begin_tab_drag(&mut self, id: u64) -> bool;
    /// `over` is the hit under the pointer with its rect, so a tab under it can take the drop.
    fn update_tab_drag(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool;
    fn drop_tab(&mut self) -> bool;
    fn tab_drag_overlay(&self) -> Option<Rect>;
    fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)>;
    fn mouse_down(
        &mut self,
        x: f32,
        y: f32,
        click_count: u32,
        modifiers: terminal::Modifiers,
    ) -> bool;
    fn mouse_drag(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) -> bool;
    fn mouse_move(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) -> bool;
    fn mouse_up(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers);
    fn scroll(&mut self, x: f32, y: f32, delta_y: f32, modifiers: terminal::Modifiers) -> bool;
    fn key(&mut self, keystroke: &terminal::Keystroke) -> TerminalKeyOutcome;
    fn text(&mut self, text: &str);
    fn paste(&mut self, text: &str);
    fn focus_changed(&mut self, focused: bool);
    fn sync(&mut self, clipboard: &dyn Fn() -> Option<String>) -> TerminalSyncOutcome;
    /// Start a shell in `cwd` (the project root when `None`) as a new active tab.
    fn open(&mut self, cwd: Option<std::path::PathBuf>);
    fn is_empty(&self) -> bool;
    fn take_open_request(&mut self) -> Option<TerminalOpenTarget>;
    /// Whether the pointer is over a link that a click would open (pointing-hand cursor).
    fn link_hovered(&self) -> bool;
    /// A new shell as a pane item, for placing outside the panel.
    fn new_item(&mut self, cwd: Option<std::path::PathBuf>) -> Option<Box<dyn Item>>;
}

pub trait FunctionView: 'static {
    fn editor_layout(&mut self, area: ui::Rect) -> EditorLayout;
    fn render_tree(&mut self) -> Option<TreePanel> {
        None
    }
    /// A modal floating over the window (e.g. go to line), given the window size.
    fn modal(&mut self, _viewport: (f32, f32)) -> Option<ModalView> {
        None
    }
    fn dismiss_modal(&mut self) {}
    /// A wheel or trackpad scroll over the caret popover.
    fn popover_scroll(&mut self, _index: usize, _dy: f32) -> bool {
        false
    }
    fn diagnostic_summary(&self) -> Option<DiagnosticSummary> {
        None
    }
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
    /// Place an item as a new tab in the focused pane.
    fn add_center_item(&mut self, _item: Box<dyn Item>) {}
    /// Whether the focused item takes raw key presses (a terminal in the center).
    fn active_wants_keystrokes(&self) -> bool {
        false
    }
    fn item_keystroke(&mut self, _keystroke: &terminal::Keystroke) -> TerminalKeyOutcome {
        TerminalKeyOutcome::Ignored
    }
    fn item_focus_changed(&mut self, _focused: bool) {}
    fn item_text(&mut self, _text: &str) {}
    fn item_pointer_down(
        &mut self,
        _x: f32,
        _y: f32,
        _click_count: u32,
        _modifiers: terminal::Modifiers,
    ) -> bool {
        false
    }
    fn item_pointer_drag(&mut self, _x: f32, _y: f32, _modifiers: terminal::Modifiers) -> bool {
        false
    }
    fn item_pointer_move(&mut self, _x: f32, _y: f32, _modifiers: terminal::Modifiers) -> bool {
        false
    }
    fn item_pointer_up(&mut self, _x: f32, _y: f32, _modifiers: terminal::Modifiers) {}
    fn item_pointer_scroll(
        &mut self,
        _x: f32,
        _y: f32,
        _delta_y: f32,
        _modifiers: terminal::Modifiers,
    ) -> bool {
        false
    }
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
    /// Open an absolute path in the editor, placing the caret at a 1-based row/column when given.
    fn open_file_at(&mut self, _path: &std::path::Path, _row: Option<u32>, _column: Option<u32>) {}
    fn editor_copy_trimmed(&self) -> Option<CopiedText> {
        None
    }
    fn editor_popovers(&mut self) -> Vec<(Node, f32, f32)> {
        Vec::new()
    }
    /// A wheel or trackpad scroll over the open modal; returns whether it moved.
    fn modal_scroll(&mut self, _dy: f32) -> bool {
        false
    }
    fn cursor_position(&self) -> Option<String> {
        None
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

    fn editor_text(&mut self, _text: &str) -> bool {
        false
    }
    fn editor_paste(&mut self, text: &str, _slices: Option<&[ClipboardSlice]>) -> bool {
        self.editor_text(text)
    }
    /// The pointer moved (no button held); returns whether anything hover-dependent changed.
    fn editor_hover(&mut self, _x: f32, _y: f32) -> bool {
        false
    }
    fn editor_ime_preedit(
        &mut self,
        _text: &str,
        _selected: Option<std::ops::Range<usize>>,
    ) -> bool {
        false
    }
    fn editor_ime_commit(&mut self, _text: &str) -> bool {
        false
    }
    fn editor_copy(&self) -> Option<CopiedText> {
        None
    }
    fn editor_cut(&mut self) -> Option<CopiedText> {
        None
    }
    fn editor_key(&mut self, _key: EditKey, _shift: bool) -> bool {
        false
    }
    fn editor_click(&mut self, _x: f32, _y: f32, _extend: bool) -> bool {
        false
    }
    /// Extend the selection to a drag point, which may lie outside the pane.
    fn editor_drag(&mut self, x: f32, y: f32) -> bool {
        self.editor_click(x, y, true)
    }
    fn editor_double_click(&mut self, _x: f32, _y: f32) -> bool {
        false
    }
    fn editor_selected_text(&self) -> Option<String> {
        None
    }
    fn editor_scroll(&mut self, _x: f32, _y: f32, _dx: f32, _dy: f32) -> bool {
        false
    }
    fn editor_focused(&self) -> bool {
        false
    }
    fn row_path(&self, _id: u64) -> Option<(String, bool)> {
        None
    }
    fn root_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn open_path(&mut self, _path: &str) {}
    fn editor_save(&mut self) -> Option<Result<(), String>> {
        None
    }
    fn refresh_disk_state(&mut self) {}
    fn is_busy(&self) -> bool {
        false
    }
    fn editor_menu_anchor_at(&self, _x: f32, _y: f32) -> Option<(Vec<usize>, usize)> {
        None
    }
    fn editor_menu_y(&self, _path: &[usize], _line: usize) -> Option<f32> {
        None
    }
    fn editor_right_press(&mut self, _x: f32, _y: f32) -> bool {
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
            left: Dock {
                position: DockPosition::Left,
                width: 260.0,
                collapsed: false,
                panel: Box::new(ProjectPanel::default()),
            },
            right: Dock {
                position: DockPosition::Right,
                width: 300.0,
                collapsed: true,
                panel: Box::new(OutlinePanel),
            },
            bottom: Dock {
                position: DockPosition::Bottom,
                width: 220.0, // height for the bottom dock
                collapsed: false,
                panel: Box::new(TerminalPanel),
            },
            // Placeholder sessions until the core backend lands. Generic names only.
            sessions: vec![
                Session::new("myproject", true),
                Session::new("api", false),
                Session::new("web", false),
                Session::new("feat-login", false),
                Session::new("docs", false),
                Session::new("worker", false),
                Session::new("gateway", false),
                Session::new("admin", false),
                Session::new("mobile", false),
                Session::new("demo", false),
            ],
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
            current_session: 0,
            session_menu: false,
            session_scroll: 0.0,
            files_view: None,
            terminal_view: None,
            files_tree_w: FILES_TREE_W,
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
        self.right.effective()
    }
    /// The bottom dock's current height (0 when collapsed).
    pub fn bottom_h(&self) -> f32 {
        self.bottom.effective()
    }

    /// Whether the WORKSPACES sidebar docks on the left (default) vs the right.
    pub fn sidebar_left(&self) -> bool {
        self.sidebar_side != DockPosition::Right
    }

    /// The panels assigned to `side`, in a stable order (functions first, then the terminal). These are the
    /// "tabs" of that dock; one of them is the active/visible panel.
    pub fn candidates_on(&self, side: DockPosition) -> Vec<Shown> {
        let mut v = Vec::new();
        for (i, kind) in PaneKind::ALL.iter().enumerate() {
            if !self.func_hidden.get(i).copied().unwrap_or(false)
                && self.func_side.get(i).copied().unwrap_or(DockPosition::Left) == side
            {
                v.push(Shown::Func(*kind));
            }
        }
        if !self.terminal_hidden && self.terminal_side == side {
            v.push(Shown::Terminal);
        }
        // The agent is the right dock's default panel (listed last so functions/terminal take priority).
        if !self.agent_hidden && side == DockPosition::Right {
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
            DockPosition::Left => true,
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

    fn tree_w(&self) -> f32 {
        if self.files_tree_active() {
            self.files_tree_w.clamp(FILES_TREE_MIN, FILES_TREE_MAX)
        } else {
            0.0
        }
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
        if sw < DOCK_MIN - 20.0 {
            self.left.collapsed = true;
        } else {
            self.left.collapsed = false;
            self.left.width = sw.clamp(DOCK_MIN, DOCK_MAX);
        }
    }

    /// Drag the agent divider; width from the window edge on its side (right: `w - x`, left: `x - left_w`).
    pub fn set_right_divider(&mut self, x: f32, w: f32) {
        let rw = if self.agent_left() {
            x - self.block_l()
        } else {
            self.editor_r(w) - x
        };
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
        let edge = if self.agent_left() {
            self.block_l() + self.right_w()
        } else {
            self.editor_r(w) - self.right_w()
        };
        y > TOP_BAR_H && (x - edge).abs() <= DIVIDER_HIT
    }

    pub fn on_tree_divider(&self, x: f32, y: f32, _w: f32) -> bool {
        if !self.files_tree_active() {
            return false;
        }
        let edge = self.editor_l() + self.files_tree_w.clamp(FILES_TREE_MIN, FILES_TREE_MAX);
        y > TOP_BAR_H && (x - edge).abs() <= DIVIDER_HIT
    }

    pub fn set_tree_divider(&mut self, x: f32) {
        self.files_tree_w = (x - self.editor_l()).clamp(FILES_TREE_MIN, FILES_TREE_MAX);
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
        if ui::chrome().branch {
            top = top.child(label("main  -  8").size(14.0).color(text_dim_c()));
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
        let bl = zl + tw;

        if rw > 0.0 {
            let ax = if self.agent_left() { bl } else { zr - rw };
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
            let edge = if self.agent_left() { bl + rw } else { zr - rw };
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

    fn agent_left(&self) -> bool {
        self.agent_side == DockPosition::Left
    }

    fn center_l(&self) -> f32 {
        self.block_l()
            + if self.agent_left() {
                self.right_w()
            } else {
                0.0
            }
    }

    /// Right edge of the center: the editor-area right, minus the agent dock if it's docked at that edge.
    fn center_r(&self, w: f32) -> f32 {
        self.editor_r(w)
            - if self.agent_left() {
                0.0
            } else {
                self.right_w()
            }
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

    /// The agent dock's interior region (editor area, above the status strip; left or right per `agent_side`).
    pub fn right_region(&self, w: f32, h: f32) -> Rect {
        let rw = self.right_w();
        let ch = (self.content_bottom(h) - TOP_BAR_H).max(0.0);
        let x = if self.agent_left() {
            self.block_l()
        } else {
            self.editor_r(w) - rw
        };
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
    pub fn header(&self, w: f32, hovered: Option<u64>) -> Painted {
        let name = self
            .sessions
            .get(self.current_session)
            .map(|s| s.name.as_str())
            .unwrap_or("no session");
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
        render(
            &bar,
            Rect::new(
                SESSION_LEFT,
                0.0,
                (w - SESSION_LEFT).max(0.0),
                TOP_BAR_H,
                Rgba::TRANSPARENT,
            ),
        )
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
        if matches.contains(&self.current_session) {
            entries.push((Some("This Window"), None));
            entries.push((None, Some(self.current_session)));
        }
        let recent: Vec<usize> = matches
            .iter()
            .copied()
            .filter(|&i| i != self.current_session)
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
        let x = SESSION_LEFT;
        let y = TOP_BAR_H + 2.0;
        let menu_w = 300.0 * scale;

        let pad = MENU_PAD * scale;
        let search_h = MENU_SEARCH_H * scale;
        let div_h = MENU_DIV_H * scale;
        let footer_h = (MENU_ACTION_H * 2.0 + 2.0) * scale; // two actions + a gap
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
        let footer: Node = div()
            .col()
            .gap(2.0)
            .child(action_row(
                plus_icon().into(),
                "New session...",
                SESSION_NEW,
                hovered,
            ))
            .child(action_row(
                folder_icon().into(),
                "Open a session...",
                SESSION_OPEN,
                hovered,
            ))
            .into();
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
                let s = &self.sessions[i];
                col = col.child(session_row(i, s, i == self.current_session, hovered));
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
        let actions = div()
            .row()
            .items_center()
            .gap(2.0)
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
            ))
            .child(icon_button(
                ui::close_icon().into(),
                SESSION_DELETE_BASE + i as u64,
                hovered,
            ));
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
    let toggle = |kind: ui::IconKind, id: u64, active: bool| {
        let color = if active { theme().icon_accent } else { dim };
        let mut b = div()
            .w_px(26.0)
            .h_px(20.0)
            .rounded(5.0)
            .items_center()
            .justify_center()
            .on_click(id)
            .child(ui::icon(kind).size(13.0).color(color));
        if hovered == Some(id) {
            b = b.bg(theme().element_hover);
        }
        b
    };
    // A small vertical divider between groups (a thin), 1px on the theme border color.
    let vsep = || div().w_px(1.0).h_px(14.0).bg(theme().border);
    let agent_left = layout.agent_side == DockPosition::Left;

    // One dock's panel buttons (editor's `PanelButtons`): a row of icon buttons — the functions on this side,
    // the terminal if it lives here, and the agent (right dock's default). No dividers between the buttons;
    // the active/visible panel's icon is accented. Returns None when the dock has no buttons.
    let dock_group = |side: DockPosition| -> Option<Node> {
        let mut row = div().row().gap(4.0).items_center();
        let mut has = false;
        for (i, kind) in PaneKind::ALL.iter().enumerate() {
            if layout.func_hidden.get(i).copied().unwrap_or(false) {
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
            row = row.child(toggle(
                kind.icon(),
                FUNC_BASE + i as u64,
                layout.func_active(i),
            ));
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
        let agent_here = (agent_left && side == DockPosition::Left)
            || (!agent_left && side == DockPosition::Right);
        if !layout.agent_hidden && agent_here {
            let active = layout.dock_open(DockPosition::Right)
                && layout.shown_on(DockPosition::Right) == Some(Shown::Agent);
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
                row = row.child(indicator);
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
            if ch.language {
                if shows_position {
                    row = row.child(vsep());
                }
                row = row.child(label("Rust").size(12.0).color(dim));
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
pub struct MenuItem {
    pub id: u64,
    pub label: &'static str,
    pub checked: bool,
    pub sep: bool,
    pub disabled: bool,
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
        let label_w = ui::measure_text_width(it.label, 13.0, false, wght);
        let right = if is_submenu(it.id) {
            14.0
        } else {
            let k = menu_key(it.id);
            if k.is_empty() {
                0.0
            } else {
                ui::measure_text_width(k, 12.0, false, wght) + 24.0
            }
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
        } else {
            text_c()
        };
        let mark = if item.checked {
            ui::check_icon().size(12.0)
        } else {
            ui::icon(ui::IconKind::Check)
                .size(12.0)
                .color(Rgba::TRANSPARENT)
        };
        row = row
            .child(mark)
            .child(label(item.label).size(13.0).color(label_color));
        if is_submenu(item.id) {
            row = row.child(div().flex(1.0)).child(
                ui::icon(ui::IconKind::ChevronRight)
                    .size(12.0)
                    .color(theme().icon_muted),
            );
        } else {
            let key = menu_key(item.id);
            if !key.is_empty() {
                row = row
                    .child(div().flex(1.0))
                    .child(label(key).size(12.0).color(theme().text_muted));
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

/// The tooltip label for a hovered status-bar item (function nav / dock toggles), if any.
pub fn status_tooltip(id: u64) -> Option<String> {
    if id == BOTTOM_TOGGLE {
        Some("Terminal  ⌘J".into())
    } else if id == RIGHT_TOGGLE {
        Some("Agent  ⌘I".into())
    } else if id == CURSOR_POSITION {
        Some("Go to Line/Column  ^G".into())
    } else if id == DIAGNOSTIC_MESSAGE {
        Some("Next Diagnostic  F8".into())
    } else if (FUNC_BASE..FUNC_BASE + PaneKind::ALL.len() as u64).contains(&id) {
        Some(PaneKind::ALL[(id - FUNC_BASE) as usize].title().to_string())
    } else {
        None
    }
}

/// A tooltip bubble anchored ABOVE `anchor` (for the status bar, which sits at the very bottom).
pub fn tooltip_above(anchor: Rect, text: &str, viewport_w: f32) -> Painted {
    let scale = ui::ui_text_scale();
    let tw = ui::measure_text_width(text, 12.0, false, ui::ui_font_weight());
    let pad = 8.0 * scale;
    let w = tw + 2.0 * pad;
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
        .child(label(text).size(12.0).color(text_c()))
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
    if ui::theme().appearance == ui::Appearance::Light {
        editor::Theme::one_light()
    } else {
        editor::Theme::one_dark()
    }
}
