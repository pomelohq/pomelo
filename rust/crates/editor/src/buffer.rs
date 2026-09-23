use crate::language::{BracketPair, LanguageConfig, Scope};
use crate::movement;
use crate::search::SearchQuery;
use crate::transform::{LineTransform, TextTransform};
use ropey::Rope;
use std::ops::Range;
use std::time::{Duration, Instant};
use tree_sitter::{InputEdit, Point};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Unix,
    Windows,
}

impl LineEnding {
    pub fn detect(text: &str) -> Self {
        let mut max = text.len().min(1000);
        while !text.is_char_boundary(max) {
            max -= 1;
        }
        match text[..max].find('\n') {
            Some(ix) if ix > 0 && text.as_bytes()[ix - 1] == b'\r' => LineEnding::Windows,
            _ => LineEnding::Unix,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Unix => "\n",
            LineEnding::Windows => "\r\n",
        }
    }
}

pub fn normalize_newlines(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\r') {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// Where vertical motion aims: the x (in the display's units) the cursor started from.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SelectionGoal {
    #[default]
    None,
    HorizontalPosition(f32),
    /// The pixel span a column selection was built from; vertical motion aims at its end.
    HorizontalRange {
        start: f32,
        end: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Selection {
    pub id: usize,
    pub start: usize,
    pub end: usize,
    pub reversed: bool,
    pub goal: SelectionGoal,
}

impl Selection {
    pub fn head(&self) -> usize {
        if self.reversed {
            self.start
        } else {
            self.end
        }
    }

    pub fn tail(&self) -> usize {
        if self.reversed {
            self.end
        } else {
            self.start
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn range(&self) -> Option<(usize, usize)> {
        (!self.is_empty()).then_some((self.start, self.end))
    }

    pub fn set_head(&mut self, head: usize, goal: SelectionGoal) {
        let tail = self.tail();
        if head < tail {
            self.start = head;
            self.end = tail;
            self.reversed = true;
        } else {
            self.start = tail;
            self.end = head;
            self.reversed = false;
        }
        self.goal = goal;
    }

    pub fn collapse_to(&mut self, offset: usize, goal: SelectionGoal) {
        self.start = offset;
        self.end = offset;
        self.reversed = false;
        self.goal = goal;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

pub fn map_offset(edits: &[Edit], offset: usize, bias: Bias) -> usize {
    let mut delta: isize = 0;
    for edit in edits {
        if offset < edit.old.start {
            break;
        }
        if offset == edit.old.start {
            return match bias {
                Bias::Left => edit.new.start,
                Bias::Right => edit.new.end,
            };
        }
        if offset <= edit.old.end {
            return edit.new.end;
        }
        delta = edit.new.end as isize - edit.old.end as isize;
    }
    (offset as isize + delta).max(0) as usize
}

#[derive(Clone, Debug)]
struct Replacement {
    old: Range<usize>,
    new: Range<usize>,
    old_text: String,
    new_text: String,
}

struct LogEntry {
    version: u64,
    edits: Vec<Edit>,
    syntax: Vec<InputEdit>,
}

struct HistoryEntry {
    id: usize,
    batches: Vec<Vec<Replacement>>,
    first_edit_at: Instant,
    last_edit_at: Instant,
    suppress_grouping: bool,
    selections_before: Vec<Selection>,
    selections_after: Vec<Selection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    PageUp(usize),
    PageDown(usize),
    WordLeft,
    WordRight,
    SubwordLeft,
    SubwordRight,
    /// Start of the row, then the line's indentation, then column 0.
    Home,
    End,
    /// Like `Home`/`End` but ignoring soft wraps.
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

/// What a delete removes when a selection is empty; non-empty selections are always deleted as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deletion {
    Backward,
    Forward,
    PreviousWordStart,
    NextWordEnd,
    PreviousSubwordStart,
    NextSubwordEnd,
    ToBeginningOfLine,
    ToEndOfLine,
}

pub const TAB_SIZE: usize = 4;

/// How one selection's piece sits in copied text: its length in chars, whether it was a whole line (copied from
/// an empty selection), and the first line's indentation when copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardSelection {
    pub len: usize,
    pub is_entire_line: bool,
    pub first_line_indent: usize,
}

/// Edits from a line-wise command plus how the selections land afterwards; `moved` lists the text ranges
/// (old offsets) a line move relocated and by how many rows, so folds inside can follow.
#[derive(Default)]
pub struct LinePlan {
    edits: Vec<(Range<usize>, String)>,
    after: AfterLineEdit,
    pub moved: Vec<(Range<usize>, isize)>,
}

type RowColumn = (usize, usize);

enum AfterLineEdit {
    /// Each listed selection collapses to an old offset carried through the edit.
    Cursors(Vec<(usize, usize)>),
    /// Selections follow the edit like anchors, then listed ones shift by whole rows.
    Anchored { row_shifts: Vec<(usize, isize)> },
    /// Selections are placed at explicit (row, column) points in the new text.
    Points(Vec<(Selection, RowColumn, RowColumn)>),
}

impl Default for AfterLineEdit {
    fn default() -> Self {
        Self::Anchored {
            row_shifts: Vec::new(),
        }
    }
}

/// The rows a buffer is displayed in (after folds and soft wrap), for motions that move by what's on screen.
pub trait DisplayRows {
    /// Snap an offset inside hidden text to the nearest visible one in `bias`'s direction.
    fn clip(&self, offset: usize, bias: Bias) -> usize;
    fn row_of(&self, offset: usize) -> usize;
    fn max_row(&self) -> usize;
    fn row_start(&self, row: usize) -> usize;
    /// The last offset shown on `row` (before a soft break, the char before it).
    fn row_end(&self, row: usize) -> usize;
    /// Start of the display line (the run of rows between hard newlines) holding `offset`.
    fn line_start(&self, offset: usize) -> usize;
    fn line_end(&self, offset: usize) -> usize;
    fn x_of(&self, offset: usize) -> f32;
    fn offset_for_x(&self, row: usize, x: f32) -> usize;
}

/// One row per buffer line, measured in columns.
pub struct LineRows<'a>(pub &'a EditorBuffer);

impl DisplayRows for LineRows<'_> {
    fn clip(&self, offset: usize, _bias: Bias) -> usize {
        offset.min(self.0.rope.len_chars())
    }
    fn row_of(&self, offset: usize) -> usize {
        self.0.line_col_of(offset).0
    }
    fn max_row(&self) -> usize {
        self.0.rope.len_lines().saturating_sub(1)
    }
    fn row_start(&self, row: usize) -> usize {
        self.0.rope.line_to_char(row)
    }
    fn row_end(&self, row: usize) -> usize {
        self.0.clamp_to_line(row, usize::MAX)
    }
    fn line_start(&self, offset: usize) -> usize {
        self.0.line_start_of(offset)
    }
    fn line_end(&self, offset: usize) -> usize {
        self.0.line_end_of(offset)
    }
    fn x_of(&self, offset: usize) -> f32 {
        self.0.line_col_of(offset).1 as f32
    }
    fn offset_for_x(&self, row: usize, x: f32) -> usize {
        self.0.clamp_to_line(row, x.max(0.0).round() as usize)
    }
}

pub struct EditorBuffer {
    pub rope: Rope,
    line_ending: LineEnding,
    selections: Vec<Selection>,
    next_selection_id: usize,
    version: u64,
    log: Vec<LogEntry>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    transaction_depth: usize,
    next_transaction_id: usize,
    group_interval: Duration,
    saved_transaction: Option<usize>,
    /// Text between auto-inserted brackets, sorted by start; typing the closer at a region's end steps over it.
    autoclose_regions: Vec<AutocloseRegion>,
    /// The query repeated Cmd+D presses search for; dropped on any other selection change.
    select_next_state: Option<SelectNextState>,
    /// Column groups built by repeated add-caret-above/below presses.
    add_selections_state: Option<AddSelectionsState>,
    /// Selections before each syntax-node expansion, so shrinking can step back.
    syntax_node_history: Vec<(Vec<Selection>, bool)>,
    /// Text an input method is composing, one range per selection.
    marked_ranges: Vec<Range<usize>>,
    /// Undo-stack depth when the current composition started; its edits merge into one entry.
    ime_undo_base: Option<usize>,
}

#[derive(Clone, Debug)]
struct SelectNextState {
    query: String,
    wordwise: bool,
    done: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AddSelectionsState {
    groups: Vec<AddSelectionsGroup>,
}

#[derive(Clone, Debug)]
struct AddSelectionsGroup {
    above: bool,
    stack: Vec<usize>,
}

pub struct AddSelectionPlan {
    selections: Vec<Selection>,
    state: AddSelectionsState,
    next_id: usize,
}

/// The inside of an auto-closed bracket pair, owned by the selection that typed the opener. Its start sticks
/// before text typed at it and its end after, so the region grows as the user types inside.
#[derive(Clone, Debug)]
struct AutocloseRegion {
    selection_id: usize,
    range: Range<usize>,
    pair: BracketPair,
}

impl Default for EditorBuffer {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl EditorBuffer {
    pub fn from_text(text: &str) -> Self {
        let line_ending = LineEnding::detect(text);
        Self {
            rope: Rope::from_str(&normalize_newlines(text)),
            line_ending,
            selections: vec![Selection {
                id: 0,
                start: 0,
                end: 0,
                reversed: false,
                goal: SelectionGoal::None,
            }],
            next_selection_id: 1,
            version: 0,
            log: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            transaction_depth: 0,
            next_transaction_id: 0,
            group_interval: Duration::from_millis(300),
            saved_transaction: None,
            autoclose_regions: Vec::new(),
            select_next_state: None,
            add_selections_state: None,
            syntax_node_history: Vec::new(),
            marked_ranges: Vec::new(),
            ime_undo_base: None,
        }
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn text_for_save(&self) -> String {
        match self.line_ending {
            LineEnding::Unix => self.text(),
            LineEnding::Windows => self.text().replace('\n', "\r\n"),
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn set_group_interval(&mut self, interval: Duration) {
        self.group_interval = interval;
    }

    pub fn edits_since(&self, since: u64) -> impl Iterator<Item = &[Edit]> {
        let start = self.log.partition_point(|e| e.version <= since);
        self.log[start..].iter().map(|e| e.edits.as_slice())
    }

    pub fn syntax_edits_since(&self, since: u64) -> impl Iterator<Item = &InputEdit> {
        let start = self.log.partition_point(|e| e.version <= since);
        self.log[start..].iter().flat_map(|e| e.syntax.iter())
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn newest(&self) -> Selection {
        let mut newest = self.selections[0];
        for s in &self.selections {
            if s.id > newest.id {
                newest = *s;
            }
        }
        newest
    }

    fn oldest(&self) -> Selection {
        let mut oldest = self.selections[0];
        for s in &self.selections {
            if s.id < oldest.id {
                oldest = *s;
            }
        }
        oldest
    }

    pub fn cursor(&self) -> usize {
        self.newest().head()
    }

    fn new_selection(&mut self, start: usize, end: usize, reversed: bool) -> Selection {
        let id = self.next_selection_id;
        self.next_selection_id += 1;
        Selection {
            id,
            start,
            end,
            reversed,
            goal: SelectionGoal::None,
        }
    }

    fn select(&mut self, mut selections: Vec<Selection>) {
        let len = self.rope.len_chars();
        for s in &mut selections {
            s.start = s.start.min(len);
            s.end = s.end.min(len);
            if s.start > s.end {
                std::mem::swap(&mut s.start, &mut s.end);
                s.reversed = !s.reversed;
            }
        }
        selections.sort_by_key(|s| s.start);
        let mut i = 1;
        while i < selections.len() {
            let (prev, cur) = (selections[i - 1], selections[i]);
            if should_merge(prev.start, prev.end, cur.start, cur.end) {
                let removed = selections.remove(i);
                let keep = &mut selections[i - 1];
                keep.start = keep.start.min(removed.start);
                keep.end = keep.end.max(removed.end);
            } else {
                i += 1;
            }
        }
        if selections.is_empty() {
            let caret = self.new_selection(0, 0, false);
            selections.push(caret);
        }
        if selections.len() == 1 {
            self.add_selections_state = None;
        }
        self.select_next_state = None;
        self.syntax_node_history.clear();
        // A region lives only while its own selection stays inside it.
        self.autoclose_regions.retain(|region| {
            selections.iter().any(|s| {
                s.id == region.selection_id
                    && s.end >= region.range.start
                    && s.start <= region.range.end
            })
        });
        self.selections = selections;
    }

    // ---- position helpers ----

    pub fn line_col(&self) -> (usize, usize) {
        self.line_col_of(self.cursor())
    }

    pub fn line_col_of(&self, off: usize) -> (usize, usize) {
        let off = off.min(self.rope.len_chars());
        let line = self.rope.char_to_line(off);
        let line_start = self.rope.line_to_char(line);
        (line, off - line_start)
    }

    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let slice = self.rope.line(line);
        let mut len = slice.len_chars();
        if len > 0 && slice.char(len - 1) == '\n' {
            len -= 1;
        }
        len
    }

    pub fn offset_at(&self, line: usize, col: usize) -> usize {
        let last = self.rope.len_lines().saturating_sub(1);
        self.clamp_to_line(line.min(last), col)
    }

    fn clamp_to_line(&self, line: usize, col: usize) -> usize {
        self.rope.line_to_char(line) + col.min(self.line_len(line))
    }

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    fn line_start_of(&self, off: usize) -> usize {
        let (line, _) = self.line_col_of(off);
        self.rope.line_to_char(line)
    }

    fn line_end_of(&self, off: usize) -> usize {
        let (line, _) = self.line_col_of(off);
        self.rope.line_to_char(line) + self.line_len(line)
    }

    fn word_range_at(&self, off: usize) -> (usize, usize) {
        let n = self.rope.len_chars();
        let off = off.min(n);
        let inside = (off < n && Self::is_word(self.rope.char(off)))
            || (off > 0 && Self::is_word(self.rope.char(off - 1)));
        if !inside {
            return (off, off);
        }
        let mut s = off;
        while s > 0 && Self::is_word(self.rope.char(s - 1)) {
            s -= 1;
        }
        let mut e = off;
        while e < n && Self::is_word(self.rope.char(e)) {
            e += 1;
        }
        (s, e)
    }

    pub fn selected_text(&self) -> Option<String> {
        let parts: Vec<String> = self
            .selections
            .iter()
            .filter_map(|s| s.range().map(|(a, b)| self.rope.slice(a..b).to_string()))
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    pub fn start_transaction_at(&mut self, now: Instant) {
        self.transaction_depth += 1;
        if self.transaction_depth == 1 {
            let id = self.next_transaction_id;
            self.next_transaction_id += 1;
            self.undo_stack.push(HistoryEntry {
                id,
                batches: Vec::new(),
                first_edit_at: now,
                last_edit_at: now,
                suppress_grouping: false,
                selections_before: self.selections.clone(),
                selections_after: Vec::new(),
            });
        }
    }

    pub fn end_transaction_at(&mut self, now: Instant) {
        if self.transaction_depth == 0 {
            return;
        }
        self.transaction_depth -= 1;
        if self.transaction_depth > 0 {
            return;
        }
        let empty = self
            .undo_stack
            .last()
            .is_none_or(|entry| entry.batches.is_empty());
        if empty {
            self.undo_stack.pop();
            return;
        }
        self.redo_stack.clear();
        let selections = self.selections.clone();
        if let Some(entry) = self.undo_stack.last_mut() {
            entry.last_edit_at = now;
            entry.selections_after = selections;
        }
        self.group();
    }

    fn group(&mut self) {
        let mut count = 0;
        let mut entries = self.undo_stack.iter();
        if let Some(mut entry) = entries.next_back() {
            while let Some(prev) = entries.next_back() {
                if !prev.suppress_grouping
                    && entry
                        .first_edit_at
                        .saturating_duration_since(prev.last_edit_at)
                        < self.group_interval
                {
                    entry = prev;
                    count += 1;
                } else {
                    break;
                }
            }
        }
        let keep = self.undo_stack.len() - count;
        let merged: Vec<HistoryEntry> = self.undo_stack.drain(keep..).collect();
        if let Some(target) = self.undo_stack.last_mut() {
            for entry in merged {
                target.batches.extend(entry.batches);
                target.last_edit_at = entry.last_edit_at;
                target.selections_after = entry.selections_after;
            }
        }
    }

    pub fn finalize_last_transaction(&mut self) {
        if let Some(entry) = self.undo_stack.last_mut() {
            entry.suppress_grouping = true;
        }
    }

    fn transact<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let now = Instant::now();
        self.start_transaction_at(now);
        let result = f(self);
        self.end_transaction_at(now);
        result
    }

    pub fn edit(&mut self, edits: Vec<(Range<usize>, String)>) -> Vec<Edit> {
        let len = self.rope.len_chars();
        let mut edits: Vec<(Range<usize>, String)> = edits
            .into_iter()
            .map(|(r, t)| {
                (
                    r.start.min(len)..r.end.min(len),
                    normalize_newlines(&t).into_owned(),
                )
            })
            .filter(|(r, t)| !(r.is_empty() && t.is_empty()))
            .collect();
        edits.sort_by_key(|(r, _)| r.start);
        let mut merged: Vec<(Range<usize>, String)> = Vec::with_capacity(edits.len());
        for (range, text) in edits {
            match merged.last_mut() {
                Some((last, last_text)) if range.start < last.end => {
                    last.end = last.end.max(range.end);
                    last_text.push_str(&text);
                }
                _ => merged.push((range, text)),
            }
        }
        if merged.is_empty() {
            return Vec::new();
        }
        let replacements = self.apply(&merged);
        let applied = replacements
            .iter()
            .map(|r| Edit {
                old: r.old.clone(),
                new: r.new.clone(),
            })
            .collect();
        self.transact(|this| {
            if let Some(entry) = this.undo_stack.last_mut() {
                entry.batches.push(replacements);
            }
        });
        applied
    }

    fn apply(&mut self, edits: &[(Range<usize>, String)]) -> Vec<Replacement> {
        let mut replacements = Vec::with_capacity(edits.len());
        let mut delta: isize = 0;
        for (range, text) in edits {
            let new_start = (range.start as isize + delta) as usize;
            let new_len = text.chars().count();
            replacements.push(Replacement {
                old: range.clone(),
                new: new_start..new_start + new_len,
                old_text: self.rope.slice(range.clone()).to_string(),
                new_text: text.clone(),
            });
            delta += new_len as isize - range.len() as isize;
        }
        let mut syntax = Vec::with_capacity(edits.len());
        for (range, text) in edits.iter().rev() {
            let start_byte = self.rope.char_to_byte(range.start);
            let old_end_byte = self.rope.char_to_byte(range.end);
            let start_position = self.point_at_byte(start_byte);
            let old_end_position = self.point_at_byte(old_end_byte);
            self.rope.remove(range.clone());
            self.rope.insert(range.start, text);
            let new_end_byte = start_byte + text.len();
            syntax.push(InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_position,
                old_end_position,
                new_end_position: self.point_at_byte(new_end_byte),
            });
        }
        for region in &mut self.autoclose_regions {
            let batch: Vec<Edit> = replacements
                .iter()
                .map(|r| Edit {
                    old: r.old.clone(),
                    new: r.new.clone(),
                })
                .collect();
            region.range.start = map_offset(&batch, region.range.start, Bias::Left);
            region.range.end = map_offset(&batch, region.range.end, Bias::Right);
        }
        self.version += 1;
        self.log.push(LogEntry {
            version: self.version,
            edits: replacements
                .iter()
                .map(|r| Edit {
                    old: r.old.clone(),
                    new: r.new.clone(),
                })
                .collect(),
            syntax,
        });
        replacements
    }

    fn point_at_byte(&self, byte: usize) -> Point {
        let byte = byte.min(self.rope.len_bytes());
        let row = self.rope.byte_to_line(byte);
        Point {
            row,
            column: byte - self.rope.line_to_byte(row),
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        let edits = self
            .selections
            .iter()
            .map(|s| (s.start..s.end, text.to_string()))
            .collect();
        self.transact(|this| {
            let applied = this.edit(edits);
            this.remap_selections(&applied, |s, map| {
                let head = map(s.end, Bias::Right);
                s.collapse_to(head, SelectionGoal::None);
            });
        });
    }

    pub fn insert_char(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        self.insert_text(ch.encode_utf8(&mut buf));
    }

    pub fn backspace(&mut self) {
        self.delete_by(Deletion::Backward);
    }

    pub fn delete_forward(&mut self) {
        self.delete_by(Deletion::Forward);
    }

    fn remap_selections(
        &mut self,
        applied: &[Edit],
        f: impl Fn(&mut Selection, &dyn Fn(usize, Bias) -> usize),
    ) {
        let map = |offset: usize, bias: Bias| map_offset(applied, offset, bias);
        let mut next = self.selections.clone();
        for s in &mut next {
            f(s, &map);
        }
        self.select(next);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn undo(&mut self) {
        if self.transaction_depth > 0 {
            return;
        }
        let Some(entry) = self.undo_stack.pop() else {
            return;
        };
        for batch in entry.batches.iter().rev() {
            let inverse: Vec<(Range<usize>, String)> = batch
                .iter()
                .map(|r| (r.new.clone(), r.old_text.clone()))
                .collect();
            self.apply(&inverse);
        }
        let selections = entry.selections_before.clone();
        self.redo_stack.push(entry);
        self.select(selections);
    }

    pub fn redo(&mut self) {
        if self.transaction_depth > 0 {
            return;
        }
        let Some(entry) = self.redo_stack.pop() else {
            return;
        };
        for batch in &entry.batches {
            let forward: Vec<(Range<usize>, String)> = batch
                .iter()
                .map(|r| (r.old.clone(), r.new_text.clone()))
                .collect();
            self.apply(&forward);
        }
        let selections = entry.selections_after.clone();
        self.undo_stack.push(entry);
        self.select(selections);
    }

    pub fn is_dirty(&self) -> bool {
        self.undo_stack.last().map(|e| e.id) != self.saved_transaction
    }

    pub fn mark_saved(&mut self) {
        self.finalize_last_transaction();
        self.saved_transaction = self.undo_stack.last().map(|e| e.id);
    }

    pub fn place_cursor(&mut self, off: usize) {
        let caret = self.new_selection(off, off, false);
        self.select(vec![caret]);
    }

    pub fn extend_cursor(&mut self, off: usize) {
        let mut newest = self.newest();
        newest.set_head(off.min(self.rope.len_chars()), SelectionGoal::None);
        let mut next: Vec<Selection> = self
            .selections
            .iter()
            .copied()
            .filter(|s| s.id != newest.id)
            .collect();
        next.push(newest);
        self.select(next);
    }

    pub fn add_cursor(&mut self, off: usize) {
        let caret = self.new_selection(off, off, false);
        let mut next = self.selections.clone();
        next.push(caret);
        self.select(next);
    }

    pub fn collapse_cursors(&mut self) {
        let mut oldest = self.oldest();
        if self.selections.len() == 1 {
            let head = oldest.head();
            oldest.collapse_to(head, SelectionGoal::None);
        }
        self.select(vec![oldest]);
    }

    pub fn select_word_at(&mut self, off: usize) {
        let (a, b) = self.word_range_at(off);
        let s = self.new_selection(a, b, false);
        self.select(vec![s]);
    }

    pub fn select_all(&mut self) {
        let s = self.new_selection(0, self.rope.len_chars(), false);
        self.select(vec![s]);
    }

    /// The run of same-kind chars around `offset` (words win over punctuation over whitespace), within its line.
    pub fn surrounding_word(&self, offset: usize) -> Range<usize> {
        let offset = offset.min(self.rope.len_chars());
        let kind = self
            .char_before(offset)
            .map(movement::char_kind)
            .max(self.char_at(offset).map(movement::char_kind));
        let same = |c: char| Some(movement::char_kind(c)) == kind && c != '\n';
        let mut start = offset;
        while self.char_before(start).is_some_and(same) {
            start -= 1;
        }
        let mut end = offset;
        while self.char_at(end).is_some_and(same) {
            end += 1;
        }
        start..end
    }

    fn is_inside_word(&self, offset: usize) -> bool {
        let is_word = |c: char| movement::char_kind(c) == movement::CharKind::Word;
        self.char_before(offset).is_some_and(is_word) && self.char_at(offset).is_some_and(is_word)
    }

    /// Non-overlapping matches of `query` (ASCII case-insensitive) in char range `range`, as char ranges.
    fn find_matches(&self, query: &str, range: Range<usize>) -> Vec<Range<usize>> {
        if query.is_empty() || range.is_empty() {
            return Vec::new();
        }
        let start_byte = self.rope.char_to_byte(range.start);
        let haystack = self
            .rope
            .slice(range.clone())
            .to_string()
            .to_ascii_lowercase();
        let needle = query.to_ascii_lowercase();
        haystack
            .match_indices(&needle)
            .map(|(at, _)| {
                let start = self.rope.byte_to_char(start_byte + at);
                let end = self.rope.byte_to_char(start_byte + at + needle.len());
                start..end
            })
            .collect()
    }

    fn select_with_state(&mut self, next: Vec<Selection>, state: Option<SelectNextState>) {
        self.select(next);
        self.select_next_state = state;
    }

    fn insert_match(&mut self, range: Range<usize>, reversed: bool, replace_newest: bool) {
        let newest = self.newest().id;
        let mut next: Vec<Selection> = self
            .selections
            .iter()
            .copied()
            .filter(|s| !replace_newest || s.id != newest)
            .collect();
        next.push(self.new_selection(range.start, range.end, reversed));
        self.select(next);
    }

    /// Cmd+D. Carets first expand to their word; after that each press adds the next occurrence of the
    /// selected text (whole words only when it started from a caret), wrapping to the top once. Returns the
    /// newly selected range.
    pub fn select_next(&mut self, replace_newest: bool) -> Option<Range<usize>> {
        if let Some(mut state) = self.select_next_state.take() {
            let mut found = None;
            if !state.done {
                let first = self.oldest();
                let last = self.newest();
                let after = self.find_matches(&state.query, last.end..self.rope.len_chars());
                let before = self.find_matches(&state.query, 0..first.start);
                found = after.into_iter().chain(before).find(|m| {
                    let whole = !state.wordwise
                        || !self.is_inside_word(m.start) && !self.is_inside_word(m.end);
                    let index = self.selections.partition_point(|s| s.end <= m.start);
                    let overlaps = self.selections.get(index).is_some_and(|s| s.start < m.end);
                    whole && !overlaps
                });
                match found.clone() {
                    Some(range) => self.insert_match(range, last.reversed, replace_newest),
                    None => state.done = true,
                }
            }
            self.select_next_state = Some(state);
            return found;
        }
        let only_carets = self.selections.iter().all(Selection::is_empty);
        let first_text = self
            .selections
            .first()
            .map(|s| self.rope.slice(s.start..s.end).to_string());
        let same_text = self
            .selections
            .iter()
            .all(|s| Some(self.rope.slice(s.start..s.end).to_string()) == first_text);
        if only_carets {
            let words: Vec<Selection> = self
                .selections
                .iter()
                .map(|s| {
                    let word = self.surrounding_word(s.start);
                    Selection {
                        start: word.start,
                        end: word.end,
                        reversed: false,
                        goal: SelectionGoal::None,
                        ..*s
                    }
                })
                .collect();
            let state = (words.len() == 1).then(|| {
                let query = self.rope.slice(words[0].start..words[0].end).to_string();
                SelectNextState {
                    done: query.is_empty(),
                    query,
                    wordwise: true,
                }
            });
            let newest = words.last().map(|s| s.start..s.end);
            self.select_with_state(words, state);
            newest
        } else if let Some(text) = first_text.filter(|_| same_text) {
            self.select_next_state = Some(SelectNextState {
                query: text,
                wordwise: false,
                done: false,
            });
            self.select_next(replace_newest)
        } else {
            None
        }
    }

    /// Cmd+Shift+L: select every occurrence of the current selection (or the word at the caret), keeping the
    /// original as the newest selection. Returns the selected ranges.
    pub fn select_all_matches(&mut self) -> Vec<Range<usize>> {
        self.select_next(false);
        let Some(state) = self.select_next_state.take().filter(|s| !s.done) else {
            return Vec::new();
        };
        let initial = self.oldest();
        let matches: Vec<Range<usize>> = self
            .find_matches(&state.query, 0..self.rope.len_chars())
            .into_iter()
            .filter(|m| {
                let partial =
                    state.wordwise && (self.is_inside_word(m.start) || self.is_inside_word(m.end));
                let is_initial = m.start == initial.start && m.end == initial.end;
                !partial && !is_initial
            })
            .collect();
        let mut next: Vec<Selection> = matches
            .into_iter()
            .map(|m| self.new_selection(m.start, m.end, initial.reversed))
            .collect();
        let mut original = initial;
        original.id = self.next_selection_id;
        self.next_selection_id += 1;
        next.push(original);
        let ranges = next.iter().map(|s| s.start..s.end).collect();
        self.select_with_state(
            next,
            Some(SelectNextState {
                done: true,
                ..state
            }),
        );
        ranges
    }

    /// Cmd+Alt+Up/Down (and Cmd+Ctrl+P/N): add a caret or same-width selection on the next row above/below
    /// every column group, stepping by display rows (`skip_soft_wrap` false, by pixel x) or by buffer rows
    /// (true, by tab-expanded columns). Pressing the opposite way removes the last one a group added.
    pub fn plan_add_selection(
        &self,
        above: bool,
        skip_soft_wrap: bool,
        rows: &dyn DisplayRows,
    ) -> AddSelectionPlan {
        let mut next_id = self.next_selection_id;
        let mut new_selection = |start: usize, end: usize, reversed: bool, goal: SelectionGoal| {
            let id = next_id;
            next_id += 1;
            Selection {
                id,
                start,
                end,
                reversed,
                goal,
            }
        };
        let columnar_ids: Vec<usize> = self
            .add_selections_state
            .iter()
            .flat_map(|state| state.groups.iter().flat_map(|g| g.stack.iter().copied()))
            .collect();
        let (mut columnar, fresh): (Vec<Selection>, Vec<Selection>) = self
            .selections
            .iter()
            .partition(|s| columnar_ids.contains(&s.id));
        let mut state = self.add_selections_state.clone().unwrap_or_default();
        let build = |row: usize,
                     positions: &Range<f32>,
                     reversed: bool,
                     make: &mut dyn FnMut(usize, usize, bool, SelectionGoal) -> Selection|
         -> Option<Selection> {
            let start = rows.offset_for_x(row, positions.start);
            let goal = SelectionGoal::HorizontalRange {
                start: positions.start,
                end: positions.end,
            };
            if positions.start == positions.end {
                return Some(make(start, start, reversed, goal));
            }
            if start >= rows.row_end(row) {
                return None;
            }
            Some(make(
                start,
                rows.offset_for_x(row, positions.end),
                reversed,
                goal,
            ))
        };
        for selection in fresh {
            let (start_x, end_x) = (rows.x_of(selection.start), rows.x_of(selection.end));
            let positions = start_x.min(end_x)..start_x.max(end_x);
            let mut stack = Vec::new();
            for row in rows.row_of(selection.start)..=rows.row_of(selection.end) {
                if let Some(s) = build(row, &positions, selection.reversed, &mut new_selection) {
                    stack.push(s.id);
                    columnar.push(s);
                }
            }
            if !stack.is_empty() {
                if above {
                    stack.reverse();
                }
                state.groups.push(AddSelectionsGroup { above, stack });
            }
        }
        let end_row = if above { 0 } else { rows.max_row() };
        let goal_columns: Vec<(usize, Range<usize>)> = if skip_soft_wrap {
            state
                .groups
                .iter()
                .filter_map(|group| {
                    let oldest = columnar
                        .iter()
                        .find(|s| Some(&s.id) == group.stack.first())?;
                    let (a, b) = (
                        self.tab_expanded_column(oldest.start),
                        self.tab_expanded_column(oldest.end),
                    );
                    Some(group.stack.iter().map(move |id| (*id, a.min(b)..a.max(b))))
                })
                .flatten()
                .collect()
        } else {
            Vec::new()
        };
        let mut finals = Vec::new();
        for selection in columnar {
            let Some(group) = state
                .groups
                .iter_mut()
                .find(|g| g.stack.last() == Some(&selection.id))
            else {
                finals.push(selection);
                continue;
            };
            if group.above != above {
                group.stack.pop();
                continue;
            }
            let row = rows.row_of(selection.start);
            let found = if skip_soft_wrap {
                let columns = goal_columns
                    .iter()
                    .find(|(id, _)| *id == selection.id)
                    .map(|(_, c)| c.clone())
                    .unwrap_or_else(|| {
                        let (a, b) = (
                            self.tab_expanded_column(selection.start),
                            self.tab_expanded_column(selection.end),
                        );
                        a.min(b)..a.max(b)
                    });
                self.next_columnar_by_buffer_row(
                    row,
                    end_row,
                    above,
                    &columns,
                    selection.reversed,
                    rows,
                    &mut new_selection,
                )
            } else {
                let positions = match selection.goal {
                    SelectionGoal::HorizontalRange { start, end } => start..end,
                    _ => {
                        let (a, b) = (rows.x_of(selection.start), rows.x_of(selection.end));
                        a.min(b)..a.max(b)
                    }
                };
                let mut found = None;
                let mut current = row;
                while current != end_row {
                    current = if above { current - 1 } else { current + 1 };
                    found = build(current, &positions, selection.reversed, &mut new_selection);
                    if found.is_some() {
                        break;
                    }
                }
                found
            };
            match found {
                Some(new) => {
                    group.stack.push(new.id);
                    if above {
                        finals.push(new);
                        finals.push(selection);
                    } else {
                        finals.push(selection);
                        finals.push(new);
                    }
                }
                None => finals.push(selection),
            }
        }
        AddSelectionPlan {
            selections: finals,
            state,
            next_id,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn next_columnar_by_buffer_row(
        &self,
        start_row: usize,
        end_row: usize,
        above: bool,
        columns: &Range<usize>,
        reversed: bool,
        rows: &dyn DisplayRows,
        make: &mut dyn FnMut(usize, usize, bool, SelectionGoal) -> Selection,
    ) -> Option<Selection> {
        let mut row = start_row;
        while row != end_row {
            let row_start = rows.row_start(row);
            let next = if above {
                if row_start == 0 {
                    return None;
                }
                rows.line_start(row_start - 1)
            } else {
                let end = rows.line_end(row_start);
                if end >= self.rope.len_chars() {
                    return None;
                }
                end + 1
            };
            row = rows.row_of(next);
            let line = self.rope.char_to_line(next);
            let line_columns = self.tab_expanded_column(self.line_end_offset(line));
            let at = |column: usize| self.offset_for_tab_expanded_column(line, column);
            let (start, end) = if columns.start == columns.end {
                (at(columns.start), at(columns.start))
            } else if columns.start >= line_columns {
                continue;
            } else {
                (at(columns.start), at(columns.end))
            };
            let (start_x, end_x) = (rows.x_of(start), rows.x_of(end));
            let goal = SelectionGoal::HorizontalRange {
                start: start_x.min(end_x),
                end: start_x.max(end_x),
            };
            return Some(make(start, end, reversed, goal));
        }
        None
    }

    fn tab_expanded_column(&self, offset: usize) -> usize {
        let (line, column) = self.line_col_of(offset);
        crate::display::to_display(self.rope.line(line), column, TAB_SIZE).0
    }

    fn offset_for_tab_expanded_column(&self, line: usize, target: usize) -> usize {
        let mut column = 0usize;
        let mut chars = 0usize;
        for ch in self.rope.line(line).chars() {
            if ch == '\n' || column >= target {
                break;
            }
            let width = if ch == '\t' {
                TAB_SIZE - column % TAB_SIZE
            } else {
                1
            };
            if column + width > target {
                break;
            }
            column += width;
            chars += 1;
        }
        self.rope.line_to_char(line) + chars
    }

    pub fn apply_add_selection(&mut self, plan: AddSelectionPlan) {
        self.next_selection_id = self.next_selection_id.max(plan.next_id);
        let mut state = plan.state;
        self.select(plan.selections);
        let ids: Vec<usize> = self.selections.iter().map(|s| s.id).collect();
        state.groups.retain_mut(|group| {
            group.stack.retain(|id| ids.contains(id));
            group.stack.len() > 1
        });
        self.add_selections_state = (!state.groups.is_empty()).then_some(state);
    }

    /// Ctrl+Shift+Right: grow every selection to the smallest named syntax node enclosing it (a word first
    /// inside strings), skipping nodes that start or end inside a fold. `ancestor` looks nodes up by char range.
    pub fn select_larger_syntax_node(
        &mut self,
        ancestor: &dyn Fn(Range<usize>) -> Option<(Range<usize>, String, bool)>,
        intersects_fold: &dyn Fn(usize) -> bool,
    ) -> bool {
        let old = self.selections.clone();
        let mut grew = false;
        let mut next: Vec<Selection> = old
            .iter()
            .map(|selection| {
                let old_range = selection.start..selection.end;
                if let Some((_, kind, _)) = ancestor(old_range.clone()) {
                    if kind == "string_content" || kind == "inline" {
                        let word = self.surrounding_word(old_range.start);
                        if !word.is_empty()
                            && word != old_range
                            && word == self.surrounding_word(old_range.end)
                        {
                            grew = true;
                            return Selection {
                                start: word.start,
                                end: word.end,
                                goal: SelectionGoal::None,
                                ..*selection
                            };
                        }
                    }
                }
                let mut range = old_range.clone();
                while let Some((node_range, _, named)) = ancestor(range.clone()) {
                    range = node_range;
                    if named && !intersects_fold(range.start) && !intersects_fold(range.end) {
                        break;
                    }
                }
                grew |= range != old_range;
                Selection {
                    start: range.start,
                    end: range.end,
                    goal: SelectionGoal::None,
                    ..*selection
                }
            })
            .collect();
        if !grew {
            return false;
        }
        let reversed = match (next.len(), old.last(), next.last_mut()) {
            (1, Some(last_old), Some(last_new)) => {
                last_new.reversed = last_old.start != last_new.start;
                last_new.reversed
            }
            (_, _, Some(last_new)) => last_new.reversed,
            _ => false,
        };
        let mut history = std::mem::take(&mut self.syntax_node_history);
        self.select(next);
        history.push((old, reversed));
        self.syntax_node_history = history;
        true
    }

    /// Ctrl+Shift+Left: step back to the selections before the last `select_larger_syntax_node`.
    pub fn select_smaller_syntax_node(&mut self) -> bool {
        let mut history = std::mem::take(&mut self.syntax_node_history);
        let Some((mut previous, reversed)) = history.pop() else {
            return false;
        };
        if let Some(last) = previous.last_mut() {
            last.reversed = reversed;
        }
        self.select(previous);
        self.syntax_node_history = history;
        true
    }

    // ---- navigation (multi-cursor aware) ----

    // ---- typing: auto-close, overtype, surround, newline ----

    fn byte_of(&self, offset: usize) -> usize {
        self.rope.char_to_byte(offset.min(self.rope.len_chars()))
    }

    fn contains_str_at(&self, offset: usize, text: &str) -> bool {
        let len = self.rope.len_chars();
        offset <= len && {
            let mut chars = self.rope.chars_at(offset);
            text.chars().all(|c| chars.next() == Some(c))
        }
    }

    fn char_before(&self, offset: usize) -> Option<char> {
        (offset > 0 && offset <= self.rope.len_chars()).then(|| self.rope.char(offset - 1))
    }

    fn char_at(&self, offset: usize) -> Option<char> {
        (offset < self.rope.len_chars()).then(|| self.rope.char(offset))
    }

    /// The auto-close region the selection with `id` is inside, if any.
    fn autoclose_region_for(&self, selection: &Selection) -> Option<&AutocloseRegion> {
        self.autoclose_regions.iter().find(|region| {
            region.selection_id == selection.id
                && region.range.start <= selection.start
                && selection.end <= region.range.end
        })
    }

    /// Whether typing a quote at `offset` would close one already open earlier on the line: an odd count of
    /// the same quote before it, ignoring ones inside strings/comments that disable the pair.
    fn is_closing_quote(
        &self,
        offset: usize,
        quote: char,
        pair: &BracketPair,
        scope_at: &dyn Fn(usize) -> Scope,
    ) -> bool {
        let line_start = self.line_start_of(offset);
        let count = (line_start..offset)
            .filter(|&at| self.rope.char(at) == quote)
            .filter(|&at| {
                pair.enabled_in(scope_at(self.byte_of(at)))
                    && pair.enabled_in(scope_at(self.byte_of(at + 1)))
            })
            .count();
        count % 2 == 1
    }

    /// Type `text` at every selection. Opening brackets auto-close when followed by whitespace or a
    /// language-listed char, typing an auto-inserted closer steps over it, and a one-char opener typed over a
    /// selection wraps it. `scope_at` gives the syntax scope at a byte offset.
    pub fn handle_input(
        &mut self,
        text: &str,
        language: &LanguageConfig,
        scope_at: &dyn Fn(usize) -> Scope,
    ) {
        let text_len = text.chars().count();
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        // Per selection: where its start and end land (old offset + bias), plus chars to step past.
        let mut targets: Vec<(Selection, usize, Bias, usize, Bias, usize)> = Vec::new();
        let mut new_regions: Vec<(usize, usize, BracketPair)> = Vec::new();
        let selections = self.selections.clone();
        for selection in &selections {
            let scope = scope_at(self.byte_of(selection.head()));
            let mut bracket = None;
            let mut is_start = false;
            let mut matching_end = None;
            if !text.is_empty() {
                for pair in language.brackets {
                    if !pair.close && !pair.surround {
                        continue;
                    }
                    if pair.enabled_in(scope) && pair.start.ends_with(text) {
                        let prefix = &pair.start[..pair.start.len() - text.len()];
                        let prefix_chars = prefix.chars().count();
                        let column = self.line_col_of(selection.start).1;
                        let prefix_matches = prefix.is_empty()
                            || column >= prefix_chars
                                && self.contains_str_at(selection.start - prefix_chars, prefix);
                        if prefix_matches {
                            bracket = Some(*pair);
                            is_start = true;
                            break;
                        }
                    }
                    if pair.end == text && matching_end.is_none() {
                        matching_end = Some(*pair);
                    }
                }
                if bracket.is_none() {
                    bracket = matching_end;
                }
            }
            if let Some(pair) = bracket {
                if selection.is_empty() {
                    if is_start {
                        let following_allows = self
                            .char_at(selection.start)
                            .is_none_or(|c| language.should_autoclose_before(c));
                        let preceding_allows = self.line_col_of(selection.start).1 == 0
                            || self.char_before(selection.start).is_none_or(|c| {
                                pair.start != pair.end
                                    || movement::char_kind(c) != movement::CharKind::Word
                            });
                        let closing_quote = pair.start == pair.end
                            && pair.start.chars().count() == 1
                            && pair.start.chars().next().is_some_and(|quote| {
                                self.is_closing_quote(selection.start, quote, &pair, scope_at)
                            });
                        if pair.close && following_allows && preceding_allows && !closing_quote {
                            edits.push((
                                selection.start..selection.end,
                                format!("{text}{}", pair.end),
                            ));
                            targets.push((
                                *selection,
                                selection.end,
                                Bias::Left,
                                selection.end,
                                Bias::Left,
                                text_len,
                            ));
                            new_regions.push((selection.id, selection.end, pair));
                            continue;
                        }
                    }
                    if let Some(region) = self.autoclose_region_for(selection) {
                        let skip = selection.end == region.range.end
                            && text == region.pair.end
                            && self.contains_str_at(region.range.end, text);
                        if skip {
                            let delta = region.pair.end.chars().count();
                            targets.push((
                                *selection,
                                selection.end,
                                Bias::Right,
                                selection.end,
                                Bias::Right,
                                delta,
                            ));
                            continue;
                        }
                    }
                } else if pair.surround && is_start && pair.start.chars().count() == 1 {
                    edits.push((selection.start..selection.start, text.to_string()));
                    edits.push((selection.end..selection.end, pair.end.to_string()));
                    targets.push((
                        *selection,
                        selection.start,
                        Bias::Right,
                        selection.end,
                        Bias::Left,
                        0,
                    ));
                    continue;
                }
            }
            // Measured from the replaced range's start, so a selection that ends where the next begins still lands
            // after its own text rather than the neighbour's.
            edits.push((selection.start..selection.end, text.to_string()));
            targets.push((
                *selection,
                selection.start,
                Bias::Left,
                selection.start,
                Bias::Left,
                text_len,
            ));
        }
        self.transact(|this| {
            let applied = this.edit(edits);
            let map = |offset: usize, bias: Bias| map_offset(&applied, offset, bias);
            let next: Vec<Selection> = targets
                .iter()
                .map(|(selection, start, start_bias, end, end_bias, delta)| {
                    let mut s = *selection;
                    s.start = map(*start, *start_bias) + delta;
                    s.end = map(*end, *end_bias) + delta;
                    if selection.is_empty() || s.start == s.end {
                        s.reversed = false;
                    }
                    s.goal = SelectionGoal::None;
                    s
                })
                .collect();
            for (selection_id, position, pair) in new_regions {
                let at = map(position, Bias::Left) + text_len;
                let region = AutocloseRegion {
                    selection_id,
                    range: at..at,
                    pair,
                };
                let index = this
                    .autoclose_regions
                    .partition_point(|r| (r.range.start, r.range.end) <= (at, at));
                this.autoclose_regions.insert(index, region);
            }
            this.select(next);
        });
    }

    /// Enter at every selection: the new line keeps the current indentation (never more than the cursor's
    /// column), continues a line comment the cursor is past, and between a newline bracket pair (`{|}`) puts
    /// the closer on its own line with the cursor on the empty line between.
    pub fn newline(&mut self, language: &LanguageConfig, scope_at: &dyn Fn(usize) -> Scope) {
        let mut edits = Vec::new();
        let mut targets = Vec::new();
        for selection in &self.selections {
            let (line, column) = self.line_col_of(selection.start);
            let full_indent = self.indent_len(line);
            let indent: String = self
                .rope
                .line(line)
                .chars()
                .take(full_indent.min(column))
                .collect();
            let empty = selection.is_empty();
            let delimiter = if empty {
                self.comment_delimiter_for_newline(line, column, language)
            } else {
                None
            };
            let extra_line = delimiter.is_none()
                && self.insert_extra_newline(selection.start..selection.end, language, scope_at);
            let mut new_text = format!("\n{indent}");
            if let Some(delimiter) = delimiter {
                new_text.push_str(delimiter);
            }
            if extra_line {
                new_text.push('\n');
                new_text.push_str(&indent);
            }
            // A line holding only indentation up to the cursor loses it instead of keeping trailing blanks.
            let edit_start = if empty && full_indent > 0 && column == full_indent {
                self.rope.line_to_char(line)
            } else {
                selection.start
            };
            edits.push((edit_start..selection.end, new_text));
            targets.push((*selection, extra_line));
        }
        self.transact(|this| {
            let applied = this.edit(edits);
            let next: Vec<Selection> = targets
                .into_iter()
                .map(|(selection, extra_line)| {
                    let mut s = selection;
                    let mut cursor = map_offset(&applied, selection.end, Bias::Right);
                    if extra_line {
                        let row = this.rope.char_to_line(cursor).saturating_sub(1);
                        cursor = this.rope.line_to_char(row) + this.line_len(row);
                    }
                    s.collapse_to(cursor, SelectionGoal::None);
                    s
                })
                .collect();
            this.select(next);
        });
    }

    fn insert_extra_newline(
        &self,
        range: Range<usize>,
        language: &LanguageConfig,
        scope_at: &dyn Fn(usize) -> Scope,
    ) -> bool {
        let inline_blank = |c: &char| c.is_whitespace() && *c != '\n';
        let mut start = range.start;
        while self.char_before(start).is_some_and(|c| inline_blank(&c)) {
            start -= 1;
        }
        let mut end = range.end;
        while self.char_at(end).is_some_and(|c| inline_blank(&c)) {
            end += 1;
        }
        let scope = scope_at(self.byte_of(range.start));
        language.brackets.iter().any(|pair| {
            let pair_start = pair.start.trim_end();
            let pair_end = pair.end.trim_start();
            let start_chars = pair_start.chars().count();
            pair.enabled_in(scope)
                && pair.newline
                && self.contains_str_at(end, pair_end)
                && start >= start_chars
                && self.contains_str_at(start - start_chars, pair_start)
        })
    }

    /// The line comment marker to repeat on the new line, when the cursor is past this line's marker.
    fn comment_delimiter_for_newline(
        &self,
        line: usize,
        column: usize,
        language: &LanguageConfig,
    ) -> Option<&'static str> {
        let max_len = language.line_comments.iter().map(|d| d.len()).max()?;
        let whitespace = self
            .rope
            .line(line)
            .chars()
            .take_while(|c| c.is_whitespace() && *c != '\n')
            .count();
        let candidate: String = self
            .rope
            .line(line)
            .chars()
            .skip(whitespace)
            .take(max_len + 2)
            .collect();
        let (delimiter, prefix_len) = language
            .line_comments
            .iter()
            .filter_map(|delimiter| {
                let prefix = delimiter.trim_end();
                candidate
                    .starts_with(prefix)
                    .then(|| (*delimiter, prefix.chars().count()))
            })
            .max_by_key(|(_, len)| *len)?;
        if let Some((block_start, _)) = language.block_comment {
            if block_start.starts_with(delimiter.trim_end()) && candidate.starts_with(block_start) {
                return None;
            }
        }
        (whitespace + prefix_len <= column).then_some(delimiter)
    }

    // ---- indentation and comments ----

    /// Re-resolve selections after `applied` the way anchored selections move: a start sticks after text
    /// inserted at it, and so does a caret, while a non-empty selection's end stays before it.
    fn remap_anchored(&mut self, applied: &[Edit]) {
        let next: Vec<Selection> = self
            .selections
            .iter()
            .map(|s| {
                let mut next = *s;
                let end_bias = if s.is_empty() {
                    Bias::Right
                } else {
                    Bias::Left
                };
                next.start = map_offset(applied, s.start, Bias::Right);
                next.end = map_offset(applied, s.end, end_bias);
                next
            })
            .collect();
        self.select(next);
    }

    fn remap_shifted(&mut self, applied: &[Edit]) {
        let next: Vec<Selection> = self
            .selections
            .iter()
            .map(|s| {
                let mut next = *s;
                next.start = map_offset(applied, s.start, Bias::Right);
                next.end = map_offset(applied, s.end, Bias::Right);
                next.goal = SelectionGoal::None;
                next
            })
            .collect();
        self.select(next);
    }

    fn indent_is_tabs(&self, line: usize) -> bool {
        self.rope.line(line).chars().next() == Some('\t')
    }

    /// Rows a selection covers for line-wise commands; a multi-row selection ending at column 0 leaves that
    /// row out.
    fn selected_rows(&self, selection: &Selection) -> (usize, usize) {
        let (start_row, _) = self.line_col_of(selection.start);
        let (mut end_row, end_col) = self.line_col_of(selection.end);
        if end_row > start_row && end_col == 0 {
            end_row -= 1;
        }
        (start_row, end_row)
    }

    /// Indent the selected rows one level (to the next tab stop), editing each row once.
    fn indent_edits(
        &self,
        selection: &Selection,
        indented: &mut Vec<usize>,
    ) -> Vec<(Range<usize>, String)> {
        let (start_row, end_row) = self.selected_rows(selection);
        let multiple_rows = start_row != end_row;
        let start_col = self.line_col_of(selection.start).1;
        let mut edits = Vec::new();
        for row in start_row..=end_row {
            if indented.contains(&row) {
                continue;
            }
            indented.push(row);
            let current = self.indent_len(row);
            let delta = if self.indent_is_tabs(row) {
                " ".repeat(TAB_SIZE)
            } else {
                " ".repeat(TAB_SIZE - current % TAB_SIZE)
            };
            let column = if multiple_rows || current < start_col {
                0
            } else {
                start_col
            };
            let at = self.rope.line_to_char(row) + column;
            edits.push((at..at, delta));
        }
        edits
    }

    /// Tab: carets insert spaces to the next tab stop; selections indent their rows.
    pub fn tab(&mut self) {
        let mut edits = Vec::new();
        let mut indented = Vec::new();
        let (mut delta_row, mut row_delta) = (usize::MAX, 0usize);
        for selection in &self.selections {
            if !selection.is_empty() {
                edits.extend(self.indent_edits(selection, &mut indented));
                continue;
            }
            let (row, column) = self.line_col_of(selection.head());
            if row != delta_row {
                delta_row = row;
                row_delta = 0;
            }
            let line_start = self.rope.line_to_char(row);
            let remainder = self
                .rope
                .slice(line_start..line_start + column)
                .chars()
                .fold(row_delta % TAB_SIZE, |count, c| {
                    if c == '\t' {
                        0
                    } else {
                        (count + 1) % TAB_SIZE
                    }
                });
            let spaces = TAB_SIZE - remainder;
            edits.push((selection.head()..selection.head(), " ".repeat(spaces)));
            row_delta += spaces;
        }
        self.transact(|this| {
            let applied = this.edit(edits);
            this.remap_shifted(&applied);
        });
    }

    /// Indent every selected row one level.
    pub fn indent(&mut self) {
        let mut edits = Vec::new();
        let mut indented = Vec::new();
        for selection in &self.selections {
            edits.extend(self.indent_edits(selection, &mut indented));
        }
        self.transact(|this| {
            let applied = this.edit(edits);
            this.remap_shifted(&applied);
        });
    }

    /// The indentation `outdent` removes: back to the previous tab stop on every row the selections span
    /// (through folds), each row once.
    pub fn outdent_ranges(&self, rows: &dyn DisplayRows) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        let mut last_outdent = None;
        for selection in &self.selections {
            let end = {
                let (start_row, _) = self.line_col_of(selection.start);
                let (end_row, end_col) = self.line_col_of(selection.end);
                if start_row != end_row && end_col == 0 {
                    self.rope.line_to_char(end_row).saturating_sub(1)
                } else {
                    selection.end
                }
            };
            let mut first_row = self.rope.char_to_line(rows.line_start(selection.start));
            let last_row = self.rope.char_to_line(rows.line_end(end));
            if last_outdent == Some(first_row) {
                first_row += 1;
            }
            let multiple_rows = last_row > first_row;
            let start_col = self.line_col_of(selection.start).1;
            for row in first_row..=last_row {
                let indent = self.indent_len(row);
                if indent == 0 {
                    continue;
                }
                let deletion = if self.indent_is_tabs(row) {
                    1
                } else if indent.is_multiple_of(TAB_SIZE) {
                    TAB_SIZE
                } else {
                    indent % TAB_SIZE
                };
                let column = if multiple_rows || deletion > start_col || indent < start_col {
                    0
                } else {
                    start_col - deletion
                };
                let at = self.rope.line_to_char(row) + column;
                ranges.push(at..at + deletion);
                last_outdent = Some(row);
            }
        }
        ranges
    }

    pub fn delete_ranges(&mut self, ranges: Vec<Range<usize>>) {
        let edits = ranges.into_iter().map(|r| (r, String::new())).collect();
        self.transact(|this| {
            let applied = this.edit(edits);
            this.remap_anchored(&applied);
        });
    }

    pub fn outdent(&mut self) {
        let ranges = self.outdent_ranges(&LineRows(self));
        self.delete_ranges(ranges);
    }

    /// The range a comment marker occupies at `row`'s indentation (plus following whitespace matching the
    /// marker's own), or an empty range there when the row doesn't start with it.
    fn comment_prefix_range(
        &self,
        row: usize,
        prefix: &str,
        prefix_whitespace: &str,
    ) -> Range<usize> {
        let start = self.rope.line_to_char(row) + self.indent_len(row);
        if !self.contains_str_at(start, prefix) {
            return start..start;
        }
        let after = start + prefix.chars().count();
        let matching = self
            .rope
            .chars_at(after.min(self.rope.len_chars()))
            .zip(prefix_whitespace.chars())
            .take_while(|(a, b)| a == b)
            .count();
        start..after + matching
    }

    fn comment_suffix_range(&self, row: usize, suffix: &str, leading_space: bool) -> Range<usize> {
        let end = self.rope.line_to_char(row) + self.line_len(row);
        let suffix_len = suffix.chars().count();
        let line_start = self.rope.line_to_char(row);
        let suffix_start = end.saturating_sub(suffix_len).max(line_start);
        if end - line_start < suffix_len || !self.contains_str_at(suffix_start, suffix) {
            return end..end;
        }
        let space = usize::from(
            leading_space && suffix_start > line_start && self.rope.char(suffix_start - 1) == ' ',
        );
        suffix_start - space..end
    }

    fn is_line_blank(&self, row: usize) -> bool {
        self.rope.line(row).chars().all(char::is_whitespace)
    }

    /// Cmd+/: comment the selected rows with the language's line comment, aligned at the least-indented row,
    /// or uncomment them when every non-blank row already is. Languages with only block comments wrap the
    /// rows instead.
    pub fn toggle_comments(&mut self, language: &LanguageConfig) {
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let mut suffixes_inserted: Vec<(usize, usize)> = Vec::new();
        let mut last_toggled_row = None;
        for selection in &self.selections {
            let (mut start_row, end_row) = self.selected_rows(selection);
            if last_toggled_row == Some(start_row) {
                start_row += 1;
            }
            last_toggled_row = Some(end_row);
            if start_row > end_row {
                continue;
            }
            if let Some(first_prefix) = language.line_comments.first() {
                let mut ranges = Vec::new();
                let (mut commented, mut uncommented) = (0, 0);
                for row in start_row..=end_row {
                    let is_blank = start_row < end_row && self.is_line_blank(row);
                    let range = language
                        .line_comments
                        .iter()
                        .map(|prefix| {
                            let trimmed = prefix.trim_end_matches(' ');
                            self.comment_prefix_range(row, trimmed, &prefix[trimmed.len()..])
                        })
                        .max_by_key(|range| range.len())
                        .unwrap_or(0..0);
                    // Blank rows never carry a marker, so they don't count towards "all commented".
                    if !is_blank {
                        if range.is_empty() {
                            uncommented += 1;
                        } else {
                            commented += 1;
                        }
                    }
                    ranges.push((row, range));
                }
                if uncommented == 0 && commented > 0 {
                    edits.extend(ranges.into_iter().map(|(_, range)| (range, String::new())));
                } else {
                    let min_column = ranges
                        .iter()
                        .map(|(row, range)| range.start - self.rope.line_to_char(*row))
                        .min()
                        .unwrap_or(0);
                    edits.extend(ranges.into_iter().map(|(row, _)| {
                        let at = self.rope.line_to_char(row) + min_column;
                        (at..at, first_prefix.to_string())
                    }));
                }
            } else if let Some((block_start, block_end)) = language.block_comment {
                let prefix = block_start.trim_end_matches(' ');
                let prefix_range =
                    self.comment_prefix_range(start_row, prefix, &block_start[prefix.len()..]);
                let suffix_range = self.comment_suffix_range(
                    end_row,
                    block_end.trim_start_matches(' '),
                    block_end.starts_with(' '),
                );
                if prefix_range.is_empty() || suffix_range.is_empty() {
                    edits.push((
                        prefix_range.start..prefix_range.start,
                        block_start.to_string(),
                    ));
                    edits.push((suffix_range.end..suffix_range.end, block_end.to_string()));
                    suffixes_inserted.push((end_row, block_end.chars().count()));
                } else {
                    edits.push((prefix_range, String::new()));
                    edits.push((suffix_range, String::new()));
                }
            }
        }
        self.transact(|this| {
            let applied = this.edit(edits);
            this.remap_anchored(&applied);
            // A selection ending at a row's end stays before the comment suffix just added there.
            let mut next = this.selections.clone();
            for s in &mut next {
                let (row, _) = this.line_col_of(s.end);
                let Some((_, suffix_len)) = suffixes_inserted.iter().find(|(r, _)| *r == row)
                else {
                    continue;
                };
                if s.end == this.rope.line_to_char(row) + this.line_len(row) {
                    if s.is_empty() {
                        s.start -= suffix_len;
                    }
                    s.end -= suffix_len;
                }
            }
            this.select(next);
        });
    }

    // ---- line operations ----

    /// Buffer rows `[start, end)` a selection covers for line-wise commands, widened to whole display lines
    /// (through folds). A multi-row selection ending at column 0 leaves that row out.
    fn spanned_rows(&self, selection: &Selection, rows: &dyn DisplayRows) -> Range<usize> {
        let (start_row, _) = self.line_col_of(selection.start);
        let (end_row, end_col) = self.line_col_of(selection.end);
        let end = if start_row != end_row && end_col == 0 {
            self.rope.line_to_char(end_row - 1)
        } else {
            selection.end
        };
        let first = self.rope.char_to_line(rows.line_start(selection.start));
        let last = self.rope.char_to_line(rows.line_end(end));
        first..last + 1
    }

    fn line_end_offset(&self, row: usize) -> usize {
        self.rope.line_to_char(row) + self.line_len(row)
    }

    fn max_row(&self) -> usize {
        self.rope.len_lines().saturating_sub(1)
    }

    /// Cmd+Shift+K: delete the selected lines; each caret lands on the following line at its old x.
    pub fn plan_delete_lines(&self, rows: &dyn DisplayRows) -> LinePlan {
        let mut plan = LinePlan::default();
        let mut cursors = Vec::new();
        let mut index = 0;
        while index < self.selections.len() {
            let selection = self.selections[index];
            let mut span = self.spanned_rows(&selection, rows);
            index += 1;
            while let Some(next) = self.selections.get(index) {
                let next_span = self.spanned_rows(next, rows);
                if next_span.start > span.end {
                    break;
                }
                span.end = next_span.end;
                index += 1;
            }
            let mut start = self.rope.line_to_char(span.start);
            let (end, target_row) = if self.max_row() >= span.end {
                (self.rope.line_to_char(span.end), span.end)
            } else {
                start = start.saturating_sub(1);
                (self.rope.len_chars(), span.start.saturating_sub(1))
            };
            let x = rows.x_of(selection.head());
            let display_row = rows.row_of(self.rope.line_to_char(target_row));
            cursors.push((selection.id, rows.offset_for_x(display_row, x)));
            plan.edits.push((start..end, String::new()));
        }
        plan.after = AfterLineEdit::Cursors(cursors);
        plan
    }

    /// Alt+Shift+Up/Down: copy the selected lines above or below them; with `whole_lines` false, a non-empty
    /// selection duplicates just its text after itself.
    pub fn plan_duplicate(
        &self,
        upwards: bool,
        whole_lines: bool,
        rows: &dyn DisplayRows,
    ) -> LinePlan {
        let mut plan = LinePlan::default();
        let mut shifts = Vec::new();
        let mut index = 0;
        while index < self.selections.len() {
            let selection = self.selections[index];
            index += 1;
            if !(whole_lines || selection.is_empty()) {
                let text = self.rope.slice(selection.start..selection.end).to_string();
                plan.edits.push((selection.end..selection.end, text));
                continue;
            }
            let mut span = self.spanned_rows(&selection, rows);
            let mut group = vec![selection.id];
            while let Some(next) = self.selections.get(index) {
                let next_span = self.spanned_rows(next, rows);
                if next_span.start >= span.end {
                    break;
                }
                span.end = next_span.end;
                group.push(next.id);
                index += 1;
            }
            let start = self.rope.line_to_char(span.start);
            let end = self.line_end_offset(span.end - 1);
            let mut text = self.rope.slice(start..end).to_string();
            let at = if upwards {
                let last_line_unterminated = span.end > self.max_row()
                    && self.line_len(self.max_row()) > 0
                    && !text.ends_with('\n');
                if last_line_unterminated {
                    text.insert(0, '\n');
                    end
                } else {
                    text.push('\n');
                    start
                }
            } else {
                text.push('\n');
                start
            };
            plan.edits.push((at..at, text));
            if upwards && whole_lines {
                let count = span.end - span.start;
                shifts.extend(group.into_iter().map(|id| (id, -(count as isize))));
            }
        }
        plan.after = AfterLineEdit::Anchored { row_shifts: shifts };
        plan
    }

    fn contiguous_rows(&self, from: usize, rows: &dyn DisplayRows) -> (usize, usize, usize) {
        let starting_row = |s: &Selection| {
            let (row, column) = self.line_col_of(s.start);
            if column > 0 {
                self.rope.char_to_line(rows.line_start(s.start))
            } else {
                row
            }
        };
        let ending_row = |s: &Selection| {
            let (row, column) = self.line_col_of(s.end);
            if column > 0 || s.is_empty() {
                self.rope.char_to_line(rows.line_end(s.end)) + 1
            } else {
                row
            }
        };
        let first = self.selections[from];
        let start_row = starting_row(&first);
        let mut end_row = ending_row(&first);
        let mut next = from + 1;
        while let Some(selection) = self.selections.get(next) {
            if self.line_col_of(selection.start).0 > end_row {
                break;
            }
            end_row = ending_row(selection);
            next += 1;
        }
        (start_row, end_row, next)
    }

    /// Alt+Up/Down: swap the selected lines with the display line above or below, carrying the selections
    /// (and, via `moved`, any folds) along.
    pub fn plan_move_lines(&self, up: bool, rows: &dyn DisplayRows) -> LinePlan {
        let mut plan = LinePlan::default();
        let mut moved_ids: Vec<(usize, isize)> = Vec::new();
        let mut index = 0;
        while index < self.selections.len() {
            let (start_row, end_row, next) = self.contiguous_rows(index, rows);
            let group: Vec<usize> = self.selections[index..next].iter().map(|s| s.id).collect();
            index = next;
            let (range, insertion, text, delta) = if up {
                if start_row == 0 {
                    continue;
                }
                let range = self.line_end_offset(start_row - 1)..self.line_end_offset(end_row - 1);
                let insertion = rows.line_start(self.rope.line_to_char(start_row - 1));
                let text: String = self
                    .rope
                    .slice(range.clone())
                    .chars()
                    .skip(1)
                    .chain(['\n'])
                    .collect();
                let delta = start_row - 1 - self.rope.char_to_line(insertion) + 1;
                (range, insertion, text, -(delta as isize))
            } else {
                if end_row > self.max_row() {
                    continue;
                }
                let range = self.rope.line_to_char(start_row)..self.rope.line_to_char(end_row);
                let insertion = rows.line_end(self.rope.line_to_char(end_row));
                let mut text = String::from("\n");
                text.push_str(&self.rope.slice(range.clone()).to_string());
                text.pop();
                let delta = self.rope.char_to_line(insertion) - end_row + 1;
                (range, insertion, text, delta as isize)
            };
            plan.edits.push((range.clone(), String::new()));
            plan.edits.push((insertion..insertion, text));
            plan.moved.push((range, delta));
            moved_ids.extend(group.into_iter().map(|id| (id, delta)));
        }
        let points = self
            .selections
            .iter()
            .map(|s| {
                let delta = moved_ids
                    .iter()
                    .find(|(id, _)| *id == s.id)
                    .map_or(0, |(_, d)| *d);
                let shift = |offset: usize| {
                    let (row, column) = self.line_col_of(offset);
                    ((row as isize + delta).max(0) as usize, column)
                };
                (*s, shift(s.start), shift(s.end))
            })
            .collect();
        plan.after = AfterLineEdit::Points(points);
        plan
    }

    /// Apply a plan from one of the `plan_*` line commands as a single undo step.
    pub fn apply_line_plan(&mut self, plan: LinePlan) {
        let before = self.selections.clone();
        let position = |this: &Self, delta: isize, offset: usize| {
            let (row, column) = this.line_col_of(offset);
            let row = (row as isize + delta).clamp(0, this.max_row() as isize) as usize;
            this.offset_at(row, column)
        };
        self.transact(|this| match plan.after {
            AfterLineEdit::Cursors(cursors) => {
                let applied = this.edit(plan.edits);
                let next = cursors
                    .into_iter()
                    .map(|(id, at)| {
                        let at = map_offset(&applied, at, Bias::Right);
                        let mut s = before
                            .iter()
                            .find(|s| s.id == id)
                            .copied()
                            .unwrap_or_default();
                        s.id = id;
                        s.collapse_to(at, SelectionGoal::None);
                        s
                    })
                    .collect();
                this.select(next);
            }
            AfterLineEdit::Anchored { row_shifts } => {
                let applied = this.edit(plan.edits);
                this.remap_anchored(&applied);
                let next = this
                    .selections
                    .iter()
                    .map(|s| {
                        let delta = row_shifts
                            .iter()
                            .find(|(id, _)| *id == s.id)
                            .map_or(0, |(_, d)| *d);
                        let mut s = *s;
                        if delta != 0 {
                            s.start = position(this, delta, s.start);
                            s.end = position(this, delta, s.end);
                        }
                        s
                    })
                    .collect();
                this.select(next);
            }
            AfterLineEdit::Points(points) => {
                this.edit(plan.edits);
                let next = points
                    .into_iter()
                    .map(|(mut s, (start_row, start_col), (end_row, end_col))| {
                        s.start = this.offset_at(start_row, start_col);
                        s.end = this.offset_at(end_row, end_col);
                        s
                    })
                    .collect();
                this.select(next);
            }
        });
    }

    /// Ctrl+J: join each selected line (a caret joins with the next) with the following one, replacing the
    /// break, the next line's indentation and a leading line-comment marker with a single space.
    pub fn join_lines(&mut self, language: &LanguageConfig) {
        let mut row_ranges: Vec<Range<usize>> = Vec::new();
        for selection in &self.selections {
            let (start, _) = self.line_col_of(selection.start);
            let (end_row, end_col) = self.line_col_of(selection.end);
            let end = if start == end_row {
                start + 1
            } else if end_col == 0 {
                if start + 1 == end_row {
                    end_row
                } else {
                    end_row - 1
                }
            } else {
                end_row
            };
            match row_ranges.last_mut() {
                Some(last) if start <= last.end => last.end = end,
                _ => row_ranges.push(start..end),
            }
        }
        let cursors: Vec<usize> = row_ranges
            .iter()
            .map(|r| self.line_end_offset(r.end.saturating_sub(1).min(self.max_row())))
            .collect();
        let mut edits = Vec::new();
        for range in &row_ranges {
            for row in range.clone() {
                let next_row = row + 1;
                if next_row > self.max_row() {
                    continue;
                }
                let indent = self.indent_len(next_row);
                let next_start = self.rope.line_to_char(next_row);
                let after_indent: String = self
                    .rope
                    .slice(next_start + indent..self.line_end_offset(next_row))
                    .chars()
                    .collect();
                let mut join_column = indent;
                if !after_indent.is_empty() {
                    let prefix_len = language
                        .line_comments
                        .iter()
                        .filter_map(|prefix| {
                            let trimmed = prefix.trim_end();
                            if after_indent.starts_with(prefix) {
                                Some(prefix.chars().count())
                            } else if after_indent.starts_with(trimmed) {
                                Some(trimmed.chars().count())
                            } else {
                                None
                            }
                        })
                        .max();
                    join_column += prefix_len.unwrap_or(0);
                }
                let replace = if self.line_len(next_row) > join_column {
                    " "
                } else {
                    ""
                };
                edits.push((
                    self.line_end_offset(row)..next_start + join_column,
                    replace.to_string(),
                ));
            }
        }
        let ids: Vec<Selection> = self.selections.clone();
        self.transact(|this| {
            let applied = this.edit(edits);
            let next = cursors
                .into_iter()
                .zip(ids)
                .map(|(at, mut s)| {
                    s.collapse_to(map_offset(&applied, at, Bias::Left), SelectionGoal::None);
                    s
                })
                .collect();
            this.select(next);
        });
    }

    /// Ctrl+T: swap the chars around each caret (at a line end, the two before it) and step past them.
    pub fn transpose(&mut self) {
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let mut next = self.selections.clone();
        for s in &mut next {
            if !s.is_empty() {
                continue;
            }
            let head = s.head();
            let (row, column) = self.line_col_of(head);
            let mut offset = head;
            if column == self.line_len(row) {
                offset = offset.saturating_sub(1);
            }
            if offset == 0 {
                continue;
            }
            let caret = (head + 1).min(self.line_end_offset(row).max(head));
            s.collapse_to(caret, SelectionGoal::None);
            let start = offset - 1;
            if edits.last().is_none_or(|(range, _)| range.end <= start) {
                let end = (offset + 1).min(self.rope.len_chars());
                let ch = self.rope.char(start);
                edits.push((start..offset, String::new()));
                edits.push((end..end, ch.to_string()));
            }
        }
        self.transact(|this| {
            this.select(next);
            let applied = this.edit(edits);
            this.remap_anchored(&applied);
        });
    }

    // ---- clipboard ----

    /// Cmd+C: the selected text, where an empty selection copies its whole line (with its newline). Pieces
    /// from several selections are joined with newlines; the slices record how to split them again.
    pub fn copy(&self) -> (String, Vec<ClipboardSelection>) {
        let mut text = String::new();
        let mut slices = Vec::with_capacity(self.selections.len());
        let mut previous_was_entire_line = false;
        for (index, selection) in self.selections.iter().enumerate() {
            let is_entire_line = selection.is_empty();
            let (mut start, mut end) = (selection.start, selection.end);
            let mut trailing_newline = false;
            if is_entire_line {
                let (row, _) = self.line_col_of(start);
                start = self.rope.line_to_char(row);
                if row < self.max_row() {
                    end = self.rope.line_to_char(row + 1);
                } else {
                    end = self.line_end_offset(row);
                    trailing_newline = true;
                }
            }
            if index > 0 && !previous_was_entire_line {
                text.push('\n');
            }
            let piece = self.rope.slice(start..end).to_string();
            let mut len = piece.chars().count();
            text.push_str(&piece);
            if trailing_newline {
                text.push('\n');
                len += 1;
            }
            previous_was_entire_line = is_entire_line;
            slices.push(ClipboardSelection {
                len,
                is_entire_line,
                first_line_indent: self.indent_len(self.line_col_of(start).0),
            });
        }
        (text, slices)
    }

    /// Cmd+X: like `copy`, then delete what was copied; an empty selection cuts its whole line.
    pub fn cut(&mut self) -> (String, Vec<ClipboardSelection>) {
        let mut text = String::new();
        let mut slices = Vec::with_capacity(self.selections.len());
        let mut previous_was_entire_line = false;
        let mut cut = self.selections.clone();
        for (index, selection) in cut.iter_mut().enumerate() {
            let is_entire_line = selection.is_empty();
            if is_entire_line {
                let (row, _) = self.line_col_of(selection.start);
                selection.start = self.rope.line_to_char(row);
                selection.end = if row < self.max_row() {
                    self.rope.line_to_char(row + 1)
                } else {
                    self.rope.len_chars()
                };
                selection.goal = SelectionGoal::None;
            }
            if index > 0 && !previous_was_entire_line {
                text.push('\n');
            }
            previous_was_entire_line = is_entire_line;
            let piece = self.rope.slice(selection.start..selection.end).to_string();
            text.push_str(&piece);
            slices.push(ClipboardSelection {
                len: piece.chars().count(),
                is_entire_line,
                first_line_indent: self.indent_len(self.line_col_of(selection.start).0),
            });
        }
        self.transact(|this| {
            this.select(cut);
            this.insert_text("");
        });
        (text, slices)
    }

    /// Cmd+V. With slices from our own copy (and one per selection), each selection gets its own piece, and a
    /// whole-line piece pasted at a caret goes in above the caret's line. Without them, text whose line count
    /// matches the selections is spread one line per selection; otherwise every selection gets all of it.
    pub fn paste(&mut self, text: &str, slices: Option<&[ClipboardSelection]>) {
        self.finalize_last_transaction();
        let text = normalize_newlines(text);
        let chars: Vec<char> = text.chars().collect();
        let piece = |range: Range<usize>| -> String { chars[range].iter().collect() };
        let mut edits = Vec::with_capacity(self.selections.len());
        match slices {
            Some(slices) => {
                let all_entire_lines = slices.iter().all(|s| s.is_entire_line);
                let per_selection = slices.len() == self.selections.len();
                let mut start = 0usize;
                for (index, selection) in self.selections.iter().enumerate() {
                    let (to_insert, entire_line) = match slices.get(index).filter(|_| per_selection)
                    {
                        Some(slice) => {
                            let end = (start + slice.len).min(chars.len());
                            let to_insert = piece(start.min(end)..end);
                            start = if slice.is_entire_line { end } else { end + 1 };
                            (to_insert, slice.is_entire_line)
                        }
                        None => (text.to_string(), all_entire_lines),
                    };
                    let range = if selection.is_empty() && entire_line {
                        let line_start = self.line_start_of(selection.start);
                        line_start..line_start
                    } else {
                        selection.start..selection.end
                    };
                    edits.push((range, to_insert));
                }
            }
            None => {
                let lines: Vec<&str> = text.split('\n').collect();
                let distribute = self.selections.len() > 1 && lines.len() == self.selections.len();
                for (index, selection) in self.selections.iter().enumerate() {
                    let to_insert = if distribute {
                        lines[index].to_string()
                    } else {
                        text.to_string()
                    };
                    edits.push((selection.start..selection.end, to_insert));
                }
            }
        }
        self.transact(|this| {
            let applied = this.edit(edits);
            this.remap_anchored(&applied);
        });
    }

    // ---- text/line transforms ----

    /// Transform each selection's text (a caret transforms its surrounding word); the selections then cover
    /// the results.
    pub fn manipulate_text(&mut self, transform: TextTransform) {
        let mut edits = Vec::new();
        let mut ranges = Vec::new();
        for selection in &self.selections {
            let range = if selection.is_empty() {
                self.surrounding_word(selection.start)
            } else {
                selection.start..selection.end
            };
            let old = self.rope.slice(range.clone()).to_string();
            let new = crate::transform::apply_text(transform, &old);
            ranges.push((*selection, range.clone()));
            if new != old {
                edits.push((range, new));
            }
        }
        if edits.is_empty() {
            return;
        }
        self.transact(|this| {
            let applied = this.edit(edits);
            let next = ranges
                .into_iter()
                .map(|(mut s, range)| {
                    s.start = map_offset(&applied, range.start, Bias::Left);
                    s.end = map_offset(&applied, range.end, Bias::Right);
                    s.goal = SelectionGoal::None;
                    s
                })
                .collect();
            this.select(next);
        });
    }

    /// Rewrite the whole lines the selections span; each contiguous group then selects its resulting lines.
    pub fn plan_manipulate_lines(
        &self,
        transform: LineTransform,
        rows: &dyn DisplayRows,
    ) -> LinePlan {
        let mut plan = LinePlan::default();
        let mut points = Vec::new();
        let (mut added, mut removed) = (0usize, 0usize);
        let mut index = 0;
        while index < self.selections.len() {
            let (start_row, end_row, next) = self.contiguous_rows(index, rows);
            let first = self.selections[index];
            index = next;
            let start = self.rope.line_to_char(start_row);
            let end = self.line_end_offset(end_row - 1);
            let text = self.rope.slice(start..end).to_string();
            let mut lines: Vec<&str> = text.split('\n').collect();
            let before = lines.len();
            crate::transform::apply_lines(transform, &mut lines);
            let after = lines.len();
            plan.edits.push((start..end, lines.join("\n")));
            let new_start = start_row + added - removed;
            let new_end = new_start + after.saturating_sub(1);
            points.push((first, (new_start, 0), (new_end, usize::MAX)));
            if after > before {
                added += after - before;
            } else {
                removed += before - after;
            }
        }
        plan.after = AfterLineEdit::Points(points);
        plan
    }

    // ---- input method composition ----

    /// The ranges of text the input method is still composing (drawn underlined), one per selection.
    pub fn marked_ranges(&self) -> &[Range<usize>] {
        &self.marked_ranges
    }

    fn select_marked(&mut self) {
        if self.marked_ranges.is_empty() {
            return;
        }
        let next: Vec<Selection> = self
            .selections
            .iter()
            .zip(self.marked_ranges.clone())
            .map(|(s, range)| Selection {
                start: range.start,
                end: range.end,
                reversed: false,
                goal: SelectionGoal::None,
                ..*s
            })
            .collect();
        self.select(next);
    }

    /// Replace the composing text (or the selections, when composing starts) with `text` and mark it.
    /// `selected` is the caret/selection the input method wants inside `text`, in chars. Every step of one
    /// composition folds into a single undo entry.
    pub fn replace_and_mark_text(&mut self, text: &str, selected: Option<Range<usize>>) {
        let base = *self.ime_undo_base.get_or_insert(self.undo_stack.len());
        let len = text.chars().count();
        self.transact(|this| {
            this.select_marked();
            this.insert_text(text);
            let marks: Vec<Range<usize>> = if text.is_empty() {
                Vec::new()
            } else {
                this.selections
                    .iter()
                    .map(|s| s.head() - len..s.head())
                    .collect()
            };
            if let Some(selected) = selected.filter(|_| !marks.is_empty()) {
                let next: Vec<Selection> = this
                    .selections
                    .iter()
                    .zip(&marks)
                    .map(|(s, mark)| Selection {
                        start: mark.start + selected.start.min(len),
                        end: mark.start + selected.end.min(len),
                        reversed: false,
                        goal: SelectionGoal::None,
                        ..*s
                    })
                    .collect();
                this.select(next);
            }
            this.marked_ranges = marks;
        });
        self.merge_undo_since(base);
    }

    /// Finish a composition: the composing text becomes `text`, typed like normal input.
    pub fn commit_text(
        &mut self,
        text: &str,
        language: &LanguageConfig,
        scope_at: &dyn Fn(usize) -> Scope,
    ) {
        let base = self.ime_undo_base.take();
        self.transact(|this| {
            if this.marked_ranges.iter().any(|r| !r.is_empty()) {
                this.select_marked();
                this.insert_text("");
            }
            this.marked_ranges.clear();
            this.handle_input(text, language, scope_at);
        });
        if let Some(base) = base {
            self.merge_undo_since(base);
        }
    }

    /// Drop the composition state without touching the text.
    pub fn unmark_text(&mut self) {
        self.marked_ranges.clear();
        self.ime_undo_base = None;
    }

    fn merge_undo_since(&mut self, base: usize) {
        if self.undo_stack.len() <= base + 1 {
            return;
        }
        let merged: Vec<HistoryEntry> = self.undo_stack.drain(base + 1..).collect();
        if let Some(target) = self.undo_stack.get_mut(base) {
            for entry in merged {
                target.batches.extend(entry.batches);
                target.last_edit_at = entry.last_edit_at;
                target.selections_after = entry.selections_after;
            }
        }
    }


    pub fn search(&self, query: &SearchQuery) -> Vec<Range<usize>> {
        let text = self.rope.to_string();
        query
            .find_in(&text)
            .into_iter()
            .map(|r| self.rope.byte_to_char(r.start)..self.rope.byte_to_char(r.end))
            .collect()
    }

    pub fn query_suggestion(&self) -> String {
        let newest = self.newest();
        if !newest.is_empty() {
            return self.rope.slice(newest.start..newest.end).to_string();
        }
        let word = self.surrounding_word(newest.start);
        let text = self.rope.slice(word.clone()).to_string();
        let is_word = text
            .chars()
            .next()
            .is_some_and(|c| movement::char_kind(c) == movement::CharKind::Word);
        if is_word && !text.trim().is_empty() {
            text
        } else {
            String::new()
        }
    }

    pub fn select_ranges(&mut self, ranges: &[Range<usize>]) {
        let next: Vec<Selection> = ranges
            .iter()
            .map(|r| self.new_selection(r.start, r.end, false))
            .collect();
        self.select(next);
    }

    fn replacement_edit(
        &self,
        query: &SearchQuery,
        range: &Range<usize>,
    ) -> Option<(Range<usize>, String)> {
        let first = self.rope.char_to_line(range.start);
        let last = self.rope.char_to_line(range.end);
        let line_start = self.rope.line_to_char(first);
        let line_end = self.line_end_offset(last);
        let line = self
            .rope
            .slice(line_start..line_end.max(range.end))
            .to_string();
        let start_byte = self.rope.char_to_byte(line_start);
        let hit = self.rope.char_to_byte(range.start) - start_byte
            ..self.rope.char_to_byte(range.end) - start_byte;
        query
            .replacement_for(&line, hit)
            .map(|text| (range.clone(), text))
    }

    pub fn replace_match(&mut self, query: &SearchQuery, range: Range<usize>) {
        if let Some(edit) = self.replacement_edit(query, &range) {
            self.transact(|this| {
                let applied = this.edit(vec![edit]);
                this.remap_anchored(&applied);
            });
        }
    }

    pub fn replace_all(&mut self, query: &SearchQuery, ranges: &[Range<usize>]) {
        let edits: Vec<(Range<usize>, String)> = ranges
            .iter()
            .filter_map(|range| self.replacement_edit(query, range))
            .collect();
        if edits.is_empty() {
            return;
        }
        self.transact(|this| {
            let applied = this.edit(edits);
            this.remap_anchored(&applied);
        });
    }

    /// Leading whitespace of `line`, in chars.
    pub fn indent_len(&self, line: usize) -> usize {
        self.rope
            .line(line)
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count()
    }

    /// `Home`: a soft-wrapped row's start first (when past the indent), then the indentation, then the line
    /// start; pressing again at the indentation toggles to the line start and back.
    fn indented_line_beginning(
        &self,
        head: usize,
        stop_at_soft_wraps: bool,
        stop_at_indent: bool,
        rows: &dyn DisplayRows,
    ) -> usize {
        let soft_start = rows.row_start(rows.row_of(head));
        let line = self.rope.char_to_line(head.min(self.rope.len_chars()));
        let indent_start = self.rope.line_to_char(line) + self.indent_len(line);
        let line_start = rows.line_start(head);
        if stop_at_soft_wraps && soft_start > indent_start && head != soft_start {
            soft_start
        } else if stop_at_indent && (head > indent_start || head == line_start) {
            indent_start
        } else {
            line_start
        }
    }

    fn line_end_for(&self, head: usize, stop_at_soft_wraps: bool, rows: &dyn DisplayRows) -> usize {
        let soft_end = rows.row_end(rows.row_of(head));
        if stop_at_soft_wraps && head != soft_end {
            soft_end
        } else {
            rows.line_end(head)
        }
    }

    /// Selections grown over what `deletion` removes (empty selections only), for `delete_selections`.
    pub fn deletion_selections(
        &self,
        deletion: Deletion,
        rows: &dyn DisplayRows,
    ) -> Vec<Selection> {
        let len = self.rope.len_chars();
        self.selections
            .iter()
            .map(|s| {
                let mut s = *s;
                s.goal = SelectionGoal::None;
                match deletion {
                    Deletion::ToBeginningOfLine => {
                        s.reversed = true;
                        let head = s.head();
                        s.set_head(
                            self.indented_line_beginning(head, false, false, rows),
                            SelectionGoal::None,
                        );
                        if s.is_empty() {
                            s.set_head(self.backspace_target(s.head(), rows), SelectionGoal::None);
                        }
                    }
                    Deletion::ToEndOfLine => {
                        s.reversed = false;
                        let head = s.head();
                        s.set_head(self.line_end_for(head, false, rows), SelectionGoal::None);
                        if s.is_empty() {
                            s.set_head(
                                rows.clip((head + 1).min(len), Bias::Right),
                                SelectionGoal::None,
                            );
                        }
                    }
                    _ if !s.is_empty() => {}
                    Deletion::Backward
                    | Deletion::PreviousWordStart
                    | Deletion::PreviousSubwordStart
                        if self.autoclose_pair_around(&s).is_some() =>
                    {
                        if let Some(pair) = self.autoclose_pair_around(&s) {
                            s.start = pair.start;
                            s.end = pair.end;
                        }
                    }
                    Deletion::Backward => {
                        s.set_head(self.backspace_target(s.head(), rows), SelectionGoal::None);
                    }
                    Deletion::Forward => {
                        s.set_head(
                            rows.clip((s.head() + 1).min(len), Bias::Right),
                            SelectionGoal::None,
                        );
                    }
                    Deletion::PreviousWordStart | Deletion::PreviousSubwordStart => {
                        let head = s.head();
                        let target = if deletion == Deletion::PreviousWordStart {
                            movement::previous_word_start_or_newline(self, head)
                        } else {
                            movement::previous_subword_start(self, head)
                        };
                        let target = rows.clip(target, Bias::Left);
                        s.set_head(
                            movement::adjust_greedy_deletion(self, head, target),
                            SelectionGoal::None,
                        );
                    }
                    Deletion::NextWordEnd | Deletion::NextSubwordEnd => {
                        let head = s.head();
                        let target = if deletion == Deletion::NextWordEnd {
                            movement::next_word_end_or_newline(self, head)
                        } else {
                            movement::next_subword_end_or_newline(self, head)
                        };
                        let target = rows.clip(target, Bias::Right);
                        s.set_head(
                            movement::adjust_greedy_deletion(self, head, target),
                            SelectionGoal::None,
                        );
                    }
                }
                s
            })
            .collect()
    }

    /// Both brackets of an auto-closed pair when an empty selection sits right after the opener.
    fn autoclose_pair_around(&self, selection: &Selection) -> Option<Range<usize>> {
        let region = self.autoclose_region_for(selection)?;
        let start_len = region.pair.start.chars().count();
        (selection.start == region.range.start
            && region.range.start >= start_len
            && self.contains_str_at(region.range.start - start_len, region.pair.start)
            && self.contains_str_at(region.range.end, region.pair.end))
        .then(|| region.range.start - start_len..region.range.end + region.pair.end.chars().count())
    }

    /// One char left, but inside the indentation back to the previous tab stop.
    fn backspace_target(&self, head: usize, rows: &dyn DisplayRows) -> usize {
        let mut target = rows.clip(head.saturating_sub(1), Bias::Left);
        let (line, column) = self.line_col_of(head);
        let indent = self.indent_len(line);
        if column > 0 && column <= indent {
            let unit = match self.rope.line(line).chars().next() {
                Some('\t') => 1,
                _ => TAB_SIZE,
            };
            target = target.min(self.rope.line_to_char(line) + (column - 1) / unit * unit);
        }
        target
    }

    /// Delete the text under `grown` (from `deletion_selections`) as one undo step.
    pub fn delete_selections(&mut self, grown: Vec<Selection>) {
        self.transact(|this| {
            this.select(grown);
            this.insert_text("");
        });
    }

    fn delete_by(&mut self, deletion: Deletion) {
        let grown = self.deletion_selections(deletion, &LineRows(self));
        self.delete_selections(grown);
    }

    /// Selections after `motion` (or, with `extend`, grown by it), measured over `rows`.
    pub fn moved_selections(
        &self,
        motion: Motion,
        extend: bool,
        rows: &dyn DisplayRows,
    ) -> Vec<Selection> {
        let len = self.rope.len_chars();
        self.selections
            .iter()
            .map(|s| {
                let mut s = *s;
                let collapsing = !extend && !s.is_empty();
                let head = s.head();
                let (to, goal) = match motion {
                    Motion::Left if collapsing => (s.start, SelectionGoal::None),
                    Motion::Right if collapsing => (s.end, SelectionGoal::None),
                    Motion::Left => (
                        rows.clip(head.saturating_sub(1), Bias::Left),
                        SelectionGoal::None,
                    ),
                    Motion::Right => (
                        rows.clip((head + 1).min(len), Bias::Right),
                        SelectionGoal::None,
                    ),
                    Motion::WordLeft => (
                        rows.clip(movement::previous_word_start(self, head), Bias::Left),
                        SelectionGoal::None,
                    ),
                    Motion::WordRight => (
                        rows.clip(movement::next_word_end(self, head), Bias::Right),
                        SelectionGoal::None,
                    ),
                    Motion::SubwordLeft => (
                        rows.clip(movement::previous_subword_start(self, head), Bias::Left),
                        SelectionGoal::None,
                    ),
                    Motion::SubwordRight => (
                        rows.clip(movement::next_subword_end(self, head), Bias::Right),
                        SelectionGoal::None,
                    ),
                    Motion::DocumentStart => (0, SelectionGoal::None),
                    Motion::DocumentEnd => (len, SelectionGoal::None),
                    Motion::Home | Motion::LineStart => (
                        self.indented_line_beginning(head, motion == Motion::Home, true, rows),
                        SelectionGoal::None,
                    ),
                    Motion::End | Motion::LineEnd => (
                        self.line_end_for(head, motion == Motion::End, rows),
                        SelectionGoal::None,
                    ),
                    Motion::Up | Motion::Down | Motion::PageUp(_) | Motion::PageDown(_) => {
                        let (up, count, page) = match motion {
                            Motion::Up => (true, 1, false),
                            Motion::PageUp(count) => (true, count, true),
                            Motion::PageDown(count) => (false, count, true),
                            _ => (false, 1, false),
                        };
                        if collapsing {
                            s.goal = SelectionGoal::None;
                        }
                        // Page moves start from the selection end in both directions.
                        let from = match (collapsing, page, up) {
                            (true, true, _) | (true, false, false) => s.end,
                            (true, false, true) => s.start,
                            _ => head,
                        };
                        let goal_x = match s.goal {
                            SelectionGoal::HorizontalPosition(x) => x,
                            SelectionGoal::HorizontalRange { end, .. } => end,
                            SelectionGoal::None => rows.x_of(from),
                        };
                        let row = rows.row_of(from);
                        let to = if up {
                            if row == 0 {
                                0
                            } else {
                                rows.offset_for_x(row.saturating_sub(count), goal_x)
                            }
                        } else if row >= rows.max_row() {
                            len
                        } else {
                            rows.offset_for_x((row + count).min(rows.max_row()), goal_x)
                        };
                        (to, SelectionGoal::HorizontalPosition(goal_x))
                    }
                };
                if extend {
                    s.set_head(to, goal);
                } else {
                    s.collapse_to(to, goal);
                }
                s
            })
            .collect()
    }

    /// Replace the selections (merging any that now overlap).
    pub fn set_selections(&mut self, selections: Vec<Selection>) {
        self.select(selections);
    }

    /// Apply `motion` with one display row per buffer line.
    pub fn apply_motion(&mut self, motion: Motion, extend: bool) {
        let next = self.moved_selections(motion, extend, &LineRows(self));
        self.select(next);
    }

    pub fn move_left(&mut self) {
        self.apply_motion(Motion::Left, false);
    }
    pub fn move_right(&mut self) {
        self.apply_motion(Motion::Right, false);
    }
    pub fn move_up(&mut self) {
        self.apply_motion(Motion::Up, false);
    }
    pub fn move_down(&mut self) {
        self.apply_motion(Motion::Down, false);
    }
    pub fn move_word_left(&mut self) {
        self.apply_motion(Motion::WordLeft, false);
    }
    pub fn move_word_right(&mut self) {
        self.apply_motion(Motion::WordRight, false);
    }
    pub fn move_home(&mut self) {
        self.apply_motion(Motion::Home, false);
    }
    pub fn move_end(&mut self) {
        self.apply_motion(Motion::End, false);
    }

    pub fn extend_left(&mut self) {
        self.apply_motion(Motion::Left, true);
    }
    pub fn extend_right(&mut self) {
        self.apply_motion(Motion::Right, true);
    }
    pub fn extend_up(&mut self) {
        self.apply_motion(Motion::Up, true);
    }
    pub fn extend_down(&mut self) {
        self.apply_motion(Motion::Down, true);
    }
    pub fn extend_word_left(&mut self) {
        self.apply_motion(Motion::WordLeft, true);
    }
    pub fn extend_word_right(&mut self) {
        self.apply_motion(Motion::WordRight, true);
    }
    pub fn extend_home(&mut self) {
        self.apply_motion(Motion::Home, true);
    }
    pub fn extend_end(&mut self) {
        self.apply_motion(Motion::End, true);
    }

    #[cfg(test)]
    fn heads(&self) -> Vec<usize> {
        self.selections.iter().map(|s| s.head()).collect()
    }
    #[cfg(test)]
    fn ranges(&self) -> Vec<(usize, usize)> {
        self.selections.iter().map(|s| (s.start, s.end)).collect()
    }
}

fn should_merge(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    let overlapping = b_start < a_end;
    let same_start = a_start == b_start;
    let caret_at_boundary = a_end == b_end && (a_start == a_end || b_start == b_end);
    overlapping || same_start || caret_at_boundary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> EditorBuffer {
        let mut b = EditorBuffer::from_text(text);
        b.set_group_interval(Duration::ZERO);
        b
    }

    #[test]
    fn typing_and_undo() {
        let mut b = EditorBuffer::from_text("");
        for ch in "abc".chars() {
            b.insert_char(ch);
        }
        assert_eq!(b.text(), "abc");
        b.undo();
        assert_eq!(b.text(), "");
        b.redo();
        assert_eq!(b.text(), "abc");
    }

    #[test]
    fn undo_groups_by_interval() {
        let mut b = EditorBuffer::from_text("");
        let t0 = Instant::now();
        b.start_transaction_at(t0);
        b.insert_char('a');
        b.end_transaction_at(t0);
        b.start_transaction_at(t0 + Duration::from_millis(100));
        b.insert_char('b');
        b.end_transaction_at(t0 + Duration::from_millis(100));
        b.start_transaction_at(t0 + Duration::from_millis(1000));
        b.insert_char('c');
        b.end_transaction_at(t0 + Duration::from_millis(1000));
        b.undo();
        assert_eq!(b.text(), "ab");
        b.undo();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn finalize_breaks_grouping() {
        let mut b = EditorBuffer::from_text("");
        let t0 = Instant::now();
        b.start_transaction_at(t0);
        b.insert_char('a');
        b.end_transaction_at(t0);
        b.finalize_last_transaction();
        b.start_transaction_at(t0);
        b.insert_char('b');
        b.end_transaction_at(t0);
        b.undo();
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn undo_restores_selections() {
        let mut b = buffer("hello world");
        b.place_cursor(5);
        b.insert_text("!");
        b.place_cursor(0);
        b.undo();
        assert_eq!(b.text(), "hello world");
        assert_eq!(b.heads(), vec![5]);
        b.redo();
        assert_eq!(b.heads(), vec![6]);
    }

    #[test]
    fn add_cursor_types_at_all() {
        let mut b = buffer("a\nb\n");
        b.place_cursor(0);
        b.add_cursor(2);
        b.insert_char('X');
        assert_eq!(b.text(), "Xa\nXb\n");
        assert_eq!(b.heads(), vec![1, 4]);
    }

    #[test]
    fn select_next_occurrence() {
        let mut b = buffer("total = total + total");
        b.place_cursor(2);
        b.select_next(false);
        assert_eq!(b.ranges(), vec![(0, 5)]);
        b.select_next(false);
        b.select_next(false);
        assert_eq!(b.ranges().len(), 3);
        b.insert_text("sum");
        assert_eq!(b.text(), "sum = sum + sum");
    }

    #[test]
    fn overlapping_cursors_merge() {
        let mut b = buffer("hello");
        b.place_cursor(2);
        b.add_cursor(2);
        assert_eq!(b.selections().len(), 1);
    }

    #[test]
    fn touching_selections_stay_separate() {
        assert!(!should_merge(0, 3, 3, 6));
        assert!(should_merge(0, 3, 3, 3));
        assert!(should_merge(0, 3, 2, 6));
    }

    #[test]
    fn backspace_across_cursors_merges_deletions() {
        let mut b = buffer("ab");
        b.place_cursor(1);
        b.add_cursor(2);
        b.backspace();
        assert_eq!(b.text(), "");
        assert_eq!(b.heads(), vec![0]);
    }

    #[test]
    fn word_and_home_end() {
        let mut b = buffer("foo bar");
        b.place_cursor(0);
        b.move_word_right();
        assert_eq!(b.cursor(), 3);
        b.move_end();
        assert_eq!(b.cursor(), 7);
        b.move_home();
        assert_eq!(b.cursor(), 0);
    }

    #[test]
    fn vertical_move_keeps_goal_column() {
        let mut b = buffer("abcdefgh\nxy\nABCDEFGH");
        b.place_cursor(6);
        b.move_down();
        assert_eq!(b.line_col(), (1, 2));
        b.move_down();
        assert_eq!(b.line_col(), (2, 6));
        b.move_up();
        assert_eq!(b.line_col(), (1, 2));
        b.move_up();
        assert_eq!(b.line_col(), (0, 6));
    }

    #[test]
    fn horizontal_move_clears_goal_column() {
        let mut b = buffer("abcdefgh\nxy\nABCDEFGH");
        b.place_cursor(6);
        b.move_down();
        b.move_left();
        b.move_down();
        assert_eq!(b.line_col(), (2, 1));
    }

    #[test]
    fn escape_collapses_to_oldest() {
        let mut b = buffer("one two three");
        b.place_cursor(0);
        b.add_cursor(4);
        b.add_cursor(8);
        b.collapse_cursors();
        assert_eq!(b.heads(), vec![0]);
    }

    #[test]
    fn map_offset_follows_bias() {
        let edits = [Edit {
            old: 2..4,
            new: 2..5,
        }];
        assert_eq!(map_offset(&edits, 1, Bias::Right), 1);
        assert_eq!(map_offset(&edits, 2, Bias::Left), 2);
        assert_eq!(map_offset(&edits, 2, Bias::Right), 5);
        assert_eq!(map_offset(&edits, 3, Bias::Left), 5);
        assert_eq!(map_offset(&edits, 6, Bias::Left), 7);
    }

    #[test]
    fn edits_since_reports_batches() {
        let mut b = buffer("abc");
        let v = b.version();
        b.place_cursor(1);
        b.insert_text("XY");
        let batches: Vec<Vec<Edit>> = b.edits_since(v).map(|e| e.to_vec()).collect();
        assert_eq!(
            batches,
            vec![vec![Edit {
                old: 1..1,
                new: 1..3
            }]]
        );
    }

    #[test]
    fn dirty_tracks_undo_to_saved_point() {
        let mut b = buffer("x");
        assert!(!b.is_dirty());
        b.place_cursor(1);
        b.insert_char('y');
        assert!(b.is_dirty());
        b.mark_saved();
        assert!(!b.is_dirty());
        b.insert_char('z');
        assert!(b.is_dirty());
        b.undo();
        assert!(!b.is_dirty());
        b.undo();
        assert!(b.is_dirty());
        b.redo();
        assert!(!b.is_dirty());
    }

    #[test]
    fn line_endings_round_trip() {
        let b = EditorBuffer::from_text("a\r\nb\r\n");
        assert_eq!(b.text(), "a\nb\n");
        assert_eq!(b.text_for_save(), "a\r\nb\r\n");
        assert_eq!(LineEnding::detect("x\ny"), LineEnding::Unix);
    }

    #[test]
    fn syntax_edits_use_byte_positions() {
        let mut b = buffer("é\nx");
        let v = b.version();
        b.place_cursor(3);
        b.insert_char('y');
        let edit = *b.syntax_edits_since(v).next().unwrap();
        assert_eq!(edit.start_byte, 4);
        assert_eq!(edit.new_end_byte, 5);
        assert_eq!(edit.start_position, Point { row: 1, column: 1 });
    }

    #[test]
    fn page_moves_by_rows_and_stops_at_the_ends() {
        let text: String = (0..10).map(|i| format!("line{i}\n")).collect();
        let mut b = buffer(&text);
        b.place_cursor(2);
        b.apply_motion(Motion::PageDown(4), false);
        assert_eq!(b.line_col(), (4, 2));
        b.apply_motion(Motion::PageDown(20), false);
        assert_eq!(b.line_col(), (10, 0));
        b.apply_motion(Motion::PageUp(4), false);
        assert_eq!(b.line_col(), (6, 2));
        b.apply_motion(Motion::PageUp(20), true);
        assert_eq!(b.line_col(), (0, 2));
        assert_eq!(b.selected_text().as_deref().map(str::len), Some(6 * 6));
    }

    #[test]
    fn home_toggles_between_indent_and_line_start() {
        let mut b = buffer("    let x = 1;");
        b.place_cursor(10);
        b.move_home();
        assert_eq!(b.cursor(), 4);
        b.move_home();
        assert_eq!(b.cursor(), 0);
        b.move_home();
        assert_eq!(b.cursor(), 4);
    }

    #[test]
    fn backspace_in_indentation_removes_to_the_previous_tab_stop() {
        let mut b = buffer("      x");
        b.place_cursor(6);
        b.backspace();
        assert_eq!(b.text(), "    x");
        b.backspace();
        assert_eq!(b.text(), "x");
    }

    fn delete(b: &mut EditorBuffer, deletion: Deletion) {
        b.delete_by(deletion);
    }

    #[test]
    fn word_deletes_stop_at_brackets_and_newlines() {
        let mut b = buffer("foo(bar_baz)");
        b.place_cursor(11);
        delete(&mut b, Deletion::PreviousSubwordStart);
        assert_eq!(b.text(), "foo(bar_)");
        delete(&mut b, Deletion::PreviousWordStart);
        assert_eq!(b.text(), "foo()");
        let mut b = buffer("a\nb");
        b.place_cursor(2);
        delete(&mut b, Deletion::PreviousWordStart);
        assert_eq!(b.text(), "ab");
        let mut b = buffer("one two");
        b.place_cursor(0);
        delete(&mut b, Deletion::NextWordEnd);
        assert_eq!(b.text(), " two");
    }

    #[test]
    fn line_deletes_join_at_the_line_edge() {
        let mut b = buffer("ab\ncd");
        b.place_cursor(4);
        delete(&mut b, Deletion::ToBeginningOfLine);
        assert_eq!(b.text(), "ab\nd");
        delete(&mut b, Deletion::ToBeginningOfLine);
        assert_eq!(b.text(), "abd");
        b.place_cursor(0);
        delete(&mut b, Deletion::ToEndOfLine);
        assert_eq!(b.text(), "");
        b.undo();
        assert_eq!(b.text(), "abd");
    }

    fn rust() -> crate::language::LanguageConfig {
        crate::language::config(crate::highlight::Lang::Rust)
    }

    fn typed(b: &mut EditorBuffer, text: &str) {
        let language = rust();
        for ch in text.chars() {
            if ch == '\n' {
                b.newline(&language, &|_| Scope::default());
            } else {
                b.handle_input(&ch.to_string(), &language, &|_| Scope::default());
            }
        }
    }

    #[test]
    fn brackets_auto_close_and_step_over_their_closer() {
        let mut b = buffer("");
        typed(&mut b, "f(a");
        assert_eq!(b.text(), "f(a)");
        typed(&mut b, ")");
        assert_eq!(b.text(), "f(a)");
        assert_eq!(b.cursor(), 4);
    }

    #[test]
    fn no_auto_close_before_a_word() {
        let mut b = buffer("x");
        b.place_cursor(0);
        typed(&mut b, "(");
        assert_eq!(b.text(), "(x");
    }

    #[test]
    fn typed_closer_without_a_region_is_inserted() {
        let mut b = buffer("()");
        b.place_cursor(1);
        typed(&mut b, ")");
        assert_eq!(b.text(), "())");
    }

    #[test]
    fn backspace_after_an_auto_closed_opener_removes_both() {
        let mut b = buffer("");
        typed(&mut b, "[");
        b.backspace();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn typing_an_opener_over_a_selection_surrounds_it() {
        let mut b = buffer("value");
        b.select_all();
        typed(&mut b, "(");
        assert_eq!(b.text(), "(value)");
        assert_eq!(b.selected_text().as_deref(), Some("value"));
    }

    #[test]
    fn quotes_do_not_auto_close_after_a_word_or_as_a_closing_quote() {
        let mut b = buffer("");
        typed(&mut b, "a\"");
        assert_eq!(b.text(), "a\"");
        let mut b = buffer("\"x");
        b.place_cursor(2);
        typed(&mut b, " \"");
        assert_eq!(b.text(), "\"x \"");
    }

    #[test]
    fn newline_keeps_indent_and_splits_bracket_pairs() {
        let mut b = buffer("    fn a() {}");
        b.place_cursor(12);
        typed(&mut b, "\n");
        assert_eq!(b.text(), "    fn a() {\n    \n    }");
        assert_eq!(b.line_col(), (1, 4));
    }

    #[test]
    fn newline_continues_line_comments() {
        let mut b = buffer("  // note");
        b.place_cursor(9);
        typed(&mut b, "\n");
        assert_eq!(b.text(), "  // note\n  // ");
        let mut b = buffer("  // note");
        b.place_cursor(1);
        typed(&mut b, "\n");
        assert_eq!(b.text(), " \n  // note");
    }

    #[test]
    fn newline_on_an_indent_only_line_drops_the_blanks() {
        let mut b = buffer("    ");
        b.place_cursor(4);
        typed(&mut b, "\n");
        assert_eq!(b.text(), "\n    ");
    }

    #[test]
    fn typing_at_touching_selections_puts_each_caret_after_its_text() {
        let mut b = buffer("ab");
        let selection = |id, start, end| Selection {
            id,
            start,
            end,
            reversed: false,
            goal: SelectionGoal::None,
        };
        b.set_selections(vec![selection(0, 0, 1), selection(1, 1, 2)]);
        typed(&mut b, "x");
        assert_eq!(b.text(), "xx");
        assert_eq!(b.heads(), vec![1, 2]);
    }

    #[test]
    fn tab_inserts_spaces_to_the_next_stop() {
        let mut b = buffer("ab");
        b.place_cursor(1);
        b.tab();
        assert_eq!(b.text(), "a   b");
        assert_eq!(b.cursor(), 4);
    }

    #[test]
    fn tab_and_outdent_shift_selected_rows() {
        let mut b = buffer("a\n  b\nc");
        b.select_all();
        b.tab();
        assert_eq!(b.text(), "    a\n    b\n    c");
        b.outdent();
        assert_eq!(b.text(), "a\nb\nc");
    }

    #[test]
    fn selection_ending_at_column_zero_leaves_that_row() {
        let mut b = buffer("a\nb\n");
        b.place_cursor(0);
        b.extend_cursor(2);
        b.indent();
        assert_eq!(b.text(), "    a\nb\n");
    }

    #[test]
    fn toggle_comments_aligns_and_reverses() {
        let language = rust();
        let mut b = buffer("    a\n  b\n");
        b.select_all();
        b.toggle_comments(&language);
        assert_eq!(b.text(), "  //   a\n  // b\n");
        b.toggle_comments(&language);
        assert_eq!(b.text(), "    a\n  b\n");
    }

    #[test]
    fn toggle_comments_wraps_rows_when_only_block_comments_exist() {
        let language = crate::language::config(crate::highlight::Lang::Css);
        let mut b = buffer("a {}");
        b.toggle_comments(&language);
        assert_eq!(b.text(), "/*a {}*/");
        b.toggle_comments(&language);
        assert_eq!(b.text(), "a {}");
    }

    fn plan_lines(b: &EditorBuffer, f: impl Fn(&EditorBuffer, &LineRows) -> LinePlan) -> LinePlan {
        f(b, &LineRows(b))
    }

    #[test]
    fn delete_line_keeps_the_column_on_the_next_line() {
        let mut b = buffer("one\ntwo\nthree");
        b.place_cursor(6);
        let plan = plan_lines(&b, |b, rows| b.plan_delete_lines(rows));
        b.apply_line_plan(plan);
        assert_eq!(b.text(), "one\nthree");
        assert_eq!(b.line_col(), (1, 2));
        b.place_cursor(9);
        let plan = plan_lines(&b, |b, rows| b.plan_delete_lines(rows));
        b.apply_line_plan(plan);
        assert_eq!(b.text(), "one");
    }

    #[test]
    fn duplicate_lines_up_and_down() {
        let mut b = buffer("a\nb\nc");
        b.place_cursor(2);
        let plan = plan_lines(&b, |b, rows| b.plan_duplicate(false, true, rows));
        b.apply_line_plan(plan);
        assert_eq!(b.text(), "a\nb\nb\nc");
        assert_eq!(b.line_col().0, 2);
        let plan = plan_lines(&b, |b, rows| b.plan_duplicate(true, true, rows));
        b.apply_line_plan(plan);
        assert_eq!(b.text(), "a\nb\nb\nb\nc");
        assert_eq!(b.line_col().0, 2);
    }

    #[test]
    fn move_lines_swaps_with_neighbours() {
        let mut b = buffer("a\nb\nc");
        b.place_cursor(2);
        let plan = plan_lines(&b, |b, rows| b.plan_move_lines(true, rows));
        b.apply_line_plan(plan);
        assert_eq!(b.text(), "b\na\nc");
        assert_eq!(b.line_col(), (0, 0));
        let plan = plan_lines(&b, |b, rows| b.plan_move_lines(false, rows));
        b.apply_line_plan(plan);
        let plan = plan_lines(&b, |b, rows| b.plan_move_lines(false, rows));
        b.apply_line_plan(plan);
        assert_eq!(b.text(), "a\nc\nb");
        assert_eq!(b.line_col(), (2, 0));
        b.undo();
        assert_eq!(b.text(), "a\nb\nc");
    }

    #[test]
    fn join_lines_collapses_indent_and_comment_markers() {
        let mut b = buffer("a\n    b\n// c");
        b.select_all();
        b.join_lines(&rust());
        assert_eq!(b.text(), "a b c");
    }

    #[test]
    fn transpose_swaps_around_the_caret() {
        let mut b = buffer("abc");
        b.place_cursor(1);
        b.transpose();
        assert_eq!(b.text(), "bac");
        assert_eq!(b.cursor(), 2);
        b.place_cursor(3);
        b.transpose();
        assert_eq!(b.text(), "bca");
        assert_eq!(b.cursor(), 3);
    }

    #[test]
    fn copy_without_selection_takes_the_line_and_pastes_above() {
        let mut b = buffer("one\ntwo");
        b.place_cursor(5);
        let (text, slices) = b.copy();
        assert_eq!(text, "two\n");
        b.place_cursor(1);
        b.paste(&text, Some(&slices));
        assert_eq!(b.text(), "two\none\ntwo");
        assert_eq!(b.line_col(), (1, 1));
    }

    #[test]
    fn cut_without_selection_removes_the_line() {
        let mut b = buffer("one\ntwo\n");
        b.place_cursor(1);
        let (text, _) = b.cut();
        assert_eq!(text, "one\n");
        assert_eq!(b.text(), "two\n");
    }

    #[test]
    fn multi_cursor_copy_pastes_one_piece_per_cursor() {
        let mut b = buffer("ab cd");
        let selection = |id, start, end| Selection {
            id,
            start,
            end,
            reversed: false,
            goal: SelectionGoal::None,
        };
        b.set_selections(vec![selection(0, 0, 2), selection(1, 3, 5)]);
        let (text, slices) = b.copy();
        assert_eq!(text, "ab\ncd");
        b.set_selections(vec![selection(0, 5, 5), selection(1, 2, 2)]);
        b.paste(&text, Some(&slices));
        assert_eq!(b.text(), "abab cdcd");
    }

    #[test]
    fn external_text_spreads_lines_over_matching_cursors() {
        let mut b = buffer("x\ny");
        b.place_cursor(1);
        b.add_cursor(3);
        b.paste("1\n2", None);
        assert_eq!(b.text(), "x1\ny2");
    }

    #[test]
    fn select_next_from_a_caret_matches_whole_words_and_wraps() {
        let mut b = buffer("foo food foo\nfoo");
        b.place_cursor(10);
        b.select_next(false);
        assert_eq!(b.ranges(), vec![(9, 12)]);
        b.select_next(false);
        assert_eq!(b.ranges(), vec![(9, 12), (13, 16)]);
        b.select_next(false);
        assert_eq!(b.ranges(), vec![(0, 3), (9, 12), (13, 16)]);
        assert_eq!(b.select_next(false), None);
    }

    #[test]
    fn select_next_from_a_selection_matches_substrings_case_insensitively() {
        let mut b = buffer("ab xAB");
        b.set_selections(vec![Selection {
            id: 0,
            start: 0,
            end: 2,
            reversed: false,
            goal: SelectionGoal::None,
        }]);
        b.select_next(false);
        assert_eq!(b.ranges(), vec![(0, 2), (4, 6)]);
    }

    #[test]
    fn select_all_matches_keeps_the_original_newest() {
        let mut b = buffer("x = x + x");
        b.place_cursor(4);
        b.select_all_matches();
        assert_eq!(b.ranges(), vec![(0, 1), (4, 5), (8, 9)]);
        assert_eq!(b.newest().start, 4);
    }

    fn add(b: &mut EditorBuffer, above: bool) {
        let plan = b.plan_add_selection(above, true, &LineRows(b));
        b.apply_add_selection(plan);
    }

    #[test]
    fn add_caret_below_and_back() {
        let mut b = buffer("abcd\nab\nabcd");
        b.place_cursor(3);
        add(&mut b, false);
        assert_eq!(b.heads(), vec![3, 7]);
        add(&mut b, false);
        assert_eq!(b.heads(), vec![3, 7, 11]);
        add(&mut b, true);
        assert_eq!(b.heads(), vec![3, 7]);
    }

    #[test]
    fn add_selection_below_skips_short_lines() {
        let mut b = buffer("abcd\nab\nabcd");
        b.set_selections(vec![Selection {
            id: 0,
            start: 2,
            end: 4,
            reversed: false,
            goal: SelectionGoal::None,
        }]);
        add(&mut b, false);
        assert_eq!(b.ranges(), vec![(2, 4), (10, 12)]);
    }

    #[test]
    fn syntax_node_selection_grows_and_shrinks() {
        let text = "fn a() { call(x, y); }";
        let mut b = buffer(text);
        let mut syntax = crate::Syntax::new(crate::Lang::Rust).unwrap();
        syntax.sync(&b);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&b);
        }
        b.place_cursor(14);
        let ancestor = |range: Range<usize>| {
            syntax
                .syntax_ancestor(range)
                .map(|node| (node.range, node.kind, node.named))
        };
        assert!(b.select_larger_syntax_node(&ancestor, &|_| false));
        assert_eq!(b.selected_text().as_deref(), Some("x"));
        b.select_larger_syntax_node(&ancestor, &|_| false);
        assert_eq!(b.selected_text().as_deref(), Some("(x, y)"));
        b.select_larger_syntax_node(&ancestor, &|_| false);
        assert_eq!(b.selected_text().as_deref(), Some("call(x, y)"));
        assert!(b.select_smaller_syntax_node());
        assert_eq!(b.selected_text().as_deref(), Some("(x, y)"));
        b.select_smaller_syntax_node();
        b.select_smaller_syntax_node();
        assert_eq!(b.cursor(), 14);
    }

    #[test]
    fn manipulate_text_converts_the_word_at_a_caret() {
        let mut b = buffer("let fooBar = 1;");
        b.place_cursor(6);
        b.manipulate_text(TextTransform::SnakeCase);
        assert_eq!(b.text(), "let foo_bar = 1;");
        assert_eq!(b.selected_text().as_deref(), Some("foo_bar"));
    }

    #[test]
    fn sort_lines_selects_the_result() {
        let mut b = buffer("c\nb\na\nz");
        b.place_cursor(0);
        b.extend_cursor(5);
        let plan = plan_lines(&b, |b, rows| {
            b.plan_manipulate_lines(LineTransform::SortCaseSensitive, rows)
        });
        b.apply_line_plan(plan);
        assert_eq!(b.text(), "a\nb\nc\nz");
        assert_eq!(b.selected_text().as_deref(), Some("a\nb\nc"));
    }

    #[test]
    fn composition_replaces_marked_text_and_undoes_as_one_step() {
        let mut b = buffer("x ");
        b.place_cursor(2);
        b.replace_and_mark_text("a", Some(1..1));
        assert_eq!(b.marked_ranges().first(), Some(&(2..3)));
        b.replace_and_mark_text("\u{e2}", Some(1..1));
        assert_eq!(b.text(), "x \u{e2}");
        b.replace_and_mark_text("", None);
        b.commit_text("\u{e2}n", &rust(), &|_| Scope::default());
        assert_eq!(b.text(), "x \u{e2}n");
        assert!(b.marked_ranges().is_empty());
        assert_eq!(b.cursor(), 4);
        b.undo();
        assert_eq!(b.text(), "x ");
    }

    #[test]
    fn search_and_replace_all_with_groups() {
        use crate::search::SearchOptions;
        let mut b = buffer("let a = 1;\nlet b = 2;");
        let q = SearchQuery::new(
            r"let (\w)",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
            Some("var $1".into()),
        )
        .unwrap();
        let hits = b.search(&q);
        assert_eq!(hits, vec![0..5, 11..16]);
        b.replace_all(&q, &hits);
        assert_eq!(b.text(), "var a = 1;\nvar b = 2;");
        b.undo();
        assert_eq!(b.text(), "let a = 1;\nlet b = 2;");
    }

    #[test]
    fn query_suggestion_prefers_selection_then_word() {
        let mut b = buffer("alpha beta");
        b.place_cursor(7);
        assert_eq!(b.query_suggestion(), "beta");
        b.place_cursor(0);
        b.extend_cursor(3);
        assert_eq!(b.query_suggestion(), "alp");
    }
}
