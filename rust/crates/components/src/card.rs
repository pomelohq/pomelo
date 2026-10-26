use ui::{div, theme, Div, Node, Radius, Rgba};

pub const CARD_RADIUS: Radius = Radius::XLarge;

/// The one card: a rounded box on the element fill with a hairline edge; the caller fills it. A `tint`
/// makes it a notice in that color, `chosen` rings it in the accent, `raised` floats it as a popover.
#[derive(Clone, Copy)]
pub struct Card {
    padding: f32,
    tint: Option<Rgba>,
    chosen: bool,
    hovered: bool,
    raised: bool,
}

pub fn card() -> Card {
    Card {
        padding: 14.0,
        tint: None,
        chosen: false,
        hovered: false,
        raised: false,
    }
}

impl Card {
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    pub fn tint(mut self, color: Rgba) -> Self {
        self.tint = Some(color);
        self
    }

    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }

    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    pub fn raised(mut self) -> Self {
        self.raised = true;
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let (fill, edge) = match (self.tint, self.raised) {
            (Some(tint), raised) => {
                let wash = tint.alpha(if self.hovered { 0.1 } else { 0.06 });
                // A raised card floats over text, so its tint sits on an opaque surface.
                let fill = if raised {
                    wash.over(colors.elevated_surface_background)
                } else {
                    wash
                };
                (fill, tint.alpha(0.3))
            }
            (None, true) => (colors.elevated_surface_background, colors.border),
            (None, false) if self.hovered => (colors.element_hover, colors.border_variant),
            (None, false) => (colors.element_background, colors.border_variant),
        };
        div()
            .col()
            .p(self.padding)
            .rounded(if self.raised {
                Radius::Large
            } else {
                CARD_RADIUS
            })
            .bg(fill)
            .border(
                1.0,
                if self.chosen {
                    colors.text_accent
                } else {
                    edge
                },
            )
    }
}

impl From<Card> for Node {
    fn from(card: Card) -> Node {
        card.render().into()
    }
}
