//! A session's turns: where each one starts and ends in its transcript (from the byte lengths its hook events
//! recorded), who sent it, how it ended, and what happened in it.

use std::path::Path;

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

use crate::sessions::{events, read_state, session_dir, EventLine};
use crate::transcript::{parse, turn_between, TurnContent};

/// Who sent a turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Human,
    Orchestrator,
    Agent,
}

/// What pom recorded when it sent a turn, `turns/<n>.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnMeta {
    pub origin: Origin,
    /// The session that sent it, for a turn one agent sent another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driven_by: Option<String>,
    pub depth: u64,
    pub sent_ms: u64,
}

pub fn write_turn_meta(
    state: &StateDir,
    session_id: &str,
    turn: u64,
    meta: &TurnMeta,
) -> std::io::Result<()> {
    let blob = serde_json::to_vec(meta).map_err(std::io::Error::other)?;
    pom_paths::write_atomic(
        &session_dir(state, session_id)
            .join("turns")
            .join(format!("{turn}.json")),
        &blob,
        0o644,
    )
}

/// The turn's record, or a human's when pom did not send it.
pub fn turn_meta(state: &StateDir, session_id: &str, turn: u64) -> TurnMeta {
    std::fs::read_to_string(
        session_dir(state, session_id)
            .join("turns")
            .join(format!("{turn}.json")),
    )
    .ok()
    .and_then(|text| serde_json::from_str(&text).ok())
    .unwrap_or_default()
}

/// One turn's place in the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnSpan {
    pub turn: u64,
    /// Where the transcript stood just before the prompt; `None` when nothing was recorded before it.
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub stop_reason: Option<&'static str>,
}

pub fn turn_spans(lines: &[EventLine]) -> Vec<TurnSpan> {
    let mut spans: Vec<TurnSpan> = Vec::new();
    let mut before: Option<u64> = None;
    for line in lines {
        match line.event.as_str() {
            "UserPromptSubmit" => spans.push(TurnSpan {
                turn: line.turn,
                start: before,
                end: None,
                stop_reason: None,
            }),
            "Stop" | "Interrupted" => {
                if let Some(span) = spans.last_mut().filter(|span| span.stop_reason.is_none()) {
                    span.end = line.transcript_bytes;
                    span.stop_reason = Some(if line.event == "Stop" {
                        "end_turn"
                    } else {
                        "cancelled"
                    });
                }
            }
            _ => {}
        }
        if line.transcript_bytes.is_some() {
            before = line.transcript_bytes;
        }
    }
    spans
}

/// The content of a turn, given its span in the transcript text.
pub fn turn_content(text: &str, span: &TurnSpan) -> Option<TurnContent> {
    let end = span.end.map(|end| end as usize);
    let upto = &text[..end.unwrap_or(text.len()).min(text.len())];
    match span.start {
        Some(start) => turn_between(text, start as usize, end).or_else(|| parse(upto).pop()),
        None => parse(upto).pop(),
    }
}

/// A turn as `read` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecordedTurn {
    pub turn: u64,
    pub origin: Origin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driven_by: Option<String>,
    pub depth: u64,
    /// `end_turn`, `cancelled`, `died`, or `running` while it is not over.
    pub stop_reason: String,
    #[serde(flatten)]
    pub content: TurnContent,
}

/// The turns of a session `select` picks, from its log and transcript. A running turn of a session whose
/// holder is gone reads as `died`.
pub fn read_turns(
    state: &StateDir,
    session_id: &str,
    select: impl Fn(u64) -> bool,
    alive: bool,
    full: bool,
) -> Vec<RecordedTurn> {
    let Some(record) = read_state(state, session_id) else {
        return Vec::new();
    };
    let text = if record.transcript.is_empty() {
        String::new()
    } else {
        std::fs::read_to_string(Path::new(&record.transcript)).unwrap_or_default()
    };
    turn_spans(&events(state, session_id))
        .into_iter()
        .filter(|span| select(span.turn))
        .map(|span| {
            let meta = turn_meta(state, session_id, span.turn);
            let content = turn_content(&text, &span).unwrap_or_default();
            let content = if full { content } else { content.truncated() };
            RecordedTurn {
                turn: span.turn,
                origin: meta.origin,
                driven_by: meta.driven_by,
                depth: meta.depth,
                stop_reason: span
                    .stop_reason
                    .unwrap_or(if alive { "running" } else { "died" })
                    .to_string(),
                content,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hooks::AgentState;
    use crate::identity::Identity;
    use crate::sessions::{record_event, SessionEvent};

    const FIXTURE: &str = include_str!("../tests/fixtures/transcript.jsonl");

    fn identity() -> Identity {
        Identity {
            holder: "ws-myproject-feat-login-claude-raw".into(),
            role: "claude".into(),
            project: "myproject".into(),
            branch: "feat-login".into(),
            driver: "claude".into(),
        }
    }

    #[test]
    fn each_recorded_turn_reads_its_own_slice_of_a_growing_transcript() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let transcript = temp.path().join("s1.jsonl");
        let second = FIXTURE.find("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"run the tests\"").expect("second prompt");
        let first = FIXTURE
            .find("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"list the files")
            .expect("first prompt");
        let hook = |name: &str, upto: usize, agent_state: AgentState| {
            std::fs::write(&transcript, &FIXTURE[..upto]).expect("transcript");
            let event = SessionEvent {
                session_id: "s1".into(),
                event: name.into(),
                state: Some(agent_state),
                transcript: transcript.to_string_lossy().into_owned(),
                ..SessionEvent::default()
            };
            record_event(&state, &identity(), &event).expect("event");
        };
        hook("SessionStart", first, AgentState::Idle);
        hook("UserPromptSubmit", first, AgentState::Thinking);
        hook("Stop", second, AgentState::Idle);
        hook("UserPromptSubmit", second, AgentState::Thinking);
        std::fs::write(&transcript, FIXTURE).expect("transcript");
        write_turn_meta(
            &state,
            "s1",
            2,
            &TurnMeta {
                origin: Origin::Orchestrator,
                driven_by: None,
                depth: 0,
                sent_ms: 1,
            },
        )
        .expect("meta");

        let turns = read_turns(&state, "s1", |_| true, true, false);
        assert_eq!(turns.len(), 2);
        assert_eq!(
            (
                turns[0].content.prompt.as_str(),
                turns[0].stop_reason.as_str(),
                turns[0].origin
            ),
            ("list the files in src", "end_turn", Origin::Human)
        );
        assert_eq!(turns[0].content.tool_calls().len(), 1);
        assert_eq!(
            (
                turns[1].content.prompt.as_str(),
                turns[1].stop_reason.as_str(),
                turns[1].origin
            ),
            ("run the tests", "running", Origin::Orchestrator)
        );
        let dead = read_turns(&state, "s1", |turn| turn == 2, false, false);
        assert_eq!(dead[0].stop_reason, "died");
    }
}
