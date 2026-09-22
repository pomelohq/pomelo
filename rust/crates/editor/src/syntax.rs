//! Incremental syntax tree + range highlighting.
//!
//! On each sync the tree is first *interpolated* (every logged edit fed to `Tree::edit`, so node positions
//! track the new text), then reparsed with the old tree under a ~1ms budget. If the budget runs out the parse
//! continues on a background thread and the interpolated tree keeps highlighting until it lands; edits made
//! meanwhile are replayed onto the result.
//!
//! Highlighting runs the highlights query only over the requested byte range and walks captures with a stack
//! of `(end, name)`: the innermost capture wins, so each emitted run is cut at the next capture start or the
//! top capture's end.

use crate::buffer::EditorBuffer;
use crate::highlight::{grammar, Lang, HIGHLIGHT_NAMES};
use ropey::Rope;
use std::ops::{ControlFlow, Range};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};
use tree_sitter::{
    Language, Node, ParseOptions, Parser, Query, QueryCaptures, QueryCursor, StreamingIterator,
    TextProvider, Tree,
};

const SYNC_PARSE_BUDGET: Duration = Duration::from_millis(1);

/// A run of source bytes and the theme key it should be colored with (`None` = default foreground).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightRun {
    pub range: Range<usize>,
    pub capture: Option<&'static str>,
}

pub struct Syntax {
    language: Language,
    query: Query,
    /// Query capture index -> recognized theme key.
    capture_keys: Vec<Option<&'static str>>,
    tree: Option<Tree>,
    /// Buffer version the tree's positions reflect (edits fed through `Tree::edit` up to here).
    interpolated_version: u64,
    /// Buffer version the tree was last actually parsed against.
    parsed_version: u64,
    background: Option<Receiver<(Option<Tree>, u64)>>,
}

impl Syntax {
    /// `None` for plain text or when the grammar's query fails to compile against the linked tree-sitter.
    pub fn new(lang: Lang) -> Option<Self> {
        let (language, source) = grammar(lang)?;
        let query = Query::new(&language, source).ok()?;
        let capture_keys = query
            .capture_names()
            .iter()
            .map(|name| recognized_key(name))
            .collect();
        Some(Self {
            language,
            query,
            capture_keys,
            tree: None,
            interpolated_version: 0,
            parsed_version: 0,
            background: None,
        })
    }

    /// Whether a background parse is still running (the caller should keep repainting to pick it up).
    pub fn is_parsing(&self) -> bool {
        self.background.is_some()
    }

    /// Bring the tree up to date with `buffer`.
    pub fn sync(&mut self, buffer: &EditorBuffer) {
        let version = buffer.version();
        self.collect_background(buffer);
        if let Some(tree) = self.tree.as_mut() {
            if self.interpolated_version < version {
                for edit in buffer.syntax_edits_since(self.interpolated_version) {
                    tree.edit(edit);
                }
            }
        }
        self.interpolated_version = version;
        let fresh = self.tree.is_some() && self.parsed_version == version;
        if fresh || self.background.is_some() {
            return;
        }
        let mut parser = Parser::new();
        if parser.set_language(&self.language).is_err() {
            return;
        }
        let started = Instant::now();
        let mut out_of_budget = |_: &tree_sitter::ParseState| {
            if started.elapsed() > SYNC_PARSE_BUDGET {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = ParseOptions::new().progress_callback(&mut out_of_budget);
        let rope = &buffer.rope;
        let parsed = parser.parse_with_options(
            &mut |byte, _| chunk_from(rope, byte),
            self.tree.as_ref(),
            Some(options),
        );
        match parsed {
            Some(tree) => {
                self.tree = Some(tree);
                self.parsed_version = version;
            }
            None => self.parse_in_background(buffer.rope.clone(), version),
        }
    }

    fn parse_in_background(&mut self, rope: Rope, version: u64) {
        let language = self.language.clone();
        let old_tree = self.tree.clone();
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let mut parser = Parser::new();
            let tree = match parser.set_language(&language) {
                Ok(()) => parser.parse_with_options(
                    &mut |byte, _| chunk_from(&rope, byte),
                    old_tree.as_ref(),
                    None,
                ),
                Err(_) => None,
            };
            // The receiver is gone only if the editor closed; nothing to report then.
            let _ = sender.send((tree, version));
        });
        self.background = Some(receiver);
    }

    /// Adopt a finished background parse, replaying edits the user made while it ran.
    fn collect_background(&mut self, buffer: &EditorBuffer) {
        let Some(receiver) = self.background.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok((Some(mut tree), parsed_at)) => {
                for edit in buffer.syntax_edits_since(parsed_at) {
                    tree.edit(edit);
                }
                self.tree = Some(tree);
                self.parsed_version = parsed_at;
                self.interpolated_version = buffer.version();
                self.background = None;
            }
            Ok((None, _)) | Err(TryRecvError::Disconnected) => self.background = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Highlight runs covering `range` (bytes) contiguously; uncaptured text gets `capture: None`.
    pub fn highlight(&self, rope: &Rope, range: Range<usize>) -> Vec<HighlightRun> {
        let mut runs = Vec::new();
        let Some(tree) = self.tree.as_ref() else {
            runs.push(HighlightRun {
                range,
                capture: None,
            });
            return runs;
        };
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let text = |node: Node| rope_chunks(rope, node.byte_range());
        let mut captures = cursor.captures(&self.query, tree.root_node(), text);
        let mut stack: Vec<(usize, usize, &'static str)> = Vec::new();
        let mut next = next_capture(&mut captures, &self.capture_keys);
        let mut pos = range.start;
        while pos < range.end {
            while stack.last().is_some_and(|(_, end, _)| *end <= pos) {
                stack.pop();
            }
            while let Some((start, end, key)) = next {
                if start > pos {
                    break;
                }
                // Several patterns can capture the same node; the bundled queries list the most specific one
                // first, so the first capture for a node keeps it.
                let same_node = stack
                    .last()
                    .is_some_and(|(s, e, _)| *s == start && *e == end);
                if end > pos && !same_node {
                    stack.push((start, end, key));
                }
                next = next_capture(&mut captures, &self.capture_keys);
            }
            let next_start = next.map_or(usize::MAX, |(start, _, _)| start);
            let mut end = range.end.min(next_start);
            if let Some((_, top_end, _)) = stack.last() {
                end = end.min(*top_end);
            }
            let end = end.max(pos + 1).min(range.end);
            let capture = stack.last().map(|(_, _, key)| *key);
            match runs.last_mut() {
                Some(last) if last.capture == capture && last.range.end == pos => {
                    last.range.end = end
                }
                _ => runs.push(HighlightRun {
                    range: pos..end,
                    capture,
                }),
            }
            pos = end;
        }
        runs
    }
}

fn next_capture<T, I>(
    captures: &mut QueryCaptures<'_, '_, '_, T, I>,
    keys: &[Option<&'static str>],
) -> Option<(usize, usize, &'static str)>
where
    T: TextProvider<I>,
    I: AsRef<[u8]>,
{
    while let Some((m, index)) = captures.next() {
        let Some(capture) = m.captures().get(*index) else {
            continue;
        };
        if let Some(Some(key)) = keys.get(capture.index as usize) {
            return Some((capture.node.start_byte(), capture.node.end_byte(), *key));
        }
    }
    None
}

/// The longest recognized key that is a whole-segment dotted prefix of `name` (`function.method.call` ->
/// `function.method`), or `None` if nothing matches.
fn recognized_key(name: &str) -> Option<&'static str> {
    HIGHLIGHT_NAMES
        .iter()
        .filter(|key| {
            name.strip_prefix(**key)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
        .max_by_key(|key| key.len())
        .copied()
}

fn chunk_from(rope: &Rope, byte: usize) -> &[u8] {
    if byte >= rope.len_bytes() {
        return &[];
    }
    let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
    &chunk.as_bytes()[byte - chunk_start..]
}

fn rope_chunks(rope: &Rope, range: Range<usize>) -> impl Iterator<Item = &[u8]> {
    let end = range.end.min(rope.len_bytes());
    let start = range.start.min(end);
    rope.byte_slice(start..end).chunks().map(str::as_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlighted(text: &str) -> Vec<(String, Option<&'static str>)> {
        let buffer = EditorBuffer::from_text(text);
        let mut syntax = Syntax::new(Lang::Rust).unwrap();
        syntax.sync(&buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&buffer);
        }
        syntax
            .highlight(&buffer.rope, 0..buffer.rope.len_bytes())
            .into_iter()
            .map(|run| (text[run.range].to_string(), run.capture))
            .collect()
    }

    #[test]
    fn runs_cover_range_contiguously() {
        let text = "fn main() { let x = 1; }";
        let runs = highlighted(text);
        let joined: String = runs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(joined, text);
        assert!(runs.contains(&("fn".to_string(), Some("keyword"))));
        assert!(runs.contains(&("main".to_string(), Some("function"))));
    }

    #[test]
    fn first_pattern_wins_for_same_node() {
        // `(use_list (self) @keyword)` precedes the generic `(self) @variable.builtin` in the query.
        let runs = highlighted("use std::io::{self};");
        assert!(runs.contains(&("self".to_string(), Some("keyword"))));
    }

    #[test]
    fn edits_reparse_incrementally() {
        let mut buffer = EditorBuffer::from_text("fn a() {}");
        let mut syntax = Syntax::new(Lang::Rust).unwrap();
        syntax.sync(&buffer);
        buffer.place_cursor(0);
        buffer.insert_text("pub ");
        syntax.sync(&buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&buffer);
        }
        let runs = syntax.highlight(&buffer.rope, 0..buffer.rope.len_bytes());
        let text = buffer.text();
        assert!(runs
            .iter()
            .any(|r| &text[r.range.clone()] == "pub" && r.capture == Some("keyword")));
    }

    #[test]
    fn dotted_prefix_picks_longest_key() {
        assert_eq!(
            recognized_key("function.method.call"),
            Some("function.method")
        );
        assert_eq!(recognized_key("keyword.control"), Some("keyword"));
        assert_eq!(recognized_key("nonsense"), None);
    }
}
