use components::ButtonKind;
use std::collections::HashMap;

use pom_secrets::SecretStore;
use terminal::{Keystroke, Modifiers};
use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Radius, Rect, Space};
use workspace::text_field::TextField;
use workspace::{EditKey, InputField, Item, ItemTick, TerminalKeyOutcome};

use crate::{action_button, hit_at, EnvironmentContext};

pub const ITEM_ID: &str = "secrets";
const ROW_H: f32 = 34.0;
const NAME_FIELD: u64 = 1;
const VALUE_FIELD: u64 = 2;
const ADD: u64 = 3;
const ROW_BASE: u64 = 100;
const REVEAL: u64 = 0;
const COPY: u64 = 1;
const DELETE: u64 = 2;
/// The row itself, so hovering anywhere on it shows its controls' tint.
const ROW: u64 = 3;
const ROW_STRIDE: u64 = 4;
const DOC_W: f32 = 820.0;
const NAME_W: f32 = 220.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Name,
    Value,
}

pub struct SecretsItem {
    context: EnvironmentContext,
    names: Vec<String>,
    revealed: HashMap<String, String>,
    /// The secret whose delete button was pressed once; a second press deletes it.
    armed: Option<String>,
    name: InputField,
    value: InputField,
    focus: Option<Focus>,
    focused: bool,
    error: Option<String>,
    copy: Option<String>,
    scroll: f32,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
}

impl SecretsItem {
    pub fn new(context: EnvironmentContext) -> SecretsItem {
        let mut item = SecretsItem {
            context,
            names: Vec::new(),
            revealed: HashMap::new(),
            armed: None,
            name: InputField::new("Name", "NAME"),
            value: InputField::new("Value", "value").masked(),
            focus: Some(Focus::Name),
            focused: false,
            error: None,
            copy: None,
            scroll: 0.0,
            hits: Vec::new(),
            hovered: None,
        };
        item.reload();
        item
    }

    fn store(&self) -> SecretStore {
        SecretStore::new(self.context.state.clone(), self.context.session())
    }

    fn reload(&mut self) {
        match self.store().names() {
            Ok(mut names) => {
                names.sort();
                self.names = names;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Could not read secrets: {error}")),
        }
    }

    fn value_of(&self, name: &str) -> Option<String> {
        self.store().get(name).ok().flatten()
    }

    /// Adds the typed secret (or replaces one of the same name).
    pub fn add(&mut self) {
        let name = self.name.text().trim().to_string();
        let value = self.value.text();
        if name.is_empty() || value.is_empty() {
            return;
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.error =
                Some("Names use letters, digits and _ only (as in {{secret.NAME}})".into());
            return;
        }
        if let Err(error) = self.store().set(&name, &value) {
            self.error = Some(format!("Could not save {name}: {error}"));
            return;
        }
        self.name.field.set_text("");
        self.value.field.set_text("");
        self.focus = Some(Focus::Name);
        self.reload();
    }

    fn delete(&mut self, name: &str) {
        // An empty value removes the secret, as the store's other writers expect.
        if let Err(error) = self.store().set(name, "") {
            self.error = Some(format!("Could not delete {name}: {error}"));
        }
        self.revealed.remove(name);
        self.armed = None;
        self.reload();
    }

    fn click(&mut self, id: u64) {
        match id {
            NAME_FIELD => self.focus = Some(Focus::Name),
            VALUE_FIELD => self.focus = Some(Focus::Value),
            ADD => self.add(),
            id if id >= ROW_BASE => {
                let offset = id - ROW_BASE;
                let Some(name) = self.names.get((offset / ROW_STRIDE) as usize).cloned() else {
                    return;
                };
                match offset % ROW_STRIDE {
                    REVEAL if self.revealed.remove(&name).is_none() => {
                        let value = self.value_of(&name).unwrap_or_default();
                        self.revealed.insert(name, value);
                    }
                    COPY => self.copy = self.value_of(&name),
                    DELETE if self.armed.as_deref() == Some(name.as_str()) => self.delete(&name),
                    DELETE => self.armed = Some(name),
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        self.armed = None;
    }

    fn row(&self, index: usize, name: &str) -> Node {
        let colors = theme();
        let base = ROW_BASE + index as u64 * ROW_STRIDE;
        let hot = |id: u64| self.hovered == Some(id);
        let mut row = div()
            .row()
            .h_px(ROW_H - 1.0)
            .px(Space::S14)
            .gap(Space::S12)
            .items_center()
            .on_click(base + ROW)
            .child(
                icon(IconKind::Key)
                    .size(12.0)
                    .color(colors.text_placeholder),
            )
            .child(
                label(format!("{{{{secret.{name}}}}}"))
                    .size(LabelSize::Small)
                    .mono()
                    .color(colors.text_accent),
            )
            .child(div().row().flex(1.0));
        if let Some(value) = self.revealed.get(name) {
            row = row.child(
                div()
                    .row()
                    .h_px(20.0)
                    .px(Space::S8)
                    .items_center()
                    .rounded(Radius::Small)
                    .bg(colors.background)
                    .border(1.0, colors.border_variant)
                    .child(
                        label(if value.is_empty() {
                            "(empty)".to_string()
                        } else {
                            value.clone()
                        })
                        .size(LabelSize::Small)
                        .mono()
                        .color(colors.text_muted)
                        .truncate(),
                    ),
            );
        }
        let revealed = self.revealed.contains_key(name);
        row = row.child(
            div()
                .row()
                .gap(Space::S10)
                .items_center()
                .child(action_button(
                    base + REVEAL,
                    IconKind::Eye,
                    if revealed {
                        colors.text_accent
                    } else {
                        colors.text_placeholder
                    },
                    hot(base + REVEAL),
                ))
                .child(action_button(
                    base + COPY,
                    IconKind::Copy,
                    colors.text_placeholder,
                    hot(base + COPY),
                )),
        );
        if self.armed.as_deref() == Some(name) {
            row = row.child(
                div()
                    .row()
                    .h_px(20.0)
                    .px(Space::S8)
                    .items_center()
                    .rounded(Radius::Small)
                    .bg(colors.error)
                    .on_click(base + DELETE)
                    .child(
                        label("Delete?")
                            .size(LabelSize::Small)
                            .weight(600)
                            .color(colors.background),
                    ),
            );
        } else {
            row = row.child(action_button(
                base + DELETE,
                IconKind::Trash,
                colors.error,
                hot(base + DELETE),
            ));
        }
        if self
            .hovered
            .is_some_and(|id| id >= base && id < base + ROW_STRIDE)
        {
            row = row.bg(colors.element_hover);
        }
        div().col().child(row).child(components::divider()).into()
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus? {
            Focus::Name => Some(&mut self.name.field),
            Focus::Value => Some(&mut self.value.field),
        }
    }

    /// The names listed now (tests).
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Types into the add form (tests).
    pub fn type_new(&mut self, name: &str, value: &str) {
        self.name.field.set_text(name);
        self.value.field.set_text(value);
        self.name.field.move_to_end();
    }

    /// Shows `revealed` with its value and `armed` waiting for its second delete press (previews).
    pub fn show_states(&mut self, revealed: (&str, &str), armed: &str) {
        self.revealed
            .insert(revealed.0.to_string(), revealed.1.to_string());
        self.armed = Some(armed.to_string());
    }
}

impl Item for SecretsItem {
    fn id(&self) -> Option<String> {
        Some(ITEM_ID.into())
    }

    fn title(&self) -> String {
        "Secrets".into()
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Key)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let colors = theme();
        let scale = ui::ui_text_scale();
        let (width, height) = (body.w / scale, body.h / scale);
        let doc_w = width.min(DOC_W);
        let intro = label("Encrypted app-local values for {{secret.NAME}} referenced in your config. Names are listed; values show only when you ask.")
            .size(LabelSize::Small)
            .color(colors.text_muted)
            .wrap(doc_w - 48.0);
        let mut list = div()
            .col()
            .rounded(Radius::Medium)
            .bg(colors.panel_background);
        if self.names.is_empty() {
            list = list.child(
                div().row().h_px(ROW_H).px(Space::S14).items_center().child(
                    label("No secrets yet.")
                        .size(LabelSize::Small)
                        .color(colors.text_muted),
                ),
            );
        }
        let first = (self.scroll / ROW_H) as usize;
        let visible = ((height - 180.0) / ROW_H).max(1.0) as usize;
        for (index, name) in self.names.iter().enumerate().skip(first).take(visible) {
            list = list.child(self.row(index, name));
        }
        let add_ready = !self.name.text().trim().is_empty() && !self.value.text().is_empty();
        let typing = |which: Focus| self.focused && self.focus == Some(which);
        let form = div()
            .row()
            .gap(Space::S8)
            .items_center()
            .child(
                div().col().w_px(NAME_W).child(self.name.render_input(
                    NAME_FIELD,
                    typing(Focus::Name),
                    false,
                )),
            )
            .child(
                div().col().flex(1.0).child(self.value.render_input(
                    VALUE_FIELD,
                    typing(Focus::Value),
                    false,
                )),
            )
            .child(
                components::button(ADD, "Add")
                    .size(components::ButtonSize::Medium)
                    .kind(ButtonKind::Primary)
                    .keystroke(Some("enter"))
                    .enabled(add_ready),
            );
        let note = match &self.error {
            Some(error) => label(error.clone())
                .size(LabelSize::Small)
                .color(colors.error),
            None => label("Names use letters, digits and _ only, as in {{secret.NAME}}.")
                .size(LabelSize::Small)
                .color(colors.text_placeholder),
        };
        let doc = div()
            .col()
            .w_px(doc_w)
            .px(Space::S24)
            .py(Space::S20)
            .gap(Space::S12)
            .child(intro)
            .child(list)
            .child(form)
            .child(note);
        let tree: Node = div()
            .col()
            .w_px(width)
            .h_px(height)
            .bg(colors.editor_background)
            .child(doc)
            .into();
        let painted = ui::render(&tree, body);
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn wants_keystrokes(&self) -> bool {
        true
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let Modifiers {
            shift, alt, cmd, ..
        } = keystroke.modifiers;
        let key = match keystroke.key.as_str() {
            "enter" => {
                self.add();
                return TerminalKeyOutcome::Handled;
            }
            "tab" => {
                self.focus = Some(if self.focus == Some(Focus::Name) {
                    Focus::Value
                } else {
                    Focus::Name
                });
                return TerminalKeyOutcome::Handled;
            }
            "v" if cmd => return TerminalKeyOutcome::Paste,
            "left" if cmd => EditKey::Home,
            "right" if cmd => EditKey::End,
            "left" if alt => EditKey::WordLeft,
            "right" if alt => EditKey::WordRight,
            "left" => EditKey::Left,
            "right" => EditKey::Right,
            "backspace" if cmd => EditKey::DeleteToLineStart,
            "backspace" if alt => EditKey::DeleteWordLeft,
            "backspace" => EditKey::Backspace,
            "delete" => EditKey::Delete,
            "a" if cmd => EditKey::SelectAll,
            _ => return TerminalKeyOutcome::Ignored,
        };
        if let Some(field) = self.field() {
            field.key(key, shift);
        }
        TerminalKeyOutcome::Handled
    }

    fn input_text(&mut self, text: &str) {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(field) = self.field() {
            field.insert(&typed);
        }
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        self.input_text(text.trim_end_matches(['\n', '\r']));
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        if let Some(id) = hit_at(&self.hits, x, y) {
            self.click(id);
        } else {
            self.armed = None;
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hit = hit_at(&self.hits, x, y);
        let changed = hit != self.hovered;
        self.hovered = hit;
        changed
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let max = (self.names.len() as f32 * ROW_H - ROW_H).max(0.0);
        let next = (self.scroll - delta_y / ui::ui_text_scale()).clamp(0.0, max);
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let copy = self.copy.take();
        ItemTick {
            changed: copy.is_some(),
            clipboard_store: copy,
            ..ItemTick::default()
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}
