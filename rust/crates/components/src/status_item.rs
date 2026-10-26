use ui::{div, icon, label, theme, Div, IconKind, LabelSize, Node, Radius, Rgba};

pub const STATUS_ITEM_HEIGHT: f32 = 20.0;

/// One thing on the window's status bar (a dock toggle, the cursor position, the language), lit on hover
/// and clicked to act on it. Built with [`status_item`].
pub struct StatusItem {
    id: Option<u64>,
    parts: Vec<Node>,
    width: Option<f32>,
    height: f32,
    hovered: bool,
}

pub fn status_item(id: Option<u64>) -> StatusItem {
    StatusItem {
        id,
        parts: Vec::new(),
        width: None,
        height: STATUS_ITEM_HEIGHT,
        hovered: false,
    }
}

impl StatusItem {
    pub fn icon(mut self, kind: IconKind, color: Rgba) -> Self {
        self.parts.push(icon(kind).size(14.0).color(color).into());
        self
    }

    pub fn badged_icon(mut self, kind: IconKind, color: Rgba, badge: Option<Rgba>) -> Self {
        let mut glyph = div()
            .row()
            .gap(1.0)
            .child(icon(kind).size(14.0).color(color));
        if let Some(badge) = badge {
            glyph = glyph.child(
                div()
                    .col()
                    .h_px(14.0)
                    .justify_end()
                    .child(crate::dot(badge, 6.0)),
            );
        }
        self.parts.push(glyph.into());
        self
    }

    pub fn text(mut self, text: impl Into<String>, color: Rgba) -> Self {
        self.parts.push(
            label(text.into())
                .size(LabelSize::Small)
                .color(color)
                .into(),
        );
        self
    }

    pub fn mono(mut self, text: impl Into<String>, color: Rgba) -> Self {
        self.parts.push(
            label(text.into())
                .size(LabelSize::Small)
                .mono()
                .color(color)
                .into(),
        );
        self
    }

    pub fn child(mut self, child: impl Into<Node>) -> Self {
        self.parts.push(child.into());
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    pub fn render(self) -> Div {
        let mut item = div()
            .row()
            .h_px(self.height)
            .px(if self.height > STATUS_ITEM_HEIGHT {
                6.0
            } else {
                4.0
            })
            .gap(if self.height > STATUS_ITEM_HEIGHT {
                6.0
            } else {
                4.0
            })
            .rounded(if self.height > STATUS_ITEM_HEIGHT {
                Radius::Medium
            } else {
                Radius::Small
            })
            .items_center();
        if let Some(width) = self.width {
            item = item.w_px(width).justify_center();
        }
        if let Some(id) = self.id {
            item = item.on_click(id);
            if self.hovered {
                item = item.bg(theme().ghost_element_hover);
            }
        }
        for part in self.parts {
            item = item.child(part);
        }
        item
    }
}

impl From<StatusItem> for Node {
    fn from(item: StatusItem) -> Node {
        item.render().into()
    }
}
