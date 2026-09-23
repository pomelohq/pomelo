//! Encoding key presses as the bytes a program on the PTY expects (xterm conventions).

use std::borrow::Cow;

use crate::TermMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub cmd: bool,
}

/// A key press: `key` is a lowercase name for special keys (`"enter"`, `"up"`, `"f5"`) or the character typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keystroke {
    pub key: String,
    pub modifiers: Modifiers,
}

impl Keystroke {
    pub fn new(key: impl Into<String>, modifiers: Modifiers) -> Self {
        Self {
            key: key.into(),
            modifiers,
        }
    }

    /// Parse `"ctrl-shift-a"`-style text; the last segment is the key.
    pub fn parse(text: &str) -> Self {
        let mut modifiers = Modifiers::default();
        let mut parts: Vec<&str> = text.split('-').collect();
        let key = parts.pop().unwrap_or_default().to_string();
        for part in parts {
            match part {
                "shift" => modifiers.shift = true,
                "alt" => modifiers.alt = true,
                "ctrl" => modifiers.ctrl = true,
                "cmd" => modifiers.cmd = true,
                _ => {}
            }
        }
        Self { key, modifiers }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    None,
    Alt,
    Ctrl,
    Shift,
    CtrlShift,
    Other,
}

impl Held {
    fn of(modifiers: Modifiers) -> Self {
        match (
            modifiers.alt,
            modifiers.ctrl,
            modifiers.shift,
            modifiers.cmd,
        ) {
            (false, false, false, false) => Held::None,
            (true, false, false, false) => Held::Alt,
            (false, true, false, false) => Held::Ctrl,
            (false, false, true, false) => Held::Shift,
            (false, true, true, false) => Held::CtrlShift,
            _ => Held::Other,
        }
    }
}

fn cursor_key(letter: char, mode: TermMode) -> &'static str {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    match (letter, app_cursor) {
        ('A', true) => "\x1bOA",
        ('A', false) => "\x1b[A",
        ('B', true) => "\x1bOB",
        ('B', false) => "\x1b[B",
        ('C', true) => "\x1bOC",
        ('C', false) => "\x1b[C",
        ('D', true) => "\x1bOD",
        ('D', false) => "\x1b[D",
        ('H', true) => "\x1bOH",
        ('H', false) => "\x1b[H",
        ('F', true) => "\x1bOF",
        _ => "\x1b[F",
    }
}

/// The final byte (and prefix) for keys whose sequence takes a modifier parameter: `CSI 1;m X` or `CSI n;m ~`.
fn parameterized(key: &str) -> Option<(&'static str, char)> {
    Some(match key {
        "up" => ("1", 'A'),
        "down" => ("1", 'B'),
        "right" => ("1", 'C'),
        "left" => ("1", 'D'),
        "home" => ("1", 'H'),
        "end" => ("1", 'F'),
        "f1" => ("1", 'P'),
        "f2" => ("1", 'Q'),
        "f3" => ("1", 'R'),
        "f4" => ("1", 'S'),
        "insert" => ("2", '~'),
        "pageup" => ("5", '~'),
        "pagedown" => ("6", '~'),
        "f5" => ("15", '~'),
        "f6" => ("17", '~'),
        "f7" => ("18", '~'),
        "f8" => ("19", '~'),
        "f9" => ("20", '~'),
        "f10" => ("21", '~'),
        "f11" => ("23", '~'),
        "f12" => ("24", '~'),
        "f13" => ("25", '~'),
        "f14" => ("26", '~'),
        "f15" => ("28", '~'),
        "f16" => ("29", '~'),
        "f17" => ("31", '~'),
        "f18" => ("32", '~'),
        "f19" => ("33", '~'),
        "f20" => ("34", '~'),
        _ => return None,
    })
}

fn unmodified(key: &str, mode: TermMode) -> Option<&'static str> {
    Some(match key {
        "tab" => "\x09",
        "escape" => "\x1b",
        "enter" => "\x0d",
        "backspace" | "back" => "\x7f",
        "up" => cursor_key('A', mode),
        "down" => cursor_key('B', mode),
        "right" => cursor_key('C', mode),
        "left" => cursor_key('D', mode),
        "home" => cursor_key('H', mode),
        "end" => cursor_key('F', mode),
        "insert" => "\x1b[2~",
        "delete" => "\x1b[3~",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
        "f13" => "\x1b[25~",
        "f14" => "\x1b[26~",
        "f15" => "\x1b[28~",
        "f16" => "\x1b[29~",
        "f17" => "\x1b[31~",
        "f18" => "\x1b[32~",
        "f19" => "\x1b[33~",
        "f20" => "\x1b[34~",
        _ => return None,
    })
}

/// Caret notation: ctrl with a letter or one of `@[\]^_?` sends the matching C0 control byte.
fn control_byte(key: &str, held: Held) -> Option<u8> {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    match held {
        Held::Ctrl if c.is_ascii_lowercase() => Some(c as u8 - b'a' + 1),
        Held::CtrlShift if c.is_ascii_uppercase() => Some(c as u8 - b'A' + 1),
        Held::Ctrl => match c {
            '@' => Some(0x00),
            '[' => Some(0x1b),
            '\\' => Some(0x1c),
            ']' => Some(0x1d),
            '^' => Some(0x1e),
            '_' => Some(0x1f),
            '?' => Some(0x7f),
            _ => None,
        },
        _ => None,
    }
}

/// xterm's modifier parameter: 1 + (shift 1 | alt 2 | ctrl 4).
fn modifier_parameter(modifiers: Modifiers) -> u8 {
    1 + modifiers.shift as u8 + ((modifiers.alt as u8) << 1) + ((modifiers.ctrl as u8) << 2)
}

/// The bytes for `keystroke`, or `None` when it should go through as typed text (or isn't for the terminal).
/// On macOS Option composes characters unless `option_as_meta` makes it an ESC prefix.
pub fn escape_sequence(
    keystroke: &Keystroke,
    mode: TermMode,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let key = keystroke.key.as_str();
    let held = Held::of(keystroke.modifiers);
    let special = match (key, held) {
        (_, Held::None) => unmodified(key, mode),
        ("enter", Held::Shift) => Some("\x0a"),
        ("enter", Held::Alt) => Some("\x1b\x0d"),
        ("tab", Held::Shift) => Some("\x1b[Z"),
        ("backspace", Held::Ctrl) => Some("\x08"),
        ("backspace", Held::Alt) => Some("\x1b\x7f"),
        ("backspace", Held::Shift) => Some("\x7f"),
        ("space", Held::Ctrl) => Some("\x00"),
        _ => None,
    };
    if let Some(sequence) = special {
        return Some(Cow::Borrowed(sequence));
    }
    if let Some(byte) = control_byte(key, held) {
        return Some(Cow::Owned((byte as char).to_string()));
    }
    if held != Held::None {
        if let Some((number, last)) = parameterized(key) {
            let parameter = modifier_parameter(keystroke.modifiers);
            return Some(Cow::Owned(format!("\x1b[{number};{parameter}{last}")));
        }
    }
    let meta = !cfg!(target_os = "macos") || option_as_meta;
    let mut chars = key.chars();
    if let (true, true, Some(c), None) = (meta, keystroke.modifiers.alt, chars.next(), chars.next())
    {
        if c.is_ascii() {
            let modifiers = keystroke.modifiers;
            if held == Held::Alt {
                return Some(Cow::Owned(format!("\x1b{c}")));
            } else if modifiers.shift {
                return Some(Cow::Owned(format!("\x1b{}", c.to_ascii_uppercase())));
            } else if modifiers.ctrl && c.is_ascii_lowercase() {
                return Some(Cow::Owned(format!("\x1b{}", (c as u8 - b'a' + 1) as char)));
            }
        }
    }
    None
}

/// What a key does in a focused terminal before it is encoded for the program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalAction {
    Copy,
    Paste,
    Clear,
    SelectAll,
    ScrollLineUp,
    ScrollLineDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
    SendKeystroke(Keystroke),
    SendText(&'static str),
}

/// The terminal's own bindings: clipboard and scrolling, plus editor-style line motions translated into the
/// shell's emacs keys.
pub fn binding(keystroke: &Keystroke) -> Option<TerminalAction> {
    let m = keystroke.modifiers;
    let only = |shift: bool, alt: bool, ctrl: bool, cmd: bool| {
        m.shift == shift && m.alt == alt && m.ctrl == ctrl && m.cmd == cmd
    };
    let cmd = only(false, false, false, true);
    let alt = only(false, true, false, false);
    let shift = only(true, false, false, false);
    let ctrl = only(false, false, true, false);
    let send = |text: &str| Some(TerminalAction::SendKeystroke(Keystroke::parse(text)));
    match keystroke.key.as_str() {
        "c" if cmd => Some(TerminalAction::Copy),
        "v" if cmd => Some(TerminalAction::Paste),
        "k" if cmd => Some(TerminalAction::Clear),
        "a" if cmd => Some(TerminalAction::SelectAll),
        "backspace" if cmd => send("ctrl-u"),
        "delete" if cmd => send("ctrl-k"),
        "right" if cmd => send("ctrl-e"),
        "left" if cmd => send("ctrl-a"),
        "backspace" if ctrl => send("ctrl-w"),
        "delete" if alt => Some(TerminalAction::SendText("\x1bd")),
        "delete" if ctrl => Some(TerminalAction::SendText("\x1b[3;5~")),
        "left" | "b" if alt => Some(TerminalAction::SendText("\x1bb")),
        "right" | "f" if alt => Some(TerminalAction::SendText("\x1bf")),
        "pageup" if shift => Some(TerminalAction::ScrollPageUp),
        "up" if cmd => Some(TerminalAction::ScrollPageUp),
        "pagedown" if shift => Some(TerminalAction::ScrollPageDown),
        "down" if cmd => Some(TerminalAction::ScrollPageDown),
        "up" if shift => Some(TerminalAction::ScrollLineUp),
        "down" if shift => Some(TerminalAction::ScrollLineDown),
        "home" if shift || cmd => Some(TerminalAction::ScrollToTop),
        "end" if shift || cmd => Some(TerminalAction::ScrollToBottom),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc(text: &str, mode: TermMode) -> Option<String> {
        escape_sequence(&Keystroke::parse(text), mode, false).map(Cow::into_owned)
    }

    #[test]
    fn cursor_keys_follow_application_mode() {
        let none = TermMode::empty();
        let app = TermMode::APP_CURSOR;
        for (key, normal, application) in [
            ("up", "\x1b[A", "\x1bOA"),
            ("down", "\x1b[B", "\x1bOB"),
            ("right", "\x1b[C", "\x1bOC"),
            ("left", "\x1b[D", "\x1bOD"),
            ("home", "\x1b[H", "\x1bOH"),
            ("end", "\x1b[F", "\x1bOF"),
        ] {
            assert_eq!(esc(key, none).as_deref(), Some(normal));
            assert_eq!(esc(key, app).as_deref(), Some(application));
        }
        assert_eq!(esc("shift-up", none).as_deref(), Some("\x1b[1;2A"));
        assert_eq!(esc("shift-end", app).as_deref(), Some("\x1b[1;2F"));
    }

    #[test]
    fn control_codes() {
        let none = TermMode::empty();
        assert_eq!(esc("ctrl-a", none).as_deref(), Some("\x01"));
        assert_eq!(esc("ctrl-shift-A", none).as_deref(), Some("\x01"));
        assert_eq!(esc("ctrl-z", none).as_deref(), Some("\x1a"));
        assert_eq!(esc("ctrl-@", none).as_deref(), Some("\x00"));
        assert_eq!(esc("ctrl-?", none).as_deref(), Some("\x7f"));
        assert_eq!(esc("ctrl-space", none).as_deref(), Some("\x00"));
        assert_eq!(esc("ctrl-backspace", none).as_deref(), Some("\x08"));
        assert_eq!(esc("shift-tab", none).as_deref(), Some("\x1b[Z"));
    }

    #[test]
    fn modifier_parameters() {
        let none = TermMode::empty();
        assert_eq!(esc("alt-left", none).as_deref(), Some("\x1b[1;3D"));
        assert_eq!(esc("ctrl-f5", none).as_deref(), Some("\x1b[15;5~"));
        assert_eq!(
            esc("ctrl-alt-shift-pageup", none).as_deref(),
            Some("\x1b[5;8~")
        );
        assert_eq!(esc("cmd-a", none), None);
        assert_eq!(esc("a", none), None);
    }

    #[test]
    fn option_as_meta_prefixes_escape() {
        let none = TermMode::empty();
        let alt_b = Keystroke::parse("alt-b");
        assert_eq!(
            escape_sequence(&alt_b, none, true).as_deref(),
            Some("\x1bb")
        );
        let alt_ctrl_c = Keystroke::parse("ctrl-alt-c");
        assert_eq!(
            escape_sequence(&alt_ctrl_c, none, true).as_deref(),
            Some("\x1b\x03")
        );
        if cfg!(target_os = "macos") {
            assert_eq!(escape_sequence(&alt_b, none, false), None);
        }
    }

    #[test]
    fn terminal_bindings() {
        assert_eq!(
            binding(&Keystroke::parse("cmd-k")),
            Some(TerminalAction::Clear)
        );
        assert_eq!(
            binding(&Keystroke::parse("cmd-left")),
            Some(TerminalAction::SendKeystroke(Keystroke::parse("ctrl-a")))
        );
        assert_eq!(
            binding(&Keystroke::parse("alt-right")),
            Some(TerminalAction::SendText("\x1bf"))
        );
        assert_eq!(
            binding(&Keystroke::parse("shift-up")),
            Some(TerminalAction::ScrollLineUp)
        );
        assert_eq!(binding(&Keystroke::parse("up")), None);
        assert_eq!(binding(&Keystroke::parse("cmd-shift-c")), None);
    }
}
