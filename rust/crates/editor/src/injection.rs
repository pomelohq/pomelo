//! Languages embedded in a file (code blocks in Markdown, `<script>` in HTML, regex literals in JS...).
//!
//! An injection query marks `@injection.content` nodes and names their language, either fixed by
//! `#set! injection.language` or read from an `@injection.language` node. Each embedded region becomes a layer:
//! its own tree, parsed over just that region's byte ranges. `injection.combined` patterns gather all their
//! regions of one language into a single layer. Layers nest (an HTML block in Markdown can hold a script) and
//! highlight on top of the layer that hosts them.
//!
//! Edits are fed to every layer's tree; a layer no edit touched is reused as-is, an edited one reparses from its
//! old tree, and only new regions parse from scratch.

use std::ops::Range;

use ropey::Rope;
use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree,
};

use crate::highlight::{grammar, injection_patterns, Lang};

/// Deepest nesting followed; deeper regions stay part of their host.
const MAX_DEPTH: usize = 3;

/// An embedded region: its language name and content byte ranges.
type Region = (String, Vec<Range<usize>>);

struct InjectionPattern {
    language: Option<String>,
    combined: bool,
}

pub(crate) struct InjectionQuery {
    query: Query,
    content_ix: u32,
    language_ix: Option<u32>,
    patterns: Vec<InjectionPattern>,
}

impl InjectionQuery {
    pub(crate) fn new(lang: Lang, language: &Language) -> Option<Self> {
        let query = Query::new(language, injection_patterns(lang)?).ok()?;
        let index_of = |name: &str| {
            query
                .capture_names()
                .iter()
                .position(|n| *n == name)
                .map(|i| i as u32)
        };
        let content_ix = index_of("injection.content")?;
        let language_ix = index_of("injection.language");
        let patterns = (0..query.pattern_count())
            .map(|pattern| {
                let mut info = InjectionPattern {
                    language: None,
                    combined: false,
                };
                for property in query.property_settings(pattern) {
                    match property.key.as_ref() {
                        "injection.language" => {
                            info.language = property.value.as_deref().map(str::to_string)
                        }
                        "injection.combined" => info.combined = true,
                        _ => {}
                    }
                }
                info
            })
            .collect();
        Some(Self {
            query,
            content_ix,
            language_ix,
            patterns,
        })
    }

    fn has_combined(&self) -> bool {
        self.patterns.iter().any(|pattern| pattern.combined)
    }

    /// Each embedded region under `tree` (or only those meeting `within`): its language name and content byte
    /// ranges.
    fn regions(&self, rope: &Rope, tree: &Tree, within: Option<&[Range<usize>]>) -> Vec<Region> {
        let text = |node: Node| rope_bytes(rope, node.byte_range());
        let everything = 0..usize::MAX;
        let query_ranges = within.unwrap_or(std::slice::from_ref(&everything));
        let mut single: Vec<Region> = Vec::new();
        let mut combined: Vec<Region> = Vec::new();
        for query_range in query_ranges {
            let mut cursor = QueryCursor::new();
            cursor.set_byte_range(
                query_range.start.saturating_sub(1)..query_range.end.saturating_add(1),
            );
            let mut matches = cursor.matches(&self.query, tree.root_node(), text);
            while let Some(found) = matches.next() {
                let content: Vec<Range<usize>> = found
                    .nodes_for_capture_index(self.content_ix)
                    .map(|node| node.byte_range())
                    .filter(|range| !range.is_empty())
                    .collect();
                if content.is_empty() {
                    continue;
                }
                let Some(pattern) = self.patterns.get(found.pattern_index) else {
                    continue;
                };
                let name = pattern.language.clone().or_else(|| {
                    let node = found.nodes_for_capture_index(self.language_ix?).next()?;
                    let name = rope.byte_slice(node.byte_range()).to_string();
                    // A path such as `src/main.rs` names its language by extension.
                    Some(match name.rfind('.') {
                        Some(dot) => name[dot + 1..].to_string(),
                        None => name,
                    })
                });
                let Some(name) = name else {
                    continue;
                };
                if pattern.combined {
                    match combined.iter_mut().find(|(n, _)| *n == name) {
                        Some((_, ranges)) => ranges.extend(content),
                        None => combined.push((name, content)),
                    }
                } else if !single.iter().any(|(n, r)| *n == name && *r == content) {
                    // Neighbouring query ranges can meet the same region.
                    single.push((name, content));
                }
            }
        }
        for (_, ranges) in &mut combined {
            ranges.sort_by_key(|range| (range.start, range.end));
            ranges.dedup();
        }
        single.extend(combined);
        single
    }
}

pub(crate) struct LayerGrammar {
    language: Language,
    highlights: Query,
    capture_keys: Vec<Option<&'static str>>,
    injections: Option<InjectionQuery>,
}

impl LayerGrammar {
    fn load(lang: Lang) -> Option<Self> {
        let (language, source) = grammar(lang)?;
        let highlights = Query::new(&language, source).ok()?;
        let capture_keys = highlights
            .capture_names()
            .iter()
            .map(|name| crate::syntax::recognized_key(name))
            .collect();
        Some(Self {
            injections: InjectionQuery::new(lang, &language),
            language,
            highlights,
            capture_keys,
        })
    }
}

struct Layer {
    grammar: usize,
    depth: usize,
    ranges: Vec<Range<usize>>,
    tree: Tree,
    /// An edit touched it since it was parsed.
    edited: bool,
}

/// One highlight capture: byte range, theme key, and the layer depth it came from (0 = the file's own).
pub(crate) type LayerCapture = (usize, usize, &'static str, usize);

#[derive(Default)]
pub(crate) struct Injections {
    grammars: Vec<Option<LayerGrammar>>,
    grammar_langs: Vec<Lang>,
    layers: Vec<Layer>,
}

impl Injections {
    pub(crate) fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    fn grammar_index(&mut self, lang: Lang) -> Option<usize> {
        let index = match self.grammar_langs.iter().position(|l| *l == lang) {
            Some(index) => index,
            None => {
                self.grammar_langs.push(lang);
                self.grammars.push(LayerGrammar::load(lang));
                self.grammars.len() - 1
            }
        };
        self.grammars.get(index)?.as_ref().map(|_| index)
    }

    /// Keep every layer's positions in step with an edit.
    pub(crate) fn edit(&mut self, edit: &InputEdit) {
        let delta = edit.new_end_byte as isize - edit.old_end_byte as isize;
        let shift = |byte: usize| (byte as isize + delta).max(0) as usize;
        for layer in &mut self.layers {
            layer.tree.edit(edit);
            for range in &mut layer.ranges {
                if range.end < edit.start_byte {
                    continue;
                }
                if range.start > edit.old_end_byte {
                    *range = shift(range.start)..shift(range.end);
                    continue;
                }
                layer.edited = true;
                let end = if range.end >= edit.old_end_byte {
                    shift(range.end)
                } else {
                    edit.new_end_byte
                };
                range.end = end.max(range.start);
            }
        }
    }

    /// Rebuild the layers for the file's freshly parsed `root` tree. With `changed` (byte ranges whose text or
    /// structure changed since the last rebuild) only layers meeting a change are redone; others stay as they
    /// are. `None` rebuilds everything.
    pub(crate) fn update(
        &mut self,
        rope: &Rope,
        root: &Tree,
        root_injections: &InjectionQuery,
        changed: Option<&[Range<usize>]>,
    ) {
        // A combined pattern's layer spans the whole file, so any change can reshape it.
        let changed = changed.filter(|_| !root_injections.has_combined());
        let mut layers = std::mem::take(&mut self.layers);
        let mut stale = Vec::new();
        let mut query_ranges: Vec<Range<usize>> = Vec::new();
        match changed {
            Some(changed) => {
                layers.sort_by_key(|layer| layer.depth);
                query_ranges.extend(changed.iter().cloned());
                let mut stale_spans: Vec<Range<usize>> = Vec::new();
                for layer in layers {
                    let touched = changed.iter().any(|c| {
                        layer
                            .ranges
                            .iter()
                            .any(|r| r.start <= c.end && c.start <= r.end)
                    });
                    let inside_stale = stale_spans.iter().any(|s| {
                        layer
                            .ranges
                            .iter()
                            .any(|r| r.start < s.end && s.start < r.end)
                    });
                    if layer.edited || touched || inside_stale {
                        let span = layer_span(&layer.ranges);
                        if layer.depth == 1 {
                            query_ranges.push(span.clone());
                        }
                        stale_spans.push(span);
                        stale.push(layer);
                    } else {
                        self.layers.push(layer);
                    }
                }
            }
            None => stale = layers,
        }
        let within = changed.map(|_| query_ranges.as_slice());
        let mut pending: Vec<(usize, Vec<Region>)> =
            vec![(1, root_injections.regions(rope, root, within))];
        while let Some((depth, regions)) = pending.pop() {
            for (name, ranges) in regions {
                let Some(lang) = Lang::for_injection(&name) else {
                    continue;
                };
                let Some(grammar) = self.grammar_index(lang) else {
                    continue;
                };
                let already_kept = self
                    .layers
                    .iter()
                    .any(|layer| layer.grammar == grammar && layer.ranges == ranges);
                if already_kept {
                    continue;
                }
                let Some(tree) = self.parse_layer(rope, grammar, &ranges, &mut stale) else {
                    continue;
                };
                if depth < MAX_DEPTH {
                    if let Some(nested) = self
                        .grammars
                        .get(grammar)
                        .and_then(Option::as_ref)
                        .and_then(|g| g.injections.as_ref())
                    {
                        pending.push((depth + 1, nested.regions(rope, &tree, None)));
                    }
                }
                self.layers.push(Layer {
                    grammar,
                    depth,
                    ranges,
                    tree,
                    edited: false,
                });
            }
        }
    }

    fn parse_layer(
        &self,
        rope: &Rope,
        grammar: usize,
        ranges: &[Range<usize>],
        previous: &mut Vec<Layer>,
    ) -> Option<Tree> {
        if let Some(index) = previous
            .iter()
            .position(|old| old.grammar == grammar && !old.edited && old.ranges == ranges)
        {
            return Some(previous.swap_remove(index).tree);
        }
        let overlaps = |old: &Layer| {
            old.grammar == grammar
                && old
                    .ranges
                    .iter()
                    .any(|a| ranges.iter().any(|b| a.start < b.end && b.start < a.end))
        };
        let old_tree = previous
            .iter()
            .position(overlaps)
            .map(|index| previous.swap_remove(index).tree);
        let layer_grammar = self.grammars.get(grammar)?.as_ref()?;
        let mut parser = Parser::new();
        parser.set_language(&layer_grammar.language).ok()?;
        let included: Vec<tree_sitter::Range> =
            ranges.iter().map(|range| ts_range(rope, range)).collect();
        parser.set_included_ranges(&included).ok()?;
        parser.parse_with_options(
            &mut |byte, _| chunk_from(rope, byte),
            old_tree.as_ref(),
            None,
        )
    }

    /// Highlight captures from every layer overlapping `range`.
    pub(crate) fn captures(&self, rope: &Rope, range: Range<usize>) -> Vec<LayerCapture> {
        let mut out = Vec::new();
        let text = |node: Node| rope_bytes(rope, node.byte_range());
        for layer in &self.layers {
            if !layer
                .ranges
                .iter()
                .any(|r| r.start < range.end && range.start < r.end)
            {
                continue;
            }
            let Some(grammar) = self.grammars.get(layer.grammar).and_then(Option::as_ref) else {
                continue;
            };
            let mut cursor = QueryCursor::new();
            cursor.set_byte_range(range.clone());
            let mut captures = cursor.captures(&grammar.highlights, layer.tree.root_node(), text);
            while let Some((found, index)) = captures.next() {
                let Some(capture) = found.captures().get(*index) else {
                    continue;
                };
                if let Some(Some(key)) = grammar.capture_keys.get(capture.index as usize) {
                    out.push((
                        capture.node.start_byte(),
                        capture.node.end_byte(),
                        *key,
                        layer.depth,
                    ));
                }
            }
        }
        out
    }

    /// The languages layered into the file, outermost first (for tests and diagnostics).
    #[cfg(test)]
    pub(crate) fn layer_langs(&self) -> Vec<(Lang, usize)> {
        let mut langs: Vec<(Lang, usize)> = self
            .layers
            .iter()
            .filter_map(|layer| Some((*self.grammar_langs.get(layer.grammar)?, layer.depth)))
            .collect();
        langs.sort_by_key(|(_, depth)| *depth);
        langs
    }
}

fn layer_span(ranges: &[Range<usize>]) -> Range<usize> {
    let start = ranges.first().map_or(0, |r| r.start);
    let end = ranges.last().map_or(start, |r| r.end);
    start..end
}

fn point_at(rope: &Rope, byte: usize) -> Point {
    let byte = byte.min(rope.len_bytes());
    let row = rope.byte_to_line(byte);
    Point::new(row, byte - rope.line_to_byte(row))
}

fn ts_range(rope: &Rope, range: &Range<usize>) -> tree_sitter::Range {
    tree_sitter::Range {
        start_byte: range.start,
        end_byte: range.end,
        start_point: point_at(rope, range.start),
        end_point: point_at(rope, range.end),
    }
}

fn chunk_from(rope: &Rope, byte: usize) -> &[u8] {
    if byte >= rope.len_bytes() {
        return &[];
    }
    let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
    &chunk.as_bytes()[byte - chunk_start..]
}

fn rope_bytes(rope: &Rope, range: Range<usize>) -> impl Iterator<Item = &[u8]> {
    let end = range.end.min(rope.len_bytes());
    let start = range.start.min(end);
    rope.byte_slice(start..end).chunks().map(str::as_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::EditorBuffer;
    use crate::syntax::Syntax;
    use std::time::Duration;

    fn settle(syntax: &mut Syntax, buffer: &EditorBuffer) {
        syntax.sync(buffer);
        while syntax.is_parsing() {
            std::thread::sleep(Duration::from_millis(1));
            syntax.sync(buffer);
        }
    }

    fn syntax_for(lang: Lang, text: &str) -> (EditorBuffer, Syntax) {
        let buffer = EditorBuffer::from_text(text);
        let mut syntax = Syntax::new(lang).unwrap();
        settle(&mut syntax, &buffer);
        (buffer, syntax)
    }

    fn capture_of(syntax: &Syntax, buffer: &EditorBuffer, word: &str) -> Option<&'static str> {
        let text = buffer.text();
        let start = text.find(word).unwrap();
        syntax
            .highlight(&buffer.rope, 0..buffer.rope.len_bytes())
            .into_iter()
            .find(|run| run.range.start <= start && start < run.range.end)
            .and_then(|run| run.capture)
    }

    #[test]
    fn injection_queries_compile() {
        for lang in [
            Lang::Markdown,
            Lang::MarkdownInline,
            Lang::Html,
            Lang::JavaScript,
            Lang::TypeScript,
            Lang::Tsx,
            Lang::Rust,
            Lang::Php,
            Lang::Elixir,
            Lang::Nix,
            Lang::Swift,
            Lang::Zig,
            Lang::Lua,
            Lang::Haskell,
        ] {
            let (language, _) = grammar(lang).unwrap();
            let source = injection_patterns(lang).unwrap();
            if let Err(error) = Query::new(&language, source) {
                panic!("{}: {error}", source.lines().next().unwrap_or(""));
            }
        }
    }

    #[test]
    fn markdown_code_blocks_highlight_as_their_language() {
        let (buffer, syntax) = syntax_for(
            Lang::Markdown,
            "# Title\n\n```rust\nfn main() {}\n```\n\nSome *text*.\n",
        );
        let langs = syntax.injected_langs();
        assert!(langs.contains(&(Lang::Rust, 1)), "{langs:?}");
        assert!(langs.contains(&(Lang::MarkdownInline, 1)), "{langs:?}");
        assert_eq!(capture_of(&syntax, &buffer, "fn "), Some("keyword"));
        assert_eq!(capture_of(&syntax, &buffer, "main"), Some("function"));
    }

    #[test]
    fn html_scripts_and_styles_are_embedded() {
        let (buffer, syntax) = syntax_for(
            Lang::Html,
            "<style>p { color: red; }</style>\n<script>let x = /a+/;</script>\n",
        );
        let langs = syntax.injected_langs();
        assert!(langs.contains(&(Lang::Css, 1)), "{langs:?}");
        assert!(langs.contains(&(Lang::JavaScript, 1)), "{langs:?}");
        assert!(langs.contains(&(Lang::Regex, 2)), "{langs:?}");
        assert_eq!(capture_of(&syntax, &buffer, "let"), Some("keyword"));
    }

    #[test]
    fn edits_inside_a_block_rehighlight_it() {
        let mut buffer = EditorBuffer::from_text("```rust\nfn a() {}\n```\n");
        let mut syntax = Syntax::new(Lang::Markdown).unwrap();
        settle(&mut syntax, &buffer);
        buffer.place_cursor(8);
        buffer.insert_text("pub ");
        settle(&mut syntax, &buffer);
        assert_eq!(capture_of(&syntax, &buffer, "pub"), Some("keyword"));
        assert_eq!(capture_of(&syntax, &buffer, "fn "), Some("keyword"));
        buffer.place_cursor(buffer.rope.len_chars());
        buffer.insert_text("\n```python\ndef b(): pass\n```\n");
        settle(&mut syntax, &buffer);
        assert!(syntax.injected_langs().contains(&(Lang::Python, 1)));
        assert_eq!(capture_of(&syntax, &buffer, "def"), Some("keyword"));
    }
}
