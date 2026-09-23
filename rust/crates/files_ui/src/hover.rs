//! Popovers about what the pointer rests on: the diagnostic under it and what the language server says about
//! the symbol. They appear once the pointer has been still for a moment (the server is asked halfway through,
//! so its answer is usually back in time), stay while the pointer heads toward them, and go when it leaves.

use std::cell::RefCell;
use std::ops::Range;
use std::time::{Duration, Instant};

use ui::{div, theme, Node, Rect, Rgba};

use crate::markdown_view::{self, Markdown, MarkdownLayout, MarkdownStyle};
use crate::{char_advance, DiagnosticEntry, FileItem, EDIT_LINE_H};

pub(crate) const HOVER_DELAY: Duration = Duration::from_millis(300);
const HIDING_DELAY: Duration = Duration::from_millis(300);
/// Space between stacked popovers.
const POPOVER_GAP: f32 = 10.0;
/// Popovers keep this far from the editor's right edge.
const POPOVER_RIGHT_OFFSET: f32 = 8.0;
const MAX_POPOVER_CHARACTERS: f32 = 120.0;
const MIN_POPOVER_CHARACTERS: f32 = 20.0;
const MAX_POPOVER_LINES: f32 = 16.0;
const MIN_POPOVER_LINES: f32 = 4.0;
/// Moving away by less than this still counts as heading toward a popover.
const CLOSER_TOLERANCE: f32 = 4.0;
const INFO_PADDING: f32 = 8.0;
const DIAGNOSTIC_PADDING_LEFT: f32 = 8.0;
const DIAGNOSTIC_PADDING_RIGHT: f32 = 32.0;
const DIAGNOSTIC_PADDING_Y: f32 = 4.0;
const BORDER: f32 = 1.0;
const RADIUS: f32 = 8.0;

/// A laid-out document kept for the width it was laid out at.
#[derive(Default)]
struct LayoutCache(RefCell<Option<(u32, MarkdownLayout)>>);

impl LayoutCache {
    fn get(&self, markdown: &Markdown, width: f32, style: MarkdownStyle) -> MarkdownLayout {
        let key = width.round() as u32;
        let mut cache = self.0.borrow_mut();
        match cache.as_ref() {
            Some((cached, layout)) if *cached == key => layout.clone(),
            _ => {
                let layout = markdown.layout(width, style);
                *cache = Some((key, layout.clone()));
                layout
            }
        }
    }
}

pub(crate) struct DiagnosticPopover {
    range: Range<usize>,
    severity: lsp::lsp_types::DiagnosticSeverity,
    markdown: Markdown,
    layout: LayoutCache,
    scroll: usize,
    keyboard: bool,
}

pub(crate) struct InfoPopover {
    pub(crate) range: Range<usize>,
    markdown: Markdown,
    layout: LayoutCache,
    scroll: usize,
    keyboard: bool,
}

/// Where the server's answer is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HoverRequest {
    Unsent,
    Sent(u64),
    Answered(Option<(String, Range<usize>)>),
}

pub(crate) struct PendingHover {
    offset: usize,
    started: Instant,
    keyboard: bool,
    pub(crate) request: HoverRequest,
    diagnostic_placed: bool,
}

#[derive(Default)]
pub(crate) struct HoverState {
    pub(crate) pending: Option<PendingHover>,
    diagnostic: Option<DiagnosticPopover>,
    pub(crate) info: Vec<InfoPopover>,
    hide_at: Option<Instant>,
    closest_distance: Option<f32>,
    /// Where the popovers were last placed, in window px.
    bounds: RefCell<Vec<Rect>>,
}

impl FileItem {
    /// A click on a popover keeps it from being a keyboard hover, so the pointer can dismiss it again.
    pub(crate) fn hover_clicked(&mut self) {
        for popover in &mut self.hover.info {
            popover.keyboard = false;
        }
        if let Some(popover) = self.hover.diagnostic.as_mut() {
            popover.keyboard = false;
        }
    }
}

impl HoverState {
    pub(crate) fn visible(&self) -> bool {
        !self.info.is_empty() || self.diagnostic.is_some()
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.pending.is_some() || self.hide_at.is_some()
    }

    fn over_popover(&self, point: (f32, f32)) -> bool {
        self.bounds
            .borrow()
            .iter()
            .any(|r| point.0 >= r.x && point.0 < r.x + r.w && point.1 >= r.y && point.1 < r.y + r.h)
    }

    /// Whether the pointer is no farther from the nearest popover than it has been since the popovers showed.
    fn is_mouse_getting_closer(&mut self, point: (f32, f32)) -> bool {
        if !self.visible() {
            return false;
        }
        let bounds = self.bounds.borrow();
        let Some(distance) = bounds
            .iter()
            .map(|r| {
                let dx = ((point.0 - (r.x + r.w / 2.0)).abs() - r.w / 2.0).max(0.0);
                let dy = ((point.1 - (r.y + r.h / 2.0)).abs() - r.h / 2.0).max(0.0);
                (dx * dx + dy * dy).sqrt()
            })
            .reduce(f32::min)
        else {
            return false;
        };
        drop(bounds);
        if self
            .closest_distance
            .is_some_and(|closest| distance > closest + CLOSER_TOLERANCE)
        {
            return false;
        }
        self.closest_distance = Some(self.closest_distance.map_or(distance, |c| c.min(distance)));
        true
    }

    fn keyboard_anchor(&self) -> bool {
        self.info.iter().any(|popover| popover.keyboard)
            || self
                .diagnostic
                .as_ref()
                .is_some_and(|popover| popover.keyboard)
    }
}

fn severity_colors(severity: lsp::lsp_types::DiagnosticSeverity) -> (Rgba, Rgba) {
    use lsp::lsp_types::DiagnosticSeverity as Severity;
    let colors = theme();
    match severity {
        Severity::ERROR => (colors.error_background, colors.error_border),
        Severity::WARNING => (colors.warning_background, colors.warning_border),
        Severity::INFORMATION => (colors.info_background, colors.info_border),
        Severity::HINT => (colors.hint_background, colors.hint_border),
        _ => (colors.ignored_background, colors.ignored_border),
    }
}

/// A diagnostic as markdown: its message, then its source and code in parentheses.
fn diagnostic_markdown(entry: &DiagnosticEntry) -> String {
    let mut markdown = markdown_view::escape(&entry.message);
    if entry.source.is_some() || entry.code.is_some() {
        markdown.push_str(" (");
        if let Some(source) = entry.source.as_deref() {
            markdown.push_str(&markdown_view::escape(source));
        }
        if entry.source.is_some() && entry.code.is_some() {
            markdown.push(' ');
        }
        if let Some(code) = entry.code.as_deref() {
            markdown.push_str(&markdown_view::escape(code));
        }
        markdown.push(')');
    }
    markdown
}

/// A popover measured for placement, in window px.
struct Measured {
    node: Node,
    width: f32,
    height: f32,
}

impl FileItem {
    /// The char the pointer at `local` (relative to the pane content) is over, if it is over text: a point past
    /// a line's end by a column or more, or below the last row, is over no char.
    pub(crate) fn hover_offset(&self, local_x: f32, local_y: f32) -> Option<usize> {
        let gutter = crate::gutter_width(self.line_count());
        if local_y < 0.0 || local_x < gutter {
            return None;
        }
        let row = ((self.scroll_y + local_y) / EDIT_LINE_H) as usize;
        if row >= self.disp_count() {
            return None;
        }
        let display_row = self.row(row)?;
        if display_row.deleted.is_some() {
            return None;
        }
        let x = local_x - gutter + self.scroll_x;
        if x - self.row_width(row) >= char_advance() {
            return None;
        }
        Some(self.offset_for_row_x(row, x.max(0.0), false))
    }

    /// Where the pointer went: over text starts (or keeps) a hover there; outside the text lets visible
    /// popovers go after a moment unless the pointer heads toward them.
    pub(crate) fn hover_pointer(&mut self, local: Option<(f32, f32)>, window: (f32, f32)) -> bool {
        let was_busy = self.hover.is_busy() || self.hover.visible();
        if self.hover.over_popover(window) {
            self.hover.closest_distance = Some(0.0);
            self.hover.hide_at = None;
            return was_busy;
        }
        let over_text = local.filter(|(x, y)| {
            *y >= 0.0 && *x >= crate::gutter_width(self.line_count()) && *y < self.body_h
        });
        let link_before = self.link_range();
        let modifiers = ui::modifiers();
        match over_text {
            Some((x, y)) => {
                let offset = self.hover_offset(x, y);
                match offset {
                    Some(offset) if modifiers.cmd => {
                        self.show_link_definition(offset, modifiers.shift)
                    }
                    _ if !modifiers.cmd => {
                        self.hide_hovered_link();
                    }
                    _ => {}
                }
                if let Some(offset) = offset {
                    self.hover_at(Some(offset), Some(window));
                }
            }
            None => {
                self.hide_hovered_link();
                self.hover_at(None, Some(window));
            }
        }
        was_busy || self.hover.is_busy() || self.link_range() != link_before || self.link.is_some()
    }

    fn hover_at(&mut self, offset: Option<usize>, pointer: Option<(f32, f32)>) {
        if self.hover.keyboard_anchor() {
            return;
        }
        if let Some(offset) = offset {
            self.hover.hide_at = None;
            self.hover.closest_distance = None;
            self.show_hover(offset, false);
        } else if !self.hover.visible() {
            self.hover.pending = None;
        } else {
            let getting_closer = pointer.is_some_and(|p| self.hover.is_mouse_getting_closer(p));
            if !getting_closer && self.hover.hide_at.is_some() {
                return;
            }
            self.hover.hide_at = Some(Instant::now() + HIDING_DELAY);
        }
    }

    /// Start a hover at `offset`, unless the popovers already describe it; `keyboard` shows it at once.
    pub(crate) fn show_hover(&mut self, offset: usize, keyboard: bool) {
        self.hover.hide_at = None;
        self.hover.closest_distance = None;
        if !keyboard {
            let covers = |range: &Range<usize>| range.start <= offset && offset <= range.end;
            let same_info = self.hover.info.iter().any(|popover| covers(&popover.range));
            if same_info || self.hover.diagnostic.is_some() {
                return;
            }
        }
        self.hide_hover();
        self.hover.pending = Some(PendingHover {
            offset,
            started: Instant::now(),
            keyboard,
            request: HoverRequest::Unsent,
            diagnostic_placed: false,
        });
    }

    pub(crate) fn hide_hover(&mut self) -> bool {
        let hidden = self.hover.visible();
        self.hover.info.clear();
        self.hover.diagnostic = None;
        self.hover.pending = None;
        self.hover.hide_at = None;
        self.hover.closest_distance = None;
        self.hover.bounds.borrow_mut().clear();
        hidden
    }

    /// The offset whose server hover should be asked for now, halfway into the delay (at once for a keyboard
    /// hover).
    pub(crate) fn hover_request_due(&self, now: Instant) -> Option<usize> {
        let pending = self.hover.pending.as_ref()?;
        let due = pending.keyboard || now.duration_since(pending.started) >= HOVER_DELAY / 2;
        (due && pending.request == HoverRequest::Unsent).then_some(pending.offset)
    }

    /// Record that the request went out under `token`, or that no server can answer.
    pub(crate) fn hover_requested(&mut self, token: Option<u64>) {
        if let Some(pending) = self.hover.pending.as_mut() {
            pending.request = match token {
                Some(token) => HoverRequest::Sent(token),
                None => HoverRequest::Answered(None),
            };
        }
    }

    /// Adopt the server's answer if it is for the pending hover, placing its range in today's text.
    pub(crate) fn hover_answered(&mut self, response: &lsp::HoverResponse) {
        let Some(pending) = self.hover.pending.as_ref() else {
            return;
        };
        if pending.request != HoverRequest::Sent(response.request) {
            return;
        }
        let offset = pending.offset;
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        let answer = response.markdown.clone().map(|markdown| {
            let range = match response.range.clone() {
                Some(mut range) => {
                    for batch in b.edits_since(response.synced.buffer_version) {
                        range.start = editor::buffer::map_offset(
                            batch,
                            range.start,
                            editor::buffer::Bias::Left,
                        );
                        range.end = editor::buffer::map_offset(
                            batch,
                            range.end,
                            editor::buffer::Bias::Right,
                        );
                    }
                    range
                }
                None => self.syntax_range_at(offset).unwrap_or(offset..offset),
            };
            (markdown, range)
        });
        if let Some(pending) = self.hover.pending.as_mut() {
            pending.request = HoverRequest::Answered(answer);
        }
    }

    /// The smallest syntax node around `offset`, in chars.
    fn syntax_range_at(&self, offset: usize) -> Option<Range<usize>> {
        let (b, syntax) = (self.buffer.as_ref()?, self.syntax.as_ref()?);
        let byte = b.rope.char_to_byte(offset.min(b.rope.len_chars()));
        let node = syntax.syntax_ancestor(byte..byte)?;
        Some(b.rope.byte_to_char(node.range.start)..b.rope.byte_to_char(node.range.end))
    }

    /// Advance the hover's timers: show the diagnostic once the delay is up, then the server's answer once
    /// it is in, and hide when the hiding delay ran out.
    pub(crate) fn tick_hover(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self.hover.hide_at.is_some_and(|at| now >= at) {
            changed |= self.hide_hover();
        }
        let Some(pending) = self.hover.pending.as_ref() else {
            return changed;
        };
        let (offset, keyboard) = (pending.offset, pending.keyboard);
        let delay_over = keyboard || now.duration_since(pending.started) >= HOVER_DELAY;
        if delay_over && !pending.diagnostic_placed {
            self.hover.diagnostic = self.diagnostic_at(offset).map(|entry| DiagnosticPopover {
                range: entry.range.clone(),
                severity: entry.severity,
                markdown: Markdown::parse(&diagnostic_markdown(entry)),
                layout: LayoutCache::default(),
                scroll: 0,
                keyboard,
            });
            if let Some(pending) = self.hover.pending.as_mut() {
                pending.diagnostic_placed = true;
            }
            changed = true;
        }
        let answered = match self
            .hover
            .pending
            .as_ref()
            .map(|p| (&p.request, p.diagnostic_placed))
        {
            Some((HoverRequest::Answered(answer), true)) => Some(answer.clone()),
            _ => None,
        };
        if let Some(answer) = answered {
            self.hover.info = answer
                .map(|(markdown, range)| InfoPopover {
                    range,
                    markdown: Markdown::parse(&markdown),
                    layout: LayoutCache::default(),
                    scroll: 0,
                    keyboard,
                })
                .filter(|popover| !popover.markdown.is_empty())
                .into_iter()
                .collect();
            self.hover.pending = None;
            changed = true;
        }
        changed
    }

    /// The diagnostic most tightly around `offset`.
    fn diagnostic_at(&self, offset: usize) -> Option<&DiagnosticEntry> {
        self.diagnostics
            .iter()
            .filter(|entry| entry.range.start <= offset && offset <= entry.range.end)
            .min_by_key(|entry| entry.range.len())
    }

    pub(crate) fn scroll_hover_popover(&mut self, index: usize, dy: f32) -> bool {
        let rows = (-dy / (EDIT_LINE_H * ui::ui_text_scale())).round() as isize;
        if rows == 0 {
            return false;
        }
        let scroll = match (index, self.hover.diagnostic.as_mut()) {
            (0, Some(diagnostic)) => &mut diagnostic.scroll,
            (index, diagnostic) => {
                let index = index - usize::from(diagnostic.is_some());
                let Some(info) = self.hover.info.get_mut(index) else {
                    return false;
                };
                &mut info.scroll
            }
        };
        let before = *scroll;
        *scroll = (*scroll as isize + rows).max(0) as usize;
        self.hover.closest_distance = Some(0.0);
        self.hover.hide_at = None;
        *scroll != before
    }

    /// The popovers placed around the hovered point: above it when they all fit inside the editor, else below.
    pub(crate) fn hover_popover_nodes(&self, content: Rect) -> Vec<(Node, f32, f32)> {
        if !self.hover.visible() || self.completions.is_some() {
            self.hover.bounds.borrow_mut().clear();
            return Vec::new();
        }
        let anchor = self
            .hover
            .diagnostic
            .as_ref()
            .map(|popover| popover.range.start)
            .or_else(|| self.hover.info.first().map(|popover| popover.range.start));
        let Some(anchor) = anchor else {
            return Vec::new();
        };
        let scale = ui::ui_text_scale();
        let em = char_advance();
        let text_width = content.w - crate::gutter_width(self.line_count());
        let max_width = (MAX_POPOVER_CHARACTERS * em)
            .min(text_width / 2.0)
            .max(MIN_POPOVER_CHARACTERS * em)
            / scale;
        let max_height = (MAX_POPOVER_LINES * EDIT_LINE_H)
            .min(self.body_h / 2.0)
            .max(MIN_POPOVER_LINES * EDIT_LINE_H)
            / scale;

        let (mut row, x) = self.position(anchor);
        let first_visible = self.first_line();
        let last_visible = first_visible + (self.body_h / EDIT_LINE_H).ceil() as usize;
        row = row.clamp(
            first_visible,
            last_visible.saturating_sub(1).max(first_visible),
        );
        let hovered_x = content.x + crate::gutter_width(self.line_count()) + x - self.scroll_x;
        let hovered_y = content.y + row as f32 * EDIT_LINE_H - self.scroll_y;
        let line_height = EDIT_LINE_H;

        let mut measured: Vec<Measured> = Vec::new();
        if let Some(diagnostic) = self.hover.diagnostic.as_ref() {
            measured.push(self.measure_diagnostic(diagnostic, max_width, max_height));
        }
        for info in &self.hover.info {
            measured.push(self.measure_info(info, max_width, max_height));
        }
        let right_edge = content.x + content.w - POPOVER_RIGHT_OFFSET * scale;
        let origin_x = |width: f32| hovered_x + (right_edge - (hovered_x + width)).min(0.0);
        let inside = |x: f32, y: f32, w: f32, h: f32| {
            x >= content.x
                && y >= content.y
                && x + w <= content.x + content.w + 0.5
                && y + h <= content.y + self.body_h + 0.5
        };
        let above = {
            let mut y = hovered_y;
            measured.iter().all(|popover| {
                y -= popover.height;
                let fits = inside(origin_x(popover.width), y, popover.width, popover.height);
                y -= POPOVER_GAP * scale;
                fits
            })
        };
        let mut placed = Vec::new();
        let mut bounds = Vec::new();
        let mut y = if above {
            hovered_y
        } else {
            hovered_y + line_height
        };
        for popover in measured {
            let x = origin_x(popover.width);
            let top = if above { y - popover.height } else { y };
            bounds.push(Rect::new(
                x,
                top,
                popover.width,
                popover.height,
                Rgba::TRANSPARENT,
            ));
            placed.push((popover.node, x, top));
            y = if above {
                top - POPOVER_GAP * scale
            } else {
                top + popover.height + POPOVER_GAP * scale
            };
        }
        *self.hover.bounds.borrow_mut() = bounds;
        placed
    }

    fn measure_diagnostic(
        &self,
        popover: &DiagnosticPopover,
        max_width: f32,
        max_height: f32,
    ) -> Measured {
        let scale = ui::ui_text_scale();
        let colors = theme();
        let layout = popover.layout.get(
            &popover.markdown,
            max_width,
            markdown_view::DIAGNOSTIC_STYLE,
        );
        let first = popover.scroll.min(layout.line_count().saturating_sub(1));
        let count = layout.lines_fitting(first, max_height);
        let body_height = layout.height_of(first, count);
        let (background, border) = severity_colors(popover.severity);
        let width =
            layout.width + DIAGNOSTIC_PADDING_LEFT + DIAGNOSTIC_PADDING_RIGHT + BORDER * 2.0;
        let height = body_height + DIAGNOSTIC_PADDING_Y * 2.0 + BORDER * 2.0;
        let inner = div()
            .col()
            .w_px(width)
            .pl(DIAGNOSTIC_PADDING_LEFT)
            .pr(DIAGNOSTIC_PADDING_RIGHT)
            .py(DIAGNOSTIC_PADDING_Y)
            .bg(background)
            .border(BORDER, border)
            .rounded(RADIUS)
            .child(layout.render(first, count, layout.width));
        let node = div()
            .col()
            .w_px(width)
            .rounded(RADIUS)
            .bg(colors.elevated_surface_background)
            .on_click(crate::HOVER_BASE)
            .child(inner)
            .into();
        Measured {
            node,
            width: width * scale,
            height: height * scale,
        }
    }

    fn measure_info(&self, popover: &InfoPopover, max_width: f32, max_height: f32) -> Measured {
        let scale = ui::ui_text_scale();
        let colors = theme();
        let text_width = (max_width - INFO_PADDING * 2.0).max(1.0);
        let layout = popover
            .layout
            .get(&popover.markdown, text_width, markdown_view::HOVER_STYLE);
        let first = popover.scroll.min(layout.line_count().saturating_sub(1));
        let count = layout.lines_fitting(first, (max_height - INFO_PADDING * 2.0).max(1.0));
        let body_height = layout.height_of(first, count);
        let width = layout.width + INFO_PADDING * 2.0 + BORDER * 2.0;
        let height = body_height + INFO_PADDING * 2.0 + BORDER * 2.0;
        let node = div()
            .col()
            .w_px(width)
            .p(INFO_PADDING)
            .rounded(RADIUS)
            .border(BORDER, colors.border_variant)
            .bg(colors.elevated_surface_background)
            .on_click(crate::HOVER_BASE + 1)
            .child(layout.render(first, count, layout.width))
            .into();
        Measured {
            node,
            width: width * scale,
            height: height * scale,
        }
    }

    /// The hovered symbol, tinted behind its text.
    pub(crate) fn hover_highlight_ranges(&self) -> Vec<Range<usize>> {
        self.hover
            .info
            .iter()
            .map(|popover| popover.range.clone())
            .filter(|range| !range.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp::lsp_types::DiagnosticSeverity;
    use workspace::Item;

    fn item(text: &str) -> FileItem {
        let mut item = FileItem::new(
            std::path::PathBuf::from("/nonexistent"),
            "a.rs",
            Some(text.into()),
        );
        item.set_body_height(20.0 * EDIT_LINE_H);
        item.ensure_visible();
        item.diagnostics = vec![DiagnosticEntry {
            range: 8..9,
            severity: DiagnosticSeverity::ERROR,
            message: "cannot find value `b`".into(),
            source: Some("rustc".into()),
            code: Some("E0425".into()),
        }];
        item
    }

    /// Local coordinates in the left part of char `column` on `row`, nearest the boundary before it.
    fn point(item: &FileItem, row: usize, column: usize) -> (f32, f32) {
        let x = crate::gutter_width(item.line_count()) + (column as f32 + 0.25) * char_advance();
        (x, row as f32 * EDIT_LINE_H + EDIT_LINE_H / 2.0)
    }

    fn content() -> Rect {
        Rect::new(0.0, 0.0, 1200.0, 20.0 * EDIT_LINE_H, Rgba::TRANSPARENT)
    }

    #[test]
    fn points_past_a_line_or_in_the_gutter_are_over_no_char() {
        let item = item("let a = b;\n");
        let (x, y) = point(&item, 0, 4);
        assert_eq!(item.hover_offset(x, y), Some(4));
        let (x, y) = point(&item, 0, 12);
        assert_eq!(item.hover_offset(x, y), None);
        assert_eq!(item.hover_offset(2.0, y), None);
        let (x, y) = point(&item, 5, 0);
        assert_eq!(item.hover_offset(x, y), None);
    }

    #[test]
    fn a_still_pointer_shows_the_diagnostic_then_the_server_answer() {
        let mut item = item("let a = b;\n");
        let local = point(&item, 0, 8);
        item.hover_pointer(Some(local), local);
        let started = Instant::now();
        assert!(item.hover_request_due(started).is_none());
        let half = started + HOVER_DELAY / 2;
        assert_eq!(item.hover_request_due(half), Some(8));
        item.hover_requested(Some(7));
        item.tick_hover(half);
        assert!(!item.hover.visible());
        item.tick_hover(started + HOVER_DELAY);
        assert!(item.hover.diagnostic.is_some());
        assert!(item.hover.info.is_empty());
        let synced = lsp::SyncedText {
            lsp_version: 0,
            buffer_version: item.buffer.as_ref().unwrap().version(),
            rope: item.buffer.as_ref().unwrap().rope.clone(),
        };
        item.hover_answered(&lsp::HoverResponse {
            request: 7,
            markdown: Some("```rust\nlet b: u32\n```".into()),
            range: Some(8..9),
            synced,
        });
        item.tick_hover(started + HOVER_DELAY);
        assert_eq!(item.hover.info.len(), 1);
        assert_eq!(item.hover_highlight_ranges(), vec![8..9]);
        let popovers = item.hover_popover_nodes(content());
        assert_eq!(popovers.len(), 2);
        // The anchor is on the first line, so there is no room above: they stack below it.
        assert!(popovers[0].2 >= EDIT_LINE_H);
        assert!(popovers[1].2 > popovers[0].2);
    }

    #[test]
    fn diagnostic_text_carries_source_and_code() {
        let item = item("let a = b;\n");
        assert_eq!(
            diagnostic_markdown(&item.diagnostics[0]),
            "cannot find value \\`b\\` (rustc E0425)"
        );
    }

    #[test]
    fn leaving_the_text_hides_after_a_delay_unless_heading_to_the_popover() {
        let mut item = item("let a = b;\n");
        item.show_hover(8, true);
        item.hover_requested(None);
        item.tick_hover(Instant::now());
        assert!(item.hover.diagnostic.is_some());
        // A keyboard hover stays put while the pointer wanders.
        item.hover_pointer(None, (5000.0, 5000.0));
        assert!(item.hover.hide_at.is_none());
        item.hover_clicked();
        let popovers = item.hover_popover_nodes(content());
        let (_, x, y) = &popovers[0];
        let far = (x + 600.0, y + 600.0);
        item.hover_pointer(None, far);
        assert!(item.hover.hide_at.is_some());
        item.hover_pointer(None, (x + 1.0, y + 1.0));
        assert!(item.hover.hide_at.is_none());
        item.hover_pointer(None, far);
        item.tick_hover(Instant::now() + HIDING_DELAY * 2);
        assert!(!item.hover.visible());
    }

    #[test]
    fn typing_or_scrolling_hides_and_the_keyboard_shows_at_once() {
        let mut item = item("let a = b;\n");
        item.buffer.as_mut().unwrap().place_cursor(8);
        item.input_key(workspace::EditKey::Hover, false);
        item.hover_requested(None);
        item.tick_hover(Instant::now());
        assert!(item.hover.visible());
        item.input_text("x");
        assert!(!item.hover.visible());
        assert!(item.hover.pending.is_none());
    }
}
