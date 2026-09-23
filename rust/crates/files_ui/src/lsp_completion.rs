//! Completions from the language server: asked for as a word or trigger character is typed, filtered locally
//! while the answer still covers what is typed, and applied with the server's own edit (snippet, insert or
//! replace range, extra edits such as an import).

use std::ops::Range;

use editor::buffer::{Bias, SelectionGoal};
use editor::snippet::Snippet;
use serde_json::Value;

use crate::completions_menu::{self, Completion, CompletionKind, CompletionsMenu};
use crate::{snippet_store, CompletionTrigger, FileItem, Lang};

/// Words from the file join server items only when the server had none, and once this much is typed.
const WORDS_MIN_LENGTH: usize = 3;

pub(crate) struct PendingCompletion {
    offset: usize,
    version: u64,
    trigger: Option<String>,
    query: Option<String>,
    request: Option<u64>,
    menu_was_open: bool,
    typed_word: bool,
}

pub(crate) struct PendingResolve {
    raw: Value,
    request: Option<u64>,
}

fn map_range(b: &editor::EditorBuffer, range: Range<usize>, since: u64) -> Range<usize> {
    let mut range = range;
    for batch in b.edits_since(since) {
        range.start = editor::buffer::map_offset(batch, range.start, Bias::Left);
        range.end = editor::buffer::map_offset(batch, range.end, Bias::Right);
    }
    range
}

impl FileItem {
    /// Whether the caret is still where a menu was asked for, having followed the typing since.
    fn at_initial_position(&self, initial: Option<(usize, u64)>) -> bool {
        let (Some((offset, version)), Some(b)) = (initial, self.buffer.as_ref()) else {
            return true;
        };
        let mut offset = offset;
        for batch in b.edits_since(version) {
            offset = editor::buffer::map_offset(batch, offset, Bias::Right);
        }
        offset == b.newest().head()
    }

    pub(crate) fn update_server_completions(&mut self, trigger: CompletionTrigger) {
        let Some(b) = self.buffer.as_ref() else {
            return self.close_completions();
        };
        let newest = b.newest();
        if !newest.is_empty() {
            return self.close_completions();
        }
        let head = newest.head();
        let version = b.version();
        let query = editor::completion::completion_query(b, head).map(|(_, query)| query);
        let trigger_character = match &trigger {
            CompletionTrigger::Character(c) => {
                let typed = c.to_string();
                self.lsp_triggers
                    .as_ref()
                    .is_some_and(|triggers| triggers.contains(&typed))
                    .then_some(typed)
            }
            _ => None,
        };
        let asks = match trigger {
            CompletionTrigger::Typed | CompletionTrigger::Show => true,
            CompletionTrigger::Character(_) => trigger_character.is_some(),
            CompletionTrigger::Refilter | CompletionTrigger::ShowWords => false,
        };
        let menu_open = self.completions.is_some();
        if let Some(menu) = self.completions.as_mut() {
            menu.filter(query.as_deref().unwrap_or(""));
        }
        if let Some(menu) = self.completions.as_ref() {
            let extends_initial = match (&menu.initial_query, &query) {
                (Some(initial), Some(query)) => query.starts_with(initial.as_str()),
                (None, _) => true,
                _ => false,
            };
            let covered = !menu.is_incomplete
                && extends_initial
                && self.at_initial_position(menu.initial_position);
            if covered && (asks || trigger == CompletionTrigger::Refilter) {
                if self
                    .completions
                    .as_ref()
                    .is_some_and(CompletionsMenu::is_empty)
                {
                    self.close_completions();
                }
                return;
            }
        }
        // Without a word before the caret, filtering stale items would show wrong ones.
        if query.is_none() && menu_open {
            self.close_completions();
        }
        if !asks && !menu_open {
            return;
        }
        if !asks && self.completions.is_some() && trigger != CompletionTrigger::Refilter {
            return self.close_completions();
        }
        self.completion_request = Some(PendingCompletion {
            offset: head,
            version,
            trigger: trigger_character,
            query,
            request: None,
            menu_was_open: menu_open,
            typed_word: trigger == CompletionTrigger::Typed,
        });
    }

    pub(crate) fn completion_request_due(&self) -> Option<(usize, Option<String>)> {
        let pending = self.completion_request.as_ref()?;
        pending
            .request
            .is_none()
            .then(|| (pending.offset, pending.trigger.clone()))
    }

    /// Record the request's token, or build the menu from local sources when no server can answer.
    pub(crate) fn completion_requested(&mut self, token: Option<u64>) {
        match token {
            Some(token) => {
                if let Some(pending) = self.completion_request.as_mut() {
                    pending.request = Some(token);
                }
            }
            None => {
                if let Some(pending) = self.completion_request.take() {
                    self.show_server_completions(pending, &[], false, 0);
                }
            }
        }
    }

    pub(crate) fn completions_answered(&mut self, response: &lsp::CompletionsResponse) {
        let matches = self
            .completion_request
            .as_ref()
            .is_some_and(|pending| pending.request == Some(response.request));
        if !matches {
            return;
        }
        if let Some(pending) = self.completion_request.take() {
            self.show_server_completions(
                pending,
                &response.items,
                response.is_incomplete,
                response.synced.buffer_version,
            );
        }
    }

    /// The menu of server items, the file's words when the server had nothing, and matching snippets.
    fn show_server_completions(
        &mut self,
        pending: PendingCompletion,
        items: &[lsp::LspCompletion],
        is_incomplete: bool,
        synced_version: u64,
    ) {
        let Some(b) = self.buffer.as_ref() else {
            return;
        };
        let newest = b.newest();
        if !newest.is_empty() {
            return self.close_completions();
        }
        let head = newest.head();
        let word = editor::completion::completion_query(b, head);
        let query = word
            .as_ref()
            .map_or("", |(_, query)| query.as_str())
            .to_string();
        let mut candidates: Vec<Completion> = items
            .iter()
            .map(|item| Completion {
                label: item.label.clone(),
                detail: item.detail.clone(),
                kind: CompletionKind::Lsp {
                    item: Box::new(item.clone()),
                    synced_version,
                },
            })
            .collect();
        let words_allowed = !matches!(self.lang, Lang::Markdown | Lang::PlainText);
        if items.is_empty() && words_allowed && query.chars().count() >= WORDS_MIN_LENGTH {
            if let Some((word_start, _)) = word.as_ref() {
                let mut word_end = head;
                while word_end < b.rope.len_chars() && {
                    let c = b.rope.char(word_end);
                    c.is_alphanumeric() || c == '_'
                } {
                    word_end += 1;
                }
                let whole_word = b.rope.slice(*word_start..word_end).to_string();
                let row = b.rope.char_to_line(head);
                let skip_digits = !query.chars().any(|c| c.is_ascii_digit());
                candidates.extend(
                    editor::completion::buffer_words(b, row, Some(&whole_word), skip_digits)
                        .into_iter()
                        .map(|word| Completion {
                            label: word,
                            detail: None,
                            kind: CompletionKind::Word,
                        }),
                );
            }
        }
        let definitions = self
            .snippet_dir
            .as_deref()
            .map(|dir| snippet_store::snippets_for(dir, self.lang))
            .unwrap_or_default();
        let needs_strong_match = pending.typed_word && !pending.menu_was_open;
        let strong = editor::snippet::has_strong_prefix_match(
            &query,
            definitions
                .iter()
                .flat_map(|d| d.prefixes.iter().map(String::as_str)),
        );
        if !definitions.is_empty() && (!needs_strong_match || strong) {
            let before = b.rope.slice(head.saturating_sub(256)..head).to_string();
            candidates.extend(
                completions_menu::match_snippets(&before, &definitions)
                    .into_iter()
                    .map(|found| found.completion),
            );
        }
        match CompletionsMenu::from_candidates(candidates, &query) {
            Some(mut menu) => {
                menu.initial_query = pending.query;
                menu.initial_position = Some((pending.offset, pending.version));
                menu.is_incomplete = is_incomplete;
                self.completions = Some(menu);
                self.hide_hover();
            }
            None => self.completions = None,
        }
    }

    /// Apply a server item at the newest caret (and at other carets with the same text around them): its
    /// replace range when what follows the caret ends its label, else its insert range.
    pub(crate) fn confirm_server_completion(
        &mut self,
        item: &lsp::LspCompletion,
        synced_version: u64,
    ) {
        let Some(b) = self.buffer.as_mut() else {
            return;
        };
        let newest = b.newest();
        let head = newest.head();
        let replace = map_range(b, item.replace_range.clone(), synced_version);
        let mut range = match item.insert_range.clone() {
            Some(insert) => {
                let insert = map_range(b, insert, synced_version);
                let should_replace = if replace.end > head {
                    let after = b.rope.slice(head..replace.end).to_string().to_lowercase();
                    item.label.to_lowercase().ends_with(&after)
                } else {
                    true
                };
                if should_replace {
                    replace
                } else {
                    insert
                }
            }
            None => replace,
        };
        range.start = range.start.min(head);
        if range.end < head {
            range.end = head;
        }
        let (text, snippet) = if item.is_snippet {
            match Snippet::parse(&item.new_text) {
                Ok(snippet) => (snippet.text.clone(), Some(snippet)),
                Err(_) => (item.new_text.clone(), None),
            }
        } else {
            (item.new_text.clone(), None)
        };
        let lookbehind = head - range.start;
        let lookahead = range.end - head;
        let prefix = b.rope.slice(range.start..head).to_string();
        let suffix = b.rope.slice(head..range.end).to_string();
        let len = b.rope.len_chars();
        let ranges: Vec<Range<usize>> = b
            .selections()
            .iter()
            .map(|selection| {
                if selection.id == newest.id {
                    return range.clone();
                }
                let mut other = selection.start..selection.end;
                if other.start >= lookbehind
                    && b.rope.slice(other.start - lookbehind..other.start) == prefix.as_str()
                {
                    other.start -= lookbehind;
                    if other.end + lookahead <= len
                        && b.rope.slice(other.end..other.end + lookahead) == suffix.as_str()
                    {
                        other.end += lookahead;
                    }
                }
                other
            })
            .collect();
        if let Some(snippet) = snippet {
            if let Some(choices) = b.insert_snippet(&ranges, &snippet) {
                self.completions = CompletionsMenu::choices(choices);
            }
        } else {
            let inserted = text.chars().count();
            let mut sorted = ranges.clone();
            sorted.sort_by_key(|range| range.start);
            let mut delta: isize = 0;
            let carets: Vec<usize> = sorted
                .iter()
                .map(|range| {
                    let caret = (range.start as isize + delta) as usize + inserted;
                    delta += inserted as isize - range.len() as isize;
                    caret
                })
                .collect();
            b.replace_ranges(
                ranges
                    .into_iter()
                    .map(|range| (range, text.clone()))
                    .collect(),
            );
            let mut next = b.selections().to_vec();
            if next.len() == carets.len() {
                for (selection, caret) in next.iter_mut().zip(carets) {
                    selection.collapse_to(caret, SelectionGoal::None);
                }
                b.set_selections(next);
            }
        }
        if !item.additional_edits.is_empty() {
            self.apply_additional_edits(&item.additional_edits, synced_version);
        } else if !item.raw.is_null() {
            self.pending_resolve = Some(PendingResolve {
                raw: item.raw.clone(),
                request: None,
            });
        }
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn apply_additional_edits(&mut self, edits: &[(Range<usize>, String)], since: u64) {
        let Some(b) = self.buffer.as_mut() else {
            return;
        };
        let mapped: Vec<(Range<usize>, String)> = edits
            .iter()
            .map(|(range, text)| (map_range(b, range.clone(), since), text.clone()))
            .collect();
        b.replace_ranges(mapped);
    }

    pub(crate) fn resolve_due(&self) -> Option<Value> {
        let pending = self.pending_resolve.as_ref()?;
        pending.request.is_none().then(|| pending.raw.clone())
    }

    pub(crate) fn resolve_requested(&mut self, token: Option<u64>) {
        match (token, self.pending_resolve.as_mut()) {
            (Some(token), Some(pending)) => pending.request = Some(token),
            _ => self.pending_resolve = None,
        }
    }

    /// The selected server item, to ask for its documentation, when the server wasn't asked about it yet.
    pub(crate) fn doc_resolve_due(&self) -> Option<Value> {
        let menu = self.completions.as_ref()?;
        let candidate = menu.selected_candidate()?;
        if menu.doc_resolved.contains(&candidate)
            || menu.doc_request.is_some_and(|(c, _)| c == candidate)
        {
            return None;
        }
        match &menu.completion_at(menu.selected)?.kind {
            CompletionKind::Lsp { item, .. } if !item.raw.is_null() => Some(item.raw.clone()),
            _ => None,
        }
    }

    pub(crate) fn doc_resolve_requested(&mut self, token: Option<u64>) {
        let Some(menu) = self.completions.as_mut() else {
            return;
        };
        let Some(candidate) = menu.selected_candidate() else {
            return;
        };
        menu.doc_resolved.insert(candidate);
        menu.doc_request = token.map(|token| (candidate, Some(token)));
    }

    pub(crate) fn resolve_answered(&mut self, resolved: &lsp::ResolvedCompletion) {
        if let Some(menu) = self.completions.as_mut() {
            if let Some((candidate, Some(token))) = menu.doc_request {
                if token == resolved.request {
                    menu.doc_request = None;
                    if let Some(documentation) = resolved.documentation.clone() {
                        menu.set_documentation(candidate, documentation);
                    }
                    return;
                }
            }
        }
        let matches = self
            .pending_resolve
            .as_ref()
            .is_some_and(|pending| pending.request == Some(resolved.request));
        if !matches {
            return;
        }
        self.pending_resolve = None;
        if !resolved.additional_edits.is_empty() {
            self.apply_additional_edits(&resolved.additional_edits, resolved.synced.buffer_version);
            self.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workspace::{EditKey, Item};

    fn item(text: &str) -> FileItem {
        let mut item = FileItem::new(
            std::path::PathBuf::from("/nonexistent"),
            "a.rs",
            Some(text.into()),
        );
        item.set_body_height(10.0 * crate::EDIT_LINE_H);
        let end = item.buffer.as_ref().unwrap().rope.len_chars();
        item.buffer.as_mut().unwrap().place_cursor(end);
        item.lsp_triggers = Some(vec![".".into()]);
        item
    }

    fn text(item: &FileItem) -> String {
        item.buffer.as_ref().unwrap().text()
    }

    fn caret(item: &FileItem) -> usize {
        item.buffer.as_ref().unwrap().newest().head()
    }

    fn server_item(label: &str, range: Range<usize>) -> lsp::LspCompletion {
        lsp::LspCompletion {
            label: label.into(),
            detail: None,
            kind: None,
            filter_text: label.into(),
            sort_text: None,
            new_text: label.into(),
            is_snippet: false,
            replace_range: range,
            insert_range: None,
            insert_as_is: false,
            deprecated: false,
            additional_edits: Vec::new(),
            documentation: None,
            raw: Value::Null,
        }
    }

    fn answer(
        item: &mut FileItem,
        token: u64,
        items: Vec<lsp::LspCompletion>,
        is_incomplete: bool,
    ) {
        let b = item.buffer.as_ref().unwrap();
        let synced = lsp::SyncedText {
            lsp_version: 0,
            buffer_version: b.version(),
            rope: b.rope.clone(),
        };
        item.completions_answered(&lsp::CompletionsResponse {
            request: token,
            items,
            is_incomplete,
            synced,
        });
    }

    #[test]
    fn a_trigger_character_asks_and_typing_filters_the_answer() {
        let mut item = item("x");
        item.input_text(".");
        assert_eq!(item.completion_request_due(), Some((2, Some(".".into()))));
        item.completion_requested(Some(1));
        answer(
            &mut item,
            1,
            vec![server_item("foo", 2..2), server_item("bar", 2..2)],
            false,
        );
        assert!(item.completions.as_ref().unwrap().word_at(1).is_some());
        item.input_text("f");
        assert!(
            item.completion_request_due().is_none(),
            "a complete answer is filtered"
        );
        assert_eq!(
            item.completions.as_ref().unwrap().selected_word(),
            Some("foo")
        );
        assert!(item.completions.as_ref().unwrap().word_at(1).is_none());
        item.input_key(EditKey::Enter, false);
        assert_eq!(text(&item), "x.foo");
        assert_eq!(caret(&item), 5);
    }

    #[test]
    fn an_incomplete_answer_asks_again_as_typing_goes_on() {
        let mut item = item("x.");
        item.input_text("f");
        item.completion_requested(Some(1));
        answer(&mut item, 1, vec![server_item("foo", 2..3)], true);
        item.input_text("o");
        assert_eq!(item.completion_request_due(), Some((4, None)));
    }

    #[test]
    fn the_replace_range_is_used_when_what_follows_ends_the_label() {
        for (label, expected) in [("foobar", "x.foobar"), ("format", "x.formatbar")] {
            let mut item = item("x.fobar");
            item.buffer.as_mut().unwrap().place_cursor(4);
            item.input_key(EditKey::ShowCompletions, false);
            item.completion_requested(Some(1));
            let mut entry = server_item(label, 2..7);
            entry.insert_range = Some(2..4);
            answer(&mut item, 1, vec![entry], false);
            item.input_key(EditKey::Enter, false);
            assert_eq!(text(&item), expected);
        }
    }

    #[test]
    fn snippet_items_expand_and_extra_edits_apply() {
        let mut item = item("fn a() { x.fo }");
        item.buffer.as_mut().unwrap().place_cursor(13);
        item.input_key(EditKey::ShowCompletions, false);
        item.completion_requested(Some(1));
        let mut entry = server_item("foo", 11..13);
        entry.new_text = "foo($1)".into();
        entry.is_snippet = true;
        entry.additional_edits = vec![(0..0, "use y;\n".into())];
        answer(&mut item, 1, vec![entry], false);
        item.input_key(EditKey::Enter, false);
        assert_eq!(text(&item), "use y;\nfn a() { x.foo() }");
        assert_eq!(caret(&item), 22);
    }

    #[test]
    fn missing_extra_edits_are_asked_for_after_choosing() {
        let mut item = item("x.fo");
        item.input_key(EditKey::ShowCompletions, false);
        item.completion_requested(Some(1));
        let mut entry = server_item("foo", 2..4);
        entry.raw = serde_json::json!({"label": "foo"});
        answer(&mut item, 1, vec![entry], false);
        item.input_key(EditKey::Enter, false);
        assert!(item.resolve_due().is_some());
        item.resolve_requested(Some(9));
        let b = item.buffer.as_ref().unwrap();
        let synced = lsp::SyncedText {
            lsp_version: 1,
            buffer_version: b.version(),
            rope: b.rope.clone(),
        };
        item.resolve_answered(&lsp::ResolvedCompletion {
            request: 9,
            additional_edits: vec![(0..0, "use z;\n".into())],
            documentation: None,
            synced,
        });
        assert_eq!(text(&item), "use z;\nx.foo");
    }

    #[test]
    fn without_a_server_answer_the_file_words_show() {
        let mut item = item("hello helpme\nhel");
        item.input_text("p");
        item.completion_requested(None);
        let menu = item.completions.as_ref().unwrap();
        assert_eq!(menu.selected_word(), Some("helpme"));
    }

    fn content() -> ui::Rect {
        ui::Rect::new(
            0.0,
            0.0,
            700.0,
            10.0 * crate::EDIT_LINE_H,
            ui::Rgba::TRANSPARENT,
        )
    }

    #[test]
    fn the_selected_item_documentation_is_fetched_and_shown_beside_the_menu() {
        let mut item = item("x.fo");
        item.input_key(EditKey::ShowCompletions, false);
        item.completion_requested(Some(1));
        let mut entry = server_item("foo", 2..4);
        entry.raw = serde_json::json!({"label": "foo"});
        answer(&mut item, 1, vec![entry], false);
        assert!(item.completion_aside(content(), (2000.0, 1000.0)).is_none());
        assert!(item.doc_resolve_due().is_some());
        item.doc_resolve_requested(Some(5));
        assert!(item.doc_resolve_due().is_none(), "asked once per item");
        let b = item.buffer.as_ref().unwrap();
        let synced = lsp::SyncedText {
            lsp_version: 0,
            buffer_version: b.version(),
            rope: b.rope.clone(),
        };
        item.resolve_answered(&lsp::ResolvedCompletion {
            request: 5,
            additional_edits: Vec::new(),
            documentation: Some(lsp::CompletionDocumentation::MultiLineMarkdown(
                "Does foo.\n\n```rust\nfn foo()\n```".into(),
            )),
            synced,
        });
        let (menu_x, _, _) = item
            .completion_popover(content())
            .map(|(_, x, y)| (x, y, 0))
            .unwrap();
        let (_, aside_x, _) = item.completion_aside(content(), (2000.0, 1000.0)).unwrap();
        assert!(aside_x > menu_x + completions_menu::WIDTH);
        // Without room on the right it goes below the menu, at the menu's left edge.
        let (_, narrow_x, narrow_y) = item.completion_aside(content(), (700.0, 1000.0)).unwrap();
        assert_eq!(narrow_x, menu_x);
        assert!(narrow_y > crate::EDIT_LINE_H);
    }

    #[test]
    fn one_line_documentation_sits_at_the_row_end() {
        let mut item = item("x.fo");
        item.input_key(EditKey::ShowCompletions, false);
        item.completion_requested(Some(1));
        let mut entry = server_item("foo", 2..4);
        entry.documentation = Some(lsp::CompletionDocumentation::SingleLine("Does foo".into()));
        answer(&mut item, 1, vec![entry], false);
        let (node, x, y) = item.completion_popover(content()).unwrap();
        let painted = ui::render(
            &node,
            ui::Rect::new(x, y, 800.0, 400.0, ui::Rgba::TRANSPARENT),
        );
        assert!(painted.texts.iter().any(|text| text.text == "Does foo"));
        assert!(item.completion_aside(content(), (2000.0, 1000.0)).is_none());
    }
}
