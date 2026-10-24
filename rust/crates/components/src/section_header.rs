use ui::{div, label, theme, Div, Node};

/// The title over a group of rows or fields: small, uppercase and muted, with what the group counts or one
/// quiet action at the right.
pub struct SectionHeader {
    title: String,
    count: Option<usize>,
    trailing: Option<Node>,
}

pub fn section_header(title: impl Into<String>) -> SectionHeader {
    SectionHeader {
        title: title.into(),
        count: None,
        trailing: None,
    }
}

impl SectionHeader {
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    pub fn trailing(mut self, trailing: impl Into<Node>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let mut row = div()
            .row()
            .h_px(SECTION_HEADER_HEIGHT)
            .items_center()
            .child(
                div().row().flex(1.0).items_center().child(
                    label(self.title.to_uppercase())
                        .size(11.0)
                        .weight(600)
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            );
        if let Some(count) = self.count {
            row = row.child(
                label(count.to_string())
                    .size(11.0)
                    .color(colors.text_placeholder),
            );
        }
        if let Some(trailing) = self.trailing {
            row = row.child(trailing);
        }
        row
    }
}

impl From<SectionHeader> for Node {
    fn from(header: SectionHeader) -> Node {
        header.render().into()
    }
}

pub const SECTION_HEADER_HEIGHT: f32 = 14.0;
