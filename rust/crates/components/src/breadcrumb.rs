use ui::{div, icon, label, theme, Div, IconKind, LabelSize, Node, Radius, Space};

const TEXT: LabelSize = LabelSize::Medium;

enum Part {
    Text {
        text: String,
        id: Option<u64>,
        current: bool,
    },
    Node(Box<Node>),
}

/// Where the thing on screen sits: its parents left to right, each a step back, the last one itself, and
/// the branch it is on. Built with [`breadcrumb`].
pub struct Breadcrumb {
    icon: Option<IconKind>,
    parts: Vec<Part>,
    branch: Option<String>,
    hovered: Option<u64>,
    click: Option<u64>,
}

pub fn breadcrumb() -> Breadcrumb {
    Breadcrumb {
        icon: None,
        parts: Vec::new(),
        branch: None,
        hovered: None,
        click: None,
    }
}

impl Breadcrumb {
    pub fn icon(mut self, kind: IconKind) -> Self {
        self.icon = Some(kind);
        self
    }

    pub fn part(mut self, text: impl Into<String>) -> Self {
        self.parts.push(Part::Text {
            text: text.into(),
            id: None,
            current: false,
        });
        self
    }

    pub fn link(mut self, id: u64, text: impl Into<String>) -> Self {
        self.parts.push(Part::Text {
            text: text.into(),
            id: Some(id),
            current: false,
        });
        self
    }

    pub fn current(mut self, text: impl Into<String>) -> Self {
        self.parts.push(Part::Text {
            text: text.into(),
            id: None,
            current: true,
        });
        self
    }

    pub fn node(mut self, node: impl Into<Node>) -> Self {
        self.parts.push(Part::Node(Box::new(node.into())));
        self
    }

    pub fn branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = Some(branch.into());
        self
    }

    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered;
        self
    }

    /// The whole trail is one button (it opens an outline).
    pub fn on_click(mut self, id: u64) -> Self {
        self.click = Some(id);
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let mut row = div().row().gap(Space::S6).items_center();
        if let Some(id) = self.click {
            row = row
                .h_px(22.0)
                .px(Space::S4)
                .rounded(Radius::Small)
                .on_click(id)
                .hover_bg(colors.ghost_element_hover);
            if self.hovered == Some(id) {
                row = row.bg(colors.ghost_element_hover);
            }
        }
        if let Some(kind) = self.icon {
            row = row.child(icon(kind).size(12.0).color(colors.icon_muted));
        }
        for (index, part) in self.parts.into_iter().enumerate() {
            if index > 0 {
                row = row.child(label("/").size(TEXT).color(colors.text_placeholder));
            }
            row = row.child(match part {
                Part::Node(node) => *node,
                Part::Text { text, id, current } => {
                    let text = label(text).size(TEXT).truncate();
                    let text = if current {
                        text.medium().color(colors.text)
                    } else if id.is_some() && id == self.hovered {
                        text.underline(colors.text).color(colors.text)
                    } else {
                        text.color(colors.text_muted)
                    };
                    match id {
                        Some(id) => div().row().items_center().on_click(id).child(text).into(),
                        None => text.into(),
                    }
                }
            });
        }
        if let Some(branch) = self.branch {
            row = row
                .child(div().w_px(2.0))
                .child(
                    label("on")
                        .size(LabelSize::Small)
                        .color(colors.text_placeholder),
                )
                .child(icon(IconKind::Branch).size(11.0).color(colors.text_accent))
                .child(
                    label(branch)
                        .size(LabelSize::Small)
                        .mono()
                        .color(colors.text_accent),
                );
        }
        row
    }
}

impl From<Breadcrumb> for Node {
    fn from(crumb: Breadcrumb) -> Node {
        crumb.render().into()
    }
}
