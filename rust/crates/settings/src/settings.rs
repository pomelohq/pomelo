//! User settings: a typed config (de)serialized as `~/.config/pomelo/settings.json`. Pure logic/state — the
//! settings screen is the separate `settings_ui` crate. Unknown/missing keys fall back to defaults so an old
//! or hand-edited file never fails to load.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub theme_mode: String,
    pub ui_font: String,
    pub ui_font_size: f32,
    pub ui_font_weight: f32,
    pub left_dock_width: f32,
    pub right_dock_width: f32,
    pub left_dock_collapsed: bool,
    pub right_dock_collapsed: bool,
    pub bottom_dock_collapsed: bool,
    pub window_width: f32,
    pub window_height: f32,
    /// Dock layout ("left"/"right"/"bottom"): where the sidebar/agent/terminal and each function button live,
    /// plus which buttons are hidden. Persisted so the user's arrangement survives a restart.
    pub sidebar_side: String,
    pub agent_side: String,
    pub terminal_side: String,
    pub func_sides: Vec<String>,
    pub func_hidden: Vec<bool>,
    pub agent_hidden: bool,
    pub terminal_hidden: bool,
    pub show_diagnostics: bool,
    pub show_cursor_position: bool,
    pub show_language: bool,
    pub show_branch: bool,
    pub show_session_name: bool,
    /// The AI CLI the Agent button opens in a workspace; `claude` also gets pom's MCP server and prompt.
    pub agent_command: String,
    /// The new-workspace ticket picker lists only Jira tickets assigned to you.
    pub jira_only_mine: bool,
    /// The Jira board the ticket picker opened on last (0: none yet).
    pub jira_board: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "One Dark".into(),
            theme_mode: "Dark".into(),
            ui_font: ".PomeloSans".into(),
            ui_font_size: 16.0,
            ui_font_weight: 400.0,
            left_dock_width: 260.0,
            right_dock_width: 300.0,
            left_dock_collapsed: false,
            right_dock_collapsed: true,
            bottom_dock_collapsed: false,
            window_width: 1280.0,
            window_height: 820.0,
            sidebar_side: "left".into(),
            agent_side: "right".into(),
            terminal_side: "bottom".into(),
            func_sides: Vec::new(),
            func_hidden: Vec::new(),
            agent_hidden: false,
            terminal_hidden: false,
            show_diagnostics: true,
            show_cursor_position: true,
            show_language: true,
            show_branch: true,
            show_session_name: true,
            agent_command: "claude".into(),
            jira_only_mine: false,
            jira_board: 0,
        }
    }
}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".config/pomelo/settings.json"))
    }

    /// Load from disk, or defaults if the file is missing or unreadable.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Write to disk (pretty JSON), creating the config dir if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_through_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"ui_font_size": 15.0}"#).unwrap();
        assert_eq!(s.ui_font_size, 15.0);
        assert_eq!(s.theme, "One Dark"); // missing key -> default
    }
}
