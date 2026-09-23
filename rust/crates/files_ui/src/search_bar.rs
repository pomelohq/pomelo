//! In-file find/replace bar shown in a pane's toolbar: a query field with case/word/regex toggles, match
//! navigation with an "n/m" count, and an optional replace row.

use std::ops::Range;

use crate::text_field::TextField;
use editor::search::{
    active_match_index, match_index_for_direction, Direction, SearchOptions, SearchQuery,
};
use ui::{div, icon, label, theme, IconKind, Node};
use workspace::EditKey;

use crate::text_field::INPUT_FONT;
const INPUT_H: f32 = 32.0;
const BUTTON: f32 = 20.0;
const BUTTON_ICON: f32 = 16.0;
const TOOLBAR_PAD_X: f32 = 8.0;
const TOOLBAR_PAD_Y: f32 = 6.0;
const LINE_GAP: f32 = 8.0;
const MAX_HISTORY: usize = 50;

/// Click targets inside the bar, offset from the pane's base id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchClick {
    Query,
    Replacement,
    CaseSensitive,
    WholeWord,
    Regex,
    ToggleReplace,
    SelectAll,
    Previous,
    Next,
    Close,
    ReplaceNext,
    ReplaceAll,
}

const CLICKS: [SearchClick; 12] = [
    SearchClick::Query,
    SearchClick::Replacement,
    SearchClick::CaseSensitive,
    SearchClick::WholeWord,
    SearchClick::Regex,
    SearchClick::ToggleReplace,
    SearchClick::SelectAll,
    SearchClick::Previous,
    SearchClick::Next,
    SearchClick::Close,
    SearchClick::ReplaceNext,
    SearchClick::ReplaceAll,
];

impl SearchClick {
    pub fn id(self, base: u64) -> u64 {
        base + CLICKS.iter().position(|c| *c == self).unwrap_or(0) as u64
    }

    pub fn from_offset(offset: u64) -> Option<Self> {
        CLICKS.get(offset as usize).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchField {
    Query,
    Replacement,
}

pub struct SearchBar {
    pub dismissed: bool,
    pub query: TextField,
    pub replacement: TextField,
    pub replace_enabled: bool,
    pub options: SearchOptions,
    pub focus: Option<SearchField>,
    pub matches: Vec<Range<usize>>,
    pub active_match: Option<usize>,
    /// What the matches were computed from: buffer version, query text and options.
    searched: Option<(u64, String, SearchOptions)>,
    pub error: Option<String>,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl Default for SearchBar {
    fn default() -> Self {
        Self {
            dismissed: true,
            query: TextField::default(),
            replacement: TextField::default(),
            replace_enabled: false,
            options: SearchOptions::default(),
            focus: None,
            matches: Vec::new(),
            active_match: None,
            searched: None,
            error: None,
            history: Vec::new(),
            history_index: None,
        }
    }
}

/// What the bar needs from the searched item.
pub trait Searchable {
    fn search_version(&self) -> u64;
    fn find(&self, query: &SearchQuery) -> Vec<Range<usize>>;
    fn query_suggestion(&self) -> String;
    /// The caret to measure "next" from, when there's a single selection.
    fn single_cursor(&self) -> Option<usize>;
    fn activate_match(&mut self, range: Range<usize>);
    fn select_matches(&mut self, ranges: &[Range<usize>]);
    fn replace_match(&mut self, query: &SearchQuery, range: Range<usize>);
    fn replace_all(&mut self, query: &SearchQuery, ranges: &[Range<usize>]);
    fn set_search_highlights(&mut self, matches: Vec<Range<usize>>, active: Option<usize>);
}

impl SearchBar {
    pub fn height(&self) -> f32 {
        if self.dismissed {
            return 0.0;
        }
        let mut h = 2.0 * TOOLBAR_PAD_Y + INPUT_H + 1.0;
        if self.error.is_some() {
            h += LINE_GAP + INPUT_FONT;
        }
        if self.replace_enabled {
            h += LINE_GAP + INPUT_H;
        }
        h
    }

    fn query_for(&self, with_replacement: bool) -> Option<SearchQuery> {
        let text = self.query.text();
        if text.is_empty() {
            return None;
        }
        let replacement = with_replacement.then(|| self.replacement.text());
        SearchQuery::new(&text, self.options, replacement).ok()
    }

    /// Cmd+F: show the bar seeded from the selection or the word at the caret, with the query selected.
    pub fn deploy(&mut self, item: &mut dyn Searchable, replace: bool) {
        self.dismissed = false;
        let suggestion = item.query_suggestion();
        if !suggestion.is_empty() {
            self.query.set_text(&suggestion);
        } else {
            self.query.select_all();
        }
        self.replace_enabled |= replace;
        self.focus = Some(if replace && !suggestion.is_empty() {
            SearchField::Replacement
        } else {
            SearchField::Query
        });
        self.searched = None;
        self.refresh(item);
        self.activate_current(item);
    }

    pub fn dismiss(&mut self, item: &mut dyn Searchable) {
        self.dismissed = true;
        self.focus = None;
        self.error = None;
        self.replace_enabled = false;
        self.matches.clear();
        self.active_match = None;
        self.searched = None;
        item.set_search_highlights(Vec::new(), None);
    }

    /// Recompute matches when the text, query or options changed since the last search.
    pub fn refresh(&mut self, item: &mut dyn Searchable) {
        if self.dismissed {
            return;
        }
        let key = (item.search_version(), self.query.text(), self.options);
        if self.searched.as_ref() == Some(&key) {
            return;
        }
        self.searched = Some(key.clone());
        self.error = None;
        if key.1.is_empty() {
            self.matches.clear();
            self.active_match = None;
        } else {
            match SearchQuery::new(&key.1, self.options, None) {
                Ok(query) => {
                    self.matches = item.find(&query);
                    let cursor = item.single_cursor().unwrap_or(0);
                    self.active_match = active_match_index(Direction::Next, &self.matches, cursor);
                }
                Err(error) => {
                    self.error = Some(error);
                    self.matches.clear();
                    self.active_match = None;
                }
            }
        }
        item.set_search_highlights(self.matches.clone(), self.active_match);
    }

    fn activate_current(&mut self, item: &mut dyn Searchable) {
        if let Some(index) = self.active_match {
            if let Some(range) = self.matches.get(index).cloned() {
                item.set_search_highlights(self.matches.clone(), Some(index));
                item.activate_match(range);
            }
        }
    }

    pub fn select_match(&mut self, item: &mut dyn Searchable, direction: Direction) {
        self.refresh(item);
        let Some(current) = self.active_match else {
            return;
        };
        let index =
            match_index_for_direction(&self.matches, current, direction, 1, item.single_cursor());
        self.active_match = Some(index);
        self.activate_current(item);
        self.remember_query();
    }

    pub fn select_all_matches(&mut self, item: &mut dyn Searchable) {
        self.refresh(item);
        if !self.matches.is_empty() {
            item.select_matches(&self.matches);
        }
    }

    pub fn replace_next(&mut self, item: &mut dyn Searchable) {
        self.refresh(item);
        let (Some(index), Some(query)) = (self.active_match, self.query_for(true)) else {
            return;
        };
        if let Some(range) = self.matches.get(index).cloned() {
            item.replace_match(&query, range);
            self.select_match(item, Direction::Next);
        }
    }

    pub fn replace_all(&mut self, item: &mut dyn Searchable) {
        self.refresh(item);
        if let Some(query) = self.query_for(true) {
            item.replace_all(&query, &self.matches);
            self.refresh(item);
        }
    }

    pub fn toggle_option(&mut self, item: &mut dyn Searchable, click: SearchClick) {
        match click {
            SearchClick::CaseSensitive => self.options.case_sensitive ^= true,
            SearchClick::WholeWord => self.options.whole_word ^= true,
            SearchClick::Regex => self.options.regex ^= true,
            _ => return,
        }
        self.refresh(item);
    }

    /// Cmd+E: search for the selection (or the word at the caret) without moving focus.
    pub fn use_selection_for_find(&mut self, item: &mut dyn Searchable) {
        let suggestion = item.query_suggestion();
        if suggestion.is_empty() {
            return;
        }
        self.query.set_text(&suggestion);
        self.searched = None;
        self.refresh(item);
    }

    pub fn toggle_replace(&mut self) {
        self.replace_enabled ^= true;
        if !self.replace_enabled && self.focus == Some(SearchField::Replacement) {
            self.focus = Some(SearchField::Query);
        }
    }

    fn remember_query(&mut self) {
        let text = self.query.text();
        if text.is_empty() {
            return;
        }
        self.history.retain(|h| *h != text);
        self.history.push(text);
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
        self.history_index = None;
    }

    fn step_history(&mut self, older: bool) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        let index = match (self.history_index, older) {
            (None, true) => last,
            (None, false) => return,
            (Some(i), true) => i.saturating_sub(1),
            (Some(i), false) if i >= last => return,
            (Some(i), false) => i + 1,
        };
        self.history_index = Some(index);
        let text = self.history[index].clone();
        self.query.set_text(&text);
        self.query.move_to_end();
    }

    /// A key while one of the bar's fields has focus; returns false to hand focus back to the editor.
    pub fn key(&mut self, item: &mut dyn Searchable, key: EditKey, shift: bool) -> bool {
        let field = self.focus.unwrap_or(SearchField::Query);
        match key {
            EditKey::Escape => {
                self.dismiss(item);
                return false;
            }
            EditKey::Tab | EditKey::Outdent => {
                if self.replace_enabled {
                    self.focus = Some(match field {
                        SearchField::Query => SearchField::Replacement,
                        SearchField::Replacement => SearchField::Query,
                    });
                    return true;
                }
                self.focus = None;
                return false;
            }
            EditKey::Enter if field == SearchField::Replacement => self.replace_next(item),
            EditKey::ReplaceAll => self.replace_all(item),
            EditKey::Enter => self.select_match(
                item,
                if shift {
                    Direction::Prev
                } else {
                    Direction::Next
                },
            ),
            EditKey::Up if field == SearchField::Query => {
                self.step_history(true);
                self.refresh(item);
                self.activate_current(item);
            }
            EditKey::Down if field == SearchField::Query => {
                self.step_history(false);
                self.refresh(item);
                self.activate_current(item);
            }
            _ => {
                let edited = match field {
                    SearchField::Query => self.query.key(key, shift),
                    SearchField::Replacement => self.replacement.key(key, shift),
                };
                if edited && field == SearchField::Query {
                    self.refresh(item);
                    self.activate_current(item);
                }
            }
        }
        true
    }

    pub fn input(&mut self, item: &mut dyn Searchable, text: &str) {
        let text = text.replace('\n', "");
        match self.focus.unwrap_or(SearchField::Query) {
            SearchField::Query => {
                self.query.insert(&text);
                self.refresh(item);
                self.activate_current(item);
            }
            SearchField::Replacement => self.replacement.insert(&text),
        }
    }

    pub fn render(&self, base: u64) -> Node {
        let colors = theme();
        let icon_button =
            |kind: IconKind, click: SearchClick, toggled: bool, enabled: bool| -> Node {
                let color = if !enabled {
                    colors.text_disabled
                } else if toggled {
                    colors.text_accent
                } else {
                    colors.icon
                };
                let mut button = div()
                    .w_px(BUTTON)
                    .h_px(BUTTON)
                    .rounded(2.0)
                    .items_center()
                    .justify_center()
                    .child(icon(kind).size(BUTTON_ICON).color(color));
                if enabled {
                    button = button.on_click(click.id(base));
                }
                button.into()
            };
        let vertical_rule = || div().w_px(1.0).h_px(BUTTON).bg(colors.border_variant);
        let input =
            |field: &TextField, which: SearchField, placeholder: &str, border: ui::Rgba, click| {
                let focused = self.focus == Some(which);
                let text_color = match (which, self.active_match) {
                    (SearchField::Query, None) if !self.query.text().is_empty() => colors.error,
                    _ => colors.text,
                };
                div()
                    .row()
                    .flex(1.0)
                    .h_px(INPUT_H)
                    .pl(8.0)
                    .pr(4.0)
                    .gap(4.0)
                    .items_center()
                    .rounded(6.0)
                    .border(1.0, border)
                    .on_click(SearchClick::id(click, base))
                    .child(field.render(placeholder, focused, text_color, INPUT_H - 2.0))
            };
        let query_border = if self.error.is_some() {
            colors.error
        } else {
            colors.border
        };
        let has_match = self.active_match.is_some();
        let count = match self.active_match {
            Some(index) => format!("{}/{}", index + 1, self.matches.len()),
            None => "0/0".to_string(),
        };
        let query_column = input(
            &self.query,
            SearchField::Query,
            "Search...",
            query_border,
            SearchClick::Query,
        )
        .child(icon_button(
            IconKind::CaseSensitive,
            SearchClick::CaseSensitive,
            self.options.case_sensitive,
            true,
        ))
        .child(icon_button(
            IconKind::WholeWord,
            SearchClick::WholeWord,
            self.options.whole_word,
            true,
        ))
        .child(icon_button(
            IconKind::Regex,
            SearchClick::Regex,
            self.options.regex,
            true,
        ));
        let matches_column = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(vertical_rule())
            .child(icon_button(
                IconKind::ChevronLeft,
                SearchClick::Previous,
                false,
                has_match,
            ))
            .child(icon_button(
                IconKind::ChevronRight,
                SearchClick::Next,
                false,
                has_match,
            ))
            .child(
                div()
                    .w_px(40.0)
                    .pl(8.0)
                    .child(label(count).size(12.0).color(if has_match {
                        colors.text
                    } else {
                        colors.text_disabled
                    })),
            );
        let mode_column = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(icon_button(
                IconKind::Replace,
                SearchClick::ToggleReplace,
                self.replace_enabled,
                true,
            ))
            .child(icon_button(
                IconKind::SelectAll,
                SearchClick::SelectAll,
                false,
                true,
            ))
            .child(matches_column)
            .child(div().flex(1.0))
            .child(icon_button(
                IconKind::Close,
                SearchClick::Close,
                false,
                true,
            ));
        let search_line = div()
            .row()
            .h_px(INPUT_H)
            .items_center()
            .gap(LINE_GAP)
            .child(query_column)
            .child(div().w_px(256.0).child(mode_column));
        let mut bar = div()
            .col()
            .gap(LINE_GAP)
            .px(TOOLBAR_PAD_X)
            .py(TOOLBAR_PAD_Y)
            .bg(colors.toolbar_background)
            .child(search_line);
        if let Some(error) = &self.error {
            bar = bar.child(
                div()
                    .h_px(INPUT_FONT)
                    .pl(8.0)
                    .child(label(error.clone()).size(12.0).color(colors.error)),
            );
        }
        if self.replace_enabled {
            let replace_actions = div()
                .row()
                .items_center()
                .gap(4.0)
                .child(icon_button(
                    IconKind::ReplaceNext,
                    SearchClick::ReplaceNext,
                    false,
                    true,
                ))
                .child(icon_button(
                    IconKind::ReplaceAll,
                    SearchClick::ReplaceAll,
                    false,
                    true,
                ));
            bar = bar.child(
                div()
                    .row()
                    .h_px(INPUT_H)
                    .items_center()
                    .gap(LINE_GAP)
                    .child(input(
                        &self.replacement,
                        SearchField::Replacement,
                        "Replace with...",
                        colors.border,
                        SearchClick::Replacement,
                    ))
                    .child(div().w_px(256.0).child(replace_actions)),
            );
        }
        div()
            .col()
            .child(bar)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .into()
    }
}
