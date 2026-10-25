use ui::{div, label, theme, Label, Node};

pub const KEY_WIDTH: f32 = 96.0;

/// One fact on a line: what it is, in a column of its own, then its value and a quieter note.
pub fn key_value(key: Label, value: Label, detail: Option<&str>) -> Node {
    let mut row = div()
        .row()
        .items_center()
        .gap(12.0)
        .h_px(20.0)
        .child(div().row().w_px(KEY_WIDTH).child(key.truncate()));
    match detail.filter(|detail| !detail.is_empty()) {
        Some(detail) => {
            row = row.child(value).child(
                div()
                    .row()
                    .flex(1.0)
                    .child(key_label(detail).color(theme().text_placeholder).truncate()),
            );
        }
        None => row = row.child(div().row().flex(1.0).child(value.truncate())),
    }
    row.into()
}

/// Key text as a [`key_value`] row sets it: small and muted.
pub fn key_label(text: impl Into<String>) -> Label {
    label(text.into()).size(12.0).color(theme().text_muted)
}
