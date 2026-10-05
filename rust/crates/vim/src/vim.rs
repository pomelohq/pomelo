//! Vim mode over an editor buffer: a per-editor state machine (mode, count, pending operator and prefix) that
//! turns typed keys in Normal and Visual mode into motions and edits, and lets Insert mode type as usual.

use std::ops::Range;

use editor::buffer::{Selection, SelectionGoal};
use editor::movement::{char_kind, CharKind};
use editor::EditorBuffer;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    /// What the status bar shows for the mode.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "-- NORMAL --",
            Mode::Insert => "-- INSERT --",
            Mode::Visual => "-- VISUAL --",
            Mode::VisualLine => "-- VISUAL LINE --",
        }
    }

    fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine)
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
    Find(FindChar),
}

/// How much of the text between the cursor and a motion's target an operator takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    /// Up to, not including, the target.
    Exclusive,
    /// Including the char at the target.
    Inclusive,
    /// Whole lines from the cursor's to the target's.
    Linewise,
}

impl Motion {
    fn reach(self) -> Reach {
        match self {
            Motion::Down | Motion::Up | Motion::FirstLine | Motion::LastLine => Reach::Linewise,
            Motion::NextWordEnd { .. } | Motion::LineEnd => Reach::Inclusive,
            Motion::Find(find) if find.forward => Reach::Inclusive,
            _ => Reach::Exclusive,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FindChar {
    target: char,
    forward: bool,
    /// `t` / `T`: stop one char short of the target.
    till: bool,
}

impl FindChar {
    fn reversed(self) -> FindChar {
        FindChar {
            forward: !self.forward,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
}

impl Operator {
    fn from_char(typed: char) -> Option<Operator> {
        Some(match typed {
            'd' => Operator::Delete,
            'c' => Operator::Change,
            'y' => Operator::Yank,
            '>' => Operator::Indent,
            '<' => Operator::Outdent,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// `g` was typed; the next key finishes a `g` command.
    G,
    /// `r` waits for the char to put under the cursor.
    Replace,
    /// `f` `t` `F` `T` wait for the char to find.
    Find { forward: bool, till: bool },
}

impl Pending {
    fn find(typed: char) -> Pending {
        Pending::Find {
            forward: typed.is_lowercase(),
            till: typed.eq_ignore_ascii_case(&'t'),
        }
    }
}

/// The text the last delete or yank took, and whether it was whole lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Register {
    text: String,
    linewise: bool,
}

/// A key of the last change, as `.` replays it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Recorded {
    Key(char),
    Typed(String),
}

/// What a Normal or Visual mode key did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Done {
    /// Waiting for more keys (a count, an operator's motion, a prefix's second key).
    Pending,
    /// Moved or yanked: nothing to repeat.
    Moved,
    /// Changed the text: `.` repeats it.
    Changed,
}

#[derive(Debug, Default)]
pub struct Vim {
    mode: Mode,
    count: Option<usize>,
    operator: Option<(Operator, usize)>,
    pending: Option<Pending>,
    /// The column `j` and `k` keep across lines shorter than it.
    goal_column: Option<usize>,
    register: Register,
    last_find: Option<FindChar>,
    /// Where Visual mode started and where its cursor is.
    visual_anchor: usize,
    visual_cursor: usize,
    /// Keys of the command being typed, then of the Insert session it opened.
    command_keys: Vec<Recorded>,
    recording_insert: bool,
    last_change: Vec<Recorded>,
    replaying: bool,
}

impl Vim {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Where the block cursor sits in Visual mode, which the selection's end does not show.
    pub fn visual_cursor(&self) -> Option<usize> {
        self.mode.is_visual().then_some(self.visual_cursor)
    }

    /// Escape: back to Normal, the cursor stepping off the end of what was typed.
    pub fn escape(&mut self, buffer: &mut EditorBuffer) -> Outcome {
        self.count = None;
        self.operator = None;
        self.pending = None;
        match self.mode {
            Mode::Insert => {
                self.mode = Mode::Normal;
                let (_, column) = buffer.line_col();
                let cursor = buffer.cursor();
                buffer.collapse_cursors();
                buffer.place_cursor(if column > 0 { cursor - 1 } else { cursor });
                if self.recording_insert {
                    self.recording_insert = false;
                    self.last_change = std::mem::take(&mut self.command_keys);
                }
            }
            Mode::Visual | Mode::VisualLine => {
                self.mode = Mode::Normal;
                buffer.collapse_cursors();
                buffer.place_cursor(clamp_normal(buffer, self.visual_cursor));
            }
            Mode::Normal => buffer.collapse_cursors(),
        }
        self.command_keys.clear();
        Outcome::Handled
    }

    /// Typed text: Insert mode types it, Normal and Visual mode read it as commands.
    pub fn text(&mut self, text: &str, buffer: &mut EditorBuffer) -> Outcome {
        if self.mode == Mode::Insert {
            self.note_typed(text);
            return Outcome::PassThrough;
        }
        for typed in text.chars() {
            self.command_char(typed, buffer);
        }
        Outcome::Handled
    }

    /// Text the editor inserts itself in Insert mode (Enter's newline), so `.` replays it too.
    pub fn note_typed(&mut self, text: &str) {
        if self.mode != Mode::Insert || !self.recording_insert || self.replaying {
            return;
        }
        match self.command_keys.last_mut() {
            Some(Recorded::Typed(typed)) => typed.push_str(text),
            _ => self.command_keys.push(Recorded::Typed(text.to_string())),
        }
    }

    fn command_char(&mut self, typed: char, buffer: &mut EditorBuffer) {
        let idle = self.pending.is_none() && self.operator.is_none() && self.count.is_none();
        if typed == '.' && self.mode == Mode::Normal && idle {
            self.command_keys.clear();
            self.repeat_last_change(buffer);
            return;
        }
        if !self.replaying {
            self.command_keys.push(Recorded::Key(typed));
        }
        let done = if self.mode.is_visual() {
            self.visual_char(typed, buffer)
        } else {
            self.normal_char(typed, buffer)
        };
        match done {
            Done::Pending => {}
            Done::Moved => self.command_keys.clear(),
            Done::Changed if self.mode == Mode::Insert => self.recording_insert = !self.replaying,
            Done::Changed => {
                if !self.replaying {
                    self.last_change = std::mem::take(&mut self.command_keys);
                }
                self.command_keys.clear();
            }
        }
    }

    fn repeat_last_change(&mut self, buffer: &mut EditorBuffer) {
        let keys = self.last_change.clone();
        if keys.is_empty() {
            return;
        }
        self.replaying = true;
        for key in &keys {
            match key {
                Recorded::Key(typed) => self.command_char(*typed, buffer),
                Recorded::Typed(text) => buffer.insert_text(text),
            }
        }
        if self.mode == Mode::Insert {
            self.escape(buffer);
        }
        self.replaying = false;
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1).max(1)
    }

    /// A count digit; `0` only continues a count (alone it is a motion).
    fn count_digit(&mut self, typed: char) -> bool {
        let Some(digit) = typed.to_digit(10) else {
            return false;
        };
        if digit == 0 && self.count.is_none() {
            return false;
        }
        let count = self.count.unwrap_or(0);
        self.count = Some(count.saturating_mul(10).saturating_add(digit as usize));
        true
    }

    /// The motion a key names on its own.
    fn motion_for(&self, typed: char) -> Option<Motion> {
        Some(match typed {
            'h' => Motion::Left,
            'l' | ' ' => Motion::Right,
            'j' => Motion::Down,
            'k' => Motion::Up,
            'w' | 'W' => Motion::NextWordStart { big: typed == 'W' },
            'e' | 'E' => Motion::NextWordEnd { big: typed == 'E' },
            'b' | 'B' => Motion::PreviousWordStart { big: typed == 'B' },
            '0' => Motion::LineStart,
            '^' => Motion::FirstNonBlank,
            '$' => Motion::LineEnd,
            'G' => Motion::LastLine,
            ';' => Motion::Find(self.last_find?),
            ',' => Motion::Find(self.last_find?.reversed()),
            _ => return None,
        })
    }

    /// The second key of a two-key motion, as the motion it finishes.
    fn finish_pending(&mut self, pending: Pending, typed: char) -> Option<Motion> {
        match pending {
            Pending::G if typed == 'g' => Some(Motion::FirstLine),
            Pending::Find { forward, till } => {
                let find = FindChar {
                    target: typed,
                    forward,
                    till,
                };
                self.last_find = Some(find);
                Some(Motion::Find(find))
            }
            _ => None,
        }
    }

    fn normal_char(&mut self, typed: char, buffer: &mut EditorBuffer) -> Done {
        if let Some(pending) = self.pending.take() {
            if pending == Pending::Replace {
                return self.replace_chars(typed, buffer);
            }
            return match self.finish_pending(pending, typed) {
                Some(motion) => self.motion_or_operator(motion, buffer),
                None => self.cancel(),
            };
        }
        if self.count_digit(typed) {
            return Done::Pending;
        }
        if let Some(motion) = self.motion_for(typed) {
            return self.motion_or_operator(motion, buffer);
        }
        if let Some(operator) = Operator::from_char(typed) {
            return match self.operator.take() {
                Some((pending, count)) if pending == operator => {
                    let count = count * self.take_count();
                    self.operate_on_lines(operator, count, buffer)
                }
                Some(_) => self.cancel(),
                None => {
                    let count = self.take_count();
                    self.operator = Some((operator, count));
                    Done::Pending
                }
            };
        }
        if self.operator.is_some() {
            return match typed {
                'g' => self.wait(Pending::G),
                'f' | 't' | 'F' | 'T' => self.wait(Pending::find(typed)),
                _ => self.cancel(),
            };
        }
        match typed {
            'g' => self.wait(Pending::G),
            'r' => self.wait(Pending::Replace),
            'f' | 't' | 'F' | 'T' => self.wait(Pending::find(typed)),
            'i' | 'a' | 'I' | 'A' | 'o' | 'O' => {
                self.enter_insert(typed, buffer);
                Done::Changed
            }
            'x' | 'X' | 's' => {
                let count = self.take_count();
                let cursor = buffer.cursor();
                let (line, column) = buffer.line_col_of(cursor);
                let range = if typed == 'X' {
                    cursor - count.min(column)..cursor
                } else {
                    cursor..cursor + count.min(buffer.line_len(line).saturating_sub(column))
                };
                let operator = if typed == 's' {
                    Operator::Change
                } else {
                    Operator::Delete
                };
                if range.is_empty() && operator == Operator::Delete {
                    return Done::Moved;
                }
                self.operate(operator, range, false, buffer)
            }
            'D' | 'C' => {
                let operator = if typed == 'D' {
                    Operator::Delete
                } else {
                    Operator::Change
                };
                self.operator = Some((operator, 1));
                self.motion_or_operator(Motion::LineEnd, buffer)
            }
            'S' | 'Y' => {
                let operator = if typed == 'S' {
                    Operator::Change
                } else {
                    Operator::Yank
                };
                let count = self.take_count();
                self.operate_on_lines(operator, count, buffer)
            }
            'p' | 'P' => self.paste(typed == 'p', buffer),
            'J' => self.join_lines(buffer),
            'u' => {
                self.count = None;
                buffer.undo();
                let cursor = clamp_normal(buffer, buffer.cursor());
                buffer.collapse_cursors();
                buffer.place_cursor(cursor);
                Done::Moved
            }
            'v' | 'V' => {
                self.count = None;
                self.mode = if typed == 'v' {
                    Mode::Visual
                } else {
                    Mode::VisualLine
                };
                self.visual_anchor = buffer.cursor();
                self.visual_cursor = self.visual_anchor;
                self.show_visual(buffer);
                Done::Moved
            }
            _ => self.cancel(),
        }
    }

    fn wait(&mut self, pending: Pending) -> Done {
        self.pending = Some(pending);
        Done::Pending
    }

    fn cancel(&mut self) -> Done {
        self.count = None;
        self.operator = None;
        self.pending = None;
        Done::Moved
    }

    fn motion_or_operator(&mut self, motion: Motion, buffer: &mut EditorBuffer) -> Done {
        let from = buffer.cursor();
        let Some((operator, count)) = self.operator.take() else {
            let target = self.target(motion, from, buffer, 1);
            buffer.collapse_cursors();
            buffer.place_cursor(clamp_normal(buffer, target));
            return Done::Moved;
        };
        match self.motion_range(operator, motion, from, count, buffer) {
            Some(range) => self.operate(operator, range, motion.reach() == Reach::Linewise, buffer),
            None => self.cancel(),
        }
    }

    /// Where `motion` lands from `from`, the typed count times `times`; a find that misses stays put.
    fn target(
        &mut self,
        motion: Motion,
        from: usize,
        buffer: &EditorBuffer,
        times: usize,
    ) -> usize {
        let explicit = self.count.is_some() || times > 1;
        let count = self.take_count() * times;
        if !matches!(motion, Motion::Down | Motion::Up) {
            self.goal_column = None;
        }
        match motion {
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
            Motion::Find(find) => find_in_line(buffer, from, find, count).unwrap_or(from),
            _ => (0..count).fold(from, |at, _| step(buffer, at, motion)),
        }
    }

    /// The text an operator with `motion` covers, or `None` when the motion goes nowhere.
    fn motion_range(
        &mut self,
        operator: Operator,
        motion: Motion,
        from: usize,
        count: usize,
        buffer: &EditorBuffer,
    ) -> Option<Range<usize>> {
        let len = buffer.rope.len_chars();
        let times = count * self.count.unwrap_or(1);
        if let Motion::NextWordStart { big } = motion {
            // `cw` on a word changes to its end, like `ce`, keeping the space after it.
            if operator == Operator::Change
                && char_at(buffer, from).is_some_and(|c| !c.is_whitespace())
            {
                self.count = None;
                let end = (0..times).fold(from, |at, index| {
                    if index == 0 && word_end_here(buffer, at, big) {
                        at
                    } else {
                        step(buffer, at, Motion::NextWordEnd { big })
                    }
                });
                return Some(from..(end + 1).min(len));
            }
        }
        if let Motion::Find(find) = motion {
            find_in_line(buffer, from, find, times)?;
        }
        let mut target = self.target(motion, from, buffer, count);
        if let Motion::NextWordStart { .. } = motion {
            // A word motion that runs onto the next line stops the operator at this line's end.
            let (line, _) = buffer.line_col_of(from);
            let (target_line, _) = buffer.line_col_of(target);
            if target_line > line {
                target = buffer.offset_at(line, buffer.line_len(line));
            } else if target + 1 == len && !word_start_here(buffer, target) {
                target = len;
            }
        }
        let (start, end) = (from.min(target), from.max(target));
        match motion.reach() {
            Reach::Linewise => {
                let (first, _) = buffer.line_col_of(start);
                let (last, _) = buffer.line_col_of(end);
                Some(line_range(buffer, first, last))
            }
            Reach::Inclusive => Some(start..(end + 1).min(len)),
            Reach::Exclusive => (start != end).then_some(start..end),
        }
    }

    fn operate_on_lines(
        &mut self,
        operator: Operator,
        count: usize,
        buffer: &mut EditorBuffer,
    ) -> Done {
        let (line, _) = buffer.line_col();
        let last = (line + count - 1).min(last_line(buffer));
        let range = line_range(buffer, line, last);
        self.operate(operator, range, true, buffer)
    }

    fn operate(
        &mut self,
        operator: Operator,
        range: Range<usize>,
        linewise: bool,
        buffer: &mut EditorBuffer,
    ) -> Done {
        self.count = None;
        let cursor = buffer.cursor();
        let (cursor_line, cursor_column) = buffer.line_col_of(cursor);
        let first_line = buffer.line_col_of(range.start).0
            + usize::from(
                linewise && range.start > 0 && char_at(buffer, range.start) == Some('\n'),
            );
        let text: String = buffer.rope.slice(range.clone()).to_string();
        if matches!(
            operator,
            Operator::Delete | Operator::Change | Operator::Yank
        ) {
            let text = match (linewise, text.strip_prefix('\n')) {
                (true, Some(rest)) => format!("{rest}\n"),
                (true, None) if !text.ends_with('\n') => format!("{text}\n"),
                _ => text.clone(),
            };
            self.register = Register { text, linewise };
        }
        buffer.collapse_cursors();
        match operator {
            Operator::Yank => {
                let cursor = if !linewise {
                    range.start
                } else if first_line < cursor_line {
                    buffer.offset_at(
                        first_line,
                        cursor_column.min(last_column(buffer, first_line)),
                    )
                } else {
                    cursor
                };
                buffer.place_cursor(cursor);
                Done::Moved
            }
            Operator::Delete => {
                buffer.replace_ranges(vec![(range.clone(), String::new())]);
                let cursor = if linewise {
                    first_non_blank(buffer, first_line.min(last_line(buffer)))
                } else {
                    clamp_normal(buffer, range.start)
                };
                buffer.place_cursor(cursor);
                Done::Changed
            }
            Operator::Change => {
                if linewise {
                    let indent = indentation(buffer, first_line);
                    let start = buffer.offset_at(first_line, 0);
                    let last = buffer.line_col_of(range.end.saturating_sub(1).max(start)).0;
                    let end = buffer.offset_at(last, buffer.line_len(last));
                    buffer.replace_ranges(vec![(start..end, indent.clone())]);
                    buffer.place_cursor(start + indent.chars().count());
                } else {
                    buffer.replace_ranges(vec![(range.clone(), String::new())]);
                    buffer.place_cursor(range.start);
                }
                self.mode = Mode::Insert;
                Done::Changed
            }
            Operator::Indent | Operator::Outdent => {
                let last = buffer
                    .line_col_of(range.end.saturating_sub(1).max(range.start))
                    .0;
                let lines =
                    buffer.offset_at(first_line, 0)..buffer.offset_at(last, buffer.line_len(last));
                buffer.select_ranges(&[lines]);
                if operator == Operator::Indent {
                    buffer.indent();
                } else {
                    buffer.outdent();
                }
                buffer.collapse_cursors();
                buffer.place_cursor(first_non_blank(buffer, first_line));
                Done::Changed
            }
        }
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
                let caret = at + indent.chars().count() + usize::from(typed == 'o');
                buffer.place_cursor(caret);
            }
            _ => {}
        }
        self.mode = Mode::Insert;
    }

    fn replace_chars(&mut self, typed: char, buffer: &mut EditorBuffer) -> Done {
        let count = self.take_count();
        let cursor = buffer.cursor();
        let (line, column) = buffer.line_col_of(cursor);
        if column + count > buffer.line_len(line) {
            return Done::Moved;
        }
        let replacement: String = std::iter::repeat_n(typed, count).collect();
        buffer.collapse_cursors();
        buffer.replace_ranges(vec![(cursor..cursor + count, replacement)]);
        buffer.place_cursor(cursor + count - 1);
        Done::Changed
    }

    fn paste(&mut self, after: bool, buffer: &mut EditorBuffer) -> Done {
        let count = self.take_count();
        if self.register.text.is_empty() {
            return Done::Moved;
        }
        let text = self.register.text.repeat(count);
        let cursor = buffer.cursor();
        let (line, column) = buffer.line_col_of(cursor);
        buffer.collapse_cursors();
        if self.register.linewise {
            let below = line + 1;
            if after && below > last_line(buffer) && !ends_with_newline(buffer) {
                let end = buffer.rope.len_chars();
                let body = text.strip_suffix('\n').unwrap_or(&text);
                buffer.replace_ranges(vec![(end..end, format!("\n{body}"))]);
                buffer.place_cursor(first_non_blank(buffer, below));
            } else {
                let target = if after { below } else { line };
                let at = buffer.offset_at(target, 0).min(buffer.rope.len_chars());
                let at = if after && below > last_line(buffer) {
                    buffer.rope.len_chars()
                } else {
                    at
                };
                buffer.replace_ranges(vec![(at..at, text)]);
                buffer.place_cursor(first_non_blank(buffer, target));
            }
        } else {
            let at = if after && buffer.line_len(line) > column {
                cursor + 1
            } else {
                cursor
            };
            let length = text.chars().count();
            buffer.replace_ranges(vec![(at..at, text)]);
            buffer.place_cursor(at + length - 1);
        }
        Done::Changed
    }

    fn join_lines(&mut self, buffer: &mut EditorBuffer) -> Done {
        let joins = self.take_count().max(2) - 1;
        let (line, _) = buffer.line_col();
        let mut cursor = buffer.cursor();
        buffer.collapse_cursors();
        for _ in 0..joins {
            if line + 1 > last_line(buffer) {
                break;
            }
            let end = buffer.offset_at(line, buffer.line_len(line));
            let next = line + 1;
            let next_indent = indentation(buffer, next).chars().count();
            let next_start = buffer.offset_at(next, 0);
            let next_blank = buffer.line_len(next) == next_indent;
            let ends_blank = end > 0 && char_at(buffer, end - 1) == Some(' ');
            let closes = char_at(buffer, next_start + next_indent) == Some(')');
            let glue = if next_blank || ends_blank || closes {
                ""
            } else {
                " "
            };
            buffer.replace_ranges(vec![(end..next_start + next_indent, glue.to_string())]);
            cursor = end;
        }
        buffer.place_cursor(clamp_normal(buffer, cursor));
        Done::Changed
    }

    fn visual_char(&mut self, typed: char, buffer: &mut EditorBuffer) -> Done {
        if let Some(pending) = self.pending.take() {
            if let Some(motion) = self.finish_pending(pending, typed) {
                self.move_visual(motion, buffer);
            }
            return Done::Moved;
        }
        if self.count_digit(typed) {
            return Done::Pending;
        }
        if let Some(motion) = self.motion_for(typed) {
            self.move_visual(motion, buffer);
            return Done::Moved;
        }
        let operator = match typed {
            'd' | 'x' => Some(Operator::Delete),
            'c' | 's' => Some(Operator::Change),
            'y' => Some(Operator::Yank),
            '>' => Some(Operator::Indent),
            '<' => Some(Operator::Outdent),
            _ => None,
        };
        if let Some(operator) = operator {
            let range = self.visual_range(buffer);
            let linewise = self.mode == Mode::VisualLine;
            self.mode = Mode::Normal;
            buffer.collapse_cursors();
            buffer.place_cursor(self.visual_anchor.min(self.visual_cursor));
            return self.operate(operator, range, linewise, buffer);
        }
        match typed {
            'g' => self.wait(Pending::G),
            'f' | 't' | 'F' | 'T' => self.wait(Pending::find(typed)),
            'v' | 'V' => {
                let wanted = if typed == 'v' {
                    Mode::Visual
                } else {
                    Mode::VisualLine
                };
                if self.mode == wanted {
                    self.escape(buffer);
                } else {
                    self.mode = wanted;
                    self.show_visual(buffer);
                }
                Done::Moved
            }
            'o' => {
                std::mem::swap(&mut self.visual_anchor, &mut self.visual_cursor);
                self.show_visual(buffer);
                Done::Moved
            }
            _ => self.cancel(),
        }
    }

    fn move_visual(&mut self, motion: Motion, buffer: &mut EditorBuffer) {
        let target = self.target(motion, self.visual_cursor, buffer, 1);
        self.visual_cursor = clamp_normal(buffer, target);
        self.show_visual(buffer);
    }

    fn visual_range(&self, buffer: &EditorBuffer) -> Range<usize> {
        let start = self.visual_anchor.min(self.visual_cursor);
        let end = self.visual_anchor.max(self.visual_cursor);
        if self.mode == Mode::VisualLine {
            let (first, _) = buffer.line_col_of(start);
            let (last, _) = buffer.line_col_of(end);
            line_range(buffer, first, last)
        } else {
            start..(end + 1).min(buffer.rope.len_chars())
        }
    }

    fn show_visual(&self, buffer: &mut EditorBuffer) {
        let range = self.visual_range(buffer);
        buffer.set_selections(vec![Selection {
            start: range.start,
            end: range.end,
            reversed: self.visual_cursor < self.visual_anchor,
            goal: SelectionGoal::None,
            ..Selection::default()
        }]);
    }
}

fn ends_with_newline(buffer: &EditorBuffer) -> bool {
    let len = buffer.rope.len_chars();
    len > 0 && buffer.rope.char(len - 1) == '\n'
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

/// Lines `first..=last` with their newlines; the text's last line takes the newline before it instead when it
/// has none of its own.
fn line_range(buffer: &EditorBuffer, first: usize, last: usize) -> Range<usize> {
    let start = buffer.offset_at(first, 0);
    let len = buffer.rope.len_chars();
    if last < last_line(buffer) {
        return start..buffer.offset_at(last + 1, 0);
    }
    if ends_with_newline(buffer) || first == 0 {
        start..len
    } else {
        start - 1..len
    }
}

/// The last column Normal mode's block cursor can sit on.
fn last_column(buffer: &EditorBuffer, line: usize) -> usize {
    buffer.line_len(line).saturating_sub(1)
}

fn clamp_normal(buffer: &EditorBuffer, offset: usize) -> usize {
    let (line, column) = buffer.line_col_of(offset);
    let line = line.min(last_line(buffer));
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

fn word_end_here(buffer: &EditorBuffer, at: usize, big: bool) -> bool {
    let Some(here) = char_at(buffer, at).map(|c| kind(c, big)) else {
        return true;
    };
    char_at(buffer, at + 1).map(|c| kind(c, big)) != Some(here)
}

fn word_start_here(buffer: &EditorBuffer, at: usize) -> bool {
    let here = char_at(buffer, at).map(|c| kind(c, false));
    at == 0 || char_at(buffer, at - 1).map(|c| kind(c, false)) != here
}

/// The `count`-th `find.target` in the cursor's line in `find`'s direction, where the find lands on it.
fn find_in_line(buffer: &EditorBuffer, from: usize, find: FindChar, count: usize) -> Option<usize> {
    let (line, column) = buffer.line_col_of(from);
    let start = buffer.offset_at(line, 0);
    let length = buffer.line_len(line);
    let mut found = 0;
    let mut at = column;
    loop {
        at = if find.forward {
            Some(at + 1).filter(|next| *next < length)?
        } else {
            at.checked_sub(1)?
        };
        if char_at(buffer, start + at) != Some(find.target) {
            continue;
        }
        found += 1;
        if found == count {
            let landing = match (find.till, find.forward) {
                (false, _) => at,
                (true, true) => at - 1,
                (true, false) => at + 1,
            };
            return Some(start + landing);
        }
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
        Motion::Down | Motion::Up | Motion::FirstLine | Motion::LastLine | Motion::Find(_) => from,
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
    fn operators_take_their_motion() {
        check("foo |bar baz", "dw", "foo |baz");
        check("foo |bar baz", "d2w", "foo| ");
        check("foo |bar baz", "2dw", "foo| ");
        check("foo |bar\nbaz", "dw", "foo| \nbaz");
        check("foo |bar baz", "de", "foo | baz");
        check("foo b|ar baz", "db", "foo |ar baz");
        check("foo |bar baz", "d$", "foo| ");
        check("foo |bar baz", "D", "foo| ");
        check("foo b|ar(x)", "dt(", "foo b|(x)");
        check("foo b|ar(x)", "df(", "foo b|x)");
        check("a(b|c)", "dF(", "a|c)");
        check("foo |bar baz", "cwX<esc>", "foo |X baz");
        check("foo |bar baz", "CX<esc>", "foo |X");
        check("foo |bar baz", "sX<esc>", "foo |Xar baz");
        check("a|bc", "X", "|bc");
    }

    #[test]
    fn line_operators_and_paste() {
        check("a\n|b\nc\n", "dd", "a\n|c\n");
        check("a\nb\n|c", "dd", "a\n|b");
        check("|a\nb\nc\n", "2dd", "|c\n");
        check("|a\nb\nc\n", "dj", "|c\n");
        check("a\nb\n|c\n", "dk", "|a\n");
        check("|a\nb\n", "ddp", "b\n|a\n");
        check("a\n|b\n", "yyP", "a\n|b\nb\n");
        check("|a\nb", "yyjp", "a\nb\n|a");
        check("|ab", "xp", "b|a");
        check("|ab cd", "ywP", "ab| ab cd");
        check("  |a\nb\n", "ccX<esc>", "  |X\nb\n");
        check("|a\nb\n", "SX<esc>", "|X\nb\n");
    }

    #[test]
    fn small_edits() {
        check("a|bc", "rX", "a|Xc");
        check("a|bcd", "2rX", "aX|Xd");
        check("|a\n  b\nc", "J", "a| b\nc");
        check("|a\nb\nc", "3J", "a b| c");
        check("|a", ">>", "    |a");
        check("    |a", "<<", "|a");
        check("|abc", "xu", "|abc");
    }

    #[test]
    fn dot_repeats_the_last_change() {
        check("|a b c d", "dw.", "|c d");
        check("|a\nb\nc\n", "dd.", "|c\n");
        check("|ab", "iX<esc>l.", "X|Xab");
        check("|a\nb\n", "AZ<esc>j.", "aZ\nb|Z\n");
        check("|abc", "x.", "|c");
    }

    #[test]
    fn visual_mode_selects_then_acts() {
        check("|abcd", "vld", "|cd");
        check("|abcd", "vlly", "|abcd");
        check("ab|cd", "vhd", "a|d");
        check("|a\nb\nc\n", "Vjd", "|c\n");
        check("|a\nb\n", "VyP", "|a\na\nb\n");
        check("|abcd", "vlcX<esc>", "|Xcd");
        check("|a\nb\n", "Vj>", "    |a\n    b\n");
        check("ab|c", "v<esc>", "ab|c");
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
