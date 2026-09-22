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

const CATEGORIES: [(&str, &[&str]); 5] = [
    ("General", &[]),
    ("Appearance", &APPEARANCE_SECTIONS),
    ("Window & Layout", &WINDOW_LAYOUT_SECTIONS),
    ("Editor", &[]),
    ("Terminal", &[]),
];

pub const WINDOW_LAYOUT: usize = 2;

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
    )
}

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
        _ => String::new(),
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
        _ => return false,
    }
    true
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
        CTRL_WIN_W_EDIT => s.window_width == d.window_width,
        CTRL_WIN_H_EDIT => s.window_height == d.window_height,
        _ => true,
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
        CTRL_WIN_W_EDIT => s.window_width = d.window_width,
        CTRL_WIN_H_EDIT => s.window_height = d.window_height,
        _ => return false,
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
pub fn page(
    selected: usize,
    s: &Settings,
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
    let tree: Node = if selected == APPEARANCE {
        render_page(&appearance_page(s), search, editing, w)
    } else if selected == WINDOW_LAYOUT {
        render_page(&window_layout_page(s), search, editing, w)
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
    w: f32,
    h: f32,
    editing: Option<(u64, &str)>,
    search: &str,
    search_active: bool,
) -> ui::Painted {
    let (mut out, _) = page(selected, s, editing, search, w, h, 0.0);
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
}

struct SettingRow {
    title: &'static str,
    description: &'static str,
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
                title: "Theme",
                description: "Color theme applied across the whole app.",
                control: Control::Dropdown {
                    id: CTRL_THEME,
                    value: s.theme.clone(),
                },
                reset: reset_if_changed(CTRL_THEME, s),
            }),
            PageItem::Header("UI Font"),
            PageItem::Row(SettingRow {
                title: "Font Family",
                description: "Font family used for interface text.",
                control: Control::Dropdown {
                    id: CTRL_FONT_FAMILY,
                    value: s.ui_font.clone(),
                },
                reset: reset_if_changed(CTRL_FONT_FAMILY, s),
            }),
            PageItem::Row(SettingRow {
                title: "Font Size",
                description: "Font size for UI elements.",
                control: Control::Stepper {
                    dec: CTRL_FONT_SIZE_DEC,
                    inc: CTRL_FONT_SIZE_INC,
                    edit: CTRL_FONT_SIZE_EDIT,
                    value: format!("{:.0}", s.ui_font_size),
                },
                reset: reset_if_changed(CTRL_FONT_SIZE_EDIT, s),
            }),
            PageItem::Row(SettingRow {
                title: "Font Weight",
                description: "Font weight for UI elements (100-900).",
                control: Control::Stepper {
                    dec: CTRL_FONT_WEIGHT_DEC,
                    inc: CTRL_FONT_WEIGHT_INC,
                    edit: CTRL_FONT_WEIGHT_EDIT,
                    value: format!("{:.0}", s.ui_font_weight),
                },
                reset: reset_if_changed(CTRL_FONT_WEIGHT_EDIT, s),
            }),
            PageItem::Row(SettingRow {
                title: "Font Features",
                description: "The OpenType features to enable for rendering in UI elements.",
                control: Control::EditInJson {
                    id: CTRL_FONT_FEATURES,
                },
                reset: None,
            }),
            PageItem::Row(SettingRow {
                title: "Font Fallbacks",
                description: "The font fallbacks to use for rendering in the UI.",
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
                title: "Show Diagnostics",
                description: "Show the error/warning count in the status bar.",
                control: Control::Toggle {
                    id: CTRL_SHOW_DIAGNOSTICS,
                    on: s.show_diagnostics,
                },
                reset: reset_if_changed(CTRL_SHOW_DIAGNOSTICS, s),
            }),
            PageItem::Row(SettingRow {
                title: "Show Cursor Position",
                description: "Show the line and column of the cursor in the status bar.",
                control: Control::Toggle {
                    id: CTRL_SHOW_CURSOR,
                    on: s.show_cursor_position,
                },
                reset: reset_if_changed(CTRL_SHOW_CURSOR, s),
            }),
            PageItem::Row(SettingRow {
                title: "Show Language",
                description: "Show the active language of the editor in the status bar.",
                control: Control::Toggle {
                    id: CTRL_SHOW_LANGUAGE,
                    on: s.show_language,
                },
                reset: reset_if_changed(CTRL_SHOW_LANGUAGE, s),
            }),
            PageItem::Header("Title Bar"),
            PageItem::Row(SettingRow {
                title: "Show Branch",
                description: "Show the current git branch in the title bar.",
                control: Control::Toggle {
                    id: CTRL_SHOW_BRANCH,
                    on: s.show_branch,
                },
                reset: reset_if_changed(CTRL_SHOW_BRANCH, s),
            }),
            PageItem::Row(SettingRow {
                title: "Show Session Name",
                description: "Show the current session name in the title bar.",
                control: Control::Toggle {
                    id: CTRL_SHOW_SESSION,
                    on: s.show_session_name,
                },
                reset: reset_if_changed(CTRL_SHOW_SESSION, s),
            }),
            PageItem::Header("Window"),
            PageItem::Row(SettingRow {
                title: "Window Width",
                description: "Default width (px) of a new window.",
                control: Control::Stepper {
                    dec: CTRL_WIN_W_DEC,
                    inc: CTRL_WIN_W_INC,
                    edit: CTRL_WIN_W_EDIT,
                    value: format!("{:.0}", s.window_width),
                },
                reset: reset_if_changed(CTRL_WIN_W_EDIT, s),
            }),
            PageItem::Row(SettingRow {
                title: "Window Height",
                description: "Default height (px) of a new window.",
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
                title: "Sidebar Side",
                description: "Which side the WORKSPACES sidebar docks on.",
                control: Control::Dropdown {
                    id: CTRL_SIDEBAR_SIDE,
                    value: cap(&s.sidebar_side),
                },
                reset: reset_if_changed(CTRL_SIDEBAR_SIDE, s),
            }),
            PageItem::Row(SettingRow {
                title: "Agent Dock Side",
                description: "Which side of the editor the agent dock renders on.",
                control: Control::Dropdown {
                    id: CTRL_AGENT_SIDE,
                    value: cap(&s.agent_side),
                },
                reset: reset_if_changed(CTRL_AGENT_SIDE, s),
            }),
            PageItem::Row(SettingRow {
                title: "Terminal Dock Side",
                description: "Which content area the terminal renders in.",
                control: Control::Dropdown {
                    id: CTRL_TERMINAL_SIDE,
                    value: cap(&s.terminal_side),
                },
                reset: reset_if_changed(CTRL_TERMINAL_SIDE, s),
            }),
            PageItem::Header("Panels"),
            PageItem::Row(SettingRow {
                title: "Show Agent Button",
                description: "Show the agent toggle in the status bar.",
                control: Control::Toggle {
                    id: CTRL_SHOW_AGENT,
                    on: !s.agent_hidden,
                },
                reset: reset_if_changed(CTRL_SHOW_AGENT, s),
            }),
            PageItem::Row(SettingRow {
                title: "Show Terminal Button",
                description: "Show the terminal toggle in the status bar.",
                control: Control::Toggle {
                    id: CTRL_SHOW_TERMINAL,
                    on: !s.terminal_hidden,
                },
                reset: reset_if_changed(CTRL_SHOW_TERMINAL, s),
            }),
        ],
    }
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
                };
                col = col.child(row_frame(
                    row.title,
                    row.description,
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
    fn stub_page_for_other_categories() {
        let expanded = [false; CATEGORY_COUNT];
        let p = panel(
            0,
            None,
            &expanded,
            &Settings::default(),
            1200.0,
            800.0,
            None,
            "",
            false,
        );
        assert!(p.texts.iter().any(|x| x.text == "General"));
        assert!(p.texts.iter().any(|x| x.text == "No settings here yet."));
    }
}
