//! Servers count columns in UTF-16 code units; the editor counts chars.

use lsp_types::Position;
use ropey::Rope;

pub fn char_to_position(rope: &Rope, offset: usize) -> Position {
    let offset = offset.min(rope.len_chars());
    let line = rope.char_to_line(offset);
    let line_start = rope.line_to_char(line);
    let column = rope.char_to_utf16_cu(offset) - rope.char_to_utf16_cu(line_start);
    Position::new(line as u32, column as u32)
}

/// The char a position names, clamped into the text: past the last line is the end, past a line's end is
/// that line's end.
pub fn position_to_char(rope: &Rope, position: Position) -> usize {
    let line = position.line as usize;
    if line >= rope.len_lines() {
        return rope.len_chars();
    }
    let line_start = rope.line_to_char(line);
    let slice = rope.line(line);
    let mut content_chars = slice.len_chars();
    if content_chars > 0 && slice.char(content_chars - 1) == '\n' {
        content_chars -= 1;
    }
    let line_end = line_start + content_chars;
    let start_units = rope.char_to_utf16_cu(line_start);
    let target = (start_units + position.character as usize).min(rope.char_to_utf16_cu(line_end));
    rope.utf16_cu_to_char(target).clamp(line_start, line_end)
}

/// A diagnostic's range in chars, widened so something is always underlined: an empty range takes the
/// next char (or the previous one at a line end), and a range covering only a line break shrinks to the
/// line's last char.
pub fn diagnostic_char_range(rope: &Rope, range: lsp_types::Range) -> std::ops::Range<usize> {
    let mut start = position_to_char(rope, range.start);
    let mut end = position_to_char(rope, range.end).max(start);
    let line_end = |offset: usize| {
        let line = rope.char_to_line(offset);
        position_to_char(rope, Position::new(line as u32, u32::MAX))
    };
    let at_line_start = |offset: usize| rope.line_to_char(rope.char_to_line(offset)) == offset;
    if start == end {
        if end < line_end(start) {
            end += 1;
        } else if !at_line_start(start) {
            start -= 1;
        }
    } else if range.end == Position::new(range.start.line + 1, 0) && start == line_end(start) {
        end = start;
        if !at_line_start(start) {
            start -= 1;
        }
    }
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_utf16_units() {
        let rope = Rope::from_str("ab\nx\u{1F600}y\n");
        assert_eq!(char_to_position(&rope, 1), Position::new(0, 1));
        assert_eq!(char_to_position(&rope, 5), Position::new(1, 3));
        assert_eq!(char_to_position(&rope, 6), Position::new(1, 4));
        assert_eq!(position_to_char(&rope, Position::new(1, 3)), 5);
        assert_eq!(position_to_char(&rope, Position::new(1, 1)), 4);
    }

    #[test]
    fn out_of_range_positions_clamp() {
        let rope = Rope::from_str("ab\ncd");
        assert_eq!(position_to_char(&rope, Position::new(0, 99)), 2);
        assert_eq!(position_to_char(&rope, Position::new(9, 0)), 5);
        assert_eq!(char_to_position(&rope, 99), Position::new(1, 2));
    }

    fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
        lsp_types::Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))
    }

    #[test]
    fn diagnostics_always_cover_a_char() {
        let rope = Rope::from_str("let x\ny");
        assert_eq!(diagnostic_char_range(&rope, range((0, 4), (0, 5))), 4..5);
        assert_eq!(diagnostic_char_range(&rope, range((0, 2), (0, 2))), 2..3);
        assert_eq!(diagnostic_char_range(&rope, range((0, 5), (0, 5))), 4..5);
        assert_eq!(diagnostic_char_range(&rope, range((0, 5), (1, 0))), 4..5);
        assert_eq!(diagnostic_char_range(&rope, range((0, 0), (1, 1))), 0..7);
    }
}
