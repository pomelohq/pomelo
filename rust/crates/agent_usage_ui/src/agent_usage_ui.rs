//! The Agent usage tab: what the coding agents used by day, by workspace, agent or model, and the
//! heaviest sessions, with the signed-in account's plan limits.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use terminal::Modifiers;
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::{Item, ItemTick, UsageInfo};

pub const TAB_ID: &str = "agent-usage";

const PERIOD_BASE: u64 = 1;
const GROUP_BASE: u64 = 10;
const ROW_BASE: u64 = 100;
const OPEN_BASE: u64 = 400;
const CLEAR_FILTER: u64 = 20;
const CONTENT_MAX_W: f32 = 860.0;
const PAD_X: f32 = 40.0;
const TOP_SESSIONS: usize = 5;
const CHART_H: f32 = 150.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Main,
    Side,
    /// Onboarding, fixing a setup, and other one-off agents.
    Task,
}

impl AgentKind {
    fn title(self) -> &'static str {
        match self {
            AgentKind::Main => "Main agent",
            AgentKind::Side => "Side agents",
            AgentKind::Task => "Task agents",
        }
    }
}

/// One agent reply, placed in its workspace.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageTurn {
    pub workspace: String,
    pub kind: AgentKind,
    pub model: String,
    /// Local calendar day, as days since 1970-01-01.
    pub day: i64,
    pub session: String,
    pub tokens: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cost: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Workspace,
    Agent,
    Model,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open this session's agent again, in its workspace.
    OpenSession { session: String, workspace: String },
}

pub struct UsageState {
    pub turns: Vec<UsageTurn>,
    pub today: i64,
    pub limits: UsageInfo,
    /// Session id -> what its tab is called.
    pub titles: HashMap<String, String>,
    pub show_costs: bool,
    pub period: i64,
    pub group: Group,
    pub filter: Option<String>,
    pub requests: Vec<Request>,
    pub version: u64,
}

impl Default for UsageState {
    fn default() -> UsageState {
        UsageState {
            turns: Vec::new(),
            today: 0,
            limits: UsageInfo::default(),
            titles: HashMap::new(),
            show_costs: true,
            period: 7,
            group: Group::Workspace,
            filter: None,
            requests: Vec::new(),
            version: 0,
        }
    }
}

pub type Shared = Rc<RefCell<UsageState>>;

fn palette(index: usize) -> Rgba {
    let colors = theme();
    [
        colors.text_accent,
        colors.success,
        colors.warning,
        colors.terminal_ansi[5],
        colors.terminal_ansi[6],
        colors.error,
    ][index % 6]
}

pub fn format_tokens(tokens: f64) -> String {
    match tokens {
        t if t >= 1e9 => format!("{:.1}B", t / 1e9),
        t if t >= 1e7 => format!("{:.0}M", t / 1e6),
        t if t >= 1e6 => format!("{:.1}M", t / 1e6),
        t if t >= 1e3 => format!("{:.0}K", t / 1e3),
        t => format!("{t:.0}"),
    }
}

pub fn format_cost(cost: f64) -> String {
    if cost >= 100.0 {
        format!("${cost:.0}")
    } else {
        format!("${cost:.2}")
    }
}

struct Summary<'a> {
    key: String,
    label: String,
    turns: Vec<&'a UsageTurn>,
}

impl UsageState {
    fn in_period(&self, from: i64, to: i64) -> Vec<&UsageTurn> {
        self.turns
            .iter()
            .filter(|turn| turn.day > self.today - to && turn.day <= self.today - from)
            .filter(|turn| self.filter.as_ref().is_none_or(|ws| turn.workspace == *ws))
            .collect()
    }

    fn metric(&self, turn: &UsageTurn) -> f64 {
        if self.show_costs {
            turn.cost
        } else {
            turn.tokens as f64
        }
    }

    fn key_of(&self, turn: &UsageTurn) -> String {
        match self.group {
            Group::Workspace => turn.workspace.clone(),
            Group::Agent => format!("{:?}", turn.kind),
            Group::Model => model_family(&turn.model).to_string(),
        }
    }

    fn label_of(&self, turn: &UsageTurn) -> String {
        match self.group {
            Group::Workspace => turn.workspace.clone(),
            Group::Agent => turn.kind.title().to_string(),
            Group::Model => model_family(&turn.model).to_string(),
        }
    }

    fn groups<'a>(&self, turns: &[&'a UsageTurn]) -> Vec<Summary<'a>> {
        let mut groups: Vec<Summary<'a>> = Vec::new();
        for turn in turns {
            let key = self.key_of(turn);
            match groups.iter_mut().find(|group| group.key == key) {
                Some(group) => group.turns.push(turn),
                None => groups.push(Summary {
                    label: self.label_of(turn),
                    key,
                    turns: vec![turn],
                }),
            }
        }
        let total = |group: &Summary| {
            group
                .turns
                .iter()
                .map(|turn| self.metric(turn))
                .sum::<f64>()
        };
        groups.sort_by(|a, b| total(b).total_cmp(&total(a)));
        groups
    }
}

fn model_family(model: &str) -> &'static str {
    let model = model.to_ascii_lowercase();
    if model.contains("opus") {
        "Claude Opus"
    } else if model.contains("haiku") {
        "Claude Haiku"
    } else if model.contains("sonnet") {
        "Claude Sonnet"
    } else {
        "Other"
    }
}

fn segmented(options: &[(u64, &str)], selected: usize, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut row = div()
        .row()
        .p(2.0)
        .gap(2.0)
        .rounded(5.0)
        .bg(colors.element_background);
    for (index, (id, text)) in options.iter().enumerate() {
        let on = index == selected;
        let mut part = div()
            .row()
            .px(10.0)
            .py(3.0)
            .rounded(4.0)
            .on_click(*id)
            .child(label(text.to_string()).size(12.5).color(if on {
                colors.text
            } else {
                colors.text_muted
            }));
        if on {
            part = part.bg(colors.element_selected);
        } else if hovered == Some(*id) {
            part = part.bg(colors.ghost_element_hover);
        }
        row = row.child(part);
    }
    row.into()
}

fn section(title: &str, trailing: Option<Node>, body: Node) -> Node {
    let colors = theme();
    let mut head = div().row().items_center().child(
        div().row().flex(1.0).child(
            label(title.to_uppercase())
                .size(11.0)
                .weight(600)
                .color(colors.text_placeholder),
        ),
    );
    if let Some(trailing) = trailing {
        head = head.child(trailing);
    }
    div().col().gap(10.0).child(head).child(body).into()
}

fn stat(value: String, title: &str, detail: Node, width: f32) -> Node {
    let colors = theme();
    div()
        .col()
        .w_px(width)
        .p(12.0)
        .gap(2.0)
        .rounded(8.0)
        .border(1.0, colors.border_variant)
        .child(label(value).size(22.0).weight(600).color(colors.text))
        .child(label(title.to_string()).size(12.0).color(colors.text_muted))
        .child(div().pt(4.0).child(detail))
        .into()
}

fn meter(used: f32) -> Node {
    let fill = (used / 100.0).clamp(0.0, 1.0);
    div()
        .row()
        .h_px(6.0)
        .rounded(3.0)
        .bg(theme().border_variant)
        .child(
            div()
                .flex(fill.max(0.0001))
                .h_px(6.0)
                .rounded(3.0)
                .bg(workspace::usage_tone(used)),
        )
        .child(div().flex((1.0 - fill).max(0.0001)))
        .into()
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

fn num(text: String, color: Rgba) -> Node {
    div()
        .row()
        .w_px(84.0)
        .justify_end()
        .child(label(text).size(12.5).color(color))
        .into()
}

fn day_name(day: i64) -> &'static str {
    // 1970-01-01 was a Thursday.
    ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][day.rem_euclid(7) as usize]
}

/// The page as it is drawn at `width`.
pub fn render(state: &UsageState, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let content_w = (width - 2.0 * PAD_X).clamp(300.0, CONTENT_MAX_W);
    let turns = state.in_period(0, state.period);
    let previous = state.in_period(state.period, state.period * 2);
    let sum = |list: &[&UsageTurn], f: &dyn Fn(&UsageTurn) -> f64| {
        list.iter().map(|turn| f(turn)).sum::<f64>()
    };
    let cost = sum(&turns, &|turn| turn.cost);
    let previous_cost = sum(&previous, &|turn| turn.cost);
    let tokens = sum(&turns, &|turn| turn.tokens as f64);
    let output = sum(&turns, &|turn| turn.output as f64);
    let cached = sum(&turns, &|turn| turn.cache_read as f64);
    let mut sessions: Vec<&str> = turns.iter().map(|turn| turn.session.as_str()).collect();
    sessions.sort_unstable();
    sessions.dedup();

    let period_index = match state.period {
        1 => 0,
        7 => 1,
        _ => 2,
    };
    let scope = match &state.filter {
        Some(workspace) => format!("Workspace {workspace}"),
        None => "Every workspace".to_string(),
    };
    let header = div()
        .row()
        .items_center()
        .child(
            div()
                .col()
                .flex(1.0)
                .gap(2.0)
                .child(
                    label("Agent usage")
                        .size(20.0)
                        .weight(600)
                        .color(colors.text),
                )
                .child(
                    label(format!(
                        "{scope} - read from the agents' transcripts, no tokens spent"
                    ))
                    .size(12.5)
                    .italic()
                    .color(colors.text_muted),
                ),
        )
        .child(segmented(
            &[
                (PERIOD_BASE, "Today"),
                (PERIOD_BASE + 1, "7 days"),
                (PERIOD_BASE + 2, "30 days"),
            ],
            period_index,
            hovered,
        ));

    let stat_w = (content_w - 30.0) / 4.0;
    let span = if state.period == 1 {
        "day".to_string()
    } else {
        format!("{} days", state.period)
    };
    let change: Node = if previous_cost > 0.0 {
        let percent = ((cost - previous_cost) / previous_cost * 100.0).round();
        label(format!("{percent:+.0}% vs the {span} before"))
            .size(11.5)
            .color(if percent > 0.0 {
                colors.warning
            } else {
                colors.success
            })
            .into()
    } else {
        label("nothing the period before")
            .size(11.5)
            .color(colors.text_placeholder)
            .into()
    };
    let hint =
        |text: String| -> Node { label(text).size(11.5).color(colors.text_placeholder).into() };
    let mut stats = div().row().gap(10.0);
    if state.show_costs {
        stats = stats.child(stat(
            format_cost(cost),
            "API-equivalent cost",
            change,
            stat_w,
        ));
    }
    stats = stats
        .child(stat(
            format_tokens(tokens),
            "tokens",
            hint(format!("{} written by the agents", format_tokens(output))),
            stat_w,
        ))
        .child(stat(
            format!(
                "{:.0}%",
                if tokens > 0.0 {
                    cached / tokens * 100.0
                } else {
                    0.0
                }
            ),
            "read from cache",
            hint("cached context costs a tenth".into()),
            stat_w,
        ))
        .child(stat(
            sessions.len().to_string(),
            "agent sessions",
            hint(match turns.len() {
                1 => "1 reply".to_string(),
                count => format!("{count} replies"),
            }),
            stat_w,
        ));

    let mut column = div()
        .col()
        .w_px(content_w)
        .gap(22.0)
        .child(header)
        .child(div().h_px(1.0).bg(colors.border_variant))
        .child(stats);

    if let (Some(account), Some(session), Some(weekly)) = (
        &state.limits.account,
        &state.limits.session,
        &state.limits.weekly,
    ) {
        let card = div()
            .col()
            .gap(6.0)
            .p(12.0)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(6.0)
                    .child(
                        label(account.name.clone())
                            .size(13.0)
                            .weight(500)
                            .color(colors.text),
                    )
                    .child(
                        label(account.plan.clone())
                            .size(12.0)
                            .color(colors.text_placeholder),
                    )
                    .child(div().flex(1.0))
                    .child(
                        label(format!(
                            "{:.0}% 5h - {:.0}% week",
                            session.used, weekly.used
                        ))
                        .size(12.0)
                        .mono()
                        .color(colors.text),
                    ),
            )
            .child(meter(session.used))
            .child(hint(format!("5 hours: resets {}", session.resets)))
            .child(meter(weekly.used))
            .child(hint(format!("Week: resets {}", weekly.resets)));
        column = column.child(section(
            "Plan limits",
            Some(hint("the Claude account signed in on this Mac".into())),
            card.into(),
        ));
    }

    let groups = state.groups(&turns);
    let color_of = |key: &str| {
        groups
            .iter()
            .position(|group| group.key == key)
            .map_or(colors.border, palette)
    };
    let group_index = match state.group {
        Group::Workspace => 0,
        Group::Agent => 1,
        Group::Model => 2,
    };
    let group_picker = segmented(
        &[
            (GROUP_BASE, "Workspace"),
            (GROUP_BASE + 1, "Agent"),
            (GROUP_BASE + 2, "Model"),
        ],
        group_index,
        hovered,
    );
    let mut chart_body = div().col().gap(6.0);
    if state.period > 1 {
        let days: Vec<i64> = (0..state.period)
            .rev()
            .map(|back| state.today - back)
            .collect();
        let totals: Vec<f64> = days
            .iter()
            .map(|day| {
                turns
                    .iter()
                    .filter(|turn| turn.day == *day)
                    .map(|turn| state.metric(turn))
                    .sum()
            })
            .collect();
        let max = totals
            .iter()
            .copied()
            .fold(0.0, f64::max)
            .max(f64::MIN_POSITIVE);
        let gap = 6.0;
        let bar_w = ((content_w - gap * (days.len() as f32 - 1.0)) / days.len() as f32).max(2.0);
        let mut bars = div().row().gap(gap).h_px(CHART_H);
        let mut axis = div().row().gap(gap);
        for (index, day) in days.iter().enumerate() {
            let height = (totals[index] / max) as f32 * CHART_H;
            let mut bar = div().col().w_px(bar_w).h_px(height.max(1.0)).rounded(3.0);
            for group in groups.iter().rev() {
                let part: f64 = group
                    .turns
                    .iter()
                    .filter(|turn| turn.day == *day)
                    .map(|turn| state.metric(turn))
                    .sum();
                if part > 0.0 && totals[index] > 0.0 {
                    bar = bar.child(
                        div()
                            .w_px(bar_w)
                            .h_px(height * (part / totals[index]) as f32)
                            .bg(color_of(&group.key)),
                    );
                }
            }
            bars = bars.child(
                div()
                    .col()
                    .w_px(bar_w)
                    .h_px(CHART_H)
                    .justify_end()
                    .child(bar),
            );
            let text = if *day == state.today {
                "today".to_string()
            } else if state.period <= 7 || index % 5 == 4 {
                day_name(*day).to_string()
            } else {
                String::new()
            };
            axis = axis.child(
                div()
                    .row()
                    .w_px(bar_w)
                    .justify_center()
                    .child(label(text).size(11.0).color(colors.text_placeholder)),
            );
        }
        chart_body = chart_body
            .child(bars)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(axis);
    }
    let mut legend = div().row().gap(14.0);
    for group in &groups {
        legend = legend.child(
            div()
                .row()
                .items_center()
                .gap(6.0)
                .child(
                    div()
                        .w_px(8.0)
                        .h_px(8.0)
                        .rounded(4.0)
                        .bg(color_of(&group.key)),
                )
                .child(
                    label(group.label.clone())
                        .size(12.0)
                        .color(colors.text_muted),
                ),
        );
    }
    chart_body = chart_body.child(legend);
    column = column.child(section(
        if state.period == 1 { "Today" } else { "By day" },
        Some(group_picker),
        chart_body.into(),
    ));

    let total: f64 = turns.iter().map(|turn| state.metric(turn)).sum();
    let group_name = match state.group {
        Group::Workspace => "Workspace",
        Group::Agent => "Agent",
        Group::Model => "Model",
    };
    let mut table = vec![div()
        .row()
        .items_center()
        .h_px(30.0)
        .px(12.0)
        .gap(10.0)
        .bg(Rgba::new(0.0, 0.0, 0.0, 0.06))
        .child(div().w_px(200.0).child(hint(group_name.into())))
        .child(div().flex(1.0).child(hint("Share".into())))
        .child(num("Sessions".into(), colors.text_placeholder))
        .child(num("Tokens".into(), colors.text_placeholder))
        .child(if state.show_costs {
            num("Cost".into(), colors.text_placeholder)
        } else {
            div().into()
        })
        .into()];
    for (index, group) in groups.iter().enumerate() {
        let value: f64 = group.turns.iter().map(|turn| state.metric(turn)).sum();
        let group_tokens: f64 = group.turns.iter().map(|turn| turn.tokens as f64).sum();
        let group_cost: f64 = group.turns.iter().map(|turn| turn.cost).sum();
        let mut ids: Vec<&str> = group
            .turns
            .iter()
            .map(|turn| turn.session.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let share = if total > 0.0 {
            (value / total) as f32
        } else {
            0.0
        };
        let id = ROW_BASE + index as u64;
        let mut row = div()
            .row()
            .items_center()
            .h_px(38.0)
            .px(12.0)
            .gap(10.0)
            .child(
                div()
                    .row()
                    .w_px(200.0)
                    .items_center()
                    .gap(8.0)
                    .child(
                        div()
                            .w_px(8.0)
                            .h_px(8.0)
                            .rounded(4.0)
                            .bg(color_of(&group.key)),
                    )
                    .child(
                        label(group.label.clone())
                            .size(13.0)
                            .color(colors.text)
                            .truncate(),
                    ),
            )
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .h_px(4.0)
                    .rounded(2.0)
                    .bg(colors.border_variant)
                    .child(
                        div()
                            .flex(share.max(0.0001))
                            .h_px(4.0)
                            .rounded(2.0)
                            .bg(color_of(&group.key)),
                    )
                    .child(div().flex((1.0 - share).max(0.0001))),
            )
            .child(num(ids.len().to_string(), colors.text_muted))
            .child(num(format_tokens(group_tokens), colors.text_muted))
            .child(if state.show_costs {
                num(format_cost(group_cost), colors.text)
            } else {
                div().into()
            });
        if state.group == Group::Workspace {
            row = row.on_click(id);
            if hovered == Some(id) {
                row = row.bg(colors.ghost_element_hover);
            }
        }
        table.push(row.into());
    }
    let clear: Option<Node> = state.filter.as_ref().map(|_| {
        div()
            .row()
            .px(8.0)
            .py(2.0)
            .rounded(4.0)
            .on_click(CLEAR_FILTER)
            .bg(if hovered == Some(CLEAR_FILTER) {
                colors.ghost_element_hover
            } else {
                Rgba::TRANSPARENT
            })
            .child(
                label("Show every workspace")
                    .size(12.0)
                    .color(colors.text_muted),
            )
            .into()
    });
    column = column.child(section(
        &format!("By {}", group_name.to_lowercase()),
        clear,
        rows(table),
    ));

    let mut by_session: Vec<(&str, &str, AgentKind, &str, f64, f64, usize)> = Vec::new();
    for turn in &turns {
        match by_session.iter_mut().find(|entry| entry.0 == turn.session) {
            Some(entry) => {
                entry.4 += turn.tokens as f64;
                entry.5 += turn.cost;
                entry.6 += 1;
            }
            None => by_session.push((
                &turn.session,
                &turn.workspace,
                turn.kind,
                &turn.model,
                turn.tokens as f64,
                turn.cost,
                1,
            )),
        }
    }
    by_session.sort_by(|a, b| {
        let key = |entry: &(&str, &str, AgentKind, &str, f64, f64, usize)| {
            if state.show_costs {
                entry.5
            } else {
                entry.4
            }
        };
        key(b).total_cmp(&key(a))
    });
    let chip = |text: String, max: f32| -> Node {
        let text_w = ui::measure(&label(text.clone()).size(11.0).into()).0;
        div()
            .row()
            .h_px(18.0)
            .px(6.0)
            .items_center()
            .rounded(4.0)
            .border(1.0, colors.border_variant)
            .child(
                div()
                    .row()
                    .w_px(text_w.min(max))
                    .child(label(text).size(11.0).color(colors.text_muted).truncate()),
            )
            .into()
    };
    // The numbers and the button keep their room; the title and the workspace share what is left.
    let fixed = 14.0 + 3.0 * 84.0 + 62.0 + 100.0 + 8.0 * 10.0 + 24.0;
    let spare = (content_w - fixed).max(120.0);
    let title_w = (spare * 0.45).min(180.0);
    let workspace_w = (spare - title_w - 14.0).max(40.0);
    let heavy: Vec<Node> = by_session
        .iter()
        .take(TOP_SESSIONS)
        .enumerate()
        .map(
            |(index, (session, workspace, kind, model, tokens, cost, replies))| {
                let title = state
                    .titles
                    .get(*session)
                    .cloned()
                    .unwrap_or_else(|| match kind {
                        AgentKind::Main => "Main".to_string(),
                        AgentKind::Side => "Side agent".to_string(),
                        AgentKind::Task => "Task agent".to_string(),
                    });
                let open = OPEN_BASE + index as u64;
                div()
                    .row()
                    .items_center()
                    .h_px(40.0)
                    .px(12.0)
                    .gap(10.0)
                    .child(
                        icon(match kind {
                            AgentKind::Main => IconKind::Sparkle,
                            AgentKind::Side => IconKind::HelpCircle,
                            AgentKind::Task => IconKind::Wrench,
                        })
                        .size(13.0)
                        .color(colors.icon_muted),
                    )
                    .child(
                        div()
                            .w_px(title_w)
                            .child(label(title).size(13.0).color(colors.text).truncate()),
                    )
                    .child(chip(workspace.to_string(), workspace_w))
                    .child(chip(model_family(model).to_string(), 88.0))
                    .child(div().flex(1.0))
                    .child(num(
                        match replies {
                            1 => "1 reply".to_string(),
                            count => format!("{count} replies"),
                        },
                        colors.text_muted,
                    ))
                    .child(num(format_tokens(*tokens), colors.text_muted))
                    .child(if state.show_costs {
                        num(format_cost(*cost), colors.text)
                    } else {
                        div().into()
                    })
                    .child(
                        div()
                            .row()
                            .h_px(26.0)
                            .px(10.0)
                            .items_center()
                            .rounded(5.0)
                            .border(1.0, colors.border)
                            .bg(if hovered == Some(open) {
                                colors.element_hover
                            } else {
                                colors.element_background
                            })
                            .on_click(open)
                            .child(label("Open").size(12.5).color(colors.text)),
                    )
                    .into()
            },
        )
        .collect();
    if !heavy.is_empty() {
        column = column.child(section(
            "Heaviest sessions",
            None,
            div()
                .col()
                .gap(8.0)
                .child(rows(heavy))
                .child(
                    label("Most of a long session's tokens are its context read again each reply; a side agent that starts compacted or fresh costs a fraction of one forked from it.")
                        .size(11.5)
                        .color(colors.text_placeholder)
                        .wrap(content_w),
                )
                .into(),
        ));
    } else {
        column = column.child(hint("No agent replies in this period.".into()));
    }
    div()
        .col()
        .w_px(width)
        .pt(36.0)
        .pb(60.0)
        .items_center()
        .bg(colors.editor_background)
        .child(column)
        .into()
}

fn click(state: &mut UsageState, id: u64) {
    match id {
        id if (PERIOD_BASE..PERIOD_BASE + 3).contains(&id) => {
            state.period = [1, 7, 30][(id - PERIOD_BASE) as usize]
        }
        id if (GROUP_BASE..GROUP_BASE + 3).contains(&id) => {
            state.group = [Group::Workspace, Group::Agent, Group::Model][(id - GROUP_BASE) as usize]
        }
        CLEAR_FILTER => {
            state.filter = None;
            state.group = Group::Workspace;
        }
        id if (ROW_BASE..OPEN_BASE).contains(&id) => {
            let turns = state.in_period(0, state.period);
            let key = state
                .groups(&turns)
                .get((id - ROW_BASE) as usize)
                .map(|group| group.key.clone());
            if let Some(key) = key {
                state.filter = Some(key);
                state.group = Group::Agent;
            }
        }
        id if id >= OPEN_BASE => {
            let turns = state.in_period(0, state.period);
            let mut totals: Vec<(&str, &str, f64)> = Vec::new();
            for turn in &turns {
                let value = state.metric(turn);
                match totals.iter_mut().find(|entry| entry.0 == turn.session) {
                    Some(entry) => entry.2 += value,
                    None => totals.push((&turn.session, &turn.workspace, value)),
                }
            }
            totals.sort_by(|a, b| b.2.total_cmp(&a.2));
            let picked = totals
                .get((id - OPEN_BASE) as usize)
                .map(|entry| (entry.0.to_string(), entry.1.to_string()));
            if let Some((session, workspace)) = picked {
                state
                    .requests
                    .push(Request::OpenSession { session, workspace });
            }
        }
        _ => {}
    }
    state.version = state.version.wrapping_add(1);
}

pub struct UsagePage {
    shared: Shared,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    scroll: f32,
    content_h: f32,
    body_h: f32,
    seen: u64,
}

impl UsagePage {
    pub fn new(shared: Shared) -> UsagePage {
        UsagePage {
            shared,
            hits: Vec::new(),
            hovered: None,
            scroll: 0.0,
            content_h: 0.0,
            body_h: 0.0,
            seen: u64::MAX,
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id)
    }

    pub fn click(&mut self, id: u64) {
        click(&mut self.shared.borrow_mut(), id);
    }
}

impl Item for UsagePage {
    fn id(&self) -> Option<String> {
        Some(TAB_ID.to_string())
    }

    fn title(&self) -> String {
        "Agent usage".into()
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Sparkle)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let tree = render(&self.shared.borrow(), body.w / scale, self.hovered);
        self.content_h = ui::measure(&tree).1 * scale;
        self.body_h = body.h;
        self.scroll = self
            .scroll
            .clamp(0.0, (self.content_h - self.body_h).max(0.0));
        let shifted = Rect::new(
            body.x,
            body.y - self.scroll,
            body.w,
            body.h.max(self.content_h),
            Rgba::TRANSPARENT,
        );
        let painted = ui::render(&tree, shifted);
        self.hits = painted
            .hits
            .iter()
            .copied()
            .filter(|(rect, _)| rect.y + rect.h > body.y && rect.y < body.y + body.h)
            .collect();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        if let Some(id) = self.hit(x, y) {
            self.click(id);
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hovered = self.hit(x, y);
        let moved = hovered != self.hovered;
        self.hovered = hovered;
        moved
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let next = (self.scroll - delta_y).clamp(0.0, (self.content_h - self.body_h).max(0.0));
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let version = self.shared.borrow().version;
        let changed = version != self.seen;
        self.seen = version;
        ItemTick {
            changed,
            ..ItemTick::default()
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests;

/// A page over made-up sessions, for headless snapshots.
pub fn preview_page() -> UsagePage {
    let workspaces = ["feat-login", "PROJ-101", "main", "feat-pay"];
    let mut seed: u64 = 7;
    let mut next = || {
        seed = (seed * 9301 + 49297) % 233_280;
        seed as f64 / 233_280.0
    };
    let today = 20_725;
    let mut turns = Vec::new();
    for back in 0..30 {
        let count = 1 + (next() * 3.0) as usize;
        for number in 0..count {
            let workspace = workspaces[(next() * if back < 7 { 2.6 } else { 4.0 }) as usize];
            let roll = next();
            let kind = if roll < 0.55 {
                AgentKind::Main
            } else if roll < 0.9 {
                AgentKind::Side
            } else {
                AgentKind::Task
            };
            let model = if next() < 0.7 {
                "claude-opus-5-5"
            } else {
                "claude-sonnet-5"
            };
            let scale = if kind == AgentKind::Main { 1.0 } else { 0.35 };
            let cache_read = ((900.0 + next() * 6000.0) * scale * 1000.0) as u64;
            let output = ((20.0 + next() * 90.0) * scale * 1000.0) as u64;
            let tokens = cache_read + output + ((60.0 + next() * 300.0) * scale * 1000.0) as u64;
            let per_million = if model.contains("opus") { 1.5 } else { 0.3 };
            turns.push(UsageTurn {
                workspace: workspace.to_string(),
                kind,
                model: model.to_string(),
                day: today - back,
                session: format!("s{back}-{number}"),
                tokens,
                output,
                cache_read,
                cost: tokens as f64 * per_million / 1e6 + output as f64 * 60.0 / 1e6,
            });
        }
    }
    let state = UsageState {
        turns,
        today,
        limits: UsageInfo {
            account: Some(workspace::UsageAccount {
                name: "toan".into(),
                email: "toan@example.com".into(),
                plan: "Team".into(),
                organization: "Example".into(),
            }),
            session: Some(workspace::UsageWindow {
                used: 23.0,
                resets: "in 2h 31m".into(),
            }),
            weekly: Some(workspace::UsageWindow {
                used: 74.0,
                resets: "Sat 17:00".into(),
            }),
            ..UsageInfo::default()
        },
        ..UsageState::default()
    };
    UsagePage::new(Rc::new(RefCell::new(state)))
}
