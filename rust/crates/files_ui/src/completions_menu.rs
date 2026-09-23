//! The menu of words completing the one being typed, floating at the caret.

use ui::{div, label, theme, Node};

use crate::fuzzy::{fuzzy_match, Match};
use crate::list_scrollbar::{self, ScrollbarReveal};

pub const WIDTH: f32 = 544.0;
pub const MAX_VISIBLE: usize = 12;
/// Popover padding above and below the rows together.
const PADDING_Y: f32 = 8.0;

pub struct CompletionsMenu {
    candidates: Vec<String>,
    matches: Vec<Match>,
    pub selected: usize,
    scroll_top: usize,
    pub scrollbar: ScrollbarReveal,
    pub hovered: Option<u64>,
}

impl CompletionsMenu {
    /// `None` when nothing matches `query`.
    pub fn new(query: &str, candidates: Vec<String>) -> Option<Self> {
        let mut menu = Self {
            candidates,
            matches: Vec::new(),
            selected: 0,
            scroll_top: 0,
            scrollbar: ScrollbarReveal::default(),
            hovered: None,
        };
        menu.filter(query);
        (!menu.matches.is_empty()).then_some(menu)
    }

    fn filter(&mut self, query: &str) {
        let names: Vec<&str> = self.candidates.iter().map(String::as_str).collect();
        let mut matches = fuzzy_match(&names, query);
        let first = query.chars().next().and_then(|c| c.to_lowercase().next());
        let candidates = &self.candidates;
        // Words where the query's first letter starts one of their parts come first; among them an exact
        // match, then score, earlier match positions, and more exact-case letters decide.
        matches.sort_by(|a, b| {
            let tier = |m: &Match| {
                let word = candidates.get(m.candidate).map_or("", String::as_str);
                first.is_some_and(|first| {
                    editor::completion::split_words(word).any(|part| {
                        part.chars().next().and_then(|c| c.to_lowercase().next()) == Some(first)
                    })
                })
            };
            let exact = |m: &Match| candidates.get(m.candidate).is_some_and(|w| w == query);
            let exact_case = |m: &Match| {
                let word: Vec<char> = candidates
                    .get(m.candidate)
                    .map_or_else(Vec::new, |w| w.chars().collect());
                query
                    .chars()
                    .zip(&m.positions)
                    .filter(|(q, p)| word.get(**p) == Some(q))
                    .count()
            };
            let label = |m: &Match| candidates.get(m.candidate).cloned().unwrap_or_default();
            match (tier(a), tier(b)) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (false, false) => b.score.total_cmp(&a.score),
                (true, true) => exact(b)
                    .cmp(&exact(a))
                    .then(b.score.total_cmp(&a.score))
                    .then_with(|| a.positions.cmp(&b.positions))
                    .then_with(|| exact_case(b).cmp(&exact_case(a)))
                    .then_with(|| label(a).cmp(&label(b))),
            }
        });
        self.matches = matches;
        self.selected = 0;
        self.scroll_top = 0;
    }

    pub fn selected_word(&self) -> Option<&str> {
        let found = self.matches.get(self.selected)?;
        self.candidates.get(found.candidate).map(String::as_str)
    }

    pub fn word_at(&self, row: usize) -> Option<&str> {
        let found = self.matches.get(row)?;
        self.candidates.get(found.candidate).map(String::as_str)
    }

    pub fn select_next(&mut self) {
        let count = self.matches.len();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
            self.scroll_to_selected();
        }
    }

    pub fn select_previous(&mut self) {
        let count = self.matches.len();
        if count > 0 {
            self.selected = (self.selected + count - 1) % count;
            self.scroll_to_selected();
        }
    }

    fn scroll_to_selected(&mut self) {
        let before = self.scroll_top;
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        } else if self.selected >= self.scroll_top + MAX_VISIBLE {
            self.scroll_top = self.selected + 1 - MAX_VISIBLE;
        }
        if self.scroll_top != before {
            self.scrollbar.reveal();
        }
    }

    pub fn scroll_by(&mut self, dy: f32) -> bool {
        let max_top = self.matches.len().saturating_sub(MAX_VISIBLE);
        let mut remainder = 0.0;
        let moved = crate::command_palette::scroll_rows(
            &mut self.scroll_top,
            &mut remainder,
            dy,
            crate::EDIT_LINE_H * ui::ui_text_scale(),
            max_top,
        );
        if moved {
            self.scrollbar.reveal();
        }
        moved
    }

    fn visible(&self) -> usize {
        self.matches.len().min(MAX_VISIBLE)
    }

    /// Height in design px.
    pub fn height(&self) -> f32 {
        self.visible() as f32 * crate::EDIT_LINE_H + PADDING_Y
    }

    /// Rows in the code font, matched letters bold, the selection tinted; row clicks are `id_base + row`.
    pub fn render(&self, id_base: u64) -> Node {
        let colors = theme();
        let end = (self.scroll_top + MAX_VISIBLE).min(self.matches.len());
        let rows = (self.scroll_top..end).map(|row| {
            let mut item = div()
                .row()
                .flex(1.0)
                .items_center()
                .h_px(crate::EDIT_LINE_H)
                .px(6.0)
                .rounded(4.0)
                .on_click(id_base + row as u64);
            if self.hovered == Some(id_base + row as u64) {
                item = item.bg(colors.ghost_element_hover);
            } else if row == self.selected {
                item = item.bg(colors.element_selected);
            }
            let (word, positions) = self
                .matches
                .get(row)
                .and_then(|m| Some((self.candidates.get(m.candidate)?, &m.positions)))
                .map_or(("", &[][..]), |(w, p)| (w.as_str(), p.as_slice()));
            let mut run = String::new();
            let mut run_bold = false;
            for (index, c) in word.chars().enumerate() {
                let bold = positions.binary_search(&index).is_ok();
                if bold != run_bold && !run.is_empty() {
                    item = item.child(word_run(std::mem::take(&mut run), run_bold));
                }
                run_bold = bold;
                run.push(c);
            }
            if !run.is_empty() {
                item = item.child(word_run(run, run_bold));
            }
            Node::from(div().row().px(4.0).child(item))
        });
        let scrollbar = list_scrollbar::render(
            WIDTH - 1.0,
            self.height(),
            self.scroll_top,
            self.visible(),
            self.matches.len(),
            self.scrollbar.opacity(),
        );
        div()
            .col()
            .w_px(WIDTH)
            .py(PADDING_Y / 2.0)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .bg(colors.elevated_surface_background)
            .children(scrollbar)
            .children(rows)
            .into()
    }
}

fn word_run(text: String, bold: bool) -> Node {
    let run = label(text)
        .size(crate::EDIT_FONT)
        .mono()
        .color(theme().editor_foreground);
    if bold {
        run.weight(700).into()
    } else {
        run.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn word_start_matches_rank_first_then_exact_and_case() {
        let menu = CompletionsMenu::new("bar", words(&["xbarx", "barrel", "fooBar", "bar", "Bar"]))
            .unwrap();
        let order: Vec<&str> = (0..5).filter_map(|i| menu.word_at(i)).collect();
        assert_eq!(order[0], "bar");
        assert_eq!(*order.last().unwrap(), "xbarx");
    }

    #[test]
    fn no_match_means_no_menu_and_selection_wraps() {
        assert!(CompletionsMenu::new("zzz", words(&["abc"])).is_none());
        let mut menu = CompletionsMenu::new("a", words(&["ab", "ac"])).unwrap();
        menu.select_previous();
        assert_eq!(menu.selected, 1);
        menu.select_next();
        assert_eq!(menu.selected, 0);
    }
}
