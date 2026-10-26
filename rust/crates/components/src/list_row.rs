use ui::{div, theme, Node, Radius, Space};

/// One row of a picker or list: what it shows left to right, lit while hovered, tinted while selected.
pub struct ListRow {
    id: u64,
    children: Vec<Node>,
    selected: bool,
    hovered: bool,
}

pub fn list_row(id: u64) -> ListRow {
    ListRow {
        id,
        children: Vec::new(),
        selected: false,
        hovered: false,
    }
}

impl ListRow {
    pub fn child(mut self, child: impl Into<Node>) -> Self {
        self.children.push(child.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Lit as hovered when `hovered` is this row, for lists that track the pointer themselves.
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered == Some(self.id);
        self
    }
}

impl From<ListRow> for Node {
    fn from(row: ListRow) -> Node {
        let colors = theme();
        let mut item = div()
            .row()
            .flex(1.0)
            .px(Space::S6)
            .py(Space::S4)
            .gap(Space::S6)
            .items_center()
            .rounded(Radius::Small)
            .on_click(row.id);
        if row.selected {
            item = item.bg(colors.overlay_selected);
        } else {
            item = item.hover_bg(colors.ghost_element_hover);
            if row.hovered {
                item = item.bg(colors.ghost_element_hover);
            }
        }
        div()
            .row()
            .px(Space::S4)
            .child(item.children(row.children))
            .into()
    }
}
