//! Cmd+K M, or a click on the language in the status bar: a modal listing every language the editor knows,
//! fuzzy-filtered as you type, with the active file's own marked `(current)` and selected. Enter switches the
//! file to the chosen language.

use editor::highlight::Lang;
use ui::{div, label, material_icon, theme, LabelSize, MaterialIcon, Node};
use workspace::text_field::{FieldFont, TextField, INPUT_FONT};

use crate::command_palette::scroll_rows;
use crate::fuzzy::{fuzzy_match, Match};

pub const WIDTH: f32 = 544.0;
const MAX_RESULTS_HEIGHT: f32 = 384.0;
const HEAD_HEIGHT: f32 = 36.0;
const PLACEHOLDER: &str = "Select a language...";
const MAX_MATCHES: usize = 100;
const CURRENT_SUFFIX: &str = " (current)";

#[derive(Clone)]
struct Candidate {
    lang: Lang,
    icon: MaterialIcon,
}

pub struct LanguageSelector {
    pub field: TextField,
    candidates: Vec<Candidate>,
    current: Option<usize>,
    matches: Vec<Match>,
    pub(crate) selected: usize,
    scroll_top: usize,
    scroll_remainder: f32,
    pub scrollbar: crate::list_scrollbar::ScrollbarReveal,
    pub hovered: Option<u64>,
}

/// Every language a file can be switched to, by name without regard to case.
fn candidates() -> Vec<Candidate> {
    let mut languages: Vec<Lang> = Lang::LANGUAGES.to_vec();
    languages.sort_by_key(|lang| lang.name().to_lowercase());
    languages
        .into_iter()
        .map(|lang| Candidate {
            lang,
            icon: language_icon(lang),
        })
        .collect()
}

/// The file icon of the first of the language's file endings that has one of its own.
fn language_icon(lang: Lang) -> MaterialIcon {
    let registered = editor::registry::language_registry()
        .read()
        .ok()
        .and_then(|registry| registry.matcher(lang))
        .map(|matcher| matcher.path_suffixes)
        .unwrap_or_default();
    registered
        .iter()
        .map(String::as_str)
        .chain(lang.path_suffixes().iter().copied())
        // A suffix can be a whole file name (`Dockerfile`, `COMMIT_EDITMSG`) as well as an extension.
        .flat_map(|suffix| {
            [
                crate::file_icon(suffix),
                crate::file_icon(&format!("file.{suffix}")),
            ]
        })
        .find(|icon| *icon != MaterialIcon::Document)
        .or_else(|| {
            // A package language that isn't installed yet registers no endings: go by its known extensions.
            (lang != Lang::PlainText).then_some(())?;
            crate::EXTENSION_ICONS
                .iter()
                .find(|(extensions, _)| extensions.iter().any(|ext| Lang::from_ext(ext) == lang))
                .map(|(_, icon)| *icon)
        })
        .unwrap_or(MaterialIcon::Document)
}

impl LanguageSelector {
    pub fn new(current: Lang) -> LanguageSelector {
        let candidates = candidates();
        let current = candidates
            .iter()
            .position(|candidate| candidate.lang == current);
        let mut selector = LanguageSelector {
            field: TextField::default(),
            candidates,
            current,
            matches: Vec::new(),
            selected: 0,
            scroll_top: 0,
            scroll_remainder: 0.0,
            scrollbar: Default::default(),
            hovered: None,
        };
        selector.update_matches();
        selector
    }

    /// Refilter for the query. With none, every language is listed and the current one selected; otherwise the
    /// best match is.
    pub fn update_matches(&mut self) {
        let names: Vec<&str> = self
            .candidates
            .iter()
            .map(|candidate| candidate.lang.name())
            .collect();
        let query = self.field.text();
        let query_is_empty = query.trim().is_empty();
        let mut matches = fuzzy_match(&names, &query);
        if !query_is_empty {
            matches.truncate(MAX_MATCHES);
        }
        self.selected = if query_is_empty {
            self.current
                .and_then(|current| matches.iter().position(|m| m.candidate == current))
                .unwrap_or(0)
        } else {
            0
        };
        self.matches = matches;
        self.scroll_top = 0;
        self.scroll_remainder = 0.0;
        self.scroll_to_selected();
    }

    #[cfg(test)]
    pub fn listed(&self) -> Vec<String> {
        (0..self.matches.len())
            .filter_map(|row| self.row_label(row))
            .collect()
    }

    #[cfg(test)]
    pub fn selected_label(&self) -> Option<String> {
        self.row_label(self.selected)
    }

    fn row_label(&self, row: usize) -> Option<String> {
        let found = self.matches.get(row)?;
        let candidate = self.candidates.get(found.candidate)?;
        let mut text = candidate.lang.name().to_string();
        if Some(found.candidate) == self.current {
            text.push_str(CURRENT_SUFFIX);
        }
        Some(text)
    }

    pub fn select_previous(&mut self) {
        let count = self.matches.len();
        if count > 0 {
            self.selected = if self.selected == 0 {
                count - 1
            } else {
                self.selected - 1
            };
            self.scroll_to_selected();
        }
    }

    pub fn select_next(&mut self) {
        let count = self.matches.len();
        if count > 0 {
            self.selected = if self.selected + 1 == count {
                0
            } else {
                self.selected + 1
            };
            self.scroll_to_selected();
        }
    }

    pub fn select_row(&mut self, row: usize) {
        if row < self.matches.len() {
            self.selected = row;
        }
    }

    /// The chosen language; `None` when nothing matches.
    pub fn confirm(&self) -> Option<Lang> {
        let found = self.matches.get(self.selected)?;
        self.candidates
            .get(found.candidate)
            .map(|candidate| candidate.lang)
    }

    pub fn scroll_by(&mut self, dy: f32) -> bool {
        let max_top = self.matches.len().saturating_sub(Self::visible_rows());
        let moved = scroll_rows(
            &mut self.scroll_top,
            &mut self.scroll_remainder,
            dy,
            row_height() * ui::ui_text_scale(),
            max_top,
        );
        if moved {
            self.scrollbar.reveal();
        }
        moved
    }

    fn visible_rows() -> usize {
        ((MAX_RESULTS_HEIGHT - 8.0) / row_height()).floor().max(1.0) as usize
    }

    fn scroll_to_selected(&mut self) {
        let visible = Self::visible_rows();
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        } else if self.selected >= self.scroll_top + visible {
            self.scroll_top = self.selected + 1 - visible;
        }
        self.scroll_top = self
            .scroll_top
            .min(self.matches.len().saturating_sub(visible));
    }

    pub fn render(&self, id_base: u64) -> Node {
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
        let visible = Self::visible_rows();
        let end = (self.scroll_top + visible).min(self.matches.len());
        let mut results = div().col().py(4.0);
        if self.matches.is_empty() {
            results = results.child(
                div().row().px(10.0).py(4.0).child(
                    label("No matches")
                        .label_size(LabelSize::Default)
                        .color(colors.text_muted),
                ),
            );
        } else {
            results = results
                .children(crate::list_scrollbar::render(
                    WIDTH - 1.0,
                    visible.min(self.matches.len()) as f32 * row_height() + 8.0,
                    self.scroll_top,
                    visible,
                    self.matches.len(),
                    self.scrollbar.opacity(),
                ))
                .children((self.scroll_top..end).map(|row| self.render_row(row, id_base)));
        }
        div()
            .col()
            .w_px(WIDTH)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .bg(colors.elevated_surface_background)
            .child(head)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(results)
            .into()
    }

    fn render_row(&self, row: usize, id_base: u64) -> Node {
        let colors = theme();
        let (Some(found), Some(text)) = (self.matches.get(row), self.row_label(row)) else {
            return div().into();
        };
        let Some(candidate) = self.candidates.get(found.candidate) else {
            return div().into();
        };
        let id = id_base + row as u64;
        let mut item = div()
            .row()
            .flex(1.0)
            .px(6.0)
            .py(4.0)
            .gap(6.0)
            .items_center()
            .rounded(4.0)
            .on_click(id);
        if row == self.selected {
            item = item.bg(colors.element_selected);
        } else if self.hovered == Some(id) {
            item = item.bg(colors.ghost_element_hover);
        }
        div()
            .row()
            .px(4.0)
            .child(item.child(material_icon(candidate.icon).size(14.0)).child(
                crate::file_finder::highlighted(
                    &text,
                    &found.positions,
                    colors.text,
                    LabelSize::Default,
                ),
            ))
            .into()
    }
}

/// A row's height in design px: a label line plus the item padding.
fn row_height() -> f32 {
    LabelSize::Default.px() * 1.4 + 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_is_listed_with_the_current_one_marked_and_selected() {
        let selector = LanguageSelector::new(Lang::Ruby);
        let listed = selector.listed();
        assert_eq!(listed.len(), Lang::LANGUAGES.len());
        assert!(listed.contains(&"Plain Text".to_string()));
        assert!(
            listed.contains(&"Kotlin".to_string()),
            "packaged languages too"
        );
        assert_eq!(selector.selected_label().as_deref(), Some("Ruby (current)"));
        assert_eq!(selector.confirm(), Some(Lang::Ruby));
        let javascript = listed.iter().position(|name| name == "JavaScript");
        let json = listed.iter().position(|name| name == "JSON");
        assert!(
            javascript < json,
            "sorted without regard to case: {listed:?}"
        );
    }

    #[test]
    fn typing_filters_and_selects_the_best_match() {
        let mut selector = LanguageSelector::new(Lang::PlainText);
        selector.field.set_text("rus");
        selector.update_matches();
        assert_eq!(selector.selected_label().as_deref(), Some("Rust"));
        assert_eq!(selector.confirm(), Some(Lang::Rust));
        selector.field.set_text("zzzz");
        selector.update_matches();
        assert!(selector.listed().is_empty());
        assert_eq!(selector.confirm(), None);
        selector.field.set_text("");
        selector.update_matches();
        assert_eq!(
            selector.selected_label().as_deref(),
            Some("Plain Text (current)"),
            "clearing the query goes back to the current language"
        );
    }

    #[test]
    fn languages_show_their_file_icon() {
        assert!(language_icon(Lang::Rust) == crate::file_icon("main.rs"));
        let generic: Vec<&str> = Lang::LANGUAGES
            .iter()
            .filter(|lang| **lang != Lang::PlainText)
            .filter(|lang| language_icon(**lang) == MaterialIcon::Document)
            .map(|lang| lang.name())
            .collect();
        assert!(generic.is_empty(), "no icon of their own: {generic:?}");
        assert!(language_icon(Lang::PlainText) == MaterialIcon::Document);
    }

    #[test]
    fn file_names_without_a_telling_extension_get_their_icon() {
        use crate::file_icon;
        for name in [".env", ".env.local", ".env.development.local"] {
            assert!(file_icon(name) == MaterialIcon::Tune, "{name}");
        }
        assert!(file_icon("Dockerfile") == MaterialIcon::Docker);
        assert!(file_icon("Makefile") == MaterialIcon::Makefile);
        assert!(file_icon("COMMIT_EDITMSG") == MaterialIcon::Git);
        assert!(file_icon("schema.sql") == MaterialIcon::Database);
        assert!(file_icon("logo.svg") == MaterialIcon::Image);
        assert!(file_icon("notes.txt") == MaterialIcon::Document);
    }
}
