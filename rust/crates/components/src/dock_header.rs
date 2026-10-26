use ui::{div, label, theme, IconKind, LabelSize, Node, Space};

use crate::icon_button;

pub const DOCK_HEADER_HEIGHT: f32 = 36.0;

/// A docked panel's title strip: the name in small caps on the left, one icon action on the right.
pub fn dock_header(title: &str, action: Option<(IconKind, u64)>) -> Node {
    let mut bar = div()
        .row()
        .h_px(DOCK_HEADER_HEIGHT)
        .pl(Space::S12)
        .pr(Space::S8)
        .gap(Space::S8)
        .items_center()
        .child(
            label(title.to_uppercase())
                .size(LabelSize::Mini)
                .weight(600)
                .color(theme().text_muted),
        )
        .child(div().flex(1.0));
    if let Some((kind, id)) = action {
        bar = bar.child(icon_button(id, kind));
    }
    bar.into()
}
