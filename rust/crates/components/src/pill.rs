use ui::{div, theme, Appearance, Div, IconKind, LabelSize, Node, Rgba};

use crate::dot;

/// The one pill: a state in a word or two with its dot, tinted by its color. `attention` is the loud
/// form for something waiting on the person.
pub struct Pill {
    text: String,
    color: Option<Rgba>,
    icon: Option<IconKind>,
    attention: bool,
    click: Option<u64>,
    hovered: bool,
}

pub fn pill(text: impl Into<String>) -> Pill {
    Pill {
        text: text.into(),
        color: None,
        icon: None,
        attention: false,
        click: None,
        hovered: false,
    }
}

impl Pill {
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn icon(mut self, icon: IconKind) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn attention(mut self) -> Self {
        self.attention = true;
        self
    }

    pub fn on_click(mut self, id: u64) -> Self {
        self.click = Some(id);
        self
    }

    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }
}

impl From<Pill> for Node {
    fn from(pill: Pill) -> Node {
        pill.render().into()
    }
}

impl Pill {
    /// The pill as a box, for a caller that sizes it or wraps it in a tooltip.
    pub fn render(self) -> Div {
        let pill = self;
        let colors = theme();
        let (fill, ink) = if pill.attention {
            (colors.attention, colors.on_attention)
        } else {
            let color = pill.color.unwrap_or(colors.text_muted);
            (color.alpha(if pill.hovered { 0.22 } else { 0.14 }), color)
        };
        let mut node = div()
            .row()
            .items_center()
            .gap(5.0)
            .h_px(20.0)
            .px(7.0)
            .rounded(10.0)
            .bg(fill)
            .child(match pill.icon {
                Some(kind) => ui::icon(kind).size(11.0).color(ink).into(),
                None => Node::from(dot(ink, 6.0)),
            })
            .child(
                div().row().flex(1.0).items_center().child(
                    ui::label(pill.text)
                        .size(LabelSize::Mini)
                        .medium()
                        .color(ink)
                        .truncate(),
                ),
            );
        if let Some(id) = pill.click {
            node = node.on_click(id);
        }
        // The attention yellow is close to white, so on a light card it needs an edge to stand off it.
        if pill.attention && (pill.hovered || colors.appearance == Appearance::Light) {
            node = node.border(
                1.0,
                if pill.hovered {
                    colors.on_attention
                } else {
                    colors.on_attention.alpha(0.22)
                },
            );
        }
        node
    }
}
