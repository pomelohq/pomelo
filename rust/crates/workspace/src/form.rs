//! Modals the window owns (forms such as creating a workspace) and the pieces they are built from: the modal
//! frame, header and footer, labeled inputs, checkboxes and a progress bar, sized after the reference's.

use std::any::Any;

use crate::text_field::{FieldFont, TextField};
use crate::EditKey;
use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Rgba};

/// Click ids of the open window modal.
pub const WINDOW_MODAL_BASE: u64 = 910_000_000;
pub const WINDOW_MODAL_END: u64 = 920_000_000;

pub fn is_window_modal_id(id: u64) -> bool {
    (WINDOW_MODAL_BASE..WINDOW_MODAL_END).contains(&id)
}

/// How a modal ended; the app knows which modal it opened and downcasts the submission.
pub enum ModalResult {
    Cancelled,
    Submitted(Box<dyn Any>),
}

/// A form floating over the whole window. It takes the keyboard while open: editing keys, Tab between fields,
/// Enter to confirm and Escape to cancel.
pub trait WindowModal: 'static {
    /// Width in design px.
    fn width(&self) -> f32;
    fn render(&mut self) -> Node;
    fn click(&mut self, id: u64);
    fn key(&mut self, key: EditKey, shift: bool) -> bool;
    fn text(&mut self, text: &str) -> bool;
    fn paste(&mut self, text: &str) -> bool {
        self.text(text)
    }
    fn copy(&self) -> Option<String> {
        None
    }
    fn cut(&mut self) -> Option<String> {
        None
    }
    /// Picks up background work; returns whether anything changed.
    fn tick(&mut self) -> bool {
        false
    }
    /// Whether background work is pending, so the window keeps ticking.
    fn busy(&self) -> bool {
        false
    }
    /// Whether a press outside the modal closes it.
    fn dismissable(&self) -> bool {
        true
    }
    fn take_result(&mut self) -> Option<ModalResult>;
}

/// Content height of a one-line input: 32px box minus 6px padding top and bottom.
const INPUT_HEIGHT: f32 = 20.0;

/// The modal surface: elevated background, large rounding and a variant border.
pub fn modal_frame(width: f32) -> ui::Div {
    let colors = theme();
    div()
        .col()
        .w_px(width)
        .rounded(8.0)
        .border(1.0, colors.border_variant)
        .bg(colors.elevated_surface_background)
}

/// Headline (muted) with an optional close button that emits `close_id`.
pub fn modal_header(title: &str, close_id: Option<u64>) -> Node {
    let mut row = div()
        .row()
        .items_center()
        .justify_between()
        .px(12.0)
        .pt(8.0)
        .pb(4.0)
        .gap(8.0)
        .child(
            label(title.to_string())
                .size(16.0)
                .color(theme().text_muted),
        );
    if let Some(id) = close_id {
        row = row.child(
            div()
                .row()
                .items_center()
                .justify_center()
                .w_px(22.0)
                .h_px(22.0)
                .rounded(4.0)
                .on_click(id)
                .child(icon(IconKind::Close).size(14.0).color(theme().icon_muted)),
        );
    }
    row.into()
}

/// The form body: 12px sides, rows `gap` apart.
pub fn modal_section(gap: f32) -> ui::Div {
    div().col().px(12.0).pb(8.0).gap(gap)
}

/// Footer with a top border: `start` on the left, `end` on the right.
pub fn modal_footer(start: Option<Node>, end: Node) -> Node {
    let row = div()
        .row()
        .items_center()
        .justify_between()
        .gap(4.0)
        .p(8.0)
        .child(start.unwrap_or_else(|| div().into()))
        .child(end);
    div()
        .col()
        .child(div().h_px(1.0).bg(theme().border_variant))
        .child(row)
        .into()
}

/// A footer button with its keystroke hint after the label.
pub fn modal_button(id: u64, text: &str, keystroke: Option<&str>, enabled: bool) -> Node {
    let colors = theme();
    let mut button = div()
        .row()
        .items_center()
        .gap(4.0)
        .h_px(22.0)
        .px(4.0)
        .rounded(4.0)
        .child(
            label(text.to_string())
                .label_size(LabelSize::Default)
                .color(if enabled {
                    colors.text
                } else {
                    colors.text_disabled
                }),
        );
    if enabled {
        button = button.on_click(id);
    }
    if let Some(keystroke) = keystroke {
        button = button.child(crate::render_keystroke(keystroke, 12.0));
    }
    button.into()
}

/// A secondary action inside the form (outlined, small label).
pub fn outlined_button(id: u64, leading: Option<IconKind>, text: &str, enabled: bool) -> Node {
    let colors = theme();
    let color = if enabled {
        colors.text
    } else {
        colors.text_disabled
    };
    let mut button = div()
        .row()
        .items_center()
        .gap(4.0)
        .h_px(22.0)
        .px(6.0)
        .rounded(4.0)
        .border(1.0, colors.border_variant)
        .bg(colors.element_background);
    if enabled {
        button = button.on_click(id);
    }
    if let Some(kind) = leading {
        button = button.child(icon(kind).size(12.0).color(if enabled {
            colors.icon_muted
        } else {
            colors.text_disabled
        }));
    }
    button
        .child(
            label(text.to_string())
                .label_size(LabelSize::Small)
                .color(color),
        )
        .into()
}

/// A text input with its label above, after the reference's input field: 32px box on the editor background,
/// variant border that turns focused (or error) colored, and the error message below.
pub struct InputField {
    pub field: TextField,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub font: FieldFont,
}

impl InputField {
    pub fn new(label: &'static str, placeholder: &'static str) -> InputField {
        InputField {
            field: TextField::default(),
            label,
            placeholder,
            font: FieldFont::Ui,
        }
    }

    pub fn mono(mut self) -> InputField {
        self.font = FieldFont::Mono;
        self
    }

    pub fn text(&self) -> String {
        self.field.text()
    }

    pub fn render(
        &self,
        click_id: u64,
        focused: bool,
        hint: Option<&str>,
        error: Option<&str>,
    ) -> Node {
        let colors = theme();
        let border = match (error.is_some(), focused) {
            (true, _) => colors.error_border,
            (false, true) => colors.border_focused,
            (false, false) => colors.border_variant,
        };
        let mut title = div().row().items_center().gap(6.0).child(
            label(self.label.to_string())
                .label_size(LabelSize::Small)
                .color(colors.text),
        );
        if let Some(hint) = hint {
            title = title.child(
                label(hint.to_string())
                    .label_size(LabelSize::Small)
                    .color(colors.text_muted),
            );
        }
        let input = div()
            .row()
            .items_center()
            .h_px(INPUT_HEIGHT + 12.0)
            .px(8.0)
            .rounded(6.0)
            .bg(colors.editor_background)
            .border(1.0, border)
            .on_click(click_id)
            .child(self.field.render(
                self.placeholder,
                focused,
                colors.text,
                INPUT_HEIGHT,
                self.font,
            ));
        let mut column = div().col().gap(4.0).child(title).child(input);
        if let Some(error) = error {
            column = column.child(
                label(error.to_string())
                    .label_size(LabelSize::Small)
                    .color(colors.error),
            );
        }
        column.into()
    }
}

/// A ghost checkbox (16px box, 6px to its muted label), the whole row clickable.
pub fn checkbox(id: u64, checked: bool, text: &str) -> Node {
    let colors = theme();
    let mut square = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(16.0)
        .h_px(16.0)
        .rounded(2.0)
        .border(1.0, colors.border);
    if checked {
        square = square.child(icon(IconKind::Check).size(14.0).color(colors.text_accent));
    }
    div()
        .row()
        .items_center()
        .gap(6.0)
        .h_px(22.0)
        .on_click(id)
        .child(
            div()
                .row()
                .items_center()
                .justify_center()
                .w_px(20.0)
                .child(square),
        )
        .child(
            label(text.to_string())
                .label_size(LabelSize::Default)
                .color(colors.text_muted),
        )
        .into()
}

/// A determinate bar: 8px track with 2px inset, filled in the info color (never below 2%).
pub fn progress_bar(value: f32) -> Node {
    let colors = theme();
    let fraction = value.clamp(0.02, 1.0);
    div()
        .row()
        .h_px(8.0)
        .p(2.0)
        .rounded(4.0)
        .bg(colors.background)
        .child(div().h_px(4.0).flex(fraction).rounded(2.0).bg(colors.info))
        .child(div().h_px(4.0).flex(1.0 - fraction))
        .into()
}

/// A small muted status line: a leading icon and truncated text.
pub fn status_line(kind: IconKind, color: Rgba, text: &str) -> Node {
    div()
        .row()
        .items_center()
        .gap(4.0)
        .child(icon(kind).size(12.0).color(color))
        .child(
            label(text.to_string())
                .label_size(LabelSize::Small)
                .color(theme().text_muted)
                .truncate(),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_modal_ids_have_their_own_range() {
        assert!(is_window_modal_id(WINDOW_MODAL_BASE + 5));
        assert!(!is_window_modal_id(crate::SIDE_PANEL_BASE));
        assert!(!is_window_modal_id(WINDOW_MODAL_END));
    }
}
