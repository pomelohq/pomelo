//! The agents' usage where the window shows it: a chip at the right of the title bar (the signed-in
//! account and its 5-hour and weekly limits) with a card under it, and today's total in the status bar.

use ui::{div, icon, label, theme, IconKind, Node, Rgba};

pub const USAGE_CHIP: u64 = 18;
pub const APP_MENU: u64 = 19;
pub const USAGE_STATUS: u64 = 20;
pub const USAGE_OPEN: u64 = 21;
pub const USAGE_REFRESH: u64 = 22;

pub const CARD_WIDTH: f32 = 300.0;
pub const TODAY_WIDTH: f32 = 320.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageAccount {
    pub name: String,
    pub email: String,
    pub plan: String,
    pub organization: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageWindow {
    pub used: f32,
    /// "in 2h 31m", "Sat 17:00".
    pub resets: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageToday {
    /// "$17.25" or "143K": what the status bar shows.
    pub total: String,
    pub sessions: usize,
    /// (workspace, its share of today) in the same unit as `total`.
    pub by_workspace: Vec<(String, String)>,
}

/// What the window shows of the agents' usage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageInfo {
    pub account: Option<UsageAccount>,
    pub session: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
    /// "Updated 12s ago", or why the limits are missing.
    pub note: String,
    pub today: Option<UsageToday>,
    /// A new version is downloading: its number.
    pub updating_to: Option<String>,
}

/// Green, then warning at 70%, error at 90%.
pub fn tone(used: f32) -> Rgba {
    let colors = theme();
    if used >= 90.0 {
        colors.error
    } else if used >= 70.0 {
        colors.warning
    } else {
        colors.success
    }
}

fn meter(used: f32, width: Option<f32>, height: f32) -> Node {
    let fill = (used / 100.0).clamp(0.0, 1.0);
    let track = div()
        .row()
        .h_px(height)
        .rounded(height / 2.0)
        .bg(Rgba::new(1.0, 1.0, 1.0, 0.1));
    let track = match width {
        Some(width) => track.w_px(width),
        None => track.flex(1.0),
    };
    track
        .child(
            div()
                .flex(fill.max(0.0001))
                .h_px(height)
                .rounded(height / 2.0)
                .bg(tone(used)),
        )
        .child(div().flex((1.0 - fill).max(0.0001)))
        .into()
}

fn percent(used: f32) -> String {
    format!("{}%", used.round() as i64)
}

/// The title bar chip: the account, a bar of its 5-hour window, and both windows' use.
pub fn chip(info: &UsageInfo, hot: bool) -> Node {
    let colors = theme();
    let mono = |text: String, color: Rgba| label(text).size(12.0).mono().color(color);
    let mut row = div()
        .row()
        .items_center()
        .gap(6.0)
        .h_px(24.0)
        .px(8.0)
        .rounded(12.0)
        .on_click(USAGE_CHIP)
        .bg(if hot {
            colors.element_hover
        } else {
            Rgba::new(1.0, 1.0, 1.0, 0.05)
        })
        .child(icon(IconKind::Sparkle).size(12.0).color(colors.icon_muted));
    match (&info.account, &info.session, &info.weekly) {
        (Some(account), Some(session), Some(weekly)) => {
            row = row
                .child(mono(account.name.clone(), colors.text))
                .child(meter(session.used, Some(36.0), 5.0))
                .child(mono(percent(session.used), colors.text))
                .child(mono("5h".into(), colors.text_muted))
                .child(mono(percent(weekly.used), colors.text))
                .child(mono("wk".into(), colors.text_muted));
        }
        (Some(account), _, _) => {
            row = row.child(mono(account.name.clone(), colors.text));
        }
        (None, _, _) => row = row.child(mono("sign in".into(), colors.text_muted)),
    }
    row.into()
}

fn card(width: f32) -> ui::Div {
    let colors = theme();
    div()
        .col()
        .w_px(width)
        .p(6.0)
        .gap(2.0)
        .rounded(9.0)
        .bg(colors.elevated_surface_background)
        .border(1.0, colors.border)
}

fn plan_chip(plan: &str) -> Node {
    let colors = theme();
    let mut text = plan.to_string();
    if let Some(first) = text.get(0..1) {
        text = first.to_uppercase() + &text[1..];
    }
    div()
        .row()
        .px(6.0)
        .rounded(8.0)
        .bg(colors.text_accent.alpha(0.14))
        .child(label(text).size(10.5).mono().color(colors.text_accent))
        .into()
}

fn menu_row(id: u64, text: &str, keys: Option<&str>, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut row = div()
        .row()
        .items_center()
        .h_px(28.0)
        .px(8.0)
        .rounded(5.0)
        .on_click(id)
        .child(label(text.to_string()).size(13.0).color(colors.text))
        .child(div().flex(1.0));
    if let Some(keys) = keys {
        row = row.child(ui::render_keystroke(keys, 12.0));
    }
    if hovered == Some(id) {
        row = row.bg(colors.ghost_element_hover);
    }
    row.into()
}

fn separator() -> Node {
    div().h_px(1.0).bg(theme().border_variant).into()
}

/// The card under the chip: the account, each window with its reset, and what to do next.
pub fn chip_card(info: &UsageInfo, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut body = div().col().gap(6.0).p(8.0);
    match &info.account {
        Some(account) => {
            let mut head = div()
                .row()
                .items_center()
                .gap(6.0)
                .child(icon(IconKind::Sparkle).size(13.0).color(colors.icon_muted))
                .child(
                    label(account.name.clone())
                        .size(13.0)
                        .weight(500)
                        .color(colors.text),
                )
                .child(
                    label(account.email.clone())
                        .size(11.5)
                        .color(colors.text_placeholder)
                        .truncate(),
                )
                .child(div().flex(1.0));
            if !account.plan.is_empty() {
                head = head.child(plan_chip(&account.plan));
            }
            body = body.child(head);
            for (title, window) in [("5 hours", &info.session), ("Week", &info.weekly)] {
                let Some(window) = window else {
                    continue;
                };
                body = body.child(
                    div()
                        .row()
                        .items_center()
                        .gap(8.0)
                        .child(
                            div()
                                .w_px(54.0)
                                .child(label(title).size(12.0).color(colors.text_muted)),
                        )
                        .child(meter(window.used, None, 5.0))
                        .child(
                            div().row().w_px(36.0).justify_end().child(
                                label(percent(window.used))
                                    .size(12.0)
                                    .mono()
                                    .color(colors.text),
                            ),
                        )
                        .child(
                            div().row().w_px(72.0).justify_end().child(
                                label(window.resets.clone())
                                    .size(11.0)
                                    .color(colors.text_placeholder),
                            ),
                        ),
                );
            }
            if !account.organization.is_empty() {
                body = body.child(
                    label(account.organization.clone())
                        .size(11.0)
                        .color(colors.text_placeholder),
                );
            }
        }
        None => {
            body = body.child(
                label("No Claude Code sign-in on this Mac. Run claude and /login; Pomelo reads what it saved, never asks for it.")
                    .size(12.0)
                    .color(colors.text_muted)
                    .wrap(CARD_WIDTH - 28.0),
            );
        }
    }
    card(CARD_WIDTH)
        .child(body)
        .child(separator())
        .child(menu_row(
            USAGE_OPEN,
            "Open Agent Usage",
            Some("cmd-shift-u"),
            hovered,
        ))
        .child(menu_row(USAGE_REFRESH, "Refresh Now", None, hovered))
        .child(separator())
        .child(
            div().px(8.0).py(4.0).child(
                label(info.note.clone())
                    .size(11.0)
                    .color(colors.text_placeholder)
                    .wrap(CARD_WIDTH - 28.0),
            ),
        )
        .into()
}

/// The status bar's usage: today's total.
pub fn status_item(today: &UsageToday, hot: bool) -> Node {
    let colors = theme();
    let mut item = div()
        .row()
        .items_center()
        .gap(6.0)
        .h_px(20.0)
        .px(6.0)
        .rounded(4.0)
        .on_click(USAGE_STATUS)
        .child(icon(IconKind::Sparkle).size(12.0).color(colors.icon_muted))
        .child(
            label(format!("{} today", today.total))
                .size(12.0)
                .color(colors.text_muted),
        );
    if hot {
        item = item.bg(colors.ghost_element_hover);
    }
    item.into()
}

/// The card over the status item: the windows, and today by workspace.
pub fn today_card(info: &UsageInfo, hovered: Option<u64>) -> Node {
    let colors = theme();
    let Some(today) = &info.today else {
        return div().into();
    };
    let mut body = div().col().gap(8.0).p(8.0).child(
        div()
            .row()
            .items_center()
            .child(
                label("Agents today")
                    .size(13.0)
                    .weight(500)
                    .color(colors.text),
            )
            .child(div().flex(1.0))
            .child(
                label(match today.sessions {
                    1 => "1 session".to_string(),
                    count => format!("{count} sessions"),
                })
                .size(12.0)
                .color(colors.text_placeholder),
            ),
    );
    for (title, window) in [
        ("5-hour window", &info.session),
        ("This week", &info.weekly),
    ] {
        let Some(window) = window else {
            continue;
        };
        body = body.child(
            div()
                .col()
                .gap(4.0)
                .child(
                    div()
                        .row()
                        .child(label(title).size(12.0).color(colors.text_muted))
                        .child(div().flex(1.0))
                        .child(
                            label(format!(
                                "{} - resets {}",
                                percent(window.used),
                                window.resets
                            ))
                            .size(12.0)
                            .mono()
                            .color(tone(window.used)),
                        ),
                )
                .child(meter(window.used, None, 5.0)),
        );
    }
    let mut rows = div().col().rounded(6.0).border(1.0, colors.border_variant);
    for (index, (workspace, value)) in today.by_workspace.iter().enumerate() {
        if index > 0 {
            rows = rows.child(div().h_px(1.0).bg(colors.border_variant));
        }
        rows = rows.child(
            div()
                .row()
                .items_center()
                .h_px(30.0)
                .px(10.0)
                .child(
                    label(workspace.clone())
                        .size(12.5)
                        .color(colors.text)
                        .truncate(),
                )
                .child(div().flex(1.0))
                .child(label(value.clone()).size(12.5).mono().color(colors.text)),
        );
    }
    if !today.by_workspace.is_empty() {
        body = body.child(rows);
    }
    card(TODAY_WIDTH)
        .child(body)
        .child(separator())
        .child(menu_row(
            USAGE_OPEN,
            "Open Agent Usage",
            Some("cmd-shift-u"),
            hovered,
        ))
        .into()
}
