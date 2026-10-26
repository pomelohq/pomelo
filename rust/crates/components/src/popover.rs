use ui::{div, label, theme, Div, LabelSize, Node, Space};

use crate::card;

/// The one popover: a raised card under the thing that opened it. A list popover holds rows edge to edge
/// with a 4px inset; a `content` popover pads its text and controls like a small dialog.
pub struct Popover {
    width: f32,
    title: Option<String>,
    aside: Option<String>,
    content: bool,
}

pub fn popover(width: f32) -> Popover {
    Popover {
        width,
        title: None,
        aside: None,
        content: false,
    }
}

impl Popover {
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A quiet note at the header's right, such as a count.
    pub fn aside(mut self, aside: impl Into<String>) -> Self {
        self.aside = Some(aside.into());
        self
    }

    pub fn content(mut self) -> Self {
        self.content = true;
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let mut frame = card()
            .padding(if self.content { 12.0 } else { 4.0 })
            .raised()
            .render()
            .w_px(self.width);
        if self.content {
            frame = frame.gap(Space::S8);
        }
        if self.title.is_none() && self.aside.is_none() {
            return frame;
        }
        let mut header = div().row().items_center().gap(Space::S8);
        header = if self.content {
            header.h_px(20.0)
        } else {
            // Lines the title up with the rows' text below it.
            header.h_px(30.0).px(Space::S8)
        };
        if let Some(title) = self.title {
            header = header.child(
                div().row().flex(1.0).child(
                    label(title)
                        .size(LabelSize::Medium)
                        .weight(500)
                        .color(colors.text)
                        .truncate(),
                ),
            );
        } else {
            header = header.child(div().flex(1.0));
        }
        if let Some(aside) = self.aside {
            header = header.child(
                label(aside)
                    .size(LabelSize::Small)
                    .color(colors.text_placeholder),
            );
        }
        frame.child(header)
    }
}

impl From<Popover> for Node {
    fn from(popover: Popover) -> Node {
        popover.render().into()
    }
}
