//! Stepping through diagnostics with F8 / Shift-F8: the caret lands on one and its message opens as a block
//! under its line (or at the line's end when it fits in one line there), until Escape or the server drops it.

use std::cell::RefCell;
use std::ops::Range;

use ui::{div, theme, Node, Rgba};

use crate::{char_advance, edit_line_h, DiagnosticEntry, FileItem};
use markdown::{Markdown, MarkdownLayout, MarkdownStyle};

const BLOCK_MAX_CHARACTERS: f32 = 120.0;
const BLOCK_PADDING_LEFT: f32 = 6.0;
const BLOCK_PADDING_RIGHT: f32 = 2.0;
const BLOCK_BORDER: f32 = 2.0;
/// Room between a line's text and a message shown at its end, in characters.
const INLINE_MARGIN_CHARACTERS: f32 = 2.0;

pub(crate) struct ActiveDiagnostic {
    pub(crate) range: Range<usize>,
    severity: lsp::lsp_types::DiagnosticSeverity,
    message: String,
    markdown: Markdown,
    layout: RefCell<Option<(u32, MarkdownLayout)>>,
}

/// Every block line one editor line tall, so the block fills whole rows.
fn block_style() -> MarkdownStyle {
    MarkdownStyle {
        line_height: edit_line_h(),
        paragraph_spacing: 0.0,
        code_block_margin: 0.0,
        heading_margin_top: 0.0,
        ..crate::hover::hover_style()
    }
}

fn block_colors(severity: lsp::lsp_types::DiagnosticSeverity) -> (Rgba, Rgba) {
    use lsp::lsp_types::DiagnosticSeverity as Severity;
    let colors = theme();
    match severity {
        Severity::ERROR => (colors.error_background, colors.error),
        Severity::WARNING => (colors.warning_background, colors.warning),
        Severity::INFORMATION => (colors.info_background, colors.info),
        Severity::HINT => (colors.hint_background, colors.hint),
        _ => (colors.ignored_background, colors.ignored),
    }
}

/// How the active diagnostic's message is placed.
pub(crate) enum BlockPlacement {
    /// At the end of this row's text.
    Inline { row_line: usize },
    /// As this many rows under the line.
    Rows { line: usize, count: usize },
}

impl FileItem {
    /// Go to the diagnostic under the caret if it isn't the open one, else to the next (or previous) one,
    /// wrapping around the file.
    pub(crate) fn go_to_diagnostic(&mut self, forward: bool) {
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        let newest = b.newest();
        let (cursor, head) = (newest.start, newest.head());
        let entries: Vec<&DiagnosticEntry> = self
            .diagnostics
            .iter()
            .filter(|entry| !entry.range.is_empty())
            .collect();
        let before: Vec<&DiagnosticEntry> = entries
            .iter()
            .copied()
            .filter(|entry| entry.range.start <= cursor)
            .collect();
        let after: Vec<&DiagnosticEntry> = entries
            .iter()
            .copied()
            .filter(|entry| entry.range.start >= cursor)
            .collect();
        let is_active = |entry: &DiagnosticEntry| {
            self.active_diagnostic.as_ref().is_some_and(|active| {
                active.range == entry.range && active.message == entry.message
            })
        };
        let mut on_active = false;
        let mut target = None;
        for entry in after.iter().chain(before.iter()) {
            let contains = entry.range.contains(&cursor) || entry.range.end == head;
            if !contains {
                continue;
            }
            if is_active(entry) {
                on_active = true;
            } else if target.is_none() {
                target = Some(*entry);
            }
        }
        let target = match (target, on_active) {
            (Some(entry), false) => Some(entry),
            _ if forward => after
                .iter()
                .chain(before.iter())
                .find(|entry| entry.range.start != cursor)
                .copied(),
            _ => before
                .iter()
                .rev()
                .chain(after.iter().rev())
                .find(|entry| entry.range.start != cursor)
                .copied(),
        };
        let Some(target) = target.cloned() else {
            return;
        };
        self.activate_diagnostic(&target);
    }

    fn activate_diagnostic(&mut self, entry: &DiagnosticEntry) {
        if let Some(b) = self.buffer.as_mut() {
            b.place_cursor(entry.range.start);
        }
        self.active_diagnostic = Some(ActiveDiagnostic {
            range: entry.range.clone(),
            severity: entry.severity,
            message: entry.message.clone(),
            markdown: Markdown::parse(&crate::hover::diagnostic_markdown(entry)),
            layout: RefCell::new(None),
        });
        self.rows = None;
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    pub(crate) fn dismiss_diagnostic(&mut self) -> bool {
        let dismissed = self.active_diagnostic.take().is_some();
        if dismissed {
            self.rows = None;
        }
        dismissed
    }

    /// Close the open message once the server no longer reports that diagnostic.
    pub(crate) fn refresh_active_diagnostic(&mut self) {
        let still_reported = self.active_diagnostic.as_ref().is_none_or(|active| {
            self.diagnostics.iter().any(|entry| {
                !entry.range.is_empty()
                    && entry.range.start == active.range.start
                    && entry.message == active.message
            })
        });
        if !still_reported {
            self.dismiss_diagnostic();
        }
    }

    /// The block's width limit in design px: 120 characters, within the text area.
    fn block_max_width(&self) -> f32 {
        (BLOCK_MAX_CHARACTERS * char_advance()).min(self.text_viewport_w()) / ui::ui_text_scale()
    }

    fn block_layout(&self) -> Option<MarkdownLayout> {
        let active = self.active_diagnostic.as_ref()?;
        let text_width =
            (self.block_max_width() - BLOCK_PADDING_LEFT - BLOCK_PADDING_RIGHT - BLOCK_BORDER)
                .max(1.0);
        let key = text_width.round() as u32;
        let mut cache = active.layout.try_borrow_mut().ok()?;
        if !matches!(cache.as_ref(), Some((cached, _)) if *cached == key) {
            *cache = Some((key, active.markdown.layout(text_width, block_style())));
        }
        cache.as_ref().map(|(_, layout)| layout.clone())
    }

    fn block_width(layout: &MarkdownLayout) -> f32 {
        layout.width + BLOCK_PADDING_LEFT + BLOCK_PADDING_RIGHT + BLOCK_BORDER
    }

    /// Inline at the end of the diagnostic's line when the message is one line and fits there, else rows
    /// under it. Uses the rows as last built, so call it while building the next.
    pub(crate) fn block_placement(&self) -> Option<BlockPlacement> {
        let active = self.active_diagnostic.as_ref()?;
        let b = self.buffer.as_ref()?;
        let line = b
            .rope
            .char_to_line(active.range.start.min(b.rope.len_chars()));
        let layout = self.block_layout()?;
        let line_width = self.line_layout(line).width;
        let fits = line_width
            + INLINE_MARGIN_CHARACTERS * char_advance()
            + Self::block_width(&layout) * ui::ui_text_scale()
            < self.text_viewport_w();
        if layout.line_count() == 1 && fits && !self.is_soft_wrapped(line) {
            return Some(BlockPlacement::Inline { row_line: line });
        }
        let count = (layout.height() / edit_line_h()).ceil().max(1.0) as usize;
        Some(BlockPlacement::Rows { line, count })
    }

    fn is_soft_wrapped(&self, line: usize) -> bool {
        self.wraps
            .get(line)
            .and_then(|wraps| wraps.as_ref())
            .is_some_and(|boundaries| !boundaries.is_empty())
    }

    /// One row of the block (`index` from its top), shifted to the diagnostic's column and kept inside the
    /// text area, in body coordinates.
    pub(crate) fn block_row(&self, index: usize) -> Option<Node> {
        let active = self.active_diagnostic.as_ref()?;
        let layout = self.block_layout()?;
        let scale = ui::ui_text_scale();
        let width = Self::block_width(&layout);
        let (_, target_x) = self.position(active.range.start);
        let em = char_advance();
        let max_x = self.text_viewport_w() - width * scale;
        let min_x = (target_x + em - width * scale).max(0.0);
        let x = target_x.min(max_x).max(min_x);
        Some(
            div()
                .row()
                .h_px(edit_line_h())
                .child(div().w_px(x / scale))
                .child(self.block_segment(&layout, index, 1))
                .into(),
        )
    }

    /// The message at the end of its line.
    pub(crate) fn inline_block(&self) -> Option<Node> {
        let layout = self.block_layout()?;
        Some(
            div()
                .row()
                .child(div().w_px(INLINE_MARGIN_CHARACTERS * char_advance() / ui::ui_text_scale()))
                .child(self.block_segment(&layout, 0, 1))
                .into(),
        )
    }

    fn block_segment(&self, layout: &MarkdownLayout, first: usize, count: usize) -> Node {
        let severity = self
            .active_diagnostic
            .as_ref()
            .map_or(lsp::lsp_types::DiagnosticSeverity::ERROR, |a| a.severity);
        let (background, border) = block_colors(severity);
        let height = edit_line_h();
        div()
            .row()
            .w_px(Self::block_width(layout))
            .h_px(height)
            .bg(background)
            .child(div().w_px(BLOCK_BORDER).h_px(height).bg(border))
            .child(div().w_px(BLOCK_PADDING_LEFT))
            .child(layout.render(first, count, layout.width))
            .into()
    }

    /// The diagnostic under the caret the status bar shows: the most severe, then the tightest.
    pub(crate) fn diagnostic_at_caret(&self) -> Option<&DiagnosticEntry> {
        let head = self.buffer.as_ref()?.newest().head();
        self.diagnostics
            .iter()
            .filter(|entry| {
                !entry.range.is_empty() && entry.range.start <= head && head <= entry.range.end
            })
            .min_by_key(|entry| (entry.severity, entry.range.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp::lsp_types::DiagnosticSeverity;
    use workspace::{EditKey, Item};

    fn entry(range: Range<usize>, severity: DiagnosticSeverity, message: &str) -> DiagnosticEntry {
        DiagnosticEntry {
            range,
            severity,
            message: message.into(),
            source: None,
            code: None,
        }
    }

    fn item(text: &str, diagnostics: Vec<DiagnosticEntry>) -> FileItem {
        let mut item = FileItem::new(
            std::path::PathBuf::from("/nonexistent"),
            "a.rs",
            Some(text.into()),
        );
        item.set_body_height(10.0 * edit_line_h());
        item.set_body_width(1000.0);
        item.ensure_visible();
        item.diagnostics = diagnostics;
        item
    }

    fn caret(item: &FileItem) -> usize {
        item.buffer.as_ref().unwrap().newest().head()
    }

    #[test]
    fn f8_steps_forward_and_back_through_diagnostics_wrapping_around() {
        let text = "let a = b;\nlet c = d;\nlet e = f;\n";
        let mut item = item(
            text,
            vec![
                entry(8..9, DiagnosticSeverity::ERROR, "b"),
                entry(30..31, DiagnosticSeverity::WARNING, "f"),
            ],
        );
        item.buffer.as_mut().unwrap().place_cursor(0);
        item.input_key(EditKey::GoToDiagnostic, false);
        assert_eq!(caret(&item), 8);
        assert!(item.active_diagnostic.is_some());
        item.input_key(EditKey::GoToDiagnostic, false);
        assert_eq!(caret(&item), 30);
        item.input_key(EditKey::GoToDiagnostic, false);
        assert_eq!(caret(&item), 8, "wraps to the first");
        item.input_key(EditKey::GoToPreviousDiagnostic, false);
        assert_eq!(caret(&item), 30);
    }

    #[test]
    fn a_caret_on_a_closed_diagnostic_opens_that_one() {
        let mut item = item(
            "let a = b;\n",
            vec![entry(8..9, DiagnosticSeverity::ERROR, "b")],
        );
        item.buffer.as_mut().unwrap().place_cursor(9);
        item.input_key(EditKey::GoToDiagnostic, false);
        assert_eq!(caret(&item), 8);
        assert_eq!(
            item.active_diagnostic.as_ref().map(|a| a.range.clone()),
            Some(8..9)
        );
    }

    #[test]
    fn a_short_message_sits_at_the_line_end_and_a_long_one_takes_rows_below() {
        let mut short = item(
            "let a = b;\nnext\n",
            vec![entry(8..9, DiagnosticSeverity::ERROR, "oops")],
        );
        short.input_key(EditKey::GoToDiagnostic, false);
        assert!(matches!(
            short.block_placement(),
            Some(BlockPlacement::Inline { row_line: 0 })
        ));
        assert_eq!(short.disp_count(), 3);

        let long = "word ".repeat(80);
        let mut item = item(
            "let a = b;\nnext\n",
            vec![entry(8..9, DiagnosticSeverity::ERROR, &long)],
        );
        item.input_key(EditKey::GoToDiagnostic, false);
        let Some(BlockPlacement::Rows { line: 0, count }) = item.block_placement() else {
            panic!("a long message takes rows");
        };
        assert!(count >= 2);
        assert_eq!(item.disp_count(), 3 + count);
        assert!(item.row(1).unwrap().block == Some(0));
        // The caret steps over the message rows.
        item.input_key(EditKey::Down, false);
        assert_eq!(item.buffer.as_ref().unwrap().line_col().0, 1);
        item.input_key(EditKey::Escape, false);
        assert!(item.active_diagnostic.is_none());
        item.ensure_visible();
        assert_eq!(item.disp_count(), 3);
    }

    #[test]
    fn the_message_closes_once_the_server_drops_the_diagnostic() {
        let mut item = item(
            "let a = b;\n",
            vec![entry(8..9, DiagnosticSeverity::ERROR, "b")],
        );
        item.input_key(EditKey::GoToDiagnostic, false);
        item.set_diagnostics(&lsp::DiagnosticsUpdate {
            path: std::path::PathBuf::from("/nonexistent/a.rs"),
            diagnostics: Vec::new(),
            synced: None,
        });
        assert!(item.active_diagnostic.is_none());
    }

    #[test]
    fn the_status_bar_shows_the_most_severe_diagnostic_at_the_caret() {
        let mut item = item(
            "let a = b;\n",
            vec![
                entry(4..9, DiagnosticSeverity::WARNING, "wide warning"),
                entry(8..9, DiagnosticSeverity::ERROR, "error"),
            ],
        );
        item.buffer.as_mut().unwrap().place_cursor(8);
        assert_eq!(item.diagnostic_message().as_deref(), Some("error"));
        item.buffer.as_mut().unwrap().place_cursor(5);
        assert_eq!(item.diagnostic_message().as_deref(), Some("wide warning"));
    }
}
