use workspace::keymap::{Action, MACOS_DEFAULTS, OTHER_DEFAULTS};

use crate::{code, table_cell};

const INTRO: &str = "\
# Key bindings

Every window action `~/.config/pomelo/keymap.json` can bind, by the name the
file uses, with its default keys. How to write the file is in
[Keyboard shortcuts](/docs/shortcuts#your-own-bindings); the editor, terminal
and pane keys listed there are fixed and cannot be rebound.

Pomelo runs on macOS today; the Windows / Linux column shows the keys those
platforms will use.

| Action | Name | macOS | Windows / Linux |
| --- | --- | --- | --- |
";

pub fn render() -> String {
    let mut page = String::from(INTRO);
    for action in Action::ALL {
        page.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            table_cell(action.label()),
            code(&binding_target(action)),
            keys(MACOS_DEFAULTS, action, ""),
            keys(OTHER_DEFAULTS, action, " pc"),
        ));
    }
    page
}

fn binding_target(action: Action) -> String {
    match action {
        Action::ActivateTab(index) => format!("[\"{}\", {index}]", action.name()),
        _ => action.name().to_string(),
    }
}

fn keys(table: &[(&str, Action)], action: Action, platform: &str) -> String {
    let bound: Vec<String> = table
        .iter()
        .filter(|(_, bound_action)| *bound_action == action)
        .map(|(keys, _)| {
            let attribute = keys
                .replace('&', "&amp;")
                .replace('"', "&quot;")
                .replace('`', "&#96;");
            format!("<Keys{platform} k=\"{attribute}\"/>")
        })
        .collect();
    if bound.is_empty() {
        "-".to_string()
    } else {
        bound.join(" or ")
    }
}
