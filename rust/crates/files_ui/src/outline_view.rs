//! Cmd+Shift+O: a modal listing the file's symbols, nested by depth and colored like the code. Typing
//! fuzzy-filters them by their path of names (`impl Point fmt`), keeping each match's ancestors as context;
//! moving through the list previews the symbol's lines and Enter jumps there.

use std::ops::Range;

use ui::{div, label, theme, Node, Rgba};

use crate::fuzzy::fuzzy_match;
use crate::text_field::{TextField, INPUT_FONT};

pub const WIDTH: f32 = 544.0;
const HEAD_HEIGHT: f32 = 36.0;
const PLACEHOLDER: &str = "Search buffer symbols...";
const MAX_MATCHES: usize = 100;
const DEPTH_INDENT: f32 = 16.0;

/// One outline entry, positioned in chars.
#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub depth: usize,
    pub range: Range<usize>,
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
    max_height: f32,
    /// Scroll to restore when the modal closes without jumping.
    pub prev_scroll: Option<(f32, f32)>,
}

impl OutlineView {
    pub fn new(symbols: Vec<Symbol>, cursor: usize, scroll: (f32, f32), max_height: f32) -> Self {
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
            max_height,
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

    fn visible_rows(&self) -> usize {
        let list = self.max_height - HEAD_HEIGHT - 8.0;
        (list / Self::row_height()).floor().max(1.0) as usize
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

    pub fn render(&self, id_base: u64) -> Node {
        let colors = theme();
        let head = div()
            .row()
            .items_center()
            .h_px(HEAD_HEIGHT)
            .px(10.0)
            .child(
                self.field
                    .render(PLACEHOLDER, true, colors.text, INPUT_FONT * 1.6),
            );
        let list: Node = if self.entries.is_empty() {
            div()
                .col()
                .py(8.0)
                .child(
                    div().row().px(4.0).child(
                        div().row().flex(1.0).px(6.0).py(4.0).child(
                            label("No matches")
                                .label_size(ui::LabelSize::Default)
                                .color(colors.text_muted),
                        ),
                    ),
                )
                .into()
        } else {
            let end = (self.scroll_top + self.visible_rows()).min(self.entries.len());
            div()
                .col()
                .py(4.0)
                .children((self.scroll_top..end).map(|row| self.render_row(row, id_base)))
                .into()
        };
        div()
            .col()
            .w_px(WIDTH)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .bg(colors.elevated_surface_background)
            .child(head)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(list)
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
            .on_click(id_base + row as u64);
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
            600.0,
        )
    }

    fn listed(view: &OutlineView) -> Vec<(&str, Vec<usize>)> {
        view.entries
            .iter()
            .map(|e| (view.symbols[e.symbol].text.as_str(), e.positions.clone()))
            .collect()
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
