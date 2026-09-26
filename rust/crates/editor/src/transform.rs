//! Text transforms applied to selections (or the word at a caret) and to whole selected lines.

use convert_case::{Case, Casing};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextTransform {
    UpperCase,
    LowerCase,
    TitleCase,
    SnakeCase,
    KebabCase,
    UpperCamelCase,
    LowerCamelCase,
    SentenceCase,
    OppositeCase,
    /// Lower-case when anything is upper-case, else upper-case.
    ToggleCase,
    Rot13,
    Rot47,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineTransform {
    SortCaseSensitive,
    SortCaseInsensitive,
    SortByLength,
    UniqueCaseSensitive,
    UniqueCaseInsensitive,
    Reverse,
}

/// Word-case conversions keep each line's surrounding whitespace.
fn convert_lines(text: &str, case: Case) -> String {
    text.split('\n')
        .map(|line| {
            let trimmed_start = line.trim_start();
            let leading = &line[..line.len() - trimmed_start.len()];
            let trimmed = trimmed_start.trim_end();
            let trailing = &trimmed_start[trimmed.len()..];
            format!("{leading}{}{trailing}", trimmed.to_case(case))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn apply_text(transform: TextTransform, text: &str) -> String {
    match transform {
        TextTransform::UpperCase => text.to_uppercase(),
        TextTransform::LowerCase => text.to_lowercase(),
        TextTransform::TitleCase => convert_lines(text, Case::Title),
        TextTransform::SnakeCase => convert_lines(text, Case::Snake),
        TextTransform::KebabCase => convert_lines(text, Case::Kebab),
        TextTransform::UpperCamelCase => convert_lines(text, Case::UpperCamel),
        TextTransform::LowerCamelCase => convert_lines(text, Case::Camel),
        TextTransform::SentenceCase => convert_lines(text, Case::Sentence),
        TextTransform::OppositeCase => text
            .chars()
            .flat_map(|c| {
                if c.is_uppercase() {
                    c.to_lowercase().collect::<Vec<_>>()
                } else {
                    c.to_uppercase().collect::<Vec<_>>()
                }
            })
            .collect(),
        TextTransform::ToggleCase => {
            if text.chars().any(char::is_uppercase) {
                text.to_lowercase()
            } else {
                text.to_uppercase()
            }
        }
        TextTransform::Rot13 => text
            .chars()
            .map(|c| match c {
                'A'..='M' | 'a'..='m' => char::from(c as u8 + 13),
                'N'..='Z' | 'n'..='z' => char::from(c as u8 - 13),
                _ => c,
            })
            .collect(),
        TextTransform::Rot47 => text
            .chars()
            .map(|c| match c as u32 {
                33..=126 => char::from_u32(33 + (c as u32 + 14) % 94).unwrap_or(c),
                _ => c,
            })
            .collect(),
    }
}

pub fn apply_lines(transform: LineTransform, lines: &mut Vec<&str>) {
    match transform {
        LineTransform::SortCaseSensitive => lines.sort(),
        LineTransform::SortCaseInsensitive => lines.sort_by_key(|line| line.to_lowercase()),
        LineTransform::SortByLength => lines.sort_by_key(|line| line.chars().count()),
        LineTransform::UniqueCaseSensitive => {
            let mut seen = std::collections::HashSet::new();
            lines.retain(|line| seen.insert(*line));
        }
        LineTransform::UniqueCaseInsensitive => {
            let mut seen = std::collections::HashSet::new();
            lines.retain(|line| seen.insert(line.to_lowercase()));
        }
        LineTransform::Reverse => lines.reverse(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_conversions_keep_line_padding() {
        assert_eq!(
            apply_text(TextTransform::SnakeCase, "  fooBar "),
            "  foo_bar "
        );
        assert_eq!(
            apply_text(TextTransform::UpperCamelCase, "foo_bar"),
            "FooBar"
        );
        assert_eq!(apply_text(TextTransform::ToggleCase, "abc"), "ABC");
        assert_eq!(apply_text(TextTransform::OppositeCase, "aB"), "Ab");
        assert_eq!(apply_text(TextTransform::Rot13, "Hello"), "Uryyb");
    }

    #[test]
    fn line_transforms() {
        let mut lines = vec!["b", "A", "a", "b"];
        apply_lines(LineTransform::UniqueCaseSensitive, &mut lines);
        assert_eq!(lines, vec!["b", "A", "a"]);
        apply_lines(LineTransform::SortCaseSensitive, &mut lines);
        assert_eq!(lines, vec!["A", "a", "b"]);
    }
}
