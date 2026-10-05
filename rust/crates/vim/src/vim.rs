//! Vim mode over an editor buffer: a per-editor state machine (mode, count, pending prefix) that turns typed
//! keys in Normal mode into motions and edits, and lets Insert mode type as usual.

use editor::movement::{char_kind, CharKind};
use editor::EditorBuffer;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
}

impl Mode {
    /// What the status bar shows for the mode.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "-- NORMAL --",
            Mode::Insert => "-- INSERT --",
        }
    }
}

/// Whether the editor still has to act on the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Handled,
    PassThrough,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Down,
    Up,
    NextWordStart { big: bool },
    NextWordEnd { big: bool },
    PreviousWordStart { big: bool },
    LineStart,
    FirstNonBlank,
    LineEnd,
    FirstLine,
    LastLine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// `g` was typed; the next key finishes a `g` command.
    G,
}

#[derive(Debug, Default)]
pub struct Vim {
    mode: Mode,
    count: Option<usize>,
    pending: Option<Pending>,
    /// The column `j` and `k` keep across lines shorter than it.
    goal_column: Option<usize>,
}

impl Vim {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Escape: back to Normal, the cursor stepping off the end of what was typed.
    pub fn escape(&mut self, buffer: &mut EditorBuffer) -> Outcome {
        self.count = None;
        self.pending = None;
        if self.mode == Mode::Insert {
            self.mode = Mode::Normal;
            let (_, column) = buffer.line_col();
            let cursor = buffer.cursor();
            buffer.collapse_cursors();
            buffer.place_cursor(if column > 0 { cursor - 1 } else { cursor });
        } else {
            buffer.collapse_cursors();
        }
        Outcome::Handled
    }

    /// Typed text: Insert mode types it, Normal mode reads it as commands.
    pub fn text(&mut self, text: &str, buffer: &mut EditorBuffer) -> Outcome {
        if self.mode == Mode::Insert {
            return Outcome::PassThrough;
        }
        for typed in text.chars() {
            self.normal_char(typed, buffer);
        }
        Outcome::Handled
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1).max(1)
    }

    fn normal_char(&mut self, typed: char, buffer: &mut EditorBuffer) {
        if let Some(Pending::G) = self.pending.take() {
            if typed == 'g' {
                self.run_motion(Motion::FirstLine, buffer);
            } else {
                self.count = None;
            }
            return;
        }
        if let Some(digit) = typed.to_digit(10) {
            if digit != 0 || self.count.is_some() {
                let count = self.count.unwrap_or(0);
                self.count = Some(count.saturating_mul(10).saturating_add(digit as usize));
                return;
            }
        }
        let motion = match typed {
            'h' => Some(Motion::Left),
            'l' | ' ' => Some(Motion::Right),
            'j' => Some(Motion::Down),
            'k' => Some(Motion::Up),
            'w' | 'W' => Some(Motion::NextWordStart { big: typed == 'W' }),
            'e' | 'E' => Some(Motion::NextWordEnd { big: typed == 'E' }),
            'b' | 'B' => Some(Motion::PreviousWordStart { big: typed == 'B' }),
            '0' => Some(Motion::LineStart),
            '^' => Some(Motion::FirstNonBlank),
            '$' => Some(Motion::LineEnd),
            'G' => Some(Motion::LastLine),
            _ => None,
        };
        if let Some(motion) = motion {
            self.run_motion(motion, buffer);
            return;
        }
        match typed {
            'g' => self.pending = Some(Pending::G),
            'i' | 'a' | 'I' | 'A' | 'o' | 'O' => self.enter_insert(typed, buffer),
            'x' => self.delete_under_cursor(buffer),
            _ => self.count = None,
        }
    }

    fn run_motion(&mut self, motion: Motion, buffer: &mut EditorBuffer) {
        let explicit = self.count.is_some();
        let count = self.take_count();
        let from = buffer.cursor();
        let vertical = matches!(motion, Motion::Down | Motion::Up);
        if !vertical {
            self.goal_column = None;
        }
        let target = match motion {
            Motion::FirstLine | Motion::LastLine => {
                let last = last_line(buffer);
                let line = match (motion, explicit) {
                    (Motion::FirstLine, false) => 0,
                    (Motion::LastLine, false) => last,
                    _ => (count - 1).min(last),
                };
                first_non_blank(buffer, line)
            }
            Motion::Down | Motion::Up => {
                let (line, column) = buffer.line_col_of(from);
                let goal = *self.goal_column.get_or_insert(column);
                let line = if motion == Motion::Down {
                    (line + count).min(last_line(buffer))
                } else {
                    line.saturating_sub(count)
                };
                buffer.offset_at(line, goal.min(last_column(buffer, line)))
            }
            _ => (0..count).fold(from, |at, _| step(buffer, at, motion)),
        };
        buffer.collapse_cursors();
        buffer.place_cursor(clamp_normal(buffer, target));
    }

    fn enter_insert(&mut self, typed: char, buffer: &mut EditorBuffer) {
        self.count = None;
        self.goal_column = None;
        let cursor = buffer.cursor();
        let (line, column) = buffer.line_col_of(cursor);
        let line_start = buffer.offset_at(line, 0);
        let line_end = line_start + buffer.line_len(line);
        buffer.collapse_cursors();
        match typed {
            'a' if column < buffer.line_len(line) => buffer.place_cursor(cursor + 1),
            'I' => buffer.place_cursor(first_non_blank(buffer, line)),
            'A' => buffer.place_cursor(line_end),
            'o' | 'O' => {
                let indent = indentation(buffer, line);
                let at = if typed == 'o' { line_end } else { line_start };
                let inserted = if typed == 'o' {
                    format!("\n{indent}")
                } else {
                    format!("{indent}\n")
                };
                buffer.replace_ranges(vec![(at..at, inserted)]);
                let caret = if typed == 'o' {
                    at + 1 + indent.chars().count()
                } else {
                    at + indent.chars().count()
                };
                buffer.place_cursor(caret);
            }
            _ => {}
        }
        self.mode = Mode::Insert;
    }

    fn delete_under_cursor(&mut self, buffer: &mut EditorBuffer) {
        let count = self.take_count();
        let cursor = buffer.cursor();
        let (line, column) = buffer.line_col_of(cursor);
        let end = cursor + count.min(buffer.line_len(line).saturating_sub(column));
        if end > cursor {
            buffer.collapse_cursors();
            buffer.replace_ranges(vec![(cursor..end, String::new())]);
            buffer.place_cursor(clamp_normal(buffer, cursor));
        }
    }
}

fn last_line(buffer: &EditorBuffer) -> usize {
    let lines = buffer.rope.len_lines();
    // A trailing newline leaves an empty last line that Normal mode cannot reach.
    if lines > 1 && buffer.line_len(lines - 1) == 0 {
        lines - 2
    } else {
        lines.saturating_sub(1)
    }
}

/// The last column Normal mode's block cursor can sit on.
fn last_column(buffer: &EditorBuffer, line: usize) -> usize {
    buffer.line_len(line).saturating_sub(1)
}

fn clamp_normal(buffer: &EditorBuffer, offset: usize) -> usize {
    let (line, column) = buffer.line_col_of(offset);
    buffer.offset_at(line, column.min(last_column(buffer, line)))
}

fn char_at(buffer: &EditorBuffer, offset: usize) -> Option<char> {
    (offset < buffer.rope.len_chars()).then(|| buffer.rope.char(offset))
}

fn indentation(buffer: &EditorBuffer, line: usize) -> String {
    let start = buffer.offset_at(line, 0);
    (start..start + buffer.line_len(line))
        .map_while(|offset| char_at(buffer, offset).filter(|c| *c == ' ' || *c == '\t'))
        .collect()
}

fn first_non_blank(buffer: &EditorBuffer, line: usize) -> usize {
    let start = buffer.offset_at(line, 0);
    start
        + indentation(buffer, line)
            .chars()
            .count()
            .min(last_column(buffer, line))
}

/// Words split where the kind of char changes; WORDs (`big`) only at whitespace.
fn kind(c: char, big: bool) -> CharKind {
    match char_kind(c) {
        CharKind::Whitespace => CharKind::Whitespace,
        _ if big => CharKind::Word,
        other => other,
    }
}

fn step(buffer: &EditorBuffer, from: usize, motion: Motion) -> usize {
    let len = buffer.rope.len_chars();
    let (line, column) = buffer.line_col_of(from);
    match motion {
        Motion::Left => from - column.min(1),
        Motion::Right => {
            if column < last_column(buffer, line) {
                from + 1
            } else {
                from
            }
        }
        Motion::LineStart => from - column,
        Motion::FirstNonBlank => first_non_blank(buffer, line),
        Motion::LineEnd => from - column + last_column(buffer, line),
        Motion::NextWordStart { big } => {
            let mut at = from;
            if let Some(start) = char_at(buffer, at).map(|c| kind(c, big)) {
                if start != CharKind::Whitespace {
                    while char_at(buffer, at).is_some_and(|c| kind(c, big) == start) {
                        at += 1;
                    }
                }
            }
            while char_at(buffer, at).is_some_and(|c| c.is_whitespace()) {
                // An empty line is a word of its own.
                if at > from
                    && char_at(buffer, at) == Some('\n')
                    && char_at(buffer, at - 1) == Some('\n')
                {
                    break;
                }
                at += 1;
            }
            at.min(len.saturating_sub(1))
        }
        Motion::NextWordEnd { big } => {
            let mut at = from + 1;
            while char_at(buffer, at).is_some_and(|c| c.is_whitespace()) {
                at += 1;
            }
            let Some(word) = char_at(buffer, at).map(|c| kind(c, big)) else {
                return len.saturating_sub(1).max(from);
            };
            while char_at(buffer, at + 1).is_some_and(|c| kind(c, big) == word) {
                at += 1;
            }
            at
        }
        Motion::PreviousWordStart { big } => {
            let mut at = from;
            while at > 0 && char_at(buffer, at - 1).is_some_and(|c| c.is_whitespace()) {
                at -= 1;
            }
            if at == 0 {
                return 0;
            }
            let word = char_at(buffer, at - 1).map(|c| kind(c, big));
            while at > 0 && char_at(buffer, at - 1).map(|c| kind(c, big)) == word {
                at -= 1;
            }
            at
        }
        Motion::Down | Motion::Up | Motion::FirstLine | Motion::LastLine => from,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `keys` (`<esc>` for Escape) on `before`, where `|` marks the cursor, and returns the text with the
    /// cursor marked the same way.
    fn run(vim: &mut Vim, before: &str, keys: &str) -> String {
        let cursor = before.find('|').expect("cursor");
        let mut buffer = EditorBuffer::from_text(&before.replacen('|', "", 1));
        buffer.place_cursor(before[..cursor].chars().count());
        let mut rest = keys;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix("<esc>") {
                vim.escape(&mut buffer);
                rest = after;
                continue;
            }
            let mut chars = rest.chars();
            let typed = chars.next().map(String::from).unwrap_or_default();
            if vim.text(&typed, &mut buffer) == Outcome::PassThrough {
                buffer.insert_text(&typed);
            }
            rest = chars.as_str();
        }
        let mut text: Vec<char> = buffer.text().chars().collect();
        text.insert(buffer.cursor(), '|');
        text.into_iter().collect()
    }

    fn check(before: &str, keys: &str, after: &str) {
        let mut vim = Vim::default();
        assert_eq!(run(&mut vim, before, keys), after, "{before:?} + {keys:?}");
    }

    #[test]
    fn motions_move_the_cursor() {
        check("ab|cd", "h", "a|bcd");
        check("|abcd", "h", "|abcd");
        check("ab|cd", "l", "abc|d");
        check("abc|d", "l", "abc|d");
        check("ab|cd", "3l", "abc|d");
        check("abc|d\nxy", "j", "abcd\nx|y");
        check("abcd\nxy\nabc|d", "kk", "abc|d\nxy\nabcd");
        check("a|bc\n\nxyz", "jj", "abc\n\nx|yz");
        check("|foo bar.baz", "w", "foo |bar.baz");
        check("|foo bar.baz", "ww", "foo bar|.baz");
        check("|foo bar.baz qux", "wW", "foo bar.baz |qux");
        check("|foo bar.baz qux", "2W", "foo bar.baz |qux");
        check("|foo bar", "e", "fo|o bar");
        check("fo|o bar", "e", "foo ba|r");
        check("foo ba|r", "b", "foo |bar");
        check("foo bar.b|az", "B", "foo |bar.baz");
        check("  ab|c", "0", "|  abc");
        check("  ab|c", "^", "  |abc");
        check("|abc", "$", "ab|c");
        check("a\nb\nc|c\n", "gg", "|a\nb\ncc\n");
        check("|a\n  b\nc\n", "G", "a\n  b\n|c\n");
        check("|a\n  b\nc\n", "2G", "a\n  |b\nc\n");
    }

    #[test]
    fn insert_entries_and_escape() {
        check("ab|c", "iX<esc>", "ab|Xc");
        check("ab|c", "aX<esc>", "abc|X");
        check("  ab|c", "IX<esc>", "  |Xabc");
        check("a|bc", "AX<esc>", "abc|X");
        check("  a|b\nc", "oX<esc>", "  ab\n  |X\nc");
        check("  a|b\nc", "OX<esc>", "  |X\n  ab\nc");
        check("ab|c", "i<esc>", "a|bc");
    }

    #[test]
    fn x_deletes_under_the_cursor_and_keeps_to_the_line() {
        check("a|bcd", "x", "a|cd");
        check("a|bcd", "2x", "a|d");
        check("ab|c\nd", "5x", "a|b\nd");
    }

    #[test]
    fn the_mode_shows_and_insert_types() {
        let mut vim = Vim::default();
        assert_eq!(vim.mode().label(), "-- NORMAL --");
        assert_eq!(run(&mut vim, "|", "ihi"), "hi|");
        assert_eq!(vim.mode(), Mode::Insert);
        assert_eq!(vim.mode().label(), "-- INSERT --");
    }
}
