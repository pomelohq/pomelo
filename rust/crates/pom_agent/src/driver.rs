//! What differs from one coding-agent CLI to the next, behind one trait: how a turn is typed into its
//! terminal, how it is interrupted, and where and how its transcript is read. The commands only talk to this.

use std::path::{Path, PathBuf};

use crate::identity::{CLAUDE_DRIVER, NO_DRIVER};
use crate::transcript::{self, TurnContent};

/// One write to the agent's terminal and how long to wait after it before the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keystrokes {
    pub bytes: Vec<u8>,
    pub pause_ms: u64,
}

pub trait AgentDriver: Send + Sync {
    fn name(&self) -> &'static str;
    /// The writes that submit `text` as one user turn.
    fn submit(&self, text: &str) -> Vec<Keystrokes>;
    /// The write that stops the current turn.
    fn interrupt(&self) -> Vec<u8>;
    /// The session's transcript file.
    fn transcript(&self, home: &Path, cwd: &Path, session_id: &str) -> Option<PathBuf>;
    /// The turns in a slice of a transcript.
    fn turns(&self, text: &str) -> Vec<TurnContent>;
}

/// Claude Code's terminal UI.
pub struct ClaudeDriver;

/// Bracketed paste markers: the whole text arrives as one paste, so its newlines do not submit it early.
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
/// The UI folds a paste in before it takes the Enter that submits it.
const SUBMIT_PAUSE_MS: u64 = 60;

impl AgentDriver for ClaudeDriver {
    fn name(&self) -> &'static str {
        CLAUDE_DRIVER
    }

    fn submit(&self, text: &str) -> Vec<Keystrokes> {
        // An escape inside the text could end the paste early and type the rest as keys.
        let text = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\x1b', "");
        let mut paste = PASTE_START.to_vec();
        paste.extend_from_slice(text.trim_end_matches('\n').as_bytes());
        paste.extend_from_slice(PASTE_END);
        vec![
            Keystrokes {
                bytes: paste,
                pause_ms: SUBMIT_PAUSE_MS,
            },
            Keystrokes {
                bytes: b"\r".to_vec(),
                pause_ms: 0,
            },
        ]
    }

    fn interrupt(&self) -> Vec<u8> {
        b"\x1b".to_vec()
    }

    fn transcript(&self, home: &Path, cwd: &Path, session_id: &str) -> Option<PathBuf> {
        crate::launch::transcript_path(home, cwd, session_id)
    }

    fn turns(&self, text: &str) -> Vec<TurnContent> {
        transcript::parse(text)
    }
}

static CLAUDE: ClaudeDriver = ClaudeDriver;

/// The driver for a session's `driver` name; `None` for a CLI that cannot be driven yet.
pub fn driver(name: &str) -> Option<&'static dyn AgentDriver> {
    match name {
        CLAUDE_DRIVER => Some(&CLAUDE),
        NO_DRIVER => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_line_turn_is_one_paste_then_enter() {
        let claude = driver("claude").expect("claude");
        let writes = claude.submit("first line\r\nsecond line\n");
        assert_eq!(writes.len(), 2);
        assert_eq!(
            writes[0].bytes,
            b"\x1b[200~first line\nsecond line\x1b[201~".to_vec()
        );
        assert!(writes[0].pause_ms > 0);
        assert_eq!(writes[1].bytes, b"\r".to_vec());
        assert_eq!(claude.interrupt(), b"\x1b".to_vec());
        assert!(driver("none").is_none());
        assert!(driver("codex").is_none());
    }
}
