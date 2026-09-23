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
    SESSION_NEW, SESSION_OPEN, SESSION_OPENNEW_BASE, SESSION_OPENTHIS_BASE, SESSION_REVEAL_BASE,
    SESSION_SEARCH, SESSION_TRIGGER, SIDEBAR_TOGGLE, TREE_MENU_TARGET,
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
    Terminal,
    Left,
    Right,
    Bottom,
    Tree,
    Center(u64),
    Tab,
    EditorSel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResizeCursor {
    Horizontal,
    Vertical,
}

pub struct WorkspaceView {
    /// Where the open modal was drawn last frame, for dismissing it on outside presses.
    modal_rect: Option<Rect>,
    /// Where the caret popover was drawn last frame, so scrolling over it scrolls it.
    popover_rects: Vec<Rect>,
    pending_prompt: Option<crate::Prompt>,
    terminal_focused: bool,
    pointer: (f32, f32),
    /// Time, place and count of the last press in the terminal grid, for double/triple-click selection.
    terminal_click: Option<(Instant, f32, f32, u32)>,
    layout: Layout,
    session_menu_hover: Option<u64>,
    session_search_query: String,
    viewport: (f32, f32),
    header_hits: Vec<(Rect, u64)>,
    dragging: Drag,
    pending_tab: Option<(u64, f32, f32)>,
    tab_ghost_at: Option<(f32, f32)>,
    last_click: Option<(Instant, f32, f32)>,
    /// An open right-click context menu: `(anchor_x, anchor_top, anchor_bottom, target button id)`.
    menu: Option<(f32, f32, f32, u64)>,
    menu_path: Option<(String, bool)>,
    submenu: Option<(f32, f32, f32, u64)>,
    menu_editor_anchor: Option<(Vec<usize>, usize)>,
    toast: Option<Toast>,
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
            pending_tab: None,
            tab_ghost_at: None,
            last_click: None,
            menu: None,
            menu_path: None,
            submenu: None,
            menu_editor_anchor: None,
            modal_rect: None,
            popover_rects: Vec::new(),
            pending_prompt: None,
            terminal_focused: false,
            pointer: (0.0, 0.0),
            terminal_click: None,
            toast: None,
            pending: WorkspaceEffects::default(),
        }
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

    pub fn ticking(&self) -> bool {
        self.toast.is_some() || self.layout.files_view.as_ref().is_some_and(|v| v.is_busy())
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
        self.sync_terminals();
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
        if let Some(files_side) = self.layout.files_side() {
            {
                let tree_region = match files_side {
                    DockPosition::Left => self.layout.tree_region(w, h),
                    DockPosition::Right => self.layout.right_region(w, h),
                    DockPosition::Bottom => self.layout.bottom_region(w, h),
                };
                let (tp, editor) = {
                    let v = self.layout.files_view.as_mut().unwrap();
                    v.set_viewport(tree_region.w, tree_region.h);
                    (v.render_tree(), v.editor_layout(cr))
                };
                let (scroll, ch) = {
                    let v = self.layout.files_view.as_ref().unwrap();
                    (v.scroll_offset(), v.content_height())
                };
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
                    blit(ui::render(
                        &ui::div().bg(ui::theme().editor_background).into(),
                        cr,
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
                    let mut push_clipped =
                        |node: &ui::Node, area: Rect, clip: Rect, hits: &mut Vec<(Rect, u64)>| {
                            let p = ui::render(node, area);
                            for (r, id) in p.hits.iter().copied() {
                                let x0 = r.x.max(clip.x);
                                let y0 = r.y.max(clip.y);
                                let x1 = (r.x + r.w).min(clip.x + clip.w);
                                let y1 = (r.y + r.h).min(clip.y + clip.h);
                                if x1 > x0 && y1 > y0 {
                                    hits.push((
                                        Rect::new(x0, y0, x1 - x0, y1 - y0, Rgba::TRANSPARENT),
                                        id,
                                    ));
                                }
                            }
                            center_overlays.push(Overlay {
                                painted: p,
                                clip: Some(clip),
                            });
                        };
                    push_clipped(&sr.tree, tree_area, tree_region, &mut panel_hits);
                    push_clipped(&sr.sticky, sticky_area, tree_region, &mut panel_hits);
                    for pane in &editor.panes {
                        let chrome = ui::render(&pane.node, pane.rect);
                        for (r, id) in chrome.hits.iter().copied() {
                            if r.x + r.w > pane.rect.x
                                && r.x < pane.rect.x + pane.rect.w
                                && r.y + r.h > pane.rect.y
                                && r.y < pane.rect.y + pane.rect.h
                            {
                                panel_hits.push((r, id));
                            }
                        }
                        center_overlays.push(Overlay {
                            painted: chrome,
                            clip: Some(pane.rect),
                        });
                        if let Some(b) = &pane.body {
                            let text_clip = b.text_clip;
                            if !pane.back.is_empty() || !pane.back_tris.is_empty() {
                                let mut sp = Painted::default();
                                sp.rects.extend(pane.back.iter().copied());
                                sp.tris.extend(pane.back_tris.iter().copied());
                                center_overlays.push(Overlay {
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
                            center_overlays.push(Overlay {
                                painted: ui::render(&b.node, area),
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
                                let gpainted = ui::render(g, garea);
                                for (r, id) in gpainted.hits.iter().copied() {
                                    if r.y + r.h > b.gutter_clip.y
                                        && r.y < b.gutter_clip.y + b.gutter_clip.h
                                    {
                                        panel_hits.push((r, id));
                                    }
                                }
                                center_overlays.push(Overlay {
                                    painted: gpainted,
                                    clip: Some(b.gutter_clip),
                                });
                            }
                            if !pane.carets.is_empty() {
                                let mut cp = Painted::default();
                                cp.rects.extend(pane.carets.iter().copied());
                                center_overlays.push(Overlay {
                                    painted: cp,
                                    clip: Some(text_clip),
                                });
                            }
                            for bar in [&pane.scrollbar, &pane.h_scrollbar] {
                                if bar.is_empty() {
                                    continue;
                                }
                                let mut bp = Painted::default();
                                bp.rects.extend(bar.iter().copied());
                                center_overlays.push(Overlay {
                                    painted: bp,
                                    clip: Some(b.rect),
                                });
                            }
                        }
                    }
                    let mut dv = Painted::default();
                    for d in &editor.dividers {
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
                        panel_hits.push((d.rect, d.id));
                    }
                    if !dv.rects.is_empty() {
                        center_overlays.push(Overlay {
                            painted: dv,
                            clip: Some(cr),
                        });
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
                    if let Some((gx, gy)) = self.tab_ghost_at {
                        if let Some((node, gw, gh)) = self
                            .layout
                            .files_view
                            .as_ref()
                            .and_then(|v| v.tab_drag_ghost())
                        {
                            let area =
                                Rect::new(gx - 14.0, gy - gh / 2.0, gw, gh, Rgba::TRANSPARENT);
                            center_overlays.push(Overlay {
                                painted: ui::render(&node, area),
                                clip: None,
                            });
                        }
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
            }
        } else {
            match self.layout.shown_on(DockPosition::Left) {
                Some(Shown::Terminal) => {
                    let p = self.terminal_painted(cr);
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
        let sessions: Vec<(String, bool)> = self
            .layout
            .sessions
            .iter()
            .map(|s| (s.name.clone(), s.running))
            .collect();
        let current = self.layout.current_session;
        if !self.layout.left.collapsed {
            let region = self.layout.left_region(w, h);
            let p = self.layout.left.render_body(region, &sessions, current);
            panel_hits.extend(p.hits.iter().copied());
            blit(p);
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
                Some(Shown::Terminal) => self.terminal_painted(region),
                Some(Shown::Func(PaneKind::Files)) => Painted::default(),
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
                Some(Shown::Terminal) => self.terminal_painted(region),
                Some(Shown::Func(PaneKind::Files)) => Painted::default(),
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

        self.popover_rects.clear();
        let popovers = self
            .layout
            .files_view
            .as_mut()
            .map(|v| v.editor_popovers())
            .unwrap_or_default();
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
        if let Some(modal) = self
            .layout
            .files_view
            .as_mut()
            .and_then(|v| v.modal(viewport))
        {
            // Modal widths are design px; the tree scales them with the UI text size.
            let modal_w = modal.width * ui::ui_text_scale();
            let x = ((w - modal_w) / 2.0).max(8.0);
            let area = Rect::new(x, MODAL_TOP, modal_w, h - MODAL_TOP, Rgba::TRANSPARENT);
            // Wrapped in a column so the modal keeps its content height instead of filling the area.
            let p = ui::render(&ui::div().col().child(modal.node).into(), area);
            let bottom = p.rects.iter().map(|r| r.y + r.h).fold(MODAL_TOP, f32::max);
            let rect = Rect::new(x, MODAL_TOP, modal_w, bottom - MODAL_TOP, Rgba::TRANSPARENT);
            self.modal_rect = Some(rect);
            header_hits.extend(p.hits.iter().copied());
            let mut painted = Painted::default();
            painted
                .rects
                .extend(elevation_shadow(rect, modal.elevation));
            painted.rects.extend(p.rects);
            painted.tris.extend(p.tris);
            painted.texts.extend(p.texts);
            painted.icons.extend(p.icons);
            overlays.push(Overlay {
                painted,
                clip: None,
            });
        }

        // The right-click context menu (topmost overlay; its hits win in `hit`).
        if let Some((mx, mtop, mbottom, target)) = self.menu {
            let (mtop, mbottom, keep) = if target == EDITOR_MENU_TARGET {
                match self.menu_editor_anchor.clone() {
                    Some((path, line)) => match self
                        .layout
                        .files_view
                        .as_ref()
                        .and_then(|v| v.editor_menu_y(&path, line))
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

    pub fn resize_cursor_at(&self, x: f32, y: f32) -> Option<ResizeCursor> {
        match self.dragging {
            Drag::Left | Drag::Right | Drag::Tree => return Some(ResizeCursor::Horizontal),
            Drag::Bottom => return Some(ResizeCursor::Vertical),
            Drag::Center(id) => return self.center_divider_cursor(id),
            Drag::Tab | Drag::EditorSel | Drag::Terminal => return None,
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
        match self.layout.files_view.as_ref()?.divider_axis(id)? {
            DividerAxis::Horizontal => Some(ResizeCursor::Horizontal),
            DividerAxis::Vertical => Some(ResizeCursor::Vertical),
        }
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
            disabled: false,
        };
        let item = |id: u64, label: &'static str, sep: bool| MenuItem {
            id,
            label,
            checked: false,
            sep,
            disabled: false,
        };
        let disabled = |id: u64, label: &'static str, sep: bool, disabled: bool| MenuItem {
            id,
            label,
            checked: false,
            sep,
            disabled,
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
            ];
        }
        if target == SIDEBAR_TOGGLE {
            let left = self.layout.sidebar_left();
            // The sidebar is always present (toggled via its button), so no Hide item.
            vec![
                MenuItem {
                    id: MENU_DOCK_LEFT,
                    label: "Dock Left",
                    checked: left,
                    sep: false,
                    disabled: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right",
                    checked: !left,
                    sep: false,
                    disabled: false,
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
                    disabled: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right",
                    checked: !left,
                    sep: false,
                    disabled: false,
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
                    disabled: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right",
                    checked: side == DockPosition::Right,
                    sep: false,
                    disabled: false,
                },
                MenuItem {
                    id: MENU_DOCK_BOTTOM,
                    label: "Dock Bottom",
                    checked: side == DockPosition::Bottom,
                    sep: false,
                    disabled: false,
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
                    label: "Dock Left",
                    checked: side == DockPosition::Left,
                    sep: false,
                    disabled: false,
                },
                MenuItem {
                    id: MENU_DOCK_RIGHT,
                    label: "Dock Right",
                    checked: side == DockPosition::Right,
                    sep: false,
                    disabled: false,
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

    fn toggle_side(&mut self, side: DockPosition, was_visible: bool) {
        match side {
            DockPosition::Right => self.layout.right.collapsed = was_visible,
            DockPosition::Bottom => self.layout.bottom.collapsed = was_visible,
            DockPosition::Left => {}
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
    fn apply_menu(&mut self, target: u64, item: u64) {
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
        if target == EDITOR_MENU_TARGET {
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
                    if let Some(v) = self.layout.files_view.as_mut() {
                        v.editor_key(EditKey::SelectAll, false);
                    }
                }
                MENU_EDIT_COPY_TRIM => {
                    let copied = self
                        .layout
                        .files_view
                        .as_ref()
                        .and_then(|v| v.editor_copy_trimmed());
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
                    if let Some(v) = self.layout.files_view.as_mut() {
                        v.editor_key(key, false);
                    }
                }
                crate::MENU_EDIT_OPEN_TERMINAL => {
                    let directory = self
                        .layout
                        .files_view
                        .as_ref()
                        .and_then(|v| v.active_file_path())
                        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
                    self.open_terminal_at(directory);
                }
                MENU_EDIT_REVEAL => {
                    let path = self
                        .layout
                        .files_view
                        .as_ref()
                        .and_then(|v| v.active_file_path());
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
        if in_tree && self.layout.files_view.is_some() {
            self.menu = Some((x, y, y, TREE_MENU_TARGET));
            self.menu_path = Some((String::new(), true));
            self.submenu = None;
            self.menu_editor_anchor = None;
            return true;
        }
        let cr = self.layout.center_region(vw, vh);
        let in_center = x >= cr.x && x < cr.x + cr.w && y >= cr.y && y < cr.y + cr.h;
        if in_center
            && self
                .layout
                .files_view
                .as_ref()
                .is_some_and(|v| v.editor_focused())
        {
            if let Some(v) = self.layout.files_view.as_mut() {
                v.editor_right_press(x, y);
            }
            self.menu = Some((x, y, y, EDITOR_MENU_TARGET));
            self.menu_path = None;
            self.submenu = None;
            self.menu_editor_anchor = self
                .layout
                .files_view
                .as_ref()
                .and_then(|v| v.editor_menu_anchor_at(x, y));
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
        let over_terminal = self.terminal_grid_at(x, y);
        let hit = self.hit(x, y);
        let terminal_repaint = match self.layout.terminal_view.as_mut() {
            Some(view) => {
                let modifiers = terminal_modifiers();
                let mut repaint = view.set_hover(hit);
                if self.dragging == Drag::Terminal {
                    repaint |= view.mouse_drag(x, y, modifiers);
                } else if over_terminal && self.terminal_focused {
                    repaint |= view.mouse_move(x, y, modifiers);
                }
                repaint
            }
            None => false,
        };
        if self.dragging == Drag::Terminal {
            return terminal_repaint;
        }
        terminal_repaint | self.mouse_move_inner(x, y)
    }

    fn mouse_move_inner(&mut self, x: f32, y: f32) -> bool {
        match self.dragging {
            Drag::Terminal => false,
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
                self.layout
                    .files_view
                    .as_mut()
                    .map(|v| v.update_tab_drag(x, y))
                    .unwrap_or(false)
            }
            Drag::EditorSel => self
                .layout
                .files_view
                .as_mut()
                .map(|v| v.editor_drag(x, y))
                .unwrap_or(false),
            Drag::None => {
                if let Some((id, px, py)) = self.pending_tab {
                    if (x - px).abs() > 5.0 || (y - py).abs() > 5.0 {
                        self.pending_tab = None;
                        if let Some(v) = self.layout.files_view.as_mut() {
                            if v.begin_tab_drag(id) {
                                self.dragging = Drag::Tab;
                                v.update_tab_drag(x, y);
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
                if let Some(v) = self.layout.files_view.as_mut() {
                    changed |= v.editor_hover(x, y);
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
                changed
            }
        }
    }

    /// Left-button press: route a header/menu click, close the menu, toggle a dock, or begin a divider drag.
    pub fn mouse_down(&mut self, x: f32, y: f32) {
        if let Some(rect) = self.modal_rect.take() {
            let inside = x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h;
            if !inside {
                if let Some(v) = self.layout.files_view.as_mut() {
                    v.dismiss_modal();
                }
                return;
            }
            self.modal_rect = Some(rect);
        }
        // A click while a context menu is open either picks an item or dismisses it.
        if let Some((_, _, _, target)) = self.menu {
            let hit = self.hit(x, y);
            if hit.is_some_and(is_submenu) {
                return;
            }
            let mut rows = self.menu_items(target);
            if let Some(parent) = self.submenu {
                rows.extend(self.menu_items(parent.3));
            }
            let row = hit.and_then(|id| rows.iter().find(|row| row.id == id));
            if let Some(row) = row {
                if row.disabled {
                    return;
                }
                self.apply_menu(target, row.id);
            }
            self.menu = None;
            self.submenu = None;
            self.menu_path = None;
            self.menu_editor_anchor = None;
            return;
        }
        let w = self.width();
        if self.layout.on_left_divider(x, y, w) {
            self.dragging = Drag::Left;
        } else if self.layout.on_tree_divider(x, y, w) {
            self.dragging = Drag::Tree;
        } else if self.layout.on_right_divider(x, y, w) {
            self.dragging = Drag::Right;
        } else if self.layout.on_bottom_divider(x, y, w, self.viewport.1) {
            self.dragging = Drag::Bottom;
        } else if let Some(id) = self.hit(x, y) {
            if self
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
            } else {
                self.header_click(id);
            }
        } else if self.terminal_grid_at(x, y) {
            self.set_terminal_focus(true);
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
            let modifiers = terminal_modifiers();
            if let Some(view) = self.layout.terminal_view.as_mut() {
                if view.mouse_down(x, y, count, modifiers) {
                    self.dragging = Drag::Terminal;
                }
            }
        } else {
            self.set_terminal_focus(false);
            let (vw, vh) = self.viewport;
            let cr = self.layout.center_region(vw, vh);
            let in_center = x >= cr.x && x < cr.x + cr.w && y >= cr.y && y < cr.y + cr.h;
            let mut consumed = false;
            if in_center {
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
                    self.layout
                        .files_view
                        .as_mut()
                        .map(|v| v.editor_double_click(x, y))
                        .unwrap_or(false)
                } else {
                    let placed = self
                        .layout
                        .files_view
                        .as_mut()
                        .map(|v| v.editor_click(x, y, false))
                        .unwrap_or(false);
                    if placed {
                        self.dragging = Drag::EditorSel; // drag extends the selection
                    }
                    placed
                };
            }
            if !consumed && self.layout.session_menu {
                self.layout.session_menu = false;
                self.session_search_query.clear();
            }
        }
    }

    pub fn editor_copy_to_clipboard(&mut self) -> bool {
        let copied = self
            .layout
            .files_view
            .as_ref()
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
        let copied = self.layout.files_view.as_mut().and_then(|v| v.editor_cut());
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
        let slices = crate::slices_for(&text);
        self.layout
            .files_view
            .as_mut()
            .map(|v| v.editor_paste(&text, slices.as_deref()))
            .unwrap_or(false)
    }

    pub fn editor_ime_preedit(
        &mut self,
        text: &str,
        selected: Option<std::ops::Range<usize>>,
    ) -> bool {
        self.layout
            .files_view
            .as_mut()
            .map(|v| v.editor_ime_preedit(text, selected))
            .unwrap_or(false)
    }

    pub fn editor_ime_commit(&mut self, text: &str) -> bool {
        self.layout
            .files_view
            .as_mut()
            .map(|v| v.editor_ime_commit(text))
            .unwrap_or(false)
    }

    pub fn editor_text(&mut self, text: &str) -> bool {
        self.layout
            .files_view
            .as_mut()
            .map(|v| v.editor_text(text))
            .unwrap_or(false)
    }

    pub fn editor_key(&mut self, key: EditKey, shift: bool) -> bool {
        let changed = self
            .layout
            .files_view
            .as_mut()
            .map(|v| v.editor_key(key, shift))
            .unwrap_or(false);
        self.show_view_toast();
        changed
    }

    pub fn editor_save(&mut self) -> Option<Result<(), String>> {
        self.layout
            .files_view
            .as_mut()
            .and_then(|v| v.editor_save())
    }

    pub fn refresh_disk_state(&mut self) {
        if let Some(v) = self.layout.files_view.as_mut() {
            v.refresh_disk_state();
        }
    }

    pub fn editor_focused(&self) -> bool {
        !self.terminal_focused
            && self
                .layout
                .files_view
                .as_ref()
                .map(|v| v.editor_focused())
                .unwrap_or(false)
    }

    pub fn editor_selected_text(&self) -> Option<String> {
        self.layout
            .files_view
            .as_ref()
            .and_then(|v| v.editor_selected_text())
    }

    pub fn mouse_up(&mut self) {
        if self.dragging == Drag::Terminal {
            let (x, y) = self.pointer;
            let modifiers = terminal_modifiers();
            if let Some(view) = self.layout.terminal_view.as_mut() {
                view.mouse_up(x, y, modifiers);
            }
        } else if self.dragging == Drag::Tab {
            if let Some(v) = self.layout.files_view.as_mut() {
                v.drop_tab();
            }
        } else if self.dragging != Drag::None {
            self.pending.persist = true;
        } else if let Some((id, _, _)) = self.pending_tab.take() {
            if let Some(v) = self.layout.files_view.as_mut() {
                v.on_click(id);
            }
        }
        self.pending_tab = None;
        self.tab_ghost_at = None;
        self.dragging = Drag::None;
    }

    fn terminal_painted(&mut self, region: Rect) -> Painted {
        let focused = self.terminal_focused;
        match self.layout.terminal_view.as_mut() {
            Some(view) => {
                if view.is_empty() {
                    view.open(None);
                }
                view.render(region, focused)
            }
            None => ui::render(&terminal_dock_body(), region),
        }
    }

    /// Bring every shell's output in before drawing; closes the panel once its last shell exits.
    fn sync_terminals(&mut self) {
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

    fn terminal_grid_at(&self, x: f32, y: f32) -> bool {
        self.layout.terminal_visible()
            && self
                .layout
                .terminal_view
                .as_ref()
                .is_some_and(|view| view.grid_contains(x, y))
    }

    fn set_terminal_focus(&mut self, focused: bool) {
        if self.terminal_focused == focused {
            return;
        }
        self.terminal_focused = focused;
        if let Some(view) = self.layout.terminal_view.as_mut() {
            view.focus_changed(focused);
        }
    }

    pub fn terminal_focused(&self) -> bool {
        self.terminal_focused && self.layout.terminal_visible()
    }

    /// Returns whether the key was consumed; unconsumed keys arrive next as typed text.
    pub fn terminal_key(&mut self, keystroke: &terminal::Keystroke) -> bool {
        let Some(view) = self.layout.terminal_view.as_mut() else {
            return false;
        };
        match view.key(keystroke) {
            crate::TerminalKeyOutcome::Ignored => false,
            crate::TerminalKeyOutcome::Handled => true,
            crate::TerminalKeyOutcome::Copy(text) => {
                Self::clip_set(&text);
                true
            }
            crate::TerminalKeyOutcome::Paste => {
                if let Some(text) = Self::clip_get() {
                    view.paste(&text);
                }
                true
            }
        }
    }

    pub fn terminal_text(&mut self, text: &str) {
        if let Some(view) = self.layout.terminal_view.as_mut() {
            view.text(text);
        }
    }

    fn show_terminal(&mut self) {
        let side = self.layout.terminal_side;
        self.layout.active_panels[side.index()] = Some(Shown::Terminal);
        match side {
            DockPosition::Right => self.layout.right.collapsed = false,
            DockPosition::Bottom => self.layout.bottom.collapsed = false,
            DockPosition::Left => {}
        }
        self.pending.persist = true;
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

    pub fn take_prompt(&mut self) -> Option<crate::Prompt> {
        self.pending_prompt.take()
    }

    pub fn prompt_answered(&mut self, token: u64, answer: usize) {
        if let Some(v) = self.layout.files_view.as_mut() {
            v.prompt_answered(token, answer);
        }
        self.show_view_toast();
    }

    fn show_view_toast(&mut self) {
        let message = self.layout.files_view.as_mut().and_then(|v| v.take_toast());
        if let Some(message) = message {
            self.show_toast(message, None);
        }
    }

    pub fn dragging(&self) -> bool {
        self.dragging != Drag::None
    }

    pub fn scroll(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let over_popover = self
            .popover_rects
            .iter()
            .position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h);
        if let Some(index) = over_popover {
            return self
                .layout
                .files_view
                .as_mut()
                .is_some_and(|v| v.popover_scroll(index, dy));
        }
        // An open modal swallows scrolling over it so the editor underneath stays put.
        if let Some(rect) = self.modal_rect {
            if x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h {
                return self
                    .layout
                    .files_view
                    .as_mut()
                    .is_some_and(|v| v.modal_scroll(dy));
            }
        }
        if !self.layout.session_menu && self.terminal_grid_at(x, y) {
            let modifiers = terminal_modifiers();
            return self
                .layout
                .terminal_view
                .as_mut()
                .is_some_and(|view| view.scroll(x, y, dy, modifiers));
        }
        if !self.layout.session_menu {
            let (w, h) = self.viewport;
            let cr = self.layout.center_region(w, h);
            let over_center = x >= cr.x && x < cr.x + cr.w && y >= cr.y && y < cr.y + cr.h;
            if over_center {
                if let Some(view) = self.layout.files_view.as_mut() {
                    if view.editor_scroll(x, y, dx, dy) {
                        return true;
                    }
                }
            }
            if let Some(side) = self.layout.files_side() {
                let region = match side {
                    DockPosition::Left => self.layout.tree_region(w, h),
                    DockPosition::Right => self.layout.right_region(w, h),
                    DockPosition::Bottom => self.layout.bottom_region(w, h),
                };
                if let Some(view) = self.layout.files_view.as_mut() {
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

    fn header_click(&mut self, id: u64) {
        if id == TOAST_CLOSE || id == TOAST_ACTION {
            self.toast = None;
            return;
        }
        if (crate::TERMINAL_VIEW_BASE..FUNC_VIEW_BASE).contains(&id) {
            self.set_terminal_focus(true);
            if let Some(view) = self.layout.terminal_view.as_mut() {
                view.click(id);
            }
            return;
        }
        self.set_terminal_focus(false);
        if id >= FUNC_VIEW_BASE {
            if let Some(view) = self.layout.files_view.as_mut() {
                view.on_click(id);
            }
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
            let msg = if id == SESSION_NEW {
                "New session - coming soon"
            } else {
                "Open a session - coming soon"
            };
            self.show_toast(msg, Some("Docs".into()));
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
    let mut rects = Vec::new();
    for &(offset, alpha, blur) in shadows {
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
                radius: (8.0 + spread).max(0.0),
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            });
        }
    }
    rects
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

#[cfg(test)]
mod tests {
    use super::*;
    use ui::Application;

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
