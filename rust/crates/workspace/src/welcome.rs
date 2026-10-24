use ui::{div, icon, label, theme, IconKind, Node};

use crate::Session;

pub const WELCOME_OPEN_PROJECT: u64 = 600;
pub const WELCOME_OPEN_SETTINGS: u64 = 601;
pub const WELCOME_NEW_PROJECT: u64 = 602;
pub const WELCOME_IMPORT_BUNDLE: u64 = 603;
pub const WELCOME_RECHECK: u64 = 604;
pub const WELCOME_NEW_PROJECT_CARD: u64 = 605;
pub const WELCOME_CONNECT_HOST: u64 = 606;
/// A button of the page shown in place of the editor (a remote project's): id = base + index.
pub const NOTICE_ACTION_BASE: u64 = 660;
pub const NOTICE_ACTION_END: u64 = 670;
/// A check's fix button: id = base + index into the checks.
pub const WELCOME_FIX_BASE: u64 = 610;
/// A recent session button: id = base + index into `Layout::sessions`.
pub const WELCOME_RECENT_BASE: u64 = 1000;
pub const WELCOME_RECENT_MAX: usize = 5;
const WELCOME_RECENT_END: u64 = 1100;
const WELCOME_FIX_END: u64 = 620;

const CONTENT_MAX_W: f32 = 640.0;
const PAD_X: f32 = 40.0;
const GAP: f32 = 24.0;

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
            | WELCOME_CONNECT_HOST
    ) || (WELCOME_RECENT_BASE..WELCOME_RECENT_END).contains(&id)
        || (WELCOME_FIX_BASE..WELCOME_FIX_END).contains(&id)
}

const LOGO_IMAGE: u64 = 0x0057_454c_434f_4d45;
const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// The app's mark, decoded once and kept registered for the life of the process.
fn logo() -> Option<u64> {
    static DECODED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let ready = *DECODED.get_or_init(|| {
        let Ok(decoded) = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png)
        else {
            return false;
        };
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        ui::retain_image(LOGO_IMAGE);
        ui::set_image(LOGO_IMAGE, width, height, rgba.into_raw());
        true
    });
    ready.then_some(LOGO_IMAGE)
}

/// A way to start: its icon, name and what it does, lit while the pointer is over it.
fn action_card(id: u64, kind: IconKind, title: &str, text: &str, width: f32, hot: bool) -> Node {
    let colors = theme();
    let fill = if hot {
        colors.element_hover
    } else {
        colors.element_background
    };
    div()
        .col()
        .w_px(width)
        .p(14.0)
        .gap(6.0)
        .rounded(10.0)
        .bg(fill)
        .on_click(id)
        .child(
            div()
                .pb(4.0)
                .child(icon(kind).size(18.0).color(colors.text_accent)),
        )
        .child(
            label(title.to_string())
                .size(13.0)
                .medium()
                .color(colors.text),
        )
        .child(
            label(text.to_string())
                .size(12.0)
                .color(colors.text_muted)
                .wrap(width - 28.0),
        )
        .into()
}

/// A check this computer fails, with its fix and a way to look again.
fn banner(index: usize, check: &MachineCheck, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut line = div()
        .row()
        .items_center()
        .gap(10.0)
        .pt(10.0)
        .pb(10.0)
        .pl(14.0)
        .pr(10.0)
        .rounded(10.0)
        .bg(colors.element_background)
        .child(icon(IconKind::Warning).size(16.0).color(colors.warning))
        .child(
            div().row().flex(1.0).child(
                label(check.detail.clone())
                    .size(12.0)
                    .color(colors.text_muted)
                    .truncate(),
            ),
        );
    if let Some(fix) = &check.fix {
        line = line.child(crate::button(WELCOME_FIX_BASE + index as u64, fix).hovered(hovered));
    } else {
        line = line.child(
            crate::button(WELCOME_RECHECK, "Check again")
                .kind(ui::ButtonKind::Ghost)
                .hovered(hovered),
        );
    }
    line.into()
}

/// The page shown in the editor area while no project is open: who it is, what this computer still needs,
/// ways to start a project and the recent ones.
pub fn welcome_page(
    sessions: &[Session],
    checks: &[MachineCheck],
    area_w: f32,
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let width = (area_w - 2.0 * PAD_X).clamp(200.0, CONTENT_MAX_W);
    let mark: Node = match logo() {
        Some(id) => div().w_px(48.0).h_px(48.0).image(id).into(),
        None => div()
            .w_px(48.0)
            .h_px(48.0)
            .rounded(10.0)
            .bg(colors.text_accent)
            .into(),
    };
    let hero = div().row().items_center().gap(16.0).child(mark).child(
        div()
            .col()
            .gap(4.0)
            .child(
                label("Welcome to Pomelo")
                    .size(20.0)
                    .weight(600)
                    .color(colors.text),
            )
            .child(
                label("Every branch a full, running dev environment")
                    .size(12.0)
                    .color(colors.text_muted),
            ),
    );
    let card_w = (width - 12.0) / 2.0;
    let first_row = div()
        .row()
        .gap(12.0)
        .child(action_card(
            WELCOME_NEW_PROJECT_CARD,
            IconKind::Plus,
            "New project",
            "Repos in, a runnable environment for every branch out",
            card_w,
            hovered == Some(WELCOME_NEW_PROJECT_CARD),
        ))
        .child(action_card(
            WELCOME_OPEN_PROJECT,
            IconKind::FolderOpen,
            "Open a project folder",
            "A folder that already has a pom.yml, from before or from a teammate.",
            card_w,
            hovered == Some(WELCOME_OPEN_PROJECT),
        ));
    let second_row = div()
        .row()
        .gap(12.0)
        .child(action_card(
            WELCOME_CONNECT_HOST,
            IconKind::Server,
            "Connect to a host",
            "A project on another machine you reach over SSH, such as a dev VM.",
            card_w,
            hovered == Some(WELCOME_CONNECT_HOST),
        ))
        .child(action_card(
            WELCOME_IMPORT_BUNDLE,
            IconKind::Package,
            "Import a bundle",
            "A .pombundle exported from another project, secrets included.",
            card_w,
            hovered == Some(WELCOME_IMPORT_BUNDLE),
        ));
    let cards = div().col().gap(12.0).child(first_row).child(second_row);
    let recent: Vec<Node> = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| !session.missing)
        .take(WELCOME_RECENT_MAX)
        .map(|(index, session)| {
            let id = WELCOME_RECENT_BASE + index as u64;
            let (kind, place) = match &session.host {
                Some(host) => (
                    IconKind::Server,
                    format!("{host}: {}", session.path.display()),
                ),
                None => (IconKind::Folder, home_relative(&session.path)),
            };
            let mut line = div()
                .row()
                .items_center()
                .gap(12.0)
                .h_px(44.0)
                .px(8.0)
                .rounded(6.0)
                .on_click(id)
                .child(icon(kind).size(16.0).color(colors.icon_muted))
                .child(
                    div()
                        .col()
                        .flex(1.0)
                        .child(
                            label(session.name.clone())
                                .size(13.0)
                                .medium()
                                .color(colors.text),
                        )
                        .child(
                            label(place)
                                .size(12.0)
                                .color(colors.text_placeholder)
                                .truncate_start(),
                        ),
                );
            if hovered == Some(id) {
                line = line.bg(colors.ghost_element_hover);
            }
            line.into()
        })
        .collect();
    let mut head = div()
        .row()
        .items_center()
        .justify_between()
        .px(8.0)
        .pb(6.0)
        .child(
            label("RECENT")
                .size(11.0)
                .weight(600)
                .color(colors.text_placeholder),
        );
    if let Some(keys) = crate::keymap::key_hint(crate::keymap::Action::OpenProject) {
        head = head.child(crate::key_chip(&keys, false));
    }
    let list = if recent.is_empty() {
        div().col().gap(2.0).child(head).child(
            div().row().px(8.0).py(6.0).child(
                label("No projects yet. Created and opened projects show here.")
                    .size(12.0)
                    .color(colors.text_placeholder),
            ),
        )
    } else {
        div().col().gap(2.0).child(head).children(recent)
    };
    let mut column = div().col().w_px(width).gap(GAP).child(hero);
    for (index, check) in checks.iter().enumerate().filter(|(_, check)| !check.ok) {
        column = column.child(banner(index, check, hovered));
    }
    column = column.child(cards).child(list);
    div()
        .col()
        .pt(72.0)
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoticeTone {
    #[default]
    Info,
    Good,
    Warning,
    Error,
}

/// A button of the page in place of the editor; the first primary one reads as the thing to do now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoticeButton {
    pub label: String,
    pub primary: bool,
}

/// Something about the project's machine that needs a hand (its pom is on another version, or updating).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoticeBanner {
    pub tone: NoticeTone,
    pub busy: bool,
    pub text: String,
    /// The index into `CenterNotice::actions` of the button at its end.
    pub action: Option<usize>,
    /// Commands to run by hand, under the text.
    pub commands: Vec<String>,
}

/// A labeled line of the page's facts card: what it is, then a muted detail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoticeFact {
    pub label: String,
    pub value: String,
    pub detail: String,
}

/// A page in place of the editor, for a project whose files are not on this machine: its name and connection,
/// banners, a facts card and buttons. The window draws the project's Services panel on a card under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CenterNotice {
    pub title: String,
    pub status: String,
    pub tone: NoticeTone,
    /// Every button; a click comes back as `NOTICE_ACTION_BASE + index`. The ones no banner holds sit by the
    /// title.
    pub actions: Vec<NoticeButton>,
    pub banners: Vec<NoticeBanner>,
    pub facts: Vec<NoticeFact>,
}

const NOTICE_CARD_RADIUS: f32 = 10.0;
const FACT_LABEL_W: f32 = 88.0;

fn notice_button(notice: &CenterNotice, index: usize, hovered: Option<u64>) -> Option<Node> {
    let button = notice.actions.get(index)?;
    let kind = if button.primary {
        crate::ButtonKind::Primary
    } else {
        crate::ButtonKind::Secondary
    };
    Some(
        crate::button(NOTICE_ACTION_BASE + index as u64, &button.label)
            .kind(kind)
            .hovered(hovered)
            .into(),
    )
}

fn notice_banner(
    notice: &CenterNotice,
    banner: &NoticeBanner,
    width: f32,
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let (kind, tint) = match (banner.tone, banner.busy) {
        (_, true) => (IconKind::LoadCircle, colors.text_accent),
        (NoticeTone::Error, false) => (IconKind::XCircle, colors.error),
        (NoticeTone::Warning, false) => (IconKind::Warning, colors.warning),
        (NoticeTone::Info | NoticeTone::Good, false) => (IconKind::Info, colors.text_accent),
    };
    let mut line = div()
        .row()
        .items_center()
        .gap(10.0)
        .h_px(24.0)
        .child(icon(kind).size(16.0).color(tint))
        .child(
            div().row().flex(1.0).child(
                label(banner.text.clone())
                    .size(12.0)
                    .color(colors.text_muted)
                    .truncate(),
            ),
        );
    if let Some(button) = banner
        .action
        .and_then(|index| notice_button(notice, index, hovered))
    {
        line = line.child(button);
    }
    let mut card = div()
        .col()
        .gap(8.0)
        .pt(10.0)
        .pb(10.0)
        .pl(14.0)
        .pr(10.0)
        .rounded(NOTICE_CARD_RADIUS)
        .bg(colors.element_background)
        .child(line);
    if !banner.commands.is_empty() {
        let mut block = div()
            .col()
            .gap(4.0)
            .px(10.0)
            .py(8.0)
            .rounded(6.0)
            .bg(colors.editor_background);
        for command in &banner.commands {
            block = block.child(
                div().row().h_px(16.0).items_center().child(
                    label(command.clone())
                        .size(12.0)
                        .mono()
                        .color(colors.text)
                        .truncate(),
                ),
            );
        }
        card = card.child(
            div()
                .row()
                .pl(26.0)
                .child(div().col().w_px((width - 50.0).max(40.0)).child(block)),
        );
    }
    card.into()
}

fn notice_facts(facts: &[NoticeFact]) -> Node {
    let colors = theme();
    let mut card = div()
        .col()
        .py(6.0)
        .px(14.0)
        .rounded(NOTICE_CARD_RADIUS)
        .bg(colors.element_background);
    for fact in facts {
        let mut row = div()
            .row()
            .items_center()
            .gap(8.0)
            .h_px(26.0)
            .child(
                div().row().w_px(FACT_LABEL_W).child(
                    label(fact.label.to_uppercase())
                        .size(11.0)
                        .weight(600)
                        .color(colors.text_placeholder),
                ),
            )
            .child(label(fact.value.clone()).size(12.0).color(colors.text));
        if !fact.detail.is_empty() {
            row = row.child(
                div().row().flex(1.0).child(
                    label(fact.detail.clone())
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            );
        }
        card = card.child(row);
    }
    card.into()
}

/// The top of the page in place of the editor, `width` wide: the project and its connection, banners, the
/// facts card. The window draws the Services panel on a card under it, its own gap below.
pub fn notice_page(notice: &CenterNotice, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let dot = match notice.tone {
        NoticeTone::Info => colors.text_accent,
        NoticeTone::Good => colors.success,
        NoticeTone::Warning => colors.warning,
        NoticeTone::Error => colors.error,
    };
    let held: Vec<usize> = notice
        .banners
        .iter()
        .filter_map(|banner| banner.action)
        .collect();
    let mut buttons = div().row().items_center().gap(8.0);
    for index in (0..notice.actions.len()).filter(|index| !held.contains(index)) {
        if let Some(button) = notice_button(notice, index, hovered) {
            buttons = buttons.child(button);
        }
    }
    let tile = div()
        .row()
        .w_px(40.0)
        .h_px(40.0)
        .items_center()
        .justify_center()
        .rounded(NOTICE_CARD_RADIUS)
        .bg(colors.element_background)
        .child(icon(IconKind::Server).size(18.0).color(colors.text_accent));
    let text = div()
        .col()
        .flex(1.0)
        .gap(4.0)
        .child(
            div().row().h_px(20.0).items_center().child(
                label(notice.title.clone())
                    .size(16.0)
                    .weight(600)
                    .color(colors.text)
                    .truncate(),
            ),
        )
        .child(
            div()
                .row()
                .h_px(16.0)
                .items_center()
                .gap(6.0)
                .child(ui::dot(dot, 8.0))
                .child(
                    div().row().flex(1.0).child(
                        label(notice.status.clone())
                            .size(12.0)
                            .color(colors.text_muted)
                            .truncate(),
                    ),
                ),
        );
    let head = div()
        .row()
        .items_center()
        .gap(12.0)
        .child(tile)
        .child(text)
        .child(buttons);
    let mut column = div()
        .col()
        .w_px(width)
        .pt(28.0)
        .pb(16.0)
        .gap(16.0)
        .child(head);
    for banner in &notice.banners {
        column = column.child(notice_banner(notice, banner, width, hovered));
    }
    if !notice.facts.is_empty() {
        column = column.child(notice_facts(&notice.facts));
    }
    column.into()
}
