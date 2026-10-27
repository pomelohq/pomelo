use components::MenuItem;
use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Radius, Rgba, Space};

use ui::inspect::{Inspect, Inspected};

use crate::comments::Comment;

use crate::scene::{Board, Group};

pub const LEFT: f32 = 248.0;
pub const RIGHT: f32 = 240.0;
const HEADER: f32 = 44.0;
const ROW: f32 = 28.0;
const INSET: Space = Space::S8;

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
pub const INSPECT: u64 = ACTIONS + 7;
pub const COMMENT_FIELD: u64 = ACTIONS + 8;
pub const COMMENT_SEND: u64 = ACTIONS + 9;
pub const COPY_FOR_AGENT: u64 = ACTIONS + 10;
pub const COMMENT: u64 = 6 << 20;
pub const RESOLVE: u64 = 7 << 20;

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
    Inspect,
    Comment(u64),
    Resolve(u64),
    CommentField,
    CommentSend,
    CopyForAgent,
}

pub fn decode(id: u64) -> Option<Click> {
    let index = (id & ((1 << 20) - 1)) as usize;
    Some(match id >> 20 {
        1 => Click::Page(index),
        2 => Click::Board(index),
        3 => Click::Theme(index),
        5 => Click::Group(index),
        6 => Click::Comment(index as u64),
        7 => Click::Resolve(index as u64),
        _ => match id {
            ZOOM_FIT => Click::Fit,
            ZOOM_ACTUAL => Click::Actual,
            ZOOM_IN => Click::ZoomIn,
            ZOOM_OUT => Click::ZoomOut,
            HIDE => Click::Hide,
            THEMES => Click::Themes,
            PAGES => Click::Pages,
            INSPECT => Click::Inspect,
            COMMENT_FIELD => Click::CommentField,
            COMMENT_SEND => Click::CommentSend,
            COPY_FOR_AGENT => Click::CopyForAgent,
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
        .pl(Space::S16)
        .pr(INSET)
        .child(label(title).size(LabelSize::Medium).weight(600))
        .child(trailing)
        .into()
}

fn section(title: &str, trailing: Option<String>) -> Node {
    let mut line = div()
        .h_px(32.0)
        .items_center()
        .justify_between()
        .px(Space::S16)
        .pt(Space::S6)
        .child(
            label(title)
                .size(LabelSize::Mini)
                .weight(600)
                .color(theme().text_muted),
        );
    if let Some(text) = trailing {
        line = line.child(
            label(text)
                .size(LabelSize::Mini)
                .color(muted(theme().text_muted, 0.7)),
        );
    }
    line.into()
}

fn icon_button(id: u64, kind: IconKind) -> Node {
    div()
        .w_px(26.0)
        .h_px(26.0)
        .items_center()
        .justify_center()
        .rounded(Radius::Medium)
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
        .w_px(width - INSET.px() * 2.0)
        .h_px(ROW)
        .items_center()
        .gap(Space::S8)
        .pl(Space::Px(8.0 + depth * 16.0))
        .pr(Space::S8)
        .rounded(Radius::Medium)
        .on_click(id)
        .hover_bg(colors.ghost_element_hover);
    if selected {
        line = line.bg(colors.element_selected);
    }
    line = line.child(leading).child(
        div().flex(1.0).child(
            label(text)
                .size(LabelSize::Small)
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
    div().py(Space::S6).child(components::divider()).into()
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
        if group.section_head.is_some() {
            layers = layers.child(section(group.section, None));
        }
        let open = !collapsed.contains(&group.title);
        let members: Vec<(usize, &Board)> = boards
            .iter()
            .enumerate()
            .filter(|(_, board)| board.def.group == group.title)
            .collect();
        layers = layers.child(
            div().px(INSET).child(
                div()
                    .w_px(LEFT - INSET.px() * 2.0)
                    .h_px(ROW)
                    .items_center()
                    .gap(Space::S6)
                    .pl(Space::S6)
                    .pr(Space::S8)
                    .rounded(Radius::Medium)
                    .on_click(GROUP + group_index as u64)
                    .hover_bg(theme().ghost_element_hover)
                    .child(components::disclosure(open))
                    .child(icon(IconKind::Grid).size(14.0).color(theme().icon_muted))
                    .child(
                        div()
                            .flex(1.0)
                            .child(label(group.title).size(LabelSize::Small).weight(600)),
                    )
                    .child(
                        label(members.len().to_string())
                            .size(LabelSize::Mini)
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
        .child(components::divider())
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
            .rounded(Radius::Medium)
            .bg(color)
            .border(1.0, muted(theme().border, 0.9))
    };
    div()
        .gap(Space::S2)
        .child(chip(colors.panel_background))
        .child(chip(colors.text_accent))
        .child(chip(colors.text))
        .into()
}

/// `inspector` is the inspected element's properties, shown in place of the themes while inspecting.
pub fn right(
    height: f32,
    themes: &[(&str, ui::Appearance)],
    current: &str,
    zoom: f32,
    inspector: Option<Node>,
) -> Node {
    let mut inspect_button = div()
        .w_px(26.0)
        .h_px(26.0)
        .items_center()
        .justify_center()
        .rounded(Radius::Medium)
        .on_click(INSPECT)
        .hover_bg(theme().ghost_element_hover);
    inspect_button = if inspector.is_some() {
        inspect_button.bg(theme().element_selected).child(
            icon(IconKind::Pointer)
                .size(15.0)
                .color(theme().icon_accent),
        )
    } else {
        inspect_button.child(icon(IconKind::Pointer).size(15.0).color(theme().icon_muted))
    };
    let zoom_bar = div()
        .items_center()
        .gap(Space::S2)
        .child(inspect_button)
        .child(icon_button(ZOOM_OUT, IconKind::Dash))
        .child(
            div()
                .w_px(52.0)
                .h_px(26.0)
                .items_center()
                .justify_center()
                .rounded(Radius::Medium)
                .on_click(ZOOM_ACTUAL)
                .hover_bg(theme().ghost_element_hover)
                .child(
                    label(format!("{}%", (zoom * 100.0).round()))
                        .size(LabelSize::Small)
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
        .child(components::divider())
        .child(inspector.unwrap_or_else(|| theme_list(RIGHT, themes, current)))
        .into()
}

/// The properties of the element under inspection, with each color named by its theme token where one matches.
pub fn inspector(
    element: Option<&Inspected>,
    origin: (f32, f32),
    parent: bool,
    child: bool,
    comments: Node,
) -> Node {
    let mut panel = div().col().child(section("Inspect", None));
    let Some(element) = element else {
        return panel
            .child(
                div().px(Space::S16).pt(Space::S4).child(
                    label("Double-click a board, then point at a part of it. Click pins it.")
                        .size(LabelSize::Small)
                        .color(theme().text_muted)
                        .wrap(RIGHT - 32.0),
                ),
            )
            .child(comments)
            .into();
    };
    let rect = element.rect;
    let kind = match &element.what {
        Inspect::Box { row: true, .. } => "Row",
        Inspect::Box { .. } => "Column",
        Inspect::Text { .. } => "Label",
        Inspect::Icon { .. } => "Icon",
    };
    panel = panel
        .child(property("Element", text_value(kind)))
        .child(property(
            "Size",
            text_value(&format!("{} x {}", px(rect.w), px(rect.h))),
        ))
        .child(property(
            "Position",
            text_value(&format!(
                "{}, {}",
                px(rect.x - origin.0),
                px(rect.y - origin.1)
            )),
        ));
    match &element.what {
        Inspect::Box {
            padding,
            gap,
            background,
            border,
            border_color,
            radius,
            ..
        } => {
            let [top, right, bottom, left] = padding.map(px);
            panel = panel
                .child(property(
                    "Padding",
                    text_value(&format!("{top} {right} {bottom} {left}")),
                ))
                .child(property("Gap", text_value(&px(*gap))))
                .child(property(
                    "Background",
                    background.map_or_else(|| text_value("none"), color_value),
                ))
                .child(property("Radius", text_value(&px(*radius))));
            if *border > 0.0 {
                panel = panel
                    .child(property("Border", text_value(&px(*border))))
                    .child(property("Border color", color_value(*border_color)));
            }
        }
        Inspect::Text {
            text,
            size,
            weight,
            color,
            font,
        } => {
            panel = panel
                .child(property("Text", text_value(text)))
                .child(property("Font", text_value(&format!("{font:?}"))))
                .child(property("Font size", text_value(&px(*size))))
                .child(property("Weight", text_value(&weight.to_string())))
                .child(property("Color", color_value(*color)));
        }
        Inspect::Icon { kind, size, color } => {
            panel = panel
                .child(property("Icon", text_value(&format!("{kind:?}"))))
                .child(property("Icon size", text_value(&px(*size))))
                .child(property("Color", color_value(*color)));
        }
    }
    let mut keys = Vec::new();
    if parent {
        keys.push("Up: parent");
    }
    if child {
        keys.push("Down: child");
    }
    keys.push("Esc: unpin");
    panel
        .child(
            div().px(Space::S16).pt(Space::S12).child(
                label(keys.join("   "))
                    .size(LabelSize::Mini)
                    .color(theme().text_muted),
            ),
        )
        .child(comments)
        .into()
}

/// The open comments on the pinned element, then the box to add one; `field` is `None` until one is pinned.
pub fn comment_thread(on_element: &[&Comment], field: Option<Node>, can_send: bool) -> Node {
    let mut thread = div().col();
    let Some(field) = field else {
        return thread.into();
    };
    thread = thread.child(section("Comment", None));
    for comment in on_element {
        thread = thread.child(comment_row(comment, None));
    }
    thread
        .child(
            div()
                .col()
                .gap(Space::S8)
                .px(Space::S16)
                .pt(Space::S4)
                .child(field)
                .child(
                    div().justify_end().child(
                        components::button(COMMENT_SEND, "Comment")
                            .kind(components::ButtonKind::Primary)
                            .keystroke(Some("cmd-enter"))
                            .enabled(can_send),
                    ),
                ),
        )
        .into()
}

/// Every open comment, each leading to its element, with the button that hands them all to an agent.
pub fn comment_list(open: &[(&Comment, &str)]) -> Node {
    let mut list = div()
        .col()
        .pt(Space::S12)
        .child(section("Open comments", Some(open.len().to_string())));
    if open.is_empty() {
        return list
            .child(
                div().px(Space::S16).child(
                    label("Pin an element and write what should change.")
                        .size(LabelSize::Small)
                        .color(theme().text_muted)
                        .wrap(RIGHT - 32.0),
                ),
            )
            .into();
    }
    for (comment, place) in open {
        list = list.child(comment_row(comment, Some(place)));
    }
    list.child(
        div().px(Space::S16).pt(Space::S8).child(
            components::button(COPY_FOR_AGENT, "Copy for agent")
                .kind(components::ButtonKind::Agent)
                .icon(Some(IconKind::Copy)),
        ),
    )
    .into()
}

fn comment_row(comment: &Comment, place: Option<&str>) -> Node {
    let mut text = div().col().flex(1.0).gap(Space::S2);
    if let Some(place) = place {
        text = text.child(
            label(place)
                .size(LabelSize::Mini)
                .color(theme().text_muted)
                .wrap(RIGHT - 32.0 - 52.0),
        );
    }
    text = text.child(
        label(comment.text.trim())
            .size(LabelSize::Small)
            .wrap(RIGHT - 32.0 - 52.0),
    );
    let line = div()
        .flex(1.0)
        .px(Space::S8)
        .py(Space::S6)
        .gap(Space::S8)
        .rounded(Radius::Medium)
        .on_click(COMMENT + comment.id)
        .hover_bg(theme().ghost_element_hover)
        .child(comment_badge(comment.id))
        .child(text)
        .child(
            div()
                .w_px(20.0)
                .h_px(20.0)
                .items_center()
                .justify_center()
                .rounded(Radius::Small)
                .on_click(RESOLVE + comment.id)
                .hover_bg(theme().ghost_element_hover)
                .child(icon(IconKind::Check).size(13.0).color(theme().icon_muted)),
        );
    div().px(INSET).child(line).into()
}

/// A comment's marker on its board, clicked to open the board with the element pinned.
pub fn comment_badge_target(id: u64, badge: Node) -> Node {
    div().on_click(COMMENT + id).child(badge).into()
}

/// The numbered marker a comment wears in the list and on its board.
pub fn comment_badge(id: u64) -> Node {
    div()
        .h_px(18.0)
        .px(Space::S6)
        .items_center()
        .justify_center()
        .rounded_px(9.0)
        .bg(theme().text_accent)
        .child(
            label(id.to_string())
                .size(LabelSize::Mini)
                .weight(600)
                .color(theme().background),
        )
        .into()
}

const NAME_W: f32 = 76.0;
const VALUE_W: f32 = RIGHT - 32.0 - NAME_W - 8.0;

fn px(value: f32) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    format!("{rounded}")
}

fn property(name: &str, value: Node) -> Node {
    div()
        .py(Space::S4)
        .items_center()
        .gap(Space::S8)
        .px(Space::S16)
        .child(
            div()
                .w_px(NAME_W)
                .child(label(name).size(LabelSize::Small).color(theme().text_muted)),
        )
        .child(div().flex(1.0).child(value))
        .into()
}

fn text_value(text: &str) -> Node {
    label(text).size(LabelSize::Small).wrap(VALUE_W).into()
}

fn color_value(color: Rgba) -> Node {
    let hex = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut code = format!(
        "#{:02X}{:02X}{:02X}",
        hex(color.r),
        hex(color.g),
        hex(color.b)
    );
    if color.a < 1.0 {
        code.push_str(&format!(" {}%", (color.a * 100.0).round()));
    }
    let mut text = div().col();
    if let Some(token) = ui::theme_file::token_name(color) {
        text = text.child(label(token).size(LabelSize::Small).wrap(VALUE_W - 18.0));
    }
    div()
        .items_center()
        .gap(Space::S6)
        .child(
            div()
                .w_px(12.0)
                .h_px(12.0)
                .rounded(Radius::Small)
                .bg(color)
                .border(1.0, theme().border),
        )
        .child(
            text.child(
                label(code)
                    .size(LabelSize::Small)
                    .mono()
                    .color(theme().text_muted),
            ),
        )
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
    components::menu(&items).hovered(hovered).into()
}

fn panels_button() -> Node {
    icon_button(HIDE, IconKind::Sidebar)
}

/// A board's title above it on the canvas, at a fixed screen size whatever the zoom.
pub fn board_title(title: &str, note: Option<&str>, width: f32) -> Node {
    let mut line = div()
        .w_px(width)
        .gap(Space::S6)
        .child(label(title).size(LabelSize::Small).weight(500).truncate());
    if let Some(note) = note {
        line = line.child(
            label(note)
                .size(LabelSize::Small)
                .color(theme().text_muted)
                .wrap(width),
        );
    }
    line.into()
}

pub fn section_title(title: &str) -> Node {
    label(title).size_px(26.0).weight(600).into()
}

pub fn group_title(title: &str) -> Node {
    label(title).size(LabelSize::Large).weight(600).into()
}

pub fn group_note(note: &str, width: f32) -> Node {
    div()
        .w_px(width)
        .child(
            label(note)
                .size(LabelSize::Small)
                .color(theme().text_muted)
                .wrap(width),
        )
        .into()
}

fn pill() -> ui::Div {
    div()
        .h_px(40.0)
        .items_center()
        .px(Space::S6)
        .rounded(Radius::XLarge)
        .bg(theme().panel_background)
        .border(1.0, theme().border)
}

fn picker(id: u64, content: Node) -> Node {
    div()
        .h_px(28.0)
        .items_center()
        .gap(Space::S6)
        .px(Space::S8)
        .rounded(Radius::Medium)
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
        .gap(Space::S4)
        .pl(Space::S14)
        .child(label("Pomelo Gallery").size(LabelSize::Medium).weight(600))
        .child(picker(
            PAGES,
            label(page)
                .size(LabelSize::Medium)
                .color(theme().text_muted)
                .into(),
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
    components::menu(&items).hovered(hovered).into()
}

/// The keys that hide and show the panels, shown while the pointer is on that button.
pub fn shortcut_tip(action: &str) -> Node {
    let key = |content: Node| {
        div()
            .h_px(20.0)
            .px(Space::S6)
            .items_center()
            .justify_center()
            .rounded(Radius::Small)
            .bg(theme().element_background)
            .child(content)
    };
    let modifier = if cfg!(target_os = "macos") {
        icon(IconKind::Command)
            .size(12.0)
            .color(theme().icon)
            .into()
    } else {
        label("Ctrl").size(LabelSize::Mini).into()
    };
    div()
        .items_center()
        .gap(Space::S6)
        .p(Space::S6)
        .rounded(Radius::Large)
        .bg(theme().elevated_surface_background)
        .border(1.0, theme().border)
        .child(label(action).size(LabelSize::Small))
        .child(
            div()
                .gap(Space::S2)
                .child(key(icon(IconKind::Shift)
                    .size(12.0)
                    .color(theme().icon)
                    .into()))
                .child(key(modifier))
                .child(key(label("\\").size(LabelSize::Mini).mono().into())),
        )
        .into()
}
