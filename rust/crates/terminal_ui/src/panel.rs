//! The terminal panel: a tab bar of shells plus the active shell's grid, driven by the workspace.

use std::path::PathBuf;

use terminal::{
    Keystroke, Modifiers, MouseButton, Palette, Terminal, TerminalAction, TerminalHost,
    TerminalOptions, Waker,
};
use ui::{div, icon, label, theme, IconKind, Node, Painted, Rect, Rgba};
use workspace::{TerminalKeyOutcome, TerminalPanelView, TerminalSyncOutcome, TERMINAL_VIEW_BASE};

use crate::{anchor_to_bottom, GridMetrics, GridOptions, GridPainter, FONT_SIZE, LINE_HEIGHT};

const TAB_H: f32 = 32.0;
const NEW_TERMINAL: u64 = TERMINAL_VIEW_BASE;
const TAB_ACTIVATE_BASE: u64 = TERMINAL_VIEW_BASE + 100;
const TAB_CLOSE_BASE: u64 = TERMINAL_VIEW_BASE + 2000;
const TAB_LIMIT: u64 = 1900;

struct Host<'a> {
    palette: Palette,
    clipboard: &'a dyn Fn() -> Option<String>,
}

impl TerminalHost for Host<'_> {
    fn palette(&self) -> &Palette {
        &self.palette
    }

    fn clipboard_text(&self) -> Option<String> {
        (self.clipboard)()
    }
}

pub struct TerminalPanel {
    root: PathBuf,
    waker: Waker,
    tabs: Vec<Terminal>,
    active: usize,
    painter: GridPainter,
    /// Where the active grid was drawn (logical px) and its cell metrics, for mapping the pointer to cells.
    grid_origin: (f32, f32),
    grid_rect: Rect,
    metrics: GridMetrics,
    hover: Option<u64>,
    focused: bool,
    spawn_error: Option<String>,
}

impl TerminalPanel {
    pub fn new(root: PathBuf, waker: Waker) -> Self {
        Self {
            root,
            waker,
            tabs: Vec::new(),
            active: 0,
            painter: GridPainter::default(),
            grid_origin: (0.0, 0.0),
            grid_rect: Rect::new(0.0, 0.0, 0.0, 0.0, Rgba::TRANSPARENT),
            metrics: GridMetrics::measure(FONT_SIZE, LINE_HEIGHT),
            hover: None,
            focused: false,
            spawn_error: None,
        }
    }

    fn active_terminal(&mut self) -> Option<&mut Terminal> {
        self.tabs.get_mut(self.active)
    }

    /// Pointer position in the active grid's design px.
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let scale = ui::ui_text_scale();
        (
            (x - self.grid_origin.0) / scale,
            (y - self.grid_origin.1) / scale,
        )
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        } else if index < self.active {
            self.active -= 1;
        }
    }

    fn tab_bar(&self) -> Node {
        let mut bar = div().row().h_px(TAB_H).bg(theme().tab_bar_background);
        for (index, terminal) in self.tabs.iter().enumerate() {
            let is_active = index == self.active;
            let activate = TAB_ACTIVATE_BASE + index as u64;
            let close = TAB_CLOSE_BASE + index as u64;
            let hovered = self.hover == Some(activate) || self.hover == Some(close);
            let mut close_slot = div()
                .w_px(16.0)
                .h_px(16.0)
                .rounded(4.0)
                .items_center()
                .justify_center()
                .on_click(close);
            if hovered {
                close_slot =
                    close_slot.child(icon(IconKind::Close).size(11.0).color(theme().icon_muted));
            }
            let text_color = if is_active {
                theme().text
            } else {
                theme().text_muted
            };
            let content = div()
                .row()
                .flex(1.0)
                .px(10.0)
                .gap(6.0)
                .items_center()
                .child(
                    icon(IconKind::Terminal)
                        .size(14.0)
                        .color(theme().icon_muted),
                )
                .child(label(terminal.tab_title(true)).size(13.0).color(text_color))
                .child(close_slot);
            let underline = if is_active {
                theme().terminal_background
            } else {
                theme().border
            };
            let cell = div()
                .col()
                .h_px(TAB_H)
                .bg(if is_active {
                    theme().tab_active_background
                } else {
                    theme().tab_inactive_background
                })
                .on_click(activate)
                .child(content)
                .child(div().h_px(1.0).bg(underline));
            bar = bar
                .child(cell)
                .child(div().w_px(1.0).h_px(TAB_H).bg(theme().border));
        }
        bar.child(
            div()
                .col()
                .flex(1.0)
                .h_px(TAB_H)
                .child(div().flex(1.0))
                .child(div().h_px(1.0).bg(theme().border)),
        )
        .child(div().w_px(1.0).h_px(TAB_H).bg(theme().border))
        .child(
            div()
                .col()
                .w_px(28.0)
                .h_px(TAB_H)
                .child(
                    div()
                        .row()
                        .flex(1.0)
                        .items_center()
                        .justify_center()
                        .on_click(NEW_TERMINAL)
                        .child(icon(IconKind::Plus).size(13.0).color(theme().icon_muted)),
                )
                .child(div().h_px(1.0).bg(theme().border)),
        )
        .into()
    }
}

impl TerminalPanelView for TerminalPanel {
    fn render(&mut self, region: Rect, focused: bool) -> Painted {
        self.focused = focused;
        let scale = ui::ui_text_scale();
        self.metrics = GridMetrics::measure(FONT_SIZE, LINE_HEIGHT);
        let metrics = self.metrics;
        let body_h = (region.h / scale - TAB_H).max(metrics.line_height);
        let bounds = metrics.bounds(region.w / scale, body_h);
        let mut padding_top = 0.0;
        let mut overlays = Vec::new();
        let grid = match self.tabs.get_mut(self.active) {
            Some(terminal) => {
                terminal.set_size(bounds);
                terminal.apply_resize();
                let content = terminal.content();
                if anchor_to_bottom(content) {
                    padding_top = body_h - bounds.height;
                }
                let paint = self.painter.render(
                    content,
                    metrics,
                    &theme(),
                    &GridOptions {
                        focused,
                        cursor_visible: true,
                        minimum_contrast: crate::MINIMUM_CONTRAST,
                    },
                );
                overlays = paint.overlays;
                paint.node
            }
            None => {
                let message = self.spawn_error.clone().unwrap_or_default();
                label(message).size(13.0).color(theme().text_muted).into()
            }
        };
        let tab_bar = self.tab_bar();
        let body = div()
            .col()
            .flex(1.0)
            .bg(theme().terminal_background)
            .pl(metrics.cell_width)
            .child(div().h_px(padding_top))
            .child(grid);
        let node: Node = div().col().child(tab_bar).child(body).into();
        let mut painted = ui::render(&node, region);
        self.grid_origin = (
            region.x + metrics.cell_width * scale,
            region.y + (TAB_H + padding_top) * scale,
        );
        self.grid_rect = Rect::new(
            region.x,
            region.y + TAB_H * scale,
            region.w,
            (region.h - TAB_H * scale).max(0.0),
            Rgba::TRANSPARENT,
        );
        painted.rects.extend(overlays.into_iter().map(|rect| {
            Rect::new(
                self.grid_origin.0 + rect.x * scale,
                self.grid_origin.1 + rect.y * scale,
                rect.w * scale,
                rect.h * scale,
                rect.color,
            )
        }));
        painted
    }

    fn click(&mut self, id: u64) -> bool {
        if id == NEW_TERMINAL {
            self.open(None);
        } else if (TAB_CLOSE_BASE..TAB_CLOSE_BASE + TAB_LIMIT).contains(&id) {
            self.close_tab((id - TAB_CLOSE_BASE) as usize);
        } else if (TAB_ACTIVATE_BASE..TAB_ACTIVATE_BASE + TAB_LIMIT).contains(&id) {
            let index = (id - TAB_ACTIVATE_BASE) as usize;
            if index < self.tabs.len() {
                self.active = index;
            }
        } else {
            return false;
        }
        true
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        let id = id.filter(|id| (TERMINAL_VIEW_BASE..workspace::FUNC_VIEW_BASE).contains(id));
        let changed = self.hover != id;
        self.hover = id;
        changed
    }

    fn grid_contains(&self, x: f32, y: f32) -> bool {
        let rect = self.grid_rect;
        x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
    }

    fn mouse_down(&mut self, x: f32, y: f32, click_count: u32, modifiers: Modifiers) -> bool {
        let (x, y) = self.local(x, y);
        match self.active_terminal() {
            Some(terminal) => {
                terminal.mouse_down(x, y, MouseButton::Left, modifiers, click_count);
                true
            }
            None => false,
        }
    }

    fn mouse_drag(&mut self, x: f32, y: f32, modifiers: Modifiers) -> bool {
        let (x, y) = self.local(x, y);
        match self.active_terminal() {
            Some(terminal) => {
                if terminal.mouse_mode(modifiers.shift) {
                    terminal.mouse_move(x, y, Some(MouseButton::Left), modifiers);
                } else {
                    terminal.mouse_drag(x, y, modifiers);
                }
                true
            }
            None => false,
        }
    }

    fn mouse_move(&mut self, x: f32, y: f32, modifiers: Modifiers) -> bool {
        let (x, y) = self.local(x, y);
        if let Some(terminal) = self.active_terminal() {
            terminal.mouse_move(x, y, None, modifiers);
        }
        false
    }

    fn mouse_up(&mut self, x: f32, y: f32, modifiers: Modifiers) {
        let (x, y) = self.local(x, y);
        if let Some(terminal) = self.active_terminal() {
            terminal.mouse_up(x, y, MouseButton::Left, modifiers);
        }
    }

    fn scroll(&mut self, x: f32, y: f32, delta_y: f32, modifiers: Modifiers) -> bool {
        let scale = ui::ui_text_scale();
        let (x, y) = self.local(x, y);
        match self.active_terminal() {
            Some(terminal) => {
                terminal.scroll_wheel(delta_y / scale, x, y, modifiers);
                true
            }
            None => false,
        }
    }

    fn key(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let Some(terminal) = self.active_terminal() else {
            return TerminalKeyOutcome::Ignored;
        };
        if let Some(action) = terminal::input::binding(keystroke) {
            return match action {
                TerminalAction::Copy => terminal
                    .selection_text()
                    .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy),
                TerminalAction::Paste => TerminalKeyOutcome::Paste,
                action => {
                    terminal.perform(&action);
                    TerminalKeyOutcome::Handled
                }
            };
        }
        if terminal.try_keystroke(keystroke, false) {
            TerminalKeyOutcome::Handled
        } else {
            TerminalKeyOutcome::Ignored
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(terminal) = self.active_terminal() {
            terminal.input(text.as_bytes().to_vec());
        }
    }

    fn paste(&mut self, text: &str) {
        if let Some(terminal) = self.active_terminal() {
            terminal.paste(text);
        }
    }

    fn focus_changed(&mut self, focused: bool) {
        self.focused = focused;
        if let Some(terminal) = self.active_terminal() {
            terminal.focus_changed(focused);
        }
    }

    fn sync(&mut self, clipboard: &dyn Fn() -> Option<String>) -> TerminalSyncOutcome {
        let host = Host {
            palette: crate::palette(&theme()),
            clipboard,
        };
        let mut outcome = TerminalSyncOutcome::default();
        let had_tabs = !self.tabs.is_empty();
        let mut index = 0;
        while index < self.tabs.len() {
            let result = self.tabs[index].sync(&host);
            outcome.changed |= result.changed || result.title_changed;
            if result.clipboard_store.is_some() {
                outcome.clipboard_store = result.clipboard_store;
            }
            if result.close {
                self.close_tab(index);
                outcome.changed = true;
            } else {
                index += 1;
            }
        }
        outcome.closed_all = had_tabs && self.tabs.is_empty();
        outcome
    }

    fn open(&mut self, cwd: Option<PathBuf>) {
        let options = TerminalOptions {
            working_directory: Some(cwd.unwrap_or_else(|| self.root.clone())),
            ..TerminalOptions::default()
        };
        match Terminal::spawn(options, self.waker.clone()) {
            Ok(terminal) => {
                self.tabs.push(terminal);
                self.active = self.tabs.len() - 1;
                self.spawn_error = None;
            }
            Err(error) => self.spawn_error = Some(format!("Failed to start the shell: {error}")),
        }
    }

    fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}
