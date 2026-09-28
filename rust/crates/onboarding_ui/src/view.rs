//! Drawing pieces of the onboarding page that carry no state of their own.

use ui::{div, icon, label, theme, IconKind, Node, Rgba};

pub const CONTENT_MAX_W: f32 = 780.0;
pub const PAGE_PAD_X: f32 = 40.0;
pub const PAGE_PAD_TOP: f32 = 44.0;
pub const PAGE_PAD_BOTTOM: f32 = 60.0;
pub const PAGE_GAP: f32 = 22.0;
pub const BUTTON_H: f32 = 28.0;
pub const TEXT: f32 = 13.0;
pub const SMALL: f32 = 12.5;
pub const HINT: f32 = 12.0;

pub fn agent_tint() -> Rgba {
    theme().terminal_ansi[5]
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Primary,
    Agent,
    Ghost,
}

/// A page button; `keys` is the shortcut shown after its label.
pub fn button(
    id: u64,
    kind: Option<IconKind>,
    text: &str,
    tone: Tone,
    hot: bool,
    enabled: bool,
    keys: Option<&str>,
) -> Node {
    let colors = theme();
    let (bg, border) = match tone {
        Tone::Plain => (colors.element_background, colors.border),
        Tone::Primary => (colors.info_background, colors.info_border),
        Tone::Agent => (agent_tint().alpha(0.12), agent_tint().alpha(0.55)),
        Tone::Ghost => (Rgba::TRANSPARENT, Rgba::TRANSPARENT),
    };
    let bg = if hot && enabled {
        match tone {
            Tone::Primary => colors.info_border.alpha(0.6),
            _ => colors.element_hover,
        }
    } else {
        bg
    };
    let text_color = match (enabled, tone, hot) {
        (false, _, _) => colors.text_disabled,
        (true, Tone::Ghost, false) => colors.text_muted,
        _ => colors.text,
    };
    let mut row = div()
        .row()
        .h_px(BUTTON_H)
        .px(12.0)
        .gap(6.0)
        .items_center()
        .rounded(5.0)
        .bg(bg)
        .border(1.0, border);
    if enabled {
        row = row.on_click(id);
    }
    if let Some(kind) = kind {
        let tint = match tone {
            Tone::Agent => agent_tint(),
            _ => text_color,
        };
        row = row.child(icon(kind).size(13.0).color(tint));
    }
    row = row.child(label(text.to_string()).size(TEXT).color(text_color));
    if let Some(keys) = keys {
        row = row.child(
            label(keys.to_string())
                .size(11.0)
                .mono()
                .color(colors.text_placeholder),
        );
    }
    row.into()
}

/// The page's top: the logo, a title with a line under it, and the buttons at the right.
pub fn header(title: &str, sub: &str, right: Vec<Node>) -> Node {
    let colors = theme();
    let logo = div()
        .row()
        .w_px(40.0)
        .h_px(40.0)
        .rounded(10.0)
        .items_center()
        .justify_center()
        .bg(Rgba::hex("#a63d9e"))
        .child(
            label("P")
                .size(18.0)
                .weight(600)
                .color(Rgba::new(1.0, 1.0, 1.0, 1.0)),
        );
    let text = div()
        .col()
        .flex(1.0)
        .gap(2.0)
        .child(
            label(title.to_string())
                .size(20.0)
                .weight(600)
                .color(colors.text)
                .truncate(),
        )
        .child(
            label(sub.to_string())
                .size(SMALL)
                .italic()
                .color(colors.text_muted)
                .truncate(),
        );
    let mut row = div().row().items_center().gap(16.0).child(logo).child(text);
    let mut buttons = div().row().items_center().gap(8.0);
    for node in right {
        buttons = buttons.child(node);
    }
    row = row.child(buttons);
    div().col().gap(PAGE_GAP).child(row).child(divider()).into()
}

pub fn divider() -> Node {
    div().h_px(1.0).bg(theme().border_variant).into()
}

/// An uppercase section title, with `trailing` at its right.
pub fn section_title(title: &str, trailing: Option<Node>) -> Node {
    let mut row = div().row().items_center().child(
        div().row().flex(1.0).child(
            label(title.to_uppercase())
                .size(11.0)
                .weight(600)
                .color(theme().text_placeholder),
        ),
    );
    if let Some(trailing) = trailing {
        row = row.child(trailing);
    }
    row.into()
}

pub fn section(title: &str, trailing: Option<Node>, body: Vec<Node>) -> Node {
    let mut column = div().col().gap(10.0).child(section_title(title, trailing));
    for node in body {
        column = column.child(node);
    }
    column.into()
}

/// Rows in a rounded box, a line between each.
pub fn rows(items: Vec<Node>) -> Node {
    let colors = theme();
    let mut column = div().col().rounded(8.0).border(1.0, colors.border_variant);
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            column = column.child(div().h_px(1.0).bg(colors.border_variant));
        }
        column = column.child(item);
    }
    column.into()
}

/// One row's frame: padding and a minimum height.
pub fn row() -> ui::Div {
    div().row().items_center().gap(10.0).px(12.0).py(9.0)
}

pub fn chip(text: &str, detected: bool) -> Node {
    let colors = theme();
    div()
        .row()
        .h_px(20.0)
        .px(7.0)
        .items_center()
        .rounded(4.0)
        .border(1.0, colors.border_variant)
        .child(label(text.to_string()).size(11.5).color(if detected {
            colors.text
        } else {
            colors.text_muted
        }))
        .into()
}

pub fn dot(color: Rgba, size: f32) -> Node {
    div()
        .w_px(size)
        .h_px(size)
        .rounded(size / 2.0)
        .bg(color)
        .into()
}

/// Lays `nodes` out left to right, starting a new line when the next would pass `width`.
pub fn pack(nodes: Vec<Node>, width: f32, gap: f32) -> Node {
    let mut column = div().col().gap(gap);
    let mut line = div().row().gap(gap).items_center();
    let mut used = 0.0;
    for node in nodes {
        let node_width = ui::measure(&node).0;
        if used > 0.0 && used + gap + node_width > width {
            column = column.child(line);
            line = div().row().gap(gap).items_center();
            used = 0.0;
        }
        used += if used > 0.0 {
            gap + node_width
        } else {
            node_width
        };
        line = line.child(node);
    }
    if used > 0.0 {
        column = column.child(line);
    }
    column.into()
}

/// A getting-started card: a title with its icon and a short description.
pub fn card(id: u64, kind: IconKind, title: &str, text: &str, width: f32, hot: bool) -> Node {
    let colors = theme();
    div()
        .col()
        .w_px(width)
        .p(14.0)
        .gap(6.0)
        .rounded(8.0)
        .border(
            1.0,
            if hot {
                colors.border
            } else {
                colors.border_variant
            },
        )
        .bg(if hot {
            colors.element_hover
        } else {
            Rgba::new(0.0, 0.0, 0.0, 0.08)
        })
        .on_click(id)
        .child(
            div()
                .row()
                .items_center()
                .gap(8.0)
                .child(icon(kind).size(14.0).color(colors.icon_muted))
                .child(label(title.to_string()).size(14.0).color(colors.text)),
        )
        .child(
            label(text.to_string())
                .size(SMALL)
                .color(colors.text_muted)
                .wrap(width - 28.0),
        )
        .into()
}

pub fn hint(text: &str) -> ui::Label {
    label(text.to_string())
        .size(HINT)
        .color(theme().text_placeholder)
}

pub fn progress_bar(value: f32, width: Option<f32>) -> Node {
    let colors = theme();
    let track = div().h_px(4.0).rounded(2.0).bg(colors.border_variant);
    let track = match width {
        Some(width) => track.w_px(width),
        None => track.flex(1.0),
    };
    let fill = value.clamp(0.0, 1.0);
    track
        .row()
        .child(
            div()
                .flex(fill.max(0.0001))
                .h_px(4.0)
                .rounded(2.0)
                .bg(colors.text_accent),
        )
        .child(div().flex((1.0 - fill).max(0.0001)).h_px(4.0))
        .into()
}

/// A 16px checkbox before a title and its explanation.
pub fn check(id: u64, on: bool, title: &str, detail: Option<Node>) -> Node {
    let colors = theme();
    let mut box_ = div()
        .row()
        .w_px(16.0)
        .h_px(16.0)
        .rounded(3.0)
        .items_center()
        .justify_center()
        .border(1.0, colors.border);
    if on {
        box_ = box_.child(icon(IconKind::Check).size(11.0).color(colors.text_accent));
    }
    let mut text = div()
        .col()
        .flex(1.0)
        .gap(2.0)
        .child(label(title.to_string()).size(TEXT).color(colors.text));
    if let Some(detail) = detail {
        text = text.child(detail);
    }
    div()
        .row()
        .gap(9.0)
        .py(4.0)
        .child(div().col().pt(1.0).child(box_).on_click(id))
        .child(text)
        .on_click(id)
        .into()
}

/// A round radio mark, filled when chosen.
pub fn radio(on: bool) -> Node {
    let colors = theme();
    let mut mark = div()
        .row()
        .w_px(16.0)
        .h_px(16.0)
        .rounded(8.0)
        .items_center()
        .justify_center()
        .border(
            1.5,
            if on {
                colors.text_accent
            } else {
                colors.border
            },
        );
    if on {
        mark = mark.child(dot(colors.text_accent, 8.0));
    }
    mark.into()
}

/// A small numbered or checked circle, as the stepper and the phase timeline use.
pub fn step_mark(content: StepMark, size: f32) -> Node {
    let colors = theme();
    let (border, color) = match content {
        StepMark::Done => (colors.success.alpha(0.6), colors.success),
        StepMark::Current(_) | StepMark::Working => (colors.text_accent, colors.text_accent),
        StepMark::Failed => (colors.error, colors.error),
        StepMark::Todo(_) => (colors.border, colors.text_placeholder),
    };
    let mut mark = div()
        .row()
        .w_px(size)
        .h_px(size)
        .rounded(size / 2.0)
        .items_center()
        .justify_center()
        .border(1.0, border)
        .bg(colors.editor_background);
    if content == StepMark::Done {
        mark = mark.bg(colors.success.alpha(0.15));
    }
    mark = match content {
        StepMark::Done => mark.child(icon(IconKind::Check).size(11.0).color(color)),
        StepMark::Working => mark.child(icon(IconKind::RotateCw).size(11.0).color(color)),
        StepMark::Failed => mark.child(label("!").size(11.0).weight(600).color(color)),
        StepMark::Current(n) | StepMark::Todo(n) => {
            mark.child(label(n.to_string()).size(11.0).color(color))
        }
    };
    mark.into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StepMark {
    Done,
    Current(usize),
    Working,
    Failed,
    Todo(usize),
}

/// A segmented control; `options` are (id, text, enabled).
pub fn segmented(options: &[(u64, &str, bool)], selected: usize) -> Node {
    let colors = theme();
    let mut row = div()
        .row()
        .p(2.0)
        .gap(2.0)
        .rounded(5.0)
        .bg(colors.element_background);
    for (index, (id, text, enabled)) in options.iter().enumerate() {
        let on = index == selected;
        let color = if !enabled {
            colors.text_muted.alpha(0.45)
        } else if on {
            colors.text
        } else {
            colors.text_muted
        };
        let mut part = div()
            .row()
            .items_center()
            .px(10.0)
            .py(3.0)
            .rounded(4.0)
            .on_click(*id)
            .child(label(text.to_string()).size(SMALL).color(color));
        if on {
            part = part.bg(colors.element_selected);
        }
        row = row.child(part);
    }
    row.into()
}

/// A text field in a bordered box.
pub fn input(id: u64, content: Node, focused: bool, width: Option<f32>, height: f32) -> Node {
    let colors = theme();
    let frame = div()
        .row()
        .items_center()
        .h_px(height)
        .px(9.0)
        .rounded(5.0)
        .bg(colors.editor_background)
        .border(
            1.0,
            if focused {
                colors.border_focused
            } else {
                colors.border_variant
            },
        )
        .on_click(id)
        .child(content);
    match width {
        Some(width) => frame.w_px(width).into(),
        None => frame.flex(1.0).into(),
    }
}

pub fn seconds_text(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        _ => format!("{}m {}s", seconds / 60, seconds % 60),
    }
}
