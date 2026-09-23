//! Terminal core: a shell running on a PTY, its output parsed by a VT emulator into a scrollback grid, and a
//! snapshot of that grid for a view to draw. No rendering here; `terminal_ui` draws `Content`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::{mpsc, Arc};

use alacritty_terminal::event::{Event as BackendEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;

pub use alacritty_terminal::term::cell::Flags;
pub use alacritty_terminal::term::TermMode;
pub use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

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
    /// Colors a program redefined (OSC 4/10/11); `None` falls back to the palette.
    pub colors: Vec<Option<Rgb>>,
    pub bounds: TerminalBounds,
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
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            shell: None,
            working_directory: None,
            env: HashMap::new(),
            scroll_history: DEFAULT_SCROLL_HISTORY_LINES,
            cursor_shape: CursorShape::Block,
        }
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Notifier,
    events: mpsc::Receiver<BackendEvent>,
    pending_resize: Option<TerminalBounds>,
    pending_scroll_to_bottom: bool,
    content: Content,
    title: String,
    keyboard_input_sent: bool,
    child_exit: Option<ExitStatus>,
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
        let pty_options = tty::Options {
            shell: options
                .shell
                .map(|(program, args)| tty::Shell::new(program, args)),
            working_directory: options.working_directory,
            drain_on_exit: true,
            env: terminal_env(options.env),
        };
        let pty = tty::new(&pty_options, bounds.window_size(), 0)?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let pty = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Self {
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
        })
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    pub fn title(&self) -> &str {
        &self.title
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

    pub fn scroll_lines(&mut self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.snapshot();
    }

    /// Apply queued resizes/scrolls and backend events, then refresh the snapshot.
    pub fn sync(&mut self, host: &dyn TerminalHost) -> SyncOutcome {
        let mut outcome = SyncOutcome::default();
        if let Some(bounds) = self.pending_resize.take() {
            self.content.bounds = bounds;
            if let Err(error) = self.pty.0.send(Msg::Resize(bounds.window_size())) {
                eprintln!("terminal resize: {error}");
            }
            self.term.lock().resize(bounds);
            outcome.changed = true;
        }
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
            BackendEvent::Wakeup => outcome.changed = true,
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

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Err(error) = self.pty.0.send(Msg::Shutdown) {
            eprintln!("terminal shutdown: {error}");
        }
    }
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
