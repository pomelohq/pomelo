//! The editor's text model: lines of text plus a cursor. Kept deliberately simple for the kit's first cut; a rope
//! (e.g. `ropey`) replaces this once editing scales. Platform-independent — no GPU, no windowing.

#[derive(Clone, Default)]
pub struct EditorBuffer {
    pub lines: Vec<String>,
    pub cursor: Cursor,
}

#[derive(Clone, Copy, Default)]
pub struct Cursor {
    pub line: usize,
    pub column: usize,
}

impl EditorBuffer {
    pub fn from_str(text: &str) -> Self {
        let lines: Vec<String> = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|s| s.to_string()).collect()
        };
        Self { lines, cursor: Cursor::default() }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn insert_char(&mut self, ch: char) {
        if ch == '\n' {
            let rest = self.lines[self.cursor.line].split_off(self.cursor.column);
            self.lines.insert(self.cursor.line + 1, rest);
            self.cursor.line += 1;
            self.cursor.column = 0;
            return;
        }
        self.lines[self.cursor.line].insert(self.cursor.column, ch);
        self.cursor.column += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor.column > 0 {
            self.cursor.column -= 1;
            self.lines[self.cursor.line].remove(self.cursor.column);
        } else if self.cursor.line > 0 {
            let removed = self.lines.remove(self.cursor.line);
            self.cursor.line -= 1;
            self.cursor.column = self.lines[self.cursor.line].len();
            self.lines[self.cursor.line].push_str(&removed);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor.column > 0 {
            self.cursor.column -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.column = self.lines[self.cursor.line].len();
        }
    }

    pub fn move_right(&mut self) {
        let len = self.lines[self.cursor.line].len();
        if self.cursor.column < len {
            self.cursor.column += 1;
        } else if self.cursor.line + 1 < self.lines.len() {
            self.cursor.line += 1;
            self.cursor.column = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.column = self.cursor.column.min(self.lines[self.cursor.line].len());
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor.line + 1 < self.lines.len() {
            self.cursor.line += 1;
            self.cursor.column = self.cursor.column.min(self.lines[self.cursor.line].len());
        }
    }
}
