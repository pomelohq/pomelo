use std::sync::Arc;

use editor::snippet::{boundary_suffixes, SnippetDefinition};
use ui::{div, label, theme, LabelSize, Node, Rgba};

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
    /// From the language server, with the buffer version its ranges are in.
    Lsp {
        item: Box<lsp::LspCompletion>,
        synced_version: u64,
    },
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub label: String,
    pub detail: Option<String>,
    pub kind: CompletionKind,
}

impl Completion {
    fn filter_text(&self) -> &str {
        match &self.kind {
            CompletionKind::Lsp { item, .. } => &item.filter_text,
            _ => &self.label,
        }
    }

    fn sort_text(&self) -> Option<&str> {
        match &self.kind {
            CompletionKind::Lsp { item, .. } => item.sort_text.as_deref(),
            _ => None,
        }
    }

    /// Keywords, then variables, constants and properties, then everything else.
    fn sort_kind(&self) -> usize {
        use lsp::lsp_types::CompletionItemKind as Kind;
        match &self.kind {
            CompletionKind::Lsp { item, .. } => match item.kind {
                Some(Kind::KEYWORD) => 0,
                Some(Kind::VARIABLE) => 1,
                Some(Kind::CONSTANT) => 2,
                Some(Kind::PROPERTY) => 3,
                _ => 4,
            },
            _ => 4,
        }
    }

    fn is_snippet(&self) -> bool {
        match &self.kind {
            CompletionKind::Snippet { .. } => true,
            CompletionKind::Lsp { item, .. } => {
                item.kind == Some(lsp::lsp_types::CompletionItemKind::SNIPPET)
            }
            _ => false,
        }
    }
}

struct Entry {
    candidate: usize,
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
    candidates: Vec<Completion>,
    entries: Vec<Entry>,
    pub selected: usize,
    scroll_top: usize,
    pub scrollbar: ScrollbarReveal,
    pub hovered: Option<u64>,
    /// For a menu from the language server: the word typed when it was asked, where the caret was (and the
    /// buffer version that offset is in), and whether more typing must ask again instead of filtering.
    pub initial_query: Option<String>,
    pub initial_position: Option<(usize, u64)>,
    pub is_incomplete: bool,
}

impl CompletionsMenu {
    fn from_parts(candidates: Vec<Completion>, entries: Vec<Entry>) -> Option<Self> {
        (!entries.is_empty()).then(|| Self {
            candidates,
            entries,
            selected: 0,
            scroll_top: 0,
            scrollbar: ScrollbarReveal::default(),
            hovered: None,
            initial_query: None,
            initial_position: None,
            is_incomplete: false,
        })
    }

    pub fn new(query: &str, words: Vec<String>, snippets: Vec<SnippetMatch>) -> Option<Self> {
        let names: Vec<&str> = words.iter().map(String::as_str).collect();
        let mut entries: Vec<Entry> = fuzzy_match(&names, query)
            .into_iter()
            .map(|m| Entry {
                candidate: m.candidate,
                positions: m.positions,
                score: m.score,
                query: query.to_string(),
            })
            .collect();
        let mut candidates: Vec<Completion> = words
            .into_iter()
            .map(|word| Completion {
                label: word,
                detail: None,
                kind: CompletionKind::Word,
            })
            .collect();
        for snippet in snippets {
            entries.push(Entry {
                candidate: candidates.len(),
                positions: snippet.positions,
                score: snippet.score,
                query: snippet.query,
            });
            candidates.push(snippet.completion);
        }
        sort_entries(&candidates, &mut entries);
        Self::from_parts(candidates, entries)
    }

    /// Every candidate matching `query`, ranked; `None` when none does.
    pub fn from_candidates(candidates: Vec<Completion>, query: &str) -> Option<Self> {
        let mut menu = Self::from_parts(
            candidates,
            vec![Entry {
                candidate: 0,
                positions: Vec::new(),
                score: 0.0,
                query: String::new(),
            }],
        )?;
        menu.filter(query);
        (!menu.entries.is_empty()).then_some(menu)
    }

    /// Match the candidates against `query` again, e.g. after more typing.
    pub fn filter(&mut self, query: &str) {
        let names: Vec<&str> = self
            .candidates
            .iter()
            .map(Completion::filter_text)
            .collect();
        self.entries = fuzzy_match(&names, query)
            .into_iter()
            .map(|m| Entry {
                candidate: m.candidate,
                positions: m.positions,
                score: m.score,
                query: query.to_string(),
            })
            .collect();
        sort_entries(&self.candidates, &mut self.entries);
        self.selected = 0;
        self.scroll_top = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn choices(choices: Vec<String>) -> Option<Self> {
        let entries = (0..choices.len())
            .map(|candidate| Entry {
                candidate,
                positions: Vec::new(),
                score: 0.0,
                query: String::new(),
            })
            .collect();
        let candidates = choices
            .into_iter()
            .map(|choice| Completion {
                label: choice,
                detail: None,
                kind: CompletionKind::Choice,
            })
            .collect();
        Self::from_parts(candidates, entries)
    }

    pub fn selected_completion(&self) -> Option<&Completion> {
        self.completion_at(self.selected)
    }

    pub fn completion_at(&self, row: usize) -> Option<&Completion> {
        self.entries
            .get(row)
            .and_then(|entry| self.candidates.get(entry.candidate))
    }

    pub fn is_choices(&self) -> bool {
        self.candidates
            .iter()
            .all(|completion| matches!(completion.kind, CompletionKind::Choice))
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
            let Some((entry, completion)) = self
                .entries
                .get(row)
                .and_then(|entry| Some((entry, self.candidates.get(entry.candidate)?)))
            else {
                return Node::from(div());
            };
            if let CompletionKind::Lsp { item: lsp_item, .. } = &completion.kind {
                for (text, color, bold) in lsp_row_runs(lsp_item, &entry.positions) {
                    item = item.child(code_run(text, color, bold));
                }
                return Node::from(div().row().px(4.0).child(item));
            }
            let (word, positions, detail) = (
                completion.label.as_str(),
                entry.positions.as_slice(),
                completion.detail.as_deref(),
            );
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

/// Candidates where the query's first letter starts one of the label's parts come first (snippets do their
/// own first-letter matching); among them an exact match, then score, earlier match positions, more exact-case
/// letters, the server's sort text, the item kind, and the label decide. The rest go by score.
fn sort_entries(candidates: &[Completion], entries: &mut [Entry]) {
    let completion = |entry: &Entry| candidates.get(entry.candidate);
    let tier = |entry: &Entry| {
        let Some(completion) = completion(entry) else {
            return false;
        };
        let first = entry
            .query
            .chars()
            .next()
            .and_then(|c| c.to_lowercase().next());
        completion.is_snippet()
            || first.is_none_or(|first| {
                editor::completion::split_words(completion.filter_text()).any(|part| {
                    part.chars().next().and_then(|c| c.to_lowercase().next()) == Some(first)
                })
            })
    };
    let exact = |entry: &Entry| completion(entry).is_some_and(|c| c.filter_text() == entry.query);
    let exact_case = |entry: &Entry| {
        let text: Vec<char> = completion(entry)
            .map(|c| c.filter_text().chars().collect())
            .unwrap_or_default();
        entry
            .query
            .chars()
            .zip(&entry.positions)
            .filter(|(q, p)| text.get(**p) == Some(q))
            .count()
    };
    let sort_text = |entry: &Entry| completion(entry).and_then(Completion::sort_text);
    let sort_kind = |entry: &Entry| completion(entry).map_or(4, Completion::sort_kind);
    let label = |entry: &Entry| completion(entry).map_or("", Completion::filter_text);
    entries.sort_by(|a, b| match (tier(a), tier(b)) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => b.score.total_cmp(&a.score),
        (true, true) => exact(b)
            .cmp(&exact(a))
            .then(b.score.total_cmp(&a.score))
            .then_with(|| a.positions.cmp(&b.positions))
            .then_with(|| exact_case(b).cmp(&exact_case(a)))
            .then_with(|| sort_text(a).cmp(&sort_text(b)))
            .then_with(|| sort_kind(a).cmp(&sort_kind(b)))
            .then_with(|| label(a).cmp(label(b))),
    });
}

/// The row text available to a label and its detail, in design px.
const ROW_TEXT_WIDTH: f32 = WIDTH - 8.0 - 12.0;

/// A server item's row: its label in the kind's syntax color (muted when deprecated), then its detail, the
/// matched letters bold, cut to fit the row.
fn lsp_row_runs(item: &lsp::LspCompletion, positions: &[usize]) -> Vec<(String, Rgba, bool)> {
    use lsp::lsp_types::CompletionItemKind as Kind;
    let text = match item.detail.as_deref() {
        Some(detail) => format!("{} {detail}", item.label),
        None => item.label.clone(),
    };
    let chars: Vec<char> = text.chars().collect();
    let label_chars = item.label.chars().count();
    let filter_start = text
        .find(item.filter_text.as_str())
        .map(|byte| text[..byte].chars().count())
        .unwrap_or(0);
    let colors = theme();
    let syntax = crate::syntax_theme();
    let capture = match item.kind {
        Some(Kind::CLASS | Kind::INTERFACE | Kind::STRUCT) => "type",
        Some(Kind::CONSTANT) => "constant",
        Some(Kind::CONSTRUCTOR) => "constructor",
        Some(Kind::ENUM) => "enum",
        Some(Kind::ENUM_MEMBER) => "variant",
        Some(Kind::FIELD | Kind::PROPERTY) => "property",
        Some(Kind::FUNCTION) => "function",
        Some(Kind::METHOD) => "function.method",
        Some(Kind::OPERATOR) => "operator",
        Some(Kind::VARIABLE) => "variable",
        Some(Kind::KEYWORD) => "keyword",
        _ => "",
    };
    let label_color = crate::color_of(&syntax, capture);
    let char_width = ui::measure_text_width("M", crate::EDIT_FONT, true, 400) / ui::ui_text_scale();
    let fits = (ROW_TEXT_WIDTH / char_width.max(1.0)).floor() as usize;
    let shown: Vec<char> = if chars.len() > fits {
        let mut cut: Vec<char> = chars.iter().take(fits.saturating_sub(3)).copied().collect();
        cut.extend("...".chars());
        cut
    } else {
        chars
    };
    let mut runs: Vec<(String, Rgba, bool)> = Vec::new();
    for (index, c) in shown.iter().enumerate() {
        let color = if item.deprecated {
            colors.text_muted
        } else if index < label_chars {
            label_color
        } else {
            colors.editor_foreground
        };
        let bold =
            index >= filter_start && positions.binary_search(&(index - filter_start)).is_ok();
        match runs.last_mut() {
            Some((text, last_color, last_bold)) if *last_color == color && *last_bold == bold => {
                text.push(*c)
            }
            _ => runs.push((c.to_string(), color, bold)),
        }
    }
    runs
}

fn code_run(text: String, color: Rgba, bold: bool) -> Node {
    let run = label(text).size(crate::EDIT_FONT).mono().color(color);
    if bold {
        run.weight(700).into()
    } else {
        run.into()
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
