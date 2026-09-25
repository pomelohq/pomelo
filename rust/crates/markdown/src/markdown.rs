//! Markdown laid out for the ui element tree: paragraphs wrapped word by word with bold, italic, strikethrough,
//! inline code and links; headings, lists and task lists, quotes, rules, tables, and code blocks highlighted in
//! their language. Lines are laid out ahead of time so a popover knows its size before it is placed and a
//! scrolling view can show only the lines in sight.

use editor::{EditorBuffer, Lang, Syntax};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ui::{div, icon, label, theme, IconKind, Node, Rgba};

const BOLD_WEIGHT: u16 = 700;
const HEADING_WEIGHT: u16 = 600;
const LIST_INDENT: f32 = 10.0;
const QUOTE_INDENT: f32 = 16.0;
const QUOTE_BAR: f32 = 2.0;
const RULE_THICKNESS: f32 = 1.0;
const TAB_COLUMNS: usize = 4;
const TASK_BOX: f32 = 12.0;
const MARKER_GAP: f32 = 6.0;
const TABLE_CELL_PAD_X: f32 = 10.0;
const TABLE_CELL_PAD_Y: f32 = 4.0;
const CODE_BLOCK_PAD: f32 = 12.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    /// Index into `Markdown::links`.
    pub link: Option<u16>,
}

#[derive(Clone, Debug, PartialEq)]
struct Span {
    text: String,
    style: SpanStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextKind {
    Paragraph,
    /// Level 1 to 6.
    Heading(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Marker {
    Bullet,
    Number(u64),
    Task(bool),
}

#[derive(Clone, Debug, PartialEq)]
enum Block {
    Text {
        spans: Vec<Span>,
        kind: TextKind,
        list_depth: usize,
        marker: Option<Marker>,
        quoted: bool,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Rule,
    Table {
        alignments: Vec<Alignment>,
        /// The header row first.
        rows: Vec<Vec<Vec<Span>>>,
    },
}

/// How headings look: popovers keep them at text size in bold; documents size them by level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HeadingStyle {
    Plain,
    Sized {
        /// Multiples of the text size, h1 to h6.
        scales: [f32; 6],
        margin_bottom: f32,
        /// A rule under h1 and h2.
        rules: bool,
    },
}

/// Sizes and spacing for one kind of markdown view, in design px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkdownStyle {
    pub text_size: f32,
    pub line_height: f32,
    /// Below each paragraph; none where lines must stay a whole number of line heights.
    pub paragraph_spacing: f32,
    pub code_block_margin: f32,
    pub heading_margin_top: f32,
    pub headings: HeadingStyle,
    pub code_size: f32,
    pub code_line_height: f32,
    /// Code blocks drawn on a faint panel of their own.
    pub code_block_box: bool,
}

impl MarkdownStyle {
    pub fn with_code(self, size: f32, line_height: f32) -> MarkdownStyle {
        MarkdownStyle {
            code_size: size,
            code_line_height: line_height,
            ..self
        }
    }

    pub fn with_text_size(self, size: f32) -> MarkdownStyle {
        MarkdownStyle {
            line_height: self.line_height / self.text_size * size,
            text_size: size,
            ..self
        }
    }

    fn heading_size(&self, level: u8) -> f32 {
        match self.headings {
            HeadingStyle::Plain => self.text_size,
            HeadingStyle::Sized { scales, .. } => {
                self.text_size * scales[usize::from(level.clamp(1, 6)) - 1]
            }
        }
    }

    fn heading_line_height(&self, level: u8) -> f32 {
        match self.headings {
            HeadingStyle::Plain => self.line_height,
            HeadingStyle::Sized { .. } => (self.heading_size(level) * 1.25).ceil(),
        }
    }
}

const POPOVER_TEXT: f32 = 16.0;

/// Info popovers: 1.3 rem lines, paragraphs 8px apart, 1 rem around code blocks and above headings.
pub const HOVER_STYLE: MarkdownStyle = MarkdownStyle {
    text_size: POPOVER_TEXT,
    line_height: POPOVER_TEXT * 1.3,
    paragraph_spacing: 8.0,
    code_block_margin: 16.0,
    heading_margin_top: 16.0,
    headings: HeadingStyle::Plain,
    code_size: 15.0,
    code_line_height: 24.0,
    code_block_box: false,
};

/// Diagnostic popovers: the default golden-ratio line height, no paragraph spacing, no heading margin.
pub const DIAGNOSTIC_STYLE: MarkdownStyle = MarkdownStyle {
    line_height: POPOVER_TEXT * 1.618,
    paragraph_spacing: 0.0,
    heading_margin_top: 0.0,
    ..HOVER_STYLE
};

/// Documents (a file preview, a pull request or ticket body): 1.5 line height, paragraphs 16px apart,
/// semibold headings from 1.75 down to 0.85 of the text, rules under h1 and h2, boxed code blocks.
pub const DOCUMENT_STYLE: MarkdownStyle = MarkdownStyle {
    text_size: 14.0,
    line_height: 21.0,
    paragraph_spacing: 16.0,
    code_block_margin: 16.0,
    heading_margin_top: 24.0,
    headings: HeadingStyle::Sized {
        scales: [1.75, 1.4, 1.2, 1.0, 0.875, 0.85],
        margin_bottom: 12.0,
        rules: true,
    },
    code_size: 13.0,
    code_line_height: 20.0,
    code_block_box: true,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Markdown {
    blocks: Vec<Block>,
    /// Byte offset in the source where each block starts.
    sources: Vec<usize>,
    links: Vec<String>,
}

#[derive(Default)]
struct TableState {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Vec<Span>>>,
    cell: Option<Vec<Span>>,
}

#[derive(Default)]
struct ParseState {
    blocks: Vec<Block>,
    sources: Vec<usize>,
    /// Where the innermost block being read started in the source.
    block_start: usize,
    links: Vec<String>,
    spans: Vec<Span>,
    kind: Option<TextKind>,
    bold: usize,
    italic: usize,
    strike: usize,
    link: Vec<u16>,
    lists: Vec<Option<u64>>,
    marker: Option<Marker>,
    quote: usize,
    code: Option<(Option<String>, String)>,
    table: Option<TableState>,
}

impl ParseState {
    fn style(&self) -> SpanStyle {
        SpanStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: false,
            link: self.link.last().copied(),
        }
    }

    fn target(&mut self) -> &mut Vec<Span> {
        match self.table.as_mut().and_then(|table| table.cell.as_mut()) {
            Some(cell) => cell,
            None => &mut self.spans,
        }
    }

    fn push(&mut self, text: &str, style: SpanStyle) {
        let spans = self.target();
        match spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => spans.push(Span {
                text: text.to_string(),
                style,
            }),
        }
    }

    fn push_block(&mut self, block: Block) {
        self.sources.push(self.block_start);
        self.blocks.push(block);
    }

    fn link_index(&mut self, url: &str) -> u16 {
        match self.links.iter().position(|known| known == url) {
            Some(index) => index as u16,
            None => {
                self.links.push(url.to_string());
                (self.links.len() - 1) as u16
            }
        }
    }

    /// Plain text with bare `http(s)://` addresses turned into links.
    fn push_text(&mut self, text: &str) {
        let style = self.style();
        if style.link.is_some() {
            self.push(text, style);
            return;
        }
        let mut rest = text;
        while let Some(start) = find_url(rest) {
            let tail = &rest[start..];
            let mut end = tail
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
                .unwrap_or(tail.len());
            while end > 0 && tail[..end].ends_with(['.', ',', ';', ':', ')', '!', '?', '\'']) {
                end -= 1;
            }
            if start > 0 {
                self.push(&rest[..start], style);
            }
            let url = &tail[..end];
            let index = self.link_index(url);
            self.push(
                url,
                SpanStyle {
                    link: Some(index),
                    ..style
                },
            );
            rest = &tail[end..];
        }
        if !rest.is_empty() {
            self.push(rest, style);
        }
    }

    fn flush(&mut self) {
        let kind = self.kind.take().unwrap_or(TextKind::Paragraph);
        if self.spans.iter().all(|span| span.text.trim().is_empty()) {
            self.spans.clear();
            if self.marker.is_none() {
                return;
            }
        }
        let start = self.block_start;
        self.sources.push(start);
        self.blocks.push(Block::Text {
            spans: std::mem::take(&mut self.spans),
            kind,
            list_depth: self.lists.len(),
            marker: self.marker.take(),
            quoted: self.quote > 0,
        });
    }
}

fn heading_slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

fn find_url(text: &str) -> Option<usize> {
    ["https://", "http://"]
        .iter()
        .filter_map(|scheme| text.find(scheme))
        .min()
}

impl Markdown {
    pub fn parse(source: &str) -> Self {
        let options = Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_GFM;
        let mut state = ParseState::default();
        for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
            if let Event::Start(
                Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::Item
                | Tag::CodeBlock(_)
                | Tag::Table(_),
            ) = &event
            {
                if !matches!(event, Event::Start(Tag::Paragraph)) || state.kind.is_none() {
                    state.block_start = range.start;
                }
            }
            if matches!(event, Event::Rule) {
                state.flush();
                state.block_start = range.start;
            }
            match event {
                Event::Start(Tag::Paragraph) if state.kind.is_none() => {
                    state.kind = Some(TextKind::Paragraph);
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    state.flush();
                    state.kind = Some(TextKind::Heading(level as u8));
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
                    state.marker = Some(match state.lists.last_mut() {
                        Some(Some(next)) => {
                            let marker = Marker::Number(*next);
                            *next += 1;
                            marker
                        }
                        _ => Marker::Bullet,
                    });
                }
                Event::TaskListMarker(checked) => state.marker = Some(Marker::Task(checked)),
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
                        state.push_block(Block::Code { language, text });
                    }
                }
                Event::Start(Tag::Table(alignments)) => {
                    state.flush();
                    state.table = Some(TableState {
                        alignments,
                        ..TableState::default()
                    });
                }
                Event::Start(Tag::TableHead | Tag::TableRow) => {
                    if let Some(table) = state.table.as_mut() {
                        table.rows.push(Vec::new());
                    }
                }
                Event::Start(Tag::TableCell) => {
                    if let Some(table) = state.table.as_mut() {
                        table.cell = Some(Vec::new());
                    }
                }
                Event::End(TagEnd::TableCell) => {
                    if let Some(table) = state.table.as_mut() {
                        let cell = table.cell.take().unwrap_or_default();
                        if let Some(row) = table.rows.last_mut() {
                            row.push(cell);
                        }
                    }
                }
                Event::End(TagEnd::Table) => {
                    if let Some(table) = state.table.take() {
                        state.push_block(Block::Table {
                            alignments: table.alignments,
                            rows: table.rows,
                        });
                    }
                }
                Event::Start(Tag::Strong) => state.bold += 1,
                Event::End(TagEnd::Strong) => state.bold = state.bold.saturating_sub(1),
                Event::Start(Tag::Emphasis) => state.italic += 1,
                Event::End(TagEnd::Emphasis) => state.italic = state.italic.saturating_sub(1),
                Event::Start(Tag::Strikethrough) => state.strike += 1,
                Event::End(TagEnd::Strikethrough) => state.strike = state.strike.saturating_sub(1),
                Event::Start(Tag::Link { dest_url, .. }) => {
                    let index = state.link_index(&dest_url);
                    state.link.push(index);
                }
                Event::End(TagEnd::Link) => {
                    state.link.pop();
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    let index = state.link_index(&dest_url);
                    state.link.push(index);
                    let style = state.style();
                    state.push("Image: ", style);
                }
                Event::End(TagEnd::Image) => {
                    state.link.pop();
                }
                Event::Text(text) => {
                    if let Some((_, code)) = state.code.as_mut() {
                        code.push_str(&text);
                    } else {
                        state.push_text(&text);
                    }
                }
                Event::Html(text) | Event::InlineHtml(text) => {
                    if let Some((_, code)) = state.code.as_mut() {
                        code.push_str(&text);
                    } else if !text.trim_start().starts_with("<!--") {
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
                    state.push_block(Block::Rule);
                }
                _ => {}
            }
        }
        state.flush();
        Self {
            blocks: state.blocks,
            sources: state.sources,
            links: state.links,
        }
    }

    /// The block holding source byte `offset`: the last one starting at or before it.
    pub fn block_at_source(&self, offset: usize) -> Option<usize> {
        match self.sources.partition_point(|start| *start <= offset) {
            0 => None,
            after => Some(after - 1),
        }
    }

    /// The heading a `#slug` link points at, slugged like hosted markdown: lowercase, spaces to dashes,
    /// punctuation dropped.
    pub fn heading_block(&self, slug: &str) -> Option<usize> {
        let wanted = slug.trim_start_matches('#').to_lowercase();
        self.blocks.iter().position(|block| match block {
            Block::Text {
                spans,
                kind: TextKind::Heading(_),
                ..
            } => {
                heading_slug(
                    &spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                ) == wanted
            }
            _ => false,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Link targets; a link's clickable id is its index added to the base passed to `render_links`.
    pub fn links(&self) -> &[String] {
        &self.links
    }
}

/// A markdown text parsed once and laid out again only when the width changes, for views that show whole
/// documents (a pull request or ticket body, a comment).
#[derive(Clone, Debug)]
pub struct MarkdownBody {
    markdown: Markdown,
    layout: Option<(u32, MarkdownLayout)>,
}

impl MarkdownBody {
    pub fn new(source: &str) -> MarkdownBody {
        MarkdownBody {
            markdown: Markdown::parse(source),
            layout: None,
        }
    }

    /// Every line at `width` in `style`, links clickable as `link_base + their index`.
    pub fn render(&mut self, width: f32, style: MarkdownStyle, link_base: u64) -> Node {
        let key = width.to_bits();
        if self.layout.as_ref().is_none_or(|(at, _)| *at != key) {
            self.layout = Some((key, self.markdown.layout(width, style)));
        }
        match &self.layout {
            Some((_, layout)) => {
                layout.render_links(0, layout.line_count(), width, Some(link_base))
            }
            None => div().into(),
        }
    }

    pub fn link(&self, index: usize) -> Option<&str> {
        self.markdown.links().get(index).map(String::as_str)
    }
}

/// Plain text escaped so markdown reads it back unchanged: punctuation backslashed, leading indentation kept
/// as non-breaking spaces (a tab as four), and each line break made a paragraph break.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    let mut leading = true;
    for c in text.chars() {
        match c {
            '\t' if leading => escaped.extend(std::iter::repeat_n('\u{a0}', TAB_COLUMNS)),
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

type Runs = Vec<(String, SpanStyle)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleKind {
    Thematic,
    Heading,
}

#[derive(Clone, Debug, PartialEq)]
enum LineKind {
    Text {
        runs: Runs,
        indent: f32,
        quoted: bool,
        heading: Option<u8>,
        /// Shown on the first row of a list item; later rows hang under its text.
        marker: Option<Marker>,
        marker_width: f32,
    },
    Code {
        segments: Vec<(String, Rgba)>,
        boxed: bool,
        first: bool,
        last: bool,
    },
    Rule(RuleKind),
    TableRow {
        cells: Vec<Runs>,
        widths: Vec<f32>,
        alignments: Vec<Alignment>,
        header: bool,
        odd: bool,
    },
    Gap,
}

#[derive(Clone, Debug, PartialEq)]
struct Line {
    kind: LineKind,
    height: f32,
    block: usize,
}

/// Markdown laid out at a width, in design px.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkdownLayout {
    lines: Vec<Line>,
    pub width: f32,
    style: MarkdownStyle,
}

#[derive(Clone, Copy)]
struct Font {
    size: f32,
    code_size: f32,
    weight: Option<u16>,
}

impl Font {
    fn of(style: &MarkdownStyle, heading: Option<u8>) -> Font {
        Font {
            size: heading.map_or(style.text_size, |level| style.heading_size(level)),
            code_size: style.code_size,
            weight: heading
                .filter(|_| matches!(style.headings, HeadingStyle::Sized { .. }))
                .map(|_| HEADING_WEIGHT)
                .or(heading.map(|_| BOLD_WEIGHT)),
        }
    }

    fn weight(&self, style: SpanStyle) -> u16 {
        if style.bold {
            BOLD_WEIGHT
        } else {
            self.weight.unwrap_or_else(ui::ui_font_weight)
        }
    }

    fn width(&self, text: &str, style: SpanStyle) -> f32 {
        let size = if style.code {
            self.code_size
        } else {
            self.size
        };
        ui::measure_text_width_styled(text, size, style.code, self.weight(style), style.italic)
            / ui::ui_text_scale()
    }
}

/// Break styled text into rows no wider than `width`: at spaces where possible, inside a word only when it
/// can't fit a row by itself, and wherever the text has a line break.
fn wrap_spans(spans: &[Span], width: f32, font: Font) -> Vec<Runs> {
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
    let mut rows: Vec<Runs> = vec![Vec::new()];
    let mut row_width = 0.0;
    let push = |rows: &mut Vec<Runs>, text: &str, style: SpanStyle| {
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
        let token_width = font.width(token.trim_end(), style);
        if row_width > 0.0 && row_width + token_width > width {
            rows.push(Vec::new());
            row_width = 0.0;
        }
        if token_width <= width || row_width > 0.0 {
            push(&mut rows, &token, style);
            row_width += font.width(&token, style);
            continue;
        }
        let mut piece = String::new();
        for c in token.chars() {
            piece.push(c);
            if font.width(&piece, style) > width && piece.chars().count() > 1 {
                piece.pop();
                push(&mut rows, &piece, style);
                rows.push(Vec::new());
                piece = c.to_string();
            }
        }
        row_width = font.width(&piece, style);
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

fn syntax_theme() -> editor::Theme {
    if theme().appearance == ui::Appearance::Light {
        editor::Theme::one_light()
    } else {
        editor::Theme::one_dark()
    }
}

fn capture_color(colors: &editor::Theme, capture: &str) -> Rgba {
    let color = if capture.is_empty() {
        colors.foreground
    } else {
        colors.syntax_color(capture)
    };
    let [r, g, b] = color.0;
    Rgba::new(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    )
}

/// A code block's lines as colored runs, highlighted when its language is known, each wrapped to `columns`.
fn code_rows(language: Option<&str>, text: &str, columns: usize) -> Vec<Vec<(String, Rgba)>> {
    let buffer = EditorBuffer::from_text(text);
    let mut syntax = language.and_then(Lang::for_injection).and_then(Syntax::new);
    if let Some(syntax) = syntax.as_mut() {
        syntax.sync(&buffer);
    }
    let colors = syntax_theme();
    let columns = columns.max(1);
    let mut rows = Vec::new();
    for line in 0..buffer.rope.len_lines() {
        let start = buffer.rope.line_to_byte(line);
        let end = start + buffer.rope.line(line).len_bytes()
            - usize::from(buffer.line_len(line) < buffer.rope.line(line).len_chars());
        let runs = match syntax.as_ref() {
            Some(syntax) => syntax.highlight(&buffer.rope, start..end),
            None => vec![editor::syntax::HighlightRun {
                range: start..end,
                capture: None,
            }],
        };
        let (mut column, mut byte) = (0usize, 0usize);
        let mut row: Vec<(String, Rgba)> = Vec::new();
        let mut used = 0;
        for run in runs.into_iter().filter(|run| !run.range.is_empty()) {
            let color = capture_color(&colors, run.capture.unwrap_or(""));
            let mut expanded = String::new();
            for chunk in buffer.rope.byte_slice(run.range).chunks() {
                editor::display::push_expanded(
                    &mut expanded,
                    chunk,
                    TAB_COLUMNS,
                    &mut column,
                    &mut byte,
                );
            }
            for c in expanded.chars() {
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

fn marker_text(marker: &Marker) -> Option<String> {
    match marker {
        Marker::Bullet => Some("-".to_string()),
        Marker::Number(number) => Some(format!("{number}.")),
        Marker::Task(_) => None,
    }
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
                    block: 0,
                });
            }
        };
        let mut widest: f32 = 0.0;
        let last = self.blocks.len().saturating_sub(1);
        for (index, block) in self.blocks.iter().enumerate() {
            let before = lines.len();
            match block {
                Block::Text {
                    spans,
                    kind,
                    list_depth,
                    marker,
                    quoted,
                } => {
                    let heading = match kind {
                        TextKind::Heading(level) => Some(*level),
                        TextKind::Paragraph => None,
                    };
                    if heading.is_some() {
                        gap(&mut lines, style.heading_margin_top);
                    }
                    let font = Font::of(&style, heading);
                    let indent = list_depth.saturating_sub(1) as f32 * LIST_INDENT
                        + if *list_depth > 0 { LIST_INDENT } else { 0.0 }
                        + if *quoted { QUOTE_INDENT } else { 0.0 };
                    let marker_width = match marker {
                        Some(Marker::Task(_)) => TASK_BOX + MARKER_GAP,
                        Some(other) => marker_text(other).map_or(0.0, |text| {
                            font.width(&text, SpanStyle::default()) + MARKER_GAP
                        }),
                        None => 0.0,
                    };
                    let line_height =
                        heading.map_or(style.line_height, |level| style.heading_line_height(level));
                    let rows = wrap_spans(spans, (width - indent - marker_width).max(1.0), font);
                    for (row, runs) in rows.into_iter().enumerate() {
                        let row_width: f32 = runs
                            .iter()
                            .map(|(text, span)| font.width(text, *span))
                            .sum();
                        widest = widest.max(indent + marker_width + row_width);
                        lines.push(Line {
                            kind: LineKind::Text {
                                runs,
                                indent,
                                quoted: *quoted,
                                heading,
                                marker: marker.clone().filter(|_| row == 0),
                                marker_width,
                            },
                            height: line_height,
                            block: 0,
                        });
                    }
                    match (heading, style.headings) {
                        (
                            Some(level),
                            HeadingStyle::Sized {
                                margin_bottom,
                                rules,
                                ..
                            },
                        ) => {
                            if rules && level <= 2 {
                                gap(&mut lines, 4.0);
                                lines.push(Line {
                                    kind: LineKind::Rule(RuleKind::Heading),
                                    height: RULE_THICKNESS,
                                    block: 0,
                                });
                                widest = width;
                            }
                            if index != last {
                                gap(&mut lines, margin_bottom);
                            }
                        }
                        (None, _) if index != last => {
                            let tight_list = marker.is_some()
                                && matches!(
                                    self.blocks.get(index + 1),
                                    Some(Block::Text {
                                        marker: Some(_),
                                        ..
                                    })
                                );
                            if tight_list {
                                gap(&mut lines, 4.0_f32.min(style.paragraph_spacing));
                            } else {
                                gap(&mut lines, style.paragraph_spacing);
                            }
                        }
                        _ => {}
                    }
                }
                Block::Code { language, text } => {
                    gap(&mut lines, style.code_block_margin);
                    let em =
                        ui::measure_text_width("M", style.code_size, true, ui::ui_font_weight())
                            / ui::ui_text_scale();
                    let pad = if style.code_block_box {
                        CODE_BLOCK_PAD
                    } else {
                        0.0
                    };
                    let columns = ((width - 2.0 * pad) / em.max(1.0)).floor() as usize;
                    let rows = code_rows(language.as_deref(), text, columns);
                    let count = rows.len();
                    for (row_index, segments) in rows.into_iter().enumerate() {
                        let cols: usize =
                            segments.iter().map(|(text, _)| text.chars().count()).sum();
                        widest = widest.max(cols as f32 * em + 2.0 * pad);
                        let (first, end) = (row_index == 0, row_index + 1 == count);
                        let mut height = style.code_line_height;
                        if style.code_block_box {
                            height += if first { pad } else { 0.0 } + if end { pad } else { 0.0 };
                            widest = width;
                        }
                        lines.push(Line {
                            kind: LineKind::Code {
                                segments,
                                boxed: style.code_block_box,
                                first,
                                last: end,
                            },
                            height,
                            block: 0,
                        });
                    }
                    if index != last {
                        gap(&mut lines, style.code_block_margin);
                    }
                }
                Block::Rule => {
                    gap(&mut lines, style.paragraph_spacing);
                    lines.push(Line {
                        kind: LineKind::Rule(RuleKind::Thematic),
                        height: RULE_THICKNESS,
                        block: 0,
                    });
                    gap(&mut lines, style.paragraph_spacing);
                    widest = width;
                }
                Block::Table { alignments, rows } => {
                    let font = Font::of(&style, None);
                    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
                    let cell_runs = |cell: &Vec<Span>| -> Runs {
                        cell.iter()
                            .map(|span| (span.text.replace('\n', " "), span.style))
                            .collect()
                    };
                    let mut widths = vec![0.0_f32; columns];
                    for (row_index, row) in rows.iter().enumerate() {
                        let cell_font = Font {
                            weight: (row_index == 0).then_some(HEADING_WEIGHT),
                            ..font
                        };
                        for (column, cell) in row.iter().enumerate() {
                            let text_width: f32 = cell_runs(cell)
                                .iter()
                                .map(|(text, span)| cell_font.width(text, *span))
                                .sum();
                            widths[column] =
                                widths[column].max(text_width + 2.0 * TABLE_CELL_PAD_X);
                        }
                    }
                    let total: f32 = widths.iter().sum::<f32>() + RULE_THICKNESS;
                    if total > width && total > 0.0 {
                        let factor = width / total;
                        for column_width in &mut widths {
                            *column_width *= factor;
                        }
                    }
                    let table_width: f32 = widths.iter().sum::<f32>() + RULE_THICKNESS;
                    widest = widest.max(table_width.min(width));
                    for (row_index, row) in rows.iter().enumerate() {
                        let mut cells: Vec<Runs> = row.iter().map(cell_runs).collect();
                        cells.resize(columns, Vec::new());
                        lines.push(Line {
                            kind: LineKind::TableRow {
                                cells,
                                widths: widths.clone(),
                                alignments: alignments.clone(),
                                header: row_index == 0,
                                odd: row_index > 0 && row_index % 2 == 0,
                            },
                            height: style.line_height + 2.0 * TABLE_CELL_PAD_Y,
                            block: 0,
                        });
                    }
                    if index != last {
                        gap(&mut lines, style.paragraph_spacing);
                    }
                }
            }
            for line in &mut lines[before..] {
                line.block = index;
            }
        }
        MarkdownLayout {
            lines,
            width: widest.min(width).ceil(),
            style,
        }
    }
}

fn with_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba::new(color.r, color.g, color.b, alpha)
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

    /// How far down block `block` starts, in design px.
    pub fn offset_of_block(&self, block: usize) -> Option<f32> {
        let first = self.lines.iter().position(|line| line.block == block)?;
        Some(self.height_of(0, first))
    }

    /// The first line at or below `offset` design px from the top, for a view scrolled that far.
    pub fn line_at(&self, offset: f32) -> usize {
        let mut used = 0.0;
        for (index, line) in self.lines.iter().enumerate() {
            if used + line.height > offset {
                return index;
            }
            used += line.height;
        }
        self.lines.len()
    }

    /// `count` lines from `first`, as a column `width` wide.
    pub fn render(&self, first: usize, count: usize, width: f32) -> Node {
        self.render_links(first, count, width, None)
    }

    /// `render` with each link clickable as `link_base + its index in Markdown::links`.
    pub fn render_links(
        &self,
        first: usize,
        count: usize,
        width: f32,
        link_base: Option<u64>,
    ) -> Node {
        let colors = theme();
        let mut column = div().col().w_px(width);
        for line in self.lines.iter().skip(first).take(count) {
            let row: Node = match &line.kind {
                LineKind::Gap => div().h_px(line.height).into(),
                LineKind::Rule(kind) => {
                    let color = match kind {
                        RuleKind::Thematic => colors.border,
                        RuleKind::Heading => colors.border_variant,
                    };
                    div().row().h_px(line.height).w_px(width).bg(color).into()
                }
                LineKind::Text {
                    runs,
                    indent,
                    quoted,
                    heading,
                    marker,
                    marker_width,
                } => {
                    let font = Font::of(&self.style, *heading);
                    let color = if *quoted || *heading == Some(6) {
                        colors.text_muted
                    } else {
                        colors.editor_foreground
                    };
                    let mut row = div().row().items_center().h_px(line.height);
                    if *quoted {
                        let lead = (*indent - QUOTE_INDENT).max(0.0);
                        if lead > 0.0 {
                            row = row.child(div().w_px(lead));
                        }
                        row = row.child(
                            div()
                                .w_px(QUOTE_BAR)
                                .h_px(line.height)
                                .bg(colors.border)
                                .child(div()),
                        );
                        row = row.child(div().w_px(QUOTE_INDENT - QUOTE_BAR));
                    } else if *indent > 0.0 {
                        row = row.child(div().w_px(*indent));
                    }
                    if *marker_width > 0.0 {
                        let mut slot = div().row().items_center().w_px(*marker_width);
                        match marker {
                            Some(Marker::Task(checked)) => {
                                let mut square = div()
                                    .row()
                                    .items_center()
                                    .justify_center()
                                    .w_px(TASK_BOX)
                                    .h_px(TASK_BOX)
                                    .rounded(2.0)
                                    .border(1.0, colors.border);
                                if *checked {
                                    square = square.child(
                                        icon(IconKind::Check)
                                            .size(TASK_BOX - 2.0)
                                            .color(colors.text_accent),
                                    );
                                }
                                slot = slot.child(square);
                            }
                            Some(other) => {
                                if let Some(text) = marker_text(other) {
                                    slot = slot.child(render_run(
                                        &text,
                                        SpanStyle::default(),
                                        colors.text_muted,
                                        font,
                                        None,
                                    ));
                                }
                            }
                            None => {}
                        }
                        row = row.child(slot);
                    }
                    for (text, style) in runs {
                        row = row.child(render_run(text, *style, color, font, link_base));
                    }
                    row.into()
                }
                LineKind::Code {
                    segments,
                    boxed,
                    first,
                    last,
                } => {
                    let mut row = div().row().items_center().h_px(self.style.code_line_height);
                    for (text, color) in segments {
                        row = row.child(
                            label(text.clone())
                                .size(self.style.code_size)
                                .mono()
                                .color(*color),
                        );
                    }
                    if *boxed {
                        let mut boxed_row = div()
                            .col()
                            .w_px(width)
                            .h_px(line.height)
                            .px(CODE_BLOCK_PAD)
                            .bg(with_alpha(colors.editor_foreground, 0.04));
                        if *first {
                            boxed_row = boxed_row.pt(CODE_BLOCK_PAD);
                        }
                        if *last {
                            boxed_row = boxed_row.pb(CODE_BLOCK_PAD);
                        }
                        boxed_row.child(row).into()
                    } else {
                        row.into()
                    }
                }
                LineKind::TableRow {
                    cells,
                    widths,
                    alignments,
                    header,
                    odd,
                } => {
                    let font = Font {
                        weight: header.then_some(HEADING_WEIGHT),
                        ..Font::of(&self.style, None)
                    };
                    let background = if *header {
                        Some(colors.title_bar_background)
                    } else if *odd {
                        Some(colors.panel_background)
                    } else {
                        None
                    };
                    let rule = || {
                        div()
                            .w_px(RULE_THICKNESS)
                            .h_px(line.height)
                            .bg(colors.border)
                    };
                    let mut row = div().row().h_px(line.height).child(rule());
                    for (column, cell) in cells.iter().enumerate() {
                        let cell_width = widths.get(column).copied().unwrap_or(0.0);
                        let mut content = div()
                            .row()
                            .items_center()
                            .w_px((cell_width - RULE_THICKNESS).max(0.0))
                            .h_px(line.height)
                            .px(TABLE_CELL_PAD_X);
                        content = match alignments.get(column) {
                            Some(Alignment::Center) => content.justify_center(),
                            Some(Alignment::Right) => content.justify_end(),
                            _ => content,
                        };
                        if let Some(background) = background {
                            content = content.bg(background);
                        }
                        for (text, style) in cell {
                            content = content.child(render_run(
                                text,
                                *style,
                                colors.editor_foreground,
                                font,
                                link_base,
                            ));
                        }
                        row = row.child(content).child(rule());
                    }
                    let table_width: f32 = widths.iter().sum::<f32>() + RULE_THICKNESS;
                    let horizontal = || {
                        div()
                            .h_px(RULE_THICKNESS)
                            .w_px(table_width)
                            .bg(colors.border)
                    };
                    let mut block = div().col().w_px(table_width);
                    if *header {
                        block = block.child(horizontal());
                    }
                    block = block.child(row).child(horizontal());
                    block.into()
                }
            };
            column = column.child(row);
        }
        column.into()
    }
}

fn render_run(
    text: &str,
    style: SpanStyle,
    color: Rgba,
    font: Font,
    link_base: Option<u64>,
) -> Node {
    let colors = theme();
    let color = if style.link.is_some() {
        colors.text_accent
    } else {
        color
    };
    let mut run = label(text.to_string())
        .color(color)
        .weight(font.weight(style));
    run = if style.code {
        run.size(font.code_size).mono()
    } else {
        run.size(font.size)
    };
    if style.italic {
        run = run.italic();
    }
    if style.strike {
        run = run.strikethrough(color);
    }
    if style.link.is_some() {
        run = run.underline(with_alpha(colors.text_accent, 0.5));
    }
    let node: Node = if style.code {
        div()
            .row()
            .rounded(4.0)
            .bg(with_alpha(colors.editor_foreground, 0.08))
            .child(run)
            .into()
    } else {
        run.into()
    };
    match (style.link, link_base) {
        (Some(index), Some(base)) => div()
            .row()
            .on_click(base + u64::from(index))
            .child(node)
            .into(),
        _ => node,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(markdown: &Markdown) -> Vec<&'static str> {
        markdown
            .blocks
            .iter()
            .map(|block| match block {
                Block::Text {
                    kind: TextKind::Heading(_),
                    ..
                } => "heading",
                Block::Text {
                    marker: Some(_), ..
                } => "item",
                Block::Text { .. } => "text",
                Block::Code { .. } => "code",
                Block::Rule => "rule",
                Block::Table { .. } => "table",
            })
            .collect()
    }

    #[test]
    fn parses_blocks_and_inline_styles() {
        let markdown = Markdown::parse(
            "```rust\nfn main()\n```\n\nSome **bold**, *soft*, ~~gone~~ and `code`.\n\n- one\n- two\n\n1. first\n\n---\n# Title",
        );
        assert_eq!(
            kinds(&markdown),
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
        assert!(spans.iter().any(|s| s.style.italic && s.text == "soft"));
        assert!(spans.iter().any(|s| s.style.strike && s.text == "gone"));
        assert!(spans.iter().any(|s| s.style.code && s.text == "code"));
        let markers: Vec<Option<Marker>> = markdown.blocks[2..5]
            .iter()
            .map(|block| match block {
                Block::Text { marker, .. } => marker.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(
            markers,
            vec![
                Some(Marker::Bullet),
                Some(Marker::Bullet),
                Some(Marker::Number(1))
            ]
        );
        assert!(matches!(
            markdown.blocks[6],
            Block::Text {
                kind: TextKind::Heading(1),
                ..
            }
        ));
    }

    #[test]
    fn links_tasks_and_tables() {
        let markdown = Markdown::parse(
            "See [docs](https://example.com/docs) or https://example.com/a).\n\n- [x] done\n- [ ] todo\n\n| a | b |\n|:--|--:|\n| 1 | 2 |\n| 3 | 4 |\n\n![logo](https://example.com/l.png)",
        );
        assert_eq!(
            markdown.links(),
            &[
                "https://example.com/docs".to_string(),
                "https://example.com/a".to_string(),
                "https://example.com/l.png".to_string()
            ]
        );
        let Block::Text { spans, .. } = &markdown.blocks[0] else {
            panic!("paragraph");
        };
        assert!(spans
            .iter()
            .any(|s| s.text == "https://example.com/a" && s.style.link == Some(1)));
        assert!(spans
            .iter()
            .any(|s| s.text == ")." && s.style.link.is_none()));
        assert!(matches!(
            &markdown.blocks[1],
            Block::Text {
                marker: Some(Marker::Task(true)),
                ..
            }
        ));
        assert!(matches!(
            &markdown.blocks[2],
            Block::Text {
                marker: Some(Marker::Task(false)),
                ..
            }
        ));
        let Block::Table { alignments, rows } = &markdown.blocks[3] else {
            panic!("table");
        };
        assert_eq!(alignments, &vec![Alignment::Left, Alignment::Right]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2][1][0].text, "4");
        let Block::Text { spans, .. } = &markdown.blocks[4] else {
            panic!("image line");
        };
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "Image: logo");
    }

    #[test]
    fn wraps_at_spaces_and_breaks_long_words() {
        let font = Font::of(&HOVER_STYLE, None);
        let spans = vec![Span {
            text: "alpha beta gamma delta".into(),
            style: SpanStyle::default(),
        }];
        let width = font
            .width("alpha beta", SpanStyle::default())
            .max(font.width("gamma delta", SpanStyle::default()))
            + 1.0;
        let rows = wrap_spans(&spans, width, font);
        let texts: Vec<String> = rows
            .iter()
            .map(|row| row.iter().map(|(t, _)| t.as_str()).collect())
            .collect();
        assert_eq!(texts, vec!["alpha beta", "gamma delta"]);
        let long = vec![Span {
            text: "x".repeat(40),
            style: SpanStyle::default(),
        }];
        let rows = wrap_spans(&long, font.width("xxxxxxxxxx", SpanStyle::default()), font);
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
        assert_eq!(layout.line_at(HOVER_STYLE.line_height + 1.0), 1);
        let code = Markdown::parse("```rust\nlet x = 1;\n```").layout(400.0, HOVER_STYLE);
        assert_eq!(code.line_count(), 1);
        assert_eq!(code.height(), HOVER_STYLE.code_line_height);
    }

    #[test]
    fn documents_size_headings_and_rule_the_top_two() {
        let layout = Markdown::parse("# One\n\ntext\n\n### Three").layout(600.0, DOCUMENT_STYLE);
        let heights: Vec<(bool, f32)> = layout
            .lines
            .iter()
            .map(|line| {
                (
                    matches!(line.kind, LineKind::Rule(RuleKind::Heading)),
                    line.height,
                )
            })
            .collect();
        assert!(heights.iter().any(|(rule, _)| *rule), "h1 gets a rule");
        assert_eq!(
            layout
                .lines
                .iter()
                .filter(|line| matches!(line.kind, LineKind::Rule(_)))
                .count(),
            1,
            "h3 gets none"
        );
        assert_eq!(layout.lines[0].height, (14.0_f32 * 1.75 * 1.25).ceil());
    }

    #[test]
    fn tables_fit_the_width() {
        let layout = Markdown::parse(&format!(
            "| {} | b |\n|---|---|\n| 1 | 2 |",
            "wide ".repeat(40)
        ))
        .layout(300.0, DOCUMENT_STYLE);
        let LineKind::TableRow { widths, .. } = &layout.lines[0].kind else {
            panic!("table row");
        };
        assert!(widths.iter().sum::<f32>() <= 300.0);
        assert!(layout.width <= 300.0);
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

    #[test]
    fn source_offsets_find_blocks_and_their_place_in_the_layout() {
        let source = "# Top\n\nfirst paragraph\n\n## Second Part\n\n- item\n\n```\ncode\n```\n";
        let markdown = Markdown::parse(source);
        let paragraph = source.find("first").expect("paragraph");
        assert_eq!(markdown.block_at_source(paragraph + 3), Some(1));
        assert_eq!(markdown.block_at_source(0), Some(0));
        let code = source.find("code").expect("code");
        assert_eq!(markdown.block_at_source(code), Some(4));
        assert_eq!(markdown.heading_block("#second-part"), Some(2));
        let layout = markdown.layout(500.0, DOCUMENT_STYLE);
        let top = layout.offset_of_block(0).expect("top");
        let second = layout.offset_of_block(2).expect("second");
        assert_eq!(top, 0.0);
        assert!(second > top);
    }

    #[test]
    fn links_render_clickable() {
        let markdown = Markdown::parse("go [there](https://example.com)");
        let layout = markdown.layout(400.0, DOCUMENT_STYLE);
        let node = layout.render_links(0, layout.line_count(), 400.0, Some(7_000));
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, 400.0, 100.0, Rgba::TRANSPARENT),
        );
        assert!(painted.hits.iter().any(|(_, id)| *id == 7_000));
        assert!(painted.texts.iter().any(|text| text.text == "there"));
    }
}
