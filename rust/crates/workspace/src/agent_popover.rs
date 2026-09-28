//! The agent dock's popovers: the "+" one that starts a side agent (what it is for, what it starts with),
//! and the history of archived side agents to reopen.

use ui::{div, icon, label, theme, IconKind, Node, Rgba};

use crate::{SideAgentRole, SideAgentStart};

pub const POPOVER_BASE: u64 = 1800;
pub const ROLE_BASE: u64 = POPOVER_BASE;
pub const START_BASE: u64 = POPOVER_BASE + 10;
pub const START_BUTTON: u64 = POPOVER_BASE + 20;
pub const NEW_WORKSPACE: u64 = POPOVER_BASE + 21;
pub const HISTORY_BASE: u64 = POPOVER_BASE + 30;
pub const POPOVER_END: u64 = POPOVER_BASE + 100;
pub const WIDTH: f32 = 380.0;

/// Above this many tokens Auto compacts inside the fork.
const COMPACT_ABOVE: usize = 50_000;

/// A side agent that is no longer open, as the history lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchivedAgent {
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentPopoverKind {
    New {
        role: SideAgentRole,
        start: SideAgentStart,
    },
    History,
}

/// "9.3k", "62k", "800".
pub fn token_text(tokens: usize) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=9_999 => format!("{:.1}k", tokens as f64 / 1000.0),
        _ => format!("{}k", tokens / 1000),
    }
}

fn role_icon(role: SideAgentRole) -> IconKind {
    match role {
        SideAgentRole::Ask => IconKind::HelpCircle,
        SideAgentRole::Review => IconKind::Search,
        SideAgentRole::SecondOpinion => IconKind::Messages,
        SideAgentRole::Fix => IconKind::Wrench,
    }
}

fn role_note(role: SideAgentRole) -> &'static str {
    match role {
        SideAgentRole::Ask => "Why, how or where - about the code or the plan",
        SideAgentRole::Review => "Read the branch diff and report bugs",
        SideAgentRole::SecondOpinion => "Same question to another CLI",
        SideAgentRole::Fix => "Fix a bug or a failing check",
    }
}

fn tag(text: &str, color: Rgba, border: Rgba) -> Node {
    div()
        .row()
        .h_px(20.0)
        .px(6.0)
        .items_center()
        .rounded(4.0)
        .border(1.0, border)
        .child(label(text.to_string()).size(12.0).color(color))
        .into()
}

fn hot(hover: Option<u64>, id: u64) -> bool {
    hover == Some(id)
}

/// The "+" popover. `sizes` are the tokens each start begins with (Auto, Fork, Compacted, Fresh);
/// `main_tokens` the main session's size; `other_cli` whether a second CLI is installed.
#[allow(clippy::too_many_arguments)]
pub fn new_agent(
    branch: &str,
    role: SideAgentRole,
    start: SideAgentStart,
    sizes: [Option<usize>; 4],
    main_tokens: Option<usize>,
    other_cli: Option<&str>,
    hover: Option<u64>,
) -> Node {
    let colors = theme();
    let inner = WIDTH - 24.0;
    let mut card = div()
        .col()
        .w_px(WIDTH)
        .p(4.0)
        .rounded(8.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border)
        .child(
            div().row().h_px(30.0).px(10.0).items_center().child(
                label(format!("New side agent in {branch}"))
                    .size(12.5)
                    .color(colors.text_muted),
            ),
        );
    for (index, each) in SideAgentRole::ALL.into_iter().enumerate() {
        let id = ROLE_BASE + index as u64;
        let unavailable = each == SideAgentRole::SecondOpinion && other_cli.is_none();
        let note = if unavailable {
            "Needs codex or gemini on PATH".to_string()
        } else if each == SideAgentRole::SecondOpinion {
            format!("Same question to {}", other_cli.unwrap_or_default())
        } else {
            role_note(each).to_string()
        };
        let (tag_text, tag_color, tag_border) = if each == SideAgentRole::Fix {
            ("can edit", colors.warning, colors.warning.alpha(0.5))
        } else {
            ("read-only", colors.text_muted, colors.border_variant)
        };
        let text_color = if unavailable {
            colors.text_disabled
        } else {
            colors.text
        };
        let mut row = div()
            .row()
            .px(10.0)
            .py(6.0)
            .gap(10.0)
            .items_center()
            .rounded(6.0)
            .child(icon(role_icon(each)).size(15.0).color(colors.icon_muted))
            .child(
                div()
                    .col()
                    .w_px(WIDTH - 165.0)
                    .gap(1.0)
                    .child(label(each.title()).size(14.0).color(text_color))
                    .child(label(note).size(12.5).color(colors.text_muted).truncate()),
            )
            .child(div().flex(1.0))
            .child(tag(tag_text, tag_color, tag_border));
        if !unavailable {
            row = row.on_click(id);
        }
        if each == role {
            row = row.bg(colors.element_selected);
        } else if hot(hover, id) {
            row = row.bg(colors.ghost_element_hover);
        }
        card = card.child(row);
    }
    card = card.child(separator());
    let main = match main_tokens {
        Some(tokens) => format!("main: {} tokens", token_text(tokens)),
        None => "no main session yet".to_string(),
    };
    card = card.child(
        div()
            .row()
            .h_px(28.0)
            .px(10.0)
            .items_center()
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .child(label("Starts with").size(12.5).color(colors.text_muted)),
            )
            .child(label(main).size(12.5).color(colors.text_muted)),
    );
    let packet_only = role == SideAgentRole::SecondOpinion;
    for (index, each) in SideAgentStart::ALL.into_iter().enumerate() {
        let id = START_BASE + index as u64;
        let size = sizes.get(index).copied().flatten();
        let needs_main = matches!(each, SideAgentStart::Fork | SideAgentStart::Compacted);
        let disabled =
            (needs_main && main_tokens.is_none()) || (packet_only && each != SideAgentStart::Fresh);
        let note = match each {
            SideAgentStart::Auto => match main_tokens {
                None => "No main session yet: starts fresh.".to_string(),
                Some(tokens) if tokens > COMPACT_ABOVE => format!(
                    "Main is {}, over 50k: fork it, then compact inside the fork.",
                    token_text(tokens)
                ),
                Some(tokens) => format!("Main is {}: fork the whole session.", token_text(tokens)),
            },
            SideAgentStart::Fork => {
                "A copy of main's whole session. Most detail; reuses main's prompt cache.".into()
            }
            SideAgentStart::Compacted => {
                "Fork, then /compact inside the fork. Main itself is never compacted.".into()
            }
            SideAgentStart::Fresh => {
                "No history. Pomelo writes a packet: branch, ticket, recent commits, changes."
                    .into()
            }
        };
        let note = if packet_only && each != SideAgentStart::Fresh {
            "Another CLI cannot read Claude's session; it gets a Pomelo packet.".to_string()
        } else {
            note
        };
        let chosen = each == start && !disabled;
        let radio = div()
            .row()
            .w_px(16.0)
            .h_px(16.0)
            .items_center()
            .justify_center()
            .rounded(8.0)
            .border(
                1.5,
                if chosen {
                    colors.text_accent
                } else {
                    colors.border
                },
            )
            .child(if chosen {
                Node::from(
                    div()
                        .w_px(8.0)
                        .h_px(8.0)
                        .rounded(4.0)
                        .bg(colors.text_accent),
                )
            } else {
                Node::from(div())
            });
        let text_color = if disabled {
            colors.text_disabled
        } else {
            colors.text
        };
        let mut row = div()
            .row()
            .px(10.0)
            .py(6.0)
            .gap(10.0)
            .rounded(6.0)
            .child(div().col().pt(2.0).child(radio))
            .child(
                div()
                    .col()
                    .flex(1.0)
                    .gap(2.0)
                    .child(
                        div()
                            .row()
                            .items_center()
                            .child(
                                div()
                                    .row()
                                    .flex(1.0)
                                    .child(label(each.title()).size(14.0).color(text_color)),
                            )
                            .child(
                                label(
                                    size.filter(|_| !disabled)
                                        .map(|tokens| format!("~{}", token_text(tokens)))
                                        .unwrap_or_default(),
                                )
                                .size(13.0)
                                .mono()
                                .color(colors.text_muted),
                            ),
                    )
                    .child(
                        label(note)
                            .size(12.5)
                            .color(colors.text_muted)
                            .wrap(inner - 30.0),
                    ),
            );
        if !disabled {
            row = row.on_click(id);
            if hot(hover, id) {
                row = row.bg(colors.ghost_element_hover);
            }
        }
        card = card.child(row);
    }
    let start_size = sizes
        .get(
            SideAgentStart::ALL
                .iter()
                .position(|each| *each == start)
                .unwrap_or(0),
        )
        .copied()
        .flatten();
    let start_text = match start_size {
        Some(tokens) => format!("Start {} - ~{} tokens", role.title(), token_text(tokens)),
        None => format!("Start {}", role.title()),
    };
    card = card
        .child(
            div()
                .row()
                .h_px(40.0)
                .px(10.0)
                .items_center()
                .child(
                    div().row().flex(1.0).child(
                        label("Double-click a type to start")
                            .size(12.5)
                            .color(colors.text_muted),
                    ),
                )
                .child(
                    div()
                        .row()
                        .h_px(28.0)
                        .px(12.0)
                        .items_center()
                        .rounded(5.0)
                        .bg(if hot(hover, START_BUTTON) {
                            colors.info_border.alpha(0.4)
                        } else {
                            colors.info_background
                        })
                        .border(1.0, colors.info_border)
                        .on_click(START_BUTTON)
                        .child(label(start_text).size(13.0).color(colors.text)),
                ),
        )
        .child(separator());
    let mut workspace = div()
        .row()
        .px(10.0)
        .py(6.0)
        .gap(10.0)
        .items_center()
        .rounded(6.0)
        .on_click(NEW_WORKSPACE)
        .child(
            icon(IconKind::SquarePlus)
                .size(15.0)
                .color(colors.icon_muted),
        )
        .child(
            div()
                .col()
                .flex(1.0)
                .gap(1.0)
                .child(
                    label("New agent in a new workspace")
                        .size(14.0)
                        .color(colors.text),
                )
                .child(
                    label("For parallel code changes: its own branch, ports and database")
                        .size(12.5)
                        .color(colors.text_muted)
                        .wrap(inner - 30.0),
                ),
        );
    if hot(hover, NEW_WORKSPACE) {
        workspace = workspace.bg(colors.ghost_element_hover);
    }
    card.child(workspace).into()
}

/// The history popover: side agents that were closed, newest first, to reopen with their transcript.
pub fn history(archived: &[ArchivedAgent], hover: Option<u64>) -> Node {
    let colors = theme();
    let mut card = div()
        .col()
        .w_px(WIDTH - 40.0)
        .p(4.0)
        .rounded(8.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border)
        .child(
            div()
                .row()
                .h_px(30.0)
                .px(10.0)
                .items_center()
                .child(
                    div().row().flex(1.0).child(
                        label("Archived side agents")
                            .size(12.5)
                            .color(colors.text_muted),
                    ),
                )
                .child(
                    label(archived.len().to_string())
                        .size(12.5)
                        .color(colors.text_muted),
                ),
        );
    if archived.is_empty() {
        return card
            .child(
                div().row().px(10.0).pb(10.0).child(
                    label(
                        "Nothing archived. Closing a side agent stops its CLI and keeps the \
                         transcript here, so you can reopen it.",
                    )
                    .size(12.5)
                    .color(colors.text_muted)
                    .wrap(WIDTH - 64.0),
                ),
            )
            .into();
    }
    for (index, agent) in archived.iter().enumerate().take(60) {
        let id = HISTORY_BASE + index as u64;
        let mut row = div()
            .col()
            .px(10.0)
            .py(6.0)
            .gap(1.0)
            .rounded(6.0)
            .on_click(id)
            .child(
                label(agent.title.clone())
                    .size(13.5)
                    .color(colors.text)
                    .truncate(),
            )
            .child(
                label(agent.detail.clone())
                    .size(12.0)
                    .color(colors.text_muted)
                    .truncate(),
            );
        if hot(hover, id) {
            row = row.bg(colors.ghost_element_hover);
        }
        card = card.child(row);
    }
    card.into()
}

fn separator() -> Node {
    div()
        .py(4.0)
        .child(div().w_px(WIDTH - 8.0).h_px(1.0).bg(theme().border_variant))
        .into()
}
