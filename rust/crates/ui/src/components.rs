//! Design-system components built on the element tree, mirroring the reference `ui` crate: label sizes,
//! buttons (by style), and dividers. Values (text sizes, button height/padding, token mapping) are taken
//! from the reference so screens compose these instead of hand-styling divs.

use crate::{div, theme, Div, Label, Rgba};

/// UI text sizes (absolute px), matching the reference's `LabelSize` -> `text_ui_*`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LabelSize {
    XSmall,
    Small,
    Default,
    Large,
}

impl LabelSize {
    pub fn px(self) -> f32 {
        match self {
            LabelSize::XSmall => 10.0,
            LabelSize::Small => 12.0,
            LabelSize::Default => 14.0,
            LabelSize::Large => 16.0,
        }
    }
}

impl Label {
    pub fn label_size(self, size: LabelSize) -> Self {
        self.size(size.px())
    }
}

/// Text color roles, mapping to theme tokens like the reference's `Color`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextColor {
    Default,
    Muted,
    Accent,
    Disabled,
    Placeholder,
}

impl TextColor {
    pub fn rgba(self) -> Rgba {
        let t = theme();
        match self {
            TextColor::Default => t.text,
            TextColor::Muted => t.text_muted,
            TextColor::Accent => t.text_accent,
            TextColor::Disabled => t.text_disabled,
            TextColor::Placeholder => t.text_placeholder,
        }
    }
}

/// Button visual styles, mirroring the reference's `ButtonStyle`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    Filled,
    Outlined,
    OutlinedGhost,
    Subtle,
    TintedAccent,
}

impl ButtonStyle {
    /// (background, border, label) resolved from the active theme. `None` = transparent / no border.
    fn colors(self) -> (Option<Rgba>, Option<Rgba>, Rgba) {
        let t = theme();
        match self {
            ButtonStyle::Filled => (Some(t.element_background), None, t.text),
            ButtonStyle::Outlined => (Some(t.element_background), Some(t.border_variant), t.text),
            ButtonStyle::OutlinedGhost => (None, Some(t.border_variant), t.text),
            ButtonStyle::Subtle => (None, None, t.text),
            ButtonStyle::TintedAccent => (Some(t.info_background), Some(t.info_border), t.text),
        }
    }
}

const BUTTON_RADIUS: f32 = 4.0;

/// Button heights/paddings, from the reference's `ButtonSize` (Default is what the files header uses; Medium
/// is the larger size dropdown triggers use).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    Default,
    Medium,
}

impl ButtonSize {
    fn height(self) -> f32 {
        match self {
            ButtonSize::Default => 22.0,
            ButtonSize::Medium => 28.0,
        }
    }
    fn pad_x(self) -> f32 {
        // reference DynamicSpacing at default density: Base04 (4px) for Default, Base08 (8px) for Medium.
        match self {
            ButtonSize::Default => 4.0,
            ButtonSize::Medium => 8.0,
        }
    }
}

fn button_impl(text: String, style: ButtonStyle, size: ButtonSize, click: Option<u64>) -> Div {
    let (bg, border, label_color) = style.colors();
    let mut b = div()
        .h_px(size.height())
        .px(size.pad_x())
        .gap(4.0)
        .items_center()
        .rounded(BUTTON_RADIUS);
    if let Some(id) = click {
        b = b.on_click(id);
    }
    if let Some(bg) = bg {
        b = b.bg(bg);
    }
    if let Some(border) = border {
        b = b.border(1.0, border);
    }
    b.child(
        crate::label(text)
            .label_size(LabelSize::Default)
            .color(label_color),
    )
}

/// A labeled button in the given style at the default (files-header) size; `id` is its click id.
pub fn button(id: u64, text: impl Into<String>, style: ButtonStyle) -> Div {
    button_impl(text.into(), style, ButtonSize::Default, Some(id))
}

/// A clickable button at a chosen size; `id` is its click id.
pub fn button_sized(id: u64, text: impl Into<String>, style: ButtonStyle, size: ButtonSize) -> Div {
    button_impl(text.into(), style, size, Some(id))
}

/// A display-only button (no click region) for controls whose action isn't wired yet.
pub fn button_static(text: impl Into<String>, style: ButtonStyle) -> Div {
    button_impl(text.into(), style, ButtonSize::Default, None)
}

/// A display-only button at a chosen size (e.g. Medium for dropdown triggers).
pub fn button_static_sized(text: impl Into<String>, style: ButtonStyle, size: ButtonSize) -> Div {
    button_impl(text.into(), style, size, None)
}

/// A 1px horizontal divider in the default border color.
pub fn divider() -> Div {
    div().h_px(1.0).bg(theme().border_variant)
}

/// A 1px horizontal divider in the faded border color (reference: `DividerColor::BorderFaded`).
pub fn divider_faded() -> Div {
    let b = theme().border;
    div().h_px(1.0).bg(Rgba::new(b.r, b.g, b.b, b.a * 0.6))
}
