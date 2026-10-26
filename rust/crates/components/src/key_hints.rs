use ui::{div, label, theme, Div, LabelSize, Node, Radius};

/// The strip at the foot of a picker that says what the keys do: Enter opens, arrows move, Escape closes.
/// A hint with an id is also a button. Built with [`key_hints`].
pub struct KeyHints {
    hints: Vec<Node>,
    hovered: Option<u64>,
}

pub fn key_hints() -> KeyHints {
    KeyHints {
        hints: Vec::new(),
        hovered: None,
    }
}

impl KeyHints {
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered;
        self
    }

    pub fn hint(mut self, id: Option<u64>, keys: &[&str], text: &str) -> Self {
        let colors = theme();
        let mut hint = div()
            .row()
            .h_px(22.0)
            .px(4.0)
            .gap(6.0)
            .items_center()
            .rounded(Radius::Small)
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(3.0)
                    .children(keys.iter().map(|keys| crate::key_chip(keys, false))),
            )
            .child(
                label(text.to_string())
                    .size(LabelSize::Small)
                    .color(colors.text_placeholder),
            );
        if let Some(id) = id {
            hint = hint.on_click(id).hover_bg(colors.ghost_element_hover);
            if self.hovered == Some(id) {
                hint = hint.bg(colors.ghost_element_hover);
            }
        }
        self.hints.push(hint.into());
        self
    }

    pub fn render(self) -> Div {
        div()
            .row()
            .justify_end()
            .items_center()
            .px(8.0)
            .py(6.0)
            .gap(6.0)
            .bg(theme().panel_background)
            .children(self.hints)
    }
}

impl From<KeyHints> for Node {
    fn from(hints: KeyHints) -> Node {
        hints.render().into()
    }
}
