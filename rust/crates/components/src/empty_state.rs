use ui::{div, icon, label, theme, Div, IconKind, Node};

/// What a pane or list says when it has nothing to show: why, and what fills it. Built with
/// [`empty_state`].
pub struct EmptyState {
    title: String,
    icon: Option<IconKind>,
    body: Option<(String, f32)>,
    action: Option<Node>,
    compact: bool,
}

pub fn empty_state(title: impl Into<String>) -> EmptyState {
    EmptyState {
        title: title.into(),
        icon: None,
        body: None,
        action: None,
        compact: false,
    }
}

impl EmptyState {
    pub fn icon(mut self, kind: IconKind) -> Self {
        self.icon = Some(kind);
        self
    }

    pub fn body(mut self, body: impl Into<String>, width: f32) -> Self {
        self.body = Some((body.into(), width));
        self
    }

    pub fn action(mut self, action: impl Into<Node>) -> Self {
        self.action = Some(action.into());
        self
    }

    /// One muted line where a list's rows would be ("No matches").
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        if self.compact {
            return div().row().h_px(28.0).px(10.0).items_center().child(
                label(self.title)
                    .size(12.0)
                    .color(colors.text_muted)
                    .truncate(),
            );
        }
        let mut column = div().col().items_center().gap(8.0);
        if let Some(kind) = self.icon {
            column = column.child(icon(kind).size(20.0).color(colors.text_placeholder));
        }
        column = column.child(
            label(self.title)
                .size(13.0)
                .medium()
                .color(colors.text_muted),
        );
        if let Some((body, width)) = self.body {
            column = column.child(crate::paragraph(body, width).centered());
        }
        if let Some(action) = self.action {
            column = column.child(div().pt(4.0).child(action));
        }
        div()
            .col()
            .flex(1.0)
            .items_center()
            .justify_center()
            .py(40.0)
            .child(column)
    }
}

impl From<EmptyState> for Node {
    fn from(state: EmptyState) -> Node {
        state.render().into()
    }
}
