//! The settings screen, built from the `ui` element tree. Layout mirrors a standard settings window: a
//! category sidebar on the left, and on the right a page whose sections are a small muted header + divider,
//! then rows spread `title (+ muted description)` on the left, control on the right. The window manages its
//! own title bar, so both columns inset their top for the traffic lights. Controls are read-only rounded
//! chips until input handling lands (edit `~/.config/pomelo/settings.json`). Pure — the binary owns display.

mod settings_view;
pub use settings_view::{SettingsView, SideEffects};

use settings::Settings;
use ui::{
    button_static, chevron_down, chevron_right, chevron_up_down, close_icon, div, divider_faded,
    label, render, search_icon, theme, ButtonStyle, Div, LabelSize, Node, Rect, Rgba,
};

// Semantic colors resolved from the active theme (no hardcoded hex), so the settings screen restyles with
// the theme. Names map the screen's needs onto the design-system tokens.
fn bg_c() -> Rgba {
    // The settings content pane sits on editor_background (darker than the window background), which is what
    // makes the ghost-outlined controls' faint border_variant edge read against it.
    theme().editor_background
}
fn sidebar_c() -> Rgba {
    theme().panel_background
}
fn search_c() -> Rgba {
    theme().editor_background
}
fn sel_c() -> Rgba {
    theme().element_selected
}
fn chip_c() -> Rgba {
    theme().element_background
}
fn border_c() -> Rgba {
    theme().border
}
fn hover_c() -> Rgba {
    theme().element_hover
}
fn title_c() -> Rgba {
    theme().text
}
fn dim_c() -> Rgba {
    theme().text_muted
}
fn menu_c() -> Rgba {
    theme().elevated_surface_background
}

const TITLEBAR: f32 = 28.0; // top clearance for the traffic lights
const ROUND: f32 = 6.0;
// Navbar item metrics from the reference's TreeViewItem: lead (disclosure) column and the sub-item indent
// column that centers the 1px guide line.
const NAV_LEAD_W: f32 = 16.0;
const NAV_INDENT_W: f32 = 22.0;

// Navbar highlight/guide colors, matching the reference TreeViewItem (element_active @ 0.5, border @ 0.4/0.5).
fn nav_sel_bg() -> Rgba {
    theme().element_active.alpha(0.5)
}
fn nav_sel_border() -> Rgba {
    theme().border.alpha(0.4)
}
fn nav_guide() -> Rgba {
    theme().border.alpha(0.5)
}

/// The reference navbar item box: full-width, rounded, a reserved 1px border (colored when selected), subtle
/// selected/hover fill. Both categories and sub-sections use it so the highlight spans the whole width.
fn nav_item_box(active: bool, hovered: bool) -> Div {
    let mut b = div()
        .row()
        .h_px(28.0)
        .pl(2.0)
        .pr(4.0)
        .gap(8.0)
        .items_center()
        .rounded(4.0)
        .border(
            1.0,
            if active {
                nav_sel_border()
            } else {
                Rgba::TRANSPARENT
            },
        );
    if active {
        b = b.bg(nav_sel_bg());
    } else if hovered {
        b = b.bg(hover_c());
    }
    b
}

// Categories and their sub-sections. Trimmed to what we have (or will fill soon); a root with sub-sections
// gets a disclosure chevron and expands to show them (indented, with a guide line).
// The Appearance page's section headers, in order. Single source of truth: the navbar lists them as jump
// entries and `appearance_page` emits the same headers, so the two never drift.
const APPEARANCE_SECTIONS: [&str; 2] = ["Theme", "UI Font"];
const WINDOW_LAYOUT_SECTIONS: [&str; 5] = ["Status Bar", "Title Bar", "Window", "Docks", "Panels"];

const INTEGRATIONS_SECTIONS: [&str; 2] = ["Jira", "Main Workspace"];
const GENERAL_SECTIONS: [&str; 2] = ["Startup", "Updates"];
const EDITOR_SECTIONS: [&str; 2] = ["Buffer Font", "Behavior"];
const TERMINAL_SECTIONS: [&str; 2] = ["Font", "Shell"];
const KEYMAP_SECTIONS: [&str; 1] = ["Bindings"];
const AGENT_SECTIONS: [&str; 2] = ["Command", "Claude Code"];
const NOTIFICATIONS_SECTIONS: [&str; 2] = ["Delivery", "Alert Sounds"];
const NETWORK_SECTIONS: [&str; 3] = ["Reverse Proxy", "Webhook Fan-out", "Recent Requests"];
const PROJECT_SECTIONS: [&str; 3] = ["Repositories", "Config", "Config Bundle"];

const CATEGORIES: [(&str, &[&str]); 11] = [
    ("General", &GENERAL_SECTIONS),
    ("Appearance", &APPEARANCE_SECTIONS),
    ("Window & Layout", &WINDOW_LAYOUT_SECTIONS),
    ("Editor", &EDITOR_SECTIONS),
    ("Terminal", &TERMINAL_SECTIONS),
    ("Keymap", &KEYMAP_SECTIONS),
    ("Agent", &AGENT_SECTIONS),
    ("Notifications", &NOTIFICATIONS_SECTIONS),
    ("Network", &NETWORK_SECTIONS),
    ("Integrations", &INTEGRATIONS_SECTIONS),
    ("Project", &PROJECT_SECTIONS),
];

pub const GENERAL: usize = 0;
pub const WINDOW_LAYOUT: usize = 2;
pub const EDITOR: usize = 3;
pub const TERMINAL: usize = 4;
pub const KEYMAP: usize = 5;
pub const AGENT: usize = 6;
pub const NOTIFICATIONS: usize = 7;
pub const NETWORK: usize = 8;
pub const INTEGRATIONS: usize = 9;
pub const PROJECT: usize = 10;

/// Index of the Appearance category (the only page with real content for now).
pub const APPEARANCE: usize = 1;
/// Number of categories, so the app can size its expanded-state vector.
pub const CATEGORY_COUNT: usize = CATEGORIES.len();

// Control click ids (>= 100 so they never collide with category ids). The app routes these to
// `handle_control`, which mutates the setting like the reference's enum-variant/stepper write.
pub const CTRL_THEME: u64 = 101;
pub const CTRL_FONT_FAMILY: u64 = 102;
/// The sidebar search box; clicking it focuses text entry that filters the page.
pub const CTRL_SEARCH: u64 = 103;
/// The clear (x) button in the search box; clicking it empties the query.
pub const CTRL_SEARCH_CLEAR: u64 = 104;
pub const NAV_JUMP_BASE: u64 = 400;
pub const NAV_JUMP_STRIDE: u64 = 10;
/// A category's disclosure toggle (the chevron): clicking id `NAV_TOGGLE_BASE + category_index` expands or
/// collapses it. Only the chevron toggles; clicking the row selects the category (reference behavior).
pub const NAV_TOGGLE_BASE: u64 = 500;
/// A section header's geometry marker in the page (so the app can read its y to scroll to it). Not a visible
/// affordance -- it only exists to record the header's rect in the hit list.
pub const SECTION_ANCHOR_BASE: u64 = 5000;
pub const CTRL_FONT_SIZE_DEC: u64 = 200;
pub const CTRL_FONT_SIZE_INC: u64 = 201;
/// The font-size value box; clicking it starts inline numeric editing.
pub const CTRL_FONT_SIZE_EDIT: u64 = 202;
pub const CTRL_FONT_WEIGHT_DEC: u64 = 203;
pub const CTRL_FONT_WEIGHT_INC: u64 = 204;
/// The font-weight value box; clicking it starts inline numeric editing.
pub const CTRL_FONT_WEIGHT_EDIT: u64 = 205;
/// "Edit in settings.json" buttons for the not-yet-implemented font fields (features/fallbacks).
pub const CTRL_FONT_FEATURES: u64 = 206;
pub const CTRL_FONT_FALLBACKS: u64 = 207;
pub const CTRL_OPEN_JSON: u64 = 108;
pub const CTRL_MODE: u64 = 210;
pub const CTRL_SIDEBAR_SIDE: u64 = 211;
pub const CTRL_AGENT_SIDE: u64 = 212;
pub const CTRL_TERMINAL_SIDE: u64 = 213;
pub const CTRL_SHOW_AGENT: u64 = 214;
pub const CTRL_SHOW_TERMINAL: u64 = 215;
pub const CTRL_WIN_W_DEC: u64 = 216;
pub const CTRL_WIN_W_INC: u64 = 217;
pub const CTRL_WIN_W_EDIT: u64 = 218;
pub const CTRL_WIN_H_DEC: u64 = 219;
pub const CTRL_WIN_H_INC: u64 = 220;
pub const CTRL_WIN_H_EDIT: u64 = 221;
pub const CTRL_SHOW_DIAGNOSTICS: u64 = 222;
pub const CTRL_SHOW_CURSOR: u64 = 223;
pub const CTRL_SHOW_LANGUAGE: u64 = 224;
pub const CTRL_SHOW_BRANCH: u64 = 225;
pub const CTRL_SHOW_SESSION: u64 = 226;
pub const CTRL_JIRA_SITE: u64 = 230;
pub const CTRL_JIRA_EMAIL: u64 = 231;
pub const CTRL_JIRA_TOKEN: u64 = 232;
pub const CTRL_JIRA_RESET_TOKEN: u64 = 233;
pub const CTRL_JIRA_TEST: u64 = 234;
pub const CTRL_JIRA_ONLY_MINE: u64 = 235;
pub const CTRL_REFRESH_MAIN: u64 = 236;
pub const CTRL_REFRESH_DEC: u64 = 237;
pub const CTRL_REFRESH_INC: u64 = 238;
pub const CTRL_REFRESH_EDIT: u64 = 239;
pub const CTRL_AGENT_COMMAND: u64 = 240;
pub const CTRL_REINSTALL_AGENTS: u64 = 241;
pub const CTRL_NOTIFY: u64 = 243;
pub const CTRL_NOTIFY_FOCUSED: u64 = 244;
pub const CTRL_TEST_NOTIFICATION: u64 = 245;
/// A sound dropdown per agent event: id = base + index into `settings::AGENT_EVENTS`.
pub const CTRL_SOUND_BASE: u64 = 246;
pub const CTRL_START_SERVERS: u64 = 250;
pub const CTRL_START_AT_LOGIN: u64 = 260;
pub const CTRL_AUTO_UPDATE: u64 = 261;
pub const CTRL_CHECK_UPDATES: u64 = 262;
pub const CTRL_BUFFER_FONT_DEC: u64 = 263;
pub const CTRL_BUFFER_FONT_INC: u64 = 264;
pub const CTRL_BUFFER_FONT_EDIT: u64 = 265;
pub const CTRL_SOFT_WRAP: u64 = 266;
pub const CTRL_DIFF_VIEW: u64 = 267;
pub const CTRL_EXTERNAL_EDITOR: u64 = 268;
pub const CTRL_TERM_FONT_DEC: u64 = 269;
pub const CTRL_TERM_FONT_INC: u64 = 270;
pub const CTRL_TERM_FONT_EDIT: u64 = 271;
pub const CTRL_TERM_SHELL: u64 = 272;
pub const CTRL_SCROLLBACK_DEC: u64 = 273;
pub const CTRL_SCROLLBACK_INC: u64 = 274;
pub const CTRL_SCROLLBACK_EDIT: u64 = 275;
pub const CTRL_EDIT_KEYMAP: u64 = 276;
pub const CTRL_EXPORT_CONFIG: u64 = 277;
pub const CTRL_IMPORT_CONFIG: u64 = 278;
pub const CTRL_EDIT_PROJECT_CONFIG: u64 = 279;
pub const CTRL_ADD_REPO: u64 = 280;
pub const CTRL_APPLY_CONFIG: u64 = 281;
pub const CTRL_SPLIT_CONFIG: u64 = 282;
pub const CTRL_NORMALIZE_CONFIG: u64 = 283;
/// Per repository row of the Project page: id = base + row index.
pub const CTRL_REPO_RENAME_BASE: u64 = 20_000;
pub const CTRL_REPO_REMOVE_BASE: u64 = 21_000;
pub const CTRL_REPO_LIMIT: u64 = 1_000;
pub const SCROLLBACK_MIN: u32 = 1_000;
pub const SCROLLBACK_MAX: u32 = 100_000;

/// Editors "Open in External Editor" knows, in the order it tries them when none is picked.
pub const EXTERNAL_EDITORS: [&str; 6] = [
    "Visual Studio Code",
    "Cursor",
    "Zed",
    "Windsurf",
    "Sublime Text",
    "IntelliJ IDEA",
];
pub const REFRESH_MINUTES_MIN: u64 = 1;
pub const REFRESH_MINUTES_MAX: u64 = 1440;
pub const WIN_W_MIN: f32 = 640.0;
pub const WIN_W_MAX: f32 = 4000.0;
pub const WIN_H_MIN: f32 = 480.0;
pub const WIN_H_MAX: f32 = 3000.0;
pub const RESET_OFFSET: u64 = 100_000;

/// Font-size bounds, matching the reference's `FontSize` stepper (min 6, max 72).
pub const FONT_SIZE_MIN: f32 = 6.0;
pub const FONT_SIZE_MAX: f32 = 72.0;
/// Font-weight bounds, matching the reference's `FontWeight` stepper (CSS weights 100-900).
pub const FONT_WEIGHT_MIN: f32 = 100.0;
pub const FONT_WEIGHT_MAX: f32 = 900.0;

/// Commit a typed font size (clamped); returns true if it changed.
pub fn set_font_size(s: &mut Settings, value: f32) -> bool {
    let next = value.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
    let changed = (next - s.ui_font_size).abs() > 0.001;
    s.ui_font_size = next;
    changed
}

/// Commit a typed font weight (clamped to 100-900); returns true if it changed.
pub fn set_font_weight(s: &mut Settings, value: f32) -> bool {
    let next = value.clamp(FONT_WEIGHT_MIN, FONT_WEIGHT_MAX);
    let changed = (next - s.ui_font_weight).abs() > 0.001;
    s.ui_font_weight = next;
    changed
}

pub fn chrome_flags(s: &Settings) -> ui::ChromeFlags {
    ui::ChromeFlags {
        diagnostics: s.show_diagnostics,
        cursor_position: s.show_cursor_position,
        language: s.show_language,
        branch: s.show_branch,
        session_name: s.show_session_name,
    }
}

const THEMES: [&str; 4] = ["One Dark", "One Light", "Ayu Mirage", "Gruvbox Dark"];

/// The theme after `current` in the theme list, wrapping around.
pub fn next_theme(current: &str) -> &'static str {
    let at = THEMES.iter().position(|theme| *theme == current);
    THEMES[at.map_or(0, |at| (at + 1) % THEMES.len())]
}

/// Popover-item click ids start here (an item's id = POPOVER_BASE + its index in the option list).
pub const POPOVER_BASE: u64 = 3000;

/// True if a control opens a popover list (dropdown) rather than acting immediately (stepper).
pub fn is_dropdown(id: u64) -> bool {
    matches!(
        id,
        CTRL_THEME
            | CTRL_FONT_FAMILY
            | CTRL_MODE
            | CTRL_SIDEBAR_SIDE
            | CTRL_AGENT_SIDE
            | CTRL_TERMINAL_SIDE
            | CTRL_SOFT_WRAP
            | CTRL_DIFF_VIEW
            | CTRL_EXTERNAL_EDITOR
    ) || sound_event(id).is_some()
}

/// The agent event a sound dropdown id stands for.
pub fn sound_event(id: u64) -> Option<&'static str> {
    let index = id.checked_sub(CTRL_SOUND_BASE)? as usize;
    settings::AGENT_EVENTS.get(index).map(|(event, _)| *event)
}

const NO_SOUND: &str = "None";

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn control_items(id: u64, fonts: &[String]) -> Vec<String> {
    let sv = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
    match id {
        CTRL_THEME => THEMES.iter().map(|s| s.to_string()).collect(),
        CTRL_FONT_FAMILY => fonts.to_vec(),
        CTRL_MODE => sv(&["System", "Light", "Dark"]),
        CTRL_SIDEBAR_SIDE | CTRL_AGENT_SIDE => sv(&["Left", "Right"]),
        CTRL_TERMINAL_SIDE => sv(&["Left", "Right", "Bottom"]),
        CTRL_SOFT_WRAP => sv(&["None", "Editor Width"]),
        CTRL_DIFF_VIEW => sv(&["Split", "Unified"]),
        CTRL_EXTERNAL_EDITOR => std::iter::once("Auto")
            .chain(EXTERNAL_EDITORS)
            .map(str::to_string)
            .collect(),
        id if sound_event(id).is_some() => std::iter::once(NO_SOUND)
            .chain(settings::SYSTEM_SOUNDS)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// The control's current value (for highlighting the selected popover item).
pub fn control_value(id: u64, s: &Settings) -> String {
    match id {
        CTRL_THEME => s.theme.clone(),
        CTRL_FONT_FAMILY => s.ui_font.clone(),
        CTRL_MODE => cap(&s.theme_mode),
        CTRL_SIDEBAR_SIDE => cap(&s.sidebar_side),
        CTRL_AGENT_SIDE => cap(&s.agent_side),
        CTRL_TERMINAL_SIDE => cap(&s.terminal_side),
        CTRL_SOFT_WRAP => if s.soft_wrap { "Editor Width" } else { "None" }.to_string(),
        CTRL_DIFF_VIEW => if s.split_diff { "Split" } else { "Unified" }.to_string(),
        CTRL_EXTERNAL_EDITOR => external_editor_label(&s.external_editor),
        id => match sound_event(id) {
            Some(event) => sound_label(s.sound_for(event)),
            None => String::new(),
        },
    }
}

/// Apply a popover selection (`index` into `control_items`) to settings; returns true if changed.
pub fn apply_choice(id: u64, index: usize, fonts: &[String], s: &mut Settings) -> bool {
    let items = control_items(id, fonts);
    let Some(val) = items.get(index) else {
        return false;
    };
    match id {
        CTRL_THEME => s.theme = val.clone(),
        CTRL_FONT_FAMILY => s.ui_font = val.clone(),
        CTRL_MODE => s.theme_mode = val.to_lowercase(),
        CTRL_SIDEBAR_SIDE => s.sidebar_side = val.to_lowercase(),
        CTRL_AGENT_SIDE => s.agent_side = val.to_lowercase(),
        CTRL_TERMINAL_SIDE => s.terminal_side = val.to_lowercase(),
        CTRL_SOFT_WRAP => s.soft_wrap = val == "Editor Width",
        CTRL_DIFF_VIEW => s.split_diff = val == "Split",
        CTRL_EXTERNAL_EDITOR => {
            s.external_editor = if val == "Auto" {
                String::new()
            } else {
                val.clone()
            }
        }
        id => {
            let Some(sound) = sound_event(id).and_then(|event| s.sound_for_mut(event)) else {
                return false;
            };
            *sound = if val == NO_SOUND {
                String::new()
            } else {
                val.clone()
            };
        }
    }
    true
}

fn external_editor_label(editor: &str) -> String {
    if editor.is_empty() {
        "Auto".to_string()
    } else {
        editor.to_string()
    }
}

fn sound_label(sound: &str) -> String {
    if sound.is_empty() {
        NO_SOUND.to_string()
    } else {
        sound.to_string()
    }
}

pub fn is_default(id: u64, s: &Settings) -> bool {
    let d = Settings::default();
    match id {
        CTRL_THEME => s.theme == d.theme,
        CTRL_FONT_FAMILY => s.ui_font == d.ui_font,
        CTRL_FONT_SIZE_EDIT => s.ui_font_size == d.ui_font_size,
        CTRL_FONT_WEIGHT_EDIT => s.ui_font_weight == d.ui_font_weight,
        CTRL_MODE => s.theme_mode == d.theme_mode,
        CTRL_SIDEBAR_SIDE => s.sidebar_side == d.sidebar_side,
        CTRL_AGENT_SIDE => s.agent_side == d.agent_side,
        CTRL_TERMINAL_SIDE => s.terminal_side == d.terminal_side,
        CTRL_SHOW_AGENT => s.agent_hidden == d.agent_hidden,
        CTRL_SHOW_TERMINAL => s.terminal_hidden == d.terminal_hidden,
        CTRL_SHOW_DIAGNOSTICS => s.show_diagnostics == d.show_diagnostics,
        CTRL_SHOW_CURSOR => s.show_cursor_position == d.show_cursor_position,
        CTRL_SHOW_LANGUAGE => s.show_language == d.show_language,
        CTRL_SHOW_BRANCH => s.show_branch == d.show_branch,
        CTRL_SHOW_SESSION => s.show_session_name == d.show_session_name,
        CTRL_JIRA_ONLY_MINE => s.jira_only_mine == d.jira_only_mine,
        CTRL_WIN_W_EDIT => s.window_width == d.window_width,
        CTRL_WIN_H_EDIT => s.window_height == d.window_height,
        CTRL_AGENT_COMMAND => s.agent_command == d.agent_command,
        CTRL_AUTO_UPDATE => s.auto_update == d.auto_update,
        CTRL_BUFFER_FONT_EDIT => s.buffer_font_size == d.buffer_font_size,
        CTRL_SOFT_WRAP => s.soft_wrap == d.soft_wrap,
        CTRL_DIFF_VIEW => s.split_diff == d.split_diff,
        CTRL_EXTERNAL_EDITOR => s.external_editor == d.external_editor,
        CTRL_TERM_FONT_EDIT => s.terminal_font_size == d.terminal_font_size,
        CTRL_TERM_SHELL => s.terminal_shell == d.terminal_shell,
        CTRL_SCROLLBACK_EDIT => s.terminal_scrollback == d.terminal_scrollback,
        CTRL_NOTIFY => s.notify_claude == d.notify_claude,
        CTRL_NOTIFY_FOCUSED => s.notify_when_focused == d.notify_when_focused,
        id => sound_event(id).is_none_or(|event| s.sound_for(event) == d.sound_for(event)),
    }
}

pub fn reset_to_default(id: u64, s: &mut Settings) -> bool {
    let d = Settings::default();
    let changed = !is_default(id, s);
    match id {
        CTRL_THEME => s.theme = d.theme,
        CTRL_FONT_FAMILY => s.ui_font = d.ui_font,
        CTRL_FONT_SIZE_EDIT => s.ui_font_size = d.ui_font_size,
        CTRL_FONT_WEIGHT_EDIT => s.ui_font_weight = d.ui_font_weight,
        CTRL_MODE => s.theme_mode = d.theme_mode,
        CTRL_SIDEBAR_SIDE => s.sidebar_side = d.sidebar_side,
        CTRL_AGENT_SIDE => s.agent_side = d.agent_side,
        CTRL_TERMINAL_SIDE => s.terminal_side = d.terminal_side,
        CTRL_SHOW_AGENT => s.agent_hidden = d.agent_hidden,
        CTRL_SHOW_TERMINAL => s.terminal_hidden = d.terminal_hidden,
        CTRL_SHOW_DIAGNOSTICS => s.show_diagnostics = d.show_diagnostics,
        CTRL_SHOW_CURSOR => s.show_cursor_position = d.show_cursor_position,
        CTRL_SHOW_LANGUAGE => s.show_language = d.show_language,
        CTRL_SHOW_BRANCH => s.show_branch = d.show_branch,
        CTRL_SHOW_SESSION => s.show_session_name = d.show_session_name,
        CTRL_JIRA_ONLY_MINE => s.jira_only_mine = d.jira_only_mine,
        CTRL_WIN_W_EDIT => s.window_width = d.window_width,
        CTRL_WIN_H_EDIT => s.window_height = d.window_height,
        CTRL_AGENT_COMMAND => s.agent_command = d.agent_command,
        CTRL_AUTO_UPDATE => s.auto_update = d.auto_update,
        CTRL_BUFFER_FONT_EDIT => s.buffer_font_size = d.buffer_font_size,
        CTRL_SOFT_WRAP => s.soft_wrap = d.soft_wrap,
        CTRL_DIFF_VIEW => s.split_diff = d.split_diff,
        CTRL_EXTERNAL_EDITOR => s.external_editor = d.external_editor,
        CTRL_TERM_FONT_EDIT => s.terminal_font_size = d.terminal_font_size,
        CTRL_TERM_SHELL => s.terminal_shell = d.terminal_shell,
        CTRL_SCROLLBACK_EDIT => s.terminal_scrollback = d.terminal_scrollback,
        CTRL_NOTIFY => s.notify_claude = d.notify_claude,
        CTRL_NOTIFY_FOCUSED => s.notify_when_focused = d.notify_when_focused,
        id => {
            let Some(event) = sound_event(id) else {
                return false;
            };
            let default = d.sound_for(event).to_string();
            let Some(sound) = s.sound_for_mut(event) else {
                return false;
            };
            *sound = default;
        }
    }
    changed
}

/// Apply a stepper control click to settings; returns true if a value changed (so the app persists).
pub fn handle_control(id: u64, s: &mut Settings) -> bool {
    match id {
        CTRL_FONT_SIZE_DEC => set_font_size(s, s.ui_font_size - 1.0),
        CTRL_FONT_SIZE_INC => set_font_size(s, s.ui_font_size + 1.0),
        CTRL_FONT_WEIGHT_DEC => set_font_weight(s, s.ui_font_weight - 100.0),
        CTRL_FONT_WEIGHT_INC => set_font_weight(s, s.ui_font_weight + 100.0),
        CTRL_WIN_W_DEC => set_clamped(&mut s.window_width, -20.0, WIN_W_MIN, WIN_W_MAX),
        CTRL_WIN_W_INC => set_clamped(&mut s.window_width, 20.0, WIN_W_MIN, WIN_W_MAX),
        CTRL_WIN_H_DEC => set_clamped(&mut s.window_height, -20.0, WIN_H_MIN, WIN_H_MAX),
        CTRL_WIN_H_INC => set_clamped(&mut s.window_height, 20.0, WIN_H_MIN, WIN_H_MAX),
        CTRL_SHOW_AGENT => {
            s.agent_hidden = !s.agent_hidden;
            true
        }
        CTRL_SHOW_TERMINAL => {
            s.terminal_hidden = !s.terminal_hidden;
            true
        }
        CTRL_SHOW_DIAGNOSTICS => {
            s.show_diagnostics = !s.show_diagnostics;
            true
        }
        CTRL_SHOW_CURSOR => {
            s.show_cursor_position = !s.show_cursor_position;
            true
        }
        CTRL_SHOW_LANGUAGE => {
            s.show_language = !s.show_language;
            true
        }
        CTRL_SHOW_BRANCH => {
            s.show_branch = !s.show_branch;
            true
        }
        CTRL_SHOW_SESSION => {
            s.show_session_name = !s.show_session_name;
            true
        }
        CTRL_JIRA_ONLY_MINE => {
            s.jira_only_mine = !s.jira_only_mine;
            true
        }
        CTRL_NOTIFY => {
            s.notify_claude = !s.notify_claude;
            true
        }
        CTRL_AUTO_UPDATE => {
            s.auto_update = !s.auto_update;
            true
        }
        CTRL_BUFFER_FONT_DEC => {
            set_clamped(&mut s.buffer_font_size, -1.0, FONT_SIZE_MIN, FONT_SIZE_MAX)
        }
        CTRL_BUFFER_FONT_INC => {
            set_clamped(&mut s.buffer_font_size, 1.0, FONT_SIZE_MIN, FONT_SIZE_MAX)
        }
        CTRL_TERM_FONT_DEC => set_clamped(
            &mut s.terminal_font_size,
            -1.0,
            FONT_SIZE_MIN,
            FONT_SIZE_MAX,
        ),
        CTRL_TERM_FONT_INC => {
            set_clamped(&mut s.terminal_font_size, 1.0, FONT_SIZE_MIN, FONT_SIZE_MAX)
        }
        CTRL_SCROLLBACK_DEC | CTRL_SCROLLBACK_INC => {
            let step: i64 = if id == CTRL_SCROLLBACK_INC {
                1_000
            } else {
                -1_000
            };
            let next = (i64::from(s.terminal_scrollback) + step)
                .clamp(i64::from(SCROLLBACK_MIN), i64::from(SCROLLBACK_MAX))
                as u32;
            let changed = next != s.terminal_scrollback;
            s.terminal_scrollback = next;
            changed
        }
        CTRL_NOTIFY_FOCUSED => {
            s.notify_when_focused = !s.notify_when_focused;
            true
        }
        _ => false,
    }
}

fn set_clamped(field: &mut f32, delta: f32, min: f32, max: f32) -> bool {
    let next = (*field + delta).clamp(min, max);
    let changed = next != *field;
    *field = next;
    changed
}

/// How many item rows the popover shows at once; longer lists scroll a window of this size.
pub const POPOVER_VISIBLE: usize = 12;
const POPOVER_ITEM_H: f32 = 30.0;
const POPOVER_SEARCH_H: f32 = 32.0;
const POPOVER_PAD: f32 = 4.0;
const POPOVER_GAP: f32 = 2.0;

/// Largest valid first-visible index for a list of `items_len` items.
pub fn popover_max_scroll(items_len: usize) -> usize {
    items_len.saturating_sub(POPOVER_VISIBLE)
}

/// Original indices of `items` matching `query` (case-insensitive substring), like the reference's font
/// picker filter. An empty query matches everything. Returns original indices so click ids stay stable.
pub fn filter_indices(items: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let q = query.to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

/// Truncate `text` with a trailing `...` so it fits `max_w` logical px at `size` (per-char measure; kerning
/// is close enough for a font list). Long font names would otherwise overflow the menu width.
fn truncate_to_width(text: &str, size: f32, mono: bool, max_w: f32) -> String {
    let weight = ui::ui_font_weight();
    if max_w <= 0.0 || ui::measure_text_width(text, size, mono, weight) <= max_w {
        return text.to_string();
    }
    let budget = (max_w - ui::measure_text_width("...", size, mono, weight)).max(0.0);
    let chars: Vec<char> = text.chars().collect();
    // Binary search the largest prefix whose width fits the budget (few measures, all cached).
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let prefix: String = chars[..mid].iter().collect();
        if ui::measure_text_width(&prefix, size, mono, weight) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut out: String = chars[..lo].iter().collect();
    out.push_str("...");
    out
}

const POPOVER_TEXT_INSET: f32 = 10.0; // px from the menu edge to search/item text (item: 4 inset + 6 inner)
const POPOVER_ITEM_INSET: f32 = 4.0; // reference ListItem inset (Base04): the row's outer horizontal padding
const POPOVER_ITEM_PAD: f32 = 6.0; // reference inner padding (Base06) inside the highlight
const POPOVER_ITEM_VINSET: f32 = 2.0; // vertical inset so the highlight doesn't fill the whole row
const POPOVER_ITEM_ROUND: f32 = 4.0;
const POPOVER_BOTTOM_MARGIN: f32 = 8.0; // keep the menu this far from the window edge
const POPOVER_MAX_H: f32 = 342.0; // reference max_height (rems(18)); the menu is this tall, not window-fit

/// The fixed number of item rows the popover shows (reference `max_height`, ~10 rows). Scale cancels out, so
/// this is a constant row count. The list scrolls within it rather than the menu shrinking to fit.
fn popover_row_cap() -> usize {
    // Each of the (rows + 1) children is separated by a gap, so a row effectively costs item_h + gap.
    let rows = ((POPOVER_MAX_H - POPOVER_SEARCH_H - 2.0 * POPOVER_PAD)
        / (POPOVER_ITEM_H + POPOVER_GAP))
        .floor()
        .max(1.0) as usize;
    POPOVER_VISIBLE.min(rows)
}

/// How many item rows the popover shows at once. The app uses this to clamp its scroll offset the same way.
pub fn popover_visible_cap(_anchor: Rect, _viewport_h: f32) -> usize {
    popover_row_cap()
}

/// Largest first-visible index for `shown` filtered items in the popover.
pub fn popover_max_scroll_for(shown: usize, _anchor: Rect, _viewport_h: f32) -> usize {
    shown.saturating_sub(popover_row_cap())
}

/// Build the popover menu for a dropdown control, anchored under its chip. `query` filters the list live (the
/// search field shows it with a caret); `scroll` is the first visible index into the FILTERED list; the row
/// count is capped so the menu fits above `viewport_h`. Item click ids stay mapped to original indices.
#[allow(clippy::too_many_arguments)]
pub fn popover(
    anchor: Rect,
    current: &str,
    items: &[String],
    scroll: usize,
    query: &str,
    hovered: Option<u64>,
    viewport_w: f32,
    viewport_h: f32,
) -> ui::Painted {
    let s = ui::ui_text_scale();
    let item_h = POPOVER_ITEM_H * s;
    let search_h = POPOVER_SEARCH_H * s;
    let pad = POPOVER_PAD * s;
    let gap = POPOVER_GAP * s;

    let matches = filter_indices(items, query);
    let w = (210.0 * s).max(anchor.w);
    // The reference anchors the menu's top-left to the trigger's left, then clamps it on screen: near the
    // right edge it shifts left so the menu's right edge sits a small margin from the window edge.
    let margin = POPOVER_BOTTOM_MARGIN * s;
    let mut x = anchor.x;
    if x + w > viewport_w - margin {
        x = viewport_w - margin - w;
    }
    x = x.max(margin);

    // Fixed height (the reference's max_height), not shrunk to the room below: the list scrolls inside it.
    // Open below the trigger; if it would run past the window bottom, shift it up just enough to fit -- it
    // may overlap the trigger, which is fine (the reference lets the menu cover its button). No flip-above.
    // (rows + 1) children each separated by a gap: a row costs item_h + gap.
    let chrome = search_h + 2.0 * pad;
    let row_h = item_h + gap;
    let menu_h = |rows: usize| chrome + rows as f32 * row_h;
    let avail = viewport_h - 2.0 * margin;
    let visible = if menu_h(matches.len()) <= avail {
        matches.len().min(popover_row_cap())
    } else {
        // Window shorter than the menu: cap rows to what fits at all.
        (((avail - chrome) / row_h).floor().max(1.0) as usize).min(popover_row_cap())
    };
    let h = menu_h(visible);
    let mut y = anchor.y + anchor.h + 2.0 * s;
    if y + h > viewport_h - margin {
        y = viewport_h - margin - h;
    }
    y = y.max(margin);
    let scroll = scroll.min(matches.len().saturating_sub(visible));

    // The highlighted "cursor" row: the sticky hovered item if it's still in the filtered list, else the
    // current value if visible, else the first match. Keeps the highlight from vanishing/jumping on filter.
    let id_of = |orig: usize| POPOVER_BASE + orig as u64;
    let cursor_id = hovered
        .filter(|h| matches.iter().any(|&o| id_of(o) == *h))
        .or_else(|| {
            matches
                .iter()
                .find(|&&o| items[o] == *current)
                .map(|&o| id_of(o))
        })
        .or_else(|| matches.first().map(|&o| id_of(o)));

    let search_label = if query.is_empty() {
        label("Search...").size(12.0).color(dim_c())
    } else {
        label(query).size(12.0).color(title_c())
    };
    let mut col = div()
        .col()
        .bg(menu_c())
        .rounded(8.0)
        .border(1.0, border_c())
        .p(POPOVER_PAD)
        .gap(POPOVER_GAP)
        // The picker's search bar is borderless: just the placeholder/query on the menu background (unlike the
        // sidebar search, which is a bordered field).
        .child(
            div()
                .h_px(POPOVER_SEARCH_H)
                .px(POPOVER_TEXT_INSET)
                .items_center()
                .child(search_label),
        );
    for i in 0..visible {
        let Some(&orig) = matches.get(scroll + i) else {
            break;
        };
        let it = &items[orig];
        let this_id = POPOVER_BASE + orig as u64;
        let highlighted = cursor_id == Some(this_id);
        let mut inner = div()
            .row()
            .flex(1.0)
            .px(POPOVER_ITEM_PAD)
            .items_center()
            .justify_between()
            .rounded(POPOVER_ITEM_ROUND)
            .child(
                label(truncate_to_width(it, 13.0, false, w - 44.0 * s))
                    .size(13.0)
                    .color(if highlighted { title_c() } else { dim_c() }),
            );
        if highlighted {
            inner = inner.bg(sel_c());
        }
        col = col.child(
            div()
                .row()
                .h_px(POPOVER_ITEM_H)
                .px(POPOVER_ITEM_INSET)
                .py(POPOVER_ITEM_VINSET)
                .items_center()
                .on_click(POPOVER_BASE + orig as u64)
                .child(inner),
        );
    }
    let mut out = render(&col.into(), Rect::new(x, y, w, h, menu_c()));

    // A soft drop shadow gives the menu elevation off the page. We fake a blur with a few stacked, growing,
    // translucent rounded rects (no shadow primitive in the renderer yet), drawn behind the menu.
    let mut layered = Vec::with_capacity(out.rects.len() + 6);
    for i in (1..=6).rev() {
        let sp = i as f32 * 2.0 * s;
        layered.push(Rect {
            x: x - sp,
            y: y - sp + 4.0 * s,
            w: w + 2.0 * sp,
            h: h + 2.0 * sp,
            color: Rgba::new(0.0, 0.0, 0.0, 0.05),
            radius: 8.0 * s + sp,
            border: 0.0,
            border_color: Rgba::TRANSPARENT,
        });
    }
    layered.extend(out.rects);
    out.rects = layered;

    // Divider between the search bar and the results (edge to edge), like the reference.
    let bv = theme().border_variant;
    out.rects.push(Rect {
        x,
        y: y + pad + search_h,
        w,
        h: 1.0 * s,
        color: Rgba::new(bv.r, bv.g, bv.b, bv.a * 0.6),
        radius: 0.0,
        border: 0.0,
        border_color: Rgba::TRANSPARENT,
    });

    // Caret at the end of the query (the search field is always focused while the popover is open).
    if ui::caret_phase() {
        let caret_x = x
            + pad
            + POPOVER_TEXT_INSET * s
            + ui::measure_text_width(query, 12.0, false, ui::ui_font_weight());
        out.rects.push(Rect {
            x: caret_x,
            y: y + pad + (search_h - 15.0 * s) / 2.0,
            w: 1.5 * s,
            h: 15.0 * s,
            color: theme().text,
            radius: 0.0,
            border: 0.0,
            border_color: Rgba::TRANSPARENT,
        });
    }

    if matches.len() > visible {
        let track_top = y + pad + search_h + gap;
        let track_h = visible as f32 * item_h;
        let thumb_h = (track_h * visible as f32 / matches.len() as f32).max(24.0 * s);
        let travel = track_h - thumb_h;
        let max_scroll = matches.len().saturating_sub(visible).max(1) as f32;
        let thumb_y = track_top + travel * (scroll as f32 / max_scroll);
        out.rects.push(Rect {
            x: x + w - 8.0 * s,
            y: thumb_y,
            w: 4.0 * s,
            h: thumb_h,
            color: theme().scrollbar_thumb_background,
            radius: 2.0 * s,
            border: 0.0,
            border_color: Rgba::TRANSPARENT,
        });
    }
    out
}

const SIDEBAR_W: f32 = 240.0;
const CONTENT_PAD: f32 = 32.0;
// Top inset of the content column before the toolbar. The traffic lights sit over the sidebar (not the
// content), so the content doesn't reserve titlebar height -- it matches the reference's content `pt_6` (24).
const CONTENT_TOP: f32 = 24.0;
// The fixed top strip of the content column: top inset + toolbar (30) + gap (16). The scrollable page starts
// below it.
const PAGE_TOP: f32 = CONTENT_TOP + 30.0 + 16.0;

fn rem(v: f32) -> f32 {
    v * ui::ui_text_scale()
}

/// The scrollable content region (logical px): right of the sidebar, below the toolbar strip. The app scissors
/// the page to this and scrolls within it.
pub fn content_region(w: f32, h: f32) -> (f32, f32, f32, f32) {
    let x = rem(SIDEBAR_W);
    let y = rem(PAGE_TOP);
    (x, y, (w - x).max(0.0), (h - y).max(0.0))
}

/// The content-page scrollbar thumb (logical px), or None when everything fits. Same style as the popover's.
pub fn content_scrollbar(clip: (f32, f32, f32, f32), total_h: f32, scroll: f32) -> Option<Rect> {
    let (cx, cy, cw, ch) = clip;
    if total_h <= ch || ch <= 0.0 {
        return None;
    }
    let thumb_h = (ch * ch / total_h).max(rem(24.0)).min(ch);
    let max_scroll = (total_h - ch).max(1.0);
    let t = (scroll / max_scroll).clamp(0.0, 1.0);
    let wpx = rem(4.0);
    Some(Rect {
        x: cx + cw - wpx - rem(4.0),
        y: cy + t * (ch - thumb_h),
        w: wpx,
        h: thumb_h,
        color: theme().scrollbar_thumb_background,
        radius: rem(2.0),
        border: 0.0,
        border_color: Rgba::TRANSPARENT,
    })
}

/// The fixed chrome: the sidebar plus the toolbar strip (opaque, so it masks scrolled-away page content).
#[allow(clippy::too_many_arguments)]
pub fn chrome(
    selected: usize,
    hovered: Option<u64>,
    active_section: Option<usize>,
    expanded: &[bool],
    w: f32,
    h: f32,
    search: &str,
    search_active: bool,
) -> ui::Painted {
    let x = rem(SIDEBAR_W);
    let mut out = render(
        &sidebar(
            selected,
            hovered,
            active_section,
            expanded,
            search,
            search_active,
        ),
        Rect::new(0.0, 0.0, x, h, sidebar_c()),
    );
    let strip: Node = div()
        .col()
        .h_px(PAGE_TOP)
        .bg(bg_c())
        .px(CONTENT_PAD)
        .child(div().h_px(CONTENT_TOP))
        .child(toolbar())
        .into();
    let sp = render(
        &strip,
        Rect::new(x, 0.0, (w - x).max(0.0), rem(PAGE_TOP), bg_c()),
    );
    out.rects.extend(sp.rects);
    out.tris.extend(sp.tris);
    out.texts.extend(sp.texts);
    out.icons.extend(sp.icons);
    out.hits.extend(sp.hits);
    out
}

/// The scrollable page for `selected`, laid out in the content region offset up by `scroll`. Returns the
/// `Painted` (with a fixed content-region background rect first) and the total content height for clamping.
#[allow(clippy::too_many_arguments)]
pub fn page(
    selected: usize,
    s: &Settings,
    state: &PageState,
    editing: Option<(u64, &str)>,
    search: &str,
    w: f32,
    h: f32,
    scroll: f32,
) -> (ui::Painted, f32) {
    let cl = rem(SIDEBAR_W);
    let top = rem(PAGE_TOP);
    let pad = rem(CONTENT_PAD);
    let mut out = ui::Painted {
        rects: vec![Rect::new(
            cl,
            top,
            (w - cl).max(0.0),
            (h - top).max(0.0),
            bg_c(),
        )],
        ..Default::default()
    };
    let jira = &state.jira;
    let tree: Node = if selected == APPEARANCE {
        render_page(&appearance_page(s), search, editing, w)
    } else if selected == WINDOW_LAYOUT {
        render_page(&window_layout_page(s), search, editing, w)
    } else if selected == GENERAL {
        render_page(&general_page(s, &state.general), search, editing, w)
    } else if selected == EDITOR {
        render_page(&editor_page(s), search, editing, w)
    } else if selected == TERMINAL {
        render_page(&terminal_page(s), search, editing, w)
    } else if selected == KEYMAP {
        render_page(&keymap_page(&state.keymap), search, editing, w)
    } else if selected == AGENT {
        render_page(&agent_page(s, &state.agent), search, editing, w)
    } else if selected == NOTIFICATIONS {
        render_page(&notifications_page(s), search, editing, w)
    } else if selected == NETWORK {
        render_page(&network_page(&state.network), search, editing, w)
    } else if selected == PROJECT && !state.project.session.is_empty() {
        render_page(&project_page(&state.project), search, editing, w)
    } else if selected == PROJECT {
        div()
            .col()
            .gap(16.0)
            .child(
                label("Project")
                    .label_size(LabelSize::Large)
                    .color(title_c()),
            )
            .child(
                label("Open a project first: its repositories and config are edited here.")
                    .color(dim_c()),
            )
            .into()
    } else if selected == INTEGRATIONS && !jira.session.is_empty() {
        render_page(&integrations_page(s, jira), search, editing, w)
    } else if selected == INTEGRATIONS {
        div()
            .col()
            .gap(16.0)
            .child(
                label("Integrations")
                    .label_size(LabelSize::Large)
                    .color(title_c()),
            )
            .child(label("Open a project first: Jira is set up per project.").color(dim_c()))
            .into()
    } else {
        stub_body(CATEGORIES[selected].0)
    };
    let area = Rect::new(
        cl + pad,
        top - scroll,
        (w - cl - 2.0 * pad).max(0.0),
        100000.0,
        Rgba::TRANSPARENT,
    );
    let tp = render(&tree, area);
    let bottom = tp
        .rects
        .iter()
        .map(|r| r.y + r.h)
        .fold(top - scroll, f32::max);
    let total_h = (bottom - (top - scroll)).max(0.0);
    out.rects.extend(tp.rects);
    out.tris.extend(tp.tris);
    out.texts.extend(tp.texts);
    out.icons.extend(tp.icons);
    out.hits.extend(tp.hits);
    (out, total_h)
}

/// Full settings screen without scrolling (page under fixed chrome). Used by the snapshot tool and tests.
#[allow(clippy::too_many_arguments)]
pub fn panel(
    selected: usize,
    hovered: Option<u64>,
    expanded: &[bool],
    s: &Settings,
    state: &PageState,
    w: f32,
    h: f32,
    editing: Option<(u64, &str)>,
    search: &str,
    search_active: bool,
) -> ui::Painted {
    let (mut out, _) = page(selected, s, state, editing, search, w, h, 0.0);
    let ch = chrome(
        selected,
        hovered,
        Some(0),
        expanded,
        w,
        h,
        search,
        search_active,
    );
    out.rects.extend(ch.rects);
    out.tris.extend(ch.tris);
    out.texts.extend(ch.texts);
    out.icons.extend(ch.icons);
    out.hits.extend(ch.hits);
    out
}

fn sidebar(
    selected: usize,
    hovered: Option<u64>,
    active_section: Option<usize>,
    expanded: &[bool],
    search: &str,
    search_active: bool,
) -> Node {
    let mut col = div()
        .col()
        .w_px(240.0)
        .bg(sidebar_c())
        .px(10.0)
        .py(10.0)
        .gap(2.0)
        .child(div().h_px(TITLEBAR)); // clear the traffic lights

    // Search box: magnifier prefix, the query/placeholder, and a clear (x) button when non-empty (reference).
    let show_caret = search_active && ui::caret_phase();
    let text_area = if search.is_empty() {
        div()
            .row()
            .flex(1.0)
            .items_center()
            .child(caret(show_caret))
            .child(label("Search settings...").size(12.0).color(dim_c()))
    } else {
        div()
            .row()
            .flex(1.0)
            .items_center()
            .child(label(search).size(12.0).color(title_c()))
            .child(caret(show_caret))
    };
    let mut search_box = div()
        .row()
        .h_px(32.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .rounded(ROUND)
        .bg(search_c())
        .border(1.0, border_c())
        .on_click(CTRL_SEARCH)
        .child(search_icon().size(14.0).color(dim_c()))
        .child(text_area);
    if !search.is_empty() {
        search_box = search_box.child(
            div()
                .items_center()
                .on_click(CTRL_SEARCH_CLEAR)
                .child(close_icon().size(14.0).color(dim_c())),
        );
    }
    col = col.child(search_box);
    col = col.child(div().h_px(8.0));

    let searching = !search.is_empty();
    let q = search.to_lowercase();
    for (i, (name, subs)) in CATEGORIES.iter().enumerate() {
        // While searching, the navbar shows only categories with matching sections (or a matching name), and
        // lists just the matching sections -- mirroring the reference's filtered navbar.
        let sections: Vec<(usize, &str)> = matching_sections(i, &q);
        let name_matches = !searching || name.to_lowercase().contains(&q);
        if searching && sections.is_empty() && !name_matches {
            continue;
        }

        let is_selected = i == selected;
        let is_hovered = hovered == Some(i as u64) || hovered == Some(NAV_TOGGLE_BASE + i as u64);
        let has_subs = !subs.is_empty();
        // Searching force-expands so matches are visible; otherwise honor the stored expansion state.
        let is_expanded = searching || expanded.get(i).copied().unwrap_or(false);

        // Disclosure chevron: its own click target (toggles expand) with a hover state, like the reference's
        // IconButton. Clicking the row only selects; only the chevron collapses/expands.
        let toggle_id = NAV_TOGGLE_BASE + i as u64;
        let lead: Node = if has_subs {
            let chevron: Node = if is_expanded {
                chevron_down().size(14.0).color(dim_c()).into()
            } else {
                chevron_right().size(14.0).color(dim_c()).into()
            };
            let mut btn = div()
                .w_px(NAV_LEAD_W)
                .h_px(20.0)
                .items_center()
                .justify_center()
                .rounded(4.0)
                .on_click(toggle_id)
                .child(chevron);
            if hovered == Some(toggle_id) {
                btn = btn.bg(hover_c());
            }
            btn.into()
        } else {
            div().w_px(NAV_LEAD_W).into()
        };

        // Single navbar highlight: a category shows its box only when selected AND no sub-section is active
        // (for a page with sections the active section carries the highlight instead), like the reference.
        let cat_active = is_selected && active_section.is_none();
        let inner = nav_item_box(cat_active, is_hovered)
            .on_click(i as u64)
            .child(lead)
            .child(label(*name).size(14.0).color(title_c()));
        col = col.child(inner);

        if has_subs && is_expanded {
            for (section_idx, sub) in sections {
                let id = NAV_JUMP_BASE + i as u64 * NAV_JUMP_STRIDE + section_idx as u64;
                let is_active = !searching && is_selected && active_section == Some(section_idx);
                col = col.child(sub_item(sub, id, hovered == Some(id), is_active));
            }
        }
    }
    col.into()
}

fn page_for(cat: usize) -> Option<Page> {
    match cat {
        APPEARANCE => Some(appearance_page(&Settings::default())),
        WINDOW_LAYOUT => Some(window_layout_page(&Settings::default())),
        INTEGRATIONS => Some(integrations_page(
            &Settings::default(),
            &IntegrationsPage::default(),
        )),
        AGENT => Some(agent_page(&Settings::default(), &AgentPage::default())),
        GENERAL => Some(general_page(&Settings::default(), &GeneralPage::default())),
        EDITOR => Some(editor_page(&Settings::default())),
        TERMINAL => Some(terminal_page(&Settings::default())),
        KEYMAP => Some(keymap_page(&KeymapPage::default())),
        NOTIFICATIONS => Some(notifications_page(&Settings::default())),
        NETWORK => Some(network_page(&NetworkPage::default())),
        PROJECT => Some(project_page(&ProjectPage::default())),
        _ => None,
    }
}

fn matching_sections(cat: usize, query: &str) -> Vec<(usize, &'static str)> {
    let Some(page) = page_for(cat) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cur: Option<(usize, &'static str, bool)> = None; // (section index, name, matched)
    let mut idx = 0usize;
    for item in &page.items {
        match item {
            PageItem::Header(name) => {
                if let Some((i, n, matched)) = cur.take() {
                    if matched {
                        out.push((i, n));
                    }
                }
                let m = query.is_empty() || name.to_lowercase().contains(query);
                cur = Some((idx, name, m));
                idx += 1;
            }
            PageItem::Row(row) => {
                if let Some((_, _, matched)) = cur.as_mut() {
                    if !*matched && row_matches(row, query) {
                        *matched = true;
                    }
                }
            }
        }
    }
    if let Some((i, n, matched)) = cur.take() {
        if matched {
            out.push((i, n));
        }
    }
    out
}

/// An indented sub-section under an expanded category. The guide line sits under the parent's chevron centre
/// (x = px(10 sidebar) + 14 = 24, matching the chevron centre at px(10)+px(8 row)+6 = 24) and the text
/// left-aligns with the parent's name (px(10)+14+1+11 = 36 = parent name x px(10)+8+12+6).
fn sub_item(name: &str, id: u64, hovered: bool, active: bool) -> Node {
    // Reference: the sub-item is the same full-width box; its lead is a 22px column with a centered 1px guide
    // line (so the guide runs continuously and sits inside the box, not cutting across a narrower highlight).
    // The guide spans the full row pitch (item 28 + col gap 2) so consecutive sub-items' lines join into one
    // continuous vertical guide instead of leaving a gap.
    let indent = div()
        .w_px(NAV_INDENT_W)
        .h_px(28.0)
        .items_center()
        .justify_center()
        .child(div().w_px(1.0).h_px(30.0).bg(nav_guide()));
    nav_item_box(active, hovered)
        .on_click(id)
        .child(indent)
        .child(
            label(name)
                .size(13.0)
                .color(if active { title_c() } else { dim_c() }),
        )
        .into()
}

/// The content-pane top bar: file switcher on the left, "Edit in settings.json" on the right. Like the
/// reference's files header, it's `justify_between` so the right button stays pinned to the content's right
/// edge (never pushed off-window); the left group truncates/overlaps under it when space is tight.
fn toolbar() -> Node {
    // The left file group grows to fill, pushing the right button to the content's right edge. When the left
    // group's content is wider than its grown slot, it overflows under the (last-drawn, on-top) right button
    // rather than shoving it off-window.
    div()
        .row()
        .h_px(30.0)
        .items_center()
        .justify_between()
        .child(
            div()
                .row()
                .gap(8.0)
                .items_center()
                .child(button_static("User", ButtonStyle::TintedAccent))
                .child(button_static("pomelo-project", ButtonStyle::Subtle)),
        )
        .child(button_static(
            "Edit in settings.json",
            ButtonStyle::OutlinedGhost,
        ))
        .into()
}

fn stub_body(name: &str) -> Node {
    div()
        .col()
        .gap(10.0)
        .child(label(name).size(20.0).color(title_c()))
        .child(label("No settings here yet.").size(13.0).color(dim_c()))
        .into()
}

// A settings page as data (the reference's `SettingsPage`/`SettingsPageItem` model): an ordered list of
// section headers and setting rows. The screen renders from this, so it can be searched/filtered uniformly.
enum Control {
    Dropdown {
        id: u64,
        value: String,
    },
    Stepper {
        dec: u64,
        inc: u64,
        edit: u64,
        value: String,
    },
    /// An "Edit in settings.json" button for a field with no in-app editor yet (reference `.unimplemented()`).
    EditInJson {
        id: u64,
    },
    Toggle {
        id: u64,
        on: bool,
    },
    /// A one-line text field (the reference's settings input field); `masked` hides what is typed.
    TextInput {
        id: u64,
        value: String,
        placeholder: &'static str,
        masked: bool,
    },
    /// A stored secret: what is set, and a button to clear it (disabled when it comes from the environment).
    Configured {
        label: &'static str,
        button: u64,
        button_label: &'static str,
        enabled: bool,
    },
    Button {
        id: u64,
        label: &'static str,
        enabled: bool,
    },
    /// Several actions on one row, left to right.
    Buttons(Vec<(u64, &'static str)>),
    /// A server's Running/Stopped chip.
    Status {
        running: bool,
    },
    /// A read-only value.
    Value {
        text: String,
    },
}

struct SettingRow {
    title: std::borrow::Cow<'static, str>,
    description: std::borrow::Cow<'static, str>,
    control: Control,
    reset: Option<u64>,
}

enum PageItem {
    Header(&'static str),
    Row(SettingRow),
}

struct Page {
    title: &'static str,
    items: Vec<PageItem>,
}

fn reset_if_changed(id: u64, s: &Settings) -> Option<u64> {
    (!is_default(id, s)).then_some(id)
}

fn appearance_page(s: &Settings) -> Page {
    Page {
        title: "Appearance",
        items: vec![
            PageItem::Header("Theme"),
            PageItem::Row(SettingRow {
                title: "Theme".into(),
                description: "Color theme applied across the whole app.".into(),
                control: Control::Dropdown {
                    id: CTRL_THEME,
                    value: s.theme.clone(),
                },
                reset: reset_if_changed(CTRL_THEME, s),
            }),
            PageItem::Header("UI Font"),
            PageItem::Row(SettingRow {
                title: "Font Family".into(),
                description: "Font family used for interface text.".into(),
                control: Control::Dropdown {
                    id: CTRL_FONT_FAMILY,
                    value: s.ui_font.clone(),
                },
                reset: reset_if_changed(CTRL_FONT_FAMILY, s),
            }),
            PageItem::Row(SettingRow {
                title: "Font Size".into(),
                description: "Font size for UI elements.".into(),
                control: Control::Stepper {
                    dec: CTRL_FONT_SIZE_DEC,
                    inc: CTRL_FONT_SIZE_INC,
                    edit: CTRL_FONT_SIZE_EDIT,
                    value: format!("{:.0}", s.ui_font_size),
                },
                reset: reset_if_changed(CTRL_FONT_SIZE_EDIT, s),
            }),
            PageItem::Row(SettingRow {
                title: "Font Weight".into(),
                description: "Font weight for UI elements (100-900).".into(),
                control: Control::Stepper {
                    dec: CTRL_FONT_WEIGHT_DEC,
                    inc: CTRL_FONT_WEIGHT_INC,
                    edit: CTRL_FONT_WEIGHT_EDIT,
                    value: format!("{:.0}", s.ui_font_weight),
                },
                reset: reset_if_changed(CTRL_FONT_WEIGHT_EDIT, s),
            }),
            PageItem::Row(SettingRow {
                title: "Font Features".into(),
                description: "The OpenType features to enable for rendering in UI elements.".into(),
                control: Control::EditInJson {
                    id: CTRL_FONT_FEATURES,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Font Fallbacks".into(),
                description: "The font fallbacks to use for rendering in the UI.".into(),
                control: Control::EditInJson {
                    id: CTRL_FONT_FALLBACKS,
                },
                reset: None,
            }),
        ],
    }
}

fn window_layout_page(s: &Settings) -> Page {
    Page {
        title: "Window & Layout",
        items: vec![
            PageItem::Header("Status Bar"),
            PageItem::Row(SettingRow {
                title: "Show Diagnostics".into(),
                description: "Show the error/warning count in the status bar.".into(),
                control: Control::Toggle {
                    id: CTRL_SHOW_DIAGNOSTICS,
                    on: s.show_diagnostics,
                },
                reset: reset_if_changed(CTRL_SHOW_DIAGNOSTICS, s),
            }),
            PageItem::Row(SettingRow {
                title: "Show Cursor Position".into(),
                description: "Show the line and column of the cursor in the status bar.".into(),
                control: Control::Toggle {
                    id: CTRL_SHOW_CURSOR,
                    on: s.show_cursor_position,
                },
                reset: reset_if_changed(CTRL_SHOW_CURSOR, s),
            }),
            PageItem::Row(SettingRow {
                title: "Show Language".into(),
                description: "Show the active language of the editor in the status bar.".into(),
                control: Control::Toggle {
                    id: CTRL_SHOW_LANGUAGE,
                    on: s.show_language,
                },
                reset: reset_if_changed(CTRL_SHOW_LANGUAGE, s),
            }),
            PageItem::Header("Title Bar"),
            PageItem::Row(SettingRow {
                title: "Show Branch".into(),
                description: "Show the current git branch in the title bar.".into(),
                control: Control::Toggle {
                    id: CTRL_SHOW_BRANCH,
                    on: s.show_branch,
                },
                reset: reset_if_changed(CTRL_SHOW_BRANCH, s),
            }),
            PageItem::Row(SettingRow {
                title: "Show Session Name".into(),
                description: "Show the current session name in the title bar.".into(),
                control: Control::Toggle {
                    id: CTRL_SHOW_SESSION,
                    on: s.show_session_name,
                },
                reset: reset_if_changed(CTRL_SHOW_SESSION, s),
            }),
            PageItem::Header("Window"),
            PageItem::Row(SettingRow {
                title: "Window Width".into(),
                description: "Default width (px) of a new window.".into(),
                control: Control::Stepper {
                    dec: CTRL_WIN_W_DEC,
                    inc: CTRL_WIN_W_INC,
                    edit: CTRL_WIN_W_EDIT,
                    value: format!("{:.0}", s.window_width),
                },
                reset: reset_if_changed(CTRL_WIN_W_EDIT, s),
            }),
            PageItem::Row(SettingRow {
                title: "Window Height".into(),
                description: "Default height (px) of a new window.".into(),
                control: Control::Stepper {
                    dec: CTRL_WIN_H_DEC,
                    inc: CTRL_WIN_H_INC,
                    edit: CTRL_WIN_H_EDIT,
                    value: format!("{:.0}", s.window_height),
                },
                reset: reset_if_changed(CTRL_WIN_H_EDIT, s),
            }),
            PageItem::Header("Docks"),
            PageItem::Row(SettingRow {
                title: "Sidebar Side".into(),
                description: "Which side the WORKSPACES sidebar docks on.".into(),
                control: Control::Dropdown {
                    id: CTRL_SIDEBAR_SIDE,
                    value: cap(&s.sidebar_side),
                },
                reset: reset_if_changed(CTRL_SIDEBAR_SIDE, s),
            }),
            PageItem::Row(SettingRow {
                title: "Agent Dock Side".into(),
                description: "Which side of the editor the agent dock renders on.".into(),
                control: Control::Dropdown {
                    id: CTRL_AGENT_SIDE,
                    value: cap(&s.agent_side),
                },
                reset: reset_if_changed(CTRL_AGENT_SIDE, s),
            }),
            PageItem::Row(SettingRow {
                title: "Terminal Dock Side".into(),
                description: "Which content area the terminal renders in.".into(),
                control: Control::Dropdown {
                    id: CTRL_TERMINAL_SIDE,
                    value: cap(&s.terminal_side),
                },
                reset: reset_if_changed(CTRL_TERMINAL_SIDE, s),
            }),
            PageItem::Header("Panels"),
            PageItem::Row(SettingRow {
                title: "Show Agent Button".into(),
                description: "Show the agent toggle in the status bar.".into(),
                control: Control::Toggle {
                    id: CTRL_SHOW_AGENT,
                    on: !s.agent_hidden,
                },
                reset: reset_if_changed(CTRL_SHOW_AGENT, s),
            }),
            PageItem::Row(SettingRow {
                title: "Show Terminal Button".into(),
                description: "Show the terminal toggle in the status bar.".into(),
                control: Control::Toggle {
                    id: CTRL_SHOW_TERMINAL,
                    on: !s.terminal_hidden,
                },
                reset: reset_if_changed(CTRL_SHOW_TERMINAL, s),
            }),
        ],
    }
}

/// Where the Jira token comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TokenSource {
    #[default]
    Missing,
    Secret,
    Environment,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ConnectionStatus {
    #[default]
    Untested,
    Testing,
    SignedIn(String),
    Failed(String),
}

/// The per-project settings of the session the settings window was opened from (empty `session`: no project
/// open): Jira, and keeping main fresh.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoSummary {
    pub name: String,
    pub alias: String,
    pub services: usize,
    /// Main has its clone.
    pub cloned: bool,
    /// Workspaces besides main that have it, of how many.
    pub present: usize,
    pub workspaces: usize,
}

/// The open project's repositories, for the Project page (no session: no project open).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectPage {
    pub session: String,
    pub repos: Vec<RepoSummary>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntegrationsPage {
    pub session: String,
    pub keep_main_fresh: bool,
    pub refresh_minutes: u64,
    pub site: String,
    pub email: String,
    pub token: TokenSource,
    pub status: ConnectionStatus,
}

/// How far registering this app with Claude Code got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Registration {
    #[default]
    Pending,
    Done,
    /// A run on a throwaway state folder leaves the real agent config alone.
    Skipped,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentPage {
    pub mcp: Registration,
    pub hooks: Registration,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestRow {
    pub time: String,
    pub method: String,
    pub path: String,
    pub profile: String,
    pub target: String,
    pub status: u16,
    pub ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkPage {
    pub proxy_running: bool,
    pub webhook_running: bool,
    pub proxy_port: u16,
    pub webhook_port: u16,
    /// The app could not bind, but `pom proxy` in a terminal answers on the port.
    pub served_elsewhere: bool,
    /// Newest first.
    pub requests: Vec<RequestRow>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeneralPage {
    pub start_at_login: bool,
    pub version: String,
    /// Only the installed app replaces itself; a dev build says so.
    pub updates_apply: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeymapPage {
    /// (what it does, its action name, its binding or empty when unbound).
    pub rows: Vec<(String, String, String)>,
    /// Mistakes found in the user's keymap file.
    pub problems: Vec<String>,
}

/// What the pages show that lives outside the settings file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageState {
    pub jira: IntegrationsPage,
    pub project: ProjectPage,
    pub agent: AgentPage,
    pub network: NetworkPage,
    pub general: GeneralPage,
    pub keymap: KeymapPage,
}

fn registration_text(registration: &Registration, done: &str) -> String {
    match registration {
        Registration::Pending => "Registering...".into(),
        Registration::Done => done.into(),
        Registration::Skipped => "Not registered: this run uses a temporary state folder.".into(),
        Registration::Failed(error) => format!("Failed: {error}"),
    }
}

fn general_page(s: &Settings, general: &GeneralPage) -> Page {
    Page {
        title: "General",
        items: vec![
            PageItem::Header("Startup"),
            PageItem::Row(SettingRow {
                title: "Start at Login".into(),
                description: "Open Pomelo when you log in to this Mac.".into(),
                control: Control::Toggle {
                    id: CTRL_START_AT_LOGIN,
                    on: general.start_at_login,
                },
                reset: None,
            }),
            PageItem::Header("Updates"),
            PageItem::Row(SettingRow {
                title: "Version".into(),
                description: "The build you are running.".into(),
                control: Control::Value {
                    text: general.version.clone(),
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Check for Updates Automatically".into(),
                description: "On launch, download and install a newer release, then relaunch."
                    .into(),
                control: Control::Toggle {
                    id: CTRL_AUTO_UPDATE,
                    on: s.auto_update,
                },
                reset: reset_if_changed(CTRL_AUTO_UPDATE, s),
            }),
            PageItem::Row(SettingRow {
                title: "Check Now".into(),
                description: if general.updates_apply {
                    "Looks for a newer release right away."
                } else {
                    "Only the installed Pomelo updates itself; this build does not."
                }
                .into(),
                control: Control::Button {
                    id: CTRL_CHECK_UPDATES,
                    label: "Check for Updates",
                    enabled: general.updates_apply,
                },
                reset: None,
            }),
        ],
    }
}

fn editor_page(s: &Settings) -> Page {
    Page {
        title: "Editor",
        items: vec![
            PageItem::Header("Buffer Font"),
            PageItem::Row(SettingRow {
                title: "Font Size".into(),
                description: "Text size of the code editor.".into(),
                control: Control::Stepper {
                    dec: CTRL_BUFFER_FONT_DEC,
                    inc: CTRL_BUFFER_FONT_INC,
                    edit: CTRL_BUFFER_FONT_EDIT,
                    value: format!("{:.0}", s.buffer_font_size),
                },
                reset: reset_if_changed(CTRL_BUFFER_FONT_EDIT, s),
            }),
            PageItem::Header("Behavior"),
            PageItem::Row(SettingRow {
                title: "Soft Wrap".into(),
                description: "How newly opened files wrap long lines.".into(),
                control: Control::Dropdown {
                    id: CTRL_SOFT_WRAP,
                    value: control_value(CTRL_SOFT_WRAP, s),
                },
                reset: reset_if_changed(CTRL_SOFT_WRAP, s),
            }),
            PageItem::Row(SettingRow {
                title: "Diff View".into(),
                description: "Side by side when the pane is wide enough, or one column.".into(),
                control: Control::Dropdown {
                    id: CTRL_DIFF_VIEW,
                    value: control_value(CTRL_DIFF_VIEW, s),
                },
                reset: reset_if_changed(CTRL_DIFF_VIEW, s),
            }),
            PageItem::Row(SettingRow {
                title: "External Editor".into(),
                description:
                    "What \"Open in External Editor\" uses; Auto picks the first installed.".into(),
                control: Control::Dropdown {
                    id: CTRL_EXTERNAL_EDITOR,
                    value: control_value(CTRL_EXTERNAL_EDITOR, s),
                },
                reset: reset_if_changed(CTRL_EXTERNAL_EDITOR, s),
            }),
        ],
    }
}

fn terminal_page(s: &Settings) -> Page {
    Page {
        title: "Terminal",
        items: vec![
            PageItem::Header("Font"),
            PageItem::Row(SettingRow {
                title: "Font Size".into(),
                description: "Text size of terminals and agents.".into(),
                control: Control::Stepper {
                    dec: CTRL_TERM_FONT_DEC,
                    inc: CTRL_TERM_FONT_INC,
                    edit: CTRL_TERM_FONT_EDIT,
                    value: format!("{:.0}", s.terminal_font_size),
                },
                reset: reset_if_changed(CTRL_TERM_FONT_EDIT, s),
            }),
            PageItem::Header("Shell"),
            PageItem::Row(SettingRow {
                title: "Shell".into(),
                description: "What new terminals run; empty runs your login shell.".into(),
                control: Control::TextInput {
                    id: CTRL_TERM_SHELL,
                    value: s.terminal_shell.clone(),
                    placeholder: "login shell",
                    masked: false,
                },
                reset: reset_if_changed(CTRL_TERM_SHELL, s),
            }),
            PageItem::Row(SettingRow {
                title: "Scrollback".into(),
                description: "Lines of history each new terminal keeps.".into(),
                control: Control::Stepper {
                    dec: CTRL_SCROLLBACK_DEC,
                    inc: CTRL_SCROLLBACK_INC,
                    edit: CTRL_SCROLLBACK_EDIT,
                    value: s.terminal_scrollback.to_string(),
                },
                reset: reset_if_changed(CTRL_SCROLLBACK_EDIT, s),
            }),
        ],
    }
}

fn keymap_page(keymap: &KeymapPage) -> Page {
    let mut items = vec![
        PageItem::Header("Bindings"),
        PageItem::Row(SettingRow {
            title: "Edit Keybindings".into(),
            description:
                "Opens keymap.json; later entries override these defaults, null unbinds a key."
                    .into(),
            control: Control::Button {
                id: CTRL_EDIT_KEYMAP,
                label: "Open keymap.json",
                enabled: true,
            },
            reset: None,
        }),
    ];
    for problem in &keymap.problems {
        items.push(PageItem::Row(SettingRow {
            title: "Problem".into(),
            description: problem.clone().into(),
            control: Control::Value {
                text: String::new(),
            },
            reset: None,
        }));
    }
    for (label, name, binding) in &keymap.rows {
        items.push(PageItem::Row(SettingRow {
            title: label.clone().into(),
            description: name.clone().into(),
            control: Control::Value {
                text: if binding.is_empty() {
                    "Unbound".to_string()
                } else {
                    binding.clone()
                },
            },
            reset: None,
        }));
    }
    Page {
        title: "Keymap",
        items,
    }
}

fn agent_page(s: &Settings, agent: &AgentPage) -> Page {
    let can_reinstall = !matches!(agent.mcp, Registration::Pending | Registration::Skipped);
    Page {
        title: "Agent",
        items: vec![
            PageItem::Header("Command"),
            PageItem::Row(SettingRow {
                title: "Agent Command".into(),
                description: "The AI CLI the Agent button opens in a workspace.".into(),
                control: Control::TextInput {
                    id: CTRL_AGENT_COMMAND,
                    value: s.agent_command.clone(),
                    placeholder: "claude",
                    masked: false,
                },
                reset: reset_if_changed(CTRL_AGENT_COMMAND, s),
            }),
            PageItem::Header("Claude Code"),
            PageItem::Row(SettingRow {
                title: "MCP Server".into(),
                description: registration_text(
                    &agent.mcp,
                    "Registered in ~/.claude.json: every Claude session gets the pom tools for its workspace.",
                )
                .into(),
                control: Control::Button {
                    id: CTRL_REINSTALL_AGENTS,
                    label: "Reinstall",
                    enabled: can_reinstall,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Activity Hooks".into(),
                description: registration_text(
                    &agent.hooks,
                    "Installed in ~/.claude/settings.json: workspace dots and notifications follow what Claude is doing.",
                )
                .into(),
                control: Control::Value {
                    text: String::new(),
                },
                reset: None,
            }),
        ],
    }
}

fn notifications_page(s: &Settings) -> Page {
    let mut items = vec![
        PageItem::Header("Delivery"),
        PageItem::Row(SettingRow {
            title: "Notify on Claude Activity".into(),
            description: "The master switch for banners and sounds when a workspace's Claude starts, finishes, needs input or compacts. Needs macOS notification permission.".into(),
            control: Control::Toggle {
                id: CTRL_NOTIFY,
                on: s.notify_claude,
            },
            reset: reset_if_changed(CTRL_NOTIFY, s),
        }),
        PageItem::Row(SettingRow {
            title: "Alert While Viewing".into(),
            description: "Also alert for the workspace on screen in the focused window.".into(),
            control: Control::Toggle {
                id: CTRL_NOTIFY_FOCUSED,
                on: s.notify_when_focused,
            },
            reset: reset_if_changed(CTRL_NOTIFY_FOCUSED, s),
        }),
        PageItem::Row(SettingRow {
            title: "Test Notification".into(),
            description: "Posts a sample banner to check that delivery works.".into(),
            control: Control::Button {
                id: CTRL_TEST_NOTIFICATION,
                label: "Send",
                enabled: true,
            },
            reset: None,
        }),
        PageItem::Header("Alert Sounds"),
    ];
    for (index, (event, title)) in settings::AGENT_EVENTS.iter().enumerate() {
        let id = CTRL_SOUND_BASE + index as u64;
        items.push(PageItem::Row(SettingRow {
            title: (*title).into(),
            description: "A macOS sound played with the notification; picking one plays it.".into(),
            control: Control::Dropdown {
                id,
                value: sound_label(s.sound_for(event)),
            },
            reset: reset_if_changed(id, s),
        }));
    }
    Page {
        title: "Notifications",
        items,
    }
}

fn network_page(network: &NetworkPage) -> Page {
    let status_description = if network.served_elsewhere && !network.proxy_running {
        "Served by `pom proxy` in a terminal; its requests are not listed here."
    } else {
        "Serves every workspace's services behind one port."
    };
    let mut items = vec![
        PageItem::Header("Reverse Proxy"),
        PageItem::Row(SettingRow {
            title: "Status".into(),
            description: status_description.into(),
            control: Control::Status {
                running: network.proxy_running || network.served_elsewhere,
            },
            reset: None,
        }),
        PageItem::Row(SettingRow {
            title: "From the Frontend".into(),
            description: "Point the frontend's backend base URL here: same origin, cookies like production, retargeted when the environment switches.".into(),
            control: Control::Value {
                text: "/_pom_dev/<repo>/<service>".into(),
            },
            reset: None,
        }),
        PageItem::Row(SettingRow {
            title: "Proxy Port".into(),
            description: format!(
                "Open a service directly at <service>.<repo>.<ticket or branch>.localhost:{}. Set POM_WEB_PORT to move it (the proxy takes that port + 2).",
                network.proxy_port
            )
            .into(),
            control: Control::Value {
                text: network.proxy_port.to_string(),
            },
            reset: None,
        }),
        PageItem::Row(SettingRow {
            title: "Bind Address".into(),
            description: "Only this machine can reach the proxy.".into(),
            control: Control::Value {
                text: "127.0.0.1".into(),
            },
            reset: None,
        }),
        PageItem::Header("Webhook Fan-out"),
        PageItem::Row(SettingRow {
            title: "Status".into(),
            description: "Hands each incoming webhook to every workspace running the service.".into(),
            control: Control::Status {
                running: network.webhook_running || network.served_elsewhere,
            },
            reset: None,
        }),
        PageItem::Row(SettingRow {
            title: "Listen Port".into(),
            description: format!(
                "Point an external webhook (Stripe, GitHub...) at localhost:{}/<repo>/<service>.",
                network.webhook_port
            )
            .into(),
            control: Control::Value {
                text: network.webhook_port.to_string(),
            },
            reset: None,
        }),
    ];
    if !network.served_elsewhere && (!network.proxy_running || !network.webhook_running) {
        items.push(PageItem::Row(SettingRow {
            title: "Servers".into(),
            description: "A port was taken, likely by another Pomelo. Free it, then start again."
                .into(),
            control: Control::Button {
                id: CTRL_START_SERVERS,
                label: "Start Servers",
                enabled: true,
            },
            reset: None,
        }));
    }
    items.push(PageItem::Header("Recent Requests"));
    if network.requests.is_empty() {
        items.push(PageItem::Row(SettingRow {
            title: "No Requests Yet".into(),
            description: "Requests the frontend sends through /_pom_dev/ show up here.".into(),
            control: Control::Value {
                text: String::new(),
            },
            reset: None,
        }));
    }
    for request in &network.requests {
        items.push(PageItem::Row(SettingRow {
            title: format!("{} {}", request.method, request.path).into(),
            description: format!(
                "{} - {} - {} - {} ms",
                request.time, request.profile, request.target, request.ms
            )
            .into(),
            control: Control::Value {
                text: request.status.to_string(),
            },
            reset: None,
        }));
    }
    Page {
        title: "Network",
        items,
    }
}

fn integrations_page(s: &Settings, jira: &IntegrationsPage) -> Page {
    let token_row = match jira.token {
        TokenSource::Missing => SettingRow {
            title: "API Token".into(),
            description: "Create one in your Atlassian account under Security > API tokens. Stored encrypted for this project; or set `JIRA_API_TOKEN`.".into(),
            control: Control::TextInput {
                id: CTRL_JIRA_TOKEN,
                value: String::new(),
                placeholder: "xxxxxxxxxxxxxxxxxxxx",
                masked: true,
            },
            reset: None,
        },
        TokenSource::Secret => SettingRow {
            title: "API Token".into(),
            description: "Stored encrypted for this project.".into(),
            control: Control::Configured {
                label: "API Token Configured",
                button: CTRL_JIRA_RESET_TOKEN,
                button_label: "Reset Token",
                enabled: true,
            },
            reset: None,
        },
        TokenSource::Environment => SettingRow {
            title: "API Token".into(),
            description: "Read from `JIRA_API_TOKEN`; unset it to use a stored token instead.".into(),
            control: Control::Configured {
                label: "API Token Set in Environment Variable",
                button: CTRL_JIRA_RESET_TOKEN,
                button_label: "Reset Token",
                enabled: false,
            },
            reset: None,
        },
    };
    let (connection, testing) = match &jira.status {
        ConnectionStatus::Untested => ("Checks the site, email and token.".into(), false),
        ConnectionStatus::Testing => ("Connecting...".into(), true),
        ConnectionStatus::SignedIn(who) => (format!("Signed in as {who}.").into(), false),
        ConnectionStatus::Failed(error) => (format!("Failed: {error}").into(), false),
    };
    let ready = !jira.site.trim().is_empty()
        && !jira.email.trim().is_empty()
        && jira.token != TokenSource::Missing;
    Page {
        title: "Integrations",
        items: vec![
            PageItem::Header("Jira"),
            PageItem::Row(SettingRow {
                title: "Site URL".into(),
                description: "Your Jira Cloud address.".into(),
                control: Control::TextInput {
                    id: CTRL_JIRA_SITE,
                    value: jira.site.clone(),
                    placeholder: "https://acme.atlassian.net",
                    masked: false,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Account Email".into(),
                description: "The email you sign in to Jira with.".into(),
                control: Control::TextInput {
                    id: CTRL_JIRA_EMAIL,
                    value: jira.email.clone(),
                    placeholder: "you@example.com",
                    masked: false,
                },
                reset: None,
            }),
            PageItem::Row(token_row),
            PageItem::Row(SettingRow {
                title: "Connection".into(),
                description: connection,
                control: Control::Button {
                    id: CTRL_JIRA_TEST,
                    label: "Test Connection",
                    enabled: ready && !testing,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Only Show My Tickets".into(),
                description: "The new-workspace ticket picker lists only tickets assigned to you."
                    .into(),
                control: Control::Toggle {
                    id: CTRL_JIRA_ONLY_MINE,
                    on: s.jira_only_mine,
                },
                reset: reset_if_changed(CTRL_JIRA_ONLY_MINE, s),
            }),
            PageItem::Header("Main Workspace"),
            PageItem::Row(SettingRow {
                title: "Keep Main Fresh".into(),
                description: "Pulls every repo of main from origin and runs its migrations on a schedule, so new workspaces start from current code and data. Repos with uncommitted changes are skipped.".into(),
                control: Control::Toggle {
                    id: CTRL_REFRESH_MAIN,
                    on: jira.keep_main_fresh,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Refresh Every".into(),
                description: "Minutes between refreshes, on the clock: every 30 runs at :00 and :30.".into(),
                control: Control::Stepper {
                    dec: CTRL_REFRESH_DEC,
                    inc: CTRL_REFRESH_INC,
                    edit: CTRL_REFRESH_EDIT,
                    value: jira.refresh_minutes.to_string(),
                },
                reset: None,
            }),
        ],
    }
}

fn project_page(project: &ProjectPage) -> Page {
    let mut items = vec![
        PageItem::Header("Repositories"),
        PageItem::Row(SettingRow {
            title: "Add Repository".into(),
            description: "Clone a git URL or folder into main, detect its services and check it out in the workspaces you pick.".into(),
            control: Control::Button {
                id: CTRL_ADD_REPO,
                label: "Add...",
                enabled: true,
            },
            reset: None,
        }),
    ];
    for (index, repo) in project
        .repos
        .iter()
        .enumerate()
        .take(CTRL_REPO_LIMIT as usize)
    {
        let title = if repo.alias.is_empty() || repo.alias == repo.name {
            repo.name.clone()
        } else {
            format!("{} (alias {})", repo.name, repo.alias)
        };
        let services = match repo.services {
            1 => "1 service".to_string(),
            count => format!("{count} services"),
        };
        let presence = if !repo.cloned {
            "not cloned into main".to_string()
        } else if repo.workspaces == 0 {
            "in main".to_string()
        } else {
            format!(
                "in {} of {} workspaces besides main",
                repo.present, repo.workspaces
            )
        };
        items.push(PageItem::Row(SettingRow {
            title: title.into(),
            description: format!("{services} - {presence}").into(),
            control: Control::Buttons(vec![
                (CTRL_REPO_RENAME_BASE + index as u64, "Rename Alias..."),
                (CTRL_REPO_REMOVE_BASE + index as u64, "Remove..."),
            ]),
            reset: None,
        }));
    }
    Page {
        title: "Project",
        items: items.into_iter().chain(project_config_items()).collect(),
    }
}

fn project_config_items() -> Vec<PageItem> {
    vec![
            PageItem::Header("Config"),
            PageItem::Row(SettingRow {
                title: "Config Files".into(),
                description: "pom.yml and its pom.d fragments. Each save is checked first and reloads at once, even from main.".into(),
                control: Control::Button {
                    id: CTRL_EDIT_PROJECT_CONFIG,
                    label: "Edit...",
                    enabled: true,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Apply to All Workspaces".into(),
                description: "Check out every repo the config has in the workspaces that lack it (clone into main first).".into(),
                control: Control::Button {
                    id: CTRL_APPLY_CONFIG,
                    label: "Apply",
                    enabled: true,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Split into pom.d".into(),
                description: "One file per repo plus shared services and environments; the old file is kept as a backup.".into(),
                control: Control::Button {
                    id: CTRL_SPLIT_CONFIG,
                    label: "Split",
                    enabled: true,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Normalize".into(),
                description: "Drop removed keys, rewrite old colon tokens to dot notation, then split.".into(),
                control: Control::Button {
                    id: CTRL_NORMALIZE_CONFIG,
                    label: "Normalize",
                    enabled: true,
                },
                reset: None,
            }),
            PageItem::Header("Config Bundle"),
            PageItem::Row(SettingRow {
                title: "Export".into(),
                description: "Save this project's merged config as YAML, or with its secrets sealed under a password, to hand to a teammate.".into(),
                control: Control::Button {
                    id: CTRL_EXPORT_CONFIG,
                    label: "Export...",
                    enabled: true,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Import".into(),
                description: "Replace this project's config with a YAML file or bundle, or let Claude merge it into yours.".into(),
                control: Control::Button {
                    id: CTRL_IMPORT_CONFIG,
                    label: "Import...",
                    enabled: true,
                },
                reset: None,
            }),
    ]
}

fn row_matches(row: &SettingRow, q: &str) -> bool {
    q.is_empty()
        || row.title.to_lowercase().contains(q)
        || row.description.to_lowercase().contains(q)
}

/// Render a page from its data, filtering rows by `query` (title/description substring) and dropping section
/// headers whose section has no visible row, like the reference's settings search.
fn render_page(page: &Page, query: &str, editing: Option<(u64, &str)>, w: f32) -> Node {
    // The title+description column is capped (like the reference's max_w_2_3) so descriptions wrap instead of
    // running under the control. Widths are in design px (the element builders re-apply the UI scale).
    let scale = ui::ui_text_scale();
    let content_design_w = (w / scale - 240.0 - 64.0).max(200.0);
    let left_col_w = content_design_w * 2.0 / 3.0;
    let q = query.to_lowercase();
    let n = page.items.len();
    let mut visible = vec![false; n];
    for (i, item) in page.items.iter().enumerate() {
        if let PageItem::Row(row) = item {
            visible[i] = row_matches(row, &q);
        }
    }
    for i in 0..n {
        if let PageItem::Header(_) = page.items[i] {
            let mut any = false;
            for (j, later) in page.items.iter().enumerate().skip(i + 1) {
                match later {
                    PageItem::Header(_) => break,
                    PageItem::Row(_) => {
                        if visible[j] {
                            any = true;
                            break;
                        }
                    }
                }
            }
            visible[i] = any;
        }
    }

    let mut col = div()
        .col()
        .child(
            label(page.title)
                .label_size(LabelSize::Large)
                .color(title_c()),
        )
        .child(div().h_px(16.0));

    if !q.is_empty() && visible.iter().all(|v| !v) {
        return col
            .child(label("No matching settings.").color(dim_c()))
            .into();
    }

    let mut first_section = true;
    let mut section_idx = 0usize;
    for (i, item) in page.items.iter().enumerate() {
        let is_header = matches!(item, PageItem::Header(_));
        if !visible[i] {
            if is_header {
                section_idx += 1;
            }
            continue;
        }
        match item {
            PageItem::Header(name) => {
                if !first_section {
                    col = col.child(div().h_px(20.0));
                }
                first_section = false;
                col = col.child(section_header(
                    name,
                    SECTION_ANCHOR_BASE + section_idx as u64,
                ));
                section_idx += 1;
            }
            PageItem::Row(row) => {
                let control = match &row.control {
                    Control::Dropdown { id, value } => dropdown(value, *id),
                    Control::Stepper {
                        dec,
                        inc,
                        edit,
                        value,
                    } => {
                        let buf = editing.and_then(|(id, b)| (id == *edit).then_some(b));
                        stepper(value, *dec, *inc, *edit, buf)
                    }
                    Control::EditInJson { id } => edit_in_json_button(*id),
                    Control::Toggle { id, on } => toggle_switch(*on, *id),
                    Control::TextInput {
                        id,
                        value,
                        placeholder,
                        masked,
                    } => {
                        let buf = editing.and_then(|(edit, b)| (edit == *id).then_some(b));
                        text_input(value, placeholder, *masked, *id, buf)
                    }
                    Control::Configured {
                        label,
                        button,
                        button_label,
                        enabled,
                    } => configured_card(label, *button, button_label, *enabled),
                    Control::Button { id, label, enabled } => action_button(*id, label, *enabled),
                    Control::Buttons(buttons) => buttons
                        .iter()
                        .fold(div().row().gap(6.0).items_center(), |row, (id, label)| {
                            row.child(action_button(*id, label, true))
                        })
                        .into(),
                    Control::Status { running } => status_chip(*running),
                    Control::Value { text } => value_text(text),
                };
                col = col.child(row_frame(
                    &row.title,
                    &row.description,
                    control,
                    row.reset,
                    left_col_w,
                ));
                col = col.child(divider());
            }
        }
    }
    col.into()
}

fn section_header(name: &str, anchor: u64) -> Node {
    div()
        .col()
        .gap(8.0)
        .on_click(anchor) // geometry marker only: lets the app read this section's y to scroll the navbar to it
        .child(
            label(name)
                .label_size(LabelSize::Small)
                .color(dim_c())
                .mono(),
        )
        .child(divider_faded())
        .into()
}

fn divider() -> Node {
    ui::divider().into()
}

fn row_frame(title: &str, desc: &str, control: Node, reset: Option<u64>, left_col_w: f32) -> Node {
    let mut title_row = div()
        .row()
        .items_center()
        .gap(6.0)
        .child(label(title).label_size(LabelSize::Default).color(title_c()));
    if let Some(primary) = reset {
        title_row = title_row.child(reset_button(primary + RESET_OFFSET));
    }
    let left = div()
        .col()
        .w_px(left_col_w)
        .gap(5.0)
        .child(title_row)
        .child(description_node(desc, left_col_w));
    div()
        .row()
        .py(14.0)
        .items_center()
        .justify_between()
        .child(left)
        .child(control)
        .into()
}

fn reset_button(id: u64) -> Node {
    div()
        .w_px(18.0)
        .h_px(18.0)
        .rounded(4.0)
        .items_center()
        .justify_center()
        .on_click(id)
        .child(ui::icon(ui::IconKind::Undo).size(12.0).color(dim_c()))
        .into()
}

fn description_node(desc: &str, wrap_w: f32) -> Node {
    if !desc.contains('`') {
        return label(desc)
            .label_size(LabelSize::Small)
            .color(dim_c())
            .wrap(wrap_w)
            .into();
    }
    let mut row = div().row().items_center();
    for (i, part) in desc.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        if i % 2 == 1 {
            row = row.child(
                div().px(4.0).rounded(3.0).bg(chip_c()).child(
                    label(part)
                        .label_size(LabelSize::Small)
                        .mono()
                        .color(title_c()),
                ),
            );
        } else {
            row = row.child(label(part).label_size(LabelSize::Small).color(dim_c()));
        }
    }
    row.into()
}

/// A subtle-bordered dropdown control: value + up/down chevron; clicking cycles the value (app-side).
fn dropdown(value: &str, id: u64) -> Node {
    div()
        .h_px(28.0)
        .px(10.0)
        .gap(14.0)
        .items_center()
        .rounded(ROUND)
        .bg(chip_c())
        .border(1.0, border_c())
        .on_click(id)
        .child(label(value).size(12.0).color(title_c()))
        .child(chevron_up_down().size(11.0).color(dim_c()))
        .into()
}

fn toggle_switch(on: bool, id: u64) -> Node {
    let track = if on { theme().icon_accent } else { chip_c() };
    let knob = div().w_px(14.0).h_px(14.0).rounded(7.0).bg(bg_c());
    let mut row = div()
        .row()
        .w_px(36.0)
        .h_px(20.0)
        .px(3.0)
        .items_center()
        .rounded(10.0)
        .bg(track)
        .border(1.0, border_c())
        .on_click(id);
    if on {
        row = row.child(div().flex(1.0)).child(knob);
    } else {
        row = row.child(knob).child(div().flex(1.0));
    }
    row.into()
}

/// A stepper control: one bordered rounded box `[- | value | +]`, the end segments clickable, with thin
/// separators between segments. Clicking the value box begins inline editing (`edit` = the typed buffer),
/// which shows a caret and focuses the box border, mirroring the reference's editable NumberField.
fn stepper(value: &str, dec: u64, inc: u64, edit_id: u64, edit: Option<&str>) -> Node {
    let editing = edit.is_some();
    let shown = edit.unwrap_or(value);
    let mut value_box = div()
        .row()
        .h_px(28.0)
        .px(14.0)
        .gap(1.0)
        .items_center()
        .on_click(edit_id)
        .child(label(shown).size(12.0).color(title_c()));
    if editing {
        // Reserve the caret slot while editing so the number doesn't shift as the caret blinks.
        value_box = value_box.child(caret(ui::caret_phase()));
    }
    div()
        .row()
        .h_px(28.0)
        .items_center()
        .rounded(ROUND)
        .bg(chip_c())
        .border(
            1.0,
            if editing {
                theme().border_focused
            } else {
                border_c()
            },
        )
        .child(step_seg("-", dec))
        .child(step_sep())
        .child(value_box)
        .child(step_sep())
        .child(step_seg("+", inc))
        .into()
}

/// The reference's settings input field: a 32px box at least 256px wide on the editor background, its border
/// focused while editing. Clicking it starts editing; Enter saves and Escape cancels.
fn text_input(value: &str, placeholder: &str, masked: bool, id: u64, edit: Option<&str>) -> Node {
    let editing = edit.is_some();
    let shown = edit.unwrap_or(value);
    let text = if masked {
        "*".repeat(shown.chars().count())
    } else {
        shown.to_string()
    };
    let mut row = div().row().items_center().gap(1.0).flex(1.0);
    row = if text.is_empty() && !editing {
        row.child(
            label(placeholder)
                .size(12.0)
                .color(theme().text_placeholder),
        )
    } else {
        row.child(label(text).size(12.0).color(title_c()).truncate())
    };
    if editing {
        row = row.child(caret(ui::caret_phase()));
    }
    div()
        .row()
        .w_px(256.0)
        .h_px(32.0)
        .px(8.0)
        .items_center()
        .rounded(ROUND)
        .bg(theme().editor_background)
        .border(
            1.0,
            if editing {
                theme().border_focused
            } else {
                border_c()
            },
        )
        .on_click(id)
        .child(row)
        .into()
}

/// A stored secret: a check, what is set, and the button that clears it (the reference's configured-key card).
fn configured_card(text: &str, button: u64, button_label: &str, enabled: bool) -> Node {
    let mut clear = div()
        .row()
        .h_px(22.0)
        .px(6.0)
        .items_center()
        .rounded(4.0)
        .border(1.0, theme().border_variant)
        .child(label(button_label).size(12.0).color(if enabled {
            title_c()
        } else {
            theme().text_disabled
        }));
    if enabled {
        clear = clear.on_click(button);
    }
    div()
        .row()
        .h_px(32.0)
        .px(8.0)
        .gap(8.0)
        .items_center()
        .rounded(ROUND)
        .bg(chip_c())
        .border(1.0, border_c())
        .child(
            ui::icon(ui::IconKind::Check)
                .size(12.0)
                .color(theme().success),
        )
        .child(label(text).size(12.0).color(title_c()))
        .child(clear)
        .into()
}

fn action_button(id: u64, text: &str, enabled: bool) -> Node {
    if enabled {
        ui::button_sized(id, text, ui::ButtonStyle::Outlined, ui::ButtonSize::Medium).into()
    } else {
        div()
            .row()
            .h_px(28.0)
            .px(8.0)
            .items_center()
            .rounded(4.0)
            .border(1.0, theme().border_variant)
            .child(label(text).size(14.0).color(theme().text_disabled))
            .into()
    }
}

fn status_chip(running: bool) -> Node {
    let (color, text) = if running {
        (theme().success, "Running")
    } else {
        (theme().error, "Stopped")
    };
    div()
        .row()
        .items_center()
        .gap(6.0)
        .child(div().w_px(7.0).h_px(7.0).rounded(3.5).bg(color))
        .child(label(text).size(12.0).color(color))
        .into()
}

fn value_text(text: &str) -> Node {
    label(text).size(12.0).mono().color(title_c()).into()
}

/// The reference renders `.unimplemented()` fields as an Outlined, Medium "Edit in settings.json" button.
fn edit_in_json_button(id: u64) -> Node {
    ui::button_sized(
        id,
        "Edit in settings.json",
        ui::ButtonStyle::Outlined,
        ui::ButtonSize::Medium,
    )
    .into()
}

/// A text caret: a fixed-width slot (so surrounding text never shifts) whose bar shows only on the blink's
/// visible phase.
fn caret(visible: bool) -> Node {
    div()
        .w_px(1.5)
        .h_px(15.0)
        .bg(if visible {
            theme().text
        } else {
            Rgba::TRANSPARENT
        })
        .into()
}

fn step_sep() -> Node {
    div().w_px(1.0).h_px(28.0).bg(border_c()).into()
}

fn step_seg(sym: &str, id: u64) -> Node {
    div()
        .h_px(28.0)
        .px(11.0)
        .items_center()
        .on_click(id)
        .child(label(sym).size(15.0).color(dim_c()))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navbar_sections_match_page_section_headers() {
        let headers: Vec<&str> = appearance_page(&Settings::default())
            .items
            .iter()
            .filter_map(|it| match it {
                PageItem::Header(name) => Some(*name),
                PageItem::Row(_) => None,
            })
            .collect();
        assert_eq!(headers, APPEARANCE_SECTIONS.to_vec());
    }

    #[test]
    fn search_filters_page_to_matching_section() {
        let expanded = [false; CATEGORY_COUNT];
        let p = panel(
            APPEARANCE,
            None,
            &expanded,
            &Settings::default(),
            &PageState::default(),
            1200.0,
            800.0,
            None,
            "font",
            true,
        );
        // The UI Font section matches; the Theme section is dropped entirely.
        assert!(p.texts.iter().any(|x| x.text == "Font Family"));
        assert!(p.texts.iter().any(|x| x.text == "UI Font"));
        assert!(!p.texts.iter().any(|x| x.text == "Theme Mode"));
    }

    #[test]
    fn renders_appearance_section() {
        let expanded = [false; CATEGORY_COUNT];
        let p = panel(
            APPEARANCE,
            None,
            &expanded,
            &Settings::default(),
            &PageState::default(),
            1200.0,
            800.0,
            None,
            "",
            false,
        );
        assert!(!p.rects.is_empty());
        assert!(p.texts.iter().any(|x| x.text == "Appearance"));
        assert!(p.texts.iter().any(|x| x.text == "One Dark"));
        assert!(p.texts.iter().any(|x| x.text == "Theme")); // first section header
        assert!(p.texts.iter().any(|x| x.text == "UI Font")); // second section header
                                                              // Categories + controls are all clickable.
        assert!(p.hits.len() >= CATEGORIES.len());
        assert!(p.hits.iter().any(|(_, id)| *id == CTRL_THEME));
        assert!(p.hits.iter().any(|(_, id)| *id == CTRL_FONT_SIZE_INC));
    }

    #[test]
    fn controls_mutate_settings() {
        let mut s = Settings::default();
        // Dropdowns select a value from a popover (index into control_items).
        assert!(apply_choice(CTRL_THEME, 1, &[], &mut s));
        assert_eq!(s.theme, "One Light");
        let fonts = vec!["Helvetica".to_string(), "Menlo".to_string()];
        assert!(apply_choice(CTRL_FONT_FAMILY, 1, &fonts, &mut s));
        assert_eq!(s.ui_font, "Menlo");
        // Steppers act immediately.
        let before = s.ui_font_size;
        assert!(handle_control(CTRL_FONT_SIZE_INC, &mut s));
        assert_eq!(s.ui_font_size, before + 1.0);
    }

    #[test]
    fn popover_lists_options_with_click_ids() {
        let items = control_items(CTRL_THEME, &[]);
        let p = popover(
            Rect::new(800.0, 300.0, 120.0, 28.0, Rgba::TRANSPARENT),
            "One Dark",
            &items,
            0,
            "",
            None,
            900.0,
            800.0,
        );
        assert!(p.texts.iter().any(|t| t.text == "Ayu Mirage"));
        assert!(p.hits.iter().any(|(_, id)| *id == POPOVER_BASE));
    }

    #[test]
    fn popover_query_filters_and_keeps_original_click_ids() {
        let items: Vec<String> = vec!["Menlo".into(), "Monaco".into(), "Arial".into()];
        let anchor = Rect::new(800.0, 300.0, 120.0, 28.0, Rgba::TRANSPARENT);
        let p = popover(anchor, "", &items, 0, "mo", None, 900.0, 800.0);
        // "Monaco" (index 1) matches; its click id must be the original index, not the filtered position.
        assert!(p.texts.iter().any(|t| t.text == "Monaco"));
        assert!(!p.texts.iter().any(|t| t.text == "Arial"));
        assert!(p.hits.iter().any(|(_, id)| *id == POPOVER_BASE + 1));
    }

    #[test]
    fn popover_scroll_windows_items_and_keeps_absolute_click_ids() {
        let items: Vec<String> = (0..40).map(|i| format!("Font {i}")).collect();
        let anchor = Rect::new(800.0, 300.0, 120.0, 28.0, Rgba::TRANSPARENT);
        let scroll = 20;
        let p = popover(anchor, "", &items, scroll, "", None, 900.0, 800.0);
        // The first visible row must map to the absolute item index, not a window-local 0.
        assert!(p
            .hits
            .iter()
            .any(|(_, id)| *id == POPOVER_BASE + scroll as u64));
        assert!(p.texts.iter().any(|t| t.text == format!("Font {scroll}")));
        // Scrolled past the first items, they are no longer drawn.
        assert!(!p.texts.iter().any(|t| t.text == "Font 0"));
    }

    #[test]
    fn expanding_appearance_shows_subsections_and_chevrons() {
        let mut expanded = [false; CATEGORY_COUNT];
        expanded[APPEARANCE] = true;
        let p = panel(
            APPEARANCE,
            None,
            &expanded,
            &Settings::default(),
            &PageState::default(),
            1200.0,
            800.0,
            None,
            "",
            false,
        );
        // Sub-section label appears in the navbar and a chevron icon is drawn.
        assert!(p.texts.iter().any(|x| x.text == "Theme"));
        assert!(!p.icons.is_empty());
    }

    #[test]
    fn every_category_has_its_page() {
        let expanded = [false; CATEGORY_COUNT];
        for (category, (name, _)) in CATEGORIES.iter().enumerate() {
            let p = panel(
                category,
                None,
                &expanded,
                &Settings::default(),
                &PageState::default(),
                1200.0,
                800.0,
                None,
                "",
                false,
            );
            assert!(
                !p.texts.iter().any(|x| x.text == "No settings here yet."),
                "{name}"
            );
        }
    }

    fn texts_of(category: usize, state: &PageState, settings: &Settings) -> Vec<ui::Text> {
        panel(
            category,
            None,
            &[false; CATEGORY_COUNT],
            settings,
            state,
            1400.0,
            2400.0,
            None,
            "",
            false,
        )
        .texts
    }

    #[test]
    fn notifications_page_lists_delivery_and_a_sound_per_event() {
        let texts = texts_of(NOTIFICATIONS, &PageState::default(), &Settings::default());
        for expected in [
            "Notify on Claude Activity",
            "Alert While Viewing",
            "Glass",
            "Ping",
        ] {
            assert!(texts.iter().any(|t| t.text == expected), "{expected}");
        }
        let headers: Vec<_> = texts
            .iter()
            .filter(|t| t.text == "Delivery" || t.text == "Alert Sounds")
            .filter(|t| t.mono)
            .collect();
        assert_eq!(headers.len(), 2, "section headers are mono");
    }

    #[test]
    fn sound_dropdowns_offer_none_and_system_sounds_and_store_the_pick() {
        let mut s = Settings::default();
        let finished = CTRL_SOUND_BASE + 1;
        assert!(is_dropdown(finished));
        assert_eq!(sound_event(finished), Some("finished"));
        let items = control_items(finished, &[]);
        assert_eq!(items[0], "None");
        assert_eq!(control_value(finished, &s), "Glass");
        let hero = items.iter().position(|item| item == "Hero").unwrap_or(0);
        assert!(apply_choice(finished, hero, &[], &mut s));
        assert_eq!(s.sound_finished, "Hero");
        assert!(!is_default(finished, &s));
        assert!(apply_choice(finished, 0, &[], &mut s));
        assert_eq!(s.sound_finished, "");
        assert!(reset_to_default(finished, &mut s));
        assert_eq!(s.sound_finished, "Glass");
        assert!(handle_control(CTRL_NOTIFY, &mut s));
        assert!(!s.notify_claude);
    }

    #[test]
    fn network_page_shows_status_ports_and_recent_requests() {
        let state = PageState {
            network: NetworkPage {
                proxy_running: true,
                webhook_running: false,
                proxy_port: 8767,
                webhook_port: 8766,
                requests: vec![RequestRow {
                    time: "10:00:00".into(),
                    method: "GET".into(),
                    path: "/_pom_dev/api/server/v1".into(),
                    profile: "local".into(),
                    target: "127.0.0.1:4000".into(),
                    status: 200,
                    ms: 7,
                }],
                ..NetworkPage::default()
            },
            ..PageState::default()
        };
        let p = panel(
            NETWORK,
            None,
            &[false; CATEGORY_COUNT],
            &Settings::default(),
            &state,
            1400.0,
            2400.0,
            None,
            "",
            false,
        );
        for expected in [
            "Running",
            "Stopped",
            "8767",
            "8766",
            "GET /_pom_dev/api/server/v1",
            "200",
        ] {
            assert!(p.texts.iter().any(|t| t.text == expected), "{expected}");
        }
        assert!(p.hits.iter().any(|(_, id)| *id == CTRL_START_SERVERS));
    }

    #[test]
    fn a_terminal_proxy_counts_as_running_and_needs_no_restart_button() {
        let state = PageState {
            network: NetworkPage {
                served_elsewhere: true,
                proxy_port: 8767,
                webhook_port: 8766,
                ..NetworkPage::default()
            },
            ..PageState::default()
        };
        let p = panel(
            NETWORK,
            None,
            &[false; CATEGORY_COUNT],
            &Settings::default(),
            &state,
            1400.0,
            2400.0,
            None,
            "",
            false,
        );
        assert!(!p.texts.iter().any(|t| t.text == "Stopped"));
        assert!(!p.hits.iter().any(|(_, id)| *id == CTRL_START_SERVERS));
    }

    #[test]
    fn agent_page_edits_the_command_and_reinstalls_once_registration_ran() {
        let pending = panel(
            AGENT,
            None,
            &[false; CATEGORY_COUNT],
            &Settings::default(),
            &PageState::default(),
            1400.0,
            2400.0,
            None,
            "",
            false,
        );
        assert!(pending.hits.iter().any(|(_, id)| *id == CTRL_AGENT_COMMAND));
        assert!(!pending
            .hits
            .iter()
            .any(|(_, id)| *id == CTRL_REINSTALL_AGENTS));
        let state = PageState {
            agent: AgentPage {
                mcp: Registration::Done,
                hooks: Registration::Failed("read-only".into()),
            },
            ..PageState::default()
        };
        let texts = texts_of(AGENT, &state, &Settings::default());
        assert!(texts.iter().any(|t| t.text.contains("Failed: read-only")));
    }

    #[test]
    fn general_editor_terminal_and_keymap_pages_show_their_settings() {
        let settings = Settings::default();
        let state = PageState {
            general: GeneralPage {
                start_at_login: true,
                version: "0.9.0".into(),
                updates_apply: false,
            },
            keymap: KeymapPage {
                rows: vec![(
                    "Git".into(),
                    "git_panel::ToggleFocus".into(),
                    "ctrl-shift-g".into(),
                )],
                problems: vec!["keymap.json: unknown action \"x\"".into()],
            },
            ..PageState::default()
        };
        let general = texts_of(GENERAL, &state, &settings);
        for expected in ["Start at Login", "0.9.0", "Check for Updates Automatically"] {
            assert!(general.iter().any(|t| t.text == expected), "{expected}");
        }
        let editor = texts_of(EDITOR, &state, &settings);
        for expected in ["15", "None", "Split", "Auto"] {
            assert!(editor.iter().any(|t| t.text == expected), "{expected}");
        }
        let terminal = texts_of(TERMINAL, &state, &settings);
        assert!(terminal.iter().any(|t| t.text == "10000"));
        let keymap = texts_of(KEYMAP, &state, &settings);
        assert!(keymap.iter().any(|t| t.text == "ctrl-shift-g"));
        assert!(keymap.iter().any(|t| t.text.contains("unknown action")));
    }

    #[test]
    fn editor_and_terminal_controls_change_their_settings_within_bounds() {
        let mut s = Settings::default();
        assert!(handle_control(CTRL_BUFFER_FONT_INC, &mut s));
        assert_eq!(s.buffer_font_size, 16.0);
        assert!(!is_default(CTRL_BUFFER_FONT_EDIT, &s));
        assert!(reset_to_default(CTRL_BUFFER_FONT_EDIT, &mut s));
        assert_eq!(s.buffer_font_size, 15.0);
        assert!(handle_control(CTRL_TERM_FONT_DEC, &mut s));
        assert_eq!(s.terminal_font_size, 14.0);
        s.terminal_scrollback = SCROLLBACK_MAX;
        assert!(
            !handle_control(CTRL_SCROLLBACK_INC, &mut s),
            "held at the maximum"
        );
        assert!(handle_control(CTRL_SCROLLBACK_DEC, &mut s));
        assert_eq!(s.terminal_scrollback, SCROLLBACK_MAX - 1_000);

        assert!(apply_choice(CTRL_SOFT_WRAP, 1, &[], &mut s));
        assert!(s.soft_wrap);
        assert!(apply_choice(CTRL_DIFF_VIEW, 1, &[], &mut s));
        assert!(!s.split_diff);
        let items = control_items(CTRL_EXTERNAL_EDITOR, &[]);
        let zed_like = items.iter().position(|item| item == "Cursor").unwrap_or(0);
        assert!(apply_choice(CTRL_EXTERNAL_EDITOR, zed_like, &[], &mut s));
        assert_eq!(s.external_editor, "Cursor");
        assert!(apply_choice(CTRL_EXTERNAL_EDITOR, 0, &[], &mut s));
        assert_eq!(control_value(CTRL_EXTERNAL_EDITOR, &s), "Auto");
        assert!(handle_control(CTRL_AUTO_UPDATE, &mut s));
        assert!(!s.auto_update);
    }
}
