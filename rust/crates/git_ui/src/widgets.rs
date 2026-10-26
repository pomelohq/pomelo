use git::working_copy::Staging;
use git::ChangeStatus;
use ui::{div, icon, label, theme, Div, IconKind, LabelSize, Node, Rgba};

pub(crate) const ROW_H: f32 = 26.0;
/// The tab track's top margin plus its padded segments.
pub(crate) const TAB_BAR_H: f32 = 38.0;
pub(crate) const TOOLBAR_H: f32 = 38.0;
pub(crate) const INDENT: f32 = 16.0;

pub(crate) fn checkbox(id: u64, staging: Staging, hot: bool) -> Node {
    div()
        .row()
        .h_px(20.0)
        .items_center()
        .child(
            components::checkbox(id, staging == Staging::Staged, "")
                .partial(staging == Staging::Partial)
                .hovered(hot),
        )
        .into()
}

pub(crate) fn status_icon(status: ChangeStatus) -> Node {
    let colors = theme();
    let (kind, color) = match status {
        ChangeStatus::Conflicted => (IconKind::Warning, colors.warning),
        ChangeStatus::Deleted => (IconKind::SquareMinus, colors.version_control_deleted),
        ChangeStatus::Modified | ChangeStatus::Renamed => {
            (IconKind::SquareDot, colors.version_control_modified)
        }
        ChangeStatus::Added => (IconKind::SquarePlus, colors.version_control_added),
    };
    icon(kind).size(13.0).color(color).into()
}

pub(crate) fn diff_stat(added: Option<u32>, deleted: Option<u32>, size: LabelSize) -> Node {
    let colors = theme();
    let (Some(added), Some(deleted)) = (added, deleted) else {
        return label("binary").size(size).color(colors.text_muted).into();
    };
    let mut row = div().row().gap(6.0).items_center();
    if added > 0 {
        row = row.child(
            label(format!("+{added}"))
                .size(size)
                .mono()
                .color(colors.version_control_added),
        );
    }
    if deleted > 0 {
        row = row.child(
            label(format!("-{deleted}"))
                .size(size)
                .mono()
                .color(colors.version_control_deleted),
        );
    }
    row.into()
}

/// One vertical guide per tree level above `depth`, 16px apart like the Files panel.
/// A bubble under (or over) the hovered control, kept inside the panel.
pub(crate) fn icon_button(id: u64, kind: IconKind, hot: bool, enabled: bool) -> Div {
    let colors = theme();
    let mut button = div()
        .w_px(22.0)
        .h_px(22.0)
        .rounded(4.0)
        .items_center()
        .justify_center()
        .on_click(id)
        .child(icon(kind).size(14.0).color(if enabled {
            colors.icon_muted
        } else {
            colors.text_disabled
        }));
    if hot && enabled {
        button = button.bg(colors.ghost_element_hover);
    }
    button
}

pub(crate) fn ghost_button(id: u64, leading: Option<IconKind>, text: &str, hot: bool) -> Div {
    components::button(id, text)
        .kind(components::ButtonKind::Ghost)
        .icon(leading)
        .hovered(hot.then_some(id))
        .render()
}

pub(crate) fn arrow_count(kind: IconKind, count: usize, color: Rgba) -> Node {
    div()
        .row()
        .gap(1.0)
        .items_center()
        .child(icon(kind).size(10.0).color(color))
        .child(label(count.to_string()).size(LabelSize::Mini).color(color))
        .into()
}

pub(crate) fn text_width(text: &str, size: f32, mono: bool) -> f32 {
    ui::measure_text_width(text, size, mono, ui::ui_font_weight()) / ui::ui_text_scale()
}
