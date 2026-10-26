use ui::{div, label, theme, IconKind, LabelSize, Node};

use crate::icon_button;

pub const TOAST_HEIGHT: f32 = 36.0;

/// A short message floating over the bottom of the window, with one action and a close button; returns the
/// node with its width and height, since the window places it.
pub fn toast(message: &str, action: Option<(&str, u64)>, close_id: u64) -> (Node, f32, f32) {
    let colors = theme();
    let weight = ui::ui_font_weight();
    let (pad_left, pad_right, gap, close_w) = (14.0, 6.0, 10.0, 24.0);
    let message_w = ui::measure_text_width(message, 13.0, false, weight);
    let action_w = action
        .map(|(text, _)| ui::measure_text_width(text, 12.0, false, weight) + 16.0 + gap)
        .unwrap_or(0.0);
    let width = pad_left + message_w + action_w + gap + close_w + pad_right;
    let mut row = div()
        .row()
        .items_center()
        .h_px(TOAST_HEIGHT)
        .pl(pad_left)
        .pr(pad_right)
        .gap(gap)
        .rounded(8.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border)
        .child(
            label(message.to_string())
                .size(LabelSize::Medium)
                .color(colors.text),
        );
    if let Some((text, id)) = action {
        row = row.child(
            div()
                .row()
                .h_px(24.0)
                .px(8.0)
                .rounded(6.0)
                .items_center()
                .hover_bg(colors.ghost_element_hover)
                .on_click(id)
                .child(
                    label(text.to_string())
                        .size(LabelSize::Small)
                        .medium()
                        .color(colors.text),
                ),
        );
    }
    let row = row.child(icon_button(close_id, IconKind::Close));
    (row.into(), width, TOAST_HEIGHT)
}
