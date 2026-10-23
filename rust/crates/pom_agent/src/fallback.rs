//! The screen-reading fallback: keys pom types into an agent's terminal from what it reads there, when no
//! hook can carry the answer. It never types an allow, types nothing while a person drives the session,
//! reads the screen again right before every key, and writes each thing it did or held back to the
//! workspace log next to the permission answers.

use pom_paths::StateDir;
use serde_json::{json, Value};

use crate::lease::{log, Lease, LeaseClass};

pub(crate) const FALLBACK_LOG_EVENT: &str = "screen_fallback";

pub(crate) const PERMISSION_ALLOW: &str = "permission_allow";
pub(crate) const PERMISSION_DENY: &str = "permission_deny";
pub(crate) const TRUST: &str = "trust";
pub(crate) const SUBMIT_RETRY: &str = "submit_retry";

pub(crate) const WHY_ALLOW_NEEDS_HOOK: &str = "an allow goes only through the agent's hook";
pub(crate) const WHY_PERSON_DRIVES: &str = "a person drives the session";
pub(crate) const WHY_CHANGED: &str = "the screen changed before the key was sent";
pub(crate) const WHY_UNREADABLE: &str = "the screen could not be read";

/// What a fresh read found right before a key was to be sent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Recheck {
    Sent,
    Changed,
    Unreadable(String),
}

pub(crate) fn person_drives(lease: &Lease) -> bool {
    lease.class == LeaseClass::Human
}

/// Reads the screen again and sends only while it still shows exactly what the decision was made on.
pub(crate) fn send_if_unchanged<T: PartialEq, E>(
    read: impl FnOnce() -> std::io::Result<Vec<String>>,
    find: impl Fn(&[String]) -> Option<T>,
    decided_on: &T,
    send: impl FnOnce() -> Result<(), E>,
) -> Result<Recheck, E> {
    let lines = match read() {
        Ok(lines) => lines,
        Err(error) => return Ok(Recheck::Unreadable(error.to_string())),
    };
    if find(&lines).as_ref() != Some(decided_on) {
        return Ok(Recheck::Changed);
    }
    send()?;
    Ok(Recheck::Sent)
}

/// A readable name for the keys the fallback sends.
pub(crate) fn key_names(bytes: &[u8]) -> String {
    match bytes {
        b"\x1b" => "esc".into(),
        b"\r" => "enter".into(),
        b"\x1b[B" => "down".into(),
        other => other
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// One fallback action as the workspace log keeps it.
pub(crate) struct Entry<'a> {
    pub holder: &'a str,
    pub role: &'a str,
    pub session_id: &'a str,
    pub action: &'a str,
    pub seen: &'a [String],
    /// The keys sent, or `None` when nothing was typed.
    pub sent: Option<String>,
    pub why: &'a str,
}

impl Entry<'_> {
    fn line(&self) -> Value {
        json!({
            "event": FALLBACK_LOG_EVENT,
            "holder": self.holder,
            "role": self.role,
            "session_id": self.session_id,
            "action": self.action,
            "seen": self.seen,
            "sent": self.sent,
            "skipped": self.sent.is_none(),
            "why": self.why,
        })
    }
}

pub(crate) fn record(state: &StateDir, project: &str, branch: &str, entry: &Entry<'_>) {
    if let Err(error) = log(state, project, branch, entry.line()) {
        eprintln!(
            "agent: log the screen fallback of {}: {error}",
            entry.holder
        );
    }
}

/// The last lines of a screen, enough to show what the fallback looked at.
pub(crate) fn tail(lines: &[String], count: usize) -> Vec<String> {
    lines[lines.len().saturating_sub(count)..]
        .iter()
        .map(|line| line.trim_end().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::Caller;
    use crate::driver::driver;
    use std::cell::Cell;

    fn dialog(selected: usize) -> Vec<String> {
        let choices = ["1. Yes", "2. Yes, and always allow", "3. No"];
        let mut lines = vec![
            "Bash command".to_string(),
            "rm -rf build".into(),
            "Do you want to proceed?".into(),
        ];
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
    fn a_key_is_sent_only_while_a_fresh_read_shows_the_same_dialog() {
        let claude = driver("claude").expect("claude");
        let find = |lines: &[String]| claude.permission_dialog(lines);
        let decided = find(&dialog(0)).expect("dialog");

        let sent = Cell::new(0);
        let send = || -> Result<(), ()> {
            sent.set(sent.get() + 1);
            Ok(())
        };
        assert_eq!(
            send_if_unchanged(|| Ok(dialog(0)), find, &decided, send),
            Ok(Recheck::Sent)
        );
        assert_eq!(sent.get(), 1);

        let send = || -> Result<(), ()> {
            sent.set(sent.get() + 1);
            Ok(())
        };
        assert_eq!(
            send_if_unchanged(|| Ok(dialog(1)), find, &decided, send),
            Ok(Recheck::Changed),
            "the highlight moved between the decision and the key"
        );
        let mut other = dialog(0);
        other[3] = "\u{276F} 1. Yes, and remember".into();
        let send = || -> Result<(), ()> {
            sent.set(sent.get() + 1);
            Ok(())
        };
        assert_eq!(
            send_if_unchanged(|| Ok(other), find, &decided, send),
            Ok(Recheck::Changed),
            "another dialog drawn in its place"
        );
        let send = || -> Result<(), ()> {
            sent.set(sent.get() + 1);
            Ok(())
        };
        assert_eq!(
            send_if_unchanged(|| Ok(vec!["> done".to_string()]), find, &decided, send),
            Ok(Recheck::Changed),
            "the dialog closed meanwhile"
        );
        let send = || -> Result<(), ()> {
            sent.set(sent.get() + 1);
            Ok(())
        };
        assert!(matches!(
            send_if_unchanged(
                || Err(std::io::Error::other("holder gone")),
                find,
                &decided,
                send
            ),
            Ok(Recheck::Unreadable(_))
        ));
        assert_eq!(sent.get(), 1, "nothing typed after the first");
    }

    #[test]
    fn only_a_session_no_person_drives_gets_keys() {
        assert!(person_drives(&Lease::person()));
        assert!(
            person_drives(&Lease::default()),
            "no lease at all is a person's"
        );
        assert!(!person_drives(&Lease::for_caller(&Caller::Operator)));
        assert!(!person_drives(&Lease::for_caller(&Caller::Agent {
            holder: "ws-myproject-feat-login-claude-lead".into()
        })));
    }

    #[test]
    fn every_fallback_action_lands_in_the_workspace_log() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let seen = vec!["Do you want to proceed?".to_string()];
        record(
            &state,
            "myproject",
            "feat-login",
            &Entry {
                holder: "ws-myproject-feat-login-claude-reviewer",
                role: "reviewer",
                session_id: "s",
                action: PERMISSION_DENY,
                seen: &seen,
                sent: Some(key_names(b"\x1b")),
                why: "denied by operator",
            },
        );
        record(
            &state,
            "myproject",
            "feat-login",
            &Entry {
                holder: "ws-myproject-feat-login-claude-reviewer",
                role: "reviewer",
                session_id: "s",
                action: PERMISSION_ALLOW,
                seen: &[],
                sent: None,
                why: WHY_ALLOW_NEEDS_HOOK,
            },
        );
        let lines = crate::lease::log_lines(&state, "myproject", "feat-login");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["event"], FALLBACK_LOG_EVENT);
        assert_eq!(lines[0]["action"], PERMISSION_DENY);
        assert_eq!(lines[0]["sent"], "esc");
        assert_eq!(lines[0]["skipped"], false);
        assert_eq!(lines[0]["seen"], json!(["Do you want to proceed?"]));
        assert_eq!(lines[1]["action"], PERMISSION_ALLOW);
        assert_eq!(lines[1]["sent"], Value::Null);
        assert_eq!(lines[1]["skipped"], true);
        assert_eq!(lines[1]["why"], WHY_ALLOW_NEEDS_HOOK);
    }

    #[test]
    fn keys_are_named_for_the_log() {
        assert_eq!(key_names(b"\x1b"), "esc");
        assert_eq!(key_names(b"\r"), "enter");
        assert_eq!(key_names(b"\x1b[B"), "down");
        assert_eq!(key_names(b"n"), "6e");
    }
}
