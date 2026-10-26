use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Rgba};

/// A small status line: a leading icon in its color, then truncated muted text.
pub struct StatusLine {
    kind: IconKind,
    color: Rgba,
    text: String,
    tinted: bool,
}

pub fn status_line(kind: IconKind, color: Rgba, text: impl Into<String>) -> StatusLine {
    StatusLine {
        kind,
        color,
        text: text.into(),
        tinted: false,
    }
}

impl StatusLine {
    /// The text in the icon's color too, for a line that is the news itself (an error, a step running).
    pub fn tinted(mut self) -> Self {
        self.tinted = true;
        self
    }
}

impl From<StatusLine> for Node {
    fn from(line: StatusLine) -> Node {
        let ink = if line.tinted {
            line.color
        } else {
            theme().text_muted
        };
        div()
            .row()
            .flex(1.0)
            .items_center()
            .gap(4.0)
            .child(icon(line.kind).size(12.0).color(line.color))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(line.text)
                        .size(LabelSize::Small)
                        .color(ink)
                        .truncate(),
                ),
            )
            .into()
    }
}
