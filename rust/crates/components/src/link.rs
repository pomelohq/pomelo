use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Space};

/// A text-only action in the accent color, beside the value it acts on, with an optional leading icon.
pub fn link(id: u64, leading: Option<IconKind>, text: &str, enabled: bool) -> Node {
    let colors = theme();
    let color = if enabled {
        colors.text_accent
    } else {
        colors.text_disabled
    };
    let mut action = div().row().items_center().gap(Space::S6).h_px(16.0);
    if enabled {
        action = action.on_click(id);
    }
    if let Some(kind) = leading {
        action = action.child(icon(kind).size(12.0).color(color));
    }
    action
        .child(label(text.to_string()).size(LabelSize::Small).color(color))
        .into()
}
