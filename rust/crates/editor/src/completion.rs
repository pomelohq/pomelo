//! Completing the word being typed from words already in the file: the query is the word's part before the
//! caret, the candidates every distinct word within a few thousand lines of it.

use std::collections::BTreeSet;
use std::ops::Range;

use crate::buffer::EditorBuffer;

/// Lines searched on each side of the caret, so huge files still complete promptly.
const WORD_LOOKUP_ROWS: usize = 5_000;

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word before `offset` (chars) up to it, with where it starts; `None` unless `offset` ends a word part.
pub fn completion_query(buffer: &EditorBuffer, offset: usize) -> Option<(usize, String)> {
    let offset = offset.min(buffer.rope.len_chars());
    let mut start = offset;
    while start > 0 && is_word_char(buffer.rope.char(start - 1)) {
        start -= 1;
    }
    (start < offset).then(|| (start, buffer.rope.slice(start..offset).to_string()))
}

/// Distinct words near line `row`, without `exclude` (the word being typed); words starting with a digit are
/// left out unless the query itself has digits.
pub fn buffer_words(
    buffer: &EditorBuffer,
    row: usize,
    exclude: Option<&str>,
    skip_digits: bool,
) -> Vec<String> {
    let rope = &buffer.rope;
    let first = row.saturating_sub(WORD_LOOKUP_ROWS);
    let last = (row + WORD_LOOKUP_ROWS).min(rope.len_lines());
    let range: Range<usize> = rope.line_to_char(first)..if last >= rope.len_lines() {
        rope.len_chars()
    } else {
        rope.line_to_char(last)
    };
    let mut words = BTreeSet::new();
    let mut current = String::new();
    for c in rope.slice(range).chars().chain(std::iter::once(' ')) {
        if is_word_char(c) {
            current.push(c);
            continue;
        }
        if !current.is_empty() {
            let digit_start = current.chars().next().is_some_and(|c| c.is_ascii_digit());
            if !(skip_digits && digit_start) && Some(current.as_str()) != exclude {
                words.insert(std::mem::take(&mut current));
            }
            current.clear();
        }
    }
    words.into_iter().collect()
}

/// The parts of `word` a query's first letter may start: a part begins at a lower-to-upper case change or
/// where a letter follows a non-alphanumeric char.
pub fn split_words(word: &str) -> impl Iterator<Item = &str> {
    let mut previous: Option<char> = None;
    let mut part_start = 0;
    word.char_indices()
        .chain(std::iter::once((word.len(), '\0')))
        .filter_map(move |(index, c)| {
            let prev = previous.replace(c)?;
            let boundary = index == word.len()
                || (!prev.is_uppercase() && c.is_uppercase())
                || (!prev.is_alphanumeric() && c.is_alphanumeric());
            if !boundary {
                return None;
            }
            let part = &word[part_start..index];
            part_start = index;
            Some(part)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_is_the_word_part_before_the_caret() {
        let buffer = EditorBuffer::from_text("let foo_ba = 1;");
        assert_eq!(
            completion_query(&buffer, 10),
            Some((4, "foo_ba".to_string()))
        );
        assert_eq!(completion_query(&buffer, 7), Some((4, "foo".to_string())));
        assert_eq!(completion_query(&buffer, 11), None);
    }

    #[test]
    fn collects_distinct_words_skipping_the_typed_one_and_numbers() {
        let buffer = EditorBuffer::from_text("alpha beta alpha 42x gamma_1\nbet");
        assert_eq!(
            buffer_words(&buffer, 1, Some("bet"), true),
            vec!["alpha", "beta", "gamma_1"]
        );
        assert!(buffer_words(&buffer, 1, None, false).contains(&"42x".to_string()));
    }

    #[test]
    fn splits_on_case_and_separators() {
        let parts: Vec<&str> = split_words("fooBar_baz").collect();
        assert_eq!(parts, vec!["foo", "Bar_", "baz"]);
    }
}
