//! Atoms put together into one control: a menu of choices, and a select with its menu open.

use ui::{div, IconKind, MenuItem, Node};

use crate::{still, Story, StoryDef};

const PAGE: &str = "Molecules";
const ID: u64 = 1;
const INSET: f32 = 16.0;
const SELECT_GAP: f32 = 4.0;
const SELECT_HEIGHT: f32 = 28.0;

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
        .child(ui::select(ID + 10, "Pomelo Dark").min_width(200.0))
        .child(ui::menu(&items).hovered(Some(ID + 1)))
        .into()
}

fn select_open_size() -> (f32, f32) {
    let (width, height) = card_size(&chosen(), false);
    (width, height + SELECT_HEIGHT + SELECT_GAP)
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
            "menu-chosen",
            "Menu",
            "One chosen",
            card_size(&chosen(), false),
            || still(|_, _| card(&chosen(), None, false)),
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
            "menu-search-empty",
            "Menu",
            "Search, just opened",
            searchable_size(""),
            || still(|_, _| searchable("", Some(ID))),
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
    ]
}
