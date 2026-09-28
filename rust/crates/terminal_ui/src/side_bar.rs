//! The row over a side agent's console: what it is for, whether it may edit, what it started with, and the
//! button that hands its answer to the main agent's prompt.

use ui::{div, icon, label, theme, IconKind, Node, Rgba};

use crate::item::{ConsoleAction, ConsoleToolbar};

const SEND: u64 = 0;
/// Ids far above every panel's and pane group's.
const IDS: u64 = 1 << 51;
const IDS_PER_BAR: u64 = 8;

pub struct SideAgentBar {
    base: u64,
    role: String,
    read_only: bool,
    /// What it started with ("Forked from main - 12k", "Fresh").
    context: String,
    /// The main agent's tab.
    main_item: String,
    /// Its last answer, for a send with nothing selected.
    last_answer: Box<dyn Fn() -> Option<String>>,
    action: Option<ConsoleAction>,
}

impl SideAgentBar {
    pub fn new(
        number: u64,
        role: &str,
        read_only: bool,
        context: String,
        main_item: String,
        last_answer: Box<dyn Fn() -> Option<String>>,
    ) -> SideAgentBar {
        SideAgentBar {
            base: IDS + number * IDS_PER_BAR,
            role: role.to_string(),
            read_only,
            context,
            main_item,
            last_answer,
            action: None,
        }
    }
}

impl ConsoleToolbar for SideAgentBar {
    fn render(&self, width: f32, _lines: usize) -> Node {
        let colors = theme();
        let (tag, tag_color) = if self.read_only {
            ("read-only", colors.text_muted)
        } else {
            ("can edit", colors.warning)
        };
        let chip = |text: String, color: Rgba| -> Node {
            div()
                .row()
                .h_px(18.0)
                .px(6.0)
                .items_center()
                .rounded(3.0)
                .border(1.0, colors.border_variant)
                .child(label(text).size(11.0).color(color))
                .into()
        };
        let send = div()
            .row()
            .h_px(22.0)
            .px(8.0)
            .gap(5.0)
            .items_center()
            .rounded(4.0)
            .bg(colors.info_background)
            .border(1.0, colors.info_border)
            .on_click(self.base + SEND)
            .child(icon(IconKind::ArrowUpRight).size(11.0).color(colors.icon))
            .child(label("Send to main").size(12.0).color(colors.text));
        div()
            .col()
            .w_px(width)
            .child(
                div()
                    .row()
                    .h_px(32.0)
                    .px(10.0)
                    .gap(6.0)
                    .items_center()
                    .bg(colors.toolbar_background)
                    .child(
                        icon(IconKind::Sparkle)
                            .size(12.0)
                            .color(colors.terminal_ansi[5]),
                    )
                    .child(
                        label(self.role.clone())
                            .size(12.5)
                            .medium()
                            .color(colors.text),
                    )
                    .child(chip(tag.to_string(), tag_color))
                    .child(chip(self.context.clone(), colors.text_muted))
                    .child(div().flex(1.0))
                    .child(send),
            )
            .child(div().h_px(1.0).bg(colors.border))
            .into()
    }

    fn click(&mut self, id: u64) -> bool {
        if id == self.base + SEND {
            self.action = Some(ConsoleAction::Send);
            return true;
        }
        false
    }

    fn take_respawn(&mut self) -> Option<(terminal::TerminalOptions, terminal::Waker)> {
        None
    }

    fn take_action(&mut self) -> Option<ConsoleAction> {
        self.action.take()
    }

    fn send_target(&self) -> Option<(String, Option<String>)> {
        Some((self.main_item.clone(), (self.last_answer)()))
    }
}
