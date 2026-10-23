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
    /// The trust prompt's choices, if the rendered screen shows them with one highlighted.
    fn trust_dialog(&self, lines: &[String]) -> Option<TrustDialog>;
    /// The next write toward a yes: Enter only while yes is highlighted, otherwise a move of the highlight.
    fn answer_trust(&self, dialog: TrustDialog) -> Keystrokes;
    /// The agent's own permission dialog, if the rendered screen shows one.
    fn permission_dialog(&self, lines: &[String]) -> Option<PermissionDialog>;
    /// The writes that answer that dialog: its highlighted first choice, or its deny key.
    fn answer_permission(&self, allow: bool) -> Vec<Keystrokes>;
    /// Whether the rendered screen shows any dialog that Enter would answer, known or not.
    fn shows_choices(&self, lines: &[String]) -> bool;
    /// Whether the agent's input box still holds `text`, typed but not yet submitted.
    fn holds_unsent(&self, lines: &[String], text: &str) -> bool;
}

/// The agent's permission dialog as it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionDialog {
    /// The highlight is on the first choice, a plain yes for this one call.
    pub first_choice_selected: bool,
}

/// The agent's folder trust prompt as it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustDialog {
    pub yes_selected: bool,
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

    fn permission_dialog(&self, lines: &[String]) -> Option<PermissionDialog> {
        let bottom = &lines[lines.len().saturating_sub(DIALOG_TAIL_LINES)..];
        let question = bottom
            .iter()
            .rposition(|line| line.trim() == DIALOG_QUESTION)?;
        // The wording of the later choices changes between versions; numbered lines ending in a plain no do not.
        let choices: Vec<(bool, &str)> = bottom[question + 1..]
            .iter()
            .take_while(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let selected = line.contains(SELECTION_MARK);
                let text = line.trim().trim_start_matches(SELECTION_MARK).trim_start();
                let (number, choice) = text.split_once(". ")?;
                number
                    .chars()
                    .all(|c| c.is_ascii_digit())
                    .then_some((selected, choice))
            })
            .take(DIALOG_CHOICES_MAX)
            .collect();
        let (_, first) = choices.first()?;
        let (_, last) = choices.last()?;
        if choices.len() < 2 || *first != DIALOG_PLAIN_YES || !last.starts_with("No") {
            return None;
        }
        let selected_at = choices.iter().position(|(selected, _)| *selected)?;
        Some(PermissionDialog {
            first_choice_selected: selected_at == 0,
        })
    }

    fn answer_permission(&self, allow: bool) -> Vec<Keystrokes> {
        // Enter takes the highlighted choice, checked to be the plain yes; Esc is the dialog's own deny key.
        vec![Keystrokes {
            bytes: if allow {
                b"\r".to_vec()
            } else {
                b"\x1b".to_vec()
            },
            pause_ms: 0,
        }]
    }

    fn trust_dialog(&self, lines: &[String]) -> Option<TrustDialog> {
        let bottom = &lines[lines.len().saturating_sub(DIALOG_TAIL_LINES)..];
        let yes_at = bottom
            .iter()
            .rposition(|line| line.contains(TRUST_YES_CHOICE))?;
        let no = bottom.get(yes_at.checked_sub(1)?)?;
        if !no.contains(TRUST_NO_CHOICE) {
            return None;
        }
        let yes = &bottom[yes_at];
        match (no.contains(SELECTION_MARK), yes.contains(SELECTION_MARK)) {
            (true, false) => Some(TrustDialog {
                yes_selected: false,
            }),
            (false, true) => Some(TrustDialog { yes_selected: true }),
            _ => None,
        }
    }

    fn answer_trust(&self, dialog: TrustDialog) -> Keystrokes {
        // Alone in its write, or the arrow's leading escape can read as Esc, which exits.
        Keystrokes {
            bytes: if dialog.yes_selected {
                b"\r".to_vec()
            } else {
                b"\x1b[B".to_vec()
            },
            pause_ms: 0,
        }
    }

    fn shows_choices(&self, lines: &[String]) -> bool {
        let bottom = &lines[lines.len().saturating_sub(DIALOG_TAIL_LINES)..];
        self.permission_dialog(lines).is_some()
            || self.trust_dialog(lines).is_some()
            || bottom.iter().any(|line| {
                DIALOG_FOOTERS.iter().any(|footer| line.contains(footer))
                    || line
                        .trim_start()
                        .strip_prefix(SELECTION_MARK)
                        .and_then(|rest| rest.trim_start().split_once(". "))
                        .is_some_and(|(number, _)| {
                            !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                        })
            })
    }

    fn holds_unsent(&self, lines: &[String], text: &str) -> bool {
        // The input box is the prompt line right under a rule; a sent turn's echo above it has no rule.
        let Some(input) = lines.iter().enumerate().rev().find_map(|(at, line)| {
            let rest = line.trim_start().strip_prefix(SELECTION_MARK)?;
            let ruled = at
                .checked_sub(1)
                .and_then(|above| lines.get(above))
                .is_some_and(|above| {
                    let above = above.trim();
                    !above.is_empty() && above.chars().all(|c| c == INPUT_RULE)
                });
            ruled.then(|| rest.split_whitespace().collect::<Vec<_>>().join(" "))
        }) else {
            return false;
        };
        if input.starts_with(PASTED_PLACEHOLDER) {
            return true;
        }
        let first_line = text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default();
        let wanted: String = first_line
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(UNSENT_PREFIX_CHARS)
            .collect();
        !wanted.is_empty() && input.starts_with(&wanted)
    }
}

/// Footers only a dialog draws under its choices.
const DIALOG_FOOTERS: [&str; 2] = ["Esc to cancel", "Enter to confirm"];
const INPUT_RULE: char = '\u{2500}';
/// What the input box shows in place of a long paste.
const PASTED_PLACEHOLDER: &str = "[Pasted text";
/// The input box wraps a long prompt, so only its start is compared.
const UNSENT_PREFIX_CHARS: usize = 16;

const DIALOG_QUESTION: &str = "Do you want to proceed?";
const DIALOG_PLAIN_YES: &str = "Yes";
const SELECTION_MARK: char = '\u{276F}';
const DIALOG_TAIL_LINES: usize = 30;
const DIALOG_CHOICES_MAX: usize = 9;

/// How much of the end of the screen text the trust prompt check reads.
const TRUST_TAIL: usize = 600;
const TRUST_YES_CHOICE: &str = "Yes, I trust this folder";
const TRUST_NO_CHOICE: &str = "No, exit";

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
    use crate::screen::rendered_lines;
    use crate::screen::tests::rendered_at_holder_size;

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
    fn the_trust_prompt_is_recognized_only_on_the_latest_screen() {
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
    }

    #[test]
    fn enter_answers_the_trust_prompt_only_while_yes_is_highlighted() {
        let claude = driver("claude").expect("claude");
        let screen = |yes: bool| -> Vec<String> {
            let mark = |on: bool| if on { "\u{276F}" } else { " " };
            vec![
                "Accessing workspace:".to_string(),
                "/private/tmp/myproject/workspace--feat-login".into(),
                "Quick safety check: Is this a project you created or one you trust?".into(),
                format!("{} No, exit", mark(!yes)),
                format!("{} Yes, I trust this folder", mark(yes)),
                String::new(),
                "Enter to confirm - Esc to cancel".into(),
            ]
        };
        let at_no = claude.trust_dialog(&screen(false));
        assert_eq!(
            at_no,
            Some(TrustDialog {
                yes_selected: false
            })
        );
        let step = claude.answer_trust(at_no.expect("dialog"));
        assert_eq!(step.bytes, b"\x1b[B".to_vec(), "never Enter on no");

        let at_yes = claude.trust_dialog(&screen(true));
        assert_eq!(at_yes, Some(TrustDialog { yes_selected: true }));
        assert_eq!(
            claude.answer_trust(at_yes.expect("dialog")).bytes,
            b"\r".to_vec()
        );

        let mut unmarked = screen(false);
        unmarked[3] = "  No, exit".into();
        assert_eq!(
            claude.trust_dialog(&unmarked),
            None,
            "no highlight drawn yet, so nothing to answer"
        );
        assert_eq!(
            claude.trust_dialog(&["> Try \"write a test\"".to_string()]),
            None
        );
    }

    /// The agent's own output at a folder it had never seen, as the holder replays it: one frame, a full clear
    /// and a redraw, words placed by column, the cursor parked on the highlighted choice.
    const TRUST_CAPTURE: &[u8] = include_bytes!("../tests/fixtures/trust-screen-2.1.309.bin");
    /// The agent's output up to its four-choice permission dialog in its full-screen layout, with the
    /// spinner it keeps redrawing above the dialog afterwards.
    const PERMISSION_CAPTURE: &[u8] =
        include_bytes!("../tests/fixtures/permission-screen-2.1.309.bin");

    #[test]
    fn the_captured_trust_prompt_reads_as_no_highlighted_above_yes() {
        let claude = driver("claude").expect("claude");
        assert!(claude.waits_for_trust(TRUST_CAPTURE));
        for lines in [
            rendered_lines(TRUST_CAPTURE),
            rendered_at_holder_size(TRUST_CAPTURE),
        ] {
            let no_at = lines
                .iter()
                .position(|line| line.trim_start() == "\u{276F} No, exit")
                .expect("no choice, highlighted");
            assert_eq!(lines[no_at + 1].trim(), "Yes, I trust this folder");
            assert_eq!(
                claude.trust_dialog(&lines),
                Some(TrustDialog {
                    yes_selected: false
                })
            );
        }
        let mut moved = TRUST_CAPTURE.to_vec();
        moved.extend_from_slice("\x1b[2G \x1b[1B\x1b[2G\u{276F}\x1b[1A".as_bytes());
        assert_eq!(
            claude.trust_dialog(&rendered_lines(&moved)),
            Some(TrustDialog { yes_selected: true }),
            "the highlight moved down onto yes"
        );
    }

    #[test]
    fn the_captured_permission_dialog_reads_with_its_first_choice_highlighted() {
        let claude = driver("claude").expect("claude");
        for lines in [
            rendered_lines(PERMISSION_CAPTURE),
            rendered_at_holder_size(PERMISSION_CAPTURE),
        ] {
            assert!(lines.iter().any(|line| line.trim() == "\u{276F} 1. Yes"));
            assert!(lines.iter().any(|line| line.trim() == "4. No"));
            assert_eq!(
                claude.permission_dialog(&lines),
                Some(PermissionDialog {
                    first_choice_selected: true
                })
            );
        }
    }

    #[test]
    fn a_permission_dialog_placed_by_columns_with_the_cursor_above_it_is_found() {
        let claude = driver("claude").expect("claude");
        let choices: [&[&str]; 4] = [
            &["Yes"],
            &[
                "Yes,",
                "and",
                "always",
                "allow",
                "access",
                "to",
                "/p/workspace--feat-login",
            ],
            &["Yes,", "and", "switch", "to", "auto", "mode"],
            &["No"],
        ];
        let screen = |selected: usize| -> Vec<u8> {
            let mut out = String::from(
                "\x1b[2J\x1b[H\r\x1b[2B\x1b[2GDo\x1b[5Gyou\x1b[9Gwant\x1b[14Gto\x1b[17Gproceed?",
            );
            for (index, words) in choices.iter().enumerate() {
                out.push_str("\r\x1b[1B");
                if index == selected {
                    out.push_str("\x1b[2G\u{276F}");
                }
                out.push_str(&format!("\x1b[4G{}.", index + 1));
                let mut column = 7;
                for word in *words {
                    out.push_str(&format!("\x1b[{column}G{word}"));
                    column += word.len() + 1;
                }
            }
            out.push_str("\r\x1b[2B\x1b[2GEsc\x1b[6Gto\x1b[9Gcancel\x1b[H\x1b[3B");
            out.into_bytes()
        };
        assert_eq!(
            claude.permission_dialog(&rendered_lines(&screen(0))),
            Some(PermissionDialog {
                first_choice_selected: true
            })
        );
        for selected in 1..4 {
            assert_eq!(
                claude.permission_dialog(&rendered_lines(&screen(selected))),
                Some(PermissionDialog {
                    first_choice_selected: false
                }),
                "choice {} is highlighted",
                selected + 1
            );
        }
    }

    fn permission_screen(head: &[&str], choices: &[&str], selected: usize) -> Vec<String> {
        let mut lines: Vec<String> = head.iter().map(|line| line.to_string()).collect();
        lines.extend(choices.iter().enumerate().map(|(index, choice)| {
            if index == selected {
                format!("\u{276F} {choice}")
            } else {
                format!("  {choice}")
            }
        }));
        lines
    }

    #[test]
    fn the_permission_dialog_is_found_with_where_its_highlight_is() {
        let claude = driver("claude").expect("claude");
        let head = [
            "Bash command",
            "Tip: auto mode handles these prompts for you - choose \"switch to auto mode\" below",
            "Create empty person.txt file",
            "----",
            "touch person.txt && echo ok",
            "----",
            "Do you want to proceed?",
        ];
        let choices = [
            "1. Yes",
            "2. Yes, and always allow access to /p/workspace--feat-login from this project",
            "3. Yes, and switch to auto mode - auto mode handles these prompts for you",
            "4. No",
        ];
        let screen = |selected: usize| {
            let mut lines = permission_screen(&head, &choices, selected);
            lines.push("Esc to cancel - Tab to amend".into());
            lines
        };
        assert_eq!(
            claude.permission_dialog(&screen(0)),
            Some(PermissionDialog {
                first_choice_selected: true
            })
        );
        for selected in 1..4 {
            assert_eq!(
                claude.permission_dialog(&screen(selected)),
                Some(PermissionDialog {
                    first_choice_selected: false
                }),
                "Enter on choice {} would allow more than this call, or decline it",
                selected + 1
            );
        }
        assert_eq!(claude.permission_dialog(&["> hello".to_string()]), None);
        assert_eq!(
            claude.permission_dialog(&screen(0)[..head.len()]),
            None,
            "a question without its choices is no dialog"
        );
        assert_eq!(claude.answer_permission(true)[0].bytes, b"\r".to_vec());
        assert_eq!(claude.answer_permission(false)[0].bytes, b"\x1b".to_vec());
    }

    #[test]
    fn the_older_permission_dialog_with_its_long_deny_choice_is_found_too() {
        let claude = driver("claude").expect("claude");
        let head = ["Bash command", "", "  make test", "Do you want to proceed?"];
        let choices = [
            "1. Yes",
            "2. Yes, and don't ask again for make commands in /p/workspace--feat-login",
            "3. No, and tell Claude what to do differently (esc)",
        ];
        assert_eq!(
            claude.permission_dialog(&permission_screen(&head, &choices, 0)),
            Some(PermissionDialog {
                first_choice_selected: true
            })
        );
        assert_eq!(
            claude.permission_dialog(&permission_screen(&head, &choices, 1)),
            Some(PermissionDialog {
                first_choice_selected: false
            }),
            "Enter there would allow every later call too"
        );
    }
}
