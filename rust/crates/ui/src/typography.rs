//! Text sizes and color roles every label picks from.

use crate::{theme, Rgba};

/// The UI type scale (absolute px).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LabelSize {
    XSmall,
    Mini,
    Small,
    Medium,
    Default,
    Large,
}

impl LabelSize {
    pub const fn px(self) -> f32 {
        match self {
            LabelSize::XSmall => 10.0,
            LabelSize::Mini => 11.0,
            LabelSize::Small => 12.0,
            LabelSize::Medium => 13.0,
            LabelSize::Default => 14.0,
            LabelSize::Large => 16.0,
        }
    }

    /// One step down the scale, for a quieter companion to text of this size.
    pub fn smaller(self) -> LabelSize {
        match self {
            LabelSize::XSmall | LabelSize::Mini => LabelSize::XSmall,
            LabelSize::Small => LabelSize::Mini,
            LabelSize::Medium => LabelSize::Small,
            LabelSize::Default => LabelSize::Medium,
            LabelSize::Large => LabelSize::Default,
        }
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
