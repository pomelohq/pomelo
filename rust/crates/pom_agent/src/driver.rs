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
    /// Whether the terminal shows the agent's prompt to trust the folder, which it asks before anything else.
    fn waits_for_trust(&self, screen: &[u8]) -> bool;
    /// The writes that answer that prompt with yes.
    fn accept_trust(&self) -> Vec<Keystrokes>;
}

/// Terminal output as plain lowercase text without spaces: the UI positions words with cursor moves, so
/// the spaces between them are not always in the stream.
fn screen_text(screen: &[u8]) -> String {
    let text = String::from_utf8_lossy(screen);
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            if !c.is_whitespace() {
                plain.extend(c.to_lowercase());
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']' | '_' | 'P') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.peek() == Some(&'\\')) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    plain
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

    fn waits_for_trust(&self, screen: &[u8]) -> bool {
        let text = screen_text(screen);
        // Only the latest screen counts: the prompt stays in the scrollback after it was answered.
        let mut start = text.len().saturating_sub(TRUST_TAIL);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        let tail = &text[start..];
        tail.contains("itrustthisfolder") && tail.contains("entertoconfirm")
    }

    fn accept_trust(&self) -> Vec<Keystrokes> {
        // "No, exit" is preselected; one step down is "Yes, I trust this folder". Separate writes, or the
        // escape that starts the arrow key reads as Esc, which cancels the prompt and exits.
        vec![
            Keystrokes {
                bytes: b"\x1b[B".to_vec(),
                pause_ms: TRUST_PAUSE_MS,
            },
            Keystrokes {
                bytes: b"\r".to_vec(),
                pause_ms: 0,
            },
        ]
    }
}

/// How much of the end of the screen text the trust prompt check reads.
const TRUST_TAIL: usize = 600;
const TRUST_PAUSE_MS: u64 = 300;

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

    /// What the agent drew at start in a folder it had never seen (cursor moves stand in for spaces).
    const TRUST_SCREEN: &[u8] = b"\x1b[2J\x1b[1;1HAccessing\x1b[1Cworkspace:\r\n/private/tmp/myproject/workspace--feat-login\r\nQuick\x1b[1Csafety\x1b[1Ccheck:\x1b[1CIs\x1b[1Cthis\x1b[1Ca\x1b[1Cproject\x1b[1Cyou\x1b[1Ccreated\x1b[1Cor\x1b[1Cone\x1b[1Cyou\x1b[1Ctrust?\r\n\x1b[38;5;153m\xe2\x9d\xaf 1. No, exit\x1b[39m\r\n  2. Yes, I trust this folder\r\n\x1b[2mEnter to confirm \xc2\xb7 Esc to cancel\x1b[22m\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

    #[test]
    fn the_trust_prompt_is_recognized_and_answered_with_down_then_enter() {
        let claude = driver("claude").expect("claude");
        assert!(claude.waits_for_trust(TRUST_SCREEN));
        let mut answered = TRUST_SCREEN.to_vec();
        answered.extend_from_slice(
            b"\x1b[2J\x1b[1;1H> Try \"write a test for <filepath>\"\r\n"
                .repeat(40)
                .as_slice(),
        );
        assert!(
            !claude.waits_for_trust(&answered),
            "a prompt answered earlier in the scrollback does not count"
        );
        let keys = claude.accept_trust();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].bytes, b"\x1b[B".to_vec());
        assert!(keys[0].pause_ms > 0);
        assert_eq!(keys[1].bytes, b"\r".to_vec());
    }
}
