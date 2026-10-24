use ui::{div, theme, Node};

/// A determinate bar: 8px track with 2px inset, filled in the info color (never below 2%).
pub fn progress_bar(value: f32) -> Node {
    let colors = theme();
    let fraction = value.clamp(0.02, 1.0);
    div()
        .row()
        .h_px(8.0)
        .p(2.0)
        .rounded(4.0)
        .bg(colors.background)
        .child(div().h_px(4.0).flex(fraction).rounded(2.0).bg(colors.info))
        .child(div().h_px(4.0).flex(1.0 - fraction))
        .into()
}
