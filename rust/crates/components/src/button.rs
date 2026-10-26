use ui::{div, theme, Div, IconKind, LabelSize, Node, Radius, Rgba, Space};

use crate::key_chip;

/// How a button reads: the one action it is for, a plain alternative, a quiet one, a destructive one, or
/// one that hands the work to an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
    Agent,
}

/// The tint every agent action wears, so asking an agent reads the same everywhere.
pub fn agent_tint() -> Rgba {
    theme().terminal_ansi[5]
}

/// XSmall sits inline beside a value, Small in panels, cards and toolbars, Medium in dialog footers and forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    XSmall,
    Small,
    Medium,
}

impl ButtonSize {
    pub(crate) fn height(self) -> f32 {
        match self {
            ButtonSize::XSmall => 18.0,
            ButtonSize::Small => 24.0,
            ButtonSize::Medium => 28.0,
        }
    }

    pub(crate) fn pad_x(self) -> f32 {
        match self {
            ButtonSize::XSmall => 4.0,
            ButtonSize::Small => 8.0,
            ButtonSize::Medium => 10.0,
        }
    }

    pub(crate) fn text(self) -> LabelSize {
        match self {
            ButtonSize::XSmall => LabelSize::Mini,
            ButtonSize::Small => LabelSize::Small,
            ButtonSize::Medium => LabelSize::Medium,
        }
    }

    pub(crate) fn icon(self) -> f32 {
        match self {
            ButtonSize::XSmall => 11.0,
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
        button.render().into()
    }
}

impl Button {
    /// The button as a box, for a caller that wraps it in a tooltip or sizes it.
    pub fn render(self) -> Div {
        let button = self;
        let colors = theme();
        let enabled = button.enabled;
        let dim = |color: Rgba| if enabled { color } else { color.alpha(0.45) };
        let primary = button.kind == ButtonKind::Primary;
        let ink = match button.kind {
            ButtonKind::Primary => colors.on_accent,
            ButtonKind::Secondary | ButtonKind::Ghost | ButtonKind::Agent => colors.text,
            ButtonKind::Danger => colors.error,
        };
        let mut node = div()
            .row()
            .items_center()
            .gap(if button.leading.is_some() {
                Space::S6
            } else {
                Space::S4
            })
            .h_px(button.size.height())
            .px(Space::Px(button.size.pad_x()))
            .rounded(Radius::Medium);
        node = match button.kind {
            ButtonKind::Primary => node.bg(dim(colors.accent_fill)),
            ButtonKind::Secondary => node
                .bg(dim(colors.element_background))
                .border(1.0, dim(colors.border)),
            ButtonKind::Agent => node
                .bg(dim(agent_tint().alpha(0.12)))
                .border(1.0, dim(agent_tint().alpha(0.55))),
            ButtonKind::Ghost | ButtonKind::Danger => node,
        };
        if enabled {
            node = node.on_click(button.id);
            if !primary {
                let hover = match button.kind {
                    ButtonKind::Secondary => raised_hover(),
                    ButtonKind::Agent => agent_tint().alpha(0.2),
                    _ => colors.ghost_element_hover,
                };
                node = node.hover_bg(hover);
                if button.hovered {
                    node = node.bg(hover);
                }
            }
        }
        if let Some(kind) = button.leading {
            let glyph = if button.kind == ButtonKind::Agent {
                agent_tint()
            } else {
                ink
            };
            node = node.child(ui::icon(kind).size(button.size.icon()).color(dim(glyph)));
        }
        node = node.child(
            ui::label(button.text)
                .size(button.size.text())
                .medium()
                .color(dim(ink)),
        );
        if let Some(keystroke) = &button.keystroke {
            node = node.child(key_chip(keystroke, primary));
        }
        node
    }
}

/// Hover for a control on the element fill. The hover tokens are tuned for the panel and some dark themes
/// set them darker than this fill, so it lifts the fill with the text color instead.
pub(crate) fn raised_hover() -> Rgba {
    let colors = theme();
    colors.text.alpha(0.06).over(colors.element_background)
}
