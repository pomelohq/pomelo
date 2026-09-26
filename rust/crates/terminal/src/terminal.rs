//! Terminal core: a shell running on a PTY, its output parsed by a VT emulator into a scrollback grid, and a
//! snapshot of that grid for a view to draw. No rendering here; `terminal_ui` draws `Content`.

mod holder;
pub mod hyperlinks;
pub mod input;
pub mod mouse;
pub mod pty_info;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as BackendEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as BackendPoint, Side as BackendSide};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;

pub use alacritty_terminal::term::cell::Flags;
pub use alacritty_terminal::term::TermMode;
pub use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
pub use holder::HolderOptions;
pub use hyperlinks::{HyperlinkMatch, PathWithPosition};
pub use input::{Keystroke, Modifiers, TerminalAction};
pub use mouse::{GridPoint, MouseButton, Side};

use mouse::grid_point_and_side;

/// Pointer travel (px) before a press becomes a selection drag, so a jittery click selects nothing.
const SELECTION_DRAG_THRESHOLD: f32 = 2.0;
/// How often output may trigger re-reading the foreground process for the tab title.
const PROCESS_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

pub const DEFAULT_SCROLL_HISTORY_LINES: usize = 10_000;
pub const MAX_SCROLL_HISTORY_LINES: usize = 100_000;
/// Index space of the emulator's color table: 256 indexed colors plus the named extras (foreground,
/// background, cursor, dim variants, bright foreground, dim background).
pub const COLOR_COUNT: usize = 269;

pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The pixel box the grid is laid out in and the size of one cell; the grid is as many whole cells as fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalBounds {
    pub cell_width: f32,
    pub line_height: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for TerminalBounds {
    fn default() -> Self {
        Self {
            cell_width: 5.0,
            line_height: 5.0,
            width: 500.0,
            height: 30.0,
        }
    }
}

impl TerminalBounds {
    /// `next_up` keeps `N * h / h` from landing just under N and losing a row to scrollback.
    pub fn num_lines(&self) -> usize {
        ((self.height / self.line_height).next_up().floor() as usize).max(1)
    }

    pub fn num_columns(&self) -> usize {
        ((self.width / self.cell_width).next_up().floor() as usize).max(1)
    }

    fn normalized(mut self) -> Self {
        self.height = self.height.max(self.line_height);
        self.width = self.width.max(self.cell_width);
        self
    }

    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.num_lines() as u16,
            num_cols: self.num_columns() as u16,
            cell_width: self.cell_width as u16,
            cell_height: self.line_height as u16,
        }
    }
}

impl Dimensions for TerminalBounds {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines()
    }

    fn columns(&self) -> usize {
        self.num_columns()
    }
}

/// The theme's terminal colors, used to answer color queries from programs (and by the view to paint).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub dim: [Rgb; 8],
    pub foreground: Rgb,
    pub background: Rgb,
    pub bright_foreground: Rgb,
    pub cursor: Rgb,
}

impl Palette {
    /// The color for an emulator color index: the 16 theme colors, the xterm 6x6x6 cube and grayscale ramp,
    /// then the named extras in the emulator's order.
    pub fn color(&self, index: usize) -> Rgb {
        let gray = |value: u8| Rgb {
            r: value,
            g: value,
            b: value,
        };
        let step = |level: u8| if level == 0 { 0 } else { level * 40 + 55 };
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let cube = (index - 16) as u8;
                Rgb {
                    r: step(cube / 36),
                    g: step((cube % 36) / 6),
                    b: step(cube % 6),
                }
            }
            232..=255 => gray((index - 232) as u8 * 10 + 8),
            256 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            259..=266 => self.dim[index - 259],
            267 => self.bright_foreground,
            268 => self.ansi[0],
            _ => gray(0),
        }
    }
}

/// What the app must provide while syncing: theme colors and the system clipboard.
pub trait TerminalHost {
    fn palette(&self) -> &Palette;
    fn clipboard_text(&self) -> Option<String>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexedCell {
    pub line: i32,
    pub column: usize,
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
    pub zerowidth: Vec<char>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub line: i32,
    pub column: usize,
    pub shape: CursorShape,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            line: 0,
            column: 0,
            shape: CursorShape::Block,
        }
    }
}

/// A snapshot of the visible grid, taken on each sync so drawing never holds the emulator lock.
#[derive(Clone, Debug, Default)]
pub struct Content {
    pub cells: Vec<IndexedCell>,
    pub cursor: Cursor,
    pub mode: TermMode,
    pub display_offset: usize,
    pub total_lines: usize,
    pub screen_lines: usize,
    pub columns: usize,
    pub selection: Option<SelectionRange>,
    /// Search results, inclusive, highlighted until the search closes.
    pub search_matches: Vec<(GridPoint, GridPoint)>,
    /// The link under the pointer while the modifier is held, drawn underlined.
    pub hovered_link: Option<HyperlinkMatch>,
    /// Colors a program redefined (OSC 4/10/11); `None` falls back to the palette.
    pub colors: Vec<Option<Rgb>>,
    pub bounds: TerminalBounds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: GridPoint,
    pub end: GridPoint,
    pub is_block: bool,
}

impl SelectionRange {
    pub fn contains(&self, point: GridPoint) -> bool {
        if self.is_block {
            return (self.start.line..=self.end.line).contains(&point.line)
                && (self.start.column..=self.end.column).contains(&point.column);
        }
        (self.start.line, self.start.column) <= (point.line, point.column)
            && (point.line, point.column) <= (self.end.line, self.end.column)
    }
}

/// What a sync produced that the host has to act on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub changed: bool,
    pub clipboard_store: Option<String>,
    pub bell: bool,
    pub title_changed: bool,
    pub close: bool,
}

#[derive(Clone)]
struct Listener {
    events: mpsc::Sender<BackendEvent>,
    waker: Waker,
}

impl EventListener for Listener {
    fn send_event(&self, event: BackendEvent) {
        if self.events.send(event).is_ok() {
            (self.waker)();
        }
    }
}

pub struct TerminalOptions {
    /// `None` runs the user's login shell.
    pub shell: Option<(String, Vec<String>)>,
    pub working_directory: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub scroll_history: usize,
    pub cursor_shape: CursorShape,
    pub holder: Option<HolderOptions>,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            shell: None,
            working_directory: None,
            env: HashMap::new(),
            scroll_history: DEFAULT_SCROLL_HISTORY_LINES,
            cursor_shape: CursorShape::Block,
            holder: None,
        }
    }
}

enum Backend {
    Local(Notifier),
    Holder(holder::HolderBackend),
}

impl Backend {
    fn notify(&mut self, bytes: impl Into<Cow<'static, [u8]>>) {
        match self {
            Backend::Local(notifier) => notifier.notify(bytes),
            Backend::Holder(holder) => holder.write(&bytes.into()),
        }
    }

    fn resize(&mut self, size: WindowSize) {
        match self {
            Backend::Local(notifier) => {
                if let Err(error) = notifier.0.send(Msg::Resize(size)) {
                    eprintln!("terminal resize: {error}");
                }
            }
            Backend::Holder(holder) => holder.resize(size),
        }
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Backend,
    events: mpsc::Receiver<BackendEvent>,
    pending_resize: Option<TerminalBounds>,
    pending_scroll_to_bottom: bool,
    content: Content,
    title: String,
    keyboard_input_sent: bool,
    child_exit: Option<ExitStatus>,
    process: pty_info::PtyProcessInfo,
    links: hyperlinks::LinkSearch,
    search_matches: Vec<(GridPoint, GridPoint)>,
    /// Bumped whenever the grid's text may have changed, so cached searches know to rerun.
    content_version: u64,
    hovered_link: Option<HyperlinkMatch>,
    process_checked: Option<Instant>,
    scroll_px: f32,
    selecting: bool,
    mouse_down_position: Option<(f32, f32)>,
    last_mouse: Option<(GridPoint, Side)>,
}

/// The variables every shell gets: a known terminal type with truecolor, a UTF-8 locale when the app was
/// launched without one (as a macOS bundle is), and no inherited SHLVL so the shell starts at level 1.
pub fn terminal_env(mut env: HashMap<String, String>) -> HashMap<String, String> {
    env.remove("SHLVL");
    if std::env::var("LANG").is_err() {
        env.entry("LANG".to_string())
            .or_insert_with(|| "en_US.UTF-8".to_string());
    }
    env.insert("TERM_PROGRAM".to_string(), "pomelo".to_string());
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    env.insert(
        "TERM_PROGRAM_VERSION".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    env
}

impl Terminal {
    pub fn spawn(options: TerminalOptions, waker: Waker) -> anyhow::Result<Self> {
        let config = Config {
            scrolling_history: options.scroll_history.min(MAX_SCROLL_HISTORY_LINES),
            default_cursor_style: alacritty_terminal::vte::ansi::CursorStyle {
                shape: options.cursor_shape,
                blinking: false,
            },
            ..Config::default()
        };
        let (events_tx, events) = mpsc::channel();
        let listener = Listener {
            events: events_tx,
            waker,
        };
        let bounds = TerminalBounds::default();
        let term = Arc::new(FairMutex::new(Term::new(config, &bounds, listener.clone())));
        if let Some(holder_options) = options.holder {
            let env: Vec<(String, String)> = terminal_env(options.env).into_iter().collect();
            let argv = match options.shell {
                Some((program, args)) => std::iter::once(program).chain(args).collect(),
                None => login_shell_argv(),
            };
            let cwd = options
                .working_directory
                .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("/"));
            let size = bounds.window_size();
            let spawn = pom_ptyhost::SpawnRequest {
                binary: &holder_options.binary,
                name: &holder_options.name,
                cwd: &cwd,
                cols: size.num_cols,
                rows: size.num_lines,
                argv: &argv,
                env: &env,
            };
            let backend = holder::HolderBackend::attach(
                holder_options.clone(),
                spawn,
                term.clone(),
                listener,
            )?;
            let process = pty_info::PtyProcessInfo::for_shell(backend.shell_pid.unwrap_or(0));
            return Ok(Self::with_backend(
                term,
                Backend::Holder(backend),
                events,
                bounds,
                process,
            ));
        }
        let pty_options = tty::Options {
            shell: options
                .shell
                .map(|(program, args)| tty::Shell::new(program, args)),
            working_directory: options.working_directory,
            drain_on_exit: true,
            env: terminal_env(options.env),
        };
        let pty = tty::new(&pty_options, bounds.window_size(), 0)?;
        let process = {
            use std::os::fd::AsRawFd;
            pty_info::PtyProcessInfo::new(pty.file().as_raw_fd(), pty.child().id())
        };
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let pty = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Self::with_backend(
            term,
            Backend::Local(pty),
            events,
            bounds,
            process,
        ))
    }

    fn with_backend(
        term: Arc<FairMutex<Term<Listener>>>,
        pty: Backend,
        events: mpsc::Receiver<BackendEvent>,
        bounds: TerminalBounds,
        process: pty_info::PtyProcessInfo,
    ) -> Self {
        Self {
            term,
            pty,
            events,
            pending_resize: None,
            pending_scroll_to_bottom: false,
            content: Content {
                bounds,
                ..Content::default()
            },
            title: String::new(),
            keyboard_input_sent: false,
            child_exit: None,
            process,
            links: hyperlinks::LinkSearch::default(),
            search_matches: Vec::new(),
            content_version: 0,
            hovered_link: None,
            process_checked: None,
            scroll_px: 0.0,
            selecting: false,
            mouse_down_position: None,
            last_mouse: None,
        }
    }

    pub fn holder(&self) -> Option<&HolderOptions> {
        match &self.pty {
            Backend::Holder(holder) => Some(&holder.options),
            Backend::Local(_) => None,
        }
    }

    pub fn terminate(&mut self) {
        if let Backend::Holder(holder) = &self.pty {
            let (dir, name) = (holder.options.dir.clone(), holder.options.name.clone());
            holder.detach();
            std::thread::spawn(move || {
                if let Err(error) = dir.kill_holder(&name) {
                    eprintln!("terminal holder kill: {error}");
                }
            });
        }
    }

    pub fn make_primary(&mut self) {
        if let Backend::Holder(holder) = &mut self.pty {
            holder.make_primary();
        }
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The tab title: the foreground process and its directory, or "Terminal" before it is known.
    pub fn tab_title(&self, truncate: bool) -> String {
        self.process
            .current
            .as_ref()
            .map(|info| pty_info::title_for(info, truncate))
            .unwrap_or_else(|| "Terminal".to_string())
    }

    pub fn process_info(&self) -> Option<&pty_info::ProcessInfo> {
        self.process.current.as_ref()
    }

    pub fn child_exit(&self) -> Option<ExitStatus> {
        self.child_exit
    }

    /// Queue a resize; pixel-level changes that keep the same grid are dropped so dragging a divider doesn't
    /// flood the shell with SIGWINCH reflows.
    pub fn set_size(&mut self, bounds: TerminalBounds) {
        let bounds = bounds.normalized();
        let old = self.pending_resize.unwrap_or(self.content.bounds);
        let same_grid = old.num_lines() == bounds.num_lines()
            && old.num_columns() == bounds.num_columns()
            && old.cell_width == bounds.cell_width
            && old.line_height == bounds.line_height;
        if !same_grid {
            self.pending_resize = Some(bounds);
        }
    }

    /// Keyboard (or pasted) bytes: they snap the view back to the live screen.
    pub fn input(&mut self, input: impl Into<Cow<'static, [u8]>>) {
        self.keyboard_input_sent = true;
        self.pending_scroll_to_bottom = true;
        self.pty.notify(input);
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self.content.mode.contains(TermMode::BRACKETED_PASTE);
        let payload = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.input(payload.into_bytes());
    }

    fn scroll(&mut self, scroll: Scroll) {
        self.term.lock().scroll_display(scroll);
        self.snapshot();
    }

    pub fn scroll_line_up(&mut self) {
        self.scroll(Scroll::Delta(1));
    }

    pub fn scroll_line_down(&mut self) {
        self.scroll(Scroll::Delta(-1));
    }

    pub fn scroll_page_up(&mut self) {
        self.scroll(Scroll::PageUp);
    }

    pub fn scroll_page_down(&mut self) {
        self.scroll(Scroll::PageDown);
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll(Scroll::Top);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll(Scroll::Bottom);
    }

    /// Returns whether the key was consumed; unconsumed keys arrive later as typed text.
    pub fn try_keystroke(&mut self, keystroke: &Keystroke, option_as_meta: bool) -> bool {
        match input::escape_sequence(keystroke, self.content.mode, option_as_meta) {
            Some(Cow::Borrowed(sequence)) => self.input(sequence.as_bytes()),
            Some(Cow::Owned(sequence)) => self.input(sequence.into_bytes()),
            None => return false,
        }
        true
    }

    pub fn focus_changed(&mut self, focused: bool) {
        if self.content.mode.contains(TermMode::FOCUS_IN_OUT) {
            let report: &'static [u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.pty.notify(report);
        }
    }

    /// Wipe the screen and scrollback, keeping the cursor's line (with the prompt) at the top.
    pub fn clear(&mut self) {
        let mut term = self.term.lock();
        alacritty_terminal::vte::ansi::Handler::clear_screen(
            &mut *term,
            alacritty_terminal::vte::ansi::ClearMode::Saved,
        );
        let cursor = term.grid().cursor.point;
        let columns = term.grid().columns();
        term.grid_mut().reset_region(..cursor.line);
        for index in 0..columns {
            let cell = term.grid()[cursor.line][Column(index)].clone();
            term.grid_mut()[Line(0)][Column(index)] = cell;
        }
        term.grid_mut().cursor.point = BackendPoint::new(Line(0), cursor.column);
        if term.screen_lines() > 1 {
            term.grid_mut().reset_region(Line(1)..);
        }
        drop(term);
        self.snapshot();
    }

    /// Programs that track the mouse get reports instead of selections; holding shift overrides that.
    pub fn mouse_mode(&self, shift: bool) -> bool {
        self.content.mode.intersects(TermMode::MOUSE_MODE) && !shift
    }

    fn cell_at(&self, x: f32, y: f32) -> (GridPoint, Side) {
        grid_point_and_side(x, y, self.content.bounds, self.content.display_offset)
    }

    /// A wheel movement of `delta_y` px at `(x, y)` in the grid: reported to mouse-tracking programs, turned
    /// into arrow keys on the alternate screen, and otherwise scrolling the history.
    pub fn scroll_wheel(&mut self, delta_y: f32, x: f32, y: f32, modifiers: Modifiers) {
        let line_height = self.content.bounds.line_height;
        let before = (self.scroll_px / line_height) as i32;
        self.scroll_px += delta_y;
        let lines = (self.scroll_px / line_height) as i32 - before;
        self.scroll_px %= self.content.bounds.height;
        if lines == 0 {
            return;
        }
        let mode = self.content.mode;
        if self.mouse_mode(modifiers.shift) {
            let (point, _) = self.cell_at(x, y);
            for report in mouse::scroll_reports(point, lines, modifiers, mode) {
                self.pty.notify(report);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
            && !modifiers.shift
        {
            self.pty.notify(mouse::alt_scroll(lines));
        } else {
            self.scroll(Scroll::Delta(lines));
        }
    }

    pub fn mouse_down(
        &mut self,
        x: f32,
        y: f32,
        button: MouseButton,
        modifiers: Modifiers,
        click_count: u32,
    ) {
        let (point, side) = self.cell_at(x, y);
        if self.mouse_mode(modifiers.shift) {
            if let Some(report) =
                mouse::button_report(point, button, modifiers, true, self.content.mode)
            {
                self.pty.notify(report);
            }
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        self.mouse_down_position = Some((x, y));
        let kind = match click_count {
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => return,
        };
        let mut term = self.term.lock();
        if kind == SelectionType::Simple && modifiers.shift {
            match term.selection.as_mut() {
                Some(selection) => selection.update(backend_point(point), backend_side(side)),
                None => {
                    term.selection = Some(Selection::new(
                        kind,
                        backend_point(point),
                        backend_side(side),
                    ))
                }
            }
        } else {
            term.selection = Some(Selection::new(
                kind,
                backend_point(point),
                backend_side(side),
            ));
        }
        drop(term);
        self.snapshot();
    }

    /// Extend the selection to the pointer; dragging above or below the grid scrolls the history, faster the
    /// further out the pointer is.
    pub fn mouse_drag(&mut self, x: f32, y: f32, modifiers: Modifiers) {
        if self.mouse_mode(modifiers.shift) {
            return;
        }
        if !self.selecting {
            if let Some((down_x, down_y)) = self.mouse_down_position {
                if (x - down_x).hypot(y - down_y) <= SELECTION_DRAG_THRESHOLD {
                    return;
                }
            }
        }
        self.selecting = true;
        let (point, side) = self.cell_at(x, y);
        let alt_screen = self.content.mode.contains(TermMode::ALT_SCREEN);
        let mut term = self.term.lock();
        if let Some(selection) = term.selection.as_mut() {
            selection.update(backend_point(point), backend_side(side));
        }
        if !alt_screen {
            let line_height = self.content.bounds.line_height;
            let bottom = self.content.bounds.height;
            let lines = if y < 0.0 {
                ((-y).powf(1.1) / line_height).ceil() as i32
            } else if y > bottom {
                (-(y - bottom).powf(1.1) / line_height).floor() as i32
            } else {
                0
            };
            if lines != 0 {
                term.scroll_display(Scroll::Delta(lines.clamp(-3, 3)));
            }
        }
        drop(term);
        self.snapshot();
    }

    pub fn mouse_move(&mut self, x: f32, y: f32, held: Option<MouseButton>, modifiers: Modifiers) {
        if !self.mouse_mode(modifiers.shift) {
            return;
        }
        let cell = self.cell_at(x, y);
        if self.last_mouse == Some(cell) {
            return;
        }
        self.last_mouse = Some(cell);
        if let Some(report) = mouse::moved_report(cell.0, held, modifiers, self.content.mode) {
            self.pty.notify(report);
        }
    }

    pub fn mouse_up(&mut self, x: f32, y: f32, button: MouseButton, modifiers: Modifiers) {
        if self.mouse_mode(modifiers.shift) {
            let (point, _) = self.cell_at(x, y);
            if let Some(report) =
                mouse::button_report(point, button, modifiers, false, self.content.mode)
            {
                self.pty.notify(report);
            }
        }
        self.selecting = false;
        self.last_mouse = None;
        self.mouse_down_position = None;
    }

    /// The link under `(x, y)` in grid design px, if any.
    pub fn hyperlink_at(&mut self, x: f32, y: f32) -> Option<HyperlinkMatch> {
        let (point, _) = self.cell_at(x, y);
        let term = self.term.lock();
        let point = BackendPoint::new(Line(point.line), Column(point.column));
        if point.line < term.topmost_line() || point.line > term.bottommost_line() {
            return None;
        }
        self.links.find(&term, point)
    }

    /// Returns whether the hovered link changed.
    pub fn set_hovered_link(&mut self, link: Option<HyperlinkMatch>) -> bool {
        if self.hovered_link == link {
            return false;
        }
        self.hovered_link = link.clone();
        self.content.hovered_link = link;
        true
    }

    pub fn content_version(&self) -> u64 {
        self.content_version
    }

    /// Lines of scrollback above the live screen; grid lines run from `-history_size` to the screen bottom.
    pub fn history_size(&self) -> usize {
        self.term.lock().grid().history_size()
    }

    /// Every match of `pattern` in the scrollback and screen, in order (case-insensitive unless the pattern has
    /// an uppercase letter). An invalid pattern finds nothing.
    pub fn find(&self, pattern: &str) -> Vec<(GridPoint, GridPoint)> {
        let Ok(mut regex) = alacritty_terminal::term::search::RegexSearch::new(pattern) else {
            return Vec::new();
        };
        let term = self.term.lock();
        let start = BackendPoint::new(term.topmost_line(), Column(0));
        let end = BackendPoint::new(term.bottommost_line(), term.last_column());
        alacritty_terminal::term::search::RegexIter::new(
            start,
            end,
            alacritty_terminal::index::Direction::Right,
            &term,
            &mut regex,
        )
        .map(|found| (grid_point(*found.start()), grid_point(*found.end())))
        .collect()
    }

    /// Select `start..=end` and scroll it into view.
    pub fn select_range(&mut self, start: GridPoint, end: GridPoint) {
        let mut term = self.term.lock();
        let mut selection = Selection::new(
            SelectionType::Simple,
            backend_point(start),
            BackendSide::Left,
        );
        selection.update(backend_point(end), BackendSide::Right);
        term.selection = Some(selection);
        term.scroll_to_point(backend_point(start));
        drop(term);
        self.snapshot();
    }

    pub fn set_search_matches(&mut self, matches: Vec<(GridPoint, GridPoint)>) {
        self.search_matches = matches;
        self.content.search_matches = self.search_matches.clone();
    }

    /// Where the selection ends, if there is one; searches measure "next" from here.
    pub fn selection_head(&self) -> Option<GridPoint> {
        self.content.selection.map(|selection| selection.end)
    }

    pub fn select_all(&mut self) {
        let mut term = self.term.lock();
        let start = BackendPoint::new(term.topmost_line(), Column(0));
        let end = BackendPoint::new(term.bottommost_line(), term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, BackendSide::Left);
        selection.update(end, BackendSide::Right);
        term.selection = Some(selection);
        drop(term);
        self.snapshot();
    }

    /// Run a terminal binding; returns the text to put on the clipboard for Copy. Paste goes through
    /// `paste` with the clipboard text instead.
    pub fn perform(&mut self, action: &TerminalAction) -> Option<String> {
        match action {
            TerminalAction::Copy => return self.selection_text(),
            TerminalAction::Paste => {}
            TerminalAction::Clear => self.clear(),
            TerminalAction::SelectAll => self.select_all(),
            TerminalAction::ScrollLineUp => self.scroll_line_up(),
            TerminalAction::ScrollLineDown => self.scroll_line_down(),
            TerminalAction::ScrollPageUp => self.scroll_page_up(),
            TerminalAction::ScrollPageDown => self.scroll_page_down(),
            TerminalAction::ScrollToTop => self.scroll_to_top(),
            TerminalAction::ScrollToBottom => self.scroll_to_bottom(),
            TerminalAction::SendKeystroke(keystroke) => {
                self.try_keystroke(keystroke, false);
            }
            TerminalAction::SendText(text) => self.input(text.as_bytes()),
        }
        None
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    pub fn clear_selection(&mut self) {
        self.term.lock().selection = None;
        self.snapshot();
    }

    /// Resize the grid and the PTY to the last `set_size`, if it changed; returns whether it did.
    pub fn apply_resize(&mut self) -> bool {
        let Some(bounds) = self.pending_resize.take() else {
            return false;
        };
        self.content.bounds = bounds;
        self.pty.resize(bounds.window_size());
        self.term.lock().resize(bounds);
        self.content_version += 1;
        self.snapshot();
        true
    }

    /// Apply queued resizes/scrolls and backend events, then refresh the snapshot.
    pub fn sync(&mut self, host: &dyn TerminalHost) -> SyncOutcome {
        let mut outcome = SyncOutcome {
            changed: self.apply_resize(),
            ..SyncOutcome::default()
        };
        if std::mem::take(&mut self.pending_scroll_to_bottom) {
            self.term.lock().scroll_display(Scroll::Bottom);
            outcome.changed = true;
        }
        while let Ok(event) = self.events.try_recv() {
            self.process_event(event, host, &mut outcome);
        }
        if outcome.changed {
            self.snapshot();
        }
        outcome
    }

    fn process_event(
        &mut self,
        event: BackendEvent,
        host: &dyn TerminalHost,
        outcome: &mut SyncOutcome,
    ) {
        match event {
            BackendEvent::Title(title) => {
                self.title = title;
                outcome.title_changed = true;
            }
            BackendEvent::ResetTitle => {
                self.title.clear();
                outcome.title_changed = true;
            }
            BackendEvent::ClipboardStore(_, text) => outcome.clipboard_store = Some(text),
            BackendEvent::ClipboardLoad(_, format) => {
                let text = host.clipboard_text().unwrap_or_default();
                self.pty.notify(format(&text).into_bytes());
            }
            BackendEvent::PtyWrite(text) => self.pty.notify(text.into_bytes()),
            BackendEvent::TextAreaSizeRequest(format) => {
                self.pty
                    .notify(format(self.content.bounds.window_size()).into_bytes());
            }
            // Answered here, in order with other PTY writes, so a program sees replies in the order it asked.
            BackendEvent::ColorRequest(index, format) => {
                let color =
                    self.term.lock().colors()[index].unwrap_or_else(|| host.palette().color(index));
                self.pty.notify(format(color).into_bytes());
            }
            BackendEvent::Bell => outcome.bell = true,
            BackendEvent::Wakeup => {
                outcome.changed = true;
                self.content_version += 1;
                let due = self
                    .process_checked
                    .is_none_or(|at| at.elapsed() >= PROCESS_REFRESH_INTERVAL);
                if due {
                    self.process_checked = Some(Instant::now());
                    outcome.title_changed |= self.process.refresh();
                }
            }
            BackendEvent::Exit => outcome.close |= self.should_close(),
            BackendEvent::ChildExit(status) => {
                self.child_exit = Some(status);
                outcome.close |= self.should_close();
            }
            BackendEvent::CursorBlinkingChange | BackendEvent::MouseCursorDirty => {}
        }
    }

    /// A shell the user typed into closes on any exit; one that never got input and failed stays open so its
    /// error remains readable.
    fn should_close(&self) -> bool {
        self.keyboard_input_sent || self.child_exit.is_none_or(|status| status.success())
    }

    fn snapshot(&mut self) {
        let term = self.term.lock();
        let renderable = term.renderable_content();
        let cells = renderable
            .display_iter
            .map(|indexed| IndexedCell {
                line: indexed.point.line.0,
                column: indexed.point.column.0,
                c: indexed.cell.c,
                fg: indexed.cell.fg,
                bg: indexed.cell.bg,
                flags: indexed.cell.flags,
                zerowidth: indexed
                    .cell
                    .zerowidth()
                    .map(<[char]>::to_vec)
                    .unwrap_or_default(),
            })
            .collect();
        let colors = (0..COLOR_COUNT)
            .map(|index| renderable.colors[index])
            .collect();
        let grid = term.grid();
        self.content = Content {
            cells,
            cursor: Cursor {
                line: renderable.cursor.point.line.0,
                column: renderable.cursor.point.column.0,
                shape: renderable.cursor.shape,
            },
            mode: renderable.mode,
            display_offset: renderable.display_offset,
            total_lines: grid.total_lines(),
            screen_lines: grid.screen_lines(),
            columns: grid.columns(),
            hovered_link: self.hovered_link.clone(),
            search_matches: self.search_matches.clone(),
            selection: renderable.selection.map(|range| SelectionRange {
                start: grid_point(range.start),
                end: grid_point(range.end),
                is_block: range.is_block,
            }),
            colors,
            bounds: self.content.bounds,
        };
    }

    /// The visible screen as text, one line per row with trailing blanks trimmed.
    pub fn screen_text(&self) -> String {
        let mut rows = vec![String::new(); self.content.screen_lines];
        let top = -(self.content.display_offset as i32);
        for cell in &self.content.cells {
            let Some(row) = usize::try_from(cell.line - top)
                .ok()
                .and_then(|index| rows.get_mut(index))
            else {
                continue;
            };
            if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                row.push(cell.c);
            }
        }
        rows.iter()
            .map(|row| row.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
    }
}

fn backend_point(point: GridPoint) -> BackendPoint {
    BackendPoint::new(Line(point.line), Column(point.column))
}

fn grid_point(point: BackendPoint) -> GridPoint {
    GridPoint {
        line: point.line.0,
        column: point.column.0,
    }
}

fn backend_side(side: Side) -> BackendSide {
    match side {
        Side::Left => BackendSide::Left,
        Side::Right => BackendSide::Right,
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        match &self.pty {
            Backend::Local(notifier) => {
                if let Err(error) = notifier.0.send(Msg::Shutdown) {
                    eprintln!("terminal shutdown: {error}");
                }
            }
            Backend::Holder(holder) => holder.detach(),
        }
    }
}

fn login_shell_argv() -> Vec<String> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_string());
    vec![shell, "-l".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct Host(Palette);

    impl TerminalHost for Host {
        fn palette(&self) -> &Palette {
            &self.0
        }
        fn clipboard_text(&self) -> Option<String> {
            None
        }
    }

    fn host() -> Host {
        let rgb = |v: u8| Rgb { r: v, g: v, b: v };
        Host(Palette {
            ansi: [rgb(1); 16],
            dim: [rgb(2); 8],
            foreground: rgb(3),
            background: rgb(4),
            bright_foreground: rgb(5),
            cursor: rgb(6),
        })
    }

    fn wait_for(
        terminal: &mut Terminal,
        host: &Host,
        done: impl Fn(&Terminal, &SyncOutcome) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let outcome = terminal.sync(host);
            if done(terminal, &outcome) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out; screen:\n{}", terminal.screen_text());
    }

    fn sh(script: &str) -> TerminalOptions {
        TerminalOptions {
            shell: Some(("/bin/sh".into(), vec!["-c".into(), script.into()])),
            ..TerminalOptions::default()
        }
    }

    #[test]
    fn a_holder_backed_terminal_survives_being_dropped() -> anyhow::Result<()> {
        let temp = tempfile::Builder::new().prefix("pty").tempdir_in("/tmp")?;
        let dir = pom_ptyhost::SocketDir::new(temp.path());
        let session = pom_ptyhost::listen_and_serve(
            &dir,
            "appsh-test",
            pom_ptyhost::StartOptions {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf held-shell; exec cat".into(),
                ],
                dir: PathBuf::from("/"),
                env: vec![("PATH".into(), "/usr/bin:/bin".into())],
                cols: 80,
                rows: 24,
                on_exit: None,
            },
        )?;
        let options = || TerminalOptions {
            holder: Some(HolderOptions {
                dir: dir.clone(),
                name: "appsh-test".into(),
                binary: PathBuf::from("/nonexistent"),
                attach_only: false,
            }),
            ..TerminalOptions::default()
        };
        let host = host();
        let mut terminal = Terminal::spawn(options(), Arc::new(|| {}))?;
        assert!(terminal
            .holder()
            .is_some_and(|holder| holder.name == "appsh-test"));
        wait_for(&mut terminal, &host, |t, _| {
            t.screen_text().contains("held-shell")
        });
        terminal.input(&b"typed-through\n"[..]);
        wait_for(&mut terminal, &host, |t, _| {
            t.screen_text().contains("typed-through")
        });
        drop(terminal);

        std::thread::sleep(Duration::from_millis(100));
        assert!(
            session.exit_status().is_none(),
            "dropping the terminal only detaches"
        );
        let mut reattached = Terminal::spawn(options(), Arc::new(|| {}))?;
        wait_for(&mut reattached, &host, |t, _| {
            let screen = t.screen_text();
            screen.contains("held-shell") && screen.contains("typed-through")
        });
        session.kill();
        wait_for(&mut reattached, &host, |t, outcome| {
            outcome.close || t.child_exit().is_some()
        });
        Ok(())
    }

    #[test]
    fn program_output_reaches_the_grid() {
        let mut terminal =
            Terminal::spawn(sh("printf 'hello\\nworld'; sleep 5"), Arc::new(|| {})).unwrap();
        terminal.set_size(TerminalBounds {
            cell_width: 8.0,
            line_height: 16.0,
            width: 800.0,
            height: 160.0,
        });
        let host = host();
        wait_for(&mut terminal, &host, |t, _| {
            t.screen_text() == "hello\nworld"
        });
        assert_eq!(terminal.content().columns, 100);
        assert_eq!(terminal.content().screen_lines, 10);
    }

    #[test]
    fn input_is_echoed_and_exit_closes() {
        let mut terminal = Terminal::spawn(
            sh("read line; printf \"got %s\" \"$line\"; sleep 1"),
            Arc::new(|| {}),
        )
        .unwrap();
        let host = host();
        terminal.input(b"abc\r".as_slice());
        wait_for(&mut terminal, &host, |t, _| {
            t.screen_text().contains("got abc")
        });
        wait_for(&mut terminal, &host, |_, outcome| outcome.close);
    }

    #[test]
    #[ignore = "starts the user's real login shell"]
    fn login_shell_runs_commands() {
        let mut terminal = Terminal::spawn(TerminalOptions::default(), Arc::new(|| {})).unwrap();
        let host = host();
        terminal.input(b"echo pomelo-$((6*7))\r".as_slice());
        wait_for(&mut terminal, &host, |t, _| {
            t.screen_text().contains("pomelo-42")
        });
    }

    fn sized(options: TerminalOptions) -> Terminal {
        let mut terminal = Terminal::spawn(options, Arc::new(|| {})).unwrap();
        terminal.set_size(TerminalBounds {
            cell_width: 10.0,
            line_height: 20.0,
            width: 400.0,
            height: 200.0,
        });
        terminal.apply_resize();
        terminal
    }

    #[test]
    fn keystrokes_are_encoded_for_the_program() {
        let mut terminal = sized(sh(
            "stty -echo; read line; printf \"<%s>\" \"$line\"; sleep 5",
        ));
        let host = host();
        terminal.sync(&host);
        std::thread::sleep(Duration::from_millis(100));
        terminal.input(b"xy".as_slice());
        assert!(terminal.try_keystroke(&Keystroke::parse("backspace"), false));
        assert!(!terminal.try_keystroke(&Keystroke::parse("z"), false));
        terminal.input(b"z".as_slice());
        assert!(terminal.try_keystroke(&Keystroke::parse("enter"), false));
        wait_for(&mut terminal, &host, |t, _| {
            t.screen_text().contains("<xz>")
        });
    }

    #[test]
    fn double_click_selects_a_word_and_clear_keeps_the_cursor_line() {
        let mut terminal = sized(sh("printf 'one\\nhello world'; sleep 5"));
        let host = host();
        wait_for(&mut terminal, &host, |t, _| {
            t.screen_text() == "one\nhello world"
        });
        terminal.mouse_down(75.0, 30.0, MouseButton::Left, Modifiers::default(), 2);
        assert_eq!(terminal.selection_text().as_deref(), Some("world"));
        assert!(terminal.content().selection.is_some());
        terminal.mouse_down(5.0, 5.0, MouseButton::Left, Modifiers::default(), 1);
        terminal.mouse_drag(35.0, 25.0, Modifiers::default());
        assert_eq!(terminal.selection_text().as_deref(), Some("one\nhel"));
        terminal.mouse_up(35.0, 25.0, MouseButton::Left, Modifiers::default());
        terminal.clear();
        assert_eq!(terminal.screen_text(), "hello world");
    }

    #[test]
    fn links_under_the_pointer() {
        let mut terminal = sized(sh(
            "sleep 0.2; printf 'see https://example.com/a(b). and Update(src/main.rs:12:3) ok'; sleep 5",
        ));
        let host = host();
        wait_for(&mut terminal, &host, |t, _| t.screen_text().ends_with("ok"));
        let url = terminal.hyperlink_at(105.0, 5.0).unwrap();
        assert_eq!(
            (url.text.as_str(), url.is_url),
            ("https://example.com/a(b)", true)
        );
        assert_eq!((url.start.column, url.end.column), (4, 27));
        let path = terminal.hyperlink_at(10.0, 25.0).unwrap();
        assert_eq!(
            (path.text.as_str(), path.is_url),
            ("src/main.rs:12:3", false)
        );
        assert!(terminal.hyperlink_at(15.0, 5.0).unwrap().text == "see");
        assert!(terminal.set_hovered_link(Some(url.clone())));
        assert!(!terminal.set_hovered_link(Some(url)));
    }

    #[test]
    fn palette_covers_cube_and_grayscale() {
        let palette = host().0;
        assert_eq!(palette.color(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            palette.color(231),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(palette.color(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(palette.color(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(palette.color(257), palette.background);
        assert_eq!(palette.color(262), palette.dim[3]);
    }

    #[test]
    fn resizes_only_when_the_grid_changes() {
        let mut terminal = Terminal::spawn(sh("sleep 5"), Arc::new(|| {})).unwrap();
        let bounds = TerminalBounds {
            cell_width: 8.0,
            line_height: 16.0,
            width: 800.0,
            height: 160.0,
        };
        terminal.set_size(bounds);
        assert!(terminal.pending_resize.is_some());
        terminal.sync(&host());
        terminal.set_size(TerminalBounds {
            width: 803.0,
            ..bounds
        });
        assert!(terminal.pending_resize.is_none());
    }
}
