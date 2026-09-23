//! The terminal panel: a tab bar of shells plus the active shell's grid, driven by the workspace.

use std::path::{Path, PathBuf};

use editor::search::{Direction, SearchQuery};
use terminal::{
    GridPoint, HyperlinkMatch, Keystroke, Modifiers, MouseButton, Palette, PathWithPosition,
    Terminal, TerminalAction, TerminalHost, TerminalOptions, Waker,
};
use ui::{div, icon, label, theme, IconKind, Node, Painted, Rect, Rgba};
use workspace::search_bar::{SearchBar, SearchClick, SearchField, SearchSupport, Searchable};
use workspace::{
    EditKey, TerminalKeyOutcome, TerminalOpenTarget, TerminalPanelView, TerminalSyncOutcome,
    TERMINAL_VIEW_BASE,
};

use crate::{anchor_to_bottom, GridMetrics, GridOptions, GridPainter, FONT_SIZE, LINE_HEIGHT};

const TAB_H: f32 = 32.0;
const NEW_TERMINAL: u64 = TERMINAL_VIEW_BASE;
const TAB_ACTIVATE_BASE: u64 = TERMINAL_VIEW_BASE + 100;
const TAB_CLOSE_BASE: u64 = TERMINAL_VIEW_BASE + 2000;
const TAB_LIMIT: u64 = 1900;
const SEARCH_BASE: u64 = TERMINAL_VIEW_BASE + 4000;

/// The search bar's view of a terminal: grid points flattened to offsets (row-major from the top of the
/// scrollback), so the shared bar's range logic works unchanged.
struct SearchTarget<'a> {
    terminal: &'a mut Terminal,
    history: usize,
    columns: usize,
}

impl<'a> SearchTarget<'a> {
    fn new(terminal: &'a mut Terminal) -> Self {
        let history = terminal.history_size();
        let columns = terminal.content().columns.max(1);
        Self {
            terminal,
            history,
            columns,
        }
    }

    fn offset(&self, point: GridPoint) -> usize {
        let row = (point.line + self.history as i32).max(0) as usize;
        row * self.columns + point.column
    }

    fn point(&self, offset: usize) -> GridPoint {
        GridPoint {
            line: (offset / self.columns) as i32 - self.history as i32,
            column: offset % self.columns,
        }
    }

    fn range(&self, range: &std::ops::Range<usize>) -> (GridPoint, GridPoint) {
        (
            self.point(range.start),
            self.point(range.end.saturating_sub(1).max(range.start)),
        )
    }
}

/// The pattern for a bar query: regex queries as written, plain text escaped. A lone `.` would match every
/// cell, so it finds nothing.
fn pattern_for(query: &SearchQuery) -> Option<String> {
    let pattern = match query {
        SearchQuery::Text { query, .. } => regex::escape(query),
        SearchQuery::Regex { regex, .. } => regex.as_str().to_string(),
    };
    (pattern != "." && !pattern.is_empty()).then_some(pattern)
}

impl Searchable for SearchTarget<'_> {
    fn search_version(&self) -> u64 {
        self.terminal.content_version()
    }

    fn find(&self, query: &SearchQuery) -> Vec<std::ops::Range<usize>> {
        let Some(pattern) = pattern_for(query) else {
            return Vec::new();
        };
        self.terminal
            .find(&pattern)
            .into_iter()
            .map(|(start, end)| self.offset(start)..self.offset(end) + 1)
            .collect()
    }

    fn query_suggestion(&self) -> String {
        self.terminal.selection_text().unwrap_or_default()
    }

    /// With no selection the search starts from the cursor, so the first hit is the newest one.
    fn single_cursor(&self) -> Option<usize> {
        let point = self.terminal.selection_head().unwrap_or(GridPoint {
            line: self.terminal.content().cursor.line,
            column: self.terminal.content().cursor.column,
        });
        Some(self.offset(point))
    }

    fn activate_match(&mut self, range: std::ops::Range<usize>) {
        let (start, end) = self.range(&range);
        self.terminal.select_range(start, end);
    }

    fn select_matches(&mut self, _ranges: &[std::ops::Range<usize>]) {}

    fn replace_match(&mut self, _query: &SearchQuery, _range: std::ops::Range<usize>) {}

    fn replace_all(&mut self, _query: &SearchQuery, _ranges: &[std::ops::Range<usize>]) {}

    fn set_search_highlights(
        &mut self,
        matches: Vec<std::ops::Range<usize>>,
        _active: Option<usize>,
    ) {
        let matches = matches.iter().map(|range| self.range(range)).collect();
        self.terminal.set_search_matches(matches);
    }
}

/// Keys for the search bar's text field, from a terminal key press.
fn search_edit_key(keystroke: &Keystroke) -> Option<EditKey> {
    let m = keystroke.modifiers;
    Some(match keystroke.key.as_str() {
        "enter" => EditKey::Enter,
        "escape" => EditKey::Escape,
        "tab" => EditKey::Tab,
        "backspace" if m.cmd => EditKey::DeleteToLineStart,
        "backspace" if m.alt => EditKey::DeleteWordLeft,
        "backspace" => EditKey::Backspace,
        "delete" if m.alt => EditKey::DeleteWordRight,
        "delete" => EditKey::Delete,
        "left" if m.cmd => EditKey::LineStart,
        "left" if m.alt => EditKey::WordLeft,
        "left" => EditKey::Left,
        "right" if m.cmd => EditKey::LineEnd,
        "right" if m.alt => EditKey::WordRight,
        "right" => EditKey::Right,
        "home" => EditKey::Home,
        "end" => EditKey::End,
        "up" => EditKey::Up,
        "down" => EditKey::Down,
        "a" if m.cmd => EditKey::SelectAll,
        "z" if m.cmd && m.shift => EditKey::Redo,
        "z" if m.cmd => EditKey::Undo,
        _ => return None,
    })
}

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
    search: SearchBar,
    hovered_target: Option<TerminalOpenTarget>,
    open_request: Option<TerminalOpenTarget>,
    /// The user closed the last tab; reported on the next sync so the panel closes like when shells exit.
    closed_last: bool,
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
            search: SearchBar::with_support(SearchSupport {
                case: false,
                word: false,
                regex: true,
                replace: false,
                select_all: false,
            }),
            hovered_target: None,
            open_request: None,
            closed_last: false,
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

    /// Run `f` with the search bar and the active terminal as its target.
    fn with_search<R>(
        &mut self,
        f: impl FnOnce(&mut SearchBar, &mut SearchTarget) -> R,
    ) -> Option<R> {
        let terminal = self.tabs.get_mut(self.active)?;
        let mut target = SearchTarget::new(terminal);
        Some(f(&mut self.search, &mut target))
    }

    fn search_focused(&self) -> bool {
        !self.search.dismissed && self.search.focus.is_some()
    }

    /// Search commands available whenever the terminal has focus; returns whether `keystroke` was one.
    fn search_command(&mut self, keystroke: &Keystroke) -> bool {
        let m = keystroke.modifiers;
        let only_cmd = m.cmd && !m.alt && !m.ctrl;
        match keystroke.key.as_str() {
            "f" if only_cmd && !m.shift => {
                if self.search.dismissed {
                    self.with_search(|bar, target| bar.deploy(target, false));
                } else {
                    self.search.focus = Some(SearchField::Query);
                    self.search.query.select_all();
                }
                true
            }
            "g" if only_cmd && !self.search.dismissed => {
                let direction = if m.shift {
                    Direction::Prev
                } else {
                    Direction::Next
                };
                self.with_search(|bar, target| bar.select_match(target, direction));
                true
            }
            "x" if m.cmd && m.alt && !m.ctrl && !self.search.dismissed => {
                self.with_search(|bar, target| bar.toggle_option(target, SearchClick::Regex));
                true
            }
            _ => false,
        }
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
        let body_h = (region.h / scale - TAB_H - self.search.height()).max(metrics.line_height);
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
        let search_h = self.search.height();
        let search_bar: Node = if self.search.dismissed {
            div().into()
        } else {
            self.search.render(SEARCH_BASE, region.w / scale)
        };
        let body = div()
            .col()
            .flex(1.0)
            .bg(theme().terminal_background)
            .pl(metrics.cell_width)
            .child(div().h_px(padding_top))
            .child(grid);
        let node: Node = div()
            .col()
            .child(tab_bar)
            .child(search_bar)
            .child(body)
            .into();
        let mut painted = ui::render(&node, region);
        self.grid_origin = (
            region.x + metrics.cell_width * scale,
            region.y + (TAB_H + search_h + padding_top) * scale,
        );
        self.grid_rect = Rect::new(
            region.x,
            region.y + (TAB_H + search_h) * scale,
            region.w,
            (region.h - (TAB_H + search_h) * scale).max(0.0),
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
        if let Some(click) = id
            .checked_sub(SEARCH_BASE)
            .and_then(SearchClick::from_offset)
        {
            match click {
                SearchClick::Query => self.search.focus = Some(SearchField::Query),
                SearchClick::Next | SearchClick::Previous => {
                    let direction = if click == SearchClick::Next {
                        Direction::Next
                    } else {
                        Direction::Prev
                    };
                    self.with_search(|bar, target| bar.select_match(target, direction));
                }
                SearchClick::Close => {
                    self.with_search(|bar, target| bar.dismiss(target));
                }
                SearchClick::Regex => {
                    self.with_search(|bar, target| bar.toggle_option(target, click));
                }
                _ => {}
            }
            return true;
        }
        if id == NEW_TERMINAL {
            self.open(None);
        } else if (TAB_CLOSE_BASE..TAB_CLOSE_BASE + TAB_LIMIT).contains(&id) {
            self.close_tab((id - TAB_CLOSE_BASE) as usize);
            self.closed_last |= self.tabs.is_empty();
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
        self.search.focus = None;
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
        if self.tabs.is_empty() {
            return TerminalKeyOutcome::Ignored;
        }
        if self.search_command(keystroke) {
            return TerminalKeyOutcome::Handled;
        }
        if self.search_focused() {
            let m = keystroke.modifiers;
            if m.cmd && keystroke.key == "v" {
                return TerminalKeyOutcome::Paste;
            }
            if m.cmd && keystroke.key == "c" {
                return self
                    .search
                    .query
                    .selected_text()
                    .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy);
            }
            return match search_edit_key(keystroke) {
                Some(key) => {
                    let keep = self
                        .with_search(|bar, target| bar.key(target, key, m.shift))
                        .unwrap_or(false);
                    if !keep {
                        self.search.focus = None;
                    }
                    TerminalKeyOutcome::Handled
                }
                None if m.cmd || m.ctrl => TerminalKeyOutcome::Handled,
                None => TerminalKeyOutcome::Ignored,
            };
        }
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
        if self.search_focused() {
            self.with_search(|bar, target| bar.input(target, text));
            return;
        }
        if let Some(terminal) = self.active_terminal() {
            terminal.input(text.as_bytes().to_vec());
        }
    }

    fn paste(&mut self, text: &str) {
        if self.search_focused() {
            self.with_search(|bar, target| bar.input(target, text));
            return;
        }
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
        outcome.closed_all =
            (had_tabs && self.tabs.is_empty()) || std::mem::take(&mut self.closed_last);
        if outcome.changed && !self.search.dismissed {
            self.with_search(|bar, target| bar.refresh(target));
        }
        outcome
    }

    fn open(&mut self, cwd: Option<PathBuf>) {
        self.closed_last = false;
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

    #[test]
    fn search_finds_newest_match_first_and_cycles() {
        let root = std::env::temp_dir();
        let mut panel = TerminalPanel::new(root, std::sync::Arc::new(|| {}));
        panel.tabs.push(
            Terminal::spawn(
                TerminalOptions {
                    shell: Some((
                        "/bin/sh".into(),
                        vec![
                            "-c".into(),
                            "printf 'alpha one\\nbeta\\nalpha two'; sleep 5".into(),
                        ],
                    )),
                    ..TerminalOptions::default()
                },
                std::sync::Arc::new(|| {}),
            )
            .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !panel.tabs[0].screen_text().contains("two") {
            assert!(std::time::Instant::now() < deadline, "no output");
            panel.sync(&|| None);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(panel.search_command(&Keystroke::parse("cmd-f")));
        panel.text("alpha");
        assert_eq!(panel.search.matches.len(), 2);
        assert_eq!(panel.search.active_match, Some(1));
        assert_eq!(panel.tabs[0].selection_text().as_deref(), Some("alpha"));
        let highlighted = &panel.tabs[0].content().search_matches;
        assert_eq!(highlighted.len(), 2);
        assert_eq!(highlighted[1].0.line - highlighted[0].0.line, 2);
        assert!(panel.search_command(&Keystroke::parse("cmd-g")));
        assert_eq!(panel.search.active_match, Some(0));
        assert_eq!(
            panel.key(&Keystroke::parse("escape")),
            TerminalKeyOutcome::Handled
        );
        assert!(panel.search.dismissed);
        assert!(panel.tabs[0].content().search_matches.is_empty());
    }

    #[test]
    fn closing_the_last_tab_closes_the_panel() {
        let mut panel = TerminalPanel::new(std::env::temp_dir(), std::sync::Arc::new(|| {}));
        panel.open(None);
        panel.open(None);
        assert!(panel.click(TAB_CLOSE_BASE));
        assert!(!panel.sync(&|| None).closed_all);
        assert!(panel.click(TAB_CLOSE_BASE));
        assert!(panel.is_empty());
        assert!(panel.sync(&|| None).closed_all);
        assert!(!panel.sync(&|| None).closed_all);
    }
}
