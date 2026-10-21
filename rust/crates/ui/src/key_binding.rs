use crate::{div, icon, label, theme, IconKind, Node, Rgba};

/// Split `cmd-shift-k` into its modifiers and key; the key may itself be `-` (`ctrl--`).
fn parse_keystroke(keystroke: &str) -> (Vec<&str>, &str) {
    let mut modifiers = Vec::new();
    let mut rest = keystroke;
    while let Some((head, tail)) = rest.split_once('-') {
        if tail.is_empty() || !matches!(head, "ctrl" | "alt" | "cmd" | "shift") {
            break;
        }
        modifiers.push(head);
        rest = tail;
    }
    (modifiers, rest)
}

fn key_icon(key: &str) -> Option<IconKind> {
    match key {
        "left" => Some(IconKind::KeyArrowLeft),
        "right" => Some(IconKind::KeyArrowRight),
        "up" => Some(IconKind::ArrowUp),
        "down" => Some(IconKind::ArrowDown),
        "backspace" | "delete" => Some(IconKind::Backspace),
        "enter" => Some(IconKind::Return),
        "tab" => Some(IconKind::Tab),
        _ => None,
    }
}

fn key_label(key: &str) -> String {
    match key {
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        "escape" => "Esc".to_string(),
        key => {
            let mut chars = key.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

/// How Linux and Windows spell a keystroke: `Ctrl+Shift+K`.
pub fn keystroke_words(keystroke: &str) -> String {
    let (modifiers, key) = parse_keystroke(keystroke);
    let mut parts: Vec<String> = ["ctrl", "alt", "shift", "cmd"]
        .into_iter()
        .filter(|name| modifiers.contains(name))
        .map(|name| {
            match name {
                "ctrl" => "Ctrl",
                "alt" => "Alt",
                "shift" => "Shift",
                _ => "Super",
            }
            .to_string()
        })
        .collect();
    parts.push(match key {
        "left" => "Left".into(),
        "right" => "Right".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "enter" => "Enter".into(),
        "backspace" | "delete" => "Backspace".into(),
        key => key_label(key),
    });
    parts.join("+")
}

/// A hint written for macOS, such as `cmd-enter`, spelled as the keys this platform types for it.
pub fn keystroke_as_typed(keystroke: &str) -> String {
    if cfg!(target_os = "macos") {
        keystroke.to_string()
    } else {
        keystroke
            .split_whitespace()
            .map(as_typed_off_macos)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Hints written for macOS (`cmd-enter`) as the keys that do the same off it: Command is Control there, and
/// Command with an arrow goes to the line or document edge, which Home and End do.
fn as_typed_off_macos(keystroke: &str) -> String {
    let (modifiers, key) = parse_keystroke(keystroke);
    if !modifiers.contains(&"cmd") {
        return keystroke.to_string();
    }
    let mut rest: Vec<&str> = modifiers
        .iter()
        .copied()
        .filter(|name| !matches!(*name, "cmd" | "ctrl"))
        .collect();
    let key = match key {
        "left" => "home",
        "right" => "end",
        "up" | "down" => {
            rest.insert(0, "ctrl");
            if key == "up" {
                "home"
            } else {
                "end"
            }
        }
        key => {
            rest.insert(0, "ctrl");
            key
        }
    };
    rest.push(key);
    rest.join("-")
}

/// One keystroke as modifier glyphs (control, option, command, shift, in that order) then the key; Linux and
/// Windows spell it out instead.
pub fn render_keystroke(keystroke: &str, size: f32) -> Node {
    render_keystroke_in(keystroke, size, theme().text_muted)
}

/// [`render_keystroke`] in `muted`, for keys drawn on a filled button.
pub fn render_keystroke_in(keystroke: &str, size: f32, muted: Rgba) -> Node {
    if !cfg!(target_os = "macos") {
        let words: Vec<String> = keystroke
            .split_whitespace()
            .map(|stroke| keystroke_words(&as_typed_off_macos(stroke)))
            .collect();
        return label(words.join(", ")).size(size).color(muted).into();
    }
    if keystroke.trim().contains(' ') {
        let mut chord = div().row().items_center().gap(size * 0.45);
        for stroke in keystroke.split_whitespace() {
            chord = chord.child(render_keystroke_in(stroke, size, muted));
        }
        return chord.into();
    }
    let (modifiers, key) = parse_keystroke(keystroke);
    let glyph = |kind: IconKind| -> Node {
        div()
            .row()
            .items_center()
            .justify_center()
            .w_px(size)
            .h_px(size)
            .child(icon(kind).size(size).color(muted))
            .into()
    };
    let mut row = div().row().items_center().gap(1.0);
    for (name, kind) in [
        ("ctrl", IconKind::Control),
        ("alt", IconKind::Option),
        ("cmd", IconKind::Command),
        ("shift", IconKind::Shift),
    ] {
        if modifiers.contains(&name) {
            row = row.child(glyph(kind));
        }
    }
    if let Some(kind) = key_icon(key) {
        return row.child(glyph(kind)).into();
    }
    // The key keeps its own line height and the row centers it on the glyphs; unboxed it would sit below them.
    let key_box = div().row().items_center();
    row.child(key_box.child(label(key_label(key)).size(size).medium().color(muted)))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keystrokes() {
        assert_eq!(parse_keystroke("cmd-shift-k"), (vec!["cmd", "shift"], "k"));
        assert_eq!(parse_keystroke("ctrl--"), (vec!["ctrl"], "-"));
        assert_eq!(
            parse_keystroke("ctrl-shift--"),
            (vec!["ctrl", "shift"], "-")
        );
        assert_eq!(parse_keystroke("pageup"), (vec![], "pageup"));
    }

    #[test]
    fn linux_and_windows_spell_keys_out() {
        assert_eq!(keystroke_words("ctrl-shift-c"), "Ctrl+Shift+C");
        assert_eq!(keystroke_words("ctrl-alt-left"), "Ctrl+Alt+Left");
        assert_eq!(keystroke_words("ctrl-pageup"), "Ctrl+PageUp");
    }

    #[test]
    fn macos_hints_read_as_the_keys_typed_elsewhere() {
        assert_eq!(as_typed_off_macos("cmd-enter"), "ctrl-enter");
        assert_eq!(as_typed_off_macos("cmd-shift-p"), "ctrl-shift-p");
        assert_eq!(as_typed_off_macos("cmd-left"), "home");
        assert_eq!(as_typed_off_macos("shift-cmd-right"), "shift-end");
        assert_eq!(as_typed_off_macos("cmd-up"), "ctrl-home");
        assert_eq!(as_typed_off_macos("cmd-ctrl-left"), "home");
        assert_eq!(as_typed_off_macos("ctrl-shift-n"), "ctrl-shift-n");
        assert_eq!(as_typed_off_macos("alt-left"), "alt-left");
    }
}
