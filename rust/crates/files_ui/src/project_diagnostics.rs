//! The project's diagnostics as one tab: each file with problems, each problem as the code around it with
//! its message under the line, errors (and warnings, when shown) only. Clicking a line opens the file there.

use std::ops::Range;
use std::time::{Duration, Instant};

use editor::syntax::Syntax;
use editor::{EditorBuffer, Lang};
use lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use terminal::Modifiers;
use ui::{div, icon, label, material_icon, theme, IconKind, Node, Rect, Rgba};
use workspace::{Item, ItemTick};

pub const ITEM_ID: &str = "project-diagnostics";
/// Changes settle this long before the tab rebuilds, so a burst of publishes redraws once.
const UPDATE_DEBOUNCE: Duration = Duration::from_millis(50);
/// Rows of code shown around a diagnostic when it spans fewer.
const CONTEXT_LINES: usize = 2;
const TOOLBAR_H: f32 = 32.0;
const BUTTON: f32 = 20.0;
const HEADER_ROW_H: f32 = 30.0;
const LINE_H: f32 = 20.0;
const SEPARATOR_H: f32 = 10.0;
const CODE_FONT: f32 = 13.0;
const MESSAGE_FONT: f32 = 13.0;
const MESSAGE_LINE_H: f32 = 18.0;
const MESSAGE_PAD_Y: f32 = 4.0;
const MESSAGE_PAD_X: f32 = 6.0;
const COPY_W: f32 = 24.0;
const GUTTER_W: f32 = 56.0;
/// Files larger than this are listed without their code.
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

const REFRESH: u64 = 1;
const WARNINGS: u64 = 2;
const SHOW_WARNINGS: u64 = 3;
const COPY_BASE: u64 = 100_000;
const ROW_BASE: u64 = 1_000;

/// One file's diagnostics and the text their positions are in.
pub struct DiagnosticsSource {
    pub relative: String,
    pub text: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub path: String,
    pub row: u32,
    pub column: u32,
}

struct Entry {
    line: usize,
    column: usize,
    severity: DiagnosticSeverity,
    message: String,
    source: String,
}

struct DiagnosticsFile {
    relative: String,
    buffer: EditorBuffer,
    syntax: Option<Syntax>,
    entries: Vec<Entry>,
    has_text: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Header(usize),
    Line { file: usize, line: usize },
    Message { file: usize, entry: usize },
    Separator,
}

pub struct ProjectDiagnostics {
    files: Vec<DiagnosticsFile>,
    pending: Option<(Instant, Vec<DiagnosticsSource>)>,
    generation: Option<u64>,
    include_warnings: bool,
    errors: usize,
    warnings: usize,
    rows: Vec<Row>,
    scroll: f32,
    body_h: f32,
    width: f32,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    open: Option<OpenRequest>,
    copied: Option<String>,
    refresh_requested: bool,
}

fn severity_of(diagnostic: &Diagnostic) -> DiagnosticSeverity {
    diagnostic.severity.unwrap_or(DiagnosticSeverity::ERROR)
}

fn severity_colors(severity: DiagnosticSeverity) -> (Rgba, Rgba) {
    let colors = theme();
    match severity {
        DiagnosticSeverity::ERROR => (colors.error, colors.error_background),
        DiagnosticSeverity::WARNING => (colors.warning, colors.warning_background),
        DiagnosticSeverity::INFORMATION => (colors.info, colors.info_background),
        _ => (colors.hint, colors.hint_background),
    }
}

/// `" (source code)"` after a message, as the reference appends it.
fn source_and_code(diagnostic: &Diagnostic) -> String {
    let code = diagnostic.code.as_ref().map(|code| match code {
        NumberOrString::Number(number) => number.to_string(),
        NumberOrString::String(text) => text.clone(),
    });
    match (diagnostic.source.as_deref(), code) {
        (Some(source), Some(code)) => format!("{source} {code}"),
        (Some(source), None) => source.to_string(),
        (None, Some(code)) => code,
        (None, None) => String::new(),
    }
}

/// The rows shown for diagnostics on rows `lines`: at least `2 * CONTEXT_LINES + 1`, the missing ones split
/// before (rounded up) and after.
fn context_rows(lines: Range<usize>, line_count: usize) -> Range<usize> {
    let wanted = 2 * CONTEXT_LINES + 1;
    let have = lines.end - lines.start;
    let missing = wanted.saturating_sub(have);
    let before = missing.div_ceil(2);
    let after = missing / 2;
    let start = lines.start.saturating_sub(before);
    let end = (lines.end + after).min(line_count.max(1));
    start..end.max(start + 1)
}

/// Overlapping or touching ranges joined, in order.
fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

impl ProjectDiagnostics {
    /// `include_warnings` as the tab opens: the reference turns them on when there are only warnings.
    pub fn new(include_warnings: bool) -> Self {
        ProjectDiagnostics {
            files: Vec::new(),
            pending: None,
            generation: None,
            include_warnings,
            errors: 0,
            warnings: 0,
            rows: Vec::new(),
            scroll: 0.0,
            body_h: 0.0,
            width: 0.0,
            hits: Vec::new(),
            hovered: None,
            open: None,
            copied: None,
            refresh_requested: false,
        }
    }

    pub fn set_include_warnings(&mut self, include: bool) {
        if self.include_warnings != include {
            self.include_warnings = include;
            self.rebuild_rows();
        }
    }

    /// The generation of diagnostics the tab shows or waits on, so the owner sends only newer ones.
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    /// Asked to refetch everything (the toolbar's Refresh).
    pub fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh_requested)
    }

    /// New diagnostics to show once they settle.
    pub fn update(&mut self, generation: u64, sources: Vec<DiagnosticsSource>, now: Instant) {
        self.generation = Some(generation);
        self.pending = Some((now, sources));
    }

    pub fn take_open(&mut self) -> Option<OpenRequest> {
        self.open.take()
    }

    fn apply(&mut self, sources: Vec<DiagnosticsSource>) {
        let (mut errors, mut warnings) = (0, 0);
        self.files = sources
            .into_iter()
            .map(|source| {
                let lang = Lang::detect(&source.relative, None);
                let has_text = source
                    .text
                    .as_ref()
                    .is_some_and(|text| text.len() <= MAX_FILE_BYTES);
                let buffer = EditorBuffer::from_text(if has_text {
                    source.text.as_deref().unwrap_or_default()
                } else {
                    ""
                });
                let mut syntax = has_text.then(|| Syntax::new(lang)).flatten();
                if let Some(syntax) = syntax.as_mut() {
                    syntax.sync(&buffer);
                }
                let entries = source
                    .diagnostics
                    .iter()
                    .map(|diagnostic| {
                        match severity_of(diagnostic) {
                            DiagnosticSeverity::ERROR => errors += 1,
                            DiagnosticSeverity::WARNING => warnings += 1,
                            _ => {}
                        }
                        let line = diagnostic.range.start.line as usize;
                        let column = if has_text {
                            let offset =
                                lsp::position_to_char(&buffer.rope, diagnostic.range.start);
                            offset - buffer.rope.line_to_char(buffer.rope.char_to_line(offset))
                        } else {
                            diagnostic.range.start.character as usize
                        };
                        Entry {
                            line,
                            column,
                            severity: severity_of(diagnostic),
                            message: diagnostic.message.trim_end().to_string(),
                            source: source_and_code(diagnostic),
                        }
                    })
                    .collect();
                DiagnosticsFile {
                    relative: source.relative,
                    buffer,
                    syntax,
                    entries,
                    has_text,
                }
            })
            .collect();
        self.errors = errors;
        self.warnings = warnings;
        self.rebuild_rows();
    }

    fn shown(&self, severity: DiagnosticSeverity) -> bool {
        severity == DiagnosticSeverity::ERROR
            || (self.include_warnings && severity == DiagnosticSeverity::WARNING)
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        for (file_index, file) in self.files.iter().enumerate() {
            let shown: Vec<usize> = (0..file.entries.len())
                .filter(|index| self.shown(file.entries[*index].severity))
                .collect();
            if shown.is_empty() {
                continue;
            }
            rows.push(Row::Header(file_index));
            if !file.has_text {
                for entry in shown {
                    rows.push(Row::Message {
                        file: file_index,
                        entry,
                    });
                }
                continue;
            }
            let line_count = file.buffer.rope.len_lines();
            let ranges = merge_ranges(
                shown
                    .iter()
                    .map(|index| {
                        let line = file.entries[*index].line.min(line_count.saturating_sub(1));
                        context_rows(line..line + 1, line_count)
                    })
                    .collect(),
            );
            for (range_index, range) in ranges.iter().enumerate() {
                if range_index > 0 {
                    rows.push(Row::Separator);
                }
                for line in range.clone() {
                    rows.push(Row::Line {
                        file: file_index,
                        line,
                    });
                    for entry in shown.iter().filter(|index| {
                        file.entries[**index].line.min(line_count.saturating_sub(1)) == line
                    }) {
                        rows.push(Row::Message {
                            file: file_index,
                            entry: *entry,
                        });
                    }
                }
            }
        }
        self.rows = rows;
    }

    fn message_lines(&self, entry: &Entry) -> usize {
        let available = (self.width - GUTTER_W - COPY_W - 2.0 * MESSAGE_PAD_X - 16.0).max(80.0);
        let scale = ui::ui_text_scale();
        let text = if entry.source.is_empty() {
            entry.message.clone()
        } else {
            format!("{} ({})", entry.message, entry.source)
        };
        text.lines()
            .map(|line| {
                let width = ui::measure_text_width(line, MESSAGE_FONT, false, 400) / scale;
                ((width / available).ceil() as usize).max(1)
            })
            .sum::<usize>()
            .max(1)
    }

    fn row_height(&self, row: Row) -> f32 {
        match row {
            Row::Header(_) => HEADER_ROW_H,
            Row::Line { .. } => LINE_H,
            Row::Message { file, entry } => {
                let lines = self.message_lines(&self.files[file].entries[entry]);
                lines as f32 * MESSAGE_LINE_H + 2.0 * MESSAGE_PAD_Y + 2.0
            }
            Row::Separator => SEPARATOR_H,
        }
    }

    fn content_height(&self) -> f32 {
        self.rows.iter().map(|row| self.row_height(*row)).sum()
    }

    fn open_row(&mut self, row: Row) {
        let (file, line, column) = match row {
            Row::Header(file) => {
                let first = self.files[file]
                    .entries
                    .iter()
                    .find(|entry| self.shown(entry.severity));
                (
                    file,
                    first.map_or(0, |entry| entry.line),
                    first.map_or(0, |entry| entry.column),
                )
            }
            Row::Line { file, line } => (file, line, 0),
            Row::Message { file, entry } => {
                let entry = &self.files[file].entries[entry];
                (file, entry.line, entry.column)
            }
            Row::Separator => return,
        };
        self.open = Some(OpenRequest {
            path: self.files[file].relative.clone(),
            row: line as u32 + 1,
            column: column as u32 + 1,
        });
    }

    fn click(&mut self, id: u64) {
        match id {
            REFRESH => self.refresh_requested = true,
            WARNINGS | SHOW_WARNINGS => {
                self.include_warnings = !self.include_warnings;
                self.rebuild_rows();
            }
            id if id >= COPY_BASE => {
                if let Some(Row::Message { file, entry }) = self.rows.get((id - COPY_BASE) as usize)
                {
                    self.copied = Some(self.files[*file].entries[*entry].message.clone());
                }
            }
            id if id >= ROW_BASE => {
                if let Some(row) = self.rows.get((id - ROW_BASE) as usize).copied() {
                    self.open_row(row);
                }
            }
            _ => {}
        }
    }

    fn icon_button(&self, id: u64, kind: IconKind, color: Rgba) -> Node {
        let mut button = div()
            .w_px(BUTTON)
            .h_px(BUTTON)
            .rounded(4.0)
            .items_center()
            .justify_center()
            .on_click(id)
            .child(icon(kind).size(14.0).color(color));
        if self.hovered == Some(id) {
            button = button.bg(theme().ghost_element_hover);
        }
        button.into()
    }

    fn toolbar(&self, width: f32) -> Node {
        let colors = theme();
        let warnings_color = if self.include_warnings {
            colors.warning
        } else {
            colors.text_disabled
        };
        div()
            .row()
            .w_px(width)
            .h_px(TOOLBAR_H)
            .px(8.0)
            .gap(4.0)
            .items_center()
            .justify_end()
            .child(self.icon_button(REFRESH, IconKind::RotateCw, colors.icon))
            .child(self.icon_button(WARNINGS, IconKind::Warning, warnings_color))
            .into()
    }

    fn empty_state(&self) -> Node {
        let colors = theme();
        let mut column = div().col().gap(8.0).items_center();
        if self.errors == 0 && self.warnings == 0 {
            column = column.child(
                label("No problems in workspace")
                    .size(14.0)
                    .color(colors.text_muted),
            );
        } else {
            column = column.child(
                label("No errors in workspace")
                    .size(14.0)
                    .color(colors.text_muted),
            );
            if !self.include_warnings && self.warnings > 0 {
                let noun = if self.warnings == 1 {
                    "warning"
                } else {
                    "warnings"
                };
                column = column.child(ui::button_sized(
                    SHOW_WARNINGS,
                    format!("Show {} {noun}", self.warnings),
                    ui::ButtonStyle::Outlined,
                    ui::ButtonSize::Default,
                ));
            }
        }
        div()
            .row()
            .flex(1.0)
            .items_center()
            .justify_center()
            .child(column)
            .into()
    }

    fn header_node(&self, file: usize, id: u64) -> Node {
        let colors = theme();
        let relative = &self.files[file].relative;
        let (folder, name) = match relative.rsplit_once('/') {
            Some((folder, name)) => (folder.to_string(), name.to_string()),
            None => (String::new(), relative.clone()),
        };
        let mut header = div()
            .row()
            .h_px(HEADER_ROW_H)
            .px(12.0)
            .gap(6.0)
            .items_center()
            .bg(colors.tab_bar_background)
            .on_click(id)
            .child(material_icon(crate::file_icon(&name)).size(14.0))
            .child(label(name).size(13.0).color(colors.text))
            .child(
                label(folder)
                    .size(12.0)
                    .color(colors.text_muted)
                    .truncate_start(),
            );
        if self.hovered == Some(id) {
            header = header.bg(colors.element_hover);
        }
        header.into()
    }

    fn line_node(&self, file: usize, line: usize, id: u64) -> Node {
        let colors = theme();
        let syntax = crate::syntax_theme();
        let file = &self.files[file];
        let mut text = div().row().items_center();
        for (segment, color) in
            crate::segments_of(&file.buffer, file.syntax.as_ref(), line, &syntax)
        {
            text = text.child(label(segment).size(CODE_FONT).mono().color(color));
        }
        let mut row = div()
            .row()
            .h_px(LINE_H)
            .items_center()
            .on_click(id)
            .child(
                div().row().w_px(GUTTER_W).pr(12.0).justify_end().child(
                    label((line + 1).to_string())
                        .size(CODE_FONT)
                        .mono()
                        .color(colors.editor_line_number),
                ),
            )
            .child(text);
        if self.hovered == Some(id) {
            row = row.bg(colors.editor_active_line);
        }
        row.into()
    }

    fn message_node(&self, row_index: usize, file: usize, entry: usize, width: f32) -> Node {
        let colors = theme();
        let entry = &self.files[file].entries[entry];
        let (border, background) = severity_colors(entry.severity);
        let block_h = self.message_lines(entry) as f32 * MESSAGE_LINE_H + 2.0 * MESSAGE_PAD_Y;
        let text_w = (width - GUTTER_W - COPY_W - 2.0 * MESSAGE_PAD_X - 16.0).max(80.0);
        let mut body = div().col().w_px(text_w);
        let lines: Vec<&str> = entry.message.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let last = index + 1 == lines.len();
            let mut row = div().row().items_center().child(
                label(line.to_string())
                    .size(MESSAGE_FONT)
                    .color(colors.text)
                    .wrap(text_w),
            );
            if last && !entry.source.is_empty() {
                row = row.child(
                    label(format!(" ({})", entry.source))
                        .size(MESSAGE_FONT)
                        .color(colors.text_muted),
                );
            }
            body = body.child(row);
        }
        let copy_id = COPY_BASE + row_index as u64;
        let mut copy = div()
            .w_px(BUTTON)
            .h_px(BUTTON)
            .rounded(4.0)
            .items_center()
            .justify_center()
            .on_click(copy_id)
            .child(icon(IconKind::Copy).size(12.0).color(colors.icon_muted));
        if self.hovered == Some(copy_id) {
            copy = copy.bg(colors.ghost_element_hover);
        }
        let row_id = ROW_BASE + row_index as u64;
        div()
            .row()
            .py(1.0)
            .pr(8.0)
            .child(div().w_px(GUTTER_W))
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .h_px(block_h)
                    .bg(background)
                    .on_click(row_id)
                    .child(div().w_px(2.0).h_px(block_h).bg(border))
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .py(MESSAGE_PAD_Y)
                            .px(MESSAGE_PAD_X)
                            .gap(4.0)
                            .child(body)
                            .child(div().flex(1.0))
                            .child(copy),
                    ),
            )
            .into()
    }

    fn results_node(&self, width: f32, height: f32) -> Node {
        let colors = theme();
        let mut column = div().col().w_px(width);
        let mut top = 0.0;
        for (index, row) in self.rows.iter().enumerate() {
            let row_h = self.row_height(*row);
            if top + row_h <= self.scroll {
                top += row_h;
                continue;
            }
            // Whole rows only: text draws above every box, so a row cut off below would show past the tab.
            if top + row_h > self.scroll + height {
                break;
            }
            top += row_h;
            let id = ROW_BASE + index as u64;
            column = column.child(match *row {
                Row::Header(file) => self.header_node(file, id),
                Row::Line { file, line } => self.line_node(file, line, id),
                Row::Message { file, entry } => self.message_node(index, file, entry, width),
                Row::Separator => div()
                    .row()
                    .h_px(SEPARATOR_H)
                    .items_center()
                    .child(div().row().flex(1.0).h_px(1.0).bg(colors.border_variant))
                    .into(),
            });
        }
        column.into()
    }

    fn hit_at(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id)
    }
}

impl Item for ProjectDiagnostics {
    fn id(&self) -> Option<String> {
        Some(ITEM_ID.to_string())
    }

    fn title(&self) -> String {
        "Diagnostics".into()
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Warning)
    }

    fn tab_content(&self, active: bool) -> Option<Node> {
        let colors = theme();
        let text = if active {
            colors.text
        } else {
            colors.text_muted
        };
        let mut row = div().row().gap(4.0).items_center();
        if self.errors == 0 && self.warnings == 0 {
            return Some(
                row.child(icon(IconKind::Check).size(14.0).color(colors.success))
                    .child(label("No problems").size(13.0).color(text))
                    .into(),
            );
        }
        if self.errors > 0 {
            row = row
                .child(icon(IconKind::XCircle).size(14.0).color(colors.error))
                .child(label(self.errors.to_string()).size(13.0).color(text));
        }
        if self.warnings > 0 {
            row = row
                .child(icon(IconKind::Warning).size(14.0).color(colors.warning))
                .child(label(self.warnings.to_string()).size(13.0).color(text));
        }
        Some(row.into())
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let (width, height) = (body.w / scale, body.h / scale);
        self.body_h = height;
        self.width = width;
        let results_h = (height - TOOLBAR_H).max(0.0);
        let max_scroll = (self.content_height() - results_h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let middle = if self.rows.is_empty() {
            self.empty_state()
        } else {
            self.results_node(width, results_h)
        };
        let tree: Node = div()
            .col()
            .w_px(width)
            .h_px(height)
            .bg(theme().editor_background)
            .child(self.toolbar(width))
            .child(div().col().h_px(results_h).child(middle))
            .into();
        let painted = ui::render(&tree, body);
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        if let Some(id) = self.hit_at(x, y) {
            self.click(id);
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hit = self.hit_at(x, y);
        let changed = hit != self.hovered;
        self.hovered = hit;
        changed
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let before = self.scroll;
        self.scroll = (self.scroll - delta_y / ui::ui_text_scale()).max(0.0);
        (self.scroll - before).abs() > 0.01
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let mut changed = false;
        if self
            .pending
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= UPDATE_DEBOUNCE)
        {
            if let Some((_, sources)) = self.pending.take() {
                self.apply(sources);
                changed = true;
            }
        }
        for file in &mut self.files {
            if let Some(syntax) = file.syntax.as_mut().filter(|syntax| syntax.is_parsing()) {
                syntax.sync(&file.buffer);
                changed |= !syntax.is_parsing();
            }
        }
        ItemTick {
            changed,
            clipboard_store: self.copied.take(),
            ..ItemTick::default()
        }
    }

    fn is_busy(&self) -> bool {
        self.pending.is_some()
            || self
                .files
                .iter()
                .any(|file| file.syntax.as_ref().is_some_and(Syntax::is_parsing))
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp::lsp_types::{Position, Range as LspRange};

    fn diagnostic(line: u32, severity: DiagnosticSeverity, message: &str) -> Diagnostic {
        Diagnostic {
            range: LspRange::new(Position::new(line, 2), Position::new(line, 4)),
            severity: Some(severity),
            message: message.into(),
            source: Some("ts".into()),
            code: Some(NumberOrString::Number(2875)),
            ..Diagnostic::default()
        }
    }

    fn source(diagnostics: Vec<Diagnostic>) -> DiagnosticsSource {
        let text: String = (0..30).map(|n| format!("line {n}\n")).collect();
        DiagnosticsSource {
            relative: "web/app.ts".into(),
            text: Some(text),
            diagnostics,
        }
    }

    #[test]
    fn context_is_padded_to_five_rows_and_overlaps_merge() {
        assert_eq!(context_rows(10..11, 30), 8..13);
        assert_eq!(context_rows(0..1, 30), 0..3);
        assert_eq!(
            merge_ranges(vec![8..13, 11..16, 20..25]),
            vec![8..16, 20..25]
        );
    }

    #[test]
    fn errors_show_with_their_code_and_warnings_follow_the_toggle() {
        let mut view = ProjectDiagnostics::new(false);
        view.apply(vec![source(vec![
            diagnostic(10, DiagnosticSeverity::ERROR, "bad"),
            diagnostic(20, DiagnosticSeverity::WARNING, "meh"),
            diagnostic(21, DiagnosticSeverity::HINT, "hint"),
        ])]);
        assert_eq!((view.errors, view.warnings), (1, 1));
        let messages = |view: &ProjectDiagnostics| {
            view.rows
                .iter()
                .filter(|row| matches!(row, Row::Message { .. }))
                .count()
        };
        assert_eq!(messages(&view), 1, "warnings off");
        assert_eq!(view.rows[0], Row::Header(0));
        assert!(view.rows.contains(&Row::Line { file: 0, line: 8 }));
        assert_eq!(view.files[0].entries[0].source, "ts 2875");
        view.click(WARNINGS);
        assert_eq!(messages(&view), 2, "hints never get a section");
        assert!(view.rows.contains(&Row::Separator));
    }

    #[test]
    fn clicking_a_message_opens_its_file_at_the_diagnostic() {
        let mut view = ProjectDiagnostics::new(true);
        view.apply(vec![source(vec![diagnostic(
            10,
            DiagnosticSeverity::ERROR,
            "bad",
        )])]);
        let row = view
            .rows
            .iter()
            .position(|row| matches!(row, Row::Message { .. }))
            .expect("message row");
        view.click(ROW_BASE + row as u64);
        assert_eq!(
            view.take_open(),
            Some(OpenRequest {
                path: "web/app.ts".into(),
                row: 11,
                column: 3,
            })
        );
    }

    #[test]
    fn updates_wait_for_the_debounce() {
        let mut view = ProjectDiagnostics::new(true);
        let now = Instant::now();
        view.update(
            3,
            vec![source(vec![diagnostic(1, DiagnosticSeverity::ERROR, "x")])],
            now,
        );
        assert!(view.rows.is_empty());
        view.pending = view
            .pending
            .take()
            .map(|(_, sources)| (now - UPDATE_DEBOUNCE, sources));
        assert!(view.tick(&|| None).changed);
        assert_eq!(view.generation(), Some(3));
        assert!(!view.rows.is_empty());
    }
}
