use ui::{div, theme, Appearance, Node, Rgba};

use crate::dot;

/// The one pill: a state in a word or two with its dot, tinted by its color. `attention` is the loud
/// form for something waiting on the person.
pub struct Pill {
    text: String,
    color: Option<Rgba>,
    attention: bool,
}

pub fn pill(text: impl Into<String>) -> Pill {
    Pill {
        text: text.into(),
        color: None,
        attention: false,
    }
}

impl Pill {
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn attention(mut self) -> Self {
        self.attention = true;
        self
    }
}

impl From<Pill> for Node {
    fn from(pill: Pill) -> Node {
        let colors = theme();
        let (fill, ink) = if pill.attention {
            (colors.attention, colors.on_attention)
        } else {
            let color = pill.color.unwrap_or(colors.text_muted);
            (color.alpha(0.14), color)
        };
        let mut node = div()
            .row()
            .items_center()
            .gap(5.0)
            .h_px(20.0)
            .px(7.0)
            .rounded(10.0)
            .bg(fill)
            .child(dot(ink, 6.0))
            .child(
                ui::label(pill.text)
                    .size(11.0)
                    .medium()
                    .color(ink)
                    .truncate(),
            );
        // The attention yellow is close to white, so on a light card it needs an edge to stand off it.
        if pill.attention && colors.appearance == Appearance::Light {
            node = node.border(1.0, colors.on_attention.alpha(0.22));
        }
        node.into()
    }
}
