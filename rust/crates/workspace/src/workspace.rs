//! Workspace layout: a self-managed top bar, a left dock (the workspace-list panel — our addition) and a
//! right dock, both resizable via a divider and collapsing to a fixed icon rail instead of vanishing, plus the
//! content area. Computes rectangles, text runs and hit regions; the app drives input and rendering.

mod panel;
mod workspace_view;
pub use panel::{
    function_bar, function_content, function_dock_body, terminal_content, terminal_dock_body,
    DockPosition, OutlinePanel, PaneKind, Panel, ProjectPanel, TerminalPanel,
};

// Re-exported below where defined: status_bar, status_tooltip, tooltip_above, session_action_tooltip, tooltip.
pub use workspace_view::{WorkspaceEffects, WorkspaceView};

use ui::{div, folder_icon, label, plus_icon, render, theme, Node, Painted, Rect, Rgba, Text};

pub const TOP_BAR_H: f32 = 38.0;
pub const STATUS_BAR_H: f32 = 24.0; // the thin status strip at the very bottom (a component, not a dock)
pub const RAIL_W: f32 = 48.0; // collapsed dock width — the icon rail; the dock never goes narrower than this
pub const FUNC_BASE: u64 = 700; // function-nav click ids (bottom bar): FUNC_BASE + PaneKind index
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
/// Agent right-click menu item ids.
pub const MENU_DOCK_LEFT: u64 = 810;
pub const MENU_DOCK_RIGHT: u64 = 811;
pub const MENU_DOCK_BOTTOM: u64 = 813;
pub const MENU_HIDE: u64 = 812;
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

    /// Left edge of the editor area (right of the sidebar when docked left; 0 when the sidebar is on the right).
    fn editor_l(&self) -> f32 {
        if self.sidebar_left() {
            self.left_w()
        } else {
            0.0
        }
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
            x - self.editor_l()
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
            self.editor_l() + self.right_w()
        } else {
            self.editor_r(w) - self.right_w()
        };
        y > TOP_BAR_H && (x - edge).abs() <= DIVIDER_HIT
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
        let mut rects = Vec::new();
        let mut texts = Vec::new();

        // top bar
        rects.push(Rect {
            x: 0.0,
            y: 0.0,
            w,
            h: TOP_BAR_H,
            color: top_bar_c(),
            radius: 0.0,
            border: 0.0,
            border_color: Rgba::TRANSPARENT,
        });
        // The session switcher trigger (left) is rendered by `header()` as an interactive overlay; keep the
        // branch label centered here.
        texts.push(Text {
            x: w / 2.0 - 34.0,
            y: 10.0,
            size: 14.0,
            color: text_dim_c(),
            text: "main  -  8".into(),
            mono: false,
            weight: ui::ui_font_weight(),
            wrap: 0.0,
        });

        // The WORKSPACES panel (special, switches project) is full height. Everything to its right is the editor
        // area: its panels sit ABOVE the status strip, and the status strip spans the whole editor width.
        let editor_h = (content_bottom - by).max(0.0);

        let zl = self.editor_l();
        let zr = self.editor_r(w);
        // WORKSPACES sidebar — full height, on its docked side
        let sx = if self.sidebar_left() { 0.0 } else { w - lw };
        rects.push(Rect::new(
            sx,
            by,
            lw,
            dock_h,
            if self.left.collapsed {
                rail_c()
            } else {
                dock_c()
            },
        ));
        // agent dock — either side of the editor area per `agent_side`, above the status strip
        if rw > 0.0 {
            let ax = if self.agent_left() { zl } else { zr - rw };
            rects.push(Rect::new(ax, by, rw, editor_h, dock_c()));
        }

        let cx0 = self.center_l();
        let cw = (self.center_r(w) - cx0).max(0.0);

        // bottom dock: center width, above the status strip, with a divider on top.
        let bd_h = self.bottom_h();
        if bd_h > 0.0 {
            let bd_y = content_bottom - bd_h;
            rects.push(Rect::new(cx0, bd_y, cw, bd_h, dock_c()));
            rects.push(Rect::new(cx0, bd_y - 1.0, cw, 1.0, divider_c()));
        }

        // status strip: spans the whole editor area, never over the sidebar; with a top border.
        let sw = (zr - zl).max(0.0);
        rects.push(Rect::new(zl, content_bottom, sw, STATUS_BAR_H, top_bar_c()));
        rects.push(Rect::new(zl, content_bottom - 1.0, sw, 1.0, divider_c()));

        // The center + dock interiors render through the element tree in `WorkspaceView`.

        // column dividers: the sidebar edge (full height), and the agent dock's inner edge.
        let sidebar_edge = if self.sidebar_left() { lw } else { w - lw };
        rects.push(Rect::new(sidebar_edge - 1.0, by, 1.0, dock_h, divider_c()));
        if rw > 0.0 {
            let edge = if self.agent_left() { zl + rw } else { zr - rw };
            rects.push(Rect::new(edge - 1.0, by, 1.0, editor_h, divider_c()));
        }

        // All dock toggles live in the status bar (conventional): the sidebar toggle is its leftmost button.
        (rects, texts)
    }

    /// Bottom edge of the center/bottom-dock content (above the fixed status strip).
    fn content_bottom(&self, h: f32) -> f32 {
        h - STATUS_BAR_H
    }

    fn agent_left(&self) -> bool {
        self.agent_side == DockPosition::Left
    }

    /// Left edge of the center: the editor-area left, plus the agent dock if it's docked at that edge.
    fn center_l(&self) -> f32 {
        self.editor_l()
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
            self.editor_l()
        } else {
            self.editor_r(w) - rw
        };
        Rect::new(x, TOP_BAR_H, rw, ch, Rgba::TRANSPARENT)
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
        let bar: Node = div()
            .row()
            .items_center()
            .h_px(TOP_BAR_H)
            .child(trigger)
            .into();
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
            row.child(label("0 errors").size(12.0).color(dim))
        })
        // Right tools: cursor + language info, then the bottom and right dock buttons pushed to the far edge,
        // each preceded by a divider (editor's Right/Bottom-dock rule: divider before the buttons).
        .child({
            let mut row = div()
                .row()
                .gap(6.0)
                .items_center()
                .child(label("Ln 1, Col 1").size(12.0).color(dim))
                .child(vsep())
                .child(label("Rust").size(12.0).color(dim));
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
}

/// A right-click context menu anchored above `(ax, ay)`, clamped to stay inside `viewport_w`.
pub fn context_menu(
    ax: f32,
    atop: f32,
    abottom: f32,
    viewport_w: f32,
    viewport_h: f32,
    items: &[MenuItem],
    hovered: Option<u64>,
) -> Painted {
    let scale = ui::ui_text_scale();
    let row_h = 26.0_f32;
    let pad = 4.0_f32;
    let mw = 168.0_f32;
    // Count only the separators actually drawn (a leading `sep` on the first item is skipped).
    let seps = items
        .iter()
        .enumerate()
        .filter(|(i, it)| it.sep && *i > 0)
        .count() as f32;
    let mh = pad * 2.0 + row_h * items.len() as f32 + 5.0 * seps;
    // Clamp within the window so the menu never spills off the right edge.
    let x = ax
        .min(viewport_w - mw * scale - 6.0 * scale)
        .max(6.0 * scale);
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
        if hovered == Some(item.id) {
            row = row.bg(theme().element_hover);
        }
        let mark = if item.checked {
            ui::check_icon().size(12.0)
        } else {
            ui::icon(ui::IconKind::Check)
                .size(12.0)
                .color(Rgba::TRANSPARENT)
        };
        row = row
            .child(mark)
            .child(label(item.label).size(13.0).color(text_c()));
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
