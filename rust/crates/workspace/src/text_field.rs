//! A one-line text input on an editor buffer, for fields like the search query or go-to-line.

use std::ops::Range;

use crate::EditKey;
use editor::buffer::{Deletion, LineRows, Motion};
use editor::EditorBuffer;
use ui::{div, label, theme, Node};

pub const INPUT_FONT: f32 = 14.0;

/// One-line fields use the UI font like other single-line inputs; search queries use the code font.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FieldFont {
    Ui,
    Mono,
}

impl FieldFont {
    /// Line height as a multiple of the font size.
    pub fn line_height(self) -> f32 {
        match self {
            FieldFont::Ui => 1.618,
            FieldFont::Mono => 1.3,
        }
    }
}

/// A one-line text field backed by an editor buffer, so it has the editor's caret and selection motions.
#[derive(Default)]
pub struct TextField {
    buffer: EditorBuffer,
    font_size: Option<f32>,
}

impl TextField {
    pub fn text(&self) -> String {
        self.buffer.text()
    }

    pub fn set_text(&mut self, text: &str) {
        self.buffer = EditorBuffer::from_text(text);
        self.buffer.select_all();
    }

    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = Some(size);
    }

    pub fn select_range(&mut self, range: Range<usize>) {
        self.buffer.select_ranges(&[range]);
    }

    pub fn select_all(&mut self) {
        self.buffer.select_all();
    }

    pub fn move_to_end(&mut self) {
        let end = self.buffer.rope.len_chars();
        self.buffer.place_cursor(end);
    }

    pub fn selected_text(&self) -> Option<String> {
        self.buffer.selected_text()
    }

    pub fn insert(&mut self, text: &str) {
        self.buffer.insert_text(text);
    }

    /// Apply an editing key; returns whether the text changed.
    pub fn key(&mut self, key: EditKey, shift: bool) -> bool {
        let before = self.buffer.version();
        let motion = match key {
            EditKey::Left => Some(Motion::Left),
            EditKey::Right => Some(Motion::Right),
            EditKey::WordLeft => Some(Motion::WordLeft),
            EditKey::WordRight => Some(Motion::WordRight),
            EditKey::SubwordLeft => Some(Motion::SubwordLeft),
            EditKey::SubwordRight => Some(Motion::SubwordRight),
            EditKey::Home | EditKey::LineStart | EditKey::DocumentStart => Some(Motion::LineStart),
            EditKey::End | EditKey::LineEnd | EditKey::DocumentEnd => Some(Motion::LineEnd),
            _ => None,
        };
        if let Some(motion) = motion {
            self.buffer.apply_motion(motion, shift);
            return false;
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
        if let Some(deletion) = deletion {
            let grown = self
                .buffer
                .deletion_selections(deletion, &LineRows(&self.buffer));
            self.buffer.delete_selections(grown);
        } else {
            match key {
                EditKey::SelectAll => self.buffer.select_all(),
                EditKey::Undo => self.buffer.undo(),
                EditKey::Redo => self.buffer.redo(),
                _ => {}
            }
        }
        self.buffer.version() != before
    }

    /// The field's text laid out on one row: the selection tinted and a 2px caret at the head while focused.
    pub fn render(
        &self,
        placeholder: &str,
        focused: bool,
        color: ui::Rgba,
        height: f32,
        font: FieldFont,
    ) -> Node {
        let text = self.buffer.text();
        let size = self.font_size.unwrap_or(INPUT_FONT);
        let mut row = div().row().items_center().flex(1.0).h_px(height);
        let styled = |text: String| {
            let label = label(text).size(size);
            if font == FieldFont::Mono {
                label.mono()
            } else {
                label
            }
        };
        let caret = |visible: bool| {
            div()
                .w_px(2.0)
                .h_px(size * font.line_height())
                .bg(if visible {
                    theme().player_cursor
                } else {
                    ui::Rgba::TRANSPARENT
                })
        };
        let show_caret = focused && ui::caret_phase();
        if text.is_empty() {
            return row
                .child(caret(show_caret))
                .child(styled(placeholder.to_string()).color(theme().text_placeholder))
                .into();
        }
        let selection = self.buffer.newest();
        let chars: Vec<char> = text.chars().collect();
        let piece = |range: Range<usize>| -> String { chars[range].iter().collect() };
        let head = selection.head();
        let (start, end) = (selection.start, selection.end);
        let push = |row: ui::Div, range: Range<usize>, selected: bool| -> ui::Div {
            if range.is_empty() {
                return row;
            }
            let text = styled(piece(range)).color(color);
            if selected && focused {
                let st = crate::syntax_theme();
                let [r, g, b] = st.selection.0;
                let tint = ui::Rgba::new(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    st.selection_alpha,
                );
                row.child(div().bg(tint).child(text))
            } else {
                row.child(text)
            }
        };
        row = push(row, 0..start, false);
        if head == start {
            row = row.child(caret(show_caret));
        }
        row = push(row, start..end, true);
        if head == end && end != start {
            row = row.child(caret(show_caret));
        }
        row = push(row, end..chars.len(), false);
        row.into()
    }
}

struct AreaRow {
    start: usize,
    end: usize,
    soft: bool,
    xs: Vec<f32>,
}

pub struct TextArea {
    buffer: EditorBuffer,
    font_size: f32,
    rows: Vec<AreaRow>,
    wrap_width: f32,
    laid_out_version: Option<u64>,
    scroll_row: usize,
    visible_rows: usize,
}

impl Default for TextArea {
    fn default() -> Self {
        TextArea {
            buffer: EditorBuffer::default(),
            font_size: 13.0,
            rows: Vec::new(),
            wrap_width: 0.0,
            laid_out_version: None,
            scroll_row: 0,
            visible_rows: 1,
        }
    }
}

struct AreaRows<'a> {
    rows: &'a [AreaRow],
    buffer: &'a EditorBuffer,
}

impl editor::buffer::DisplayRows for AreaRows<'_> {
    fn clip(&self, offset: usize, _bias: editor::buffer::Bias) -> usize {
        offset.min(self.buffer.rope.len_chars())
    }
    fn row_of(&self, offset: usize) -> usize {
        self.rows
            .partition_point(|row| row.start <= offset)
            .saturating_sub(1)
    }
    fn max_row(&self) -> usize {
        self.rows.len().saturating_sub(1)
    }
    fn row_start(&self, row: usize) -> usize {
        self.rows.get(row).map_or(0, |row| row.start)
    }
    fn row_end(&self, row: usize) -> usize {
        self.rows.get(row).map_or(0, |row| {
            if row.soft {
                row.end.saturating_sub(1).max(row.start)
            } else {
                row.end
            }
        })
    }
    fn line_start(&self, offset: usize) -> usize {
        let rope = &self.buffer.rope;
        rope.line_to_char(rope.char_to_line(offset.min(rope.len_chars())))
    }
    fn line_end(&self, offset: usize) -> usize {
        let rope = &self.buffer.rope;
        let line = rope.char_to_line(offset.min(rope.len_chars()));
        let start = rope.line_to_char(line);
        let text = rope.line(line);
        let newline = text.chars().filter(|c| *c == '\n' || *c == '\r').count();
        start + text.len_chars() - newline
    }
    fn x_of(&self, offset: usize) -> f32 {
        let row = &self.rows[self.row_of(offset).min(self.rows.len().saturating_sub(1))];
        row.xs
            .get(offset.saturating_sub(row.start))
            .copied()
            .unwrap_or_default()
    }
    fn offset_for_x(&self, row: usize, x: f32) -> usize {
        let Some(area_row) = self.rows.get(row) else {
            return 0;
        };
        let last = self.row_end(row) - area_row.start;
        let column = (0..=last)
            .min_by(|a, b| {
                let da = (area_row.xs[*a] - x).abs();
                let db = (area_row.xs[*b] - x).abs();
                da.total_cmp(&db)
            })
            .unwrap_or(0);
        area_row.start + column
    }
}

impl TextArea {
    pub fn text(&self) -> String {
        self.buffer.text()
    }

    pub fn set_text(&mut self, text: &str) {
        self.buffer = EditorBuffer::from_text(text);
        let end = self.buffer.rope.len_chars();
        self.buffer.place_cursor(end);
        self.laid_out_version = None;
    }

    pub fn set_font_size(&mut self, size: f32) {
        self.font_size = size;
        self.laid_out_version = None;
    }

    pub fn line_height(&self) -> f32 {
        self.font_size * FieldFont::Mono.line_height()
    }

    pub fn selected_text(&self) -> Option<String> {
        self.buffer.selected_text()
    }

    pub fn insert(&mut self, text: &str) {
        self.buffer.insert_text(&text.replace("\r\n", "\n"));
    }

    fn layout(&mut self, width: f32) {
        let version = self.buffer.version();
        if self.laid_out_version == Some(version) && (self.wrap_width - width).abs() < 0.5 {
            return;
        }
        self.laid_out_version = Some(version);
        self.wrap_width = width;
        let scale = ui::ui_text_scale().max(0.01);
        let rope = &self.buffer.rope;
        let mut rows = Vec::new();
        for line in 0..rope.len_lines() {
            let start = rope.line_to_char(line);
            let text: String = rope
                .line(line)
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .collect();
            let (glyphs, advance) = ui::measure_glyphs(&text, self.font_size, true, 400);
            let mut xs = Vec::with_capacity(text.chars().count() + 1);
            let mut glyph = 0;
            let mut x = 0.0;
            for (byte, _) in text.char_indices() {
                while glyph < glyphs.len() && glyphs[glyph].0 <= byte {
                    x = glyphs[glyph].1 / scale;
                    glyph += 1;
                }
                xs.push(x);
            }
            xs.push(advance / scale);
            let chars: Vec<char> = text.chars().collect();
            let mut row_start = 0;
            while chars.len() - row_start > 0 || row_start == 0 {
                let base = xs[row_start];
                let fits = (row_start..chars.len())
                    .take_while(|index| xs[index + 1] - base <= width || *index == row_start)
                    .count();
                let mut row_end = row_start + fits;
                if row_end < chars.len() {
                    if let Some(space) = (row_start + 1..row_end)
                        .rev()
                        .find(|index| chars[*index - 1] == ' ')
                    {
                        row_end = space;
                    }
                }
                let soft = row_end < chars.len();
                rows.push(AreaRow {
                    start: start + row_start,
                    end: start + row_end,
                    soft,
                    xs: xs[row_start..=row_end].iter().map(|x| x - base).collect(),
                });
                if !soft {
                    break;
                }
                row_start = row_end;
            }
        }
        self.rows = rows;
    }

    fn rows(&self) -> AreaRows<'_> {
        AreaRows {
            rows: &self.rows,
            buffer: &self.buffer,
        }
    }

    fn keep_caret_visible(&mut self) {
        let head = self.buffer.newest().head();
        let row = editor::buffer::DisplayRows::row_of(&self.rows(), head);
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + self.visible_rows {
            self.scroll_row = row + 1 - self.visible_rows;
        }
    }

    pub fn click(&mut self, x: f32, y: f32) {
        if self.rows.is_empty() {
            return;
        }
        let row =
            (self.scroll_row + (y.max(0.0) / self.line_height()) as usize).min(self.rows.len() - 1);
        let offset = editor::buffer::DisplayRows::offset_for_x(&self.rows(), row, x);
        self.buffer.place_cursor(offset);
    }

    pub fn scroll(&mut self, rows: isize) -> bool {
        let max = self.rows.len().saturating_sub(self.visible_rows);
        let next = (self.scroll_row as isize + rows).clamp(0, max as isize) as usize;
        let moved = next != self.scroll_row;
        self.scroll_row = next;
        moved
    }

    pub fn key(&mut self, key: EditKey, shift: bool) -> bool {
        let before = self.buffer.version();
        let motion = match key {
            EditKey::Left => Some(Motion::Left),
            EditKey::Right => Some(Motion::Right),
            EditKey::Up => Some(Motion::Up),
            EditKey::Down => Some(Motion::Down),
            EditKey::WordLeft => Some(Motion::WordLeft),
            EditKey::WordRight => Some(Motion::WordRight),
            EditKey::SubwordLeft => Some(Motion::SubwordLeft),
            EditKey::SubwordRight => Some(Motion::SubwordRight),
            EditKey::Home => Some(Motion::Home),
            EditKey::End => Some(Motion::End),
            EditKey::LineStart => Some(Motion::LineStart),
            EditKey::LineEnd => Some(Motion::LineEnd),
            EditKey::DocumentStart => Some(Motion::DocumentStart),
            EditKey::DocumentEnd => Some(Motion::DocumentEnd),
            _ => None,
        };
        if let Some(motion) = motion {
            let moved = self.buffer.moved_selections(motion, shift, &self.rows());
            self.buffer.set_selections(moved);
            self.keep_caret_visible();
            return false;
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
        if let Some(deletion) = deletion {
            let grown = self
                .buffer
                .deletion_selections(deletion, &LineRows(&self.buffer));
            self.buffer.delete_selections(grown);
        } else {
            match key {
                EditKey::Enter => self.buffer.insert_text("\n"),
                EditKey::SelectAll => self.buffer.select_all(),
                EditKey::Undo => self.buffer.undo(),
                EditKey::Redo => self.buffer.redo(),
                _ => {}
            }
        }
        self.buffer.version() != before
    }

    pub fn render(&mut self, placeholder: &str, focused: bool, width: f32, rows: usize) -> Node {
        self.visible_rows = rows.max(1);
        self.layout(width);
        self.keep_caret_visible();
        let size = self.font_size;
        let line_h = self.line_height();
        let text_color = theme().text;
        let show_caret = focused && ui::caret_phase();
        let caret = |visible: bool| {
            div().w_px(2.0).h_px(line_h).bg(if visible {
                theme().player_cursor
            } else {
                ui::Rgba::TRANSPARENT
            })
        };
        let mut column = div()
            .col()
            .w_px(width)
            .h_px(line_h * self.visible_rows as f32);
        if self.buffer.rope.len_chars() == 0 {
            let placeholder_row = div()
                .row()
                .h_px(line_h)
                .items_center()
                .child(caret(show_caret))
                .child(
                    label(placeholder.to_string())
                        .size(size)
                        .mono()
                        .color(theme().text_placeholder),
                );
            return column.child(placeholder_row).into();
        }
        let selection = self.buffer.newest();
        let (start, end, head) = (selection.start, selection.end, selection.head());
        let st = crate::syntax_theme();
        let [r, g, b] = st.selection.0;
        let tint = ui::Rgba::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            st.selection_alpha,
        );
        let last_row = self.rows.len().saturating_sub(1);
        for (index, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll_row)
            .take(self.visible_rows)
        {
            let text: String = self.buffer.rope.slice(row.start..row.end).chars().collect();
            let chars: Vec<char> = text.chars().collect();
            let piece = |from: usize, to: usize| -> String {
                chars[from.saturating_sub(row.start).min(chars.len())
                    ..to.saturating_sub(row.start).min(chars.len())]
                    .iter()
                    .collect()
            };
            let mut line = div().row().h_px(line_h).items_center();
            let caret_here = head >= row.start
                && (head < row.end || (head == row.end && (!row.soft || index == last_row)));
            let cut_start = start.clamp(row.start, row.end);
            let cut_end = end.clamp(row.start, row.end);
            for (from, to, selected) in [
                (row.start, cut_start, false),
                (cut_start, cut_end, focused && start != end),
                (cut_end, row.end, false),
            ] {
                if caret_here && head == from && (from != to || from == row.end || selected) {
                    line = line.child(caret(show_caret));
                }
                if from == to {
                    continue;
                }
                let text = label(piece(from, to)).size(size).mono().color(text_color);
                line = if selected {
                    line.child(div().bg(tint).child(text))
                } else {
                    line.child(text)
                };
            }
            if caret_here && head == row.end && row.end != row.start && cut_end != row.end {
                line = line.child(caret(show_caret));
            }
            column = column.child(line);
        }
        column.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_texts(area: &TextArea) -> Vec<String> {
        area.rows
            .iter()
            .map(|row| area.buffer.rope.slice(row.start..row.end).to_string())
            .collect()
    }

    #[test]
    fn long_lines_wrap_at_spaces_and_newlines_start_rows() {
        let mut area = TextArea::default();
        area.set_text("alpha beta gamma delta\nsecond");
        let char_w = ui::measure_text_width("m", 13.0, true, 400) / ui::ui_text_scale();
        area.render("", false, char_w * 12.0, 6);
        assert_eq!(row_texts(&area), ["alpha beta ", "gamma delta", "second"]);
        assert!(area.rows[0].soft && !area.rows[1].soft);
    }

    #[test]
    fn up_and_down_move_between_wrapped_rows_and_enter_breaks_the_line() {
        let mut area = TextArea::default();
        area.set_text("alpha beta gamma");
        let char_w = ui::measure_text_width("m", 13.0, true, 400) / ui::ui_text_scale();
        area.render("", true, char_w * 12.0, 6);
        area.key(EditKey::DocumentStart, false);
        area.key(EditKey::Down, false);
        let head = area.buffer.newest().head();
        assert!(
            head >= "alpha beta ".len(),
            "moved to the wrapped row: {head}"
        );
        area.key(EditKey::Up, false);
        assert!(area.buffer.newest().head() < "alpha beta ".len());
        area.key(EditKey::DocumentEnd, false);
        assert!(area.key(EditKey::Enter, false));
        assert_eq!(area.text(), "alpha beta gamma\n");
    }
}
