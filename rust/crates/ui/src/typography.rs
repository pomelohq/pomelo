//! Text sizes and color roles every label picks from.

use crate::{theme, Label, Rgba};

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
