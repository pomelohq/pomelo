use ui::{div, label, theme, Div, LabelSize, Node, Radius, Rgba, Space};

use crate::dot;

const MAX: usize = 99;

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
        let pill = div().row().h_px(16.0);
        // One digit stays a circle; more grow sideways.
        let pill = if text.len() == 1 {
            pill.w_px(16.0)
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
