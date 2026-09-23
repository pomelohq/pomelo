//! Snippets: text with numbered stops the caret visits in turn after insertion, written in the common
//! `$1` / `${1:placeholder}` / `${1|one,two|}` syntax, and loaded from per-language JSON files.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use serde::Deserialize;

use crate::highlight::Lang;

#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub text: String,
    /// Visiting order: ascending numbers, `$0` (or an implied stop at the end) last. Ranges are chars.
    pub tabstops: Vec<Tabstop>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tabstop {
    pub ranges: Vec<Range<usize>>,
    pub choices: Option<Vec<String>>,
}

#[derive(Debug, PartialEq)]
pub struct ParseError(String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

struct Parser {
    source: Vec<char>,
    at: usize,
    text: String,
    text_len: usize,
    stops: BTreeMap<usize, Tabstop>,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.source.get(self.at).copied()
    }

    fn push(&mut self, c: char) {
        self.text.push(c);
        self.text_len += 1;
    }

    /// Text up to the end of the source, or up to an unmatched `}` when `nested`.
    fn body(&mut self, nested: bool) -> Result<(), ParseError> {
        while let Some(c) = self.peek() {
            match c {
                '$' => {
                    self.at += 1;
                    self.tabstop()?;
                }
                // Only `$`, `\` and `}` are escapable; any other backslash is literal.
                '\\' => {
                    self.at += 1;
                    match self.peek() {
                        Some(next @ ('$' | '\\' | '}')) => {
                            self.push(next);
                            self.at += 1;
                        }
                        _ => self.push('\\'),
                    }
                }
                '}' if nested => return Ok(()),
                _ => {
                    self.push(c);
                    self.at += 1;
                }
            }
        }
        Ok(())
    }

    fn number(&mut self) -> Result<usize, ParseError> {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.at += 1;
        }
        let digits: String = self
            .source
            .get(start..self.at)
            .unwrap_or_default()
            .iter()
            .collect();
        digits
            .parse()
            .map_err(|_| ParseError(format!("expected a tabstop number at {start}")))
    }

    fn tabstop(&mut self) -> Result<(), ParseError> {
        let start = self.text_len;
        let mut choices = None;
        let number = if self.peek() == Some('{') {
            self.at += 1;
            let number = self.number()?;
            if self.peek() == Some('|') {
                self.at += 1;
                choices = Some(self.choices()?);
            }
            if self.peek() == Some(':') {
                self.at += 1;
                self.body(true)?;
            }
            if self.peek() != Some('}') {
                return Err(ParseError(format!("expected '}}' at {}", self.at)));
            }
            self.at += 1;
            number
        } else {
            self.number()?
        };
        self.stops
            .entry(number)
            .or_insert(Tabstop {
                ranges: Vec::new(),
                choices,
            })
            .ranges
            .push(start..self.text_len);
        Ok(())
    }

    /// `a,b|`: every char may be escaped; the first choice is what gets inserted.
    fn choices(&mut self) -> Result<Vec<String>, ParseError> {
        let mut choices = Vec::new();
        let mut current = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(ParseError("expected '|' closing the choices".into()));
            };
            self.at += 1;
            match c {
                '\\' => {
                    if let Some(escaped) = self.peek() {
                        current.push(escaped);
                        self.at += 1;
                    }
                }
                ',' => choices.push(std::mem::take(&mut current)),
                '|' => {
                    choices.push(current);
                    break;
                }
                _ => current.push(c),
            }
        }
        for c in choices
            .first()
            .map(|first| first.chars().collect::<Vec<_>>())
            .unwrap_or_default()
        {
            self.push(c);
        }
        Ok(choices)
    }
}

impl Snippet {
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let mut parser = Parser {
            source: source.chars().collect(),
            at: 0,
            text: String::with_capacity(source.len()),
            text_len: 0,
            stops: BTreeMap::new(),
        };
        parser.body(false)?;
        let end = parser.text_len;
        let last = parser.stops.remove(&0);
        let mut tabstops: Vec<Tabstop> = parser.stops.into_values().collect();
        match last {
            Some(last) => tabstops.push(last),
            None => {
                let at_end = Tabstop {
                    ranges: std::iter::once(end..end).collect(),
                    choices: None,
                };
                if tabstops.last() != Some(&at_end) {
                    tabstops.push(at_end);
                }
            }
        }
        Ok(Snippet {
            text: parser.text,
            tabstops,
        })
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// A snippet from a user file: the words that summon it, what it expands to, and how the menu names it.
#[derive(Clone, Debug)]
pub struct SnippetDefinition {
    pub name: String,
    pub prefixes: Vec<String>,
    pub body: Snippet,
    pub description: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(one) => vec![one],
            OneOrMany::Many(many) => many,
        }
    }

    fn joined(self) -> String {
        self.into_vec().join("\n")
    }
}

#[derive(Deserialize)]
struct SnippetEntry {
    prefix: Option<OneOrMany>,
    body: OneOrMany,
    description: Option<OneOrMany>,
}

/// Every snippet in a JSON file of `{"name": {"prefix", "body", "description"}}`; a snippet whose body doesn't
/// parse is reported and skipped, so one typo doesn't hide the rest. Sorted by name for a stable order.
pub fn parse_snippet_file(json: &str, source: &Path) -> Result<Vec<SnippetDefinition>, String> {
    let entries: BTreeMap<String, SnippetEntry> =
        serde_json::from_str(json).map_err(|error| format!("{}: {error}", source.display()))?;
    Ok(entries
        .into_iter()
        .filter_map(|(name, entry)| {
            let body = match Snippet::parse(&entry.body.joined()) {
                Ok(body) => body,
                Err(error) => {
                    eprintln!("snippet '{name}' in {}: {error}", source.display());
                    return None;
                }
            };
            let prefixes = entry
                .prefix
                .map_or_else(|| vec![name.clone()], OneOrMany::into_vec);
            Some(SnippetDefinition {
                name,
                prefixes,
                body,
                description: entry.description.map(OneOrMany::joined),
            })
        })
        .collect())
}

/// The file stem a language's snippets live under (`rust.json`); `snippets.json` holds ones for every language.
pub fn snippet_scope(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "rust",
        Lang::TypeScript => "typescript",
        Lang::Tsx => "tsx",
        Lang::JavaScript => "javascript",
        Lang::Go => "go",
        Lang::Python => "python",
        Lang::Json => "json",
        Lang::C => "c",
        Lang::Cpp => "c++",
        Lang::Bash => "shell script",
        Lang::Css => "css",
        Lang::Html => "html",
        Lang::Ruby => "ruby",
        Lang::Java => "java",
        Lang::Toml => "toml",
        Lang::Yaml => "yaml",
        Lang::Lua => "lua",
        Lang::CSharp => "c#",
        Lang::Markdown | Lang::MarkdownInline => "markdown",
        Lang::Php => "php",
        Lang::Scala => "scala",
        Lang::Elixir => "elixir",
        Lang::Haskell => "haskell",
        Lang::Ocaml => "ocaml",
        Lang::Scss => "scss",
        Lang::Nix => "nix",
        Lang::Swift => "swift",
        Lang::Make => "makefile",
        Lang::Xml => "xml",
        Lang::Zig => "zig",
        Lang::Dart => "dart",
        Lang::Sql => "sql",
        Lang::Kotlin => "kotlin",
        Lang::Svelte => "svelte",
        Lang::Dockerfile => "dockerfile",
        Lang::GraphQl => "graphql",
        Lang::Hcl => "hcl",
        Lang::Proto => "proto",
        Lang::Diff => "diff",
        Lang::GitCommit => "git commit",
        Lang::Ini => "ini",
        Lang::Erlang => "erlang",
        Lang::Gleam => "gleam",
        Lang::R => "r",
        Lang::Elm => "elm",
        Lang::Prisma => "prisma",
        Lang::Regex => "regex",
        Lang::JsDoc => "jsdoc",
        Lang::PlainText => "plaintext",
    }
}

pub const GLOBAL_SCOPE: &str = "snippets";

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The tails of `text` that begin at a word boundary, shortest first: `"a.bc"` gives `"bc"`, `".bc"`, `"a.bc"`.
/// A multi-word prefix like `"if let"` is matched against the tail with as many words.
pub fn boundary_suffixes(text: &str) -> Vec<&str> {
    let mut suffixes = Vec::new();
    let mut after: Option<char> = None;
    for (index, c) in text.char_indices().rev() {
        if let Some(next) = after {
            if !(is_word_char(c) && is_word_char(next)) {
                suffixes.push(&text[index + next.len_utf8()..]);
            }
        }
        after = Some(c);
    }
    if !text.is_empty() {
        suffixes.push(text);
    }
    suffixes
}

/// Whether typing `query` should bring up the menu for snippets alone: it has two chars or more and some
/// prefix (or a word-boundary tail of one) starts with it, ignoring case.
pub fn has_strong_prefix_match<'a>(
    query: &str,
    prefixes: impl IntoIterator<Item = &'a str>,
) -> bool {
    if query.chars().nth(1).is_none() {
        return false;
    }
    let query = query.to_lowercase();
    prefixes.into_iter().any(|prefix| {
        boundary_suffixes(prefix)
            .into_iter()
            .any(|tail| tail.to_lowercase().starts_with(&query))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stops(snippet: &Snippet) -> Vec<Vec<Range<usize>>> {
        snippet.tabstops.iter().map(|t| t.ranges.clone()).collect()
    }

    fn choices(snippet: &Snippet) -> Vec<Option<Vec<String>>> {
        snippet.tabstops.iter().map(|t| t.choices.clone()).collect()
    }

    #[test]
    fn plain_text_gets_a_stop_at_the_end() {
        let snippet = Snippet::parse("one-two-three").unwrap();
        assert_eq!(snippet.text, "one-two-three");
        assert_eq!(stops(&snippet), vec![vec![13..13]]);
    }

    #[test]
    fn numbered_stops_visit_in_ascending_order() {
        let snippet = Snippet::parse("one$1two").unwrap();
        assert_eq!(snippet.text, "onetwo");
        assert_eq!(stops(&snippet), vec![vec![3..3], vec![6..6]]);
        let snippet = Snippet::parse("one$123-$99-two").unwrap();
        assert_eq!(snippet.text, "one--two");
        assert_eq!(stops(&snippet), vec![vec![4..4], vec![3..3], vec![8..8]]);
    }

    #[test]
    fn no_extra_stop_when_the_last_is_at_the_end_or_explicit() {
        let snippet = Snippet::parse("foo.$1").unwrap();
        assert_eq!(stops(&snippet), vec![vec![4..4]]);
        let snippet = Snippet::parse(r#"<div class="$1">$0</div>"#).unwrap();
        assert_eq!(snippet.text, r#"<div class=""></div>"#);
        assert_eq!(stops(&snippet), vec![vec![12..12], vec![14..14]]);
    }

    #[test]
    fn placeholders_select_their_text() {
        let snippet = Snippet::parse("one${1:two}three${2:four}").unwrap();
        assert_eq!(snippet.text, "onetwothreefour");
        assert_eq!(
            stops(&snippet),
            vec![vec![3..6], vec![11..15], vec![15..15]]
        );
    }

    #[test]
    fn choices_insert_the_first_and_keep_escapes() {
        let snippet = Snippet::parse("type ${1|i32, u32|} = $2").unwrap();
        assert_eq!(snippet.text, "type i32 = ");
        assert_eq!(stops(&snippet), vec![vec![5..8], vec![11..11]]);
        assert_eq!(
            choices(&snippet)[0],
            Some(vec!["i32".to_string(), " u32".to_string()])
        );
        let snippet = Snippet::parse(r"${1|one,two\|2,three\\3|}").unwrap();
        assert_eq!(snippet.text, "one");
        assert_eq!(
            choices(&snippet)[0],
            Some(vec!["one".into(), "two|2".into(), r"three\3".into()])
        );
    }

    #[test]
    fn nested_placeholders_and_repeated_stops() {
        let snippet = Snippet::parse(
            "for (${1:var ${2:i} = 0; ${2:i} < ${3:${4:array}.length}; ${2:i}++}) {$0}",
        )
        .unwrap();
        assert_eq!(snippet.text, "for (var i = 0; i < array.length; i++) {}");
        assert_eq!(
            stops(&snippet),
            vec![
                vec![5..37],
                vec![9..10, 16..17, 34..35],
                vec![20..32],
                vec![20..25],
                vec![40..40],
            ]
        );
    }

    #[test]
    fn escapes() {
        assert_eq!(
            Snippet::parse("\"\\$schema\": $1").unwrap().text,
            "\"$schema\": "
        );
        assert_eq!(Snippet::parse("{a\\}").unwrap().text, "{a}");
        assert_eq!(Snippet::parse("a\\b").unwrap().text, "a\\b");
        let snippet = Snippet::parse("one\\\\$1two").unwrap();
        assert_eq!(snippet.text, "one\\two");
        assert_eq!(stops(&snippet), vec![vec![4..4], vec![7..7]]);
    }

    #[test]
    fn offsets_count_chars_not_bytes() {
        let snippet = Snippet::parse("é${1:ü}x").unwrap();
        assert_eq!(stops(&snippet), vec![vec![1..2], vec![3..3]]);
    }

    #[test]
    fn malformed_bodies_fail() {
        assert!(Snippet::parse("${1:open").is_err());
        assert!(Snippet::parse("$x").is_err());
    }

    #[test]
    fn reads_a_snippet_file() {
        let json = r#"{
            "Log": {"prefix": ["log", "cl"], "body": ["console.log(${1:value});", "$0"], "description": "Print"},
            "fn": {"body": "fn $1() {}"},
            "bad": {"prefix": "bad", "body": "${1:x"}
        }"#;
        let snippets = parse_snippet_file(json, Path::new("javascript.json")).unwrap();
        assert_eq!(snippets.len(), 2);
        assert_eq!(snippets[0].name, "Log");
        assert_eq!(snippets[0].prefixes, vec!["log", "cl"]);
        assert_eq!(snippets[0].body.text, "console.log(value);\n");
        assert_eq!(snippets[0].description.as_deref(), Some("Print"));
        assert_eq!(snippets[1].prefixes, vec!["fn"]);
    }

    #[test]
    fn suffixes_start_at_word_boundaries() {
        assert_eq!(boundary_suffixes("a.bc"), vec!["bc", ".bc", "a.bc"]);
        assert_eq!(boundary_suffixes("if let"), vec!["let", " let", "if let"]);
        assert!(has_strong_prefix_match("le", ["if let"]));
        assert!(has_strong_prefix_match("Fo", ["for"]));
        assert!(!has_strong_prefix_match("f", ["for"]));
        assert!(!has_strong_prefix_match("fx", ["for"]));
    }
}
