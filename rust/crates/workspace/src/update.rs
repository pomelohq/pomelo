//! The self-update button at the right of the title bar and the app menu's update item. The app maps the
//! updater's state into these; the window only draws them and reports clicks.

use ui::{div, icon, label, theme, IconKind, Node};

pub const UPDATE_BUTTON: u64 = 960;
pub const UPDATE_DISMISS: u64 = 961;

/// What an update control asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    Restart,
    Dismiss,
    /// Show why the update failed, and a way to try again.
    OpenDetails,
    ReleaseNotes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpdateTone {
    #[default]
    Normal,
    /// Waiting on the user: outlined in the accent color.
    Ready,
    Warning,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateButton {
    pub label: String,
    pub icon: IconKind,
    /// 0.0-1.0: a thin bar after the label.
    pub progress: Option<f32>,
    pub tooltip: Option<String>,
    pub tone: UpdateTone,
    pub dismissable: bool,
    /// None while the updater is busy: the button only explains itself on hover.
    pub click: Option<UpdateAction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateMenuItem {
    pub label: String,
    pub enabled: bool,
    pub action: UpdateAction,
}

/// What the window shows of the updater.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateInfo {
    pub button: Option<UpdateButton>,
    /// None where this build does not update itself.
    pub menu: Option<UpdateMenuItem>,
}

fn progress_bar(progress: f32) -> Node {
    let colors = theme();
    let fill = progress.clamp(0.0, 1.0);
    div()
        .row()
        .w_px(46.0)
        .h_px(3.0)
        .rounded(2.0)
        .bg(colors.border_variant)
        .child(
            div()
                .flex(fill.max(0.0001))
                .h_px(3.0)
                .rounded(2.0)
                .bg(colors.text_accent),
        )
        .child(div().flex((1.0 - fill).max(0.0001)))
        .into()
}

/// The title bar button; `hovered` is the hit id under the pointer.
pub fn button(update: &UpdateButton, hovered: Option<u64>) -> Node {
    let colors = theme();
    let hot = hovered == Some(UPDATE_BUTTON) && update.click.is_some();
    let (text, icon_color) = match update.tone {
        UpdateTone::Normal if hot => (colors.text, colors.text),
        UpdateTone::Normal => (colors.text_muted, colors.text_muted),
        UpdateTone::Ready => (colors.text, colors.text_accent),
        UpdateTone::Warning => (colors.warning, colors.warning),
    };
    let mut chip = div()
        .row()
        .items_center()
        .gap(6.0)
        .h_px(24.0)
        .px(8.0)
        .rounded(5.0)
        .on_click(UPDATE_BUTTON)
        .child(icon(update.icon).size(13.0).color(icon_color))
        .child(label(update.label.clone()).size(12.0).color(text));
    if let Some(progress) = update.progress {
        chip = chip.child(progress_bar(progress));
    }
    if update.dismissable {
        let mut dismiss =
            div()
                .w_px(16.0)
                .h_px(16.0)
                .rounded(3.0)
                .items_center()
                .justify_center()
                .on_click(UPDATE_DISMISS)
                .child(icon(IconKind::Close).size(10.0).color(
                    if hovered == Some(UPDATE_DISMISS) {
                        colors.text
                    } else {
                        colors.text_placeholder
                    },
                ));
        if hovered == Some(UPDATE_DISMISS) {
            dismiss = dismiss.bg(colors.element_selected);
        }
        chip = chip.child(dismiss);
    }
    match update.tone {
        UpdateTone::Ready => {
            chip = chip
                .bg(colors.text_accent.alpha(0.14))
                .border(1.0, colors.text_accent.alpha(0.45));
        }
        _ if hot => chip = chip.bg(colors.ghost_element_hover),
        _ => {}
    }
    chip.into()
}

/// The tooltip for the hovered part of the button, if it has one.
pub fn tooltip(update: &UpdateButton, hovered: u64) -> Option<&str> {
    match hovered {
        UPDATE_BUTTON => update.tooltip.as_deref(),
        UPDATE_DISMISS if update.dismissable => Some("Dismiss"),
        _ => None,
    }
}
