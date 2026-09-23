//! Cmd+Shift+O: a modal listing the file's symbols, nested by depth and colored like the code. Typing
//! fuzzy-filters them by their path of names (`impl Point fmt`), keeping each match's ancestors as context;
//! moving through the list previews the symbol's lines and Enter jumps there.

use std::ops::Range;

use ui::{div, icon, label, theme, IconKind, Node, Rgba};

use crate::command_palette::footer_button;
use crate::fuzzy::fuzzy_match;
use crate::text_field::{FieldFont, TextField, INPUT_FONT};

/// Width with the preview hidden; showing it grows the picker to a share of the window.
pub const WIDTH: f32 = 544.0;
const HEAD_HEIGHT: f32 = 36.0;
const FOOTER_HEIGHT: f32 = 35.0;
const LIST_PADDING: f32 = 8.0;
const PREVIEW_KEY: &str = "cmd-alt-p";
const PLACEHOLDER: &str = "Search buffer symbols...";
const MAX_MATCHES: usize = 100;
const DEPTH_INDENT: f32 = 16.0;

/// Where the code preview sits; hidden by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewLayout {
    #[default]
    Hidden,
    Right,
    Below,
}

/// Click targets inside the outline, as offsets from the view's id base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineClick {
    TogglePreview,
    PreviewBelow,
    PreviewRight,
    Row(usize),
}

impl OutlineClick {
    pub fn offset(self) -> u64 {
        match self {
            OutlineClick::TogglePreview => 0,
            OutlineClick::PreviewBelow => 1,
            OutlineClick::PreviewRight => 2,
            OutlineClick::Row(row) => row as u64 + 3,
        }
    }

    pub fn from_offset(offset: u64) -> Self {
        match offset {
            0 => OutlineClick::TogglePreview,
            1 => OutlineClick::PreviewBelow,
            2 => OutlineClick::PreviewRight,
            n => OutlineClick::Row((n - 3) as usize),
        }
    }
}

/// One line of the code preview: its number, colored text (already scrolled sideways and cut to the pane),
/// whether it belongs to the symbol, and the columns of the symbol's name on it.
pub struct PreviewRow {
    pub number: usize,
    pub segments: Vec<(String, Rgba)>,
    pub in_symbol: bool,
    pub name_columns: Option<Range<usize>>,
}

pub struct PreviewContent {
    pub rows: Vec<PreviewRow>,
    pub gutter_width: f32,
}

/// The picker's size for a layout, in design px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickerSize {
    pub width: f32,
    /// Fixed total height with a preview; without one the picker shrinks to its rows.
    pub height: Option<f32>,
    /// The results column (head, list, footer).
    pub results: (f32, f32),
    pub preview: (f32, f32),
    pub list_height: f32,
}

/// One outline entry, positioned in chars.
#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub depth: usize,
    pub range: Range<usize>,
    /// Where its name sits.
    pub name: Range<usize>,
    pub text: String,
    /// Byte ranges of `text` and their syntax colors.
    pub colors: Vec<(Range<usize>, Rgba)>,
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    symbol: usize,
    /// Char indices into the symbol's text that matched the query.
    positions: Vec<usize>,
    score: f64,
}

pub struct OutlineView {
    pub field: TextField,
    symbols: Vec<Symbol>,
    /// Each symbol's text prefixed by its ancestors', for searching by nesting.
    paths: Vec<String>,
    /// Where each path's own symbol text starts, in chars.
    leaf_offsets: Vec<usize>,
    entries: Vec<Entry>,
    selected: usize,
    scroll_top: usize,
    cursor: usize,
    /// The window size in design px, which the picker and its preview size themselves by.
    viewport: (f32, f32),
    pub preview: PreviewLayout,
    /// Scroll to restore when the modal closes without jumping.
    pub prev_scroll: Option<(f32, f32)>,
}

impl OutlineView {
    pub fn new(
        symbols: Vec<Symbol>,
        cursor: usize,
        scroll: (f32, f32),
        viewport: (f32, f32),
        preview: PreviewLayout,
    ) -> Self {
        let mut paths = Vec::with_capacity(symbols.len());
        let mut leaf_offsets = Vec::with_capacity(symbols.len());
        let mut path = String::new();
        let mut ends: Vec<usize> = Vec::new();
        for symbol in &symbols {
            if symbol.depth < ends.len() {
                ends.truncate(symbol.depth);
                path.truncate(ends.last().copied().unwrap_or(0));
            }
            if !path.is_empty() {
                path.push(' ');
            }
            leaf_offsets.push(path.chars().count());
            path.push_str(&symbol.text);
            ends.push(path.len());
            paths.push(path.clone());
        }
        let mut view = Self {
            field: TextField::default(),
            symbols,
            paths,
            leaf_offsets,
            entries: Vec::new(),
            selected: 0,
            scroll_top: 0,
            cursor,
            viewport,
            preview,
            prev_scroll: Some(scroll),
        };
        view.update_matches();
        view
    }

    pub fn query_is_empty(&self) -> bool {
        self.field.text().trim_start().is_empty()
    }

    /// Refilter for the current query. An empty query lists everything with the innermost symbol around the
    /// caret selected; otherwise the best-scoring real match is.
    pub fn update_matches(&mut self) {
        let query = self.field.text();
        let query = query.trim_start();
        if query.is_empty() {
            self.entries = (0..self.symbols.len())
                .map(|symbol| Entry {
                    symbol,
                    positions: Vec::new(),
                    score: 0.0,
                })
                .collect();
            self.selected = self
                .symbols
                .iter()
                .enumerate()
                .filter(|(_, s)| s.range.contains(&self.cursor))
                .max_by_key(|(index, s)| (s.depth, std::cmp::Reverse(*index)))
                .map_or(0, |(index, _)| index);
        } else {
            self.entries = self.search(query);
            self.selected = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| !entry.positions.is_empty())
                .max_by(|(a_index, a), (b_index, b)| {
                    a.score.total_cmp(&b.score).then(b_index.cmp(a_index))
                })
                .map_or(0, |(index, _)| index);
        }
        self.scroll_to_selected();
    }

    fn search(&self, query: &str) -> Vec<Entry> {
        let names: Vec<&str> = self.paths.iter().map(String::as_str).collect();
        let mut matches = fuzzy_match(&names, query);
        matches.truncate(MAX_MATCHES);
        matches.sort_by_key(|m| m.candidate);
        // One word must match inside the symbol's own name; several words may also match its ancestors, which
        // then show as context rows with nothing highlighted.
        let single_word = !query.contains(char::is_whitespace);
        let mut kept = Vec::with_capacity(matches.len());
        for mut found in matches {
            let leaf = self.leaf_offsets.get(found.candidate).copied().unwrap_or(0);
            let total = found.positions.len();
            found.positions.retain(|position| *position >= leaf);
            if single_word && found.positions.len() != total {
                continue;
            }
            if found.positions.is_empty() {
                found.score = 0.0;
            }
            for position in &mut found.positions {
                *position -= leaf;
            }
            kept.push(Entry {
                symbol: found.candidate,
                positions: found.positions,
                score: found.score,
            });
        }
        self.with_ancestors(kept)
    }

    /// Insert each match's not-yet-listed ancestors above it, so the tree shape stays readable.
    fn with_ancestors(&self, matches: Vec<Entry>) -> Vec<Entry> {
        let depth_at = |index: usize| self.symbols.get(index).map_or(0, |s| s.depth);
        let mut out = Vec::with_capacity(matches.len());
        let mut next_unlisted = 0;
        for entry in matches {
            let insert_at = out.len();
            let mut depth = depth_at(entry.symbol);
            for index in (next_unlisted..entry.symbol).rev() {
                if depth == 0 {
                    break;
                }
                if depth_at(index) == depth - 1 {
                    out.insert(
                        insert_at,
                        Entry {
                            symbol: index,
                            positions: Vec::new(),
                            score: 0.0,
                        },
                    );
                    depth -= 1;
                }
            }
            next_unlisted = entry.symbol + 1;
            out.push(entry);
        }
        out
    }

    pub fn selected_symbol(&self) -> Option<&Symbol> {
        let entry = self.entries.get(self.selected)?;
        self.symbols.get(entry.symbol)
    }

    pub fn select_next(&mut self) {
        let count = self.entries.len();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
            self.scroll_to_selected();
        }
    }

    pub fn select_previous(&mut self) {
        let count = self.entries.len();
        if count > 0 {
            self.selected = (self.selected + count - 1) % count;
            self.scroll_to_selected();
        }
    }

    pub fn select_row(&mut self, row: usize) {
        if row < self.entries.len() {
            self.selected = row;
        }
    }

    fn row_height() -> f32 {
        crate::EDIT_FONT * ui::ui_text_scale() * 1.4 + 10.0
    }

    /// Resize for a window of `viewport` real px.
    pub fn set_viewport(&mut self, viewport: (f32, f32)) {
        let scale = ui::ui_text_scale();
        self.viewport = (viewport.0 / scale, viewport.1 / scale);
        self.scroll_to_selected();
    }

    pub fn set_preview(&mut self, preview: PreviewLayout) {
        self.preview = preview;
        self.scroll_to_selected();
    }

    /// Hidden: the standard width, rows up to three quarters of the window. Shown: 60% of the window each way
    /// (never past the space left under the modal's top offset), the preview taking 30% of the window.
    pub fn size(&self) -> PickerSize {
        let (window_w, window_h) = self.viewport;
        let header_chrome = HEAD_HEIGHT + 1.0 + FOOTER_HEIGHT + LIST_PADDING;
        if self.preview == PreviewLayout::Hidden {
            let list_height = window_h * 0.75;
            return PickerSize {
                width: WIDTH,
                height: None,
                results: (WIDTH, list_height + header_chrome),
                preview: (0.0, 0.0),
                list_height,
            };
        }
        let max_height = ((window_h - 160.0) * 0.95).max(320.0);
        let width = (window_w * 0.6).min(window_w * 0.95).max(280.0 + 128.0);
        let height = (window_h * 0.6).clamp(320.0, max_height);
        let (results, preview) = if self.preview == PreviewLayout::Right {
            let preview_w = (window_w * 0.3).clamp(128.0, width - 280.0);
            ((width - preview_w, height), (preview_w, height))
        } else {
            let preview_h = (window_h * 0.3).clamp(96.0, height - 160.0);
            ((width, height - preview_h), (width, preview_h))
        };
        PickerSize {
            width,
            height: Some(height),
            results,
            preview,
            list_height: (results.1 - header_chrome).max(Self::row_height()),
        }
    }

    fn visible_rows(&self) -> usize {
        (self.size().list_height / Self::row_height())
            .floor()
            .max(1.0) as usize
    }

    fn scroll_to_selected(&mut self) {
        let visible = self.visible_rows();
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        } else if self.selected >= self.scroll_top + visible {
            self.scroll_top = self.selected + 1 - visible;
        }
        self.scroll_top = self
            .scroll_top
            .min(self.entries.len().saturating_sub(visible));
    }

    /// Code lines the preview pane has room for, and columns across.
    pub fn preview_capacity(&self, gutter_width: f32) -> (usize, usize) {
        let (width, height) = self.size().preview;
        let rows = (height / crate::EDIT_LINE_H).floor().max(1.0) as usize;
        let columns = ((width - gutter_width) / crate::char_advance())
            .floor()
            .max(1.0) as usize;
        (rows, columns)
    }

    pub fn render(&self, id_base: u64, preview: Option<PreviewContent>) -> Node {
        let colors = theme();
        let size = self.size();
        let results = self.render_results(id_base, &size);
        let mut root = div()
            .col()
            .w_px(size.width)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .bg(colors.elevated_surface_background);
        match (self.preview, size.height) {
            (PreviewLayout::Right, Some(height)) => {
                root = root.h_px(height).child(
                    div()
                        .row()
                        .flex(1.0)
                        .child(results)
                        .child(div().w_px(1.0).bg(colors.border_variant))
                        .child(render_preview(preview, size.preview)),
                );
            }
            (PreviewLayout::Below, Some(height)) => {
                root = root
                    .h_px(height)
                    .child(results)
                    .child(div().h_px(1.0).bg(colors.border_variant))
                    .child(render_preview(preview, size.preview));
            }
            _ => root = root.child(results),
        }
        root.into()
    }

    fn render_results(&self, id_base: u64, size: &PickerSize) -> Node {
        let colors = theme();
        let head = div()
            .row()
            .items_center()
            .h_px(HEAD_HEIGHT)
            .px(10.0)
            .child(self.field.render(
                PLACEHOLDER,
                true,
                colors.editor_foreground,
                INPUT_FONT * FieldFont::Ui.line_height(),
                FieldFont::Ui,
            ));
        let mut list = if self.entries.is_empty() {
            div().col().py(8.0).child(
                div().row().px(4.0).child(
                    div().row().flex(1.0).px(6.0).py(4.0).child(
                        label("No matches")
                            .label_size(ui::LabelSize::Default)
                            .color(colors.text_muted),
                    ),
                ),
            )
        } else {
            let end = (self.scroll_top + self.visible_rows()).min(self.entries.len());
            div()
                .col()
                .py(4.0)
                .children((self.scroll_top..end).map(|row| self.render_row(row, id_base)))
        };
        let fixed = size.height.is_some();
        if fixed {
            list = list.flex(1.0);
        }
        let mut column = div()
            .col()
            .child(head)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(list)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(self.render_footer(id_base));
        if fixed {
            column = column.w_px(size.results.0).h_px(size.results.1);
        }
        column.into()
    }

    fn render_footer(&self, id_base: u64) -> Node {
        let colors = theme();
        let visible = self.preview != PreviewLayout::Hidden;
        let text_color = if visible {
            colors.text_accent
        } else {
            colors.text
        };
        let mut controls = div().row().items_center().child(footer_button(
            id_base + OutlineClick::TogglePreview.offset(),
            "Preview",
            PREVIEW_KEY,
            text_color,
            false,
        ));
        if visible {
            let layout_button = |click: OutlineClick, kind: IconKind, selected: bool| -> Node {
                let mut button = div()
                    .row()
                    .items_center()
                    .justify_center()
                    .w_px(22.0)
                    .h_px(22.0)
                    .rounded(4.0)
                    .on_click(id_base + click.offset())
                    .child(icon(kind).size(14.0).color(colors.icon));
                if selected {
                    button = button.bg(colors.element_selected);
                }
                button.into()
            };
            controls = controls
                .child(div().w_px(1.0).h_px(16.0).bg(colors.border_variant))
                .child(div().w_px(4.0))
                .child(layout_button(
                    OutlineClick::PreviewBelow,
                    IconKind::DiffUnified,
                    self.preview == PreviewLayout::Below,
                ))
                .child(layout_button(
                    OutlineClick::PreviewRight,
                    IconKind::DiffSplit,
                    self.preview == PreviewLayout::Right,
                ));
        }
        div()
            .row()
            .justify_between()
            .items_center()
            .p(6.0)
            .child(controls)
            .into()
    }

    fn render_row(&self, row: usize, id_base: u64) -> Node {
        let colors = theme();
        let Some((entry, symbol)) = self
            .entries
            .get(row)
            .and_then(|entry| Some((entry, self.symbols.get(entry.symbol)?)))
        else {
            return div().into();
        };
        let mut item = div()
            .row()
            .flex(1.0)
            .px(6.0)
            .py(4.0)
            .rounded(4.0)
            .on_click(id_base + OutlineClick::Row(row).offset());
        if row == self.selected {
            item = item.bg(colors.element_selected);
        }
        let text = div()
            .row()
            .items_center()
            .pl(symbol.depth as f32 * DEPTH_INDENT)
            .children(label_segments(symbol, &entry.positions));
        div().row().px(4.0).child(item.child(text)).into()
    }
}

/// A read-only look at the code around the selected symbol: line numbers, its lines tinted like the active
/// line, its name marked like a search match.
fn render_preview(content: Option<PreviewContent>, (width, height): (f32, f32)) -> Node {
    let colors = theme();
    let pane = div()
        .col()
        .w_px(width)
        .h_px(height)
        .bg(colors.editor_background);
    let Some(content) = content else {
        return pane
            .p(8.0)
            .child(
                label("No results to preview")
                    .label_size(ui::LabelSize::Default)
                    .color(colors.text_muted),
            )
            .into();
    };
    let rows = content.rows.into_iter().map(|row| {
        let number_color = if row.in_symbol {
            colors.editor_active_line_number
        } else {
            colors.editor_line_number
        };
        let mut line = div()
            .row()
            .items_center()
            .h_px(crate::EDIT_LINE_H)
            .child(
                div()
                    .row()
                    .items_center()
                    .justify_end()
                    .w_px(content.gutter_width)
                    .pr(8.0)
                    .child(
                        label(row.number.to_string())
                            .size(crate::EDIT_FONT)
                            .mono()
                            .color(number_color),
                    ),
            )
            .children(preview_segments(row.segments, row.name_columns));
        if row.in_symbol {
            line = line.bg(colors.editor_active_line);
        }
        Node::from(line)
    });
    pane.children(rows).into()
}

/// Split colored runs at the name's columns so it can sit on the match background.
fn preview_segments(segments: Vec<(String, Rgba)>, name: Option<Range<usize>>) -> Vec<Node> {
    let highlight = theme().search_match_background;
    let mut out = Vec::new();
    let mut column = 0;
    for (text, color) in segments {
        let mut run = String::new();
        let mut run_marked = false;
        for c in text.chars() {
            let marked = name.as_ref().is_some_and(|n| n.contains(&column));
            if marked != run_marked && !run.is_empty() {
                out.push(preview_run(
                    std::mem::take(&mut run),
                    color,
                    run_marked,
                    highlight,
                ));
            }
            run_marked = marked;
            run.push(c);
            column += 1;
        }
        if !run.is_empty() {
            out.push(preview_run(run, color, run_marked, highlight));
        }
    }
    out
}

fn preview_run(text: String, color: Rgba, marked: bool, highlight: Rgba) -> Node {
    let text = label(text).size(crate::EDIT_FONT).mono().color(color);
    if marked {
        div().row().bg(highlight).child(text).into()
    } else {
        text.into()
    }
}

/// The symbol's text in runs of one syntax color, matched chars on an accent background.
fn label_segments(symbol: &Symbol, positions: &[usize]) -> Vec<Node> {
    let default = theme().text;
    let highlight = theme().text_accent.alpha(0.3);
    let color_at = |byte: usize| {
        symbol
            .colors
            .iter()
            .find(|(range, _)| range.contains(&byte))
            .map_or(default, |(_, color)| *color)
    };
    let mut runs: Vec<(String, Rgba, bool)> = Vec::new();
    for (index, (byte, c)) in symbol.text.char_indices().enumerate() {
        let color = color_at(byte);
        let matched = positions.binary_search(&index).is_ok();
        match runs.last_mut() {
            Some((text, last_color, last_matched))
                if *last_color == color && *last_matched == matched =>
            {
                text.push(c)
            }
            _ => runs.push((c.to_string(), color, matched)),
        }
    }
    runs.into_iter()
        .map(|(text, color, matched)| {
            let text = label(text).size(crate::EDIT_FONT).mono().color(color);
            if matched {
                div().row().bg(highlight).child(text).into()
            } else {
                text.into()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(depth: usize, range: Range<usize>, text: &str) -> Symbol {
        Symbol {
            depth,
            name: range.clone(),
            range,
            text: text.to_string(),
            colors: Vec::new(),
        }
    }

    fn view(cursor: usize) -> OutlineView {
        OutlineView::new(
            vec![
                symbol(0, 0..50, "struct Point"),
                symbol(1, 10..20, "x"),
                symbol(0, 60..120, "impl Point"),
                symbol(1, 70..90, "fn distance"),
                symbol(1, 95..110, "fn scale"),
                symbol(0, 130..150, "fn main"),
            ],
            cursor,
            (0.0, 0.0),
            (1200.0, 800.0),
            PreviewLayout::Hidden,
        )
    }

    fn listed(view: &OutlineView) -> Vec<(&str, Vec<usize>)> {
        view.entries
            .iter()
            .map(|e| (view.symbols[e.symbol].text.as_str(), e.positions.clone()))
            .collect()
    }

    #[test]
    fn preview_layouts_size_the_picker_from_the_window() {
        let round = |v: f32| v.round();
        let pair = |(a, b): (f32, f32)| (round(a), round(b));
        let mut view = view(0);
        view.set_viewport((1200.0 * ui::ui_text_scale(), 800.0 * ui::ui_text_scale()));
        let hidden = view.size();
        assert_eq!((round(hidden.width), hidden.height), (WIDTH, None));
        assert_eq!(round(hidden.list_height), 600.0);
        view.set_preview(PreviewLayout::Right);
        let right = view.size();
        assert_eq!(round(right.width), 720.0);
        assert_eq!(right.height.map(round), Some(480.0));
        assert_eq!(pair(right.preview), (360.0, 480.0));
        assert_eq!(pair(right.results), (360.0, 480.0));
        view.set_preview(PreviewLayout::Below);
        let below = view.size();
        assert_eq!(pair(below.preview), (720.0, 240.0));
        assert_eq!(pair(below.results), (720.0, 240.0));
    }

    #[test]
    fn clicks_round_trip() {
        for click in [
            OutlineClick::TogglePreview,
            OutlineClick::PreviewBelow,
            OutlineClick::PreviewRight,
            OutlineClick::Row(7),
        ] {
            assert_eq!(OutlineClick::from_offset(click.offset()), click);
        }
    }

    #[test]
    fn empty_query_selects_the_innermost_symbol_around_the_caret() {
        let view = view(80);
        assert_eq!(view.entries.len(), 6);
        assert_eq!(
            view.selected_symbol().map(|s| s.text.as_str()),
            Some("fn distance")
        );
        assert_eq!(
            self::view(200).selected_symbol().map(|s| s.text.as_str()),
            Some("struct Point")
        );
    }

    #[test]
    fn a_single_word_matches_names_and_keeps_ancestors_as_context() {
        let mut view = view(0);
        view.field.insert("scale");
        view.update_matches();
        assert_eq!(
            listed(&view),
            vec![("impl Point", vec![]), ("fn scale", vec![3, 4, 5, 6, 7])]
        );
        assert_eq!(
            view.selected_symbol().map(|s| s.text.as_str()),
            Some("fn scale")
        );
    }

    #[test]
    fn several_words_can_match_through_the_parent() {
        let mut view = view(0);
        view.field.insert("impl dist");
        view.update_matches();
        let texts: Vec<&str> = listed(&view).into_iter().map(|(t, _)| t).collect();
        assert!(texts.contains(&"fn distance"));
        assert_eq!(
            view.selected_symbol().map(|s| s.text.as_str()),
            Some("fn distance")
        );
    }
}
