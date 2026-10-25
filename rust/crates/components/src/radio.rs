use ui::{div, theme, Node};

use crate::dot;

/// A 14px radio mark: a hairline ring, or the accent fill with a dot when chosen.
pub fn radio(on: bool) -> Node {
    let colors = theme();
    let mark = div().row().w_px(14.0).h_px(14.0).rounded(7.0);
    if on {
        mark.items_center()
            .justify_center()
            .bg(colors.accent_fill)
            .child(dot(colors.on_accent, 6.0))
            .into()
    } else {
        mark.border(1.0, colors.border).into()
    }
}
