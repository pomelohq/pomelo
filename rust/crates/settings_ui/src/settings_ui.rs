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
const WINDOW_LAYOUT_SECTIONS: [&str; 2] = ["Status Bar", "Title Bar"];
const APPEARANCE_SECTIONS: [&str; 6] = [
    "Theme",
    "Buffer Font",
    "UI Font",
    "Agent Panel Font",
    "Terminal Font",
    "Cursor",
];

const INTEGRATIONS_SECTIONS: [&str; 2] = ["Jira", "Main Workspace"];
const GENERAL_SECTIONS: [&str; 2] = ["Startup", "Updates"];
const EDITOR_SECTIONS: [&str; 1] = ["Behavior"];
const LANGUAGES_TOOLS_SECTIONS: [&str; 6] = [
    "LSP",
    "LSP Completions",
    "File Types",
    "Diagnostics",
    "Inline Diagnostics",
    "Languages",
];
const TERMINAL_SECTIONS: [&str; 1] = ["Shell"];
const KEYMAP_SECTIONS: [&str; 1] = ["Bindings"];
const AGENT_SECTIONS: [&str; 2] = ["Command", "Claude Code"];
const NOTIFICATIONS_SECTIONS: [&str; 2] = ["Delivery", "Alert Sounds"];
const DEV_SERVICES_SECTIONS: [&str; 4] = [
    "Reverse Proxy",
    "Webhook Fan-out",
    "Shared node_modules",
    "Servers",
];
const PROJECT_SECTIONS: [&str; 3] = ["Repositories", "Config", "Config Bundle"];

const CATEGORIES: [(&str, &[&str]); 12] = [
    ("General", &GENERAL_SECTIONS),
    ("Appearance", &APPEARANCE_SECTIONS),
    ("Window & Layout", &WINDOW_LAYOUT_SECTIONS),
    ("Editor", &EDITOR_SECTIONS),
    ("Languages & Tools", &LANGUAGES_TOOLS_SECTIONS),
    ("Terminal", &TERMINAL_SECTIONS),
    ("Keymap", &KEYMAP_SECTIONS),
    ("Agent", &AGENT_SECTIONS),
    ("Notifications", &NOTIFICATIONS_SECTIONS),
    ("Dev Services", &DEV_SERVICES_SECTIONS),
    ("Integrations", &INTEGRATIONS_SECTIONS),
    ("Project", &PROJECT_SECTIONS),
];

pub const GENERAL: usize = 0;
pub const WINDOW_LAYOUT: usize = 2;
pub const EDITOR: usize = 3;
pub const LANGUAGES_TOOLS: usize = 4;
pub const TERMINAL: usize = 5;
pub const KEYMAP: usize = 6;
pub const AGENT: usize = 7;
pub const NOTIFICATIONS: usize = 8;
pub const DEV_SERVICES: usize = 9;
pub const INTEGRATIONS: usize = 10;
pub const PROJECT: usize = 11;

/// Index of the Appearance category (the only page with real content for now).
pub const APPEARANCE: usize = 1;
/// Number of categories, so the app can size its expanded-state vector.
pub const CATEGORY_COUNT: usize = CATEGORIES.len();

// Control click ids (>= 100 so they never collide with category ids). The app routes these to
// `handle_control`, which mutates the setting like the reference's enum-variant/stepper write.
pub const CTRL_THEME: u64 = 101;
pub const CTRL_THEME_SELECTION: u64 = 105;
pub const CTRL_THEME_LIGHT: u64 = 106;
pub const CTRL_THEME_DARK: u64 = 107;
pub const CTRL_OPEN_THEMES: u64 = 109;
pub const CTRL_FONT_FAMILY: u64 = 102;
/// The sidebar search box; clicking it focuses text entry that filters the page.
pub const CTRL_SEARCH: u64 = 103;
/// The clear (x) button in the search box; clicking it empties the query.
pub const CTRL_SEARCH_CLEAR: u64 = 104;
pub const NAV_JUMP_BASE: u64 = 400;
pub const NAV_JUMP_STRIDE: u64 = 10;
pub const NAV_JUMP_END: u64 = NAV_JUMP_BASE + CATEGORY_COUNT as u64 * NAV_JUMP_STRIDE;
/// A category's disclosure toggle (the chevron): clicking id `NAV_TOGGLE_BASE + category_index` expands or
/// collapses it. Only the chevron toggles; clicking the row selects the category (reference behavior).
pub const NAV_TOGGLE_BASE: u64 = 2_000;
const _: () = assert!(
    NAV_JUMP_END <= NAV_TOGGLE_BASE && NAV_TOGGLE_BASE + CATEGORY_COUNT as u64 <= POPOVER_BASE
);
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
pub const CTRL_SHOW_AGENT: u64 = 214;
pub const CTRL_SHOW_TERMINAL: u64 = 215;
/// One toggle per function button, `FUNCTION_BUTTONS` long.
pub const CTRL_SHOW_FUNCTION_BASE: u64 = 340;
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
pub const CTRL_GROUP_WORKSPACES: u64 = 242;
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
pub const CTRL_PROXY_ENABLED: u64 = 310;
pub const CTRL_PROXY_PORT_DEC: u64 = 311;
pub const CTRL_PROXY_PORT_INC: u64 = 312;
pub const CTRL_PROXY_PORT_EDIT: u64 = 313;
pub const CTRL_WEBHOOK_ENABLED: u64 = 314;
pub const CTRL_WEBHOOK_PORT_DEC: u64 = 315;
pub const CTRL_WEBHOOK_PORT_INC: u64 = 316;
pub const CTRL_WEBHOOK_PORT_EDIT: u64 = 317;
pub const CTRL_OPEN_REQUESTS: u64 = 318;
pub const CTRL_MODULES_ENABLED: u64 = 320;
pub const CTRL_MODULES_FALLBACK: u64 = 321;
pub const CTRL_MODULES_LIMIT_DEC: u64 = 322;
pub const CTRL_MODULES_LIMIT_INC: u64 = 323;
pub const CTRL_MODULES_LIMIT_EDIT: u64 = 324;
pub const CTRL_MODULES_DAYS_DEC: u64 = 325;
pub const CTRL_MODULES_DAYS_INC: u64 = 326;
pub const CTRL_MODULES_DAYS_EDIT: u64 = 327;
pub const CTRL_OPEN_STORE: u64 = 328;
pub const CTRL_HIDE_MOUSE: u64 = 330;
pub const CTRL_AGENT_TAB_CLOSE: u64 = 331;
pub const CTRL_MULTI_CURSOR_MODIFIER: u64 = 332;
pub const CTRL_CURSOR_BLINK: u64 = 333;
pub const CTRL_CURSOR_ANIMATION: u64 = 334;
pub const CTRL_CURSOR_SHAPE: u64 = 335;
pub const CTRL_REDUCE_MOTION: u64 = 336;
pub const CTRL_RESTORE_ON_STARTUP: u64 = 337;
pub const CTRL_ENABLE_LANGUAGE_SERVER: u64 = 350;
pub const CTRL_LANGUAGE_SERVERS: u64 = 351;
pub const CTRL_DEFINITION_SCROLL: u64 = 352;
pub const CTRL_LSP_COMPLETIONS: u64 = 353;
pub const CTRL_COMPLETION_TIMEOUT_DEC: u64 = 354;
pub const CTRL_COMPLETION_TIMEOUT_INC: u64 = 355;
pub const CTRL_COMPLETION_TIMEOUT_EDIT: u64 = 356;
pub const CTRL_FILE_TYPES: u64 = 357;
pub const CTRL_DIAGNOSTICS_SEVERITY: u64 = 358;
pub const CTRL_INLINE_DIAGNOSTICS: u64 = 359;
pub const CTRL_INLINE_PADDING_DEC: u64 = 360;
pub const CTRL_INLINE_PADDING_INC: u64 = 361;
pub const CTRL_INLINE_PADDING_EDIT: u64 = 362;
pub const CTRL_INLINE_COLUMN_DEC: u64 = 370;
pub const CTRL_INLINE_COLUMN_INC: u64 = 371;
pub const CTRL_INLINE_COLUMN_EDIT: u64 = 372;
pub const CTRL_LANGUAGE_BACK: u64 = 380;
/// A language's Configure button, by its index in `language_names`.
pub const LANGUAGE_OPEN_BASE: u64 = 29_000;
/// A language's own controls: `LANGUAGE_CTRL_BASE + index * LANGUAGE_CTRL_STRIDE + field`.
pub const LANGUAGE_CTRL_BASE: u64 = 30_000;
const LANGUAGE_CTRL_STRIDE: u64 = 8;
const LANGUAGE_ENABLE: u64 = 0;
const LANGUAGE_SERVERS: u64 = 1;
const LANGUAGE_COMPLETIONS: u64 = 2;
const LANGUAGE_TIMEOUT_DEC: u64 = 3;
const LANGUAGE_TIMEOUT_INC: u64 = 4;
const LANGUAGE_TIMEOUT_EDIT: u64 = 5;
pub const COMPLETION_TIMEOUT_MAX: u64 = 60_000;
const COMPLETION_TIMEOUT_STEP: i64 = 100;
pub const INLINE_COLUMNS_MAX: u64 = 1_000;
pub const CTRL_START_AT_LOGIN: u64 = 260;
pub const CTRL_AUTO_UPDATE: u64 = 261;
pub const CTRL_CHECK_UPDATES: u64 = 262;
pub const CTRL_BUFFER_FONT_DEC: u64 = 263;
pub const CTRL_BUFFER_FONT_INC: u64 = 264;
pub const CTRL_BUFFER_FONT_EDIT: u64 = 265;
pub const CTRL_SOFT_WRAP: u64 = 266;
pub const CTRL_DIFF_VIEW: u64 = 267;
pub const CTRL_EXTERNAL_EDITOR: u64 = 268;
pub const CTRL_BUFFER_FAMILY: u64 = 290;
pub const CTRL_BUFFER_WEIGHT_DEC: u64 = 291;
pub const CTRL_BUFFER_WEIGHT_INC: u64 = 292;
pub const CTRL_BUFFER_WEIGHT_EDIT: u64 = 293;
pub const CTRL_BUFFER_LINE_HEIGHT: u64 = 294;
pub const CTRL_BUFFER_FEATURES: u64 = 295;
pub const CTRL_BUFFER_FALLBACKS: u64 = 296;
pub const CTRL_TERM_FAMILY: u64 = 297;
pub const CTRL_TERM_WEIGHT_DEC: u64 = 298;
pub const CTRL_TERM_WEIGHT_INC: u64 = 299;
pub const CTRL_TERM_WEIGHT_EDIT: u64 = 300;
pub const CTRL_TERM_LINE_HEIGHT: u64 = 301;
pub const CTRL_TERM_FEATURES: u64 = 302;
pub const CTRL_TERM_FALLBACKS: u64 = 303;
pub const CTRL_TERM_FONT_DEC: u64 = 269;
pub const CTRL_TERM_FONT_INC: u64 = 270;
pub const CTRL_TERM_FONT_EDIT: u64 = 271;
pub const CTRL_AGENT_FONT_DEC: u64 = 284;
pub const CTRL_AGENT_FONT_INC: u64 = 285;
pub const CTRL_AGENT_FONT_EDIT: u64 = 286;
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
/// Per repository row of the Project page: id = base + row index.
pub const CTRL_REPO_RENAME_BASE: u64 = 20_000;
pub const CTRL_REPO_REMOVE_BASE: u64 = 21_000;
pub const CTRL_REPO_LIMIT: u64 = 1_000;
pub const PORT_MIN: u16 = 1024;
pub const MODULES_LIMIT_MAX_GB: u64 = 1024;
pub const MODULES_DAYS_MAX: u64 = 365;
/// When the pointer hides until the mouse moves, as (setting value, label).
/// What closing an agent's tab does, as (setting value, label).
const AGENT_TAB_CLOSE: [(&str, &str); 2] =
    [("hide", "Keep It Running"), ("stop", "Stop the Agent")];
const RESTORE_ON_STARTUP: [(&str, &str); 2] =
    [("last_session", "Last Session"), ("none", "Nothing")];
const MULTI_CURSOR_MODIFIERS: [(&str, &str); 2] = [("alt", "Alt"), ("cmd_or_ctrl", "Cmd Or Ctrl")];
const CURSOR_SHAPES: [(&str, &str); 4] = [
    ("bar", "Bar"),
    ("block", "Block"),
    ("underline", "Underline"),
    ("hollow", "Hollow"),
];
const REDUCE_MOTION: [(&str, &str); 2] = [("off", "Off"), ("on", "On")];
const DEFINITION_SCROLLS: [(&str, &str); 4] = [
    ("center", "Center"),
    ("minimum", "Minimum"),
    ("top", "Top"),
    ("preserve", "Preserve"),
];
const DIAGNOSTIC_SEVERITIES: [(&str, &str); 6] = [
    ("off", "Off"),
    ("error", "Error"),
    ("warning", "Warning"),
    ("info", "Info"),
    ("hint", "Hint"),
    ("all", "All"),
];

/// Every language a file can be, by name, as the Languages section lists them.
pub fn language_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = editor::Lang::LANGUAGES
        .iter()
        .map(|lang| lang.name())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// The language a per-language control belongs to, and which of its controls it is.
fn language_control(id: u64) -> Option<(&'static str, u64)> {
    let offset = id.checked_sub(LANGUAGE_CTRL_BASE)?;
    let name = *language_names().get((offset / LANGUAGE_CTRL_STRIDE) as usize)?;
    Some((name, offset % LANGUAGE_CTRL_STRIDE))
}

fn language_control_id(index: usize, field: u64) -> u64 {
    LANGUAGE_CTRL_BASE + index as u64 * LANGUAGE_CTRL_STRIDE + field
}

/// The language whose page a Configure button opens.
pub fn language_to_open(id: u64) -> Option<usize> {
    let index = id.checked_sub(LANGUAGE_OPEN_BASE)? as usize;
    (index < language_names().len()).then_some(index)
}

/// A number field on the Languages & Tools pages: its value as the field starts editing.
pub fn language_number(id: u64, s: &Settings) -> Option<String> {
    let value = match id {
        CTRL_COMPLETION_TIMEOUT_EDIT => s.completions.lsp_fetch_timeout_ms,
        CTRL_INLINE_PADDING_EDIT => u64::from(s.diagnostics.inline.padding),
        CTRL_INLINE_COLUMN_EDIT => u64::from(s.diagnostics.inline.min_column),
        id => match language_control(id)? {
            (name, LANGUAGE_TIMEOUT_EDIT) => s.language_server_settings(name).completion_timeout_ms,
            _ => return None,
        },
    };
    Some(value.to_string())
}

/// Store a number typed into a Languages & Tools field; returns whether it changed.
pub fn set_language_number(id: u64, value: f32, s: &mut Settings) -> bool {
    let value = value.max(0.0) as u64;
    match id {
        CTRL_COMPLETION_TIMEOUT_EDIT => {
            let next = value.min(COMPLETION_TIMEOUT_MAX);
            std::mem::replace(&mut s.completions.lsp_fetch_timeout_ms, next) != next
        }
        CTRL_INLINE_PADDING_EDIT | CTRL_INLINE_COLUMN_EDIT => {
            let next = value.min(INLINE_COLUMNS_MAX) as u32;
            let slot = if id == CTRL_INLINE_PADDING_EDIT {
                &mut s.diagnostics.inline.padding
            } else {
                &mut s.diagnostics.inline.min_column
            };
            std::mem::replace(slot, next) != next
        }
        id => match language_control(id) {
            Some((name, LANGUAGE_TIMEOUT_EDIT)) => {
                let next = value.min(COMPLETION_TIMEOUT_MAX);
                let changed = s.language_server_settings(name).completion_timeout_ms != next;
                language_completions(s, name).lsp_fetch_timeout_ms = Some(next);
                changed
            }
            _ => false,
        },
    }
}

fn language_entry<'a>(s: &'a mut Settings, name: &str) -> &'a mut settings::LanguageSettings {
    s.languages.entry(name.to_string()).or_default()
}

fn language_completions<'a>(
    s: &'a mut Settings,
    name: &str,
) -> &'a mut settings::LanguageCompletions {
    language_entry(s, name)
        .completions
        .get_or_insert_with(Default::default)
}

/// Drop a language's entry once nothing in it is set, so the file keeps no empty objects.
fn prune_language(s: &mut Settings, name: &str) {
    if let Some(own) = s.languages.get_mut(name) {
        if own
            .completions
            .as_ref()
            .is_some_and(|completions| *completions == Default::default())
        {
            own.completions = None;
        }
        if own.is_empty() {
            s.languages.remove(name);
        }
    }
}

fn step_completion_timeout(value: u64, step: i64) -> u64 {
    (value as i64 + step).clamp(0, COMPLETION_TIMEOUT_MAX as i64) as u64
}

/// A per-language control clicked: its own value flips or steps, over the shared one.
fn handle_language_control(id: u64, s: &mut Settings) -> bool {
    let Some((name, field)) = language_control(id) else {
        return false;
    };
    let current = s.language_server_settings(name);
    match field {
        LANGUAGE_ENABLE => language_entry(s, name).enable_language_server = Some(!current.enabled),
        LANGUAGE_COMPLETIONS => language_completions(s, name).lsp = Some(!current.completions),
        LANGUAGE_TIMEOUT_DEC | LANGUAGE_TIMEOUT_INC => {
            let step = if field == LANGUAGE_TIMEOUT_INC {
                COMPLETION_TIMEOUT_STEP
            } else {
                -COMPLETION_TIMEOUT_STEP
            };
            let next = step_completion_timeout(current.completion_timeout_ms, step);
            if next == current.completion_timeout_ms {
                return false;
            }
            language_completions(s, name).lsp_fetch_timeout_ms = Some(next);
        }
        _ => return false,
    }
    true
}

/// Whether a per-language control is left to the shared setting.
fn language_is_default(id: u64, s: &Settings) -> Option<bool> {
    let (name, field) = language_control(id)?;
    let own = s.languages.get(name);
    let completions = own.and_then(|own| own.completions.as_ref());
    Some(match field {
        LANGUAGE_ENABLE => own.is_none_or(|own| own.enable_language_server.is_none()),
        LANGUAGE_SERVERS => own.is_none_or(|own| own.language_servers.is_none()),
        LANGUAGE_COMPLETIONS => completions.is_none_or(|completions| completions.lsp.is_none()),
        LANGUAGE_TIMEOUT_EDIT => {
            completions.is_none_or(|completions| completions.lsp_fetch_timeout_ms.is_none())
        }
        _ => true,
    })
}

fn language_reset(id: u64, s: &mut Settings) -> Option<bool> {
    let (name, field) = language_control(id)?;
    let changed = !language_is_default(id, s)?;
    if let Some(own) = s.languages.get_mut(name) {
        match field {
            LANGUAGE_ENABLE => own.enable_language_server = None,
            LANGUAGE_SERVERS => own.language_servers = None,
            LANGUAGE_COMPLETIONS => {
                if let Some(completions) = own.completions.as_mut() {
                    completions.lsp = None;
                }
            }
            LANGUAGE_TIMEOUT_EDIT => {
                if let Some(completions) = own.completions.as_mut() {
                    completions.lsp_fetch_timeout_ms = None;
                }
            }
            _ => {}
        }
    }
    prune_language(s, name);
    Some(changed)
}

fn labels(choices: &[(&str, &str)]) -> Vec<String> {
    choices.iter().map(|(_, label)| label.to_string()).collect()
}

/// The label of the choice `value` is, or the first one's for a value the list does not know.
fn label_of(choices: &[(&str, &str)], value: &str) -> String {
    choices
        .iter()
        .find(|(choice, _)| *choice == value)
        .map_or(choices[0].1, |(_, label)| label)
        .to_string()
}

const HIDE_MOUSE: [(&str, &str); 3] = [
    ("never", "Never"),
    ("on_typing", "On Typing"),
    ("on_typing_and_action", "On Typing and Action"),
];
/// Where a copy-on-write clone is impossible, as (setting value, label); `module_store::Fallback` reads them.
const MODULES_FALLBACKS: [(&str, &str); 3] = [
    ("hardlink", "Hard Links"),
    ("copy", "Copy"),
    ("install", "Run Install"),
];
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
pub const RESET_OFFSET: u64 = 100_000;

/// Font-size bounds, matching the reference's `FontSize` stepper (min 6, max 72).
pub const FONT_SIZE_MIN: f32 = 6.0;
pub const FONT_SIZE_MAX: f32 = 72.0;
/// Font-weight bounds, matching the reference's `FontWeight` stepper (CSS weights 100-900).
pub const FONT_WEIGHT_MIN: f32 = 100.0;
pub const FONT_WEIGHT_MAX: f32 = 900.0;

/// Step a whole-number setting within `min..=max`; returns true if it changed.
pub fn step_u64(value: &mut u64, step: i64, min: u64, max: u64) -> bool {
    let next = (*value as i64 + step).clamp(min as i64, max as i64) as u64;
    std::mem::replace(value, next) != next
}

/// Step or set a listening port, kept out of the privileged range; returns true if it changed.
pub fn set_port(port: &mut u16, step: i32) -> bool {
    let next = (i32::from(*port) + step).clamp(i32::from(PORT_MIN), i32::from(u16::MAX)) as u16;
    std::mem::replace(port, next) != next
}

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
        language_servers: s.global_lsp_settings.button,
    }
}

const THEMES: [&str; 4] = ["One Dark", "One Light", "Ayu Mirage", "Gruvbox Dark"];

/// The built-in themes, then the user's (a user theme named like a built-in one replaces it).
pub fn theme_names() -> Vec<String> {
    let mut names: Vec<String> = THEMES.iter().map(|name| name.to_string()).collect();
    for name in ui::theme_file::user_theme_names() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The theme after `current` in the theme list, wrapping around.
pub fn next_theme(current: &str) -> String {
    let names = theme_names();
    let at = names.iter().position(|theme| *theme == current);
    names
        .get(at.map_or(0, |at| (at + 1) % names.len()))
        .cloned()
        .unwrap_or_else(|| THEMES[0].to_string())
}

/// Popover-item click ids start here (an item's id = POPOVER_BASE + its index in the option list).
pub const POPOVER_BASE: u64 = 3000;

/// True if a control opens a popover list (dropdown) rather than acting immediately (stepper).
pub fn is_dropdown(id: u64) -> bool {
    matches!(
        id,
        CTRL_THEME
            | CTRL_THEME_SELECTION
            | CTRL_THEME_LIGHT
            | CTRL_THEME_DARK
            | CTRL_FONT_FAMILY
            | CTRL_BUFFER_FAMILY
            | CTRL_TERM_FAMILY
            | CTRL_BUFFER_LINE_HEIGHT
            | CTRL_TERM_LINE_HEIGHT
            | CTRL_MODE
            | CTRL_SOFT_WRAP
            | CTRL_DIFF_VIEW
            | CTRL_EXTERNAL_EDITOR
            | CTRL_MODULES_FALLBACK
            | CTRL_HIDE_MOUSE
            | CTRL_AGENT_TAB_CLOSE
            | CTRL_RESTORE_ON_STARTUP
            | CTRL_MULTI_CURSOR_MODIFIER
            | CTRL_CURSOR_SHAPE
            | CTRL_REDUCE_MOTION
            | CTRL_DEFINITION_SCROLL
            | CTRL_DIAGNOSTICS_SEVERITY
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
        CTRL_THEME | CTRL_THEME_LIGHT | CTRL_THEME_DARK => theme_names(),
        CTRL_THEME_SELECTION => sv(&["Static", "Dynamic"]),
        CTRL_FONT_FAMILY => fonts.to_vec(),
        CTRL_BUFFER_FAMILY | CTRL_TERM_FAMILY => std::iter::once(".PomeloMono".to_string())
            .chain(fonts.iter().filter(|font| *font != ".PomeloMono").cloned())
            .collect(),
        CTRL_BUFFER_LINE_HEIGHT | CTRL_TERM_LINE_HEIGHT => sv(&["Comfortable", "Standard"]),
        CTRL_MODE => sv(&["Light", "Dark", "System"]),
        CTRL_SOFT_WRAP => sv(&["None", "Editor Width"]),
        CTRL_DIFF_VIEW => sv(&["Split", "Unified"]),
        CTRL_MODULES_FALLBACK => MODULES_FALLBACKS
            .iter()
            .map(|(_, label)| label.to_string())
            .collect(),
        CTRL_HIDE_MOUSE => HIDE_MOUSE
            .iter()
            .map(|(_, label)| label.to_string())
            .collect(),
        CTRL_AGENT_TAB_CLOSE => AGENT_TAB_CLOSE
            .iter()
            .map(|(_, label)| label.to_string())
            .collect(),
        CTRL_RESTORE_ON_STARTUP => labels(&RESTORE_ON_STARTUP),
        CTRL_MULTI_CURSOR_MODIFIER => labels(&MULTI_CURSOR_MODIFIERS),
        CTRL_CURSOR_SHAPE => labels(&CURSOR_SHAPES),
        CTRL_REDUCE_MOTION => labels(&REDUCE_MOTION),
        CTRL_DEFINITION_SCROLL => labels(&DEFINITION_SCROLLS),
        CTRL_DIAGNOSTICS_SEVERITY => labels(&DIAGNOSTIC_SEVERITIES),
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
        CTRL_THEME_SELECTION => cap(&s.theme_selection),
        CTRL_THEME_LIGHT => s.theme_light.clone(),
        CTRL_THEME_DARK => s.theme_dark.clone(),
        CTRL_FONT_FAMILY => s.ui_font.clone(),
        CTRL_BUFFER_FAMILY => s.buffer_font_family.clone(),
        CTRL_TERM_FAMILY => s.terminal_font_family.clone(),
        CTRL_BUFFER_LINE_HEIGHT => line_height_label(&s.buffer_line_height),
        CTRL_TERM_LINE_HEIGHT => line_height_label(&s.terminal_line_height),
        CTRL_MODE => cap(&s.theme_mode),
        CTRL_SOFT_WRAP => if s.soft_wrap { "Editor Width" } else { "None" }.to_string(),
        CTRL_DIFF_VIEW => if s.split_diff { "Split" } else { "Unified" }.to_string(),
        CTRL_HIDE_MOUSE => HIDE_MOUSE
            .iter()
            .find(|(value, _)| *value == s.hide_mouse)
            .map_or(HIDE_MOUSE[2].1, |(_, label)| label)
            .to_string(),
        CTRL_AGENT_TAB_CLOSE => AGENT_TAB_CLOSE
            .iter()
            .find(|(value, _)| *value == s.agent_tab_close)
            .map_or(AGENT_TAB_CLOSE[0].1, |(_, label)| label)
            .to_string(),
        CTRL_RESTORE_ON_STARTUP => label_of(&RESTORE_ON_STARTUP, &s.restore_on_startup),
        CTRL_MULTI_CURSOR_MODIFIER => label_of(&MULTI_CURSOR_MODIFIERS, &s.multi_cursor_modifier),
        CTRL_CURSOR_SHAPE => label_of(&CURSOR_SHAPES, &s.cursor_shape),
        CTRL_REDUCE_MOTION => label_of(&REDUCE_MOTION, &s.reduce_motion),
        CTRL_DEFINITION_SCROLL => {
            label_of(&DEFINITION_SCROLLS, &s.go_to_definition_scroll_strategy)
        }
        CTRL_DIAGNOSTICS_SEVERITY => label_of(&DIAGNOSTIC_SEVERITIES, &s.diagnostics_max_severity),
        CTRL_MODULES_FALLBACK => MODULES_FALLBACKS
            .iter()
            .find(|(value, _)| *value == s.modules_fallback)
            .map_or(MODULES_FALLBACKS[0].1, |(_, label)| label)
            .to_string(),
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
        CTRL_THEME_SELECTION => s.theme_selection = val.to_lowercase(),
        CTRL_THEME_LIGHT => s.theme_light = val.clone(),
        CTRL_THEME_DARK => s.theme_dark = val.clone(),
        CTRL_FONT_FAMILY => s.ui_font = val.clone(),
        CTRL_BUFFER_FAMILY => s.buffer_font_family = val.clone(),
        CTRL_TERM_FAMILY => s.terminal_font_family = val.clone(),
        CTRL_BUFFER_LINE_HEIGHT => {
            s.buffer_line_height = serde_json::Value::String(val.to_lowercase())
        }
        CTRL_TERM_LINE_HEIGHT => {
            s.terminal_line_height = serde_json::Value::String(val.to_lowercase())
        }
        CTRL_MODE => s.theme_mode = val.to_lowercase(),
        CTRL_SOFT_WRAP => s.soft_wrap = val == "Editor Width",
        CTRL_DIFF_VIEW => s.split_diff = val == "Split",
        CTRL_HIDE_MOUSE => {
            let Some((value, _)) = HIDE_MOUSE.iter().find(|(_, label)| label == val) else {
                return false;
            };
            s.hide_mouse = value.to_string();
        }
        CTRL_AGENT_TAB_CLOSE => {
            let Some((value, _)) = AGENT_TAB_CLOSE.iter().find(|(_, label)| label == val) else {
                return false;
            };
            s.agent_tab_close = value.to_string();
        }
        CTRL_MULTI_CURSOR_MODIFIER
        | CTRL_RESTORE_ON_STARTUP
        | CTRL_CURSOR_SHAPE
        | CTRL_REDUCE_MOTION
        | CTRL_DEFINITION_SCROLL
        | CTRL_DIAGNOSTICS_SEVERITY => {
            let (choices, slot) = match id {
                CTRL_RESTORE_ON_STARTUP => (&RESTORE_ON_STARTUP[..], &mut s.restore_on_startup),
                CTRL_MULTI_CURSOR_MODIFIER => {
                    (&MULTI_CURSOR_MODIFIERS[..], &mut s.multi_cursor_modifier)
                }
                CTRL_CURSOR_SHAPE => (&CURSOR_SHAPES[..], &mut s.cursor_shape),
                CTRL_DEFINITION_SCROLL => (
                    &DEFINITION_SCROLLS[..],
                    &mut s.go_to_definition_scroll_strategy,
                ),
                CTRL_DIAGNOSTICS_SEVERITY => {
                    (&DIAGNOSTIC_SEVERITIES[..], &mut s.diagnostics_max_severity)
                }
                _ => (&REDUCE_MOTION[..], &mut s.reduce_motion),
            };
            let Some((value, _)) = choices.iter().find(|(_, label)| label == val) else {
                return false;
            };
            *slot = value.to_string();
        }
        CTRL_MODULES_FALLBACK => {
            let Some((value, _)) = MODULES_FALLBACKS.iter().find(|(_, label)| label == val) else {
                return false;
            };
            s.modules_fallback = value.to_string();
        }
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
        CTRL_THEME_SELECTION => s.theme_selection == d.theme_selection,
        CTRL_THEME_LIGHT => s.theme_light == d.theme_light,
        CTRL_THEME_DARK => s.theme_dark == d.theme_dark,
        CTRL_FONT_FAMILY => s.ui_font == d.ui_font,
        CTRL_BUFFER_FAMILY => s.buffer_font_family == d.buffer_font_family,
        CTRL_TERM_FAMILY => s.terminal_font_family == d.terminal_font_family,
        CTRL_BUFFER_WEIGHT_EDIT => s.buffer_font_weight == d.buffer_font_weight,
        CTRL_TERM_WEIGHT_EDIT => s.terminal_font_weight == d.terminal_font_weight,
        CTRL_BUFFER_LINE_HEIGHT => s.buffer_line_height == d.buffer_line_height,
        CTRL_TERM_LINE_HEIGHT => s.terminal_line_height == d.terminal_line_height,
        CTRL_FONT_SIZE_EDIT => s.ui_font_size == d.ui_font_size,
        CTRL_FONT_WEIGHT_EDIT => s.ui_font_weight == d.ui_font_weight,
        CTRL_MODE => s.theme_mode == d.theme_mode,
        CTRL_SHOW_AGENT => s.agent_hidden == d.agent_hidden,
        id if function_button(id).is_some() => {
            let index = function_button(id).unwrap_or_default();
            s.func_hidden.get(index).copied().unwrap_or(false)
                == d.func_hidden.get(index).copied().unwrap_or(false)
        }
        CTRL_SHOW_TERMINAL => s.terminal_hidden == d.terminal_hidden,
        CTRL_SHOW_DIAGNOSTICS => s.show_diagnostics == d.show_diagnostics,
        CTRL_SHOW_CURSOR => s.show_cursor_position == d.show_cursor_position,
        CTRL_SHOW_LANGUAGE => s.show_language == d.show_language,
        CTRL_SHOW_BRANCH => s.show_branch == d.show_branch,
        CTRL_SHOW_SESSION => s.show_session_name == d.show_session_name,
        CTRL_JIRA_ONLY_MINE => s.jira_only_mine == d.jira_only_mine,
        CTRL_GROUP_WORKSPACES => s.group_workspaces == d.group_workspaces,
        CTRL_PROXY_ENABLED => s.dev_proxy_enabled == d.dev_proxy_enabled,
        CTRL_WEBHOOK_ENABLED => s.webhook_enabled == d.webhook_enabled,
        CTRL_PROXY_PORT_EDIT => s.dev_proxy_port == d.dev_proxy_port,
        CTRL_WEBHOOK_PORT_EDIT => s.webhook_port == d.webhook_port,
        CTRL_MODULES_ENABLED => s.modules_store_enabled == d.modules_store_enabled,
        CTRL_MODULES_FALLBACK => s.modules_fallback == d.modules_fallback,
        CTRL_HIDE_MOUSE => s.hide_mouse == d.hide_mouse,
        CTRL_AGENT_TAB_CLOSE => s.agent_tab_close == d.agent_tab_close,
        CTRL_RESTORE_ON_STARTUP => s.restore_on_startup == d.restore_on_startup,
        CTRL_MULTI_CURSOR_MODIFIER => s.multi_cursor_modifier == d.multi_cursor_modifier,
        CTRL_CURSOR_BLINK => s.cursor_blink == d.cursor_blink,
        CTRL_CURSOR_ANIMATION => s.cursor_animation == d.cursor_animation,
        CTRL_CURSOR_SHAPE => s.cursor_shape == d.cursor_shape,
        CTRL_REDUCE_MOTION => s.reduce_motion == d.reduce_motion,
        CTRL_MODULES_LIMIT_EDIT => s.modules_size_limit_gb == d.modules_size_limit_gb,
        CTRL_MODULES_DAYS_EDIT => s.modules_unused_days == d.modules_unused_days,
        CTRL_AGENT_COMMAND => s.agent_command == d.agent_command,
        CTRL_AUTO_UPDATE => s.auto_update == d.auto_update,
        CTRL_BUFFER_FONT_EDIT => s.buffer_font_size == d.buffer_font_size,
        CTRL_SOFT_WRAP => s.soft_wrap == d.soft_wrap,
        CTRL_DIFF_VIEW => s.split_diff == d.split_diff,
        CTRL_EXTERNAL_EDITOR => s.external_editor == d.external_editor,
        CTRL_TERM_FONT_EDIT => s.terminal_font_size == d.terminal_font_size,
        CTRL_AGENT_FONT_EDIT => s.agent_font_size == d.agent_font_size,
        CTRL_TERM_SHELL => s.terminal_shell == d.terminal_shell,
        CTRL_SCROLLBACK_EDIT => s.terminal_scrollback == d.terminal_scrollback,
        CTRL_NOTIFY => s.notify_claude == d.notify_claude,
        CTRL_NOTIFY_FOCUSED => s.notify_when_focused == d.notify_when_focused,
        CTRL_ENABLE_LANGUAGE_SERVER => s.enable_language_server == d.enable_language_server,
        CTRL_DEFINITION_SCROLL => {
            s.go_to_definition_scroll_strategy == d.go_to_definition_scroll_strategy
        }
        CTRL_LSP_COMPLETIONS => s.completions.lsp == d.completions.lsp,
        CTRL_COMPLETION_TIMEOUT_EDIT => {
            s.completions.lsp_fetch_timeout_ms == d.completions.lsp_fetch_timeout_ms
        }
        CTRL_DIAGNOSTICS_SEVERITY => s.diagnostics_max_severity == d.diagnostics_max_severity,
        CTRL_INLINE_DIAGNOSTICS => s.diagnostics.inline.enabled == d.diagnostics.inline.enabled,
        CTRL_INLINE_PADDING_EDIT => s.diagnostics.inline.padding == d.diagnostics.inline.padding,
        CTRL_INLINE_COLUMN_EDIT => {
            s.diagnostics.inline.min_column == d.diagnostics.inline.min_column
        }
        id if language_control(id).is_some() => language_is_default(id, s).unwrap_or(true),
        id => sound_event(id).is_none_or(|event| s.sound_for(event) == d.sound_for(event)),
    }
}

pub fn reset_to_default(id: u64, s: &mut Settings) -> bool {
    let d = Settings::default();
    let changed = !is_default(id, s);
    match id {
        CTRL_THEME => s.theme = d.theme,
        CTRL_THEME_SELECTION => s.theme_selection = d.theme_selection,
        CTRL_THEME_LIGHT => s.theme_light = d.theme_light,
        CTRL_THEME_DARK => s.theme_dark = d.theme_dark,
        CTRL_FONT_FAMILY => s.ui_font = d.ui_font,
        CTRL_FONT_SIZE_EDIT => s.ui_font_size = d.ui_font_size,
        CTRL_FONT_WEIGHT_EDIT => s.ui_font_weight = d.ui_font_weight,
        CTRL_BUFFER_FAMILY => s.buffer_font_family = d.buffer_font_family,
        CTRL_TERM_FAMILY => s.terminal_font_family = d.terminal_font_family,
        CTRL_BUFFER_WEIGHT_EDIT => s.buffer_font_weight = d.buffer_font_weight,
        CTRL_TERM_WEIGHT_EDIT => s.terminal_font_weight = d.terminal_font_weight,
        CTRL_BUFFER_LINE_HEIGHT => s.buffer_line_height = d.buffer_line_height,
        CTRL_TERM_LINE_HEIGHT => s.terminal_line_height = d.terminal_line_height,
        CTRL_MODE => s.theme_mode = d.theme_mode,
        CTRL_SHOW_AGENT => s.agent_hidden = d.agent_hidden,
        id if function_button(id).is_some() => {
            set_function_hidden(s, function_button(id).unwrap_or_default(), false)
        }
        CTRL_SHOW_TERMINAL => s.terminal_hidden = d.terminal_hidden,
        CTRL_SHOW_DIAGNOSTICS => s.show_diagnostics = d.show_diagnostics,
        CTRL_SHOW_CURSOR => s.show_cursor_position = d.show_cursor_position,
        CTRL_SHOW_LANGUAGE => s.show_language = d.show_language,
        CTRL_SHOW_BRANCH => s.show_branch = d.show_branch,
        CTRL_SHOW_SESSION => s.show_session_name = d.show_session_name,
        CTRL_JIRA_ONLY_MINE => s.jira_only_mine = d.jira_only_mine,
        CTRL_GROUP_WORKSPACES => s.group_workspaces = d.group_workspaces,
        CTRL_PROXY_ENABLED => s.dev_proxy_enabled = d.dev_proxy_enabled,
        CTRL_WEBHOOK_ENABLED => s.webhook_enabled = d.webhook_enabled,
        CTRL_PROXY_PORT_EDIT => s.dev_proxy_port = d.dev_proxy_port,
        CTRL_WEBHOOK_PORT_EDIT => s.webhook_port = d.webhook_port,
        CTRL_MODULES_ENABLED => s.modules_store_enabled = d.modules_store_enabled,
        CTRL_MODULES_FALLBACK => s.modules_fallback = d.modules_fallback.clone(),
        CTRL_HIDE_MOUSE => s.hide_mouse = d.hide_mouse.clone(),
        CTRL_AGENT_TAB_CLOSE => s.agent_tab_close = d.agent_tab_close.clone(),
        CTRL_RESTORE_ON_STARTUP => s.restore_on_startup = d.restore_on_startup.clone(),
        CTRL_MULTI_CURSOR_MODIFIER => s.multi_cursor_modifier = d.multi_cursor_modifier.clone(),
        CTRL_CURSOR_BLINK => s.cursor_blink = d.cursor_blink,
        CTRL_CURSOR_ANIMATION => s.cursor_animation = d.cursor_animation.clone(),
        CTRL_CURSOR_SHAPE => s.cursor_shape = d.cursor_shape.clone(),
        CTRL_REDUCE_MOTION => s.reduce_motion = d.reduce_motion.clone(),
        CTRL_MODULES_LIMIT_EDIT => s.modules_size_limit_gb = d.modules_size_limit_gb,
        CTRL_MODULES_DAYS_EDIT => s.modules_unused_days = d.modules_unused_days,
        CTRL_AGENT_COMMAND => s.agent_command = d.agent_command,
        CTRL_AUTO_UPDATE => s.auto_update = d.auto_update,
        CTRL_BUFFER_FONT_EDIT => s.buffer_font_size = d.buffer_font_size,
        CTRL_SOFT_WRAP => s.soft_wrap = d.soft_wrap,
        CTRL_DIFF_VIEW => s.split_diff = d.split_diff,
        CTRL_EXTERNAL_EDITOR => s.external_editor = d.external_editor,
        CTRL_TERM_FONT_EDIT => s.terminal_font_size = d.terminal_font_size,
        CTRL_AGENT_FONT_EDIT => s.agent_font_size = d.agent_font_size,
        CTRL_TERM_SHELL => s.terminal_shell = d.terminal_shell,
        CTRL_SCROLLBACK_EDIT => s.terminal_scrollback = d.terminal_scrollback,
        CTRL_NOTIFY => s.notify_claude = d.notify_claude,
        CTRL_NOTIFY_FOCUSED => s.notify_when_focused = d.notify_when_focused,
        CTRL_ENABLE_LANGUAGE_SERVER => s.enable_language_server = d.enable_language_server,
        CTRL_DEFINITION_SCROLL => {
            s.go_to_definition_scroll_strategy = d.go_to_definition_scroll_strategy
        }
        CTRL_LSP_COMPLETIONS => s.completions.lsp = d.completions.lsp,
        CTRL_COMPLETION_TIMEOUT_EDIT => {
            s.completions.lsp_fetch_timeout_ms = d.completions.lsp_fetch_timeout_ms
        }
        CTRL_DIAGNOSTICS_SEVERITY => s.diagnostics_max_severity = d.diagnostics_max_severity,
        CTRL_INLINE_DIAGNOSTICS => s.diagnostics.inline.enabled = d.diagnostics.inline.enabled,
        CTRL_INLINE_PADDING_EDIT => s.diagnostics.inline.padding = d.diagnostics.inline.padding,
        CTRL_INLINE_COLUMN_EDIT => {
            s.diagnostics.inline.min_column = d.diagnostics.inline.min_column
        }
        id if language_control(id).is_some() => return language_reset(id, s).unwrap_or(false),
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
        id if function_button(id).is_some() => {
            let index = function_button(id).unwrap_or_default();
            let hidden = s.func_hidden.get(index).copied().unwrap_or(false);
            set_function_hidden(s, index, !hidden);
            true
        }
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
        CTRL_CURSOR_BLINK => {
            s.cursor_blink = !s.cursor_blink;
            true
        }
        CTRL_CURSOR_ANIMATION => {
            s.cursor_animation.enabled = !s.cursor_animation.enabled;
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
        CTRL_GROUP_WORKSPACES => {
            s.group_workspaces = !s.group_workspaces;
            true
        }
        CTRL_PROXY_ENABLED => {
            s.dev_proxy_enabled = !s.dev_proxy_enabled;
            true
        }
        CTRL_WEBHOOK_ENABLED => {
            s.webhook_enabled = !s.webhook_enabled;
            true
        }
        CTRL_PROXY_PORT_DEC => set_port(&mut s.dev_proxy_port, -1),
        CTRL_PROXY_PORT_INC => set_port(&mut s.dev_proxy_port, 1),
        CTRL_WEBHOOK_PORT_DEC => set_port(&mut s.webhook_port, -1),
        CTRL_WEBHOOK_PORT_INC => set_port(&mut s.webhook_port, 1),
        CTRL_MODULES_ENABLED => {
            s.modules_store_enabled = !s.modules_store_enabled;
            true
        }
        CTRL_MODULES_LIMIT_DEC => {
            step_u64(&mut s.modules_size_limit_gb, -5, 1, MODULES_LIMIT_MAX_GB)
        }
        CTRL_MODULES_LIMIT_INC => {
            step_u64(&mut s.modules_size_limit_gb, 5, 1, MODULES_LIMIT_MAX_GB)
        }
        CTRL_MODULES_DAYS_DEC => step_u64(&mut s.modules_unused_days, -1, 0, MODULES_DAYS_MAX),
        CTRL_MODULES_DAYS_INC => step_u64(&mut s.modules_unused_days, 1, 0, MODULES_DAYS_MAX),
        CTRL_NOTIFY => {
            s.notify_claude = !s.notify_claude;
            true
        }
        CTRL_AUTO_UPDATE => {
            s.auto_update = !s.auto_update;
            true
        }
        CTRL_BUFFER_WEIGHT_DEC => set_clamped(
            &mut s.buffer_font_weight,
            -100.0,
            FONT_WEIGHT_MIN,
            FONT_WEIGHT_MAX,
        ),
        CTRL_BUFFER_WEIGHT_INC => set_clamped(
            &mut s.buffer_font_weight,
            100.0,
            FONT_WEIGHT_MIN,
            FONT_WEIGHT_MAX,
        ),
        CTRL_TERM_WEIGHT_DEC => set_clamped(
            &mut s.terminal_font_weight,
            -100.0,
            FONT_WEIGHT_MIN,
            FONT_WEIGHT_MAX,
        ),
        CTRL_TERM_WEIGHT_INC => set_clamped(
            &mut s.terminal_font_weight,
            100.0,
            FONT_WEIGHT_MIN,
            FONT_WEIGHT_MAX,
        ),
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
        CTRL_AGENT_FONT_DEC => {
            set_clamped(&mut s.agent_font_size, -1.0, FONT_SIZE_MIN, FONT_SIZE_MAX)
        }
        CTRL_AGENT_FONT_INC => {
            set_clamped(&mut s.agent_font_size, 1.0, FONT_SIZE_MIN, FONT_SIZE_MAX)
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
        CTRL_ENABLE_LANGUAGE_SERVER => {
            s.enable_language_server = !s.enable_language_server;
            true
        }
        CTRL_LSP_COMPLETIONS => {
            s.completions.lsp = !s.completions.lsp;
            true
        }
        CTRL_COMPLETION_TIMEOUT_DEC | CTRL_COMPLETION_TIMEOUT_INC => {
            let step = if id == CTRL_COMPLETION_TIMEOUT_INC {
                COMPLETION_TIMEOUT_STEP
            } else {
                -COMPLETION_TIMEOUT_STEP
            };
            let next = step_completion_timeout(s.completions.lsp_fetch_timeout_ms, step);
            std::mem::replace(&mut s.completions.lsp_fetch_timeout_ms, next) != next
        }
        CTRL_INLINE_DIAGNOSTICS => {
            s.diagnostics.inline.enabled = !s.diagnostics.inline.enabled;
            true
        }
        CTRL_INLINE_PADDING_DEC
        | CTRL_INLINE_PADDING_INC
        | CTRL_INLINE_COLUMN_DEC
        | CTRL_INLINE_COLUMN_INC => {
            let slot = if matches!(id, CTRL_INLINE_PADDING_DEC | CTRL_INLINE_PADDING_INC) {
                &mut s.diagnostics.inline.padding
            } else {
                &mut s.diagnostics.inline.min_column
            };
            let next = if matches!(id, CTRL_INLINE_PADDING_INC | CTRL_INLINE_COLUMN_INC) {
                (*slot + 1).min(INLINE_COLUMNS_MAX as u32)
            } else {
                slot.saturating_sub(1)
            };
            std::mem::replace(slot, next) != next
        }
        id if language_control(id).is_some() => handle_language_control(id, s),
        _ => false,
    }
}

/// A line height setting as the dropdown shows it; a number or `{"custom": n}` is Custom.
fn line_height_label(value: &serde_json::Value) -> String {
    match value.as_str() {
        Some("comfortable") => "Comfortable".into(),
        Some("standard") => "Standard".into(),
        _ => "Custom".into(),
    }
}

/// The rows every font section has, as the reference lays them out: family, size, weight, line height (not
/// for the UI font), features and fallbacks.
struct FontRows {
    family: u64,
    family_value: String,
    size: (u64, u64, u64),
    size_value: f32,
    size_description: &'static str,
    weight: (u64, u64, u64),
    weight_value: f32,
    line_height: Option<(u64, String)>,
    features: u64,
    fallbacks: u64,
    what: &'static str,
}

fn font_rows(s: &Settings, rows: FontRows) -> Vec<PageItem> {
    let mut items = vec![
        PageItem::Row(SettingRow {
            title: "Font Family".into(),
            description: format!("Font family used for {}.", rows.what).into(),
            control: Control::Dropdown {
                id: rows.family,
                value: rows.family_value,
            },
            reset: reset_if_changed(rows.family, s),
        }),
        PageItem::Row(SettingRow {
            title: "Font Size".into(),
            description: rows.size_description.into(),
            control: Control::Stepper {
                dec: rows.size.0,
                inc: rows.size.1,
                edit: rows.size.2,
                value: format!("{:.0}", rows.size_value),
            },
            reset: reset_if_changed(rows.size.2, s),
        }),
        PageItem::Row(SettingRow {
            title: "Font Weight".into(),
            description: format!("Font weight for {} (100-900).", rows.what).into(),
            control: Control::Stepper {
                dec: rows.weight.0,
                inc: rows.weight.1,
                edit: rows.weight.2,
                value: format!("{:.0}", rows.weight_value),
            },
            reset: reset_if_changed(rows.weight.2, s),
        }),
    ];
    if let Some((id, value)) = rows.line_height {
        items.push(PageItem::Row(SettingRow {
            title: "Line Height".into(),
            description: format!("Line height for {}.", rows.what).into(),
            control: Control::Dropdown { id, value },
            reset: reset_if_changed(id, s),
        }));
    }
    items.push(PageItem::Row(SettingRow {
        title: "Font Features".into(),
        description: format!(
            "The OpenType features to enable for rendering in {}.",
            rows.what
        )
        .into(),
        control: Control::EditInJson { id: rows.features },
        reset: None,
    }));
    items.push(PageItem::Row(SettingRow {
        title: "Font Fallbacks".into(),
        description: format!("The font fallbacks to use for rendering in {}.", rows.what).into(),
        control: Control::EditInJson { id: rows.fallbacks },
        reset: None,
    }));
    items
}

/// Whether `id` is one of the "Edit in settings.json" buttons, which all open the settings file.
pub fn is_edit_in_json(id: u64) -> bool {
    matches!(
        id,
        CTRL_FONT_FEATURES
            | CTRL_FONT_FALLBACKS
            | CTRL_BUFFER_FEATURES
            | CTRL_BUFFER_FALLBACKS
            | CTRL_TERM_FEATURES
            | CTRL_TERM_FALLBACKS
            | CTRL_LANGUAGE_SERVERS
            | CTRL_FILE_TYPES
    ) || language_control(id).is_some_and(|(_, field)| field == LANGUAGE_SERVERS)
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

/// The window chrome: `fixed` is the sidebar's top and the toolbar strip (opaque, so it masks scrolled-away page
/// content); `nav` is the category list laid out `nav_scroll` up, to be clipped to `nav_clip`, and `nav_height`
/// its full height.
pub struct Chrome {
    pub fixed: ui::Painted,
    pub nav: ui::Painted,
    pub nav_clip: Rect,
    pub nav_height: f32,
}

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
    nav_scroll: f32,
    project: &str,
    language: Option<usize>,
) -> Chrome {
    let project_scope =
        (matches!(selected, PROJECT | INTEGRATIONS) && !project.is_empty()).then_some(project);
    let sub_page = language
        .filter(|_| selected == LANGUAGES_TOOLS)
        .and_then(|index| language_names().get(index).copied());
    let x = rem(SIDEBAR_W);
    let mut out = render(
        &sidebar_head(search, search_active),
        Rect::new(0.0, 0.0, x, h, sidebar_c()),
    );
    let nav_top = out
        .hits
        .iter()
        .find(|(_, id)| *id == CTRL_SEARCH)
        .map_or(0.0, |(rect, _)| rect.y + rect.h)
        + rem(10.0);
    let nav_clip = Rect::new(0.0, nav_top, x, (h - nav_top).max(0.0), Rgba::TRANSPARENT);
    let origin = nav_top - nav_scroll;
    let nav = render(
        &sidebar_nav(selected, hovered, active_section, expanded, search),
        Rect::new(0.0, origin, x, h.max(1.0) * 4.0, Rgba::TRANSPARENT),
    );
    let nav_height = nav
        .hits
        .iter()
        .map(|(rect, _)| rect.y + rect.h)
        .fold(origin, f32::max)
        - origin
        + rem(10.0);
    let strip: Node = div()
        .col()
        .h_px(PAGE_TOP)
        .bg(bg_c())
        .px(CONTENT_PAD)
        .child(div().h_px(CONTENT_TOP))
        .child(match sub_page {
            Some(name) => {
                sub_page_header(&["User", CATEGORIES[LANGUAGES_TOOLS].0, "Languages", name])
            }
            None => toolbar(project_scope),
        })
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
    Chrome {
        fixed: out,
        nav,
        nav_clip,
        nav_height,
    }
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
    } else if selected == LANGUAGES_TOOLS {
        match state
            .language
            .and_then(|index| Some((index, *language_names().get(index)?)))
        {
            Some((index, name)) => render_page_under(
                div().into(),
                &language_page(s, index, name),
                search,
                editing,
                w,
            ),
            None => render_page(&languages_tools_page(s), search, editing, w),
        }
    } else if selected == TERMINAL {
        render_page(&terminal_page(s), search, editing, w)
    } else if selected == KEYMAP {
        render_page(&keymap_page(&state.keymap), search, editing, w)
    } else if selected == AGENT {
        render_page(&agent_page(s, &state.agent), search, editing, w)
    } else if selected == NOTIFICATIONS {
        render_page(&notifications_page(s), search, editing, w)
    } else if selected == DEV_SERVICES {
        render_page(
            &dev_services_page(s, &state.dev_services),
            search,
            editing,
            w,
        )
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
        0.0,
        &state.project.session,
        state.language,
    );
    for part in [ch.fixed, ch.nav] {
        out.rects.extend(part.rects);
        out.tris.extend(part.tris);
        out.texts.extend(part.texts);
        out.icons.extend(part.icons);
        out.hits.extend(part.hits);
    }
    out
}

/// The sidebar's fixed top: room for the traffic lights and the search box.
fn sidebar_head(search: &str, search_active: bool) -> Node {
    let col = div()
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
    col.child(search_box).into()
}

/// The category list under the search box; it scrolls when it is taller than the window.
fn sidebar_nav(
    selected: usize,
    hovered: Option<u64>,
    active_section: Option<usize>,
    expanded: &[bool],
    search: &str,
) -> Node {
    let mut col = div().col().w_px(240.0).px(10.0).pb(10.0).gap(2.0);
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
        LANGUAGES_TOOLS => Some(languages_tools_page(&Settings::default())),
        TERMINAL => Some(terminal_page(&Settings::default())),
        KEYMAP => Some(keymap_page(&KeymapPage::default())),
        NOTIFICATIONS => Some(notifications_page(&Settings::default())),
        DEV_SERVICES => Some(dev_services_page(
            &Settings::default(),
            &DevServicesPage::default(),
        )),
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
/// The scope the page edits: the user's settings file, or (Project and Integrations) the open project, whose
/// config the right button then opens.
fn toolbar(project: Option<&str>) -> Node {
    let (scope, file): (Node, Node) = match project {
        Some(name) => (
            button_static(name.to_string(), ButtonStyle::TintedAccent).into(),
            ui::button_sized(
                CTRL_EDIT_PROJECT_CONFIG,
                "Edit pom.yml",
                ButtonStyle::OutlinedGhost,
                ui::ButtonSize::Default,
            )
            .into(),
        ),
        None => (
            button_static("User", ButtonStyle::TintedAccent).into(),
            ui::button_sized(
                CTRL_OPEN_JSON,
                "Edit in settings.json",
                ButtonStyle::OutlinedGhost,
                ui::ButtonSize::Default,
            )
            .into(),
        ),
    };
    // The left file group grows to fill, pushing the right button to the content's right edge. When the left
    // group's content is wider than its grown slot, it overflows under the (last-drawn, on-top) right button
    // rather than shoving it off-window.
    div()
        .row()
        .h_px(30.0)
        .items_center()
        .justify_between()
        .child(div().row().gap(8.0).items_center().child(scope))
        .child(file)
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
    /// A key binding, drawn as key glyphs: `cmd-k cmd-s` is two keystrokes.
    Keys {
        binding: String,
    },
    /// Opens a page of its own.
    SubPage {
        id: u64,
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

fn theme_items(s: &Settings) -> Vec<PageItem> {
    let dropdown = |title: &str, description: &str, id: u64, value: String| {
        PageItem::Row(SettingRow {
            title: title.to_string().into(),
            description: description.to_string().into(),
            control: Control::Dropdown { id, value },
            reset: reset_if_changed(id, s),
        })
    };
    let mut items = vec![
        PageItem::Header("Theme"),
        dropdown(
            "Theme Mode",
            "Choose a static, fixed theme or dynamically select themes based on appearance and light/dark modes.",
            CTRL_THEME_SELECTION,
            cap(&s.theme_selection),
        ),
    ];
    if s.theme_selection == "dynamic" {
        items.push(dropdown(
            "Mode",
            "Choose whether to use the selected light or dark theme or to follow your OS appearance configuration.",
            CTRL_MODE,
            cap(&s.theme_mode),
        ));
        items.push(dropdown(
            "Light Theme",
            "The theme used when light mode is active.",
            CTRL_THEME_LIGHT,
            s.theme_light.clone(),
        ));
        items.push(dropdown(
            "Dark Theme",
            "The theme used when dark mode is active.",
            CTRL_THEME_DARK,
            s.theme_dark.clone(),
        ));
    } else {
        items.push(dropdown(
            "Theme Name",
            "The name of your selected theme.",
            CTRL_THEME,
            s.theme.clone(),
        ));
    }
    items.push(PageItem::Row(SettingRow {
        title: "Your Themes".into(),
        description: "Theme files in ~/.config/pomelo/themes, read again whenever they change."
            .into(),
        control: Control::Button {
            id: CTRL_OPEN_THEMES,
            label: "Open Themes Folder",
            enabled: true,
        },
        reset: None,
    }));
    for problem in ui::theme_file::theme_problems() {
        items.push(PageItem::Row(SettingRow {
            title: "Problem".into(),
            description: problem.into(),
            control: Control::Value {
                text: String::new(),
            },
            reset: None,
        }));
    }
    items
}

fn appearance_page(s: &Settings) -> Page {
    let mut items = theme_items(s);
    items.extend(buffer_font_items(s));
    items.extend(vec![
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
    ]);
    items.extend(agent_font_items(s));
    items.extend(terminal_font_items(s));
    items.extend(vec![
        PageItem::Header("Cursor"),
        PageItem::Row(SettingRow {
            title: "Multi Cursor Modifier".into(),
            description: "Modifier key for adding multiple cursors.".into(),
            control: Control::Dropdown {
                id: CTRL_MULTI_CURSOR_MODIFIER,
                value: control_value(CTRL_MULTI_CURSOR_MODIFIER, s),
            },
            reset: reset_if_changed(CTRL_MULTI_CURSOR_MODIFIER, s),
        }),
        PageItem::Row(SettingRow {
            title: "Cursor Blink".into(),
            description: "Whether the cursor blinks in the editor.".into(),
            control: Control::Toggle {
                id: CTRL_CURSOR_BLINK,
                on: s.cursor_blink,
            },
            reset: reset_if_changed(CTRL_CURSOR_BLINK, s),
        }),
        PageItem::Row(SettingRow {
            title: "Cursor Animation".into(),
            description: "Whether the cursor smoothly animates when moving around the editor.".into(),
            control: Control::Toggle {
                id: CTRL_CURSOR_ANIMATION,
                on: s.cursor_animation.enabled,
            },
            reset: reset_if_changed(CTRL_CURSOR_ANIMATION, s),
        }),
        PageItem::Row(SettingRow {
            title: "Cursor Shape".into(),
            description: "Cursor shape for the editor.".into(),
            control: Control::Dropdown {
                id: CTRL_CURSOR_SHAPE,
                value: control_value(CTRL_CURSOR_SHAPE, s),
            },
            reset: reset_if_changed(CTRL_CURSOR_SHAPE, s),
        }),
        PageItem::Row(SettingRow {
            title: "Hide Mouse".into(),
            description: "When to hide the mouse cursor.".into(),
            control: Control::Dropdown {
                id: CTRL_HIDE_MOUSE,
                value: control_value(CTRL_HIDE_MOUSE, s),
            },
            reset: reset_if_changed(CTRL_HIDE_MOUSE, s),
        }),
        PageItem::Row(SettingRow {
            title: "Reduce Motion".into(),
            description: "Whether to reduce non-essential motion, such as loading spinners, by rendering them in a static state.".into(),
            control: Control::Dropdown {
                id: CTRL_REDUCE_MOTION,
                value: control_value(CTRL_REDUCE_MOTION, s),
            },
            reset: reset_if_changed(CTRL_REDUCE_MOTION, s),
        }),
    ]);
    Page {
        title: "Appearance",
        items,
    }
}

/// Which function button's toggle `id` is.
fn function_button(id: u64) -> Option<usize> {
    let index = id.checked_sub(CTRL_SHOW_FUNCTION_BASE)? as usize;
    (index < FUNCTION_BUTTONS.len()).then_some(index)
}

fn set_function_hidden(s: &mut Settings, index: usize, hidden: bool) {
    if s.func_hidden.len() <= index {
        s.func_hidden.resize(FUNCTION_BUTTONS.len(), false);
    }
    s.func_hidden[index] = hidden;
}

/// The status bar's function buttons, in `PaneKind::ALL` order (the index into `func_hidden`).
pub const FUNCTION_BUTTONS: [&str; 4] = ["Files", "Services", "Git", "Database"];

fn window_layout_page(s: &Settings) -> Page {
    let mut items = vec![PageItem::Header("Status Bar")];
    for (index, name) in FUNCTION_BUTTONS.iter().enumerate() {
        let id = CTRL_SHOW_FUNCTION_BASE + index as u64;
        items.push(PageItem::Row(SettingRow {
            title: format!("{name} Button").into(),
            description: format!("Show the {} button in the status bar.", name.to_lowercase())
                .into(),
            control: Control::Toggle {
                id,
                on: !s.func_hidden.get(index).copied().unwrap_or(false),
            },
            reset: reset_if_changed(id, s),
        }));
    }
    Page {
        title: "Window & Layout",
        items: items
            .into_iter()
            .chain([
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
                PageItem::Row(SettingRow {
                    title: "Terminal Button".into(),
                    description: "Show the terminal button in the status bar.".into(),
                    control: Control::Toggle {
                        id: CTRL_SHOW_TERMINAL,
                        on: !s.terminal_hidden,
                    },
                    reset: reset_if_changed(CTRL_SHOW_TERMINAL, s),
                }),
                PageItem::Row(SettingRow {
                    title: "Agent Button".into(),
                    description: "Show the agent button in the status bar.".into(),
                    control: Control::Toggle {
                        id: CTRL_SHOW_AGENT,
                        on: !s.agent_hidden,
                    },
                    reset: reset_if_changed(CTRL_SHOW_AGENT, s),
                }),
                PageItem::Header("Title Bar"),
                PageItem::Row(SettingRow {
                    title: "Show Branch Name".into(),
                    description: "Show the active workspace's branch in the title bar.".into(),
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
            ])
            .collect(),
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
pub struct DevServicesPage {
    pub proxy_running: bool,
    pub webhook_running: bool,
    pub proxy_port: u16,
    pub webhook_port: u16,
    /// The app could not bind, but `pom proxy` in a terminal answers on the port.
    pub served_elsewhere: bool,
    /// `POM_WEB_PORT` overrides the configured ports.
    pub port_from_env: bool,
    /// How the node_modules store reaches project folders on this drive (empty until checked).
    pub modules_method: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeneralPage {
    pub start_at_login: bool,
    pub version: String,
    /// Only the installed app replaces itself; a dev build says so.
    pub updates_apply: bool,
    /// What the last check found (checking, up to date, downloading, or why it failed).
    pub update_note: Option<String>,
    pub update_button: Option<(&'static str, bool)>,
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
    pub dev_services: DevServicesPage,
    pub general: GeneralPage,
    pub keymap: KeymapPage,
    /// The language whose own page is open over Languages & Tools, by its index in `language_names`.
    pub language: Option<usize>,
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
            PageItem::Row(SettingRow {
                title: "Restore on Startup".into(),
                description: "Reopen each workspace's tabs from last time, or start without them. After a crash at startup they stay closed until you reopen them.".into(),
                control: Control::Dropdown {
                    id: CTRL_RESTORE_ON_STARTUP,
                    value: control_value(CTRL_RESTORE_ON_STARTUP, s),
                },
                reset: reset_if_changed(CTRL_RESTORE_ON_STARTUP, s),
            }),
            PageItem::Header("Updates"),
            PageItem::Row(SettingRow {
                title: "Check for Updates Automatically".into(),
                description: "Every hour, quietly. Nothing installs until you restart.".into(),
                control: Control::Toggle {
                    id: CTRL_AUTO_UPDATE,
                    on: s.auto_update,
                },
                reset: reset_if_changed(CTRL_AUTO_UPDATE, s),
            }),
            PageItem::Row(SettingRow {
                title: format!("Pomelo {}", general.version).into(),
                description: match (&general.update_note, general.updates_apply) {
                    (Some(note), true) => note.clone().into(),
                    (None, true) => "Looks for a newer release right away.".into(),
                    (_, false) => {
                        "Only the installed Pomelo updates itself; this build does not.".into()
                    }
                },
                control: Control::Button {
                    id: CTRL_CHECK_UPDATES,
                    label: general
                        .update_button
                        .map_or("Check Now", |(label, _)| label),
                    enabled: general.updates_apply
                        && general.update_button.is_none_or(|(_, enabled)| enabled),
                },
                reset: None,
            }),
        ],
    }
}

/// The editor's font: its rows sit under Appearance with the other fonts.
fn buffer_font_items(s: &Settings) -> Vec<PageItem> {
    vec![PageItem::Header("Buffer Font")]
        .into_iter()
        .chain(font_rows(
            s,
            FontRows {
                family: CTRL_BUFFER_FAMILY,
                family_value: s.buffer_font_family.clone(),
                size: (
                    CTRL_BUFFER_FONT_DEC,
                    CTRL_BUFFER_FONT_INC,
                    CTRL_BUFFER_FONT_EDIT,
                ),
                size_value: s.buffer_font_size,
                size_description: "Text size of the code editor.",
                weight: (
                    CTRL_BUFFER_WEIGHT_DEC,
                    CTRL_BUFFER_WEIGHT_INC,
                    CTRL_BUFFER_WEIGHT_EDIT,
                ),
                weight_value: s.buffer_font_weight,
                line_height: Some((
                    CTRL_BUFFER_LINE_HEIGHT,
                    line_height_label(&s.buffer_line_height),
                )),
                features: CTRL_BUFFER_FEATURES,
                fallbacks: CTRL_BUFFER_FALLBACKS,
                what: "the code editor",
            },
        ))
        .collect()
}

fn editor_page(s: &Settings) -> Page {
    Page {
        title: "Editor",
        items: vec![
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

fn toggle_row(
    title: &'static str,
    description: &'static str,
    id: u64,
    on: bool,
    s: &Settings,
) -> PageItem {
    PageItem::Row(SettingRow {
        title: title.into(),
        description: description.into(),
        control: Control::Toggle { id, on },
        reset: reset_if_changed(id, s),
    })
}

fn stepper_row(
    title: &'static str,
    description: &'static str,
    (dec, inc, edit): (u64, u64, u64),
    value: u64,
    s: &Settings,
) -> PageItem {
    PageItem::Row(SettingRow {
        title: title.into(),
        description: description.into(),
        control: Control::Stepper {
            dec,
            inc,
            edit,
            value: value.to_string(),
        },
        reset: reset_if_changed(edit, s),
    })
}

const ENABLE_LANGUAGE_SERVER: (&str, &str) = (
    "Enable Language Server",
    "Whether to use language servers to provide code intelligence.",
);
const LANGUAGE_SERVERS_ROW: (&str, &str) = (
    "Language Servers",
    "The list of language servers to use (or disable) for this language.",
);
const LSP_COMPLETIONS_ROW: (&str, &str) = ("Enabled", "Whether to fetch LSP completions or not.");
const COMPLETION_TIMEOUT_ROW: (&str, &str) = (
    "Fetch Timeout (milliseconds)",
    "When fetching LSP completions, determines how long to wait for a response of a particular server (set to 0 to wait indefinitely).",
);

fn languages_tools_page(s: &Settings) -> Page {
    let inline = &s.diagnostics.inline;
    let mut items = vec![
        PageItem::Header("LSP"),
        toggle_row(
            ENABLE_LANGUAGE_SERVER.0,
            ENABLE_LANGUAGE_SERVER.1,
            CTRL_ENABLE_LANGUAGE_SERVER,
            s.enable_language_server,
            s,
        ),
        PageItem::Row(SettingRow {
            title: LANGUAGE_SERVERS_ROW.0.into(),
            description: LANGUAGE_SERVERS_ROW.1.into(),
            control: Control::EditInJson {
                id: CTRL_LANGUAGE_SERVERS,
            },
            reset: None,
        }),
        PageItem::Row(SettingRow {
            title: "Go To Definition Scroll Strategy".into(),
            description:
                "How to scroll the target into view when navigating to a definition or reference."
                    .into(),
            control: Control::Dropdown {
                id: CTRL_DEFINITION_SCROLL,
                value: control_value(CTRL_DEFINITION_SCROLL, s),
            },
            reset: reset_if_changed(CTRL_DEFINITION_SCROLL, s),
        }),
        PageItem::Header("LSP Completions"),
        toggle_row(
            LSP_COMPLETIONS_ROW.0,
            LSP_COMPLETIONS_ROW.1,
            CTRL_LSP_COMPLETIONS,
            s.completions.lsp,
            s,
        ),
        stepper_row(
            COMPLETION_TIMEOUT_ROW.0,
            COMPLETION_TIMEOUT_ROW.1,
            (
                CTRL_COMPLETION_TIMEOUT_DEC,
                CTRL_COMPLETION_TIMEOUT_INC,
                CTRL_COMPLETION_TIMEOUT_EDIT,
            ),
            s.completions.lsp_fetch_timeout_ms,
            s,
        ),
        PageItem::Header("File Types"),
        PageItem::Row(SettingRow {
            title: "File Type Associations".into(),
            description: "A mapping from languages to files and file extensions that should be treated as that language.".into(),
            control: Control::EditInJson { id: CTRL_FILE_TYPES },
            reset: None,
        }),
        PageItem::Header("Diagnostics"),
        PageItem::Row(SettingRow {
            title: "Max Severity".into(),
            description: "Which level to use to filter out diagnostics displayed in the editor."
                .into(),
            control: Control::Dropdown {
                id: CTRL_DIAGNOSTICS_SEVERITY,
                value: control_value(CTRL_DIAGNOSTICS_SEVERITY, s),
            },
            reset: reset_if_changed(CTRL_DIAGNOSTICS_SEVERITY, s),
        }),
        PageItem::Header("Inline Diagnostics"),
        toggle_row(
            "Enabled",
            "Whether to show diagnostics inline or not.",
            CTRL_INLINE_DIAGNOSTICS,
            inline.enabled,
            s,
        ),
        stepper_row(
            "Padding",
            "The amount of padding between the end of the source line and the start of the inline diagnostic.",
            (
                CTRL_INLINE_PADDING_DEC,
                CTRL_INLINE_PADDING_INC,
                CTRL_INLINE_PADDING_EDIT,
            ),
            u64::from(inline.padding),
            s,
        ),
        stepper_row(
            "Minimum Column",
            "The minimum column at which to display inline diagnostics.",
            (
                CTRL_INLINE_COLUMN_DEC,
                CTRL_INLINE_COLUMN_INC,
                CTRL_INLINE_COLUMN_EDIT,
            ),
            u64::from(inline.min_column),
            s,
        ),
        PageItem::Header("Languages"),
    ];
    items.extend(
        language_names()
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                PageItem::Row(SettingRow {
                    title: name.into(),
                    description: "".into(),
                    control: Control::SubPage {
                        id: LANGUAGE_OPEN_BASE + index as u64,
                    },
                    reset: None,
                })
            }),
    );
    Page {
        title: "Languages & Tools",
        items,
    }
}

/// One language's page: what it sets over the shared language-server settings.
fn language_page(s: &Settings, index: usize, name: &str) -> Page {
    let chosen = s.language_server_settings(name);
    let id = |field| language_control_id(index, field);
    Page {
        title: "Languages & Tools",
        items: vec![
            PageItem::Header("LSP"),
            toggle_row(
                ENABLE_LANGUAGE_SERVER.0,
                ENABLE_LANGUAGE_SERVER.1,
                id(LANGUAGE_ENABLE),
                chosen.enabled,
                s,
            ),
            PageItem::Row(SettingRow {
                title: LANGUAGE_SERVERS_ROW.0.into(),
                description: LANGUAGE_SERVERS_ROW.1.into(),
                control: Control::EditInJson {
                    id: id(LANGUAGE_SERVERS),
                },
                reset: reset_if_changed(id(LANGUAGE_SERVERS), s),
            }),
            PageItem::Header("LSP Completions"),
            toggle_row(
                LSP_COMPLETIONS_ROW.0,
                LSP_COMPLETIONS_ROW.1,
                id(LANGUAGE_COMPLETIONS),
                chosen.completions,
                s,
            ),
            stepper_row(
                COMPLETION_TIMEOUT_ROW.0,
                COMPLETION_TIMEOUT_ROW.1,
                (
                    id(LANGUAGE_TIMEOUT_DEC),
                    id(LANGUAGE_TIMEOUT_INC),
                    id(LANGUAGE_TIMEOUT_EDIT),
                ),
                chosen.completion_timeout_ms,
                s,
            ),
        ],
    }
}

/// The header over a page opened from another: a back button, then where it sits, muted.
fn sub_page_header(trail: &[&str]) -> Node {
    let mut crumbs = div().row().items_center().gap(4.0);
    for (index, part) in trail.iter().enumerate() {
        if index > 0 {
            crumbs = crumbs.child(label("/").color(dim_c()));
        }
        crumbs = crumbs.child(label(part.to_string()).color(dim_c()));
    }
    div()
        .row()
        .h_px(30.0)
        .items_center()
        .justify_between()
        .child(
            div()
                .row()
                .items_center()
                .gap(4.0)
                .child(
                    div()
                        .w_px(22.0)
                        .h_px(22.0)
                        .rounded(4.0)
                        .items_center()
                        .justify_center()
                        .on_click(CTRL_LANGUAGE_BACK)
                        .child(
                            ui::icon(ui::IconKind::ArrowLeft)
                                .size(14.0)
                                .color(title_c()),
                        ),
                )
                .child(crumbs),
        )
        .child(ui::button_sized(
            CTRL_OPEN_JSON,
            "Edit in settings.json",
            ButtonStyle::OutlinedGhost,
            ui::ButtonSize::Default,
        ))
        .into()
}

/// The terminal's font: its rows sit under Appearance with the other fonts.
fn terminal_font_items(s: &Settings) -> Vec<PageItem> {
    vec![PageItem::Header("Terminal Font")]
        .into_iter()
        .chain(font_rows(
            s,
            FontRows {
                family: CTRL_TERM_FAMILY,
                family_value: s.terminal_font_family.clone(),
                size: (CTRL_TERM_FONT_DEC, CTRL_TERM_FONT_INC, CTRL_TERM_FONT_EDIT),
                size_value: s.terminal_font_size,
                size_description:
                    "Text size of terminals (the agent has its own, under Agent Panel Font).",
                weight: (
                    CTRL_TERM_WEIGHT_DEC,
                    CTRL_TERM_WEIGHT_INC,
                    CTRL_TERM_WEIGHT_EDIT,
                ),
                weight_value: s.terminal_font_weight,
                line_height: Some((
                    CTRL_TERM_LINE_HEIGHT,
                    line_height_label(&s.terminal_line_height),
                )),
                features: CTRL_TERM_FEATURES,
                fallbacks: CTRL_TERM_FALLBACKS,
                what: "terminals",
            },
        ))
        .collect()
}

fn terminal_page(s: &Settings) -> Page {
    Page {
        title: "Terminal",
        items: vec![
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
            control: if binding.is_empty() {
                Control::Value {
                    text: "Unbound".to_string(),
                }
            } else {
                Control::Keys {
                    binding: binding.clone(),
                }
            },
            reset: None,
        }));
    }
    Page {
        title: "Keymap",
        items,
    }
}

/// The agent's tabs' text size, apart from terminals.
fn agent_font_items(s: &Settings) -> Vec<PageItem> {
    vec![
        PageItem::Header("Agent Panel Font"),
        PageItem::Row(SettingRow {
            title: "Font Size".into(),
            description: "Text size of the agent's tabs, apart from terminals.".into(),
            control: Control::Stepper {
                dec: CTRL_AGENT_FONT_DEC,
                inc: CTRL_AGENT_FONT_INC,
                edit: CTRL_AGENT_FONT_EDIT,
                value: format!("{:.0}", s.agent_font_size),
            },
            reset: reset_if_changed(CTRL_AGENT_FONT_EDIT, s),
        }),
    ]
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
            PageItem::Row(SettingRow {
                title: "Closing an Agent Tab".into(),
                description: "Keep the agent running in the background, or stop it. Stop Agent in a tab's right-click menu always stops it.".into(),
                control: Control::Dropdown {
                    id: CTRL_AGENT_TAB_CLOSE,
                    value: control_value(CTRL_AGENT_TAB_CLOSE, s),
                },
                reset: reset_if_changed(CTRL_AGENT_TAB_CLOSE, s),
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

fn dev_services_page(s: &Settings, services: &DevServicesPage) -> Page {
    let status = |enabled: bool, running: bool| {
        if enabled {
            Control::Status {
                running: running || services.served_elsewhere,
            }
        } else {
            Control::Value { text: "Off".into() }
        }
    };
    let env_note = if services.port_from_env {
        " POM_WEB_PORT is set, so the ports below are ignored."
    } else {
        ""
    };
    let proxy_status = if services.served_elsewhere && !services.proxy_running {
        "Served by `pom proxy` in a terminal; its requests are not listed in Dev Requests."
            .to_string()
    } else {
        format!("Serves every workspace's services behind one port.{env_note}")
    };
    let mut items = vec![
        PageItem::Header("Reverse Proxy"),
        PageItem::Row(SettingRow {
            title: "Enabled".into(),
            description: "Run the reverse proxy while a project is open.".into(),
            control: Control::Toggle {
                id: CTRL_PROXY_ENABLED,
                on: s.dev_proxy_enabled,
            },
            reset: reset_if_changed(CTRL_PROXY_ENABLED, s),
        }),
        PageItem::Row(SettingRow {
            title: "Status".into(),
            description: proxy_status.into(),
            control: status(s.dev_proxy_enabled, services.proxy_running),
            reset: None,
        }),
        PageItem::Row(SettingRow {
            title: "Port".into(),
            description: format!(
                "Open a service at <service>.<repo>.<ticket or branch>.localhost:{}. Service URLs in env files use it too; restart running services after changing it.",
                services.proxy_port
            )
            .into(),
            control: Control::Stepper {
                dec: CTRL_PROXY_PORT_DEC,
                inc: CTRL_PROXY_PORT_INC,
                edit: CTRL_PROXY_PORT_EDIT,
                value: s.dev_proxy_port.to_string(),
            },
            reset: reset_if_changed(CTRL_PROXY_PORT_EDIT, s),
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
            title: "Bind Address".into(),
            description: "Only this machine can reach the proxy.".into(),
            control: Control::Value {
                text: "127.0.0.1".into(),
            },
            reset: None,
        }),
        PageItem::Header("Webhook Fan-out"),
        PageItem::Row(SettingRow {
            title: "Enabled".into(),
            description: "Run the webhook relay while a project is open.".into(),
            control: Control::Toggle {
                id: CTRL_WEBHOOK_ENABLED,
                on: s.webhook_enabled,
            },
            reset: reset_if_changed(CTRL_WEBHOOK_ENABLED, s),
        }),
        PageItem::Row(SettingRow {
            title: "Status".into(),
            description: "Hands each incoming webhook to every workspace running the service.".into(),
            control: status(s.webhook_enabled, services.webhook_running),
            reset: None,
        }),
        PageItem::Row(SettingRow {
            title: "Port".into(),
            description: format!(
                "Point an external webhook (Stripe, GitHub...) at localhost:{}/<repo>/<service>.",
                services.webhook_port
            )
            .into(),
            control: Control::Stepper {
                dec: CTRL_WEBHOOK_PORT_DEC,
                inc: CTRL_WEBHOOK_PORT_INC,
                edit: CTRL_WEBHOOK_PORT_EDIT,
                value: s.webhook_port.to_string(),
            },
            reset: reset_if_changed(CTRL_WEBHOOK_PORT_EDIT, s),
        }),
        PageItem::Header("Shared node_modules"),
        PageItem::Row(SettingRow {
            title: "Enabled".into(),
            description: "A new workspace whose lockfile matches a stored one takes its node_modules instead of installing. pnpm and Yarn's hard-link or Plug'n'Play modes share packages themselves and are left alone.".into(),
            control: Control::Toggle {
                id: CTRL_MODULES_ENABLED,
                on: s.modules_store_enabled,
            },
            reset: reset_if_changed(CTRL_MODULES_ENABLED, s),
        }),
        PageItem::Row(SettingRow {
            title: "Import Method".into(),
            description: "What this drive allows between the store and your projects: a copy-on-write clone takes no extra space.".into(),
            control: Control::Value {
                text: if services.modules_method.is_empty() {
                    "-".into()
                } else {
                    services.modules_method.clone()
                },
            },
            reset: None,
        }),
        PageItem::Row(SettingRow {
            title: "When Cloning Is Unsupported".into(),
            description: "Hard links share the files (they are made read-only, so a tool editing one in place fails instead of changing it for every workspace); a copy takes the full size again; Run Install skips the store.".into(),
            control: Control::Dropdown {
                id: CTRL_MODULES_FALLBACK,
                value: control_value(CTRL_MODULES_FALLBACK, s),
            },
            reset: reset_if_changed(CTRL_MODULES_FALLBACK, s),
        }),
        PageItem::Row(SettingRow {
            title: "Size Limit".into(),
            description: "GB kept at most; the least recently used copies go first.".into(),
            control: Control::Stepper {
                dec: CTRL_MODULES_LIMIT_DEC,
                inc: CTRL_MODULES_LIMIT_INC,
                edit: CTRL_MODULES_LIMIT_EDIT,
                value: s.modules_size_limit_gb.to_string(),
            },
            reset: reset_if_changed(CTRL_MODULES_LIMIT_EDIT, s),
        }),
        PageItem::Row(SettingRow {
            title: "Remove Unused After".into(),
            description: "Days a copy is kept without a workspace taking it (0 keeps them until the size limit).".into(),
            control: Control::Stepper {
                dec: CTRL_MODULES_DAYS_DEC,
                inc: CTRL_MODULES_DAYS_INC,
                edit: CTRL_MODULES_DAYS_EDIT,
                value: s.modules_unused_days.to_string(),
            },
            reset: reset_if_changed(CTRL_MODULES_DAYS_EDIT, s),
        }),
        PageItem::Row(SettingRow {
            title: "Stored Copies".into(),
            description: "See each copy, its size and the workspaces using it; delete or prune them.".into(),
            control: Control::Buttons(vec![(CTRL_OPEN_STORE, "Open Store")]),
            reset: None,
        }),
        PageItem::Header("Servers"),
    ];
    let mut actions = Vec::new();
    if !services.served_elsewhere {
        actions.push((CTRL_START_SERVERS, "Restart"));
    }
    actions.push((CTRL_OPEN_REQUESTS, "Open Requests"));
    let description = if services.served_elsewhere {
        "See each request and webhook delivery in a tab."
    } else {
        "Restart binds both ports again (after freeing one another app held). Open Requests lists each request and webhook delivery in a tab."
    };
    items.push(PageItem::Row(SettingRow {
        title: "Servers".into(),
        description: description.into(),
        control: Control::Buttons(actions),
        reset: None,
    }));
    Page {
        title: "Dev Services",
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
            PageItem::Row(SettingRow {
                title: "Group Workspaces by Ticket Status".into(),
                description: "The WORKSPACES list is split into In progress, In review, Backlog, Done and Other. Click a group to fold it, drag it to move it.".into(),
                control: Control::Toggle {
                    id: CTRL_GROUP_WORKSPACES,
                    on: s.group_workspaces,
                },
                reset: reset_if_changed(CTRL_GROUP_WORKSPACES, s),
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
                title: "Config File".into(),
                description: "The project's pom.yml. Each save is checked first and reloads at once, even from main.".into(),
                control: Control::Button {
                    id: CTRL_EDIT_PROJECT_CONFIG,
                    label: "Edit...",
                    enabled: true,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Clone Missing Repos".into(),
                description: "Clone into main the repos the config names but main does not have yet (a teammate added them, or the config came from a bundle).".into(),
                control: Control::Button {
                    id: CTRL_APPLY_CONFIG,
                    label: "Clone...",
                    enabled: true,
                },
                reset: None,
            }),
            PageItem::Header("Config Bundle"),
            PageItem::Row(SettingRow {
                title: "Export".into(),
                description: "Save this project's config as YAML, or with its secrets sealed under a password, to hand to a teammate.".into(),
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
    let heading = label(page.title)
        .label_size(LabelSize::Large)
        .color(title_c())
        .into();
    render_page_under(heading, page, query, editing, w)
}

fn render_page_under(
    heading: Node,
    page: &Page,
    query: &str,
    editing: Option<(u64, &str)>,
    w: f32,
) -> Node {
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

    let mut col = div().col().child(heading).child(div().h_px(16.0));

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
                    Control::Keys { binding } => keys(binding),
                    Control::SubPage { id } => sub_page_button(*id),
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

fn keys(binding: &str) -> Node {
    let mut row = div().row().items_center().gap(6.0);
    for stroke in binding.split_whitespace() {
        row = row.child(ui::render_keystroke(stroke, 13.0));
    }
    row.into()
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
    let mut left = div().col().w_px(left_col_w).gap(5.0).child(title_row);
    if !desc.is_empty() {
        left = left.child(description_node(desc, left_col_w));
    }
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

fn sub_page_button(id: u64) -> Node {
    ui::button_sized(
        id,
        "Configure",
        ui::ButtonStyle::OutlinedGhost,
        ui::ButtonSize::Medium,
    )
    .child(
        ui::icon(ui::IconKind::ChevronRight)
            .size(14.0)
            .color(dim_c()),
    )
    .into()
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
    fn a_language_page_sets_its_own_values_and_reset_hands_them_back() {
        let mut s = Settings::default();
        let rust = language_names()
            .iter()
            .position(|name| *name == "Rust")
            .expect("Rust is listed");
        assert_eq!(
            language_to_open(LANGUAGE_OPEN_BASE + rust as u64),
            Some(rust)
        );
        let enable = language_control_id(rust, LANGUAGE_ENABLE);
        assert!(is_default(enable, &s));
        assert!(handle_control(enable, &mut s));
        assert!(!s.language_server_settings("Rust").enabled);
        assert!(
            s.language_server_settings("Go").enabled,
            "other languages keep the shared value"
        );
        assert!(!is_default(enable, &s));

        let timeout = language_control_id(rust, LANGUAGE_TIMEOUT_EDIT);
        assert!(set_language_number(timeout, 250.0, &mut s));
        assert_eq!(language_number(timeout, &s).as_deref(), Some("250"));
        assert!(reset_to_default(timeout, &mut s));
        assert!(reset_to_default(enable, &mut s));
        assert!(s.languages.is_empty(), "nothing set leaves no entry behind");
        assert!(is_edit_in_json(language_control_id(rust, LANGUAGE_SERVERS)));
    }

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
    fn nav_jump_ids_never_collide_with_category_toggles() {
        for (category, (_, subs)) in CATEGORIES.iter().enumerate() {
            assert!(subs.len() as u64 <= NAV_JUMP_STRIDE, "{category}");
        }
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
            .filter(|t| t.font.is_mono())
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

    fn dev_services_panel(settings: &Settings, services: DevServicesPage) -> ui::Painted {
        let state = PageState {
            dev_services: services,
            ..PageState::default()
        };
        panel(
            DEV_SERVICES,
            None,
            &[false; CATEGORY_COUNT],
            settings,
            &state,
            1400.0,
            2400.0,
            None,
            "",
            false,
        )
    }

    #[test]
    fn dev_services_shows_status_ports_and_the_server_actions() {
        let p = dev_services_panel(
            &Settings::default(),
            DevServicesPage {
                proxy_running: true,
                webhook_running: false,
                proxy_port: 8767,
                webhook_port: 8766,
                ..DevServicesPage::default()
            },
        );
        for expected in [
            "Running",
            "Stopped",
            "8767",
            "8766",
            "Restart",
            "Open Requests",
        ] {
            assert!(p.texts.iter().any(|t| t.text == expected), "{expected}");
        }
        for id in [
            CTRL_START_SERVERS,
            CTRL_OPEN_REQUESTS,
            CTRL_PROXY_ENABLED,
            CTRL_WEBHOOK_PORT_INC,
        ] {
            assert!(p.hits.iter().any(|(_, hit)| *hit == id), "{id}");
        }
    }

    #[test]
    fn a_turned_off_server_reads_off_and_a_terminal_proxy_needs_no_restart() {
        let settings = Settings {
            webhook_enabled: false,
            ..Settings::default()
        };
        let p = dev_services_panel(
            &settings,
            DevServicesPage {
                served_elsewhere: true,
                proxy_port: 8767,
                webhook_port: 8766,
                ..DevServicesPage::default()
            },
        );
        assert!(p.texts.iter().any(|t| t.text == "Off"));
        assert!(!p.texts.iter().any(|t| t.text == "Stopped"));
        assert!(!p.hits.iter().any(|(_, id)| *id == CTRL_START_SERVERS));
        assert!(p.hits.iter().any(|(_, id)| *id == CTRL_OPEN_REQUESTS));
    }

    #[test]
    fn ports_step_toggle_and_reset() {
        let mut s = Settings::default();
        assert!(handle_control(CTRL_PROXY_PORT_INC, &mut s));
        assert_eq!(s.dev_proxy_port, 8768);
        assert!(!is_default(CTRL_PROXY_PORT_EDIT, &s));
        assert!(reset_to_default(CTRL_PROXY_PORT_EDIT, &mut s));
        assert_eq!(s.dev_proxy_port, 8767);
        s.webhook_port = PORT_MIN;
        assert!(!handle_control(CTRL_WEBHOOK_PORT_DEC, &mut s));
        assert!(handle_control(CTRL_WEBHOOK_ENABLED, &mut s));
        assert!(!s.webhook_enabled);
    }

    #[test]
    fn the_cursor_rows_edit_their_settings() {
        let mut s = Settings::default();
        assert_eq!(control_value(CTRL_CURSOR_SHAPE, &s), "Bar");
        assert!(apply_choice(CTRL_CURSOR_SHAPE, 3, &[], &mut s));
        assert_eq!(s.cursor_shape, "hollow");
        assert!(apply_choice(CTRL_MULTI_CURSOR_MODIFIER, 1, &[], &mut s));
        assert_eq!(s.multi_cursor_modifier, "cmd_or_ctrl");
        assert!(apply_choice(CTRL_REDUCE_MOTION, 1, &[], &mut s));
        assert_eq!(s.reduce_motion, "on");
        assert!(handle_control(CTRL_CURSOR_BLINK, &mut s));
        assert!(!s.cursor_blink);
        assert!(handle_control(CTRL_CURSOR_ANIMATION, &mut s));
        assert!(s.cursor_animation.enabled);
        for id in [
            CTRL_CURSOR_SHAPE,
            CTRL_MULTI_CURSOR_MODIFIER,
            CTRL_REDUCE_MOTION,
            CTRL_CURSOR_BLINK,
            CTRL_CURSOR_ANIMATION,
        ] {
            assert!(reset_to_default(id, &mut s));
        }
        assert_eq!(s, Settings::default());
        let saved: Settings =
            serde_json::from_str(r#"{"cursor_animation": {"enabled": true}}"#).expect("json");
        assert!(saved.cursor_animation.enabled);
    }

    #[test]
    fn each_status_bar_button_has_a_toggle() {
        let mut s = Settings::default();
        let git = CTRL_SHOW_FUNCTION_BASE + 2;
        assert!(handle_control(git, &mut s));
        assert_eq!(s.func_hidden.get(2), Some(&true));
        assert!(reset_if_changed(git, &s).is_some());
        assert!(handle_control(CTRL_SHOW_TERMINAL, &mut s));
        assert!(s.terminal_hidden);
        assert!(reset_to_default(git, &mut s));
        assert_eq!(s.func_hidden.get(2), Some(&false));
        assert_eq!(
            function_button(CTRL_SHOW_FUNCTION_BASE + FUNCTION_BUTTONS.len() as u64),
            None
        );
    }

    #[test]
    fn hide_mouse_picks_when_the_pointer_hides() {
        let mut s = Settings::default();
        assert_eq!(control_value(CTRL_HIDE_MOUSE, &s), "On Typing and Action");
        assert!(apply_choice(CTRL_HIDE_MOUSE, 0, &[], &mut s));
        assert_eq!(s.hide_mouse, "never");
        assert!(reset_to_default(CTRL_HIDE_MOUSE, &mut s));
        assert_eq!(s.hide_mouse, "on_typing_and_action");
    }

    #[test]
    fn closing_an_agent_tab_keeps_it_running_unless_set_to_stop() {
        let mut s = Settings::default();
        assert_eq!(control_value(CTRL_RESTORE_ON_STARTUP, &s), "Last Session");
        assert!(apply_choice(CTRL_RESTORE_ON_STARTUP, 1, &[], &mut s));
        assert_eq!(s.restore_on_startup, "none");
        assert!(reset_to_default(CTRL_RESTORE_ON_STARTUP, &mut s));
        assert_eq!(s.restore_on_startup, "last_session");
        assert_eq!(control_value(CTRL_AGENT_TAB_CLOSE, &s), "Keep It Running");
        assert!(apply_choice(CTRL_AGENT_TAB_CLOSE, 1, &[], &mut s));
        assert_eq!(s.agent_tab_close, "stop");
        assert!(reset_to_default(CTRL_AGENT_TAB_CLOSE, &mut s));
        assert_eq!(s.agent_tab_close, "hide");
    }

    #[test]
    fn the_store_rows_edit_their_settings() {
        let mut s = Settings::default();
        assert_eq!(control_value(CTRL_MODULES_FALLBACK, &s), "Hard Links");
        assert!(apply_choice(CTRL_MODULES_FALLBACK, 2, &[], &mut s));
        assert_eq!(s.modules_fallback, "install");
        assert!(!is_default(CTRL_MODULES_FALLBACK, &s));
        assert!(handle_control(CTRL_MODULES_LIMIT_INC, &mut s));
        assert_eq!(s.modules_size_limit_gb, 25);
        s.modules_unused_days = 0;
        assert!(!handle_control(CTRL_MODULES_DAYS_DEC, &mut s));
        assert!(handle_control(CTRL_MODULES_ENABLED, &mut s));
        assert!(!s.modules_store_enabled);
        let p = dev_services_panel(
            &Settings::default(),
            DevServicesPage {
                modules_method: "Copy-on-write clone".into(),
                ..DevServicesPage::default()
            },
        );
        assert!(p.texts.iter().any(|t| t.text == "Copy-on-write clone"));
        assert!(p.hits.iter().any(|(_, id)| *id == CTRL_OPEN_STORE));
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
                update_note: None,
                update_button: None,
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
        for expected in [
            "Start at Login",
            "Pomelo 0.9.0",
            "Check for Updates Automatically",
        ] {
            assert!(general.iter().any(|t| t.text == expected), "{expected}");
        }
        let editor = texts_of(EDITOR, &state, &settings);
        for expected in ["None", "Split", "Auto"] {
            assert!(editor.iter().any(|t| t.text == expected), "{expected}");
        }
        // Every font sits under Appearance, as in the reference.
        let appearance = texts_of(1, &state, &settings);
        for expected in [
            "Buffer Font",
            "UI Font",
            "Agent Panel Font",
            "Terminal Font",
            "Cursor",
        ] {
            assert!(appearance.iter().any(|t| t.text == expected), "{expected}");
        }
        assert!(!editor.iter().any(|t| t.text == "Buffer Font"));
        let terminal = texts_of(TERMINAL, &state, &settings);
        assert!(terminal.iter().any(|t| t.text == "10000"));
        let keymap = texts_of(KEYMAP, &state, &settings);
        assert!(
            keymap.iter().any(|t| t.text == "G")
                && !keymap.iter().any(|t| t.text == "ctrl-shift-g"),
            "a binding is drawn as key glyphs, not its text"
        );
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
