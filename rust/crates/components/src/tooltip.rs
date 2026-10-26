use ui::{div, theme, Div, LabelSize, Radius, Space};

const TOOLTIP_HEIGHT: f32 = 24.0;

/// The bubble a tooltip shows: its text, then the keys that run the action when there are any.
pub fn tooltip_bubble(text: &str, keys: Option<&str>) -> Div {
    let colors = theme();
    let mut content = div().row().gap(Space::S8).items_center().child(
        ui::label(text.to_string())
            .size(LabelSize::Small)
            .color(colors.text),
    );
    if let Some(keys) = keys.filter(|keys| !keys.is_empty()) {
        let mut strokes = div().row().gap(Space::S4).items_center();
        for stroke in keys.split_whitespace() {
            strokes = strokes.child(ui::render_keystroke(stroke, LabelSize::Small));
        }
        content = content.child(strokes);
    }
    div()
        .row()
        .items_center()
        .px(Space::S8)
        .h_px(TOOLTIP_HEIGHT)
        .rounded(Radius::Medium)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border)
        .child(content)
}

/// `target` with `text` in a bubble pinned under it, or over it when `above`.
pub fn with_tooltip(target: Div, text: &str, above: bool) -> Div {
    let width = ui::measure_text_width(text, 12.0, false, ui::ui_font_weight())
        / ui::ui_text_scale()
        + 16.0;
    let (corner, dy) = if above {
        (ui::Corner::TopLeft, -(TOOLTIP_HEIGHT + 4.0))
    } else {
        (ui::Corner::BottomLeft, TOOLTIP_HEIGHT + 4.0)
    };
    target.pin(
        corner,
        0.0,
        dy,
        width,
        TOOLTIP_HEIGHT,
        ui::deferred(tooltip_bubble(text, None))
            .priority(5)
            .snap_to_window(),
    )
}
