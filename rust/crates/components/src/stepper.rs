use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Radius, Rgba, Space};

/// A number with a minus and a plus beside it; clicking the number edits it in place.
pub struct Stepper {
    value: String,
    dec: u64,
    inc: u64,
    edit: u64,
    draft: Option<String>,
}

pub fn stepper(value: impl Into<String>, dec: u64, inc: u64, edit: u64) -> Stepper {
    Stepper {
        value: value.into(),
        dec,
        inc,
        edit,
        draft: None,
    }
}

impl Stepper {
    /// The text being typed while the number is edited.
    pub fn editing(mut self, draft: Option<&str>) -> Self {
        self.draft = draft.map(str::to_string);
        self
    }
}

impl From<Stepper> for Node {
    fn from(stepper: Stepper) -> Node {
        let colors = theme();
        let editing = stepper.draft.is_some();
        let shown = stepper.draft.unwrap_or(stepper.value);
        let mut value_box = div()
            .row()
            .h_px(28.0)
            .w_px(42.0)
            .gap(Space::Px(1.0))
            .items_center()
            .justify_center()
            .on_click(stepper.edit)
            .child(
                label(shown)
                    .size(LabelSize::Medium)
                    .mono()
                    .color(colors.text),
            );
        if editing {
            // The caret keeps its slot while it blinks so the number does not shift.
            value_box = value_box.child(caret(ui::caret_phase()));
        }
        div()
            .row()
            .h_px(28.0)
            .items_center()
            .rounded(Radius::Medium)
            .bg(colors.element_background)
            .border(
                1.0,
                if editing {
                    colors.border_focused
                } else {
                    colors.border
                },
            )
            .child(step(IconKind::Dash, stepper.dec))
            .child(value_box)
            .child(step(IconKind::Plus, stepper.inc))
            .into()
    }
}

fn caret(visible: bool) -> Node {
    div()
        .w_px(1.5)
        .h_px(15.0)
        .bg(if visible {
            theme().text
        } else {
            Rgba::TRANSPARENT
        })
        .into()
}

fn step(kind: IconKind, id: u64) -> Node {
    div()
        .w_px(28.0)
        .h_px(28.0)
        .items_center()
        .justify_center()
        .on_click(id)
        .child(icon(kind).size(12.0).color(theme().text_muted))
        .into()
}
