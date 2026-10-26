use ui::{div, icon, label, theme, Div, IconKind, LabelSize, Node, Radius, Rgba};

pub const TAB_HEIGHT: f32 = 26.0;

/// One open thing in a pane's tab bar: its icon and title, and a close button that a pin or an unsaved
/// dot stands in for. Built with [`tab`].
pub struct Tab {
    id: u64,
    title: String,
    glyph: Option<Node>,
    detail: Option<String>,
    content: Option<Node>,
    close: Option<u64>,
    active: bool,
    hovered: bool,
    preview: bool,
    pinned: bool,
    unsaved: Option<Rgba>,
    floating: bool,
}

pub fn tab(id: u64, title: impl Into<String>) -> Tab {
    Tab {
        id,
        title: title.into(),
        glyph: None,
        detail: None,
        content: None,
        close: None,
        active: false,
        hovered: false,
        preview: false,
        pinned: false,
        unsaved: None,
        floating: false,
    }
}

impl Tab {
    pub fn glyph(mut self, glyph: impl Into<Node>) -> Self {
        self.glyph = Some(glyph.into());
        self
    }

    pub fn detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }

    /// What the tab shows instead of its icon and title (a terminal's live state).
    pub fn content(mut self, content: Option<Node>) -> Self {
        self.content = content;
        self
    }

    pub fn close(mut self, id: u64) -> Self {
        self.close = Some(id);
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    /// Opened for a look only: the title in italics until it is edited or kept.
    pub fn preview(mut self, preview: bool) -> Self {
        self.preview = preview;
        self
    }

    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    /// Unsaved changes, as a dot of this color where the close button is until the tab is hovered.
    pub fn unsaved(mut self, color: Option<Rgba>) -> Self {
        self.unsaved = color;
        self
    }

    pub fn floating(mut self) -> Self {
        self.floating = true;
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let mut row = div()
            .row()
            .h_px(TAB_HEIGHT)
            .px(10.0)
            .gap(6.0)
            .rounded(Radius::Medium)
            .items_center();
        match self.content {
            Some(content) => row = row.child(content),
            None => {
                if let Some(glyph) = self.glyph {
                    row = row.child(glyph);
                }
                let mut title = label(self.title).size(LabelSize::Medium);
                title = if self.active || self.floating {
                    title.medium().color(colors.text)
                } else {
                    title.color(colors.text_muted)
                };
                if self.preview {
                    title = title.italic();
                }
                row = row.child(title);
                if let Some(detail) = self.detail {
                    row = row.child(
                        label(detail)
                            .size(LabelSize::Medium)
                            .color(colors.text_placeholder),
                    );
                }
            }
        }
        if let Some(close) = self.close {
            row = row.child(end_slot(
                close,
                self.pinned,
                self.unsaved.filter(|_| !self.hovered && !self.pinned),
                self.hovered,
            ));
        }
        if self.floating {
            return row
                .bg(colors.elevated_surface_background)
                .border(1.0, colors.border);
        }
        row = row.on_click(self.id);
        if self.active {
            row = row.bg(colors.editor_background);
        } else if self.hovered {
            row = row.bg(colors.element_hover);
        }
        // The active tab sits on a one-pixel shadow edge below it, the way a raised card casts its shadow.
        let cell = div().col().h_px(TAB_HEIGHT + 2.0).pt(1.0);
        if self.active {
            cell.child(
                div()
                    .col()
                    .h_px(TAB_HEIGHT + 1.0)
                    .pb(1.0)
                    .rounded(Radius::Large)
                    .bg(theme().edge_shadow())
                    .child(row),
            )
        } else {
            cell.child(row)
        }
    }
}

fn end_slot(id: u64, pinned: bool, unsaved: Option<Rgba>, hovered: bool) -> Node {
    let colors = theme();
    let mut slot = div()
        .w_px(if unsaved.is_some() { 7.0 } else { 12.0 })
        .h_px(16.0)
        .rounded(Radius::Small)
        .items_center()
        .justify_center()
        .on_click(id);
    let ink = if hovered {
        colors.icon_muted
    } else {
        colors.text_placeholder
    };
    slot = match (pinned, unsaved) {
        (_, Some(color)) => slot.child(crate::dot(color, 7.0)),
        (true, None) => slot.child(icon(IconKind::Pin).size(12.0).color(ink)),
        (false, None) => slot.child(icon(IconKind::Close).size(12.0).color(ink)),
    };
    slot.into()
}

impl From<Tab> for Node {
    fn from(tab: Tab) -> Node {
        tab.render().into()
    }
}
