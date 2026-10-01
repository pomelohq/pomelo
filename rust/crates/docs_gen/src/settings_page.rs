use settings::Settings;

use crate::{code, table_cell};

/// Every key `settings.json` holds, by the Settings page that edits it, with what it does. A test keeps this in
/// step with the `Settings` struct, so a new key cannot ship undocumented.
const KEYS: &[(&str, &str, &str)] = &[
    ("theme", "Appearance", "The theme shown when `theme_selection` is `\"static\"`."),
    ("theme_selection", "Appearance", "`\"static\"` always shows `theme`; `\"dynamic\"` picks `theme_light` or `theme_dark` by `theme_mode`."),
    ("theme_mode", "Appearance", "With dynamic selection: `\"light\"`, `\"dark\"` or `\"system\"` (follow the macOS appearance)."),
    ("theme_light", "Appearance", "The theme used when light mode is active."),
    ("theme_dark", "Appearance", "The theme used when dark mode is active."),
    ("theme_overrides", "Appearance", "Per theme name, color tokens laid over that theme: `{\"One Dark\": {\"editor.background\": \"#1e2127\"}}`."),
    ("ui_font", "Appearance", "Font family used for interface text."),
    ("ui_font_size", "Appearance", "Font size for UI elements."),
    ("ui_font_weight", "Appearance", "Font weight for UI elements (100-900)."),
    ("ui_font_features", "Appearance", "OpenType features for UI text, by tag: `{\"calt\": false, \"ss01\": true}`."),
    ("ui_font_fallbacks", "Appearance", "Families tried, in order, for characters the UI font lacks."),
    ("multi_cursor_modifier", "Appearance", "The key held to add a cursor with a click: `\"alt\"`, or `\"cmd_or_ctrl\"` (Alt-click then goes to the definition)."),
    ("cursor_blink", "Appearance", "Whether the cursor blinks in the editor."),
    ("cursor_animation.enabled", "Appearance", "The cursor glides to where it moves instead of jumping."),
    ("cursor_shape", "Appearance", "`\"bar\"`, `\"block\"`, `\"underline\"` or `\"hollow\"`."),
    ("hide_mouse", "Appearance", "When the pointer hides until the mouse moves: `\"never\"`, `\"on_typing\"` or `\"on_typing_and_action\"`."),
    ("reduce_motion", "Appearance", "`\"on\"` holds loading shimmers, pulsing placeholders and the cursor's glide still."),
    ("func_hidden", "Window & Layout", "Which status bar buttons are hidden, in the order Files, Services, Git, Database."),
    ("terminal_hidden", "Window & Layout", "The terminal button is hidden from the status bar."),
    ("agent_hidden", "Window & Layout", "The agent button is hidden from the status bar."),
    ("show_diagnostics", "Window & Layout", "Show the error/warning count in the status bar."),
    ("show_cursor_position", "Window & Layout", "Show the line and column of the cursor in the status bar."),
    ("show_language", "Window & Layout", "Show the active language of the editor in the status bar."),
    ("global_lsp_settings.button", "Window & Layout", "Show the language server button in the status bar."),
    ("show_branch", "Window & Layout", "Show the active workspace's branch in the title bar."),
    ("show_session_name", "Window & Layout", "Show the current session name in the title bar."),
    ("restore_on_startup", "General", "`\"last_session\"` reopens each workspace's tabs from last time; `\"none\"` starts without them."),
    ("auto_update", "General", "The installed app checks for a newer release. Nothing installs until you restart."),
    ("buffer_font_family", "Editor", "Font family used for the code editor."),
    ("buffer_font_size", "Editor", "Text size of the code editor."),
    ("buffer_font_weight", "Editor", "Font weight for the code editor (100-900)."),
    ("buffer_line_height", "Editor", "Line height for the code editor: `\"comfortable\"` (1.618), `\"standard\"` (1.3), a number or `{\"custom\": n}`."),
    ("buffer_font_features", "Editor", "OpenType features for the code editor, by tag."),
    ("buffer_font_fallbacks", "Editor", "Families tried, in order, for characters the editor font lacks."),
    ("soft_wrap", "Editor", "Newly opened files wrap long lines at the editor's width."),
    ("split_diff", "Editor", "Diffs open side by side when the pane is wide enough, else in one column."),
    ("external_editor", "Editor", "The app \"Open in External Editor\" uses; empty picks the first one installed."),
    ("enable_language_server", "Languages & Tools", "Language servers start for the languages they serve; off, none does."),
    ("language_servers", "Languages & Tools", "Which servers a language uses, in order: a name turns one on, `!name` turns it off, `...` stands for the rest of its defaults."),
    ("completions.lsp", "Languages & Tools", "Language servers are asked for completions."),
    ("completions.lsp_fetch_timeout_ms", "Languages & Tools", "How long to wait for a server's completions, in milliseconds; 0 waits for them."),
    ("go_to_definition_scroll_strategy", "Languages & Tools", "Where a definition lands in view: `\"center\"`, `\"minimum\"`, `\"top\"` or `\"preserve\"`."),
    ("diagnostics_max_severity", "Languages & Tools", "The least severe diagnostic shown: `\"off\"`, `\"error\"`, `\"warning\"`, `\"info\"`, `\"hint\"` or `\"all\"`."),
    ("diagnostics.inline.enabled", "Languages & Tools", "New editors show each line's diagnostic at its end."),
    ("diagnostics.inline.padding", "Languages & Tools", "Columns between a line's end and its diagnostic."),
    ("diagnostics.inline.min_column", "Languages & Tools", "The column inline diagnostics line up at, when the line is shorter."),
    ("file_types", "Languages & Tools", "Language names to the file names, extensions or `*` globs that are that language, over detection: `{\"JSON\": [\"*.jsonc\"]}`."),
    ("languages", "Languages & Tools", "Per language name, settings over the shared ones: `enable_language_server`, `language_servers` and `completions` (`lsp`, `lsp_fetch_timeout_ms`). Unset keys keep the shared value."),
    ("terminal_font_family", "Terminal", "Font family used for terminals."),
    ("terminal_font_size", "Terminal", "Text size of terminals (the agent's tabs have `agent_font_size`)."),
    ("terminal_font_weight", "Terminal", "Font weight for terminals (100-900)."),
    ("terminal_line_height", "Terminal", "Line height for terminals, in the same forms as `buffer_line_height`."),
    ("terminal_font_features", "Terminal", "OpenType features for terminals, by tag."),
    ("terminal_font_fallbacks", "Terminal", "Families tried, in order, for characters the terminal font lacks."),
    ("terminal_shell", "Terminal", "The shell new terminals run, with its arguments; empty runs your login shell."),
    ("terminal_scrollback", "Terminal", "Lines of history each new terminal keeps."),
    ("agent_command", "Agent", "The AI CLI the Agent button opens in a workspace; `claude` also gets pom's MCP server and prompt."),
    ("agent_tab_close", "Agent", "Closing an agent's tab: `\"hide\"` leaves the agent running, `\"stop\"` ends it."),
    ("agent_font_size", "Agent", "Text size of the agent's tabs, apart from terminals."),
    ("notify_claude", "Notifications", "Banners and sounds when a workspace's Claude starts, finishes, needs input or compacts. Needs macOS notification permission."),
    ("notify_when_focused", "Notifications", "Also alert for the workspace on screen in the focused window."),
    ("sound_working", "Notifications", "The macOS sound played when an agent starts working; empty plays nothing."),
    ("sound_finished", "Notifications", "The macOS sound played when an agent finishes; empty plays nothing."),
    ("sound_needs_input", "Notifications", "The macOS sound played when an agent needs your input; empty plays nothing."),
    ("sound_compacting", "Notifications", "The macOS sound played when an agent compacts its context; empty plays nothing."),
    ("dev_proxy_enabled", "Dev Services", "Run the dev proxy while a project is open."),
    ("dev_proxy_port", "Dev Services", "The dev proxy's port: a service opens at `<service>.<repo>.<ticket or branch>.localhost:<port>`. Restart running services after changing it."),
    ("webhook_enabled", "Dev Services", "Run the webhook relay while a project is open."),
    ("webhook_port", "Dev Services", "The webhook relay's port: point an external webhook at `localhost:<port>/<repo>/<service>`."),
    ("modules_store_enabled", "Dev Services", "A new workspace whose lockfile matches a stored one takes its node_modules instead of installing."),
    ("modules_fallback", "Dev Services", "Where a copy-on-write clone is impossible: `\"hardlink\"`, `\"copy\"` or `\"install\"` (skip the store)."),
    ("modules_size_limit_gb", "Dev Services", "GB the node_modules store keeps at most; the least recently used copies go first."),
    ("modules_unused_days", "Dev Services", "Days a stored copy is kept without a workspace taking it (0 keeps them until the size limit)."),
    ("jira_only_mine", "Integrations", "The new-workspace ticket picker lists only Jira tickets assigned to you."),
    ("group_workspaces", "Integrations", "The WORKSPACES list is grouped under its tickets' statuses."),
    ("onboard_with_ai", "Kept by the app", "New projects are finished by the coding agent; off, you review the drafted pom.yml yourself."),
    ("jira_board", "Kept by the app", "The Jira board the ticket picker opened on last (0: none yet)."),
    ("sidebar_side", "Kept by the app", "The dock the sidebar lives in: `\"left\"`, `\"right\"` or `\"bottom\"`."),
    ("agent_side", "Kept by the app", "The dock the agent lives in."),
    ("terminal_side", "Kept by the app", "The dock the terminal lives in."),
    ("func_sides", "Kept by the app", "The dock each status bar button lives in, in the order of `func_hidden`."),
    ("left_dock_width", "Kept by the app", "Width of the left dock, in points."),
    ("right_dock_width", "Kept by the app", "Width of the right dock, in points."),
    ("left_dock_collapsed", "Kept by the app", "The left dock is closed."),
    ("right_dock_collapsed", "Kept by the app", "The right dock is closed."),
    ("bottom_dock_collapsed", "Kept by the app", "The bottom dock is closed."),
    ("panels_collapsed", "Kept by the app", "The left panel column (Files, Services, ...) is closed."),
];

const INTRO: &str = "\
# Settings

Every key in `~/.config/pomelo/settings.json`, with its default. Most are
edited in the Settings window (<Keys k=\"cmd-,\"/>), on the page each section
below is named after; **Edit in settings.json** at the top of the window
opens the file itself. A missing key keeps its default, and keys this version
does not know are written back untouched, so a file from a newer version
survives.

Nested keys are written with dots here: `completions.lsp` is
`{\"completions\": {\"lsp\": true}}` in the file.

The keys under [Kept by the app](#kept-by-the-app) record how you left the
window; they change as you use it and rarely need editing by hand.
";

pub fn render() -> Result<String, String> {
    let defaults =
        flatten(&serde_json::to_value(Settings::default()).map_err(|error| error.to_string())?);
    let mut page = String::from(INTRO);
    let mut section = "";
    for (key, key_section, description) in KEYS {
        if *key_section != section {
            section = key_section;
            page.push_str(&format!(
                "\n## {section}\n\n| Key | Default | Description |\n| --- | --- | --- |\n"
            ));
        }
        let default = defaults
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.to_string())
            .ok_or_else(|| format!("settings key `{key}` has no default"))?;
        page.push_str(&format!(
            "| {} | {} | {} |\n",
            code(key),
            code(&default),
            table_cell(description)
        ));
    }
    Ok(page)
}

/// Objects with keys of their own become dotted keys; empty ones (maps the user fills) stay whole.
fn flatten(value: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    let mut keys = Vec::new();
    flatten_into("", value, &mut keys);
    keys
}

fn flatten_into(
    prefix: &str,
    value: &serde_json::Value,
    keys: &mut Vec<(String, serde_json::Value)>,
) {
    match value {
        serde_json::Value::Object(fields) if !fields.is_empty() => {
            for (name, field) in fields {
                let key = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                flatten_into(&key, field, keys);
            }
        }
        _ => keys.push((prefix.to_string(), value.clone())),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_settings_key_is_documented_once() {
        let defaults = flatten(&serde_json::to_value(Settings::default()).unwrap_or_default());
        let actual: BTreeSet<&str> = defaults.iter().map(|(key, _)| key.as_str()).collect();
        let documented: BTreeSet<&str> = KEYS.iter().map(|(key, _, _)| *key).collect();
        assert_eq!(documented.len(), KEYS.len(), "a key is documented twice");
        assert_eq!(actual, documented);
    }

    #[test]
    fn sections_are_contiguous() {
        let mut seen = BTreeSet::new();
        let mut previous = "";
        for (_, section, _) in KEYS {
            if *section != previous {
                assert!(seen.insert(*section), "section `{section}` is split");
                previous = section;
            }
        }
    }
}
