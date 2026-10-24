use ui::{div, theme, Div};

/// A 1px horizontal divider in the default border color.
pub fn divider() -> Div {
    div().h_px(1.0).bg(theme().border_variant)
}
