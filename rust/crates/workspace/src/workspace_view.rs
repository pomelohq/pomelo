//! `WorkspaceView`: the main window as a reactive `RawView`. It owns the `Layout` plus the header session-menu
//! interaction state, composes the layered `Frame` (body + header base, then the session menu's fixed chrome,
//! clipped scrolling list, and tooltip overlays), and handles input via mouse/scroll/key methods. Dock-divider
//! drag and dock persistence are surfaced through `WorkspaceEffects` for the shell (winit + persisted settings).

use crate::{
    context_menu, function_content, function_dock_body, session_action_tooltip, status_bar,
    status_tooltip, terminal_content, terminal_dock_body, tooltip, tooltip_above, DockPosition,
    Layout, MenuItem, PaneKind, Shown, AGENT_TOGGLE, BOTTOM_TOGGLE, FUNC_BASE, MENU_DOCK_BOTTOM,
    MENU_DOCK_LEFT, MENU_DOCK_RIGHT, MENU_HIDE, RIGHT_TOGGLE, SESSION_DELETE_BASE,
    SESSION_ITEM_BASE, SESSION_NEW, SESSION_OPEN, SESSION_OPENNEW_BASE, SESSION_OPENTHIS_BASE,
    SESSION_REVEAL_BASE, SESSION_SEARCH, SESSION_TRIGGER, SIDEBAR_TOGGLE,
};
use ui::{Context, Frame, IconKind, Overlay, Painted, RawView, Rect, Rgba, Window};

/// Work the shell must do after an input the view handled: persist dock geometry, and/or open a new window for
/// a session ("Open in new window").
#[derive(Default, Clone, Copy)]
pub struct WorkspaceEffects {
    pub persist: bool,
    pub open_new_window: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Left,
    Right,
    Bottom,
}

pub struct WorkspaceView {
    layout: Layout,
    session_menu_hover: Option<u64>,
    session_search_query: String,
    viewport: (f32, f32),
    header_hits: Vec<(Rect, u64)>,
    dragging: Drag,
    /// An open right-click context menu: `(anchor_x, anchor_top, anchor_bottom, target button id)`.
    menu: Option<(f32, f32, f32, u64)>,
    pending: WorkspaceEffects,
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
            menu: None,
            pending: WorkspaceEffects::default(),
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn take_effects(&mut self) -> WorkspaceEffects {
        std::mem::take(&mut self.pending)
    }

    fn width(&self) -> f32 {
        self.viewport.0
    }

    fn build(&mut self, window: &Window) -> Frame {
        let (w, h) = (window.width, window.height);
        self.viewport = (w, h);
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

        // Content follows the dock its function is assigned to (Zed-style): a function docked Left renders in
        // the center (main) area, Right in the right dock, Bottom in the bottom dock. The status-bar clusters
        // are the panel "tabs". A dock with no function falls back to its default panel (outline/terminal).
        let center = match self.layout.shown_on(DockPosition::Left) {
            Some(Shown::Terminal) => terminal_content(),
            Some(Shown::Func(k)) => function_content(k),
            _ => ui::div().bg(ui::theme().editor_background).into(),
        };
        blit(ui::render(&center, self.layout.center_region(w, h)));
        let sessions: Vec<(String, bool)> = self
            .layout
            .sessions
            .iter()
            .map(|s| (s.name.clone(), s.running))
            .collect();
        let current = self.layout.current_session;
        // Clickables inside a docked panel (e.g. the sidebar's collapse toggle) route through these hits.
        let mut panel_hits: Vec<(Rect, u64)> = Vec::new();
        if !self.layout.left.collapsed {
            let region = self.layout.left_region(w, h);
            let p = self.layout.left.render_body(region, &sessions, current);
            panel_hits.extend(p.hits.iter().copied());
            blit(p);
        }
        // The WORKSPACES sidebar has its own footer (bottom-left), Zed-style: the collapse toggle lives there —
        // not in the main status bar (which never covers this special sidebar) and not in the header. Always
        // shown (even in the collapsed rail), accented while the sidebar is open.
        {
            let region = self.layout.left_region(w, h);
            let strip_top = region.y + region.h - crate::STATUS_BAR_H;
            // The footer strip: same background + top border as the status bar so the two read as one continuous
            // bottom strip of equal height (Zed's sidebar footer aligns with the status bar).
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
                Some(Shown::Terminal) => ui::render(&terminal_dock_body(), region),
                Some(Shown::Func(k)) => ui::render(&function_dock_body(k), region),
                // Agent (the right dock's default panel) and the empty case both render the OutlinePanel.
                Some(Shown::Agent) | None => {
                    self.layout.right.render_body(region, &sessions, current)
                }
            };
            panel_hits.extend(p.hits.iter().copied());
            blit(p);
        }
        if !self.layout.bottom.collapsed {
            let region = self.layout.bottom_region(w, h);
            // The bottom dock has no default panel: it only shows whatever is docked there (terminal/function).
            let p = match self.layout.shown_on(DockPosition::Bottom) {
                Some(Shown::Terminal) => ui::render(&terminal_dock_body(), region),
                Some(Shown::Func(k)) => ui::render(&function_dock_body(k), region),
                _ => ui::render(&ui::div().bg(ui::theme().panel_background).into(), region),
            };
            panel_hits.extend(p.hits.iter().copied());
            blit(p);
        }
        let status = ui::render(
            &status_bar(&self.layout, self.session_menu_hover),
            self.layout.status_region(w, h),
        );
        let status_hits = status.hits.clone();
        blit(status);
        // A tooltip above the hovered status-bar item (function nav / dock toggles), Zed-style.
        let status_tip = self.session_menu_hover.and_then(|hv| {
            let text = status_tooltip(hv)?;
            let rect = status_hits
                .iter()
                .find(|(_, id)| *id == hv)
                .map(|(r, _)| *r)?;
            Some(tooltip_above(rect, &text, w))
        });

        let header = self.layout.header(w, self.session_menu_hover);
        let mut header_hits = header.hits.clone();
        header_hits.extend(panel_hits);
        header_hits.extend(status_hits);
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

        if let Some(t) = status_tip {
            overlays.push(Overlay {
                painted: t,
                clip: None,
            });
        }

        // The right-click context menu (topmost overlay; its hits win in `hit`).
        if let Some((mx, mtop, mbottom, target)) = self.menu {
            let items = self.menu_items(target);
            let painted = context_menu(mx, mtop, mbottom, w, h, &items, self.session_menu_hover);
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

    fn hit(&self, x: f32, y: f32) -> Option<u64> {
        self.header_hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|(_, id)| *id)
    }

    /// Whether `(x, y)` is over a clickable region (for the shell to show a pointer cursor).
    pub fn hit_at(&self, x: f32, y: f32) -> Option<u64> {
        self.hit(x, y)
    }

    /// Whether a status-bar button id can be right-clicked for a context menu.
    fn menuable(id: u64) -> bool {
        id == SIDEBAR_TOGGLE
            || id == AGENT_TOGGLE
            || id == BOTTOM_TOGGLE
            || (FUNC_BASE..FUNC_BASE + PaneKind::ALL.len() as u64).contains(&id)
    }

    /// The context-menu items for a given status-bar button (dock positions valid for it + Hide Button).
    fn menu_items(&self, target: u64) -> Vec<MenuItem> {
        let hide = MenuItem {
            id: MENU_HIDE,
            label: "Hide Button",
            checked: false,
            sep: true,
        };
        if target == SIDEBAR_TOGGLE {
            let left = self.layout.sidebar_left();
            // The sidebar is always present (toggled via its button), so no Hide item.
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left",
                    checked: left,
                    sep: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right",
                    checked: !left,
                    sep: false,
                },
            ]
        } else if target == AGENT_TOGGLE {
            let left = self.layout.agent_side == DockPosition::Left;
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left",
                    checked: left,
                    sep: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right",
                    checked: !left,
                    sep: false,
                },
                hide,
            ]
        } else if target == BOTTOM_TOGGLE {
            let side = self.layout.terminal_side;
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left",
                    checked: side == DockPosition::Left,
                    sep: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right",
                    checked: side == DockPosition::Right,
                    sep: false,
                },
                MenuItem {
                    id: MENU_DOCK_BOTTOM,
                    label: "Dock Bottom",
                    checked: side == DockPosition::Bottom,
                    sep: false,
                },
                hide,
            ]
        } else if let Some(i) = target.checked_sub(FUNC_BASE) {
            // A function button: which dock its content renders in (Left = center) + Hide.
            let side = self
                .layout
                .func_side
                .get(i as usize)
                .copied()
                .unwrap_or(DockPosition::Left);
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left",
                    checked: side == DockPosition::Left,
                    sep: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right",
                    checked: side == DockPosition::Right,
                    sep: false,
                },
                MenuItem {
                    id: MENU_DOCK_BOTTOM,
                    label: "Dock Bottom",
                    checked: side == DockPosition::Bottom,
                    sep: false,
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
            DockPosition::Left => {}
        }
    }

    /// Activate a panel on `side`: open the dock, or close it if the panel was already the visible one (Zed's
    /// "clicking the active panel button closes the dock"). The center never collapses.
    fn toggle_side(&mut self, side: DockPosition, was_visible: bool) {
        match side {
            DockPosition::Right => self.layout.right.collapsed = was_visible,
            DockPosition::Bottom => self.layout.bottom.collapsed = was_visible,
            DockPosition::Left => {}
        }
    }

    /// Apply a context-menu item to its target button.
    fn apply_menu(&mut self, target: u64, item: u64) {
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
                    self.layout.agent_side = Self::dock_item_side(item);
                    self.layout.active_panels[DockPosition::Right.index()] = Some(Shown::Agent);
                    self.layout.right.collapsed = false;
                }
                MENU_HIDE => self.layout.agent_hidden = true,
                _ => {}
            }
        }
    }

    /// Right-click: open the context menu for a status-bar button; elsewhere closes any menu. Returns true if
    /// something changed (repaint).
    pub fn right_click(&mut self, x: f32, y: f32) -> bool {
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
                return true;
            }
        }
        self.menu.take().is_some()
    }

    /// Cursor move: drag a divider, or hover the header/menu. Returns true if a repaint is warranted.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        match self.dragging {
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
            Drag::None => {
                let hovered = self.hit(x, y);
                if hovered != self.session_menu_hover {
                    self.session_menu_hover = hovered;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Left-button press: route a header/menu click, close the menu, toggle a dock, or begin a divider drag.
    pub fn mouse_down(&mut self, x: f32, y: f32) {
        // A click while a context menu is open either picks an item or dismisses it.
        if let Some((_, _, _, target)) = self.menu.take() {
            if let Some(item) = self.hit(x, y).filter(|id| {
                matches!(
                    *id,
                    MENU_DOCK_LEFT | MENU_DOCK_RIGHT | MENU_DOCK_BOTTOM | MENU_HIDE
                )
            }) {
                self.apply_menu(target, item);
            }
            return;
        }
        let w = self.width();
        if let Some(id) = self.hit(x, y) {
            self.header_click(id);
        } else if self.layout.session_menu {
            self.layout.session_menu = false;
            self.session_search_query.clear();
        } else if self.layout.on_left_divider(x, y, w) {
            self.dragging = Drag::Left;
        } else if self.layout.on_right_divider(x, y, w) {
            self.dragging = Drag::Right;
        } else if self.layout.on_bottom_divider(x, y, w, self.viewport.1) {
            self.dragging = Drag::Bottom;
        }
    }

    pub fn mouse_up(&mut self) {
        if self.dragging != Drag::None {
            self.pending.persist = true;
        }
        self.dragging = Drag::None;
    }

    pub fn dragging(&self) -> bool {
        self.dragging != Drag::None
    }

    /// Wheel over the open session menu scrolls its list. Returns true if it moved.
    pub fn scroll(&mut self, dy: f32) -> bool {
        if !self.layout.session_menu {
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

    fn header_click(&mut self, id: u64) {
        if (FUNC_BASE..FUNC_BASE + PaneKind::ALL.len() as u64).contains(&id) {
            let i = (id - FUNC_BASE) as usize;
            // A panel button: activate this function on its side; if it was already the visible panel, toggle
            // its dock closed (Zed's active-button-closes-the-dock behavior).
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
        } else if id == BOTTOM_TOGGLE {
            // The terminal button: activate the terminal on its side, toggling the dock if already visible.
            let side = self.layout.terminal_side;
            let was_visible = self.layout.terminal_visible();
            self.layout.active_panels[side.index()] = Some(Shown::Terminal);
            self.toggle_side(side, was_visible);
            self.pending.persist = true;
        } else if id == AGENT_TOGGLE {
            // The agent button: activate the agent in the right dock, toggling it if already visible.
            let was_visible = self.layout.dock_open(DockPosition::Right)
                && self.layout.shown_on(DockPosition::Right) == Some(Shown::Agent);
            self.layout.active_panels[DockPosition::Right.index()] = Some(Shown::Agent);
            self.toggle_side(DockPosition::Right, was_visible);
            self.pending.persist = true;
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
        } else if id == SESSION_NEW || id == SESSION_OPEN {
            self.layout.session_menu = false;
            self.session_search_query.clear();
        } else if (SESSION_REVEAL_BASE..SESSION_REVEAL_BASE + 100).contains(&id) {
            // Reveal in Finder: no session path yet (placeholder until the core backend lands).
        } else if (SESSION_OPENNEW_BASE..SESSION_OPENNEW_BASE + 100).contains(&id) {
            let i = (id - SESSION_OPENNEW_BASE) as usize;
            if i < self.layout.sessions.len() {
                self.pending.open_new_window = Some(i);
            }
            self.layout.session_menu = false;
            self.session_search_query.clear();
        } else if (SESSION_OPENTHIS_BASE..SESSION_OPENTHIS_BASE + 100).contains(&id) {
            let i = (id - SESSION_OPENTHIS_BASE) as usize;
            if i < self.layout.sessions.len() {
                self.layout.current_session = i;
            }
            self.layout.session_menu = false;
            self.session_search_query.clear();
        } else if (SESSION_DELETE_BASE..SESSION_DELETE_BASE + 100).contains(&id) {
            let i = (id - SESSION_DELETE_BASE) as usize;
            if i < self.layout.sessions.len() && self.layout.sessions.len() > 1 {
                self.layout.sessions.remove(i);
                if self.layout.current_session >= i && self.layout.current_session > 0 {
                    self.layout.current_session -= 1;
                }
            }
        } else if (SESSION_ITEM_BASE..SESSION_ITEM_BASE + 100).contains(&id) {
            let i = (id - SESSION_ITEM_BASE) as usize;
            if i < self.layout.sessions.len() {
                self.layout.current_session = i;
            }
            self.layout.session_menu = false;
            self.session_search_query.clear();
        }
    }
}

impl RawView for WorkspaceView {
    fn render_frame(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> Frame {
        self.build(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui::Application;

    fn open() -> (Application, ui::WindowHandle, ui::Entity<WorkspaceView>) {
        let mut app = Application::new();
        let (h, e) = app.open_raw_window(
            ui::WindowOptions {
                width: 1200.0,
                height: 800.0,
                scale: 2.0,
                ..Default::default()
            },
            |_| WorkspaceView::new(Layout::default()),
        );
        (app, h, e)
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
    }

    #[test]
    fn open_in_new_window_flags_an_effect() {
        let (mut app, h, e) = open();
        // Open the menu first so the row actions are laid out.
        app.draw(h);
        let trigger = app
            .window(h)
            .and_then(|w| w.center_of(SESSION_TRIGGER))
            .unwrap();
        e.update(app.app_mut(), |v, _| v.mouse_down(trigger.0, trigger.1));
        e.update(app.app_mut(), |v, _| {
            v.header_click(SESSION_OPENNEW_BASE + 1)
        });
        let effects = e.update(app.app_mut(), |v, _| v.take_effects());
        assert_eq!(effects.open_new_window, Some(1));
        assert!(!e.read(app.app()).menu_open(), "menu closed after action");
    }
}
