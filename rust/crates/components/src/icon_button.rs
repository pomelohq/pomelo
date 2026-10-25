use ui::{div, theme, Div, IconKind, Node, Rgba};

use crate::ButtonSize;

/// A square button showing only an icon, for toolbars and the actions beside a value; say what it does
/// with a tooltip.
pub struct IconButton {
    id: u64,
    icon: IconKind,
    size: ButtonSize,
    color: Option<Rgba>,
    toggled: bool,
    enabled: bool,
    hovered: bool,
}

pub fn icon_button(id: u64, icon: IconKind) -> IconButton {
    IconButton {
        id,
        icon,
        size: ButtonSize::Small,
        color: None,
        toggled: false,
        enabled: true,
        hovered: false,
    }
}

impl IconButton {
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn toggled(mut self, toggled: bool) -> Self {
        self.toggled = toggled;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Lit as hovered when `hovered` is this button, for screens that track the pointer themselves.
    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered == Some(self.id);
        self
    }
}

impl From<IconButton> for Node {
    fn from(button: IconButton) -> Node {
        button.render().into()
    }
}

impl IconButton {
    /// The button as a box, for a caller that wraps it (a tooltip).
    pub fn render(self) -> Div {
        let button = self;
        let colors = theme();
        let side = button.size.height();
        let glyph = match button.size {
            ButtonSize::XSmall => 11.0,
            ButtonSize::Small => 14.0,
            ButtonSize::Medium => 16.0,
        };
        let ink = if !button.enabled {
            colors.text_disabled
        } else if button.toggled {
            colors.text_accent
        } else {
            button.color.unwrap_or(colors.icon_muted)
        };
        let mut node = div()
            .row()
            .w_px(side)
            .h_px(side)
            .items_center()
            .justify_center()
            .rounded(if button.size == ButtonSize::XSmall {
                4.0
            } else {
                6.0
            })
            .child(ui::icon(button.icon).size(glyph).color(ink));
        if button.toggled {
            node = node.bg(colors.element_selected);
        }
        if button.enabled {
            node = node
                .on_click(button.id)
                .active_bg(colors.ghost_element_active);
            if !button.toggled {
                node = node.hover_bg(colors.ghost_element_hover);
                if button.hovered {
                    node = node.bg(colors.ghost_element_hover);
                }
            }
        }
        node
    }
}
