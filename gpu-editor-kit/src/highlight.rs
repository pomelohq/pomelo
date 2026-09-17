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

// Capture names we recognize; tree-sitter maps each query capture to an index into this list.
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute", "boolean", "comment", "constant", "constant.builtin", "constructor", "escape", "function",
    "function.builtin", "function.method", "keyword", "label", "number", "operator", "property", "punctuation",
    "punctuation.bracket", "punctuation.delimiter", "string", "string.escape", "string.special", "tag", "type",
    "type.builtin", "variable", "variable.builtin", "variable.parameter",
];

fn plain() -> Color { Color::rgb(220, 223, 228) }

fn color_for_name(name: &str) -> Color {
    if name.starts_with("keyword") {
        Color::rgb(198, 120, 221)
    } else if name.starts_with("function") || name == "constructor" {
        Color::rgb(97, 175, 239)
    } else if name.starts_with("type") {
        Color::rgb(229, 192, 123)
    } else if name.starts_with("string") || name == "escape" {
        Color::rgb(152, 195, 121)
    } else if name.starts_with("comment") {
        Color::rgb(92, 99, 112)
    } else if name.starts_with("number") || name.starts_with("constant") || name == "boolean" {
        Color::rgb(209, 154, 102)
    } else if name.starts_with("property") || name.starts_with("tag") || name.starts_with("attribute") {
        Color::rgb(224, 108, 117)
    } else if name.starts_with("operator") || name.starts_with("punctuation") {
        Color::rgb(171, 178, 191)
    } else {
        plain()
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
