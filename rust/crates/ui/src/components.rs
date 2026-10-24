//! Design-system components built on the element tree, mirroring the reference `ui` crate: label sizes,
//! buttons (by style), and dividers. Values (text sizes, button height/padding, token mapping) are taken
//! from the reference so screens compose these instead of hand-styling divs.

use crate::{div, theme, Div, IconKind, Label, Node, Rgba};

/// UI text sizes (absolute px), matching the reference's `LabelSize` -> `text_ui_*`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LabelSize {
    XSmall,
    Small,
    Default,
    Large,
}

impl LabelSize {
    pub fn px(self) -> f32 {
        match self {
            LabelSize::XSmall => 10.0,
            LabelSize::Small => 12.0,
            LabelSize::Default => 14.0,
            LabelSize::Large => 16.0,
        }
    }
}

impl Label {
    pub fn label_size(self, size: LabelSize) -> Self {
        self.size(size.px())
    }
}

/// Text color roles, mapping to theme tokens like the reference's `Color`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextColor {
    Default,
    Muted,
    Accent,
    Disabled,
    Placeholder,
}

impl TextColor {
    pub fn rgba(self) -> Rgba {
        let t = theme();
        match self {
            TextColor::Default => t.text,
            TextColor::Muted => t.text_muted,
            TextColor::Accent => t.text_accent,
            TextColor::Disabled => t.text_disabled,
            TextColor::Placeholder => t.text_placeholder,
        }
    }
}

/// How a button reads: the one action it is for, a plain alternative, a quiet one, or a destructive one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// Small sits in panels, cards and toolbars; Medium in dialog footers and forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    Small,
    Medium,
}

impl ButtonSize {
    fn height(self) -> f32 {
        match self {
            ButtonSize::Small => 24.0,
            ButtonSize::Medium => 28.0,
        }
    }

    fn pad_x(self) -> f32 {
        match self {
            ButtonSize::Small => 8.0,
            ButtonSize::Medium => 10.0,
        }
    }

    fn text(self) -> f32 {
        match self {
            ButtonSize::Small => 12.0,
            ButtonSize::Medium => 13.0,
        }
    }

    fn icon(self) -> f32 {
        match self {
            ButtonSize::Small => 12.0,
            ButtonSize::Medium => 14.0,
        }
    }
}

/// The one button every screen uses; build it with [`button`] and set what differs from a small secondary.
#[derive(Clone)]
pub struct Button {
    id: u64,
    text: String,
    kind: ButtonKind,
    size: ButtonSize,
    leading: Option<IconKind>,
    keystroke: Option<String>,
    enabled: bool,
    hovered: bool,
}

pub fn button(id: u64, text: impl Into<String>) -> Button {
    Button {
        id,
        text: text.into(),
        kind: ButtonKind::Secondary,
        size: ButtonSize::Small,
        leading: None,
        keystroke: None,
        enabled: true,
        hovered: false,
    }
}

impl Button {
    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn icon(mut self, leading: Option<IconKind>) -> Self {
        self.leading = leading;
        self
    }

    /// The key that runs it, shown in a chip after the label.
    pub fn keystroke(mut self, keystroke: Option<&str>) -> Self {
        self.keystroke = keystroke.map(str::to_string);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// For views drawn without pointer tracking, which pass the hovered id themselves.
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered == Some(self.id);
        self
    }
}

impl From<Button> for Node {
    fn from(button: Button) -> Node {
        let colors = theme();
        let enabled = button.enabled;
        let dim = |color: Rgba| if enabled { color } else { color.alpha(0.45) };
        let primary = button.kind == ButtonKind::Primary;
        let ink = match button.kind {
            ButtonKind::Primary => colors.on_accent,
            ButtonKind::Secondary | ButtonKind::Ghost => colors.text,
            ButtonKind::Danger => colors.error,
        };
        let mut node = div()
            .row()
            .items_center()
            .gap(if button.leading.is_some() { 6.0 } else { 4.0 })
            .h_px(button.size.height())
            .px(button.size.pad_x())
            .rounded(6.0);
        node = match button.kind {
            ButtonKind::Primary => node.bg(dim(colors.accent_fill)),
            ButtonKind::Secondary => node
                .bg(dim(colors.element_background))
                .border(1.0, dim(colors.border)),
            ButtonKind::Ghost | ButtonKind::Danger => node,
        };
        if enabled {
            node = node.on_click(button.id);
            if !primary {
                let hover = if button.kind == ButtonKind::Secondary {
                    raised_hover()
                } else {
                    colors.ghost_element_hover
                };
                node = node.hover_bg(hover);
                if button.hovered {
                    node = node.bg(hover);
                }
            }
        }
        if let Some(kind) = button.leading {
            node = node.child(crate::icon(kind).size(button.size.icon()).color(dim(ink)));
        }
        node = node.child(
            crate::label(button.text)
                .size(button.size.text())
                .medium()
                .color(dim(ink)),
        );
        if let Some(keystroke) = &button.keystroke {
            node = node.child(key_chip(keystroke, primary));
        }
        node.into()
    }
}

/// Hover for a control on the element fill. The hover tokens are tuned for the panel and some dark themes
/// set them darker than this fill, so it lifts the fill with the text color instead.
fn raised_hover() -> Rgba {
    let colors = theme();
    colors.text.alpha(0.06).over(colors.element_background)
}

/// The closed face of a pick-one control: the chosen value and an up-down chevron; clicking it opens the
/// screen's menu of choices. Built with [`select`].
#[derive(Clone)]
pub struct Select {
    id: u64,
    value: String,
    placeholder: String,
    leading: Option<IconKind>,
    size: ButtonSize,
    width: Option<f32>,
    min_width: f32,
    enabled: bool,
    hovered: bool,
}

pub fn select(id: u64, value: impl Into<String>) -> Select {
    Select {
        id,
        value: value.into(),
        placeholder: String::new(),
        leading: None,
        size: ButtonSize::Medium,
        width: None,
        min_width: 0.0,
        enabled: true,
        hovered: false,
    }
}

impl Select {
    /// What shows, dimmed, while nothing is chosen.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn icon(mut self, leading: Option<IconKind>) -> Self {
        self.leading = leading;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// A fixed width; a longer value is cut short.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Hugs its value but never narrower than this, so a column of selects lines up.
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// For views drawn without pointer tracking, which pass the hovered id themselves.
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered == Some(self.id);
        self
    }
}

impl From<Select> for Node {
    fn from(select: Select) -> Node {
        let colors = theme();
        let enabled = select.enabled;
        let dim = |color: Rgba| if enabled { color } else { color.alpha(0.45) };
        let size = select.size;
        let empty = select.value.is_empty();
        let text = if empty {
            select.placeholder
        } else {
            select.value
        };
        let icon_size = size.icon() - 2.0;
        let gap = 8.0;
        let natural = size.pad_x() * 2.0
            + crate::measure_text_width(&text, size.text(), false, crate::ui_font_weight())
            + gap
            + icon_size
            + select.leading.map_or(0.0, |_| size.icon() + gap);
        let mut node = div()
            .row()
            .items_center()
            .gap(gap)
            .h_px(size.height())
            .px(size.pad_x())
            .rounded(6.0)
            .bg(dim(colors.element_background))
            .border(1.0, dim(colors.border));
        node = match select.width {
            Some(width) => node.w_px(width),
            None => node.w_px(natural.max(select.min_width)),
        };
        if enabled {
            node = node.on_click(select.id).hover_bg(raised_hover());
            if select.hovered {
                node = node.bg(raised_hover());
            }
        }
        if let Some(kind) = select.leading {
            node = node.child(
                crate::icon(kind)
                    .size(size.icon())
                    .color(dim(colors.icon_muted)),
            );
        }
        node.child(
            div().row().flex(1.0).child(
                crate::label(text)
                    .size(size.text())
                    .color(dim(if empty {
                        colors.text_placeholder
                    } else {
                        colors.text
                    }))
                    .truncate(),
            ),
        )
        .child(
            crate::icon(IconKind::ChevronUpDown)
                .size(icon_size)
                .color(dim(colors.icon_muted)),
        )
        .into()
    }
}

/// A key binding in a small cap, for buttons and footers; `on_accent` when it sits on an accent fill.
pub fn key_chip(keystroke: &str, on_accent: bool) -> Node {
    let colors = theme();
    let (fill, ink) = if on_accent {
        (Rgba::new(1.0, 1.0, 1.0, 0.18), colors.on_accent)
    } else {
        let fill = match colors.appearance {
            crate::Appearance::Dark => Rgba::new(1.0, 1.0, 1.0, 0.05),
            crate::Appearance::Light => colors.element_active,
        };
        (fill, colors.text_muted)
    };
    div()
        .row()
        .h_px(18.0)
        .px(5.0)
        .items_center()
        .rounded(4.0)
        .bg(fill)
        .child(crate::render_keystroke_in(keystroke, 11.0, ink))
        .into()
}

/// A short read-only tag: a branch, a kind, a plan, a detected stack.
pub struct Chip {
    label: String,
    color: Option<Rgba>,
    icon: Option<IconKind>,
    fill: Option<Rgba>,
    border: Option<Rgba>,
}

pub fn chip(label: impl Into<String>) -> Chip {
    Chip {
        label: label.into(),
        color: None,
        icon: None,
        fill: None,
        border: None,
    }
}

impl Chip {
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn icon(mut self, icon: IconKind) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn fill(mut self, fill: Rgba) -> Self {
        self.fill = Some(fill);
        self
    }

    pub fn border(mut self, border: Rgba) -> Self {
        self.border = Some(border);
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let ink = self.color.unwrap_or(colors.text);
        let mut chip = div()
            .row()
            .h_px(18.0)
            .px(4.0)
            .gap(2.0)
            .items_center()
            .rounded(4.0)
            .border(1.0, self.border.unwrap_or(colors.border))
            .bg(self.fill.unwrap_or(colors.element_background));
        if let Some(kind) = self.icon {
            chip = chip.child(crate::icon(kind).size(10.0).color(ink));
        }
        chip.child(
            crate::label(self.label)
                .label_size(LabelSize::XSmall)
                .mono()
                .color(ink),
        )
    }
}

impl From<Chip> for Node {
    fn from(chip: Chip) -> Node {
        chip.render().into()
    }
}

/// One choice of a [`Segmented`].
pub struct Segment {
    id: u64,
    text: String,
    icon: Option<IconKind>,
    enabled: bool,
}

pub fn segment(id: u64, text: impl Into<String>) -> Segment {
    Segment {
        id,
        text: text.into(),
        icon: None,
        enabled: true,
    }
}

impl Segment {
    pub fn icon(mut self, icon: Option<IconKind>) -> Self {
        self.icon = icon;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Mutually exclusive choices on a sunken track, the chosen one raised: a toolbar filter or a form choice.
pub struct Segmented {
    segments: Vec<Segment>,
    selected: usize,
    hovered: Option<u64>,
    size: ButtonSize,
}

pub fn segmented(segments: impl IntoIterator<Item = Segment>, selected: usize) -> Segmented {
    Segmented {
        segments: segments.into_iter().collect(),
        selected,
        hovered: None,
        size: ButtonSize::Small,
    }
}

impl Segmented {
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let size = self.size;
        let mut track = div()
            .row()
            .p(SEGMENT_INSET)
            .gap(SEGMENT_INSET)
            .rounded(6.0)
            .bg(colors.background);
        for (index, segment) in self.segments.into_iter().enumerate() {
            let chosen = index == self.selected;
            let hot = !chosen && segment.enabled && self.hovered == Some(segment.id);
            let ink = if !segment.enabled {
                colors.text_disabled
            } else if chosen || hot {
                colors.text
            } else {
                colors.text_muted
            };
            let mut cell = div()
                .row()
                .h_px(size.height() - 2.0 * SEGMENT_INSET)
                .px(size.pad_x())
                .gap(6.0)
                .items_center()
                .rounded(4.0);
            if segment.enabled {
                cell = cell.on_click(segment.id);
            }
            if chosen {
                cell = cell
                    .bg(colors.element_background)
                    .border(1.0, colors.border_variant);
            } else if hot {
                // The ghost hover token is darker than this track in some dark themes, so tint from the text color.
                cell = cell.bg(colors.text.alpha(0.025));
            }
            if let Some(kind) = segment.icon {
                cell = cell.child(crate::icon(kind).size(size.icon()).color(ink));
            }
            track =
                track.child(cell.child(crate::label(segment.text).size(size.text()).color(ink)));
        }
        div().row().child(track)
    }
}

impl From<Segmented> for Node {
    fn from(segmented: Segmented) -> Node {
        segmented.render().into()
    }
}

const SEGMENT_INSET: f32 = 2.0;

const TOOLTIP_HEIGHT: f32 = 24.0;

/// The bubble a tooltip shows: its text, then the keys that run the action when there are any.
pub fn tooltip_bubble(text: &str, keys: Option<&str>) -> Div {
    let colors = theme();
    let mut content = div()
        .row()
        .gap(8.0)
        .items_center()
        .child(crate::label(text.to_string()).size(12.0).color(colors.text));
    if let Some(keys) = keys.filter(|keys| !keys.is_empty()) {
        let mut strokes = div().row().gap(4.0).items_center();
        for stroke in keys.split_whitespace() {
            strokes = strokes.child(crate::render_keystroke(stroke, 12.0));
        }
        content = content.child(strokes);
    }
    div()
        .row()
        .items_center()
        .px(8.0)
        .h_px(TOOLTIP_HEIGHT)
        .rounded(6.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border)
        .child(content)
}

/// `target` with `text` in a bubble pinned under it, or over it when `above`.
pub fn with_tooltip(target: Div, text: &str, above: bool) -> Div {
    let width = crate::measure_text_width(text, 12.0, false, crate::ui_font_weight())
        / crate::ui_text_scale()
        + 16.0;
    let (corner, dy) = if above {
        (crate::Corner::TopLeft, -(TOOLTIP_HEIGHT + 4.0))
    } else {
        (crate::Corner::BottomLeft, TOOLTIP_HEIGHT + 4.0)
    };
    target.pin(
        corner,
        0.0,
        dy,
        width,
        TOOLTIP_HEIGHT,
        crate::deferred(tooltip_bubble(text, None))
            .priority(5)
            .snap_to_window(),
    )
}

/// The one pill: a state or something waiting on the person, as a dot and words in `color` on its wash.
pub fn status_pill(text: &str, color: Rgba) -> Node {
    div()
        .row()
        .items_center()
        .gap(5.0)
        .h_px(20.0)
        .px(7.0)
        .rounded(10.0)
        .bg(color.alpha(0.14))
        .child(dot(color, 6.0))
        .child(
            crate::label(text.to_string())
                .size(11.0)
                .medium()
                .color(color)
                .truncate(),
        )
        .into()
}

/// A round status mark: running, failing, needs a look.
pub fn dot(color: Rgba, size: f32) -> Div {
    div().w_px(size).h_px(size).rounded(size / 2.0).bg(color)
}

/// The hollow form of [`dot`], for a state that has not started.
pub fn ring(color: Rgba, size: f32) -> Div {
    div()
        .w_px(size)
        .h_px(size)
        .rounded(size / 2.0)
        .border(1.5, color)
}

/// A 1px horizontal divider in the default border color.
pub fn divider() -> Div {
    div().h_px(1.0).bg(theme().border_variant)
}

/// A 1px horizontal divider in the faded border color (reference: `DividerColor::BorderFaded`).
pub fn divider_faded() -> Div {
    let b = theme().border;
    div().h_px(1.0).bg(Rgba::new(b.r, b.g, b.b, b.a * 0.6))
}

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
pub const MENU_PAD: f32 = 4.0;
pub const MENU_SEARCH_ROW: f32 = 32.0;
pub const MENU_SEARCH_INSET: f32 = 8.0;
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
}

pub fn menu(items: &[MenuItem]) -> Menu<'_> {
    Menu {
        items,
        hovered: None,
        opens_submenu: |_| false,
        search: None,
        search_at_bottom: false,
        min_width: MENU_MIN_WIDTH,
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

    /// A search line above the items showing `query`, or a placeholder while it is empty.
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

    /// The card and its width and height in points.
    pub fn render(self) -> (Node, f32, f32) {
        let colors = theme();
        let weight = crate::ui_font_weight();
        let marks = self
            .items
            .iter()
            .any(|item| item.checked || item.icon.is_some());
        let mark_width = if marks { 12.0 + 6.0 } else { 0.0 };
        let mut content_width = 0.0_f32;
        for item in self.items {
            let label_width = crate::measure_text_width(&item.label, 13.0, false, weight);
            let right = match (&item.hint, (self.opens_submenu)(item.id)) {
                (Some(hint), true) => {
                    14.0 + crate::measure_text_width(hint, 12.0, false, weight) + 12.0
                }
                (None, true) => 14.0,
                (Some(key), false) if !key.is_empty() => {
                    crate::measure(&crate::render_keystroke(key, 12.0)).0 + 24.0
                }
                _ => 0.0,
            };
            content_width = content_width.max(label_width + right);
        }
        let width = (16.0 + mark_width + content_width + 2.0 * MENU_PAD).max(self.min_width);
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
        let height = MENU_PAD * 2.0
            + search_height
            + MENU_ROW * self.items.len() as f32
            + MENU_SEPARATOR * separators;
        let separator = || {
            div().h_px(MENU_SEPARATOR).py(2.0).child(
                div()
                    .h_px(1.0)
                    .w_px(width - 2.0 * MENU_PAD)
                    .bg(colors.border),
            )
        };
        let mut card = div()
            .col()
            .p(MENU_PAD)
            .rounded(8.0)
            .bg(colors.elevated_surface_background)
            .border(1.0, colors.border);
        let mut search = self.search.map(|query| {
            let line = if query.is_empty() {
                crate::label("Search...")
                    .size(12.0)
                    .color(colors.text_muted)
            } else {
                crate::label(query.to_string())
                    .size(12.0)
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
                card = card.child(
                    div().row().h_px(MENU_ROW).px(8.0).items_center().child(
                        crate::label(item.label.to_string())
                            .size(12.0)
                            .color(colors.text_muted),
                    ),
                );
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
            .px(8.0)
            .gap(6.0)
            .items_center()
            .rounded(5.0)
            .on_click(item.id);
        if self.hovered == Some(item.id) && !item.disabled {
            row = row.bg(menu_hover());
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
                (true, _) => crate::check_icon().size(12.0),
                (false, Some(kind)) => {
                    crate::icon(kind)
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
                (false, None) => crate::icon(IconKind::Check)
                    .size(12.0)
                    .color(Rgba::TRANSPARENT),
            });
        }
        row = row.child(
            crate::label(item.label.to_string())
                .size(13.0)
                .color(label_color),
        );
        if (self.opens_submenu)(item.id) {
            row = row.child(div().flex(1.0));
            if let Some(hint) = &item.hint {
                row = row.child(
                    crate::label(hint.to_string())
                        .size(12.0)
                        .color(colors.text_muted),
                );
            }
            row.child(
                crate::icon(IconKind::ChevronRight)
                    .size(12.0)
                    .color(colors.icon_muted),
            )
        } else {
            match item.hint.as_deref() {
                Some(key) if !key.is_empty() => row
                    .child(div().flex(1.0))
                    .child(crate::render_keystroke(key, 12.0)),
                _ => row,
            }
        }
    }
}

impl From<Menu<'_>> for Node {
    fn from(menu: Menu<'_>) -> Node {
        menu.render().0
    }
}

// On the raised card the element fill is not under the row, so tint toward the text instead.
fn menu_hover() -> Rgba {
    match theme().appearance {
        crate::Appearance::Dark => Rgba::new(1.0, 1.0, 1.0, 0.05),
        crate::Appearance::Light => Rgba::new(0.0, 0.0, 0.0, 0.04),
    }
}

/// Where a `size` menu opening from `anchor` goes inside `viewport`: under it and left-aligned when it
/// fits, right-aligned to it near the right edge, above it near the bottom; never past `margin`.
pub fn place_menu(
    anchor: crate::Rect,
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
    use crate::{Rect, Rgba};

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
