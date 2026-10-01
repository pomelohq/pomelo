//! What an agent did in a turn, read from its transcript (an append-only JSONL file) rather than its screen:
//! the prompt, the assistant's text, each tool call with its input and result, the stop reason and the token
//! usage. A turn is the slice of the file between two byte offsets the hooks record.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

/// Tool results longer than this are cut unless the reader asks for everything.
pub const RESULT_PREVIEW: usize = 2048;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub is_error: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    Text { text: String },
    ToolCall(ToolCall),
}

/// One turn's content.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TurnContent {
    pub prompt: String,
    pub items: Vec<Item>,
    pub usage: Usage,
    /// The model's own reason for its last message (`end_turn`, `tool_use`, `max_tokens`...).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model_stop: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub started_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ended_at: String,
}

impl TurnContent {
    pub fn tool_calls(&self) -> Vec<&ToolCall> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::ToolCall(call) => Some(call),
                Item::Text { .. } => None,
            })
            .collect()
    }

    /// The assistant's text, in order.
    pub fn text(&self) -> String {
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Text { text } => Some(text.as_str()),
                Item::ToolCall(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Cuts every tool result to the preview length.
    pub fn truncated(mut self) -> TurnContent {
        for item in &mut self.items {
            if let Item::ToolCall(call) = item {
                if let Some(result) = &mut call.result {
                    if result.len() > RESULT_PREVIEW {
                        let mut end = RESULT_PREVIEW;
                        while !result.is_char_boundary(end) {
                            end -= 1;
                        }
                        result.truncate(end);
                        call.truncated = true;
                    }
                }
            }
        }
        self
    }
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn flag(entry: &Value, name: &str) -> bool {
    entry.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// A prompt someone typed or sent, as opposed to tool results and the agent's own bookkeeping entries.
fn real_prompt(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("user")
        || [
            "isMeta",
            "isCompactSummary",
            "isVisibleInTranscriptOnly",
            "isSidechain",
        ]
        .iter()
        .any(|name| flag(entry, name))
    {
        return None;
    }
    let content = entry.get("message")?.get("content")?;
    if let Value::Array(parts) = content {
        if parts
            .iter()
            .any(|part| part.get("type").and_then(Value::as_str) == Some("tool_result"))
        {
            return None;
        }
    }
    let text = text_of(content);
    (!text.trim().is_empty()).then_some(text)
}

fn usage_of(message: &Value) -> Usage {
    let number = |key: &str| {
        message
            .get("usage")
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        input_tokens: number("input_tokens"),
        output_tokens: number("output_tokens"),
        cache_read: number("cache_read_input_tokens"),
        cache_write: number("cache_creation_input_tokens"),
    }
}

/// Every turn in a slice of a transcript, split at real prompts. Entries before the first prompt (the tail of
/// an earlier turn) are dropped.
pub fn parse(text: &str) -> Vec<TurnContent> {
    let mut turns: Vec<TurnContent> = Vec::new();
    // One assistant message is written as several entries with the same id; its usage counts once.
    let mut usage_by_message: Vec<HashMap<String, Usage>> = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if flag(&entry, "isSidechain") {
            continue;
        }
        let timestamp = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(prompt) = real_prompt(&entry) {
            turns.push(TurnContent {
                prompt,
                started_at: timestamp.clone(),
                ended_at: timestamp,
                ..TurnContent::default()
            });
            usage_by_message.push(HashMap::new());
            continue;
        }
        let (Some(turn), Some(usages)) = (turns.last_mut(), usage_by_message.last_mut()) else {
            continue;
        };
        if !timestamp.is_empty() {
            turn.ended_at = timestamp;
        }
        let Some(message) = entry.get("message") else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                if let Some(id) = message.get("id").and_then(Value::as_str) {
                    usages.insert(id.to_string(), usage_of(message));
                }
                if let Some(stop) = message.get("stop_reason").and_then(Value::as_str) {
                    turn.model_stop = stop.to_string();
                }
                for part in message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                            if !text.trim().is_empty() {
                                turn.items.push(Item::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                        Some("tool_use") => turn.items.push(Item::ToolCall(ToolCall {
                            id: part
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                            name: part
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                            input: part.get("input").cloned().unwrap_or(Value::Null),
                            result: None,
                            is_error: false,
                            truncated: false,
                        })),
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for part in message
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_result"))
                {
                    let id = part
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let result = text_of(part.get("content").unwrap_or(&Value::Null));
                    let is_error = flag(part, "is_error");
                    if let Some(Item::ToolCall(call)) = turn
                        .items
                        .iter_mut()
                        .rev()
                        .find(|item| matches!(item, Item::ToolCall(call) if call.id == id))
                    {
                        call.result = Some(result);
                        call.is_error = is_error;
                    }
                }
            }
            _ => {}
        }
    }
    for (turn, usages) in turns.iter_mut().zip(usage_by_message) {
        for usage in usages.values() {
            turn.usage.add(usage);
        }
    }
    turns
}

/// The turn that starts in `text[from..to]`: the first prompt in that range and everything after it up to
/// `to` (or the end).
pub fn turn_between(text: &str, from: usize, to: Option<usize>) -> Option<TurnContent> {
    let end = to.unwrap_or(text.len()).min(text.len());
    let start = from.min(end);
    let start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    parse(&text[start..end]).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/transcript.jsonl");

    #[test]
    fn a_transcript_reads_as_turns_with_text_tool_calls_results_and_usage() {
        let turns = parse(FIXTURE);
        assert_eq!(
            turns.len(),
            2,
            "the compact summary and tool results are not prompts"
        );
        let first = &turns[0];
        assert_eq!(first.prompt, "list the files in src");
        let calls = first.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "Bash");
        assert_eq!(calls[0].input["command"], "ls src");
        assert_eq!(calls[0].result.as_deref(), Some("main.rs\nlib.rs"));
        assert!(!calls[0].is_error);
        assert_eq!(first.text(), "Two files: main.rs and lib.rs.");
        assert_eq!(first.model_stop, "end_turn");
        assert_eq!(
            first.usage,
            Usage {
                input_tokens: 30,
                output_tokens: 25,
                cache_read: 100,
                cache_write: 7
            },
            "a message split over two entries counts once"
        );
        let second = &turns[1];
        assert_eq!(second.prompt, "run the tests");
        assert!(second.tool_calls()[0].is_error);
        assert_eq!(second.ended_at, "2026-10-02T10:01:09.000Z");
    }

    #[test]
    fn long_results_are_cut_unless_asked_for_and_a_turn_slice_starts_at_its_prompt() {
        let long = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"go\"}}}}\n{{\"type\":\"assistant\",\"message\":{{\"id\":\"m\",\"content\":[{{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"Read\",\"input\":{{}}}}]}}}}\n{{\"type\":\"user\",\"message\":{{\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"t\",\"content\":\"{}\"}}]}}}}\n",
            "x".repeat(5000)
        );
        let turn = parse(&long).remove(0).truncated();
        let call = turn.tool_calls()[0].clone();
        assert!(call.truncated);
        assert_eq!(call.result.map(|result| result.len()), Some(RESULT_PREVIEW));

        let second_prompt = FIXTURE.find("run the tests").expect("prompt");
        let turn = turn_between(FIXTURE, second_prompt, None).expect("turn");
        assert_eq!(turn.prompt, "run the tests");
        assert_eq!(
            turn_between(FIXTURE, 0, Some(second_prompt))
                .map(|turn| turn.prompt)
                .as_deref(),
            Some("list the files in src")
        );
    }
}
