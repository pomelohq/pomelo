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

    /// An opaque color from 8-bit channels, as syntax themes store them.
    pub const fn rgb8([r, g, b]: [u8; 3]) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
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

    /// This color painted over an opaque `base`, as the renderer blends: for overlapping shapes that must not
    /// double their tint.
    pub fn over(self, base: Rgba) -> Rgba {
        let mix = |top: f32, bottom: f32| top * self.a + bottom * (1.0 - self.a);
        Rgba::new(
            mix(self.r, base.r),
            mix(self.g, base.g),
            mix(self.b, base.b),
            1.0,
        )
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
    /// The selected row of a picker, whose elevated surface the panel's flattened selection barely shows on.
    pub overlay_selected: Rgba,
    pub ghost_element_hover: Rgba,
    pub ghost_element_active: Rgba,

    pub text: Rgba,
    pub text_muted: Rgba,
    pub text_placeholder: Rgba,
    pub text_accent: Rgba,
    pub warning: Rgba,
    pub success: Rgba,
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
    pub player_selection: Rgba,
    pub terminal_background: Rgba,
    pub terminal_ansi_background: Rgba,
    pub terminal_foreground: Rgba,
    pub terminal_bright_foreground: Rgba,
    pub terminal_dim_foreground: Rgba,
    /// Black, red, green, yellow, blue, magenta, cyan, white, then their bright variants.
    pub terminal_ansi: [Rgba; 16],
    pub terminal_ansi_dim: [Rgba; 8],
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
    pub link_text_hover: Rgba,
    pub editor_highlighted_line: Rgba,
    pub search_active_match_background: Rgba,
    pub error: Rgba,

    pub scrollbar_thumb_background: Rgba,
    pub scrollbar_thumb_border: Rgba,
    pub scrollbar_track_border: Rgba,
    pub scrollbar_thumb_hover_background: Rgba,
    /// Faint vertical guide line for indented panel/tree items.
    pub panel_indent_guide: Rgba,
    pub panel_focused_border: Rgba,

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

    /// The one thing that needs you now (an agent waiting), as a fill, and the ink drawn on it.
    pub attention: Rgba,
    pub on_attention: Rgba,
    /// The accent as a fill (the current workspace's tile), and the ink drawn on it.
    pub accent_fill: Rgba,
    pub on_accent: Rgba,
}

/// Pomelo Dark: the app's own theme, layered surfaces with one violet accent. Borders and element states are
/// flattened over the panel layer because callers re-tint them with `alpha`.
pub fn pomelo_dark() -> ThemeColors {
    let h = Rgba::hex;
    ThemeColors {
        appearance: Appearance::Dark,
        background: h("#15171c"),
        surface_background: h("#1e2127"),
        elevated_surface_background: h("#30343e"),
        panel_background: h("#1e2127"),
        border: h("#393c41"),
        border_variant: h("#2b2e34"),
        border_focused: h("#8e47b0"),
        border_selected: h("#c39ae666"),
        element_background: h("#2a2e36"),
        element_hover: h("#292c32"),
        element_active: h("#32353a"),
        element_selected: h("#312e3d"),
        overlay_selected: h("#413f52"),
        ghost_element_hover: h("#292c32"),
        ghost_element_active: h("#32353a"),
        text: h("#e7e9ee"),
        text_muted: h("#a9afbb"),
        text_placeholder: h("#7c8290"),
        text_accent: h("#c39ae6"),
        warning: h("#e3bf6a"),
        success: h("#8fc27a"),
        text_disabled: h("#7c8290"),
        icon: h("#e7e9ee"),
        icon_muted: h("#a9afbb"),
        icon_accent: h("#c39ae6"),
        title_bar_background: h("#15171c"),
        toolbar_background: h("#23262d"),
        tab_bar_background: h("#1e2127"),
        tab_active_background: h("#23262d"),
        tab_inactive_background: h("#1e2127"),
        editor_background: h("#23262d"),
        editor_foreground: h("#e7e9ee"),
        editor_active_line: h("#ffffff0d"),
        editor_gutter_background: h("#23262d"),
        editor_line_number: h("#7c8290"),
        editor_active_line_number: h("#e7e9ee"),
        editor_indent_guide: h("#ffffff12"),
        editor_indent_guide_active: h("#ffffff30"),
        player_cursor: h("#c39ae6"),
        player_selection: h("#c39ae640"),
        terminal_background: h("#23262d"),
        terminal_ansi_background: h("#15171c"),
        terminal_foreground: h("#e7e9ee"),
        terminal_bright_foreground: h("#e7e9ee"),
        terminal_dim_foreground: h("#7c8290"),
        terminal_ansi: [
            h("#2a2e36"),
            h("#ef8a8f"),
            h("#8fc27a"),
            h("#e3bf6a"),
            h("#86b6ec"),
            h("#c39ae6"),
            h("#6fc3c9"),
            h("#c9ccd4"),
            h("#7c8290"),
            h("#f5a3a7"),
            h("#a9d696"),
            h("#f0d38f"),
            h("#a5cbf5"),
            h("#d6b6f0"),
            h("#8fd8dd"),
            h("#ffffff"),
        ],
        terminal_ansi_dim: [
            h("#2a2e36b3"),
            h("#ef8a8fb3"),
            h("#8fc27ab3"),
            h("#e3bf6ab3"),
            h("#86b6ecb3"),
            h("#c39ae6b3"),
            h("#6fc3c9b3"),
            h("#c9ccd4b3"),
        ],
        editor_invisible: h("#7c829080"),
        search_match_background: h("#86b6ec4d"),
        editor_document_highlight_bracket_background: h("#c39ae61f"),
        version_control_added: h("#8fc27a"),
        version_control_modified: h("#e3bf6a"),
        version_control_deleted: h("#ef8a8f"),
        hint: h("#7c8290"),
        info: h("#86b6ec"),
        ignored: h("#7c8290"),
        link_text_hover: h("#c39ae6"),
        editor_highlighted_line: h("#292c32"),
        search_active_match_background: h("#f2d64b66"),
        error: h("#ef8a8f"),
        scrollbar_thumb_background: h("#e7e9ee33"),
        scrollbar_thumb_border: h("#2b2e34"),
        scrollbar_track_border: h("#2b2e34"),
        scrollbar_thumb_hover_background: h("#e7e9ee4d"),
        panel_indent_guide: h("#32353a"),
        panel_focused_border: h("#8e47b0"),
        info_background: h("#86b6ec1f"),
        info_border: h("#3d4e62"),
        error_background: h("#ef8a8f1f"),
        error_border: h("#5d4146"),
        warning_background: h("#e3bf6a1f"),
        warning_border: h("#59513b"),
        hint_background: h("#7c82901f"),
        hint_border: h("#3a3e47"),
        ignored_background: h("#7c82901f"),
        ignored_border: h("#3a3e47"),
        attention: h("#f2d64b"),
        on_attention: h("#2b2300"),
        accent_fill: h("#8e47b0"),
        on_accent: h("#ffffff"),
    }
}

/// Pomelo Light: the app's own theme, layered surfaces with one violet accent.
pub fn pomelo_light() -> ThemeColors {
    let h = Rgba::hex;
    ThemeColors {
        appearance: Appearance::Light,
        background: h("#e7e5ec"),
        surface_background: h("#f4f3f7"),
        elevated_surface_background: h("#ffffff"),
        panel_background: h("#f4f3f7"),
        border: h("#d4d2d9"),
        border_variant: h("#e3e2e7"),
        border_focused: h("#7b2f95"),
        border_selected: h("#7b2f9566"),
        element_background: h("#ffffff"),
        element_hover: h("#eceaef"),
        element_active: h("#e3e2e7"),
        element_selected: h("#e8dfed"),
        overlay_selected: h("#f2eaf4"),
        ghost_element_hover: h("#eceaef"),
        ghost_element_active: h("#e3e2e7"),
        text: h("#1f1b27"),
        text_muted: h("#5a5466"),
        text_placeholder: h("#827c8e"),
        text_accent: h("#7b2f95"),
        warning: h("#8a6400"),
        success: h("#3f7d2e"),
        text_disabled: h("#827c8e"),
        icon: h("#1f1b27"),
        icon_muted: h("#5a5466"),
        icon_accent: h("#7b2f95"),
        title_bar_background: h("#e7e5ec"),
        toolbar_background: h("#ffffff"),
        tab_bar_background: h("#f4f3f7"),
        tab_active_background: h("#ffffff"),
        tab_inactive_background: h("#f4f3f7"),
        editor_background: h("#ffffff"),
        editor_foreground: h("#1f1b27"),
        editor_active_line: h("#1d17300a"),
        editor_gutter_background: h("#ffffff"),
        editor_line_number: h("#827c8e"),
        editor_active_line_number: h("#1f1b27"),
        editor_indent_guide: h("#00000014"),
        editor_indent_guide_active: h("#0000002b"),
        player_cursor: h("#7b2f95"),
        player_selection: h("#7b2f9540"),
        terminal_background: h("#ffffff"),
        terminal_ansi_background: h("#e7e5ec"),
        terminal_foreground: h("#1f1b27"),
        terminal_bright_foreground: h("#1f1b27"),
        terminal_dim_foreground: h("#827c8e"),
        terminal_ansi: [
            h("#1f1b27"),
            h("#b3263a"),
            h("#3f7d2e"),
            h("#8a6400"),
            h("#2f62b0"),
            h("#7b2f95"),
            h("#0f7c85"),
            h("#d9d6e0"),
            h("#827c8e"),
            h("#cf3a4f"),
            h("#4f9a3a"),
            h("#a37a00"),
            h("#3d78cc"),
            h("#9440b0"),
            h("#16939d"),
            h("#ffffff"),
        ],
        terminal_ansi_dim: [
            h("#1f1b27b3"),
            h("#b3263ab3"),
            h("#3f7d2eb3"),
            h("#8a6400b3"),
            h("#2f62b0b3"),
            h("#7b2f95b3"),
            h("#0f7c85b3"),
            h("#d9d6e0b3"),
        ],
        editor_invisible: h("#827c8e80"),
        search_match_background: h("#2f62b04d"),
        editor_document_highlight_bracket_background: h("#7b2f951f"),
        version_control_added: h("#3f7d2e"),
        version_control_modified: h("#8a6400"),
        version_control_deleted: h("#b3263a"),
        hint: h("#827c8e"),
        info: h("#2f62b0"),
        ignored: h("#827c8e"),
        link_text_hover: h("#7b2f95"),
        editor_highlighted_line: h("#eceaef"),
        search_active_match_background: h("#f2d64b66"),
        error: h("#b3263a"),
        scrollbar_thumb_background: h("#1f1b2733"),
        scrollbar_thumb_border: h("#e3e2e7"),
        scrollbar_track_border: h("#e3e2e7"),
        scrollbar_thumb_hover_background: h("#1f1b274d"),
        panel_indent_guide: h("#dedde1"),
        panel_focused_border: h("#7b2f95"),
        info_background: h("#2f62b01f"),
        info_border: h("#b9c7e2"),
        error_background: h("#b3263a1f"),
        error_border: h("#e0b5be"),
        warning_background: h("#8a64001f"),
        warning_border: h("#d4c8ac"),
        hint_background: h("#827c8e1f"),
        hint_border: h("#d2cfd7"),
        ignored_background: h("#827c8e1f"),
        ignored_border: h("#d2cfd7"),
        attention: h("#f2d64b"),
        on_attention: h("#2b2300"),
        accent_fill: h("#7b2f95"),
        on_accent: h("#ffffff"),
    }
}

/// A theme written in the design system's tokens; [`from_palette`] lays it out on [`ThemeColors`] the way
/// Pomelo's own themes are laid out, so every built-in theme layers and tints its surfaces the same way.
pub struct Palette {
    pub name: &'static str,
    pub appearance: Appearance,
    /// app, rail, panel, surface, raised, overlay, hover, press, selected, line, line-strong, text, text-2,
    /// text-3, accent, accent-fill, on-accent, attention, on-attention, success, warning, danger, info,
    /// indent-guide, indent-guide-active, editor-active-line.
    pub colors: [&'static str; 26],
    /// keyword, function, symbol, string, type, number.
    pub syntax: [&'static str; 6],
}

/// The other built-in themes, each re-expressed in the design tokens.
pub const PALETTES: &[Palette] = &[
    Palette {
        name: "One Dark",
        appearance: Appearance::Dark,
        colors: [
            "#1f2228",
            "#23262c",
            "#262a30",
            "#282c33",
            "#2f343e",
            "#353a45",
            "#ffffff0d",
            "#ffffff17",
            "#74ade826",
            "#ffffff0f",
            "#ffffff1f",
            "#dce0e5",
            "#a9afbc",
            "#878a98",
            "#74ade8",
            "#3f6fb4",
            "#ffffff",
            "#e5c07b",
            "#282c33",
            "#a1c181",
            "#d19a66",
            "#d07277",
            "#56b6c2",
            "#ffffff12",
            "#ffffff30",
            "#ffffff0d",
        ],
        syntax: [
            "#c678dd", "#61afef", "#56b6c2", "#98c379", "#e5c07b", "#d19a66",
        ],
    },
    Palette {
        name: "One Light",
        appearance: Appearance::Light,
        colors: [
            "#e2e2e4",
            "#e8e8ea",
            "#f1f1f2",
            "#fafafa",
            "#ffffff",
            "#ffffff",
            "#0000000a",
            "#00000014",
            "#5c78e21f",
            "#00000014",
            "#00000026",
            "#242529",
            "#58585a",
            "#7e8086",
            "#5c78e2",
            "#4f6bd6",
            "#ffffff",
            "#e2b93d",
            "#2b2300",
            "#669f59",
            "#a8762a",
            "#d36151",
            "#3b8eea",
            "#00000014",
            "#0000002b",
            "#0000000a",
        ],
        syntax: [
            "#a626a4", "#4078f2", "#0184bc", "#50a14f", "#c18401", "#986801",
        ],
    },
    Palette {
        name: "Ayu Mirage",
        appearance: Appearance::Dark,
        colors: [
            "#1b1f29",
            "#1f2330",
            "#212532",
            "#242835",
            "#2b3040",
            "#333849",
            "#ffffff0d",
            "#ffffff17",
            "#72cffe26",
            "#ffffff0f",
            "#ffffff1f",
            "#cccac2",
            "#9a9a98",
            "#7b7d7f",
            "#72cffe",
            "#2f86bd",
            "#ffffff",
            "#ffd173",
            "#242835",
            "#d5ff80",
            "#ffad66",
            "#f28779",
            "#5ccfe6",
            "#ffffff12",
            "#ffffff30",
            "#ffffff0d",
        ],
        syntax: [
            "#ffad66", "#ffd173", "#d5ff80", "#d5ff80", "#73d0ff", "#dfbfff",
        ],
    },
    Palette {
        name: "Gruvbox Dark",
        appearance: Appearance::Dark,
        colors: [
            "#1d2021",
            "#212324",
            "#252525",
            "#282828",
            "#32302f",
            "#3c3836",
            "#ffffff0d",
            "#ffffff17",
            "#83a59826",
            "#ffffff0f",
            "#ffffff1f",
            "#fbf1c7",
            "#d5c4a1",
            "#a89984",
            "#83a598",
            "#458588",
            "#fbf1c7",
            "#fabd2f",
            "#282828",
            "#b8bb26",
            "#fe8019",
            "#fb4934",
            "#8ec07c",
            "#ffffff12",
            "#ffffff30",
            "#ffffff0d",
        ],
        syntax: [
            "#fb4934", "#b8bb26", "#83a598", "#b8bb26", "#fabd2f", "#d3869b",
        ],
    },
    Palette {
        name: "Catppuccin Mocha",
        appearance: Appearance::Dark,
        colors: [
            "#11111b",
            "#141420",
            "#181825",
            "#1e1e2e",
            "#262637",
            "#313244",
            "#ffffff0d",
            "#ffffff17",
            "#cba6f726",
            "#ffffff0f",
            "#ffffff1f",
            "#cdd6f4",
            "#bac2de",
            "#7f849c",
            "#cba6f7",
            "#cba6f7",
            "#11111b",
            "#f9e2af",
            "#1e1e2e",
            "#a6e3a1",
            "#fab387",
            "#f38ba8",
            "#89dceb",
            "#ffffff12",
            "#ffffff30",
            "#ffffff0d",
        ],
        syntax: [
            "#cba6f7", "#89b4fa", "#94e2d5", "#a6e3a1", "#f9e2af", "#fab387",
        ],
    },
    Palette {
        name: "Catppuccin Macchiato",
        appearance: Appearance::Dark,
        colors: [
            "#181926",
            "#1b1d2b",
            "#1e2030",
            "#24273a",
            "#2c3045",
            "#363a4f",
            "#ffffff0d",
            "#ffffff17",
            "#c6a0f626",
            "#ffffff0f",
            "#ffffff1f",
            "#cad3f5",
            "#b8c0e0",
            "#8087a2",
            "#c6a0f6",
            "#c6a0f6",
            "#181926",
            "#eed49f",
            "#24273a",
            "#a6da95",
            "#f5a97f",
            "#ed8796",
            "#91d7e3",
            "#ffffff12",
            "#ffffff30",
            "#ffffff0d",
        ],
        syntax: [
            "#c6a0f6", "#8aadf4", "#8bd5ca", "#a6da95", "#eed49f", "#f5a97f",
        ],
    },
    Palette {
        name: "Catppuccin Frappe",
        appearance: Appearance::Dark,
        colors: [
            "#232634",
            "#262938",
            "#292c3c",
            "#303446",
            "#383c50",
            "#414559",
            "#ffffff0d",
            "#ffffff17",
            "#ca9ee626",
            "#ffffff0f",
            "#ffffff1f",
            "#c6d0f5",
            "#b5bfe2",
            "#838ba7",
            "#ca9ee6",
            "#ca9ee6",
            "#232634",
            "#e5c890",
            "#303446",
            "#a6d189",
            "#ef9f76",
            "#e78284",
            "#99d1db",
            "#ffffff12",
            "#ffffff30",
            "#ffffff0d",
        ],
        syntax: [
            "#ca9ee6", "#8caaee", "#81c8be", "#a6d189", "#e5c890", "#ef9f76",
        ],
    },
    Palette {
        name: "Catppuccin Latte",
        appearance: Appearance::Light,
        colors: [
            "#dce0e8",
            "#e1e4eb",
            "#e6e9ef",
            "#eff1f5",
            "#f5f6f9",
            "#ffffff",
            "#0000000a",
            "#00000014",
            "#8839ef1f",
            "#00000014",
            "#00000026",
            "#4c4f69",
            "#5c5f77",
            "#8c8fa1",
            "#8839ef",
            "#8839ef",
            "#ffffff",
            "#e5b32a",
            "#2b2300",
            "#40a02b",
            "#fe640b",
            "#d20f39",
            "#04a5e5",
            "#00000014",
            "#0000002b",
            "#0000000a",
        ],
        syntax: [
            "#8839ef", "#1e66f5", "#179299", "#40a02b", "#df8e1d", "#fe640b",
        ],
    },
    Palette {
        name: "Tokyo Night",
        appearance: Appearance::Dark,
        colors: [
            "#13131a",
            "#16161e",
            "#181923",
            "#1a1b26",
            "#1f2335",
            "#292e42",
            "#ffffff0d",
            "#ffffff17",
            "#7aa2f726",
            "#ffffff0f",
            "#ffffff1f",
            "#c0caf5",
            "#a9b1d6",
            "#737aa2",
            "#7aa2f7",
            "#3d59a1",
            "#ffffff",
            "#e0af68",
            "#1a1b26",
            "#9ece6a",
            "#ff9e64",
            "#f7768e",
            "#7dcfff",
            "#ffffff12",
            "#ffffff30",
            "#ffffff0d",
        ],
        syntax: [
            "#bb9af7", "#7aa2f7", "#7dcfff", "#9ece6a", "#2ac3de", "#ff9e64",
        ],
    },
    Palette {
        name: "Nord",
        appearance: Appearance::Dark,
        colors: [
            "#242933",
            "#272c36",
            "#2a303b",
            "#2e3440",
            "#3b4252",
            "#434c5e",
            "#ffffff0d",
            "#ffffff17",
            "#88c0d026",
            "#ffffff0f",
            "#ffffff1f",
            "#eceff4",
            "#d8dee9",
            "#9aa3b4",
            "#88c0d0",
            "#5e81ac",
            "#ffffff",
            "#ebcb8b",
            "#2e3440",
            "#a3be8c",
            "#d08770",
            "#bf616a",
            "#81a1c1",
            "#ffffff12",
            "#ffffff30",
            "#ffffff0d",
        ],
        syntax: [
            "#81a1c1", "#88c0d0", "#8fbcbb", "#a3be8c", "#8fbcbb", "#b48ead",
        ],
    },
    Palette {
        name: "Rose Pine Dawn",
        appearance: Appearance::Light,
        colors: [
            "#f2e9e1",
            "#f4ece4",
            "#f7f0e8",
            "#faf4ed",
            "#fffaf3",
            "#ffffff",
            "#0000000a",
            "#00000014",
            "#907aa91f",
            "#00000014",
            "#00000026",
            "#575279",
            "#797593",
            "#9893a5",
            "#907aa9",
            "#907aa9",
            "#ffffff",
            "#ea9d34",
            "#2b2300",
            "#56949f",
            "#d7827e",
            "#b4637a",
            "#286983",
            "#00000014",
            "#0000002b",
            "#0000000a",
        ],
        syntax: [
            "#286983", "#d7827e", "#56949f", "#ea9d34", "#907aa9", "#b4637a",
        ],
    },
];

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba::new(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        1.0,
    )
}

/// A palette laid out on the theme fields. Translucent borders and states are flattened over the panel
/// layer because callers re-tint them with `alpha`.
pub fn from_palette(palette: &Palette) -> ThemeColors {
    let h = Rgba::hex;
    let [app, _rail, panel, surface, raised, overlay, hover, press, selected, line, line_strong, text, text_2, text_3, accent, accent_fill, on_accent, attention, on_attention, success, warning, danger, info, indent_guide, indent_guide_active, active_line] =
        palette.colors.map(h);
    let flat = |color: Rgba| color.over(panel);
    let tint = |color: Rgba, a: f32| color.alpha(a);
    let edge = |color: Rgba| color.alpha(0.3).over(panel);
    let dark = palette.appearance == Appearance::Dark;
    let cyan = h(palette.syntax[4]);
    let (black, white) = if dark {
        (raised, text_2)
    } else {
        (text, flat(press))
    };
    let bright = |color: Rgba| mix(color, h("#ffffff"), if dark { 0.25 } else { 0.15 });
    let base = [black, danger, success, warning, info, accent, cyan, white];
    let ansi = [
        black,
        danger,
        success,
        warning,
        info,
        accent,
        cyan,
        white,
        text_3,
        bright(danger),
        bright(success),
        bright(warning),
        bright(info),
        bright(accent),
        bright(cyan),
        if dark { text } else { h("#ffffff") },
    ];
    ThemeColors {
        appearance: palette.appearance,
        background: app,
        surface_background: panel,
        elevated_surface_background: overlay,
        panel_background: panel,
        border: flat(line_strong),
        border_variant: flat(line),
        border_focused: accent_fill,
        border_selected: tint(accent, 0.4),
        element_background: raised,
        element_hover: flat(hover),
        element_active: flat(press),
        element_selected: flat(selected),
        overlay_selected: selected.over(overlay),
        ghost_element_hover: flat(hover),
        ghost_element_active: flat(press),
        text,
        text_muted: text_2,
        text_placeholder: text_3,
        text_accent: accent,
        warning,
        success,
        text_disabled: text_3,
        icon: text,
        icon_muted: text_2,
        icon_accent: accent,
        title_bar_background: app,
        toolbar_background: surface,
        tab_bar_background: panel,
        tab_active_background: surface,
        tab_inactive_background: panel,
        editor_background: surface,
        editor_foreground: text,
        editor_active_line: active_line,
        editor_gutter_background: surface,
        editor_line_number: text_3,
        editor_active_line_number: text,
        editor_indent_guide: indent_guide,
        editor_indent_guide_active: indent_guide_active,
        player_cursor: accent,
        player_selection: tint(accent, 0.25),
        terminal_background: surface,
        terminal_ansi_background: app,
        terminal_foreground: text,
        terminal_bright_foreground: text,
        terminal_dim_foreground: text_3,
        terminal_ansi: ansi,
        terminal_ansi_dim: base.map(|color| tint(color, 0.7)),
        editor_invisible: tint(text_3, 0.5),
        search_match_background: tint(info, 0.3),
        editor_document_highlight_bracket_background: tint(accent, 0.12),
        version_control_added: success,
        version_control_modified: warning,
        version_control_deleted: danger,
        hint: text_3,
        info,
        ignored: text_3,
        link_text_hover: accent,
        editor_highlighted_line: flat(hover),
        search_active_match_background: tint(attention, 0.4),
        error: danger,
        scrollbar_thumb_background: tint(text, 0.2),
        scrollbar_thumb_border: flat(line),
        scrollbar_track_border: flat(line),
        scrollbar_thumb_hover_background: tint(text, 0.3),
        panel_indent_guide: flat(press),
        panel_focused_border: accent_fill,
        info_background: tint(info, 0.12),
        info_border: edge(info),
        error_background: tint(danger, 0.12),
        error_border: edge(danger),
        warning_background: tint(warning, 0.12),
        warning_border: edge(warning),
        hint_background: tint(text_3, 0.12),
        hint_border: edge(text_3),
        ignored_background: tint(text_3, 0.12),
        ignored_border: edge(text_3),
        attention,
        on_attention,
        accent_fill,
        on_accent,
    }
}

fn palette(name: &str) -> Option<&'static Palette> {
    PALETTES.iter().find(|palette| palette.name == name)
}

/// One Dark, on the design tokens.
pub fn one_dark() -> ThemeColors {
    palette("One Dark")
        .map(from_palette)
        .unwrap_or_else(pomelo_dark)
}

/// One Light, on the design tokens.
pub fn one_light() -> ThemeColors {
    palette("One Light")
        .map(from_palette)
        .unwrap_or_else(pomelo_light)
}

/// Every built-in theme name: Pomelo's own, then the palettes; dark themes before light ones.
pub fn builtin_theme_names() -> Vec<(&'static str, Appearance)> {
    let mut names = vec![
        ("Pomelo Dark", Appearance::Dark),
        ("Pomelo Light", Appearance::Light),
    ];
    names.extend(
        PALETTES
            .iter()
            .map(|palette| (palette.name, palette.appearance)),
    );
    names.sort_by_key(|(_, appearance)| *appearance == Appearance::Light);
    names
}

/// The rem base the design-system text sizes (`LabelSize`) are expressed at; matches the reference's default
/// `ui_font_size`. The active `ui_font_size` scales all UI text relative to this.
pub const UI_FONT_BASE: f32 = 16.0;

static TEXT_SCALE: RwLock<f32> = RwLock::new(1.0);

/// Global UI text scale (`ui_font_size / UI_FONT_BASE`): every rendered text size and its measured extent are
/// multiplied by this, so changing the UI font size zooms all interface text like the reference does.
pub fn ui_text_scale() -> f32 {
    if let Some(scale) = SCALE_OVERRIDE.with(std::cell::Cell::get) {
        return scale;
    }
    TEXT_SCALE.read().map(|g| *g).unwrap_or(1.0)
}

thread_local! {
    static SCALE_OVERRIDE: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` as if the UI text scale were `scale`: what it builds, lays out and measures is sized at it, and
/// the text it renders keeps that size when drawn. The editor lays out at 1 so its text follows the buffer
/// font size alone, not the UI font size.
pub fn with_ui_scale<R>(scale: f32, f: impl FnOnce() -> R) -> R {
    let before = SCALE_OVERRIDE.with(|cell| cell.replace(Some(scale)));
    let result = f();
    SCALE_OVERRIDE.with(|cell| cell.set(before));
    result
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
    pub language_servers: bool,
}

static CHROME: RwLock<ChromeFlags> = RwLock::new(ChromeFlags {
    diagnostics: true,
    cursor_position: true,
    language: true,
    branch: true,
    session_name: true,
    language_servers: true,
});

pub fn chrome() -> ChromeFlags {
    CHROME.read().map(|g| *g).unwrap_or(ChromeFlags {
        diagnostics: true,
        cursor_position: true,
        language: true,
        branch: true,
        session_name: true,
        language_servers: true,
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

/// Which configured font a run of text is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextFont {
    #[default]
    Ui,
    /// The code editor's font.
    Buffer,
    Terminal,
}

impl From<bool> for TextFont {
    /// `true` is the monospace code font, as the older `mono` flag meant.
    fn from(mono: bool) -> Self {
        if mono {
            TextFont::Buffer
        } else {
            TextFont::Ui
        }
    }
}

impl TextFont {
    fn index(self) -> usize {
        match self {
            TextFont::Ui => 0,
            TextFont::Buffer => 1,
            TextFont::Terminal => 2,
        }
    }

    pub fn is_mono(self) -> bool {
        self != TextFont::Ui
    }
}

/// One font's settings: its family (`None` = the bundled default), the weight regular text uses, OpenType
/// features (tag and value) and the families tried, in order, for characters the family lacks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FontSettings {
    pub family: Option<String>,
    pub weight: Option<u16>,
    pub features: Vec<([u8; 4], u32)>,
    pub fallbacks: Vec<String>,
}

static FONT_SETTINGS: RwLock<[Option<FontSettings>; 3]> = RwLock::new([None, None, None]);
static FONT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn font_settings(font: TextFont) -> FontSettings {
    FONT_SETTINGS
        .read()
        .ok()
        .and_then(|fonts| fonts[font.index()].clone())
        .unwrap_or_default()
}

pub fn set_font_settings(font: TextFont, settings: FontSettings) {
    if let Ok(mut fonts) = FONT_SETTINGS.write() {
        if fonts[font.index()].as_ref() != Some(&settings) {
            fonts[font.index()] = Some(settings);
            FONT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Bumped whenever a font's settings change, so shaped and measured text is keyed to the fonts it used.
pub fn font_generation() -> u64 {
    FONT_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// `{"calt": false, "ss01": true, "cv01": 2}` as feature tags and values; the problems say what was skipped.
pub fn parse_font_features(value: &serde_json::Value) -> (Vec<([u8; 4], u32)>, Vec<String>) {
    let mut features = Vec::new();
    let mut problems = Vec::new();
    let Some(object) = value.as_object() else {
        if !value.is_null() {
            problems.push("font features must be an object like {\"calt\": false}".into());
        }
        return (features, problems);
    };
    for (tag, value) in object {
        let bytes = tag.as_bytes();
        let Ok(tag_bytes) = <[u8; 4]>::try_from(bytes) else {
            problems.push(format!("font feature \"{tag}\" is not a four-letter tag"));
            continue;
        };
        let value = match value {
            serde_json::Value::Bool(on) => u32::from(*on),
            serde_json::Value::Number(number) => {
                match number.as_u64().and_then(|n| u32::try_from(n).ok()) {
                    Some(number) => number,
                    None => {
                        problems.push(format!(
                            "font feature \"{tag}\" needs true, false or a whole number"
                        ));
                        continue;
                    }
                }
            }
            _ => {
                problems.push(format!(
                    "font feature \"{tag}\" needs true, false or a whole number"
                ));
                continue;
            }
        };
        features.push((tag_bytes, value));
    }
    (features, problems)
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

/// How the editor draws its caret.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaretShape {
    #[default]
    Bar,
    /// Fills the character after it, which shows through in the background color.
    Block,
    Underline,
    /// A frame around the character after it.
    Hollow,
}

/// The editor's caret settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaretStyle {
    pub shape: CaretShape,
    pub blink: bool,
    /// The caret glides to where it moves instead of jumping.
    pub animate: bool,
    /// Cmd-click adds a caret (and Alt-click goes to definition) instead of the other way round.
    pub add_with_cmd: bool,
}

impl Default for CaretStyle {
    fn default() -> Self {
        CaretStyle {
            shape: CaretShape::Bar,
            blink: true,
            animate: false,
            add_with_cmd: false,
        }
    }
}

static CARET_STYLE: RwLock<CaretStyle> = RwLock::new(CaretStyle {
    shape: CaretShape::Bar,
    blink: true,
    animate: false,
    add_with_cmd: false,
});

pub fn caret_style() -> CaretStyle {
    CARET_STYLE.read().map(|g| *g).unwrap_or_default()
}

pub fn set_caret_style(style: CaretStyle) {
    if let Ok(mut g) = CARET_STYLE.write() {
        *g = style;
    }
}

static REDUCE_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Non-essential motion (loading shimmers, pulsing placeholders, the gliding caret) stays still.
pub fn reduce_motion() -> bool {
    REDUCE_MOTION.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_reduce_motion(reduce: bool) {
    REDUCE_MOTION.store(reduce, std::sync::atomic::Ordering::Relaxed);
}

static CURRENT: RwLock<Option<ThemeColors>> = RwLock::new(None);

/// The active theme (defaults to One Dark until `set_theme` is called).
/// Translucent tints derived from the theme, for edges and fills that must read the same over any surface.
impl ThemeColors {
    fn ink(&self, dark: Rgba, alpha_dark: f32, alpha_light: f32) -> Rgba {
        match self.appearance {
            Appearance::Dark => dark.alpha(alpha_dark),
            Appearance::Light => self.text.alpha(alpha_light),
        }
    }

    /// A rule or control edge that reads the same on a panel and on a tinted row.
    pub fn hairline(&self) -> Rgba {
        self.ink(Rgba::new(1.0, 1.0, 1.0, 1.0), 0.12, 0.15)
    }

    /// The ring around a floating surface; on light layers it needs a dark tint to stand off the page.
    pub fn overlay_ring(&self) -> Rgba {
        self.ink(Rgba::new(1.0, 1.0, 1.0, 1.0), 0.08, 0.12)
    }

    /// A hover or chip fill on a raised card, where the element fill would not show.
    pub fn tint(&self) -> Rgba {
        self.ink(Rgba::new(1.0, 1.0, 1.0, 1.0), 0.05, 0.04)
    }

    /// The one-pixel shadow edge under a raised tab or along a panel's side.
    pub fn edge_shadow(&self) -> Rgba {
        self.ink(Rgba::new(0.0, 0.0, 0.0, 1.0), 0.3, 0.08)
    }

    /// A band set into the surface around it, such as a strip of shared rows.
    pub fn recess(&self) -> Rgba {
        Rgba::new(0.0, 0.0, 0.0, 0.08)
    }

    /// The dim laid over the window behind a modal.
    pub fn scrim(&self) -> Rgba {
        Rgba::new(0.0, 0.0, 0.0, 0.2)
    }
}

pub fn theme() -> ThemeColors {
    CURRENT
        .read()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_else(pomelo_dark)
}

pub fn set_theme(colors: ThemeColors) {
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(colors);
    }
}

/// Resolve a theme by name, the user's themes first; unknown names fall back to Pomelo Dark.
pub fn by_name(name: &str) -> ThemeColors {
    if let Some(theme) = crate::theme_file::user_theme(name) {
        return theme.colors;
    }
    match name {
        "Pomelo Light" => pomelo_light(),
        name => palette(name).map(from_palette).unwrap_or_else(pomelo_dark),
    }
}

/// Syntax colors a built-in theme lays over the palette of its appearance; empty for themes that use it as is.
pub fn builtin_syntax(name: &str) -> Vec<(String, Rgba)> {
    let palette: &[(&str, &str)] = match name {
        "Pomelo Dark" => &[
            ("keyword", "#c39ae6"),
            ("preproc", "#c39ae6"),
            ("function", "#86b6ec"),
            ("constructor", "#86b6ec"),
            ("type", "#6fc3c9"),
            ("enum", "#6fc3c9"),
            ("string", "#e2b86b"),
            ("string.special.symbol", "#8fc27a"),
            ("number", "#e39b7b"),
            ("boolean", "#e39b7b"),
            ("constant", "#e39b7b"),
            ("comment", "#7c8290"),
            ("property", "#ef8a8f"),
            ("tag", "#86b6ec"),
            ("attribute", "#c39ae6"),
            ("title", "#c39ae6"),
            ("variable", "#e7e9ee"),
            ("variable.special", "#e39b7b"),
            ("operator", "#a9afbb"),
            ("punctuation", "#a9afbb"),
        ],
        "Pomelo Light" => &[
            ("keyword", "#7b2f95"),
            ("preproc", "#7b2f95"),
            ("function", "#2f62b0"),
            ("constructor", "#2f62b0"),
            ("type", "#0f7c85"),
            ("enum", "#0f7c85"),
            ("string", "#8a6400"),
            ("string.special.symbol", "#3f7d2e"),
            ("number", "#a8502c"),
            ("boolean", "#a8502c"),
            ("constant", "#a8502c"),
            ("comment", "#827c8e"),
            ("property", "#b3263a"),
            ("tag", "#2f62b0"),
            ("attribute", "#7b2f95"),
            ("title", "#7b2f95"),
            ("variable", "#1f1b27"),
            ("variable.special", "#a8502c"),
            ("operator", "#5a5466"),
            ("punctuation", "#5a5466"),
        ],
        name => {
            return palette(name).map(palette_syntax).unwrap_or_default();
        }
    };
    palette
        .iter()
        .map(|(capture, color)| (capture.to_string(), Rgba::hex(color)))
        .collect()
}

/// A palette's six syntax tokens spread over the captures the highlighter emits, as Pomelo's are.
fn palette_syntax(palette: &Palette) -> Vec<(String, Rgba)> {
    let [keyword, function, symbol, string, kind, number] = palette.syntax;
    let text = palette.colors[11];
    let text_2 = palette.colors[12];
    let text_3 = palette.colors[13];
    let danger = palette.colors[21];
    [
        ("keyword", keyword),
        ("preproc", keyword),
        ("function", function),
        ("constructor", function),
        ("type", kind),
        ("enum", kind),
        ("string", string),
        ("string.special.symbol", symbol),
        ("number", number),
        ("boolean", number),
        ("constant", number),
        ("comment", text_3),
        ("property", danger),
        ("tag", function),
        ("attribute", keyword),
        ("title", keyword),
        ("variable", text),
        ("variable.special", number),
        ("operator", text_2),
        ("punctuation", text_2),
    ]
    .iter()
    .map(|(capture, color)| (capture.to_string(), Rgba::hex(color)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_features_read_like_the_settings_file() {
        let (features, problems) = parse_font_features(
            &serde_json::json!({"calt": false, "ss01": true, "cv01": 2, "toolong": 1}),
        );
        assert_eq!(features, vec![(*b"calt", 0), (*b"cv01", 2), (*b"ss01", 1)]);
        assert_eq!(problems.len(), 1);
        assert!(parse_font_features(&serde_json::Value::Null).1.is_empty());
    }

    #[test]
    fn hex_parses_rgb_and_rgba() {
        assert_eq!(Rgba::hex("#ffffff"), Rgba::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(Rgba::hex("#000000ff"), Rgba::new(0.0, 0.0, 0.0, 1.0));
        let half = Rgba::hex("#74ade866");
        assert_eq!(half.to_u8(), [116, 173, 232, 102]);
    }

    #[test]
    fn pomelo_is_the_fallback_and_has_both_appearances() {
        assert_eq!(by_name("no such theme"), pomelo_dark());
        assert_eq!(by_name("Pomelo Light"), pomelo_light());
        assert_eq!(pomelo_dark().appearance, Appearance::Dark);
        assert_eq!(pomelo_light().appearance, Appearance::Light);
        assert_ne!(pomelo_dark().background, pomelo_light().background);
        assert!(!builtin_syntax("Pomelo Dark").is_empty());
        assert!(!builtin_syntax("One Dark").is_empty());
    }

    #[test]
    fn palettes_lay_out_like_pomelo() {
        // Pomelo's own tokens through the shared builder land on the hand-tuned Pomelo themes.
        let pomelo = Palette {
            name: "Pomelo Dark",
            appearance: Appearance::Dark,
            colors: [
                "#15171c",
                "#191b21",
                "#1e2127",
                "#23262d",
                "#2a2e36",
                "#30343e",
                "#ffffff0d",
                "#ffffff17",
                "#b98be01f",
                "#ffffff0f",
                "#ffffff1f",
                "#e7e9ee",
                "#a9afbb",
                "#7c8290",
                "#c39ae6",
                "#8e47b0",
                "#ffffff",
                "#f2d64b",
                "#2b2300",
                "#8fc27a",
                "#e3bf6a",
                "#ef8a8f",
                "#86b6ec",
                "#ffffff12",
                "#ffffff30",
                "#ffffff0d",
            ],
            syntax: [
                "#c39ae6", "#86b6ec", "#8fc27a", "#e2b86b", "#6fc3c9", "#e39b7b",
            ],
        };
        let built = from_palette(&pomelo);
        let hand = pomelo_dark();
        let close = |a: Rgba, b: Rgba| {
            let [a, b] = [a.to_u8(), b.to_u8()];
            a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 1)
        };
        for (name, a, b) in [
            ("background", built.background, hand.background),
            ("panel", built.panel_background, hand.panel_background),
            ("editor", built.editor_background, hand.editor_background),
            ("border", built.border, hand.border),
            ("border_variant", built.border_variant, hand.border_variant),
            ("element_hover", built.element_hover, hand.element_hover),
            (
                "element_selected",
                built.element_selected,
                hand.element_selected,
            ),
            (
                "overlay_selected",
                built.overlay_selected,
                hand.overlay_selected,
            ),
            ("text_accent", built.text_accent, hand.text_accent),
            ("info_border", built.info_border, hand.info_border),
            (
                "panel_indent_guide",
                built.panel_indent_guide,
                hand.panel_indent_guide,
            ),
            ("attention", built.attention, hand.attention),
        ] {
            assert!(close(a, b), "{name}: {:?} vs {:?}", a.to_u8(), b.to_u8());
        }
    }

    #[test]
    fn every_builtin_theme_resolves_with_its_own_colors_and_syntax() {
        let names = builtin_theme_names();
        let mut seen = std::collections::HashSet::new();
        for (name, appearance) in &names {
            assert!(seen.insert(*name), "duplicate theme {name}");
            let theme = by_name(name);
            assert_eq!(theme.appearance, *appearance, "{name}");
            assert!(
                !builtin_syntax(name).is_empty(),
                "{name} has no syntax colors"
            );
            if !name.starts_with("Pomelo") {
                assert_ne!(theme, pomelo_dark(), "{name} fell back to Pomelo");
            }
        }
        assert_eq!(names.len(), 2 + PALETTES.len());
        let first_light = names
            .iter()
            .position(|(_, a)| *a == Appearance::Light)
            .unwrap_or(names.len());
        assert!(names[first_light..]
            .iter()
            .all(|(_, a)| *a == Appearance::Light));
        for kept in ["One Dark", "One Light", "Ayu Mirage", "Gruvbox Dark"] {
            assert!(
                names.iter().any(|(name, _)| *name == kept),
                "{kept} must keep its name"
            );
        }
    }

    #[test]
    fn one_dark_and_light_differ_in_appearance() {
        assert_eq!(one_dark().appearance, Appearance::Dark);
        assert_eq!(one_light().appearance, Appearance::Light);
    }
}
