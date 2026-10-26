use ui::{div, label, theme, Div, IconKind, LabelSize, Node, Radius, Rgba, Space};

pub const MODAL_RADIUS: Radius = Radius::XLarge;
/// Horizontal inset of the header title and the body.
pub const MODAL_INSET: Space = Space::S20;

pub fn overlay_ring() -> Rgba {
    theme().overlay_ring()
}

/// The modal surface: the overlay layer, large rounding and a hairline ring.
pub fn modal_frame(width: f32) -> Div {
    div()
        .col()
        .w_px(width)
        .rounded(MODAL_RADIUS)
        .ring(1.0, overlay_ring())
        .bg(theme().elevated_surface_background)
}

/// The title (heading weight) with an optional close button that emits `close_id`.
pub fn modal_header(title: &str, close_id: Option<u64>) -> Node {
    let colors = theme();
    let mut row = div()
        .row()
        .items_center()
        .justify_between()
        .pl(MODAL_INSET)
        .pr(Space::S12)
        .pt(Space::S14)
        .pb(Space::S6)
        .gap(Space::S8)
        .child(
            div().row().flex(1.0).items_center().child(
                label(title.to_string())
                    .size(LabelSize::Large)
                    .weight(600)
                    .color(colors.text)
                    .truncate(),
            ),
        );
    if let Some(id) = close_id {
        row = row.child(crate::icon_button(id, IconKind::Close));
    }
    row.into()
}

/// The form body: the modal's inset at the sides, groups `gap` apart.
pub fn modal_section(gap: f32) -> Div {
    div()
        .col()
        .px(MODAL_INSET)
        .pt(Space::S10)
        .pb(Space::S20)
        .gap(Space::Px(gap))
}

/// The footer bar on the panel layer: `start` (a note) on the left, `end` (the buttons) on the right.
pub fn modal_footer(start: Option<Node>, end: Node) -> Node {
    let colors = theme();
    let row = div()
        .row()
        .items_center()
        .gap(Space::S8)
        .px(Space::S14)
        .pt(Space::S2)
        .pb(Space::S12)
        .child(
            div()
                .row()
                .flex(1.0)
                .items_center()
                .child(start.unwrap_or_else(|| div().into())),
        )
        .child(end);
    // The rounded bar keeps the frame's bottom corners; the strip above squares its top ones.
    div()
        .col()
        .rounded(MODAL_RADIUS)
        .bg(colors.panel_background)
        .child(div().h_px(10.0).bg(colors.panel_background))
        .child(row)
        .into()
}

/// A muted one-line note for the footer's start.
pub fn modal_note(text: &str) -> Node {
    label(text.to_string())
        .size(LabelSize::Small)
        .color(theme().text_placeholder)
        .truncate()
        .into()
}
