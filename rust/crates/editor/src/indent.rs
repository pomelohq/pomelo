//! Syntax-aware auto-indentation.
//!
//! An indent query marks ranges whose inner lines sit one level deeper (`@indent`, trimmed by `@start` /
//! `@end`, cut short by `@outdent`), and per-language regexes nudge single lines in or out. Each line gets a
//! suggestion relative to a basis row. After an edit, lines are re-indented only where the suggestion computed
//! on the new text differs from the one on the text before the edit, so typing on a line the user indented by
//! hand leaves it alone until the syntax around it actually changes.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use regex::Regex;
use ropey::Rope;
use tree_sitter::{Language, Point, Query, QueryCursor, StreamingIterator, Tree};

use crate::highlight::Lang;
use crate::language::IndentRules;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentKind {
    Space,
    Tab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentSize {
    pub len: usize,
    pub kind: IndentKind,
}

impl Default for IndentSize {
    fn default() -> Self {
        Self::spaces(0)
    }
}

impl IndentSize {
    pub fn spaces(len: usize) -> Self {
        Self {
            len,
            kind: IndentKind::Space,
        }
    }

    pub fn char(self) -> char {
        match self.kind {
            IndentKind::Space => ' ',
            IndentKind::Tab => '\t',
        }
    }

    pub fn text(self) -> String {
        std::iter::repeat_n(self.char(), self.len).collect()
    }

    /// One `unit` deeper or shallower; mixing tabs and spaces leaves the size unchanged.
    pub fn with_delta(mut self, direction: Ordering, unit: IndentSize) -> Self {
        match direction {
            Ordering::Less => {
                if self.kind == unit.kind && self.len >= unit.len {
                    self.len -= unit.len;
                }
            }
            Ordering::Equal => {}
            Ordering::Greater => {
                if self.len == 0 {
                    self = unit;
                } else if self.kind == unit.kind {
                    self.len += unit.len;
                }
            }
        }
        self
    }
}

/// Leading whitespace; its kind is whatever the first char is.
pub fn indent_size_for_text(text: impl Iterator<Item = char>) -> IndentSize {
    let mut size = IndentSize::default();
    for c in text {
        let kind = match c {
            ' ' => IndentKind::Space,
            '\t' => IndentKind::Tab,
            _ => break,
        };
        if size.len == 0 {
            size.kind = kind;
        }
        size.len += 1;
    }
    size
}

pub fn indent_size_for_line(rope: &Rope, row: usize) -> IndentSize {
    if row >= rope.len_lines() {
        return IndentSize::default();
    }
    indent_size_for_text(rope.line(row).chars())
}

/// The line's text without its line break.
fn line_text(rope: &Rope, row: usize) -> String {
    let mut text = rope.line(row).to_string();
    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
    text
}

fn is_line_blank(rope: &Rope, row: usize) -> bool {
    row >= rope.len_lines() || rope.line(row).chars().all(char::is_whitespace)
}

/// Bracket pairs every grammar shares; patterns naming tokens a grammar lacks are dropped when compiling.
const GENERIC_PATTERNS: [&str; 3] = [
    "(_ \"{\" \"}\" @end) @indent",
    "(_ \"[\" \"]\" @end) @indent",
    "(_ \"(\" \")\" @end) @indent",
];

fn language_patterns(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => include_str!("../queries/rust/indents.scm"),
        Lang::JavaScript => include_str!("../queries/javascript/indents.scm"),
        Lang::TypeScript => include_str!("../queries/typescript/indents.scm"),
        Lang::Tsx => include_str!("../queries/tsx/indents.scm"),
        Lang::Go => include_str!("../queries/go/indents.scm"),
        Lang::Python => include_str!("../queries/python/indents.scm"),
        Lang::C | Lang::Cpp => include_str!("../queries/c/indents.scm"),
        Lang::Java | Lang::CSharp => include_str!("../queries/java/indents.scm"),
        Lang::Html => include_str!("../queries/html/indents.scm"),
        Lang::Xml => include_str!("../queries/xml/indents.scm"),
        _ => "",
    }
}

pub struct IndentQuery {
    query: Query,
    indent_ix: Option<u32>,
    start_ix: Option<u32>,
    end_ix: Option<u32>,
    outdent_ix: Option<u32>,
    /// `@start.<name>` captures: where a block of that name begins, for the regex outdent rules.
    suffixed_starts: HashMap<u32, String>,
    errors: Option<Query>,
}

impl IndentQuery {
    pub fn new(lang: Lang, language: &Language) -> Option<Self> {
        let mut source: String = GENERIC_PATTERNS
            .iter()
            .filter(|pattern| Query::new(language, pattern).is_ok())
            .map(|pattern| format!("{pattern}\n"))
            .collect();
        let extra = language_patterns(lang);
        if Query::new(language, extra).is_ok() {
            source.push_str(extra);
        }
        let query = Query::new(language, &source).ok()?;
        let mut indent = Self {
            indent_ix: None,
            start_ix: None,
            end_ix: None,
            outdent_ix: None,
            suffixed_starts: HashMap::new(),
            errors: Query::new(language, "(ERROR) @error").ok(),
            query,
        };
        for (index, name) in indent.query.capture_names().iter().enumerate() {
            let index = index as u32;
            match *name {
                "indent" => indent.indent_ix = Some(index),
                "start" => indent.start_ix = Some(index),
                "end" => indent.end_ix = Some(index),
                "outdent" => indent.outdent_ix = Some(index),
                name => {
                    if let Some(suffix) = name.strip_prefix("start.") {
                        indent.suffixed_starts.insert(index, suffix.to_string());
                    }
                }
            }
        }
        Some(indent)
    }
}

/// Compiled per-language line rules.
pub struct IndentRegexes {
    increase: Option<Regex>,
    decrease: Option<Regex>,
    decrease_after: Vec<(Regex, &'static [&'static str])>,
    pub using_last_non_empty_line: bool,
}

impl IndentRegexes {
    pub fn new(rules: &IndentRules) -> Self {
        let compile = |pattern: Option<&str>| pattern.and_then(|p| Regex::new(p).ok());
        Self {
            increase: compile(rules.increase),
            decrease: compile(rules.decrease),
            decrease_after: rules
                .decrease_after
                .iter()
                .filter_map(|(pattern, valid_after)| {
                    Some((Regex::new(pattern).ok()?, *valid_after))
                })
                .collect(),
            using_last_non_empty_line: rules.using_last_non_empty_line,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentSuggestion {
    pub basis_row: usize,
    pub delta: Ordering,
    pub within_error: bool,
}

/// A text and its syntax tree, for suggesting indents.
pub struct IndentView<'a> {
    pub rope: &'a Rope,
    pub tree: &'a Tree,
    pub query: &'a IndentQuery,
    pub regexes: &'a IndentRegexes,
}

fn row_start_byte(rope: &Rope, row: usize) -> usize {
    if row >= rope.len_lines() {
        rope.len_bytes()
    } else {
        rope.line_to_byte(row)
    }
}

impl IndentView<'_> {
    fn prev_non_blank_row(&self, mut row: usize) -> Option<usize> {
        while row > 0 {
            row -= 1;
            if !is_line_blank(self.rope, row) {
                return Some(row);
            }
        }
        None
    }

    fn indent_len(&self, row: usize) -> usize {
        indent_size_for_line(self.rope, row).len
    }

    /// One suggestion per row of `rows`; `None` leaves that row as it is.
    pub fn suggest(&self, rows: Range<usize>) -> Vec<Option<IndentSuggestion>> {
        let prev_non_blank = self.prev_non_blank_row(rows.start);
        let first_row = prev_non_blank.unwrap_or(rows.start);
        let bytes = row_start_byte(self.rope, first_row)..row_start_byte(self.rope, rows.end);
        let text = |node: tree_sitter::Node| {
            let range = node.byte_range();
            let end = range.end.min(self.rope.len_bytes());
            let start = range.start.min(end);
            self.rope.byte_slice(start..end).chunks().map(str::as_bytes)
        };

        let mut indent_ranges: Vec<Range<Point>> = Vec::new();
        let mut start_positions: Vec<(Point, &str)> = Vec::new();
        let mut outdent_positions: Vec<Point> = Vec::new();
        let query = self.query;
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(bytes.clone());
        let mut matches = cursor.matches(&query.query, self.tree.root_node(), text);
        while let Some(found) = matches.next() {
            let mut start: Option<Point> = None;
            let mut end: Option<Point> = None;
            for capture in found.captures() {
                let index = Some(capture.index);
                if index == query.indent_ix {
                    start.get_or_insert(capture.node.start_position());
                    end.get_or_insert(capture.node.end_position());
                } else if index == query.start_ix {
                    start = Some(capture.node.end_position());
                } else if index == query.end_ix {
                    end = Some(capture.node.start_position());
                } else if index == query.outdent_ix {
                    outdent_positions.push(capture.node.start_position());
                } else if let Some(suffix) = query.suffixed_starts.get(&capture.index) {
                    start_positions.push((capture.node.start_position(), suffix.as_str()));
                }
            }
            let (Some(start), Some(end)) = (start, end) else {
                continue;
            };
            if start.row == end.row {
                continue;
            }
            match indent_ranges.binary_search_by_key(&start, |range| range.start) {
                Err(index) => indent_ranges.insert(index, start..end),
                Ok(index) => {
                    if let Some(existing) = indent_ranges.get_mut(index) {
                        existing.end = existing.end.max(end);
                    }
                }
            }
        }

        let mut error_ranges: Vec<Range<Point>> = Vec::new();
        if let Some(errors) = query.errors.as_ref() {
            let mut cursor = QueryCursor::new();
            cursor.set_byte_range(bytes);
            let mut matches = cursor.matches(errors, self.tree.root_node(), text);
            while let Some(found) = matches.next() {
                let Some(capture) = found.captures().first() else {
                    continue;
                };
                let range = capture.node.start_position()..capture.node.end_position();
                let (Ok(index) | Err(index)) =
                    error_ranges.binary_search_by_key(&range.start, |r| r.start);
                let mut end_index = index;
                while error_ranges
                    .get(end_index)
                    .is_some_and(|existing| existing.end < range.end)
                {
                    end_index += 1;
                }
                error_ranges.splice(index..end_index, [range]);
            }
        }

        outdent_positions.sort();
        for position in outdent_positions {
            if let Some(innermost) = indent_ranges
                .iter_mut()
                .rfind(|range| range.contains(&position))
            {
                innermost.end = position;
            }
        }
        start_positions.sort_by_key(|(start, _)| *start);

        let mut regex_outdents: HashMap<usize, usize> = HashMap::new();
        let mut seen_starts: HashMap<&str, Vec<Point>> = HashMap::new();
        let mut pending_starts = start_positions.iter().peekable();
        let mut indent_changes: Vec<(usize, Ordering)> = Vec::new();
        let regexes = self.regexes;
        for row in first_row..rows.end.min(self.rope.len_lines()) {
            let line = line_text(self.rope, row);
            if regexes.decrease.as_ref().is_some_and(|r| r.is_match(&line)) {
                indent_changes.push((row, Ordering::Less));
            }
            if regexes.increase.as_ref().is_some_and(|r| r.is_match(&line)) {
                indent_changes.push((row + 1, Ordering::Greater));
            }
            while let Some((start, suffix)) = pending_starts.next_if(|(start, _)| start.row < row) {
                seen_starts.entry(suffix).or_default().push(*start);
            }
            if let Some((_, valid_after)) = regexes
                .decrease_after
                .iter()
                .find(|(pattern, _)| pattern.is_match(&line))
            {
                let row_indent = self.indent_len(row);
                let basis = valid_after
                    .iter()
                    .filter_map(|name| seen_starts.get(name))
                    .flatten()
                    .filter(|start| start.column <= row_indent)
                    .max_by_key(|start| start.row);
                if let Some(basis) = basis {
                    regex_outdents.insert(row, basis.row);
                }
            }
        }

        let mut changes = indent_changes.into_iter().peekable();
        let mut prev_row = if regexes.using_last_non_empty_line {
            prev_non_blank.unwrap_or(0)
        } else {
            rows.start.saturating_sub(1)
        };
        let mut prev_row_start = Point::new(prev_row, self.indent_len(prev_row));
        rows.map(|row| {
            let row_start = Point::new(row, self.indent_len(row));
            let mut indent_from_prev = false;
            let mut outdent_from_prev = false;
            let mut outdent_to_row = usize::MAX;
            let mut from_regex = false;

            while let Some((change_row, delta)) = changes.peek() {
                match change_row.cmp(&row) {
                    Ordering::Equal => match delta {
                        Ordering::Less => {
                            from_regex = true;
                            outdent_from_prev = true;
                        }
                        Ordering::Greater => {
                            from_regex = true;
                            indent_from_prev = true;
                        }
                        Ordering::Equal => {}
                    },
                    Ordering::Greater => break,
                    Ordering::Less => {}
                }
                changes.next();
            }

            for range in &indent_ranges {
                if range.start.row >= row {
                    break;
                }
                if range.start.row == prev_row && range.end > row_start {
                    indent_from_prev = true;
                }
                if range.end > prev_row_start && range.end <= row_start {
                    outdent_to_row = outdent_to_row.min(range.start.row);
                }
            }

            if let Some(basis_row) = regex_outdents.get(&row) {
                indent_from_prev = false;
                outdent_to_row = *basis_row;
                from_regex = true;
            }

            let within_error = error_ranges
                .iter()
                .any(|error| error.start.row < row && error.end > row_start)
                && !from_regex;
            let suggest = |basis_row, delta| {
                Some(IndentSuggestion {
                    basis_row,
                    delta,
                    within_error,
                })
            };
            let suggestion =
                if outdent_to_row == prev_row || (outdent_from_prev && indent_from_prev) {
                    suggest(prev_row, Ordering::Equal)
                } else if indent_from_prev {
                    suggest(prev_row, Ordering::Greater)
                } else if outdent_to_row < prev_row {
                    suggest(outdent_to_row, Ordering::Equal)
                } else if outdent_from_prev {
                    suggest(prev_row, Ordering::Less)
                } else if regexes.using_last_non_empty_line || !is_line_blank(self.rope, prev_row) {
                    suggest(prev_row, Ordering::Equal)
                } else {
                    None
                };
            prev_row = row;
            prev_row_start = row_start;
            suggestion
        })
        .collect()
    }
}

/// A span of an edit whose lines want re-indenting once the syntax tree catches up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoindentEntry {
    /// Char range in the current text.
    pub range: Range<usize>,
    /// The edit's start row before the edit, when its first line already existed.
    pub old_row: Option<usize>,
    /// For block indentation: the column the first inserted line was copied from.
    pub original_indent_column: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct AutoindentRequest {
    pub before: Rope,
    pub before_version: u64,
    pub entries: Vec<AutoindentEntry>,
    /// Indent only each insertion's first line, shifting the rest by the same amount.
    pub block_mode: bool,
    pub ignore_empty_lines: bool,
}

/// Group sorted `values` into runs of consecutive numbers at most `max_len` long.
fn contiguous_ranges(values: impl Iterator<Item = usize>, max_len: usize) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for value in values {
        match ranges.last_mut() {
            Some(range) if range.end == value && range.len() < max_len => range.end += 1,
            _ => ranges.push(value..value + 1),
        }
    }
    ranges
}

/// The new indentation for each row the requests touched, where it should change.
pub fn compute_autoindents(
    requests: &[AutoindentRequest],
    before_tree: &Tree,
    after: &IndentView,
    unit: IndentSize,
) -> BTreeMap<usize, IndentSize> {
    let mut indent_sizes: BTreeMap<usize, (IndentSize, bool)> = BTreeMap::new();
    for request in requests {
        let before = IndentView {
            rope: &request.before,
            tree: before_tree,
            query: after.query,
            regexes: after.regexes,
        };
        let mut row_ranges = Vec::new();
        let mut old_to_new_rows: BTreeMap<usize, usize> = BTreeMap::new();
        for entry in &request.entries {
            let new_row = after.rope.char_to_line(entry.range.start);
            let new_end_row = after.rope.char_to_line(entry.range.end) + 1;
            if let Some(old_row) = entry.old_row {
                old_to_new_rows.insert(old_row, new_row);
            }
            row_ranges.push((new_row..new_end_row, entry.original_indent_column));
        }

        // What the edited lines would have been indented to before the edit, keyed by their new rows.
        let mut old_suggestions: BTreeMap<usize, (IndentSize, bool)> = BTreeMap::new();
        for old_range in contiguous_ranges(old_to_new_rows.keys().copied(), 100) {
            let suggestions = before.suggest(old_range.clone());
            for (old_row, suggestion) in old_range.zip(suggestions) {
                let (Some(suggestion), Some(&new_row)) =
                    (suggestion, old_to_new_rows.get(&old_row))
                else {
                    continue;
                };
                let basis = old_to_new_rows
                    .get(&suggestion.basis_row)
                    .and_then(|from_row| old_suggestions.get(from_row))
                    .map(|(size, _)| *size)
                    .unwrap_or_else(|| indent_size_for_line(before.rope, suggestion.basis_row));
                old_suggestions.insert(
                    new_row,
                    (
                        basis.with_delta(suggestion.delta, unit),
                        suggestion.within_error,
                    ),
                );
            }
        }

        for (row_range, original_indent_column) in row_ranges {
            let suggested_rows = if request.block_mode {
                row_range.start..row_range.start + 1
            } else {
                row_range.clone()
            };
            let suggestions = after.suggest(suggested_rows.clone());
            for (new_row, suggestion) in suggested_rows.zip(suggestions) {
                let Some(suggestion) = suggestion else {
                    continue;
                };
                let suggested = indent_sizes
                    .get(&suggestion.basis_row)
                    .map(|(size, _)| *size)
                    .unwrap_or_else(|| indent_size_for_line(after.rope, suggestion.basis_row))
                    .with_delta(suggestion.delta, unit);
                let changed =
                    old_suggestions
                        .get(&new_row)
                        .is_none_or(|(old, was_within_error)| {
                            suggested != *old && (!suggestion.within_error || *was_within_error)
                        });
                if changed {
                    indent_sizes.insert(new_row, (suggested, request.ignore_empty_lines));
                }
            }

            if let (true, Some(original_column)) = (request.block_mode, original_indent_column) {
                let new_indent = indent_sizes
                    .get(&row_range.start)
                    .map(|(size, _)| *size)
                    .unwrap_or_else(|| indent_size_for_line(after.rope, row_range.start));
                let delta = new_indent.len as i64 - original_column as i64;
                if delta != 0 {
                    for row in row_range.skip(1) {
                        indent_sizes.entry(row).or_insert_with(|| {
                            let mut size = indent_size_for_line(after.rope, row);
                            // An unindented line has no kind of its own yet.
                            if size.len == 0 {
                                size.kind = new_indent.kind;
                            }
                            if size.kind == new_indent.kind {
                                size.len = (size.len as i64 + delta).max(0) as usize;
                            }
                            (size, request.ignore_empty_lines)
                        });
                    }
                }
            }
        }
    }
    indent_sizes
        .into_iter()
        .filter(|(row, (_, ignore_empty))| {
            !(*ignore_empty && line_text(after.rope, *row).is_empty())
        })
        .map(|(row, (size, _))| (row, size))
        .collect()
}

/// The smallest edit (char range, text) turning `current` leading whitespace on the line starting at
/// `line_start` into `target`.
pub fn edit_for_indent_adjustment(
    line_start: usize,
    current: IndentSize,
    target: IndentSize,
) -> Option<(Range<usize>, String)> {
    if target.kind == current.kind {
        match target.len.cmp(&current.len) {
            Ordering::Greater => Some((
                line_start..line_start,
                IndentSize {
                    len: target.len - current.len,
                    kind: target.kind,
                }
                .text(),
            )),
            Ordering::Less => Some((
                line_start..line_start + current.len - target.len,
                String::new(),
            )),
            Ordering::Equal => None,
        }
    } else {
        Some((line_start..line_start + current.len, target.text()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::EditorBuffer;
    use crate::language::{config, LanguageConfig, Scope};
    use crate::syntax::Syntax;
    use std::time::Duration;

    fn settled(lang: Lang, buffer: &EditorBuffer) -> Syntax {
        let mut syntax = Syntax::new(lang).unwrap();
        syntax.sync(buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(buffer);
        }
        syntax
    }

    /// Apply `op` at the `|` in `text`, then let the syntax re-indent; returns the text with `|` at the caret.
    fn edited(
        lang: Lang,
        text: &str,
        op: impl FnOnce(&mut EditorBuffer, &LanguageConfig),
    ) -> String {
        let caret = text.find('|').unwrap();
        let mut buffer = EditorBuffer::from_text(&text.replace('|', ""));
        buffer.place_cursor(text[..caret].chars().count());
        let mut syntax = settled(lang, &buffer);
        op(&mut buffer, &config(lang));
        syntax.autoindent(&mut buffer);
        syntax.sync(&buffer);
        let mut result = buffer.text();
        let byte = result
            .char_indices()
            .nth(buffer.cursor())
            .map_or(result.len(), |(i, _)| i);
        result.insert(byte, '|');
        result
    }

    fn newline(lang: Lang, text: &str) -> String {
        edited(lang, text, |b, language| {
            b.newline(language, &|_| Scope::default())
        })
    }

    fn typed(lang: Lang, text: &str, input: &str) -> String {
        edited(lang, text, |b, language| {
            b.handle_input(input, language, &|_| Scope::default())
        })
    }

    #[test]
    fn language_patterns_compile() {
        for lang in [
            Lang::Rust,
            Lang::JavaScript,
            Lang::TypeScript,
            Lang::Tsx,
            Lang::Go,
            Lang::Python,
            Lang::C,
            Lang::Cpp,
            Lang::Java,
            Lang::Html,
            Lang::Xml,
        ] {
            let (language, _) = crate::highlight::grammar(lang).unwrap();
            if let Err(error) = Query::new(&language, language_patterns(lang)) {
                panic!("{}: {error}", language_patterns(lang));
            }
        }
    }

    #[test]
    fn enter_between_braces_indents_the_inner_line() {
        assert_eq!(newline(Lang::Rust, "fn a() {|}"), "fn a() {\n    |\n}");
        assert_eq!(
            newline(Lang::Rust, "fn a() {\n    if x {|}\n}"),
            "fn a() {\n    if x {\n        |\n    }\n}"
        );
    }

    #[test]
    fn enter_inside_an_open_block_or_call_indents() {
        assert_eq!(newline(Lang::Rust, "fn a() {|\n}"), "fn a() {\n    |\n}");
        assert_eq!(
            newline(Lang::Rust, "fn a() {\n    foo(a,|b);\n}"),
            "fn a() {\n    foo(a,\n        |b);\n}"
        );
    }

    #[test]
    fn typing_a_closer_outdents_its_line() {
        assert_eq!(
            typed(Lang::Rust, "fn a() {\n    let x = 1;\n    |", "}"),
            "fn a() {\n    let x = 1;\n}|"
        );
    }

    #[test]
    fn hand_indentation_survives_unrelated_typing() {
        assert_eq!(
            typed(Lang::Rust, "fn a() {\n        x|\n}", "y"),
            "fn a() {\n        xy|\n}"
        );
    }

    #[test]
    fn python_blocks_follow_colons_and_else_lines_up_with_if() {
        assert_eq!(newline(Lang::Python, "if x:|"), "if x:\n    |");
        assert_eq!(
            typed(Lang::Python, "if x:\n    pass\n    else|", ":"),
            "if x:\n    pass\nelse:|"
        );
    }

    #[test]
    fn regex_rules_indent_languages_without_block_syntax() {
        assert_eq!(newline(Lang::Yaml, "a:|"), "a:\n    |");
        assert_eq!(
            typed(Lang::Ruby, "def a\n    x\n    en|", "d"),
            "def a\n    x\nend|"
        );
    }

    #[test]
    fn autoindent_undoes_with_its_edit() {
        let mut buffer = EditorBuffer::from_text("fn a() {}");
        buffer.set_group_interval(Duration::ZERO);
        buffer.place_cursor(8);
        let mut syntax = settled(Lang::Rust, &buffer);
        buffer.newline(&config(Lang::Rust), &|_| Scope::default());
        syntax.autoindent(&mut buffer);
        assert_eq!(buffer.text(), "fn a() {\n    \n}");
        buffer.undo();
        assert_eq!(buffer.text(), "fn a() {}");
    }
}
