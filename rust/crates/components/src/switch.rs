use ui::{div, theme, Node, Radius, Space};

/// An on/off switch: the knob slides right on the accent fill when on.
pub fn switch(id: u64, on: bool) -> Node {
    let colors = theme();
    let knob = div()
        .w_px(14.0)
        .h_px(14.0)
        .rounded(Radius::Large)
        .bg(if on {
            colors.on_accent
        } else {
            colors.text_muted
        });
    let track = div()
        .row()
        .w_px(34.0)
        .h_px(20.0)
        .px(Space::S4)
        .items_center()
        .rounded(Radius::XLarge)
        .on_click(id);
    if on {
        track
            .bg(colors.accent_fill)
            .child(div().flex(1.0))
            .child(knob)
            .into()
    } else {
        track
            .bg(colors.element_active)
            .border(1.0, colors.border)
            .child(knob)
            .child(div().flex(1.0))
            .into()
    }
}
