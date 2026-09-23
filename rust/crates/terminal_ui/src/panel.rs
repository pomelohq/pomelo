//! The terminal panel: a tab bar of shells plus the active shell's grid, driven by the workspace.

use std::path::{Path, PathBuf};

use terminal::{
    HyperlinkMatch, Keystroke, Modifiers, MouseButton, Palette, PathWithPosition, Terminal,
    TerminalAction, TerminalHost, TerminalOptions, Waker,
};
use ui::{div, icon, label, theme, IconKind, Node, Painted, Rect, Rgba};
use workspace::{
    TerminalKeyOutcome, TerminalOpenTarget, TerminalPanelView, TerminalSyncOutcome,
    TERMINAL_VIEW_BASE,
};

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
    /// The link a cmd-press landed on; releasing over the same link opens it.
    pressed_link: Option<HyperlinkMatch>,
    hovered_target: Option<TerminalOpenTarget>,
    open_request: Option<TerminalOpenTarget>,
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
            pressed_link: None,
            hovered_target: None,
            open_request: None,
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

    /// What a link points at: URLs as they are; paths only when they name an existing file, tried as written
    /// and with a position suffix split off, without diff (`a/`, `b/`) or `./` prefixes, against the shell's
    /// directory and then the project root.
    fn resolve(&self, link: &HyperlinkMatch) -> Option<TerminalOpenTarget> {
        if link.is_url {
            return Some(TerminalOpenTarget::Url(link.text.clone()));
        }
        let parsed = PathWithPosition::parse(&link.text);
        let mut candidates = vec![
            (link.text.clone(), None, None),
            (parsed.path, parsed.row, parsed.column),
        ];
        for index in 0..candidates.len() {
            for prefix in ["a/", "b/", "./"] {
                let (path, row, column) = candidates[index].clone();
                if let Some(stripped) = path.strip_prefix(prefix) {
                    candidates.push((stripped.to_string(), row, column));
                }
            }
        }
        let cwd = self
            .tabs
            .get(self.active)
            .and_then(|terminal| terminal.process_info())
            .map(|info| info.cwd.clone())
            .filter(|cwd| !cwd.as_os_str().is_empty());
        let bases: Vec<PathBuf> = cwd.into_iter().chain([self.root.clone()]).collect();
        candidates.into_iter().find_map(|(path, row, column)| {
            let path = Path::new(&path);
            let found = if path.is_absolute() {
                path.is_file().then(|| path.to_path_buf())
            } else {
                bases
                    .iter()
                    .map(|base| base.join(path))
                    .find(|full| full.is_file())
            };
            found.map(|path| TerminalOpenTarget::Path { path, row, column })
        })
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
        if modifiers.cmd {
            let link = self
                .active_terminal()
                .and_then(|terminal| terminal.hyperlink_at(x, y));
            if let Some(link) = link.filter(|link| self.resolve(link).is_some()) {
                self.pressed_link = Some(link);
                return true;
            }
        }
        match self.active_terminal() {
            Some(terminal) => {
                terminal.mouse_down(x, y, MouseButton::Left, modifiers, click_count);
                true
            }
            None => false,
        }
    }

    fn mouse_drag(&mut self, x: f32, y: f32, modifiers: Modifiers) -> bool {
        if self.pressed_link.is_some() {
            return false;
        }
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
        let inside = self.grid_contains(x, y);
        let (x, y) = self.local(x, y);
        let focused = self.focused;
        let link = if modifiers.cmd && inside {
            self.active_terminal()
                .and_then(|terminal| terminal.hyperlink_at(x, y))
        } else {
            None
        };
        let target = link.as_ref().and_then(|link| self.resolve(link));
        let link = link.filter(|_| target.is_some());
        self.hovered_target = target;
        let Some(terminal) = self.active_terminal() else {
            return false;
        };
        if focused && inside && !modifiers.cmd {
            terminal.mouse_move(x, y, None, modifiers);
        }
        terminal.set_hovered_link(link)
    }

    fn mouse_up(&mut self, x: f32, y: f32, modifiers: Modifiers) {
        let (x, y) = self.local(x, y);
        if let Some(pressed) = self.pressed_link.take() {
            let released = self
                .active_terminal()
                .and_then(|terminal| terminal.hyperlink_at(x, y));
            if released.as_ref() == Some(&pressed) {
                self.open_request = self.resolve(&pressed);
            }
            return;
        }
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

    fn take_open_request(&mut self) -> Option<TerminalOpenTarget> {
        self.open_request.take()
    }

    fn link_hovered(&self) -> bool {
        self.hovered_target.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminal::GridPoint;

    fn link(text: &str, is_url: bool) -> HyperlinkMatch {
        HyperlinkMatch {
            text: text.into(),
            is_url,
            start: GridPoint::default(),
            end: GridPoint::default(),
        }
    }

    #[test]
    fn links_resolve_to_existing_files_or_urls() {
        let root = std::env::temp_dir().join(format!("pomelo-links-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).ok();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").ok();
        let panel = TerminalPanel::new(root.clone(), std::sync::Arc::new(|| {}));
        assert_eq!(
            panel.resolve(&link("b/src/main.rs:3:7", false)),
            Some(TerminalOpenTarget::Path {
                path: root.join("src/main.rs"),
                row: Some(3),
                column: Some(7),
            })
        );
        assert_eq!(
            panel.resolve(&link("./src/main.rs", false)),
            Some(TerminalOpenTarget::Path {
                path: root.join("src/main.rs"),
                row: None,
                column: None,
            })
        );
        assert_eq!(panel.resolve(&link("src/missing.rs", false)), None);
        assert_eq!(panel.resolve(&link("src", false)), None);
        assert_eq!(
            panel.resolve(&link("https://example.com", true)),
            Some(TerminalOpenTarget::Url("https://example.com".into()))
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
