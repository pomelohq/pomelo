//! Modals the window owns (forms such as creating a workspace) and the pieces they are built from: the modal
//! frame, header and footer, labeled inputs, checkboxes and a progress bar, sized after the reference's.

use std::any::Any;

use crate::text_field::{FieldFont, TextField};
use crate::EditKey;
use ui::{button, div, icon, label, theme, ButtonKind, IconKind, LabelSize, Node, Rgba};

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
    /// The modal's control under the pointer changed; returns whether it needs a repaint.
    fn hover(&mut self, _id: Option<u64>) -> bool {
        false
    }
    /// Whether `id` shows the pointer cursor (backgrounds and text fields keep the arrow).
    fn is_pointer(&self, _id: u64) -> bool {
        true
    }
    /// Whether a press outside the modal closes it.
    fn dismissable(&self) -> bool {
        true
    }
    fn take_result(&mut self) -> Option<ModalResult>;
}

/// Content height of a one-line input: 30px box minus 5px padding top and bottom.
const INPUT_HEIGHT: f32 = 20.0;
const INPUT_FONT: f32 = 13.0;
const MODAL_RADIUS: f32 = 10.0;
/// Horizontal inset of the header title and the body.
const MODAL_INSET: f32 = 18.0;

/// The hairline ring around a floating surface; on light layers it needs a dark tint to stand off the page.
pub fn overlay_ring() -> Rgba {
    match theme().appearance {
        ui::Appearance::Dark => Rgba::new(1.0, 1.0, 1.0, 0.08),
        ui::Appearance::Light => Rgba::hex("#1d17301f"),
    }
}

/// The modal surface: the overlay layer, large rounding and a hairline ring.
pub fn modal_frame(width: f32) -> ui::Div {
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
        .pr(12.0)
        .pt(14.0)
        .pb(6.0)
        .gap(8.0)
        .child(
            div().row().flex(1.0).items_center().child(
                label(title.to_string())
                    .size(15.0)
                    .weight(600)
                    .color(colors.text)
                    .truncate(),
            ),
        );
    if let Some(id) = close_id {
        row = row.child(
            div()
                .row()
                .items_center()
                .justify_center()
                .w_px(28.0)
                .h_px(24.0)
                .rounded(6.0)
                .on_click(id)
                .hover_bg(colors.ghost_element_hover)
                .child(icon(IconKind::Close).size(14.0).color(colors.icon)),
        );
    }
    row.into()
}

/// The form body: the modal's inset at the sides, groups `gap` apart.
pub fn modal_section(gap: f32) -> ui::Div {
    div().col().px(MODAL_INSET).pt(10.0).pb(18.0).gap(gap)
}

/// The footer bar on the panel layer: `start` (a note) on the left, `end` (the buttons) on the right.
pub fn modal_footer(start: Option<Node>, end: Node) -> Node {
    let colors = theme();
    let row = div()
        .row()
        .items_center()
        .gap(8.0)
        .px(14.0)
        .pt(2.0)
        .pb(12.0)
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
        .size(12.0)
        .color(theme().text_placeholder)
        .truncate()
        .into()
}

/// An inline link-like action in the accent color, with an optional leading icon.
pub fn accent_action(id: u64, leading: Option<IconKind>, text: &str, enabled: bool) -> Node {
    let colors = theme();
    let color = if enabled {
        colors.text_accent
    } else {
        colors.text_disabled
    };
    let mut action = div().row().items_center().gap(6.0).h_px(16.0);
    if enabled {
        action = action.on_click(id);
    }
    if let Some(kind) = leading {
        action = action.child(icon(kind).size(12.0).color(color));
    }
    action
        .child(label(text.to_string()).size(12.0).color(color))
        .into()
}

/// A field's label: small, uppercase and muted, as every form labels its inputs.
pub fn field_label(text: &str) -> Node {
    text_line(
        14.0,
        label(text.to_uppercase())
            .size(11.0)
            .weight(600)
            .color(theme().text_placeholder),
    )
}

/// `text` in a box of the design's line height, so stacked lines keep the rhythm the type scale sets.
pub fn text_line(height: f32, text: ui::Label) -> Node {
    div().row().h_px(height).items_center().child(text).into()
}

/// The line under a field: what it is for, or what is wrong with it.
fn field_note(hint: Option<&str>, error: Option<&str>) -> Option<Node> {
    let colors = theme();
    match (error, hint) {
        (Some(error), _) => Some(text_line(
            16.0,
            label(error.to_string()).size(12.0).color(colors.error),
        )),
        // A long hint (a path) keeps its end, the part that differs, and never pushes past the field.
        (None, Some(hint)) if !hint.is_empty() => Some(text_line(
            16.0,
            label(hint.to_string())
                .size(12.0)
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
        .child(field_label(title))
        .child(control);
    if let Some(note) = field_note(hint, error) {
        column = column.child(note);
    }
    column.into()
}

/// Width of a confirmation in design px.
pub const PROMPT_WIDTH: f32 = 460.0;

/// Answers that lose something, so their button reads as destructive.
const DESTRUCTIVE: [&str; 9] = [
    "Delete", "Discard", "Remove", "Restore", "Trash", "Drop", "Reset", "Prepare", "Force",
];

/// A confirmation as a modal: the question as the title, the detail as the body (one line each, "- " a
/// bullet), and one footer button per answer, the first (confirming) one last. Enter takes
/// `active`, Escape the one named Cancel; button `i` emits `button_base + i`.
pub fn prompt_card(prompt: &crate::Prompt, active: usize, button_base: u64) -> Node {
    let colors = theme();
    let text_width = PROMPT_WIDTH - 2.0 * MODAL_INSET;
    let mut body = modal_section(10.0);
    let detail = prompt.detail.as_deref().unwrap_or_default();
    let bullet = |item: &str| -> Node {
        div()
            .row()
            .gap(10.0)
            .pl(6.0)
            .child(
                div()
                    .row()
                    .h_px(20.0)
                    .items_center()
                    .child(div().w_px(4.0).h_px(4.0).rounded(2.0).bg(colors.text_muted)),
            )
            .child(body_text(item, text_width - 18.0))
            .into()
    };
    // Lines stand 10px apart; a run of bullets holds together 4px apart.
    let mut list: Option<ui::Div> = None;
    for line in detail.lines().filter(|line| !line.trim().is_empty()) {
        match line.trim_start().strip_prefix("- ") {
            Some(item) => {
                list = Some(
                    list.unwrap_or_else(|| div().col().gap(4.0))
                        .child(bullet(item)),
                );
            }
            None => {
                if let Some(done) = list.take() {
                    body = body.child(done);
                }
                body = body.child(body_text(line, text_width));
            }
        }
    }
    if let Some(done) = list {
        body = body.child(done);
    }
    let mut buttons = div().row().items_center().gap(8.0);
    for (index, text) in prompt.buttons.iter().enumerate().rev() {
        let cancel = text == "Cancel";
        let kind = match index {
            0 if DESTRUCTIVE
                .iter()
                .any(|word| text.split_whitespace().next() == Some(*word)) =>
            {
                ButtonKind::Danger
            }
            0 => ButtonKind::Primary,
            _ => ButtonKind::Secondary,
        };
        let keystroke = if index == active {
            Some("enter")
        } else if cancel {
            Some("escape")
        } else {
            None
        };
        buttons = buttons.child(
            button(button_base + index as u64, text)
                .size(ui::ButtonSize::Medium)
                .kind(kind)
                .keystroke(keystroke),
        );
    }
    let cancel = prompt
        .buttons
        .iter()
        .position(|text| text == "Cancel")
        .map(|index| button_base + index as u64);
    if let Some(warning) = &prompt.warning {
        body = body.child(
            div()
                .row()
                .gap(8.0)
                .px(12.0)
                .py(10.0)
                .rounded(6.0)
                .bg(colors.editor_background)
                .child(
                    div()
                        .row()
                        .h_px(20.0)
                        .items_center()
                        .child(icon(IconKind::Warning).size(14.0).color(colors.warning)),
                )
                .child(paragraph(
                    warning,
                    13.0,
                    20.0,
                    text_width - 46.0,
                    colors.text,
                )),
        );
    }
    let mut card = modal_frame(PROMPT_WIDTH).child(modal_header(&prompt.message, cancel));
    if !detail.trim().is_empty() || prompt.warning.is_some() {
        card = card.child(body);
    } else {
        card = card.child(div().h_px(12.0));
    }
    card.child(modal_footer(None, buttons.into())).into()
}

/// `text` broken at spaces into lines that fit `width`, each `line_height` tall, so wrapped copy keeps the
/// type scale's leading.
pub fn paragraph(text: &str, size: f32, line_height: f32, width: f32, color: Rgba) -> Node {
    let fits = |candidate: &str| {
        ui::measure_text_width(candidate, size, false, ui::ui_font_weight()) / ui::ui_text_scale()
            <= width
    };
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if fits(&format!("{line} {word}")) => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    let mut column = div().col();
    for line in lines {
        column = column.child(text_line(line_height, label(line).size(size).color(color)));
    }
    column.into()
}

/// Muted body copy at the body line height, wrapping only when it does not fit `width`. A run between `**`
/// marks is set strong, in the text color: the thing the sentence is about.
fn body_text(text: &str, width: f32) -> Node {
    let colors = theme();
    let plain = text.replace("**", "");
    let fits = ui::measure_text_width(&plain, 13.0, false, ui::ui_font_weight())
        / ui::ui_text_scale()
        <= width;
    if !fits {
        return paragraph(&plain, 13.0, 20.0, width, colors.text_muted);
    }
    let mut row = div().row().h_px(20.0).items_center();
    for (index, run) in text.split("**").enumerate() {
        if run.is_empty() {
            continue;
        }
        let run = label(run.to_string()).size(13.0);
        row = row.child(if index % 2 == 1 {
            run.weight(600).color(colors.text)
        } else {
            run.color(colors.text_muted)
        });
    }
    row.into()
}

/// A text input with its label above and its hint or error below: a 30px box on the surface layer whose ring
/// turns accent while focused (or error colored).
pub struct InputField {
    pub field: TextField,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub font: FieldFont,
    pub masked: bool,
    pub leading: Option<IconKind>,
}

impl InputField {
    pub fn new(label: &'static str, placeholder: &'static str) -> InputField {
        let mut field = TextField::default();
        field.set_font_size(INPUT_FONT);
        InputField {
            field,
            label,
            placeholder,
            font: FieldFont::Ui,
            masked: false,
            leading: None,
        }
    }

    /// Draws `*` per character, for passwords.
    pub fn masked(mut self) -> InputField {
        self.masked = true;
        self
    }

    pub fn mono(mut self) -> InputField {
        self.font = FieldFont::Mono;
        self
    }

    /// An icon inside the box before the text, saying what goes in it.
    pub fn with_icon(mut self, kind: IconKind) -> InputField {
        self.leading = Some(kind);
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
        field(
            self.label,
            self.render_input(click_id, focused, error.is_some()),
            hint,
            error,
        )
    }
}

impl InputField {
    /// Just the input box, for forms that lay out the title themselves.
    pub fn render_input(&self, click_id: u64, focused: bool, error: bool) -> Node {
        self.render_input_with(click_id, focused, error, None)
    }

    /// The input box with muted `suffix` after the text, such as what a typed key stands for.
    pub fn render_input_with(
        &self,
        click_id: u64,
        focused: bool,
        error: bool,
        suffix: Option<&str>,
    ) -> Node {
        let colors = theme();
        let border = match (error, focused) {
            (true, _) => colors.error,
            (false, true) => colors.border_focused,
            (false, false) => colors.border,
        };
        let mut row = div()
            .row()
            .items_center()
            .gap(8.0)
            .h_px(INPUT_HEIGHT + 10.0)
            .px(10.0)
            .rounded(6.0)
            .bg(colors.editor_background)
            .border(1.0, border)
            .on_click(click_id);
        if focused && !error {
            row = row.ring(3.0, colors.overlay_selected);
        }
        if let Some(kind) = self.leading {
            row = row.child(icon(kind).size(14.0).color(colors.text_placeholder));
        }
        // Away from the box, a known value reads with what it stands for; editing shows only the value.
        if let Some(suffix) = suffix.filter(|_| !focused) {
            return row
                .child(label(self.field.text()).size(INPUT_FONT).color(colors.text))
                .child(
                    div().row().flex(1.0).items_center().child(
                        label(suffix.to_string())
                            .size(INPUT_FONT)
                            .color(colors.text_placeholder)
                            .truncate(),
                    ),
                )
                .into();
        }
        row = row.child(match self.dots() {
            Some(dots) => dots.render(
                self.placeholder,
                focused,
                colors.text,
                INPUT_HEIGHT,
                self.font,
            ),
            None => self.field.render(
                self.placeholder,
                focused,
                colors.text,
                INPUT_HEIGHT,
                self.font,
            ),
        });
        row.into()
    }

    fn dots(&self) -> Option<TextField> {
        if !self.masked || self.field.text().is_empty() {
            return None;
        }
        let mut dots = TextField::default();
        dots.set_font_size(INPUT_FONT);
        dots.set_text(&"*".repeat(self.field.text().chars().count()));
        dots.move_to_end();
        Some(dots)
    }
}

/// A checkbox (14px box, accent filled when on) and its label, the whole row clickable.
pub fn checkbox(id: u64, checked: bool, text: &str) -> Node {
    let colors = theme();
    let mut square = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(14.0)
        .h_px(14.0)
        .rounded(4.0);
    square = if checked {
        square
            .bg(colors.accent_fill)
            .child(icon(IconKind::Check).size(10.0).color(colors.on_accent))
    } else {
        square.border(1.0, colors.border)
    };
    let mut row = div()
        .row()
        .items_center()
        .gap(8.0)
        .h_px(16.0)
        .on_click(id)
        .child(square);
    if !text.is_empty() {
        row = row.child(label(text.to_string()).size(12.0).color(colors.text));
    }
    row.into()
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

/// Compact mutually exclusive choices on a tinted track, for toolbars; the chosen one sits on a raised chip.
pub fn segmented(options: &[(u64, &str)], selected: usize, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut row = div()
        .row()
        .p(2.0)
        .gap(2.0)
        .rounded(5.0)
        .bg(colors.element_background);
    for (index, (id, text)) in options.iter().enumerate() {
        let on = index == selected;
        let hot = !on && hovered == Some(*id);
        let mut part = div()
            .row()
            .px(10.0)
            .py(3.0)
            .rounded(4.0)
            .on_click(*id)
            .child(
                label(text.to_string())
                    .label_size(LabelSize::Small)
                    .color(if on || hot {
                        colors.text
                    } else {
                        colors.text_muted
                    }),
            );
        if on {
            part = part.bg(colors.element_selected);
        } else if hot {
            // The ghost hover token is darker than this track in some dark themes, so tint from the text color.
            part = part.bg(colors.text.alpha(0.045));
        }
        row = row.child(part);
    }
    row.into()
}

/// Mutually exclusive choices on a sunken track; the chosen one sits on a raised chip. `choices` are (click id,
/// leading icon, label).
pub fn toggle_button_group(choices: &[(u64, Option<IconKind>, &str)], selected: usize) -> Node {
    let colors = theme();
    let mut group = div()
        .row()
        .p(2.0)
        .gap(2.0)
        .rounded(6.0)
        .bg(colors.background);
    for (index, (id, leading, text)) in choices.iter().enumerate() {
        let chosen = index == selected;
        let ink = if chosen {
            colors.text
        } else {
            colors.text_muted
        };
        let mut cell = div()
            .row()
            .h_px(26.0)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .rounded(4.0)
            .on_click(*id);
        if chosen {
            cell = cell.bg(colors.element_background);
        }
        if let Some(kind) = leading {
            cell = cell.child(icon(*kind).size(12.0).color(ink));
        }
        group = group.child(cell.child(label(text.to_string()).size(12.0).color(ink)));
    }
    div().row().child(group).into()
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
