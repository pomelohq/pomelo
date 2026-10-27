use ui::{div, theme, Div, IconKind, LabelSize, Node, Radius, Rgba, Space};

/// One row of a menu: click `id`, a `label`, a `checked` mark, and a `sep`arator line above it.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub id: u64,
    pub label: std::borrow::Cow<'static, str>,
    pub checked: bool,
    pub sep: bool,
    pub disabled: bool,
    /// Loses data: drawn in the error color.
    pub danger: bool,
    /// Shown before the label when the item is not checked.
    pub icon: Option<IconKind>,
    /// Muted text at the right: its key, or what a submenu is set to.
    pub hint: Option<std::borrow::Cow<'static, str>>,
    /// The icon's color, over the default for its kind.
    pub color: Option<Rgba>,
    /// A group's title: small, muted and not clickable.
    pub header: bool,
}

impl MenuItem {
    pub fn new(id: u64, label: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        MenuItem {
            id,
            label: label.into(),
            checked: false,
            sep: false,
            disabled: false,
            danger: false,
            icon: None,
            hint: None,
            color: None,
            header: false,
        }
    }
}

pub const MENU_ROW: f32 = 26.0;
pub const MENU_PAD: Space = Space::S4;
pub const MENU_SEARCH_ROW: f32 = 32.0;
pub const MENU_SEARCH_INSET: Space = Space::S8;
const MENU_SEPARATOR: f32 = 5.0;
const MENU_MIN_WIDTH: f32 = 200.0;

/// A list of choices on a raised card: context menus and the list a select opens.
pub struct Menu<'a> {
    items: &'a [MenuItem],
    hovered: Option<u64>,
    opens_submenu: fn(u64) -> bool,
    search: Option<&'a str>,
    search_at_bottom: bool,
    min_width: f32,
    plain_hints: bool,
}

pub fn menu(items: &[MenuItem]) -> Menu<'_> {
    Menu {
        items,
        hovered: None,
        opens_submenu: |_| false,
        search: None,
        search_at_bottom: false,
        min_width: MENU_MIN_WIDTH,
        plain_hints: false,
    }
}

impl<'a> Menu<'a> {
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered;
        self
    }

    /// Rows whose id this returns true for show a chevron, with their hint as the value they are set to.
    pub fn submenus(mut self, opens_submenu: fn(u64) -> bool) -> Self {
        self.opens_submenu = opens_submenu;
        self
    }

    pub fn search(mut self, query: &'a str) -> Self {
        self.search = Some(query);
        self
    }

    /// Keeps the search line next to the anchor when the menu opens above it.
    pub fn search_at_bottom(mut self, at_bottom: bool) -> Self {
        self.search_at_bottom = at_bottom;
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    /// Hints are muted text (a count, a date) rather than keys.
    pub fn plain_hints(mut self) -> Self {
        self.plain_hints = true;
        self
    }

    pub fn render(self) -> (Node, f32, f32) {
        let colors = theme();
        let weight = ui::ui_font_weight();
        let marks = self
            .items
            .iter()
            .any(|item| item.checked || item.icon.is_some());
        let mark_width = if marks { 12.0 + 6.0 } else { 0.0 };
        let mut content_width = 0.0_f32;
        for item in self.items {
            let label_width = ui::measure_text_width(&item.label, 13.0, false, weight);
            let right = match (&item.hint, (self.opens_submenu)(item.id)) {
                (Some(hint), true) => {
                    14.0 + ui::measure_text_width(hint, 12.0, false, weight) + 12.0
                }
                (Some(hint), false) if self.plain_hints => {
                    ui::measure_text_width(hint, 12.0, false, weight) + 24.0
                }
                (None, true) => 14.0,
                (Some(key), false) if !key.is_empty() => {
                    ui::measure(&ui::render_keystroke(key, LabelSize::Small)).0 + 24.0
                }
                _ => 0.0,
            };
            content_width = content_width.max(label_width + right);
        }
        let width = (16.0 + mark_width + content_width + 2.0 * MENU_PAD.px()).max(self.min_width);
        // A leading separator is not drawn, so it takes no height.
        let separators = self
            .items
            .iter()
            .enumerate()
            .filter(|(index, item)| item.sep && *index > 0)
            .count() as f32;
        let search_height = if self.search.is_some() {
            MENU_SEARCH_ROW + MENU_SEPARATOR
        } else {
            0.0
        };
        let height = MENU_PAD.px() * 2.0
            + search_height
            + MENU_ROW * self.items.len() as f32
            + MENU_SEPARATOR * separators;
        let separator = || {
            div().h_px(MENU_SEPARATOR).py(Space::S2).child(
                div()
                    .h_px(1.0)
                    .w_px(width - 2.0 * MENU_PAD.px())
                    .bg(colors.border),
            )
        };
        let mut card = div()
            .col()
            .p(MENU_PAD)
            .rounded(Radius::Large)
            .bg(colors.elevated_surface_background)
            .border(1.0, colors.border);
        let mut search = self.search.map(|query| {
            let line = if query.is_empty() {
                ui::label("Search...")
                    .size(LabelSize::Small)
                    .color(colors.text_muted)
            } else {
                ui::label(query.to_string())
                    .size(LabelSize::Small)
                    .color(colors.text)
            };
            div()
                .row()
                .h_px(MENU_SEARCH_ROW)
                .px(MENU_SEARCH_INSET)
                .items_center()
                .child(line)
        });
        if !self.search_at_bottom {
            if let Some(line) = search.take() {
                card = card.child(line).child(separator());
            }
        }
        for (index, item) in self.items.iter().enumerate() {
            if item.sep && index > 0 {
                card = card.child(separator());
            }
            if item.header {
                card = card.child(menu_header(item.label.to_string()));
                continue;
            }
            card = card.child(self.row(item, marks));
        }
        if let Some(line) = search {
            card = card.child(separator()).child(line);
        }
        (card.w_px(width).into(), width, height)
    }

    fn row(&self, item: &MenuItem, marks: bool) -> Div {
        let colors = theme();
        let mut row = div()
            .row()
            .h_px(MENU_ROW)
            .px(Space::S8)
            .gap(Space::S6)
            .items_center()
            .rounded(Radius::Medium)
            .on_click(item.id);
        if self.hovered == Some(item.id) && !item.disabled {
            row = row.bg(theme().tint());
        }
        let label_color = if item.disabled {
            colors.text_disabled
        } else if item.danger {
            colors.error
        } else {
            colors.text
        };
        if marks {
            row = row.child(match (item.checked, item.icon) {
                (true, _) => ui::check_icon().size(12.0),
                (false, Some(kind)) => {
                    ui::icon(kind)
                        .size(12.0)
                        .color(if let Some(color) = item.color {
                            color
                        } else if item.disabled {
                            colors.text_disabled
                        } else if item.danger {
                            colors.error
                        } else if kind == IconKind::Sparkle {
                            colors.icon_accent
                        } else {
                            colors.icon_muted
                        })
                }
                (false, None) => ui::icon(IconKind::Check)
                    .size(12.0)
                    .color(Rgba::TRANSPARENT),
            });
        }
        row = row.child(
            ui::label(item.label.to_string())
                .size(LabelSize::Medium)
                .color(label_color),
        );
        if (self.opens_submenu)(item.id) {
            row = row.child(div().flex(1.0));
            if let Some(hint) = &item.hint {
                row = row.child(
                    ui::label(hint.to_string())
                        .size(LabelSize::Small)
                        .color(colors.text_muted),
                );
            }
            row.child(
                ui::icon(IconKind::ChevronRight)
                    .size(12.0)
                    .color(colors.icon_muted),
            )
        } else {
            match item.hint.as_deref() {
                Some(hint) if self.plain_hints => row.child(div().flex(1.0)).child(
                    ui::label(hint.to_string())
                        .size(LabelSize::Small)
                        .color(colors.text_placeholder),
                ),
                Some(key) if !key.is_empty() => row
                    .child(div().flex(1.0))
                    .child(ui::render_keystroke(key, LabelSize::Small)),
                _ => row,
            }
        }
    }
}

/// A group's title in a menu: small, muted and not clickable.
pub fn menu_header(text: impl Into<String>) -> Div {
    div()
        .row()
        .h_px(MENU_ROW)
        .px(Space::S8)
        .items_center()
        .child(
            ui::label(text.into())
                .size(LabelSize::Small)
                .color(theme().text_muted),
        )
}

/// One menu row on its own, for a card that mixes other content with a few commands (a popover).
pub fn menu_row(item: &MenuItem, hovered: Option<u64>) -> Node {
    let items = std::slice::from_ref(item);
    let marks = item.checked || item.icon.is_some();
    menu(items).hovered(hovered).row(item, marks).into()
}

impl From<Menu<'_>> for Node {
    fn from(menu: Menu<'_>) -> Node {
        menu.render().0
    }
}

/// Where a `size` menu opening from `anchor` goes inside `viewport`: under it and left-aligned when it
/// fits, right-aligned to it near the right edge, above it near the bottom; never past `margin`.
pub fn place_menu(
    anchor: ui::Rect,
    size: (f32, f32),
    viewport: (f32, f32),
    gap: f32,
    margin: f32,
) -> (f32, f32) {
    let (width, height) = size;
    let (viewport_w, viewport_h) = viewport;
    let mut x = anchor.x;
    if x + width > viewport_w - margin {
        x = anchor.x + anchor.w - width;
    }
    let x = x.min(viewport_w - margin - width).max(margin);
    let below = anchor.y + anchor.h + gap;
    let above = anchor.y - gap - height;
    let y = if below + height <= viewport_h - margin {
        below
    } else if above >= margin {
        above
    } else {
        (viewport_h - margin - height).max(margin)
    };
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::place_menu;
    use ui::{Rect, Rgba};

    fn anchor(x: f32, y: f32) -> Rect {
        Rect::new(x, y, 140.0, 28.0, Rgba::TRANSPARENT)
    }

    #[test]
    fn a_menu_opens_under_its_anchor_and_flips_at_the_edges() {
        let viewport = (400.0, 300.0);
        assert_eq!(
            place_menu(anchor(20.0, 20.0), (200.0, 100.0), viewport, 4.0, 8.0),
            (20.0, 52.0)
        );
        assert_eq!(
            place_menu(anchor(250.0, 20.0), (200.0, 100.0), viewport, 4.0, 8.0),
            (190.0, 52.0)
        );
        assert_eq!(
            place_menu(anchor(20.0, 260.0), (200.0, 100.0), viewport, 4.0, 8.0),
            (20.0, 156.0)
        );
        assert_eq!(
            place_menu(anchor(20.0, 100.0), (200.0, 250.0), viewport, 4.0, 8.0),
            (20.0, 42.0)
        );
    }
}
