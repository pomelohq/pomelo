//! User themes: theme family files (`{"name", "author", "themes": [{"name", "appearance", "style"}]}`) whose
//! `style` holds color tokens by name plus `syntax` colors. A theme names only what it changes; every other
//! token comes from the built-in theme of the same appearance.

use std::sync::RwLock;

use crate::theme::{one_dark, one_light, Appearance, Rgba, ThemeColors};

#[derive(Clone, Debug, PartialEq)]
pub struct UserTheme {
    pub name: String,
    pub colors: ThemeColors,
    /// Tree-sitter capture name and its color.
    pub syntax: Vec<(String, Rgba)>,
}

/// A color written as `#rrggbb` or `#rrggbbaa`; anything else is a mistake worth reporting.
fn color(value: &serde_json::Value) -> Option<Rgba> {
    let text = value.as_str()?.trim();
    let hex = text.strip_prefix('#')?;
    (matches!(hex.len(), 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| Rgba::hex(text))
}

const ANSI: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// The field a token names, or `None` for tokens the app does not draw.
fn slot<'a>(colors: &'a mut ThemeColors, token: &str) -> Option<&'a mut Rgba> {
    if let Some(name) = token.strip_prefix("terminal.ansi.") {
        if let Some(name) = name.strip_prefix("bright_") {
            let index = ANSI.iter().position(|ansi| *ansi == name)?;
            return colors.terminal_ansi.get_mut(index + 8);
        }
        if let Some(name) = name.strip_prefix("dim_") {
            let index = ANSI.iter().position(|ansi| *ansi == name)?;
            return colors.terminal_ansi_dim.get_mut(index);
        }
        if name == "background" {
            return Some(&mut colors.terminal_ansi_background);
        }
        let index = ANSI.iter().position(|ansi| *ansi == name)?;
        return colors.terminal_ansi.get_mut(index);
    }
    Some(match token {
        "background" => &mut colors.background,
        "surface.background" => &mut colors.surface_background,
        "elevated_surface.background" => &mut colors.elevated_surface_background,
        "panel.background" => &mut colors.panel_background,
        "border" => &mut colors.border,
        "border.variant" => &mut colors.border_variant,
        "border.focused" => &mut colors.border_focused,
        "border.selected" => &mut colors.border_selected,
        "element.background" => &mut colors.element_background,
        "element.hover" => &mut colors.element_hover,
        "element.active" => &mut colors.element_active,
        "element.selected" => &mut colors.element_selected,
        "ghost_element.hover" => &mut colors.ghost_element_hover,
        "ghost_element.active" => &mut colors.ghost_element_active,
        "text" => &mut colors.text,
        "text.muted" => &mut colors.text_muted,
        "text.placeholder" => &mut colors.text_placeholder,
        "text.accent" => &mut colors.text_accent,
        "text.disabled" => &mut colors.text_disabled,
        "warning" => &mut colors.warning,
        "warning.background" => &mut colors.warning_background,
        "warning.border" => &mut colors.warning_border,
        "success" => &mut colors.success,
        "error" => &mut colors.error,
        "error.background" => &mut colors.error_background,
        "error.border" => &mut colors.error_border,
        "hint" => &mut colors.hint,
        "hint.background" => &mut colors.hint_background,
        "hint.border" => &mut colors.hint_border,
        "info" => &mut colors.info,
        "info.background" => &mut colors.info_background,
        "info.border" => &mut colors.info_border,
        "ignored" => &mut colors.ignored,
        "ignored.background" => &mut colors.ignored_background,
        "ignored.border" => &mut colors.ignored_border,
        "icon" => &mut colors.icon,
        "icon.muted" => &mut colors.icon_muted,
        "icon.accent" => &mut colors.icon_accent,
        "title_bar.background" => &mut colors.title_bar_background,
        "toolbar.background" => &mut colors.toolbar_background,
        "tab_bar.background" => &mut colors.tab_bar_background,
        "tab.active_background" => &mut colors.tab_active_background,
        "tab.inactive_background" => &mut colors.tab_inactive_background,
        "editor.background" => &mut colors.editor_background,
        "editor.foreground" => &mut colors.editor_foreground,
        "editor.active_line.background" => &mut colors.editor_active_line,
        "editor.gutter.background" => &mut colors.editor_gutter_background,
        "editor.line_number" => &mut colors.editor_line_number,
        "editor.active_line_number" => &mut colors.editor_active_line_number,
        "editor.indent_guide" => &mut colors.editor_indent_guide,
        "editor.indent_guide_active" => &mut colors.editor_indent_guide_active,
        "editor.invisible" => &mut colors.editor_invisible,
        "editor.highlighted_line.background" => &mut colors.editor_highlighted_line,
        "editor.document_highlight.bracket_background" => {
            &mut colors.editor_document_highlight_bracket_background
        }
        "search.match_background" => &mut colors.search_match_background,
        "search.active_match_background" => &mut colors.search_active_match_background,
        "version_control.added" => &mut colors.version_control_added,
        "version_control.modified" => &mut colors.version_control_modified,
        "version_control.deleted" => &mut colors.version_control_deleted,
        "link_text.hover" => &mut colors.link_text_hover,
        "terminal.background" => &mut colors.terminal_background,
        "terminal.foreground" => &mut colors.terminal_foreground,
        "terminal.bright_foreground" => &mut colors.terminal_bright_foreground,
        "terminal.dim_foreground" => &mut colors.terminal_dim_foreground,
        "scrollbar.thumb.background" => &mut colors.scrollbar_thumb_background,
        "scrollbar.thumb.border" => &mut colors.scrollbar_thumb_border,
        "scrollbar.track.border" => &mut colors.scrollbar_track_border,
        "scrollbar.thumb.hover_background" => &mut colors.scrollbar_thumb_hover_background,
        "panel.indent_guide" => &mut colors.panel_indent_guide,
        "panel.focused_border" => &mut colors.panel_focused_border,
        _ => return None,
    })
}

/// Apply a `style` object: color tokens, the first player's cursor and selection, and syntax colors.
/// Returns the problems found (bad colors), labelled with `origin`.
pub fn apply_style(
    colors: &mut ThemeColors,
    syntax: &mut Vec<(String, Rgba)>,
    style: &serde_json::Map<String, serde_json::Value>,
    origin: &str,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (token, value) in style {
        match token.as_str() {
            "syntax" => {
                let Some(entries) = value.as_object() else {
                    continue;
                };
                for (capture, entry) in entries {
                    let Some(value) = entry.get("color").filter(|value| !value.is_null()) else {
                        continue;
                    };
                    match color(value) {
                        Some(rgba) => {
                            syntax.retain(|(name, _)| name != capture);
                            syntax.push((capture.clone(), rgba));
                        }
                        None => problems.push(format!(
                            "{origin}: syntax \"{capture}\" is not a #rrggbb color"
                        )),
                    }
                }
            }
            "players" => {
                let first = value.as_array().and_then(|players| players.first());
                if let Some(player) = first {
                    for (field, target) in [("cursor", 0), ("selection", 1)] {
                        if let Some(rgba) = player.get(field).and_then(color) {
                            if target == 0 {
                                colors.player_cursor = rgba;
                            } else {
                                colors.player_selection = rgba;
                            }
                        }
                    }
                }
            }
            _ if value.is_null() => {}
            _ => {
                let Some(target) = slot(colors, token) else {
                    continue;
                };
                match color(value) {
                    Some(rgba) => *target = rgba,
                    None => problems.push(format!("{origin}: \"{token}\" is not a #rrggbb color")),
                }
            }
        }
    }
    problems
}

/// Every theme in a theme family file, and what was wrong with it.
pub fn parse_family(text: &str, origin: &str) -> (Vec<UserTheme>, Vec<String>) {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => return (Vec::new(), vec![format!("{origin}: {error}")]),
    };
    let Some(themes) = value.get("themes").and_then(|themes| themes.as_array()) else {
        return (
            Vec::new(),
            vec![format!("{origin}: expected a \"themes\" list")],
        );
    };
    let mut loaded = Vec::new();
    let mut problems = Vec::new();
    for theme in themes {
        let Some(name) = theme.get("name").and_then(|name| name.as_str()) else {
            problems.push(format!("{origin}: a theme has no \"name\""));
            continue;
        };
        let appearance = match theme.get("appearance").and_then(|a| a.as_str()) {
            Some("light") => Appearance::Light,
            Some("dark") | None => Appearance::Dark,
            Some(other) => {
                problems.push(format!(
                    "{origin}: \"{name}\" has appearance \"{other}\", not light or dark"
                ));
                Appearance::Dark
            }
        };
        let mut colors = match appearance {
            Appearance::Light => one_light(),
            Appearance::Dark => one_dark(),
        };
        let mut syntax = Vec::new();
        if let Some(style) = theme.get("style").and_then(|style| style.as_object()) {
            problems.extend(apply_style(
                &mut colors,
                &mut syntax,
                style,
                &format!("{origin}: {name}"),
            ));
        }
        loaded.push(UserTheme {
            name: name.to_string(),
            colors,
            syntax,
        });
    }
    (loaded, problems)
}

static USER_THEMES: RwLock<Vec<UserTheme>> = RwLock::new(Vec::new());
static PROBLEMS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// What was wrong with the theme files and overrides, for Settings to show.
pub fn theme_problems() -> Vec<String> {
    PROBLEMS
        .read()
        .map(|problems| problems.clone())
        .unwrap_or_default()
}

pub fn set_theme_problems(problems: Vec<String>) {
    if let Ok(mut slot) = PROBLEMS.write() {
        *slot = problems;
    }
}
static SYNTAX: RwLock<Vec<(String, Rgba)>> = RwLock::new(Vec::new());

pub fn set_user_themes(themes: Vec<UserTheme>) {
    if let Ok(mut slot) = USER_THEMES.write() {
        *slot = themes;
    }
}

pub fn user_theme_names() -> Vec<String> {
    USER_THEMES
        .read()
        .map(|themes| themes.iter().map(|theme| theme.name.clone()).collect())
        .unwrap_or_default()
}

pub fn user_theme(name: &str) -> Option<UserTheme> {
    USER_THEMES
        .read()
        .ok()?
        .iter()
        .rev()
        .find(|theme| theme.name == name)
        .cloned()
}

/// Syntax colors of the active theme, laid over the built-in palette of its appearance.
pub fn syntax_colors() -> Vec<(String, Rgba)> {
    SYNTAX
        .read()
        .map(|syntax| syntax.clone())
        .unwrap_or_default()
}

pub fn set_syntax_colors(syntax: Vec<(String, Rgba)>) {
    if let Ok(mut slot) = SYNTAX.write() {
        *slot = syntax;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAMILY: &str = r##"{
        "name": "Mine", "author": "me",
        "themes": [
            {"name": "Mine Dark", "appearance": "dark", "style": {
                "editor.background": "#101010ff",
                "terminal.ansi.bright_red": "#ff0000",
                "players": [{"cursor": "#00ff00ff", "selection": "#00ff0040"}],
                "conflict": "#123456",
                "text": "blue",
                "syntax": {"keyword": {"color": "#aa00aa", "font_style": null}, "comment": {"color": null}}
            }},
            {"name": "Mine Light", "appearance": "light", "style": {}}
        ]
    }"##;

    #[test]
    fn a_family_fills_what_it_names_and_keeps_the_rest() {
        let (themes, problems) = parse_family(FAMILY, "mine.json");
        assert_eq!(themes.len(), 2);
        let dark = &themes[0];
        assert_eq!(dark.colors.editor_background, Rgba::hex("#101010ff"));
        assert_eq!(dark.colors.terminal_ansi[9], Rgba::hex("#ff0000"));
        assert_eq!(dark.colors.player_cursor, Rgba::hex("#00ff00ff"));
        assert_eq!(dark.colors.panel_background, one_dark().panel_background);
        assert_eq!(
            dark.syntax,
            vec![("keyword".to_string(), Rgba::hex("#aa00aa"))]
        );
        assert_eq!(themes[1].colors.appearance, Appearance::Light);
        assert_eq!(themes[1].colors, one_light());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("\"text\""), "{problems:?}");
    }

    #[test]
    fn a_broken_file_says_so() {
        let (themes, problems) = parse_family("{", "bad.json");
        assert!(themes.is_empty());
        assert!(problems[0].starts_with("bad.json"));
        let (_, problems) = parse_family("{}", "empty.json");
        assert!(problems[0].contains("themes"));
    }
}
