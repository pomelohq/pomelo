use ui::{div, icon, label, theme, Div, IconKind, Node};

/// What a banner reports, which sets its icon and color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerTone {
    Info,
    Warning,
    Error,
    /// Something is still running; the icon spins in the accent color.
    Busy,
}

/// A card across a page that says one thing went wrong or needs doing, with the action that fixes it and
/// the commands it would run.
pub struct Banner {
    tone: BannerTone,
    text: String,
    action: Option<Node>,
    commands: Vec<String>,
}

pub fn banner(tone: BannerTone, text: impl Into<String>) -> Banner {
    Banner {
        tone,
        text: text.into(),
        action: None,
        commands: Vec::new(),
    }
}

impl Banner {
    pub fn action(mut self, action: impl Into<Node>) -> Self {
        self.action = Some(action.into());
        self
    }

    /// Shell lines shown under the text, so the person sees what a fix runs before running it.
    pub fn commands(mut self, commands: Vec<String>) -> Self {
        self.commands = commands;
        self
    }

    pub fn render(self) -> Div {
        let colors = theme();
        let (kind, tint) = match self.tone {
            BannerTone::Busy => (IconKind::LoadCircle, colors.text_accent),
            BannerTone::Error => (IconKind::XCircle, colors.error),
            BannerTone::Warning => (IconKind::Warning, colors.warning),
            BannerTone::Info => (IconKind::Info, colors.text_accent),
        };
        let mut line = div()
            .row()
            .items_center()
            .gap(10.0)
            .h_px(24.0)
            .child(icon(kind).size(16.0).color(tint))
            .child(
                div().row().flex(1.0).child(
                    label(self.text)
                        .size(12.0)
                        .color(colors.text_muted)
                        .truncate(),
                ),
            );
        if let Some(action) = self.action {
            line = line.child(action);
        }
        let mut card = div()
            .col()
            .gap(8.0)
            .pt(10.0)
            .pb(10.0)
            .pl(14.0)
            .pr(10.0)
            .rounded(BANNER_RADIUS)
            .bg(colors.element_background)
            .child(line);
        if !self.commands.is_empty() {
            let mut block = div()
                .col()
                .flex(1.0)
                .gap(4.0)
                .px(10.0)
                .py(8.0)
                .rounded(6.0)
                .bg(colors.editor_background);
            for command in self.commands {
                block = block.child(
                    div().row().h_px(16.0).items_center().child(
                        label(command)
                            .size(12.0)
                            .mono()
                            .color(colors.text)
                            .truncate(),
                    ),
                );
            }
            card = card.child(div().row().pl(26.0).child(block));
        }
        card
    }
}

impl From<Banner> for Node {
    fn from(banner: Banner) -> Node {
        banner.render().into()
    }
}

pub const BANNER_RADIUS: f32 = 10.0;
