use ui::{div, theme, Div, IconKind, LabelSize, Node, Radius, Rgba, Space};

/// A short read-only tag: a branch, a kind, a plan, a detected stack.
pub struct Chip {
    label: String,
    color: Option<Rgba>,
    icon: Option<IconKind>,
    fill: Option<Rgba>,
    border: Option<Rgba>,
    max_width: Option<f32>,
}

pub fn chip(label: impl Into<String>) -> Chip {
    Chip {
        label: label.into(),
        color: None,
        icon: None,
        fill: None,
        border: None,
        max_width: None,
    }
}

impl Chip {
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn icon(mut self, icon: IconKind) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn fill(mut self, fill: Rgba) -> Self {
        self.fill = Some(fill);
        self
    }

    pub fn border(mut self, border: Rgba) -> Self {
        self.border = Some(border);
        self
    }

    /// Text, fill and edge all in one color, for a tag that names a kind (a plan, a scope, a label).
    pub fn tinted(self, color: Rgba) -> Self {
        self.color(color)
            .fill(color.alpha(0.14))
            .border(color.alpha(0.3))
    }

    /// Truncates the label past this width, for a free-form value in a fixed column.
    pub fn max_width(mut self, width: f32) -> Self {
        self.max_width = Some(width);
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let ink = self.color.unwrap_or(colors.text);
        let mut chip = div()
            .row()
            .h_px(18.0)
            .px(Space::S4)
            .gap(Space::S2)
            .items_center()
            .rounded(Radius::Small)
            .border(1.0, self.border.unwrap_or(colors.border))
            .bg(self.fill.unwrap_or(colors.element_background));
        if let Some(kind) = self.icon {
            chip = chip.child(ui::icon(kind).size(10.0).color(ink));
        }
        let text = ui::label(self.label)
            .size(LabelSize::XSmall)
            .mono()
            .color(ink);
        match self.max_width {
            Some(max) => {
                let width = ui::measure(&text.clone().into()).0.min(max);
                chip.child(div().row().w_px(width).child(text.truncate()))
            }
            None => chip.child(text),
        }
    }
}

impl From<Chip> for Node {
    fn from(chip: Chip) -> Node {
        chip.render().into()
    }
}
