//! Files feature view — the file tree plus the selected file's contents (syntax-highlighted), rendered as a
//! `workspace::FunctionView`. Ported from the Swift `FilesPane`: a tree on the left (indent per depth, folders
//! expand/collapse) and the open file on the right. Logic (listing/reading) lives in the `files` crate; syntax
//! highlighting reuses the `editor` crate. The `workspace` toolkit stays free of this crate .

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use editor::buffer::{
    Bias, ClipboardSelection, Deletion, DisplayRows, Motion, Selection, SelectionGoal,
};
use editor::fold::FoldMap;
use editor::search::SearchQuery;
use editor::transform::{LineTransform, TextTransform};
use workspace::pane::{Pane, PaneCommand};
#[cfg(test)]
use workspace::pane_group::Member;
use workspace::pane_group::SplitDirection;
use workspace::pane_group_view::{
    self, GroupClick, PaneButton, PaneButtonAction, PaneGroupConfig, PaneGroupView,
};
use workspace::search_bar::Searchable;
use workspace::text_field;

mod command_palette;
mod completions_menu;
mod definition;
mod diagnostic_nav;
mod fuzzy;
mod git_diff;
mod go_to_line;
mod hover;
mod list_scrollbar;
mod lsp_completion;
mod markdown_view;
mod outline_view;
mod saved_state;
mod snippet_store;
mod tree_actions;
use editor::wrap::Boundary;
use editor::{EditorBuffer, Lang, Syntax, Theme};
use files::FileNode;
use ui::{div, icon, label, material_icon, theme, IconKind, MaterialIcon, Node, Rect, Rgba};
use workspace::{
    ClipboardSlice, CopiedText, DividerAxis, EditKey, EditorLayout, Elevation, FunctionView, Item,
    ItemInput, ModalView, FUNC_VIEW_BASE,
};

// Editor text metrics (design px). Each source line is `EDIT_LINE_H` tall with `EDIT_FONT` mono text. Caret/selection geometry uses these plus the mono advance so it aligns with the glyphs.
const EDIT_FONT: f32 = 15.0; // matches the reference's default buffer font size
const EDIT_LINE_H: f32 = 24.0; // ~15 * 1.618 ("comfortable" line height), snapped to a whole pixel
const MESSAGE_PAD: f32 = 8.0;
const CARET_W: f32 = 2.0;
const VERTICAL_SCROLL_MARGIN: f32 = 3.0;
const HORIZONTAL_SCROLL_MARGIN: f32 = 5.0;
/// With soft wrap off, lines still break past this many columns so pathological lines stay cheap to lay out.
const UNWRAPPED_MAX_COLUMNS: f32 = 512.0;
/// Blank columns between a line's end and its blame annotation.
const INLINE_BLAME_PADDING: f32 = 7.0;
const FOLD_PILL_PAD: f32 = 4.0;
/// Line numbers reserve at least this many digits so the gutter doesn't resize as a file grows past 999 lines.
const MIN_LINE_NUMBER_DIGITS: usize = 4;
const FOLD_ICON: f32 = 14.0;
const SCROLLBAR_WIDTH: f32 = 15.0;
const SCROLLBAR_BORDER: f32 = 1.0;
const SCROLLBAR_MIN_THUMB: f32 = 25.0;
const SCROLLBAR_LINE_MARKER: f32 = 2.0;
const SCROLLBAR_CURSOR_MARKER_LIMIT: usize = 100;
/// Scrollbars show while scrolling and hide this long after the last scroll.
const SCROLLBAR_SHOW_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const TAB_COLS: usize = 4; // indent-guide spacing: one guide per this many leading columns

/// The monospace advance of one column at the editor font (Lilex is monospace, so every column is this wide).
fn char_advance() -> f32 {
    ui::measure_text_width("M", EDIT_FONT, true, 400)
}

/// Gutter layout: `left_padding` (room for run/breakpoint markers), right-aligned line numbers, `right_padding`
/// (the fold toggles), then `margin` before the text.
struct GutterDimensions {
    left_padding: f32,
    right_padding: f32,
    width: f32,
    margin: f32,
}

impl GutterDimensions {
    fn for_lines(line_count: usize) -> Self {
        let ch = char_advance();
        let digits = line_count
            .max(1)
            .to_string()
            .len()
            .max(MIN_LINE_NUMBER_DIGITS);
        let left_padding = 3.0 * ch;
        let right_padding = 4.0 * ch;
        Self {
            left_padding,
            right_padding,
            width: digits as f32 * ch + left_padding + right_padding,
            margin: ui::mono_descent(EDIT_FONT),
        }
    }

    fn full_width(&self) -> f32 {
        self.width + self.margin
    }

    fn fold_area_width(&self) -> f32 {
        self.margin + self.right_padding
    }
}

fn gutter_width(line_count: usize) -> f32 {
    GutterDimensions::for_lines(line_count).full_width()
}

const INDENT: f32 = 16.0; // per-depth indent (~20 in the design; trimmed for the narrower panel)
const ROW_H: f32 = 22.0;
const MAX_PANES: usize = 6; // ceiling on total leaf panes in the group

// Click ids in the feature-view space: tree rows below FUNC_VIEW_BASE + 1M, the editor pane group from there up
// to pane_group_view::ID_SPAN, then the ranges below (checked high-to-low in `on_click`, so they must not overlap).
const STICKY_BASE: u64 = FUNC_VIEW_BASE + pane_group_view::ID_SPAN;
const PALETTE_BASE: u64 = FUNC_VIEW_BASE + 14_500_000; // + PaletteClick
const OUTLINE_BASE: u64 = FUNC_VIEW_BASE + 14_600_000; // + row
const COMPLETION_BASE: u64 = FUNC_VIEW_BASE + 14_700_000; // + row
const HOVER_BASE: u64 = FUNC_VIEW_BASE + 14_800_000; // + popover
const MODAL_END: u64 = FUNC_VIEW_BASE + 15_000_000;

/// An open file as a center `Item` (an editor item): path/name + decoded text + language (`None` text =
/// binary) + its own syntax highlighter. Any center tab is a `Box<dyn Item>`, so a terminal/search/etc. can
/// share the same pane later.
/// A shaped display row: each glyph's starting byte index (into the tab-expanded text) with its x.
#[derive(Default)]
struct LineLayout {
    glyphs: Vec<(usize, f32)>,
    width: f32,
    len: usize,
}

impl LineLayout {
    /// The x of the glyph at or after `index`; the row's width past the end.
    fn x_for_index(&self, index: usize) -> f32 {
        self.glyphs
            .iter()
            .find(|(i, _)| *i >= index)
            .map_or(self.width, |(_, x)| *x)
    }

    /// The glyph boundary nearest `x`.
    fn closest_index_for_x(&self, x: f32) -> usize {
        let (mut prev_index, mut prev_x) = (0usize, 0.0f32);
        for &(index, glyph_x) in &self.glyphs {
            if glyph_x >= x {
                return if glyph_x - x < x - prev_x {
                    index
                } else {
                    prev_index
                };
            }
            prev_index = index;
            prev_x = glyph_x;
        }
        if self.len == 1 {
            return usize::from(x > self.width / 2.0);
        }
        self.len
    }
}

/// One screen row: a slice `[start, end)` of `line`'s tab-expanded text (`end` is `usize::MAX` on a line's last
/// row), drawn after `indent` blank columns (non-zero only on soft-wrap continuation rows).
#[derive(Clone, Copy, Debug, PartialEq)]
struct DisplayRow {
    line: usize,
    start: usize,
    end: usize,
    indent: usize,
    /// A committed line an expanded change removed, shown above `line` and never holding the caret.
    deleted: Option<usize>,
    block: Option<usize>,
    /// A blank row keeping a split diff's two sides level where the old side has more lines.
    spacer: bool,
}

impl DisplayRow {
    fn is_virtual(&self) -> bool {
        self.deleted.is_some() || self.block.is_some() || self.spacer
    }
}

/// What a split diff's old side shows on one display row: a line of the old text (or nothing), and
/// whether that line is part of a change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SplitLeft {
    base: Option<usize>,
    changed: bool,
}

/// Below this many columns a diff shows one column even when two are asked for.
const SPLIT_MIN_COLUMNS: f32 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineCommand {
    Delete,
    Duplicate { up: bool },
    Move { up: bool },
    Manipulate(LineTransform),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SoftWrap {
    None,
    EditorWidth,
}

struct FileItem {
    root: PathBuf,
    path: String,
    name: String,
    /// The editable text model (`None` = binary/unpreviewable). Char-offset rope with multi-cursor + undo.
    buffer: Option<EditorBuffer>,
    /// Incremental tree; highlighting runs per visible line against it. `None` = plain text.
    syntax: Option<Syntax>,
    /// Buffer version that folds and `max_cols` were last brought up to.
    synced_version: Option<u64>,
    /// Disk mtime at load/save; a newer one on disk means reload (clean) or conflict (dirty).
    saved_mtime: Option<std::time::SystemTime>,
    conflict: bool,
    /// Whether this item's pane is focused -- the caret only blinks in the focused pane.
    focused: bool,
    /// Vertical scroll in pixels (smooth, sub-line) and the visible body height (px) set before render.
    scroll_y: f32,
    /// Char offset whose display row sits at the top of the viewport, plus the pixel offset past that row, so the
    /// view stays on the same text when edits or folds change the rows above it.
    scroll_anchor: usize,
    scroll_anchor_offset: f32,
    body_h: f32,
    /// Horizontal scroll in pixels and the visible body width (px), for long lines. The gutter stays fixed; only
    /// the text scrolls left by `scroll_x`.
    scroll_x: f32,
    body_w: f32,
    /// Visual width per line (`None` = needs measuring); edits splice it so only touched lines are re-measured.
    line_widths: Vec<Option<usize>>,
    /// Shaped rows by buffer line, dropped whenever the text changes.
    layout_cache: std::cell::RefCell<std::collections::HashMap<usize, std::rc::Rc<LineLayout>>>,
    folds: FoldMap,
    /// Screen rows after folds and soft wrap (rebuilt when text, folds or wrap width change). `None` = stale.
    rows: Option<Vec<DisplayRow>>,
    /// First row of each buffer line; a line hidden in a fold maps to its header's first row.
    line_rows: Vec<usize>,
    /// Soft-wrap breaks per buffer line (`None` = not computed), spliced on edits like `line_widths`.
    wraps: Vec<Option<Rc<[Boundary]>>>,
    wrap_width: f32,
    lang: Lang,
    soft_wrap: SoftWrap,
    soft_wrap_override: Option<SoftWrap>,
    /// Widest row in columns, sizing horizontal scroll.
    max_row_cols: usize,
    char_widths: RefCell<HashMap<char, f32>>,
    gutter_hovered: bool,
    /// When the view last scrolled, which keeps the scrollbars visible for `SCROLLBAR_SHOW_INTERVAL`.
    scrolled_at: Option<std::time::Instant>,
    search_highlights: Vec<Range<usize>>,
    active_search_highlight: Option<usize>,
    /// Buffer lines (first, last) previewed by a modal.
    highlighted_rows: Option<(usize, usize)>,
    git: git_diff::GitDiff,
    /// A branch diff: compared with the file where the branch started, every change expanded.
    branch_diff: bool,
    /// The diff shows two columns when it is wide enough; `split_active` is whether it does now.
    split: bool,
    split_active: bool,
    /// Per display row, what the old side shows (split diffs only).
    split_left: Vec<SplitLeft>,
    /// Changes shown expanded, as char ranges carried through edits.
    expanded: Vec<Range<usize>>,
    /// The committed text expanded changes show their removed lines from.
    base: Option<BaseText>,
    /// Whether `rows` holds removed lines of expanded changes.
    has_virtual_rows: bool,
    /// Words completing the one being typed, when the menu is open.
    completions: Option<completions_menu::CompletionsMenu>,
    /// The menu was asked for explicitly, so it stays open below the minimum query length.
    completions_forced: bool,
    /// Where the user's snippet files are read from.
    snippet_dir: Option<PathBuf>,
    /// What the language server reports about this file, carried through edits.
    diagnostics: Vec<DiagnosticEntry>,
    /// Saved since the language server last heard about it.
    saved_unannounced: bool,
    hover: hover::HoverState,
    /// The server's trigger characters, when a server completes this file.
    lsp_triggers: Option<Vec<String>>,
    completion_request: Option<lsp_completion::PendingCompletion>,
    pending_resolve: Option<lsp_completion::PendingResolve>,
    definition_request: Option<definition::PendingDefinition>,
    /// Definition targets a cmd-click resolved, for the editor service to open.
    navigation: Option<(Vec<lsp::DefinitionTarget>, Option<f32>)>,
    link: Option<definition::LinkState>,
    active_diagnostic: Option<diagnostic_nav::ActiveDiagnostic>,
    scratch: Option<(String, String)>,
    footer: Option<Footer>,
}

struct Footer {
    view: Box<dyn workspace::ItemFooter>,
    area: Option<Rect>,
    focused: bool,
    pressed: bool,
    version: Option<u64>,
}

impl Footer {
    fn contains(&self, x: f32, y: f32) -> bool {
        let grip = FOOTER_GRIP * ui::ui_text_scale();
        self.area.is_some_and(|area| {
            x >= area.x && x < area.x + area.w && y >= area.y - grip && y < area.y + area.h
        })
    }
}

const FOOTER_GRIP: f32 = 4.0;

impl FileItem {
    fn run_request(&self, all: bool) -> Option<workspace::RunRequest> {
        let buffer = self.buffer.as_ref()?;
        let caret_char = buffer.newest().head().min(buffer.rope.len_chars());
        Some(workspace::RunRequest {
            text: buffer.text(),
            selection: buffer
                .selected_text()
                .filter(|selected| !selected.trim().is_empty()),
            caret: buffer.rope.char_to_byte(caret_char),
            all,
        })
    }

    fn footer_key(&mut self, key: EditKey, shift: bool) -> bool {
        let request = if key == EditKey::ReplaceAll {
            self.run_request(shift)
        } else {
            None
        };
        let Some(footer) = self.footer.as_mut() else {
            return false;
        };
        if let Some(request) = request {
            footer.view.run(request);
            return true;
        }
        footer.focused && footer.view.key(key, shift)
    }
}

pub fn scratch_editor(
    id: String,
    title: String,
    text: &str,
    footer: Box<dyn workspace::ItemFooter>,
) -> Box<dyn Item> {
    let mut item = FileItem::new(std::env::temp_dir(), "console.sql", Some(text.to_string()));
    item.saved_mtime = None;
    item.git = git_diff::GitDiff::default();
    item.scratch = Some((id, title));
    item.footer = Some(Footer {
        view: footer,
        area: None,
        focused: false,
        pressed: false,
        version: item.buffer.as_ref().map(EditorBuffer::version),
    });
    Box::new(item)
}

/// A problem a language server reported, over a char range of the buffer.
#[derive(Clone, Debug, PartialEq)]
struct DiagnosticEntry {
    range: Range<usize>,
    severity: lsp::lsp_types::DiagnosticSeverity,
    message: String,
    source: Option<String>,
    code: Option<String>,
}

/// Where a wavy underline sits in a line: below the baseline by most of the font's descent, as text is
/// centered in the line box. Ascent and descent are the mono font's 1.025 and 0.275 em.
const DIAGNOSTIC_UNDERLINE_TOP: f32 =
    (EDIT_LINE_H - EDIT_FONT * 1.3) / 2.0 + EDIT_FONT * 1.025 + EDIT_FONT * 0.275 * 0.618;
const DIAGNOSTIC_UNDERLINE_THICKNESS: f32 = 1.0;

fn diagnostic_color(severity: lsp::lsp_types::DiagnosticSeverity) -> Rgba {
    use lsp::lsp_types::DiagnosticSeverity as Severity;
    let colors = theme();
    match severity {
        Severity::ERROR => colors.error,
        Severity::WARNING => colors.warning,
        Severity::INFORMATION => colors.info,
        Severity::HINT => colors.hint,
        _ => colors.ignored,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CompletionTrigger {
    Typed,
    /// A single non-word char, which may be one of the server's trigger characters.
    Character(char),
    Refilter,
    Show,
    ShowWords,
}

/// Tab ids of branch diffs, so a file's diff and its plain tab can both be open.
const DIFF_ID_PREFIX: &str = "diff:";

/// The committed version of the file, highlighted like the file, for expanded changes' removed lines.
struct BaseText {
    bases: std::sync::Arc<git::DiffBases>,
    buffer: EditorBuffer,
    syntax: Option<Syntax>,
}

impl FileItem {
    /// The file compared with `base` (its text where the branch started; `None` when the branch added
    /// it), showing every change with its removed lines. It stays editable like the plain tab.
    fn branch_diff(root: PathBuf, path: &str, text: Option<String>, base: Option<String>) -> Self {
        let mut item = FileItem::new(root, path, text);
        item.git = git_diff::GitDiff::with_bases(
            item.root.join(path),
            git::DiffBases {
                head: base.clone(),
                index: base,
            },
        );
        item.branch_diff = true;
        item
    }

    fn retarget(&mut self, path: &str) {
        self.path = path.to_string();
        self.name = path.rsplit('/').next().unwrap_or(path).to_string();
        self.saved_mtime = files::mtime(&self.root, path).ok();
        self.diagnostics.clear();
        if self.buffer.is_some() {
            self.git = git_diff::GitDiff::load(self.root.join(path));
        }
    }

    fn new(root: PathBuf, path: &str, text: Option<String>) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let lang = Lang::detect(path, text.as_deref().and_then(|t| t.lines().next()));
        let buffer = text.map(|t| EditorBuffer::from_text(&t));
        let syntax = buffer.as_ref().and_then(|_| Syntax::new(lang));
        let saved_mtime = files::mtime(&root, path).ok();
        let git = if buffer.is_some() {
            git_diff::GitDiff::load(root.join(path))
        } else {
            git_diff::GitDiff::default()
        };
        Self {
            root,
            path: path.to_string(),
            name,
            buffer,
            syntax,
            synced_version: None,
            saved_mtime,
            conflict: false,
            focused: false,
            scroll_y: 0.0,
            scroll_anchor: 0,
            scroll_anchor_offset: 0.0,
            body_h: 400.0,
            scroll_x: 0.0,
            body_w: 400.0,
            line_widths: Vec::new(),
            layout_cache: Default::default(),
            folds: FoldMap::default(),
            rows: None,
            line_rows: Vec::new(),
            wraps: Vec::new(),
            wrap_width: 0.0,
            lang,
            soft_wrap: if lang == Lang::Markdown {
                SoftWrap::EditorWidth
            } else {
                SoftWrap::None
            },
            soft_wrap_override: None,
            max_row_cols: 0,
            char_widths: RefCell::default(),
            gutter_hovered: false,
            scrolled_at: None,
            search_highlights: Vec::new(),
            active_search_highlight: None,
            highlighted_rows: None,
            git,
            branch_diff: false,
            split: true,
            split_active: false,
            split_left: Vec::new(),
            expanded: Vec::new(),
            base: None,
            has_virtual_rows: false,
            completions: None,
            completions_forced: false,
            snippet_dir: if cfg!(test) {
                None
            } else {
                snippet_store::default_dir()
            },
            diagnostics: Vec::new(),
            saved_unannounced: false,
            hover: hover::HoverState::default(),
            lsp_triggers: None,
            completion_request: None,
            pending_resolve: None,
            definition_request: None,
            navigation: None,
            link: None,
            active_diagnostic: None,
            scratch: None,
            footer: None,
        }
    }

    /// Adopt a file's diagnostics: placed in the text they were computed for, then carried through the edits
    /// made since.
    fn set_diagnostics(&mut self, update: &lsp::DiagnosticsUpdate) {
        self.refresh();
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        let (rope, since) = match &update.synced {
            Some(synced) => (&synced.rope, Some(synced.buffer_version)),
            None => (&b.rope, None),
        };
        let mut entries: Vec<DiagnosticEntry> = update
            .diagnostics
            .iter()
            .map(|diagnostic| DiagnosticEntry {
                range: lsp::diagnostic_char_range(rope, diagnostic.range),
                severity: diagnostic
                    .severity
                    .unwrap_or(lsp::lsp_types::DiagnosticSeverity::ERROR),
                message: diagnostic.message.clone(),
                source: diagnostic.source.clone(),
                code: diagnostic.code.as_ref().map(|code| match code {
                    lsp::lsp_types::NumberOrString::Number(number) => number.to_string(),
                    lsp::lsp_types::NumberOrString::String(text) => text.clone(),
                }),
            })
            .collect();
        if let Some(since) = since {
            for batch in b.edits_since(since) {
                for entry in &mut entries {
                    entry.range.start =
                        editor::buffer::map_offset(batch, entry.range.start, Bias::Left);
                    entry.range.end =
                        editor::buffer::map_offset(batch, entry.range.end, Bias::Left);
                }
            }
        }
        self.diagnostics = entries;
        self.refresh_active_diagnostic();
    }

    /// Wavy underlines under the visible diagnostics, the more severe drawn on top.
    fn diagnostic_rects(&self, content: Rect, first: usize, last: usize) -> Vec<Rect> {
        let gw = gutter_width(self.line_count());
        let mut ordered: Vec<&DiagnosticEntry> = self.diagnostics.iter().collect();
        ordered.sort_by_key(|entry| std::cmp::Reverse(entry.severity));
        let mut rects = Vec::new();
        for entry in ordered {
            let (start_row, start_x) = self.position(entry.range.start);
            let (end_row, end_x) = self.position(entry.range.end);
            if end_row < first || start_row >= last {
                continue;
            }
            for row in start_row.max(first)..=end_row.min(last.saturating_sub(1)) {
                let left = if row == start_row { start_x } else { 0.0 };
                let right = if row == end_row {
                    end_x
                } else {
                    self.row_width(row)
                };
                if right <= left {
                    continue;
                }
                rects.push(Rect::wavy_underline(
                    content.x + gw + left - self.scroll_x,
                    content.y + row as f32 * EDIT_LINE_H - self.scroll_y + DIAGNOSTIC_UNDERLINE_TOP,
                    right - left,
                    DIAGNOSTIC_UNDERLINE_THICKNESS,
                    diagnostic_color(entry.severity),
                ));
            }
        }
        rects
    }

    /// Bring dependents up to the buffer: the syntax tree, fold positions (carried through the edits since the
    /// last sync), and the widest line.
    fn refresh(&mut self) {
        match (self.syntax.as_mut(), self.buffer.as_mut()) {
            (Some(syntax), Some(b)) => syntax.autoindent(b),
            (None, Some(b)) => {
                b.take_autoindent_requests();
            }
            _ => {}
        }
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        if let Some(syntax) = self.syntax.as_mut() {
            syntax.sync(b);
        }
        let version = b.version();
        if self.synced_version == Some(version) {
            return;
        }
        if let Some(since) = self.synced_version {
            for batch in b.edits_since(since) {
                self.scroll_anchor =
                    editor::buffer::map_offset(batch, self.scroll_anchor, Bias::Left);
                for range in &mut self.expanded {
                    range.start = editor::buffer::map_offset(batch, range.start, Bias::Left);
                    range.end = editor::buffer::map_offset(batch, range.end, Bias::Right);
                }
                for entry in &mut self.diagnostics {
                    entry.range.start =
                        editor::buffer::map_offset(batch, entry.range.start, Bias::Left);
                    entry.range.end =
                        editor::buffer::map_offset(batch, entry.range.end, Bias::Left);
                }
                if let Some(active) = self.active_diagnostic.as_mut() {
                    active.range.start =
                        editor::buffer::map_offset(batch, active.range.start, Bias::Left);
                    active.range.end =
                        editor::buffer::map_offset(batch, active.range.end, Bias::Right);
                }
            }
        }
        self.folds.sync(b);
        match self.synced_version {
            // Edits are logged in application order with rows valid at that moment, so splicing in that order
            // keeps every slot aligned with its line; only the spliced-in rows get re-measured.
            Some(since) if !self.line_widths.is_empty() => {
                for edit in b.syntax_edits_since(since) {
                    let start = edit.start_position.row.min(self.line_widths.len());
                    let old_end =
                        (edit.old_end_position.row + 1).clamp(start, self.line_widths.len());
                    let fresh = edit.new_end_position.row + 1 - edit.start_position.row;
                    self.line_widths
                        .splice(start..old_end, std::iter::repeat_n(None, fresh));
                    let wrap_end = old_end.min(self.wraps.len());
                    let wrap_start = start.min(wrap_end);
                    self.wraps
                        .splice(wrap_start..wrap_end, std::iter::repeat_n(None, fresh));
                }
            }
            _ => {
                self.line_widths.clear();
                self.wraps.clear();
            }
        }
        self.line_widths.resize(b.rope.len_lines(), None);
        self.wraps.resize(b.rope.len_lines(), None);
        for (line, width) in self.line_widths.iter_mut().enumerate() {
            if width.is_none() {
                *width = Some(Self::vis_col(b, line, b.line_len(line)));
            }
        }
        self.rows = None;
        self.layout_cache.borrow_mut().clear();
        self.synced_version = Some(version);
    }

    /// Buffer `line` as colored runs (tabs expanded onto the grid), highlighted from the syntax tree.
    fn line_segments(&self, line: usize, colors: &Theme) -> Vec<(String, Rgba)> {
        match self.buffer.as_ref() {
            Some(b) => segments_of(b, self.syntax.as_ref(), line, colors),
            None => Vec::new(),
        }
    }

    /// A committed line an expanded change removed, colored like the code.
    fn base_line_segments(&self, line: usize, colors: &Theme) -> Vec<(String, Rgba)> {
        match self.base.as_ref() {
            Some(base) => segments_of(&base.buffer, base.syntax.as_ref(), line, colors),
            None => Vec::new(),
        }
    }
}

/// Buffer `line` as colored runs (tabs expanded onto the grid), highlighted from `syntax` when there is one.
fn segments_of(
    b: &EditorBuffer,
    syntax: Option<&Syntax>,
    line: usize,
    colors: &Theme,
) -> Vec<(String, Rgba)> {
    {
        if line >= b.rope.len_lines() {
            return Vec::new();
        }
        let start = b.rope.line_to_byte(line);
        let end = start + b.rope.line(line).len_bytes()
            - usize::from(b.line_len(line) < b.rope.line(line).len_chars());
        let runs = match syntax {
            Some(syntax) => syntax.highlight(&b.rope, start..end),
            None => vec![editor::syntax::HighlightRun {
                range: start..end,
                capture: None,
            }],
        };
        let (mut col, mut byte) = (0usize, 0usize);
        runs.into_iter()
            .filter(|run| !run.range.is_empty())
            .map(|run| {
                let mut text = String::new();
                for chunk in b.rope.byte_slice(run.range).chunks() {
                    editor::display::push_expanded(&mut text, chunk, TAB_COLS, &mut col, &mut byte);
                }
                (text, color_of(colors, run.capture.unwrap_or("")))
            })
            .collect()
    }
}

impl FileItem {
    fn line_count(&self) -> usize {
        self.buffer
            .as_ref()
            .map(|b| b.rope.len_lines())
            .unwrap_or(1)
    }

    // ---- display rows: folds hide buffer rows, soft wrap splits a line into several rows ----

    fn is_folded(&self, line: usize) -> bool {
        self.buffer
            .as_ref()
            .is_some_and(|b| self.folds.fold_on_row(b, line).is_some())
    }

    fn is_foldable(&self, line: usize) -> bool {
        self.buffer
            .as_ref()
            .is_some_and(|b| editor::fold::starts_indent(b, line))
            || self.is_folded(line)
    }

    /// Display rows each selection's lines cover, with whether that selection is non-empty. These rows get the
    /// active line-number color; the ones without a non-empty selection also get the active-line background.
    fn active_rows(&self) -> Vec<(std::ops::RangeInclusive<usize>, bool)> {
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        b.selections()
            .iter()
            .map(|s| {
                let first_line = self.buf_of(self.position(s.start).0);
                let last_line = self.buf_of(self.position(s.end).0);
                (
                    self.disp_of(first_line)..=self.last_row_of_line(last_line),
                    !s.is_empty(),
                )
            })
            .collect()
    }

    fn soft_wrap_mode(&self) -> SoftWrap {
        self.soft_wrap_override.unwrap_or(self.soft_wrap)
    }

    fn current_wrap_width(&self) -> f32 {
        let em = char_advance();
        match self.soft_wrap_mode() {
            SoftWrap::None => UNWRAPPED_MAX_COLUMNS * em,
            SoftWrap::EditorWidth => (self.text_viewport_w() - 2.0 * em).max(em),
        }
    }

    fn toggle_soft_wrap(&mut self) {
        self.soft_wrap_override = match self.soft_wrap_override {
            Some(_) => None,
            None => Some(match self.soft_wrap_mode() {
                SoftWrap::None => SoftWrap::EditorWidth,
                SoftWrap::EditorWidth => SoftWrap::None,
            }),
        };
        self.rows = None;
        self.ensure_visible();
        self.scroll_x = self.scroll_x.min(self.max_scroll_x());
        self.ensure_cursor_visible();
    }

    fn rebuild_rows(&mut self) {
        let Some(b) = self.buffer.as_ref() else {
            self.rows = Some(Vec::new());
            return;
        };
        let wrap_width = self.current_wrap_width();
        if (wrap_width - self.wrap_width).abs() > 0.5 {
            self.wraps.iter_mut().for_each(|w| *w = None);
            self.wrap_width = wrap_width;
        }
        let line_count = b.rope.len_lines();
        let lines: Vec<usize> = if self.folds.is_empty() {
            (0..line_count).collect()
        } else {
            self.folds.visible_rows(b)
        };
        let folded: std::collections::HashSet<usize> = self
            .folds
            .merged()
            .iter()
            .map(|f| b.rope.char_to_line(f.start))
            .collect();
        let em = char_advance();
        let block = self.block_placement();
        let mut block_rows = false;
        let deletions = self.expanded_deletions();
        let mut pending_deletions = deletions.iter().peekable();
        let mut rows = Vec::with_capacity(lines.len());
        let mut line_rows = vec![0usize; line_count];
        let mut max_row_cols = 0usize;
        let push_deleted = |rows: &mut Vec<DisplayRow>, line: usize, base_rows: &Range<usize>| {
            for base_line in base_rows.clone() {
                rows.push(DisplayRow {
                    line,
                    start: 0,
                    end: usize::MAX,
                    indent: 0,
                    deleted: Some(base_line),
                    block: None,
                    spacer: false,
                });
            }
        };
        for (i, &line) in lines.iter().enumerate() {
            // Removed lines sit above the line their change starts at; ones under a fold stay hidden.
            while let Some((at, base_rows)) = pending_deletions.next_if(|(at, _)| *at <= line) {
                if *at == line {
                    push_deleted(&mut rows, line, base_rows);
                }
            }
            let next = lines.get(i + 1).copied().unwrap_or(line_count);
            for slot in line_rows.iter_mut().take(next).skip(line) {
                *slot = rows.len();
            }
            let columns = self.line_widths.get(line).copied().flatten().unwrap_or(0);
            let boundaries = if folded.contains(&line) {
                None
            } else {
                match self.wraps.get(line).cloned().flatten() {
                    Some(cached) => Some(cached),
                    None => {
                        let computed =
                            wrap_boundaries(b, line, columns, wrap_width, em, &self.char_widths);
                        if let Some(slot) = self.wraps.get_mut(line) {
                            *slot = Some(computed.clone());
                        }
                        Some(computed)
                    }
                }
            };
            let (mut start, mut indent) = (0usize, 0usize);
            for boundary in boundaries.iter().flat_map(|b| b.iter()) {
                rows.push(DisplayRow {
                    line,
                    start,
                    end: boundary.index,
                    indent,
                    deleted: None,
                    block: None,
                    spacer: false,
                });
                max_row_cols = max_row_cols.max(indent + boundary.index - start);
                start = boundary.index;
                indent = boundary.indent;
            }
            rows.push(DisplayRow {
                line,
                start,
                end: usize::MAX,
                indent,
                deleted: None,
                block: None,
                spacer: false,
            });
            if let Some(diagnostic_nav::BlockPlacement::Rows { line: at, count }) = &block {
                if *at == line {
                    for index in 0..*count {
                        rows.push(DisplayRow {
                            line,
                            start: 0,
                            end: 0,
                            indent: 0,
                            deleted: None,
                            block: Some(index),
                            spacer: false,
                        });
                    }
                    block_rows = true;
                }
            }
            let last_cols = if start == 0 {
                columns
            } else {
                indent + Self::display_index(b, line, b.line_len(line)).saturating_sub(start)
            };
            max_row_cols = max_row_cols.max(last_cols);
        }
        for (at, base_rows) in pending_deletions {
            if *at >= line_count {
                push_deleted(&mut rows, line_count.saturating_sub(1), base_rows);
            }
        }
        let (rows, line_rows) = if self.split_active {
            self.split_rows(rows, line_count)
        } else {
            self.split_left.clear();
            (rows, line_rows)
        };
        self.has_virtual_rows =
            !deletions.is_empty() || block_rows || rows.iter().any(|row| row.spacer);
        self.rows = Some(rows);
        self.line_rows = line_rows;
        self.max_row_cols = max_row_cols;
    }

    /// The old text is needed on the left of a split diff even when no change is expanded.
    fn base_needed_for_split(&mut self) {
        if self.split_active {
            self.sync_base_text();
        }
    }

    /// The old side of a split diff: its line numbers and text on the same rows as the new side, removed
    /// lines tinted, blanks where the new side added lines.
    fn paint_old_side(&mut self, area: Rect) -> ui::Painted {
        self.sync_base_text();
        self.ensure_visible();
        let colors = syntax_theme();
        let ui_colors = theme();
        let dcount = self.disp_count();
        let first = self.first_line().min(dcount);
        let visible = (self.body_h / EDIT_LINE_H).ceil() as usize + 2;
        let last = (first + visible).min(dcount);
        let base_lines = self
            .base
            .as_ref()
            .map_or(1, |base| base.buffer.rope.len_lines());
        let gutter = gutter_width(base_lines);
        let offset = first as f32 * EDIT_LINE_H - self.scroll_y;
        let mut numbers = div().col().w_px(gutter);
        let mut text = div().col().flex(1.0).pl(char_advance());
        let mut bands = Vec::new();
        let light = ui_colors.appearance == ui::Appearance::Light;
        let fill = if light { 0.16 } else { 0.12 };
        for row_index in first..last {
            let side = self.split_left.get(row_index).copied().unwrap_or_default();
            let y = area.y + row_index as f32 * EDIT_LINE_H - self.scroll_y;
            if side.changed && side.base.is_some() {
                bands.push(Rect::new(
                    area.x,
                    y,
                    area.w,
                    EDIT_LINE_H,
                    ui_colors.version_control_deleted.alpha(fill),
                ));
            }
            let Some(base_line) = side.base else {
                numbers = numbers.child(div().h_px(EDIT_LINE_H));
                text = text.child(div().h_px(EDIT_LINE_H));
                continue;
            };
            numbers = numbers.child(
                div()
                    .row()
                    .h_px(EDIT_LINE_H)
                    .items_center()
                    .justify_end()
                    .pr(char_advance())
                    .child(
                        label((base_line + 1).to_string())
                            .size(EDIT_FONT)
                            .mono()
                            .color(ui_colors.editor_line_number),
                    ),
            );
            let mut row = div().row().h_px(EDIT_LINE_H).items_center();
            let segments = self.base_line_segments(base_line, &colors);
            if segments.is_empty() {
                row = row.child(label(" ").size(EDIT_FONT).mono());
            }
            for (segment, color) in segments {
                row = row.child(label(segment).size(EDIT_FONT).mono().color(color));
            }
            text = text.child(row);
        }
        let node: Node = div()
            .col()
            .child(div().h_px(offset.max(0.0)))
            .child(div().row().child(numbers).child(text))
            .into();
        let mut painted = ui::Painted::default();
        painted.rects.push(Rect::new(
            area.x,
            area.y,
            area.w,
            area.h,
            ui_colors.editor_background,
        ));
        painted.rects.extend(bands);
        let top = Rect::new(
            area.x,
            area.y + offset.min(0.0),
            area.w + 4000.0,
            area.h + EDIT_LINE_H * 2.0,
            Rgba::TRANSPARENT,
        );
        let rendered = ui::render(&node, top);
        painted.rects.extend(rendered.rects);
        painted.texts.extend(rendered.texts);
        painted.icons.extend(rendered.icons);
        painted
    }

    /// Levels a split diff: after each change whose old side is longer, blank rows fill the new side,
    /// and every row records the old line shown beside it. Returns the rows and each line's first row.
    fn split_rows(
        &mut self,
        rows: Vec<DisplayRow>,
        line_count: usize,
    ) -> (Vec<DisplayRow>, Vec<usize>) {
        let mut hunks: Vec<git::DiffHunk> = self.git.hunks().to_vec();
        hunks.sort_by_key(|hunk| hunk.rows.start);
        let mut out = Vec::with_capacity(rows.len());
        let mut left = Vec::with_capacity(rows.len());
        let mut delta: isize = 0;
        let mut next_hunk = 0usize;
        let spacer = |line: usize| DisplayRow {
            line,
            start: 0,
            end: 0,
            indent: 0,
            deleted: None,
            block: None,
            spacer: true,
        };
        // A change's extra old lines go right after its new lines (at its start when it only removed).
        let flush = |until_line: usize,
                     out: &mut Vec<DisplayRow>,
                     left: &mut Vec<SplitLeft>,
                     delta: &mut isize,
                     next_hunk: &mut usize| {
            while let Some(hunk) = hunks.get(*next_hunk) {
                if hunk.rows.end > until_line {
                    break;
                }
                let (added, removed) = (hunk.rows.len(), hunk.base_rows.len());
                let anchor = hunk
                    .rows
                    .end
                    .saturating_sub(1)
                    .min(line_count.saturating_sub(1));
                for extra in added..removed {
                    out.push(spacer(anchor));
                    left.push(SplitLeft {
                        base: Some(hunk.base_rows.start + extra),
                        changed: true,
                    });
                }
                *delta += removed as isize - added as isize;
                *next_hunk += 1;
            }
        };
        for row in rows {
            let first_of_line = !row.is_virtual() && row.start == 0;
            if first_of_line {
                flush(row.line, &mut out, &mut left, &mut delta, &mut next_hunk);
            }
            let side = if row.is_virtual() || row.start > 0 {
                SplitLeft::default()
            } else if let Some(hunk) = hunks
                .get(next_hunk)
                .filter(|hunk| hunk.rows.contains(&row.line))
            {
                let index = row.line - hunk.rows.start;
                SplitLeft {
                    base: (index < hunk.base_rows.len()).then(|| hunk.base_rows.start + index),
                    changed: true,
                }
            } else {
                SplitLeft {
                    base: usize::try_from(row.line as isize + delta).ok(),
                    changed: false,
                }
            };
            out.push(row);
            left.push(side);
        }
        flush(usize::MAX, &mut out, &mut left, &mut delta, &mut next_hunk);
        let mut line_rows = vec![usize::MAX; line_count];
        for (index, row) in out.iter().enumerate() {
            if !row.is_virtual() && row.start == 0 {
                if let Some(slot) = line_rows.get_mut(row.line) {
                    if *slot == usize::MAX {
                        *slot = index;
                    }
                }
            }
        }
        let mut last = 0;
        for slot in line_rows.iter_mut() {
            if *slot == usize::MAX {
                *slot = last;
            } else {
                last = *slot;
            }
        }
        self.split_left = left;
        (out, line_rows)
    }

    fn ensure_visible(&mut self) {
        if self.rows.is_none() || (self.current_wrap_width() - self.wrap_width).abs() > 0.5 {
            self.rebuild_rows();
            self.restore_scroll_anchor();
        }
    }

    fn restore_scroll_anchor(&mut self) {
        if self.buffer.is_none() {
            return;
        }
        let top = self.position(self.scroll_anchor).0 as f32 * EDIT_LINE_H;
        self.scroll_y = (top + self.scroll_anchor_offset).clamp(0.0, self.max_scroll());
    }

    fn scrollbars_revealed(&self) -> bool {
        self.scrolled_at
            .is_some_and(|at| at.elapsed() < SCROLLBAR_SHOW_INTERVAL)
    }

    fn set_scroll_y(&mut self, scroll_y: f32) {
        let scroll_y = scroll_y.clamp(0.0, self.max_scroll());
        if (scroll_y - self.scroll_y).abs() > 0.01 {
            self.scrolled_at = Some(std::time::Instant::now());
        }
        self.scroll_y = scroll_y;
        let top_row = self.first_line();
        self.scroll_anchor = self.row_start_offset(top_row);
        self.scroll_anchor_offset = self.scroll_y - top_row as f32 * EDIT_LINE_H;
    }

    fn row(&self, row: usize) -> Option<DisplayRow> {
        self.rows.as_ref().and_then(|rows| rows.get(row).copied())
    }

    fn disp_count(&self) -> usize {
        self.rows
            .as_ref()
            .map(|rows| rows.len())
            .unwrap_or_else(|| self.line_count())
    }

    /// Buffer line shown on a display row.
    fn buf_of(&self, row: usize) -> usize {
        self.row(row).map_or(row, |r| r.line)
    }

    /// First display row of a buffer line (the fold header's row for a hidden line).
    fn disp_of(&self, line: usize) -> usize {
        self.line_rows.get(line).copied().unwrap_or(line)
    }

    fn last_row_of_line(&self, line: usize) -> usize {
        let mut row = self.disp_of(line);
        while self
            .row(row + 1)
            .is_some_and(|next| next.line == line && !next.is_virtual())
        {
            row += 1;
        }
        row
    }

    /// Whether `row` ends at a soft break (its line continues on the next row).
    fn soft_break_after(&self, row: usize) -> bool {
        match (self.row(row), self.row(row + 1)) {
            (Some(this), Some(next)) => {
                next.line == this.line && !this.is_virtual() && !next.is_virtual()
            }
            _ => false,
        }
    }

    fn display_offset(&self, line: usize, index: usize) -> usize {
        let Some(b) = self.buffer.as_ref() else {
            return 0;
        };
        b.offset_at(line, Self::char_col_for_display_index(b, line, index))
    }

    fn row_start_offset(&self, row: usize) -> usize {
        match self.row(row) {
            Some(r) => self.display_offset(r.line, r.start),
            None => self.buffer.as_ref().map_or(0, |b| b.rope.len_chars()),
        }
    }

    /// End of the text a display line shows: past a fold, the end of the row holding the fold's end.
    fn display_line_end(&self, line: usize) -> usize {
        let Some(b) = self.buffer.as_ref() else {
            return 0;
        };
        let line_end = |line: usize| b.rope.line_to_char(line) + b.line_len(line);
        let folds = self.folds.merged();
        let mut end = line_end(line);
        while let Some(fold) = folds.iter().find(|f| f.start <= end && end < f.end) {
            end = line_end(b.rope.char_to_line(fold.end));
        }
        end
    }

    /// Last caret position on `row`: before a soft break that's the char before it.
    fn row_end_offset(&self, row: usize) -> usize {
        let Some(r) = self.row(row) else {
            return self.buffer.as_ref().map_or(0, |b| b.rope.len_chars());
        };
        if self.soft_break_after(row) {
            self.display_offset(r.line, r.end).saturating_sub(1)
        } else {
            self.display_line_end(r.line)
        }
    }

    /// The display row and x (text-local px) of a buffer offset. An offset on a soft break shows at the start of
    /// the following row; one after a fold shows in the fold header row's tail.
    fn position(&self, offset: usize) -> (usize, f32) {
        let Some(b) = self.buffer.as_ref() else {
            return (0, 0.0);
        };
        let (line, col) = b.line_col_of(offset.min(b.rope.len_chars()));
        let index = Self::display_index(b, line, col);
        let first = self.disp_of(line);
        let Some(first_row) = self.row(first) else {
            return (first, 0.0);
        };
        if first_row.line != line {
            return (first, self.tail_x(first_row.line, line, index));
        }
        let mut row = first;
        while self
            .row(row + 1)
            .is_some_and(|next| next.line == line && !next.is_virtual() && next.start <= index)
        {
            row += 1;
        }
        (row, self.x_in_row(row, index))
    }

    fn x_in_row(&self, row: usize, index: usize) -> f32 {
        let Some(r) = self.row(row) else {
            return 0.0;
        };
        let layout = self.line_layout(r.line);
        layout.x_for_index(index) - layout.x_for_index(r.start) + r.indent as f32 * char_advance()
    }

    fn fold_pill_w() -> f32 {
        3.0 * char_advance() + 2.0 * FOLD_PILL_PAD
    }

    /// x of display `index` on `line`, drawn in the tail of the folded `header` row.
    fn tail_x(&self, header: usize, line: usize, index: usize) -> f32 {
        let Some(b) = self.buffer.as_ref() else {
            return 0.0;
        };
        let Some(fold) = self.folds.fold_on_row(b, header) else {
            return self.line_layout(header).width;
        };
        let end_line = b.rope.char_to_line(fold.end);
        let skip = Self::display_index(b, end_line, fold.end - b.rope.line_to_char(end_line));
        let layout = self.line_layout(line);
        self.line_layout(header).width
            + Self::fold_pill_w()
            + (layout.x_for_index(index) - layout.x_for_index(skip)).max(0.0)
    }

    fn row_width(&self, row: usize) -> f32 {
        let Some(r) = self.row(row) else {
            return 0.0;
        };
        if !self.soft_break_after(row) {
            if let Some(b) = self.buffer.as_ref() {
                if self.folds.fold_on_row(b, r.line).is_some() {
                    return self.position(self.display_line_end(r.line)).1;
                }
            }
        }
        self.x_in_row(row, r.end)
    }

    /// The buffer offset under text-local `x` on `row`. A click in a continuation row's indent, or past the end
    /// of a row before a soft break, lands just before the break; vertical motion lands at the row's start.
    fn offset_for_row_x(&self, row: usize, x: f32, click: bool) -> usize {
        let Some(r) = self.row(row) else {
            return self.buffer.as_ref().map_or(0, |b| b.rope.len_chars());
        };
        let em = char_advance();
        let indent_w = r.indent as f32 * em;
        if click && r.start > 0 && x < indent_w {
            return self.display_offset(r.line, r.start).saturating_sub(1);
        }
        let layout = self.line_layout(r.line);
        let local = (x - indent_w).max(0.0) + layout.x_for_index(r.start);
        let index = layout.closest_index_for_x(local).clamp(r.start, r.end);
        let offset = self.display_offset(r.line, index);
        if index >= r.end && self.soft_break_after(row) {
            offset.saturating_sub(1)
        } else {
            offset
        }
    }

    /// Multi-cursor and syntax-selection commands; returns whether `key` was one of them.
    fn run_selection_command(&mut self, key: EditKey) -> bool {
        let add = match key {
            EditKey::AddCursorAbove => Some((true, true)),
            EditKey::AddCursorBelow => Some((false, true)),
            EditKey::AddCursorAboveRow => Some((true, false)),
            EditKey::AddCursorBelowRow => Some((false, false)),
            _ => None,
        };
        if !matches!(
            key,
            EditKey::SelectNext
                | EditKey::SelectAllMatches
                | EditKey::SelectLargerSyntaxNode
                | EditKey::SelectSmallerSyntaxNode
        ) && add.is_none()
        {
            return false;
        }
        self.refresh();
        self.ensure_visible();
        let mut revealed = Vec::new();
        if let Some((above, skip_soft_wrap)) = add {
            let plan = self
                .buffer
                .as_ref()
                .map(|b| b.plan_add_selection(above, skip_soft_wrap, &EditorRows::new(self)));
            if let (Some(b), Some(plan)) = (self.buffer.as_mut(), plan) {
                b.apply_add_selection(plan);
            }
        } else if let Some(b) = self.buffer.as_mut() {
            match key {
                EditKey::SelectNext => revealed.extend(b.select_next(false)),
                EditKey::SelectAllMatches => revealed = b.select_all_matches(),
                EditKey::SelectSmallerSyntaxNode => {
                    b.select_smaller_syntax_node();
                }
                _ => {
                    let folds = self.folds.merged();
                    let syntax = &self.syntax;
                    let rope = b.rope.clone();
                    let ancestor = |range: Range<usize>| {
                        let bytes = rope.char_to_byte(range.start)..rope.char_to_byte(range.end);
                        syntax.as_ref()?.syntax_ancestor(bytes).map(|node| {
                            let chars = rope.byte_to_char(node.range.start)
                                ..rope.byte_to_char(node.range.end);
                            (chars, node.kind, node.named)
                        })
                    };
                    let intersects_fold =
                        |offset: usize| folds.iter().any(|f| f.start < offset && offset < f.end);
                    b.select_larger_syntax_node(&ancestor, &intersects_fold);
                }
            }
        }
        // A match hidden inside a fold is revealed.
        let mut unfolded = false;
        for range in revealed {
            unfolded |= !self.folds.take_overlapping(range).is_empty();
        }
        if unfolded {
            self.rows = None;
            self.ensure_visible();
        }
        self.ensure_cursor_visible();
        true
    }

    /// Plan a line-wise command against the display rows, apply it, and carry folds inside moved lines along.
    fn run_line_command(&mut self, command: LineCommand) {
        self.refresh();
        self.ensure_visible();
        let plan = self.buffer.as_ref().map(|b| {
            let rows = EditorRows::new(self);
            match command {
                LineCommand::Delete => b.plan_delete_lines(&rows),
                LineCommand::Duplicate { up } => b.plan_duplicate(up, true, &rows),
                LineCommand::Move { up } => b.plan_move_lines(up, &rows),
                LineCommand::Manipulate(transform) => b.plan_manipulate_lines(transform, &rows),
            }
        });
        let Some(plan) = plan else {
            return;
        };
        let mut refolds = Vec::new();
        if let Some(b) = self.buffer.as_ref() {
            for (range, delta) in &plan.moved {
                for fold in self.folds.take_overlapping(range.clone()) {
                    let shift = |offset: usize| {
                        let (row, column) = b.line_col_of(offset);
                        ((row as isize + delta).max(0) as usize, column)
                    };
                    refolds.push((shift(fold.start), shift(fold.end)));
                }
            }
        }
        if let Some(b) = self.buffer.as_mut() {
            b.apply_line_plan(plan);
        }
        self.refresh();
        if let Some(b) = self.buffer.as_ref() {
            for ((start_row, start_col), (end_row, end_col)) in refolds {
                let range = b.offset_at(start_row, start_col)..b.offset_at(end_row, end_col);
                self.folds.fold(b, range);
            }
        }
        self.rows = None;
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn do_toggle_fold(&mut self, row: usize) {
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        if self.folds.fold_on_row(b, row).is_some() {
            self.folds.unfold_row(b, row);
        } else if let Some(range) = editor::fold::fold_range_for_row(b, self.syntax.as_ref(), row) {
            self.folds.fold(b, range);
        } else {
            return;
        }
        self.rows = None;
    }

    /// Colored runs of the text after the fold headed by `row`, or `None` when `row` isn't folded.
    fn fold_tail(&self, row: usize, colors: &Theme) -> Option<Vec<(String, Rgba)>> {
        let b = self.buffer.as_ref()?;
        let fold = self.folds.fold_on_row(b, row)?;
        let end_row = b.rope.char_to_line(fold.end);
        let skip = Self::display_index(b, end_row, fold.end - b.rope.line_to_char(end_row));
        let mut consumed = 0usize;
        let mut tail = Vec::new();
        for (text, color) in self.line_segments(end_row, colors) {
            let from = skip.saturating_sub(consumed).min(text.len());
            consumed += text.len();
            if from < text.len() {
                tail.push((text[from..].to_string(), color));
            }
        }
        Some(tail)
    }

    /// Total content height (px), in display rows.
    fn content_h(&self) -> f32 {
        self.disp_count() as f32 * EDIT_LINE_H
    }

    /// The last row may scroll up to the top of the viewport.
    fn max_scroll(&self) -> f32 {
        (self.disp_count().saturating_sub(1)) as f32 * EDIT_LINE_H
    }

    /// The text viewport width (px): the body minus the padding and the fixed gutter.
    fn text_viewport_w(&self) -> f32 {
        (self.body_w - gutter_width(self.line_count()) - SCROLLBAR_WIDTH).max(0.0)
    }

    /// Total text width (px) of the widest line, plus a trailing column so the last glyph isn't flush to the edge.
    fn content_w(&self) -> f32 {
        (self.max_row_cols + 1) as f32 * char_advance()
    }

    /// The largest valid horizontal scroll (so the widest line's end can reach the right edge).
    fn max_scroll_x(&self) -> f32 {
        (self.content_w() - self.text_viewport_w()).max(0.0)
    }

    /// First visible line index, and the sub-line offset the shell shifts the body up by (in `(-line_h, 0]`).
    fn first_line(&self) -> usize {
        (self.scroll_y / EDIT_LINE_H).floor().max(0.0) as usize
    }

    /// Scroll just enough to show the selections with up to `VERTICAL_SCROLL_MARGIN` rows around them, falling
    /// back to the newest selection when they don't all fit.
    fn autoscroll_vertically(&mut self) {
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        let (Some(first), Some(last)) = (b.selections().first(), b.selections().last()) else {
            return;
        };
        let row_of = |offset: usize| self.position(offset).0 as f32;
        let visible_lines = self.body_h / EDIT_LINE_H;
        let mut target_top = row_of(first.head());
        let mut target_bottom = row_of(last.head()) + 1.0;
        if target_bottom - target_top > visible_lines {
            target_top = row_of(b.newest().head());
            target_bottom = target_top + 1.0;
        }
        let margin = ((visible_lines - (target_bottom - target_top)) / 2.0)
            .floor()
            .clamp(0.0, VERTICAL_SCROLL_MARGIN);
        let target_top = (target_top - margin).max(0.0);
        let target_bottom = target_bottom + margin;
        let start_row = self.scroll_y / EDIT_LINE_H;
        let needs_up = target_top < start_row;
        let needs_down = target_bottom >= start_row + visible_lines;
        if needs_up && !needs_down {
            self.set_scroll_y(target_top * EDIT_LINE_H);
        } else if needs_down && !needs_up {
            self.set_scroll_y((target_bottom - visible_lines) * EDIT_LINE_H);
        }
    }

    /// Scroll so each on-screen selection's span on its head row (or, if that's wider than the view, the head)
    /// is visible.
    fn autoscroll_horizontally(&mut self) {
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        let first_row = self.first_line();
        let last_row = first_row + (self.body_h / EDIT_LINE_H).ceil() as usize + 1;
        let em = char_advance();
        let viewport = self.text_viewport_w();
        let (mut target_left, mut target_right) = (f32::INFINITY, 0.0f32);
        for selection in b.selections() {
            let (head_row, head_x) = self.position(selection.head());
            if !(first_row..last_row).contains(&head_row) {
                continue;
            }
            let (start_row, start_x) = self.position(selection.start);
            let (end_row, end_x) = self.position(selection.end);
            let row_width = self.row_width(head_row);
            let start_x = if start_row == head_row { start_x } else { 0.0 };
            let end_x = if end_row == head_row {
                end_x
            } else {
                row_width
            }
            .min(row_width);
            let (mut left, mut right) = (start_x, end_x + em);
            if right - left > viewport {
                left = head_x;
                right = head_x.min(row_width) + em;
            }
            target_left = target_left.min(left);
            target_right = target_right.max(right);
        }
        let target_right = target_right.min(self.content_w());
        if target_right - target_left > viewport {
            return;
        }
        if target_left < self.scroll_x {
            self.scroll_x = target_left;
        } else if target_right > self.scroll_x + viewport {
            self.scroll_x = target_right - viewport;
        }
        self.scroll_x = self.scroll_x.clamp(0.0, self.max_scroll_x());
    }

    fn ensure_cursor_visible(&mut self) {
        self.autoscroll_vertically();
        self.autoscroll_horizontally();
    }

    /// Map a content-local pointer position (before the gutter, before scroll) to a buffer offset.
    fn offset_at_local(&self, local_x: f32, local_y: f32) -> Option<usize> {
        let b = self.buffer.as_ref()?;
        let gw = gutter_width(self.line_count());
        let row = ((self.scroll_y + local_y) / EDIT_LINE_H).max(0.0) as usize;
        if row >= self.disp_count() {
            return Some(b.rope.len_chars());
        }
        let x = (local_x - gw + self.scroll_x).max(0.0);
        Some(self.offset_for_row_x(row, x, true))
    }

    /// Leading-whitespace width of a line in visual columns (tabs -> next `TAB_COLS` stop), or `None` if the line
    /// is blank (only whitespace). Blank lines have no indent of their own -- guides span across them.
    fn indent_cols(b: &EditorBuffer, line: usize) -> Option<usize> {
        if line >= b.rope.len_lines() {
            return None;
        }
        let mut cols = 0usize;
        for ch in b.rope.line(line).chars() {
            match ch {
                ' ' => cols += 1,
                '\t' => cols += TAB_COLS - (cols % TAB_COLS),
                '\n' | '\r' => return None,
                _ => return Some(cols),
            }
        }
        None
    }

    /// The block around buffer `row` for highlighting its indent guide: the rows between the nearest
    /// less-indented lines above and below (a row that opens a deeper block counts as inside it; a blank row
    /// takes the deeper of its non-blank neighbours), with the guide's column.
    fn enclosing_indent(b: &EditorBuffer, mut row: usize) -> Option<(usize, usize, usize)> {
        const SEARCH_ROW_LIMIT: usize = 25_000;
        const SEARCH_WHITESPACE_ROW_LIMIT: usize = 2_500;
        let max_row = b.rope.len_lines().saturating_sub(1);
        if row >= max_row {
            return None;
        }
        let mut indent = Self::indent_cols(b, row);
        if let (Some(current), Some(next)) = (indent, Self::indent_cols(b, row + 1)) {
            if current < next {
                indent = Some(next);
                row += 1;
            }
        }
        let target = match indent {
            Some(target) => target,
            None => {
                let above = (row.saturating_sub(SEARCH_WHITESPACE_ROW_LIMIT)..row)
                    .rev()
                    .find_map(|r| Self::indent_cols(b, r).map(|i| (r, i)));
                let below = (row..=(row + SEARCH_WHITESPACE_ROW_LIMIT).min(max_row))
                    .find_map(|r| Self::indent_cols(b, r).map(|i| (r, i)));
                let (found_row, found) = match (above, below) {
                    (Some(a), Some(b)) => {
                        if a.1 >= b.1 {
                            a
                        } else {
                            b
                        }
                    }
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => return None,
                };
                row = found_row;
                found
            }
        };
        let (start_row, start_indent) = (row.saturating_sub(SEARCH_ROW_LIMIT)..row)
            .rev()
            .find_map(|r| {
                Self::indent_cols(b, r)
                    .filter(|i| *i < target)
                    .map(|i| (r, i))
            })?;
        let limit = (row + SEARCH_ROW_LIMIT).min(max_row);
        let (end_row, end_indent) = (row..=limit)
            .find_map(|r| {
                Self::indent_cols(b, r)
                    .filter(|i| *i < target)
                    .map(|i| (r.saturating_sub(1), Some(i)))
            })
            .unwrap_or((limit, None));
        let column = match end_indent {
            Some(end) if start_indent <= end => end,
            _ => start_indent,
        };
        Some((start_row, end_row, column))
    }

    /// Indent-guide segments `(start_row, end_row_inclusive, depth)` visible in `[first, last)`, ported from the
    /// reference's stack scan: a blank line inherits the depth of the next non-blank line so a guide stays
    /// continuous through blank lines; a guide closes when the indent drops below its level.
    fn indent_guides(&self, first: usize, last: usize) -> Vec<(usize, usize, usize)> {
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        let n = b.rope.len_lines();
        if n == 0 || first >= n {
            return Vec::new();
        }
        let end_row = last.min(n).saturating_sub(1);
        const TRAILING_LIMIT: usize = 25;
        let mut result: Vec<(usize, usize, usize)> = Vec::new();
        let mut stack: Vec<(usize, usize, usize)> = Vec::new(); // (start_row, end_row, depth)
        let mut row = first;
        while row <= end_row {
            let first_row = row;
            let mut last_row = row;
            let (depth, _found) = match Self::indent_cols(b, row) {
                Some(c) => (c / TAB_COLS, true),
                None => {
                    // Blank: seek forward to the next non-blank line (bounded) and adopt its depth.
                    let mut d = 0usize;
                    let mut found = false;
                    let mut r = row + 1;
                    while r < n && r <= end_row + TRAILING_LIMIT {
                        if let Some(c) = Self::indent_cols(b, r) {
                            last_row = r.min(end_row);
                            d = c / TAB_COLS;
                            found = true;
                            break;
                        }
                        r += 1;
                    }
                    row = r; // consume the scanned blanks (+ the found line, re-processed as this block's start)
                    (d, found)
                }
            };
            let current_depth = stack.len();
            if depth < current_depth {
                for _ in 0..(current_depth - depth) {
                    let mut ind = stack.pop().unwrap();
                    if last_row != first_row {
                        ind.1 = first_row.saturating_sub(1);
                    }
                    result.push(ind);
                }
            } else if depth > current_depth {
                for nd in current_depth..depth {
                    stack.push((first_row, last_row, nd));
                }
            }
            for ind in stack.iter_mut() {
                ind.1 = last_row;
            }
            row += 1;
        }
        result.extend(stack);
        result
    }
    /// Display column (tabs expanded) of buffer column `char_col` on `line`.
    fn vis_col(b: &EditorBuffer, line: usize, char_col: usize) -> usize {
        if line >= b.rope.len_lines() {
            return char_col;
        }
        editor::display::to_display(b.rope.line(line), char_col, TAB_COLS).0
    }
    /// Byte index into the tab-expanded display text of `line` for buffer column `char_col`.
    fn display_index(b: &EditorBuffer, line: usize, char_col: usize) -> usize {
        if line >= b.rope.len_lines() {
            return 0;
        }
        editor::display::to_display(b.rope.line(line), char_col, TAB_COLS).1
    }
    /// Buffer column for a display byte index; a position inside a tab resolves to before it.
    fn char_col_for_display_index(b: &EditorBuffer, line: usize, index: usize) -> usize {
        if line >= b.rope.len_lines() {
            return 0;
        }
        editor::display::from_display_index(b.rope.line(line), index, TAB_COLS, Bias::Left)
    }

    /// The shaped display row for buffer `line`, laid out from the same colored segments the renderer draws
    /// (each segment is its own label placed after the previous one). Cached until the text changes.
    fn line_layout(&self, line: usize) -> std::rc::Rc<LineLayout> {
        if let Some(hit) = self.layout_cache.borrow().get(&line) {
            return hit.clone();
        }
        let colors = syntax_theme();
        let mut layout = LineLayout::default();
        for (segment, _) in self.line_segments(line, &colors) {
            let (glyphs, width) = ui::measure_glyphs(&segment, EDIT_FONT, true, 400);
            let (base_index, base_x) = (layout.len, layout.width);
            layout
                .glyphs
                .extend(glyphs.iter().map(|(i, x)| (base_index + i, base_x + x)));
            layout.len += segment.len();
            layout.width += width;
        }
        let layout = std::rc::Rc::new(layout);
        self.layout_cache.borrow_mut().insert(line, layout.clone());
        layout
    }
}

impl FileItem {
    fn selection_band_tris(&self, content: Rect) -> Vec<ui::Tri> {
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        let gw = gutter_width(self.line_count());
        let first = self.first_line();
        let last = (first + (self.body_h / EDIT_LINE_H).ceil() as usize + 2).min(self.disp_count());
        // Rounded connected selection shape (reference metrics): radius 0.15*line, right-edge overshoot 0.3*line.
        let radius = 0.15 * EDIT_LINE_H;
        let overshoot = 0.3 * EDIT_LINE_H;
        let text_x = content.x + gw - self.scroll_x;
        // The reference tints the selection with the player color (a translucent blue) behind the text, not the
        // opaque gray UI-list selection.
        let st = syntax_theme();
        let [sr, sg, sb] = st.selection.0;
        // Selections dim while the editor isn't focused.
        let unfocused_opacity = if self.focused { 1.0 } else { 0.5 };
        let sel_color = Rgba::new(
            sr as f32 / 255.0,
            sg as f32 / 255.0,
            sb as f32 / 255.0,
            st.selection_alpha * unfocused_opacity,
        );
        let mut tris = Vec::new();
        for s in b.selections() {
            let Some((a, e)) = s.range() else { continue };
            let mut rows: Vec<(f32, f32)> = Vec::new();
            let mut top_row = None;
            for row in first..last {
                let row_start = self.row_start_offset(row);
                let soft = self.soft_break_after(row);
                let row_next = match self.row(row + 1) {
                    Some(next) if soft => self.display_offset(next.line, next.start),
                    _ => self.display_line_end(self.buf_of(row)) + 1,
                };
                if a >= row_next || e <= row_start {
                    continue;
                }
                top_row.get_or_insert(row);
                let start_x = if a > row_start {
                    self.position(a).1
                } else {
                    0.0
                };
                // A band running past a hard line end overshoots so the block reads as one shape.
                let end_x = if e < row_next {
                    self.position(e).1
                } else if soft {
                    self.row_width(row)
                } else {
                    self.row_width(row) + overshoot
                };
                rows.push((text_x + start_x, text_x + end_x.max(start_x + 1.0)));
            }
            let Some(top) = top_row else { continue };
            let top_y = content.y + top as f32 * EDIT_LINE_H - self.scroll_y;
            tris.extend(ui::selection_path(
                &rows,
                top_y,
                EDIT_LINE_H,
                radius,
                sel_color,
            ));
        }
        tris
    }

    /// Whitespace inside non-empty selections is marked: a dot centered in each space's cell and an arrow
    /// centered in each tab's, drawn at half the text size above the selection band.
    fn invisible_tris(&self, content: Rect) -> Vec<ui::Tri> {
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        let selected: Vec<(usize, usize)> =
            b.selections().iter().filter_map(|s| s.range()).collect();
        if selected.is_empty() {
            return Vec::new();
        }
        let gw = gutter_width(self.line_count());
        let first = self.first_line();
        let last = (first + (self.body_h / EDIT_LINE_H).ceil() as usize + 2).min(self.disp_count());
        let color = theme().editor_invisible;
        let symbol = EDIT_FONT / 2.0;
        let mut tris = Vec::new();
        for row_index in first..last {
            let Some(row) = self.row(row_index) else {
                break;
            };
            if row.is_virtual() {
                continue;
            }
            let line_start = b.rope.line_to_char(row.line);
            let layout = self.line_layout(row.line);
            let row_y = content.y + row_index as f32 * EDIT_LINE_H - self.scroll_y;
            let center_y = row_y + EDIT_LINE_H / 2.0;
            let origin_x = content.x + gw - self.scroll_x - layout.x_for_index(row.start)
                + row.indent as f32 * char_advance();
            let mut index = 0usize;
            for (char_col, ch) in b.rope.line(row.line).chars().enumerate() {
                if ch == '\n' || ch == '\r' {
                    break;
                }
                let width = if ch == '\t' {
                    editor::display::to_display(b.rope.line(row.line), char_col + 1, TAB_COLS).1
                        - index
                } else {
                    ch.len_utf8()
                };
                let offset = line_start + char_col;
                let in_row = index >= row.start && index < row.end;
                let is_selected = selected.iter().any(|(s, e)| *s <= offset && offset < *e);
                if in_row && is_selected && (ch == ' ' || ch == '\t') {
                    let x0 = origin_x + layout.x_for_index(index);
                    let x1 = origin_x + layout.x_for_index(index + width);
                    let center_x = (x0 + x1) / 2.0;
                    if ch == ' ' {
                        tris.extend(dot_tris(center_x, center_y, symbol * 0.2, color));
                    } else {
                        tris.extend(arrow_tris(center_x, center_y, symbol * 0.6, color));
                    }
                }
                index += width;
            }
        }
        tris
    }
}

impl FileItem {
    fn cursor_status_text(&self) -> Option<String> {
        let b = self.buffer.as_ref()?;
        let newest = b.newest();
        let (line, column) = b.line_col_of(newest.head());
        let mut text = format!("{}:{}", line + 1, column + 1);
        let (mut lines, mut characters) = (0usize, 0usize);
        for s in b.selections() {
            characters += s.end - s.start;
            if !s.is_empty() {
                let (start_row, _) = b.line_col_of(s.start);
                let (end_row, end_col) = b.line_col_of(s.end);
                lines += end_row - start_row + usize::from(end_col != 0);
            }
        }
        let selections = b.selections().len();
        let parts: Vec<String> = [
            (selections > 1).then_some((selections, "selection")),
            (lines > 1).then_some((lines, "line")),
            (characters > 0).then_some((characters, "character")),
        ]
        .into_iter()
        .flatten()
        .map(|(count, name)| format!("{count} {name}{}", if count > 1 { "s" } else { "" }))
        .collect();
        if !parts.is_empty() {
            text.push_str(&format!(" ({})", parts.join(", ")));
        }
        Some(text)
    }

    /// Bracket pairs around `range` (chars), as char ranges.
    fn enclosing_brackets(&self, range: Range<usize>) -> Vec<(Range<usize>, Range<usize>)> {
        let (Some(syntax), Some(b)) = (self.syntax.as_ref(), self.buffer.as_ref()) else {
            return Vec::new();
        };
        let rope = &b.rope;
        let len = rope.len_chars();
        let to_byte = |offset: usize| rope.char_to_byte(offset.min(len));
        let to_chars =
            |bytes: Range<usize>| rope.byte_to_char(bytes.start)..rope.byte_to_char(bytes.end);
        syntax
            .enclosing_bracket_ranges(rope, to_byte(range.start)..to_byte(range.end))
            .into_iter()
            .map(|(open, close)| (to_chars(open), to_chars(close)))
            .collect()
    }

    /// The innermost pair around an empty newest selection, highlighted while the caret sits in it.
    fn matching_brackets(&self) -> Vec<Range<usize>> {
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        let newest = b.newest();
        if !newest.is_empty() {
            return Vec::new();
        }
        let head = newest.head();
        self.enclosing_brackets(head..head)
            .into_iter()
            .min_by_key(|(open, close)| close.end - open.start)
            .map(|(open, close)| vec![open, close])
            .unwrap_or_default()
    }

    /// The text as it stands, to read syntax scopes from while the buffer itself is being edited.
    fn rope_snapshot(&self) -> ropey::Rope {
        self.buffer
            .as_ref()
            .map(|b| b.rope.clone())
            .unwrap_or_default()
    }

    /// The display row the blame annotation trails: the newest caret's, while focused and off a blank line.
    fn inline_blame_row(&self) -> Option<usize> {
        let b = self.buffer.as_ref()?;
        if !self.focused || self.git.blame().is_empty() {
            return None;
        }
        let head = b.newest().head();
        let line = b.rope.char_to_line(head.min(b.rope.len_chars()));
        (b.line_len(line) > 0).then(|| self.position(head).0)
    }

    /// `author, when` for the newest caret's line.
    fn inline_blame_text(&self) -> Option<String> {
        let b = self.buffer.as_ref()?;
        let line = b
            .rope
            .char_to_line(b.newest().head().min(b.rope.len_chars()));
        git::entry_for_row(self.git.blame(), line).map(git::inline_text)
    }

    fn close_completions(&mut self) {
        self.completions = None;
        self.completions_forced = false;
        self.completion_request = None;
    }

    fn update_completions(&mut self, trigger: CompletionTrigger) {
        let trigger = match trigger {
            CompletionTrigger::Character(_) if self.lsp_triggers.is_none() => {
                CompletionTrigger::Refilter
            }
            trigger => trigger,
        };
        if self
            .completions
            .as_ref()
            .is_some_and(|menu| menu.is_choices())
        {
            self.completions = None;
            if trigger == CompletionTrigger::Refilter {
                return;
            }
        }
        if self.lsp_triggers.is_some() && trigger != CompletionTrigger::ShowWords {
            return self.update_server_completions(trigger);
        }
        let menu_open = self.completions.is_some();
        if trigger == CompletionTrigger::Refilter && !menu_open {
            return;
        }
        let Some(b) = self.buffer.as_ref() else {
            return self.close_completions();
        };
        let newest = b.newest();
        let word_query = newest
            .is_empty()
            .then(|| editor::completion::completion_query(b, newest.head()))
            .flatten();
        let explicit = matches!(
            trigger,
            CompletionTrigger::Show | CompletionTrigger::ShowWords
        );
        if !newest.is_empty() || (word_query.is_none() && !explicit) {
            return self.close_completions();
        }
        self.completions_forced |= trigger == CompletionTrigger::ShowWords;
        let head = newest.head();
        let query = word_query.as_ref().map_or("", |(_, q)| q.as_str());
        let words_allowed = !matches!(self.lang, Lang::Markdown | Lang::PlainText);
        let mut words = Vec::new();
        if let Some((word_start, query)) = word_query.as_ref().filter(|_| words_allowed) {
            if self.completions_forced || query.chars().count() >= 3 {
                let mut word_end = head;
                while word_end < b.rope.len_chars() && {
                    let c = b.rope.char(word_end);
                    c.is_alphanumeric() || c == '_'
                } {
                    word_end += 1;
                }
                let whole_word = b.rope.slice(*word_start..word_end).to_string();
                let row = b.rope.char_to_line(head);
                let skip_digits = !query.chars().any(|c| c.is_ascii_digit());
                words = editor::completion::buffer_words(b, row, Some(&whole_word), skip_digits);
            }
        }
        let mut snippets = Vec::new();
        if !self.completions_forced {
            let definitions = self
                .snippet_dir
                .as_deref()
                .map(|dir| snippet_store::snippets_for(dir, self.lang))
                .unwrap_or_default();
            let needs_strong_match = trigger == CompletionTrigger::Typed && !menu_open;
            let strong = || {
                editor::snippet::has_strong_prefix_match(
                    query,
                    definitions
                        .iter()
                        .flat_map(|d| d.prefixes.iter().map(String::as_str)),
                )
            };
            if !definitions.is_empty() && (!needs_strong_match || strong()) {
                let before = b.rope.slice(head.saturating_sub(256)..head).to_string();
                snippets = completions_menu::match_snippets(&before, &definitions);
            }
        }
        self.completions = completions_menu::CompletionsMenu::new(query, words, snippets);
        if self.completions.is_none() {
            self.completions_forced = false;
        } else {
            self.hide_hover();
        }
    }

    fn confirm_completion(&mut self, row: Option<usize>) {
        let Some(menu) = self.completions.take() else {
            return;
        };
        self.completions_forced = false;
        let completion = match row {
            Some(row) => menu.completion_at(row),
            None => menu.selected_completion(),
        };
        let (Some(completion), Some(b)) = (completion.cloned(), self.buffer.as_mut()) else {
            return;
        };
        match completion.kind {
            completions_menu::CompletionKind::Word => {
                let newest_query =
                    editor::completion::completion_query(b, b.newest().head()).map(|(_, q)| q);
                let edits: Vec<(Range<usize>, String)> = b
                    .selections()
                    .iter()
                    .filter(|s| s.is_empty())
                    .filter_map(|s| {
                        let (start, query) = editor::completion::completion_query(b, s.head())?;
                        (Some(&query) == newest_query.as_ref())
                            .then(|| (start..s.head(), completion.label.clone()))
                    })
                    .collect();
                b.replace_ranges(edits);
            }
            completions_menu::CompletionKind::Snippet { snippet, replaced } => {
                let head = b.newest().head();
                let matched = b
                    .rope
                    .slice(head.saturating_sub(replaced)..head)
                    .to_string();
                let ranges: Vec<Range<usize>> = b
                    .selections()
                    .iter()
                    .filter(|s| s.is_empty() && s.head() >= replaced)
                    .map(|s| s.head() - replaced..s.head())
                    .filter(|range| b.rope.slice(range.clone()) == matched.as_str())
                    .collect();
                if let Some(choices) = b.insert_snippet(&ranges, &snippet.body) {
                    self.completions = completions_menu::CompletionsMenu::choices(choices);
                }
            }
            completions_menu::CompletionKind::Lsp {
                item,
                synced_version,
            } => {
                self.confirm_server_completion(&item, synced_version);
                return;
            }
            completions_menu::CompletionKind::Choice => {
                let edits = b
                    .selections()
                    .iter()
                    .map(|s| (s.start..s.end, completion.label.clone()))
                    .collect();
                b.replace_ranges(edits);
                let collapsed = b
                    .selections()
                    .iter()
                    .map(|s| {
                        let mut s = *s;
                        s.collapse_to(s.end, SelectionGoal::None);
                        s
                    })
                    .collect();
                b.set_selections(collapsed);
            }
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn move_to_snippet_stop(&mut self, forward: bool) -> bool {
        let Some(b) = self.buffer.as_mut() else {
            return false;
        };
        let Some(choices) = b.move_to_snippet_stop(forward) else {
            return false;
        };
        self.completions = choices.and_then(completions_menu::CompletionsMenu::choices);
        self.ensure_cursor_visible();
        true
    }

    /// The word menu and its window position: under the caret's line, or above it when that has more room
    /// and the menu doesn't fit below.
    /// Where the menu goes: `(x, y, height, above)`, above the caret's line when that has more room and the
    /// menu doesn't fit below.
    fn completion_menu_place(&self, content: Rect) -> Option<(f32, f32, f32, bool)> {
        let menu = self.completions.as_ref()?;
        let b = self.buffer.as_ref()?;
        let (row, x) = self.position(b.newest().head());
        let x = content.x + gutter_width(self.line_count()) + x - self.scroll_x;
        let row_top = content.y + row as f32 * EDIT_LINE_H - self.scroll_y;
        let height = menu.height() * ui::ui_text_scale();
        let below = row_top + EDIT_LINE_H;
        let room_below = content.y + self.body_h - below;
        let room_above = row_top - content.y;
        if height > room_below && room_above > room_below {
            Some((x, row_top - height, height, true))
        } else {
            Some((x, below, height, false))
        }
    }

    fn completion_menu_popover(&self, content: Rect) -> Option<(Node, f32, f32)> {
        let menu = self.completions.as_ref()?;
        let (x, y, _, _) = self.completion_menu_place(content)?;
        Some((menu.render(COMPLETION_BASE), x, y))
    }

    /// The selected item's documentation: right of the menu when there is room for it, else on the menu's
    /// side of the caret line (below it first), else on the other side.
    fn completion_aside_popover(
        &self,
        content: Rect,
        viewport: (f32, f32),
    ) -> Option<(Node, f32, f32)> {
        use completions_menu::{ASIDE_MAX_WIDTH, ASIDE_MIN_WIDTH, MAX_VISIBLE, MENU_GAP, WIDTH};
        let menu = self.completions.as_ref()?;
        let (x, y, height, above) = self.completion_menu_place(content)?;
        let scale = ui::ui_text_scale();
        let width = WIDTH * scale;
        let gap = MENU_GAP * scale;
        let (mut top, mut bottom) = (y - gap, y + height + gap);
        if above {
            bottom += EDIT_LINE_H;
        } else {
            top -= EDIT_LINE_H;
        }
        let right = x + width + gap;
        let max_menu_height = (MAX_VISIBLE as f32 * EDIT_LINE_H + 8.0) * scale;
        let room_right = viewport.0 - right;
        if room_right >= ASIDE_MIN_WIDTH * scale {
            let max_width = (room_right - 1.0).min(ASIDE_MAX_WIDTH * scale);
            let (node, _, _) = menu.render_aside(max_width / scale, max_menu_height / scale)?;
            return Some((node, right, y));
        }
        let (room_above, room_below) = (top, viewport.1 - bottom);
        let max_width = (width - 2.0).max(ASIDE_MIN_WIDTH * scale).min(viewport.0);
        let max_height = max_menu_height.min(room_above.max(room_below)) - 8.0 * scale;
        let (node, _, aside_height) = menu.render_aside(max_width / scale, max_height / scale)?;
        let aside_height = aside_height * scale;
        let below_fits = aside_height < room_below;
        let above_fits = aside_height < room_above;
        let aside_y = match (above, below_fits, above_fits) {
            (false, true, _) | (true, true, false) => bottom,
            (_, _, true) => top - aside_height,
            _ => return None,
        };
        Some((node, x, aside_y))
    }

    /// Hunks touching any selection's lines; a deletion counts when it sits right above or below them.
    fn hunks_in_selections(&self) -> Vec<git::DiffHunk> {
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        let mut picked: Vec<git::DiffHunk> = Vec::new();
        for selection in b.selections() {
            let first = b.rope.char_to_line(selection.start);
            let query = first..b.rope.char_to_line(selection.end) + 1;
            for hunk in self.git.hunks() {
                let touches = if hunk.rows.is_empty() {
                    hunk.rows.start == query.start || hunk.rows.start == query.end
                } else {
                    hunk.rows.start < query.end && query.start < hunk.rows.end
                };
                if touches && !picked.contains(hunk) {
                    picked.push(hunk.clone());
                }
            }
        }
        picked.sort_by_key(|hunk| hunk.rows.start);
        picked
    }

    /// Move the caret to the start of the next (or previous) change, wrapping around the file.
    fn go_to_hunk(&mut self, next: bool) {
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        let head = b.newest().head();
        let row = b.rope.char_to_line(head.min(b.rope.len_chars()));
        let line_start = |line: usize| {
            if line >= b.rope.len_lines() {
                b.rope.len_chars()
            } else {
                b.rope.line_to_char(line)
            }
        };
        let hunks = self.git.hunks();
        let target = if next {
            hunks
                .iter()
                .find(|hunk| hunk.rows.start > row)
                .or_else(|| hunks.iter().find(|hunk| hunk.rows.end < row))
        } else {
            hunks
                .iter()
                .rev()
                .find(|hunk| line_start(hunk.rows.end) < head)
                .or_else(|| hunks.last())
        };
        let Some(line) = target.map(|hunk| hunk.rows.start) else {
            return;
        };
        let offset = line_start(line);
        if !self.folds.take_overlapping(offset..offset + 1).is_empty() {
            self.rows = None;
        }
        if let Some(b) = self.buffer.as_mut() {
            b.place_cursor(offset);
        }
        self.scroll_line_to_center(line);
    }

    /// Stage or unstage the changes under the selections; `None` stages unless all of them already are.
    fn stage_hunks(&mut self, stage: Option<bool>) {
        let hunks = self.hunks_in_selections();
        if hunks.is_empty() {
            return;
        }
        let stage = stage.unwrap_or_else(|| hunks.iter().any(|hunk| !hunk.staged));
        self.write_staged(&hunks, stage);
    }

    /// A dirty buffer is saved first, since the index gets what the file holds.
    fn write_staged(&mut self, hunks: &[git::DiffHunk], stage: bool) {
        if self.is_dirty() {
            if let Err(error) = self.save() {
                eprintln!("git: {error}");
                return;
            }
        }
        let (Some(bases), Some(b)) = (self.git.bases(), self.buffer.as_ref()) else {
            return;
        };
        if let Some(index) = git::index_after(&bases, &b.text(), hunks, stage) {
            self.git.write_index(self.root.join(&self.path), index);
        }
    }

    /// Put the changes under the selections back to their committed text, unstaging them first.
    fn restore_hunks(&mut self) {
        let hunks = self.hunks_in_selections();
        let Some(bases) = self.git.bases() else {
            return;
        };
        if hunks.is_empty() {
            return;
        }
        let staged: Vec<git::DiffHunk> = hunks.iter().filter(|h| h.staged).cloned().collect();
        if !staged.is_empty() {
            self.write_staged(&staged, false);
        }
        let head = bases.head.clone().unwrap_or_default();
        let Some(b) = self.buffer.as_mut() else {
            return;
        };
        let line_start = |b: &EditorBuffer, line: usize| {
            if line >= b.rope.len_lines() {
                b.rope.len_chars()
            } else {
                b.rope.line_to_char(line)
            }
        };
        let edits: Vec<(Range<usize>, String)> = hunks
            .iter()
            .map(|hunk| {
                (
                    line_start(b, hunk.rows.start)..line_start(b, hunk.rows.end),
                    git::line_text(&head, hunk.base_rows.clone()),
                )
            })
            .collect();
        b.replace_ranges(edits);
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    /// Where a change sits in the text, as chars: its lines, or the point its removal happened at.
    fn hunk_char_range(&self, hunk: &git::DiffHunk) -> Range<usize> {
        let Some(b) = self.buffer.as_ref() else {
            return 0..0;
        };
        let line_start = |line: usize| {
            if line >= b.rope.len_lines() {
                b.rope.len_chars()
            } else {
                b.rope.line_to_char(line)
            }
        };
        line_start(hunk.rows.start)..line_start(hunk.rows.end)
    }

    fn is_expanded(&self, hunk: &git::DiffHunk) -> bool {
        let range = self.hunk_char_range(hunk);
        self.expanded
            .iter()
            .any(|e| e.start <= range.end && range.start <= e.end)
    }

    /// For each expanded change that removed lines: the line it starts at and the committed lines it removed.
    fn expanded_deletions(&self) -> Vec<(usize, Range<usize>)> {
        if self.expanded.is_empty() || self.base.is_none() || self.split_active {
            return Vec::new();
        }
        self.git
            .hunks()
            .iter()
            .filter(|hunk| !hunk.base_rows.is_empty() && self.is_expanded(hunk))
            .map(|hunk| (hunk.rows.start, hunk.base_rows.clone()))
            .collect()
    }

    /// Whether some change covers `line` or removed lines right above it.
    fn hunk_at_line(&self, line: usize) -> bool {
        self.git.hunks().iter().any(|hunk| {
            hunk.rows.contains(&line) || (hunk.rows.is_empty() && hunk.rows.start == line)
        })
    }

    /// Keep the committed text (and its highlighting) current while any change is expanded.
    fn sync_base_text(&mut self) {
        if self.expanded.is_empty() && !self.split_active {
            return;
        }
        let Some(bases) = self.git.bases() else {
            return;
        };
        let stale = self
            .base
            .as_ref()
            .is_none_or(|base| !std::sync::Arc::ptr_eq(&base.bases, &bases));
        if stale {
            let buffer = EditorBuffer::from_text(bases.head.as_deref().unwrap_or(""));
            self.base = Some(BaseText {
                syntax: Syntax::new(self.lang),
                buffer,
                bases,
            });
            self.rows = None;
        }
        if let Some(base) = self.base.as_mut() {
            if let Some(syntax) = base.syntax.as_mut() {
                syntax.sync(&base.buffer);
            }
        }
    }

    /// Expand the changes under the selections, or collapse them if any already is.
    fn toggle_selected_hunks(&mut self) {
        let hunks = self.hunks_in_selections();
        if hunks.is_empty() {
            return;
        }
        if hunks.iter().any(|hunk| self.is_expanded(hunk)) {
            let ranges: Vec<Range<usize>> = hunks
                .iter()
                .map(|hunk| self.hunk_char_range(hunk))
                .collect();
            self.expanded
                .retain(|e| !ranges.iter().any(|r| e.start <= r.end && r.start <= e.end));
        } else {
            let ranges: Vec<Range<usize>> = hunks
                .iter()
                .map(|hunk| self.hunk_char_range(hunk))
                .collect();
            self.expanded.extend(ranges);
        }
        self.after_expansion_change();
    }

    fn expand_all_hunks(&mut self) {
        let ranges: Vec<Range<usize>> = self
            .git
            .hunks()
            .iter()
            .map(|hunk| self.hunk_char_range(hunk))
            .collect();
        self.expanded = ranges;
        self.after_expansion_change();
    }

    /// A click on the change strip at `line`.
    fn toggle_hunk_at_line(&mut self, line: usize) {
        let Some(hunk) = self
            .git
            .hunks()
            .iter()
            .find(|hunk| {
                hunk.rows.contains(&line) || (hunk.rows.is_empty() && hunk.rows.start == line)
            })
            .cloned()
        else {
            return;
        };
        let range = self.hunk_char_range(&hunk);
        if self.is_expanded(&hunk) {
            self.expanded
                .retain(|e| !(e.start <= range.end && range.start <= e.end));
        } else {
            self.expanded.push(range);
        }
        self.after_expansion_change();
    }

    fn after_expansion_change(&mut self) {
        self.sync_base_text();
        self.rows = None;
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    /// Expanded changes tint their lines: added ones green, removed ones (shown above) red, across the gutter.
    /// Staged ones get a lighter tint edged top and bottom.
    fn expanded_hunk_rects(&self, content: Rect, first: usize, last: usize) -> Vec<Rect> {
        if self.expanded.is_empty() {
            return Vec::new();
        }
        let colors = theme();
        let light = colors.appearance == ui::Appearance::Light;
        let (fill, hollow_fill, hollow_edge) = if light {
            (0.16, 0.08, 0.48)
        } else {
            (0.12, 0.06, 0.36)
        };
        let row_y = |row: usize| content.y + row as f32 * EDIT_LINE_H - self.scroll_y;
        let line_count = self.line_count();
        let display_row = |line: usize| {
            if line >= line_count {
                self.disp_count()
            } else {
                self.disp_of(line)
            }
        };
        let mut rects = Vec::new();
        let mut band = |top: usize, bottom: usize, color: Rgba, staged: bool| {
            let (top, bottom) = (top.max(first), bottom.min(last));
            if top >= bottom {
                return;
            }
            let (y, h) = (row_y(top), (bottom - top) as f32 * EDIT_LINE_H);
            let alpha = if staged { hollow_fill } else { fill };
            rects.push(Rect::new(content.x, y, content.w, h, color.alpha(alpha)));
            if staged {
                let edge = color.alpha(hollow_edge);
                rects.push(Rect::new(content.x, y, content.w, 1.0, edge));
                rects.push(Rect::new(content.x, y + h - 1.0, content.w, 1.0, edge));
            }
        };
        for hunk in self
            .git
            .hunks()
            .iter()
            .filter(|hunk| self.is_expanded(hunk))
        {
            let text_top = display_row(hunk.rows.start);
            let removed = if self.base.is_some() && !self.split_active {
                hunk.base_rows.len()
            } else {
                0
            };
            band(
                text_top.saturating_sub(removed),
                text_top,
                colors.version_control_deleted,
                hunk.staged,
            );
            // Split, the blank rows levelling a longer old side follow the change and stay untinted.
            let text_bottom = if self.split_active && !hunk.rows.is_empty() {
                self.last_row_of_line(hunk.rows.end - 1) + 1
            } else {
                display_row(hunk.rows.end)
            };
            band(
                text_top,
                text_bottom,
                colors.version_control_added,
                hunk.staged,
            );
        }
        rects
    }

    /// Uncommitted changes as strips at the gutter's left edge: added, modified, and a half-pill between the
    /// lines where some were deleted. Staged changes are drawn hollow.
    fn diff_hunk_rects(&self, content: Rect, first: usize, last: usize) -> Vec<Rect> {
        let strip_width = (0.275 * EDIT_LINE_H).floor();
        let deleted_width = (0.35 * EDIT_LINE_H).floor();
        let colors = theme();
        let row_y = |row: usize| content.y + row as f32 * EDIT_LINE_H - self.scroll_y;
        let line_count = self.line_count();
        let display_row = |line: usize| {
            if line >= line_count {
                self.disp_count()
            } else {
                self.disp_of(line)
            }
        };
        let mut rects = Vec::new();
        for hunk in self.git.hunks() {
            let color = match hunk.kind {
                git::HunkKind::Added => colors.version_control_added,
                git::HunkKind::Modified => colors.version_control_modified,
                git::HunkKind::Deleted => colors.version_control_deleted,
            };
            let removed = if self.is_expanded(hunk) && self.base.is_some() && !self.split_active {
                hunk.base_rows.len()
            } else {
                0
            };
            let bottom = display_row(hunk.rows.end);
            let top = display_row(hunk.rows.start) - removed.min(display_row(hunk.rows.start));
            let mut rect = if hunk.rows.is_empty() && removed == 0 {
                if top + 1 < first || top > last {
                    continue;
                }
                let mut pill = Rect::new(
                    content.x - deleted_width,
                    row_y(top) - EDIT_LINE_H / 2.0,
                    deleted_width * 2.0,
                    EDIT_LINE_H,
                    color,
                );
                pill.radius = EDIT_LINE_H;
                pill
            } else {
                if bottom <= first || top >= last {
                    continue;
                }
                Rect::new(
                    content.x,
                    row_y(top),
                    strip_width,
                    (bottom - top) as f32 * EDIT_LINE_H,
                    color,
                )
            };
            if hunk.staged {
                rect.color = blend(colors.editor_background, color.alpha(color.a * 0.3));
                rect.border = 1.0;
                rect.border_color = color;
            }
            rects.push(rect);
        }
        rects
    }

    /// The file's symbols in chars, labels colored like the code.
    fn outline_symbols(&self) -> Vec<outline_view::Symbol> {
        let (Some(syntax), Some(b)) = (self.syntax.as_ref(), self.buffer.as_ref()) else {
            return Vec::new();
        };
        let rope = &b.rope;
        let colors = syntax_theme();
        syntax
            .outline(rope)
            .into_iter()
            .map(|item| {
                let mut label_colors = Vec::new();
                for (span, source_start) in &item.source_spans {
                    let source = *source_start..*source_start + span.len();
                    for run in syntax.highlight(rope, source) {
                        let start = span.start + run.range.start - source_start;
                        let end = span.start + run.range.end - source_start;
                        label_colors
                            .push((start..end, color_of(&colors, run.capture.unwrap_or(""))));
                    }
                }
                outline_view::Symbol {
                    depth: item.depth,
                    range: rope.byte_to_char(item.range.start)..rope.byte_to_char(item.range.end),
                    name: rope.byte_to_char(item.selection_range.start)
                        ..rope.byte_to_char(item.selection_range.end),
                    text: item.text,
                    colors: label_colors,
                }
            })
            .collect()
    }

    /// The code around `symbol` for an outline preview `rows` lines tall and `columns` wide: its first line
    /// centered, scrolled sideways only as far as needed to show its name.
    fn preview_content(
        &self,
        symbol: &outline_view::Symbol,
        rows: usize,
        columns: usize,
    ) -> Option<outline_view::PreviewContent> {
        let b = self.buffer.as_ref()?;
        let rope = &b.rope;
        let line_of = |offset: usize| rope.char_to_line(offset.min(rope.len_chars()));
        let column_of =
            |offset: usize| offset.min(rope.len_chars()) - rope.line_to_char(line_of(offset));
        let (first_line, last_line) = (line_of(symbol.range.start), line_of(symbol.range.end));
        let name_line = line_of(symbol.name.start);
        let (name_start, name_end) = (column_of(symbol.name.start), column_of(symbol.name.end));
        let name_end = if line_of(symbol.name.end) == name_line {
            name_end
        } else {
            name_start + 1
        };
        let top = first_line.saturating_sub(rows.saturating_sub(1) / 2);
        let shift = name_start.min((name_end + 1).saturating_sub(columns));
        let colors = syntax_theme();
        let dims = GutterDimensions::for_lines(self.line_count());
        let rows = (top..(top + rows).min(self.line_count()))
            .map(|line| {
                let mut skip = shift;
                let mut room = columns;
                let mut segments = Vec::new();
                for (text, color) in self.line_segments(line, &colors) {
                    let chars: Vec<char> = text.chars().collect();
                    let dropped = skip.min(chars.len());
                    skip -= dropped;
                    let kept: String = chars[dropped..].iter().take(room).collect();
                    room -= kept.chars().count();
                    if !kept.is_empty() {
                        segments.push((kept, color));
                    }
                }
                outline_view::PreviewRow {
                    number: line + 1,
                    segments,
                    in_symbol: (first_line..=last_line).contains(&line),
                    name_columns: (line == name_line)
                        .then(|| name_start.saturating_sub(shift)..name_end.saturating_sub(shift)),
                }
            })
            .collect();
        Some(outline_view::PreviewContent {
            rows,
            gutter_width: dims.full_width() - dims.fold_area_width(),
        })
    }

    /// Highlight the lines of the char range `range` and center its first line.
    fn preview_range(&mut self, range: Option<Range<usize>>) {
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        self.highlighted_rows = range.map(|range| {
            (
                b.rope.char_to_line(range.start),
                b.rope.char_to_line(range.end),
            )
        });
        if let Some((line, _)) = self.highlighted_rows {
            self.scroll_line_to_center(line);
        }
    }

    fn go_to_offset(&mut self, offset: usize) {
        self.highlighted_rows = None;
        let Some(b) = self.buffer.as_mut() else {
            return;
        };
        b.place_cursor(offset);
        let line = b.rope.char_to_line(offset.min(b.rope.len_chars()));
        self.scroll_line_to_center(line);
    }

    fn manipulate_text(&mut self, transform: TextTransform) {
        if let Some(b) = self.buffer.as_mut() {
            b.manipulate_text(transform);
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn caret_line_column(&self) -> (usize, usize) {
        let Some(b) = self.buffer.as_ref() else {
            return (1, 1);
        };
        let end = b.selections().last().map_or(0, |s| s.end);
        let (line, column) = b.line_col_of(end);
        (line + 1, column + 1)
    }

    fn scroll_position(&self) -> (f32, f32) {
        (self.scroll_y, self.scroll_x)
    }

    fn set_scroll_position(&mut self, (y, x): (f32, f32)) {
        self.set_scroll_y(y);
        self.scroll_x = x.clamp(0.0, self.max_scroll_x());
    }

    fn scroll_line_to_center(&mut self, line: usize) {
        self.ensure_visible();
        let row = self.disp_of(line) as f32;
        let visible = self.body_h / EDIT_LINE_H;
        let margin = ((visible - 1.0) / 2.0).floor().max(0.0);
        self.set_scroll_y((row - margin).max(0.0) * EDIT_LINE_H);
    }

    fn preview_line(&mut self, target: Option<(usize, usize)>) {
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        let last = b.rope.len_lines().saturating_sub(1);
        self.highlighted_rows = target.map(|(line, _)| (line.min(last), line.min(last)));
        if let Some((line, _)) = self.highlighted_rows {
            self.scroll_line_to_center(line);
        }
    }

    fn go_to(&mut self, line: usize, column: usize) {
        self.highlighted_rows = None;
        if let Some(b) = self.buffer.as_mut() {
            let offset = b.offset_at(line, column);
            b.place_cursor(offset);
        }
        self.scroll_line_to_center(line);
    }
}

impl Searchable for FileItem {
    fn search_version(&self) -> u64 {
        self.buffer.as_ref().map_or(0, EditorBuffer::version)
    }

    fn find(&self, query: &SearchQuery) -> Vec<Range<usize>> {
        self.buffer
            .as_ref()
            .map_or_else(Vec::new, |b| b.search(query))
    }

    fn query_suggestion(&self) -> String {
        self.buffer
            .as_ref()
            .map_or_else(String::new, EditorBuffer::query_suggestion)
    }

    fn single_cursor(&self) -> Option<usize> {
        let b = self.buffer.as_ref()?;
        (b.selections().len() == 1).then(|| b.newest().head())
    }

    fn activate_match(&mut self, range: Range<usize>) {
        if !self.folds.take_overlapping(range.clone()).is_empty() {
            self.rows = None;
        }
        if let Some(b) = self.buffer.as_mut() {
            b.select_ranges(std::slice::from_ref(&range));
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn select_matches(&mut self, ranges: &[Range<usize>]) {
        let mut unfolded = false;
        for range in ranges {
            unfolded |= !self.folds.take_overlapping(range.clone()).is_empty();
        }
        if unfolded {
            self.rows = None;
        }
        if let Some(b) = self.buffer.as_mut() {
            b.select_ranges(ranges);
        }
        self.ensure_visible();
    }

    fn replace_match(&mut self, query: &SearchQuery, range: Range<usize>) {
        if let Some(b) = self.buffer.as_mut() {
            b.replace_match(query, range);
        }
        self.refresh();
        self.ensure_visible();
    }

    fn replace_all(&mut self, query: &SearchQuery, ranges: &[Range<usize>]) {
        if let Some(b) = self.buffer.as_mut() {
            b.replace_all(query, ranges);
        }
        self.refresh();
        self.ensure_visible();
    }

    fn set_search_highlights(&mut self, matches: Vec<Range<usize>>, active: Option<usize>) {
        self.search_highlights = matches;
        self.active_search_highlight = active;
    }
}

impl Item for FileItem {
    fn footer_height(&mut self, body_h: f32) -> f32 {
        self.footer
            .as_mut()
            .map_or(0.0, |footer| footer.view.height(body_h))
    }

    fn paint_footer(&mut self, area: Rect) -> Option<ui::Painted> {
        let footer = self.footer.as_mut()?;
        footer.area = Some(area);
        footer.view.paint(area)
    }

    fn pointer_down(
        &mut self,
        x: f32,
        y: f32,
        click_count: u32,
        _modifiers: terminal::Modifiers,
    ) -> bool {
        let Some(footer) = self.footer.as_mut() else {
            return false;
        };
        footer.focused = footer.contains(x, y);
        footer.pressed = footer.focused && footer.view.pointer_down(x, y, click_count);
        let pressed = footer.pressed;
        if let Some(all) = footer.view.take_run() {
            if let Some(request) = self.run_request(all) {
                if let Some(footer) = self.footer.as_mut() {
                    footer.view.run(request);
                }
            }
        }
        pressed
    }

    fn pointer_drag(&mut self, x: f32, y: f32, _modifiers: terminal::Modifiers) -> bool {
        self.footer
            .as_mut()
            .filter(|footer| footer.pressed)
            .is_some_and(|footer| footer.view.pointer_drag(x, y))
    }

    fn pointer_up(&mut self, _x: f32, _y: f32, _modifiers: terminal::Modifiers) {
        if let Some(footer) = self.footer.as_mut().filter(|footer| footer.pressed) {
            footer.pressed = false;
            footer.view.pointer_up();
        }
    }

    fn pointer_move(
        &mut self,
        x: f32,
        y: f32,
        _modifiers: terminal::Modifiers,
        _focused: bool,
    ) -> bool {
        self.footer
            .as_mut()
            .is_some_and(|footer| footer.view.pointer_move(x, y))
    }

    fn pointer_scroll(
        &mut self,
        x: f32,
        y: f32,
        delta_y: f32,
        modifiers: terminal::Modifiers,
    ) -> bool {
        self.footer
            .as_mut()
            .filter(|footer| footer.contains(x, y))
            .is_some_and(|footer| footer.view.scroll(0.0, delta_y, modifiers.shift))
    }

    fn pointer_scroll_x(&mut self, x: f32, y: f32, delta_x: f32) -> bool {
        self.footer
            .as_mut()
            .filter(|footer| footer.contains(x, y))
            .is_some_and(|footer| footer.view.scroll(delta_x, 0.0, false))
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> workspace::ItemTick {
        let version = self.buffer.as_ref().map(EditorBuffer::version);
        let text = self.buffer.as_ref().map(|buffer| buffer.text());
        let Some(footer) = self.footer.as_mut() else {
            return workspace::ItemTick::default();
        };
        if footer.version != version {
            footer.version = version;
            if let Some(text) = text {
                footer.view.text_changed(&text);
            }
        }
        workspace::ItemTick {
            changed: footer.view.tick(),
            ..workspace::ItemTick::default()
        }
    }

    fn companion_width(&mut self, body_w: f32) -> f32 {
        let wide = self.branch_diff
            && self.split
            && self.buffer.is_some()
            && body_w >= SPLIT_MIN_COLUMNS * char_advance();
        if wide != self.split_active {
            self.split_active = wide;
            self.rows = None;
            self.base_needed_for_split();
        }
        if wide {
            (body_w / 2.0).floor()
        } else {
            0.0
        }
    }

    fn paint_companion(&mut self, area: Rect) -> Option<ui::Painted> {
        self.split_active.then(|| self.paint_old_side(area))
    }

    fn abs_path(&self) -> Option<PathBuf> {
        if self.scratch.is_some() {
            return None;
        }
        Some(self.root.join(&self.path))
    }

    fn serialize(&self) -> Option<workspace::persistence::SerializedItem> {
        if let Some(footer) = self.footer.as_ref().filter(|_| self.scratch.is_some()) {
            return footer.view.serialize();
        }
        if self.branch_diff {
            return None;
        }
        self.saved_state()
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn searchable(&mut self) -> Option<&mut dyn Searchable> {
        Some(self)
    }

    fn nav_position(&self) -> Option<(usize, usize, (f32, f32))> {
        let b = self.buffer.as_ref()?;
        let head = b.newest().head();
        Some((head, b.rope.char_to_line(head), self.scroll_position()))
    }

    fn navigate_to(&mut self, cursor: usize, scroll: (f32, f32)) -> bool {
        let Some(b) = self.buffer.as_mut() else {
            return false;
        };
        let cursor = cursor.min(b.rope.len_chars());
        if b.newest().head() == cursor {
            return false;
        }
        b.place_cursor(cursor);
        self.set_scroll_position(scroll);
        true
    }

    fn cursor_status(&self) -> Option<String> {
        self.cursor_status_text()
    }

    fn id(&self) -> Option<String> {
        if let Some((id, _)) = &self.scratch {
            return Some(id.clone());
        }
        Some(if self.branch_diff {
            format!("{DIFF_ID_PREFIX}{}", self.path)
        } else {
            self.path.clone()
        })
    }

    fn title(&self) -> String {
        if let Some(title) = self.footer.as_ref().and_then(|footer| footer.view.title()) {
            return title;
        }
        if let Some((_, title)) = &self.scratch {
            return title.clone();
        }
        if self.branch_diff {
            format!("{} (diff)", self.name)
        } else {
            self.name.clone()
        }
    }

    fn icon(&self) -> Option<MaterialIcon> {
        Some(file_icon(&self.name))
    }

    fn clone_on_split(&self) -> Option<Box<dyn Item>> {
        if self.scratch.is_some() {
            return None;
        }
        let text = self.buffer.as_ref().map(|b| b.text_for_save());
        let mut item = FileItem::new(self.root.clone(), &self.path, text);
        item.saved_mtime = self.saved_mtime;
        Some(Box::new(item))
    }

    fn render(&mut self) -> Node {
        if self.buffer.is_none() {
            return div()
                .col()
                .flex(1.0)
                .px(MESSAGE_PAD)
                .py(MESSAGE_PAD)
                .child(
                    label("Cannot preview this file")
                        .size(13.0)
                        .color(theme().text_muted),
                )
                .into();
        }
        self.refresh();
        self.ensure_visible();
        let colors = syntax_theme();
        // Virtualize over display rows. The shell offsets the body up by the sub-row remainder
        // (`body_y_offset`) so scroll is pixel-smooth.
        let dcount = self.disp_count();
        let first = self.first_line().min(dcount);
        let visible = (self.body_h / EDIT_LINE_H).ceil() as usize + 2;
        let last = (first + visible).min(dcount);
        // Text rows only (no gutter): the shell shifts this node left by `scroll_x`. The gutter is a separate
        // fixed overlay (see `gutter`), so long lines scroll under a stationary line-number column.
        let blame_row = self.inline_blame_row();
        let inline_diagnostic = self.block_placement();
        let mut body = div().col().flex(1.0);
        for row_index in first..last {
            let Some(row) = self.row(row_index) else {
                break;
            };
            if row.spacer {
                body = body.child(div().h_px(EDIT_LINE_H));
                continue;
            }
            if let Some(index) = row.block {
                body = body.child(
                    self.block_row(index)
                        .unwrap_or_else(|| div().h_px(EDIT_LINE_H).into()),
                );
                continue;
            }
            let mut r = div().row().h_px(EDIT_LINE_H).items_center();
            if let Some(base_line) = row.deleted {
                let segments = self.base_line_segments(base_line, &colors);
                if segments.is_empty() {
                    r = r.child(label(" ").size(EDIT_FONT).mono());
                }
                for (text, color) in segments {
                    r = r.child(label(text).size(EDIT_FONT).mono().color(color));
                }
                body = body.child(r);
                continue;
            }
            if row.indent > 0 {
                r = r.child(label(" ".repeat(row.indent)).size(EDIT_FONT).mono());
            }
            let mut byte = 0usize;
            let mut empty = true;
            for (segment, color) in
                self.with_link_color(row.line, self.line_segments(row.line, &colors))
            {
                let segment_end = byte + segment.len();
                let from = row.start.clamp(byte, segment_end) - byte;
                let to = row.end.clamp(byte, segment_end) - byte;
                if from < to {
                    empty = false;
                    r = r.child(
                        label(segment[from..to].to_string())
                            .size(EDIT_FONT)
                            .mono()
                            .color(color),
                    );
                }
                byte = segment_end;
            }
            if empty && row.indent == 0 {
                r = r.child(label(" ").size(EDIT_FONT).mono());
            }
            let inline_block = matches!(
                &inline_diagnostic,
                Some(diagnostic_nav::BlockPlacement::Inline { row_line }) if *row_line == row.line
            ) && !self.soft_break_after(row_index);
            if inline_block {
                if let Some(block) = self.inline_block() {
                    r = r.child(block);
                }
            } else if Some(row_index) == blame_row {
                if let Some(text) = self.inline_blame_text() {
                    let hint = theme().hint;
                    r = r
                        .child(div().w_px(INLINE_BLAME_PADDING * char_advance()))
                        .child(icon(IconKind::FileGit).size(16.0).color(hint))
                        .child(div().w_px(8.0))
                        .child(label(text).size(EDIT_FONT).mono().color(hint));
                }
            }
            // The row resumes after the placeholder with the text following the fold's end, so a block reads `{...}`.
            if !self.soft_break_after(row_index) {
                if let Some(tail) = self.fold_tail(row.line, &colors) {
                    r = r.child(
                        div()
                            .px(FOLD_PILL_PAD)
                            .rounded(3.0)
                            .bg(theme().element_hover)
                            .child(
                                label("...".to_string())
                                    .size(EDIT_FONT)
                                    .mono()
                                    .color(theme().text_muted),
                            ),
                    );
                    for (text, color) in tail {
                        r = r.child(label(text).size(EDIT_FONT).mono().color(color));
                    }
                }
            }
            body = body.child(r);
        }
        body.into()
    }

    fn gutter(&mut self, fold_base: u64) -> Option<Node> {
        self.ensure_visible();
        if let Some(b) = self.buffer.as_ref() {
            let changed = self.git.poll(&b.rope, b.version());
            if changed && self.branch_diff {
                self.expand_all_hunks();
            } else if changed && !self.expanded.is_empty() {
                self.rows = None;
            }
        }
        self.sync_base_text();
        let buf = self.buffer.as_ref()?;
        let dims = GutterDimensions::for_lines(self.line_count());
        let active = self.active_rows();
        let cursor_rows: Vec<usize> = buf
            .selections()
            .iter()
            .map(|s| self.position(s.head()).0)
            .collect();
        let dcount = self.disp_count();
        let first = self.first_line().min(dcount);
        let visible = (self.body_h / EDIT_LINE_H).ceil() as usize + 2;
        let last = (first + visible).min(dcount);
        let mut col = div().col().flex(1.0);
        for row_index in first..last {
            let Some(row) = self.row(row_index) else {
                break;
            };
            let line = row.line;
            let hunk_click = self
                .hunk_at_line(line)
                .then_some(fold_base + pane_group_view::HUNK_FROM_FOLD + line as u64);
            if row.is_virtual() || row.start > 0 {
                let mut cell = div().w_px(dims.full_width()).h_px(EDIT_LINE_H);
                if let Some(id) = hunk_click {
                    cell = cell.on_click(id);
                }
                col = col.child(cell);
                continue;
            }
            let number_color = if active.iter().any(|(rows, _)| rows.contains(&row_index)) {
                theme().editor_active_line_number
            } else {
                theme().editor_line_number
            };
            // A toggle shows on folded rows always, and on foldable rows under a cursor or while the gutter is
            // hovered.
            let folded = self.is_folded(line);
            let show_toggle = folded
                || self.is_foldable(line)
                    && (self.gutter_hovered || cursor_rows.contains(&row_index));
            let mut fold_cell = div()
                .w_px(dims.fold_area_width())
                .h_px(EDIT_LINE_H)
                .items_center()
                .justify_center();
            if show_toggle {
                let (kind, color) = if folded {
                    (IconKind::ChevronRight, theme().text_accent)
                } else {
                    (IconKind::ChevronDown, theme().text_muted)
                };
                fold_cell = fold_cell
                    .on_click(fold_base + line as u64)
                    .child(icon(kind).size(FOLD_ICON).color(color));
            }
            col = col.child(
                div()
                    .row()
                    .w_px(dims.full_width())
                    .h_px(EDIT_LINE_H)
                    .items_center()
                    .child({
                        // The change strip sits in the left padding; clicking it expands the change.
                        let strip = div().w_px(dims.left_padding).h_px(EDIT_LINE_H);
                        match hunk_click {
                            Some(id) => strip.on_click(id),
                            None => strip,
                        }
                    })
                    .child(div().flex(1.0))
                    .child(
                        label((line + 1).to_string())
                            .size(EDIT_FONT)
                            .mono()
                            .color(number_color),
                    )
                    .child(fold_cell),
            );
        }
        Some(col.into())
    }

    fn set_gutter_hovered(&mut self, hovered: bool) -> bool {
        let changed = self.gutter_hovered != hovered;
        self.gutter_hovered = hovered;
        changed
    }

    fn gutter_w(&self) -> f32 {
        gutter_width(self.line_count())
    }

    fn toggle_fold(&mut self, line: usize) {
        self.do_toggle_fold(line);
    }

    fn toggle_diff_hunk(&mut self, line: usize) {
        self.toggle_hunk_at_line(line);
    }

    fn completion_popover(&self, content: Rect) -> Option<(Node, f32, f32)> {
        self.completion_menu_popover(content)
    }

    fn completion_aside(&self, content: Rect, viewport: (f32, f32)) -> Option<(Node, f32, f32)> {
        self.completion_aside_popover(content, viewport)
    }

    fn scroll_completion_aside(&mut self, dy: f32) -> bool {
        self.completions
            .as_mut()
            .is_some_and(|menu| menu.scroll_aside(dy))
    }

    fn click_completion(&mut self, row: usize) {
        self.confirm_completion(Some(row));
    }

    fn scroll_completion(&mut self, dy: f32) -> bool {
        self.completions
            .as_mut()
            .is_some_and(|menu| menu.scroll_by(dy))
    }

    fn pointer_moved(&mut self, local: Option<(f32, f32)>, window: (f32, f32)) -> bool {
        self.hover_pointer(local, window)
    }

    fn hover_popovers(&self, content: Rect) -> Vec<(Node, f32, f32)> {
        self.hover_popover_nodes(content)
    }

    fn scroll_hover(&mut self, index: usize, dy: f32) -> bool {
        self.scroll_hover_popover(index, dy)
    }

    fn diagnostic_message(&self) -> Option<String> {
        self.diagnostic_at_caret()
            .map(|entry| entry.message.clone())
    }

    fn hover_completion(&mut self, id: Option<u64>) {
        if let Some(menu) = self.completions.as_mut() {
            menu.hovered = id.filter(|id| (COMPLETION_BASE..HOVER_BASE).contains(id));
        }
    }

    fn popover_click(&mut self, id: u64) -> bool {
        if (COMPLETION_BASE..HOVER_BASE).contains(&id) {
            self.click_completion((id - COMPLETION_BASE) as usize);
            return true;
        }
        if (HOVER_BASE..MODAL_END).contains(&id) {
            self.hover_clicked();
            return true;
        }
        false
    }

    fn save(&mut self) -> Result<(), String> {
        if self.scratch.is_some() {
            return Ok(());
        }
        let Some(b) = self.buffer.as_mut() else {
            return Ok(());
        };
        let mtime = files::write(&self.root, &self.path, &b.text_for_save())
            .map_err(|e| format!("Could not save {}: {e}", self.path))?;
        b.mark_saved();
        self.saved_mtime = Some(mtime);
        self.conflict = false;
        self.saved_unannounced = true;
        self.git.reload_bases(self.root.join(&self.path));
        Ok(())
    }

    fn is_dirty(&self) -> bool {
        if self.scratch.is_some() {
            return false;
        }
        self.conflict || self.buffer.as_ref().is_some_and(|b| b.is_dirty())
    }

    fn has_conflict(&self) -> bool {
        self.conflict
    }

    fn refresh_disk_state(&mut self) {
        if self.scratch.is_some() {
            return;
        }
        let Ok(disk) = files::mtime(&self.root, &self.path) else {
            return;
        };
        if self.saved_mtime.is_some_and(|saved| disk <= saved) {
            return;
        }
        if self.buffer.as_ref().is_some_and(|b| b.is_dirty()) {
            self.conflict = true;
            return;
        }
        let Some(text) = files::read(&self.root, &self.path)
            .ok()
            .and_then(|c| c.text)
        else {
            return;
        };
        if let Some(b) = self.buffer.as_mut() {
            let cursor = b.cursor();
            let len = b.rope.len_chars();
            b.edit(vec![(0..len, text)]);
            b.mark_saved();
            b.place_cursor(cursor.min(b.rope.len_chars()));
        }
        self.git.reload_bases(self.root.join(&self.path));
        self.saved_mtime = Some(disk);
        self.refresh();
        self.ensure_visible();
    }

    fn is_busy(&self) -> bool {
        // Keep repainting while a parse runs or the scrollbars wait to hide.
        self.footer
            .as_ref()
            .is_some_and(|footer| footer.view.busy())
            || self.syntax.as_ref().is_some_and(|s| s.is_parsing())
            || self.scrollbars_revealed()
            || self.git.is_busy()
            || self.hover.is_busy()
            || self
                .completions
                .as_ref()
                .is_some_and(|menu| menu.scrollbar.is_animating())
            || self
                .base
                .as_ref()
                .and_then(|base| base.syntax.as_ref())
                .is_some_and(Syntax::is_parsing)
    }

    fn right_press(&mut self, local_x: f32, local_y: f32) {
        let Some(off) = self.offset_at_local(local_x, local_y) else {
            return;
        };
        // Keep the selection if the click is inside it; otherwise move the caret to the click.
        if let Some(b) = self.buffer.as_ref() {
            let inside = b
                .selections()
                .iter()
                .any(|s| s.range().is_some_and(|(a, e)| off >= a && off < e));
            if inside {
                return;
            }
        }
        if let Some(b) = self.buffer.as_mut() {
            b.place_cursor(off);
        }
    }

    fn buffer_line_at(&self, local_y: f32) -> Option<usize> {
        self.buffer.as_ref()?;
        let row = ((self.scroll_y + local_y) / EDIT_LINE_H).max(0.0) as usize;
        Some(self.buf_of(row.min(self.disp_count().saturating_sub(1))))
    }

    fn line_screen_y(&self, content: Rect, line: usize) -> Option<f32> {
        self.buffer.as_ref()?;
        let y = content.y + self.disp_of(line) as f32 * EDIT_LINE_H - self.scroll_y;
        (y + EDIT_LINE_H > content.y && y < content.y + self.body_h).then_some(y)
    }

    fn body_x_offset(&self) -> f32 {
        self.scroll_x
    }

    fn set_body_width(&mut self, w: f32) {
        self.body_w = w;
        self.ensure_visible();
        self.scroll_x = self.scroll_x.min(self.max_scroll_x());
    }

    fn scroll_by_x(&mut self, dx: f32) -> bool {
        let before = self.scroll_x;
        self.scroll_x = (self.scroll_x - dx).clamp(0.0, self.max_scroll_x());
        let moved = (self.scroll_x - before).abs() > 0.01;
        if moved {
            self.scrolled_at = Some(std::time::Instant::now());
        }
        moved
    }

    fn h_scrollbar(&self, content: Rect) -> Vec<Rect> {
        if self.buffer.is_none() || !self.scrollbars_revealed() {
            return Vec::new();
        }
        let gw = gutter_width(self.line_count());
        let track = Rect::new(
            content.x + gw,
            content.y + content.h - SCROLLBAR_WIDTH,
            (content.w - gw - SCROLLBAR_WIDTH).max(0.0),
            SCROLLBAR_WIDTH,
            Rgba::TRANSPARENT,
        );
        let em = char_advance();
        let page = self.text_viewport_w() / em;
        let total = self.content_w() / em;
        let Some((thumb_len, unit)) = thumb_metrics(track.w, page, total) else {
            return Vec::new();
        };
        vec![Rect::new(
            track.x + self.scroll_x / em * unit,
            track.y,
            thumb_len,
            track.h,
            theme().scrollbar_thumb_background,
        )]
    }

    fn cmd_click(&mut self, local_x: f32, local_y: f32) -> bool {
        if let Some(targets) = self.definition_click(local_x, local_y) {
            self.navigation = Some((targets, self.caret_top()));
        }
        true
    }

    fn drag_select(&mut self, x: f32, y: f32, body: Rect) {
        let text_bottom = body.y + body.h;
        let vertical_margin = EDIT_LINE_H.min(body.h / 3.0);
        let delta_rows = if y < body.y + vertical_margin {
            -drag_autoscroll_rows(body.y + vertical_margin - y)
        } else if y > text_bottom - vertical_margin {
            drag_autoscroll_rows(y - (text_bottom - vertical_margin))
        } else {
            0.0
        };
        let em = char_advance();
        let horizontal_space = HORIZONTAL_SCROLL_MARGIN * em;
        let text_left = body.x + self.gutter_w() + horizontal_space;
        let text_right = body.x + body.w - horizontal_space;
        let delta_columns = if x < text_left {
            -drag_autoscroll_columns(text_left - x)
        } else if x > text_right {
            drag_autoscroll_columns(x - text_right)
        } else {
            0.0
        };
        self.scroll_by(-delta_rows * EDIT_LINE_H);
        self.scroll_by_x(-delta_columns * em);
        self.place_cursor((x - body.x).max(0.0), y - body.y, true);
    }

    fn is_editable(&self) -> bool {
        self.buffer.is_some()
    }

    fn set_focused(&mut self, focused: bool) {
        // Regaining focus is a hard undo boundary, so edits across a focus change never merge.
        if focused && !self.focused {
            if let Some(b) = self.buffer.as_mut() {
                b.finalize_last_transaction();
            }
        }
        self.focused = focused;
    }

    fn set_body_height(&mut self, h: f32) {
        self.body_h = h;
        // Catch up with the buffer and build the display map before the &self readers (back_rects/carets) run.
        self.refresh();
        self.ensure_visible();
        self.set_scroll_y(self.scroll_y);
    }

    fn body_y_offset(&self) -> f32 {
        self.first_line() as f32 * EDIT_LINE_H - self.scroll_y
    }

    fn scroll_by(&mut self, dy: f32) -> bool {
        self.hide_hover();
        let before = self.scroll_y;
        self.set_scroll_y(self.scroll_y - dy);
        (self.scroll_y - before).abs() > 0.01
    }

    fn input_text(&mut self, text: &str) {
        self.hide_hover();
        self.refresh();
        let language = editor::language::config(self.lang);
        let rope = self.rope_snapshot();
        let scope_at = scope_lookup(&self.syntax, &rope);
        if let Some(b) = self.buffer.as_mut() {
            b.unmark_text();
            b.handle_input(text, &language, &scope_at);
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
        let mut chars = text.chars();
        self.update_completions(match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_alphanumeric() || c == '_' => CompletionTrigger::Typed,
            (Some(c), None) => CompletionTrigger::Character(c),
            _ => CompletionTrigger::Refilter,
        });
    }

    fn ime_preedit(&mut self, text: &str, selected: Option<Range<usize>>) {
        if let Some(b) = self.buffer.as_mut() {
            b.replace_and_mark_text(text, selected);
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn ime_commit(&mut self, text: &str) {
        self.refresh();
        let language = editor::language::config(self.lang);
        let rope = self.rope_snapshot();
        let scope_at = scope_lookup(&self.syntax, &rope);
        if let Some(b) = self.buffer.as_mut() {
            b.commit_text(text, &language, &scope_at);
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn copy(&self) -> Option<CopiedText> {
        if let Some(footer) = self.footer.as_ref().filter(|footer| footer.focused) {
            return footer.view.copy().map(|text| CopiedText {
                text,
                slices: Vec::new(),
            });
        }
        self.buffer.as_ref().map(|b| copied_text(b.copy()))
    }

    fn copy_trimmed(&self) -> Option<CopiedText> {
        self.buffer.as_ref().map(|b| copied_text(b.copy_trimmed()))
    }

    fn cut(&mut self) -> Option<CopiedText> {
        let copied = self.buffer.as_mut().map(|b| copied_text(b.cut()));
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
        copied
    }

    fn paste(&mut self, text: &str, slices: Option<&[ClipboardSlice]>) {
        let slices: Option<Vec<ClipboardSelection>> = slices.map(|slices| {
            slices
                .iter()
                .map(|s| ClipboardSelection {
                    len: s.len,
                    is_entire_line: s.is_entire_line,
                    first_line_indent: s.first_line_indent,
                })
                .collect()
        });
        if let Some(b) = self.buffer.as_mut() {
            b.paste(text, slices.as_deref());
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn input_key(&mut self, key: EditKey, shift: bool) {
        if self.footer_key(key, shift) {
            return;
        }
        if key == EditKey::Hover {
            if let Some(caret) = self.buffer.as_ref().map(|b| b.newest().head()) {
                self.show_hover(caret, true);
            }
            return;
        }
        let definition_kind = match key {
            EditKey::GoToDefinition => Some(lsp::DefinitionKind::Definition),
            EditKey::GoToDeclaration => Some(lsp::DefinitionKind::Declaration),
            EditKey::GoToTypeDefinition => Some(lsp::DefinitionKind::TypeDefinition),
            EditKey::GoToImplementation => Some(lsp::DefinitionKind::Implementation),
            _ => None,
        };
        if let Some(kind) = definition_kind {
            self.hide_hover();
            return self.go_to_definition(kind);
        }
        if matches!(
            key,
            EditKey::GoToDiagnostic | EditKey::GoToPreviousDiagnostic
        ) {
            self.hide_hover();
            return self.go_to_diagnostic(key == EditKey::GoToDiagnostic);
        }
        self.hide_hover();
        if let Some(menu) = self.completions.as_mut() {
            match key {
                EditKey::Up => return menu.select_previous(),
                EditKey::Down => return menu.select_next(),
                EditKey::Enter | EditKey::Tab => return self.confirm_completion(None),
                EditKey::Escape => {
                    self.close_completions();
                    if let Some(b) = self.buffer.as_mut() {
                        b.exit_snippet();
                    }
                    return;
                }
                EditKey::Backspace | EditKey::Delete => {
                    // Edit with the menu out of the way, then refilter it for the shorter word.
                    self.completions = None;
                    self.input_key(key, shift);
                    return self.update_completions(CompletionTrigger::Refilter);
                }
                _ => self.close_completions(),
            }
        }
        match key {
            EditKey::ShowCompletions => return self.update_completions(CompletionTrigger::Show),
            EditKey::ShowWordCompletions => {
                return self.update_completions(CompletionTrigger::ShowWords)
            }
            EditKey::Tab if self.move_to_snippet_stop(true) => return,
            EditKey::Backtab if self.move_to_snippet_stop(false) => return,
            EditKey::Escape if self.buffer.as_mut().is_some_and(|b| b.exit_snippet()) => return,
            EditKey::Escape if self.dismiss_diagnostic() => return,
            _ => {}
        }
        // One row of the previous page stays on screen.
        let page_rows = ((self.body_h / EDIT_LINE_H) as usize).saturating_sub(1);
        let motion = match key {
            EditKey::Left => Some(Motion::Left),
            EditKey::Right => Some(Motion::Right),
            EditKey::Up => Some(Motion::Up),
            EditKey::Down => Some(Motion::Down),
            EditKey::Home => Some(Motion::Home),
            EditKey::End => Some(Motion::End),
            EditKey::WordLeft => Some(Motion::WordLeft),
            EditKey::WordRight => Some(Motion::WordRight),
            EditKey::SubwordLeft => Some(Motion::SubwordLeft),
            EditKey::SubwordRight => Some(Motion::SubwordRight),
            EditKey::LineStart => Some(Motion::LineStart),
            EditKey::LineEnd => Some(Motion::LineEnd),
            EditKey::DocumentStart => Some(Motion::DocumentStart),
            EditKey::DocumentEnd => Some(Motion::DocumentEnd),
            EditKey::PageUp => Some(Motion::PageUp(page_rows)),
            EditKey::PageDown => Some(Motion::PageDown(page_rows)),
            _ => None,
        };
        if let Some(motion) = motion {
            self.refresh();
            self.ensure_visible();
            let next: Option<Vec<Selection>> = self
                .buffer
                .as_ref()
                .map(|b| b.moved_selections(motion, shift, &EditorRows::new(self)));
            if let (Some(b), Some(next)) = (self.buffer.as_mut(), next) {
                b.set_selections(next);
            }
            self.ensure_cursor_visible();
            return;
        }
        if key == EditKey::ToggleSoftWrap {
            self.toggle_soft_wrap();
            return;
        }
        if key == EditKey::Escape && !self.expanded.is_empty() {
            self.expanded.clear();
            return self.after_expansion_change();
        }
        match key {
            EditKey::ToggleSelectedDiffHunks => return self.toggle_selected_hunks(),
            EditKey::ExpandAllDiffHunks => return self.expand_all_hunks(),
            EditKey::ToggleSplitDiff => {
                if self.branch_diff {
                    self.split = !self.split;
                    self.split_active = self.split && self.split_active;
                    self.rows = None;
                    self.after_expansion_change();
                }
                return;
            }
            EditKey::GoToHunk | EditKey::GoToPreviousHunk => {
                return self.go_to_hunk(key == EditKey::GoToHunk)
            }
            EditKey::GitRestore => return self.restore_hunks(),
            EditKey::ToggleStaged => return self.stage_hunks(None),
            EditKey::StageAndNext | EditKey::UnstageAndNext => {
                let only_carets = self
                    .buffer
                    .as_ref()
                    .is_some_and(|b| b.selections().iter().all(|s| s.is_empty()));
                self.stage_hunks(Some(key == EditKey::StageAndNext));
                if only_carets {
                    self.go_to_hunk(true);
                }
                return;
            }
            _ => {}
        }
        if key == EditKey::MoveToEnclosingBracket {
            self.refresh();
            let enclosing = |range: Range<usize>| self.enclosing_brackets(range);
            let next = self
                .buffer
                .as_ref()
                .map(|b| b.enclosing_bracket_selections(&enclosing));
            if let (Some(b), Some(next)) = (self.buffer.as_mut(), next) {
                b.set_selections(next);
            }
            self.ensure_cursor_visible();
            return;
        }
        let deletion = match key {
            EditKey::Backspace => Some(Deletion::Backward),
            EditKey::Delete => Some(Deletion::Forward),
            EditKey::DeleteWordLeft => Some(Deletion::PreviousWordStart),
            EditKey::DeleteWordRight => Some(Deletion::NextWordEnd),
            EditKey::DeleteSubwordLeft => Some(Deletion::PreviousSubwordStart),
            EditKey::DeleteSubwordRight => Some(Deletion::NextSubwordEnd),
            EditKey::DeleteToLineStart => Some(Deletion::ToBeginningOfLine),
            EditKey::DeleteToLineEnd => Some(Deletion::ToEndOfLine),
            _ => None,
        };
        let line_command = match key {
            EditKey::DeleteLine => Some(LineCommand::Delete),
            EditKey::DuplicateLineUp => Some(LineCommand::Duplicate { up: true }),
            EditKey::DuplicateLineDown => Some(LineCommand::Duplicate { up: false }),
            EditKey::MoveLineUp => Some(LineCommand::Move { up: true }),
            EditKey::MoveLineDown => Some(LineCommand::Move { up: false }),
            _ => None,
        };
        if self.run_selection_command(key) {
            return;
        }
        if let Some(command) = line_command {
            self.run_line_command(command);
            return;
        }
        if matches!(key, EditKey::Outdent | EditKey::Backtab) {
            self.refresh();
            self.ensure_visible();
            let ranges: Option<Vec<Range<usize>>> = self
                .buffer
                .as_ref()
                .map(|b| b.outdent_ranges(&EditorRows::new(self)));
            if let (Some(b), Some(ranges)) = (self.buffer.as_mut(), ranges) {
                b.delete_ranges(ranges);
            }
            self.refresh();
            self.ensure_visible();
            self.ensure_cursor_visible();
            return;
        }
        if let Some(deletion) = deletion {
            self.refresh();
            self.ensure_visible();
            let grown: Option<Vec<Selection>> = self
                .buffer
                .as_ref()
                .map(|b| b.deletion_selections(deletion, &EditorRows::new(self)));
            if let (Some(b), Some(grown)) = (self.buffer.as_mut(), grown) {
                b.delete_selections(grown);
            }
            self.refresh();
            self.ensure_visible();
            self.ensure_cursor_visible();
            return;
        }
        let mut edited = false;
        if let Some(b) = self.buffer.as_mut() {
            match key {
                EditKey::Enter => {
                    let language = editor::language::config(self.lang);
                    let rope = b.rope.clone();
                    let scope_at = scope_lookup(&self.syntax, &rope);
                    b.newline(&language, &scope_at);
                    edited = true;
                }
                EditKey::Undo => {
                    b.undo();
                    edited = true;
                }
                EditKey::Redo => {
                    b.redo();
                    edited = true;
                }
                EditKey::Tab => {
                    b.tab();
                    edited = true;
                }
                EditKey::Indent => {
                    b.indent();
                    edited = true;
                }
                EditKey::ToggleComments => {
                    b.toggle_comments(&editor::language::config(self.lang));
                    edited = true;
                }
                EditKey::JoinLines => {
                    b.join_lines(&editor::language::config(self.lang));
                    edited = true;
                }
                EditKey::Transpose => {
                    b.transpose();
                    edited = true;
                }
                EditKey::SelectAll => b.select_all(),
                EditKey::Escape => b.collapse_cursors(),
                _ => {}
            }
        }
        if edited {
            self.refresh();
            self.ensure_visible();
        }
        self.ensure_cursor_visible();
    }

    fn place_cursor(&mut self, local_x: f32, local_y: f32, extend: bool) {
        self.hide_hover();
        self.close_completions();
        let Some(off) = self.offset_at_local(local_x, local_y) else {
            return;
        };
        if let Some(b) = self.buffer.as_mut() {
            if extend {
                b.extend_cursor(off);
            } else {
                b.place_cursor(off);
            }
        }
    }

    fn select_word_at(&mut self, local_x: f32, local_y: f32) {
        self.hide_hover();
        let Some(off) = self.offset_at_local(local_x, local_y) else {
            return;
        };
        if let Some(b) = self.buffer.as_mut() {
            b.select_word_at(off);
        }
    }

    fn selected_text(&self) -> Option<String> {
        self.buffer.as_ref().and_then(|b| b.selected_text())
    }

    fn carets(&self, content: Rect) -> Vec<Rect> {
        // Hidden when unfocused or during the caret's off blink phase.
        if !self.focused || !ui::caret_phase() {
            return Vec::new();
        }
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        let gw = gutter_width(self.line_count());
        b.selections()
            .iter()
            .filter_map(|s| {
                let (row, x) = self.position(s.head());
                let y = content.y + row as f32 * EDIT_LINE_H - self.scroll_y;
                if y + EDIT_LINE_H <= content.y || y >= content.y + self.body_h {
                    return None;
                }
                Some(Rect::new(
                    content.x + gw + x - self.scroll_x,
                    y,
                    CARET_W,
                    EDIT_LINE_H,
                    theme().player_cursor,
                ))
            })
            .collect()
    }

    fn back_rects(&self, content: Rect) -> Vec<Rect> {
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        let cw = char_advance();
        let gw = gutter_width(self.line_count());
        let row_y = |row: usize| content.y + row as f32 * EDIT_LINE_H - self.scroll_y;
        let first = self.first_line();
        let last = (first + (self.body_h / EDIT_LINE_H).ceil() as usize + 2).min(self.disp_count());
        let mut rects = self.expanded_hunk_rects(content, first, last);

        // Every caret's display line is highlighted across the gutter and text, except rows that also hold a
        // non-empty selection.
        let active = self.active_rows();
        let selected_rows: Vec<&std::ops::RangeInclusive<usize>> = active
            .iter()
            .filter(|(_, selected)| *selected)
            .map(|(rows, _)| rows)
            .collect();
        for row in first..last {
            let is_active = active.iter().any(|(rows, _)| rows.contains(&row));
            if is_active && !selected_rows.iter().any(|rows| rows.contains(&row)) {
                rects.push(Rect::new(
                    content.x,
                    row_y(row),
                    content.w,
                    EDIT_LINE_H,
                    theme().editor_active_line,
                ));
            }
        }

        if let Some((first_line, last_line)) = self.highlighted_rows {
            let (top, bottom) = (self.disp_of(first_line), self.last_row_of_line(last_line));
            if top < last && bottom >= first {
                rects.push(Rect::new(
                    content.x,
                    row_y(top),
                    content.w,
                    (bottom + 1 - top) as f32 * EDIT_LINE_H,
                    theme().editor_highlighted_line,
                ));
            }
        }

        for range in self.hover_highlight_ranges() {
            let (start_row, start_x) = self.position(range.start);
            let (end_row, end_x) = self.position(range.end);
            for row in start_row.max(first)..=end_row.min(last.saturating_sub(1)) {
                let left = if row == start_row { start_x } else { 0.0 };
                let right = if row == end_row {
                    end_x
                } else {
                    self.row_width(row)
                };
                rects.push(Rect::new(
                    content.x + gw + left - self.scroll_x,
                    row_y(row),
                    (right - left).max(1.0),
                    EDIT_LINE_H,
                    theme().element_hover,
                ));
            }
        }

        for (index, range) in self.search_highlights.iter().enumerate() {
            let color = if Some(index) == self.active_search_highlight {
                theme().search_active_match_background
            } else {
                theme().search_match_background
            };
            let (start_row, start_x) = self.position(range.start);
            let (end_row, end_x) = self.position(range.end);
            if end_row < first || start_row >= last {
                continue;
            }
            for row in start_row.max(first)..=end_row.min(last.saturating_sub(1)) {
                let left = if row == start_row { start_x } else { 0.0 };
                let right = if row == end_row {
                    end_x
                } else {
                    self.row_width(row)
                };
                rects.push(Rect::new(
                    content.x + gw + left - self.scroll_x,
                    row_y(row),
                    (right - left).max(1.0),
                    EDIT_LINE_H,
                    color,
                ));
            }
        }

        for range in self.matching_brackets() {
            let (row, start_x) = self.position(range.start);
            let (_, end_x) = self.position(range.end);
            if row >= first && row < last {
                rects.push(Rect::new(
                    content.x + gw + start_x - self.scroll_x,
                    row_y(row),
                    (end_x - start_x).max(1.0),
                    EDIT_LINE_H,
                    theme().editor_document_highlight_bracket_background,
                ));
            }
        }

        rects.extend(self.diagnostic_rects(content, first, last));
        rects.extend(self.link_underline_rects(content, first, last));

        // Text an input method is still composing is underlined.
        for marked in b.marked_ranges() {
            let (start_row, start_x) = self.position(marked.start);
            let (end_row, end_x) = self.position(marked.end);
            for row in start_row.max(first)..=end_row.min(last.saturating_sub(1)) {
                let left = if row == start_row { start_x } else { 0.0 };
                let right = if row == end_row {
                    end_x
                } else {
                    self.row_width(row)
                };
                rects.push(Rect::new(
                    content.x + gw + left - self.scroll_x,
                    row_y(row) + EDIT_LINE_H - 4.0,
                    (right - left).max(1.0),
                    1.0,
                    theme().text,
                ));
            }
        }

        // Indent guides: one continuous vertical line per (block, level) over buffer lines -- blank lines inside
        // a block inherit its depth so the guide doesn't break -- then mapped to display rows.
        let buf_first = self.buf_of(first);
        let buf_last = self.buf_of(last.saturating_sub(1)) + 1;
        let cursor_line = b.line_col_of(b.newest().head()).0;
        let active_block = Self::enclosing_indent(b, cursor_line);
        for (s, e, depth) in self.indent_guides(buf_first, buf_last) {
            let active = active_block.is_some_and(|(start, end, column)| {
                depth * TAB_COLS == column && start <= e && s <= end
            });
            let guide = if active {
                theme().editor_indent_guide_active
            } else {
                theme().editor_indent_guide
            };
            let top = self.disp_of(s);
            let bottom = self.last_row_of_line(e);
            // A guide starting inside a collapsed fold would draw a stub through the header's text.
            if bottom < top || self.buf_of(top) != s {
                continue;
            }
            rects.push(Rect::new(
                content.x + gw + (depth * TAB_COLS) as f32 * cw - self.scroll_x,
                row_y(top),
                1.0,
                (bottom + 1 - top) as f32 * EDIT_LINE_H,
                guide,
            ));
        }

        rects.extend(self.diff_hunk_rects(content, first, last));
        rects
    }

    fn selection_tris(&self, content: Rect) -> Vec<ui::Tri> {
        let mut tris = self.selection_band_tris(content);
        tris.extend(self.invisible_tris(content));
        tris
    }

    fn scrollbar(&self, content: Rect) -> Vec<Rect> {
        if self.buffer.is_none() || !self.scrollbars_revealed() {
            return Vec::new();
        }
        let track = Rect::new(
            content.x + content.w - SCROLLBAR_WIDTH,
            content.y,
            SCROLLBAR_WIDTH,
            content.h,
            Rgba::TRANSPARENT,
        );
        // Rows are the scroll unit; the range runs one page past the last row.
        let page = self.body_h / EDIT_LINE_H;
        let total = (self.content_h() + self.body_h) / EDIT_LINE_H;
        let Some((thumb_len, unit)) = thumb_metrics(track.h, page, total) else {
            return Vec::new();
        };
        let mut rects = vec![Rect::new(
            track.x,
            track.y,
            SCROLLBAR_BORDER,
            track.h,
            theme().scrollbar_track_border,
        )];
        let Some(b) = self.buffer.as_ref() else {
            return rects;
        };
        // One 2px marker per caret row, merged where they touch.
        let mut cursor_rows: Vec<usize> = b
            .selections()
            .iter()
            .map(|s| self.position(s.head()).0)
            .collect();
        cursor_rows.sort_unstable();
        cursor_rows.dedup();
        if cursor_rows.len() <= SCROLLBAR_CURSOR_MARKER_LIMIT {
            let mut markers: Vec<(f32, f32)> = Vec::new();
            for row in cursor_rows {
                let start = row as f32 * unit;
                let end = start + SCROLLBAR_LINE_MARKER;
                match markers.last_mut() {
                    Some(last) if last.1 >= start - 1.0 => last.1 = last.1.max(end),
                    _ => markers.push((start, end)),
                }
            }
            rects.extend(markers.into_iter().map(|(start, end)| {
                Rect::new(
                    track.x + SCROLLBAR_BORDER,
                    track.y + start,
                    track.w - SCROLLBAR_BORDER,
                    end - start,
                    theme().player_cursor,
                )
            }));
        }
        let thumb_y = track.y + self.scroll_y / EDIT_LINE_H * unit;
        rects.push(Rect::new(
            track.x,
            thumb_y,
            track.w,
            thumb_len,
            theme().scrollbar_thumb_background,
        ));
        rects.push(Rect::new(
            track.x,
            thumb_y,
            SCROLLBAR_BORDER,
            thumb_len,
            theme().scrollbar_thumb_border,
        ));
        rects
    }
}

/// Thumb length and px per scroll unit on a track `track_len` long, for `page` units visible out of `total`;
/// `None` when everything fits.
fn thumb_metrics(track_len: f32, page: f32, total: f32) -> Option<(f32, f32)> {
    let scrollable = total - page;
    if scrollable <= 0.0 || total <= 0.0 {
        return None;
    }
    let thumb = (track_len * page / total)
        .max(SCROLLBAR_MIN_THUMB)
        .min(track_len);
    Some((thumb, (track_len - thumb) / scrollable))
}

/// Syntax scope at a byte offset, for bracket rules that differ inside strings and comments.
fn scope_lookup<'a>(
    syntax: &'a Option<Syntax>,
    rope: &'a ropey::Rope,
) -> impl Fn(usize) -> editor::language::Scope + Copy + 'a {
    move |byte| {
        syntax
            .as_ref()
            .map(|s| s.scope_at(rope, byte))
            .unwrap_or_default()
    }
}

fn copied_text((text, selections): (String, Vec<ClipboardSelection>)) -> CopiedText {
    CopiedText {
        text,
        slices: selections
            .into_iter()
            .map(|s| ClipboardSlice {
                len: s.len,
                is_entire_line: s.is_entire_line,
                first_line_indent: s.first_line_indent,
            })
            .collect(),
    }
}

/// A filled circle as a fan of triangles.
fn dot_tris(cx: f32, cy: f32, radius: f32, color: Rgba) -> Vec<ui::Tri> {
    const SEGMENTS: usize = 12;
    (0..SEGMENTS)
        .map(|i| {
            let angle = |k: usize| k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let (a, b) = (angle(i), angle(i + 1));
            ui::Tri {
                p: [
                    [cx, cy],
                    [cx + radius * a.cos(), cy + radius * a.sin()],
                    [cx + radius * b.cos(), cy + radius * b.sin()],
                ],
                color,
            }
        })
        .collect()
}

/// A right-pointing arrow `width` wide centered on (cx, cy): a 1px shaft and a triangular head.
fn arrow_tris(cx: f32, cy: f32, width: f32, color: Rgba) -> Vec<ui::Tri> {
    let (left, right) = (cx - width / 2.0, cx + width / 2.0);
    let head = width * 0.4;
    let half = 0.5;
    vec![
        ui::Tri {
            p: [
                [left, cy - half],
                [right - head, cy - half],
                [right - head, cy + half],
            ],
            color,
        },
        ui::Tri {
            p: [
                [left, cy - half],
                [right - head, cy + half],
                [left, cy + half],
            ],
            color,
        },
        ui::Tri {
            p: [
                [right - head, cy - head * 0.7],
                [right, cy],
                [right - head, cy + head * 0.7],
            ],
            color,
        },
    ]
}

/// Soft-wrap breaks for `line`, skipping the measure when an all-ASCII line clearly fits.
fn wrap_boundaries(
    buffer: &EditorBuffer,
    line: usize,
    columns: usize,
    wrap_width: f32,
    em: f32,
    char_widths: &RefCell<HashMap<char, f32>>,
) -> Rc<[Boundary]> {
    let slice = buffer.rope.line(line);
    if slice.len_bytes() == slice.len_chars() && columns as f32 * em <= wrap_width {
        return Rc::from([]);
    }
    let mut text = String::new();
    let (mut column, mut byte) = (0usize, 0usize);
    for chunk in slice.chunks() {
        let chunk = chunk.trim_end_matches(['\n', '\r']);
        editor::display::push_expanded(&mut text, chunk, TAB_COLS, &mut column, &mut byte);
    }
    let mut widths = char_widths.borrow_mut();
    let width_of = |c: char| {
        if c.is_ascii() {
            return em;
        }
        *widths
            .entry(c)
            .or_insert_with(|| ui::measure_text_width(&c.to_string(), EDIT_FONT, true, 400))
    };
    editor::wrap::wrap_line(&text, wrap_width, width_of).into()
}

/// A file item's screen rows, for motions that move by what's displayed.
struct EditorRows<'a> {
    item: &'a FileItem,
    folds: Vec<Range<usize>>,
    /// With removed lines shown, the display rows that hold text: motions count only these, so the caret
    /// steps over the removed lines.
    text_rows: Option<Vec<usize>>,
}

impl<'a> EditorRows<'a> {
    fn new(item: &'a FileItem) -> Self {
        let text_rows = item.has_virtual_rows.then(|| {
            (0..item.disp_count())
                .filter(|row| item.row(*row).is_some_and(|r| !r.is_virtual()))
                .collect()
        });
        Self {
            item,
            folds: item.folds.merged(),
            text_rows,
        }
    }

    fn display_row_of(&self, row: usize) -> usize {
        match &self.text_rows {
            Some(rows) => rows.get(row).copied().unwrap_or(self.item.disp_count()),
            None => row,
        }
    }

    fn text_row_of(&self, row: usize) -> usize {
        match &self.text_rows {
            Some(rows) => rows.partition_point(|r| *r < row),
            None => row,
        }
    }
}

impl DisplayRows for EditorRows<'_> {
    fn clip(&self, offset: usize, bias: Bias) -> usize {
        match self
            .folds
            .iter()
            .find(|f| f.start < offset && offset < f.end)
        {
            Some(fold) => match bias {
                Bias::Left => fold.start,
                Bias::Right => fold.end,
            },
            None => offset,
        }
    }

    fn row_of(&self, offset: usize) -> usize {
        self.text_row_of(self.item.position(offset).0)
    }

    fn max_row(&self) -> usize {
        match &self.text_rows {
            Some(rows) => rows.len().saturating_sub(1),
            None => self.item.disp_count().saturating_sub(1),
        }
    }

    fn row_start(&self, row: usize) -> usize {
        self.item.row_start_offset(self.display_row_of(row))
    }

    fn row_end(&self, row: usize) -> usize {
        self.item.row_end_offset(self.display_row_of(row))
    }

    fn line_start(&self, offset: usize) -> usize {
        let line = self.item.buf_of(self.item.position(offset).0);
        self.item.display_offset(line, 0)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.item
            .display_line_end(self.item.buf_of(self.item.position(offset).0))
    }

    fn x_of(&self, offset: usize) -> f32 {
        self.item.position(offset).1
    }

    fn offset_for_x(&self, row: usize, x: f32) -> usize {
        self.clip(
            self.item
                .offset_for_row_x(self.display_row_of(row), x, false),
            Bias::Left,
        )
    }
}

/// Rows to scroll per drag event for a pointer `overshoot` px past the edge band, accelerating with distance.
fn drag_autoscroll_rows(overshoot: f32) -> f32 {
    (overshoot.powf(1.2) / 100.0).min(3.0)
}

fn drag_autoscroll_columns(overshoot: f32) -> f32 {
    overshoot.powf(1.2) / 300.0
}

/// Whether a file extension is a viewable raster image.
fn is_image_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    )
}

/// A stable id for an image path, keying its registered GPU texture (`ui::set_image`).
fn image_id(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "image".hash(&mut h);
    path.hash(&mut h);
    h.finish()
}

/// An image file shown as a center tab: decoded once to RGBA and registered with the renderer, then drawn
/// aspect-fit. Not editable.
struct ImageItem {
    root: PathBuf,
    path: String,
    name: String,
    id: u64,
    ok: bool,
}

impl ImageItem {
    fn new(root: &std::path::Path, path: &str) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let id = image_id(path);
        let ok = ui::has_image(id)
            || match std::fs::read(root.join(path))
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
            {
                Some(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    ui::set_image(id, w, h, rgba.into_raw());
                    true
                }
                None => false,
            };
        Self {
            root: root.to_path_buf(),
            path: path.to_string(),
            name,
            id,
            ok,
        }
    }
}

impl Item for ImageItem {
    fn abs_path(&self) -> Option<PathBuf> {
        Some(self.root.join(&self.path))
    }

    fn serialize(&self) -> Option<workspace::persistence::SerializedItem> {
        self.saved_state()
    }

    fn id(&self) -> Option<String> {
        Some(self.path.clone())
    }
    fn title(&self) -> String {
        self.name.clone()
    }
    fn icon(&self) -> Option<MaterialIcon> {
        Some(MaterialIcon::Image)
    }
    fn clone_on_split(&self) -> Option<Box<dyn Item>> {
        Some(Box::new(ImageItem {
            root: self.root.clone(),
            path: self.path.clone(),
            name: self.name.clone(),
            id: self.id,
            ok: self.ok,
        }))
    }
    fn render(&mut self) -> Node {
        if !self.ok {
            return div()
                .col()
                .flex(1.0)
                .px(MESSAGE_PAD)
                .py(MESSAGE_PAD)
                .child(
                    label("Cannot decode this image")
                        .size(13.0)
                        .color(theme().text_muted),
                )
                .into();
        }
        // Fill the body; the renderer aspect-fits the picture centered within it.
        div()
            .col()
            .flex(1.0)
            .px(12.0)
            .py(12.0)
            .image(self.id)
            .into()
    }
}

/// Opening files is the files view's business, so it extends the shared pane rather than living in it.
trait OpenFile {
    fn open_file(&mut self, root: &std::path::Path, path: &str);
}

impl OpenFile for Pane {
    fn open_file(&mut self, root: &std::path::Path, path: &str) {
        if let Some(i) = self
            .open
            .iter()
            .position(|o| o.id().as_deref() == Some(path))
        {
            self.activate_user(i);
            return;
        }
        let ext = path.rsplit('.').next().unwrap_or("");
        if is_image_ext(ext) {
            self.open.push(Box::new(ImageItem::new(root, path)));
            self.activate_user(self.open.len() - 1);
            return;
        }
        let text = files::read(root, path).ok().and_then(|c| c.text);
        self.open
            .push(Box::new(FileItem::new(root.to_path_buf(), path, text)));
        self.activate_user(self.open.len() - 1);
    }
}

/// One flattened, currently-visible tree row (a directory or a file).
struct Row {
    name: String,
    path: String,
    is_dir: bool,
    depth: usize,
    open: bool,
    edit: bool,
}

pub struct FilesView {
    root: PathBuf,
    tree: Vec<FileNode>,
    /// The tree walk still running off the UI thread; the tree fills in as it reports.
    scan: Option<files::BackgroundScan>,
    expanded: HashSet<String>,
    /// The editor area's split panes and their tabs.
    panes: PaneGroupView,
    /// Tree moves (from, to) still to apply to the other pane group's tabs, and that group's dirty file ids.
    pending_moves: Vec<(String, String)>,
    other_dirty: Vec<String>,
    /// The hit id under the pointer, for modal rows that light up on hover.
    hover: Option<u64>,
    /// Rebuilt each render: click id `FUNC_VIEW_BASE + i` maps to `(path, is_dir)`.
    click_targets: Vec<(String, bool)>,
    /// Rebuilt each render: the folder path for each pinned sticky-breadcrumb row (click to collapse it).
    sticky_paths: Vec<String>,
    /// Cached flattened visible rows, rebuilt only when the tree structure changes (expand/collapse), not on
    /// scroll — so scrolling a huge tree doesn't re-flatten every frame.
    flat_cache: Vec<Row>,
    flat_dirty: bool,
    /// Tree scroll (px) on both axes, the content height/width (set each render) for clamping, and the visible
    /// height (set before render) so the tree can virtualize.
    scroll: f32,
    scroll_x: f32,
    content_h: f32,
    content_w: f32,
    viewport_h: f32,
    /// The tree column's visible width, refreshed each frame from the layout (the dock owns the width now), so
    /// horizontal scroll can clamp against it.
    tree_vw: f32,
    go_to_line: Option<(Vec<usize>, go_to_line::GoToLine)>,
    palette: Option<(Vec<usize>, command_palette::CommandPalette)>,
    palette_memory: command_palette::PaletteMemory,
    /// The open symbol outline and the pane it navigates.
    outline: Option<(Vec<usize>, outline_view::OutlineView)>,
    /// Where the outline's preview last sat, kept for the next time it opens.
    outline_preview: outline_view::PreviewLayout,
    /// The window size last seen by `modal`, for sizing a modal as it opens.
    window_size: (f32, f32),
    /// The project's language servers; none in tests, which must not start real servers.
    lsp: Option<lsp::LspStore>,
    tree_ops: tree_actions::TreeOps,
    request: Option<workspace::ViewRequest>,
}

/// The syntax palette matching the active UI theme's light/dark appearance, so highlighting stays in sync with
/// the app theme (the reference drives both UI and syntax from one theme).
pub(crate) use workspace::syntax_theme;

impl FilesView {
    /// Opens at once with an empty tree; the tree fills in from a background walk so a large workspace
    /// never stalls the window.
    pub fn new(root: PathBuf) -> Self {
        let scan = files::BackgroundScan::start(root.clone(), std::sync::Arc::new(ui::wake));
        let mut view = Self::with_tree(root, Vec::new());
        view.scan = Some(scan);
        view
    }

    #[cfg(test)]
    pub(crate) fn scanned(root: PathBuf) -> Self {
        let tree = files::build_tree(&files::list(&root));
        Self::with_tree(root, tree)
    }

    /// Take the newest scan progress into the tree; returns whether the tree changed.
    fn apply_scan(&mut self) -> bool {
        let Some(update) = self.scan.as_ref().and_then(files::BackgroundScan::latest) else {
            return false;
        };
        if !update.scanning {
            self.scan = None;
        }
        self.tree = files::build_tree(&update.entries);
        self.flat_dirty = true;
        true
    }

    fn with_tree(root: PathBuf, tree: Vec<FileNode>) -> Self {
        let lsp =
            (!cfg!(test)).then(|| lsp::LspStore::new(root.clone(), std::sync::Arc::new(ui::wake)));
        Self {
            root,
            tree,
            scan: None,
            expanded: HashSet::new(),
            panes: PaneGroupView::new(PaneGroupConfig {
                id_base: FUNC_VIEW_BASE,
                show_nav: true,
                buttons: vec![
                    PaneButton {
                        icon: IconKind::PanelRight,
                        action: PaneButtonAction::Split(SplitDirection::Right),
                    },
                    PaneButton {
                        icon: IconKind::PanelBottom,
                        action: PaneButtonAction::Split(SplitDirection::Down),
                    },
                    PaneButton {
                        icon: IconKind::Maximize,
                        action: PaneButtonAction::ToggleZoom,
                    },
                ],
                max_panes: MAX_PANES,
                split_filter: None,
                zoom_whole_group: false,
            }),
            hover: None,
            pending_moves: Vec::new(),
            other_dirty: Vec::new(),
            go_to_line: None,
            palette: None,
            palette_memory: command_palette::PaletteMemory::default(),
            outline: None,
            outline_preview: outline_view::PreviewLayout::Hidden,
            window_size: (1200.0, 800.0),
            lsp,
            tree_ops: tree_actions::TreeOps::default(),
            request: None,
            click_targets: Vec::new(),
            sticky_paths: Vec::new(),
            flat_cache: Vec::new(),
            flat_dirty: true,
            scroll: 0.0,
            scroll_x: 0.0,
            content_h: 0.0,
            content_w: 0.0,
            viewport_h: 600.0,
            tree_vw: 260.0,
        }
    }

    /// The focused pane's active item id (a file path), for highlighting its row in the tree.
    fn active_path(&self) -> Option<String> {
        // A shared read-only walk (no `&mut`), so it can't self-heal a stale path -- the render always calls a
        // `&mut` method first, so by the time this runs the active path is valid.
        self.panes.active_item()?.id()
    }

    /// Open a file in the focused pane.
    fn visible_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        flatten(&self.tree, 0, &self.expanded, &mut rows);
        if let Some(edit) = self.tree_ops.edit.as_ref() {
            tree_actions::place_edit_row(&mut rows, &edit.target);
        }
        rows
    }

    fn open_file(&mut self, path: &str) {
        let root = self.root.clone();
        if let Some(pane) = self.panes.active_pane_mut() {
            pane.open_file(&root, path);
        }
    }

    /// Bring the language servers up to date with the files open in the editor area and in `other` (another
    /// pane group, such as the terminal panel), give each file its diagnostics, and open definitions that
    /// resolved. A file open in several panes is mirrored from its first pane's buffer.
    fn sync_language_servers(&mut self, mut other: Option<&mut PaneGroupView>) {
        let now = std::time::Instant::now();
        let Some(lsp) = self.lsp.as_mut() else {
            let offline = |item: &mut dyn Item, navigations: &mut Vec<Navigation>| {
                if let Some(file) = item.as_any_mut().and_then(|a| a.downcast_mut::<FileItem>()) {
                    navigations.extend(file.navigation.take());
                    if file.hover_request_due(now).is_some() {
                        file.hover_requested(None);
                    }
                    while file.definition_request_due().is_some() {
                        file.definition_requested(None);
                    }
                    file.tick_hover(now);
                }
            };
            let mut navigations = Vec::new();
            self.panes
                .for_each_item_mut(&mut |item| offline(item, &mut navigations));
            for (targets, caret_top) in navigations {
                open_definition(&self.root, &mut self.panes, &targets, caret_top);
            }
            if let Some(group) = other {
                let mut navigations = Vec::new();
                group.for_each_item_mut(&mut |item| offline(item, &mut navigations));
                for (targets, caret_top) in navigations {
                    open_definition(&self.root, group, &targets, caret_top);
                }
            }
            return;
        };
        let events = lsp.poll();
        let mut updates = Vec::new();
        let mut hovers = Vec::new();
        let mut completion_answers = Vec::new();
        let mut resolutions = Vec::new();
        let mut definition_answers = Vec::new();
        for event in events {
            match event {
                lsp::StoreEvent::Diagnostics(update) => updates.push(update),
                lsp::StoreEvent::Hover(response) => hovers.push(response),
                lsp::StoreEvent::Completions(response) => completion_answers.push(response),
                lsp::StoreEvent::CompletionResolved(resolved) => resolutions.push(resolved),
                lsp::StoreEvent::Definitions(response) => definition_answers.push(response),
            }
        }
        let mut synced: HashSet<PathBuf> = HashSet::new();
        let mut serve = |item: &mut dyn Item, navigations: &mut Vec<Navigation>| {
            let Some(file) = item
                .as_any_mut()
                .and_then(|any| any.downcast_mut::<FileItem>())
            else {
                return;
            };
            navigations.extend(file.navigation.take());
            for response in &hovers {
                file.hover_answered(response);
            }
            for response in &completion_answers {
                file.completions_answered(response);
            }
            for resolved in &resolutions {
                file.resolve_answered(resolved);
            }
            for response in &definition_answers {
                if let Some(targets) = file.definitions_answered(response) {
                    navigations.push((targets, file.caret_top()));
                }
            }
            let path = file.root.join(&file.path);
            if let (Some(offset), Some(b)) = (file.hover_request_due(now), file.buffer.as_ref()) {
                let token = lsp.hover(&path, file.lang, b, offset);
                file.hover_requested(token);
            }
            file.tick_hover(now);
            if file.buffer.is_none() || !synced.insert(path.clone()) {
                return;
            }
            for update in updates.iter().filter(|update| update.path == path) {
                file.set_diagnostics(update);
            }
            file.refresh();
            if let Some(b) = file.buffer.as_ref() {
                lsp.sync_document(&path, file.lang, b);
                if std::mem::take(&mut file.saved_unannounced) {
                    lsp.did_save(&path, b);
                }
            }
            file.lsp_triggers = lsp.completion_triggers(&path);
            if let Some((offset, trigger)) = file.completion_request_due() {
                let token = file
                    .buffer
                    .as_ref()
                    .and_then(|b| lsp.completion(&path, file.lang, b, offset, trigger.as_deref()));
                file.completion_requested(token);
            }
            if let Some(raw) = file.resolve_due() {
                let token = lsp.resolve_completion(&path, &raw);
                file.resolve_requested(token);
            }
            if let Some(raw) = file.doc_resolve_due() {
                let token = lsp.resolve_completion(&path, &raw);
                file.doc_resolve_requested(token);
            }
            while let Some((offset, kind)) = file.definition_request_due() {
                let token = file
                    .buffer
                    .as_ref()
                    .and_then(|b| lsp.definitions(&path, file.lang, b, offset, kind));
                file.definition_requested(token);
            }
        };
        let mut navigations: Vec<Navigation> = Vec::new();
        let mut other_navigations: Vec<Navigation> = Vec::new();
        self.panes
            .for_each_item_mut(&mut |item| serve(item, &mut navigations));
        if let Some(group) = other.as_deref_mut() {
            group.for_each_item_mut(&mut |item| serve(item, &mut other_navigations));
        }
        let closed: Vec<PathBuf> = lsp
            .open_documents()
            .filter(|path| !synced.contains(*path))
            .map(Path::to_path_buf)
            .collect();
        for path in closed {
            lsp.close_document(&path);
        }
        for (targets, caret_top) in navigations {
            open_definition(&self.root, &mut self.panes, &targets, caret_top);
        }
        if let Some(group) = other {
            for (targets, caret_top) in other_navigations {
                open_definition(&self.root, group, &targets, caret_top);
            }
        }
    }

    fn go_to_line_item(&mut self, path: &[usize]) -> Option<&mut FileItem> {
        let pane = self.panes.group.leaf_at_mut(path)?;
        let index = pane.active?;
        pane.open
            .get_mut(index)?
            .as_any_mut()?
            .downcast_mut::<FileItem>()
    }

    fn open_go_to_line(&mut self) {
        let path = self.panes.active.clone();
        let Some(item) = self.go_to_line_item(&path) else {
            return;
        };
        let (line, column) = item.caret_line_column();
        let modal =
            go_to_line::GoToLine::new(line, column, item.line_count(), item.scroll_position());
        self.go_to_line = Some((path, modal));
    }

    fn close_go_to_line(&mut self, confirm: bool) {
        let Some((path, modal)) = self.go_to_line.take() else {
            return;
        };
        let Some(item) = self.go_to_line_item(&path) else {
            return;
        };
        match (confirm, modal.target()) {
            (true, Some((line, column))) => item.go_to(line, column),
            _ => {
                item.highlighted_rows = None;
                if let Some(scroll) = modal.prev_scroll {
                    item.set_scroll_position(scroll);
                }
            }
        }
    }

    fn go_to_line_key(&mut self, key: EditKey, shift: bool) {
        match key {
            EditKey::Escape | EditKey::ToggleGoToLine => return self.close_go_to_line(false),
            EditKey::Enter => return self.close_go_to_line(true),
            _ => {}
        }
        let Some((path, modal)) = self.go_to_line.as_mut() else {
            return;
        };
        let edited = if key == EditKey::Tab {
            modal.accept_placeholder();
            true
        } else {
            modal.field.key(key, shift)
        };
        if edited {
            let (path, target) = (path.clone(), modal.target());
            if let Some(item) = self.go_to_line_item(&path) {
                item.preview_line(target);
            }
        }
    }

    fn go_to_line_input(&mut self, text: &str) {
        let Some((path, modal)) = self.go_to_line.as_mut() else {
            return;
        };
        modal.field.insert(&text.replace('\n', ""));
        let (path, target) = (path.clone(), modal.target());
        if let Some(item) = self.go_to_line_item(&path) {
            item.preview_line(target);
        }
    }

    fn toggle_outline(&mut self) {
        if self.outline.is_some() {
            return self.close_outline(false);
        }
        let path = self.panes.active.clone();
        let (window_size, preview) = (self.window_size, self.outline_preview);
        let Some(item) = self.go_to_line_item(&path) else {
            return;
        };
        let symbols = item.outline_symbols();
        let cursor = item.buffer.as_ref().map_or(0, EditorBuffer::cursor);
        let mut view = outline_view::OutlineView::new(
            symbols,
            cursor,
            item.scroll_position(),
            (0.0, 0.0),
            preview,
        );
        view.set_viewport(window_size);
        self.outline = Some((path, view));
    }

    /// Close the outline; confirming jumps to the selected symbol, otherwise the scroll comes back.
    fn close_outline(&mut self, confirm: bool) {
        let Some((path, view)) = self.outline.take() else {
            return;
        };
        let target = view.selected_symbol().map(|symbol| symbol.range.start);
        let Some(item) = self.go_to_line_item(&path) else {
            return;
        };
        match (confirm, target) {
            (true, Some(offset)) => item.go_to_offset(offset),
            _ => {
                item.preview_range(None);
                if let Some(scroll) = view.prev_scroll {
                    item.set_scroll_position(scroll);
                }
            }
        }
    }

    /// Preview the selected symbol, or put the view back when the query was cleared.
    fn preview_outline(&mut self, restore: bool) {
        let Some((path, view)) = self.outline.as_ref() else {
            return;
        };
        let (path, range, scroll) = (
            path.clone(),
            view.selected_symbol().map(|symbol| symbol.range.clone()),
            view.prev_scroll,
        );
        let Some(item) = self.go_to_line_item(&path) else {
            return;
        };
        if restore {
            item.preview_range(None);
            if let Some(scroll) = scroll {
                item.set_scroll_position(scroll);
            }
        } else {
            item.preview_range(range);
        }
    }

    fn outline_key(&mut self, key: EditKey, shift: bool) {
        let Some((_, view)) = self.outline.as_mut() else {
            return;
        };
        match key {
            EditKey::Escape | EditKey::ToggleOutline => return self.close_outline(false),
            EditKey::Enter => return self.close_outline(true),
            EditKey::Up => view.select_previous(),
            EditKey::Down => view.select_next(),
            EditKey::TogglePickerPreview => {
                let next = if view.preview == outline_view::PreviewLayout::Hidden {
                    outline_view::PreviewLayout::Right
                } else {
                    outline_view::PreviewLayout::Hidden
                };
                return self.set_outline_preview(next);
            }
            EditKey::SetPickerPreviewRight => {
                return self.set_outline_preview(outline_view::PreviewLayout::Right)
            }
            EditKey::AddCursorBelow => {
                return self.set_outline_preview(outline_view::PreviewLayout::Below)
            }
            EditKey::AddCursorAbove => {
                return self.set_outline_preview(outline_view::PreviewLayout::Hidden)
            }
            _ => {
                if !view.field.key(key, shift) {
                    return;
                }
                view.update_matches();
                let restore = view.query_is_empty();
                return self.preview_outline(restore);
            }
        }
        self.preview_outline(false);
    }

    fn set_outline_preview(&mut self, layout: outline_view::PreviewLayout) {
        self.outline_preview = layout;
        if let Some((_, view)) = self.outline.as_mut() {
            view.set_preview(layout);
        }
    }

    fn outline_input(&mut self, text: &str) {
        let Some((_, view)) = self.outline.as_mut() else {
            return;
        };
        view.field.insert(&text.replace('\n', ""));
        view.update_matches();
        let restore = view.query_is_empty();
        self.preview_outline(restore);
    }

    fn toggle_palette(&mut self) {
        if self.palette.take().is_some() {
            return;
        }
        let path = self.panes.active.clone();
        if self.go_to_line_item(&path).is_some() {
            let palette = command_palette::CommandPalette::new(&self.palette_memory);
            self.palette = Some((path, palette));
        }
    }

    fn palette_key(&mut self, key: EditKey, shift: bool) {
        let Some((_, palette)) = self.palette.as_mut() else {
            return;
        };
        match key {
            EditKey::Escape | EditKey::ToggleCommandPalette => self.palette = None,
            EditKey::Enter => self.confirm_palette(),
            EditKey::Up => palette.select_previous(&mut self.palette_memory),
            EditKey::Down => palette.select_next(&mut self.palette_memory),
            _ => {
                if palette.field.key(key, shift) {
                    palette.update_matches();
                }
            }
        }
    }

    fn palette_input(&mut self, text: &str) {
        if let Some((_, palette)) = self.palette.as_mut() {
            palette.field.insert(&text.replace('\n', ""));
            palette.update_matches();
        }
    }

    fn palette_click(&mut self, click: command_palette::PaletteClick) {
        if let (command_palette::PaletteClick::Row(row), Some((_, palette))) =
            (click, self.palette.as_mut())
        {
            palette.select_row(row);
        }
        self.track_nav(|v| v.confirm_palette());
    }

    fn confirm_palette(&mut self) {
        let Some((path, palette)) = self.palette.take() else {
            return;
        };
        let Some(action) = palette.confirm(&mut self.palette_memory) else {
            return;
        };
        self.panes.active = path.clone();
        match action {
            command_palette::PaletteAction::Key(key) => {
                if !self.modal_key(key, false) {
                    self.panes.editor_key_untracked(key, false);
                }
            }
            command_palette::PaletteAction::Text(transform) => {
                if let Some(item) = self.go_to_line_item(&path) {
                    item.manipulate_text(transform);
                }
            }
            command_palette::PaletteAction::Lines(transform) => {
                if let Some(item) = self.go_to_line_item(&path) {
                    item.run_line_command(LineCommand::Manipulate(transform));
                }
            }
        }
        if let Some(pane) = self.panes.group.leaf_at_mut(&path) {
            if let Some((bar, item)) = pane.search_target() {
                bar.refresh(item);
            }
        }
    }

    fn track_nav<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let snapshot = self.panes.nav_snapshot();
        let result = f(self);
        self.panes.record_nav_jump(snapshot);
        result
    }

    /// Keys the editor service handles before the panes: modals, the tree's inline rename, and their toggles.
    fn modal_key(&mut self, key: EditKey, shift: bool) -> bool {
        if key == EditKey::NewCenterTerminal {
            self.palette = None;
            self.request = Some(workspace::ViewRequest::NewCenterTerminal);
            return true;
        }
        if self.tree_edit_active() {
            self.tree_edit_key(key, shift);
            return true;
        }
        if self.outline.is_some() {
            self.outline_key(key, shift);
            return true;
        }
        if key == EditKey::ToggleOutline {
            self.palette = None;
            self.close_go_to_line(false);
            self.toggle_outline();
            return true;
        }
        if self.palette.is_some() {
            self.palette_key(key, shift);
            return true;
        }
        if key == EditKey::ToggleCommandPalette {
            self.go_to_line = None;
            self.toggle_palette();
            return true;
        }
        if self.go_to_line.is_some() {
            self.go_to_line_key(key, shift);
            return true;
        }
        if key == EditKey::ToggleGoToLine {
            self.open_go_to_line();
            return true;
        }
        false
    }

    /// Text for an open modal or the tree's inline rename; returns false when none takes it.
    fn modal_text(&mut self, text: &str) -> bool {
        if self.tree_edit_active() {
            self.tree_edit_input(text);
            return true;
        }
        if self.outline.is_some() {
            self.outline_input(text);
            return true;
        }
        if self.palette.is_some() {
            self.palette_input(text);
            return true;
        }
        if self.go_to_line.is_some() {
            self.go_to_line_input(text);
            return true;
        }
        false
    }

    /// Open the definition a cmd-click in an editor resolved.
    fn follow_navigation(&mut self) {
        let mut navigations = Vec::new();
        self.panes.for_each_item_mut(&mut |item| {
            if let Some(file) = item.as_any_mut().and_then(|a| a.downcast_mut::<FileItem>()) {
                navigations.extend(file.navigation.take());
            }
        });
        for (targets, caret_top) in navigations {
            open_definition(&self.root, &mut self.panes, &targets, caret_top);
        }
    }
}

type Navigation = (Vec<lsp::DefinitionTarget>, Option<f32>);

/// Open the first of `targets` in `group`'s focused pane and select it, recording the jump in its history.
fn open_definition(
    root: &Path,
    group: &mut PaneGroupView,
    targets: &[lsp::DefinitionTarget],
    caret_top: Option<f32>,
) {
    let Some(target) = targets.first() else {
        return;
    };
    let snapshot = group.nav_snapshot();
    let (file_root, relative) = match target.path.strip_prefix(root) {
        Ok(relative) => (root.to_path_buf(), relative.to_string_lossy().into_owned()),
        Err(_) => (
            PathBuf::from("/"),
            target
                .path
                .to_string_lossy()
                .trim_start_matches('/')
                .to_string(),
        ),
    };
    if let Some(pane) = group.active_pane_mut() {
        pane.open_file(&file_root, &relative);
    }
    if let Some(file) = group
        .active_item_mut()
        .and_then(|item| item.as_any_mut())
        .and_then(|any| any.downcast_mut::<FileItem>())
    {
        file.select_target_range(target.range, caret_top);
    }
    group.record_nav_jump(snapshot);
}

/// The Material file-type icon for a filename, by full name then extension (mirrors material-icon-theme's
/// `fileNames`/`fileExtensions`). Falls back to a generic document.
fn file_icon(name: &str) -> MaterialIcon {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" => {
            return MaterialIcon::Lock
        }
        "package.json" => return MaterialIcon::NodeJs,
        ".gitignore" | ".gitattributes" | ".gitmodules" => return MaterialIcon::Git,
        _ => {}
    }
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => MaterialIcon::Rust,
        "go" => MaterialIcon::Go,
        "ts" | "mts" | "cts" => MaterialIcon::TypeScript,
        "tsx" | "jsx" => MaterialIcon::React,
        "js" | "mjs" | "cjs" => MaterialIcon::JavaScript,
        "json" => MaterialIcon::Json,
        "md" | "markdown" | "mdx" => MaterialIcon::Markdown,
        "toml" => MaterialIcon::Toml,
        "yaml" | "yml" => MaterialIcon::Yaml,
        "html" | "htm" => MaterialIcon::Html,
        "css" => MaterialIcon::Css,
        "scss" | "sass" => MaterialIcon::Sass,
        "py" | "pyi" => MaterialIcon::Python,
        "lock" => MaterialIcon::Lock,
        "sh" | "bash" | "zsh" | "fish" => MaterialIcon::Console,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => MaterialIcon::Image,
        _ => MaterialIcon::Document,
    }
}

/// Depth-first flatten of the visible tree (a directory's children appear only when it is in `expanded`).
fn flatten(nodes: &[FileNode], depth: usize, expanded: &HashSet<String>, out: &mut Vec<Row>) {
    for n in nodes {
        let open = n.is_dir && expanded.contains(&n.path);
        out.push(Row {
            name: n.name.clone(),
            path: n.path.clone(),
            is_dir: n.is_dir,
            depth,
            open,
            edit: false,
        });
        if open {
            flatten(&n.children, depth + 1, expanded, out);
        }
    }
}

fn blend(base: Rgba, over: Rgba) -> Rgba {
    let mix = |b: f32, o: f32| b * (1.0 - over.a) + o * over.a;
    Rgba::new(
        mix(base.r, over.r),
        mix(base.g, over.g),
        mix(base.b, over.b),
        1.0,
    )
}

fn color_of(theme: &Theme, capture: &str) -> Rgba {
    let c = if capture.is_empty() {
        theme.foreground
    } else {
        theme.syntax_color(capture)
    };
    let [r, g, b] = c.0;
    Rgba::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

impl ItemInput for FilesView {
    fn editor_key(&mut self, key: EditKey, shift: bool) -> bool {
        if self.track_nav(|view| view.modal_key(key, shift)) {
            return true;
        }
        self.panes.editor_key(key, shift)
    }

    fn editor_text(&mut self, text: &str) -> bool {
        if self.track_nav(|view| view.modal_text(text)) {
            return true;
        }
        self.panes.editor_text(text)
    }

    fn editor_paste(&mut self, text: &str, slices: Option<&[ClipboardSlice]>) -> bool {
        if self.track_nav(|view| view.modal_text(text)) {
            return true;
        }
        self.panes.editor_paste(text, slices)
    }

    fn editor_ime_preedit(&mut self, text: &str, selected: Option<Range<usize>>) -> bool {
        self.panes.editor_ime_preedit(text, selected)
    }

    fn editor_ime_commit(&mut self, text: &str) -> bool {
        self.panes.editor_ime_commit(text)
    }

    fn editor_click(&mut self, x: f32, y: f32, extend: bool) -> bool {
        self.confirm_tree_edit(true);
        let clicked = self.panes.editor_click(x, y, extend);
        self.follow_navigation();
        clicked
    }

    fn editor_double_click(&mut self, x: f32, y: f32) -> bool {
        self.panes.editor_double_click(x, y)
    }

    fn editor_drag(&mut self, x: f32, y: f32) -> bool {
        self.panes.editor_drag(x, y)
    }

    fn editor_hover(&mut self, x: f32, y: f32) -> bool {
        self.panes.editor_hover(x, y)
    }

    fn editor_scroll(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        self.panes.editor_scroll(x, y, dx, dy)
    }

    fn editor_copy(&self) -> Option<CopiedText> {
        if let Some(edit) = self.tree_ops.edit.as_ref() {
            return edit.field.selected_text().map(|text| CopiedText {
                text,
                slices: Vec::new(),
            });
        }
        self.panes.editor_copy()
    }

    fn editor_cut(&mut self) -> Option<CopiedText> {
        self.panes.editor_cut()
    }

    fn editor_copy_trimmed(&self) -> Option<CopiedText> {
        self.panes.editor_copy_trimmed()
    }

    fn editor_selected_text(&self) -> Option<String> {
        self.panes.editor_selected_text()
    }

    fn editor_right_press(&mut self, x: f32, y: f32) -> bool {
        self.panes.editor_right_press(x, y)
    }

    fn editor_menu_anchor_at(&self, x: f32, y: f32) -> Option<(Vec<usize>, usize)> {
        self.panes.editor_menu_anchor_at(x, y)
    }

    fn editor_split_diff(&self) -> Option<bool> {
        let file = self
            .panes
            .active_item()?
            .as_any()?
            .downcast_ref::<FileItem>()?;
        file.branch_diff.then_some(file.split)
    }

    fn editor_menu_y(&self, path: &[usize], line: usize) -> Option<f32> {
        self.panes.editor_menu_y(path, line)
    }

    fn editor_save(&mut self) -> Option<Result<(), String>> {
        self.panes.editor_save()
    }

    fn editor_focused(&self) -> bool {
        self.tree_edit_active() || self.panes.editor_focused()
    }

    fn cursor_position(&self) -> Option<String> {
        self.panes.cursor_position()
    }

    fn editor_popovers(&mut self, viewport: (f32, f32)) -> Vec<(Node, f32, f32)> {
        let popovers = self.panes.editor_popovers(viewport);
        if self.outline.is_some() || self.palette.is_some() || self.go_to_line.is_some() {
            return Vec::new();
        }
        popovers
    }

    fn popover_scroll(&mut self, index: usize, dy: f32) -> bool {
        self.panes.popover_scroll(index, dy)
    }

    fn popover_click(&mut self, id: u64) -> bool {
        self.panes.popover_click(id)
    }

    fn popover_hover(&mut self, id: Option<u64>) {
        self.panes.popover_hover(id);
    }

    fn active_wants_keystrokes(&self) -> bool {
        self.panes.active_wants_keystrokes()
    }

    fn item_keystroke(&mut self, keystroke: &terminal::Keystroke) -> workspace::TerminalKeyOutcome {
        self.panes.item_keystroke(keystroke)
    }

    fn item_text(&mut self, text: &str) {
        self.panes.item_text(text);
    }

    fn item_paste(&mut self, text: &str) {
        self.panes.item_paste(text);
    }

    fn item_focus_changed(&mut self, focused: bool) {
        self.panes.item_focus_changed(focused);
    }

    fn item_pointer_down(
        &mut self,
        x: f32,
        y: f32,
        click_count: u32,
        modifiers: terminal::Modifiers,
    ) -> bool {
        self.panes.item_pointer_down(x, y, click_count, modifiers)
    }

    fn item_pointer_drag(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) -> bool {
        self.panes.item_pointer_drag(x, y, modifiers)
    }

    fn item_pointer_move(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) -> bool {
        self.panes.item_pointer_move(x, y, modifiers)
    }

    fn item_pointer_up(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) {
        self.panes.item_pointer_up(x, y, modifiers);
    }

    fn item_pointer_scroll(
        &mut self,
        x: f32,
        y: f32,
        delta: (f32, f32),
        modifiers: terminal::Modifiers,
    ) -> bool {
        self.panes.item_pointer_scroll(x, y, delta, modifiers)
    }
}

impl FunctionView for FilesView {
    fn modal(&mut self, viewport: (f32, f32)) -> Option<ModalView> {
        self.window_size = viewport;
        if let Some((path, view)) = self.outline.as_mut() {
            view.set_viewport(viewport);
            let path = path.clone();
            let shown = view.preview != outline_view::PreviewLayout::Hidden;
            let selected = view.selected_symbol().cloned();
            let capacity =
                |view: &outline_view::OutlineView, gutter: f32| view.preview_capacity(gutter);
            let preview = match (shown, selected) {
                (true, Some(symbol)) => {
                    let gutter = {
                        let item = self.go_to_line_item(&path)?;
                        let dims = GutterDimensions::for_lines(item.line_count());
                        dims.full_width() - dims.fold_area_width()
                    };
                    let (rows, columns) = self
                        .outline
                        .as_ref()
                        .map(|(_, view)| capacity(view, gutter))?;
                    self.go_to_line_item(&path)
                        .and_then(|item| item.preview_content(&symbol, rows, columns))
                }
                _ => None,
            };
            let (_, view) = self.outline.as_ref()?;
            return Some(ModalView {
                node: view.render(OUTLINE_BASE, preview),
                width: view.size().width,
                elevation: Elevation::Modal,
            });
        }
        if let Some((_, palette)) = self.palette.as_ref() {
            return Some(ModalView {
                node: palette.render(PALETTE_BASE),
                width: command_palette::WIDTH,
                elevation: Elevation::Modal,
            });
        }
        let (_, modal) = self.go_to_line.as_ref()?;
        Some(ModalView {
            node: modal.render(),
            width: go_to_line::WIDTH,
            elevation: Elevation::Elevated,
        })
    }

    fn diagnostic_summary(&self) -> Option<workspace::DiagnosticSummary> {
        let (errors, warnings) = self
            .lsp
            .as_ref()
            .map_or((0, 0), |lsp| lsp.diagnostic_summary());
        let current = self
            .panes
            .active_item()
            .and_then(|item| item.diagnostic_message())
            .map(|message| message.lines().next().unwrap_or_default().to_string());
        Some(workspace::DiagnosticSummary {
            errors,
            warnings,
            current,
        })
    }

    fn modal_scroll(&mut self, dy: f32) -> bool {
        if let Some((_, view)) = self.outline.as_mut() {
            return view.scroll_by(dy);
        }
        if let Some((_, palette)) = self.palette.as_mut() {
            return palette.scroll_by(dy);
        }
        false
    }

    fn dismiss_modal(&mut self) {
        self.close_outline(false);
        self.palette = None;
        self.close_go_to_line(false);
    }

    fn sync_items(&mut self, mut other: Option<&mut PaneGroupView>) {
        self.sync_other_group(other.as_deref_mut());
        self.sync_language_servers(other);
    }

    fn editor_layout(&mut self, area: Rect) -> EditorLayout {
        let (panes, dividers) = self.panes.layout(area);
        EditorLayout { panes, dividers }
    }

    fn render_tree(&mut self) -> Option<workspace::TreePanel> {
        self.apply_scan();
        // Left: the file tree. Rebuild the flattened visible rows + the click-id mapping.
        // Re-flatten only when the tree structure changed (expand/collapse), not on scroll.
        if self.flat_dirty {
            let f = self.visible_rows();
            self.click_targets = f.iter().map(|r| (r.path.clone(), r.is_dir)).collect();
            self.flat_cache = f;
            self.flat_dirty = false;
        }
        self.content_h = 8.0 + self.flat_cache.len() as f32 * ROW_H;

        // Virtualize by row: render only the visible window starting at `first`. To scroll pixel-smoothly the
        // caller shifts the whole window up by the sub-row remainder `y_offset` rather than snapping per row;
        // the extra `+2` rows cover the partial row exposed at the top and bottom.
        let first = ((self.scroll / ROW_H).floor().max(0.0)) as usize;
        let visible = (self.viewport_h / ROW_H).ceil() as usize + 2;
        let last = (first + visible).min(self.flat_cache.len());
        let y_offset = first as f32 * ROW_H - self.scroll;
        // Widest visible row only (cheap), for horizontal-scroll clamping: indent + icons + name text.
        self.content_w = self.flat_cache[first..last]
            .iter()
            .map(|r| 46.0 + r.depth as f32 * INDENT + r.name.chars().count() as f32 * 7.5)
            .fold(0.0_f32, f32::max);
        let flat = &self.flat_cache;
        let mut tree_col = div().col().py(4.0);
        let active_path = self.active_path();
        for (i, row) in flat.iter().enumerate().take(last).skip(first) {
            let id = FUNC_VIEW_BASE + i as u64;
            let selected = active_path.as_deref() == Some(row.path.as_str());
            // One column (INDENT wide) per depth. Each ancestor column holds a centered 1px indent guide; the
            // last column holds this row's disclosure chevron — so guides line up exactly under the chevrons.
            let mut lead_row = div().row().items_center();
            for _ in 0..row.depth {
                lead_row = lead_row.child(
                    div()
                        .row()
                        .w_px(INDENT)
                        .h_px(ROW_H)
                        .justify_center()
                        .child(div().w_px(1.0).h_px(ROW_H).bg(theme().panel_indent_guide)),
                );
            }
            let chevron: Node = if row.is_dir {
                icon(if row.open {
                    IconKind::ChevronDown
                } else {
                    IconKind::ChevronRight
                })
                .size(12.0)
                .color(theme().icon_muted)
                .into()
            } else {
                div().into()
            };
            lead_row = lead_row.child(
                div()
                    .row()
                    .w_px(INDENT)
                    .h_px(ROW_H)
                    .items_center()
                    .justify_center()
                    .child(chevron),
            );
            // Folders use the monochrome folder glyph (open/closed); files use the full-color Material icon.
            let glyph: Node = if row.is_dir {
                let g = if row.open {
                    IconKind::FolderOpen
                } else {
                    IconKind::Folder
                };
                icon(g).size(14.0).color(theme().icon_accent).into()
            } else {
                material_icon(file_icon(&row.name)).size(15.0).into()
            };
            let edit = self.tree_ops.edit.as_ref().filter(|_| row.edit);
            let glyph = match edit {
                Some(edit) if !row.is_dir => {
                    let typed = edit.field.text();
                    let name = if typed.chars().count() > 2 {
                        typed
                    } else {
                        row.name.clone()
                    };
                    material_icon(file_icon(&name)).size(15.0).into()
                }
                _ => glyph,
            };
            let name: Node = match edit {
                Some(edit) => edit.field.render(
                    "",
                    true,
                    theme().text,
                    ROW_H - 2.0,
                    text_field::FieldFont::Ui,
                ),
                None => label(row.name.clone()).color(theme().text).into(),
            };
            let content = div()
                .row()
                .gap(5.0)
                .pr(6.0)
                .items_center()
                .flex(if edit.is_some() { 1.0 } else { 0.0 })
                .child(lead_row)
                .child(glyph)
                .child(name);
            let mut r = div()
                .row()
                .h_px(ROW_H)
                .pl(8.0)
                .items_center()
                .rounded(4.0)
                .on_click(id)
                .child(content);
            if row.edit {
                r = r.border(1.0, theme().panel_focused_border);
            } else if selected {
                r = r.bg(theme().element_selected);
            }
            tree_col = tree_col.child(r);
        }

        // Sticky breadcrumb: the ancestor folders of the top visible row, pinned (opaque) at the tree's top.
        // Each is clickable (collapse that folder). A bottom border + soft shadow separate it from the scroll.
        self.sticky_paths.clear();
        let mut sticky = div().col();
        if first > 0 {
            if let Some(top) = flat.get(first) {
                let comps: Vec<&str> = top.path.split('/').collect();
                for k in 0..top.depth {
                    let name = comps.get(k).copied().unwrap_or("").to_string();
                    let path = comps[..=k].join("/");
                    let click_id = STICKY_BASE + self.sticky_paths.len() as u64;
                    self.sticky_paths.push(path);
                    let mut lead = div().row().items_center();
                    for _ in 0..k {
                        lead =
                            lead.child(
                                div().row().w_px(INDENT).h_px(ROW_H).justify_center().child(
                                    div().w_px(1.0).h_px(ROW_H).bg(theme().panel_indent_guide),
                                ),
                            );
                    }
                    lead = lead.child(
                        div()
                            .row()
                            .w_px(INDENT)
                            .h_px(ROW_H)
                            .items_center()
                            .justify_center()
                            .child(
                                icon(IconKind::ChevronDown)
                                    .size(12.0)
                                    .color(theme().icon_muted),
                            ),
                    );
                    sticky = sticky.child(
                        div()
                            .row()
                            .h_px(ROW_H)
                            .pl(8.0)
                            .items_center()
                            .bg(theme().panel_background)
                            .on_click(click_id)
                            .child(
                                div()
                                    .row()
                                    .gap(5.0)
                                    .pr(6.0)
                                    .items_center()
                                    .child(lead)
                                    .child(
                                        icon(IconKind::FolderOpen)
                                            .size(14.0)
                                            .color(theme().icon_accent),
                                    )
                                    .child(label(name).color(theme().text)),
                            ),
                    );
                }
                // Separator under the pinned stack: a 1px border then a soft shadow fading into the content.
                sticky = sticky
                    .child(div().h_px(1.0).bg(theme().border))
                    .child(div().h_px(4.0).bg(Rgba::new(0.0, 0.0, 0.0, 0.10)));
            }
        }

        Some(workspace::TreePanel {
            tree: tree_col.into(),
            sticky: sticky.into(),
            scroll_x: self.scroll_x,
            content_w: self.content_w,
            y_offset,
        })
    }

    fn on_click(&mut self, id: u64) -> bool {
        match self.panes.click(id) {
            GroupClick::NotMine => {}
            GroupClick::Handled => return true,
            GroupClick::Search { pane, click } => {
                self.track_nav(|v| v.panes.search_click(pane, click));
                return true;
            }
            GroupClick::Button { path, action } => {
                if let PaneButtonAction::Split(direction) = action {
                    let item = self.panes.clone_active_of(&path);
                    self.panes.split(&path, direction, item);
                }
                return true;
            }
        }
        if id >= OUTLINE_BASE {
            match outline_view::OutlineClick::from_offset(id - OUTLINE_BASE) {
                outline_view::OutlineClick::Row(row) => {
                    if let Some((_, view)) = self.outline.as_mut() {
                        view.select_row(row);
                    }
                    self.track_nav(|v| v.close_outline(true));
                }
                outline_view::OutlineClick::TogglePreview => {
                    self.outline_key(EditKey::TogglePickerPreview, false)
                }
                outline_view::OutlineClick::PreviewBelow => {
                    self.set_outline_preview(outline_view::PreviewLayout::Below)
                }
                outline_view::OutlineClick::PreviewRight => {
                    self.set_outline_preview(outline_view::PreviewLayout::Right)
                }
            }
            return true;
        }
        if id >= PALETTE_BASE {
            self.palette_click(command_palette::PaletteClick::from_offset(
                id - PALETTE_BASE,
            ));
            return true;
        }
        // A pinned sticky breadcrumb folder: collapse it and scroll so it becomes the top row.
        if id >= STICKY_BASE {
            let k = (id - STICKY_BASE) as usize;
            if let Some(path) = self.sticky_paths.get(k).cloned() {
                self.expanded.remove(&path);
                self.flat_dirty = true;
                // Scroll to the collapsed folder's row so it's visible at the top.
                let mut flat = Vec::new();
                flatten(&self.tree, 0, &self.expanded, &mut flat);
                if let Some(idx) = flat.iter().position(|r| r.path == path) {
                    self.scroll = idx as f32 * ROW_H;
                }
            }
            return true;
        }
        // Otherwise a tree row.
        let idx = (id - FUNC_VIEW_BASE) as usize;
        if self.flat_cache.get(idx).is_some_and(|row| row.edit) {
            return true;
        }
        if self.tree_edit_active() {
            self.confirm_tree_edit(true);
            return true;
        }
        let Some((path, is_dir)) = self.click_targets.get(idx).cloned() else {
            return false;
        };
        if is_dir {
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            self.flat_dirty = true;
        } else {
            self.open_file(&path);
        }
        true
    }

    fn scroll_offset(&self) -> f32 {
        self.scroll
    }

    fn content_height(&self) -> f32 {
        self.content_h
    }

    fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport_h = h;
        self.tree_vw = w;
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        // Tab hits reveal a close button and modal rows light up, so only changes there need a repaint.
        let tab_changed = self.panes.set_hover(id);
        let modal = |x: Option<u64>| x.is_some_and(|v| (PALETTE_BASE..MODAL_END).contains(&v));
        let entered = self.hover != id;
        let changed = tab_changed || (entered && (modal(self.hover) || modal(id)));
        self.hover = id;
        let over_modal = id.filter(|v| (PALETTE_BASE..MODAL_END).contains(v));
        let mut outline_hovered_row = None;
        if let Some((_, view)) = self.outline.as_mut() {
            view.hovered = over_modal;
            if let Some(outline_view::OutlineClick::Row(row)) = over_modal
                .filter(|v| *v >= OUTLINE_BASE && *v < COMPLETION_BASE)
                .map(|v| outline_view::OutlineClick::from_offset(v - OUTLINE_BASE))
            {
                // Pointing into the list shows its scrollbar, as scrolling does.
                view.scrollbar.reveal();
                // Moving onto a row selects it, so keyboard moves under a still pointer aren't undone.
                if entered && view.selected != row {
                    view.select_row(row);
                    outline_hovered_row = Some(row);
                }
            }
        }
        if outline_hovered_row.is_some() {
            self.preview_outline(false);
        }
        if let Some((_, palette)) = self.palette.as_mut() {
            palette.hovered = over_modal;
            if over_modal.is_some_and(|v| v > PALETTE_BASE && v < OUTLINE_BASE) {
                palette.scrollbar.reveal();
            }
            if let Some(command_palette::PaletteClick::Row(row)) = over_modal
                .filter(|v| *v >= PALETTE_BASE && *v < OUTLINE_BASE && entered)
                .map(|v| command_palette::PaletteClick::from_offset(v - PALETTE_BASE))
            {
                palette.select_row(row);
            }
        }
        changed || over_modal.is_some()
    }

    fn divider_axis(&self, id: u64) -> Option<DividerAxis> {
        self.panes.divider_axis(id)
    }

    fn drag_divider(&mut self, id: u64, x: f32, y: f32) -> bool {
        self.panes.drag_divider(id, x, y)
    }

    fn is_tab(&self, id: u64) -> bool {
        self.panes.is_tab(id)
    }

    fn begin_tab_drag(&mut self, id: u64) -> bool {
        self.panes.begin_tab_drag(id)
    }

    fn update_tab_drag(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool {
        self.panes.update_tab_drag(x, y, over)
    }

    fn drop_tab(&mut self) -> bool {
        self.panes.drop_tab()
    }

    fn cancel_tab_drag(&mut self) {
        self.panes.cancel_tab_drag();
    }

    fn dragging_tab(&self) -> bool {
        self.panes.dragging_tab()
    }

    fn tab_drag_overlay(&self) -> Option<Rect> {
        self.panes.tab_drag_overlay()
    }

    fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)> {
        self.panes.tab_drag_ghost()
    }

    fn save_panes(&self) -> Option<workspace::persistence::SerializedMember> {
        Some(self.panes.serialize())
    }

    fn restore_panes(
        &mut self,
        saved: &workspace::persistence::SerializedMember,
        fallback: &mut dyn FnMut(&workspace::persistence::SerializedItem) -> Option<Box<dyn Item>>,
    ) -> bool {
        self.panes.restore(saved, &mut |item| {
            saved_state::restore_item(item).or_else(|| fallback(item))
        })
    }

    fn zoom_shown(&self) -> bool {
        self.panes.zoom_shown()
    }

    fn pane_group(&self) -> Option<&PaneGroupView> {
        Some(&self.panes)
    }

    fn pane_group_mut(&mut self) -> Option<&mut PaneGroupView> {
        Some(&mut self.panes)
    }

    fn reveal_in_tree(&mut self, path: &Path) {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return;
        };
        let relative = relative.to_string_lossy().into_owned();
        self.reveal_row(&relative);
    }

    fn claims_key(&self, key: EditKey) -> bool {
        !self.accepts_pane_keys()
            || matches!(
                key,
                EditKey::NewCenterTerminal
                    | EditKey::ToggleCommandPalette
                    | EditKey::ToggleOutline
                    | EditKey::ToggleGoToLine
            )
    }

    fn accepts_pane_keys(&self) -> bool {
        self.outline.is_none()
            && self.palette.is_none()
            && self.go_to_line.is_none()
            && !self.tree_edit_active()
    }

    fn pane_command(&mut self, command: PaneCommand) -> bool {
        match command {
            PaneCommand::Split(direction) => {
                let path = self.panes.active.clone();
                let item = self.panes.clone_active_of(&path);
                self.panes.split(&path, direction, item);
                true
            }
            command => self.panes.pane_command(command),
        }
    }

    fn dragged_item(&self) -> Option<&dyn Item> {
        self.panes.dragged_item()
    }

    fn take_dragged_item(&mut self) -> Option<Box<dyn Item>> {
        self.panes.take_dragged_item()
    }

    fn accepts_item(&self, _item: &dyn Item) -> bool {
        true
    }

    fn update_foreign_drop(
        &mut self,
        x: f32,
        y: f32,
        over: Option<(u64, Rect)>,
        item: &dyn Item,
    ) -> bool {
        self.panes.update_foreign_drop(x, y, over, item)
    }

    fn clear_foreign_drop(&mut self) {
        self.panes.clear_foreign_drop();
    }

    fn accept_foreign_item(&mut self, item: Box<dyn Item>) {
        self.panes.accept_foreign_item(item);
    }

    fn refresh_disk_state(&mut self) {
        self.panes.refresh_disk_state();
    }

    fn is_busy(&self) -> bool {
        // A picker's scrollbar keeps fading out until it's gone.
        let busy = self
            .outline
            .as_ref()
            .is_some_and(|(_, view)| view.scrollbar.is_animating())
            || self
                .palette
                .as_ref()
                .is_some_and(|(_, palette)| palette.scrollbar.is_animating());
        busy || self.panes.is_busy()
    }

    fn tree_menu_state(&self, path: Option<&str>) -> workspace::TreeMenuState {
        self.menu_state(path)
    }

    fn tree_action(
        &mut self,
        path: Option<&str>,
        action: workspace::TreeAction,
    ) -> Option<workspace::Prompt> {
        self.cancel_tree_edit();
        self.run_tree_action(path, action)
    }

    fn prompt_answered(&mut self, token: u64, answer: usize) {
        self.tree_prompt_answered(token, answer);
    }

    fn take_toast(&mut self) -> Option<String> {
        self.take_tree_toast()
    }

    fn take_request(&mut self) -> Option<workspace::ViewRequest> {
        self.request.take()
    }

    fn add_center_item(&mut self, item: Box<dyn Item>) {
        if let Some(pane) = self.panes.active_pane_mut() {
            pane.add_item(item);
        }
    }

    fn tick_items(&mut self, clipboard: &dyn Fn() -> Option<String>) -> workspace::ItemTick {
        let mut outcome = workspace::ItemTick::default();
        let mut closed: Vec<(u64, String)> = Vec::new();
        self.panes.group.for_each_pane_mut(&mut |pane| {
            for item in pane.open.iter_mut() {
                let tick = item.tick(clipboard);
                outcome.changed |= tick.changed;
                if tick.clipboard_store.is_some() {
                    outcome.clipboard_store = tick.clipboard_store;
                }
                if tick.close {
                    if let Some(id) = item.id() {
                        closed.push((pane.id, id));
                    }
                }
            }
        });
        for (pane_id, item_id) in closed {
            let Some(path) = self.panes.group.path_of(pane_id) else {
                continue;
            };
            let index = self
                .panes
                .group
                .leaf_at(&path)
                .and_then(|pane| pane.index_of_id(&item_id));
            if let Some(index) = index {
                self.panes.close_tab(&path, index);
                outcome.changed = true;
            }
        }
        outcome
    }

    fn take_item_open_request(&mut self) -> Option<workspace::TerminalOpenTarget> {
        let mut request = None;
        self.panes.group.for_each_pane_mut(&mut |pane| {
            for item in pane.open.iter_mut() {
                if request.is_none() {
                    request = item.take_open_request();
                }
            }
        });
        request
    }

    fn item_link_hovered(&self) -> bool {
        let mut hovered = false;
        self.panes.group.for_each_pane(&mut |pane| {
            hovered |= pane.active_item().is_some_and(|item| item.link_hovered());
        });
        hovered
    }

    fn active_file_path(&self) -> Option<PathBuf> {
        let id = self.panes.active_item()?.id()?;
        Some(self.root.join(id))
    }

    fn open_diff(&mut self, path: &Path, base: Option<String>) {
        let path = path.to_path_buf();
        self.track_nav(|view| {
            let (root, relative) = match path.strip_prefix(&view.root) {
                Ok(relative) => (view.root.clone(), relative.to_string_lossy().into_owned()),
                Err(_) => (
                    PathBuf::from("/"),
                    path.to_string_lossy().trim_start_matches('/').to_string(),
                ),
            };
            let Some(pane) = view.panes.active_pane_mut() else {
                return;
            };
            let id = format!("{DIFF_ID_PREFIX}{relative}");
            if let Some(index) = pane.index_of_id(&id) {
                pane.activate_user(index);
                return;
            }
            let text = files::read(&root, &relative)
                .ok()
                .and_then(|contents| contents.text);
            pane.add_item(Box::new(FileItem::branch_diff(root, &relative, text, base)));
        });
    }

    fn open_file_at(&mut self, path: &Path, row: Option<u32>, column: Option<u32>) {
        let path = path.to_path_buf();
        self.track_nav(|view| {
            let (root, relative) = match path.strip_prefix(&view.root) {
                Ok(relative) => (view.root.clone(), relative.to_string_lossy().into_owned()),
                Err(_) => (
                    PathBuf::from("/"),
                    path.to_string_lossy().trim_start_matches('/').to_string(),
                ),
            };
            if let Some(pane) = view.panes.active_pane_mut() {
                pane.open_file(&root, &relative);
            }
            if let Some(row) = row {
                let active = view.panes.active.clone();
                if let Some(item) = view.go_to_line_item(&active) {
                    let column = column.unwrap_or(1).saturating_sub(1) as usize;
                    item.go_to(row.saturating_sub(1) as usize, column);
                }
            }
        });
    }

    fn row_path(&self, id: u64) -> Option<(String, bool)> {
        if id < FUNC_VIEW_BASE {
            return None;
        }
        let idx = (id - FUNC_VIEW_BASE) as usize;
        if self.flat_cache.get(idx).is_some_and(|row| row.edit) {
            return None;
        }
        self.click_targets.get(idx).cloned()
    }

    fn root_dir(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn open_path(&mut self, path: &str) {
        self.open_file(path);
    }

    fn on_scroll(&mut self, dx: f32, dy: f32, vw: f32, vh: f32) -> bool {
        let max_y = (self.content_h - vh).max(0.0);
        let ny = (self.scroll - dy).clamp(0.0, max_y);
        let max_x = (self.content_w - vw).max(0.0);
        let nx = (self.scroll_x - dx).clamp(0.0, max_x);
        if (ny - self.scroll).abs() < 0.01 && (nx - self.scroll_x).abs() < 0.01 {
            return false;
        }
        self.scroll = ny;
        self.scroll_x = nx;
        true
    }
}

#[cfg(test)]
mod line_layout_tests {
    use super::*;

    #[test]
    fn scrolling_an_editor_past_its_end_leaves_the_tree_alone() {
        let root = std::env::temp_dir().join(format!("pomelo-scroll-chain-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("root");
        for index in 0..200 {
            std::fs::write(root.join(format!("file{index:03}.txt")), "x\n").expect("file");
        }
        let mut files = FilesView::scanned(root.clone());
        files.open_path("file000.txt");
        let mut app = ui::Application::new();
        let (handle, entity) = app.open_raw_window(
            ui::WindowOptions {
                width: 1200.0,
                height: 800.0,
                scale: 2.0,
                ..Default::default()
            },
            move |_| {
                workspace::WorkspaceView::new(workspace::Layout {
                    project: Some(workspace::ProjectInfo::default()),
                    files_view: Some(Box::new(files)),
                    ..Default::default()
                })
            },
        );
        app.draw(handle);
        let (center, tree) = {
            let view = entity.read(app.app());
            let layout = view.layout();
            (
                layout.center_region(1200.0, 800.0),
                layout.tree_region(1200.0, 800.0),
            )
        };
        let tree_scroll = |app: &ui::Application| {
            entity
                .read(app.app())
                .layout()
                .files_view
                .as_ref()
                .map_or(0.0, |view| view.scroll_offset())
        };
        for _ in 0..20 {
            entity.update(app.app_mut(), |view, _| {
                view.scroll(
                    center.x + center.w / 2.0,
                    center.y + center.h / 2.0,
                    0.0,
                    -200.0,
                )
            });
        }
        assert_eq!(
            tree_scroll(&app),
            0.0,
            "the editor's overscroll moved the tree"
        );
        entity.update(app.app_mut(), |view, _| {
            view.scroll(tree.x + tree.w / 2.0, tree.y + tree.h / 2.0, 0.0, -200.0)
        });
        assert!(
            tree_scroll(&app) > 0.0,
            "scrolling over the tree scrolls it"
        );
        if let Err(error) = std::fs::remove_dir_all(&root) {
            eprintln!("leaving {}: {error}", root.display());
        }
    }

    #[test]
    fn new_view_opens_empty_and_fills_its_tree_from_the_scan() {
        let root = std::env::temp_dir().join(format!("pomelo-scan-view-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).expect("dirs");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("file");
        let mut view = FilesView::new(root.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while view.scan.is_some() && std::time::Instant::now() < deadline {
            view.render_tree();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(view.scan.is_none(), "scan finished");
        assert_eq!(view.tree, FilesView::scanned(root.clone()).tree);
        if let Err(error) = std::fs::remove_dir_all(&root) {
            eprintln!("leaving {}: {error}", root.display());
        }
    }

    #[test]
    fn display_index_expands_tabs_and_multibyte() {
        let b = EditorBuffer::from_text("\tab\u{e9}c\n");
        assert_eq!(FileItem::display_index(&b, 0, 0), 0);
        assert_eq!(FileItem::display_index(&b, 0, 1), TAB_COLS);
        assert_eq!(FileItem::display_index(&b, 0, 3), TAB_COLS + 2);
        assert_eq!(FileItem::display_index(&b, 0, 4), TAB_COLS + 4);
        for col in 0..=5 {
            let index = FileItem::display_index(&b, 0, col);
            assert_eq!(FileItem::char_col_for_display_index(&b, 0, index), col);
        }
        assert_eq!(FileItem::char_col_for_display_index(&b, 0, 3), 0);
    }

    #[test]
    fn layout_hit_testing_matches_glyph_boundaries() {
        let layout = LineLayout {
            glyphs: vec![(0, 0.0), (1, 10.0), (2, 20.0)],
            width: 30.0,
            len: 3,
        };
        assert_eq!(layout.x_for_index(1), 10.0);
        assert_eq!(layout.x_for_index(3), 30.0);
        assert_eq!(layout.closest_index_for_x(4.0), 0);
        assert_eq!(layout.closest_index_for_x(6.0), 1);
        assert_eq!(layout.closest_index_for_x(25.0), 3);
    }

    #[test]
    fn row_layout_spans_all_segments() {
        let item = FileItem::new(
            PathBuf::from("/nonexistent"),
            "t.rs",
            Some("fn main() {}\n".into()),
        );
        let layout = item.line_layout(0);
        assert_eq!(layout.len, "fn main() {}".len());
        assert!(layout.width > 0.0);
        assert!(layout
            .glyphs
            .windows(2)
            .all(|w| w[0].1 <= w[1].1 && w[0].0 < w[1].0));
    }
}

#[cfg(test)]
mod line_width_tests {
    use super::*;

    fn full_widths(item: &FileItem) -> Vec<Option<usize>> {
        let b = item.buffer.as_ref().unwrap();
        (0..b.rope.len_lines())
            .map(|l| Some(FileItem::vis_col(b, l, b.line_len(l))))
            .collect()
    }

    #[test]
    fn incremental_widths_match_full_recompute() {
        let text = "a\n\tbb\nccc\n\ndddd\n";
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.txt", Some(text.into()));
        item.refresh();
        assert_eq!(item.line_widths, full_widths(&item));
        type Step = Box<dyn Fn(&mut EditorBuffer)>;
        let steps: Vec<Step> = vec![
            Box::new(|b| {
                b.place_cursor(1);
                b.insert_text("XYZ\nlonger line here\n");
            }),
            Box::new(|b| {
                b.place_cursor(5);
                b.backspace();
                b.backspace();
            }),
            Box::new(|b| {
                b.select_all();
                b.insert_text("one\ntwo");
            }),
            Box::new(|b| b.undo()),
            Box::new(|b| b.undo()),
            Box::new(|b| b.redo()),
            Box::new(|b| {
                b.place_cursor(0);
                b.add_cursor(4);
                b.insert_text("\t\n");
            }),
        ];
        for step in steps {
            step(item.buffer.as_mut().unwrap());
            item.refresh();
            assert_eq!(item.line_widths, full_widths(&item));
            assert_eq!(item.wraps.len(), item.line_widths.len());
            item.ensure_visible();
            assert_eq!(
                item.max_row_cols,
                full_widths(&item).into_iter().flatten().max().unwrap_or(0)
            );
        }
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;

    fn item(lines: usize, visible_rows: usize) -> FileItem {
        let text: String = (0..lines).map(|i| format!("line {i}\n")).collect();
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.txt", Some(text));
        item.set_body_height(visible_rows as f32 * EDIT_LINE_H);
        item
    }

    fn top_row(item: &FileItem) -> f32 {
        item.scroll_y / EDIT_LINE_H
    }

    #[test]
    fn moving_down_keeps_a_margin_below_the_cursor() {
        let mut item = item(100, 10);
        for _ in 0..6 {
            item.input_key(EditKey::Down, false);
        }
        assert_eq!(top_row(&item), 0.0);
        item.input_key(EditKey::Down, false);
        assert_eq!(top_row(&item), 1.0);
    }

    #[test]
    fn page_down_moves_one_row_short_of_a_page() {
        let mut item = item(100, 10);
        item.input_key(EditKey::PageDown, false);
        assert_eq!(item.buffer.as_ref().unwrap().line_col().0, 9);
    }

    #[test]
    fn last_row_can_scroll_to_the_top() {
        let mut item = item(20, 10);
        item.scroll_by(-1000.0 * EDIT_LINE_H);
        assert_eq!(top_row(&item), 20.0);
    }

    #[test]
    fn view_stays_on_the_same_text_when_lines_are_inserted_above() {
        let mut item = item(100, 10);
        item.scroll_by(-40.0 * EDIT_LINE_H);
        if let Some(b) = item.buffer.as_mut() {
            b.place_cursor(0);
            b.insert_text("new\nnew\n");
        }
        item.set_body_height(10.0 * EDIT_LINE_H);
        assert_eq!(top_row(&item), 42.0);
    }
}

#[cfg(test)]
mod wrap_tests {
    use super::*;

    /// A soft-wrapping item whose text area fits `columns` columns.
    fn wrapped(text: &str, columns: usize) -> FileItem {
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.md", Some(text.into()));
        let em = char_advance();
        let body_w =
            (columns as f32 + 2.0) * em + gutter_width(item.line_count()) + SCROLLBAR_WIDTH;
        item.set_body_width(body_w);
        item.set_body_height(10.0 * EDIT_LINE_H);
        item
    }

    fn row_texts(item: &FileItem) -> Vec<String> {
        let b = item.buffer.as_ref().unwrap();
        item.rows
            .as_ref()
            .unwrap()
            .iter()
            .map(|r| {
                let line: String = b.rope.line(r.line).chars().collect();
                let line = line.trim_end_matches('\n');
                format!(
                    "{}{}",
                    " ".repeat(r.indent),
                    &line[r.start..r.end.min(line.len())]
                )
            })
            .collect()
    }

    #[test]
    fn long_lines_wrap_with_the_line_indent() {
        let item = wrapped("  alpha beta gamma\nx\n", 10);
        assert_eq!(
            row_texts(&item),
            vec!["  alpha ", "  beta ", "  gamma", "x", ""]
        );
    }

    #[test]
    fn down_moves_through_wrapped_rows_of_one_line() {
        let mut item = wrapped("alpha beta gamma\nx\n", 8);
        item.input_key(EditKey::Down, false);
        let b = item.buffer.as_ref().unwrap();
        assert_eq!(b.line_col(), (0, "alpha ".len()));
        assert_eq!(item.position(b.cursor()).0, 1);
    }

    #[test]
    fn end_stops_before_the_soft_break_first() {
        let mut item = wrapped("alpha beta gamma\n", 8);
        item.input_key(EditKey::End, false);
        assert_eq!(item.buffer.as_ref().unwrap().cursor(), "alpha".len());
        item.input_key(EditKey::End, false);
        assert_eq!(
            item.buffer.as_ref().unwrap().cursor(),
            "alpha beta gamma".len()
        );
    }

    #[test]
    fn toggling_soft_wrap_restores_one_row_per_line() {
        let mut item = wrapped("alpha beta gamma\n", 8);
        assert_eq!(item.disp_count(), 4);
        item.input_key(EditKey::ToggleSoftWrap, false);
        assert_eq!(item.disp_count(), 2);
    }
}

#[cfg(test)]
mod line_command_tests {
    use super::*;

    #[test]
    fn moving_a_folded_block_keeps_it_folded() {
        let text = "x\nfn a() {\n    y;\n}\n";
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.txt", Some(text.into()));
        item.set_body_height(10.0 * EDIT_LINE_H);
        item.do_toggle_fold(1);
        item.ensure_visible();
        if let Some(b) = item.buffer.as_mut() {
            b.place_cursor(2);
        }
        item.input_key(EditKey::MoveLineUp, false);
        let b = item.buffer.as_ref().unwrap();
        assert_eq!(b.text(), "fn a() {\n    y;\n}\nx\n");
        assert!(item.is_folded(0));
        assert_eq!(item.disp_count(), 3);
    }
}

#[cfg(test)]
mod indent_guide_tests {
    use super::*;

    #[test]
    fn enclosing_indent_finds_the_block_around_the_cursor() {
        let b = EditorBuffer::from_text("fn a() {\n    if x {\n        y;\n    }\n    z;\n}\n");
        assert_eq!(FileItem::enclosing_indent(&b, 2), Some((1, 2, 4)));
        assert_eq!(FileItem::enclosing_indent(&b, 4), Some((0, 4, 0)));
        assert_eq!(FileItem::enclosing_indent(&b, 1), Some((1, 2, 4)));
    }
}

#[cfg(test)]
mod search_bar_tests {
    use super::*;
    use editor::search::Direction;
    use workspace::search_bar::{SearchBar, SearchClick, SearchField};

    fn item(text: &str) -> FileItem {
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.txt", Some(text.into()));
        item.set_body_height(10.0 * EDIT_LINE_H);
        item
    }

    #[test]
    fn deploy_seeds_from_the_word_and_selects_the_nearest_match() {
        let mut item = item("foo bar foo\nbar");
        item.buffer.as_mut().unwrap().place_cursor(5);
        let mut bar = SearchBar::default();
        bar.deploy(&mut item, false);
        assert_eq!(bar.query.text(), "bar");
        assert_eq!(bar.matches, vec![4..7, 12..15]);
        assert_eq!(bar.active_match, Some(0));
        bar.select_match(&mut item, Direction::Next);
        assert_eq!(bar.active_match, Some(1));
        assert_eq!(
            item.buffer.as_ref().unwrap().selected_text().as_deref(),
            Some("bar")
        );
        bar.select_match(&mut item, Direction::Next);
        assert_eq!(bar.active_match, Some(0));
    }

    #[test]
    fn typing_a_query_searches_and_replace_all_is_one_undo() {
        let mut item = item("a1 a2 a3");
        let mut bar = SearchBar::default();
        bar.deploy(&mut item, true);
        bar.focus = Some(SearchField::Query);
        bar.input(&mut item, "a");
        assert_eq!(bar.matches.len(), 3);
        bar.focus = Some(SearchField::Replacement);
        bar.input(&mut item, "b");
        bar.replace_all(&mut item);
        assert_eq!(item.buffer.as_ref().unwrap().text(), "b1 b2 b3");
        assert!(bar.matches.is_empty());
        item.buffer.as_mut().unwrap().undo();
        assert_eq!(item.buffer.as_ref().unwrap().text(), "a1 a2 a3");
    }

    #[test]
    fn invalid_regex_reports_an_error() {
        let mut item = item("x");
        let mut bar = SearchBar::default();
        bar.deploy(&mut item, false);
        bar.toggle_option(&mut item, SearchClick::Regex);
        bar.input(&mut item, "(");
        assert!(bar.error.is_some());
        assert!(bar.matches.is_empty());
    }
}

#[cfg(test)]
mod go_to_line_tests {
    use super::*;

    fn view_with(text: &str) -> FilesView {
        let mut view = FilesView::scanned(PathBuf::from("/nonexistent"));
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.txt", Some(text.into()));
        item.set_body_height(4.0 * EDIT_LINE_H);
        if let Member::Leaf(pane) = &mut view.panes.group {
            pane.open.push(Box::new(item));
            pane.active = Some(0);
        }
        view
    }

    fn cursor(view: &mut FilesView) -> (usize, usize) {
        let path = view.panes.active.clone();
        let item = view.go_to_line_item(&path).unwrap();
        item.buffer.as_ref().unwrap().line_col()
    }

    #[test]
    fn confirm_moves_the_caret_and_cancel_restores_scroll() {
        let text: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let mut view = view_with(&text);
        view.editor_key(EditKey::ToggleGoToLine, false);
        view.editor_text("20:3");
        let path = view.panes.active.clone();
        assert_eq!(
            view.go_to_line_item(&path).unwrap().highlighted_rows,
            Some((19, 19))
        );
        view.editor_key(EditKey::Enter, false);
        assert!(view.go_to_line.is_none());
        assert_eq!(cursor(&mut view), (19, 2));

        view.editor_key(EditKey::ToggleGoToLine, false);
        let before = view.go_to_line_item(&path).unwrap().scroll_position();
        view.editor_text("1");
        view.editor_key(EditKey::Escape, false);
        let item = view.go_to_line_item(&path).unwrap();
        assert_eq!(item.scroll_position(), before);
        assert_eq!(item.highlighted_rows, None);
        assert_eq!(cursor(&mut view), (19, 2));
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    fn view_with(text: &str) -> FilesView {
        let mut view = FilesView::scanned(PathBuf::from("/nonexistent"));
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.txt", Some(text.into()));
        item.set_body_height(4.0 * EDIT_LINE_H);
        if let Member::Leaf(pane) = &mut view.panes.group {
            pane.open.push(Box::new(item));
            pane.active = Some(0);
        }
        view
    }

    fn line(view: &mut FilesView) -> usize {
        let path = view.panes.active.clone();
        view.go_to_line_item(&path)
            .unwrap()
            .buffer
            .as_ref()
            .unwrap()
            .line_col()
            .0
    }

    #[test]
    fn far_jumps_are_recorded_and_walked_back_and_forth() {
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let mut view = view_with(&text);
        view.editor_key(EditKey::Down, false);
        view.editor_key(EditKey::ToggleGoToLine, false);
        view.editor_text("50");
        view.editor_key(EditKey::Enter, false);
        view.editor_key(EditKey::ToggleGoToLine, false);
        view.editor_text("80");
        view.editor_key(EditKey::Enter, false);
        assert_eq!(line(&mut view), 79);
        view.editor_key(EditKey::GoBack, false);
        assert_eq!(line(&mut view), 49);
        view.editor_key(EditKey::GoBack, false);
        assert_eq!(line(&mut view), 1);
        view.editor_key(EditKey::GoForward, false);
        assert_eq!(line(&mut view), 49);
        view.editor_key(EditKey::GoForward, false);
        assert_eq!(line(&mut view), 79);
    }

    #[test]
    fn small_moves_are_not_recorded() {
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let mut view = view_with(&text);
        for _ in 0..5 {
            view.editor_key(EditKey::Down, false);
        }
        if let Member::Leaf(pane) = &view.panes.group {
            assert!(pane.back.is_empty());
        }
    }
}

#[cfg(test)]
mod cursor_status_tests {
    use super::*;

    #[test]
    fn describes_the_caret_and_selection() {
        let mut item = FileItem::new(
            PathBuf::from("/nonexistent"),
            "t.txt",
            Some("abc\ndef\nghi".into()),
        );
        let b = item.buffer.as_mut().unwrap();
        b.place_cursor(5);
        assert_eq!(item.cursor_status_text().as_deref(), Some("2:2"));
        let b = item.buffer.as_mut().unwrap();
        b.place_cursor(1);
        b.extend_cursor(9);
        assert_eq!(
            item.cursor_status_text().as_deref(),
            Some("3:2 (3 lines, 8 characters)")
        );
        let b = item.buffer.as_mut().unwrap();
        b.place_cursor(0);
        b.add_cursor(4);
        assert_eq!(
            item.cursor_status_text().as_deref(),
            Some("2:1 (2 selections)")
        );
    }
}

#[cfg(test)]
mod command_palette_tests {
    use super::*;

    fn view_with(text: &str) -> FilesView {
        let mut view = FilesView::scanned(PathBuf::from("/nonexistent"));
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.txt", Some(text.into()));
        item.set_body_height(4.0 * EDIT_LINE_H);
        if let Member::Leaf(pane) = &mut view.panes.group {
            pane.open.push(Box::new(item));
            pane.active = Some(0);
        }
        view
    }

    fn text(view: &mut FilesView) -> String {
        let path = view.panes.active.clone();
        let item = view.go_to_line_item(&path).unwrap();
        item.buffer.as_ref().unwrap().text()
    }

    #[test]
    fn runs_transforms_and_editor_commands() {
        let mut view = view_with("hello world\nb\na\n");
        view.editor_key(EditKey::ToggleCommandPalette, false);
        view.editor_text("convert to upper case");
        view.editor_key(EditKey::Enter, false);
        assert!(view.palette.is_none());
        assert_eq!(text(&mut view), "HELLO world\nb\na\n");

        view.editor_key(EditKey::SelectAll, false);
        view.editor_key(EditKey::ToggleCommandPalette, false);
        view.editor_text("sort lines case sensitive");
        view.editor_key(EditKey::Enter, false);
        assert_eq!(text(&mut view), "HELLO world\na\nb\n");

        view.editor_key(EditKey::ToggleCommandPalette, false);
        view.editor_text("go to line");
        view.editor_key(EditKey::Enter, false);
        assert!(view.go_to_line.is_some());
    }

    #[test]
    fn escape_and_outside_clicks_close_without_running() {
        let mut view = view_with("abc");
        view.editor_key(EditKey::ToggleCommandPalette, false);
        view.editor_text("upper");
        view.editor_key(EditKey::Escape, false);
        assert!(view.palette.is_none());
        view.editor_key(EditKey::ToggleCommandPalette, false);
        assert!(view.modal((1200.0, 800.0)).is_some());
        view.dismiss_modal();
        assert!(view.modal((1200.0, 800.0)).is_none());
        assert_eq!(text(&mut view), "abc");
    }
}

#[cfg(test)]
mod outline_tests {
    use super::*;

    fn rust_view(text: &str) -> FilesView {
        let mut view = FilesView::scanned(PathBuf::from("/nonexistent"));
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "t.rs", Some(text.into()));
        item.set_body_height(4.0 * EDIT_LINE_H);
        item.refresh();
        while item.syntax.as_ref().is_some_and(Syntax::is_parsing) {
            std::thread::sleep(std::time::Duration::from_millis(1));
            item.refresh();
        }
        if let Member::Leaf(pane) = &mut view.panes.group {
            pane.open.push(Box::new(item));
            pane.active = Some(0);
        }
        view
    }

    fn caret_line(view: &mut FilesView) -> usize {
        let path = view.panes.active.clone();
        let item = view.go_to_line_item(&path).unwrap();
        item.buffer.as_ref().unwrap().line_col().0
    }

    #[test]
    fn typing_a_name_previews_and_enter_jumps_to_it() {
        let filler: String = (0..40).map(|i| format!("// line {i}\n")).collect();
        let mut view = rust_view(&format!("fn alpha() {{}}\n{filler}fn omega() {{}}\n"));
        view.editor_key(EditKey::ToggleOutline, false);
        assert!(view.modal((1200.0, 800.0)).is_some());
        view.editor_text("omega");
        let path = view.panes.active.clone();
        assert_eq!(
            view.go_to_line_item(&path).unwrap().highlighted_rows,
            Some((41, 41))
        );
        view.editor_key(EditKey::Enter, false);
        assert!(view.outline.is_none());
        assert_eq!(caret_line(&mut view), 41);
    }

    #[test]
    fn preview_toggles_and_shows_the_symbol_centered() {
        let filler: String = (0..40).map(|i| format!("// line {i}\n")).collect();
        let mut view = rust_view(&format!("fn alpha() {{}}\n{filler}fn omega() {{}}\n"));
        view.editor_key(EditKey::ToggleOutline, false);
        view.editor_text("omega");
        let viewport = (1200.0 * ui::ui_text_scale(), 800.0 * ui::ui_text_scale());
        assert_eq!(
            view.modal(viewport).map(|m| m.width),
            Some(outline_view::WIDTH)
        );
        view.editor_key(EditKey::TogglePickerPreview, false);
        assert_eq!(view.modal(viewport).map(|m| m.width.round()), Some(720.0));
        assert_eq!(view.outline_preview, outline_view::PreviewLayout::Right);

        let path = view.panes.active.clone();
        let symbol = view
            .outline
            .as_ref()
            .and_then(|(_, v)| v.selected_symbol().cloned())
            .unwrap();
        let item = view.go_to_line_item(&path).unwrap();
        let content = item.preview_content(&symbol, 9, 80).unwrap();
        let middle = &content.rows[4];
        assert_eq!(middle.number, 42);
        assert!(middle.in_symbol);
        assert_eq!(middle.name_columns, Some(3..8));
        assert!(!content.rows[0].in_symbol);

        view.editor_key(EditKey::TogglePickerPreview, false);
        assert_eq!(view.outline_preview, outline_view::PreviewLayout::Hidden);
    }

    #[test]
    fn hovering_a_row_selects_it_and_reveals_the_scrollbar() {
        let mut view = rust_view("fn a() {}\nfn b() {}\n");
        view.editor_key(EditKey::ToggleOutline, false);
        let row = OUTLINE_BASE + outline_view::OutlineClick::Row(1).offset();
        assert!(view.set_hover(Some(row)));
        let (_, outline) = view.outline.as_ref().unwrap();
        assert_eq!(outline.selected, 1);
        assert!(outline.scrollbar.is_animating());
        view.editor_key(EditKey::Up, false);
        assert!(view.set_hover(Some(row)));
        assert_eq!(view.outline.as_ref().unwrap().1.selected, 0);
        view.set_hover(None);
        assert_eq!(view.outline.as_ref().unwrap().1.hovered, None);
        assert_eq!(view.outline.as_ref().unwrap().1.selected, 0);
    }

    #[test]
    fn hovering_a_palette_row_selects_it() {
        let mut view = rust_view("fn a() {}\n");
        view.editor_key(EditKey::ToggleCommandPalette, false);
        let row = PALETTE_BASE + command_palette::PaletteClick::Row(2).offset();
        view.set_hover(Some(row));
        assert_eq!(view.palette.as_ref().unwrap().1.selected, 2);
    }

    #[test]
    fn escape_restores_the_scroll() {
        let filler: String = (0..40).map(|i| format!("// line {i}\n")).collect();
        let mut view = rust_view(&format!("fn alpha() {{}}\n{filler}fn omega() {{}}\n"));
        let path = view.panes.active.clone();
        let before = view.go_to_line_item(&path).unwrap().scroll_position();
        view.editor_key(EditKey::ToggleOutline, false);
        view.editor_text("omega");
        view.editor_key(EditKey::Escape, false);
        let item = view.go_to_line_item(&path).unwrap();
        assert_eq!(item.scroll_position(), before);
        assert_eq!(item.highlighted_rows, None);
        assert_eq!(caret_line(&mut view), 0);
    }
}

#[cfg(test)]
mod git_gutter_tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn settle(item: &mut FileItem) {
        for _ in 0..2000 {
            if let Some(b) = item.buffer.as_ref() {
                item.git.poll(&b.rope, b.version());
            }
            if !item.git.is_busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn uncommitted_changes_show_as_gutter_strips() {
        let root = std::env::temp_dir().join(format!("pomelo-gutter-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        assert!(git(&["init", "-q"]));
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        assert!(git(&["add", "a.txt"]));
        assert!(git(&["commit", "-q", "-m", "init"]));

        let mut item = FileItem::new(root.clone(), "a.txt", Some("one\nTWO\nthree\n".into()));
        item.set_body_height(10.0 * EDIT_LINE_H);
        settle(&mut item);
        let kinds: Vec<(Range<usize>, git::HunkKind)> = item
            .git
            .hunks()
            .iter()
            .map(|h| (h.rows.clone(), h.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (1..2, git::HunkKind::Modified),
                (3..3, git::HunkKind::Deleted)
            ]
        );
        let content = Rect::new(0.0, 0.0, 400.0, 10.0 * EDIT_LINE_H, Rgba::TRANSPARENT);
        let strips = item.diff_hunk_rects(content, 0, 10);
        assert_eq!(strips.len(), 2);
        assert_eq!(strips[0].y, EDIT_LINE_H);
        assert_eq!(strips[0].w, (0.275 * EDIT_LINE_H).floor());
        assert!(strips[1].x < 0.0);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod hunk_action_tests {
    use super::*;
    use std::process::{Command, Stdio};

    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str, committed: &str) -> Self {
            let root = std::env::temp_dir().join(format!("pomelo-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let repo = Repo(root);
            assert!(repo.git(&["init", "-q"]).is_some());
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "user.name", "test"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            std::fs::write(repo.0.join("a.txt"), committed).unwrap();
            repo.git(&["add", "a.txt"]);
            repo.git(&["commit", "-q", "-m", "init"]);
            repo
        }

        fn git(&self, args: &[&str]) -> Option<String> {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .stderr(Stdio::null())
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).to_string())
        }

        fn open(&self, text: &str) -> FileItem {
            std::fs::write(self.0.join("a.txt"), text).unwrap();
            let mut item = FileItem::new(self.0.clone(), "a.txt", Some(text.into()));
            item.set_body_height(10.0 * EDIT_LINE_H);
            settle(&mut item);
            item
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn settle(item: &mut FileItem) {
        for _ in 0..3000 {
            if let Some(b) = item.buffer.as_ref() {
                item.git.poll(&b.rope, b.version());
            }
            if !item.git.is_busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn caret_line(item: &FileItem) -> usize {
        item.buffer.as_ref().unwrap().line_col().0
    }

    #[test]
    fn a_branch_diff_shows_every_change_against_the_start_of_the_branch() {
        let dir = std::env::temp_dir().join(format!("pomelo-branch-diff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a\nB\nc\nd\n").unwrap();
        let mut item = FileItem::branch_diff(
            dir.clone(),
            "a.txt",
            Some("a\nB\nc\nd\n".into()),
            Some("a\nb\nc\n".into()),
        );
        item.set_body_height(10.0 * EDIT_LINE_H);
        assert_eq!(item.id().as_deref(), Some("diff:a.txt"));
        assert_eq!(item.title(), "a.txt (diff)");
        assert!(
            item.serialize().is_none(),
            "a diff is not reopened as a plain file"
        );
        let settle_expanded = |item: &mut FileItem, rows: usize| {
            for _ in 0..3000 {
                item.gutter(0);
                item.ensure_visible();
                if item.disp_count() == rows {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("expected {rows} rows, have {}", item.disp_count());
        };
        // a, the removed "b", B, c, d and the empty last line: every change opens without asking.
        settle_expanded(&mut item, 6);
        assert_eq!(item.row(1).and_then(|r| r.deleted), Some(1));

        item.buffer.as_mut().unwrap().place_cursor(0);
        item.input_text("x\n");
        settle_expanded(&mut item, 7);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A branch diff of `text` against `base`, laid out wide enough for two columns once its hunks are in.
    fn split_diff(text: &str, base: &str) -> FileItem {
        let mut item = FileItem::branch_diff(
            std::env::temp_dir(),
            "split-test.txt",
            Some(text.into()),
            Some(base.into()),
        );
        item.set_body_height(40.0 * EDIT_LINE_H);
        let wide = 400.0 * char_advance();
        for _ in 0..3000 {
            let companion = item.companion_width(wide);
            item.set_body_width(wide - companion);
            item.gutter(0);
            item.ensure_visible();
            if item.split_active && !item.git.hunks().is_empty() && !item.git.is_busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        item.rows = None;
        item.ensure_visible();
        item
    }

    /// Each display row as (new line or "-" for a blank, old line or "-").
    fn split_layout(item: &FileItem) -> Vec<(String, String)> {
        let rows = item.rows.as_ref().expect("rows");
        rows.iter()
            .zip(&item.split_left)
            .map(|(row, left)| {
                let new = if row.spacer {
                    "-".to_string()
                } else {
                    row.line.to_string()
                };
                let old = left.base.map_or("-".to_string(), |line| line.to_string());
                (new, old)
            })
            .collect()
    }

    #[test]
    fn a_split_diff_keeps_both_sides_level() {
        // Old b and c became one X; f and g were added; e was removed.
        let item = split_diff("a\nX\nd\nf\ng\n", "a\nb\nc\nd\ne\n");
        let pair = |new: &str, old: &str| (new.to_string(), old.to_string());
        assert_eq!(
            split_layout(&item),
            [
                pair("0", "0"),
                pair("1", "1"),
                pair("-", "2"),
                pair("2", "3"),
                pair("3", "4"),
                pair("4", "-"),
                pair("5", "5"),
            ]
        );
        assert!(item.split_left[2].changed && !item.split_left[0].changed);
        let painted = {
            let mut item = item;
            item.paint_old_side(Rect::new(0.0, 0.0, 300.0, 400.0, Rgba::TRANSPARENT))
        };
        let texts: Vec<&str> = painted
            .texts
            .iter()
            .map(|text| text.text.as_str())
            .collect();
        assert!(
            texts.contains(&"b") && texts.contains(&"c") && texts.contains(&"e"),
            "{texts:?}"
        );
    }

    #[test]
    fn a_narrow_or_unsplit_diff_shows_one_column_with_removed_lines_inline() {
        let mut item = split_diff("a\nX\nc\n", "a\nb\nc\n");
        assert!(item.split_active);
        assert_eq!(
            item.companion_width(40.0 * char_advance()),
            0.0,
            "too narrow for two"
        );
        assert!(!item.split_active);
        item.ensure_visible();
        assert_eq!(
            item.row(1).and_then(|row| row.deleted),
            Some(1),
            "back to inline"
        );

        let wide = 400.0 * char_advance();
        assert!(item.companion_width(wide) > 0.0);
        item.input_key(EditKey::ToggleSplitDiff, false);
        assert_eq!(
            item.companion_width(wide),
            0.0,
            "the user asked for one column"
        );
        item.input_key(EditKey::ToggleSplitDiff, false);
        assert!(item.companion_width(wide) > 0.0);
    }

    #[test]
    fn expanding_shows_removed_lines_the_caret_steps_over() {
        let repo = Repo::new("hunk-expand", "a\nb\nc\n");
        let mut item = repo.open("a\nB\nc\n");
        item.buffer.as_mut().unwrap().place_cursor(2);
        item.input_key(EditKey::ToggleSelectedDiffHunks, false);
        item.ensure_visible();
        assert_eq!(item.disp_count(), 5);
        assert_eq!(item.row(1).and_then(|r| r.deleted), Some(1));
        assert_eq!(item.base_line_segments(1, &syntax_theme())[0].0, "b");
        assert_eq!(item.disp_of(1), 2);

        item.buffer.as_mut().unwrap().place_cursor(0);
        item.input_key(EditKey::Down, false);
        assert_eq!(caret_line(&item), 1);
        item.input_key(EditKey::Up, false);
        assert_eq!(caret_line(&item), 0);

        let content = Rect::new(0.0, 0.0, 400.0, 10.0 * EDIT_LINE_H, Rgba::TRANSPARENT);
        let bands = item.expanded_hunk_rects(content, 0, 10);
        assert_eq!(bands.len(), 2);
        assert_eq!(bands[0].y, EDIT_LINE_H);
        assert_eq!(bands[1].y, 2.0 * EDIT_LINE_H);

        item.input_key(EditKey::Escape, false);
        item.ensure_visible();
        assert_eq!(item.disp_count(), 4);

        item.toggle_diff_hunk(1);
        item.ensure_visible();
        assert_eq!(item.disp_count(), 5);
        item.toggle_diff_hunk(1);
        item.ensure_visible();
        assert_eq!(item.disp_count(), 4);
    }

    #[test]
    fn a_pure_deletion_expands_above_the_following_line() {
        let repo = Repo::new("hunk-expand-del", "a\nb\nc\n");
        let mut item = repo.open("a\nc\n");
        item.input_key(EditKey::ExpandAllDiffHunks, false);
        item.ensure_visible();
        assert_eq!(item.disp_count(), 4);
        assert_eq!(item.row(1).and_then(|r| r.deleted), Some(1));
        assert_eq!(item.row(2).map(|r| (r.line, r.deleted)), Some((1, None)));
    }

    #[test]
    fn caret_line_shows_its_blame_while_focused() {
        let repo = Repo::new("blame", "one\n\nthree\n");
        let mut item = repo.open("one\n\nthree\n");
        assert_eq!(item.inline_blame_row(), None);
        item.focused = true;
        item.buffer.as_mut().unwrap().place_cursor(0);
        assert_eq!(item.inline_blame_row(), Some(0));
        assert_eq!(item.inline_blame_text().as_deref(), Some("test, Just now"));
        item.buffer.as_mut().unwrap().place_cursor(4);
        assert_eq!(item.inline_blame_row(), None);
    }

    #[test]
    fn next_and_previous_hunk_wrap_around() {
        let repo = Repo::new("hunk-nav", "a\nb\nc\nd\ne\nf\n");
        let mut item = repo.open("a\nB\nc\nd\nE\nf\n");
        item.input_key(EditKey::GoToHunk, false);
        assert_eq!(caret_line(&item), 1);
        item.input_key(EditKey::GoToHunk, false);
        assert_eq!(caret_line(&item), 4);
        item.input_key(EditKey::GoToHunk, false);
        assert_eq!(caret_line(&item), 1);
        item.input_key(EditKey::GoToPreviousHunk, false);
        assert_eq!(caret_line(&item), 4);
    }

    #[test]
    fn restore_puts_the_committed_lines_back_and_undoes() {
        let repo = Repo::new("hunk-restore", "a\nb\nc\n");
        let mut item = repo.open("a\nB\nc\nnew\n");
        item.buffer.as_mut().unwrap().place_cursor(2);
        item.input_key(EditKey::GitRestore, false);
        assert_eq!(item.buffer.as_ref().unwrap().text(), "a\nb\nc\nnew\n");
        item.input_key(EditKey::Undo, false);
        assert_eq!(item.buffer.as_ref().unwrap().text(), "a\nB\nc\nnew\n");
    }

    #[test]
    fn staging_writes_only_the_hunk_under_the_caret() {
        let repo = Repo::new("hunk-stage", "a\nb\nc\nd\ne\n");
        let mut item = repo.open("a\nB\nc\nd\nE\n");
        item.buffer.as_mut().unwrap().place_cursor(2);
        item.input_key(EditKey::StageAndNext, false);
        settle(&mut item);
        assert_eq!(
            repo.git(&["show", ":a.txt"]).as_deref(),
            Some("a\nB\nc\nd\ne\n")
        );
        assert_eq!(caret_line(&item), 4);
        let staged: Vec<bool> = item.git.hunks().iter().map(|h| h.staged).collect();
        assert_eq!(staged, vec![true, false]);

        item.buffer.as_mut().unwrap().place_cursor(2);
        item.input_key(EditKey::ToggleStaged, false);
        settle(&mut item);
        assert_eq!(
            repo.git(&["show", ":a.txt"]).as_deref(),
            Some("a\nb\nc\nd\ne\n")
        );
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position};

    fn item(text: &str) -> FileItem {
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), "a.rs", Some(text.into()));
        item.set_body_height(10.0 * EDIT_LINE_H);
        item
    }

    fn diagnostic(line: u32, start: u32, end: u32, severity: DiagnosticSeverity) -> Diagnostic {
        Diagnostic {
            range: lsp::lsp_types::Range::new(Position::new(line, start), Position::new(line, end)),
            severity: Some(severity),
            message: "problem".into(),
            ..Default::default()
        }
    }

    #[test]
    fn diagnostics_for_an_older_version_follow_the_edits_since() {
        let mut item = item("let a = b;\nlet c = d;\n");
        let b = item.buffer.as_ref().unwrap();
        let synced = lsp::SyncedText {
            lsp_version: 0,
            buffer_version: b.version(),
            rope: b.rope.clone(),
        };
        item.buffer.as_mut().unwrap().place_cursor(0);
        item.input_text("// x\n");
        item.set_diagnostics(&lsp::DiagnosticsUpdate {
            path: PathBuf::from("/nonexistent/a.rs"),
            diagnostics: vec![diagnostic(1, 8, 9, DiagnosticSeverity::ERROR)],
            synced: Some(synced),
        });
        let text = item.buffer.as_ref().unwrap().text();
        let entry = &item.diagnostics[0];
        assert_eq!(&text[entry.range.clone()], "d");
        item.buffer.as_mut().unwrap().place_cursor(5);
        item.input_text("  ");
        item.refresh();
        let text = item.buffer.as_ref().unwrap().text();
        assert_eq!(&text[item.diagnostics[0].range.clone()], "d");
    }

    #[test]
    fn visible_diagnostics_draw_wavy_underlines_in_their_color() {
        let mut item = item("let a = b;\n");
        item.set_diagnostics(&lsp::DiagnosticsUpdate {
            path: PathBuf::from("/nonexistent/a.rs"),
            diagnostics: vec![
                diagnostic(0, 4, 5, DiagnosticSeverity::WARNING),
                diagnostic(0, 8, 9, DiagnosticSeverity::ERROR),
            ],
            synced: None,
        });
        let content = Rect::new(0.0, 0.0, 800.0, 240.0, Rgba::TRANSPARENT);
        let rects = item.diagnostic_rects(content, 0, 2);
        assert_eq!(rects.len(), 2);
        assert!(rects.iter().all(|r| r.radius < 0.0));
        assert_eq!(rects.last().unwrap().color, theme().error);
        assert!(
            rects[0].y > DIAGNOSTIC_UNDERLINE_TOP - 1.0 && rects[0].y + rects[0].h <= EDIT_LINE_H
        );
        assert!(item.diagnostic_rects(content, 1, 2).is_empty());
    }
}

#[cfg(test)]
mod snippet_tests {
    use super::*;

    fn item_with_snippets(name: &str, text: &str, json: &str) -> FileItem {
        let dir = std::env::temp_dir().join(format!(
            "pomelo-snippets-{}-{}",
            std::process::id(),
            name.replace('.', "-")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rust.json"), json).unwrap();
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), name, Some(text.into()));
        item.snippet_dir = Some(dir);
        item.set_body_height(10.0 * EDIT_LINE_H);
        let end = item.buffer.as_ref().unwrap().rope.len_chars();
        item.buffer.as_mut().unwrap().place_cursor(end);
        item
    }

    fn text(item: &FileItem) -> String {
        item.buffer.as_ref().unwrap().text()
    }

    fn selection(item: &FileItem) -> Range<usize> {
        let newest = item.buffer.as_ref().unwrap().newest();
        newest.start..newest.end
    }

    const FUNCTION: &str =
        r#"{"Function": {"prefix": "fn", "body": ["fn ${1:name}($2) {", "    $0", "}"]}}"#;

    #[test]
    fn two_chars_of_a_prefix_open_the_menu_and_enter_expands_indented() {
        let mut item = item_with_snippets("expand.rs", "mod a {\n    f", FUNCTION);
        item.input_text("n");
        let menu = item
            .completions
            .as_ref()
            .expect("snippet prefix opens the menu");
        assert_eq!(menu.selected_word(), Some("fn"));
        assert_eq!(
            menu.selected_completion().unwrap().detail.as_deref(),
            Some("Function")
        );
        item.input_key(EditKey::Enter, false);
        assert_eq!(text(&item), "mod a {\n    fn name() {\n        \n    }");
        assert_eq!(selection(&item), 15..19);
        item.input_text("run");
        item.input_key(EditKey::Tab, false);
        assert_eq!(selection(&item), 19..19);
        item.input_key(EditKey::Backtab, false);
        assert_eq!(selection(&item), 15..18);
        item.input_key(EditKey::Tab, false);
        item.input_key(EditKey::Tab, false);
        assert_eq!(text(&item), "mod a {\n    fn run() {\n        \n    }");
        assert_eq!(selection(&item), 31..31);
        assert!(!item.buffer.as_ref().unwrap().in_snippet());
    }

    #[test]
    fn choices_open_a_menu_at_their_stop() {
        let json = r#"{"Int": {"prefix": "int", "body": "let x: ${1|i32,u64|} = $2;"}}"#;
        let mut item = item_with_snippets("choice.rs", "in", json);
        item.input_text("t");
        item.input_key(EditKey::Enter, false);
        assert_eq!(text(&item), "let x: i32 = ;");
        let menu = item.completions.as_ref().expect("choices menu");
        assert!(menu.is_choices());
        item.input_key(EditKey::Down, false);
        item.input_key(EditKey::Enter, false);
        assert_eq!(text(&item), "let x: u64 = ;");
        item.input_key(EditKey::Tab, false);
        assert_eq!(selection(&item), 13..13);
    }

    #[test]
    fn escape_leaves_the_snippet_so_tab_indents_again() {
        let mut item = item_with_snippets("escape.rs", "f", FUNCTION);
        item.input_key(EditKey::ShowCompletions, false);
        item.input_key(EditKey::Enter, false);
        assert!(item.buffer.as_ref().unwrap().in_snippet());
        item.input_key(EditKey::Escape, false);
        assert!(!item.buffer.as_ref().unwrap().in_snippet());
        let before = text(&item);
        item.input_key(EditKey::Tab, false);
        assert_ne!(text(&item), before);
    }

    #[test]
    fn one_char_or_a_non_matching_word_opens_nothing() {
        let mut item = item_with_snippets("none.rs", "", FUNCTION);
        item.input_text("f");
        assert!(item.completions.is_none());
        item.input_text("x");
        assert!(item.completions.is_none());
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    fn item(name: &str, text: &str) -> FileItem {
        let mut item = FileItem::new(PathBuf::from("/nonexistent"), name, Some(text.into()));
        item.set_body_height(10.0 * EDIT_LINE_H);
        let end = item.buffer.as_ref().unwrap().rope.len_chars();
        item.buffer.as_mut().unwrap().place_cursor(end);
        item
    }

    fn text(item: &FileItem) -> String {
        item.buffer.as_ref().unwrap().text()
    }

    #[test]
    fn typing_three_word_chars_opens_the_menu_and_enter_completes() {
        let mut item = item("a.rs", "hello help world\nhe");
        item.input_text("l");
        let menu = item
            .completions
            .as_ref()
            .expect("menu opens at three chars");
        let first = menu.selected_word().unwrap().to_string();
        assert!(first == "hello" || first == "help");
        item.input_key(EditKey::Down, false);
        let second = item
            .completions
            .as_ref()
            .unwrap()
            .selected_word()
            .unwrap()
            .to_string();
        assert_ne!(first, second);
        item.input_key(EditKey::Enter, false);
        assert!(item.completions.is_none());
        assert_eq!(text(&item), format!("hello help world\n{second}"));
        let b = item.buffer.as_ref().unwrap();
        assert_eq!(b.cursor(), b.rope.len_chars());
    }

    #[test]
    fn short_words_non_word_chars_and_escape_close_it() {
        let mut item = item("a.rs", "hello helpme\nh");
        item.input_text("e");
        assert!(item.completions.is_none());
        item.input_text("l");
        assert!(item.completions.is_some());
        item.input_key(EditKey::Backspace, false);
        assert!(item.completions.is_none());
        item.input_text("l");
        item.input_key(EditKey::Escape, false);
        assert!(item.completions.is_none());
        item.input_text("p");
        assert!(item.completions.is_some());
        item.input_text(" ");
        assert!(item.completions.is_none());
    }

    #[test]
    fn markdown_has_no_word_menu_but_it_can_be_forced() {
        let mut item = item("a.md", "hello help\nhel");
        item.input_text("p");
        assert!(item.completions.is_none());
        let mut item = super::completion_tests::item("a.rs", "hello help\nh");
        item.input_key(EditKey::ShowWordCompletions, false);
        assert!(item.completions.is_some());
    }

    #[test]
    fn menu_opens_below_the_caret_or_above_near_the_bottom() {
        let mut item = item("a.rs", "hello help\nhel");
        item.input_key(EditKey::ShowCompletions, false);
        let content = Rect::new(0.0, 0.0, 800.0, 10.0 * EDIT_LINE_H, Rgba::TRANSPARENT);
        let (_, _, y) = item.completion_menu_popover(content).unwrap();
        assert_eq!(y, 2.0 * EDIT_LINE_H);
        let lines: String = (0..9).map(|i| format!("help{i}\n")).collect();
        let mut item = super::completion_tests::item("a.rs", &format!("{lines}hel"));
        item.input_key(EditKey::ShowCompletions, false);
        let (_, _, y) = item.completion_menu_popover(content).unwrap();
        assert!(y < 9.0 * EDIT_LINE_H, "{y}");
    }
}

#[cfg(test)]
mod self_painted_item_tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Shell {
        keys: Rc<Cell<usize>>,
        exit: Rc<Cell<bool>>,
    }

    impl Item for Shell {
        fn id(&self) -> Option<String> {
            Some("shell".into())
        }
        fn title(&self) -> String {
            "shell".into()
        }
        fn render(&mut self) -> Node {
            div().into()
        }
        fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
            let mut painted = ui::Painted::default();
            painted.rects.push(body);
            Some(painted)
        }
        fn wants_keystrokes(&self) -> bool {
            true
        }
        fn keystroke(&mut self, _keystroke: &terminal::Keystroke) -> workspace::TerminalKeyOutcome {
            self.keys.set(self.keys.get() + 1);
            workspace::TerminalKeyOutcome::Handled
        }
        fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> workspace::ItemTick {
            workspace::ItemTick {
                close: self.exit.get(),
                ..workspace::ItemTick::default()
            }
        }
    }

    #[test]
    fn center_items_paint_take_keys_and_close_themselves() {
        let mut view = FilesView::scanned(PathBuf::from("/nonexistent"));
        assert!(view.editor_key(EditKey::NewCenterTerminal, false));
        assert_eq!(
            view.take_request(),
            Some(workspace::ViewRequest::NewCenterTerminal)
        );
        let keys = Rc::new(Cell::new(0));
        let exit = Rc::new(Cell::new(false));
        view.add_center_item(Box::new(Shell {
            keys: keys.clone(),
            exit: exit.clone(),
        }));
        assert!(view.active_wants_keystrokes());
        assert_eq!(
            view.item_keystroke(&terminal::Keystroke::parse("a")),
            workspace::TerminalKeyOutcome::Handled
        );
        assert_eq!(keys.get(), 1);
        let layout = view.editor_layout(Rect::new(0.0, 0.0, 600.0, 400.0, Rgba::TRANSPARENT));
        let painted = layout.panes[0].painted.as_ref().map(|(_, clip)| *clip);
        assert!(painted.is_some_and(|clip| clip.y > 0.0 && clip.h < 400.0));
        assert!(!view.tick_items(&|| None).changed);
        exit.set(true);
        assert!(view.tick_items(&|| None).changed);
        assert!(!view.active_wants_keystrokes());
    }
}

#[cfg(test)]
mod footer_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use workspace::{ItemFooter, RunRequest};

    #[derive(Default)]
    struct Recorded {
        runs: Vec<RunRequest>,
        texts: Vec<String>,
        run_button: bool,
    }

    struct Results(Rc<RefCell<Recorded>>);

    impl ItemFooter for Results {
        fn height(&mut self, _body_h: f32) -> f32 {
            100.0
        }
        fn paint(&mut self, _area: Rect) -> Option<ui::Painted> {
            None
        }
        fn pointer_down(&mut self, _x: f32, _y: f32, _click_count: u32) -> bool {
            self.0.borrow_mut().run_button = true;
            true
        }
        fn take_run(&mut self) -> Option<bool> {
            std::mem::take(&mut self.0.borrow_mut().run_button).then_some(true)
        }
        fn run(&mut self, request: RunRequest) {
            self.0.borrow_mut().runs.push(request);
        }
        fn text_changed(&mut self, text: &str) {
            self.0.borrow_mut().texts.push(text.to_string());
        }
    }

    fn console(text: &str) -> (Box<dyn Item>, Rc<RefCell<Recorded>>) {
        let recorded = Rc::new(RefCell::new(Recorded::default()));
        let item = scratch_editor(
            "console:1".into(),
            "query 1".into(),
            text,
            Box::new(Results(recorded.clone())),
        );
        (item, recorded)
    }

    #[test]
    fn cmd_enter_hands_the_footer_the_text_caret_and_selection() {
        let (mut item, recorded) = console("select 1;\nselect 2;");
        item.input_key(EditKey::DocumentEnd, false);
        item.input_key(EditKey::ReplaceAll, false);
        item.input_key(EditKey::Left, true);
        item.input_key(EditKey::ReplaceAll, true);
        let runs = &recorded.borrow().runs;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].caret, "select 1;\nselect 2;".len());
        assert_eq!(runs[0].selection, None);
        assert!(!runs[0].all);
        assert_eq!(runs[1].selection.as_deref(), Some(";"));
        assert!(runs[1].all);
    }

    #[test]
    fn a_scratch_tab_never_touches_disk_and_reports_edits() {
        let (mut item, recorded) = console("select 1;");
        assert_eq!(item.abs_path(), None);
        assert!(!item.is_dirty());
        item.input_text("x");
        item.tick(&|| None);
        assert_eq!(recorded.borrow().texts.len(), 1);
        assert!(!item.is_dirty());
    }

    #[test]
    fn a_press_on_the_footer_can_ask_for_a_run() {
        let (mut item, recorded) = console("select 1;");
        let body = Rect::new(0.0, 0.0, 400.0, 400.0, ui::Rgba::TRANSPARENT);
        let footer_h = item.footer_height(body.h);
        item.paint_footer(Rect::new(
            0.0,
            body.h - footer_h,
            400.0,
            footer_h,
            ui::Rgba::TRANSPARENT,
        ));
        assert!(!item.pointer_down(10.0, 50.0, 1, terminal::Modifiers::default()));
        assert!(item.pointer_down(10.0, 350.0, 1, terminal::Modifiers::default()));
        let runs = &recorded.borrow().runs;
        assert_eq!(runs.len(), 1);
        assert!(runs[0].all);
    }
}
