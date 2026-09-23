//! Buffer search queries: plain text (ASCII case folding, optional whole-word) or regex (case flag, `\c`/`\C`
//! inline overrides, whole-word via `\b`), with replacement that expands capture groups for regex queries.

use fancy_regex::{Regex, RegexBuilder};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

pub enum SearchQuery {
    Text {
        query: String,
        case_sensitive: bool,
        whole_word: bool,
        replacement: Option<String>,
    },
    Regex {
        regex: Regex,
        /// Built from escaped literal text, so replacements are literal too.
        escaped: bool,
        replacement: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Next,
    Prev,
}

impl SearchQuery {
    pub fn new(
        query: &str,
        options: SearchOptions,
        replacement: Option<String>,
    ) -> Result<Self, String> {
        let query = crate::buffer::normalize_newlines(query).into_owned();
        if options.regex {
            return build_regex(query, options, false, replacement);
        }
        // ASCII case folding can't handle other scripts, so those go through an escaped regex.
        if !options.case_sensitive && !query.is_ascii() {
            return build_regex(
                fancy_regex::escape(&query).into_owned(),
                options,
                true,
                replacement,
            );
        }
        Ok(Self::Text {
            query,
            case_sensitive: options.case_sensitive,
            whole_word: options.whole_word,
            replacement,
        })
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Text { query, .. } => query.is_empty(),
            Self::Regex { regex, .. } => regex.as_str().is_empty(),
        }
    }

    /// Byte ranges of the matches in `text`, in order.
    pub fn find_in(&self, text: &str) -> Vec<Range<usize>> {
        if self.is_empty() {
            return Vec::new();
        }
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
        match self {
            Self::Text {
                query,
                case_sensitive,
                whole_word,
                ..
            } => {
                let hits: Vec<Range<usize>> = if *case_sensitive {
                    text.match_indices(query.as_str())
                        .map(|(at, m)| at..at + m.len())
                        .collect()
                } else {
                    let haystack = text.to_ascii_lowercase();
                    let needle = query.to_ascii_lowercase();
                    haystack
                        .match_indices(needle.as_str())
                        .map(|(at, m)| at..at + m.len())
                        .collect()
                };
                hits.into_iter()
                    .filter(|hit| {
                        !*whole_word
                            || !text[..hit.start].chars().last().is_some_and(is_word_char)
                                && !text[hit.end..].chars().next().is_some_and(is_word_char)
                    })
                    .collect()
            }
            Self::Regex { regex, .. } => regex
                .find_iter(text)
                .flatten()
                .map(|m| m.start()..m.end())
                .collect(),
        }
    }

    /// The text replacing `hit` (byte range within `line`, the lines it spans). Regex replacements expand
    /// `$1`-style groups and `\n`, `\t`, `\\` escapes.
    pub fn replacement_for(&self, line: &str, hit: Range<usize>) -> Option<String> {
        match self {
            Self::Text { replacement, .. }
            | Self::Regex {
                replacement,
                escaped: true,
                ..
            } => replacement.clone(),
            Self::Regex {
                regex,
                replacement: Some(replacement),
                escaped: false,
            } => {
                let replacement = unescape_replacement(replacement);
                let captures = regex
                    .captures_from_pos(line, hit.start)
                    .ok()
                    .flatten()
                    .filter(|c| c.get(0).is_some_and(|m| m.range() == hit));
                match captures {
                    Some(captures) => {
                        let mut out = String::new();
                        captures.expand(&replacement, &mut out);
                        Some(out)
                    }
                    None => Some(
                        regex
                            .replace(line.get(hit)?, replacement.as_str())
                            .into_owned(),
                    ),
                }
            }
            Self::Regex {
                replacement: None, ..
            } => None,
        }
    }
}

fn unescape_replacement(replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len());
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('\\') => {
                    out.push('\\');
                    chars.next();
                }
                Some('n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('t') => {
                    out.push('\t');
                    chars.next();
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `\c` in a pattern forces case-insensitive and `\C` case-sensitive matching; the flags are removed.
fn case_sensitive_from_pattern(pattern: &str) -> Option<(bool, String)> {
    if !(pattern.contains("\\c") || pattern.contains("\\C")) {
        return None;
    }
    let mut escaped = false;
    let mut out = String::new();
    let mut case_sensitive = None;
    for c in pattern.chars() {
        if escaped {
            match c {
                'c' => case_sensitive = Some(false),
                'C' => case_sensitive = Some(true),
                _ => {
                    out.push('\\');
                    out.push(c);
                }
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            out.push(c);
        }
    }
    case_sensitive.map(|flag| (flag, out))
}

fn build_regex(
    mut pattern: String,
    options: SearchOptions,
    escaped: bool,
    replacement: Option<String>,
) -> Result<SearchQuery, String> {
    let mut case_sensitive = options.case_sensitive;
    if let Some((flag, stripped)) = case_sensitive_from_pattern(&pattern) {
        case_sensitive = flag;
        pattern = stripped;
    }
    if options.whole_word {
        // Only anchor at edges that are word chars; `\b` next to punctuation would never match.
        let is_word = |s: &str| {
            s.chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        };
        let mut word = String::new();
        if pattern.get(0..1).is_some_and(is_word) {
            word.push_str("\\b");
        }
        word.push_str(&pattern);
        if pattern
            .get(pattern.len().saturating_sub(1)..)
            .is_some_and(is_word)
        {
            word.push_str("\\b");
        }
        pattern = word;
    }
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .multi_line(true)
        .crlf(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(SearchQuery::Regex {
        regex,
        escaped,
        replacement,
    })
}

/// The match at or around `cursor` (char offsets): one containing it, else the nearest in `direction`.
pub fn active_match_index(
    direction: Direction,
    matches: &[Range<usize>],
    cursor: usize,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let found = matches.binary_search_by(|m| {
        if m.end < cursor {
            std::cmp::Ordering::Less
        } else if m.start > cursor {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    Some(match direction {
        Direction::Prev => match found {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        },
        Direction::Next => match found {
            Ok(i) | Err(i) => i.min(matches.len() - 1),
        },
    })
}

/// The match `count` steps from `cursor` in `direction`, wrapping around the ends.
pub fn match_index_for_direction(
    matches: &[Range<usize>],
    current: usize,
    direction: Direction,
    count: usize,
    cursor: Option<usize>,
) -> usize {
    if count == 0 || matches.is_empty() {
        return current;
    }
    let cursor = cursor.unwrap_or(matches[current].start);
    let first = match direction {
        Direction::Next => matches.iter().position(|m| m.start > cursor).unwrap_or(0),
        Direction::Prev => matches
            .iter()
            .rposition(|m| m.end < cursor)
            .unwrap_or(matches.len() - 1),
    } as isize;
    let steps = count.saturating_sub(1) as isize;
    let len = matches.len() as isize;
    let index = match direction {
        Direction::Next => first + steps,
        Direction::Prev => first - steps,
    };
    index.rem_euclid(len) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str, options: SearchOptions) -> SearchQuery {
        SearchQuery::new(text, options, None).unwrap()
    }

    #[test]
    fn text_search_folds_ascii_case_and_respects_whole_words() {
        let q = query("foo", SearchOptions::default());
        assert_eq!(q.find_in("Foo food foo"), vec![0..3, 4..7, 9..12]);
        let q = query(
            "foo",
            SearchOptions {
                whole_word: true,
                ..Default::default()
            },
        );
        assert_eq!(q.find_in("Foo food foo"), vec![0..3, 9..12]);
    }

    #[test]
    fn regex_replacement_expands_groups() {
        let q = SearchQuery::new(
            r"(\w+)=(\d)",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
            Some("$2:$1".into()),
        )
        .unwrap();
        let line = "a=1 b=2";
        let hits = q.find_in(line);
        assert_eq!(hits, vec![0..3, 4..7]);
        assert_eq!(q.replacement_for(line, 4..7).as_deref(), Some("2:b"));
    }

    #[test]
    fn inline_case_flag_overrides_the_option() {
        let q = query(
            r"\CFoo",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
        );
        assert_eq!(q.find_in("foo Foo"), vec![4..7]);
    }

    #[test]
    fn non_ascii_case_insensitive_text_goes_through_regex() {
        let q = query("\u{c9}t\u{e9}", SearchOptions::default());
        assert_eq!(q.find_in("\u{e9}t\u{e9}").len(), 1);
    }

    #[test]
    fn match_navigation_wraps() {
        let matches = vec![0..1, 5..6, 9..10];
        assert_eq!(active_match_index(Direction::Next, &matches, 3), Some(1));
        assert_eq!(
            match_index_for_direction(&matches, 2, Direction::Next, 1, Some(9)),
            0
        );
        assert_eq!(
            match_index_for_direction(&matches, 0, Direction::Prev, 1, Some(0)),
            2
        );
    }
}
