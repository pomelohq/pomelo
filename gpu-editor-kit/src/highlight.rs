//! Syntax highlighting via tree-sitter + each grammar's `highlights.scm` query — the same approach as Zed. A capture
//! name (e.g. `keyword`, `string`) maps to a theme color; the renderer only consumes the resulting `Span`s.

use std::collections::HashMap;

use glyphon::Color;
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

pub struct Span {
    pub text: String,
    pub color: Color,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Go,
    Python,
    Json,
    PlainText,
}

impl Lang {
    pub fn from_ext(ext: &str) -> Lang {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Lang::Rust,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
            "go" => Lang::Go,
            "py" | "pyi" => Lang::Python,
            "json" => Lang::Json,
            _ => Lang::PlainText,
        }
    }
}

// Capture names we recognize; tree-sitter maps each query capture to an index into this list. Ported from Zed's One
// Dark syntax keys (assets/themes/one/one.json) so grammar captures resolve to the same styles Zed uses.
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute", "boolean", "comment", "comment.doc", "constant", "constant.builtin", "constructor", "embedded",
    "enum", "function", "function.method", "keyword", "label", "namespace", "number", "operator", "predictive",
    "preproc", "primary", "property", "punctuation", "punctuation.bracket", "punctuation.delimiter",
    "punctuation.list_marker", "punctuation.special", "string", "string.escape", "string.regex", "string.special",
    "string.special.symbol", "tag", "text.literal", "title", "type", "type.builtin", "variable",
    "variable.parameter", "variable.special", "variant",
];

// Foreground (#acb2be) — Zed One Dark editor.foreground.
fn plain() -> Color { Color::rgb(172, 178, 190) }

// Exact One Dark syntax colors (Zed). Dotted names fall back to their base (function.method -> function), matching
// Zed's HighlightMap longest-prefix resolution.
fn exact(name: &str) -> Option<Color> {
    Some(match name {
        "keyword" | "preproc" => Color::rgb(180, 119, 207),
        "function" | "constructor" => Color::rgb(115, 173, 233),
        "type" | "operator" | "enum" | "link_uri" => Color::rgb(110, 180, 191),
        "string" | "string.regex" => Color::rgb(161, 193, 129),
        "comment" => Color::rgb(93, 99, 111),
        "number" | "boolean" | "variable.special" | "string.special.symbol" | "string.escape" | "string.special" => {
            Color::rgb(191, 149, 106)
        }
        "constant" => Color::rgb(223, 193, 132),
        "property" | "title" => Color::rgb(208, 114, 119),
        "tag" | "attribute" | "label" | "emphasis" => Color::rgb(116, 173, 232),
        "variable" | "punctuation" | "embedded" => Color::rgb(172, 178, 190),
        "punctuation.bracket" | "punctuation.delimiter" => Color::rgb(178, 185, 198),
        _ => return None,
    })
}

fn color_for_name(name: &str) -> Color {
    let mut n = name;
    loop {
        if let Some(c) = exact(n) {
            return c;
        }
        match n.rfind('.') {
            Some(i) => n = &n[..i],
            None => return plain(),
        }
    }
}

pub struct Highlighter {
    inner: TsHighlighter,
    configs: HashMap<Lang, Option<HighlightConfiguration>>,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self { inner: TsHighlighter::new(), configs: HashMap::new() }
    }
}

impl Highlighter {
    fn build(lang: Lang) -> Option<HighlightConfiguration> {
        let (language, highlights, injections, locals): (tree_sitter::Language, &str, &str, &str) = match lang {
            Lang::Rust => (tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY, "", ""),
            Lang::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                "",
                tree_sitter_typescript::LOCALS_QUERY,
            ),
            Lang::Tsx => (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                "",
                tree_sitter_typescript::LOCALS_QUERY,
            ),
            Lang::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            Lang::Go => (tree_sitter_go::LANGUAGE.into(), tree_sitter_go::HIGHLIGHTS_QUERY, "", ""),
            Lang::Python => (tree_sitter_python::LANGUAGE.into(), tree_sitter_python::HIGHLIGHTS_QUERY, "", ""),
            Lang::Json => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY, "", ""),
            Lang::PlainText => return None,
        };
        let mut config = HighlightConfiguration::new(language, "src", highlights, injections, locals).ok()?;
        config.configure(HIGHLIGHT_NAMES);
        Some(config)
    }

    pub fn highlight(&mut self, text: &str, lang: Lang) -> Vec<Span> {
        self.configs.entry(lang).or_insert_with(|| Self::build(lang));
        let Highlighter { inner, configs } = self;
        let Some(Some(config)) = configs.get(&lang) else {
            return vec![Span { text: text.to_string(), color: plain() }];
        };

        let mut spans = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let events = match inner.highlight(config, text.as_bytes(), None, None, |_| None) {
            Ok(e) => e,
            Err(_) => return vec![Span { text: text.to_string(), color: plain() }],
        };
        for event in events.flatten() {
            match event {
                HighlightEvent::HighlightStart(Highlight(i)) => stack.push(i),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let color = stack
                        .last()
                        .map(|&i| color_for_name(HIGHLIGHT_NAMES[i]))
                        .unwrap_or_else(plain);
                    spans.push(Span { text: text[start..end].to_string(), color });
                }
            }
        }
        spans
    }
}
