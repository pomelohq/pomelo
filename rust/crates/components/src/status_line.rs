use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Rgba};

/// A small muted status line: a leading icon and truncated text.
pub fn status_line(kind: IconKind, color: Rgba, text: &str) -> Node {
    div()
        .row()
        .items_center()
        .gap(4.0)
        .child(icon(kind).size(12.0).color(color))
        .child(
            label(text.to_string())
                .label_size(LabelSize::Small)
                .color(theme().text_muted)
                .truncate(),
        )
        .into()
}
