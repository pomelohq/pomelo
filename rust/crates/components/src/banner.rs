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
    icon: Option<IconKind>,
    lead: Option<Node>,
    detail: Option<String>,
    wrap: Option<f32>,
    mono: bool,
    actions: Vec<Node>,
    stack_actions: bool,
    commands: Vec<String>,
    flush: bool,
}

pub fn banner(tone: BannerTone, text: impl Into<String>) -> Banner {
    Banner {
        tone,
        text: text.into(),
        icon: None,
        lead: None,
        detail: None,
        wrap: None,
        mono: false,
        actions: Vec::new(),
        stack_actions: false,
        commands: Vec::new(),
        flush: false,
    }
}

impl Banner {
    /// Another action after the ones already added.
    pub fn action(mut self, action: impl Into<Node>) -> Self {
        self.actions.push(action.into());
        self
    }

    /// Shell lines shown under the text, so the person sees what a fix runs before running it.
    pub fn commands(mut self, commands: Vec<String>) -> Self {
        self.commands = commands;
        self
    }

    /// An icon that says more than the tone's own (an eye for "someone else drives this").
    pub fn icon(mut self, kind: IconKind) -> Self {
        self.icon = Some(kind);
        self
    }

    /// What stands in front of the text instead of the icon: a pill naming the state.
    pub fn lead(mut self, lead: impl Into<Node>) -> Self {
        self.lead = Some(lead.into());
        self
    }

    /// A second, quieter line: what happens meanwhile.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The whole text over as many lines as it needs within `width`, for a message that cannot be cut.
    pub fn wrap(mut self, width: f32) -> Self {
        self.wrap = Some(width);
        self
    }

    /// The text in the code font, for a command or a tool call.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// The actions on their own line under the text, for a narrow panel.
    pub fn stack_actions(mut self, stack: bool) -> Self {
        self.stack_actions = stack;
        self
    }

    /// Edge to edge across the top of a panel, square, with a line under it.
    pub fn flush(mut self) -> Self {
        self.flush = true;
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
        let lead: Node = match self.lead {
            Some(lead) => lead,
            None => icon(self.icon.unwrap_or(kind))
                .size(16.0)
                .color(tint)
                .into(),
        };
        let ink = if self.detail.is_some() || self.wrap.is_some() || self.mono {
            colors.text
        } else {
            colors.text_muted
        };
        let mut text = label(self.text).size(12.0).color(ink);
        if self.mono {
            text = text.mono();
        }
        text = match self.wrap {
            Some(width) => text.wrap(width),
            None => text.truncate(),
        };
        let mut words = div().col().flex(1.0).gap(2.0).child(text);
        if let Some(detail) = self.detail {
            words = words.child(label(detail).size(12.0).color(colors.text_muted).truncate());
        }
        let mut line = div()
            .row()
            .items_center()
            .gap(10.0)
            .child(div().row().h_px(24.0).items_center().child(lead))
            .child(words);
        let mut actions = div().row().gap(6.0).items_center();
        let has_actions = !self.actions.is_empty();
        for action in self.actions {
            actions = actions.child(action);
        }
        if has_actions && !self.stack_actions {
            line = line.child(actions);
            actions = div();
        }
        let mut card = div().col().gap(8.0).py(10.0).pl(14.0).pr(10.0).child(line);
        if has_actions && self.stack_actions {
            card = card.child(div().row().justify_end().child(actions));
        }
        card = if self.flush {
            card.bg(colors.panel_background)
        } else {
            card.rounded(BANNER_RADIUS).bg(colors.element_background)
        };
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
        if self.flush {
            div().col().child(card).child(crate::edge_divider())
        } else {
            card
        }
    }
}

impl From<Banner> for Node {
    fn from(banner: Banner) -> Node {
        banner.render().into()
    }
}

pub const BANNER_RADIUS: f32 = 10.0;
