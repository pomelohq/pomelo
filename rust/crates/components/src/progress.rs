use ui::{div, theme, Node, Rgba, Space};

/// A determinate bar: an 8px track with a 2px inset, filled in the info color (never below 2%).
pub struct ProgressBar {
    value: f32,
    thin: bool,
    color: Option<Rgba>,
    height: Option<f32>,
    width: Option<f32>,
}

pub fn progress_bar(value: f32) -> ProgressBar {
    ProgressBar {
        value,
        thin: false,
        color: None,
        height: None,
        width: None,
    }
}

impl ProgressBar {
    /// A 3px line with no track inset, for a bar that sits inside a button.
    pub fn thin(mut self) -> Self {
        self.thin = true;
        self
    }

    /// A bar whose fill says how close to a limit it is (a usage meter), rather than the info color.
    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    /// A line of this height with no inset, for a meter.
    pub fn height(mut self, height: f32) -> Self {
        self.thin = true;
        self.height = Some(height);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
}

impl From<ProgressBar> for Node {
    fn from(bar: ProgressBar) -> Node {
        let colors = theme();
        let fraction = bar.value.clamp(0.02, 1.0);
        let (track, inset, fill) = match (bar.thin, bar.height) {
            (_, Some(height)) => (height, 0.0, height),
            (true, None) => (3.0, 0.0, 3.0),
            (false, None) => (8.0, 2.0, 4.0),
        };
        let mut node = div()
            .row()
            .h_px(track)
            .p(Space::Px(inset))
            .rounded_px(track / 2.0)
            .bg(if bar.thin {
                colors.element_active
            } else {
                colors.background
            });
        node = match bar.width {
            Some(width) => node.w_px(width),
            None => node.flex(1.0),
        };
        node.child(
            div()
                .h_px(fill)
                .flex(fraction)
                .rounded_px(fill / 2.0)
                .bg(bar.color.unwrap_or(colors.info)),
        )
        .child(div().h_px(fill).flex(1.0 - fraction))
        .into()
    }
}
