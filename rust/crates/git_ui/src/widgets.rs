use git::working_copy::Staging;
use git::ChangeStatus;
use ui::{div, icon, label, theme, Div, IconKind, Node, Rgba};

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

pub(crate) fn diff_stat(added: Option<u32>, deleted: Option<u32>, size: f32) -> Node {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChipTone {
    Push,
    Pull,
    Quiet,
}

pub(crate) fn chip(id: u64, tone: ChipTone, parts: Vec<Node>, hot: bool) -> Node {
    let colors = theme();
    let fill = match tone {
        ChipTone::Push | ChipTone::Pull => colors.accent_fill,
        ChipTone::Quiet => colors.element_hover,
    };
    let mut chip = div()
        .row()
        .h_px(24.0)
        .px(10.0)
        .gap(5.0)
        .rounded(12.0)
        .items_center()
        .on_click(id)
        .bg(fill);
    if hot {
        chip = chip.bg(match tone {
            ChipTone::Quiet => colors.element_active,
            _ => colors.accent_fill.alpha(0.85),
        });
    }
    for part in parts {
        chip = chip.child(part);
    }
    chip.into()
}

pub(crate) fn chip_text(text: String, tone: ChipTone) -> Node {
    let mut text = label(text).size(12.0).color(tone_color(tone));
    text = if tone == ChipTone::Quiet {
        text.medium()
    } else {
        text.weight(600)
    };
    text.into()
}

/// A count inside a chip, in the code font so the digits line up.
pub(crate) fn chip_count(arrow: IconKind, count: u32, tone: ChipTone) -> Node {
    div()
        .row()
        .gap(1.0)
        .items_center()
        .child(chip_icon(arrow, tone))
        .child(
            label(count.to_string())
                .size(11.0)
                .mono()
                .color(tone_color(tone)),
        )
        .into()
}

pub(crate) fn chip_icon(kind: IconKind, tone: ChipTone) -> Node {
    icon(kind).size(11.0).color(tone_color(tone)).into()
}

fn tone_color(tone: ChipTone) -> Rgba {
    let colors = theme();
    match tone {
        ChipTone::Push | ChipTone::Pull => colors.on_accent,
        ChipTone::Quiet => colors.text_muted,
    }
}

pub(crate) fn avatar(author: &str) -> Node {
    let colors = theme();
    let initial: String = author
        .chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default();
    div()
        .w_px(18.0)
        .h_px(18.0)
        .rounded(9.0)
        .items_center()
        .justify_center()
        .bg(colors.element_hover)
        .child(
            label(initial)
                .size(10.0)
                .weight(600)
                .color(colors.text_muted),
        )
        .into()
}

pub(crate) fn arrow_count(kind: IconKind, count: usize, color: Rgba) -> Node {
    div()
        .row()
        .gap(1.0)
        .items_center()
        .child(icon(kind).size(10.0).color(color))
        .child(label(count.to_string()).size(10.5).color(color))
        .into()
}

pub(crate) fn text_width(text: &str, size: f32, mono: bool) -> f32 {
    ui::measure_text_width(text, size, mono, ui::ui_font_weight()) / ui::ui_text_scale()
}
