use ui::{div, label, theme, Div, IconKind, LabelSize, Node, Radius, Rgba, Space};

use crate::dot;

const MAX: usize = 99;
pub const BADGE_SIZE: f32 = 16.0;

/// How many of something wait to be seen, as a small filled count ("99+" past 99). Zero shows nothing,
/// so a caller can pass the count straight through.
pub struct Badge {
    count: usize,
    color: Option<Rgba>,
}

pub fn badge(count: usize) -> Badge {
    Badge { count, color: None }
}

impl Badge {
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }
}

impl From<Badge> for Node {
    fn from(badge: Badge) -> Node {
        if badge.count == 0 {
            return div().into();
        }
        let colors = theme();
        let text = if badge.count > MAX {
            format!("{MAX}+")
        } else {
            badge.count.to_string()
        };
        let pill = div().row().h_px(BADGE_SIZE);
        // One digit stays a circle; more grow sideways.
        let pill = if text.len() == 1 {
            pill.w_px(BADGE_SIZE)
        } else {
            pill.px(Space::S4)
        };
        pill.items_center()
            .justify_center()
            .rounded(Radius::Large)
            .bg(badge.color.unwrap_or(colors.accent_fill))
            .child(
                label(text)
                    .size(LabelSize::XSmall)
                    .weight(600)
                    .color(colors.on_accent),
            )
            .into()
    }
}

/// The count-less form, set at an icon's lower right: something there wants a look.
pub fn badge_dot(color: Rgba) -> Div {
    dot(color, 6.0)
}

/// The glyph form, for a state with no count hung on a tile's corner (an update running, done or failed).
pub fn badge_icon(kind: IconKind, color: Rgba) -> Div {
    div()
        .row()
        .w_px(BADGE_SIZE)
        .h_px(BADGE_SIZE)
        .items_center()
        .justify_center()
        .rounded(Radius::Large)
        .bg(color)
        .child(ui::icon(kind).size(10.0).color(theme().background))
}
