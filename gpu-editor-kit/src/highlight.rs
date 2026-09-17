//! A small, dependency-free syntax highlighter: it splits source into colored spans (keywords, strings, line comments,
//! numbers, plain). It exists to prove colored text renders on the GPU; a tree-sitter highlighter replaces it later
//! without touching the renderer, which only consumes `Span`s.

use glyphon::Color;

pub struct Span {
    pub text: String,
    pub color: Color,
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "pub", "struct", "enum", "impl", "trait", "for", "while", "loop", "if", "else", "match",
    "return", "use", "mod", "self", "Self", "const", "static", "move", "async", "await", "where", "as", "in", "ref",
    "true", "false",
];

fn plain() -> Color { Color::rgb(220, 223, 228) }
fn keyword() -> Color { Color::rgb(198, 120, 221) }
fn string() -> Color { Color::rgb(152, 195, 121) }
fn comment() -> Color { Color::rgb(92, 99, 112) }
fn number() -> Color { Color::rgb(209, 154, 102) }

pub fn highlight(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            spans.push(Span { text: "\n".into(), color: plain() });
        }
        highlight_line(line, &mut spans);
    }
    spans
}

fn highlight_line(line: &str, spans: &mut Vec<Span>) {
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut word_start: Option<usize> = None;

    let flush_word = |spans: &mut Vec<Span>, bytes: &[char], start: usize, end: usize| {
        let word: String = bytes[start..end].iter().collect();
        let color = if KEYWORDS.contains(&word.as_str()) {
            keyword()
        } else if word.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            number()
        } else {
            plain()
        };
        spans.push(Span { text: word, color });
    };

    while i < bytes.len() {
        let c = bytes[i];

        // Line comment: the rest of the line.
        if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '/' {
            if let Some(s) = word_start.take() {
                flush_word(spans, &bytes, s, i);
            }
            let rest: String = bytes[i..].iter().collect();
            spans.push(Span { text: rest, color: comment() });
            return;
        }

        // String literal.
        if c == '"' {
            if let Some(s) = word_start.take() {
                flush_word(spans, &bytes, s, i);
            }
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == '\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let lit: String = bytes[start..i].iter().collect();
            spans.push(Span { text: lit, color: string() });
            continue;
        }

        let is_word = c.is_alphanumeric() || c == '_';
        if is_word {
            if word_start.is_none() {
                word_start = Some(i);
            }
        } else {
            if let Some(s) = word_start.take() {
                flush_word(spans, &bytes, s, i);
            }
            spans.push(Span { text: c.to_string(), color: plain() });
        }
        i += 1;
    }
    if let Some(s) = word_start.take() {
        flush_word(spans, &bytes, s, bytes.len());
    }
}
