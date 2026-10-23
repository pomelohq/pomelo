//! The reply an agent is still writing. Its transcript only gets a block once the block is complete, so the
//! agent's `MessageDisplay` hook, which runs on each batch of finished lines as a reply streams to the
//! screen, appends them here per turn. It is whole lines, not tokens, and display text only.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sessions::{now_ms, read_state, session_dir, with_lock};

const LIVE_DIR: &str = "live";

/// One batch of a reply's lines.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveDelta {
    pub t_ms: u64,
    pub turn: u64,
    /// The agent's id for the message, the same for each of its batches.
    pub message_id: String,
    pub index: u64,
    /// The message's last batch.
    #[serde(rename = "final")]
    pub last: bool,
    pub delta: String,
}

/// One reply message of a turn, as far as it was written.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveMessage {
    pub message_id: String,
    pub text: String,
    #[serde(rename = "final")]
    pub last: bool,
}

/// What a turn's replies said so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTurn {
    pub turn: u64,
    pub messages: Vec<LiveMessage>,
    /// Every message's text, one after the other.
    pub text: String,
    /// Where it comes from: the agent's display hook.
    pub source: String,
}

pub(crate) fn live_path(state: &StateDir, session_id: &str, turn: u64) -> PathBuf {
    session_dir(state, session_id)
        .join(LIVE_DIR)
        .join(format!("{turn}.ndjson"))
}

/// Records one `MessageDisplay` batch against the session's current turn; drops turns before the last one.
pub(crate) fn record_delta(state: &StateDir, body: &Value) -> std::io::Result<()> {
    let Some(session_id) = body.get("session_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let turn = read_state(state, session_id).map_or(0, |record| record.turn);
    let delta = LiveDelta {
        t_ms: now_ms(),
        turn,
        message_id: body
            .get("message_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        index: body.get("index").and_then(Value::as_u64).unwrap_or(0),
        last: body.get("final").and_then(Value::as_bool).unwrap_or(false),
        delta: body
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    let mut line = serde_json::to_string(&delta).map_err(std::io::Error::other)?;
    line.push('\n');
    with_lock(&live_path(state, session_id, turn), |file| {
        file.write_all(line.as_bytes())
    })?;
    if turn >= 2 {
        let stale = live_path(state, session_id, turn - 2);
        if let Err(error) = std::fs::remove_file(&stale) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
        }
    }
    Ok(())
}

/// The batches of `turn` recorded after byte `from`, and the offset to read from next.
pub fn live_deltas(
    state: &StateDir,
    session_id: &str,
    turn: u64,
    from: u64,
) -> (Vec<LiveDelta>, u64) {
    let Ok(mut file) = std::fs::File::open(live_path(state, session_id, turn)) else {
        return (Vec::new(), from);
    };
    let mut text = String::new();
    if file.seek(SeekFrom::Start(from)).is_err() || file.read_to_string(&mut text).is_err() {
        return (Vec::new(), from);
    }
    let complete = text.rfind('\n').map_or(0, |at| at + 1);
    let deltas = text[..complete]
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    (deltas, from + complete as u64)
}

/// What `turn`'s replies said so far, messages in the order they started.
pub fn live_turn(state: &StateDir, session_id: &str, turn: u64) -> LiveTurn {
    let (mut deltas, _) = live_deltas(state, session_id, turn, 0);
    let mut order: Vec<String> = Vec::new();
    for delta in &deltas {
        if !order.contains(&delta.message_id) {
            order.push(delta.message_id.clone());
        }
    }
    deltas.sort_by_key(|delta| {
        (
            order
                .iter()
                .position(|id| *id == delta.message_id)
                .unwrap_or(usize::MAX),
            delta.index,
        )
    });
    let mut messages: Vec<LiveMessage> = Vec::new();
    for delta in deltas {
        match messages.last_mut() {
            Some(message) if message.message_id == delta.message_id => {
                message.text.push_str(&delta.delta);
                message.last |= delta.last;
            }
            _ => messages.push(LiveMessage {
                message_id: delta.message_id,
                text: delta.delta,
                last: delta.last,
            }),
        }
    }
    let text = messages
        .iter()
        .map(|message| message.text.trim_end_matches('\n'))
        .collect::<Vec<_>>()
        .join("\n\n");
    LiveTurn {
        turn,
        messages,
        text,
        source: "display_hook".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn batches_add_up_to_each_message_of_the_turn_and_old_turns_are_dropped() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let batch = |message: &str, index: u64, last: bool, delta: &str| {
            record_delta(
                &state,
                &json!({"session_id": "s", "hook_event_name": "MessageDisplay", "turn_id": "t-1", "message_id": message, "index": index, "final": last, "delta": delta}),
            )
            .expect("batch");
        };
        batch("m1", 0, false, "Here is the plan:\n");
        batch("m1", 1, true, "1. read the code");
        batch("m2", 0, false, "Now the tests:\n");
        let live = live_turn(&state, "s", 0);
        assert_eq!(
            live.messages
                .iter()
                .map(|message| (message.message_id.as_str(), message.last))
                .collect::<Vec<_>>(),
            [("m1", true), ("m2", false)]
        );
        assert_eq!(
            live.text,
            "Here is the plan:\n1. read the code\n\nNow the tests:"
        );
        let (first, next) = live_deltas(&state, "s", 0, 0);
        assert_eq!(first.len(), 3);
        assert_eq!(live_deltas(&state, "s", 0, next).0, Vec::new());

        let dir = session_dir(&state, "s");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("state.json"),
            json!({"schema": "pom.agent/v1", "session_id": "s", "holder": "h", "project": "myproject", "branch": "feat-login", "role": "claude", "driver": "claude", "state": "thinking", "last_event": "UserPromptSubmit", "event_ms": 1, "turn": 2, "turn_state": "running"}).to_string(),
        )
        .expect("state");
        batch("m3", 0, true, "next turn");
        assert!(
            !live_path(&state, "s", 0).exists(),
            "two turns back is dropped"
        );
        assert_eq!(live_turn(&state, "s", 2).text, "next turn");
    }
}
