use ui::{div, Div, Rgba};

/// A round status mark: running, failing, needs a look.
pub fn dot(color: Rgba, size: f32) -> Div {
    div().w_px(size).h_px(size).rounded_px(size / 2.0).bg(color)
}

/// The hollow form of [`dot`], for a state that has not started.
pub fn ring(color: Rgba, size: f32) -> Div {
    div()
        .w_px(size)
        .h_px(size)
        .rounded_px(size / 2.0)
        .border(1.5, color)
}
