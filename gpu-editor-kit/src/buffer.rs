//! The editor's text model: a rope (O(log n) edits/indexing, scales to large files) plus a char-offset cursor.
//! Platform-independent — no GPU, no windowing.

use ropey::Rope;

pub struct EditorBuffer {
    pub rope: Rope,
    /// Cursor as a character offset into the rope.
    pub cursor: usize,
}

impl Default for EditorBuffer {
    fn default() -> Self {
        Self { rope: Rope::from_str(""), cursor: 0 }
    }
}

impl EditorBuffer {
    pub fn from_str(text: &str) -> Self {
        Self { rope: Rope::from_str(text), cursor: 0 }
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Cursor position as (line, column) in characters.
    pub fn line_col(&self) -> (usize, usize) {
        let line = self.rope.char_to_line(self.cursor);
        let line_start = self.rope.line_to_char(line);
        (line, self.cursor - line_start)
    }

    pub fn insert_char(&mut self, ch: char) {
        self.rope.insert_char(self.cursor, ch);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.rope.remove(self.cursor - 1..self.cursor);
            self.cursor -= 1;
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.rope.len_chars() {
            self.cursor += 1;
        }
    }

    pub fn move_up(&mut self) {
        let (line, col) = self.line_col();
        if line == 0 {
            return;
        }
        self.cursor = self.clamp_to_line(line - 1, col);
    }

    pub fn move_down(&mut self) {
        let (line, col) = self.line_col();
        if line + 1 >= self.rope.len_lines() {
            return;
        }
        self.cursor = self.clamp_to_line(line + 1, col);
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
