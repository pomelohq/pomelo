use ui::{div, theme, LabelSize, Node, Rgba};

/// A key binding in a small cap, for buttons and footers; `on_accent` when it sits on an accent fill.
pub fn key_chip(keystroke: &str, on_accent: bool) -> Node {
    let colors = theme();
    let (fill, ink) = if on_accent {
        (Rgba::new(1.0, 1.0, 1.0, 0.18), colors.on_accent)
    } else {
        let fill = match colors.appearance {
            ui::Appearance::Dark => Rgba::new(1.0, 1.0, 1.0, 0.05),
            ui::Appearance::Light => colors.element_active,
        };
        (fill, colors.text_muted)
    };
    div()
        .row()
        .h_px(18.0)
        .px(5.0)
        .items_center()
        .rounded(4.0)
        .bg(fill)
        .child(ui::render_keystroke_in(keystroke, LabelSize::Mini, ink))
        .into()
}
