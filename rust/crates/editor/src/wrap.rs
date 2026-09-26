//! Soft wrap: where a display line breaks to fit a width. Breaks prefer the start of a word that follows a space
//! (or any non-word char once the line has content); a word longer than the width breaks mid-word. Every
//! continuation row is indented like the line's own leading whitespace, capped at `MAX_INDENT` columns.

pub const MAX_INDENT: usize = 256;

/// A continuation row starts at byte `index` of the line and is drawn after `indent` columns of blank space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boundary {
    pub index: usize,
    pub indent: usize,
}

/// Break `text` (one display line, no newline) into rows no wider than `wrap_width`.
pub fn wrap_line(
    text: &str,
    wrap_width: f32,
    mut width_of: impl FnMut(char) -> f32,
) -> Vec<Boundary> {
    let mut boundaries = Vec::new();
    let mut width = 0.0f32;
    let mut first_non_whitespace = None;
    let mut indent = None;
    let mut candidate_index = 0usize;
    let mut candidate_width = 0.0f32;
    let mut last_wrap = 0usize;
    let mut prev = '\0';
    let space_width = width_of(' ');
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            continue;
        }
        let has_content = first_non_whitespace.is_some();
        let breaks_before = if is_word_char(ch) {
            prev == ' ' && ch != ' ' && has_content
        } else {
            ch != ' ' && has_content
        };
        if breaks_before {
            candidate_index = index;
            candidate_width = width;
        }
        if ch != ' ' && first_non_whitespace.is_none() {
            first_non_whitespace = Some(index);
        }
        let ch_width = width_of(ch);
        width += ch_width;
        if width > wrap_width && index > last_wrap {
            if let (None, Some(first)) = (indent, first_non_whitespace) {
                indent = Some(MAX_INDENT.min(first - last_wrap));
            }
            if candidate_index > 0 {
                last_wrap = candidate_index;
                width -= candidate_width;
                candidate_index = 0;
            } else {
                last_wrap = index;
                width = ch_width;
            }
            let indent = indent.unwrap_or(0);
            width += space_width * indent as f32;
            boundaries.push(Boundary {
                index: last_wrap,
                indent,
            });
            // The char that forced the break doesn't become `prev`, matching how the next candidate is judged.
            continue;
        }
        prev = ch;
    }
    boundaries
}

/// Chars that stay glued to their neighbours: letters (Latin, Cyrillic, Vietnamese, Bengali), digits, and
/// punctuation that reads as part of a word or must not start a row (`a-b`, `x.y`, `100%`, `see)`, `a,`). `/`
/// and `?` are deliberately absent so paths and URLs can break.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '\u{00C0}'..='\u{024F}')
        || matches!(c, '\u{0400}'..='\u{04FF}')
        || matches!(c, '\u{1E00}'..='\u{1EFF}')
        || matches!(c, '\u{0300}'..='\u{036F}')
        || matches!(c, '\u{0980}'..='\u{09FF}')
        || matches!(
            c,
            '-' | '_'
                | '.'
                | '\''
                | '\u{2019}'
                | '\u{2018}'
                | '$'
                | '%'
                | '@'
                | '#'
                | '^'
                | '~'
                | ','
                | '='
                | ':'
                | ';'
        )
        || matches!(
            c,
            '!' | ')' | ']' | '}' | '"' | '\u{201D}' | '\u{00BB}' | '\u{2026}'
        )
        || matches!(c, '\u{22EF}')
        || matches!(c, '\u{202F}' | '\u{00A0}' | '\u{2011}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, columns: usize) -> Vec<&str> {
        let mut rows = Vec::new();
        let mut start = 0;
        for boundary in wrap_line(text, columns as f32, |_| 1.0) {
            rows.push(&text[start..boundary.index]);
            start = boundary.index;
        }
        rows.push(&text[start..]);
        rows
    }

    #[test]
    fn breaks_before_words_after_spaces() {
        assert_eq!(rows("aaa bbb ccc", 8), vec!["aaa bbb ", "ccc"]);
    }

    #[test]
    fn long_words_break_mid_word() {
        assert_eq!(rows("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn continuation_rows_keep_the_line_indent() {
        let boundaries = wrap_line("    let value = compute(a, b);", 16.0, |_| 1.0);
        assert!(!boundaries.is_empty());
        assert!(boundaries.iter().all(|b| b.indent == 4));
    }

    #[test]
    fn paths_break_at_slashes() {
        assert_eq!(
            rows("/usr/local/share", 8),
            vec!["/usr", "/local", "/share"]
        );
    }

    #[test]
    fn short_lines_do_not_wrap() {
        assert!(wrap_line("short", 80.0, |_| 1.0).is_empty());
    }
}
