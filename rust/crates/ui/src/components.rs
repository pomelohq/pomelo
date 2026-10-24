//! Design-system components built on the element tree, mirroring the reference `ui` crate: label sizes,
//! buttons (by style), and dividers. Values (text sizes, button height/padding, token mapping) are taken
//! from the reference so screens compose these instead of hand-styling divs.

use crate::{div, theme, Div, IconKind, Label, Node, Rgba};

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

/// How a button reads: the one action it is for, a plain alternative, a quiet one, or a destructive one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// Small sits in panels, cards and toolbars; Medium in dialog footers and forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    Small,
    Medium,
}

impl ButtonSize {
    fn height(self) -> f32 {
        match self {
            ButtonSize::Small => 24.0,
            ButtonSize::Medium => 28.0,
        }
    }

    fn pad_x(self) -> f32 {
        match self {
            ButtonSize::Small => 8.0,
            ButtonSize::Medium => 10.0,
        }
    }

    fn text(self) -> f32 {
        match self {
            ButtonSize::Small => 12.0,
            ButtonSize::Medium => 13.0,
        }
    }

    fn icon(self) -> f32 {
        match self {
            ButtonSize::Small => 12.0,
            ButtonSize::Medium => 14.0,
        }
    }
}

/// The one button every screen uses; build it with [`button`] and set what differs from a small secondary.
#[derive(Clone)]
pub struct Button {
    id: u64,
    text: String,
    kind: ButtonKind,
    size: ButtonSize,
    leading: Option<IconKind>,
    keystroke: Option<String>,
    enabled: bool,
    hovered: bool,
}

pub fn button(id: u64, text: impl Into<String>) -> Button {
    Button {
        id,
        text: text.into(),
        kind: ButtonKind::Secondary,
        size: ButtonSize::Small,
        leading: None,
        keystroke: None,
        enabled: true,
        hovered: false,
    }
}

impl Button {
    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn icon(mut self, leading: Option<IconKind>) -> Self {
        self.leading = leading;
        self
    }

    /// The key that runs it, shown in a chip after the label.
    pub fn keystroke(mut self, keystroke: Option<&str>) -> Self {
        self.keystroke = keystroke.map(str::to_string);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// For views drawn without pointer tracking, which pass the hovered id themselves.
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered == Some(self.id);
        self
    }
}

impl From<Button> for Node {
    fn from(button: Button) -> Node {
        let colors = theme();
        let enabled = button.enabled;
        let dim = |color: Rgba| if enabled { color } else { color.alpha(0.45) };
        let primary = button.kind == ButtonKind::Primary;
        let ink = match button.kind {
            ButtonKind::Primary => colors.on_accent,
            ButtonKind::Secondary | ButtonKind::Ghost => colors.text,
            ButtonKind::Danger => colors.error,
        };
        let mut node = div()
            .row()
            .items_center()
            .gap(if button.leading.is_some() { 6.0 } else { 4.0 })
            .h_px(button.size.height())
            .px(button.size.pad_x())
            .rounded(6.0);
        node = match button.kind {
            ButtonKind::Primary => node.bg(dim(colors.accent_fill)),
            ButtonKind::Secondary => node
                .bg(dim(colors.element_background))
                .border(1.0, dim(colors.border)),
            ButtonKind::Ghost | ButtonKind::Danger => node,
        };
        if enabled {
            node = node.on_click(button.id);
            if !primary {
                node = node.hover_bg(colors.ghost_element_hover);
                if button.hovered {
                    node = node.bg(colors.ghost_element_hover);
                }
            }
        }
        if let Some(kind) = button.leading {
            node = node.child(crate::icon(kind).size(button.size.icon()).color(dim(ink)));
        }
        node = node.child(
            crate::label(button.text)
                .size(button.size.text())
                .medium()
                .color(dim(ink)),
        );
        if let Some(keystroke) = &button.keystroke {
            node = node.child(key_chip(keystroke, primary));
        }
        node.into()
    }
}

/// A key binding in a small cap, for buttons and footers; `on_accent` when it sits on an accent fill.
pub fn key_chip(keystroke: &str, on_accent: bool) -> Node {
    let colors = theme();
    let (fill, ink) = if on_accent {
        (Rgba::new(1.0, 1.0, 1.0, 0.18), colors.on_accent)
    } else {
        let fill = match colors.appearance {
            crate::Appearance::Dark => Rgba::new(1.0, 1.0, 1.0, 0.05),
            crate::Appearance::Light => colors.element_active,
        };
        (fill, colors.text_muted)
    };
    div()
        .row()
        .h_px(18.0)
        .px(5.0)
        .items_center()
        .rounded(4.0)
        .bg(fill)
        .child(crate::render_keystroke_in(keystroke, 11.0, ink))
        .into()
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
