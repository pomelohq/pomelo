use ui::{div, icon, label, theme, IconKind, MenuItem, Node, Rgba};

use crate::scene::{Board, Group};

pub const LEFT: f32 = 248.0;
pub const RIGHT: f32 = 240.0;
const HEADER: f32 = 44.0;
const ROW: f32 = 28.0;
const INSET: f32 = 8.0;

pub const PAGE: u64 = 1 << 20;
pub const BOARD: u64 = 2 << 20;
pub const THEME: u64 = 3 << 20;
pub const GROUP: u64 = 5 << 20;
const ACTIONS: u64 = 4 << 20;
pub const ZOOM_FIT: u64 = ACTIONS;
pub const ZOOM_ACTUAL: u64 = ACTIONS + 1;
pub const HIDE: u64 = ACTIONS + 2;
pub const ZOOM_IN: u64 = ACTIONS + 3;
pub const ZOOM_OUT: u64 = ACTIONS + 4;
pub const THEMES: u64 = ACTIONS + 5;
pub const PAGES: u64 = ACTIONS + 6;

pub enum Click {
    Page(usize),
    Board(usize),
    Theme(usize),
    Group(usize),
    Themes,
    Pages,
    Fit,
    Actual,
    ZoomIn,
    ZoomOut,
    Hide,
}

pub fn decode(id: u64) -> Option<Click> {
    let index = (id & ((1 << 20) - 1)) as usize;
    Some(match id >> 20 {
        1 => Click::Page(index),
        2 => Click::Board(index),
        3 => Click::Theme(index),
        5 => Click::Group(index),
        _ => match id {
            ZOOM_FIT => Click::Fit,
            ZOOM_ACTUAL => Click::Actual,
            ZOOM_IN => Click::ZoomIn,
            ZOOM_OUT => Click::ZoomOut,
            HIDE => Click::Hide,
            THEMES => Click::Themes,
            PAGES => Click::Pages,
            _ => return None,
        },
    })
}

fn muted(color: Rgba, alpha: f32) -> Rgba {
    Rgba::new(color.r, color.g, color.b, color.a * alpha)
}

fn header(title: &str, trailing: Node) -> Node {
    div()
        .h_px(HEADER)
        .items_center()
        .justify_between()
        .pl(16.0)
        .pr(INSET)
        .child(label(title).size(13.0).weight(600))
        .child(trailing)
        .into()
}

fn section(title: &str, trailing: Option<String>) -> Node {
    let mut line = div()
        .h_px(32.0)
        .items_center()
        .justify_between()
        .px(16.0)
        .pt(6.0)
        .child(
            label(title)
                .size(11.0)
                .weight(600)
                .color(theme().text_muted),
        );
    if let Some(text) = trailing {
        line = line.child(label(text).size(11.0).color(muted(theme().text_muted, 0.7)));
    }
    line.into()
}

fn icon_button(id: u64, kind: IconKind) -> Node {
    div()
        .w_px(26.0)
        .h_px(26.0)
        .items_center()
        .justify_center()
        .rounded(6.0)
        .on_click(id)
        .hover_bg(theme().ghost_element_hover)
        .child(icon(kind).size(15.0).color(theme().icon_muted))
        .into()
}

/// A full-width list row: `depth` steps of indent, an icon, the name, and what sits at its right end.
fn row(
    id: u64,
    width: f32,
    depth: f32,
    leading: Node,
    text: &str,
    selected: bool,
    trailing: Option<Node>,
) -> Node {
    let colors = theme();
    let mut line = div()
        .w_px(width - INSET * 2.0)
        .h_px(ROW)
        .items_center()
        .gap(8.0)
        .pl(8.0 + depth * 16.0)
        .pr(8.0)
        .rounded(6.0)
        .on_click(id)
        .hover_bg(colors.ghost_element_hover);
    if selected {
        line = line.bg(colors.element_selected);
    }
    line = line.child(leading).child(
        div().flex(1.0).child(
            label(text)
                .size(12.0)
                .weight(if selected { 500 } else { 400 })
                .color(if selected {
                    colors.text
                } else {
                    muted(colors.text, 0.86)
                })
                .truncate(),
        ),
    );
    if let Some(trailing) = trailing {
        line = line.child(trailing);
    }
    div().px(INSET).child(line).into()
}

fn row_icon(kind: IconKind, selected: bool) -> Node {
    icon(kind)
        .size(14.0)
        .color(if selected {
            theme().icon
        } else {
            theme().icon_muted
        })
        .into()
}

fn rule() -> Node {
    div().py(6.0).child(ui::divider()).into()
}

/// The left panel's fixed top (title, pages, the Layers heading) and its layer list, which scrolls under it.
pub fn left(
    pages: &[&str],
    page: usize,
    groups: &[Group],
    boards: &[Board],
    focused: Option<usize>,
    collapsed: &[&str],
) -> (Node, Node) {
    let mut layers = div().col().pb(INSET);
    for (group_index, group) in groups.iter().enumerate() {
        let open = !collapsed.contains(&group.title);
        let members: Vec<(usize, &Board)> = boards
            .iter()
            .enumerate()
            .filter(|(_, board)| board.def.group == group.title)
            .collect();
        layers = layers.child(
            div().px(INSET).child(
                div()
                    .w_px(LEFT - INSET * 2.0)
                    .h_px(ROW)
                    .items_center()
                    .gap(6.0)
                    .pl(6.0)
                    .pr(8.0)
                    .rounded(6.0)
                    .on_click(GROUP + group_index as u64)
                    .hover_bg(theme().ghost_element_hover)
                    .child(
                        icon(if open {
                            IconKind::ChevronDown
                        } else {
                            IconKind::ChevronRight
                        })
                        .size(12.0)
                        .color(theme().icon_muted),
                    )
                    .child(icon(IconKind::Grid).size(14.0).color(theme().icon_muted))
                    .child(
                        div()
                            .flex(1.0)
                            .child(label(group.title).size(12.0).weight(600)),
                    )
                    .child(
                        label(members.len().to_string())
                            .size(11.0)
                            .color(muted(theme().text_muted, 0.7)),
                    ),
            ),
        );
        if !open {
            continue;
        }
        for (index, board) in members {
            layers = layers.child(row(
                BOARD + index as u64,
                LEFT,
                1.0,
                row_icon(IconKind::Window, focused == Some(index)),
                board.def.title,
                focused == Some(index),
                board.error.as_ref().map(|_| {
                    icon(IconKind::Warning)
                        .size(12.0)
                        .color(theme().warning)
                        .into()
                }),
            ));
        }
    }
    let top = div()
        .col()
        .w_px(LEFT)
        .child(header("Pomelo Gallery", panels_button()))
        .child(ui::divider())
        .child(section("Pages", Some(pages.len().to_string())))
        .children(pages.iter().enumerate().map(|(index, name)| {
            row(
                PAGE + index as u64,
                LEFT,
                0.0,
                row_icon(IconKind::File, index == page),
                name,
                index == page,
                None,
            )
        }))
        .child(rule())
        .child(section("Layers", Some(boards.len().to_string())))
        .into();
    (top, layers.into())
}

/// Three colors of a theme side by side: its panel, its accent and its text.
fn swatch(name: &str) -> Node {
    let colors = ui::by_name(name);
    let chip = |color: Rgba| {
        div()
            .w_px(10.0)
            .h_px(10.0)
            .rounded(5.0)
            .bg(color)
            .border(1.0, muted(theme().border, 0.9))
    };
    div()
        .gap(2.0)
        .child(chip(colors.panel_background))
        .child(chip(colors.text_accent))
        .child(chip(colors.text))
        .into()
}

pub fn right(height: f32, themes: &[(&str, ui::Appearance)], current: &str, zoom: f32) -> Node {
    let zoom_bar = div()
        .items_center()
        .gap(2.0)
        .child(icon_button(ZOOM_OUT, IconKind::Dash))
        .child(
            div()
                .w_px(52.0)
                .h_px(26.0)
                .items_center()
                .justify_center()
                .rounded(6.0)
                .on_click(ZOOM_ACTUAL)
                .hover_bg(theme().ghost_element_hover)
                .child(
                    label(format!("{}%", (zoom * 100.0).round()))
                        .size(12.0)
                        .mono(),
                ),
        )
        .child(icon_button(ZOOM_IN, IconKind::Plus))
        .child(icon_button(ZOOM_FIT, IconKind::Maximize));
    div()
        .col()
        .w_px(RIGHT)
        .h_px(height)
        .bg(theme().panel_background)
        .child(header("View", zoom_bar.into()))
        .child(ui::divider())
        .child(theme_list(RIGHT, themes, current))
        .into()
}

fn theme_list(width: f32, themes: &[(&str, ui::Appearance)], current: &str) -> Node {
    let mut list = div().col();
    for (appearance, title) in [
        (ui::Appearance::Dark, "Dark themes"),
        (ui::Appearance::Light, "Light themes"),
    ] {
        list = list.child(section(title, None));
        for (index, (name, _)) in themes
            .iter()
            .enumerate()
            .filter(|(_, (_, kind))| *kind == appearance)
        {
            let selected = *name == current;
            list = list.child(row(
                THEME + index as u64,
                width,
                0.0,
                swatch(name),
                name,
                selected,
                Some(if selected {
                    icon(IconKind::Check)
                        .size(13.0)
                        .color(theme().icon_accent)
                        .into()
                } else {
                    div().into()
                }),
            ));
        }
    }
    list.into()
}

/// The theme list floating under the mini bar while the panels are hidden, each theme marked by its accent.
pub fn theme_menu(themes: &[(&str, ui::Appearance)], current: &str, hovered: Option<u64>) -> Node {
    let mut items = Vec::new();
    for (appearance, title) in [
        (ui::Appearance::Dark, "Dark themes"),
        (ui::Appearance::Light, "Light themes"),
    ] {
        items.push(MenuItem {
            header: true,
            sep: !items.is_empty(),
            ..MenuItem::new(0, title)
        });
        items.extend(
            themes
                .iter()
                .enumerate()
                .filter(|(_, (_, kind))| *kind == appearance)
                .map(|(index, (name, _))| MenuItem {
                    checked: *name == current,
                    icon: Some(IconKind::Circle),
                    color: Some(ui::by_name(name).text_accent),
                    ..MenuItem::new(THEME + index as u64, name.to_string())
                }),
        );
    }
    ui::menu(&items).hovered(hovered).into()
}

fn panels_button() -> Node {
    icon_button(HIDE, IconKind::Sidebar)
}

/// A board's title above it on the canvas, at a fixed screen size whatever the zoom.
pub fn board_title(title: &str, note: Option<&str>, width: f32) -> Node {
    let mut line = div()
        .w_px(width)
        .gap(6.0)
        .child(label(title).size(12.0).weight(500).truncate());
    if let Some(note) = note {
        line = line.child(label(note).size(12.0).color(theme().text_muted).wrap(width));
    }
    line.into()
}

pub fn group_title(title: &str) -> Node {
    label(title).size(15.0).weight(600).into()
}

pub fn group_note(note: &str, width: f32) -> Node {
    div()
        .w_px(width)
        .child(label(note).size(12.0).color(theme().text_muted).wrap(width))
        .into()
}

fn pill() -> ui::Div {
    div()
        .h_px(40.0)
        .items_center()
        .px(6.0)
        .rounded(10.0)
        .bg(theme().panel_background)
        .border(1.0, theme().border)
}

fn picker(id: u64, content: Node) -> Node {
    div()
        .h_px(28.0)
        .items_center()
        .gap(6.0)
        .px(8.0)
        .rounded(6.0)
        .on_click(id)
        .hover_bg(theme().ghost_element_hover)
        .child(content)
        .child(
            icon(IconKind::ChevronDown)
                .size(12.0)
                .color(theme().icon_muted),
        )
        .into()
}

/// What stays on screen with the panels hidden: the gallery's name, a page picker, and a way back.
pub fn mini(page: &str) -> Node {
    pill()
        .gap(4.0)
        .pl(14.0)
        .child(label("Pomelo Gallery").size(13.0).weight(600))
        .child(picker(
            PAGES,
            label(page).size(13.0).color(theme().text_muted).into(),
        ))
        .child(panels_button())
        .into()
}

pub fn theme_pill(current: &str) -> Node {
    pill().child(picker(THEMES, swatch(current))).into()
}

pub fn page_menu(pages: &[&str], page: usize, hovered: Option<u64>) -> Node {
    let items: Vec<MenuItem> = pages
        .iter()
        .enumerate()
        .map(|(index, name)| MenuItem {
            checked: index == page,
            icon: Some(IconKind::File),
            ..MenuItem::new(PAGE + index as u64, name.to_string())
        })
        .collect();
    ui::menu(&items).hovered(hovered).into()
}

/// The keys that hide and show the panels, shown while the pointer is on that button.
pub fn shortcut_tip(action: &str) -> Node {
    let key = |content: Node| {
        div()
            .h_px(20.0)
            .px(5.0)
            .items_center()
            .justify_center()
            .rounded(4.0)
            .bg(theme().element_background)
            .child(content)
    };
    let modifier = if cfg!(target_os = "macos") {
        icon(IconKind::Command)
            .size(12.0)
            .color(theme().icon)
            .into()
    } else {
        label("Ctrl").size(11.0).into()
    };
    div()
        .items_center()
        .gap(6.0)
        .p(6.0)
        .rounded(8.0)
        .bg(theme().elevated_surface_background)
        .border(1.0, theme().border)
        .child(label(action).size(12.0))
        .child(
            div()
                .gap(2.0)
                .child(key(icon(IconKind::Shift)
                    .size(12.0)
                    .color(theme().icon)
                    .into()))
                .child(key(modifier))
                .child(key(label("\\").size(11.0).mono().into())),
        )
        .into()
}
