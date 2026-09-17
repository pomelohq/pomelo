//! The editor's text model: a rope (O(log n) edits/indexing, scales to large files) plus a char-offset cursor.
//! Platform-independent — no GPU, no windowing.

use ropey::Rope;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    None,
    Insert,
    Delete,
}

#[derive(Clone)]
struct Snapshot {
    rope: Rope,
    cursor: usize,
    anchor: usize,
}

pub struct EditorBuffer {
    pub rope: Rope,
    /// Cursor as a character offset into the rope.
    pub cursor: usize,
    /// Selection anchor; a selection exists while it differs from `cursor`.
    pub anchor: usize,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    last_edit: EditKind,
}

impl Default for EditorBuffer {
    fn default() -> Self {
        Self::from_str("")
    }
}

impl EditorBuffer {
    pub fn from_str(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            cursor: 0,
            anchor: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: EditKind::None,
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { rope: self.rope.clone(), cursor: self.cursor, anchor: self.anchor }
    }

    // Push an undo entry only when the edit kind changes, so a run of typing (or deleting) coalesces into one undo.
    fn record(&mut self, kind: EditKind) {
        if self.last_edit != kind {
            self.undo_stack.push(self.snapshot());
            self.redo_stack.clear();
            self.last_edit = kind;
        }
    }

    fn break_run(&mut self) {
        self.last_edit = EditKind::None;
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.snapshot());
            self.rope = prev.rope;
            self.cursor = prev.cursor.min(self.rope.len_chars());
            self.anchor = prev.anchor.min(self.rope.len_chars());
            self.last_edit = EditKind::None;
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.rope = next.rope;
            self.cursor = next.cursor.min(self.rope.len_chars());
            self.anchor = next.anchor.min(self.rope.len_chars());
            self.last_edit = EditKind::None;
        }
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.rope.len_chars();
        self.break_run();
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|(s, e)| self.rope.slice(s..e).to_string())
    }

    /// Selected char range (start, end) if any, else None.
    pub fn selection(&self) -> Option<(usize, usize)> {
        if self.cursor == self.anchor {
            None
        } else {
            Some((self.cursor.min(self.anchor), self.cursor.max(self.anchor)))
        }
    }

    fn collapse(&mut self) {
        self.anchor = self.cursor;
    }

    /// Char count of `line` excluding its trailing newline.
    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let mut len = self.rope.line(line).len_chars();
        if self.rope.line(line).chars().last() == Some('\n') {
            len -= 1;
        }
        len
    }

    pub fn offset_at(&self, line: usize, col: usize) -> usize {
        let last = self.rope.len_lines().saturating_sub(1);
        let line = line.min(last);
        self.clamp_to_line(line, col)
    }

    /// Place the cursor and collapse any selection (a plain click).
    pub fn place_cursor(&mut self, off: usize) {
        self.cursor = off.min(self.rope.len_chars());
        self.collapse();
        self.break_run();
    }

    /// Move the cursor while keeping the anchor (a drag extends the selection).
    pub fn extend_cursor(&mut self, off: usize) {
        self.cursor = off.min(self.rope.len_chars());
        self.break_run();
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Cursor position as (line, column) in characters.
    pub fn line_col(&self) -> (usize, usize) {
        self.line_col_of(self.cursor)
    }

    /// (line, column) of an arbitrary char offset.
    pub fn line_col_of(&self, off: usize) -> (usize, usize) {
        let off = off.min(self.rope.len_chars());
        let line = self.rope.char_to_line(off);
        let line_start = self.rope.line_to_char(line);
        (line, off - line_start)
    }

    /// Remove the current selection if any; returns true if something was deleted.
    fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection() {
            self.rope.remove(s..e);
            self.cursor = s;
            self.collapse();
            true
        } else {
            false
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.record(EditKind::Insert);
        self.delete_selection();
        self.rope.insert_char(self.cursor, ch);
        self.cursor += 1;
        self.collapse();
    }

    pub fn backspace(&mut self) {
        self.record(EditKind::Delete);
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            self.rope.remove(self.cursor - 1..self.cursor);
            self.cursor -= 1;
        }
        self.collapse();
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.collapse();
        self.break_run();
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.rope.len_chars() {
            self.cursor += 1;
        }
        self.collapse();
        self.break_run();
    }

    pub fn move_up(&mut self) {
        let (line, col) = self.line_col();
        if line > 0 {
            self.cursor = self.clamp_to_line(line - 1, col);
        }
        self.collapse();
        self.break_run();
    }

    pub fn move_down(&mut self) {
        let (line, col) = self.line_col();
        if line + 1 < self.rope.len_lines() {
            self.cursor = self.clamp_to_line(line + 1, col);
        }
        self.collapse();
        self.break_run();
    }

    // Shift+arrow: move the cursor but keep the anchor, extending the selection.
    pub fn extend_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.break_run();
    }

    pub fn extend_right(&mut self) {
        if self.cursor < self.rope.len_chars() {
            self.cursor += 1;
        }
        self.break_run();
    }

    pub fn extend_up(&mut self) {
        let (line, col) = self.line_col();
        if line > 0 {
            self.cursor = self.clamp_to_line(line - 1, col);
        }
        self.break_run();
    }

    pub fn extend_down(&mut self) {
        let (line, col) = self.line_col();
        if line + 1 < self.rope.len_lines() {
            self.cursor = self.clamp_to_line(line + 1, col);
        }
        self.break_run();
    }

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    // Next word boundary to the right: skip non-word chars, then the word.
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

    // Previous word boundary to the left: skip non-word chars, then the word.
    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && !Self::is_word(self.rope.char(i - 1)) {
            i -= 1;
        }
        while i > 0 && Self::is_word(self.rope.char(i - 1)) {
            i -= 1;
        }
        i
    }

    fn line_start(&self) -> usize {
        let (line, _) = self.line_col();
        self.rope.line_to_char(line)
    }

    fn line_end(&self) -> usize {
        let (line, _) = self.line_col();
        self.rope.line_to_char(line) + self.line_len(line)
    }

    pub fn move_word_left(&mut self) {
        self.cursor = self.word_left(self.cursor);
        self.collapse();
        self.break_run();
    }

    pub fn move_word_right(&mut self) {
        self.cursor = self.word_right(self.cursor);
        self.collapse();
        self.break_run();
    }

    pub fn extend_word_left(&mut self) {
        self.cursor = self.word_left(self.cursor);
        self.break_run();
    }

    pub fn extend_word_right(&mut self) {
        self.cursor = self.word_right(self.cursor);
        self.break_run();
    }

    pub fn move_home(&mut self) {
        self.cursor = self.line_start();
        self.collapse();
        self.break_run();
    }

    pub fn move_end(&mut self) {
        self.cursor = self.line_end();
        self.collapse();
        self.break_run();
    }

    pub fn extend_home(&mut self) {
        self.cursor = self.line_start();
        self.break_run();
    }

    pub fn extend_end(&mut self) {
        self.cursor = self.line_end();
        self.break_run();
    }

    /// Select the word around `off` (double-click).
    pub fn select_word_at(&mut self, off: usize) {
        let n = self.rope.len_chars();
        let off = off.min(n);
        let inside = (off < n && Self::is_word(self.rope.char(off))) || (off > 0 && Self::is_word(self.rope.char(off - 1)));
        if !inside {
            self.place_cursor(off);
            return;
        }
        let mut start = off;
        while start > 0 && Self::is_word(self.rope.char(start - 1)) {
            start -= 1;
        }
        let mut end = off;
        while end < n && Self::is_word(self.rope.char(end)) {
            end += 1;
        }
        self.anchor = start;
        self.cursor = end;
        self.break_run();
    }

    /// Char offset at `col` on `line`, clamped to that line's length (excluding its trailing newline).
    fn clamp_to_line(&self, line: usize, col: usize) -> usize {
        let start = self.rope.line_to_char(line);
        let mut len = self.rope.line(line).len_chars();
        if self.rope.line(line).chars().last() == Some('\n') {
            len -= 1;
        }
        start + col.min(len)
    }
}
