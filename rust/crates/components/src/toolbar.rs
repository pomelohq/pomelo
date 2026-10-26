use ui::{div, label, theme, Div, LabelSize, Node, Space};

pub const TOOLBAR_HEIGHT: f32 = 32.0;

/// The strip across the top of a tab or panel with what acts on its content: where it is, filters, and
/// buttons. Built with [`toolbar`].
pub struct Toolbar {
    height: f32,
    children: Vec<Node>,
    edge: bool,
}

pub fn toolbar() -> Toolbar {
    Toolbar {
        height: TOOLBAR_HEIGHT,
        children: Vec::new(),
        edge: false,
    }
}

impl Toolbar {
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn child(mut self, child: impl Into<Node>) -> Self {
        self.children.push(child.into());
        self
    }

    /// The part that takes the room left over, cut short when the strip is narrow (a breadcrumb, a status).
    pub fn lead(mut self, lead: impl Into<Node>) -> Self {
        self.children.push(
            div()
                .row()
                .flex(1.0)
                .items_center()
                .child(lead.into())
                .into(),
        );
        self
    }

    /// What the last action did, taking the room left over: muted, or in the error color when it failed.
    pub fn status(self, text: impl Into<String>, error: bool) -> Self {
        let colors = theme();
        self.lead(
            label(text.into())
                .size(LabelSize::Small)
                .color(if error {
                    colors.error
                } else {
                    colors.text_muted
                })
                .truncate(),
        )
    }

    /// Quiet text that counts what is shown: 24 items, 3 changes.
    pub fn note(self, text: impl Into<String>) -> Self {
        self.child(
            label(text.into())
                .size(LabelSize::Small)
                .color(theme().text_muted),
        )
    }

    pub fn spacer(mut self) -> Self {
        self.children.push(div().flex(1.0).into());
        self
    }

    pub fn edge(mut self) -> Self {
        self.edge = true;
        self
    }

    pub fn render(self) -> Div {
        let mut bar = div()
            .row()
            .h_px(self.height)
            .pl(Space::S12)
            .pr(Space::S8)
            .gap(Space::S8)
            .items_center()
            .bg(theme().toolbar_background);
        for child in self.children {
            bar = bar.child(child);
        }
        if self.edge {
            div().col().child(bar).child(crate::edge_divider())
        } else {
            bar
        }
    }
}

impl From<Toolbar> for Node {
    fn from(toolbar: Toolbar) -> Node {
        toolbar.render().into()
    }
}
