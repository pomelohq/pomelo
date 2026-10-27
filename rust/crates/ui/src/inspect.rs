//! What each element of a painted tree is, where it landed and how it is styled, recorded only while a caller
//! asks for it (the gallery's inspector); painting records nothing otherwise.

use std::cell::RefCell;

use crate::{IconKind, Rect, Rgba, TextFont};

#[derive(Clone, Debug, PartialEq)]
pub enum Inspect {
    Box {
        /// Top, right, bottom, left, in px as laid out.
        padding: [f32; 4],
        gap: f32,
        row: bool,
        background: Option<Rgba>,
        border: f32,
        border_color: Rgba,
        radius: f32,
    },
    Text {
        text: String,
        size: f32,
        weight: u16,
        color: Rgba,
        font: TextFont,
    },
    Icon {
        kind: IconKind,
        size: f32,
        color: Rgba,
    },
}

#[derive(Clone)]
pub struct Inspected {
    pub rect: Rect,
    /// The box it was laid out in; `None` at the root and for floating layers.
    pub parent: Option<usize>,
    pub what: Inspect,
}

#[derive(Default)]
struct Recording {
    elements: Vec<Inspected>,
    parents: Vec<usize>,
}

thread_local! {
    static RECORDING: RefCell<Option<Recording>> = const { RefCell::new(None) };
}

/// Runs `paint` and returns what it painted along with every element it laid out, parents before children.
pub fn inspect<T>(paint: impl FnOnce() -> T) -> (T, Vec<Inspected>) {
    RECORDING.with(|recording| *recording.borrow_mut() = Some(Recording::default()));
    let painted = paint();
    let elements = RECORDING
        .with(|recording| recording.borrow_mut().take())
        .map(|recording| recording.elements)
        .unwrap_or_default();
    (painted, elements)
}

/// Notes an element under the box being laid out; its index while recording.
pub(crate) fn record(rect: Rect, what: impl FnOnce() -> Inspect) -> Option<usize> {
    RECORDING.with(|recording| {
        let mut recording = recording.borrow_mut();
        let recording = recording.as_mut()?;
        let parent = recording.parents.last().copied();
        recording.elements.push(Inspected {
            rect,
            parent,
            what: what(),
        });
        Some(recording.elements.len() - 1)
    })
}

/// Makes `index` the parent of what is recorded until [`leave`].
pub(crate) fn enter(index: Option<usize>) {
    let Some(index) = index else {
        return;
    };
    RECORDING.with(|recording| {
        if let Some(recording) = recording.borrow_mut().as_mut() {
            recording.parents.push(index);
        }
    });
}

pub(crate) fn leave(index: Option<usize>) {
    if index.is_none() {
        return;
    }
    RECORDING.with(|recording| {
        if let Some(recording) = recording.borrow_mut().as_mut() {
            recording.parents.pop();
        }
    });
}

/// The innermost element at (`x`, `y`): the last laid out there, since children follow their parents.
pub fn element_at(elements: &[Inspected], x: f32, y: f32) -> Option<usize> {
    elements.iter().rposition(|element| {
        let rect = element.rect;
        x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
    })
}

/// The elements laid out directly in `parent`, in order.
pub fn children(elements: &[Inspected], parent: usize) -> Vec<usize> {
    elements
        .iter()
        .enumerate()
        .filter(|(_, element)| element.parent == Some(parent))
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{div, icon, label, theme, Space};

    #[test]
    fn painting_records_each_element_under_its_box_only_when_asked() {
        let node: crate::Node = div()
            .col()
            .p(Space::S8)
            .gap(Space::S4)
            .bg(theme().text_muted)
            .child(label("Billing export").size_px(13.0))
            .child(icon(IconKind::Check).size(12.0))
            .into();
        let area = Rect::new(10.0, 20.0, 200.0, 80.0, Rgba::TRANSPARENT);
        let (_, elements) = inspect(|| crate::render(&node, area));
        assert_eq!(elements.len(), 3);
        let Inspect::Box {
            padding,
            gap,
            row,
            background,
            ..
        } = &elements[0].what
        else {
            panic!("the box comes first");
        };
        assert_eq!((*padding, *gap, *row), ([8.0; 4], 4.0, false));
        assert_eq!(
            background.and_then(crate::theme_file::token_name),
            Some("text.muted")
        );
        assert!(
            matches!(&elements[1].what, Inspect::Text { text, .. } if text == "Billing export")
        );
        assert!(matches!(elements[2].what, Inspect::Icon { size, .. } if size == 12.0));
        assert_eq!(elements[1].parent, Some(0));
        assert_eq!(children(&elements, 0), [1, 2]);
        let label_rect = elements[1].rect;
        assert_eq!(
            element_at(&elements, label_rect.x + 1.0, label_rect.y + 1.0),
            Some(1)
        );
        assert_eq!(
            element_at(&elements, 11.0, 21.0),
            Some(0),
            "the padding is the box"
        );

        crate::render(&node, area);
        let (_, again) = inspect(|| ());
        assert!(again.is_empty(), "nothing is recorded outside inspect");
    }
}
