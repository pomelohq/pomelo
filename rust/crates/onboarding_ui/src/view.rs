//! Drawing pieces of the onboarding page that carry no state of their own.

use components::text_line;
use ui::{
    div, icon, label, material_icon, theme, IconKind, LabelSize, MaterialIcon, Node, Radius, Space,
};
use workspace::text_field::{FieldFont, TextField};

pub const COLUMN_W: f32 = 640.0;
pub const WIDE_W: f32 = 760.0;
pub const PAGE_PAD_X: f32 = 24.0;
pub const PAGE_PAD_TOP: f32 = 36.0;
pub const PAGE_PAD_BOTTOM: f32 = 20.0;
pub const PAGE_GAP: f32 = 16.0;
pub const FOOTER_H: f32 = 52.0;

/// The wizard's place along Repositories, Setup, Review; `done` marks every step finished.
pub fn steps(titles: &[&str], current: usize, done: bool) -> Node {
    let colors = theme();
    let mut row = div().row().items_center().gap(Space::S20);
    for (index, title) in titles.iter().enumerate() {
        let finished = done || index < current;
        let active = !done && index == current;
        let mut mark = div()
            .row()
            .w_px(20.0)
            .h_px(20.0)
            .rounded(Radius::XLarge)
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
            mark.child(
                label((index + 1).to_string())
                    .size(LabelSize::Small)
                    .color(if active {
                        colors.on_accent
                    } else {
                        colors.text_placeholder
                    }),
            )
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
                .gap(Space::S8)
                .on_click(crate::ids::STEP_BASE + index as u64)
                .child(mark)
                .child(label(title.to_string()).size(LabelSize::Small).color(ink)),
        );
    }
    row.into()
}

/// A page title with the muted line under it.
pub fn head(title: &str, sub: Option<&str>) -> Node {
    let colors = theme();
    let mut column = div().col().gap(Space::S4).child(text_line(
        28.0,
        label(title.to_string())
            .size_px(20.0)
            .weight(600)
            .color(colors.text)
            .truncate(),
    ));
    if let Some(sub) = sub {
        column = column.child(text_line(
            20.0,
            label(sub.to_string())
                .size(LabelSize::Medium)
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
        .gap(Space::S2)
        .p(Space::Px(inset))
        .rounded(Radius::Medium)
        .bg(theme().panel_background);
    for item in items {
        column = column.child(item);
    }
    column.into()
}

/// A card that rings in the accent when `chosen`; the caller sets its padding.
pub fn card_frame(chosen: bool) -> ui::Div {
    components::card().padding(0.0).chosen(chosen).render()
}

pub fn chip(text: &str, on: bool) -> Node {
    let colors = theme();
    components::chip(text)
        .color(if on {
            colors.text_muted
        } else {
            colors.text_placeholder
        })
        .into()
}

pub fn muted(text: &str) -> ui::Label {
    label(text.to_string())
        .size(LabelSize::Small)
        .color(theme().text_placeholder)
}

/// Small muted copy broken into lines that fit `width`.
pub fn note_paragraph(text: &str, width: f32) -> Node {
    Node::from(
        components::paragraph(text, width)
            .size(LabelSize::Small, 16.0)
            .color(theme().text_placeholder),
    )
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
    let frame = components::input_box(id, focused, error);
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
    let mut start = div().row().flex(1.0).items_center().gap(Space::S8);
    if let Some(note) = note {
        start = start.child(muted(note).truncate());
    }
    for node in leading {
        start = start.child(node);
    }
    let mut row = div()
        .row()
        .h_px(FOOTER_H)
        .px(Space::S16)
        .gap(Space::S8)
        .items_center()
        .bg(theme().panel_background)
        .child(start);
    for button in buttons {
        row = row.child(button);
    }
    row.into()
}

/// A small icon-only button with no fill until hovered.
pub fn icon_button(id: u64, kind: IconKind, hot: bool) -> Node {
    components::icon_button(id, kind)
        .color(theme().icon)
        .hovered(hot.then_some(id))
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
        .gap(Space::S12)
        .pl(Space::S10)
        .pr(Space::S6)
        .py(Space::S8)
        .rounded(Radius::Small)
        .child(lead)
        .child(
            div().col().flex(1.0).child(name).child(text_line(
                16.0,
                label(second.to_string())
                    .size(LabelSize::Small)
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
            .size(LabelSize::Medium)
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
