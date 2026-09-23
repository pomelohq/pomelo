use std::sync::Arc;

use editor::snippet::{boundary_suffixes, SnippetDefinition};
use ui::{div, label, theme, LabelSize, Node};

use crate::fuzzy::fuzzy_match;
use crate::list_scrollbar::{self, ScrollbarReveal};

pub const WIDTH: f32 = 544.0;
pub const MAX_VISIBLE: usize = 12;
/// Popover padding above and below the rows together.
const PADDING_Y: f32 = 8.0;
const MAX_PREFIX_CHARS: usize = 128;
const MAX_SNIPPET_MATCHES: usize = 100;

#[derive(Clone, Debug)]
pub enum CompletionKind {
    Word,
    Snippet {
        snippet: Arc<SnippetDefinition>,
        replaced: usize,
    },
    Choice,
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub label: String,
    pub detail: Option<String>,
    pub kind: CompletionKind,
}

struct Entry {
    completion: Completion,
    positions: Vec<usize>,
    score: f64,
    query: String,
}

pub struct SnippetMatch {
    pub completion: Completion,
    pub positions: Vec<usize>,
    pub score: f64,
    pub query: String,
}

pub fn match_snippets(
    before_caret: &str,
    snippets: &[Arc<SnippetDefinition>],
) -> Vec<SnippetMatch> {
    let skip = before_caret
        .chars()
        .count()
        .saturating_sub(MAX_PREFIX_CHARS);
    let window: String = before_caret.chars().skip(skip).collect();
    if window.is_empty() {
        return Vec::new();
    }
    let mut prefixes: Vec<(&Arc<SnippetDefinition>, &str, usize)> = snippets
        .iter()
        .flat_map(|snippet| {
            snippet
                .prefixes
                .iter()
                .map(move |prefix| (snippet, prefix.as_str(), boundary_suffixes(prefix).len()))
        })
        .collect();
    prefixes.sort_by_key(|(_, _, words)| std::cmp::Reverse(*words));
    let most_words = prefixes.first().map_or(0, |(_, _, words)| *words);
    let tails: Vec<&str> = boundary_suffixes(&window)
        .into_iter()
        .take(most_words)
        .collect();
    let lowered_first = |text: &str| {
        text.chars()
            .next()
            .map(|c| c.to_lowercase().collect::<String>())
    };
    let mut found: Vec<SnippetMatch> = Vec::new();
    for (index, tail) in tails.iter().enumerate().rev() {
        let words = index + 1;
        let eligible: Vec<&(&Arc<SnippetDefinition>, &str, usize)> = prefixes
            .iter()
            .filter(|(_, prefix, prefix_words)| {
                *prefix_words >= words && lowered_first(prefix) == lowered_first(tail)
            })
            .collect();
        let names: Vec<&str> = eligible.iter().map(|(_, prefix, _)| *prefix).collect();
        for matched in fuzzy_match(&names, tail) {
            let Some((snippet, prefix, _)) = eligible.get(matched.candidate) else {
                continue;
            };
            let duplicate = found.iter().any(|m| {
                m.completion.label == *prefix
                    && matches!(&m.completion.kind, CompletionKind::Snippet { snippet: s, .. } if Arc::ptr_eq(s, snippet))
            });
            if duplicate {
                continue;
            }
            found.push(SnippetMatch {
                completion: Completion {
                    label: prefix.to_string(),
                    detail: Some(snippet.name.clone()),
                    kind: CompletionKind::Snippet {
                        snippet: Arc::clone(snippet),
                        replaced: tail.chars().count(),
                    },
                },
                positions: matched.positions,
                score: matched.score,
                query: tail.to_string(),
            });
            if found.len() >= MAX_SNIPPET_MATCHES {
                return found;
            }
        }
    }
    found
}

pub struct CompletionsMenu {
    entries: Vec<Entry>,
    pub selected: usize,
    scroll_top: usize,
    pub scrollbar: ScrollbarReveal,
    pub hovered: Option<u64>,
}

impl CompletionsMenu {
    fn from_entries(entries: Vec<Entry>) -> Option<Self> {
        (!entries.is_empty()).then(|| Self {
            entries,
            selected: 0,
            scroll_top: 0,
            scrollbar: ScrollbarReveal::default(),
            hovered: None,
        })
    }

    pub fn new(query: &str, words: Vec<String>, snippets: Vec<SnippetMatch>) -> Option<Self> {
        let names: Vec<&str> = words.iter().map(String::as_str).collect();
        let mut entries: Vec<Entry> = fuzzy_match(&names, query)
            .into_iter()
            .filter_map(|m| {
                Some(Entry {
                    completion: Completion {
                        label: words.get(m.candidate)?.clone(),
                        detail: None,
                        kind: CompletionKind::Word,
                    },
                    positions: m.positions,
                    score: m.score,
                    query: query.to_string(),
                })
            })
            .collect();
        entries.extend(snippets.into_iter().map(|m| Entry {
            completion: m.completion,
            positions: m.positions,
            score: m.score,
            query: m.query,
        }));
        sort_entries(&mut entries);
        Self::from_entries(entries)
    }

    pub fn choices(choices: Vec<String>) -> Option<Self> {
        Self::from_entries(
            choices
                .into_iter()
                .map(|choice| Entry {
                    completion: Completion {
                        label: choice,
                        detail: None,
                        kind: CompletionKind::Choice,
                    },
                    positions: Vec::new(),
                    score: 0.0,
                    query: String::new(),
                })
                .collect(),
        )
    }

    pub fn selected_completion(&self) -> Option<&Completion> {
        self.completion_at(self.selected)
    }

    pub fn completion_at(&self, row: usize) -> Option<&Completion> {
        self.entries.get(row).map(|entry| &entry.completion)
    }

    pub fn is_choices(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| matches!(entry.completion.kind, CompletionKind::Choice))
    }

    #[cfg(test)]
    pub fn selected_word(&self) -> Option<&str> {
        self.selected_completion().map(|c| c.label.as_str())
    }

    #[cfg(test)]
    pub fn word_at(&self, row: usize) -> Option<&str> {
        self.completion_at(row).map(|c| c.label.as_str())
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
        let max_top = self.entries.len().saturating_sub(MAX_VISIBLE);
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
        self.entries.len().min(MAX_VISIBLE)
    }

    /// Height in design px.
    pub fn height(&self) -> f32 {
        self.visible() as f32 * crate::EDIT_LINE_H + PADDING_Y
    }

    /// Rows in the code font, matched letters bold, the selection tinted; row clicks are `id_base + row`.
    pub fn render(&self, id_base: u64) -> Node {
        let colors = theme();
        let end = (self.scroll_top + MAX_VISIBLE).min(self.entries.len());
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
            let (word, positions, detail) =
                self.entries.get(row).map_or(("", &[][..], None), |e| {
                    (
                        e.completion.label.as_str(),
                        e.positions.as_slice(),
                        e.completion.detail.as_deref(),
                    )
                });
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
            if let Some(detail) = detail.map(str::trim).filter(|d| !d.is_empty()) {
                item = item.child(div().flex(1.0)).child(
                    div().row().pl(16.0).child(
                        label(detail.to_string())
                            .label_size(LabelSize::Small)
                            .color(colors.text_muted),
                    ),
                );
            }
            Node::from(div().row().px(4.0).child(item))
        });
        let scrollbar = list_scrollbar::render(
            WIDTH - 1.0,
            self.height(),
            self.scroll_top,
            self.visible(),
            self.entries.len(),
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

fn sort_entries(entries: &mut [Entry]) {
    let tier = |entry: &Entry| {
        let first = entry
            .query
            .chars()
            .next()
            .and_then(|c| c.to_lowercase().next());
        first.is_some_and(|first| {
            editor::completion::split_words(&entry.completion.label).any(|part| {
                part.chars().next().and_then(|c| c.to_lowercase().next()) == Some(first)
            })
        })
    };
    let exact = |entry: &Entry| entry.completion.label == entry.query;
    let exact_case = |entry: &Entry| {
        let label: Vec<char> = entry.completion.label.chars().collect();
        entry
            .query
            .chars()
            .zip(&entry.positions)
            .filter(|(q, p)| label.get(**p) == Some(q))
            .count()
    };
    entries.sort_by(|a, b| match (tier(a), tier(b)) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => b.score.total_cmp(&a.score),
        (true, true) => exact(b)
            .cmp(&exact(a))
            .then(b.score.total_cmp(&a.score))
            .then_with(|| a.positions.cmp(&b.positions))
            .then_with(|| exact_case(b).cmp(&exact_case(a)))
            .then_with(|| a.completion.label.cmp(&b.completion.label)),
    });
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

    fn snippet(name: &str, prefixes: &[&str]) -> Arc<SnippetDefinition> {
        Arc::new(SnippetDefinition {
            name: name.to_string(),
            prefixes: words(prefixes),
            body: editor::snippet::Snippet::parse("x").unwrap(),
            description: None,
        })
    }

    #[test]
    fn word_start_matches_rank_first_then_exact_and_case() {
        let menu = CompletionsMenu::new(
            "bar",
            words(&["xbarx", "barrel", "fooBar", "bar", "Bar"]),
            Vec::new(),
        )
        .unwrap();
        let order: Vec<&str> = (0..5).filter_map(|i| menu.word_at(i)).collect();
        assert_eq!(order[0], "bar");
        assert_eq!(*order.last().unwrap(), "xbarx");
    }

    #[test]
    fn no_match_means_no_menu_and_selection_wraps() {
        assert!(CompletionsMenu::new("zzz", words(&["abc"]), Vec::new()).is_none());
        let mut menu = CompletionsMenu::new("a", words(&["ab", "ac"]), Vec::new()).unwrap();
        menu.select_previous();
        assert_eq!(menu.selected, 1);
        menu.select_next();
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn snippets_match_the_tail_with_as_many_words_as_their_prefix() {
        let snippets = vec![
            snippet("Function", &["fn"]),
            snippet("If let", &["if let"]),
            snippet("Log", &["log"]),
        ];
        let found = match_snippets("    fn", &snippets);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].completion.label, "fn");
        assert!(matches!(
            found[0].completion.kind,
            CompletionKind::Snippet { replaced: 2, .. }
        ));
        let found = match_snippets("x = if le", &snippets);
        assert_eq!(found.len(), 1);
        assert!(matches!(
            found[0].completion.kind,
            CompletionKind::Snippet { replaced: 5, .. }
        ));
        assert!(match_snippets("zz", &snippets).is_empty());
        assert!(match_snippets("", &snippets).is_empty());
    }

    #[test]
    fn snippets_merge_with_words_and_show_their_name() {
        let snippets = match_snippets("lo", &[snippet("Print to log", &["log"])]);
        let menu = CompletionsMenu::new("lo", words(&["lookup"]), snippets).unwrap();
        let labels: Vec<&str> = (0..2).filter_map(|i| menu.word_at(i)).collect();
        assert_eq!(labels.len(), 2);
        let log = (0..2)
            .filter_map(|i| menu.completion_at(i))
            .find(|c| c.label == "log")
            .unwrap();
        assert_eq!(log.detail.as_deref(), Some("Print to log"));
    }

    #[test]
    fn choices_keep_their_order() {
        let menu = CompletionsMenu::choices(words(&["b", "a"])).unwrap();
        assert_eq!(menu.word_at(0), Some("b"));
        assert!(matches!(
            menu.completion_at(1).unwrap().kind,
            CompletionKind::Choice
        ));
    }
}
