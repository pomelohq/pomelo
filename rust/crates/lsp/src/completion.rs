//! Completion items from a server, each placed in the text it was asked about: what to insert, over which
//! range, and how to show and rank it.

use std::ops::Range;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemTag, CompletionTextEdit, InsertTextFormat,
    InsertTextMode, TextEdit,
};
use ropey::Rope;
use serde::Deserialize;
use serde_json::Value;

use crate::position::position_to_char;
use crate::store::SyncedText;

/// One item, ranges in chars of the text it was computed for.
#[derive(Clone, Debug, PartialEq)]
pub struct LspCompletion {
    pub label: String,
    /// Shown after the label: the item's detail, or its label description.
    pub detail: Option<String>,
    pub kind: Option<CompletionItemKind>,
    pub filter_text: String,
    pub sort_text: Option<String>,
    pub new_text: String,
    /// `new_text` is a snippet with stops to fill.
    pub is_snippet: bool,
    pub replace_range: Range<usize>,
    /// A shorter range that only inserts, when the server offers both.
    pub insert_range: Option<Range<usize>>,
    pub insert_as_is: bool,
    pub deprecated: bool,
    pub additional_edits: Vec<(Range<usize>, String)>,
    /// The item as the server sent it, to ask it to fill in the rest.
    pub raw: Value,
}

#[derive(Clone, Debug)]
pub struct CompletionsResponse {
    pub request: u64,
    pub items: Vec<LspCompletion>,
    /// More typing should ask again rather than filter these.
    pub is_incomplete: bool,
    pub synced: SyncedText,
}

/// Additional edits a server filled in for a chosen item, in `synced`'s text.
#[derive(Clone, Debug)]
pub struct ResolvedCompletion {
    pub request: u64,
    pub additional_edits: Vec<(Range<usize>, String)>,
    pub synced: SyncedText,
}

/// A range the server named, if it lies exactly within the text (a clipped one means the server's view of
/// the text differs, and the item is dropped).
fn exact_range(rope: &Rope, range: lsp_types::Range) -> Option<Range<usize>> {
    let start = position_to_char(rope, range.start);
    let end = position_to_char(rope, range.end);
    let back = |offset: usize| crate::char_to_position(rope, offset);
    (back(start) == range.start && back(end) == range.end).then_some(start..end)
}

pub(crate) fn text_edits(rope: &Rope, edits: &[TextEdit]) -> Vec<(Range<usize>, String)> {
    edits
        .iter()
        .filter_map(|edit| {
            Some((
                exact_range(rope, edit.range)?,
                edit.new_text.replace("\r\n", "\n"),
            ))
        })
        .collect()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The word around `offset`, or an empty range at it.
fn word_around(rope: &Rope, offset: usize) -> Range<usize> {
    let mut start = offset;
    while start > 0 && is_word_char(rope.char(start - 1)) {
        start -= 1;
    }
    let mut end = offset;
    while end < rope.len_chars() && is_word_char(rope.char(end)) {
        end += 1;
    }
    start..end
}

/// Items with their edits placed in `rope`, asked about at `offset`. Items whose edits don't fit the text
/// are dropped, and the result is then marked incomplete so the next keystroke asks again.
pub(crate) fn parse_completions(
    response: Value,
    rope: &Rope,
    offset: usize,
) -> (Vec<LspCompletion>, bool) {
    let (items, mut is_incomplete, defaults) = match response {
        Value::Array(items) => (items, false, None),
        Value::Object(_) => match serde_json::from_value::<CompletionListJson>(response) {
            Ok(list) => (list.items, list.is_incomplete, list.item_defaults),
            Err(_) => (Vec::new(), false, None),
        },
        _ => (Vec::new(), false, None),
    };
    let default_range = defaults.as_ref().and_then(|d| d.edit_range.as_ref());
    let fallback = ItemDefaults {
        format: defaults.as_ref().and_then(|d| d.insert_text_format),
        mode: defaults.as_ref().and_then(|d| d.insert_text_mode),
        data: defaults.as_ref().and_then(|d| d.data.clone()),
    };
    let count = items.len();
    let word = word_around(rope, offset);
    let parsed: Vec<LspCompletion> = items
        .into_iter()
        .filter_map(|value| {
            let text_edit_text = value
                .get("textEditText")
                .and_then(Value::as_str)
                .map(str::to_string);
            let item = serde_json::from_value::<CompletionItem>(value).ok()?;
            let edit = item.text_edit.clone().or_else(|| {
                let new_text = text_edit_text.unwrap_or_else(|| item.label.clone());
                match default_range? {
                    EditRangeDefault::Range(range) => {
                        Some(CompletionTextEdit::Edit(TextEdit::new(*range, new_text)))
                    }
                    EditRangeDefault::InsertAndReplace { insert, replace } => Some(
                        CompletionTextEdit::InsertAndReplace(lsp_types::InsertReplaceEdit {
                            new_text,
                            insert: *insert,
                            replace: *replace,
                        }),
                    ),
                }
            });
            let (replace_range, insert_range, new_text) = match edit {
                Some(CompletionTextEdit::Edit(edit)) => {
                    (exact_range(rope, edit.range)?, None, edit.new_text)
                }
                Some(CompletionTextEdit::InsertAndReplace(edit)) => (
                    exact_range(rope, edit.replace)?,
                    Some(exact_range(rope, edit.insert)?),
                    edit.new_text,
                ),
                None => (
                    word.clone(),
                    None,
                    item.insert_text
                        .clone()
                        .unwrap_or_else(|| item.label.clone()),
                ),
            };
            Some(to_completion(
                item,
                Placement {
                    replace_range,
                    insert_range,
                    new_text,
                },
                rope,
                &fallback,
            ))
        })
        .collect();
    if parsed.len() != count {
        is_incomplete = true;
    }
    (parsed, is_incomplete)
}

/// A completion list, with the defaults its items may leave out (newer than the protocol types).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionListJson {
    #[serde(default)]
    is_incomplete: bool,
    #[serde(default)]
    items: Vec<Value>,
    item_defaults: Option<ItemDefaultsJson>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemDefaultsJson {
    edit_range: Option<EditRangeDefault>,
    insert_text_format: Option<InsertTextFormat>,
    insert_text_mode: Option<InsertTextMode>,
    data: Option<Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EditRangeDefault {
    Range(lsp_types::Range),
    InsertAndReplace {
        insert: lsp_types::Range,
        replace: lsp_types::Range,
    },
}

/// What a list says for items that leave it out.
struct ItemDefaults {
    format: Option<InsertTextFormat>,
    mode: Option<InsertTextMode>,
    data: Option<Value>,
}

struct Placement {
    replace_range: Range<usize>,
    insert_range: Option<Range<usize>>,
    new_text: String,
}

fn to_completion(
    item: CompletionItem,
    placement: Placement,
    rope: &Rope,
    defaults: &ItemDefaults,
) -> LspCompletion {
    let Placement {
        replace_range,
        insert_range,
        new_text,
    } = placement;
    let detail = item
        .detail
        .clone()
        .filter(|detail| *detail != item.label)
        .or_else(|| {
            item.label_details
                .as_ref()
                .and_then(|details| details.description.clone())
                .filter(|description| *description != item.label)
        })
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "));
    let deprecated = item.deprecated == Some(true)
        || item
            .tags
            .as_ref()
            .is_some_and(|tags| tags.contains(&CompletionItemTag::DEPRECATED));
    let format = item.insert_text_format.or(defaults.format);
    let mode = item.insert_text_mode.or(defaults.mode);
    let additional_edits = item
        .additional_text_edits
        .as_deref()
        .map(|edits| text_edits(rope, edits))
        .unwrap_or_default();
    let mut raw = serde_json::to_value(&item).unwrap_or(Value::Null);
    if item.data.is_none() {
        if let (Some(data), Some(object)) = (defaults.data.clone(), raw.as_object_mut()) {
            object.insert("data".into(), data);
        }
    }
    LspCompletion {
        filter_text: item
            .filter_text
            .clone()
            .unwrap_or_else(|| item.label.clone()),
        label: item.label.split_whitespace().collect::<Vec<_>>().join(" "),
        detail,
        kind: item.kind,
        sort_text: item.sort_text.clone(),
        new_text: new_text.replace("\r\n", "\n"),
        is_snippet: format == Some(InsertTextFormat::SNIPPET),
        replace_range,
        insert_range,
        insert_as_is: mode == Some(InsertTextMode::AS_IS),
        deprecated,
        additional_edits,
        raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
        lsp_types::Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))
    }

    #[test]
    fn places_edits_defaults_and_word_ranges() {
        let rope = Rope::from_str("let va = x.fo\n");
        let items = vec![
            CompletionItem {
                label: "value".into(),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range((0, 4), (0, 6)),
                    "value".into(),
                ))),
                detail: Some("u32".into()),
                ..Default::default()
            },
            CompletionItem {
                label: "foo()".into(),
                insert_text: Some("foo($1)".into()),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            },
            CompletionItem {
                label: "bad".into(),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range((0, 4), (0, 99)),
                    "bad".into(),
                ))),
                ..Default::default()
            },
        ];
        let (parsed, incomplete) = parse_completions(
            serde_json::json!({"isIncomplete": false, "items": items}),
            &rope,
            13,
        );
        assert!(incomplete, "a dropped item marks the list incomplete");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].replace_range, 4..6);
        assert_eq!(parsed[0].detail.as_deref(), Some("u32"));
        assert_eq!(parsed[1].replace_range, 11..13);
        assert_eq!(parsed[1].new_text, "foo($1)");
        assert!(parsed[1].is_snippet);
    }

    #[test]
    fn list_defaults_fill_in_items() {
        let rope = Rope::from_str("x.fo");
        let (parsed, _) = parse_completions(
            serde_json::json!({
                "isIncomplete": true,
                "itemDefaults": {
                    "editRange": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 4}},
                    "insertTextFormat": 2,
                    "data": {"id": 1},
                },
                "items": [{"label": "foo", "textEditText": "foo($0)"}],
            }),
            &rope,
            4,
        );
        assert_eq!(parsed[0].replace_range, 2..4);
        assert_eq!(parsed[0].new_text, "foo($0)");
        assert!(parsed[0].is_snippet);
        assert_eq!(parsed[0].raw["data"]["id"], 1);
    }
}
