//! Atoms put together into one control: a menu of choices, and a select with its menu open.

use ui::{div, IconKind, MenuItem, Node};

use crate::{still, Story, StoryDef};

const PAGE: &str = "Molecules";
const ID: u64 = 1;
const INSET: f32 = 16.0;
const SELECT_GAP: f32 = 4.0;
const SELECT_HEIGHT: f32 = 28.0;
const SELECT_WIDTH: f32 = 160.0;

fn item(index: u64, text: &'static str) -> MenuItem {
    MenuItem::new(ID + index, text)
}

fn plain() -> Vec<MenuItem> {
    vec![
        item(0, "Open"),
        item(1, "Open in new pane"),
        item(2, "Reveal in Finder"),
    ]
}

fn chosen() -> Vec<MenuItem> {
    ["Pomelo Dark", "Pomelo Light", "Solarized Dark"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| MenuItem {
            checked: index == 0,
            ..item(index as u64, name)
        })
        .collect()
}

fn icons_and_keys() -> Vec<MenuItem> {
    vec![
        MenuItem {
            icon: Some(IconKind::Play),
            hint: Some("cmd-shift-s".into()),
            ..item(0, "Start all")
        },
        MenuItem {
            icon: Some(IconKind::Stop),
            hint: Some("cmd-shift-x".into()),
            ..item(1, "Stop all")
        },
        MenuItem {
            icon: Some(IconKind::Sparkle),
            ..item(2, "Ask the agent")
        },
    ]
}

fn groups() -> Vec<MenuItem> {
    vec![
        MenuItem {
            header: true,
            ..item(0, "Clipboard")
        },
        MenuItem {
            hint: Some("cmd-x".into()),
            ..item(1, "Cut")
        },
        MenuItem {
            hint: Some("cmd-c".into()),
            ..item(2, "Copy")
        },
        MenuItem {
            sep: true,
            header: true,
            ..item(3, "Path")
        },
        MenuItem {
            hint: Some("cmd-shift-c".into()),
            ..item(4, "Copy relative path")
        },
    ]
}

fn danger_and_disabled() -> Vec<MenuItem> {
    vec![
        item(0, "Rename"),
        MenuItem {
            disabled: true,
            ..item(1, "Paste")
        },
        MenuItem {
            sep: true,
            danger: true,
            icon: Some(IconKind::Trash),
            ..item(2, "Delete")
        },
    ]
}

fn submenus() -> Vec<MenuItem> {
    vec![
        MenuItem {
            hint: Some("Pomelo Dark".into()),
            ..item(0, "Theme")
        },
        MenuItem {
            hint: Some("13".into()),
            ..item(1, "Font size")
        },
        MenuItem {
            sep: true,
            ..item(2, "Open settings file")
        },
    ]
}

fn opens_submenu(id: u64) -> bool {
    id < ID + 2
}

fn menu(items: &[MenuItem], nested: bool) -> ui::Menu<'_> {
    let menu = ui::menu(items);
    if nested {
        menu.submenus(opens_submenu)
    } else {
        menu
    }
}

fn card(items: &[MenuItem], hovered: Option<u64>, nested: bool) -> Node {
    div()
        .p(INSET)
        .child(menu(items, nested).hovered(hovered))
        .into()
}

fn card_size(items: &[MenuItem], nested: bool) -> (f32, f32) {
    let (_, width, height) = menu(items, nested).render();
    (width + 2.0 * INSET, height + 2.0 * INSET)
}

const THEMES: [&str; 5] = [
    "Pomelo Dark",
    "Pomelo Light",
    "One Dark",
    "Solarized Dark",
    "Gruvbox Light",
];

/// The themes matching `query`, the way a searchable list filters as you type.
fn themes(query: &str) -> Vec<MenuItem> {
    let query = query.to_lowercase();
    THEMES
        .into_iter()
        .enumerate()
        .filter(|(_, name)| name.to_lowercase().contains(&query))
        .map(|(index, name)| MenuItem {
            checked: index == 0,
            ..item(index as u64, name)
        })
        .collect()
}

fn searchable(query: &'static str, hovered: Option<u64>) -> Node {
    let items = themes(query);
    div()
        .p(INSET)
        .child(ui::menu(&items).search(query).hovered(hovered))
        .into()
}

fn searchable_size(query: &str) -> (f32, f32) {
    let items = themes(query);
    let (_, width, height) = ui::menu(&items).search(query).render();
    (width + 2.0 * INSET, height + 2.0 * INSET)
}

fn select_open() -> Node {
    let items = chosen();
    div()
        .col()
        .p(INSET)
        .gap(SELECT_GAP)
        .child(ui::select(ID + 10, "Pomelo Dark").width(SELECT_WIDTH))
        .child(
            ui::menu(&items)
                .hovered(Some(ID + 1))
                .min_width(SELECT_WIDTH),
        )
        .into()
}

fn select_open_size() -> (f32, f32) {
    let (_, width, height) = ui::menu(&chosen()).min_width(SELECT_WIDTH).render();
    (
        width + 2.0 * INSET,
        height + SELECT_HEIGHT + SELECT_GAP + 2.0 * INSET,
    )
}

const EDGE_BOARD: (f32, f32) = (340.0, 240.0);
const EDGE_MARGIN: f32 = 8.0;

/// A select at `select_at` on a board standing in for the window, with its menu where
/// `ui::place_menu` puts it: so the board shows the real flip, not a hand-drawn one.
fn at_edge(select_at: (f32, f32), searchable: bool) -> Node {
    let items = if searchable { themes("") } else { chosen() };
    let edge_menu = || {
        let menu = ui::menu(&items).hovered(Some(ID)).min_width(SELECT_WIDTH);
        if searchable {
            menu.search("")
        } else {
            menu
        }
    };
    let (_, width, height) = edge_menu().render();
    let anchor = ui::Rect::new(
        select_at.0,
        select_at.1,
        SELECT_WIDTH,
        SELECT_HEIGHT,
        ui::Rgba::TRANSPARENT,
    );
    let (x, y) = ui::place_menu(anchor, (width, height), EDGE_BOARD, SELECT_GAP, EDGE_MARGIN);
    let select = div()
        .pl(anchor.x)
        .child(ui::select(ID + 10, "Pomelo Dark").width(SELECT_WIDTH));
    let menu = div()
        .pl(x)
        .child(edge_menu().search_at_bottom(y < anchor.y));
    let column = div().col().w_px(EDGE_BOARD.0).h_px(EDGE_BOARD.1);
    if y > anchor.y {
        column
            .child(select.pt(anchor.y))
            .child(menu.pt(y - anchor.y - SELECT_HEIGHT))
            .into()
    } else {
        column
            .child(menu.pt(y))
            .child(select.pt(anchor.y - y - height))
            .into()
    }
}

pub fn stories() -> Vec<StoryDef> {
    let story = |id, group, title, size, build: fn() -> anyhow::Result<Box<dyn Story>>| StoryDef {
        id,
        page: PAGE,
        group,
        title,
        size,
        build,
    };
    vec![
        story(
            "menu-plain",
            "Menu",
            "Plain",
            card_size(&plain(), false),
            || still(|_, _| card(&plain(), None, false)),
        ),
        story(
            "menu-hovered",
            "Menu",
            "Hovered",
            card_size(&plain(), false),
            || still(|_, _| card(&plain(), Some(ID + 1), false)),
        ),
        story(
            "menu-icons-keys",
            "Menu",
            "Icons and keys",
            card_size(&icons_and_keys(), false),
            || still(|_, _| card(&icons_and_keys(), None, false)),
        ),
        story(
            "menu-groups",
            "Menu",
            "Groups",
            card_size(&groups(), false),
            || still(|_, _| card(&groups(), None, false)),
        ),
        story(
            "menu-danger-disabled",
            "Menu",
            "Danger and disabled",
            card_size(&danger_and_disabled(), false),
            || still(|_, _| card(&danger_and_disabled(), None, false)),
        ),
        story(
            "menu-submenus",
            "Menu",
            "Submenus",
            card_size(&submenus(), true),
            || still(|_, _| card(&submenus(), None, true)),
        ),
        story(
            "menu-search-typed",
            "Menu",
            "Search, filtered",
            searchable_size("dark"),
            || still(|_, _| searchable("dark", Some(ID + 2))),
        ),
        story(
            "select-open",
            "Select menu",
            "Open",
            select_open_size(),
            || still(|_, _| select_open()),
        ),
        story(
            "select-right-edge",
            "Select menu",
            "Near the right edge",
            EDGE_BOARD,
            || still(|_, _| at_edge((EDGE_BOARD.0 - EDGE_MARGIN - SELECT_WIDTH, INSET), false)),
        ),
        story(
            "select-bottom-search",
            "Select menu",
            "Near the bottom edge",
            EDGE_BOARD,
            || still(|_, _| at_edge((INSET, EDGE_BOARD.1 - EDGE_MARGIN - SELECT_HEIGHT), true)),
        ),
        story(
            "select-corner",
            "Select menu",
            "Near the bottom right corner",
            EDGE_BOARD,
            || {
                still(|_, _| {
                    at_edge(
                        (
                            EDGE_BOARD.0 - EDGE_MARGIN - SELECT_WIDTH,
                            EDGE_BOARD.1 - EDGE_MARGIN - SELECT_HEIGHT,
                        ),
                        false,
                    )
                })
            },
        ),
    ]
}
