use ui::{div, icon, label, theme, IconKind, Node, Rgba};

use crate::Session;

pub const WELCOME_OPEN_PROJECT: u64 = 600;
pub const WELCOME_OPEN_SETTINGS: u64 = 601;
pub const WELCOME_NEW_PROJECT: u64 = 602;
pub const WELCOME_IMPORT_BUNDLE: u64 = 603;
pub const WELCOME_RECHECK: u64 = 604;
pub const WELCOME_NEW_PROJECT_CARD: u64 = 605;
/// A check's fix button: id = base + index into the checks.
pub const WELCOME_FIX_BASE: u64 = 610;
/// A recent session button: id = base + index into `Layout::sessions`.
pub const WELCOME_RECENT_BASE: u64 = 1000;
pub const WELCOME_RECENT_MAX: usize = 5;
const WELCOME_RECENT_END: u64 = 1100;
const WELCOME_FIX_END: u64 = 620;

const CONTENT_MAX_W: f32 = 780.0;
const PAD_X: f32 = 40.0;
const GAP: f32 = 22.0;

/// One thing the Mac needs before a project can run (Docker, git, an agent CLI), as the welcome page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachineCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    /// The button that fixes it, when there is one ("Start Docker").
    pub fix: Option<String>,
}

pub fn is_welcome_id(id: u64) -> bool {
    matches!(
        id,
        WELCOME_OPEN_PROJECT
            | WELCOME_OPEN_SETTINGS
            | WELCOME_NEW_PROJECT
            | WELCOME_IMPORT_BUNDLE
            | WELCOME_RECHECK
            | WELCOME_NEW_PROJECT_CARD
    ) || (WELCOME_RECENT_BASE..WELCOME_RECENT_END).contains(&id)
        || (WELCOME_FIX_BASE..WELCOME_FIX_END).contains(&id)
}

fn button(id: u64, kind: Option<IconKind>, text: &str, primary: bool, keys: Option<&str>) -> Node {
    let colors = theme();
    let (bg, border) = if primary {
        (colors.info_background, colors.info_border)
    } else {
        (colors.element_background, colors.border)
    };
    let mut row = div()
        .row()
        .h_px(28.0)
        .px(12.0)
        .gap(6.0)
        .items_center()
        .rounded(5.0)
        .bg(bg)
        .border(1.0, border)
        .on_click(id);
    if let Some(kind) = kind {
        row = row.child(icon(kind).size(13.0).color(colors.text));
    }
    row = row.child(label(text.to_string()).size(13.0).color(colors.text));
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

fn section(title: &str, trailing: Option<Node>, body: Node) -> Node {
    let mut head = div().row().items_center().child(
        div().row().flex(1.0).child(
            label(title.to_uppercase())
                .size(11.0)
                .weight(600)
                .color(theme().text_placeholder),
        ),
    );
    if let Some(trailing) = trailing {
        head = head.child(trailing);
    }
    div().col().gap(10.0).child(head).child(body).into()
}

fn rows(items: Vec<Node>) -> Node {
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

fn row() -> ui::Div {
    div().row().items_center().gap(10.0).px(12.0).py(9.0)
}

fn card(id: u64, kind: IconKind, title: &str, text: &str, width: f32, hot: bool) -> Node {
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
                .size(12.5)
                .color(colors.text_muted)
                .wrap(width - 28.0),
        )
        .into()
}

/// The page shown in the editor area while no project is open: ways to start one, the recent sessions, and
/// whether this Mac has what a project needs.
pub fn welcome_page(
    sessions: &[Session],
    checks: &[MachineCheck],
    area_w: f32,
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let width = (area_w - 2.0 * PAD_X).clamp(200.0, CONTENT_MAX_W);
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
    let header = div()
        .row()
        .items_center()
        .gap(16.0)
        .child(logo)
        .child(
            div()
                .col()
                .flex(1.0)
                .gap(2.0)
                .child(
                    label("Welcome to Pomelo")
                        .size(20.0)
                        .weight(600)
                        .color(colors.text),
                )
                .child(
                    label("Every branch a full, running dev environment")
                        .size(12.5)
                        .italic()
                        .color(colors.text_muted),
                ),
        )
        .child(button(
            WELCOME_NEW_PROJECT,
            Some(IconKind::Plus),
            "New project",
            true,
            Some("cmd-shift-n"),
        ));
    let card_w = (width - 20.0) / 3.0;
    let cards = div()
        .row()
        .gap(10.0)
        .child(card(
            WELCOME_NEW_PROJECT_CARD,
            IconKind::Plus,
            "New project",
            "Point at your repos (folders or git URLs). Pomelo clones them, detects the stack and writes pom.yml.",
            card_w,
            hovered == Some(WELCOME_NEW_PROJECT_CARD),
        ))
        .child(card(
            WELCOME_OPEN_PROJECT,
            IconKind::FolderOpen,
            "Open a project folder",
            "A folder that already has a pom.yml, from before or from a teammate.",
            card_w,
            hovered == Some(WELCOME_OPEN_PROJECT),
        ))
        .child(card(
            WELCOME_IMPORT_BUNDLE,
            IconKind::Package,
            "Import a bundle",
            "A teammate's exported project: config, and secrets if they shared them.",
            card_w,
            hovered == Some(WELCOME_IMPORT_BUNDLE),
        ));
    let recent: Vec<Node> = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| !session.missing)
        .take(WELCOME_RECENT_MAX)
        .map(|(index, session)| {
            let id = WELCOME_RECENT_BASE + index as u64;
            let mut line = row()
                .on_click(id)
                .child(icon(IconKind::Folder).size(14.0).color(colors.icon_muted))
                .child(label(session.name.clone()).size(13.0).color(colors.text))
                .child(div().flex(1.0))
                .child(
                    label(home_relative(&session.path))
                        .size(12.0)
                        .mono()
                        .color(colors.text_placeholder)
                        .truncate_start(),
                );
            if hovered == Some(id) {
                line = line.bg(colors.ghost_element_hover);
            }
            line.into()
        })
        .collect();
    let recent = if recent.is_empty() {
        rows(vec![row()
            .child(
                label("No projects yet. Created and opened projects show here.")
                    .size(13.0)
                    .color(colors.text_placeholder),
            )
            .into()])
    } else {
        rows(recent)
    };
    let mut column = div()
        .col()
        .w_px(width)
        .gap(GAP)
        .child(header)
        .child(div().h_px(1.0).bg(colors.border_variant))
        .child(section("Get started", None, cards.into()))
        .child(section("Recent", None, recent));
    if !checks.is_empty() {
        let lines = checks
            .iter()
            .enumerate()
            .map(|(index, check)| {
                let mut line = row()
                    .child(div().w_px(8.0).h_px(8.0).rounded(4.0).bg(if check.ok {
                        colors.success
                    } else {
                        colors.error
                    }))
                    .child(
                        div()
                            .w_px(110.0)
                            .child(label(check.name.clone()).size(13.0).color(colors.text)),
                    )
                    .child(
                        div().row().flex(1.0).child(
                            label(check.detail.clone())
                                .size(12.0)
                                .color(if check.ok {
                                    colors.text_muted
                                } else {
                                    colors.error
                                })
                                .truncate(),
                        ),
                    );
                if let Some(fix) = &check.fix {
                    line = line.child(button(
                        WELCOME_FIX_BASE + index as u64,
                        None,
                        fix,
                        false,
                        None,
                    ));
                }
                line.into()
            })
            .collect();
        let again = div()
            .row()
            .h_px(22.0)
            .px(8.0)
            .items_center()
            .rounded(4.0)
            .bg(if hovered == Some(WELCOME_RECHECK) {
                colors.element_hover
            } else {
                Rgba::TRANSPARENT
            })
            .on_click(WELCOME_RECHECK)
            .child(label("Check again").size(12.0).color(colors.text_muted));
        column = column.child(section("This Mac", Some(again.into()), rows(lines)));
    }
    div()
        .col()
        .pt(44.0)
        .pb(60.0)
        .items_center()
        .bg(colors.editor_background)
        .child(column)
        .into()
}

fn home_relative(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&format!("{home}/")) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}
