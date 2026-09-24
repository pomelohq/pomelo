//! A terminal as a pane item: one shell, its drawn grid, and the pointer state that belongs to it (link under
//! a cmd-press, the link being hovered). It can sit in any pane: the terminal panel's today, the center's later.

use std::path::{Path, PathBuf};

use editor::search::SearchQuery;
use terminal::{
    GridPoint, HyperlinkMatch, Keystroke, Modifiers, MouseButton, Palette, PathWithPosition,
    SyncOutcome, Terminal, TerminalAction, TerminalHost, TerminalOptions, Waker,
};
use ui::{div, theme, IconKind, Node, Painted, Rect, Rgba};
use workspace::search_bar::{SearchSupport, Searchable};
use workspace::{ClipboardSlice, Item, ItemTick, TerminalKeyOutcome, TerminalOpenTarget};

use crate::{anchor_to_bottom, GridMetrics, GridOptions, GridPainter, FONT_SIZE, LINE_HEIGHT};

pub(crate) struct Host<'a> {
    pub(crate) palette: Palette,
    pub(crate) clipboard: &'a dyn Fn() -> Option<String>,
}

impl TerminalHost for Host<'_> {
    fn palette(&self) -> &Palette {
        &self.palette
    }

    fn clipboard_text(&self) -> Option<String> {
        (self.clipboard)()
    }
}

pub struct TerminalItem {
    id: u64,
    root: PathBuf,
    /// Where the shell started, saved when the shell's current directory is not known yet.
    start_dir: PathBuf,
    terminal: Terminal,
    painter: GridPainter,
    /// Where the grid was last drawn (logical px), for mapping the pointer to cells.
    grid_origin: (f32, f32),
    body: Rect,
    /// The link a cmd-press landed on; releasing over the same link opens it.
    pressed_link: Option<HyperlinkMatch>,
    hovered_target: Option<TerminalOpenTarget>,
    open_request: Option<TerminalOpenTarget>,
    console: Option<Console>,
}

struct Console {
    item_id: String,
    title: String,
    /// A service's output stays readable after it exits; an agent's tab goes when the agent quits.
    keep_after_exit: bool,
}

/// What a link points at: URLs as they are; paths only when they name an existing file, tried as written and
/// with a position suffix split off, without diff (`a/`, `b/`) or `./` prefixes, against the shell's directory
/// and then the project root.
pub fn resolve_link(
    link: &HyperlinkMatch,
    cwd: Option<&Path>,
    root: &Path,
) -> Option<TerminalOpenTarget> {
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
    let bases: Vec<&Path> = cwd
        .filter(|cwd| !cwd.as_os_str().is_empty())
        .into_iter()
        .chain([root])
        .collect();
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

/// The pattern for a find-bar query: regex queries as written, plain text escaped. A lone `.` would match
/// every cell, so it finds nothing.
fn pattern_for(query: &SearchQuery) -> Option<String> {
    let pattern = match query {
        SearchQuery::Text { query, .. } => regex::escape(query),
        SearchQuery::Regex { regex, .. } => regex.as_str().to_string(),
    };
    (pattern != "." && !pattern.is_empty()).then_some(pattern)
}

fn contains(rect: &Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

impl TerminalItem {
    pub fn spawn(
        id: u64,
        root: PathBuf,
        cwd: Option<PathBuf>,
        waker: Waker,
        holder: Option<terminal::HolderOptions>,
    ) -> anyhow::Result<Self> {
        let start_dir = cwd.unwrap_or_else(|| root.clone());
        let options = TerminalOptions {
            working_directory: Some(start_dir.clone()),
            holder,
            ..TerminalOptions::default()
        };
        let mut item = Self::with_terminal(id, root, Terminal::spawn(options, waker)?);
        item.start_dir = start_dir;
        Ok(item)
    }

    pub fn service_console(
        id: u64,
        root: PathBuf,
        item_id: String,
        title: String,
        holder: terminal::HolderOptions,
        waker: Waker,
    ) -> anyhow::Result<Self> {
        let options = TerminalOptions {
            working_directory: Some(root.clone()),
            holder: Some(terminal::HolderOptions {
                attach_only: true,
                ..holder
            }),
            ..TerminalOptions::default()
        };
        let mut item = Self::with_terminal(id, root, Terminal::spawn(options, waker)?);
        item.console = Some(Console {
            item_id,
            title,
            keep_after_exit: true,
        });
        Ok(item)
    }

    /// A coding agent running `argv` in its own holder, started when not running yet. Closing the tab
    /// leaves the agent running; the tab closes when the agent exits.
    pub fn agent(
        id: u64,
        root: PathBuf,
        item_id: String,
        title: String,
        holder: terminal::HolderOptions,
        argv: Vec<String>,
        waker: Waker,
    ) -> anyhow::Result<Self> {
        let mut argv = argv.into_iter();
        let program = argv.next().unwrap_or_else(|| "zsh".to_string());
        let options = TerminalOptions {
            shell: Some((program, argv.collect())),
            working_directory: Some(root.clone()),
            holder: Some(holder),
            ..TerminalOptions::default()
        };
        let mut item = Self::with_terminal(id, root, Terminal::spawn(options, waker)?);
        item.console = Some(Console {
            item_id,
            title,
            keep_after_exit: false,
        });
        Ok(item)
    }

    pub fn service_log(
        id: u64,
        root: PathBuf,
        item_id: String,
        title: String,
        log: &Path,
        waker: Waker,
    ) -> anyhow::Result<Self> {
        let args = vec!["-n".into(), "+2".into(), log.to_string_lossy().into_owned()];
        Self::command_output(
            id,
            root,
            item_id,
            title,
            "/usr/bin/tail".into(),
            args,
            waker,
        )
    }

    /// A read-only tab running `program` (a log follower, ...); it stays open after the program ends.
    pub fn command_output(
        id: u64,
        root: PathBuf,
        item_id: String,
        title: String,
        program: String,
        args: Vec<String>,
        waker: Waker,
    ) -> anyhow::Result<Self> {
        let options = TerminalOptions {
            shell: Some((program, args)),
            working_directory: Some(root.clone()),
            ..TerminalOptions::default()
        };
        let mut item = Self::with_terminal(id, root, Terminal::spawn(options, waker)?);
        item.console = Some(Console {
            item_id,
            title,
            keep_after_exit: true,
        });
        Ok(item)
    }

    pub fn with_terminal(id: u64, root: PathBuf, terminal: Terminal) -> Self {
        Self {
            id,
            start_dir: root.clone(),
            root,
            terminal,
            painter: GridPainter::default(),
            grid_origin: (0.0, 0.0),
            body: Rect::new(0.0, 0.0, 0.0, 0.0, Rgba::TRANSPARENT),
            pressed_link: None,
            hovered_target: None,
            open_request: None,
            console: None,
        }
    }

    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub fn body(&self) -> Rect {
        self.body
    }

    pub fn body_contains(&self, x: f32, y: f32) -> bool {
        contains(&self.body, x, y)
    }

    /// Size the grid to `body` (logical px) and draw it: the terminal background, one cell of left gutter, the
    /// grid hugging the bottom when the screen is full, and overlay rects (block glyphs, bar cursor, links).
    pub fn paint(&mut self, body: Rect, focused: bool) -> Painted {
        self.body = body;
        let scale = ui::ui_text_scale();
        let metrics = GridMetrics::measure(FONT_SIZE, LINE_HEIGHT);
        let body_h = (body.h / scale).max(metrics.line_height);
        self.terminal
            .set_size(metrics.bounds(body.w / scale, body_h));
        self.terminal.apply_resize();
        let content = self.terminal.content();
        let padding_top = if anchor_to_bottom(content) {
            body_h - content.bounds.height
        } else {
            0.0
        };
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
        let node: Node = div()
            .col()
            .flex(1.0)
            .bg(theme().terminal_background)
            .pl(metrics.cell_width)
            .child(div().h_px(padding_top))
            .child(paint.node)
            .into();
        let mut painted = ui::render(&node, body);
        self.grid_origin = (
            body.x + metrics.cell_width * scale,
            body.y + padding_top * scale,
        );
        let origin = self.grid_origin;
        painted.rects.extend(paint.overlays.into_iter().map(|rect| {
            Rect::new(
                origin.0 + rect.x * scale,
                origin.1 + rect.y * scale,
                rect.w * scale,
                rect.h * scale,
                rect.color,
            )
        }));
        painted
    }

    /// Pointer position in the grid's design px.
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let scale = ui::ui_text_scale();
        (
            (x - self.grid_origin.0) / scale,
            (y - self.grid_origin.1) / scale,
        )
    }

    fn resolve(&self, link: &HyperlinkMatch) -> Option<TerminalOpenTarget> {
        let cwd = self.terminal.process_info().map(|info| info.cwd.as_path());
        resolve_link(link, cwd, &self.root)
    }

    pub fn mouse_down(&mut self, x: f32, y: f32, click_count: u32, modifiers: Modifiers) {
        let (x, y) = self.local(x, y);
        if modifiers.cmd {
            let link = self.terminal.hyperlink_at(x, y);
            if let Some(link) = link.filter(|link| self.resolve(link).is_some()) {
                self.pressed_link = Some(link);
                return;
            }
        }
        self.terminal
            .mouse_down(x, y, MouseButton::Left, modifiers, click_count);
    }

    pub fn mouse_drag(&mut self, x: f32, y: f32, modifiers: Modifiers) -> bool {
        if self.pressed_link.is_some() {
            return false;
        }
        let (x, y) = self.local(x, y);
        if self.terminal.mouse_mode(modifiers.shift) {
            self.terminal
                .mouse_move(x, y, Some(MouseButton::Left), modifiers);
        } else {
            self.terminal.mouse_drag(x, y, modifiers);
        }
        true
    }

    /// Pointer motion: with cmd held, find the link under it (only ones a click could open); otherwise report
    /// motion to mouse-tracking programs when focused. Returns whether the hovered link changed.
    pub fn mouse_move(&mut self, x: f32, y: f32, modifiers: Modifiers, focused: bool) -> bool {
        let inside = self.body_contains(x, y);
        let (x, y) = self.local(x, y);
        let link = if modifiers.cmd && inside {
            self.terminal.hyperlink_at(x, y)
        } else {
            None
        };
        let target = link.as_ref().and_then(|link| self.resolve(link));
        let link = link.filter(|_| target.is_some());
        self.hovered_target = target;
        if focused && inside && !modifiers.cmd {
            self.terminal.mouse_move(x, y, None, modifiers);
        }
        self.terminal.set_hovered_link(link)
    }

    pub fn mouse_up(&mut self, x: f32, y: f32, modifiers: Modifiers) {
        let (x, y) = self.local(x, y);
        if let Some(pressed) = self.pressed_link.take() {
            if self.terminal.hyperlink_at(x, y).as_ref() == Some(&pressed) {
                self.open_request = self.resolve(&pressed);
            }
            return;
        }
        self.terminal.mouse_up(x, y, MouseButton::Left, modifiers);
    }

    pub fn scroll(&mut self, x: f32, y: f32, delta_y: f32, modifiers: Modifiers) {
        let scale = ui::ui_text_scale();
        let (x, y) = self.local(x, y);
        self.terminal.scroll_wheel(delta_y / scale, x, y, modifiers);
    }

    /// The terminal's own bindings, then the key encoded for the program.
    pub fn key(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        if let Some(action) = terminal::input::binding(keystroke) {
            return match action {
                TerminalAction::Copy => self
                    .terminal
                    .selection_text()
                    .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy),
                TerminalAction::Paste => TerminalKeyOutcome::Paste,
                action => {
                    self.terminal.perform(&action);
                    TerminalKeyOutcome::Handled
                }
            };
        }
        if self.terminal.try_keystroke(keystroke, false) {
            TerminalKeyOutcome::Handled
        } else {
            TerminalKeyOutcome::Ignored
        }
    }

    pub fn text(&mut self, text: &str) {
        self.terminal.input(text.as_bytes().to_vec());
    }

    pub fn paste(&mut self, text: &str) {
        self.terminal.paste(text);
    }

    pub fn focus_changed(&mut self, focused: bool) {
        self.terminal.focus_changed(focused);
    }

    pub fn sync(&mut self, host: &dyn TerminalHost) -> SyncOutcome {
        self.terminal.sync(host)
    }

    pub fn take_link_request(&mut self) -> Option<TerminalOpenTarget> {
        self.open_request.take()
    }

    pub fn hovering_link(&self) -> bool {
        self.hovered_target.is_some()
    }

    /// Grid points flattened row-major from the top of the scrollback, so the shared find bar's offset ranges
    /// work unchanged.
    fn offset(&self, point: GridPoint) -> usize {
        let history = self.terminal.history_size() as i32;
        let columns = self.terminal.content().columns.max(1);
        let row = (point.line + history).max(0) as usize;
        row * columns + point.column
    }

    fn point(&self, offset: usize) -> GridPoint {
        let history = self.terminal.history_size() as i32;
        let columns = self.terminal.content().columns.max(1);
        GridPoint {
            line: (offset / columns) as i32 - history,
            column: offset % columns,
        }
    }

    fn grid_range(&self, range: &std::ops::Range<usize>) -> (GridPoint, GridPoint) {
        (
            self.point(range.start),
            self.point(range.end.saturating_sub(1).max(range.start)),
        )
    }
}

impl Searchable for TerminalItem {
    fn supported_options(&self) -> SearchSupport {
        SearchSupport {
            case: false,
            word: false,
            regex: true,
            replace: false,
            select_all: false,
        }
    }

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
        let (start, end) = self.grid_range(&range);
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
        let matches = matches.iter().map(|range| self.grid_range(range)).collect();
        self.terminal.set_search_matches(matches);
    }
}

pub(crate) const TERMINAL_KIND: &str = "terminal";

/// The directory a saved terminal was in, to start its replacement shell there.
pub(crate) fn saved_cwd(item: &workspace::persistence::SerializedItem) -> Option<PathBuf> {
    (item.kind == TERMINAL_KIND)
        .then(|| item.data.get("cwd")?.as_str().map(PathBuf::from))
        .flatten()
}

pub(crate) fn saved_holder(item: &workspace::persistence::SerializedItem) -> Option<String> {
    (item.kind == TERMINAL_KIND)
        .then(|| item.data.get("holder")?.as_str().map(str::to_string))
        .flatten()
}

impl Item for TerminalItem {
    fn serialize(&self) -> Option<workspace::persistence::SerializedItem> {
        if self.console.is_some() {
            return None;
        }
        let cwd = self
            .terminal
            .process_info()
            .map_or_else(|| self.start_dir.clone(), |info| info.cwd.clone());
        let holder = self.terminal.holder().map(|holder| holder.name.clone());
        Some(workspace::persistence::SerializedItem {
            kind: TERMINAL_KIND.into(),
            data: serde_json::json!({ "cwd": cwd, "holder": holder }),
        })
    }

    fn closed(&mut self) {
        if self.console.is_none() {
            self.terminal.terminate();
        }
    }

    fn id(&self) -> Option<String> {
        Some(match &self.console {
            Some(console) => console.item_id.clone(),
            None => format!("terminal:{}", self.id),
        })
    }

    fn title(&self) -> String {
        match &self.console {
            Some(console) => console.title.clone(),
            None => self.terminal.tab_title(true),
        }
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Terminal)
    }

    fn body_background(&self) -> Rgba {
        theme().terminal_background
    }

    fn searchable(&mut self) -> Option<&mut dyn Searchable> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn render(&mut self) -> Node {
        div().flex(1.0).bg(theme().terminal_background).into()
    }

    fn set_focused(&mut self, focused: bool) {
        if focused {
            self.terminal.make_primary();
        }
        self.focus_changed(focused);
    }

    fn input_text(&mut self, text: &str) {
        self.text(text);
    }

    fn paste(&mut self, text: &str, _slices: Option<&[ClipboardSlice]>) {
        TerminalItem::paste(self, text);
    }

    fn paint_body(&mut self, body: Rect, focused: bool) -> Option<Painted> {
        Some(self.paint(body, focused))
    }

    fn wants_keystrokes(&self) -> bool {
        true
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        self.key(keystroke)
    }

    fn pointer_down(&mut self, x: f32, y: f32, click_count: u32, modifiers: Modifiers) -> bool {
        if !self.body_contains(x, y) {
            return false;
        }
        self.mouse_down(x, y, click_count, modifiers);
        true
    }

    fn pointer_drag(&mut self, x: f32, y: f32, modifiers: Modifiers) -> bool {
        self.mouse_drag(x, y, modifiers)
    }

    fn pointer_move(&mut self, x: f32, y: f32, modifiers: Modifiers, focused: bool) -> bool {
        self.mouse_move(x, y, modifiers, focused)
    }

    fn pointer_up(&mut self, x: f32, y: f32, modifiers: Modifiers) {
        self.mouse_up(x, y, modifiers);
    }

    fn pointer_scroll(&mut self, x: f32, y: f32, delta_y: f32, modifiers: Modifiers) -> bool {
        if !self.body_contains(x, y) {
            return false;
        }
        self.scroll(x, y, delta_y, modifiers);
        true
    }

    fn tick(&mut self, clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let host = Host {
            palette: crate::palette(&theme()),
            clipboard,
        };
        let result = self.sync(&host);
        ItemTick {
            changed: result.changed || result.title_changed,
            clipboard_store: result.clipboard_store,
            close: result.close
                && !self
                    .console
                    .as_ref()
                    .is_some_and(|console| console.keep_after_exit),
        }
    }

    fn take_open_request(&mut self) -> Option<TerminalOpenTarget> {
        self.take_link_request()
    }

    fn link_hovered(&self) -> bool {
        self.hovering_link()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            resolve_link(&link("b/src/main.rs:3:7", false), None, &root),
            Some(TerminalOpenTarget::Path {
                path: root.join("src/main.rs"),
                row: Some(3),
                column: Some(7),
            })
        );
        assert_eq!(
            resolve_link(&link("./main.rs", false), Some(&root.join("src")), &root),
            Some(TerminalOpenTarget::Path {
                path: root.join("src/main.rs"),
                row: None,
                column: None,
            })
        );
        assert_eq!(
            resolve_link(&link("src/missing.rs", false), None, &root),
            None
        );
        assert_eq!(resolve_link(&link("src", false), None, &root), None);
        assert_eq!(
            resolve_link(&link("https://example.com", true), None, &root),
            Some(TerminalOpenTarget::Url("https://example.com".into()))
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
