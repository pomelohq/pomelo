use ui::{div, label, theme, Div, Label, LabelSize, Node, Radius};

pub const INPUT_HEIGHT: f32 = 30.0;

/// The box every one-line input sits in: the editor surface, ringed in accent while focused or in the
/// error color; the caller puts the text field inside.
pub fn input_box(click_id: u64, focused: bool, error: bool) -> Div {
    let colors = theme();
    let border = match (error, focused) {
        (true, _) => colors.error,
        (false, true) => colors.border_focused,
        (false, false) => colors.border,
    };
    let frame = div()
        .row()
        .items_center()
        .gap(8.0)
        .h_px(INPUT_HEIGHT)
        .px(10.0)
        .rounded(Radius::Medium)
        .bg(colors.editor_background)
        .border(1.0, border)
        .on_click(click_id);
    if focused && !error {
        frame.ring(3.0, colors.overlay_selected)
    } else {
        frame
    }
}

/// `text` in a box of the design's line height, so stacked lines keep the rhythm the type scale sets.
pub fn text_line(height: f32, text: Label) -> Node {
    div().row().h_px(height).items_center().child(text).into()
}

/// The line under a field: what it is for, or what is wrong with it.
fn field_note(hint: Option<&str>, error: Option<&str>) -> Option<Node> {
    let colors = theme();
    match (error, hint) {
        (Some(error), _) => Some(text_line(
            16.0,
            label(error.to_string())
                .size(LabelSize::Small)
                .color(colors.error),
        )),
        // A long hint (a path) keeps its end, the part that differs, and never pushes past the field.
        (None, Some(hint)) if !hint.is_empty() => Some(text_line(
            16.0,
            label(hint.to_string())
                .size(LabelSize::Small)
                .color(colors.text_placeholder)
                .truncate_start(),
        )),
        _ => None,
    }
}

/// A labeled group in a form: the label, the control, and the hint or error under it.
pub fn field(title: &str, control: Node, hint: Option<&str>, error: Option<&str>) -> Node {
    let mut column = div()
        .col()
        .gap(6.0)
        .child(crate::section_header(title))
        .child(control);
    if let Some(note) = field_note(hint, error) {
        column = column.child(note);
    }
    column.into()
}

pub const SEARCH_HEIGHT: f32 = 26.0;

/// The small box a panel filters its list with: a search icon, then the caller's text field.
pub fn search_box(click_id: u64, focused: bool) -> Div {
    let colors = theme();
    div()
        .row()
        .items_center()
        .gap(6.0)
        .h_px(SEARCH_HEIGHT)
        .px(8.0)
        .rounded(Radius::Medium)
        .bg(colors.editor_background)
        .border(
            1.0,
            if focused {
                colors.border_focused
            } else {
                colors.border_variant
            },
        )
        .on_click(click_id)
        .child(
            ui::icon(ui::IconKind::Search)
                .size(12.0)
                .color(colors.text_placeholder),
        )
}
