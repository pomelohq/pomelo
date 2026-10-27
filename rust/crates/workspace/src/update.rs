//! The self-update button at the right of the title bar and the app menu's update item. The app maps the
//! updater's state into these; the window only draws them and reports clicks.

use ui::{div, theme, IconKind, Node};

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

/// The title bar button; `hovered` is the hit id under the pointer.
pub fn button(update: &UpdateButton, hovered: Option<u64>) -> Node {
    let colors = theme();
    let hot = hovered == Some(UPDATE_BUTTON) && update.click.is_some();
    let mut button = components::button(UPDATE_BUTTON, update.label.clone())
        .icon(Some(update.icon))
        .hovered(hot.then_some(UPDATE_BUTTON));
    button = match update.tone {
        UpdateTone::Normal => button.kind(components::ButtonKind::Ghost),
        UpdateTone::Ready => button.kind(components::ButtonKind::Primary),
        UpdateTone::Warning => button
            .kind(components::ButtonKind::Ghost)
            .color(colors.warning),
    };
    let mut chip = button.render();
    if let Some(progress) = update.progress {
        chip = chip.child(
            div()
                .row()
                .w_px(46.0)
                .child(components::progress_bar(progress).thin()),
        );
    }
    if update.dismissable {
        let mut dismiss = components::icon_button(UPDATE_DISMISS, IconKind::Close)
            .size(components::ButtonSize::XSmall)
            .hovered(hovered);
        if update.tone == UpdateTone::Ready {
            dismiss = dismiss.color(colors.on_accent);
        }
        chip = chip.child(dismiss);
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
