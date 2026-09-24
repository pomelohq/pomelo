use std::collections::HashMap;

use pom_secrets::SecretStore;
use terminal::{Keystroke, Modifiers};
use ui::{div, icon, label, theme, IconKind, Node, Rect};
use workspace::text_field::{FieldFont, TextField};
use workspace::{EditKey, Item, ItemTick, TerminalKeyOutcome};

use crate::{hit_at, icon_button, EnvironmentContext};

pub const ITEM_ID: &str = "secrets";
const ROW_H: f32 = 34.0;
const NAME_FIELD: u64 = 1;
const VALUE_FIELD: u64 = 2;
const ADD: u64 = 3;
const ROW_BASE: u64 = 100;
const REVEAL: u64 = 0;
const COPY: u64 = 1;
const DELETE: u64 = 2;
const ROW_STRIDE: u64 = 4;

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
    name: TextField,
    value: TextField,
    focus: Option<Focus>,
    focused: bool,
    error: Option<String>,
    copy: Option<String>,
    scroll: f32,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
}

fn field() -> TextField {
    let mut field = TextField::default();
    field.set_font_size(12.0);
    field
}

impl SecretsItem {
    pub fn new(context: EnvironmentContext) -> SecretsItem {
        let mut item = SecretsItem {
            context,
            names: Vec::new(),
            revealed: HashMap::new(),
            armed: None,
            name: field(),
            value: field(),
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
        self.name.set_text("");
        self.value.set_text("");
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
            .h_px(ROW_H)
            .px(16.0)
            .gap(10.0)
            .items_center()
            .child(icon(IconKind::Key).size(12.0).color(colors.icon_muted))
            .child(
                label(format!("{{{{secret.{name}}}}}"))
                    .size(12.0)
                    .mono()
                    .color(colors.text_accent),
            )
            .child(div().row().flex(1.0));
        if let Some(value) = self.revealed.get(name) {
            row = row.child(
                div()
                    .row()
                    .h_px(22.0)
                    .px(8.0)
                    .items_center()
                    .rounded(5.0)
                    .bg(colors.editor_background)
                    .border(1.0, colors.border_variant)
                    .child(
                        label(if value.is_empty() {
                            "(empty)".to_string()
                        } else {
                            value.clone()
                        })
                        .size(11.0)
                        .mono()
                        .color(colors.text_muted)
                        .truncate(),
                    ),
            );
        }
        let revealed = self.revealed.contains_key(name);
        row = row
            .child(icon_button(
                base + REVEAL,
                IconKind::Eye,
                if revealed {
                    colors.text_accent
                } else {
                    colors.icon_muted
                },
                hot(base + REVEAL),
            ))
            .child(icon_button(
                base + COPY,
                IconKind::Copy,
                colors.icon_muted,
                hot(base + COPY),
            ));
        if self.armed.as_deref() == Some(name) {
            row = row.child(
                div()
                    .row()
                    .h_px(20.0)
                    .px(6.0)
                    .items_center()
                    .rounded(4.0)
                    .bg(colors.error)
                    .on_click(base + DELETE)
                    .child(label("Delete?").size(11.0).color(colors.editor_background)),
            );
        } else {
            row = row.child(icon_button(
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
            row = row.bg(colors.ghost_element_hover);
        }
        row.into()
    }

    fn input(
        &self,
        id: u64,
        which: Focus,
        field: &TextField,
        placeholder: &str,
        masked: bool,
    ) -> ui::Div {
        let colors = theme();
        let focused = self.focused && self.focus == Some(which);
        let content: Node = if masked && !field.text().is_empty() {
            let dots = "*".repeat(field.text().chars().count().min(24));
            let mut masked_field = TextField::default();
            masked_field.set_font_size(12.0);
            masked_field.set_text(&dots);
            masked_field.move_to_end();
            masked_field.render(placeholder, focused, colors.text, 24.0, FieldFont::Mono)
        } else {
            field.render(placeholder, focused, colors.text, 24.0, FieldFont::Mono)
        };
        div()
            .row()
            .h_px(28.0)
            .px(8.0)
            .items_center()
            .rounded(4.0)
            .border(
                1.0,
                if focused {
                    colors.border_focused
                } else {
                    colors.border
                },
            )
            .on_click(id)
            .child(content)
    }

    fn field(&mut self) -> Option<&mut TextField> {
        match self.focus? {
            Focus::Name => Some(&mut self.name),
            Focus::Value => Some(&mut self.value),
        }
    }

    /// The names listed now (tests).
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Types into the add form (tests).
    pub fn type_new(&mut self, name: &str, value: &str) {
        self.name.set_text(name);
        self.value.set_text(value);
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
        let intro = div().col().px(16.0).pt(12.0).pb(8.0).child(
            label("Encrypted app-local values for {{secret.NAME}} referenced in your config. Names are listed; values show only when you ask.")
                .size(11.0)
                .color(colors.text_muted)
                .wrap(width - 32.0),
        );
        let mut list = div().col().py(4.0);
        if self.names.is_empty() {
            list = list.child(
                div()
                    .row()
                    .px(16.0)
                    .py(12.0)
                    .child(label("No secrets yet.").size(12.0).color(colors.text_muted)),
            );
        }
        let first = (self.scroll / ROW_H) as usize;
        let visible = ((height - 140.0) / ROW_H).max(1.0) as usize;
        for (index, name) in self.names.iter().enumerate().skip(first).take(visible) {
            list = list.child(self.row(index, name));
        }
        let add_ready = !self.name.text().trim().is_empty() && !self.value.text().is_empty();
        let mut add_button = div()
            .row()
            .h_px(28.0)
            .px(12.0)
            .items_center()
            .rounded(4.0)
            .bg(if add_ready {
                colors.element_selected
            } else {
                colors.element_background
            })
            .child(label("Add").size(12.0).color(if add_ready {
                colors.text
            } else {
                colors.text_disabled
            }));
        if add_ready {
            add_button = add_button.on_click(ADD);
        }
        let mut form = div().col().gap(6.0).px(16.0).py(12.0).child(
            div()
                .row()
                .gap(8.0)
                .items_center()
                .child(
                    self.input(NAME_FIELD, Focus::Name, &self.name, "NAME", false)
                        .w_px(200.0),
                )
                .child(
                    self.input(VALUE_FIELD, Focus::Value, &self.value, "value", true)
                        .flex(1.0),
                )
                .child(add_button),
        );
        if let Some(error) = &self.error {
            form = form.child(label(error.clone()).size(12.0).color(colors.error));
        }
        let tree: Node = div()
            .col()
            .w_px(width)
            .h_px(height)
            .bg(colors.editor_background)
            .child(intro)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(div().col().flex(1.0).child(list))
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(form)
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
