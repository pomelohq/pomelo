//! Splitting SQL into statements on `;` that are code: not inside quotes (`'..'`, `".."`), dollar-quoted
//! bodies (`$$..$$`, `$tag$..$tag$`) or comments (`-- ..`, `/* .. */`).

use std::ops::Range;

/// Byte ranges of the statements, trimmed, empty ones dropped.
pub fn statement_ranges(sql: &str) -> Vec<Range<usize>> {
    let bytes = sql.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\'' | b'"' => at = skip_quoted(bytes, at, bytes[at]),
            b'-' if bytes.get(at + 1) == Some(&b'-') => {
                at = bytes[at..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(bytes.len(), |offset| at + offset + 1);
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at = sql[at + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |offset| at + 2 + offset + 2);
            }
            b'$' => at = skip_dollar_quoted(sql, at),
            b';' => {
                push_trimmed(sql, start..at, &mut ranges);
                at += 1;
                start = at;
            }
            _ => at += 1,
        }
    }
    push_trimmed(sql, start..bytes.len(), &mut ranges);
    ranges
}

fn push_trimmed(sql: &str, range: Range<usize>, ranges: &mut Vec<Range<usize>>) {
    let piece = &sql[range.clone()];
    let leading = piece.len() - piece.trim_start().len();
    let trailing = piece.len() - piece.trim_end().len();
    if leading + trailing < piece.len() {
        ranges.push(range.start + leading..range.end - trailing);
    }
}

/// Past a quoted run starting at `at`; a doubled quote is an escaped one.
fn skip_quoted(bytes: &[u8], at: usize, quote: u8) -> usize {
    let mut index = at + 1;
    while index < bytes.len() {
        if bytes[index] == quote {
            if bytes.get(index + 1) == Some(&quote) {
                index += 2;
                continue;
            }
            return index + 1;
        }
        index += 1;
    }
    bytes.len()
}

/// Past a `$tag$ .. $tag$` body starting at `at`, or one byte on for a `$` that opens none (`$1`).
fn skip_dollar_quoted(sql: &str, at: usize) -> usize {
    let rest = &sql[at + 1..];
    let tag_len = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let tag = &rest[..tag_len];
    if !rest[tag_len..].starts_with('$') || tag.starts_with(|c: char| c.is_ascii_digit()) {
        return at + 1;
    }
    let opener = format!("${tag}$");
    let body = at + opener.len();
    sql[body..]
        .find(&opener)
        .map_or(sql.len(), |offset| body + offset + opener.len())
}

/// The statement the caret (a byte offset) is in, or the nearest one before it; the whole text when there
/// is no statement at all.
pub fn statement_at(sql: &str, caret: usize) -> String {
    let ranges = statement_ranges(sql);
    let chosen = ranges
        .iter()
        .find(|range| caret >= range.start && caret <= range.end + 1)
        .or_else(|| ranges.iter().rev().find(|range| range.end <= caret))
        .or_else(|| ranges.first());
    chosen.map_or_else(
        || sql.trim().to_string(),
        |range| sql[range.clone()].to_string(),
    )
}

/// The first keyword of a statement, lowercased, past leading comments.
pub fn first_keyword(sql: &str) -> String {
    let mut rest = sql.trim_start();
    loop {
        if let Some(line) = rest.strip_prefix("--") {
            rest = line
                .find('\n')
                .map_or("", |at| &line[at + 1..])
                .trim_start();
        } else if let Some(block) = rest.strip_prefix("/*") {
            rest = block
                .find("*/")
                .map_or("", |at| &block[at + 2..])
                .trim_start();
        } else {
            break;
        }
    }
    rest.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statements(sql: &str) -> Vec<&str> {
        statement_ranges(sql)
            .into_iter()
            .map(|range| &sql[range])
            .collect()
    }

    #[test]
    fn semicolons_in_strings_comments_and_bodies_do_not_split() {
        assert_eq!(
            statements("select ';' as a; select \"x;y\" from t;\n-- a; comment\nselect 1 /* ; */;"),
            [
                "select ';' as a",
                "select \"x;y\" from t",
                "-- a; comment\nselect 1 /* ; */"
            ]
        );
        assert_eq!(
            statements("create function f() returns int as $$ begin; return 1; end $$ language plpgsql; select $1;"),
            [
                "create function f() returns int as $$ begin; return 1; end $$ language plpgsql",
                "select $1"
            ]
        );
        assert_eq!(
            statements("select 'it''s; fine'; ;  "),
            ["select 'it''s; fine'"]
        );
        assert!(statements("  ;; ").is_empty());
    }

    #[test]
    fn the_caret_picks_its_statement() {
        let sql = "select 1;\nselect 2;\n\nselect 3";
        assert_eq!(statement_at(sql, 3), "select 1");
        assert_eq!(
            statement_at(sql, 9),
            "select 1",
            "right after the semicolon"
        );
        assert_eq!(statement_at(sql, 12), "select 2");
        assert_eq!(
            statement_at(sql, 20),
            "select 2",
            "on the blank line: the one before"
        );
        assert_eq!(statement_at(sql, sql.len()), "select 3");
        assert_eq!(statement_at("  ", 1), "");
    }

    #[test]
    fn first_keywords_skip_comments() {
        assert_eq!(
            first_keyword("  -- note\n/* x */ WITH a AS (select 1)"),
            "with"
        );
        assert_eq!(first_keyword("Select 1"), "select");
        assert_eq!(first_keyword(""), "");
    }
}
