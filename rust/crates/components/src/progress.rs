use ui::{div, theme, Node};

/// A determinate bar: an 8px track with a 2px inset, filled in the info color (never below 2%).
pub struct ProgressBar {
    value: f32,
    thin: bool,
}

pub fn progress_bar(value: f32) -> ProgressBar {
    ProgressBar { value, thin: false }
}

impl ProgressBar {
    /// A 3px line with no track inset, for a bar that sits inside a button.
    pub fn thin(mut self) -> Self {
        self.thin = true;
        self
    }
}

impl From<ProgressBar> for Node {
    fn from(bar: ProgressBar) -> Node {
        let colors = theme();
        let fraction = bar.value.clamp(0.02, 1.0);
        let (track, inset, fill) = if bar.thin {
            (3.0, 0.0, 3.0)
        } else {
            (8.0, 2.0, 4.0)
        };
        div()
            .row()
            .flex(1.0)
            .h_px(track)
            .p(inset)
            .rounded(track / 2.0)
            .bg(if bar.thin {
                colors.element_active
            } else {
                colors.background
            })
            .child(
                div()
                    .h_px(fill)
                    .flex(fraction)
                    .rounded(fill / 2.0)
                    .bg(colors.info),
            )
            .child(div().h_px(fill).flex(1.0 - fraction))
            .into()
    }
}
