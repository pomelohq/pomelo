use ui::{div, icon, label, theme, IconKind, Node};

/// A checkbox (14px box, accent filled when on) and its label, the whole row clickable.
pub fn checkbox(id: u64, checked: bool, text: &str) -> Node {
    let colors = theme();
    let mut square = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(14.0)
        .h_px(14.0)
        .rounded(4.0);
    square = if checked {
        square
            .bg(colors.accent_fill)
            .child(icon(IconKind::Check).size(10.0).color(colors.on_accent))
    } else {
        square.border(1.0, colors.border)
    };
    let mut row = div()
        .row()
        .items_center()
        .gap(8.0)
        .h_px(16.0)
        .on_click(id)
        .child(square);
    if !text.is_empty() {
        row = row.child(label(text.to_string()).size(12.0).color(colors.text));
    }
    row.into()
}
