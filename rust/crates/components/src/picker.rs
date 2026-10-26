use ui::{div, Div, Node, Space};

use crate::{card, divider, empty_state};

pub const PICKER_HEAD_HEIGHT: f32 = 36.0;

/// The one picker: a query on top, the matches under it (or why there are none), and the keys at the
/// foot. The caller owns the query field and the rows, so selection and scrolling stay with it.
pub struct Picker {
    width: f32,
    query: Node,
    rows: Vec<Node>,
    empty: String,
    footer: Option<Node>,
}

pub fn picker(width: f32, query: impl Into<Node>) -> Picker {
    Picker {
        width,
        query: query.into(),
        rows: Vec::new(),
        empty: "No matches".into(),
        footer: None,
    }
}

/// The query row on its own, for a picker laid out around a preview.
pub fn picker_head(query: impl Into<Node>) -> Div {
    div()
        .row()
        .items_center()
        .h_px(PICKER_HEAD_HEIGHT)
        .px(Space::S10)
        .child(query.into())
}

/// The empty list's note, sized like the rows it stands in for.
pub fn picker_empty(text: impl Into<String>) -> Div {
    div()
        .col()
        .py(Space::S8)
        .child(empty_state(text.into()).compact())
}

impl Picker {
    pub fn child(mut self, row: impl Into<Node>) -> Self {
        self.rows.push(row.into());
        self
    }

    pub fn children(mut self, rows: impl IntoIterator<Item = impl Into<Node>>) -> Self {
        self.rows.extend(rows.into_iter().map(Into::into));
        self
    }

    pub fn empty(mut self, text: impl Into<String>) -> Self {
        self.empty = text.into();
        self
    }

    pub fn footer(mut self, footer: impl Into<Node>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    pub fn render(self) -> Div {
        let body = if self.rows.is_empty() {
            picker_empty(self.empty)
        } else {
            div().col().py(Space::S4).children(self.rows)
        };
        let mut frame = card()
            .padding(0.0)
            .raised()
            .render()
            .w_px(self.width)
            .child(picker_head(self.query))
            .child(divider())
            .child(body);
        if let Some(footer) = self.footer {
            frame = frame.child(divider()).child(footer);
        }
        frame
    }
}

impl From<Picker> for Node {
    fn from(picker: Picker) -> Node {
        picker.render().into()
    }
}
