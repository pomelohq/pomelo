use ui::{div, icon, label, theme, IconKind, LabelSize, Node};

use crate::Session;

pub const WELCOME_OPEN_PROJECT: u64 = 600;
pub const WELCOME_OPEN_SETTINGS: u64 = 601;
/// A recent session button: id = base + index into `Layout::sessions`.
pub const WELCOME_RECENT_BASE: u64 = 1000;
pub const WELCOME_RECENT_MAX: usize = 5;
const WELCOME_RECENT_END: u64 = 1100;

const CONTENT_MAX_W: f32 = 512.0;
const CONTENT_PAD: f32 = 32.0;
const SECTION_GAP: f32 = 24.0;
const HEADLINE_SIZE: f32 = 18.0;
const BUTTON_H: f32 = 28.0;

pub fn is_welcome_id(id: u64) -> bool {
    id == WELCOME_OPEN_PROJECT
        || id == WELCOME_OPEN_SETTINGS
        || (WELCOME_RECENT_BASE..WELCOME_RECENT_END).contains(&id)
}

/// The page shown in the editor area while no project is open: getting-started actions and the
/// most recently used sessions.
pub fn welcome_page(sessions: &[Session], area_w: f32, hovered: Option<u64>) -> Node {
    let content_w = (area_w - 2.0 * CONTENT_PAD).clamp(0.0, CONTENT_MAX_W);
    let header = div().row().items_center().justify_center().gap(16.0).child(
        div()
            .col()
            .child(
                label("Welcome to Pomelo")
                    .size(HEADLINE_SIZE)
                    .color(theme().text),
            )
            .child(
                label("Every branch, its own running environment")
                    .label_size(LabelSize::Small)
                    .color(theme().text_muted),
            ),
    );
    let get_started = section("Get Started", content_w).children([
        section_button(
            WELCOME_OPEN_PROJECT,
            IconKind::FolderOpen,
            "Open Project",
            "cmd-o",
            content_w,
            hovered,
        ),
        section_button(
            WELCOME_OPEN_SETTINGS,
            IconKind::Grid,
            "Open Settings",
            "cmd-,",
            content_w,
            hovered,
        ),
    ]);
    let mut column = div()
        .col()
        .w_px(content_w)
        .gap(SECTION_GAP)
        .child(div().py(8.0).child(header))
        .child(get_started);
    let recent: Vec<Node> = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| !session.missing)
        .take(WELCOME_RECENT_MAX)
        .map(|(index, session)| {
            section_button(
                WELCOME_RECENT_BASE + index as u64,
                IconKind::Folder,
                &session.name,
                "",
                content_w,
                hovered,
            )
        })
        .collect();
    if !recent.is_empty() {
        column = column.child(section("Recent Sessions", content_w).children(recent));
    }
    div()
        .col()
        .justify_center()
        .bg(theme().editor_background)
        .child(div().row().justify_center().child(column))
        .into()
}

fn section(title: &str, width: f32) -> ui::Div {
    let header = div()
        .row()
        .items_center()
        .gap(8.0)
        .px(4.0)
        .child(
            label(title.to_ascii_uppercase())
                .label_size(LabelSize::XSmall)
                .mono()
                .color(theme().text_muted),
        )
        .child(ui::divider().flex(1.0));
    div().col().gap(8.0).w_px(width).child(header)
}

fn section_button(
    id: u64,
    kind: IconKind,
    text: &str,
    keystroke: &str,
    width: f32,
    hovered: Option<u64>,
) -> Node {
    let mut button = div()
        .row()
        .items_center()
        .justify_between()
        .w_px(width)
        .h_px(BUTTON_H)
        .px(8.0)
        .rounded(4.0)
        .on_click(id)
        .child(
            div()
                .row()
                .items_center()
                .gap(8.0)
                .child(icon(kind).size(14.0).color(theme().icon_muted))
                .child(
                    label(text)
                        .label_size(LabelSize::Default)
                        .color(theme().text),
                ),
        );
    if !keystroke.is_empty() {
        button = button.child(crate::render_keystroke(keystroke, 12.0));
    }
    if hovered == Some(id) {
        button = button.bg(theme().ghost_element_hover);
    }
    button.into()
}
