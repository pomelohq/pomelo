//! User settings: a typed config (de)serialized as `~/.config/pomelo/settings.json`. Pure logic/state — the
//! settings screen is the separate `settings_ui` crate. Unknown/missing keys fall back to defaults so an old
//! or hand-edited file never fails to load.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    /// "static" uses `theme`; "dynamic" picks `theme_light` or `theme_dark` by `theme_mode`.
    pub theme_selection: String,
    /// In dynamic selection: "light", "dark" or "system" (follow the OS appearance).
    pub theme_mode: String,
    pub theme_light: String,
    pub theme_dark: String,
    /// Per theme name, color tokens laid over that theme: `{"One Dark": {"editor.background": "#1e2127"}}`.
    pub theme_overrides: serde_json::Value,
    pub ui_font: String,
    pub ui_font_size: f32,
    pub ui_font_weight: f32,
    /// OpenType features by tag, `{"calt": false, "ss01": true}`.
    pub ui_font_features: serde_json::Value,
    /// Families tried, in order, for characters the UI font lacks.
    pub ui_font_fallbacks: Vec<String>,
    pub left_dock_width: f32,
    pub right_dock_width: f32,
    pub left_dock_collapsed: bool,
    pub right_dock_collapsed: bool,
    pub bottom_dock_collapsed: bool,
    /// The left panel column (Files, Services, ...) is closed.
    pub panels_collapsed: bool,
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
    /// The WORKSPACES list is grouped under its tickets' statuses.
    pub group_workspaces: bool,
    /// New projects are finished by the coding agent (else the user reviews the drafted pom.yml).
    pub onboard_with_ai: bool,
    pub dev_proxy_enabled: bool,
    pub webhook_enabled: bool,
    /// Also read by `pom_env::dev_ports` (service URLs, the CLI); keep the key names in step.
    pub dev_proxy_port: u16,
    pub webhook_port: u16,
    /// Also read by `module_store::Options` (workspace creation, the CLI); keep the key names in step.
    pub modules_store_enabled: bool,
    /// Where a copy-on-write clone is impossible: "hardlink", "copy" or "install".
    pub modules_fallback: String,
    pub modules_size_limit_gb: u64,
    pub modules_unused_days: u64,
    /// When the pointer hides until the mouse moves: "never", "on_typing" or "on_typing_and_action".
    pub hide_mouse: String,
    pub notify_claude: bool,
    /// Also alert for the workspace on screen in the focused window.
    pub notify_when_focused: bool,
    /// macOS system sound per agent event; empty plays nothing.
    pub sound_working: String,
    pub sound_finished: String,
    pub sound_needs_input: String,
    pub sound_compacting: String,
    /// The installed app checks for a newer release on launch.
    pub auto_update: bool,
    pub buffer_font_size: f32,
    pub buffer_font_family: String,
    pub buffer_font_weight: f32,
    /// "comfortable", "standard", `{"custom": 2.0}` or a number.
    pub buffer_line_height: serde_json::Value,
    pub buffer_font_features: serde_json::Value,
    pub buffer_font_fallbacks: Vec<String>,
    /// New files wrap long lines at the editor's width.
    pub soft_wrap: bool,
    /// Diffs open side by side when wide enough (else unified).
    pub split_diff: bool,
    /// The app "Open in External Editor" uses; empty picks the first one installed.
    pub external_editor: String,
    pub terminal_font_size: f32,
    pub terminal_font_family: String,
    pub terminal_font_weight: f32,
    pub terminal_line_height: serde_json::Value,
    pub terminal_font_features: serde_json::Value,
    pub terminal_font_fallbacks: Vec<String>,
    /// Text size of the coding agent's tabs, apart from shells.
    pub agent_font_size: f32,
    /// The shell new terminals run (with its arguments); empty runs the login shell.
    pub terminal_shell: String,
    pub terminal_scrollback: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "One Dark".into(),
            theme_selection: "static".into(),
            theme_mode: "system".into(),
            theme_light: "One Light".into(),
            theme_dark: "One Dark".into(),
            theme_overrides: serde_json::Value::Object(serde_json::Map::new()),
            ui_font: ".PomeloSans".into(),
            ui_font_size: 16.0,
            ui_font_weight: 400.0,
            ui_font_features: serde_json::Value::Object(serde_json::Map::new()),
            ui_font_fallbacks: Vec::new(),
            left_dock_width: 260.0,
            right_dock_width: 300.0,
            left_dock_collapsed: false,
            panels_collapsed: false,
            right_dock_collapsed: true,
            bottom_dock_collapsed: false,
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
            group_workspaces: false,
            onboard_with_ai: true,
            dev_proxy_enabled: true,
            webhook_enabled: true,
            dev_proxy_port: 8767,
            webhook_port: 8766,
            modules_store_enabled: true,
            modules_fallback: "hardlink".into(),
            modules_size_limit_gb: 20,
            modules_unused_days: 14,
            hide_mouse: "on_typing_and_action".into(),
            notify_claude: true,
            notify_when_focused: false,
            sound_working: String::new(),
            sound_finished: "Glass".into(),
            sound_needs_input: "Ping".into(),
            sound_compacting: String::new(),
            auto_update: true,
            buffer_font_size: 15.0,
            buffer_font_family: ".PomeloMono".into(),
            buffer_font_weight: 400.0,
            buffer_line_height: serde_json::Value::String("comfortable".into()),
            buffer_font_features: serde_json::Value::Object(serde_json::Map::new()),
            buffer_font_fallbacks: Vec::new(),
            soft_wrap: false,
            split_diff: true,
            external_editor: String::new(),
            terminal_font_size: 15.0,
            terminal_font_family: ".PomeloMono".into(),
            terminal_font_weight: 400.0,
            terminal_line_height: serde_json::Value::String("standard".into()),
            terminal_font_features: serde_json::Value::Object(serde_json::Map::new()),
            terminal_font_fallbacks: Vec::new(),
            agent_font_size: 12.0,
            terminal_shell: String::new(),
            terminal_scrollback: 10_000,
        }
    }
}

pub const AGENT_EVENTS: [(&str, &str); 4] = [
    ("working", "Started Working"),
    ("finished", "Finished"),
    ("needs_input", "Needs Your Input"),
    ("compacting", "Compacting"),
];

pub const SYSTEM_SOUNDS: [&str; 14] = [
    "Basso",
    "Blow",
    "Bottle",
    "Frog",
    "Funk",
    "Glass",
    "Hero",
    "Morse",
    "Ping",
    "Pop",
    "Purr",
    "Sosumi",
    "Submarine",
    "Tink",
];

/// A line height setting as a multiple of the font size: "comfortable" (1.618), "standard" (1.3), a number or
/// `{"custom": n}`; anything else is `fallback`.
pub fn line_height_ratio(value: &serde_json::Value, fallback: f32) -> f32 {
    let custom = value
        .get("custom")
        .and_then(serde_json::Value::as_f64)
        .or_else(|| value.as_f64());
    match (value.as_str(), custom) {
        (Some("comfortable"), _) => 1.618,
        (Some("standard"), _) => 1.3,
        (_, Some(ratio)) => (ratio as f32).clamp(1.0, 3.0),
        _ => fallback,
    }
}

impl Settings {
    /// The theme to show, given whether the OS is in dark mode.
    pub fn active_theme(&self, system_dark: bool) -> &str {
        if self.theme_selection != "dynamic" {
            return &self.theme;
        }
        let dark = match self.theme_mode.as_str() {
            "light" => false,
            "dark" => true,
            _ => system_dark,
        };
        if dark {
            &self.theme_dark
        } else {
            &self.theme_light
        }
    }

    /// Make `name` the theme shown now: the static theme, or the dynamic slot currently in use.
    pub fn set_active_theme(&mut self, name: String, system_dark: bool) {
        if self.theme_selection != "dynamic" {
            self.theme = name;
            return;
        }
        let dark = match self.theme_mode.as_str() {
            "light" => false,
            "dark" => true,
            _ => system_dark,
        };
        if dark {
            self.theme_dark = name;
        } else {
            self.theme_light = name;
        }
    }

    pub fn sound_for(&self, event: &str) -> &str {
        match event {
            "working" => &self.sound_working,
            "finished" => &self.sound_finished,
            "needs_input" => &self.sound_needs_input,
            "compacting" => &self.sound_compacting,
            _ => "",
        }
    }

    pub fn sound_for_mut(&mut self, event: &str) -> Option<&mut String> {
        match event {
            "working" => Some(&mut self.sound_working),
            "finished" => Some(&mut self.sound_finished),
            "needs_input" => Some(&mut self.sound_needs_input),
            "compacting" => Some(&mut self.sound_compacting),
            _ => None,
        }
    }

    /// Whether an agent event is announced; `viewing` means the user is looking at that workspace.
    pub fn announces(&self, viewing: bool) -> bool {
        self.notify_claude && (!viewing || self.notify_when_focused)
    }

    pub fn path() -> Option<PathBuf> {
        Some(pom_paths::config_dir()?.join("settings.json"))
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
    fn announcing_follows_the_master_switch_and_the_focus_rule() {
        let mut s = Settings::default();
        assert!(s.announces(false));
        assert!(!s.announces(true));
        s.notify_when_focused = true;
        assert!(s.announces(true));
        s.notify_claude = false;
        assert!(!s.announces(false));
        assert_eq!(s.sound_for("finished"), "Glass");
        assert_eq!(s.sound_for("working"), "");
    }

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"ui_font_size": 15.0}"#).unwrap();
        assert_eq!(s.ui_font_size, 15.0);
        assert_eq!(s.theme, "One Dark"); // missing key -> default
    }
}
