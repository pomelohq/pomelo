//! The row over a side agent's console: what it is for, whether it may edit, what it started with, and the
//! button that hands its answer to the main agent's prompt.

use ui::{div, icon, label, theme, IconKind, Node};

use crate::item::{ConsoleAction, ConsoleToolbar};

const SEND: u64 = 0;
/// Ids far above every panel's and pane group's.
const IDS: u64 = 1 << 51;
const IDS_PER_BAR: u64 = 8;

const ARCHIVE: u64 = 1;

pub struct SideAgentBar {
    base: u64,
    role_icon: IconKind,
    role: String,
    read_only: bool,
    /// What it started with ("Auto: fork + compact - 9.3k", "Fresh - packet").
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
        role_icon: IconKind,
        role: &str,
        read_only: bool,
        context: String,
        main_item: String,
        last_answer: Box<dyn Fn() -> Option<String>>,
    ) -> SideAgentBar {
        SideAgentBar {
            base: IDS + number * IDS_PER_BAR,
            role_icon,
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
        let tag = if self.read_only {
            div()
                .row()
                .h_px(20.0)
                .px(6.0)
                .items_center()
                .rounded(4.0)
                .border(1.0, colors.border_variant)
                .child(label("read-only").size(12.0).color(colors.text_muted))
        } else {
            div()
                .row()
                .h_px(20.0)
                .px(6.0)
                .items_center()
                .rounded(4.0)
                .border(1.0, colors.warning.alpha(0.5))
                .child(label("can edit").size(12.0).color(colors.warning))
        };
        let context = div()
            .row()
            .h_px(24.0)
            .px(9.0)
            .gap(5.0)
            .items_center()
            .rounded(12.0)
            .border(1.0, colors.border_variant)
            .child(icon(IconKind::Branch).size(12.0).color(colors.icon_muted))
            .child(label(self.context.clone()).size(12.5).color(colors.text));
        let send = div()
            .row()
            .h_px(26.0)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .rounded(5.0)
            .border(1.0, colors.border_variant)
            .on_click(self.base + SEND)
            .child(icon(IconKind::Return).size(12.0).color(colors.icon_muted))
            .child(label("Send to main").size(12.5).color(colors.text));
        let archive = div()
            .row()
            .w_px(26.0)
            .h_px(26.0)
            .items_center()
            .justify_center()
            .rounded(4.0)
            .on_click(self.base + ARCHIVE)
            .child(icon(IconKind::Archive).size(14.0).color(colors.icon_muted));
        div()
            .col()
            .w_px(width)
            .child(
                div()
                    .row()
                    .h_px(38.0)
                    .px(10.0)
                    .gap(8.0)
                    .items_center()
                    .bg(colors.toolbar_background)
                    .child(icon(self.role_icon).size(13.0).color(colors.icon_muted))
                    .child(label(self.role.clone()).size(13.0).color(colors.text))
                    .child(tag)
                    .child(context)
                    .child(div().flex(1.0))
                    .child(send)
                    .child(archive),
            )
            .child(div().h_px(1.0).bg(colors.border))
            .into()
    }

    fn click(&mut self, id: u64) -> bool {
        if id == self.base + SEND {
            self.action = Some(ConsoleAction::Send);
            return true;
        }
        if id == self.base + ARCHIVE {
            self.action = Some(ConsoleAction::Archive);
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
