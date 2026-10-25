use ui::{div, theme, Div};

/// A 1px horizontal divider in the default border color.
pub fn divider() -> Div {
    div().h_px(1.0).bg(theme().border_variant)
}

/// The stronger line where a bar (a toolbar, a tray) ends and the content under it starts.
pub fn edge_divider() -> Div {
    div().h_px(1.0).bg(theme().border)
}

/// A 1px vertical divider between groups in a row.
pub fn vertical_divider(height: f32) -> Div {
    div().w_px(1.0).h_px(height).bg(theme().border_variant)
}
