//! Word and subword boundaries over char offsets. A boundary sits between two chars (`left`, `right`); word
//! motions stop where the char kind changes, skipping whitespace on the far side, and subword motions also stop
//! at `_` runs and lower-to-upper case changes (`fooBar_baz` -> `foo|Bar|_|baz`).

use crate::buffer::EditorBuffer;
use std::ops::Range;

/// Ordered so the "strongest" kind next to a position wins (word over punctuation over whitespace).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CharKind {
    Whitespace,
    Punctuation,
    Word,
}

pub fn char_kind(c: char) -> CharKind {
    if c.is_alphanumeric() || c == '_' {
        CharKind::Word
    } else if c.is_whitespace() {
        CharKind::Whitespace
    } else {
        CharKind::Punctuation
    }
}

/// Walk left from `from` to the first boundary `is_boundary(left, right)` accepts, or the start of the text.
fn find_preceding_boundary(
    buffer: &EditorBuffer,
    from: usize,
    mut is_boundary: impl FnMut(char, char) -> bool,
) -> usize {
    let mut offset = from.min(buffer.rope.len_chars());
    let mut chars = buffer.rope.chars_at(offset);
    let mut right = None;
    while let Some(left) = chars.prev() {
        if right.is_some_and(|right| is_boundary(left, right)) {
            break;
        }
        offset -= 1;
        right = Some(left);
    }
    offset
}

/// Walk right from `from` to the first boundary `is_boundary(left, right)` accepts, or the end of the text.
fn find_boundary(
    buffer: &EditorBuffer,
    from: usize,
    mut is_boundary: impl FnMut(char, char) -> bool,
) -> usize {
    let mut offset = from.min(buffer.rope.len_chars());
    let mut left = None;
    for right in buffer.rope.chars_at(offset) {
        if left.is_some_and(|left| is_boundary(left, right)) {
            break;
        }
        offset += 1;
        left = Some(right);
    }
    offset
}

fn kind_changes(left: char, right: char) -> bool {
    char_kind(left) != char_kind(right)
}

pub fn previous_word_start(buffer: &EditorBuffer, from: usize) -> usize {
    find_preceding_boundary(buffer, from, |left, right| {
        (kind_changes(left, right) && !right.is_whitespace()) || left == '\n'
    })
}

pub fn previous_word_start_or_newline(buffer: &EditorBuffer, from: usize) -> usize {
    find_preceding_boundary(buffer, from, |left, right| {
        (kind_changes(left, right) && !right.is_whitespace()) || left == '\n' || right == '\n'
    })
}

pub fn next_word_end(buffer: &EditorBuffer, from: usize) -> usize {
    find_boundary(buffer, from, |left, right| {
        (kind_changes(left, right) && !left.is_whitespace()) || right == '\n'
    })
}

/// Like `next_word_end`, but once past the starting line's newline it stops before the next word instead of
/// after it, so deleting forward at a line end only removes the newline and indentation.
pub fn next_word_end_or_newline(buffer: &EditorBuffer, from: usize) -> usize {
    let mut on_starting_row = true;
    find_boundary(buffer, from, |left, right| {
        if left == '\n' {
            on_starting_row = false;
        }
        (kind_changes(left, right)
            && ((on_starting_row && !left.is_whitespace())
                || (!on_starting_row && !right.is_whitespace())))
            || right == '\n'
    })
}

fn is_subword_start(left: char, right: char) -> bool {
    let is_word_start = kind_changes(left, right) && !right.is_whitespace();
    let is_subword_start = left == '_' && right != '_'
        || left != '_' && right == '_'
        || left.is_lowercase() && right.is_uppercase();
    is_word_start || is_subword_start
}

fn is_subword_boundary_end(left: char, right: char) -> bool {
    left != '_' && right == '_'
        || left == '_' && right != '_'
        || left.is_lowercase() && right.is_uppercase()
}

fn is_subword_end(left: char, right: char) -> bool {
    let is_word_end = kind_changes(left, right) && !left.is_whitespace();
    is_word_end || is_subword_boundary_end(left, right)
}

pub fn previous_subword_start(buffer: &EditorBuffer, from: usize) -> usize {
    find_preceding_boundary(buffer, from, |left, right| {
        is_subword_start(left, right) || left == '\n' || right == '\n'
    })
}

pub fn next_subword_end(buffer: &EditorBuffer, from: usize) -> usize {
    find_boundary(buffer, from, |left, right| {
        is_subword_end(left, right) || left == '\n' || right == '\n'
    })
}

pub fn next_subword_end_or_newline(buffer: &EditorBuffer, from: usize) -> usize {
    let mut on_starting_row = true;
    find_boundary(buffer, from, |left, right| {
        if left == '\n' {
            on_starting_row = false;
        }
        ((kind_changes(left, right) || is_subword_boundary_end(left, right))
            && ((on_starting_row && !left.is_whitespace())
                || (!on_starting_row && !right.is_whitespace())))
            || right == '\n'
    })
}

/// Word motions jump too far for deleting. Shrink the range `from..until` (either direction) so the deletion
/// stops after a bracket or after a run of 2+ whitespace chars nearest `from`.
pub fn adjust_greedy_deletion(buffer: &EditorBuffer, from: usize, until: usize) -> usize {
    if from == until {
        return until;
    }
    let backward = until < from;
    let range = if backward { until..from } else { from..until };
    let text: Vec<char> = buffer.rope.slice(range.clone()).chars().collect();
    let bracket = |c: char| matches!(c, '(' | ')' | '[' | ']' | '{' | '}');
    let interior = |i: usize| i > 0 && i < text.len();
    let bracket_edges = text
        .iter()
        .enumerate()
        .filter(|(_, c)| bracket(**c))
        .flat_map(|(i, _)| [i, i + 1]);
    let trimmed: Range<usize> = {
        let edges = bracket_edges.filter(|i| interior(*i));
        let nearest = if backward { edges.max() } else { edges.min() };
        match (nearest, backward) {
            (Some(edge), true) => edge..text.len(),
            (Some(edge), false) => 0..edge,
            (None, _) => 0..text.len(),
        }
    };
    let mut runs = Vec::new();
    let mut run_start = None;
    for i in trimmed.clone() {
        if text[i].is_whitespace() {
            run_start.get_or_insert(i);
        } else if let Some(start) = run_start.take() {
            if i - start >= 2 {
                runs.push(start..i);
            }
        }
    }
    if let Some(start) = run_start {
        if trimmed.end - start >= 2 {
            runs.push(start..trimmed.end);
        }
    }
    let stop = if backward {
        runs.last().map_or(trimmed.start, |run| run.start)
    } else {
        runs.first().map_or(trimmed.end, |run| run.end)
    };
    range.start + stop
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> EditorBuffer {
        EditorBuffer::from_text(text)
    }

    #[test]
    fn word_motions_stop_at_kind_changes() {
        let b = buffer("let foo.bar = 1;");
        assert_eq!(next_word_end(&b, 0), 3);
        assert_eq!(next_word_end(&b, 3), 7);
        assert_eq!(next_word_end(&b, 7), 8);
        assert_eq!(previous_word_start(&b, 11), 8);
        assert_eq!(previous_word_start(&b, 8), 7);
        assert_eq!(previous_word_start(&b, 7), 4);
    }

    #[test]
    fn word_motions_cross_lines_but_stop_at_line_edges() {
        let b = buffer("ab\n  cd");
        assert_eq!(previous_word_start(&b, 5), 3);
        assert_eq!(previous_word_start(&b, 3), 0);
        assert_eq!(next_word_end(&b, 2), 7);
        assert_eq!(previous_word_start_or_newline(&b, 5), 3);
        assert_eq!(previous_word_start_or_newline(&b, 3), 2);
        assert_eq!(next_word_end_or_newline(&b, 2), 5);
    }

    #[test]
    fn subword_motions_split_case_and_underscores() {
        let b = buffer("fooBar_baz");
        assert_eq!(next_subword_end(&b, 0), 3);
        assert_eq!(next_subword_end(&b, 3), 6);
        assert_eq!(next_subword_end(&b, 6), 7);
        assert_eq!(previous_subword_start(&b, 10), 7);
        assert_eq!(previous_subword_start(&b, 7), 6);
        assert_eq!(previous_subword_start(&b, 6), 3);
    }

    #[test]
    fn greedy_deletion_stops_after_brackets_and_wide_whitespace() {
        let b = buffer("call(arg)   next");
        assert_eq!(adjust_greedy_deletion(&b, 9, previous_word_start(&b, 9)), 8);
        assert_eq!(adjust_greedy_deletion(&b, 16, 0), 9);
        assert_eq!(adjust_greedy_deletion(&buffer("a   next"), 8, 0), 1);
        assert_eq!(adjust_greedy_deletion(&b, 9, 16), 12);
        assert_eq!(adjust_greedy_deletion(&b, 5, 8), 8);
    }
}
