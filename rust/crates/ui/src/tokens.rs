//! Spacing, corner radius, elevation and motion steps every surface picks from.

use std::time::Duration;

use crate::{theme, Appearance, Rect, Rgba};

/// Spacing steps for gaps and padding (absolute px); `Px` is for space that follows something else, such as a
/// hairline or a box's own size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Space {
    S2,
    S4,
    S6,
    S8,
    S10,
    S12,
    S14,
    S16,
    S20,
    S24,
    Px(f32),
}

impl Space {
    pub const STEPS: [Space; 10] = [
        Space::S2,
        Space::S4,
        Space::S6,
        Space::S8,
        Space::S10,
        Space::S12,
        Space::S14,
        Space::S16,
        Space::S20,
        Space::S24,
    ];

    pub const fn px(self) -> f32 {
        match self {
            Space::S2 => 2.0,
            Space::S4 => 4.0,
            Space::S6 => 6.0,
            Space::S8 => 8.0,
            Space::S10 => 10.0,
            Space::S12 => 12.0,
            Space::S14 => 14.0,
            Space::S16 => 16.0,
            Space::S20 => 20.0,
            Space::S24 => 24.0,
            Space::Px(px) => px,
        }
    }
}

/// Corner radius steps (absolute px).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Radius {
    XSmall,
    Small,
    Medium,
    Large,
    XLarge,
}

impl Radius {
    pub const ALL: [Radius; 5] = [
        Radius::XSmall,
        Radius::Small,
        Radius::Medium,
        Radius::Large,
        Radius::XLarge,
    ];

    pub const fn px(self) -> f32 {
        match self {
            Radius::XSmall => 2.0,
            Radius::Small => 4.0,
            Radius::Medium => 6.0,
            Radius::Large => 8.0,
            Radius::XLarge => 10.0,
        }
    }
}

/// How far a surface floats above the window, which sets the shadow under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// Popovers, menus, tooltips and small floating panels.
    Elevated,
    /// Modals that take over input, such as pickers.
    Modal,
    Overlay,
}

impl Elevation {
    pub const ALL: [Elevation; 3] = [Elevation::Elevated, Elevation::Modal, Elevation::Overlay];
}

/// Durations of UI motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Hover and press feedback.
    Fast,
    /// Things that enter or leave: toasts, menus.
    Normal,
    /// Fades of what lingers, such as a highlight dying out.
    Slow,
}

impl Motion {
    pub const ALL: [Motion; 3] = [Motion::Fast, Motion::Normal, Motion::Slow];

    pub const fn duration(self) -> Duration {
        Duration::from_millis(match self {
            Motion::Fast => 100,
            Motion::Normal => 160,
            Motion::Slow => 400,
        })
    }
}

/// The renderer has no blur, so a blurred layer is a stack of rects growing across the blur width.
const SHADOW_MAX_LAYERS: f32 = 48.0;

/// Layered shadows under a floating surface at `rect` (device px, `scale` px per point): (y offset, alpha,
/// blur) per elevation.
pub fn elevation_shadow(rect: Rect, elevation: Elevation, scale: f32) -> Vec<Rect> {
    let light = theme().appearance == Appearance::Light;
    let shadows: &[(f32, f32, f32)] = match (elevation, light) {
        (Elevation::Elevated, true) => &[(2.0, 0.12, 3.0), (1.0, 0.03, 0.0)],
        (Elevation::Elevated, false) => &[(2.0, 0.12, 3.0), (1.0, 0.06, 0.0)],
        (Elevation::Modal, true) => &[
            (2.0, 0.06, 3.0),
            (3.0, 0.06, 6.0),
            (6.0, 0.04, 12.0),
            (1.0, 0.04, 0.0),
        ],
        (Elevation::Modal, false) => &[
            (2.0, 0.12, 3.0),
            (3.0, 0.08, 6.0),
            (6.0, 0.04, 12.0),
            (1.0, 0.12, 0.0),
        ],
        (Elevation::Overlay, true) => &[(12.0, 0.16, 16.0)],
        (Elevation::Overlay, false) => &[(12.0, 0.5, 16.0)],
    };
    let layers: Vec<(f32, f32, f32, f32)> = shadows
        .iter()
        .map(|&(offset, alpha, blur)| (offset * scale, alpha, blur * scale, 0.0))
        .collect();
    box_shadow(rect, &layers, Radius::Large.px() * scale)
}

/// Rects approximating CSS box shadows under `rect`, each `(offset_y, alpha, blur, spread)` in black.
pub fn box_shadow(rect: Rect, shadows: &[(f32, f32, f32, f32)], radius: f32) -> Vec<Rect> {
    let mut rects = Vec::new();
    for &(offset, alpha, blur, grow) in shadows {
        let rect = Rect::new(
            rect.x - grow,
            rect.y - grow,
            (rect.w + grow * 2.0).max(0.0),
            (rect.h + grow * 2.0).max(0.0),
            Rgba::TRANSPARENT,
        );
        // A gaussian blur with sigma = blur fades out within 3 sigma of the edge, reaching above the box even
        // though the shadow drops down; nested rects weighted by the gaussian sum to that falloff.
        let layers: Vec<(f32, f32)> = if blur <= 0.0 {
            vec![(0.0, 1.0)]
        } else {
            let extent = 3.0 * blur;
            let steps = (2.0 * extent).ceil().min(SHADOW_MAX_LAYERS) as usize;
            let step = 2.0 * extent / steps as f32;
            (0..steps)
                .map(|index| {
                    let spread = -extent + (index as f32 + 0.5) * step;
                    let density = (-0.5 * (spread / blur).powi(2)).exp()
                        / (blur * (2.0 * std::f32::consts::PI).sqrt());
                    (spread, density * step)
                })
                .collect()
        };
        for (spread, weight) in layers {
            rects.push(Rect {
                x: rect.x - spread,
                y: rect.y + offset - spread,
                w: rect.w + spread * 2.0,
                h: rect.h + spread * 2.0,
                color: Rgba::new(0.0, 0.0, 0.0, alpha * weight),
                radius: (radius + spread).max(0.0),
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            });
        }
    }
    rects
}
