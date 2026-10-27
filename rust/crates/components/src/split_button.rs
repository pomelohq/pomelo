use ui::{div, icon, label, theme, Div, IconKind, Node, Radius, Rgba, Space};

use crate::button::raised_hover;
use crate::ButtonSize;

/// The main action and a chevron that opens its other ways (Merge / Squash / Rebase), as one control.
/// Built with [`split_button`].
pub struct SplitButton {
    main: u64,
    menu: u64,
    text: String,
    note: Option<String>,
    enabled: bool,
    hovered: Option<u64>,
    primary: bool,
    size: ButtonSize,
    width: Option<f32>,
}

pub fn split_button(main: u64, menu: u64, text: impl Into<String>) -> SplitButton {
    SplitButton {
        main,
        menu,
        text: text.into(),
        note: None,
        enabled: true,
        hovered: None,
        primary: false,
        size: ButtonSize::Small,
        width: None,
    }
}

impl SplitButton {
    pub fn note(mut self, note: Option<&str>) -> Self {
        self.note = note.map(str::to_string);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn hovered(mut self, hovered: Option<u64>) -> Self {
        self.hovered = hovered;
        self
    }

    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Stretches the control to `width`, its label centered in the main half.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn ink(&self) -> Rgba {
        let colors = theme();
        match (self.enabled, self.primary) {
            (false, _) => colors.text_placeholder,
            (true, true) => colors.on_accent,
            (true, false) => colors.text,
        }
    }

    fn hover(&self) -> Rgba {
        if self.primary {
            theme().on_accent.alpha(0.12)
        } else {
            raised_hover()
        }
    }

    /// The main half alone, for a caller that wraps it in a tooltip before [`SplitButton::finish`].
    pub fn main_half(&self) -> Div {
        let colors = theme();
        // A disabled button still takes the pointer, so its tooltip can say why.
        let mut main = div()
            .row()
            .h_px(self.size.height())
            .px(Space::Px(self.size.pad_x() + 2.0))
            .gap(Space::S6)
            .items_center()
            .on_click(self.main);
        if self.width.is_some() {
            main = main.flex(1.0).justify_center();
        }
        if self.enabled && self.hovered == Some(self.main) {
            main = main.bg(self.hover());
        }
        main = main.child(
            label(self.text.clone())
                .size(self.size.text())
                .medium()
                .color(self.ink()),
        );
        if let Some(note) = &self.note {
            main = main.child(label(note.clone()).size(self.size.text()).color(
                if self.primary && self.enabled {
                    colors.on_accent.alpha(0.85)
                } else {
                    colors.text_placeholder
                },
            ));
        }
        main
    }

    pub fn finish(self, main: Div) -> Node {
        let colors = theme();
        let height = self.size.height();
        let mut chevron = div()
            .row()
            .w_px(height)
            .h_px(height)
            .items_center()
            .justify_center()
            .child(icon(IconKind::ChevronDown).size(self.size.icon()).color(
                match (self.enabled, self.primary) {
                    (true, true) => colors.on_accent,
                    _ => colors.icon_muted,
                },
            ));
        if self.enabled {
            chevron = chevron.on_click(self.menu);
            if self.hovered == Some(self.menu) {
                chevron = chevron.bg(self.hover());
            }
        }
        let (fill, divider) = match (self.enabled, self.primary) {
            (true, true) => (colors.accent_fill, colors.on_accent.alpha(0.2)),
            (false, true) => (colors.element_hover, colors.border_variant),
            _ => (colors.element_background, colors.border),
        };
        let mut control = div().row().rounded(Radius::Medium).items_center().bg(fill);
        if let Some(width) = self.width {
            control = control.w_px(width);
        }
        if !self.primary {
            control = control.border(1.0, colors.border);
        }
        control
            .child(main)
            .child(div().w_px(1.0).h_px(height).bg(divider))
            .child(chevron)
            .into()
    }
}

impl From<SplitButton> for Node {
    fn from(button: SplitButton) -> Node {
        let main = button.main_half();
        button.finish(main)
    }
}
