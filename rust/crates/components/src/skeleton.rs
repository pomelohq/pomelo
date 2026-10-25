use ui::{div, theme, Node};

const PERIOD: f32 = 1.2;

/// A bar standing where a value will be once it is read, pulsing so it reads as on its way. `seconds` is
/// the screen's clock; `delay` staggers a column of them.
pub fn skeleton(width: f32, seconds: f32, delay: f32) -> Node {
    let mut fill = theme().text_muted;
    fill.a = if ui::reduce_motion() {
        0.14
    } else {
        let phase = (seconds / PERIOD - delay) * std::f32::consts::TAU;
        0.10 + 0.08 * (0.5 + 0.5 * phase.sin())
    };
    div().w_px(width).h_px(10.0).rounded(5.0).bg(fill).into()
}
