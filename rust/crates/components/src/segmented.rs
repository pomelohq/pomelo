use ui::{div, theme, Div, IconKind, Node, Rgba};

use crate::ButtonSize;

/// One choice of a [`Segmented`].
pub struct Segment {
    id: u64,
    text: String,
    icon: Option<IconKind>,
    count: Option<usize>,
    dot: Option<Rgba>,
    enabled: bool,
}

pub fn segment(id: u64, text: impl Into<String>) -> Segment {
    Segment {
        id,
        text: text.into(),
        icon: None,
        count: None,
        dot: None,
        enabled: true,
    }
}

impl Segment {
    pub fn icon(mut self, icon: Option<IconKind>) -> Self {
        self.icon = icon;
        self
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    pub fn dot(mut self, color: Rgba) -> Self {
        self.dot = Some(color);
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
    fill: bool,
}

pub fn segmented(segments: impl IntoIterator<Item = Segment>, selected: usize) -> Segmented {
    Segmented {
        segments: segments.into_iter().collect(),
        selected,
        hovered: None,
        size: ButtonSize::Small,
        fill: false,
    }
}

impl Segmented {
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered;
        self
    }

    /// Stretches over its parent's width, the choices sharing it evenly: a panel's tabs.
    pub fn fill(mut self) -> Self {
        self.fill = true;
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
                if !chosen {
                    cell = cell.hover_bg(colors.text.alpha(HOVER_TINT));
                }
            }
            if chosen {
                cell = cell
                    .bg(colors.element_background)
                    .border(1.0, colors.border_variant);
            } else if hot {
                // The ghost hover token is darker than this track in some dark themes, so tint from the text color.
                cell = cell.bg(colors.text.alpha(HOVER_TINT));
            }
            if let Some(kind) = segment.icon {
                cell = cell.child(ui::icon(kind).size(size.icon()).color(ink));
            }
            cell = cell.child(ui::label(segment.text).size(size.text()).color(ink));
            if let Some(count) = segment.count {
                cell = cell.child(
                    ui::label(count.to_string())
                        .size(size.text() - 1.0)
                        .color(colors.text_placeholder),
                );
            }
            if let Some(color) = segment.dot {
                cell = cell.child(crate::dot(color, 6.0));
            }
            if self.fill {
                cell = cell.flex(1.0).justify_center();
            }
            track = track.child(cell);
        }
        // Centered rather than stretched, so a taller parent does not leave the cells hugging the track's top.
        if self.fill {
            div().row().flex(1.0).items_center().child(track.flex(1.0))
        } else {
            div().row().items_center().child(track)
        }
    }
}

impl From<Segmented> for Node {
    fn from(segmented: Segmented) -> Node {
        segmented.render().into()
    }
}

const SEGMENT_INSET: f32 = 2.0;
const HOVER_TINT: f32 = 0.025;
