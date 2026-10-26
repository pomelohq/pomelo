use ui::{div, theme, LabelSize, Node, Radius, Space};

/// A key binding in a small cap, for buttons and footers; `on_accent` when it sits on an accent fill.
pub fn key_chip(keystroke: &str, on_accent: bool) -> Node {
    let colors = theme();
    let (fill, ink) = if on_accent {
        (colors.on_accent.alpha(0.18), colors.on_accent)
    } else {
        let fill = match colors.appearance {
            ui::Appearance::Dark => colors.tint(),
            ui::Appearance::Light => colors.element_active,
        };
        (fill, colors.text_muted)
    };
    div()
        .row()
        .h_px(18.0)
        .px(Space::S6)
        .items_center()
        .rounded(Radius::Small)
        .bg(fill)
        .child(ui::render_keystroke_in(keystroke, LabelSize::Mini, ink))
        .into()
}
