use ui::{div, icon, theme, Div, Icon, IconKind, Node, Radius, Rgba};

pub const TREE_INDENT: f32 = 16.0;
const PAD_LEFT: f32 = 8.0;
const EXPANDER_W: f32 = 12.0;

/// One row of a tree (files, databases, changes, services): indented by its depth with a guide per level,
/// a chevron when it opens, then what the screen puts in it. Built with [`tree_row`].
pub struct TreeRow {
    id: u64,
    depth: usize,
    height: f32,
    expander: Option<bool>,
    expander_id: Option<u64>,
    leaf_gap: bool,
    selected: bool,
    focused: bool,
    hovered: bool,
    marked: Option<Rgba>,
    rounded: bool,
    children: Vec<Node>,
}

pub fn tree_row(id: u64, depth: usize) -> TreeRow {
    TreeRow {
        id,
        depth,
        height: 26.0,
        expander: None,
        expander_id: None,
        leaf_gap: true,
        selected: false,
        focused: false,
        hovered: false,
        marked: None,
        rounded: false,
        children: Vec::new(),
    }
}

impl TreeRow {
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// A chevron, down while open; without one a leaf keeps the chevron's room so names line up.
    pub fn expander(mut self, open: Option<bool>) -> Self {
        self.expander = open;
        self
    }

    /// Clicking the chevron does something of its own (opens without selecting).
    pub fn expander_id(mut self, id: u64) -> Self {
        self.expander_id = Some(id);
        self
    }

    pub fn flush_leaf(mut self) -> Self {
        self.leaf_gap = false;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    /// A bar at the left edge and a wash of this color: the row belongs to what is picked elsewhere.
    pub fn marked(mut self, color: Option<Rgba>) -> Self {
        self.marked = color;
        self
    }

    pub fn rounded(mut self) -> Self {
        self.rounded = true;
        self
    }

    pub fn child(mut self, child: impl Into<Node>) -> Self {
        self.children.push(child.into());
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let mut row = div()
            .row()
            .h_px(self.height)
            .pl(PAD_LEFT + self.depth as f32 * TREE_INDENT)
            .pr(8.0)
            .gap(6.0)
            .items_center()
            .on_click(self.id);
        if self.rounded {
            row = row.rounded(Radius::Medium);
        }
        row = indent_guides(row, self.depth);
        if self.focused {
            row = row
                .bg(colors.element_selected)
                .border(1.0, colors.panel_focused_border);
        } else if self.selected {
            row = row.bg(colors.element_selected);
        } else if self.hovered {
            row = row.bg(colors.ghost_element_hover);
        } else if let Some(color) = self.marked {
            row = row
                .bg(color.alpha(0.08))
                .pin_left_edge(0.0, 0.0, 2.0, div().bg(color));
        }
        match self.expander {
            Some(open) => {
                let mut slot = div()
                    .w_px(EXPANDER_W)
                    .h_px(self.height)
                    .items_center()
                    .justify_center()
                    .child(disclosure(open));
                if let Some(id) = self.expander_id {
                    slot = slot.on_click(id);
                }
                row = row.child(slot);
            }
            None if self.leaf_gap => row = row.child(div().w_px(EXPANDER_W)),
            None => {}
        }
        for child in self.children {
            row = row.child(child);
        }
        row
    }
}

/// The chevron of something that folds: down while open, right while folded.
pub fn disclosure(open: bool) -> Icon {
    icon(if open {
        IconKind::ChevronDown
    } else {
        IconKind::ChevronRight
    })
    .size(12.0)
    .color(theme().icon_muted)
}

/// A guide line for each level above `depth`, for a row in a tree that is not itself a [`TreeRow`] (a note
/// under a folder).
pub fn indent_guides(mut row: Div, depth: usize) -> Div {
    for level in 0..depth {
        row = row.pin_left_edge(
            PAD_LEFT + EXPANDER_W / 2.0 + level as f32 * TREE_INDENT,
            0.0,
            1.0,
            div().bg(theme().panel_indent_guide),
        );
    }
    row
}

impl From<TreeRow> for Node {
    fn from(row: TreeRow) -> Node {
        row.render().into()
    }
}
