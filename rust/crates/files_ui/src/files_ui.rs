//! Files feature view — the file tree plus the selected file's contents (syntax-highlighted), rendered as a
//! `workspace::FunctionView`. Ported from the Swift `FilesPane`: a tree on the left (indent per depth, folders
//! expand/collapse) and the open file on the right. Logic (listing/reading) lives in the `files` crate; syntax
//! highlighting reuses the `editor` crate. The `workspace` toolkit stays free of this crate .

use std::collections::HashSet;
use std::path::PathBuf;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use editor::buffer::{Bias, ClipboardSelection, Deletion, DisplayRows, Motion, Selection};
use editor::fold::FoldMap;
use editor::wrap::Boundary;
use editor::{EditorBuffer, Lang, Syntax, Theme};
use files::FileNode;
use ui::{div, icon, label, material_icon, theme, IconKind, MaterialIcon, Node, Rect, Rgba};
use workspace::{
    ClipboardSlice, CopiedText, DividerAxis, DividerPlacement, EditKey, EditorLayout, FunctionView,
    Item, PaneBody, PanePlacement, FUNC_VIEW_BASE,
};

// Editor text metrics (design px). The body pads by `EDIT_PAD_*`; each source line is `EDIT_LINE_H` tall with
// `EDIT_FONT` mono text. Caret/selection geometry uses these plus the mono advance so it aligns with the glyphs.
const EDIT_FONT: f32 = 15.0; // matches the reference's default buffer font size
const EDIT_LINE_H: f32 = 24.0; // ~15 * 1.618 ("comfortable" line height), snapped to a whole pixel
const EDIT_PAD_X: f32 = 8.0;
const EDIT_PAD_Y: f32 = 8.0;
const CARET_W: f32 = 2.0;
const GUTTER_GAP: f32 = 12.0; // space between the line-number gutter and the first text column
const VERTICAL_SCROLL_MARGIN: f32 = 3.0;
const HORIZONTAL_SCROLL_MARGIN: f32 = 5.0;
/// With soft wrap off, lines still break past this many columns so pathological lines stay cheap to lay out.
const UNWRAPPED_MAX_COLUMNS: f32 = 512.0;
const FOLD_PILL_PAD: f32 = 4.0;
const FOLD_COL: f32 = 14.0; // fold-chevron column, between the line number and the text
const TAB_COLS: usize = 4; // indent-guide spacing: one guide per this many leading columns

/// The monospace advance of one column at the editor font (Lilex is monospace, so every column is this wide).
fn char_advance() -> f32 {
    ui::measure_text_width("M", EDIT_FONT, true, 400)
}

/// The gutter width for `line_count` lines: room for the widest line number, the fold chevron column, and a
/// trailing gap before the text.
fn gutter_width(line_count: usize) -> f32 {
    let digits = line_count.max(1).to_string().len().max(2);
    digits as f32 * char_advance() + FOLD_COL + GUTTER_GAP
}

const INDENT: f32 = 16.0; // per-depth indent (~20 in the design; trimmed for the narrower panel)
const ROW_H: f32 = 22.0;
const TAB_H: f32 = 32.0; // editor tab-bar height
const DIVIDER_LINE: f32 = 1.0; // the visible gap between panes (a 1px line fills it -- no white band)
const DIVIDER_GRAB: f32 = 5.0; // pointer grab width: a hit strip centered on the line (overlaps panes)
const MIN_PANE_W: f32 = 80.0; // min pane width for a horizontal split (the reference's HORIZONTAL_MIN_SIZE)
const MIN_PANE_H: f32 = 100.0; // min pane height for a vertical split (the reference's VERTICAL_MIN_SIZE)
const MAX_PANES: usize = 6; // ceiling on total leaf panes in the group
const DROP_EDGE: f32 = 0.2; // a tab dropped within this fraction of a pane's smaller side splits that edge

// Click-id ranges within the feature-view space (`FUNC_VIEW_BASE`). Tree rows use the low range; the editor's
// pane tabs + split/divider affordances use higher offsets. Tab ids are keyed by `pane_index * PANE_STRIDE +
// tab`, where `pane_index` is the pane's position in render order (rebuilt every frame). Checked high-to-low
// in `on_click`, so the ranges must not overlap.
const TAB_ACTIVATE_BASE: u64 = FUNC_VIEW_BASE + 1_000_000;
const TAB_CLOSE_BASE: u64 = FUNC_VIEW_BASE + 3_000_000;
const STICKY_BASE: u64 = FUNC_VIEW_BASE + 4_000_000;
const SPLIT_RIGHT_BASE: u64 = FUNC_VIEW_BASE + 5_000_000;
const SPLIT_DOWN_BASE: u64 = FUNC_VIEW_BASE + 6_000_000;
const NAV_BACK_BASE: u64 = FUNC_VIEW_BASE + 7_000_000;
const NAV_FWD_BASE: u64 = FUNC_VIEW_BASE + 8_000_000;
const FOLD_BASE: u64 = FUNC_VIEW_BASE + 10_000_000; // + pane*PANE_STRIDE + buffer_line
const DIVIDER_BASE: u64 = FUNC_VIEW_BASE + 12_000_000;
const PANE_STRIDE: u64 = 100_000;

/// The split axis of a pane group node. `Horizontal` lays panes left-to-right (a `row`), `Vertical` stacks
/// them top-to-bottom (a `col`).
#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// A split intent (toolbar button or a tab drop on a pane edge). Maps to an `Axis` plus whether the new pane
/// goes after (right/down) or before (left/up) the current one.
#[derive(Clone, Copy, PartialEq)]
enum SplitDir {
    Left,
    Right,
    Up,
    Down,
}

impl SplitDir {
    fn axis(self) -> Axis {
        match self {
            SplitDir::Left | SplitDir::Right => Axis::Horizontal,
            SplitDir::Up | SplitDir::Down => Axis::Vertical,
        }
    }
    fn after(self) -> bool {
        matches!(self, SplitDir::Right | SplitDir::Down)
    }
}

/// A node of the center pane group: either a leaf `Pane` or a `Split` of child members along one axis. This is
/// the recursive binary-ish tree that lets panes tile in any nesting (a row of panes, one of which is a column
/// of panes, ...), sized by per-child flex ratios.
enum Member {
    Leaf(Pane),
    Split(Split),
}

/// A split container: an axis, its child members, and a flex ratio per child. Flexes sum to `members.len()`, so
/// each child's extent along the axis is `container * flex[i] / members.len()` (equal flexes = equal sizes).
struct Split {
    axis: Axis,
    members: Vec<Member>,
    flexes: Vec<f32>,
}

/// A rebuilt-per-frame record of a rendered divider: which split owns it (path of child indices from the root),
/// the gap index within that split (between child `index` and `index + 1`), the split's axis, and the pixel
/// length available to its children along that axis (so a drag turns px into flex without re-walking the tree).
struct DividerRef {
    split_path: Vec<usize>,
    index: usize,
    axis: Axis,
    container: f32,
    /// The along-axis pixel origin of child `index` (its left/top edge), so a drag can position the boundary
    /// at the absolute cursor position (like the reference), not by accumulating deltas.
    child_start: f32,
}

/// An in-progress tab drag: the source pane (by stable id) and the tab index within it, plus the current drop
/// resolved each pointer move. `None` drop = the pointer isn't over a pane (dropping there cancels).
struct TabDrag {
    source: u64,
    index: usize,
    title: String,
    icon: MaterialIcon,
    drop: Option<TabDrop>,
}

/// Where a dragged tab would land: a target pane (by id) and either a split direction (dropped on an edge) or
/// `None` (dropped in the center = move the tab into that pane).
struct TabDrop {
    pane: u64,
    dir: Option<SplitDir>,
}

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineCommand {
    Delete,
    Duplicate { up: bool },
    Move { up: bool },
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
}

impl FileItem {
    fn new(root: PathBuf, path: &str, text: Option<String>) -> Self {
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let lang = Lang::from_ext(path.rsplit('.').next().unwrap_or(""));
        let buffer = text.map(|t| EditorBuffer::from_text(&t));
        let syntax = buffer.as_ref().and_then(|_| Syntax::new(lang));
        let saved_mtime = files::mtime(&root, path).ok();
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
        }
    }

    /// Bring dependents up to the buffer: the syntax tree, fold positions (carried through the edits since the
    /// last sync), and the widest line.
    fn refresh(&mut self) {
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
        let Some(b) = self.buffer.as_ref() else {
            return Vec::new();
        };
        if line >= b.rope.len_lines() {
            return Vec::new();
        }
        let start = b.rope.line_to_byte(line);
        let end = start + b.rope.line(line).len_bytes()
            - usize::from(b.line_len(line) < b.rope.line(line).len_chars());
        let runs = match self.syntax.as_ref() {
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
        let mut rows = Vec::with_capacity(lines.len());
        let mut line_rows = vec![0usize; line_count];
        let mut max_row_cols = 0usize;
        for (i, &line) in lines.iter().enumerate() {
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
            });
            let last_cols = if start == 0 {
                columns
            } else {
                indent + Self::display_index(b, line, b.line_len(line)).saturating_sub(start)
            };
            max_row_cols = max_row_cols.max(last_cols);
        }
        self.rows = Some(rows);
        self.line_rows = line_rows;
        self.max_row_cols = max_row_cols;
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

    fn set_scroll_y(&mut self, scroll_y: f32) {
        self.scroll_y = scroll_y.clamp(0.0, self.max_scroll());
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
        while self.row(row + 1).is_some_and(|next| next.line == line) {
            row += 1;
        }
        row
    }

    /// Whether `row` ends at a soft break (its line continues on the next row).
    fn soft_break_after(&self, row: usize) -> bool {
        match (self.row(row), self.row(row + 1)) {
            (Some(this), Some(next)) => next.line == this.line,
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
            .is_some_and(|next| next.line == line && next.start <= index)
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
        (self.body_w - 2.0 * EDIT_PAD_X - gutter_width(self.line_count())).max(0.0)
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

    /// The visual-column range `[start, end)` selected on a buffer line (merged across cursors), for drawing
    /// whitespace dots. `None` if the line has no selected text.
    fn sel_vis_range_for(&self, line: usize) -> Option<(usize, usize)> {
        let b = self.buffer.as_ref()?;
        let mut range: Option<(usize, usize)> = None;
        for s in b.selections() {
            let Some((a, e)) = s.range() else { continue };
            let (l0, c0) = b.line_col_of(a);
            let (l1, c1) = b.line_col_of(e);
            if line < l0 || line > l1 {
                continue;
            }
            let start = if line == l0 { c0 } else { 0 };
            let end = if line == l1 { c1 } else { b.line_len(line) };
            let sv = Self::vis_col(b, line, start);
            let ev = Self::vis_col(b, line, end);
            range = Some(match range {
                Some((lo, hi)) => (lo.min(sv), hi.max(ev)),
                None => (sv, ev),
            });
        }
        range
    }
}

impl Item for FileItem {
    fn id(&self) -> Option<String> {
        Some(self.path.clone())
    }

    fn title(&self) -> String {
        self.name.clone()
    }

    fn icon(&self) -> Option<MaterialIcon> {
        Some(file_icon(&self.name))
    }

    fn clone_on_split(&self) -> Option<Box<dyn Item>> {
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
                .px(EDIT_PAD_X)
                .py(EDIT_PAD_Y)
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
        let mut body = div().col().flex(1.0).py(EDIT_PAD_Y);
        let dot = theme().text_muted.alpha(0.55);
        for row_index in first..last {
            let Some(row) = self.row(row_index) else {
                break;
            };
            let mut r = div().row().h_px(EDIT_LINE_H).items_center();
            if row.indent > 0 {
                r = r.child(label(" ".repeat(row.indent)).size(EDIT_FONT).mono());
            }
            // Whitespace inside a selection shows as faint middle dots, injected into the text flow so they sit
            // on the glyph grid.
            let dots = self.sel_vis_range_for(row.line);
            let (mut byte, mut column) = (0usize, 0usize);
            let mut empty = true;
            for (segment, color) in self.line_segments(row.line, &colors) {
                let segment_end = byte + segment.len();
                if segment_end <= row.start || byte >= row.end {
                    column += segment.chars().count();
                    byte = segment_end;
                    continue;
                }
                let mut run = String::new();
                let mut run_color = color;
                for (offset, ch) in segment.char_indices() {
                    let at = byte + offset;
                    if at < row.start || at >= row.end {
                        column += 1;
                        continue;
                    }
                    let is_dot = ch == ' ' && dots.is_some_and(|(s, e)| column >= s && column < e);
                    let ch_color = if is_dot { dot } else { color };
                    if ch_color != run_color && !run.is_empty() {
                        r = r.child(
                            label(std::mem::take(&mut run))
                                .size(EDIT_FONT)
                                .mono()
                                .color(run_color),
                        );
                    }
                    run_color = ch_color;
                    run.push(if is_dot { '\u{b7}' } else { ch });
                    column += 1;
                }
                if !run.is_empty() {
                    empty = false;
                    r = r.child(label(run).size(EDIT_FONT).mono().color(run_color));
                }
                byte = segment_end;
            }
            if empty && row.indent == 0 {
                r = r.child(label(" ").size(EDIT_FONT).mono());
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
        let buf = self.buffer.as_ref()?;
        let (cur_line, _) = buf.line_col();
        let gw = gutter_width(self.line_count());
        let dcount = self.disp_count();
        let first = self.first_line().min(dcount);
        let visible = (self.body_h / EDIT_LINE_H).ceil() as usize + 2;
        let last = (first + visible).min(dcount);
        let mut col = div().col().flex(1.0).px(EDIT_PAD_X).py(EDIT_PAD_Y);
        for row_index in first..last {
            let Some(row) = self.row(row_index) else {
                break;
            };
            let line = row.line;
            if row.start > 0 {
                col = col.child(div().w_px(gw).h_px(EDIT_LINE_H));
                continue;
            }
            let num_color = if line == cur_line {
                theme().text
            } else {
                theme().text_muted
            };
            // Fold chevron: down when expanded, right when collapsed; empty (fixed width) for non-foldable lines
            // so the numbers stay aligned.
            let mut fold_cell = div()
                .w_px(FOLD_COL)
                .h_px(EDIT_LINE_H)
                .items_center()
                .justify_center();
            if self.is_foldable(line) {
                let kind = if self.is_folded(line) {
                    IconKind::ChevronRight
                } else {
                    IconKind::ChevronDown
                };
                fold_cell = fold_cell
                    .on_click(fold_base + line as u64)
                    .child(icon(kind).size(12.0).color(theme().icon_muted));
            }
            col = col.child(
                div()
                    .row()
                    .w_px(gw)
                    .h_px(EDIT_LINE_H)
                    .items_center()
                    .pr(GUTTER_GAP)
                    .child(div().flex(1.0))
                    .child(
                        label((line + 1).to_string())
                            .size(EDIT_FONT)
                            .mono()
                            .color(num_color),
                    )
                    .child(fold_cell),
            );
        }
        Some(col.into())
    }

    fn gutter_w(&self) -> f32 {
        gutter_width(self.line_count())
    }

    fn toggle_fold(&mut self, line: usize) {
        self.do_toggle_fold(line);
    }

    fn save(&mut self) -> Result<(), String> {
        let Some(b) = self.buffer.as_mut() else {
            return Ok(());
        };
        let mtime = files::write(&self.root, &self.path, &b.text_for_save())
            .map_err(|e| format!("Could not save {}: {e}", self.path))?;
        b.mark_saved();
        self.saved_mtime = Some(mtime);
        self.conflict = false;
        Ok(())
    }

    fn is_dirty(&self) -> bool {
        self.conflict || self.buffer.as_ref().is_some_and(|b| b.is_dirty())
    }

    fn has_conflict(&self) -> bool {
        self.conflict
    }

    fn refresh_disk_state(&mut self) {
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
        self.saved_mtime = Some(disk);
        self.refresh();
        self.ensure_visible();
    }

    fn is_busy(&self) -> bool {
        self.syntax.as_ref().is_some_and(|s| s.is_parsing())
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
        (self.scroll_x - before).abs() > 0.01
    }

    fn h_scrollbar(&self, content: Rect) -> Option<Rect> {
        self.buffer.as_ref()?;
        let max_scroll = self.max_scroll_x();
        if max_scroll <= 0.0 {
            return None;
        }
        let gw = gutter_width(self.line_count());
        let track_x = content.x + gw;
        let track_w = (content.w - gw).max(0.0);
        let content_w = self.content_w();
        let thumb_w = (track_w * track_w / content_w).clamp(28.0, track_w);
        let t = (self.scroll_x / max_scroll).clamp(0.0, 1.0);
        let x = track_x + t * (track_w - thumb_w);
        Some(Rect::new(
            x,
            content.y + content.h + EDIT_PAD_Y - 6.0,
            thumb_w,
            4.0,
            theme().scrollbar_thumb_background,
        ))
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
        let before = self.scroll_y;
        self.set_scroll_y(self.scroll_y - dy);
        (self.scroll_y - before).abs() > 0.01
    }

    fn input_text(&mut self, text: &str) {
        self.refresh();
        let language = editor::language::config(self.lang);
        let scope_at = scope_lookup(&self.syntax);
        if let Some(b) = self.buffer.as_mut() {
            b.unmark_text();
            b.handle_input(text, &language, &scope_at);
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
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
        let scope_at = scope_lookup(&self.syntax);
        if let Some(b) = self.buffer.as_mut() {
            b.commit_text(text, &language, &scope_at);
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn copy(&self) -> Option<CopiedText> {
        self.buffer.as_ref().map(|b| copied_text(b.copy()))
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
        if key == EditKey::Outdent {
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
                    let scope_at = scope_lookup(&self.syntax);
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
                    theme().text,
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
        let mut rects = Vec::new();

        // The cursor's whole display line is highlighted, across its soft-wrapped rows.
        if self.focused {
            let line = self.buf_of(self.position(b.cursor()).0);
            let (top, bottom) = (self.disp_of(line), self.last_row_of_line(line));
            if top < last && bottom >= first {
                rects.push(Rect::new(
                    content.x,
                    row_y(top),
                    content.w,
                    (bottom + 1 - top) as f32 * EDIT_LINE_H,
                    theme().editor_active_line,
                ));
            }
        }

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
        let guide = theme().panel_indent_guide;
        for (s, e, depth) in self.indent_guides(buf_first, buf_last) {
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
        rects
    }

    fn selection_tris(&self, content: Rect) -> Vec<ui::Tri> {
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
        let sel_color = Rgba::new(
            sr as f32 / 255.0,
            sg as f32 / 255.0,
            sb as f32 / 255.0,
            st.selection_alpha,
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

    fn scrollbar(&self, content: Rect) -> Option<Rect> {
        self.buffer.as_ref()?;
        let max_scroll = self.max_scroll();
        let visible = content.h;
        if self.content_h() <= visible || max_scroll <= 0.0 {
            return None;
        }
        // The scroll range includes one page past the last row.
        let thumb_h = (visible * visible / (self.content_h() + visible)).clamp(28.0, visible);
        let t = (self.scroll_y / max_scroll).clamp(0.0, 1.0);
        Some(Rect::new(
            content.x + content.w + EDIT_PAD_X - 6.0,
            content.y + t * (visible - thumb_h),
            4.0,
            thumb_h,
            theme().scrollbar_thumb_background,
        ))
    }
}

/// Syntax scope at a byte offset, for bracket rules that differ inside strings and comments.
fn scope_lookup(syntax: &Option<Syntax>) -> impl Fn(usize) -> editor::language::Scope + Copy + '_ {
    move |byte| {
        syntax
            .as_ref()
            .map(|s| s.scope_at(byte))
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
}

impl<'a> EditorRows<'a> {
    fn new(item: &'a FileItem) -> Self {
        Self {
            item,
            folds: item.folds.merged(),
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
        self.item.position(offset).0
    }

    fn max_row(&self) -> usize {
        self.item.disp_count().saturating_sub(1)
    }

    fn row_start(&self, row: usize) -> usize {
        self.item.row_start_offset(row)
    }

    fn row_end(&self, row: usize) -> usize {
        self.item.row_end_offset(row)
    }

    fn line_start(&self, offset: usize) -> usize {
        let line = self.item.buf_of(self.row_of(offset));
        self.item.display_offset(line, 0)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.item
            .display_line_end(self.item.buf_of(self.row_of(offset)))
    }

    fn x_of(&self, offset: usize) -> f32 {
        self.item.position(offset).1
    }

    fn offset_for_x(&self, row: usize, x: f32) -> usize {
        self.clip(self.item.offset_for_row_x(row, x, false), Bias::Left)
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
            path: path.to_string(),
            name,
            id,
            ok,
        }
    }
}

impl Item for ImageItem {
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
                .px(EDIT_PAD_X)
                .py(EDIT_PAD_Y)
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

/// One editor pane: a set of open tabs (`Item`s) and the active one (the pane model). A leaf of the pane group.
/// `id` is stable across re-layouts so a drag-and-drop that restructures the tree can still find source/target
/// panes after their paths shift.
#[derive(Default)]
struct Pane {
    id: u64,
    open: Vec<Box<dyn Item>>,
    active: Option<usize>,
    /// Back/forward navigation stacks of item ids (paths), like the reference's per-pane nav history. Activating
    /// a different tab pushes the previous one onto `back` and clears `fwd`; the arrows walk between them.
    back: Vec<String>,
    fwd: Vec<String>,
}

impl Pane {
    fn active_id(&self) -> Option<String> {
        self.active.and_then(|i| self.open.get(i)?.id())
    }

    fn index_of_id(&self, id: &str) -> Option<usize> {
        self.open.iter().position(|o| o.id().as_deref() == Some(id))
    }

    /// Activate `i` from a user gesture (tab click / open), recording the previous tab for back-navigation.
    fn activate_user(&mut self, i: usize) {
        if self.active == Some(i) {
            return;
        }
        if let Some(prev) = self.active_id() {
            self.back.push(prev);
            self.fwd.clear();
        }
        self.active = Some(i);
    }

    fn can_back(&self) -> bool {
        self.back.iter().any(|id| self.index_of_id(id).is_some())
    }

    fn can_forward(&self) -> bool {
        self.fwd.iter().any(|id| self.index_of_id(id).is_some())
    }

    fn nav_back(&mut self) {
        while let Some(id) = self.back.pop() {
            if let Some(i) = self.index_of_id(&id) {
                if let Some(cur) = self.active_id() {
                    self.fwd.push(cur);
                }
                self.active = Some(i);
                return;
            }
        }
    }

    fn nav_forward(&mut self) {
        while let Some(id) = self.fwd.pop() {
            if let Some(i) = self.index_of_id(&id) {
                if let Some(cur) = self.active_id() {
                    self.back.push(cur);
                }
                self.active = Some(i);
                return;
            }
        }
    }

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

    fn close_tab(&mut self, i: usize) {
        if i >= self.open.len() {
            return;
        }
        self.open.remove(i);
        self.active = if self.open.is_empty() {
            None
        } else {
            Some(self.active.unwrap_or(0).min(self.open.len() - 1))
        };
    }

    fn active_item(&self) -> Option<&dyn Item> {
        self.active
            .and_then(|i| self.open.get(i))
            .map(|b| b.as_ref())
    }
}

/// Rescale `flexes` in place so they sum to their count (the pane-group invariant), preserving ratios. A no-op
/// when already normalized; falls back to equal weights if the sum is degenerate.
fn renormalize(flexes: &mut [f32]) {
    let n = flexes.len() as f32;
    let sum: f32 = flexes.iter().sum();
    if sum > 0.001 {
        let k = n / sum;
        for f in flexes.iter_mut() {
            *f *= k;
        }
    } else {
        flexes.iter_mut().for_each(|f| *f = 1.0);
    }
}

impl Member {
    /// The leaf `Pane` at `path` (a sequence of child indices from this node), if `path` lands on a leaf.
    fn leaf_at_mut(&mut self, path: &[usize]) -> Option<&mut Pane> {
        match self {
            Member::Leaf(p) => path.is_empty().then_some(p),
            Member::Split(s) => {
                let (i, rest) = path.split_first()?;
                s.members.get_mut(*i)?.leaf_at_mut(rest)
            }
        }
    }

    /// The `Split` at `path` (an empty path lands on this node if it is a split).
    fn split_at_mut(&mut self, path: &[usize]) -> Option<&mut Split> {
        match self {
            Member::Leaf(_) => None,
            Member::Split(s) => match path.split_first() {
                None => Some(s),
                Some((i, rest)) => s.members.get_mut(*i)?.split_at_mut(rest),
            },
        }
    }

    /// The path to the leaf pane with the given stable `id`, if it still exists.
    fn path_of(&self, id: u64, path: &mut Vec<usize>) -> Option<Vec<usize>> {
        match self {
            Member::Leaf(p) => (p.id == id).then(|| path.clone()),
            Member::Split(s) => {
                for (i, m) in s.members.iter().enumerate() {
                    path.push(i);
                    let found = m.path_of(id, path);
                    path.pop();
                    if found.is_some() {
                        return found;
                    }
                }
                None
            }
        }
    }

    /// The path to the first leaf (used as the active fallback after a structural change).
    fn first_leaf_path(&self) -> Vec<usize> {
        let mut path = Vec::new();
        let mut cur = self;
        while let Member::Split(s) = cur {
            path.push(0);
            match s.members.first() {
                Some(m) => cur = m,
                None => break,
            }
        }
        path
    }

    fn for_each_pane(&self, f: &mut dyn FnMut(&Pane)) {
        match self {
            Member::Leaf(p) => f(p),
            Member::Split(s) => s.members.iter().for_each(|m| m.for_each_pane(f)),
        }
    }

    fn for_each_pane_mut(&mut self, f: &mut dyn FnMut(&mut Pane)) {
        match self {
            Member::Leaf(p) => f(p),
            Member::Split(s) => s.members.iter_mut().for_each(|m| m.for_each_pane_mut(f)),
        }
    }

    /// Total leaf panes under this node.
    fn leaf_count(&self) -> usize {
        match self {
            Member::Leaf(_) => 1,
            Member::Split(s) => s.members.iter().map(Member::leaf_count).sum(),
        }
    }

    /// Collapse any split that has a single child into that child, bottom-up, so removing a pane never leaves a
    /// redundant one-way split (matching the reference's group-normalization on close).
    fn collapse(&mut self) {
        if let Member::Split(s) = self {
            for m in &mut s.members {
                m.collapse();
            }
            if s.members.len() == 1 {
                let only = s.members.remove(0);
                *self = only;
            }
        }
    }
}

/// One flattened, currently-visible tree row (a directory or a file).
struct Row {
    name: String,
    path: String,
    is_dir: bool,
    depth: usize,
    open: bool,
}

pub struct FilesView {
    root: PathBuf,
    tree: Vec<FileNode>,
    expanded: HashSet<String>,
    /// The editor pane group: a recursive tree of split panes, and the path (child indices from the root) of
    /// the focused leaf (an empty path = the group is itself a single pane).
    group: Member,
    active: Vec<usize>,
    /// Monotonic source of stable pane ids (see `Pane::id`).
    next_pane_id: u64,
    /// Rebuilt each `editor_layout`: pane render-index -> that pane's path, its stable id, and its screen rect;
    /// plus the divider metadata keyed by `DIVIDER_BASE + i`.
    pane_order: Vec<Vec<usize>>,
    pane_ids: Vec<u64>,
    pane_rects: Vec<Rect>,
    divider_order: Vec<DividerRef>,
    /// The hit id the pointer is over, so a tab reveals its close button only while hovered.
    hover: Option<u64>,
    /// An in-progress tab drag and the highlight rect previewing where it would land.
    tab_drag: Option<TabDrag>,
    drag_preview: Option<Rect>,
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
}

/// The syntax palette matching the active UI theme's light/dark appearance, so highlighting stays in sync with
/// the app theme (the reference drives both UI and syntax from one theme).
fn syntax_theme() -> Theme {
    if theme().appearance == ui::Appearance::Light {
        Theme::one_light()
    } else {
        Theme::one_dark()
    }
}

impl FilesView {
    pub fn new(root: PathBuf) -> Self {
        let tree = files::build_tree(&files::list(&root));
        Self {
            root,
            tree,
            expanded: HashSet::new(),
            group: Member::Leaf(Pane {
                id: 0,
                ..Pane::default()
            }),
            active: Vec::new(),
            next_pane_id: 1,
            pane_order: Vec::new(),
            pane_ids: Vec::new(),
            pane_rects: Vec::new(),
            divider_order: Vec::new(),
            hover: None,
            tab_drag: None,
            drag_preview: None,
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

    /// The focused pane, resolving the active path (falling back to the first leaf if the path went stale).
    fn active_pane_mut(&mut self) -> Option<&mut Pane> {
        if self.group.leaf_at_mut(&self.active).is_none() {
            self.active = self.group.first_leaf_path();
        }
        let path = self.active.clone();
        self.group.leaf_at_mut(&path)
    }

    /// The focused pane's active item id (a file path), for highlighting its row in the tree.
    fn active_path(&self) -> Option<String> {
        // A shared read-only walk (no `&mut`), so it can't self-heal a stale path -- the render always calls a
        // `&mut` method first, so by the time this runs the active path is valid.
        let mut cur = &self.group;
        for &i in &self.active {
            match cur {
                Member::Split(s) => cur = s.members.get(i)?,
                Member::Leaf(_) => return None,
            }
        }
        match cur {
            Member::Leaf(p) => p.active_item().and_then(|it| it.id()),
            Member::Split(_) => None,
        }
    }

    /// Open a file in the focused pane.
    fn open_file(&mut self, path: &str) {
        let root = self.root.clone();
        if let Some(pane) = self.active_pane_mut() {
            pane.open_file(&root, path);
        }
    }

    /// The active item of the focused pane (for keyboard input).
    fn active_item_mut(&mut self) -> Option<&mut dyn Item> {
        let pane = self.active_pane_mut()?;
        let i = pane.active?;
        pane.open.get_mut(i).map(|b| b.as_mut())
    }

    /// Read-only walk to the leaf `Pane` at `path`.
    fn pane_at(&self, path: &[usize]) -> Option<&Pane> {
        let mut cur = &self.group;
        for &i in path {
            match cur {
                Member::Split(s) => cur = s.members.get(i)?,
                Member::Leaf(_) => return None,
            }
        }
        match cur {
            Member::Leaf(p) => Some(p),
            Member::Split(_) => None,
        }
    }

    /// The focused pane's active item (read-only), for clipboard reads.
    fn active_item_ref(&self) -> Option<&dyn Item> {
        let pane = self.pane_at(&self.active)?;
        let i = pane.active?;
        pane.open.get(i).map(|b| b.as_ref())
    }

    /// The pane body under `(x, y)`: its path and the content-local pointer (before gutter/scroll), or `None`
    /// if the point isn't in a pane body (e.g. the tab bar).
    fn editor_local(&self, x: f32, y: f32) -> Option<(Vec<usize>, f32, f32)> {
        let idx = self
            .pane_rects
            .iter()
            .position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)?;
        let rect = self.pane_rects[idx];
        let path = self.pane_order[idx].clone();
        let empty = self
            .pane_at(&path)
            .map(|p| p.open.is_empty())
            .unwrap_or(true);
        let tab_h = if empty { 0.0 } else { TAB_H };
        let local_x = (x - (rect.x + EDIT_PAD_X)).max(0.0);
        let local_y = y - (rect.y + tab_h + EDIT_PAD_Y);
        if local_y < 0.0 {
            return None; // tab bar
        }
        Some((path, local_x, local_y))
    }

    /// Whether the focused pane's active item accepts text input.
    fn focused_editable(&self) -> bool {
        let mut cur = &self.group;
        for &i in &self.active {
            match cur {
                Member::Split(s) => match s.members.get(i) {
                    Some(m) => cur = m,
                    None => return false,
                },
                Member::Leaf(_) => return false,
            }
        }
        match cur {
            Member::Leaf(p) => p.active_item().map(|it| it.is_editable()).unwrap_or(false),
            Member::Split(_) => false,
        }
    }

    /// A clone-on-split of the active item of the pane at `path` (for the toolbar split buttons).
    fn clone_active_of(&mut self, path: &[usize]) -> Option<Box<dyn Item>> {
        self.group
            .leaf_at_mut(path)
            .and_then(|p| p.active_item())
            .and_then(|it| it.clone_on_split())
    }

    /// Split the pane at `path`, placing `item` (if any) in a new pane in `dir`, and focus the new pane. If the
    /// pane's parent split already runs along the split axis the new pane is inserted as a sibling; otherwise the
    /// pane is wrapped in a fresh split of the two. Returns the new pane's path.
    fn do_split(
        &mut self,
        path: &[usize],
        dir: SplitDir,
        item: Option<Box<dyn Item>>,
    ) -> Option<Vec<usize>> {
        if self.group.leaf_count() >= MAX_PANES {
            return None;
        }
        let mut new_pane = Pane {
            id: self.next_pane_id,
            ..Pane::default()
        };
        self.next_pane_id += 1;
        if let Some(it) = item {
            new_pane.open = vec![it];
            new_pane.active = Some(0);
        }
        let axis = dir.axis();
        let after = dir.after();

        let Some((&idx, parent_path)) = path.split_last() else {
            // The group is a single pane: wrap the whole group in a new split.
            let old = std::mem::replace(&mut self.group, Member::Leaf(Pane::default()));
            let members = if after {
                vec![old, Member::Leaf(new_pane)]
            } else {
                vec![Member::Leaf(new_pane), old]
            };
            self.group = Member::Split(Split {
                axis,
                flexes: vec![1.0; 2],
                members,
            });
            let ap = vec![if after { 1 } else { 0 }];
            self.active = ap.clone();
            return Some(ap);
        };

        let parent = self.group.split_at_mut(parent_path)?;
        let ap = if parent.axis == axis {
            let ins = if after { idx + 1 } else { idx };
            parent.members.insert(ins, Member::Leaf(new_pane));
            parent.flexes.insert(ins, 1.0);
            renormalize(&mut parent.flexes);
            [parent_path, &[ins]].concat()
        } else {
            let old = std::mem::replace(&mut parent.members[idx], Member::Leaf(Pane::default()));
            let members = if after {
                vec![old, Member::Leaf(new_pane)]
            } else {
                vec![Member::Leaf(new_pane), old]
            };
            parent.members[idx] = Member::Split(Split {
                axis,
                flexes: vec![1.0; 2],
                members,
            });
            [parent_path, &[idx, if after { 1 } else { 0 }]].concat()
        };
        self.active = ap.clone();
        Some(ap)
    }

    /// Move (or, when `dir` is set, split-and-move) the dragged tab to its resolved drop target. Removes the
    /// item from the source pane first, then splits/moves into the target (resolved by stable id so the split's
    /// restructuring can't invalidate it), and finally prunes the source pane if the move emptied it.
    fn apply_tab_drop(
        &mut self,
        source: u64,
        index: usize,
        target_pane: u64,
        dir: Option<SplitDir>,
    ) {
        let Some(src_path) = self.group.path_of(source, &mut Vec::new()) else {
            return;
        };
        // A center drop back onto the same pane is a no-op (nothing to reorder in v1).
        if dir.is_none() && target_pane == source {
            return;
        }
        let item = {
            let Some(pane) = self.group.leaf_at_mut(&src_path) else {
                return;
            };
            if index >= pane.open.len() {
                return;
            }
            let it = pane.open.remove(index);
            pane.active = if pane.open.is_empty() {
                None
            } else {
                Some(index.min(pane.open.len() - 1))
            };
            it
        };
        let Some(target_path) = self.group.path_of(target_pane, &mut Vec::new()) else {
            // Target vanished; put the item back so it isn't lost.
            if let Some(pane) = self.group.leaf_at_mut(&src_path) {
                pane.open.push(item);
                pane.active = Some(pane.open.len() - 1);
            }
            return;
        };
        match dir {
            Some(dir) => {
                self.do_split(&target_path, dir, Some(item));
            }
            None => {
                if let Some(pane) = self.group.leaf_at_mut(&target_path) {
                    pane.open.push(item);
                    pane.active = Some(pane.open.len() - 1);
                    self.active = target_path;
                }
            }
        }
        // Prune the source pane if the move left it empty.
        if let Some(src) = self.group.path_of(source, &mut Vec::new()) {
            let empty = self
                .group
                .leaf_at_mut(&src)
                .map(|p| p.open.is_empty())
                .unwrap_or(false);
            if empty && self.group.leaf_count() > 1 {
                self.remove_pane(&src);
            }
        }
    }

    /// Close tab `t` in the pane at `path`; if the pane empties and it isn't the only one, remove it and collapse
    /// any now-single-child split.
    fn close_tab(&mut self, path: &[usize], t: usize) {
        let Some(pane) = self.group.leaf_at_mut(path) else {
            return;
        };
        pane.close_tab(t);
        if pane.open.is_empty() && self.group.leaf_count() > 1 {
            self.remove_pane(path);
        }
    }

    /// Remove the leaf pane at `path` from its parent split, renormalize the parent's flexes, then collapse any
    /// single-child split and re-anchor the focus to the first remaining leaf.
    fn remove_pane(&mut self, path: &[usize]) {
        let Some((&idx, parent_path)) = path.split_last() else {
            return; // never remove the sole root pane
        };
        if let Some(parent) = self.group.split_at_mut(parent_path) {
            if idx < parent.members.len() {
                parent.members.remove(idx);
                parent.flexes.remove(idx);
                renormalize(&mut parent.flexes);
            }
        }
        self.group.collapse();
        self.active = self.group.first_leaf_path();
    }
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
        });
        if open {
            flatten(&n.children, depth + 1, expanded, out);
        }
    }
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

/// Lay out one member of the group into `rect`. Leaves emit a `PanePlacement` (rect + node); splits divide
/// `rect` by their flex ratios (subtracting the divider gaps), recurse, and emit a `DividerPlacement` per gap.
/// Accumulates the render-order pane paths (for tab/split click routing) and divider metadata (for drag).
#[allow(clippy::too_many_arguments)]
fn layout_member(
    m: &mut Member,
    rect: Rect,
    path: &mut Vec<usize>,
    active: &[usize],
    hover: Option<u64>,
    panes: &mut Vec<PanePlacement>,
    dividers: &mut Vec<DividerPlacement>,
    pane_order: &mut Vec<Vec<usize>>,
    pane_ids: &mut Vec<u64>,
    pane_rects: &mut Vec<Rect>,
    divider_order: &mut Vec<DividerRef>,
) {
    // Snap the incoming rect to whole pixels so 1px divider lines and pane edges stay crisp (no 1-2-3px fuzz).
    let rect = Rect::new(
        rect.x.round(),
        rect.y.round(),
        rect.w.round(),
        rect.h.round(),
        Rgba::TRANSPARENT,
    );
    match m {
        Member::Leaf(pane) => {
            let p = pane_order.len();
            pane_order.push(path.clone());
            pane_ids.push(pane.id);
            pane_rects.push(rect);
            // The active item's text content area (below the tab bar, inside the body padding) is where its
            // caret/selection geometry is anchored. Focus (caret visibility) follows the group's active pane.
            let is_focused = path.as_slice() == active;
            let tab_h = if pane.open.is_empty() { 0.0 } else { TAB_H };
            let body_rect = Rect::new(
                rect.x,
                rect.y + tab_h,
                rect.w,
                (rect.h - tab_h).max(0.0),
                Rgba::TRANSPARENT,
            );
            // The text content area (inside the body padding) is where caret/selection/guides are anchored.
            let content = Rect::new(
                rect.x + EDIT_PAD_X,
                rect.y + tab_h + EDIT_PAD_Y,
                (rect.w - 2.0 * EDIT_PAD_X).max(0.0),
                (rect.h - tab_h - 2.0 * EDIT_PAD_Y).max(0.0),
                Rgba::TRANSPARENT,
            );
            let (back, back_tris, carets, scrollbar, h_scrollbar, body) =
                match pane.active.and_then(|i| pane.open.get_mut(i)) {
                    Some(item) => {
                        item.set_focused(is_focused);
                        item.set_body_height(content.h);
                        item.set_body_width(content.w);
                        let back = item.back_rects(content);
                        let back_tris = item.selection_tris(content);
                        let carets = item.carets(content);
                        let scrollbar = item.scrollbar(content);
                        let h_scrollbar = item.h_scrollbar(content);
                        let y_offset = item.body_y_offset();
                        let x_offset = item.body_x_offset();
                        let gw = item.gutter_w();
                        let gutter = item.gutter(FOLD_BASE + p as u64 * PANE_STRIDE);
                        let node = item.render();
                        // Text starts right of the fixed gutter; it (plus caret/selection) clips to that region so
                        // scrolled glyphs never paint over the line numbers. The gutter clips to the left strip.
                        let text_left = content.x + gw;
                        let text_clip = Rect::new(
                            text_left,
                            body_rect.y,
                            (body_rect.x + body_rect.w - text_left).max(0.0),
                            body_rect.h,
                            Rgba::TRANSPARENT,
                        );
                        let gutter_clip = Rect::new(
                            body_rect.x,
                            body_rect.y,
                            (text_left - body_rect.x).max(0.0),
                            body_rect.h,
                            Rgba::TRANSPARENT,
                        );
                        (
                            back,
                            back_tris,
                            carets,
                            scrollbar,
                            h_scrollbar,
                            Some(PaneBody {
                                node,
                                rect: body_rect,
                                y_offset,
                                text_left,
                                x_offset,
                                text_clip,
                                gutter,
                                gutter_clip,
                            }),
                        )
                    }
                    None => (Vec::new(), Vec::new(), Vec::new(), None, None, None),
                };
            let node = render_pane(pane, p, hover);
            panes.push(PanePlacement {
                rect,
                node,
                body,
                back,
                back_tris,
                carets,
                scrollbar,
                h_scrollbar,
            });
        }
        Member::Split(s) => {
            let axis = s.axis;
            let n = s.members.len();
            let (along, base) = match axis {
                Axis::Horizontal => (rect.w, rect.x),
                Axis::Vertical => (rect.h, rect.y),
            };
            // The gap between children is a single 1px line (filled, so no white band); children share the rest.
            let avail = (along - (n.saturating_sub(1) as f32) * DIVIDER_LINE).max(0.0);
            let mut pos = 0.0f32; // logical offset from `base`, boundaries rounded so edges never straddle a pixel
            for i in 0..n {
                let start = (base + pos).round();
                pos += avail * s.flexes[i] / n as f32;
                let end = (base + pos).round();
                let extent = (end - start).max(0.0);
                let child_rect = match axis {
                    Axis::Horizontal => Rect::new(start, rect.y, extent, rect.h, Rgba::TRANSPARENT),
                    Axis::Vertical => Rect::new(rect.x, start, rect.w, extent, Rgba::TRANSPARENT),
                };
                path.push(i);
                layout_member(
                    &mut s.members[i],
                    child_rect,
                    path,
                    active,
                    hover,
                    panes,
                    dividers,
                    pane_order,
                    pane_ids,
                    pane_rects,
                    divider_order,
                );
                path.pop();
                if i + 1 < n {
                    // The 1px gap sits at [end, end+1]; the grab hit strip is wider and overlaps both panes.
                    let off = (DIVIDER_GRAB - DIVIDER_LINE) / 2.0;
                    let grab = match axis {
                        Axis::Horizontal => {
                            Rect::new(end - off, rect.y, DIVIDER_GRAB, rect.h, Rgba::TRANSPARENT)
                        }
                        Axis::Vertical => {
                            Rect::new(rect.x, end - off, rect.w, DIVIDER_GRAB, Rgba::TRANSPARENT)
                        }
                    };
                    let id = DIVIDER_BASE + divider_order.len() as u64;
                    dividers.push(DividerPlacement {
                        rect: grab,
                        id,
                        axis: match axis {
                            Axis::Horizontal => DividerAxis::Horizontal,
                            Axis::Vertical => DividerAxis::Vertical,
                        },
                    });
                    divider_order.push(DividerRef {
                        split_path: path.clone(),
                        index: i,
                        axis,
                        container: avail,
                        child_start: start,
                    });
                    pos += DIVIDER_LINE;
                }
            }
        }
    }
}

/// One leaf pane: a tab bar (each tab a column [content | 1px underline]) plus the active item's body, filling
/// its clipped rect. `p` is the pane's render-order index (keys its tab/split ids); `hover` is the pointer's
/// current hit id, so a tab shows its close button only while hovered.
fn render_pane(pane: &Pane, p: usize, hover: Option<u64>) -> Node {
    // Tab bar: a full-width bottom border that the active tab punches through -- the active tab's bottom line
    // matches the editor so it merges into the body below; inactive tabs keep the border and sit on the line.
    let mut tab_bar = div().row().h_px(TAB_H).bg(theme().tab_bar_background);
    // Leading nav group (back/forward through this pane's activation history) + a vertical separator, mirroring
    // the reference's tab-bar start slot.
    tab_bar = tab_bar
        .child(nav_button(
            IconKind::ArrowLeft,
            NAV_BACK_BASE + p as u64,
            pane.can_back(),
        ))
        .child(nav_button(
            IconKind::ArrowRight,
            NAV_FWD_BASE + p as u64,
            pane.can_forward(),
        ))
        .child(div().w_px(1.0).h_px(TAB_H).bg(theme().border));
    for (t, o) in pane.open.iter().enumerate() {
        let is_active = pane.active == Some(t);
        let activate_id = TAB_ACTIVATE_BASE + p as u64 * PANE_STRIDE + t as u64;
        let close_id = TAB_CLOSE_BASE + p as u64 * PANE_STRIDE + t as u64;
        let hovered = hover == Some(activate_id) || hover == Some(close_id);
        let underline = if is_active {
            theme().editor_background
        } else {
            theme().border
        };
        // The close slot keeps its 16px whether or not the glyph shows, so revealing it on hover never shifts
        // the tab's width.
        let mut close_slot = div()
            .w_px(16.0)
            .h_px(16.0)
            .rounded(4.0)
            .items_center()
            .justify_center()
            .on_click(close_id);
        if hovered {
            close_slot =
                close_slot.child(icon(IconKind::Close).size(11.0).color(theme().icon_muted));
        }
        // Leading slot: a dot while the item has unsaved edits (warning color on a disk conflict).
        let mut dirty_slot = div().w_px(12.0).h_px(12.0).items_center().justify_center();
        if o.is_dirty() {
            let color = if o.has_conflict() {
                theme().warning
            } else {
                theme().text_accent
            };
            dirty_slot = dirty_slot.child(div().w_px(6.0).h_px(6.0).rounded(3.0).bg(color));
        }
        let content = div()
            .row()
            .flex(1.0)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .child(dirty_slot)
            .child(material_icon(o.icon().unwrap_or(MaterialIcon::Document)).size(14.0))
            .child(label(o.title()).size(13.0).color(if is_active {
                theme().text
            } else {
                theme().text_muted
            }))
            .child(close_slot);
        let cell = div()
            .col()
            .h_px(TAB_H)
            .bg(if is_active {
                theme().tab_active_background
            } else {
                theme().tab_inactive_background
            })
            .on_click(activate_id)
            .child(content)
            .child(div().h_px(1.0).bg(underline));
        tab_bar = tab_bar
            .child(cell)
            .child(div().w_px(1.0).h_px(TAB_H).bg(theme().border));
    }
    // The empty remainder carries the tab bar's bottom border; the split-right / split-down actions sit at its
    // trailing edge.
    tab_bar = tab_bar
        .child(
            div()
                .col()
                .flex(1.0)
                .h_px(TAB_H)
                .child(div().flex(1.0))
                .child(div().h_px(1.0).bg(theme().border)),
        )
        .child(div().w_px(1.0).h_px(TAB_H).bg(theme().border))
        .child(split_button(
            IconKind::PanelRight,
            SPLIT_RIGHT_BASE + p as u64,
        ))
        .child(split_button(
            IconKind::PanelBottom,
            SPLIT_DOWN_BASE + p as u64,
        ));

    // Just the pane chrome (the tab bar). The scrollable body is rendered separately by the shell so it can be
    // offset for smooth scroll; an empty pane has no chrome (the base editor-background fill shows).
    if pane.open.is_empty() {
        return div().col().flex(1.0).into();
    }
    div().col().flex(1.0).child(tab_bar).into()
}

/// A back/forward nav button in the pane's tab bar. Dimmed and non-clickable when there's nowhere to go.
fn nav_button(kind: IconKind, id: u64, enabled: bool) -> Node {
    let color = if enabled {
        theme().icon_muted
    } else {
        theme().icon_muted.alpha(0.35)
    };
    let mut inner = div()
        .row()
        .flex(1.0)
        .items_center()
        .justify_center()
        .child(icon(kind).size(15.0).color(color));
    if enabled {
        inner = inner.on_click(id);
    }
    div()
        .col()
        .w_px(26.0)
        .h_px(TAB_H)
        .child(inner)
        .child(div().h_px(1.0).bg(theme().border))
        .into()
}

/// A split-action button in the pane's tab bar (still carrying the bar's bottom border).
fn split_button(kind: IconKind, id: u64) -> Node {
    div()
        .col()
        .w_px(26.0)
        .h_px(TAB_H)
        .child(
            div()
                .row()
                .flex(1.0)
                .items_center()
                .justify_center()
                .on_click(id)
                .child(icon(kind).size(13.0).color(theme().icon_muted)),
        )
        .child(div().h_px(1.0).bg(theme().border))
        .into()
}

impl FunctionView for FilesView {
    fn editor_layout(&mut self, area: Rect) -> EditorLayout {
        let mut el = EditorLayout::default();
        let mut pane_order = Vec::new();
        let mut pane_ids = Vec::new();
        let mut pane_rects = Vec::new();
        let mut divider_order = Vec::new();
        let mut path = Vec::new();
        let active = self.active.clone();
        layout_member(
            &mut self.group,
            area,
            &mut path,
            &active,
            self.hover,
            &mut el.panes,
            &mut el.dividers,
            &mut pane_order,
            &mut pane_ids,
            &mut pane_rects,
            &mut divider_order,
        );
        self.pane_order = pane_order;
        self.pane_ids = pane_ids;
        self.pane_rects = pane_rects;
        self.divider_order = divider_order;
        el
    }

    fn render_tree(&mut self) -> Option<workspace::TreePanel> {
        // Left: the file tree. Rebuild the flattened visible rows + the click-id mapping.
        // Re-flatten only when the tree structure changed (expand/collapse), not on scroll.
        if self.flat_dirty {
            let mut f = Vec::new();
            flatten(&self.tree, 0, &self.expanded, &mut f);
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
            let content = div()
                .row()
                .gap(5.0)
                .pr(6.0)
                .items_center()
                .child(lead_row)
                .child(glyph)
                // Folder and file names share the same color (one filename color for both).
                .child(label(row.name.clone()).color(theme().text));
            let mut r = div()
                .row()
                .h_px(ROW_H)
                .pl(8.0)
                .items_center()
                .rounded(4.0)
                .on_click(id)
                .child(content);
            if selected {
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
        // Dividers are dragged, not clicked (the shell begins a drag on mouse-down); a bare click is a no-op.
        if id >= DIVIDER_BASE {
            return false;
        }
        // Fold chevron in the gutter: toggle the code fold on that pane's active item.
        if id >= FOLD_BASE {
            let n = id - FOLD_BASE;
            let (p, line) = ((n / PANE_STRIDE) as usize, (n % PANE_STRIDE) as usize);
            if let Some(path) = self.pane_order.get(p).cloned() {
                if let Some(item) = self
                    .group
                    .leaf_at_mut(&path)
                    .and_then(|pane| pane.active.and_then(|i| pane.open.get_mut(i)))
                {
                    item.toggle_fold(line);
                }
            }
            return true;
        }
        // Nav forward / back arrows: walk that pane's activation history.
        if id >= NAV_FWD_BASE {
            if let Some(path) = self.pane_order.get((id - NAV_FWD_BASE) as usize).cloned() {
                if let Some(pane) = self.group.leaf_at_mut(&path) {
                    pane.nav_forward();
                    self.active = path;
                }
            }
            return true;
        }
        if id >= NAV_BACK_BASE {
            if let Some(path) = self.pane_order.get((id - NAV_BACK_BASE) as usize).cloned() {
                if let Some(pane) = self.group.leaf_at_mut(&path) {
                    pane.nav_back();
                    self.active = path;
                }
            }
            return true;
        }
        // Split-down / split-right buttons: split the pane at that render index (a clone of its active item).
        if id >= SPLIT_DOWN_BASE {
            if let Some(path) = self
                .pane_order
                .get((id - SPLIT_DOWN_BASE) as usize)
                .cloned()
            {
                let item = self.clone_active_of(&path);
                self.do_split(&path, SplitDir::Down, item);
            }
            return true;
        }
        if id >= SPLIT_RIGHT_BASE {
            if let Some(path) = self
                .pane_order
                .get((id - SPLIT_RIGHT_BASE) as usize)
                .cloned()
            {
                let item = self.clone_active_of(&path);
                self.do_split(&path, SplitDir::Right, item);
            }
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
        // Tab close (higher range so it wins the nested hit over activate).
        if id >= TAB_CLOSE_BASE {
            let n = id - TAB_CLOSE_BASE;
            let (p, t) = ((n / PANE_STRIDE) as usize, (n % PANE_STRIDE) as usize);
            if let Some(path) = self.pane_order.get(p).cloned() {
                self.close_tab(&path, t);
            }
            return true;
        }
        // Tab activate: focus that pane + activate the tab.
        if id >= TAB_ACTIVATE_BASE {
            let n = id - TAB_ACTIVATE_BASE;
            let (p, t) = ((n / PANE_STRIDE) as usize, (n % PANE_STRIDE) as usize);
            if let Some(path) = self.pane_order.get(p).cloned() {
                if let Some(pane) = self.group.leaf_at_mut(&path) {
                    if t < pane.open.len() {
                        pane.activate_user(t);
                        self.active = path;
                    }
                }
            }
            return true;
        }
        // Otherwise a tree row.
        let idx = (id - FUNC_VIEW_BASE) as usize;
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
        // Only tab hits reveal a close button, so only a change in the hovered tab needs a repaint.
        let tabbish =
            |x: Option<u64>| x.is_some_and(|v| (TAB_ACTIVATE_BASE..STICKY_BASE).contains(&v));
        let changed = self.hover != id && (tabbish(self.hover) || tabbish(id));
        self.hover = id;
        changed
    }

    fn divider_axis(&self, id: u64) -> Option<DividerAxis> {
        if id < DIVIDER_BASE {
            return None;
        }
        let d = self.divider_order.get((id - DIVIDER_BASE) as usize)?;
        Some(match d.axis {
            Axis::Horizontal => DividerAxis::Horizontal,
            Axis::Vertical => DividerAxis::Vertical,
        })
    }

    /// Resize the split by dragging divider `id` so child `index`'s trailing edge tracks the absolute pointer
    /// (`x`, `y`). Ports the reference's `compute_resize`: it positions the boundary at the cursor, then empties
    /// the resulting pixel "bucket" across successive panes (forward or backward), clamping each to its minimum
    /// so a shrinking neighbour cascades the change onto the pane past it instead of stopping the drag.
    fn drag_divider(&mut self, id: u64, x: f32, y: f32) -> bool {
        if id < DIVIDER_BASE {
            return false;
        }
        let Some(d) = self.divider_order.get((id - DIVIDER_BASE) as usize) else {
            return false;
        };
        let split_path = d.split_path.clone();
        let ix = d.index;
        let axis = d.axis;
        let container = d.container;
        let child_start = d.child_start;
        if container <= 1.0 {
            return false;
        }
        let (pointer, min) = match axis {
            Axis::Horizontal => (x, MIN_PANE_W),
            Axis::Vertical => (y, MIN_PANE_H),
        };
        let Some(split) = self.group.split_at_mut(&split_path) else {
            return false;
        };
        let n = split.members.len();
        if ix + 1 >= n {
            return false;
        }
        let flexes = &mut split.flexes;
        // Faithful port of the reference's interactive resize: flex<->pixel via `size`, then empty a pixel
        // "bucket" (how far child `ix`'s trailing edge is from the cursor) across successive panes forward or
        // backward, clamping each to `min` so a shrinking neighbour cascades onto the pane past it. Because the
        // bucket is recomputed from the absolute cursor every mouse-move, the boundary converges on the pointer
        // across events (the reference's characteristic feel), rather than snapping in one step.
        let size = |i: usize, f: &[f32]| container * f[i] / n as f32;
        if min - 1.0 > size(ix, flexes) {
            return false;
        }
        let before = flexes.clone();
        let mut proposed = (pointer - child_start) - size(ix, flexes);
        let forward = proposed > 0.0;
        let mut offset: usize = 0;
        while proposed.abs() > 0.0 {
            // `ix - offset` only when `offset <= ix` (avoid usize underflow).
            let current = if forward {
                (ix + 1 + offset < n).then_some(ix + offset)
            } else if offset <= ix {
                Some(ix - offset)
            } else {
                None
            };
            let Some(cur) = current else { break };
            offset += 1;

            let next_target = (size(cur + 1, flexes) - proposed).max(min);
            let cur_target = (size(cur, flexes) + size(cur + 1, flexes) - next_target).max(min);
            let change = cur_target - size(cur, flexes);
            let flex_change = change / container;
            flexes[cur] += flex_change;
            flexes[cur + 1] -= flex_change;
            proposed -= change;
        }
        *flexes != before
    }

    fn is_tab(&self, id: u64) -> bool {
        (TAB_ACTIVATE_BASE..TAB_CLOSE_BASE).contains(&id)
    }

    fn begin_tab_drag(&mut self, id: u64) -> bool {
        if !self.is_tab(id) {
            return false;
        }
        let n = id - TAB_ACTIVATE_BASE;
        let (p, t) = ((n / PANE_STRIDE) as usize, (n % PANE_STRIDE) as usize);
        let Some(&source) = self.pane_ids.get(p) else {
            return false;
        };
        let path = self.pane_order.get(p).cloned().unwrap_or_default();
        let (title, icon) = self
            .group
            .leaf_at_mut(&path)
            .and_then(|pane| pane.open.get(t))
            .map(|it| (it.title(), it.icon().unwrap_or(MaterialIcon::Document)))
            .unwrap_or_else(|| (String::new(), MaterialIcon::Document));
        self.tab_drag = Some(TabDrag {
            source,
            index: t,
            title,
            icon,
            drop: None,
        });
        self.drag_preview = None;
        true
    }

    fn update_tab_drag(&mut self, x: f32, y: f32) -> bool {
        if self.tab_drag.is_none() {
            return false;
        }
        // The pane under the pointer (if any), then the drop zone: an outer edge -> split that side; the
        // interior -> move into the pane. Mirrors the reference's 20%-of-smaller-side edge bands.
        let target = self
            .pane_rects
            .iter()
            .position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|i| (self.pane_ids[i], self.pane_rects[i]));
        let (drop, preview) = match target {
            Some((pane, r)) => {
                let zone = DROP_EDGE * r.w.min(r.h);
                let (lx, ly) = (x - r.x, y - r.y);
                let dir = if lx < zone || lx > r.w - zone || ly < zone || ly > r.h - zone {
                    let (dl, dr, dt, db) = (lx, r.w - lx, ly, r.h - ly);
                    let m = dl.min(dr).min(dt).min(db);
                    Some(if m == dl {
                        SplitDir::Left
                    } else if m == dr {
                        SplitDir::Right
                    } else if m == dt {
                        SplitDir::Up
                    } else {
                        SplitDir::Down
                    })
                } else {
                    None
                };
                let half = |a, b, c, d| Rect::new(a, b, c, d, Rgba::TRANSPARENT);
                let preview = match dir {
                    Some(SplitDir::Left) => half(r.x, r.y, r.w / 2.0, r.h),
                    Some(SplitDir::Right) => half(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h),
                    Some(SplitDir::Up) => half(r.x, r.y, r.w, r.h / 2.0),
                    Some(SplitDir::Down) => half(r.x, r.y + r.h / 2.0, r.w, r.h / 2.0),
                    None => r,
                };
                (Some(TabDrop { pane, dir }), Some(preview))
            }
            None => (None, None),
        };
        if let Some(d) = self.tab_drag.as_mut() {
            d.drop = drop;
        }
        self.drag_preview = preview;
        true
    }

    fn drop_tab(&mut self) -> bool {
        let Some(drag) = self.tab_drag.take() else {
            return false;
        };
        self.drag_preview = None;
        match drag.drop {
            Some(drop) => {
                self.apply_tab_drop(drag.source, drag.index, drop.pane, drop.dir);
                true
            }
            None => false,
        }
    }

    fn cancel_tab_drag(&mut self) {
        self.tab_drag = None;
        self.drag_preview = None;
    }

    fn dragging_tab(&self) -> bool {
        self.tab_drag.is_some()
    }

    fn tab_drag_overlay(&self) -> Option<Rect> {
        self.drag_preview
    }

    fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)> {
        let drag = self.tab_drag.as_ref()?;
        // A floating copy of the dragged tab: icon + title in an elevated chip that follows the cursor.
        let w = (drag.title.chars().count() as f32 * 7.5 + 52.0).clamp(90.0, 260.0);
        let node = div()
            .row()
            .items_center()
            .gap(6.0)
            .px(10.0)
            .h_px(TAB_H)
            .rounded(6.0)
            .bg(theme().elevated_surface_background)
            .border(1.0, theme().border)
            .child(material_icon(drag.icon).size(14.0))
            .child(label(drag.title.clone()).size(13.0).color(theme().text))
            .into();
        Some((node, w, TAB_H))
    }

    fn editor_save(&mut self) -> Option<Result<(), String>> {
        match self.active_item_mut() {
            Some(item) if item.is_editable() => Some(item.save()),
            _ => None,
        }
    }

    fn refresh_disk_state(&mut self) {
        self.group.for_each_pane_mut(&mut |pane| {
            pane.open
                .iter_mut()
                .for_each(|item| item.refresh_disk_state())
        });
    }

    fn is_busy(&self) -> bool {
        let mut busy = false;
        self.group
            .for_each_pane(&mut |pane| busy |= pane.open.iter().any(|item| item.is_busy()));
        busy
    }

    fn editor_focused(&self) -> bool {
        self.focused_editable()
    }

    fn row_path(&self, id: u64) -> Option<(String, bool)> {
        if id < FUNC_VIEW_BASE {
            return None;
        }
        let idx = (id - FUNC_VIEW_BASE) as usize;
        self.click_targets.get(idx).cloned()
    }

    fn root_dir(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn open_path(&mut self, path: &str) {
        self.open_file(path);
    }

    fn editor_right_press(&mut self, x: f32, y: f32) -> bool {
        let Some((path, lx, ly)) = self.editor_local(x, y) else {
            return false;
        };
        self.active = path;
        match self.active_item_mut() {
            Some(item) if item.is_editable() => {
                item.right_press(lx, ly);
                true
            }
            _ => false,
        }
    }

    fn editor_menu_anchor_at(&self, x: f32, y: f32) -> Option<(Vec<usize>, usize)> {
        let (path, _lx, ly) = self.editor_local(x, y)?;
        let line = self.pane_at(&path)?.active_item()?.buffer_line_at(ly)?;
        Some((path, line))
    }

    fn editor_menu_y(&self, path: &[usize], line: usize) -> Option<f32> {
        let idx = self.pane_order.iter().position(|p| p.as_slice() == path)?;
        let rect = *self.pane_rects.get(idx)?;
        let pane = self.pane_at(path)?;
        let tab_h = if pane.open.is_empty() { 0.0 } else { TAB_H };
        let content = Rect::new(
            rect.x + EDIT_PAD_X,
            rect.y + tab_h + EDIT_PAD_Y,
            (rect.w - 2.0 * EDIT_PAD_X).max(0.0),
            (rect.h - tab_h - 2.0 * EDIT_PAD_Y).max(0.0),
            Rgba::TRANSPARENT,
        );
        pane.active_item()?.line_screen_y(content, line)
    }

    fn editor_paste(&mut self, text: &str, slices: Option<&[ClipboardSlice]>) -> bool {
        match self.active_item_mut() {
            Some(item) if item.is_editable() => {
                item.paste(text, slices);
                true
            }
            _ => false,
        }
    }

    fn editor_ime_preedit(&mut self, text: &str, selected: Option<Range<usize>>) -> bool {
        match self.active_item_mut() {
            Some(item) if item.is_editable() => {
                item.ime_preedit(text, selected);
                true
            }
            _ => false,
        }
    }

    fn editor_ime_commit(&mut self, text: &str) -> bool {
        match self.active_item_mut() {
            Some(item) if item.is_editable() => {
                item.ime_commit(text);
                true
            }
            _ => false,
        }
    }

    fn editor_copy(&self) -> Option<CopiedText> {
        self.active_item_ref().and_then(|item| item.copy())
    }

    fn editor_cut(&mut self) -> Option<CopiedText> {
        match self.active_item_mut() {
            Some(item) if item.is_editable() => item.cut(),
            _ => None,
        }
    }

    fn editor_text(&mut self, text: &str) -> bool {
        match self.active_item_mut() {
            Some(item) if item.is_editable() => {
                item.input_text(text);
                true
            }
            _ => false,
        }
    }

    fn editor_key(&mut self, key: EditKey, shift: bool) -> bool {
        match self.active_item_mut() {
            Some(item) if item.is_editable() => {
                item.input_key(key, shift);
                true
            }
            _ => false,
        }
    }

    fn editor_click(&mut self, x: f32, y: f32, extend: bool) -> bool {
        // A click in a pane body (below the tab bar) focuses it and places the caret. Tab-bar clicks are routed
        // by the tab hit ids, not here.
        let Some((path, local_x, local_y)) = self.editor_local(x, y) else {
            return false;
        };
        if !extend {
            self.active = path;
        }
        match self.active_item_mut() {
            Some(item) if item.is_editable() => {
                item.place_cursor(local_x, local_y, extend);
                true
            }
            _ => false,
        }
    }

    fn editor_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(index) = self.pane_order.iter().position(|p| *p == self.active) else {
            return false;
        };
        let Some(rect) = self.pane_rects.get(index).copied() else {
            return false;
        };
        let text_top = rect.y + TAB_H;
        let text_bottom = rect.y + rect.h;
        let vertical_margin = EDIT_LINE_H.min((text_bottom - text_top) / 3.0);
        let delta_rows = if y < text_top + vertical_margin {
            -drag_autoscroll_rows(text_top + vertical_margin - y)
        } else if y > text_bottom - vertical_margin {
            drag_autoscroll_rows(y - (text_bottom - vertical_margin))
        } else {
            0.0
        };
        let em = char_advance();
        let horizontal_space = HORIZONTAL_SCROLL_MARGIN * em;
        let Some(item) = self.active_item_mut() else {
            return false;
        };
        if !item.is_editable() {
            return false;
        }
        let text_left = rect.x + EDIT_PAD_X + item.gutter_w() + horizontal_space;
        let text_right = rect.x + rect.w - horizontal_space;
        let delta_columns = if x < text_left {
            -drag_autoscroll_columns(text_left - x)
        } else if x > text_right {
            drag_autoscroll_columns(x - text_right)
        } else {
            0.0
        };
        item.scroll_by(-delta_rows * EDIT_LINE_H);
        item.scroll_by_x(-delta_columns * em);
        let local_x = (x - (rect.x + EDIT_PAD_X)).max(0.0);
        let local_y = y - (rect.y + TAB_H + EDIT_PAD_Y);
        item.place_cursor(local_x, local_y, true);
        true
    }

    fn editor_double_click(&mut self, x: f32, y: f32) -> bool {
        let Some((path, local_x, local_y)) = self.editor_local(x, y) else {
            return false;
        };
        self.active = path;
        match self.active_item_mut() {
            Some(item) if item.is_editable() => {
                item.select_word_at(local_x, local_y);
                true
            }
            _ => false,
        }
    }

    fn editor_selected_text(&self) -> Option<String> {
        self.active_item_ref().and_then(|it| it.selected_text())
    }

    fn editor_scroll(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let idx = self
            .pane_rects
            .iter()
            .position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h);
        let Some(idx) = idx else { return false };
        let path = self.pane_order[idx].clone();
        match self
            .group
            .leaf_at_mut(&path)
            .and_then(|p| p.active.and_then(|i| p.open.get_mut(i)))
        {
            Some(item) if item.is_editable() => {
                let moved_y = item.scroll_by(dy);
                let moved_x = item.scroll_by_x(dx);
                moved_x || moved_y
            }
            _ => false,
        }
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
            (columns as f32 + 2.0) * em + 2.0 * EDIT_PAD_X + gutter_width(item.line_count());
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
