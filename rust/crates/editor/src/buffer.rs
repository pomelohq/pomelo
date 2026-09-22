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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectionGoal {
    #[default]
    None,
    Column(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    Home,
    End,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
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

    fn word_right(&self, mut i: usize) -> usize {
        let n = self.rope.len_chars();
        while i < n && !Self::is_word(self.rope.char(i)) {
            i += 1;
        }
        while i < n && Self::is_word(self.rope.char(i)) {
            i += 1;
        }
        i
    }

    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && !Self::is_word(self.rope.char(i - 1)) {
            i -= 1;
        }
        while i > 0 && Self::is_word(self.rope.char(i - 1)) {
            i -= 1;
        }
        i
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
        self.delete_each(|s| {
            if s.start > 0 {
                s.start - 1..s.start
            } else {
                0..0
            }
        });
    }

    pub fn delete_forward(&mut self) {
        let len = self.rope.len_chars();
        self.delete_each(|s| s.start..(s.start + 1).min(len));
    }

    fn delete_each(&mut self, caret_range: impl Fn(&Selection) -> Range<usize>) {
        let edits = self
            .selections
            .iter()
            .map(|s| {
                let range = if s.is_empty() {
                    caret_range(s)
                } else {
                    s.start..s.end
                };
                (range, String::new())
            })
            .collect();
        self.transact(|this| {
            let applied = this.edit(edits);
            this.remap_selections(&applied, |s, map| {
                let at = map(s.start, Bias::Left);
                s.collapse_to(at, SelectionGoal::None);
            });
        });
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

    pub fn select_next(&mut self) {
        let newest = self.newest();
        if newest.is_empty() {
            let (a, b) = self.word_range_at(newest.head());
            let mut next: Vec<Selection> = self
                .selections
                .iter()
                .copied()
                .filter(|s| s.id != newest.id)
                .collect();
            next.push(Selection {
                start: a,
                end: b,
                reversed: false,
                ..newest
            });
            self.select(next);
            return;
        }
        let needle = self.rope.slice(newest.start..newest.end).to_string();
        if let Some(pos) = self.find_from(&needle, newest.end) {
            let len = needle.chars().count();
            let s = self.new_selection(pos, pos + len, false);
            let mut next = self.selections.clone();
            next.push(s);
            self.select(next);
        }
    }

    fn find_from(&self, needle: &str, start_char: usize) -> Option<usize> {
        if needle.is_empty() {
            return None;
        }
        let text = self.rope.to_string();
        let start_byte = self
            .rope
            .char_to_byte(start_char.min(self.rope.len_chars()));
        let after = text[start_byte..]
            .find(needle)
            .map(|b| self.rope.byte_to_char(start_byte + b));
        after.or_else(|| {
            text[..start_byte]
                .find(needle)
                .map(|b| self.rope.byte_to_char(b))
        })
    }

    // ---- navigation (multi-cursor aware) ----

    /// Snap an offset strictly inside a fold to its start (`Left`) or end (`Right`); `folds` are merged, sorted.
    fn clip_to_folds(off: usize, bias: Bias, folds: &[Range<usize>]) -> usize {
        match folds.iter().find(|f| f.start < off && off < f.end) {
            Some(fold) => match bias {
                Bias::Left => fold.start,
                Bias::Right => fold.end,
            },
            None => off,
        }
    }

    /// Start of the display row holding `off`: a row hidden by a fold belongs to the fold's header row.
    fn display_row_start(&self, off: usize, folds: &[Range<usize>]) -> usize {
        let mut start = self.line_start_of(off);
        while let Some(fold) = folds.iter().find(|f| f.start < start && start <= f.end) {
            start = self.line_start_of(fold.start);
        }
        start
    }

    fn display_row_end(&self, off: usize, folds: &[Range<usize>]) -> usize {
        let mut end = self.line_end_of(off);
        while let Some(fold) = folds.iter().find(|f| f.start <= end && end < f.end) {
            end = self.line_end_of(fold.end);
        }
        end
    }

    fn vertical_target(
        &self,
        from: usize,
        goal: usize,
        dir: Direction,
        rows: usize,
        folds: &[Range<usize>],
    ) -> usize {
        match dir {
            Direction::Up => {
                let mut row_start = self.display_row_start(from, folds);
                if row_start == 0 {
                    return 0;
                }
                for _ in 0..rows {
                    if row_start == 0 {
                        break;
                    }
                    row_start = self.display_row_start(row_start - 1, folds);
                }
                self.clamp_to_line(self.rope.char_to_line(row_start), goal)
            }
            Direction::Down => {
                let len = self.rope.len_chars();
                let mut row_end = self.display_row_end(from, folds);
                if row_end >= len {
                    return len;
                }
                let mut line = self.rope.char_to_line(from);
                for _ in 0..rows {
                    if row_end >= len {
                        break;
                    }
                    line = self.rope.char_to_line(row_end + 1);
                    row_end = self.display_row_end(row_end + 1, folds);
                }
                Self::clip_to_folds(self.clamp_to_line(line, goal), Bias::Left, folds)
            }
        }
    }

    /// Move (or, with `extend`, grow) every selection by `motion`, treating each fold as one unit.
    pub fn apply_motion(&mut self, motion: Motion, extend: bool, folds: &[Range<usize>]) {
        let len = self.rope.len_chars();
        let next: Vec<Selection> = self
            .selections
            .iter()
            .map(|s| {
                let mut s = *s;
                let collapsing = !extend && !s.is_empty();
                let head = s.head();
                let (to, goal) = match motion {
                    Motion::Left if collapsing => (s.start, SelectionGoal::None),
                    Motion::Right if collapsing => (s.end, SelectionGoal::None),
                    Motion::Left => (
                        Self::clip_to_folds(head.saturating_sub(1), Bias::Left, folds),
                        SelectionGoal::None,
                    ),
                    Motion::Right => (
                        Self::clip_to_folds((head + 1).min(len), Bias::Right, folds),
                        SelectionGoal::None,
                    ),
                    Motion::WordLeft => (
                        Self::clip_to_folds(self.word_left(head), Bias::Left, folds),
                        SelectionGoal::None,
                    ),
                    Motion::WordRight => (
                        Self::clip_to_folds(self.word_right(head), Bias::Right, folds),
                        SelectionGoal::None,
                    ),
                    Motion::Home => (self.display_row_start(head, folds), SelectionGoal::None),
                    Motion::End => (self.display_row_end(head, folds), SelectionGoal::None),
                    Motion::Up | Motion::Down | Motion::PageUp(_) | Motion::PageDown(_) => {
                        let (dir, rows, page) = match motion {
                            Motion::Up => (Direction::Up, 1, false),
                            Motion::PageUp(rows) => (Direction::Up, rows, true),
                            Motion::PageDown(rows) => (Direction::Down, rows, true),
                            _ => (Direction::Down, 1, false),
                        };
                        // Page moves start from the selection end in both directions.
                        let from = match (collapsing, page, dir) {
                            (true, true, _) | (true, false, Direction::Down) => s.end,
                            (true, false, Direction::Up) => s.start,
                            _ => head,
                        };
                        let goal = match s.goal {
                            SelectionGoal::Column(column) if !collapsing => column,
                            _ => self.line_col_of(from).1,
                        };
                        (
                            self.vertical_target(from, goal, dir, rows, folds),
                            SelectionGoal::Column(goal),
                        )
                    }
                };
                if extend {
                    s.set_head(to, goal);
                } else {
                    s.collapse_to(to, goal);
                }
                s
            })
            .collect();
        self.select(next);
    }

    pub fn move_left(&mut self) {
        self.apply_motion(Motion::Left, false, &[]);
    }
    pub fn move_right(&mut self) {
        self.apply_motion(Motion::Right, false, &[]);
    }
    pub fn move_up(&mut self) {
        self.apply_motion(Motion::Up, false, &[]);
    }
    pub fn move_down(&mut self) {
        self.apply_motion(Motion::Down, false, &[]);
    }
    pub fn move_word_left(&mut self) {
        self.apply_motion(Motion::WordLeft, false, &[]);
    }
    pub fn move_word_right(&mut self) {
        self.apply_motion(Motion::WordRight, false, &[]);
    }
    pub fn move_home(&mut self) {
        self.apply_motion(Motion::Home, false, &[]);
    }
    pub fn move_end(&mut self) {
        self.apply_motion(Motion::End, false, &[]);
    }

    pub fn extend_left(&mut self) {
        self.apply_motion(Motion::Left, true, &[]);
    }
    pub fn extend_right(&mut self) {
        self.apply_motion(Motion::Right, true, &[]);
    }
    pub fn extend_up(&mut self) {
        self.apply_motion(Motion::Up, true, &[]);
    }
    pub fn extend_down(&mut self) {
        self.apply_motion(Motion::Down, true, &[]);
    }
    pub fn extend_word_left(&mut self) {
        self.apply_motion(Motion::WordLeft, true, &[]);
    }
    pub fn extend_word_right(&mut self) {
        self.apply_motion(Motion::WordRight, true, &[]);
    }
    pub fn extend_home(&mut self) {
        self.apply_motion(Motion::Home, true, &[]);
    }
    pub fn extend_end(&mut self) {
        self.apply_motion(Motion::End, true, &[]);
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
        b.select_next();
        assert_eq!(b.ranges(), vec![(0, 5)]);
        b.select_next();
        b.select_next();
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
        b.apply_motion(Motion::PageDown(4), false, &[]);
        assert_eq!(b.line_col(), (4, 2));
        b.apply_motion(Motion::PageDown(20), false, &[]);
        assert_eq!(b.line_col(), (10, 0));
        b.apply_motion(Motion::PageUp(4), false, &[]);
        assert_eq!(b.line_col(), (6, 2));
        b.apply_motion(Motion::PageUp(20), true, &[]);
        assert_eq!(b.line_col(), (0, 2));
        assert_eq!(b.selected_text().as_deref().map(str::len), Some(6 * 6));
    }
}
