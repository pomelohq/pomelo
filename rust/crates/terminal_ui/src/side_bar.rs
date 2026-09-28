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
        let send = |label_too: bool| {
            let mut send = div()
                .row()
                .h_px(26.0)
                .px(if label_too { 10.0 } else { 7.0 })
                .gap(6.0)
                .items_center()
                .rounded(5.0)
                .border(1.0, colors.border_variant)
                .on_click(self.base + SEND)
                .child(icon(IconKind::Return).size(12.0).color(colors.icon_muted));
            if label_too {
                send = send.child(label("Send to main").size(12.5).color(colors.text));
            }
            send
        };
        let archive = div()
            .row()
            .w_px(26.0)
            .h_px(26.0)
            .items_center()
            .justify_center()
            .rounded(4.0)
            .on_click(self.base + ARCHIVE)
            .child(icon(IconKind::Archive).size(14.0).color(colors.icon_muted));
        // A narrow dock drops the least needed first: the send button's label, then what it started with,
        // then the access tag. Send and Archive always stay.
        let row = |tag: Option<&ui::Div>, context: Option<&ui::Div>, send_label: bool| {
            let mut row = div()
                .row()
                .h_px(38.0)
                .px(10.0)
                .gap(8.0)
                .items_center()
                .bg(colors.toolbar_background)
                .child(icon(self.role_icon).size(13.0).color(colors.icon_muted))
                .child(label(self.role.clone()).size(13.0).color(colors.text));
            if let Some(tag) = tag {
                row = row.child(tag.clone());
            }
            if let Some(context) = context {
                row = row.child(context.clone());
            }
            row.child(div().flex(1.0))
                .child(send(send_label))
                .child(archive.clone())
        };
        let fits = |row: &ui::Div| ui::measure(&row.clone().into()).0 <= width;
        let bar = [
            (true, true, true),
            (true, true, false),
            (true, false, false),
            (false, false, false),
        ]
        .into_iter()
        .map(|(tag_on, context_on, send_label)| {
            row(
                tag_on.then_some(&tag),
                context_on.then_some(&context),
                send_label,
            )
        })
        .find(fits)
        .unwrap_or_else(|| row(None, None, false));
        div()
            .col()
            .w_px(width)
            .child(bar)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> SideAgentBar {
        SideAgentBar::new(
            0,
            IconKind::Wrench,
            "Fix",
            false,
            "Auto: fork + compact - 59k".into(),
            "main".into(),
            Box::new(|| None),
        )
    }

    fn texts(width: f32) -> Vec<String> {
        let node = bar().render(width, 0);
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, width, 40.0, ui::Rgba::TRANSPARENT),
        );
        painted.texts.iter().map(|text| text.text.clone()).collect()
    }

    #[test]
    fn a_narrow_bar_drops_labels_but_keeps_send_and_archive_inside() {
        let wide = texts(900.0);
        assert!(wide.iter().any(|t| t == "Send to main") && wide.iter().any(|t| t.contains("59k")));
        let narrow = texts(330.0);
        assert!(!narrow.iter().any(|t| t == "Send to main"), "{narrow:?}");
        let width = 330.0;
        let node = bar().render(width, 0);
        let painted = ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, width, 40.0, ui::Rgba::TRANSPARENT),
        );
        let archive = painted
            .hits
            .iter()
            .find(|(_, id)| *id == IDS + ARCHIVE)
            .expect("archive");
        assert!(
            archive.0.x + archive.0.w <= width,
            "archive ends at {}",
            archive.0.x + archive.0.w
        );
    }
}
