use ui::{div, icon, label, theme, IconKind, Node};

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
        key => {
            let mut chars = key.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

/// One keystroke as modifier glyphs (control, option, command, shift, in that order) then the key.
pub fn render_keystroke(keystroke: &str, size: f32) -> Node {
    let muted = theme().text_muted;
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
    let mut row = div().row().items_center();
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
    let text = key_label(key);
    let key_box = div().row().items_center().h_px(size);
    let key_box = if text.chars().count() == 1 {
        key_box.w_px(size).justify_center()
    } else {
        key_box.px(2.0)
    };
    row.child(key_box.child(label(text).size(size).color(muted)))
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
}
