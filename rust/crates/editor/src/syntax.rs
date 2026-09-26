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

use crate::buffer::{EditorBuffer, TAB_SIZE};
use crate::highlight::{grammar, Lang, HIGHLIGHT_NAMES};
use crate::indent::{compute_autoindents, IndentQuery, IndentRegexes, IndentSize, IndentView};
use crate::injection::{InjectionQuery, Injections, LayerCapture};
use crate::outline::{OutlineItem, OutlineQuery};
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
    queries: std::sync::Arc<LanguageQueries>,
    tree: Option<Tree>,
    /// Buffer version the tree's positions reflect (edits fed through `Tree::edit` up to here).
    interpolated_version: u64,
    /// Buffer version the tree was last actually parsed against.
    parsed_version: u64,
    background: Option<Receiver<(Option<Tree>, u64)>>,
    injections: Injections,
    /// Buffer version the injection layers' positions reflect.
    injections_interpolated: u64,
    /// Buffer version the layers were last rebuilt from a fresh parse at.
    injections_parsed: Option<u64>,
    /// The file's tree at that rebuild, kept in step with edits since, to find what the next parse changed.
    injection_base: Option<Tree>,
    /// Byte ranges edited since that rebuild.
    injection_edits: Vec<Range<usize>>,
    /// The tree as of the last sync and the buffer version it reflects: the "before" side when re-indenting
    /// lines after the next edit.
    synced: Option<(Tree, u64)>,
}

/// A language's grammar and compiled queries. Compiling them costs tens of milliseconds, so each language is
/// compiled once and shared by every open file.
struct LanguageQueries {
    language: Language,
    query: Query,
    /// Query capture index -> recognized theme key.
    capture_keys: Vec<Option<&'static str>>,
    indent: Option<IndentQuery>,
    indent_regexes: IndentRegexes,
    brackets: Option<BracketQuery>,
    overrides: Option<OverrideQuery>,
    outline: Option<OutlineQuery>,
    injection_query: Option<InjectionQuery>,
}

impl LanguageQueries {
    fn for_lang(lang: Lang) -> Option<std::sync::Arc<LanguageQueries>> {
        type Cache = std::sync::Mutex<
            std::collections::HashMap<Lang, Option<std::sync::Arc<LanguageQueries>>>,
        >;
        static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some(compiled) = cache.lock().ok()?.get(&lang) {
            return compiled.clone();
        }
        // Compiled outside the lock so a slow language doesn't hold up files of another.
        let compiled = LanguageQueries::compile(lang).map(std::sync::Arc::new);
        cache.lock().ok()?.entry(lang).or_insert(compiled).clone()
    }

    fn compile(lang: Lang) -> Option<LanguageQueries> {
        let (language, source) = grammar(lang)?;
        let query = Query::new(&language, source).ok()?;
        let capture_keys = query
            .capture_names()
            .iter()
            .map(|name| recognized_key(name))
            .collect();
        Some(LanguageQueries {
            indent: IndentQuery::new(lang, &language),
            brackets: BracketQuery::new(&language),
            overrides: OverrideQuery::new(lang, &language),
            outline: OutlineQuery::new(lang, &language),
            injection_query: InjectionQuery::new(lang, &language),
            indent_regexes: IndentRegexes::new(&crate::language::config(lang).indent),
            language,
            query,
            capture_keys,
        })
    }
}

impl Syntax {
    /// `None` for plain text or when the grammar's query fails to compile against the linked tree-sitter.
    pub fn new(lang: Lang) -> Option<Self> {
        Some(Self {
            queries: LanguageQueries::for_lang(lang)?,
            injections: Injections::default(),
            injections_interpolated: 0,
            injections_parsed: None,
            injection_base: None,
            injection_edits: Vec::new(),
            tree: None,
            interpolated_version: 0,
            parsed_version: 0,
            background: None,
            synced: None,
        })
    }

    /// Whether a background parse is still running (the caller should keep repainting to pick it up).
    pub fn is_parsing(&self) -> bool {
        self.background.is_some()
    }

    fn interpolate(&mut self, buffer: &EditorBuffer) {
        let version = buffer.version();
        if self.injections_interpolated < version {
            for edit in buffer.syntax_edits_since(self.injections_interpolated) {
                self.injections.edit(edit);
                if let Some(base) = self.injection_base.as_mut() {
                    base.edit(edit);
                }
                let delta = edit.new_end_byte as isize - edit.old_end_byte as isize;
                for range in &mut self.injection_edits {
                    if range.start >= edit.old_end_byte {
                        range.start = (range.start as isize + delta).max(0) as usize;
                        range.end = (range.end as isize + delta).max(0) as usize;
                    } else if range.end >= edit.start_byte {
                        range.end = (range.end as isize + delta).max(range.start as isize) as usize;
                    }
                }
                self.injection_edits
                    .push(edit.start_byte..edit.new_end_byte);
            }
            self.injections_interpolated = version;
        }
        self.collect_background(buffer);
        if let Some(tree) = self.tree.as_mut() {
            if self.interpolated_version < version {
                for edit in buffer.syntax_edits_since(self.interpolated_version) {
                    tree.edit(edit);
                }
            }
        }
        self.interpolated_version = version;
    }

    /// Rebuild the embedded-language layers once the file's own tree is parsed for the current text.
    fn refresh_injections(&mut self, buffer: &EditorBuffer) {
        let version = buffer.version();
        if self.parsed_version != version || self.injections_parsed == Some(version) {
            return;
        }
        if let (Some(tree), Some(query)) =
            (self.tree.as_ref(), self.queries.injection_query.as_ref())
        {
            let changed: Option<Vec<Range<usize>>> = self.injection_base.as_ref().map(|base| {
                let mut changed: Vec<Range<usize>> = base
                    .changed_ranges(tree)
                    .map(|range| range.start_byte..range.end_byte)
                    .collect();
                changed.extend(self.injection_edits.iter().cloned());
                changed
            });
            self.injections
                .update(&buffer.rope, tree, query, changed.as_deref());
            self.injection_base = Some(tree.clone());
        }
        self.injection_edits.clear();
        self.injections_parsed = Some(version);
    }

    /// Bring the tree up to date with `buffer`.
    pub fn sync(&mut self, buffer: &EditorBuffer) {
        self.parse_within_budget(buffer);
        self.refresh_injections(buffer);
        self.synced = self.tree.clone().map(|tree| (tree, buffer.version()));
    }

    fn parse_within_budget(&mut self, buffer: &EditorBuffer) {
        let version = buffer.version();
        self.interpolate(buffer);
        let fresh = self.tree.is_some() && self.parsed_version == version;
        if fresh || self.background.is_some() {
            return;
        }
        let mut parser = Parser::new();
        if parser.set_language(&self.queries.language).is_err() {
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

    /// Parse to completion on this thread, abandoning any background parse.
    fn parse_now(&mut self, buffer: &EditorBuffer) {
        let version = buffer.version();
        self.interpolate(buffer);
        if self.tree.is_some() && self.parsed_version == version {
            return;
        }
        self.background = None;
        let mut parser = Parser::new();
        if parser.set_language(&self.queries.language).is_err() {
            return;
        }
        let rope = &buffer.rope;
        if let Some(tree) = parser.parse_with_options(
            &mut |byte, _| chunk_from(rope, byte),
            self.tree.as_ref(),
            None,
        ) {
            self.tree = Some(tree);
            self.parsed_version = version;
        }
    }

    /// Re-indent the lines `buffer`'s latest edits asked for, comparing indent suggestions on the text before
    /// and after each edit. Call before `sync`, which records the "before" tree for the next edit.
    pub fn autoindent(&mut self, buffer: &mut EditorBuffer) {
        let requests = buffer.take_autoindent_requests();
        let Some((before_tree, before_version)) = self.synced.clone() else {
            return;
        };
        let requests: Vec<_> = requests
            .into_iter()
            .filter(|request| request.before_version == before_version)
            .collect();
        if requests.is_empty() || self.queries.indent.is_none() {
            return;
        }
        // Edits wait on the parse, as the indent query needs the tree for the new text.
        self.parse_now(buffer);
        let (Some(tree), Some(query)) = (self.tree.as_ref(), self.queries.indent.as_ref()) else {
            return;
        };
        let view = IndentView {
            rope: &buffer.rope,
            tree,
            query,
            regexes: &self.queries.indent_regexes,
        };
        let sizes =
            compute_autoindents(&requests, &before_tree, &view, IndentSize::spaces(TAB_SIZE));
        buffer.apply_autoindents(sizes);
    }

    fn parse_in_background(&mut self, rope: Rope, version: u64) {
        let language = self.queries.language.clone();
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

    /// Bracket pairs (open, close byte ranges) whose span covers `range`, including brackets touching it.
    pub fn enclosing_bracket_ranges(
        &self,
        rope: &Rope,
        range: Range<usize>,
    ) -> Vec<(Range<usize>, Range<usize>)> {
        let (Some(tree), Some(brackets)) = (self.tree.as_ref(), self.queries.brackets.as_ref())
        else {
            return Vec::new();
        };
        let len = rope.len_bytes();
        let start = range.start.min(len);
        let end = range.end.min(len);
        // Widen by one char either side so a caret right next to a bracket still finds its pair.
        let before = rope.byte_to_char(start).saturating_sub(1);
        let after = (rope.byte_to_char(end) + 1).min(rope.len_chars());
        let query_range = rope.char_to_byte(before)..rope.char_to_byte(after);
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(query_range);
        let text = |node: Node| rope_chunks(rope, node.byte_range());
        let mut matches = cursor.matches(&brackets.query, tree.root_node(), text);
        let mut pairs = Vec::new();
        while let Some(found) = matches.next() {
            let mut open = None;
            let mut close = None;
            for capture in found.captures() {
                if capture.node.is_missing() {
                    continue;
                }
                if Some(capture.index) == brackets.open_ix {
                    open = Some(capture.node.byte_range());
                } else if Some(capture.index) == brackets.close_ix {
                    close = Some(capture.node.byte_range());
                }
            }
            if let (Some(open), Some(close)) = (open, close) {
                if open.start <= start
                    && close.end >= end
                    && !pairs.contains(&(open.clone(), close.clone()))
                {
                    pairs.push((open, close));
                }
            }
        }
        pairs
    }

    /// The file's symbols in source order, nested by containment.
    pub fn outline(&self, rope: &Rope) -> Vec<OutlineItem> {
        match (self.queries.outline.as_ref(), self.tree.as_ref()) {
            (Some(outline), Some(tree)) => outline.items(rope, tree),
            _ => Vec::new(),
        }
    }

    /// The smallest bracket pair around `range`.
    pub fn innermost_enclosing_bracket_ranges(
        &self,
        rope: &Rope,
        range: Range<usize>,
    ) -> Option<(Range<usize>, Range<usize>)> {
        self.enclosing_bracket_ranges(rope, range)
            .into_iter()
            .min_by_key(|(open, close)| close.end - open.start)
    }

    /// End byte of the smallest named node spanning `range`.
    pub fn enclosing_node_end(&self, range: Range<usize>) -> Option<usize> {
        let tree = self.tree.as_ref()?;
        let mut node = tree
            .root_node()
            .named_descendant_for_byte_range(range.start, range.end)?;
        while node.start_byte() > range.start || node.end_byte() < range.end {
            node = node.parent()?;
        }
        Some(node.end_byte())
    }

    /// Whether `byte` lies inside a string or comment node.
    pub fn in_string_or_comment(&self, byte: usize) -> bool {
        let Some(tree) = self.tree.as_ref() else {
            return false;
        };
        let mut node = tree.root_node().descendant_for_byte_range(byte, byte);
        while let Some(n) = node {
            let kind = n.kind();
            if kind.contains("string") || kind.contains("comment") {
                return true;
            }
            node = n.parent();
        }
        false
    }

    /// The smallest node that encloses the byte `range` and is larger than it. For an empty range sitting
    /// between two nodes, the right one wins unless only the left one is named.
    pub fn syntax_ancestor(&self, range: Range<usize>) -> Option<SyntaxNode> {
        let tree = self.tree.as_ref()?;
        let mut cursor = tree.root_node().walk();
        if !goto_node_enclosing_range(&mut cursor, &range) {
            return None;
        }
        let left = cursor.node();
        let mut result = left;
        if left.end_byte() == range.start {
            let mut right = None;
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    break;
                }
            }
            while cursor.node().start_byte() == range.start {
                right = Some(cursor.node());
                if !cursor.goto_first_child() {
                    break;
                }
            }
            if let Some(right) = right.filter(|r| r.is_named() || !left.is_named()) {
                result = right;
            }
        }
        Some(SyntaxNode {
            range: result.byte_range(),
            kind: result.kind().to_string(),
            named: result.is_named(),
        })
    }

    /// Whether `byte` is in a string or a comment. With an overrides query the smallest captured node around
    /// `byte` decides (a `.inclusive` capture also counts its own edges); otherwise any enclosing node whose
    /// kind names a string or comment does, edges excluded.
    pub fn scope_at(&self, rope: &Rope, byte: usize) -> crate::language::Scope {
        let mut scope = crate::language::Scope::default();
        let Some(tree) = self.tree.as_ref() else {
            return scope;
        };
        if let Some(overrides) = self.queries.overrides.as_ref() {
            match overrides.name_at(rope, tree, byte) {
                Some("string") => scope.in_string = true,
                Some("comment") => scope.in_comment = true,
                _ => {}
            }
            return scope;
        }
        let mut node = tree.root_node().descendant_for_byte_range(byte, byte);
        while let Some(n) = node {
            if n.start_byte() < byte && byte < n.end_byte() {
                let kind = n.kind();
                scope.in_string |= kind.contains("string");
                scope.in_comment |= kind.contains("comment");
            }
            node = n.parent();
        }
        scope
    }

    /// Highlight runs covering `range` (bytes) contiguously; uncaptured text gets `capture: None`. Embedded
    /// languages color over the text that hosts them.
    pub fn highlight(&self, rope: &Rope, range: Range<usize>) -> Vec<HighlightRun> {
        let mut runs = Vec::new();
        let Some(tree) = self.tree.as_ref() else {
            runs.push(HighlightRun {
                range,
                capture: None,
            });
            return runs;
        };
        let mut captures: Vec<LayerCapture> = Vec::new();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let text = |node: Node| rope_chunks(rope, node.byte_range());
        let mut root = cursor.captures(&self.queries.query, tree.root_node(), text);
        while let Some((start, end, key)) = next_capture(&mut root, &self.queries.capture_keys) {
            captures.push((start, end, key, 0));
        }
        if !self.injections.is_empty() {
            captures.extend(self.injections.captures(rope, range.clone()));
            // Stable, so each layer keeps its own capture order; at one start the host goes first and the
            // embedded language lands on top of it.
            captures.sort_by_key(|(start, _, _, depth)| (*start, *depth));
        }
        let mut stack: Vec<LayerCapture> = Vec::new();
        let mut pending = captures.into_iter().peekable();
        let mut pos = range.start;
        while pos < range.end {
            while stack.last().is_some_and(|(_, end, _, _)| *end <= pos) {
                stack.pop();
            }
            while let Some(capture) = pending.next_if(|(start, _, _, _)| *start <= pos) {
                let (start, end, _, depth) = capture;
                // Several patterns can capture the same node; the bundled queries list the most specific one
                // first, so the first capture for a node keeps it.
                let same_node = stack
                    .last()
                    .is_some_and(|(s, e, _, d)| *s == start && *e == end && *d == depth);
                if end > pos && !same_node {
                    stack.push(capture);
                }
            }
            let next_start = pending.peek().map_or(usize::MAX, |(start, _, _, _)| *start);
            let mut end = range.end.min(next_start);
            if let Some((_, top_end, _, _)) = stack.last() {
                end = end.min(*top_end);
            }
            let end = end.max(pos + 1).min(range.end);
            let capture = stack.last().map(|(_, _, key, _)| *key);
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

    #[cfg(test)]
    pub(crate) fn injected_langs(&self) -> Vec<(Lang, usize)> {
        self.injections.layer_langs()
    }
}

fn override_patterns(lang: Lang) -> Option<&'static str> {
    Some(match lang {
        Lang::Rust => include_str!("../queries/rust/overrides.scm"),
        Lang::JavaScript => include_str!("../queries/javascript/overrides.scm"),
        Lang::TypeScript => include_str!("../queries/typescript/overrides.scm"),
        Lang::Tsx => include_str!("../queries/tsx/overrides.scm"),
        Lang::Go => include_str!("../queries/go/overrides.scm"),
        Lang::Python => include_str!("../queries/python/overrides.scm"),
        Lang::C => include_str!("../queries/c/overrides.scm"),
        Lang::Cpp => include_str!("../queries/cpp/overrides.scm"),
        Lang::Bash => include_str!("../queries/bash/overrides.scm"),
        Lang::Css => include_str!("../queries/css/overrides.scm"),
        Lang::Json => include_str!("../queries/json/overrides.scm"),
        Lang::Yaml => include_str!("../queries/yaml/overrides.scm"),
        Lang::Java => include_str!("../queries/java/overrides.scm"),
        Lang::Ruby => include_str!("../queries/ruby/overrides.scm"),
        Lang::Lua => include_str!("../queries/lua/overrides.scm"),
        Lang::Html => include_str!("../queries/html/overrides.scm"),
        _ => return None,
    })
}

/// Named syntax regions (`@string`, `@comment`, ...) that change editing behaviour inside them.
struct OverrideQuery {
    query: Query,
    /// Per capture index: the region name and whether the node's own edges count as inside.
    captures: Vec<(String, bool)>,
}

impl OverrideQuery {
    fn new(lang: Lang, language: &Language) -> Option<Self> {
        let query = Query::new(language, override_patterns(lang)?).ok()?;
        let captures = query
            .capture_names()
            .iter()
            .map(|name| match name.strip_suffix(".inclusive") {
                Some(base) => (base.to_string(), true),
                None => (name.to_string(), false),
            })
            .collect();
        Some(Self { query, captures })
    }

    fn name_at(&self, rope: &Rope, tree: &Tree, byte: usize) -> Option<&str> {
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(byte.saturating_sub(1)..byte.saturating_add(1));
        let text = |node: Node| rope_chunks(rope, node.byte_range());
        let mut matches = cursor.matches(&self.query, tree.root_node(), text);
        let mut smallest: Option<(u32, Range<usize>)> = None;
        while let Some(found) = matches.next() {
            for capture in found.captures() {
                let Some((_, inclusive)) = self.captures.get(capture.index as usize) else {
                    continue;
                };
                let range = capture.node.byte_range();
                let inside = if *inclusive {
                    range.start <= byte && byte <= range.end
                } else {
                    range.start < byte && byte < range.end
                };
                if inside && smallest.as_ref().is_none_or(|(_, s)| range.len() < s.len()) {
                    smallest = Some((capture.index, range));
                }
            }
        }
        let (index, _) = smallest?;
        self.captures
            .get(index as usize)
            .map(|(name, _)| name.as_str())
    }
}

/// Delimiter pairs every grammar may have; patterns naming tokens a grammar lacks are dropped.
const BRACKET_PATTERNS: [&str; 6] = [
    "(\"(\" @open \")\" @close)",
    "(\"[\" @open \"]\" @close)",
    "(\"{\" @open \"}\" @close)",
    "(\"<\" @open \">\" @close)",
    "(\"\\\"\" @open \"\\\"\" @close)",
    "(\"'\" @open \"'\" @close)",
];

struct BracketQuery {
    query: Query,
    open_ix: Option<u32>,
    close_ix: Option<u32>,
}

impl BracketQuery {
    fn new(language: &Language) -> Option<Self> {
        let source: String = BRACKET_PATTERNS
            .iter()
            .filter(|pattern| Query::new(language, pattern).is_ok())
            .map(|pattern| format!("{pattern}\n"))
            .collect();
        if source.is_empty() {
            return None;
        }
        let query = Query::new(language, &source).ok()?;
        let index_of = |name: &str| {
            query
                .capture_names()
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };
        Some(Self {
            open_ix: index_of("open"),
            close_ix: index_of("close"),
            query,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxNode {
    pub range: Range<usize>,
    pub kind: String,
    pub named: bool,
}

/// Move `cursor` to the smallest node enclosing `range` that is strictly larger than it.
fn goto_node_enclosing_range(cursor: &mut tree_sitter::TreeCursor, range: &Range<usize>) -> bool {
    let mut ascending = false;
    loop {
        let mut node_range = cursor.node().byte_range();
        if range.is_empty() {
            if node_range.start > range.start {
                cursor.goto_previous_sibling();
                node_range = cursor.node().byte_range();
            }
        } else if node_range.end == range.start {
            cursor.goto_next_sibling();
            node_range = cursor.node().byte_range();
        }
        let encloses = node_range.start <= range.start
            && range.end <= node_range.end
            && node_range.len() > range.len();
        if !encloses {
            ascending = true;
            if !cursor.goto_parent() {
                return false;
            }
            continue;
        } else if ascending {
            return true;
        }
        if cursor.goto_first_child_for_byte(range.start).is_none() {
            return true;
        }
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
pub(crate) fn recognized_key(name: &str) -> Option<&'static str> {
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

    fn settled(lang: Lang, text: &str) -> (EditorBuffer, Syntax) {
        let buffer = EditorBuffer::from_text(text);
        let mut syntax = Syntax::new(lang).unwrap();
        syntax.sync(&buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&buffer);
        }
        (buffer, syntax)
    }

    fn innermost(lang: Lang, text: &str, at: usize) -> Option<(String, String)> {
        let (buffer, syntax) = settled(lang, text);
        let (open, close) = syntax.innermost_enclosing_bracket_ranges(&buffer.rope, at..at)?;
        Some((open.start.to_string(), close.start.to_string()))
    }

    #[test]
    fn finds_the_innermost_pair_around_or_touching_the_caret() {
        let text = "fn a() { b(1, [2]); }";
        let pair = |open: usize, close: usize| Some((open.to_string(), close.to_string()));
        assert_eq!(innermost(Lang::Rust, text, 12), pair(10, 17));
        assert_eq!(innermost(Lang::Rust, text, 15), pair(14, 16));
        assert_eq!(innermost(Lang::Rust, text, 10), pair(10, 17));
        assert_eq!(innermost(Lang::Rust, text, 18), pair(10, 17));
        assert_eq!(innermost(Lang::Rust, text, 8), pair(7, 20));
    }

    #[test]
    fn finds_pairs_far_from_the_caret() {
        let body: String = (0..500).map(|i| format!("    let x{i} = {i};\n")).collect();
        let text = format!("fn a() {{\n{body}}}\n");
        let middle = text.len() / 2;
        let close = text.rfind('}').unwrap();
        assert_eq!(
            innermost(Lang::Rust, &text, middle),
            Some((7.to_string(), close.to_string()))
        );
    }

    #[test]
    fn quotes_pair_and_missing_closers_are_ignored() {
        assert_eq!(
            innermost(Lang::Rust, "let s = \"ab\";", 10),
            Some((8.to_string(), 11.to_string()))
        );
        assert_eq!(innermost(Lang::Rust, "fn a() { b(", 11), None);
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

#[cfg(test)]
mod enclosing_bracket_tests {
    use super::*;
    use std::time::Duration;

    fn jump(text: &str, at: usize) -> usize {
        let mut buffer = EditorBuffer::from_text(text);
        let mut syntax = Syntax::new(Lang::Rust).unwrap();
        syntax.sync(&buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&buffer);
        }
        buffer.place_cursor(at);
        let rope = buffer.rope.clone();
        let enclosing = |range: Range<usize>| syntax.enclosing_bracket_ranges(&rope, range);
        let next = buffer.enclosing_bracket_selections(&enclosing);
        buffer.set_selections(next);
        buffer.cursor()
    }

    #[test]
    fn jumps_between_a_pair_and_from_inside_to_the_closer() {
        let text = "fn a() { b(1, 2); }";
        assert_eq!(jump(text, 10), 16);
        assert_eq!(jump(text, 16), 10);
        assert_eq!(jump(text, 12), 15);
        assert_eq!(jump(text, 7), 19);
    }
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::language::Scope;
    use std::time::Duration;

    fn scope(lang: Lang, text: &str) -> Scope {
        let at = text.find('|').unwrap();
        let buffer = EditorBuffer::from_text(&text.replace('|', ""));
        let mut syntax = Syntax::new(lang).unwrap();
        syntax.sync(&buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&buffer);
        }
        syntax.scope_at(&buffer.rope, at)
    }

    const STRING: Scope = Scope {
        in_string: true,
        in_comment: false,
    };
    const COMMENT: Scope = Scope {
        in_string: false,
        in_comment: true,
    };
    const CODE: Scope = Scope {
        in_string: false,
        in_comment: false,
    };

    #[test]
    fn override_queries_compile() {
        for lang in [
            Lang::Rust,
            Lang::JavaScript,
            Lang::TypeScript,
            Lang::Tsx,
            Lang::Go,
            Lang::Python,
            Lang::C,
            Lang::Cpp,
            Lang::Bash,
            Lang::Css,
            Lang::Json,
            Lang::Yaml,
            Lang::Java,
            Lang::Ruby,
            Lang::Lua,
            Lang::Html,
        ] {
            let (language, _) = grammar(lang).unwrap();
            let source = override_patterns(lang).unwrap();
            if let Err(error) = Query::new(&language, source) {
                panic!("{source}: {error}");
            }
        }
    }

    #[test]
    fn strings_exclude_their_edges_and_comments_include_them() {
        assert_eq!(scope(Lang::Rust, "let s = \"a|b\";"), STRING);
        assert_eq!(scope(Lang::Rust, "let s = |\"ab\";"), CODE);
        assert_eq!(scope(Lang::Rust, "let s = \"ab\"|;"), CODE);
        assert_eq!(scope(Lang::Rust, "x; // note|\ny;"), COMMENT);
        assert_eq!(scope(Lang::Rust, "x; |// note\ny;"), COMMENT);
    }

    fn typed_quote(text: &str) -> String {
        let at = text.find('|').unwrap();
        let mut buffer = EditorBuffer::from_text(&text.replace('|', ""));
        let mut syntax = Syntax::new(Lang::JavaScript).unwrap();
        syntax.sync(&buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&buffer);
        }
        buffer.place_cursor(at);
        let rope = buffer.rope.clone();
        let scope_at = |byte| syntax.scope_at(&rope, byte);
        buffer.handle_input("'", &crate::language::config(Lang::JavaScript), &scope_at);
        buffer.text()
    }

    #[test]
    fn quotes_do_not_autoclose_inside_comments() {
        assert_eq!(typed_quote("x; // it|\n"), "x; // it'\n");
        assert_eq!(typed_quote("x = |;\n"), "x = '';\n");
    }

    #[test]
    fn template_interpolations_are_code() {
        assert_eq!(scope(Lang::JavaScript, "let s = `a|b${c}`;"), STRING);
        assert_eq!(scope(Lang::JavaScript, "let s = `a${b|}`;"), CODE);
        assert_eq!(scope(Lang::Python, "x = 1  # hi|"), COMMENT);
    }
}

#[cfg(test)]
mod new_language_tests {
    use super::*;
    use std::time::Duration;

    fn capture_of(lang: Lang, text: &str, word: &str) -> Option<&'static str> {
        let buffer = EditorBuffer::from_text(text);
        let mut syntax = Syntax::new(lang).unwrap();
        syntax.sync(&buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(&buffer);
        }
        let start = text.find(word).unwrap();
        syntax
            .highlight(&buffer.rope, 0..buffer.rope.len_bytes())
            .into_iter()
            .find(|run| run.range.start <= start && start < run.range.end)
            .and_then(|run| run.capture)
    }

    #[test]
    fn new_languages_color_their_keywords() {
        assert_eq!(
            capture_of(Lang::Kotlin, "fun main() {}\n", "fun"),
            Some("keyword")
        );
        assert_eq!(
            capture_of(Lang::Kotlin, "fun main() {}\n", "main"),
            Some("function")
        );
        assert_eq!(
            capture_of(
                Lang::Hcl,
                "resource \"x\" \"y\" {\n  a = 1\n}\n",
                "resource"
            ),
            Some("type")
        );
        assert_eq!(
            capture_of(Lang::GraphQl, "type Query { a: Int }\n", "type"),
            Some("keyword")
        );
        assert_eq!(
            capture_of(Lang::Sql, "SELECT a FROM t;\n", "SELECT"),
            Some("keyword")
        );
        assert_eq!(
            capture_of(Lang::Dockerfile, "FROM alpine\n", "FROM"),
            Some("keyword")
        );
        assert_eq!(
            capture_of(
                Lang::Proto,
                "syntax = \"proto3\";\nmessage A {}\n",
                "message"
            ),
            Some("keyword")
        );
    }
}
