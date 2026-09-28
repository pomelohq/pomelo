use git::working_copy::Staging;
use git::ChangeStatus;
use ui::{deferred, div, icon, label, theme, Corner, Div, IconKind, Node, Rgba};

pub(crate) const ROW_H: f32 = 28.0;
pub(crate) const TAB_BAR_H: f32 = 32.0;
pub(crate) const TOOLBAR_H: f32 = 32.0;
pub(crate) const INDENT: f32 = 16.0;
const BUTTON_H: f32 = 22.0;
const CHEVRON_W: f32 = 20.0;
const TOOLTIP_H: f32 = 24.0;

pub(crate) fn checkbox(id: u64, staging: Staging, hot: bool) -> Node {
    let colors = theme();
    let mut square = div()
        .w_px(16.0)
        .h_px(16.0)
        .rounded(2.0)
        .items_center()
        .justify_center()
        .border(1.0, colors.border);
    if hot {
        square = square.bg(colors.ghost_element_hover);
    }
    square = match staging {
        Staging::Staged => square.child(icon(IconKind::Check).size(12.0).color(colors.text_accent)),
        Staging::Partial => square.child(icon(IconKind::Dash).size(12.0).color(colors.text_accent)),
        Staging::Unstaged => square,
    };
    div()
        .w_px(20.0)
        .h_px(20.0)
        .items_center()
        .justify_center()
        .on_click(id)
        .child(square)
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
    icon(kind).size(14.0).color(color).into()
}

pub(crate) fn diff_stat(added: Option<u32>, deleted: Option<u32>, size: f32) -> Node {
    let colors = theme();
    let (Some(added), Some(deleted)) = (added, deleted) else {
        return label("binary").size(size).color(colors.text_muted).into();
    };
    let mut row = div().row().gap(4.0).items_center();
    if added > 0 {
        row = row.child(
            label(format!("+{added}"))
                .size(size)
                .color(colors.version_control_added),
        );
    }
    if deleted > 0 {
        row = row.child(
            label(format!("-{deleted}"))
                .size(size)
                .color(colors.version_control_deleted),
        );
    }
    row.into()
}

/// One vertical guide per tree level above `depth`, 16px apart like the Files panel.
pub(crate) fn with_guides(mut row: Div, depth: usize) -> Div {
    for level in 0..depth {
        row = row.pin_left_edge(
            15.0 + level as f32 * INDENT,
            0.0,
            1.0,
            div().bg(theme().panel_indent_guide),
        );
    }
    row
}

pub(crate) fn chevron(open: bool) -> Node {
    icon(if open {
        IconKind::ChevronDown
    } else {
        IconKind::ChevronRight
    })
    .size(12.0)
    .color(theme().icon_muted)
    .into()
}

/// A bubble under (or over) the hovered control, kept inside the panel.
pub(crate) fn tooltip(target: Div, text: &str, above: bool) -> Div {
    let width = ui::measure_text_width(text, 12.0, false, ui::ui_font_weight())
        / ui::ui_text_scale()
        + 16.0;
    let bubble = div()
        .row()
        .items_center()
        .px(8.0)
        .h_px(TOOLTIP_H)
        .rounded(6.0)
        .bg(theme().elevated_surface_background)
        .border(1.0, theme().border)
        .child(label(text.to_string()).size(12.0).color(theme().text));
    let (corner, dy) = if above {
        (Corner::TopLeft, -(TOOLTIP_H + 4.0))
    } else {
        (Corner::BottomLeft, TOOLTIP_H + 4.0)
    };
    target.pin(
        corner,
        0.0,
        dy,
        width,
        TOOLTIP_H,
        deferred(bubble).priority(5).snap_to_window(),
    )
}

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
    let colors = theme();
    let mut button = div()
        .row()
        .h_px(BUTTON_H)
        .px(6.0)
        .gap(4.0)
        .rounded(4.0)
        .items_center()
        .on_click(id);
    if let Some(kind) = leading {
        button = button.child(icon(kind).size(14.0).color(colors.icon_muted));
    }
    button = button.child(label(text.to_string()).size(12.0).color(if hot {
        colors.text
    } else {
        colors.text_muted
    }));
    if hot {
        button = button.bg(colors.ghost_element_hover);
    }
    button
}

/// The main action and a chevron that opens its menu, as one bordered control.
pub(crate) struct SplitButton<'a> {
    pub main: u64,
    pub menu: u64,
    pub text: &'a str,
    pub note: Option<&'a str>,
    pub enabled: bool,
    pub hover: Option<u64>,
}

impl SplitButton<'_> {
    /// The main half (for a tooltip) and the whole control, built by `finish`.
    pub fn main_half(&self) -> Div {
        let colors = theme();
        let mut main = div().row().h_px(BUTTON_H).px(7.0).gap(3.0).items_center();
        // A disabled button still takes the pointer, so its tooltip can say why.
        main = main.on_click(self.main);
        if self.enabled && self.hover == Some(self.main) {
            main = main.bg(colors.ghost_element_hover);
        }
        main = main.child(
            label(self.text.to_string())
                .size(12.0)
                .color(if self.enabled {
                    colors.text
                } else {
                    colors.text_placeholder
                }),
        );
        if let Some(note) = self.note {
            main = main.child(
                label(note.to_string())
                    .size(11.0)
                    .color(colors.text_placeholder),
            );
        }
        main
    }

    pub fn finish(&self, main: Div) -> Node {
        let colors = theme();
        let mut chevron = div()
            .row()
            .w_px(CHEVRON_W)
            .h_px(BUTTON_H)
            .items_center()
            .justify_center()
            .on_click(self.menu)
            .child(
                icon(IconKind::ChevronDown)
                    .size(10.0)
                    .color(colors.icon_muted),
            );
        if self.hover == Some(self.menu) {
            chevron = chevron.bg(colors.ghost_element_hover);
        }
        div()
            .row()
            .rounded(4.0)
            .items_center()
            .bg(colors.element_background)
            .border(1.0, colors.border_variant)
            .child(main)
            .child(div().w_px(1.0).h_px(BUTTON_H).bg(colors.border_variant))
            .child(chevron)
            .into()
    }
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
        ChipTone::Push => colors.text_accent.alpha(0.12),
        ChipTone::Pull => colors.warning.alpha(0.12),
        ChipTone::Quiet => Rgba::TRANSPARENT,
    };
    let mut chip = div()
        .row()
        .h_px(20.0)
        .px(7.0)
        .gap(3.0)
        .rounded(4.0)
        .items_center()
        .on_click(id)
        .bg(if hot { fill.alpha(fill.a + 0.08) } else { fill });
    if tone == ChipTone::Quiet {
        chip = chip.border(1.0, colors.border_variant);
    }
    for part in parts {
        chip = chip.child(part);
    }
    chip.into()
}

pub(crate) fn chip_text(text: String, tone: ChipTone) -> Node {
    label(text).size(11.5).color(tone_color(tone)).into()
}

pub(crate) fn chip_icon(kind: IconKind, tone: ChipTone) -> Node {
    icon(kind).size(11.0).color(tone_color(tone)).into()
}

fn tone_color(tone: ChipTone) -> Rgba {
    let colors = theme();
    match tone {
        ChipTone::Push => colors.text_accent,
        ChipTone::Pull => colors.warning,
        ChipTone::Quiet => colors.text_placeholder,
    }
}

/// A round avatar with the author's initial, tinted from the name so each author keeps a color.
pub(crate) fn avatar(author: &str) -> Node {
    let colors = theme();
    let palette = [
        colors.text_accent,
        colors.success,
        colors.warning,
        colors.version_control_deleted,
        colors.info,
    ];
    let hash = author.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as usize)
    });
    let tint = palette
        .get(hash % palette.len())
        .copied()
        .unwrap_or(colors.text_accent);
    let initial: String = author
        .chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default();
    div()
        .w_px(14.0)
        .h_px(14.0)
        .rounded(7.0)
        .items_center()
        .justify_center()
        .bg(tint)
        .child(label(initial).size(8.0).color(colors.background))
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
