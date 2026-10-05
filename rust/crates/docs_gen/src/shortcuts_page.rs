use workspace::keymap::{
    Action, MACOS_CONTEXT_DEFAULTS, MACOS_DEFAULTS, OTHER_CONTEXT_DEFAULTS, OTHER_DEFAULTS,
    WORKSPACE,
};

use crate::{code, prose};

const INTRO: &str = "\
# Key bindings

Every window action `~/.config/pomelo/keymap.json` can bind, by the name the
file uses, with its default keys. How to write the file is in
[Keyboard shortcuts](/docs/shortcuts#your-own-bindings); the editor, terminal
and pane keys listed there are fixed and cannot be rebound.

Pomelo runs on macOS today; the Windows / Linux column shows the keys those
platforms will use.

The context says where a binding works: `Workspace` everywhere in the window,
`ProjectPanel` only while the file tree has the keyboard, `TabSwitcher` only
while the tab switcher is open. A keymap file
section binds a context with `\"context\": \"ProjectPanel\"`.

| Action | Name | Context | macOS | Windows / Linux |
| --- | --- | --- | --- | --- |
";

pub fn render() -> String {
    let mut page = String::from(INTRO);
    for action in Action::ALL {
        let context = MACOS_CONTEXT_DEFAULTS
            .iter()
            .find(|(_, _, bound)| *bound == action)
            .map_or(WORKSPACE, |(context, _, _)| *context);
        page.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            prose(action.label()),
            code(&binding_target(action)),
            code(context),
            keys(MACOS_DEFAULTS, MACOS_CONTEXT_DEFAULTS, action, ""),
            keys(OTHER_DEFAULTS, OTHER_CONTEXT_DEFAULTS, action, " pc"),
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

fn keys(
    table: &[(&str, Action)],
    contextual: &[(&str, &str, Action)],
    action: Action,
    platform: &str,
) -> String {
    let bound: Vec<String> = table
        .iter()
        .map(|(keys, bound)| (*keys, *bound))
        .chain(contextual.iter().map(|(_, keys, bound)| (*keys, *bound)))
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
