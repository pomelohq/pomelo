//! Semantic color tokens. The whole UI reads colors from the active `ThemeColors` rather than hardcoding hex,
//! so changing theme or light/dark appearance restyles everything at once. Token names and the built-in
//! One palette values mirror the design system we follow. Only the subset the app actually uses is modeled.

use std::sync::RwLock;

/// Straight-alpha RGBA in 0..=1. The renderer linearizes rgb for the sRGB surface and blends using `a`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const TRANSPARENT: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.0);

    /// The same color at a new alpha (0..=1).
    pub const fn alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Parse `#rrggbb` or `#rrggbbaa`. Unknown/short input yields opaque black rather than panicking.
    pub fn hex(s: &str) -> Self {
        let s = s.trim_start_matches('#');
        let channel = |i: usize| -> f32 {
            s.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .unwrap_or(0) as f32
                / 255.0
        };
        let a = if s.len() >= 8 { channel(6) } else { 1.0 };
        Self::new(channel(0), channel(2), channel(4), a)
    }

    /// 8-bit sRGB components (for glyph rendering, which takes u8 rgba).
    pub fn to_u8(self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [c(self.r), c(self.g), c(self.b), c(self.a)]
    }
}

/// Whether a theme is a light or dark variant (drives the default when following the OS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// The subset of semantic color tokens the app renders with. Field names match the design system's tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeColors {
    pub appearance: Appearance,

    pub background: Rgba,
    pub surface_background: Rgba,
    pub elevated_surface_background: Rgba,
    pub panel_background: Rgba,

    pub border: Rgba,
    pub border_variant: Rgba,
    pub border_focused: Rgba,
    pub border_selected: Rgba,

    pub element_background: Rgba,
    pub element_hover: Rgba,
    pub element_active: Rgba,
    pub element_selected: Rgba,
    pub ghost_element_hover: Rgba,
    pub ghost_element_active: Rgba,

    pub text: Rgba,
    pub text_muted: Rgba,
    pub text_placeholder: Rgba,
    pub text_accent: Rgba,
    pub warning: Rgba,
    pub text_disabled: Rgba,

    pub icon: Rgba,
    pub icon_muted: Rgba,
    pub icon_accent: Rgba,

    pub title_bar_background: Rgba,
    pub toolbar_background: Rgba,
    pub tab_bar_background: Rgba,
    pub tab_active_background: Rgba,
    pub tab_inactive_background: Rgba,

    pub editor_background: Rgba,
    pub editor_foreground: Rgba,
    pub editor_active_line: Rgba,
    pub editor_gutter_background: Rgba,
    pub editor_line_number: Rgba,
    pub editor_active_line_number: Rgba,
    pub editor_indent_guide: Rgba,
    pub editor_indent_guide_active: Rgba,
    /// The local user's caret color.
    pub player_cursor: Rgba,
    /// Markers drawn for whitespace (spaces and tabs).
    pub editor_invisible: Rgba,
    pub search_match_background: Rgba,
    pub editor_document_highlight_bracket_background: Rgba,
    pub version_control_added: Rgba,
    pub version_control_modified: Rgba,
    pub version_control_deleted: Rgba,
    /// Status color for hints (inline blame, inlay hints).
    pub hint: Rgba,
    pub info: Rgba,
    /// Status color for things of no concern, e.g. diagnostics without a known severity.
    pub ignored: Rgba,
    pub editor_highlighted_line: Rgba,
    pub search_active_match_background: Rgba,
    pub error: Rgba,

    pub scrollbar_thumb_background: Rgba,
    pub scrollbar_thumb_border: Rgba,
    pub scrollbar_track_border: Rgba,
    pub scrollbar_thumb_hover_background: Rgba,
    /// Faint vertical guide line for indented panel/tree items.
    pub panel_indent_guide: Rgba,

    /// Status "info" accent (used by accent-tinted buttons like the selected file pill).
    pub info_background: Rgba,
    pub info_border: Rgba,
    pub error_background: Rgba,
    pub error_border: Rgba,
    pub warning_background: Rgba,
    pub warning_border: Rgba,
    pub hint_background: Rgba,
    pub hint_border: Rgba,
    pub ignored_background: Rgba,
    pub ignored_border: Rgba,
}

/// One Dark.
pub fn one_dark() -> ThemeColors {
    let h = Rgba::hex;
    ThemeColors {
        appearance: Appearance::Dark,
        background: h("#3b414d"),
        surface_background: h("#2f343e"),
        elevated_surface_background: h("#2f343e"),
        panel_background: h("#2f343e"),
        border: h("#464b57"),
        border_variant: h("#363c46"),
        border_focused: h("#47679e"),
        border_selected: h("#293b5b"),
        element_background: h("#2e343e"),
        element_hover: h("#363c46"),
        element_active: h("#454a56"),
        element_selected: h("#454a56"),
        ghost_element_hover: h("#363c46"),
        ghost_element_active: h("#454a56"),
        text: h("#dce0e5"),
        text_muted: h("#a9afbc"),
        text_placeholder: h("#878a98"),
        text_accent: h("#74ade8"),
        warning: h("#dec184"),
        text_disabled: h("#878a98"),
        icon: h("#dce0e5"),
        icon_muted: h("#a9afbc"),
        icon_accent: h("#74ade8"),
        title_bar_background: h("#3b414d"),
        toolbar_background: h("#282c33"),
        tab_bar_background: h("#2f343e"),
        tab_active_background: h("#282c33"),
        tab_inactive_background: h("#2f343e"),
        editor_background: h("#282c33"),
        editor_foreground: h("#acb2be"),
        editor_active_line: h("#2f343ebf"),
        editor_gutter_background: h("#282c33"),
        editor_line_number: h("#4e5a5f"),
        editor_active_line_number: h("#d0d4da"),
        editor_indent_guide: h("#fefef31b"),
        editor_indent_guide_active: h("#fffaed2d"),
        player_cursor: h("#74ade8"),
        editor_invisible: h("#4e5a5f"),
        search_match_background: h("#74ade866"),
        editor_document_highlight_bracket_background: h("#74ade81a"),
        version_control_added: h("#27a657"),
        version_control_modified: h("#d3b020"),
        version_control_deleted: h("#e06c76"),
        hint: h("#788ca6"),
        info: h("#74ade8"),
        ignored: h("#878a98"),
        editor_highlighted_line: h("#2f343e"),
        search_active_match_background: h("#e8af7466"),
        error: h("#d07277"),
        scrollbar_thumb_background: h("#c8ccd44c"),
        scrollbar_thumb_border: h("#363c46"),
        scrollbar_track_border: h("#2e333c"),
        scrollbar_thumb_hover_background: h("#363c46"),
        panel_indent_guide: h("#363c46"),
        info_background: h("#74ade81a"),
        info_border: h("#293b5b"),
        error_background: h("#d072771a"),
        error_border: h("#4c2b2c"),
        warning_background: h("#dec1841a"),
        warning_border: h("#5d4c2f"),
        hint_background: h("#5a6f891a"),
        hint_border: h("#293b5b"),
        ignored_background: h("#696b771a"),
        ignored_border: h("#464b57"),
    }
}

/// One Light.
pub fn one_light() -> ThemeColors {
    let h = Rgba::hex;
    ThemeColors {
        appearance: Appearance::Light,
        background: h("#dcdcdd"),
        surface_background: h("#ebebec"),
        elevated_surface_background: h("#ebebec"),
        panel_background: h("#ebebec"),
        border: h("#c9c9ca"),
        border_variant: h("#dfdfe0"),
        border_focused: h("#7d82e8"),
        border_selected: h("#cbcdf6"),
        element_background: h("#ebebec"),
        element_hover: h("#dfdfe0"),
        element_active: h("#cacaca"),
        element_selected: h("#cacaca"),
        ghost_element_hover: h("#dfdfe0"),
        ghost_element_active: h("#cacaca"),
        text: h("#242529"),
        text_muted: h("#58585a"),
        text_placeholder: h("#7e8086"),
        text_accent: h("#5c78e2"),
        warning: h("#a48819"),
        text_disabled: h("#7e8086"),
        icon: h("#242529"),
        icon_muted: h("#58585a"),
        icon_accent: h("#5c78e2"),
        title_bar_background: h("#dcdcdd"),
        toolbar_background: h("#fafafa"),
        tab_bar_background: h("#ebebec"),
        tab_active_background: h("#fafafa"),
        tab_inactive_background: h("#ebebec"),
        editor_background: h("#fafafa"),
        editor_foreground: h("#242529"),
        editor_active_line: h("#ebebecbf"),
        editor_gutter_background: h("#fafafa"),
        editor_line_number: h("#b4b4bb"),
        editor_active_line_number: h("#44454b"),
        editor_indent_guide: h("#1f180021"),
        editor_indent_guide_active: h("#19130029"),
        player_cursor: h("#5c78e2"),
        editor_invisible: h("#b4b4bb"),
        search_match_background: h("#5c79e266"),
        editor_document_highlight_bracket_background: h("#5c78e225"),
        version_control_added: h("#27a657"),
        version_control_modified: h("#d3b020"),
        version_control_deleted: h("#e06c76"),
        hint: h("#7274a7"),
        info: h("#5c78e2"),
        ignored: h("#7e8086"),
        editor_highlighted_line: h("#ebebec"),
        search_active_match_background: h("#d0a92366"),
        error: h("#d36151"),
        scrollbar_thumb_background: h("#383a414c"),
        scrollbar_thumb_border: h("#dfdfe0"),
        scrollbar_track_border: h("#eeeeee"),
        scrollbar_thumb_hover_background: h("#dfdfe0"),
        panel_indent_guide: h("#dfdfe0"),
        info_background: h("#e2e2fa"),
        info_border: h("#cbcdf6"),
        error_background: h("#fbdfd9"),
        error_border: h("#f6c6bd"),
        warning_background: h("#faf2e6"),
        warning_border: h("#f4e7d1"),
        hint_background: h("#e2e2fa"),
        hint_border: h("#cbcdf6"),
        ignored_background: h("#dcdcdd"),
        ignored_border: h("#c9c9ca"),
    }
}

/// Gruvbox Dark.
pub fn gruvbox_dark() -> ThemeColors {
    let h = Rgba::hex;
    ThemeColors {
        appearance: Appearance::Dark,
        background: h("#4c4642"),
        surface_background: h("#3a3735"),
        elevated_surface_background: h("#3a3735"),
        panel_background: h("#3a3735"),
        border: h("#5b534d"),
        border_variant: h("#494340"),
        border_focused: h("#303a36"),
        border_selected: h("#303a36"),
        element_background: h("#3a3735"),
        element_hover: h("#494340"),
        element_active: h("#5b524c"),
        element_selected: h("#5b524c"),
        ghost_element_hover: h("#494340"),
        ghost_element_active: h("#5b524c"),
        text: h("#fbf1c7"),
        text_muted: h("#c5b597"),
        text_placeholder: h("#998b78"),
        text_accent: h("#83a598"),
        warning: h("#f9bd2f"),
        text_disabled: h("#998b78"),
        icon: h("#fbf1c7"),
        icon_muted: h("#c5b597"),
        icon_accent: h("#83a598"),
        title_bar_background: h("#4c4642"),
        toolbar_background: h("#282828"),
        tab_bar_background: h("#3a3735"),
        tab_active_background: h("#282828"),
        tab_inactive_background: h("#3a3735"),
        editor_background: h("#282828"),
        editor_foreground: h("#ebdbb2"),
        editor_active_line: h("#3c3836bf"),
        editor_gutter_background: h("#282828"),
        editor_line_number: h("#6e6b5e"),
        editor_active_line_number: h("#dedcd3"),
        editor_indent_guide: h("#fefef31b"),
        editor_indent_guide_active: h("#fffaed2d"),
        player_cursor: h("#83a598"),
        editor_invisible: h("#928474"),
        search_match_background: h("#83a59866"),
        editor_document_highlight_bracket_background: h("#83a5981a"),
        version_control_added: h("#b7bb26"),
        version_control_modified: h("#f9bd2f"),
        version_control_deleted: h("#fb4a35"),
        hint: h("#8c957d"),
        info: h("#83a598"),
        ignored: h("#998b78"),
        editor_highlighted_line: h("#3a3735"),
        search_active_match_background: h("#c09f3f66"),
        error: h("#fb4a35"),
        scrollbar_thumb_background: h("#a899844c"),
        scrollbar_thumb_border: h("#494340"),
        scrollbar_track_border: h("#373432"),
        scrollbar_thumb_hover_background: h("#fbf1c74c"),
        panel_indent_guide: h("#494340"),
        info_background: h("#83a5981a"),
        info_border: h("#303a36"),
        error_background: h("#590a0f"),
        error_border: h("#771617"),
        warning_background: h("#572e10"),
        warning_border: h("#754916"),
        hint_background: h("#1e2321"),
        hint_border: h("#303a36"),
        ignored_background: h("#4c4642"),
        ignored_border: h("#5b534d"),
    }
}

/// Ayu Mirage.
pub fn ayu_mirage() -> ThemeColors {
    let h = Rgba::hex;
    ThemeColors {
        appearance: Appearance::Dark,
        background: h("#464a52"),
        surface_background: h("#353944"),
        elevated_surface_background: h("#353944"),
        panel_background: h("#353944"),
        border: h("#53565d"),
        border_variant: h("#43464f"),
        border_focused: h("#24556f"),
        border_selected: h("#24556f"),
        element_background: h("#353944"),
        element_hover: h("#43464f"),
        element_active: h("#53565d"),
        element_selected: h("#53565d"),
        ghost_element_hover: h("#43464f"),
        ghost_element_active: h("#53565d"),
        text: h("#cccac2"),
        text_muted: h("#9a9a98"),
        text_placeholder: h("#7b7d7f"),
        text_accent: h("#72cffe"),
        warning: h("#fecf72"),
        text_disabled: h("#7b7d7f"),
        icon: h("#cccac2"),
        icon_muted: h("#9a9a98"),
        icon_accent: h("#72cffe"),
        title_bar_background: h("#464a52"),
        toolbar_background: h("#242835"),
        tab_bar_background: h("#353944"),
        tab_active_background: h("#242835"),
        tab_inactive_background: h("#353944"),
        editor_background: h("#242835"),
        editor_foreground: h("#cccac2"),
        editor_active_line: h("#2f3547bf"),
        editor_gutter_background: h("#242835"),
        editor_line_number: h("#575c6b"),
        editor_active_line_number: h("#e1e3ea"),
        editor_indent_guide: h("#fefef31b"),
        editor_indent_guide_active: h("#fffaed2d"),
        player_cursor: h("#72cffe"),
        editor_invisible: h("#787a7c"),
        search_match_background: h("#73cffe66"),
        editor_document_highlight_bracket_background: h("#72cffe1a"),
        version_control_added: h("#d5fe80"),
        version_control_modified: h("#fecf72"),
        version_control_deleted: h("#f18779"),
        hint: h("#7399a3"),
        info: h("#72cffe"),
        ignored: h("#7b7d7f"),
        editor_highlighted_line: h("#353944"),
        search_active_match_background: h("#fd722b66"),
        error: h("#f18779"),
        scrollbar_thumb_background: h("#cccac24c"),
        scrollbar_thumb_border: h("#43464f"),
        scrollbar_track_border: h("#323641"),
        scrollbar_thumb_hover_background: h("#43464f"),
        panel_indent_guide: h("#43464f"),
        info_background: h("#72cffe1a"),
        info_border: h("#24556f"),
        error_background: h("#481a1b"),
        error_border: h("#662e2d"),
        warning_background: h("#574018"),
        warning_border: h("#765a29"),
        hint_background: h("#123950"),
        hint_border: h("#24556f"),
        ignored_background: h("#464a52"),
        ignored_border: h("#53565d"),
    }
}

/// The rem base the design-system text sizes (`LabelSize`) are expressed at; matches the reference's default
/// `ui_font_size`. The active `ui_font_size` scales all UI text relative to this.
pub const UI_FONT_BASE: f32 = 16.0;

static TEXT_SCALE: RwLock<f32> = RwLock::new(1.0);

/// Global UI text scale (`ui_font_size / UI_FONT_BASE`): every rendered text size and its measured extent are
/// multiplied by this, so changing the UI font size zooms all interface text like the reference does.
pub fn ui_text_scale() -> f32 {
    TEXT_SCALE.read().map(|g| *g).unwrap_or(1.0)
}

pub fn set_ui_text_scale(scale: f32) {
    if let Ok(mut g) = TEXT_SCALE.write() {
        *g = scale.clamp(0.25, 6.0);
    }
}

static UI_FONT_WEIGHT: RwLock<u16> = RwLock::new(400);

/// Global base weight (100-900) for non-emphasized UI text; the label builder uses it as the default and both
/// rendering and measurement shape at it, so a heavier weight never overflows its laid-out box.
pub fn ui_font_weight() -> u16 {
    UI_FONT_WEIGHT.read().map(|g| *g).unwrap_or(400)
}

pub fn set_ui_font_weight(weight: u16) {
    if let Ok(mut g) = UI_FONT_WEIGHT.write() {
        *g = weight.clamp(100, 900);
    }
}

#[derive(Clone, Copy)]
pub struct ChromeFlags {
    pub diagnostics: bool,
    pub cursor_position: bool,
    pub language: bool,
    pub branch: bool,
    pub session_name: bool,
}

static CHROME: RwLock<ChromeFlags> = RwLock::new(ChromeFlags {
    diagnostics: true,
    cursor_position: true,
    language: true,
    branch: true,
    session_name: true,
});

pub fn chrome() -> ChromeFlags {
    CHROME.read().map(|g| *g).unwrap_or(ChromeFlags {
        diagnostics: true,
        cursor_position: true,
        language: true,
        branch: true,
        session_name: true,
    })
}

pub fn set_chrome(flags: ChromeFlags) {
    if let Ok(mut g) = CHROME.write() {
        *g = flags;
    }
}

static BUNDLED_FONTS: RwLock<(Option<String>, Option<String>)> = RwLock::new((None, None));

/// Record the real family names of the bundled fonts (sans, mono) so `snap_weight` can tell them apart from
/// system families.
pub fn set_bundled_fonts(sans: Option<String>, mono: Option<String>) {
    if let Ok(mut g) = BUNDLED_FONTS.write() {
        *g = (sans, mono);
    }
}

/// Clamp a requested weight to the bundled families' real static range (IBM Plex ships Thin 100 .. Bold 700).
/// Requesting a weight outside a family's available range makes cosmic-text substitute a whole different family
/// (the mono font silently becomes a sans one); clamping keeps the chosen family. Within [100,700] fontdb picks
/// the nearest available face. System families, which carry their own range, pass through unchanged.
pub fn snap_weight(family: Option<&str>, weight: u16) -> u16 {
    let Some(fam) = family else {
        return weight;
    };
    let Ok(g) = BUNDLED_FONTS.read() else {
        return weight;
    };
    if g.1.as_deref() == Some(fam) {
        if weight <= 550 {
            400
        } else {
            700
        }
    } else if g.0.as_deref() == Some(fam) {
        weight.clamp(100, 700)
    } else {
        weight
    }
}

static ACTIVE_UI_FONT: RwLock<Option<String>> = RwLock::new(None);

/// The family name the renderer draws non-mono UI text in (set by `set_ui_font`). Text measurement reads it so
/// the laid-out box width matches the shaped glyphs; without this, picking a wider family overflows every box.
pub fn active_ui_font() -> Option<String> {
    ACTIVE_UI_FONT.read().ok().and_then(|g| g.clone())
}

pub fn set_active_ui_font(name: Option<String>) {
    if let Ok(mut g) = ACTIVE_UI_FONT.write() {
        *g = name;
    }
}

static CARET_PHASE: RwLock<bool> = RwLock::new(true);

/// Whether the text caret is in its visible blink phase. Caret-drawing code gates on this; the app toggles it
/// on a timer so carets blink.
pub fn caret_phase() -> bool {
    CARET_PHASE.read().map(|g| *g).unwrap_or(true)
}

pub fn set_caret_phase(on: bool) {
    if let Ok(mut g) = CARET_PHASE.write() {
        *g = on;
    }
}

static CURRENT: RwLock<Option<ThemeColors>> = RwLock::new(None);

/// The active theme (defaults to One Dark until `set_theme` is called).
pub fn theme() -> ThemeColors {
    CURRENT
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_else(one_dark)
}

pub fn set_theme(colors: ThemeColors) {
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(colors);
    }
}

/// Resolve a theme by name; unknown names fall back to One Dark.
pub fn by_name(name: &str) -> ThemeColors {
    match name {
        "One Light" => one_light(),
        "Ayu Mirage" => ayu_mirage(),
        "Gruvbox Dark" => gruvbox_dark(),
        _ => one_dark(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_rgb_and_rgba() {
        assert_eq!(Rgba::hex("#ffffff"), Rgba::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(Rgba::hex("#000000ff"), Rgba::new(0.0, 0.0, 0.0, 1.0));
        let half = Rgba::hex("#74ade866");
        assert_eq!(half.to_u8(), [116, 173, 232, 102]);
    }

    #[test]
    fn one_dark_and_light_differ_in_appearance() {
        assert_eq!(one_dark().appearance, Appearance::Dark);
        assert_eq!(one_light().appearance, Appearance::Light);
    }
}
