use ui::{div, icon, label, theme, IconKind, Node};

/// A checkbox (14px box, accent filled when on) and its label, the whole row clickable. Built with
/// [`checkbox`].
pub struct Checkbox {
    id: u64,
    checked: bool,
    partial: bool,
    hovered: bool,
    text: String,
}

pub fn checkbox(id: u64, checked: bool, text: &str) -> Checkbox {
    Checkbox {
        id,
        checked,
        partial: false,
        hovered: false,
        text: text.to_string(),
    }
}

impl Checkbox {
    pub fn partial(mut self, partial: bool) -> Self {
        self.partial = partial;
        self
    }

    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }
}

impl From<Checkbox> for Node {
    fn from(checkbox: Checkbox) -> Node {
        let colors = theme();
        let mut square = div()
            .row()
            .items_center()
            .justify_center()
            .w_px(14.0)
            .h_px(14.0)
            .rounded(4.0);
        square = if checkbox.partial {
            square
                .bg(colors.accent_fill)
                .child(div().w_px(7.0).h_px(2.0).rounded(1.0).bg(colors.on_accent))
        } else if checkbox.checked {
            square
                .bg(colors.accent_fill)
                .child(icon(IconKind::Check).size(10.0).color(colors.on_accent))
        } else if checkbox.hovered {
            square.border(1.0, colors.text_accent)
        } else {
            square.border(1.0, colors.border)
        };
        let mut row = div()
            .row()
            .items_center()
            .gap(8.0)
            .h_px(16.0)
            .on_click(checkbox.id)
            .child(square);
        if !checkbox.text.is_empty() {
            row = row.child(label(checkbox.text).size(12.0).color(colors.text));
        }
        row.into()
    }
}
