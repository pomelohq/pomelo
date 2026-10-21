//! Drawing pieces of the onboarding page that carry no state of their own.

use ui::{div, icon, label, material_icon, theme, IconKind, MaterialIcon, Node, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::{text_line, ButtonKind};

pub const COLUMN_W: f32 = 640.0;
pub const WIDE_W: f32 = 760.0;
pub const PAGE_PAD_X: f32 = 24.0;
pub const PAGE_PAD_TOP: f32 = 36.0;
pub const PAGE_PAD_BOTTOM: f32 = 20.0;
pub const PAGE_GAP: f32 = 16.0;
pub const FOOTER_H: f32 = 52.0;
const INPUT_H: f32 = 30.0;

/// The wizard's place along Repositories, Setup, Review; `done` marks every step finished.
pub fn steps(titles: &[&str], current: usize, done: bool) -> Node {
    let colors = theme();
    let mut row = div().row().items_center().gap(18.0);
    for (index, title) in titles.iter().enumerate() {
        let finished = done || index < current;
        let active = !done && index == current;
        let mut mark = div()
            .row()
            .w_px(20.0)
            .h_px(20.0)
            .rounded(10.0)
            .items_center()
            .justify_center()
            .bg(if active {
                colors.accent_fill
            } else {
                colors.element_background
            });
        mark = if finished {
            mark.child(icon(IconKind::Check).size(10.0).color(colors.success))
        } else {
            mark.child(label((index + 1).to_string()).size(12.0).color(if active {
                colors.on_accent
            } else {
                colors.text_placeholder
            }))
        };
        let ink = if active {
            colors.text
        } else if finished {
            colors.text_muted
        } else {
            colors.text_placeholder
        };
        row = row.child(
            div()
                .row()
                .items_center()
                .gap(8.0)
                .on_click(crate::ids::STEP_BASE + index as u64)
                .child(mark)
                .child(label(title.to_string()).size(12.0).color(ink)),
        );
    }
    row.into()
}

/// A page title with the muted line under it.
pub fn head(title: &str, sub: Option<&str>) -> Node {
    let colors = theme();
    let mut column = div().col().gap(4.0).child(text_line(
        28.0,
        label(title.to_string())
            .size(20.0)
            .weight(600)
            .color(colors.text)
            .truncate(),
    ));
    if let Some(sub) = sub {
        column = column.child(text_line(
            20.0,
            label(sub.to_string())
                .size(13.0)
                .color(colors.text_placeholder)
                .truncate(),
        ));
    }
    column.into()
}

/// Rows on the panel layer, 2px apart inside a 4px inset.
pub fn panel_list(items: Vec<Node>, inset: f32) -> Node {
    let mut column = div()
        .col()
        .gap(2.0)
        .p(inset)
        .rounded(6.0)
        .bg(theme().panel_background);
    for item in items {
        column = column.child(item);
    }
    column.into()
}

/// A card on the panel layer; `chosen` rings it in the accent.
pub fn card_frame(chosen: bool) -> ui::Div {
    let colors = theme();
    let frame = div().rounded(10.0).bg(colors.panel_background);
    if chosen {
        frame.border(1.0, colors.text_accent)
    } else {
        frame
    }
}

pub fn chip(text: &str, on: bool) -> Node {
    let colors = theme();
    div()
        .row()
        .h_px(20.0)
        .px(8.0)
        .items_center()
        .rounded(4.0)
        .bg(colors.element_active)
        .child(label(text.to_string()).size(12.0).color(if on {
            colors.text_muted
        } else {
            colors.text_placeholder
        }))
        .into()
}

/// A round status badge: a dot in the tone's color, then the word.
pub fn pill(text: &str, tone: Rgba) -> Node {
    div()
        .row()
        .h_px(20.0)
        .px(7.0)
        .gap(5.0)
        .items_center()
        .rounded(10.0)
        .bg(theme().ghost_element_hover)
        .child(dot(tone, 6.0))
        .child(label(text.to_string()).size(11.0).medium().color(tone))
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

/// A 14px radio mark: a hairline ring, or a thick accent ring when chosen.
pub fn radio(on: bool) -> Node {
    let colors = theme();
    let mark = div().row().w_px(14.0).h_px(14.0).rounded(7.0);
    if on {
        mark.items_center()
            .justify_center()
            .bg(colors.accent_fill)
            .child(dot(colors.panel_background, 6.0))
            .into()
    } else {
        mark.border(1.0, colors.border).into()
    }
}

pub fn muted(text: &str) -> ui::Label {
    label(text.to_string())
        .size(12.0)
        .color(theme().text_placeholder)
}

/// Small muted copy broken into lines that fit `width`.
pub fn note_paragraph(text: &str, width: f32) -> Node {
    workspace::paragraph(text, 12.0, 16.0, width, theme().text_placeholder)
}

/// Mutually exclusive choices on a sunken track; the chosen one sits raised, unavailable ones are dimmed.
pub fn segmented(options: &[(u64, &str, bool)], selected: usize) -> Node {
    let colors = theme();
    let mut group = div()
        .row()
        .p(2.0)
        .gap(2.0)
        .rounded(6.0)
        .bg(colors.background);
    for (index, (id, text, enabled)) in options.iter().enumerate() {
        let chosen = index == selected;
        let ink = if !enabled {
            colors.text_disabled
        } else if chosen {
            colors.text
        } else {
            colors.text_muted
        };
        let mut cell = div()
            .row()
            .h_px(26.0)
            .px(10.0)
            .items_center()
            .rounded(4.0)
            .on_click(*id);
        if chosen {
            cell = cell.bg(colors.element_background);
        }
        group = group.child(cell.child(label(text.to_string()).size(12.0).color(ink)));
    }
    div().row().child(group).into()
}

/// A one-line input box around `field`, on the surface layer, ringed while focused.
pub fn input(
    id: u64,
    field: &TextField,
    placeholder: &str,
    focused: bool,
    error: bool,
    font: FieldFont,
) -> ui::Div {
    let colors = theme();
    let border = match (error, focused) {
        (true, _) => colors.error,
        (false, true) => colors.border_focused,
        (false, false) => colors.border,
    };
    let mut frame = div()
        .row()
        .items_center()
        .h_px(INPUT_H)
        .px(10.0)
        .rounded(6.0)
        .bg(colors.editor_background)
        .border(1.0, border)
        .on_click(id);
    if focused && !error {
        frame = frame.ring(3.0, colors.overlay_selected);
    }
    frame.child(div().row().flex(1.0).items_center().child(field.render(
        placeholder,
        focused,
        colors.text,
        20.0,
        font,
    )))
}

/// The footer bar: an optional muted note or quiet actions at the left, the buttons right-aligned.
pub fn footer(note: Option<&str>, leading: Vec<Node>, buttons: Vec<Node>) -> Node {
    let mut start = div().row().flex(1.0).items_center().gap(8.0);
    if let Some(note) = note {
        start = start.child(muted(note).truncate());
    }
    for node in leading {
        start = start.child(node);
    }
    let mut row = div()
        .row()
        .h_px(FOOTER_H)
        .px(16.0)
        .gap(8.0)
        .items_center()
        .bg(theme().panel_background)
        .child(start);
    for button in buttons {
        row = row.child(button);
    }
    row.into()
}

pub fn button(id: u64, text: &str, keys: Option<&str>, kind: ButtonKind, enabled: bool) -> Node {
    workspace::modal_button(id, text, keys, kind, enabled)
}

/// A small icon-only button with no fill until hovered.
pub fn icon_button(id: u64, kind: IconKind, hot: bool) -> Node {
    let colors = theme();
    div()
        .row()
        .w_px(28.0)
        .h_px(24.0)
        .rounded(6.0)
        .items_center()
        .justify_center()
        .bg(if hot {
            colors.ghost_element_hover
        } else {
            Rgba::TRANSPARENT
        })
        .on_click(id)
        .child(icon(kind).size(14.0).color(colors.icon))
        .into()
}

/// The file-type icon a repo's detected stack reads as, if any.
pub fn stack_icon(stack: &[String]) -> Option<MaterialIcon> {
    let has = |words: &[&str]| {
        stack
            .iter()
            .any(|name| words.iter().any(|word| name.eq_ignore_ascii_case(word)))
    };
    Some(if has(&["React", "Next.js", "Vite"]) {
        MaterialIcon::React
    } else if has(&["Rails", "Ruby", "Rack", "Sinatra"]) {
        MaterialIcon::Ruby
    } else if has(&["Node", "Express", "NestJS"]) {
        MaterialIcon::NodeJs
    } else if has(&["Python", "Django", "Flask", "FastAPI"]) {
        MaterialIcon::Python
    } else if has(&["Go"]) {
        MaterialIcon::Go
    } else if has(&["Rust", "Cargo"]) {
        MaterialIcon::Rust
    } else if has(&["Angular", "TypeScript"]) {
        MaterialIcon::TypeScript
    } else if has(&["PHP", "Laravel"]) {
        MaterialIcon::Php
    } else if has(&["Elixir", "Phoenix"]) {
        MaterialIcon::Elixir
    } else if has(&["Java", "Spring"]) {
        MaterialIcon::Java
    } else {
        return None;
    })
}

/// A repo's icon: its stack's file icon, else a folder for a local repo and a branch for one still to clone.
pub fn repo_icon(stack: &[String], remote: bool) -> Node {
    match stack_icon(stack) {
        Some(material) => material_icon(material).size(18.0).into(),
        None => icon(if remote {
            IconKind::Branch
        } else {
            IconKind::Folder
        })
        .size(18.0)
        .color(theme().icon_muted)
        .into(),
    }
}

/// One repo row: its icon, its name over a muted second line, then `trailing` at the right.
pub fn repo_row(lead: Node, name: Node, second: &str, trailing: Vec<Node>) -> Node {
    let colors = theme();
    let mut row = div()
        .row()
        .items_center()
        .gap(12.0)
        .pl(10.0)
        .pr(6.0)
        .py(8.0)
        .rounded(4.0)
        .child(lead)
        .child(
            div().col().flex(1.0).child(name).child(text_line(
                16.0,
                label(second.to_string())
                    .size(12.0)
                    .color(colors.text_placeholder)
                    .truncate_start(),
            )),
        );
    for node in trailing {
        row = row.child(node);
    }
    row.into()
}

pub fn strong_line(text: &str) -> Node {
    text_line(
        20.0,
        label(text.to_string())
            .size(13.0)
            .medium()
            .color(theme().text)
            .truncate(),
    )
}

pub fn seconds_text(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        _ => format!("{}m {}s", seconds / 60, seconds % 60),
    }
}
