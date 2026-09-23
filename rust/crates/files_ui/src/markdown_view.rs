//! Markdown from a language server laid out for a popover: paragraphs wrapped word by word with bold, inline
//! code and links, headings, lists, quotes, rules, and code blocks highlighted in their language. Lines are
//! laid out ahead of time so the popover knows its size before it is placed, and can scroll by line.

use editor::{EditorBuffer, Lang, Syntax};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ui::{div, label, theme, Node, Rgba};

/// UI text size of popover prose.
const TEXT_SIZE: f32 = 16.0;
const BOLD_WEIGHT: u16 = 700;
const LIST_INDENT: f32 = 10.0;
const QUOTE_INDENT: f32 = 16.0;
const RULE_THICKNESS: f32 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub bold: bool,
    pub code: bool,
    pub link: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Span {
    text: String,
    style: SpanStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextKind {
    Paragraph,
    Heading,
}

#[derive(Clone, Debug, PartialEq)]
enum Block {
    Text {
        spans: Vec<Span>,
        kind: TextKind,
        list_depth: usize,
        marker: Option<String>,
        quoted: bool,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Rule,
}

/// Spacing and line height for one kind of popover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkdownStyle {
    pub line_height: f32,
    /// Below each paragraph; none where lines must stay a whole number of line heights.
    pub paragraph_spacing: f32,
    pub code_block_margin: f32,
    pub heading_margin_top: f32,
}

/// Info popovers: 1.3 rem lines, paragraphs 8px apart, 1 rem around code blocks and above headings.
pub const HOVER_STYLE: MarkdownStyle = MarkdownStyle {
    line_height: TEXT_SIZE * 1.3,
    paragraph_spacing: 8.0,
    code_block_margin: 16.0,
    heading_margin_top: 16.0,
};

/// Diagnostic popovers: the default golden-ratio line height, no paragraph spacing, no heading margin.
pub const DIAGNOSTIC_STYLE: MarkdownStyle = MarkdownStyle {
    line_height: TEXT_SIZE * 1.618,
    paragraph_spacing: 0.0,
    code_block_margin: 16.0,
    heading_margin_top: 0.0,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Markdown {
    blocks: Vec<Block>,
}

#[derive(Default)]
struct ParseState {
    blocks: Vec<Block>,
    spans: Vec<Span>,
    kind: Option<TextKind>,
    bold: usize,
    link: usize,
    lists: Vec<Option<u64>>,
    marker: Option<String>,
    quote: usize,
    code: Option<(Option<String>, String)>,
}

impl ParseState {
    fn style(&self) -> SpanStyle {
        SpanStyle {
            bold: self.bold > 0 || self.kind == Some(TextKind::Heading),
            code: false,
            link: self.link > 0,
        }
    }

    fn push(&mut self, text: &str, style: SpanStyle) {
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.spans.push(Span {
                text: text.to_string(),
                style,
            }),
        }
    }

    fn flush(&mut self) {
        let kind = self.kind.take().unwrap_or(TextKind::Paragraph);
        if self.spans.iter().all(|span| span.text.trim().is_empty()) {
            self.spans.clear();
            return;
        }
        self.blocks.push(Block::Text {
            spans: std::mem::take(&mut self.spans),
            kind,
            list_depth: self.lists.len(),
            marker: self.marker.take(),
            quoted: self.quote > 0,
        });
    }
}

impl Markdown {
    pub fn parse(source: &str) -> Self {
        let mut state = ParseState::default();
        for event in Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH) {
            match event {
                Event::Start(Tag::Paragraph) => state.kind = Some(TextKind::Paragraph),
                Event::Start(Tag::Heading { .. }) => {
                    state.flush();
                    state.kind = Some(TextKind::Heading);
                }
                Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => state.flush(),
                Event::Start(Tag::BlockQuote(_)) => {
                    state.flush();
                    state.quote += 1;
                }
                Event::End(TagEnd::BlockQuote(_)) => {
                    state.flush();
                    state.quote = state.quote.saturating_sub(1);
                }
                Event::Start(Tag::List(start)) => {
                    state.flush();
                    state.lists.push(start);
                }
                Event::End(TagEnd::List(_)) => {
                    state.flush();
                    state.lists.pop();
                }
                Event::Start(Tag::Item) => {
                    state.flush();
                    // Plain ASCII markers; ordered lists count up from their start.
                    state.marker = Some(match state.lists.last_mut() {
                        Some(Some(next)) => {
                            let marker = format!("{next}.");
                            *next += 1;
                            marker
                        }
                        _ => "-".to_string(),
                    });
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    state.flush();
                    let language = match kind {
                        CodeBlockKind::Fenced(info) => info
                            .split(|c: char| c.is_whitespace() || c == ',')
                            .next()
                            .filter(|name| !name.is_empty())
                            .map(str::to_string),
                        CodeBlockKind::Indented => None,
                    };
                    state.code = Some((language, String::new()));
                }
                Event::End(TagEnd::CodeBlock) => {
                    if let Some((language, mut text)) = state.code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        state.blocks.push(Block::Code { language, text });
                    }
                }
                Event::Start(Tag::Strong) => state.bold += 1,
                Event::End(TagEnd::Strong) => state.bold = state.bold.saturating_sub(1),
                Event::Start(Tag::Link { .. }) => state.link += 1,
                Event::End(TagEnd::Link) => state.link = state.link.saturating_sub(1),
                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                    if let Some((_, code)) = state.code.as_mut() {
                        code.push_str(&text);
                    } else {
                        let style = state.style();
                        state.push(&text, style);
                    }
                }
                Event::Code(text) => {
                    let style = SpanStyle {
                        code: true,
                        ..state.style()
                    };
                    state.push(&text, style);
                }
                Event::SoftBreak => {
                    let style = state.style();
                    state.push(" ", style);
                }
                Event::HardBreak => {
                    let style = state.style();
                    state.push("\n", style);
                }
                Event::Rule => {
                    state.flush();
                    state.blocks.push(Block::Rule);
                }
                _ => {}
            }
        }
        state.flush();
        Self {
            blocks: state.blocks,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Plain text escaped so markdown reads it back unchanged: punctuation backslashed, leading indentation kept
/// as non-breaking spaces (a tab as four), and each line break made a paragraph break.
pub fn escape(text: &str) -> String {
    const TAB_SIZE: usize = 4;
    let mut escaped = String::with_capacity(text.len());
    let mut leading = true;
    for c in text.chars() {
        match c {
            '\t' if leading => escaped.extend(std::iter::repeat_n('\u{a0}', TAB_SIZE)),
            ' ' if leading => escaped.push('\u{a0}'),
            '\n' => escaped.push_str("\n\n"),
            c if c.is_ascii_punctuation() => {
                escaped.push('\\');
                escaped.push(c);
            }
            c => escaped.push(c),
        }
        leading = c == '\n' || (leading && (c == ' ' || c == '\t'));
    }
    escaped
}

#[derive(Clone, Debug, PartialEq)]
enum LineKind {
    Text {
        runs: Vec<(String, SpanStyle)>,
        indent: f32,
        quoted: bool,
    },
    Code(Vec<(String, Rgba)>),
    Rule,
    Gap,
}

#[derive(Clone, Debug, PartialEq)]
struct Line {
    kind: LineKind,
    height: f32,
}

/// Markdown laid out at a width, in design px.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkdownLayout {
    lines: Vec<Line>,
    pub width: f32,
    style: MarkdownStyle,
}

fn text_width(text: &str, style: SpanStyle) -> f32 {
    let weight = if style.bold {
        BOLD_WEIGHT
    } else {
        ui::ui_font_weight()
    };
    let size = if style.code {
        crate::EDIT_FONT
    } else {
        TEXT_SIZE
    };
    ui::measure_text_width(text, size, style.code, weight) / ui::ui_text_scale()
}

/// Break styled text into rows no wider than `width`: at spaces where possible, inside a word only when it
/// can't fit a row by itself, and wherever the text has a line break.
fn wrap_spans(spans: &[Span], width: f32) -> Vec<Vec<(String, SpanStyle)>> {
    let mut tokens: Vec<(String, SpanStyle)> = Vec::new();
    for span in spans {
        let mut current = String::new();
        for c in span.text.chars() {
            if c == '\n' {
                if !current.is_empty() {
                    tokens.push((std::mem::take(&mut current), span.style));
                }
                tokens.push(("\n".to_string(), span.style));
                continue;
            }
            if c != ' ' && current.ends_with(' ') {
                tokens.push((std::mem::take(&mut current), span.style));
            }
            current.push(c);
        }
        if !current.is_empty() {
            tokens.push((current, span.style));
        }
    }
    let mut rows: Vec<Vec<(String, SpanStyle)>> = vec![Vec::new()];
    let mut row_width = 0.0;
    let push = |rows: &mut Vec<Vec<(String, SpanStyle)>>, text: &str, style: SpanStyle| {
        let Some(row) = rows.last_mut() else {
            return;
        };
        match row.last_mut() {
            Some((last, last_style)) if *last_style == style => last.push_str(text),
            _ => row.push((text.to_string(), style)),
        }
    };
    for (token, style) in tokens {
        if token == "\n" {
            rows.push(Vec::new());
            row_width = 0.0;
            continue;
        }
        let token_width = text_width(token.trim_end(), style);
        if row_width > 0.0 && row_width + token_width > width {
            rows.push(Vec::new());
            row_width = 0.0;
        }
        if token_width <= width || row_width > 0.0 {
            push(&mut rows, &token, style);
            row_width += text_width(&token, style);
            continue;
        }
        let mut piece = String::new();
        for c in token.chars() {
            piece.push(c);
            if text_width(&piece, style) > width && piece.chars().count() > 1 {
                piece.pop();
                push(&mut rows, &piece, style);
                rows.push(Vec::new());
                piece = c.to_string();
            }
        }
        row_width = text_width(&piece, style);
        push(&mut rows, &piece, style);
    }
    for row in &mut rows {
        if let Some((last, _)) = row.last_mut() {
            let trimmed = last.trim_end().len();
            last.truncate(trimmed);
        }
        row.retain(|(text, _)| !text.is_empty());
    }
    while rows.last().is_some_and(Vec::is_empty) && rows.len() > 1 {
        rows.pop();
    }
    rows
}

/// A code block's lines as colored runs, highlighted when its language is known, each wrapped to `columns`.
fn code_rows(language: Option<&str>, text: &str, columns: usize) -> Vec<Vec<(String, Rgba)>> {
    let buffer = EditorBuffer::from_text(text);
    let mut syntax = language.and_then(Lang::for_injection).and_then(Syntax::new);
    if let Some(syntax) = syntax.as_mut() {
        syntax.sync(&buffer);
    }
    let colors = crate::syntax_theme();
    let columns = columns.max(1);
    let mut rows = Vec::new();
    for line in 0..buffer.rope.len_lines() {
        let segments = crate::segments_of(&buffer, syntax.as_ref(), line, &colors);
        let mut row: Vec<(String, Rgba)> = Vec::new();
        let mut used = 0;
        for (text, color) in segments {
            for c in text.chars() {
                if used == columns {
                    rows.push(std::mem::take(&mut row));
                    used = 0;
                }
                match row.last_mut() {
                    Some((last, last_color)) if *last_color == color => last.push(c),
                    _ => row.push((c.to_string(), color)),
                }
                used += 1;
            }
        }
        rows.push(row);
    }
    rows
}

impl Markdown {
    /// Lay out for a text area `width` design px wide.
    pub fn layout(&self, width: f32, style: MarkdownStyle) -> MarkdownLayout {
        let mut lines: Vec<Line> = Vec::new();
        let gap = |lines: &mut Vec<Line>, height: f32| {
            if height > 0.0 && !lines.is_empty() {
                lines.push(Line {
                    kind: LineKind::Gap,
                    height,
                });
            }
        };
        let code_line_height = crate::EDIT_LINE_H;
        let mut widest: f32 = 0.0;
        let last = self.blocks.len().saturating_sub(1);
        for (index, block) in self.blocks.iter().enumerate() {
            match block {
                Block::Text {
                    spans,
                    kind,
                    list_depth,
                    marker,
                    quoted,
                } => {
                    if *kind == TextKind::Heading {
                        gap(&mut lines, style.heading_margin_top);
                    }
                    let indent =
                        *list_depth as f32 * LIST_INDENT + if *quoted { QUOTE_INDENT } else { 0.0 };
                    let mut spans = spans.clone();
                    if let Some(marker) = marker {
                        spans.insert(
                            0,
                            Span {
                                text: format!("{marker} "),
                                style: SpanStyle::default(),
                            },
                        );
                    }
                    for runs in wrap_spans(&spans, (width - indent).max(1.0)) {
                        let row_width: f32 = runs
                            .iter()
                            .map(|(text, style)| text_width(text, *style))
                            .sum();
                        widest = widest.max(indent + row_width);
                        lines.push(Line {
                            kind: LineKind::Text {
                                runs,
                                indent,
                                quoted: *quoted,
                            },
                            height: style.line_height,
                        });
                    }
                    if *kind == TextKind::Paragraph && index != last {
                        gap(&mut lines, style.paragraph_spacing);
                    }
                }
                Block::Code { language, text } => {
                    gap(&mut lines, style.code_block_margin);
                    let em = text_width(
                        "M",
                        SpanStyle {
                            code: true,
                            ..SpanStyle::default()
                        },
                    );
                    let columns = (width / em.max(1.0)).floor() as usize;
                    for row in code_rows(language.as_deref(), text, columns) {
                        let cols: usize = row.iter().map(|(text, _)| text.chars().count()).sum();
                        widest = widest.max(cols as f32 * em);
                        lines.push(Line {
                            kind: LineKind::Code(row),
                            height: code_line_height,
                        });
                    }
                    if index != last {
                        gap(&mut lines, style.code_block_margin);
                    }
                }
                Block::Rule => {
                    gap(&mut lines, style.paragraph_spacing);
                    lines.push(Line {
                        kind: LineKind::Rule,
                        height: RULE_THICKNESS,
                    });
                    gap(&mut lines, style.paragraph_spacing);
                    widest = width;
                }
            }
        }
        MarkdownLayout {
            lines,
            width: widest.min(width).ceil(),
            style,
        }
    }
}

impl MarkdownLayout {
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn height(&self) -> f32 {
        self.lines.iter().map(|line| line.height).sum()
    }

    /// How many lines from `first` fit in `max_height`.
    pub fn lines_fitting(&self, first: usize, max_height: f32) -> usize {
        let mut used = 0.0;
        let mut count = 0;
        for line in self.lines.iter().skip(first) {
            if used + line.height > max_height + 0.5 {
                break;
            }
            used += line.height;
            count += 1;
        }
        count.max(1)
    }

    pub fn height_of(&self, first: usize, count: usize) -> f32 {
        self.lines
            .iter()
            .skip(first)
            .take(count)
            .map(|line| line.height)
            .sum()
    }

    /// `count` lines from `first`, as a column `width` wide.
    pub fn render(&self, first: usize, count: usize, width: f32) -> Node {
        let colors = theme();
        let mut column = div().col().w_px(width);
        for line in self.lines.iter().skip(first).take(count) {
            let row = match &line.kind {
                LineKind::Gap => div().h_px(line.height),
                LineKind::Rule => div().row().h_px(line.height).w_px(width).bg(colors.border),
                LineKind::Text {
                    runs,
                    indent,
                    quoted,
                } => {
                    let color = if *quoted {
                        colors.text_muted
                    } else {
                        colors.editor_foreground
                    };
                    let mut row = div().row().items_center().h_px(line.height);
                    if *quoted {
                        row = row.child(
                            div()
                                .w_px(2.0)
                                .h_px(line.height)
                                .bg(colors.text_muted)
                                .child(div()),
                        );
                        row = row.child(div().w_px(*indent - 2.0));
                    } else if *indent > 0.0 {
                        row = row.child(div().w_px(*indent));
                    }
                    for (text, style) in runs {
                        row = row.child(render_run(text, *style, color));
                    }
                    row
                }
                LineKind::Code(segments) => {
                    let mut row = div().row().items_center().h_px(line.height);
                    for (text, color) in segments {
                        row = row.child(
                            label(text.clone())
                                .size(crate::EDIT_FONT)
                                .mono()
                                .color(*color),
                        );
                    }
                    row
                }
            };
            column = column.child(row);
        }
        column.into()
    }
}

fn render_run(text: &str, style: SpanStyle, color: Rgba) -> Node {
    let mut run = label(text.to_string()).color(color);
    run = if style.code {
        run.size(crate::EDIT_FONT).mono()
    } else {
        run.size(TEXT_SIZE)
    };
    if style.bold {
        run = run.weight(BOLD_WEIGHT);
    }
    if style.code {
        return div().row().bg(theme().background).child(run).into();
    }
    run.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_and_inline_styles() {
        let markdown = Markdown::parse(
            "```rust\nfn main()\n```\n\nSome **bold** and `code`.\n\n- one\n- two\n\n1. first\n\n---\n# Title",
        );
        let kinds: Vec<&str> = markdown
            .blocks
            .iter()
            .map(|block| match block {
                Block::Text {
                    kind: TextKind::Heading,
                    ..
                } => "heading",
                Block::Text {
                    marker: Some(_), ..
                } => "item",
                Block::Text { .. } => "text",
                Block::Code { .. } => "code",
                Block::Rule => "rule",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["code", "text", "item", "item", "item", "rule", "heading"]
        );
        let Block::Code { language, text } = &markdown.blocks[0] else {
            panic!("code first");
        };
        assert_eq!(
            (language.as_deref(), text.as_str()),
            (Some("rust"), "fn main()")
        );
        let Block::Text { spans, .. } = &markdown.blocks[1] else {
            panic!("paragraph second");
        };
        assert!(spans.iter().any(|s| s.style.bold && s.text == "bold"));
        assert!(spans.iter().any(|s| s.style.code && s.text == "code"));
        let markers: Vec<Option<&str>> = markdown.blocks[2..5]
            .iter()
            .map(|block| match block {
                Block::Text { marker, .. } => marker.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(markers, vec![Some("-"), Some("-"), Some("1.")]);
    }

    #[test]
    fn wraps_at_spaces_and_breaks_long_words() {
        let spans = vec![Span {
            text: "alpha beta gamma delta".into(),
            style: SpanStyle::default(),
        }];
        let width = text_width("alpha beta", SpanStyle::default())
            .max(text_width("gamma delta", SpanStyle::default()))
            + 1.0;
        let rows = wrap_spans(&spans, width);
        let texts: Vec<String> = rows
            .iter()
            .map(|row| row.iter().map(|(t, _)| t.as_str()).collect())
            .collect();
        assert_eq!(texts, vec!["alpha beta", "gamma delta"]);
        let long = vec![Span {
            text: "x".repeat(40),
            style: SpanStyle::default(),
        }];
        let rows = wrap_spans(&long, text_width("xxxxxxxxxx", SpanStyle::default()));
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn layout_measures_lines_gaps_and_scroll_windows() {
        let markdown = Markdown::parse("first paragraph\n\nsecond paragraph");
        let layout = markdown.layout(400.0, HOVER_STYLE);
        assert_eq!(layout.line_count(), 3);
        assert_eq!(
            layout.height(),
            HOVER_STYLE.line_height * 2.0 + HOVER_STYLE.paragraph_spacing
        );
        assert!(layout.width <= 400.0 && layout.width > 0.0);
        assert_eq!(layout.lines_fitting(0, HOVER_STYLE.line_height), 1);
        let code = Markdown::parse("```rust\nlet x = 1;\n```").layout(400.0, HOVER_STYLE);
        assert_eq!(code.line_count(), 1);
        assert_eq!(code.height(), crate::EDIT_LINE_H);
    }

    #[test]
    fn escaping_keeps_text_literal() {
        let text = "a*b_c [x](y) `z`";
        let markdown = Markdown::parse(&escape(text));
        let Block::Text { spans, .. } = &markdown.blocks[0] else {
            panic!("one paragraph");
        };
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, text);
    }
}
