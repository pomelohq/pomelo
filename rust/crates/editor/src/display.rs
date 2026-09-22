//! Display-space mapping for one buffer line: hard tabs expand to the next tab stop. A tab starting at display
//! column `c` spans `tab_size - c % tab_size` columns; a tab at buffer byte column `MAX_EXPANSION_COLUMN` or
//! later renders as a single space, so pathological lines stay cheap. Display positions are byte indices into
//! the expanded text (tabs become spaces), matching how the row is shaped.

use crate::buffer::Bias;
use ropey::RopeSlice;

pub const MAX_EXPANSION_COLUMN: usize = 256;

fn tab_width(tab_size: usize, display_col: usize, buffer_byte: usize) -> usize {
    if buffer_byte >= MAX_EXPANSION_COLUMN {
        1
    } else {
        tab_size - display_col % tab_size
    }
}

/// Expand `text` onto `out`. `display_col` / `buffer_byte` carry the position within the line across calls.
pub fn push_expanded(
    out: &mut String,
    text: &str,
    tab_size: usize,
    display_col: &mut usize,
    buffer_byte: &mut usize,
) {
    for ch in text.chars() {
        if ch == '\t' {
            let width = tab_width(tab_size, *display_col, *buffer_byte);
            out.extend(std::iter::repeat_n(' ', width));
            *display_col += width;
        } else {
            out.push(ch);
            *display_col += 1;
        }
        *buffer_byte += ch.len_utf8();
    }
}

/// `(display column, display byte index)` of buffer char column `char_col` on `line`.
pub fn to_display(line: RopeSlice, char_col: usize, tab_size: usize) -> (usize, usize) {
    let (mut col, mut index, mut byte) = (0usize, 0usize, 0usize);
    for (i, ch) in line.chars().enumerate() {
        if i >= char_col || ch == '\n' {
            break;
        }
        if ch == '\t' {
            let width = tab_width(tab_size, col, byte);
            col += width;
            index += width;
        } else {
            col += 1;
            index += ch.len_utf8();
        }
        byte += ch.len_utf8();
    }
    (col, index)
}

/// Buffer char column for a display byte index. An index inside a tab's expansion resolves to before the tab
/// (`Left`) or after it (`Right`).
pub fn from_display_index(line: RopeSlice, index: usize, tab_size: usize, bias: Bias) -> usize {
    let (mut col, mut at, mut byte, mut char_col) = (0usize, 0usize, 0usize, 0usize);
    for ch in line.chars() {
        if ch == '\n' || at >= index {
            break;
        }
        let (width_cols, width_bytes) = if ch == '\t' {
            let w = tab_width(tab_size, col, byte);
            (w, w)
        } else {
            (1, ch.len_utf8())
        };
        if index < at + width_bytes {
            return match bias {
                Bias::Left => char_col,
                Bias::Right => char_col + 1,
            };
        }
        at += width_bytes;
        col += width_cols;
        byte += ch.len_utf8();
        char_col += 1;
    }
    char_col
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn tabs_expand_to_stops() {
        let rope = Rope::from_str("a\tb\t\tc");
        let line = rope.line(0);
        assert_eq!(to_display(line, 2, 4), (4, 4));
        assert_eq!(to_display(line, 4, 4), (8, 8));
        assert_eq!(to_display(line, 5, 4), (12, 12));
        let mut out = String::new();
        let (mut col, mut byte) = (0, 0);
        push_expanded(&mut out, "a\tb\t\tc", 4, &mut col, &mut byte);
        assert_eq!(out, "a   b       c");
    }

    #[test]
    fn index_inside_tab_follows_bias() {
        let rope = Rope::from_str("\tx");
        let line = rope.line(0);
        assert_eq!(from_display_index(line, 2, 4, Bias::Left), 0);
        assert_eq!(from_display_index(line, 2, 4, Bias::Right), 1);
        assert_eq!(from_display_index(line, 4, 4, Bias::Left), 1);
        assert_eq!(from_display_index(line, 5, 4, Bias::Left), 2);
    }

    #[test]
    fn tabs_past_expansion_limit_are_one_space() {
        let text = format!("{}\t.", "x".repeat(MAX_EXPANSION_COLUMN));
        let rope = Rope::from_str(&text);
        let line = rope.line(0);
        assert_eq!(
            to_display(line, MAX_EXPANSION_COLUMN + 1, 4).0,
            MAX_EXPANSION_COLUMN + 1
        );
    }

    #[test]
    fn multibyte_chars_count_one_column() {
        let rope = Rope::from_str("\u{e9}\tz");
        let line = rope.line(0);
        assert_eq!(to_display(line, 1, 4), (1, 2));
        assert_eq!(to_display(line, 2, 4), (4, 5));
        assert_eq!(from_display_index(line, 5, 4, Bias::Left), 2);
    }
}
