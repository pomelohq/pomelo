//! Syntax highlighting via tree-sitter + each grammar's `highlights.scm` query. Each span
//! carries a capture name (e.g. `keyword`, `string`); the active `Theme` maps that name to a color. Colors live in
//! the theme, not here, so themes are user-configurable.

use std::collections::HashMap;

use tree_sitter_highlight::{
    Highlight, HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter,
};

pub struct Span {
    pub text: String,
    /// Tree-sitter capture name (one of HIGHLIGHT_NAMES), or "" for un-captured/plain text.
    pub capture: &'static str,
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

// Capture names we recognize; tree-sitter maps each query capture to an index into this list. Based on the One
// Dark syntax keys so grammar captures resolve to consistent styles.
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment.doc",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "enum",
    "function",
    "function.method",
    "keyword",
    "label",
    "namespace",
    "number",
    "operator",
    "predictive",
    "preproc",
    "primary",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.list_marker",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regex",
    "string.special",
    "string.special.symbol",
    "tag",
    "text.literal",
    "title",
    "type",
    "type.builtin",
    "variable",
    "variable.parameter",
    "variable.special",
    "variant",
];

pub struct Highlighter {
    inner: TsHighlighter,
    configs: HashMap<Lang, Option<HighlightConfiguration>>,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self {
            inner: TsHighlighter::new(),
            configs: HashMap::new(),
        }
    }
}

impl Highlighter {
    fn build(lang: Lang) -> Option<HighlightConfiguration> {
        let (language, highlights, injections, locals): (tree_sitter::Language, &str, &str, &str) =
            match lang {
                Lang::Rust => (
                    tree_sitter_rust::LANGUAGE.into(),
                    tree_sitter_rust::HIGHLIGHTS_QUERY,
                    "",
                    "",
                ),
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
                Lang::Go => (
                    tree_sitter_go::LANGUAGE.into(),
                    tree_sitter_go::HIGHLIGHTS_QUERY,
                    "",
                    "",
                ),
                Lang::Python => (
                    tree_sitter_python::LANGUAGE.into(),
                    tree_sitter_python::HIGHLIGHTS_QUERY,
                    "",
                    "",
                ),
                Lang::Json => (
                    tree_sitter_json::LANGUAGE.into(),
                    tree_sitter_json::HIGHLIGHTS_QUERY,
                    "",
                    "",
                ),
                Lang::PlainText => return None,
            };
        let mut config =
            HighlightConfiguration::new(language, "src", highlights, injections, locals).ok()?;
        config.configure(HIGHLIGHT_NAMES);
        Some(config)
    }

    pub fn highlight(&mut self, text: &str, lang: Lang) -> Vec<Span> {
        self.configs
            .entry(lang)
            .or_insert_with(|| Self::build(lang));
        let Highlighter { inner, configs } = self;
        let Some(Some(config)) = configs.get(&lang) else {
            return vec![Span {
                text: text.to_string(),
                capture: "",
            }];
        };

        let mut spans = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let events = match inner.highlight(config, text.as_bytes(), None, None, |_| None) {
            Ok(e) => e,
            Err(_) => {
                return vec![Span {
                    text: text.to_string(),
                    capture: "",
                }]
            }
        };
        for event in events.flatten() {
            match event {
                HighlightEvent::HighlightStart(Highlight(i)) => stack.push(i),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let capture = stack.last().map(|&i| HIGHLIGHT_NAMES[i]).unwrap_or("");
                    spans.push(Span {
                        text: text[start..end].to_string(),
                        capture,
                    });
                }
            }
        }
        spans
    }
}
