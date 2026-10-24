use git::working_copy::Staging;
use git::ChangeStatus;
use ui::{div, icon, label, theme, Div, IconKind, Node, Rgba};

pub(crate) const ROW_H: f32 = 26.0;
/// The tab track's top margin plus its padded segments.
pub(crate) const TAB_BAR_H: f32 = 38.0;
pub(crate) const TOOLBAR_H: f32 = 38.0;
pub(crate) const INDENT: f32 = 16.0;
const BUTTON_H: f32 = 26.0;
const CHEVRON_W: f32 = 24.0;
const PRIMARY_H: f32 = 28.0;

pub(crate) fn checkbox(id: u64, staging: Staging, hot: bool) -> Node {
    let colors = theme();
    let mut square = div()
        .w_px(14.0)
        .h_px(14.0)
        .rounded(4.0)
        .items_center()
        .justify_center();
    square = match staging {
        Staging::Staged => square
            .bg(colors.accent_fill)
            .child(icon(IconKind::Check).size(10.0).color(colors.on_accent)),
        Staging::Partial => square
            .bg(colors.accent_fill)
            .child(div().w_px(7.0).h_px(2.0).rounded(1.0).bg(colors.on_accent)),
        // A clear fill of the ring's own color keeps the ring's inner edge from blending toward black.
        Staging::Unstaged if hot => square
            .bg(colors.text_accent.alpha(0.0))
            .border(1.5, colors.text_accent),
        Staging::Unstaged => square
            .bg(colors.border.alpha(0.0))
            .border(1.5, colors.border),
    };
    div()
        .w_px(14.0)
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
        .px(8.0)
        .gap(6.0)
        .rounded(4.0)
        .items_center()
        .on_click(id);
    if let Some(kind) = leading {
        button = button.child(icon(kind).size(13.0).color(colors.icon_muted));
    }
    button = button.child(label(text.to_string()).size(12.0).color(colors.text));
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
    /// The panel's main action: filled with the accent instead of sitting on the surface.
    pub primary: bool,
}

impl SplitButton<'_> {
    fn ink(&self) -> Rgba {
        let colors = theme();
        match (self.enabled, self.primary) {
            (false, _) => colors.text_placeholder,
            (true, true) => colors.on_accent,
            (true, false) => colors.text,
        }
    }

    /// The main half (for a tooltip) and the whole control, built by `finish`.
    pub fn main_half(&self) -> Div {
        let colors = theme();
        let mut main = div()
            .row()
            .h_px(if self.primary { PRIMARY_H } else { BUTTON_H })
            .px(if self.primary { 12.0 } else { 10.0 })
            .gap(6.0)
            .items_center();
        // A disabled button still takes the pointer, so its tooltip can say why.
        main = main.on_click(self.main);
        if self.enabled && self.hover == Some(self.main) {
            main = main.bg(if self.primary {
                colors.on_accent.alpha(0.12)
            } else {
                colors.ghost_element_hover
            });
        }
        let size = if self.primary { 13.0 } else { 12.0 };
        let mut text = label(self.text.to_string()).size(size).color(self.ink());
        if self.primary {
            text = text.weight(600);
        }
        main = main.child(text);
        if let Some(note) = self.note {
            main = main.child(label(note.to_string()).size(size).color(
                if self.primary && self.enabled {
                    colors.on_accent.alpha(0.85)
                } else {
                    colors.text_placeholder
                },
            ));
        }
        main
    }

    pub fn finish(&self, main: Div) -> Node {
        let colors = theme();
        let height = if self.primary { PRIMARY_H } else { BUTTON_H };
        let mut chevron = div()
            .row()
            .w_px(if self.primary {
                CHEVRON_W + 2.0
            } else {
                CHEVRON_W
            })
            .h_px(height)
            .items_center()
            .justify_center()
            .on_click(self.menu)
            .child(icon(IconKind::ChevronDown).size(12.0).color(
                match (self.enabled, self.primary) {
                    (true, true) => colors.on_accent,
                    _ => colors.icon_muted,
                },
            ));
        if self.hover == Some(self.menu) {
            chevron = chevron.bg(colors.ghost_element_hover);
        }
        let (fill, divider) = match (self.enabled, self.primary) {
            (true, true) => (colors.accent_fill, colors.on_accent.alpha(0.2)),
            (false, true) => (colors.element_hover, colors.border_variant),
            _ => (colors.element_background, colors.border_variant),
        };
        let mut control = div()
            .row()
            .rounded(if self.primary { 6.0 } else { 4.0 })
            .items_center()
            .bg(fill);
        if !self.primary {
            control = control.border(1.0, colors.border_variant);
        }
        control
            .child(main)
            .child(div().w_px(1.0).h_px(height).bg(divider))
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
