//! A one-line text input on an editor buffer, for fields like the search query or go-to-line.

use std::ops::Range;

use editor::buffer::{Deletion, LineRows, Motion};
use editor::EditorBuffer;
use ui::{div, label, theme, Node};
use workspace::EditKey;

pub const INPUT_FONT: f32 = 14.0;

/// A one-line text field backed by an editor buffer, so it has the editor's caret and selection motions.
#[derive(Default)]
pub struct TextField {
    buffer: EditorBuffer,
}

impl TextField {
    pub fn text(&self) -> String {
        self.buffer.text()
    }

    pub fn set_text(&mut self, text: &str) {
        self.buffer = EditorBuffer::from_text(text);
        self.buffer.select_all();
    }

    pub fn select_all(&mut self) {
        self.buffer.select_all();
    }

    pub fn move_to_end(&mut self) {
        let end = self.buffer.rope.len_chars();
        self.buffer.place_cursor(end);
    }

    pub fn selected_text(&self) -> Option<String> {
        self.buffer.selected_text()
    }

    pub fn insert(&mut self, text: &str) {
        self.buffer.insert_text(text);
    }

    /// Apply an editing key; returns whether the text changed.
    pub fn key(&mut self, key: EditKey, shift: bool) -> bool {
        let before = self.buffer.version();
        let motion = match key {
            EditKey::Left => Some(Motion::Left),
            EditKey::Right => Some(Motion::Right),
            EditKey::WordLeft => Some(Motion::WordLeft),
            EditKey::WordRight => Some(Motion::WordRight),
            EditKey::SubwordLeft => Some(Motion::SubwordLeft),
            EditKey::SubwordRight => Some(Motion::SubwordRight),
            EditKey::Home | EditKey::LineStart | EditKey::DocumentStart => Some(Motion::LineStart),
            EditKey::End | EditKey::LineEnd | EditKey::DocumentEnd => Some(Motion::LineEnd),
            _ => None,
        };
        if let Some(motion) = motion {
            self.buffer.apply_motion(motion, shift);
            return false;
        }
        let deletion = match key {
            EditKey::Backspace => Some(Deletion::Backward),
            EditKey::Delete => Some(Deletion::Forward),
            EditKey::DeleteWordLeft => Some(Deletion::PreviousWordStart),
            EditKey::DeleteWordRight => Some(Deletion::NextWordEnd),
            EditKey::DeleteSubwordLeft => Some(Deletion::PreviousSubwordStart),
            EditKey::DeleteSubwordRight => Some(Deletion::NextSubwordEnd),
            EditKey::DeleteToLineStart => Some(Deletion::ToBeginningOfLine),
            EditKey::DeleteToLineEnd => Some(Deletion::ToEndOfLine),
            _ => None,
        };
        if let Some(deletion) = deletion {
            let grown = self
                .buffer
                .deletion_selections(deletion, &LineRows(&self.buffer));
            self.buffer.delete_selections(grown);
        } else {
            match key {
                EditKey::SelectAll => self.buffer.select_all(),
                EditKey::Undo => self.buffer.undo(),
                EditKey::Redo => self.buffer.redo(),
                _ => {}
            }
        }
        self.buffer.version() != before
    }

    /// The field's text laid out on one row: the selection tinted and a 2px caret at the head while focused.
    pub fn render(&self, placeholder: &str, focused: bool, color: ui::Rgba, height: f32) -> Node {
        let text = self.buffer.text();
        let mut row = div().row().items_center().flex(1.0).h_px(height);
        let caret = |visible: bool| {
            div().w_px(2.0).h_px(INPUT_FONT * 1.3).bg(if visible {
                theme().player_cursor
            } else {
                ui::Rgba::TRANSPARENT
            })
        };
        let show_caret = focused && ui::caret_phase();
        if text.is_empty() {
            return row
                .child(caret(show_caret))
                .child(
                    label(placeholder.to_string())
                        .size(INPUT_FONT)
                        .mono()
                        .color(theme().text_placeholder),
                )
                .into();
        }
        let selection = self.buffer.newest();
        let chars: Vec<char> = text.chars().collect();
        let piece = |range: Range<usize>| -> String { chars[range].iter().collect() };
        let head = selection.head();
        let (start, end) = (selection.start, selection.end);
        let push = |row: ui::Div, range: Range<usize>, selected: bool| -> ui::Div {
            if range.is_empty() {
                return row;
            }
            let text = label(piece(range)).size(INPUT_FONT).mono().color(color);
            if selected && focused {
                let st = crate::syntax_theme();
                let [r, g, b] = st.selection.0;
                let tint = ui::Rgba::new(
                    r as f32 / 255.0,
                    g as f32 / 255.0,
                    b as f32 / 255.0,
                    st.selection_alpha,
                );
                row.child(div().bg(tint).child(text))
            } else {
                row.child(text)
            }
        };
        row = push(row, 0..start, false);
        if head == start {
            row = row.child(caret(show_caret));
        }
        row = push(row, start..end, true);
        if head == end && end != start {
            row = row.child(caret(show_caret));
        }
        row = push(row, end..chars.len(), false);
        row.into()
    }
}
