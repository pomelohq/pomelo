use ui::{div, label, theme, Label, Node};

/// The head of a page in a tab: what it is (a ticket, a pull request, a service) and its actions. The full
/// form puts the lead (key, state) and actions on a line over a large wrapped title and a meta line;
/// `compact` puts the title, the lead and the actions on one 44px line.
pub struct PageHeader {
    title: Node,
    lead: Vec<Node>,
    actions: Vec<Node>,
    meta: Option<Node>,
    compact: bool,
}

pub fn page_header(title: impl Into<Node>) -> PageHeader {
    PageHeader {
        title: title.into(),
        lead: Vec::new(),
        actions: Vec::new(),
        meta: None,
        compact: false,
    }
}

/// The large title of the full form, wrapped to `width`.
pub fn page_title(text: impl Into<String>, width: f32) -> Label {
    label(text.into())
        .size_px(20.0)
        .weight(600)
        .color(theme().text)
        .wrap(width)
}

impl PageHeader {
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn lead(mut self, node: impl Into<Node>) -> Self {
        self.lead.push(node.into());
        self
    }

    pub fn action(mut self, node: impl Into<Node>) -> Self {
        self.actions.push(node.into());
        self
    }

    pub fn meta(mut self, node: impl Into<Node>) -> Self {
        self.meta = Some(node.into());
        self
    }
}

impl From<PageHeader> for Node {
    fn from(header: PageHeader) -> Node {
        let mut line = div().row().items_center().gap(8.0);
        let mut title = Some(header.title);
        if header.compact {
            line = line.h_px(44.0).children(title.take());
        }
        for node in header.lead {
            line = line.child(node);
        }
        line = line.child(div().row().flex(1.0));
        for node in header.actions {
            line = line.child(node);
        }
        if header.compact {
            return line.into();
        }
        let mut column = div().col().gap(10.0).child(line.pb(2.0)).children(title);
        if let Some(meta) = header.meta {
            column = column.child(meta);
        }
        column.into()
    }
}
