//! The editor's text model: a rope plus one-or-more selections (multi-cursor), with coalesced undo/redo.
//! Platform-independent — no GPU, no windowing. Modeled on Zed's SelectionsCollection: edits apply to every
//! selection (processed right-to-left so earlier offsets stay valid), and overlapping selections merge.

use ropey::Rope;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    None,
    Insert,
    Delete,
}

/// One selection: an anchor and a cursor (head). A caret is a selection with anchor == cursor.
#[derive(Clone, Copy)]
pub struct Sel {
    pub anchor: usize,
    pub cursor: usize,
}

impl Sel {
    fn caret(off: usize) -> Self {
        Sel { anchor: off, cursor: off }
    }
    pub fn start(&self) -> usize {
        self.anchor.min(self.cursor)
    }
    pub fn end(&self) -> usize {
        self.anchor.max(self.cursor)
    }
    fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }
    pub fn range(&self) -> Option<(usize, usize)> {
        if self.is_empty() {
            None
        } else {
            Some((self.start(), self.end()))
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    rope: Rope,
    sels: Vec<Sel>,
}

pub struct EditorBuffer {
    pub rope: Rope,
    /// Non-empty; the last element is the primary/newest selection (drives caret-follow).
    sels: Vec<Sel>,
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
            sels: vec![Sel::caret(0)],
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: EditKind::None,
        }
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn selections(&self) -> &[Sel] {
        &self.sels
    }

    fn primary(&self) -> Sel {
        *self.sels.last().unwrap()
    }

    pub fn cursor(&self) -> usize {
        self.primary().cursor
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
        let mut len = self.rope.line(line).len_chars();
        if self.rope.line(line).chars().last() == Some('\n') {
            len -= 1;
        }
        len
    }

    pub fn offset_at(&self, line: usize, col: usize) -> usize {
        let last = self.rope.len_lines().saturating_sub(1);
        self.clamp_to_line(line.min(last), col)
    }

    fn clamp_to_line(&self, line: usize, col: usize) -> usize {
        let start = self.rope.line_to_char(line);
        start + col.min(self.line_len(line))
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

    fn up_offset(&self, off: usize) -> usize {
        let (line, col) = self.line_col_of(off);
        if line == 0 {
            off
        } else {
            self.clamp_to_line(line - 1, col)
        }
    }

    fn down_offset(&self, off: usize) -> usize {
        let (line, col) = self.line_col_of(off);
        if line + 1 >= self.rope.len_lines() {
            off
        } else {
            self.clamp_to_line(line + 1, col)
        }
    }

    fn word_range_at(&self, off: usize) -> (usize, usize) {
        let n = self.rope.len_chars();
        let off = off.min(n);
        let inside = (off < n && Self::is_word(self.rope.char(off))) || (off > 0 && Self::is_word(self.rope.char(off - 1)));
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
        let mut parts: Vec<(usize, String)> = self
            .sels
            .iter()
            .filter_map(|s| s.range().map(|(a, b)| (a, self.rope.slice(a..b).to_string())))
            .collect();
        if parts.is_empty() {
            return None;
        }
        parts.sort_by_key(|(a, _)| *a);
        Some(parts.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join("\n"))
    }

    // ---- undo/redo ----

    fn snapshot(&self) -> Snapshot {
        Snapshot { rope: self.rope.clone(), sels: self.sels.clone() }
    }

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

    fn restore(&mut self, snap: Snapshot) {
        self.rope = snap.rope;
        let n = self.rope.len_chars();
        self.sels = snap.sels.into_iter().map(|s| Sel { anchor: s.anchor.min(n), cursor: s.cursor.min(n) }).collect();
        if self.sels.is_empty() {
            self.sels.push(Sel::caret(0));
        }
        self.last_edit = EditKind::None;
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            let cur = self.snapshot();
            self.redo_stack.push(cur);
            self.restore(prev);
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            let cur = self.snapshot();
            self.undo_stack.push(cur);
            self.restore(next);
        }
    }

    // ---- selection bookkeeping ----

    /// Sort by position and merge overlapping/touching selections; keep the primary (nearest the old primary cursor) last.
    fn merge(&mut self) {
        if self.sels.len() > 1 {
            let primary_cursor = self.primary().cursor;
            self.sels.sort_by_key(|s| s.start());
            let mut merged: Vec<Sel> = Vec::with_capacity(self.sels.len());
            for s in std::mem::take(&mut self.sels) {
                if let Some(last) = merged.last_mut() {
                    if s.start() <= last.end() {
                        *last = Sel { anchor: last.start().min(s.start()), cursor: last.end().max(s.end()) };
                        continue;
                    }
                }
                merged.push(s);
            }
            self.sels = merged;
            if let Some(pi) = self.sels.iter().position(|s| s.start() <= primary_cursor && primary_cursor <= s.end()) {
                let p = self.sels.remove(pi);
                self.sels.push(p);
            }
        }
        if self.sels.is_empty() {
            self.sels.push(Sel::caret(0));
        }
    }

    // Move each selection's cursor via `f`, collapsing the anchor (plain arrow), then merge.
    fn move_each(&mut self, f: impl Fn(&Self, usize) -> usize) {
        let next: Vec<Sel> = self.sels.iter().map(|s| Sel::caret(f(self, s.cursor))).collect();
        self.sels = next;
        self.merge();
        self.break_run();
    }

    // Move each selection's cursor via `f`, keeping the anchor (shift+arrow), then merge.
    fn extend_each(&mut self, f: impl Fn(&Self, usize) -> usize) {
        let next: Vec<Sel> = self.sels.iter().map(|s| Sel { anchor: s.anchor, cursor: f(self, s.cursor) }).collect();
        self.sels = next;
        self.merge();
        self.break_run();
    }

    // ---- editing (applies to every selection, right-to-left) ----

    fn edit_each(&mut self, f: impl Fn(&mut Rope, Sel) -> Sel) {
        // Apply left-to-right, shifting each later selection by the net length change of the earlier edits so their
        // positions stay valid. (Right-to-left is wrong: a left edit still shifts the already-placed right cursors.)
        let mut order: Vec<usize> = (0..self.sels.len()).collect();
        order.sort_by_key(|&i| self.sels[i].start());
        let mut delta: isize = 0;
        for &i in &order {
            let s = self.sels[i];
            let shifted = Sel {
                anchor: (s.anchor as isize + delta).max(0) as usize,
                cursor: (s.cursor as isize + delta).max(0) as usize,
            };
            let before = self.rope.len_chars() as isize;
            let ns = f(&mut self.rope, shifted);
            delta += self.rope.len_chars() as isize - before;
            self.sels[i] = ns;
        }
        self.merge();
    }

    pub fn insert_char(&mut self, ch: char) {
        self.record(EditKind::Insert);
        self.edit_each(|rope, sel| {
            let (s, e) = (sel.start(), sel.end());
            if e > s {
                rope.remove(s..e);
            }
            rope.insert_char(s, ch);
            Sel::caret(s + 1)
        });
    }

    pub fn backspace(&mut self) {
        self.record(EditKind::Delete);
        self.edit_each(|rope, sel| {
            let (s, e) = (sel.start(), sel.end());
            if e > s {
                rope.remove(s..e);
                Sel::caret(s)
            } else if s > 0 {
                rope.remove(s - 1..s);
                Sel::caret(s - 1)
            } else {
                Sel::caret(0)
            }
        });
    }

    // ---- single-cursor gestures (from the mouse / simple keys) ----

    pub fn place_cursor(&mut self, off: usize) {
        let off = off.min(self.rope.len_chars());
        self.sels = vec![Sel::caret(off)];
        self.break_run();
    }

    pub fn extend_cursor(&mut self, off: usize) {
        let off = off.min(self.rope.len_chars());
        self.sels.last_mut().unwrap().cursor = off;
        self.break_run();
    }

    /// Add a caret at `off` (Cmd+click multi-cursor).
    pub fn add_cursor(&mut self, off: usize) {
        let off = off.min(self.rope.len_chars());
        self.sels.push(Sel::caret(off));
        self.merge();
        self.break_run();
    }

    /// Collapse to a single caret at the primary (Esc).
    pub fn collapse_cursors(&mut self) {
        let c = self.primary().cursor;
        self.sels = vec![Sel::caret(c)];
        self.break_run();
    }

    pub fn select_word_at(&mut self, off: usize) {
        let (a, b) = self.word_range_at(off);
        self.sels = vec![Sel { anchor: a, cursor: b }];
        self.break_run();
    }

    pub fn select_all(&mut self) {
        self.sels = vec![Sel { anchor: 0, cursor: self.rope.len_chars() }];
        self.break_run();
    }

    /// Cmd+D: first press selects the word at the primary; further presses add the next occurrence as a new cursor.
    pub fn select_next(&mut self) {
        let p = self.primary();
        if p.is_empty() {
            let (a, b) = self.word_range_at(p.cursor);
            *self.sels.last_mut().unwrap() = Sel { anchor: a, cursor: b };
        } else {
            let needle = self.rope.slice(p.start()..p.end()).to_string();
            if !needle.is_empty() {
                if let Some(pos) = self.find_from(&needle, p.end()) {
                    let len = needle.chars().count();
                    self.sels.push(Sel { anchor: pos, cursor: pos + len });
                    self.merge();
                }
            }
        }
        self.break_run();
    }

    fn find_from(&self, needle: &str, start_char: usize) -> Option<usize> {
        let text = self.rope.to_string();
        let start_byte = text.char_indices().nth(start_char).map(|(b, _)| b).unwrap_or(text.len());
        let after = text[start_byte..].find(needle).map(|b| text[..start_byte + b].chars().count());
        after.or_else(|| text[..start_byte].find(needle).map(|b| text[..b].chars().count()))
    }

    // ---- navigation (multi-cursor aware) ----

    pub fn move_left(&mut self) {
        self.move_each(|_, c| c.saturating_sub(1));
    }
    pub fn move_right(&mut self) {
        let n = self.rope.len_chars();
        self.move_each(move |_, c| (c + 1).min(n));
    }
    pub fn move_up(&mut self) {
        self.move_each(|b, c| b.up_offset(c));
    }
    pub fn move_down(&mut self) {
        self.move_each(|b, c| b.down_offset(c));
    }
    pub fn move_word_left(&mut self) {
        self.move_each(|b, c| b.word_left(c));
    }
    pub fn move_word_right(&mut self) {
        self.move_each(|b, c| b.word_right(c));
    }
    pub fn move_home(&mut self) {
        self.move_each(|b, c| b.line_start_of(c));
    }
    pub fn move_end(&mut self) {
        self.move_each(|b, c| b.line_end_of(c));
    }

    pub fn extend_left(&mut self) {
        self.extend_each(|_, c| c.saturating_sub(1));
    }
    pub fn extend_right(&mut self) {
        let n = self.rope.len_chars();
        self.extend_each(move |_, c| (c + 1).min(n));
    }
    pub fn extend_up(&mut self) {
        self.extend_each(|b, c| b.up_offset(c));
    }
    pub fn extend_down(&mut self) {
        self.extend_each(|b, c| b.down_offset(c));
    }
    pub fn extend_word_left(&mut self) {
        self.extend_each(|b, c| b.word_left(c));
    }
    pub fn extend_word_right(&mut self) {
        self.extend_each(|b, c| b.word_right(c));
    }
    pub fn extend_home(&mut self) {
        self.extend_each(|b, c| b.line_start_of(c));
    }
    pub fn extend_end(&mut self) {
        self.extend_each(|b, c| b.line_end_of(c));
    }

    #[cfg(test)]
    fn cursors(&self) -> Vec<usize> {
        self.sels.iter().map(|s| s.cursor).collect()
    }
    #[cfg(test)]
    fn ranges(&self) -> Vec<(usize, usize)> {
        self.sels.iter().map(|s| (s.start(), s.end())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_undo() {
        let mut b = EditorBuffer::from_str("");
        for ch in "abc".chars() {
            b.insert_char(ch);
        }
        assert_eq!(b.text(), "abc");
        b.undo(); // one coalesced typing run
        assert_eq!(b.text(), "");
        b.redo();
        assert_eq!(b.text(), "abc");
    }

    #[test]
    fn add_cursor_types_at_all() {
        let mut b = EditorBuffer::from_str("a\nb\n");
        b.place_cursor(0); // before 'a'
        b.add_cursor(2); // before 'b'
        b.insert_char('X');
        assert_eq!(b.text(), "Xa\nXb\n");
        assert_eq!(b.cursors(), vec![1, 4]);
    }

    #[test]
    fn select_next_occurrence() {
        let mut b = EditorBuffer::from_str("total = total + total");
        b.place_cursor(2); // inside the first "total"
        b.select_next(); // selects first "total"
        assert_eq!(b.ranges(), vec![(0, 5)]);
        b.select_next(); // add second
        b.select_next(); // add third
        assert_eq!(b.ranges().len(), 3);
        // typing replaces all three occurrences
        for ch in "sum".chars() {
            b.insert_char(ch);
        }
        assert_eq!(b.text(), "sum = sum + sum");
    }

    #[test]
    fn overlapping_cursors_merge() {
        let mut b = EditorBuffer::from_str("hello");
        b.place_cursor(2);
        b.add_cursor(2); // same spot -> should dedup
        assert_eq!(b.selections().len(), 1);
    }

    #[test]
    fn word_and_home_end() {
        let mut b = EditorBuffer::from_str("foo bar");
        b.place_cursor(0);
        b.move_word_right();
        assert_eq!(b.cursor(), 3); // end of "foo"
        b.move_end();
        assert_eq!(b.cursor(), 7);
        b.move_home();
        assert_eq!(b.cursor(), 0);
    }
}
