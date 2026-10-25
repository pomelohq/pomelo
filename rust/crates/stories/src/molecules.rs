//! Atoms put together into one control: a menu of choices, and a select with its menu open.

use components::{BannerTone, ButtonKind, MenuItem};
use ui::{div, label, theme, IconKind, Node};
use workspace::text_field::{FieldFont, TextField};
use workspace::InputField;

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

/// What a right click on a file opens: every kind of row a menu has, in the order a real one uses them.
fn context_menu() -> Vec<MenuItem> {
    vec![
        MenuItem {
            icon: Some(IconKind::File),
            hint: Some("cmd-o".into()),
            ..item(0, "Open")
        },
        item(1, "Open in new pane"),
        item(2, "Reveal in Finder"),
        MenuItem {
            sep: true,
            header: true,
            ..item(3, "Copy")
        },
        MenuItem {
            icon: Some(IconKind::Copy),
            hint: Some("cmd-alt-c".into()),
            ..item(4, "Copy path")
        },
        MenuItem {
            hint: Some("cmd-shift-c".into()),
            ..item(5, "Copy relative path")
        },
        MenuItem {
            sep: true,
            ..item(6, "Rename")
        },
        MenuItem {
            disabled: true,
            ..item(7, "Paste")
        },
        MenuItem {
            sep: true,
            danger: true,
            icon: Some(IconKind::Trash),
            ..item(8, "Delete")
        },
    ]
}

const CONTEXT_HOVERED: u64 = ID + 6;

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

fn menu(items: &[MenuItem], nested: bool) -> components::Menu<'_> {
    let menu = components::menu(items);
    if nested {
        menu.submenus(opens_submenu)
    } else {
        menu
    }
}

fn submenu_open() -> Node {
    let choices = chosen();
    div()
        .row()
        .p(INSET)
        .gap(2.0)
        .child(menu(&submenus(), true).hovered(Some(ID)))
        .child(components::menu(&choices).hovered(Some(ID + 1)))
        .into()
}

fn submenu_open_size() -> (f32, f32) {
    let (_, parent_width, parent_height) = menu(&submenus(), true).render();
    let (_, child_width, child_height) = components::menu(&chosen()).render();
    (
        parent_width + 2.0 + child_width + 2.0 * INSET,
        parent_height.max(child_height) + 2.0 * INSET,
    )
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
        .child(components::menu(&items).search(query).hovered(hovered))
        .into()
}

fn searchable_size(query: &str) -> (f32, f32) {
    let items = themes(query);
    let (_, width, height) = components::menu(&items).search(query).render();
    (width + 2.0 * INSET, height + 2.0 * INSET)
}

fn select_open() -> Node {
    let items = chosen();
    div()
        .col()
        .p(INSET)
        .gap(SELECT_GAP)
        .child(components::select(ID + 10, "Pomelo Dark").width(SELECT_WIDTH))
        .child(
            components::menu(&items)
                .hovered(Some(ID + 1))
                .min_width(SELECT_WIDTH),
        )
        .into()
}

fn select_open_size() -> (f32, f32) {
    let (_, width, height) = components::menu(&chosen()).min_width(SELECT_WIDTH).render();
    (
        width + 2.0 * INSET,
        height + SELECT_HEIGHT + SELECT_GAP + 2.0 * INSET,
    )
}

const EDGE_BOARD: (f32, f32) = (340.0, 240.0);
const EDGE_MARGIN: f32 = 8.0;

/// A select at `select_at` on a board standing in for the window, with its menu where
/// `components::place_menu` puts it: so the board shows the real flip, not a hand-drawn one.
fn at_edge(select_at: (f32, f32), searchable: bool) -> Node {
    let items = if searchable { themes("") } else { chosen() };
    let edge_menu = || {
        let menu = components::menu(&items)
            .hovered(Some(ID))
            .min_width(SELECT_WIDTH);
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
    let (x, y) =
        components::place_menu(anchor, (width, height), EDGE_BOARD, SELECT_GAP, EDGE_MARGIN);
    let select = div()
        .pl(anchor.x)
        .child(components::select(ID + 10, "Pomelo Dark").width(SELECT_WIDTH));
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

const WIDE: (f32, f32) = (360.0, 120.0);

fn on_board(width: f32, node: impl Into<Node>) -> Node {
    div().col().w_px(width).p(INSET).child(node).into()
}

fn field(hint: Option<&str>, error: Option<&str>, text: &str) -> Node {
    let mut input = InputField::new("Branch", "feat-login");
    input.field.set_text(text);
    input.field.move_to_end();
    on_board(WIDE.0, input.render(ID, false, hint, error))
}

fn search_field(text: &str, focused: bool) -> Node {
    let mut query = TextField::default();
    query.set_text(text);
    query.move_to_end();
    let mut box_ = components::search_box(ID, focused).w_px(240.0).child(
        div().row().flex(1.0).items_center().child(query.render(
            "Filter by name",
            focused,
            theme().text,
            20.0,
            FieldFont::Ui,
        )),
    );
    if !text.is_empty() {
        box_ = box_.child(
            components::icon_button(ID + 1, IconKind::Close).size(components::ButtonSize::XSmall),
        );
    }
    on_board(WIDE.0, box_)
}

fn list(rows: &[(&str, IconKind)], selected: usize, hovered: Option<u64>) -> Node {
    let mut column = div()
        .col()
        .w_px(280.0)
        .py(4.0)
        .rounded(8.0)
        .bg(theme().elevated_surface_background);
    for (index, (text, kind)) in rows.iter().enumerate() {
        let id = ID + index as u64;
        column = column.child(
            components::list_row(id)
                .selected(index == selected)
                .hovered(hovered)
                .child(ui::icon(*kind).size(14.0).color(theme().icon_muted))
                .child(label(text.to_string()).size(13.0).color(theme().text)),
        );
    }
    div().p(INSET).child(column).into()
}

const ROWS: [(&str, IconKind); 3] = [
    ("api/src/server.rs", IconKind::File),
    ("web/src/pages/Login.tsx", IconKind::File),
    ("docs", IconKind::Folder),
];

fn toast_board(action: Option<&str>) -> Node {
    let (node, width, _) = components::toast(
        "Pulled feat-login in 3 repos",
        action.map(|text| (text, ID)),
        ID + 1,
    );
    div().p(INSET).child(div().w_px(width).child(node)).into()
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
        story("field-hint", "Field", "With a hint", (360.0, 112.0), || {
            still(|_, _| field(Some("A new worktree for every repo"), None, ""))
        }),
        story(
            "field-error",
            "Field",
            "With an error",
            (360.0, 130.0),
            || still(|_, _| field(None, Some("A branch name has no spaces"), "feat login")),
        ),
        story(
            "search-field-empty",
            "Search field",
            "Empty",
            (360.0, 60.0),
            || still(|_, _| search_field("", false)),
        ),
        story(
            "search-field-typed",
            "Search field",
            "Typed",
            (360.0, 60.0),
            || still(|_, _| search_field("api", true)),
        ),
        story(
            "section-header-count",
            "Section header",
            "With a count",
            (320.0, 48.0),
            || still(|_, _| on_board(320.0, components::section_header("This workspace").count(3))),
        ),
        story(
            "section-header-action",
            "Section header",
            "With an action",
            (320.0, 56.0),
            || {
                still(|_, _| {
                    on_board(
                        320.0,
                        components::section_header("Shared - all workspaces").trailing(
                            components::icon_button(ID, IconKind::Plus)
                                .size(components::ButtonSize::XSmall),
                        ),
                    )
                })
            },
        ),
        story(
            "list-row-selected",
            "List row",
            "Selected",
            (312.0, 128.0),
            || still(|_, _| list(&ROWS, 0, None)),
        ),
        story(
            "list-row-hovered",
            "List row",
            "One hovered",
            (312.0, 128.0),
            || still(|_, _| list(&ROWS, 0, Some(ID + 1))),
        ),
        story(
            "banner-warning",
            "Banner",
            "Warning with its fix",
            (520.0, 80.0),
            || {
                still(|_, _| {
                    on_board(
                        520.0,
                        components::banner(BannerTone::Warning, "Docker is not running")
                            .action(components::button(ID, "Start Docker")),
                    )
                })
            },
        ),
        story(
            "banner-commands",
            "Banner",
            "With the commands it runs",
            (520.0, 136.0),
            || {
                still(|_, _| {
                    on_board(
                        520.0,
                        components::banner(BannerTone::Error, "pom on dev is older than this app")
                            .action(components::button(ID, "Update pom").kind(ButtonKind::Primary))
                            .commands(vec!["pom self-update".into(), "pom server restart".into()]),
                    )
                })
            },
        ),
        story("banner-busy", "Banner", "Busy", (520.0, 80.0), || {
            still(|_, _| {
                on_board(
                    520.0,
                    components::banner(BannerTone::Busy, "Updating pom on dev..."),
                )
            })
        }),
        story("toast", "Toast", "Message", (420.0, 68.0), || {
            still(|_, _| toast_board(None))
        }),
        story(
            "toast-action",
            "Toast",
            "With an action",
            (420.0, 68.0),
            || still(|_, _| toast_board(Some("Switch"))),
        ),
        story(
            "menu-context",
            "Menu",
            "Right click on a file",
            card_size(&context_menu(), false),
            || still(|_, _| card(&context_menu(), Some(CONTEXT_HOVERED), false)),
        ),
        story(
            "menu-search-typed",
            "Menu",
            "A long list, filtered",
            searchable_size("dark"),
            || still(|_, _| searchable("dark", Some(ID + 2))),
        ),
        story(
            "menu-submenu-open",
            "Menu",
            "A submenu open",
            submenu_open_size(),
            || still(|_, _| submenu_open()),
        ),
        story(
            "select-open",
            "Select menu",
            "Opens below",
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
            "Near the bottom: opens above",
            EDGE_BOARD,
            || still(|_, _| at_edge((INSET, EDGE_BOARD.1 - EDGE_MARGIN - SELECT_HEIGHT), true)),
        ),
        story("card-plain", "Card", "Plain", (320.0, 120.0), || {
            still(|_, _| card_board(components::card()))
        }),
        story("card-hovered", "Card", "Hovered", (320.0, 120.0), || {
            still(|_, _| card_board(components::card().hovered(true)))
        }),
        story("card-chosen", "Card", "Chosen", (320.0, 120.0), || {
            still(|_, _| card_board(components::card().chosen(true)))
        }),
        story(
            "card-tint",
            "Card",
            "About a problem",
            (320.0, 120.0),
            || still(|_, _| card_board(components::card().tint(theme().error))),
        ),
        story(
            "card-raised",
            "Card",
            "Raised, over the screen",
            (320.0, 120.0),
            || still(|_, _| card_board(components::card().raised())),
        ),
        story(
            "page-header-full",
            "Page header",
            "A ticket",
            (560.0, 150.0),
            || {
                still(|_, _| {
                    let colors = theme();
                    div()
                        .col()
                        .w_px(560.0)
                        .p(INSET)
                        .child(
                            components::page_header(components::page_title(
                                "Let people sign in with a magic link",
                                528.0,
                            ))
                            .lead(label("PROJ-101").size(13.0).mono().color(colors.text_muted))
                            .lead(components::pill("In progress").color(colors.text_accent))
                            .action(components::button(ID, "Copy Link").icon(Some(IconKind::Copy)))
                            .action(
                                components::button(ID + 1, "Reload").icon(Some(IconKind::RotateCw)),
                            )
                            .meta(label("Assigned to you").size(12.0).color(colors.text_muted)),
                        )
                        .into()
                })
            },
        ),
        story(
            "page-header-compact",
            "Page header",
            "Compact, one line",
            (560.0, 76.0),
            || {
                still(|_, _| {
                    let colors = theme();
                    div()
                        .col()
                        .w_px(560.0)
                        .px(INSET)
                        .child(
                            components::page_header(
                                div()
                                    .row()
                                    .gap(8.0)
                                    .items_center()
                                    .child(components::dot(colors.success, 7.0))
                                    .child(label("api").size(14.0).color(colors.text_muted))
                                    .child(label(">").size(14.0).color(colors.text_placeholder))
                                    .child(label("web").size(14.0).medium().color(colors.text)),
                            )
                            .compact()
                            .action(components::button(ID, "Stop").icon(Some(IconKind::Stop)))
                            .action(components::icon_button(ID + 1, IconKind::Ellipsis)),
                        )
                        .into()
                })
            },
        ),
        story(
            "dock-header",
            "Dock header",
            "A docked panel",
            (280.0, 68.0),
            || {
                still(|_, _| {
                    div()
                        .col()
                        .w_px(280.0)
                        .child(components::dock_header(
                            "Files",
                            Some((IconKind::Search, ID)),
                        ))
                        .into()
                })
            },
        ),
        story(
            "banner-detail",
            "Banner",
            "With a second line, flush",
            (420.0, 80.0),
            || {
                still(|_, _| {
                    div()
                        .col()
                        .w_px(420.0)
                        .child(
                            components::banner(BannerTone::Warning, "Reconnecting to dev...")
                                .detail("Edits stay here; saves wait for it.")
                                .flush(),
                        )
                        .into()
                })
            },
        ),
        story(
            "banner-wrap",
            "Banner",
            "A message that wraps",
            (420.0, 110.0),
            || {
                still(|_, _| {
                    div()
                    .col()
                    .w_px(420.0)
                    .p(INSET)
                    .child(
                        components::banner(
                            BannerTone::Info,
                            "Jira answered 401. Sign in again under Settings > Integrations, then reload.",
                        )
                        .wrap(320.0),
                    )
                    .into()
                })
            },
        ),
        story(
            "banner-decision",
            "Banner",
            "Waiting on a decision",
            (620.0, 80.0),
            || {
                still(|_, _| {
                    div()
                        .col()
                        .w_px(620.0)
                        .p(INSET)
                        .child(
                            components::banner(BannerTone::Warning, "rm -rf target/debug")
                                .lead(components::pill("Waiting for approval").attention())
                                .mono()
                                .action(
                                    components::button(ID, "Allow")
                                        .kind(components::ButtonKind::Primary),
                                )
                                .action(
                                    components::button(ID + 1, "Deny")
                                        .kind(components::ButtonKind::Ghost),
                                ),
                        )
                        .into()
                })
            },
        ),
        story(
            "banner-watching",
            "Banner",
            "Its own icon",
            (520.0, 80.0),
            || {
                still(|_, _| {
                    div()
                        .col()
                        .w_px(520.0)
                        .p(INSET)
                        .child(
                            components::banner(
                                BannerTone::Info,
                                "Watching - an orchestrator drives this session",
                            )
                            .icon(IconKind::Eye)
                            .action(
                                components::button(ID, "Take over")
                                    .kind(components::ButtonKind::Ghost),
                            ),
                        )
                        .into()
                })
            },
        ),
        story(
            "empty-state",
            "Empty state",
            "A pane with nothing yet",
            (420.0, 220.0),
            || {
                still(|_, _| {
                    div()
                    .col()
                    .w_px(420.0)
                    .h_px(220.0)
                    .child(
                        components::empty_state("No ports forwarded")
                            .icon(IconKind::ArrowUpRight)
                            .body(
                                "Shared services of feat-login are forwarded once they run. Forward any other port of dev above.",
                                320.0,
                            ),
                    )
                    .into()
                })
            },
        ),
        story(
            "empty-state-action",
            "Empty state",
            "With what to do",
            (420.0, 180.0),
            || {
                still(|_, _| {
                    div()
                        .col()
                        .w_px(420.0)
                        .h_px(180.0)
                        .child(
                            components::empty_state("No errors in workspace")
                                .action(components::button(ID, "Show 3 warnings")),
                        )
                        .into()
                })
            },
        ),
        story(
            "empty-state-compact",
            "Empty state",
            "Compact, in a list",
            (320.0, 60.0),
            || {
                still(|_, _| {
                    div()
                        .col()
                        .w_px(320.0)
                        .p(INSET)
                        .child(components::empty_state("No matches").compact())
                        .into()
                })
            },
        ),
        story("modal", "Modal", "A form", (480.0, 220.0), || {
            still(|_, _| {
                div()
                    .col()
                    .p(INSET)
                    .child(
                        components::modal_frame(440.0)
                            .child(components::modal_header("Rename Workspace", Some(ID)))
                            .child(
                                components::modal_section(8.0)
                                    .child(ui::label("feat-login").size(13.0).color(theme().text)),
                            )
                            .child(components::modal_footer(
                                Some(components::modal_note("The branch keeps its name.")),
                                div()
                                    .row()
                                    .gap(8.0)
                                    .child(
                                        components::button(ID + 1, "Cancel")
                                            .size(components::ButtonSize::Medium),
                                    )
                                    .child(
                                        components::button(ID + 2, "Rename")
                                            .kind(components::ButtonKind::Primary)
                                            .size(components::ButtonSize::Medium),
                                    )
                                    .into(),
                            )),
                    )
                    .into()
            })
        }),
    ]
}

/// A card at a typical size with a title and a line, so its fill and edge read against the page.
fn card_board(card: components::Card) -> Node {
    let colors = theme();
    div()
        .col()
        .w_px(320.0)
        .w_px(320.0)
        .p(INSET)
        .child(
            card.render()
                .gap(4.0)
                .child(label("web crashed").medium().color(colors.text))
                .child(
                    label("Stopped with exit code 1.")
                        .size(12.0)
                        .color(colors.text_muted),
                ),
        )
        .into()
}
