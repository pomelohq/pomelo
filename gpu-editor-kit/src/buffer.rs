//! The editor's text model: a rope (O(log n) edits/indexing, scales to large files) plus a char-offset cursor.
//! Platform-independent — no GPU, no windowing.

use ropey::Rope;

pub struct EditorBuffer {
    pub rope: Rope,
    /// Cursor as a character offset into the rope.
    pub cursor: usize,
    /// Selection anchor; a selection exists while it differs from `cursor`.
    pub anchor: usize,
}

impl Default for EditorBuffer {
    fn default() -> Self {
        Self { rope: Rope::from_str(""), cursor: 0, anchor: 0 }
    }
}

impl EditorBuffer {
    pub fn from_str(text: &str) -> Self {
        Self { rope: Rope::from_str(text), cursor: 0, anchor: 0 }
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
    }

    /// Move the cursor while keeping the anchor (a drag extends the selection).
    pub fn extend_cursor(&mut self, off: usize) {
        self.cursor = off.min(self.rope.len_chars());
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
        self.delete_selection();
        self.rope.insert_char(self.cursor, ch);
        self.cursor += 1;
        self.collapse();
    }

    pub fn backspace(&mut self) {
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
        if self.cursor > 0 {
            self.cursor -= 1;
        }
        self.collapse();
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.rope.len_chars() {
            self.cursor += 1;
        }
        self.collapse();
    }

    pub fn move_up(&mut self) {
        let (line, col) = self.line_col();
        if line == 0 {
            self.collapse();
            return;
        }
        self.cursor = self.clamp_to_line(line - 1, col);
        self.collapse();
    }

    pub fn move_down(&mut self) {
        let (line, col) = self.line_col();
        if line + 1 >= self.rope.len_lines() {
            self.collapse();
            return;
        }
        self.cursor = self.clamp_to_line(line + 1, col);
        self.collapse();
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
