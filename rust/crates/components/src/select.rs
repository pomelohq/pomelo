use ui::{div, theme, IconKind, Node, Rgba};

use crate::button::raised_hover;
use crate::ButtonSize;

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
    quiet: bool,
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
        quiet: false,
    }
}

impl Select {
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

    /// No fill or edge until hovered, for a select inside a line of text (a header, a toolbar).
    pub fn quiet(mut self) -> Self {
        self.quiet = true;
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
            + ui::measure_text_width(&text, size.text(), false, ui::ui_font_weight())
            + gap
            + icon_size
            + select.leading.map_or(0.0, |_| size.icon() + gap);
        let mut node = div()
            .row()
            .items_center()
            .gap(gap)
            .h_px(size.height())
            .px(size.pad_x())
            .rounded(6.0);
        if !select.quiet {
            node = node
                .bg(dim(colors.element_background))
                .border(1.0, dim(colors.border));
        }
        let hover = if select.quiet {
            colors.ghost_element_hover
        } else {
            raised_hover()
        };
        node = match select.width {
            Some(width) => node.w_px(width),
            None => node.w_px(natural.max(select.min_width)),
        };
        if enabled {
            node = node.on_click(select.id).hover_bg(hover);
            if select.hovered {
                node = node.bg(hover);
            }
        }
        if let Some(kind) = select.leading {
            node = node.child(
                ui::icon(kind)
                    .size(size.icon())
                    .color(dim(colors.icon_muted)),
            );
        }
        node.child(
            div().row().flex(1.0).child(
                ui::label(text)
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
            ui::icon(IconKind::ChevronUpDown)
                .size(icon_size)
                .color(dim(colors.icon_muted)),
        )
        .into()
    }
}
