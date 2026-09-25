//! A file's symbols (functions, types, fields, headings...) from an outline query: `@item` spans the symbol,
//! `@name` and `@context` nodes make up its label (`pub fn main`), and items nest by containment.

use std::cmp::Reverse;
use std::ops::Range;

use ropey::Rope;
use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator, Tree};

use crate::highlight::Lang;

fn outline_patterns(lang: Lang) -> Option<&'static str> {
    Some(match lang {
        Lang::Rust => include_str!("../queries/rust/outline.scm"),
        Lang::JavaScript => include_str!("../queries/javascript/outline.scm"),
        Lang::TypeScript => include_str!("../queries/typescript/outline.scm"),
        Lang::Tsx => include_str!("../queries/tsx/outline.scm"),
        Lang::Go => include_str!("../queries/go/outline.scm"),
        Lang::Python => include_str!("../queries/python/outline.scm"),
        Lang::C => include_str!("../queries/c/outline.scm"),
        Lang::Cpp => include_str!("../queries/cpp/outline.scm"),
        Lang::Java => include_str!("../queries/java/outline.scm"),
        Lang::Markdown => include_str!("../queries/markdown/outline.scm"),
        Lang::Yaml => include_str!("../queries/yaml/outline.scm"),
        Lang::Json => include_str!("../queries/json/outline.scm"),
        Lang::Css => include_str!("../queries/css/outline.scm"),
        Lang::Bash => include_str!("../queries/bash/outline.scm"),
        Lang::Ruby => include_str!("../queries/ruby/outline.scm"),
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    /// How many items enclose this one.
    pub depth: usize,
    /// Bytes the symbol spans.
    pub range: Range<usize>,
    /// Bytes of its name nodes.
    pub selection_range: Range<usize>,
    /// The label: context and name nodes (first line of each), space-separated where the source had a gap.
    pub text: String,
    /// Byte ranges into `text` that came from the source, paired with the source byte they start at, for
    /// coloring the label like the code.
    pub source_spans: Vec<(Range<usize>, usize)>,
    /// Byte ranges into `text` holding the name.
    pub name_ranges: Vec<Range<usize>>,
}

pub struct OutlineQuery {
    query: Query,
    item_ix: u32,
    name_ix: u32,
    context_ix: Option<u32>,
}

impl OutlineQuery {
    pub fn new(lang: Lang, language: &Language) -> Option<Self> {
        let query = Query::new(language, outline_patterns(lang)?).ok()?;
        let index_of = |name: &str| {
            query
                .capture_names()
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };
        Some(Self {
            item_ix: index_of("item")?,
            name_ix: index_of("name")?,
            context_ix: index_of("context"),
            query,
        })
    }

    pub fn items(&self, rope: &Rope, tree: &Tree) -> Vec<OutlineItem> {
        let text = |node: Node| {
            let range = node.byte_range();
            let end = range.end.min(rope.len_bytes());
            let start = range.start.min(end);
            rope.byte_slice(start..end).chunks().map(str::as_bytes)
        };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), text);
        let mut items = Vec::new();
        while let Some(found) = matches.next() {
            let Some(item) = found
                .captures()
                .iter()
                .find(|capture| capture.index == self.item_ix)
            else {
                continue;
            };
            // Each label part keeps only its first line.
            let mut parts: Vec<(Range<usize>, bool)> = Vec::new();
            for capture in found.captures() {
                let is_name = capture.index == self.name_ix;
                if !is_name && Some(capture.index) != self.context_ix {
                    continue;
                }
                let node = capture.node;
                let mut range = node.byte_range();
                if node.end_position().row > node.start_position().row {
                    let row = node.start_position().row;
                    let line_end = rope.line_to_byte(row) + line_len_bytes(rope, row);
                    range.end = range.end.min(line_end);
                }
                if !range.is_empty() {
                    parts.push((range, is_name));
                }
            }
            parts.sort_by_key(|(range, _)| range.start);
            let Some(selection_range) = parts
                .iter()
                .filter(|(_, is_name)| *is_name)
                .map(|(range, _)| range.clone())
                .reduce(|combined, next| combined.start..next.end)
            else {
                continue;
            };
            let mut label = String::new();
            let mut source_spans = Vec::new();
            let mut name_ranges: Vec<Range<usize>> = Vec::new();
            let mut last_end = 0;
            for (range, is_name) in parts {
                let space_added = !label.is_empty() && range.start > last_end;
                if space_added {
                    label.push(' ');
                }
                let start = label.len();
                label.push_str(&rope.byte_slice(range.clone()).to_string());
                source_spans.push((start..label.len(), range.start));
                if is_name {
                    // Consecutive name parts read as one name, the space between included.
                    let name_start = if space_added && !name_ranges.is_empty() {
                        start - 1
                    } else {
                        start
                    };
                    name_ranges.push(name_start..label.len());
                }
                last_end = range.end;
            }
            items.push(OutlineItem {
                depth: 0,
                range: item.node.byte_range(),
                selection_range,
                text: label,
                source_spans,
                name_ranges,
            });
        }
        items.sort_by_key(|item| (item.range.start, Reverse(item.range.end)));
        items.dedup_by(|a, b| a.range == b.range && a.text == b.text);
        let mut enclosing_ends: Vec<usize> = Vec::new();
        for item in &mut items {
            while enclosing_ends
                .last()
                .is_some_and(|end| *end < item.range.end)
            {
                enclosing_ends.pop();
            }
            item.depth = enclosing_ends.len();
            enclosing_ends.push(item.range.end);
        }
        items
    }
}

fn line_len_bytes(rope: &Rope, row: usize) -> usize {
    rope.line(row)
        .chars()
        .take_while(|c| *c != '\n' && *c != '\r')
        .map(char::len_utf8)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::EditorBuffer;
    use crate::syntax::Syntax;
    use std::time::Duration;

    fn outline(lang: Lang, text: &str) -> Vec<(usize, String)> {
        let buffer = EditorBuffer::from_text(text);
        let mut syntax = Syntax::new(lang).unwrap();
        syntax.sync(&buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&buffer);
        }
        syntax
            .outline(&buffer.rope)
            .into_iter()
            .map(|item| (item.depth, item.text))
            .collect()
    }

    #[test]
    fn outline_queries_compile() {
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
            Lang::Markdown,
            Lang::Ruby,
            Lang::Yaml,
            Lang::Json,
            Lang::Css,
            Lang::Bash,
        ] {
            let (language, _) = crate::highlight::grammar(lang).unwrap();
            let source = outline_patterns(lang).unwrap();
            if let Err(error) = Query::new(&language, source) {
                panic!("{source}: {error}");
            }
        }
    }

    #[test]
    fn rust_items_nest_with_their_context() {
        let text = "pub struct Point {\n    x: i32,\n}\n\nimpl Display for Point {\n    fn fmt(&self) {}\n}\n\nasync fn run() {}\n";
        assert_eq!(
            outline(Lang::Rust, text),
            vec![
                (0, "pub struct Point".to_string()),
                (1, "x".to_string()),
                (0, "impl Display for Point".to_string()),
                (1, "fn fmt".to_string()),
                (0, "async fn run".to_string()),
            ]
        );
    }

    #[test]
    fn ruby_outline_has_definitions_and_spec_blocks() {
        assert_eq!(
            outline(
                Lang::Ruby,
                "module Billing\n  class Invoice\n    LIMIT = 3\n    def total\n    end\n    def self.build\n    end\n  end\nend\n"
            ),
            vec![
                (0, "module Billing".to_string()),
                (1, "class Invoice".to_string()),
                (2, "LIMIT".to_string()),
                (2, "def total".to_string()),
                (2, "def self.build".to_string()),
            ]
        );
        assert_eq!(
            outline(
                Lang::Ruby,
                "RSpec.describe Invoice do\n  let(:user) { 1 }\n  context \"when empty\" do\n    it \"is zero\" do\n    end\n  end\nend\n"
            ),
            vec![
                (0, "describe Invoice".to_string()),
                (1, "context \"when empty\"".to_string()),
                (2, "it \"is zero\"".to_string()),
            ]
        );
    }

    #[test]
    fn python_and_markdown_outlines() {
        assert_eq!(
            outline(Lang::Python, "class A:\n    def b(self):\n        pass\n"),
            vec![(0, "class A".to_string()), (1, "def b".to_string())]
        );
        assert_eq!(
            outline(Lang::Markdown, "# Title\n\ntext\n\n## Part\n\nmore\n"),
            vec![(0, "# Title".to_string()), (1, "## Part".to_string())]
        );
    }
}
