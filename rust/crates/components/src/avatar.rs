use ui::{div, label, theme, Node};

/// A person, by the first letter of their name in a circle: a commit's author, a reviewer.
pub fn avatar(name: &str) -> Node {
    let colors = theme();
    let initial: String = name.chars().take(1).flat_map(char::to_uppercase).collect();
    div()
        .w_px(18.0)
        .h_px(18.0)
        .rounded(9.0)
        .items_center()
        .justify_center()
        .bg(colors.element_hover)
        .child(
            label(initial)
                .size(10.0)
                .weight(600)
                .color(colors.text_muted),
        )
        .into()
}
