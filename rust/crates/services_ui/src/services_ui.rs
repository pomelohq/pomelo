//! The Services panel: an overview of the active workspace's services (a summary card and what needs
//! attention), then its services by repo, with the shared containers pinned at the bottom. Clicking a running
//! service opens its console as a center tab.

mod model;
mod tab;
mod view;

use std::collections::HashSet;
use std::path::PathBuf;

use pom_services::ServiceTarget;
use terminal_ui::TerminalItem;
use ui::{div, icon, label, theme, IconKind, Node, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::{AgentFix, EditKey, MenuItem, PaletteEntry, PaneKind, PanelRequest, SidePanelView};

use model::{shared_key, Model, SharedRun, ALL_SHARED, WORKSPACE_GROUP};
pub use model::{Action, Crash, ServicesContext, SharedConfig, Status};
use view::{action_button, State, Tone};

const SHARED_GROUP: &str = "_shared";

const ROW_H: f32 = 26.0;
const INDENT: f32 = 16.0;
const HEADER_H: f32 = 34.0;
const FILTER_H: f32 = 64.0;
const BUTTON: f32 = 20.0;
/// Hit ids per row: the row itself plus its controls.
const ROW_STRIDE: u64 = 16;
/// Controls above the tree: the filter, the status filter, and the summary card's buttons.
const HEAD_BASE: u64 = 8_000_000;
const FILTER: u64 = HEAD_BASE;
const STATUS_FILTER: u64 = HEAD_BASE + 1;
const START_ALL: u64 = HEAD_BASE + 10;
const STOP_ALL: u64 = HEAD_BASE + 11;
const RESTART_FAILED: u64 = HEAD_BASE + 12;
/// A "needs attention" card's controls: the card, then its buttons.
const ATTENTION_BASE: u64 = HEAD_BASE + 100_000;
const ATTENTION_STRIDE: u64 = 8;
const MENU_BASE: u64 = 9_000_000;
const TAB_BUTTON_BASE: u64 = MENU_BASE + 1_000;
/// Palette entries: a service by twice its index (plus one to stop it), a repo command from here on.
const PALETTE_COMMAND_BASE: u64 = 50_000;

pub struct TabButton {
    pub icon: IconKind,
    pub id: String,
    pub open: std::sync::Arc<dyn Fn() -> Box<dyn workspace::Item>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Control {
    Row = 0,
    Start = 1,
    Stop = 2,
    Restart = 3,
    OpenUrl = 4,
    StartAll = 5,
    StopAll = 6,
    Logs = 7,
    More = 8,
}

impl Control {
    fn from_offset(offset: u64) -> Option<Control> {
        [
            Control::Row,
            Control::Start,
            Control::Stop,
            Control::Restart,
            Control::OpenUrl,
            Control::StartAll,
            Control::StopAll,
            Control::Logs,
            Control::More,
        ]
        .get(offset as usize)
        .copied()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CardAction {
    Open = 0,
    Logs = 1,
    Fix = 2,
    Restart = 3,
    NewPort = 4,
}

impl CardAction {
    fn from_offset(offset: u64) -> Option<CardAction> {
        [
            CardAction::Open,
            CardAction::Logs,
            CardAction::Fix,
            CardAction::Restart,
            CardAction::NewPort,
        ]
        .get(offset as usize)
        .copied()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum StatusFilter {
    #[default]
    All,
    Running,
    Failed,
    Stopped,
}

impl StatusFilter {
    const ALL: [StatusFilter; 4] = [
        StatusFilter::All,
        StatusFilter::Running,
        StatusFilter::Failed,
        StatusFilter::Stopped,
    ];

    fn title(self) -> &'static str {
        match self {
            StatusFilter::All => "All",
            StatusFilter::Running => "Running",
            StatusFilter::Failed => "Failed",
            StatusFilter::Stopped => "Stopped",
        }
    }

    fn keeps(self, state: &State) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Running => matches!(state, State::Running | State::Busy(_)),
            StatusFilter::Failed => state.needs_attention(),
            StatusFilter::Stopped => *state == State::Stopped,
        }
    }
}

#[derive(Clone, Debug)]
enum Row {
    Group {
        key: String,
        label: String,
        running: usize,
        total: usize,
        collapsed: bool,
        /// The standing of each of its services, for the pips.
        states: Vec<State>,
        /// The shared containers its config uses.
        engines: Vec<pom_db::Engine>,
    },
    Service {
        target: ServiceTarget,
        holder: String,
    },
    Shared {
        name: String,
    },
}

/// A service that crashed or could not start, shown as a card above the tree.
#[derive(Clone, Debug)]
struct Attention {
    target: ServiceTarget,
    holder: String,
    crash: Option<Crash>,
    /// The error of a start that failed.
    error: Option<String>,
    port: Option<u16>,
}

#[derive(Clone, Copy, Default)]
struct Counts {
    total: usize,
    running: usize,
    busy: usize,
    attention: usize,
    stopped: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuAction {
    Nothing,
    Run(Action),
    OpenTab,
    ViewLogs,
    Fix,
    Ask,
    CopyCommand,
    Mode(usize),
    Profile(usize),
    OpenUrl,
    CopyUrl,
    RepoAll(Action),
    RunCommand(usize),
    Environment,
    Fold,
    Shared(Action),
    SharedLogs,
}

/// What a right-click (or "...") opened on: a service, a repo, or a shared container.
#[derive(Clone, Debug)]
enum MenuSubject {
    Service {
        target: ServiceTarget,
        holder: String,
    },
    Repo {
        key: String,
        row: usize,
    },
    Shared {
        name: String,
    },
}

struct OpenMenu {
    subject: MenuSubject,
    items: Vec<(MenuItem, MenuAction)>,
    submenus: Vec<(u64, Vec<(MenuItem, MenuAction)>)>,
    modes: Vec<String>,
    profiles: Vec<String>,
    commands: Vec<RepoCommand>,
}

/// Builds a menu's items with ids from `base` upward.
struct MenuBuilder {
    base: u64,
    next: u64,
    items: Vec<(MenuItem, MenuAction)>,
    separate: bool,
}

impl MenuBuilder {
    fn new(base: u64) -> MenuBuilder {
        MenuBuilder {
            base,
            next: 0,
            items: Vec::new(),
            separate: false,
        }
    }

    fn push(
        &mut self,
        label: String,
        action: MenuAction,
        icon: Option<IconKind>,
        hint: Option<&str>,
        disabled: bool,
    ) -> &mut MenuItem {
        let sep = std::mem::take(&mut self.separate) && !self.items.is_empty();
        let item = MenuItem {
            id: self.base + self.next,
            label: label.into(),
            checked: false,
            sep,
            disabled,
            danger: false,
            icon,
            hint: hint.map(|hint| hint.to_string().into()),
        };
        self.next += 1;
        self.items.push((item, action));
        let last = self.items.len() - 1;
        &mut self.items[last].0
    }

    fn header(&mut self, text: String) {
        self.push(text, MenuAction::Nothing, None, None, true);
    }

    fn item(&mut self, label: &str, action: MenuAction, icon: IconKind, hint: Option<&str>) {
        self.push(label.to_string(), action, Some(icon), hint, false);
    }

    fn sep(&mut self) {
        self.separate = true;
    }

    /// An item opening submenu `id`, with `hint` telling what it is set to.
    fn submenu(&mut self, id: u64, label: &str, icon: IconKind, hint: &str) {
        let sep = std::mem::take(&mut self.separate) && !self.items.is_empty();
        self.items.push((
            MenuItem {
                id,
                label: label.to_string().into(),
                checked: false,
                sep,
                disabled: false,
                danger: false,
                icon: Some(icon),
                hint: (!hint.is_empty()).then(|| hint.to_string().into()),
            },
            MenuAction::Nothing,
        ));
    }
}

pub struct ServicesPanel {
    model: Model,
    /// Where consoles start (the workspace root).
    root: PathBuf,
    base: u64,
    rows: Vec<Row>,
    attention: Vec<Attention>,
    counts: Counts,
    collapsed: HashSet<String>,
    hover: Option<u64>,
    scroll: f32,
    viewport_h: f32,
    content_h: f32,
    filter: TextField,
    filter_focused: bool,
    status_filter: StatusFilter,
    menu: Option<OpenMenu>,
    requests: Vec<PanelRequest>,
    tab_buttons: Vec<TabButton>,
    next_item: u64,
    /// A shared stop waiting for the user to confirm, with its prompt tag.
    confirm: Option<(u64, SharedRun)>,
}

/// One of a repo's pre-written commands (the config's `shortcuts`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct RepoCommand {
    repo: String,
    label: String,
    cmd: String,
}

impl ServicesPanel {
    pub fn new(context: ServicesContext, root: PathBuf) -> ServicesPanel {
        ServicesPanel {
            model: Model::new(context),
            root,
            base: workspace::side_panel_base(PaneKind::Services),
            rows: Vec::new(),
            attention: Vec::new(),
            counts: Counts::default(),
            collapsed: HashSet::new(),
            hover: None,
            scroll: 0.0,
            viewport_h: 0.0,
            content_h: 0.0,
            filter: {
                let mut field = TextField::default();
                field.set_font_size(12.5);
                field
            },
            filter_focused: false,
            status_filter: StatusFilter::All,
            menu: None,
            requests: Vec::new(),
            tab_buttons: Vec::new(),
            next_item: 1 << 40,
            confirm: None,
        }
    }

    /// The last known status of a service (`repo` is `_ws` for a workspace-level one).
    pub fn status(&self, repo: &str, service: &str) -> Status {
        let target = self.model.context.target(repo, service);
        self.model
            .status(&self.model.context.runner.holder_name(&target))
    }

    /// Shows `repo`/`service` in `status` with the given failure, without running anything (snapshots, tests).
    pub fn show_status(
        &mut self,
        repo: &str,
        service: &str,
        status: Status,
        error: Option<String>,
        crash: Option<Crash>,
    ) {
        let holder = self
            .model
            .context
            .runner
            .holder_name(&self.model.context.target(repo, service));
        if let Ok(mut shared) = self.model.shared.lock() {
            shared.status.insert(holder.clone(), status);
            match error {
                Some(error) => shared.errors.insert(holder.clone(), error),
                None => shared.errors.remove(&holder),
            };
            match crash {
                Some(crash) => shared.crashes.insert(holder, crash),
                None => shared.crashes.remove(&holder),
            };
        }
    }

    /// The header and facts of `repo`/`service`'s tab, `width` wide (snapshots).
    pub fn tab_toolbar(&self, repo: &str, service: &str, width: f32) -> Node {
        tab::toolbar_preview(
            self.model.context.clone(),
            self.model.shared.clone(),
            self.model.context.target(repo, service),
            self.root.clone(),
            width,
        )
    }

    /// Shows shared container `name` as running (snapshots, tests).
    pub fn show_shared_running(&mut self, name: &str) {
        if let Ok(mut shared) = self.model.shared.lock() {
            shared.shared_running.insert(name.to_string());
        }
    }

    fn state(&self, holder: &str) -> State {
        if let Some(action) = self.model.pending(holder) {
            return State::Busy(action.progress_label());
        }
        match self.model.status(holder) {
            Status::Running => State::Running,
            Status::Crashed => State::Crashed,
            Status::Stopped => match self.model.error(holder) {
                Some(message) => State::Failed {
                    port: port_in(&message),
                },
                None => State::Stopped,
            },
        }
    }

    fn filtering(&self) -> bool {
        !self.filter.text().trim().is_empty() || self.status_filter != StatusFilter::All
    }

    fn keeps(&self, target: &ServiceTarget, state: &State) -> bool {
        let query = self.filter.text().trim().to_lowercase();
        let named = query.is_empty()
            || target.service.to_lowercase().contains(&query)
            || target.repo.to_lowercase().contains(&query);
        named && self.status_filter.keeps(state)
    }

    fn rebuild_rows(&mut self) {
        let context = &self.model.context;
        let Some(config) = context.config() else {
            self.rows.clear();
            self.attention.clear();
            self.counts = Counts::default();
            return;
        };
        let mut groups: Vec<(String, String, Vec<ServiceTarget>)> = Vec::new();
        for target in context.targets(&config) {
            let key = target.repo.clone();
            match groups.iter_mut().find(|(group, _, _)| *group == key) {
                Some((_, _, targets)) => targets.push(target),
                None => {
                    let label = if key == WORKSPACE_GROUP {
                        "Workspace".to_string()
                    } else {
                        key.clone()
                    };
                    groups.push((key, label, vec![target]));
                }
            }
        }
        let users: Vec<(String, pom_db::Engine, Vec<String>)> = config
            .shared_services
            .iter()
            .map(|(name, def)| {
                (
                    name.clone(),
                    pom_db::Engine::of_service(name, def),
                    pom_db::service_users(&config, name),
                )
            })
            .collect();
        let filtering = self.filtering();
        let mut rows = Vec::new();
        let mut attention = Vec::new();
        let mut counts = Counts::default();
        for (key, group_label, targets) in groups {
            let holders: Vec<String> = targets
                .iter()
                .map(|target| context.runner.holder_name(target))
                .collect();
            let states: Vec<State> = holders.iter().map(|holder| self.state(holder)).collect();
            for ((target, holder), state) in targets.iter().zip(&holders).zip(&states) {
                counts.total += 1;
                match state {
                    State::Running => counts.running += 1,
                    State::Busy(_) => counts.busy += 1,
                    State::Stopped => counts.stopped += 1,
                    State::Crashed | State::Failed { .. } => {
                        counts.attention += 1;
                        let error = self.model.error(holder);
                        attention.push(Attention {
                            target: target.clone(),
                            holder: holder.clone(),
                            crash: self.model.crash(holder),
                            port: error.as_deref().and_then(port_in),
                            error,
                        });
                    }
                }
            }
            let kept: Vec<(ServiceTarget, String)> = targets
                .into_iter()
                .zip(holders)
                .zip(&states)
                .filter(|((target, _), state)| self.keeps(target, state))
                .map(|(pair, _)| pair)
                .collect();
            if filtering && kept.is_empty() {
                continue;
            }
            let alias = config
                .repos
                .get(&key)
                .map(|dir| {
                    if dir.alias.is_empty() {
                        key.clone()
                    } else {
                        dir.alias.clone()
                    }
                })
                .unwrap_or_else(|| key.clone());
            let engines = users
                .iter()
                .filter(|(_, _, used_by)| used_by.contains(&alias))
                .map(|(_, engine, _)| *engine)
                .collect();
            let collapsed = !filtering && self.collapsed.contains(&key);
            rows.push(Row::Group {
                key,
                label: group_label,
                running: states
                    .iter()
                    .filter(|state| **state == State::Running)
                    .count(),
                total: states.len(),
                collapsed,
                states,
                engines,
            });
            if collapsed {
                continue;
            }
            for (target, holder) in kept {
                rows.push(Row::Service { target, holder });
            }
        }
        if !config.shared_services.is_empty() {
            let names: Vec<String> = config.shared_services.keys().cloned().collect();
            let collapsed = self.collapsed.contains(SHARED_GROUP);
            rows.push(Row::Group {
                key: SHARED_GROUP.to_string(),
                label: "Shared - all workspaces".to_string(),
                running: names
                    .iter()
                    .filter(|name| self.model.shared_running(name))
                    .count(),
                total: names.len(),
                collapsed,
                states: Vec::new(),
                engines: Vec::new(),
            });
            if !collapsed {
                for name in names {
                    rows.push(Row::Shared { name });
                }
            }
        }
        self.rows = rows;
        self.attention = attention;
        self.counts = counts;
    }

    fn id(&self, row: usize, control: Control) -> u64 {
        self.base + row as u64 * ROW_STRIDE + control as u64
    }

    fn card_id(&self, card: usize, action: CardAction) -> u64 {
        self.base + ATTENTION_BASE + card as u64 * ATTENTION_STRIDE + action as u64
    }

    pub fn with_tab_buttons(mut self, buttons: Vec<TabButton>) -> ServicesPanel {
        self.tab_buttons = buttons;
        self
    }

    fn tab_button_index(&self, id: u64) -> Option<usize> {
        let index = id.checked_sub(self.base + TAB_BUTTON_BASE)? as usize;
        (index < self.tab_buttons.len()).then_some(index)
    }

    fn decode(&self, id: u64) -> Option<(usize, Control)> {
        let offset = id.checked_sub(self.base)?;
        if offset >= HEAD_BASE {
            return None;
        }
        let row = (offset / ROW_STRIDE) as usize;
        Some((row, Control::from_offset(offset % ROW_STRIDE)?))
    }

    fn decode_card(&self, id: u64) -> Option<(usize, CardAction)> {
        let offset = id.checked_sub(self.base + ATTENTION_BASE)?;
        if offset >= MENU_BASE - HEAD_BASE - 100_000 {
            return None;
        }
        let card = (offset / ATTENTION_STRIDE) as usize;
        Some((card, CardAction::from_offset(offset % ATTENTION_STRIDE)?))
    }

    fn ours(&self, id: u64) -> bool {
        id.checked_sub(self.base)
            .is_some_and(|offset| offset < MENU_BASE)
            || self.tab_button_index(id).is_some()
    }

    fn hovered_row(&self) -> Option<usize> {
        self.hover
            .and_then(|id| self.decode(id))
            .map(|(row, _)| row)
    }

    fn hot(&self, id: u64) -> bool {
        self.hover == Some(id)
    }

    fn button(&self, id: u64, kind: IconKind) -> Node {
        let hot = self.hot(id);
        div()
            .w_px(BUTTON)
            .h_px(BUTTON)
            .rounded(4.0)
            .items_center()
            .justify_center()
            .on_click(id)
            .bg(if hot {
                theme().element_hover
            } else {
                Rgba::TRANSPARENT
            })
            .child(icon(kind).size(12.0).color(if hot {
                theme().icon
            } else {
                theme().icon_muted
            }))
            .into()
    }

    fn more_button(&self, index: usize) -> Node {
        self.button(self.id(index, Control::More), IconKind::Ellipsis)
    }

    /// The rows the hovered row is tied to: the shared containers a repo or service uses, or the repos (and
    /// their services) that use a hovered container.
    fn related_rows(&self) -> HashSet<usize> {
        let mut related = HashSet::new();
        let Some(hovered) = self.hovered_row() else {
            return related;
        };
        let Some(config) = self.model.context.config() else {
            return related;
        };
        let alias_of = |key: &str| {
            config
                .repos
                .get(key)
                .map(|dir| {
                    if dir.alias.is_empty() {
                        key.to_string()
                    } else {
                        dir.alias.clone()
                    }
                })
                .unwrap_or_else(|| key.to_string())
        };
        match self.rows.get(hovered) {
            Some(Row::Group { key, .. }) if key != SHARED_GROUP && key != WORKSPACE_GROUP => {
                let alias = alias_of(key);
                for (index, row) in self.rows.iter().enumerate() {
                    if let Row::Shared { name } = row {
                        if pom_db::service_users(&config, name).contains(&alias) {
                            related.insert(index);
                        }
                    }
                }
            }
            Some(Row::Service { target, .. }) if !target.is_workspace_level() => {
                let alias = alias_of(&target.repo);
                for (index, row) in self.rows.iter().enumerate() {
                    if let Row::Shared { name } = row {
                        if pom_db::service_users(&config, name).contains(&alias) {
                            related.insert(index);
                        }
                    }
                }
            }
            Some(Row::Shared { name }) => {
                let users = pom_db::service_users(&config, name);
                for (index, row) in self.rows.iter().enumerate() {
                    let repo = match row {
                        Row::Group { key, .. } if key != SHARED_GROUP => key,
                        Row::Service { target, .. } => &target.repo,
                        _ => continue,
                    };
                    if users.contains(&alias_of(repo)) {
                        related.insert(index);
                    }
                }
            }
            _ => {}
        }
        related
    }

    fn guide_column(&self) -> Node {
        div()
            .row()
            .w_px(INDENT)
            .h_px(ROW_H)
            .justify_center()
            .child(div().w_px(1.0).h_px(ROW_H).bg(theme().panel_indent_guide))
            .into()
    }

    fn render_row(&self, index: usize, row: &Row) -> Node {
        let colors = theme();
        let hovered = self.hovered_row() == Some(index);
        let related = self.related_rows().contains(&index);
        let mut body = div()
            .row()
            .h_px(ROW_H)
            .pl(8.0)
            .pr(6.0)
            .gap(6.0)
            .items_center()
            .on_click(self.id(index, Control::Row));
        if hovered {
            body = body.bg(colors.ghost_element_hover);
        } else if related {
            body = body.bg(colors.text_accent.alpha(0.08)).pin_left_edge(
                0.0,
                0.0,
                2.0,
                div().bg(colors.text_accent),
            );
        }
        match row {
            Row::Group {
                key,
                label: text,
                running,
                total,
                collapsed,
                states,
                engines,
            } => {
                let chevron = if *collapsed {
                    IconKind::ChevronRight
                } else {
                    IconKind::ChevronDown
                };
                let mut name = div().row().flex(1.0).gap(6.0).items_center();
                if key == SHARED_GROUP {
                    name = name.child(
                        label(text.to_uppercase())
                            .size(11.0)
                            .weight(600)
                            .color(colors.text_placeholder)
                            .truncate(),
                    );
                } else {
                    name = name.child(label(text.clone()).medium().color(colors.text).truncate());
                    let mut logos = div().row().gap(3.0).items_center();
                    for engine in engines {
                        let (kind, tint) = database_ui::engine_logo(*engine);
                        logos = logos.child(icon(kind).size(12.0).color(tint));
                    }
                    name = name.child(logos);
                }
                let trailing: Node =
                    if hovered {
                        div()
                            .row()
                            .gap(2.0)
                            .child(self.button(self.id(index, Control::StartAll), IconKind::Play))
                            .child(self.button(self.id(index, Control::StopAll), IconKind::Stop))
                            .child(self.more_button(index))
                            .into()
                    } else {
                        let failed = states.iter().any(State::needs_attention);
                        let mut pips = div().row().gap(3.0).items_center();
                        for state in states.iter().take(8) {
                            pips = pips.child(view::dot(view::state_color(state), 5.0));
                        }
                        div()
                            .row()
                            .gap(6.0)
                            .items_center()
                            .child(pips)
                            .child(label(format!("{running}/{total}")).size(11.0).color(
                                if failed {
                                    colors.error
                                } else {
                                    colors.text_muted
                                },
                            ))
                            .into()
                    };
                body.child(
                    div()
                        .row()
                        .w_px(12.0)
                        .h_px(ROW_H)
                        .items_center()
                        .justify_center()
                        .child(icon(chevron).size(12.0).color(colors.icon_muted)),
                )
                .child(name)
                .child(trailing)
                .into()
            }
            Row::Service { target, holder } => {
                let state = self.state(holder);
                let trailing: Node = match (&state, hovered) {
                    (State::Busy(text), _) => {
                        label(*text).size(11.0).color(colors.text_muted).into()
                    }
                    (State::Running, true) => {
                        let mut buttons = div().row().gap(2.0).child(
                            self.button(self.id(index, Control::Restart), IconKind::RotateCw),
                        );
                        if self.port(target).is_some() {
                            buttons =
                                buttons.child(self.button(
                                    self.id(index, Control::OpenUrl),
                                    IconKind::ArrowUpRight,
                                ));
                        }
                        buttons
                            .child(self.button(self.id(index, Control::Logs), IconKind::Terminal))
                            .child(self.button(self.id(index, Control::Stop), IconKind::Stop))
                            .child(self.more_button(index))
                            .into()
                    }
                    (_, true) => div()
                        .row()
                        .gap(2.0)
                        .child(self.button(self.id(index, Control::Start), IconKind::Play))
                        .child(self.button(self.id(index, Control::Logs), IconKind::Terminal))
                        .child(self.more_button(index))
                        .into(),
                    (State::Crashed, false) => {
                        label("crashed").size(11.0).color(colors.error).into()
                    }
                    (State::Failed { port: Some(port) }, false) => label(format!(":{port} in use"))
                        .size(11.0)
                        .mono()
                        .color(colors.warning)
                        .into(),
                    (State::Failed { port: None }, false) => {
                        label("failed").size(11.0).color(colors.error).into()
                    }
                    (_, false) => match self.port(target) {
                        Some(port) => label(format!(":{port}"))
                            .size(11.0)
                            .mono()
                            .color(colors.text_placeholder)
                            .into(),
                        None if state == State::Running => {
                            let pidfile = self.model.context.runner.holders().pidfile(holder);
                            match view::uptime(&pidfile) {
                                Some(uptime) => label(format!("up {uptime}"))
                                    .size(11.0)
                                    .color(colors.text_placeholder)
                                    .into(),
                                None => div().into(),
                            }
                        }
                        None => div().into(),
                    },
                };
                let mut name = div()
                    .row()
                    .flex(1.0)
                    .gap(6.0)
                    .items_center()
                    .child(label(target.service.clone()).color(colors.text).truncate());
                if let Some(mode) = self.mode(target).filter(|mode| !mode.is_empty()) {
                    name = name.child(view::tag(&mode));
                }
                body.child(self.guide_column())
                    .child(view::status_icon(&state))
                    .child(name)
                    .child(trailing)
                    .into()
            }
            Row::Shared { name } => {
                let running = self.model.shared_running(name);
                let pending = self.model.pending(&shared_key(name));
                let failed = self.model.error(&shared_key(name)).is_some();
                let state = match (pending, running, failed) {
                    (Some(action), _, _) => State::Busy(action.progress_label()),
                    (None, true, _) => State::Running,
                    (None, false, true) => State::Failed { port: None },
                    (None, false, false) => State::Stopped,
                };
                let used_by = self
                    .model
                    .context
                    .config()
                    .map(|config| pom_db::service_users(&config, name))
                    .unwrap_or_default();
                let engine = self
                    .model
                    .context
                    .config()
                    .and_then(|config| {
                        config
                            .shared_services
                            .get(name)
                            .map(|def| pom_db::Engine::of_service(name, def))
                    })
                    .unwrap_or(pom_db::Engine::Other);
                let (logo, tint) = database_ui::engine_logo(engine);
                let trailing: Node = match (&state, hovered) {
                    (State::Busy(text), _) => {
                        label(*text).size(11.0).color(colors.text_accent).into()
                    }
                    (State::Running, true) => div()
                        .row()
                        .gap(2.0)
                        .child(self.button(self.id(index, Control::Restart), IconKind::RotateCw))
                        .child(self.button(self.id(index, Control::Logs), IconKind::Terminal))
                        .child(self.button(self.id(index, Control::Stop), IconKind::Stop))
                        .child(self.more_button(index))
                        .into(),
                    (_, true) => div()
                        .row()
                        .gap(2.0)
                        .child(self.button(self.id(index, Control::Start), IconKind::Play))
                        .child(self.more_button(index))
                        .into(),
                    (_, false) if used_by.is_empty() => label("not used here")
                        .size(11.0)
                        .color(colors.text_placeholder)
                        .into(),
                    (_, false) => label(used_by.join(", "))
                        .size(11.0)
                        .color(colors.text_muted)
                        .truncate()
                        .into(),
                };
                body.child(view::status_icon(&state))
                    .child(icon(logo).size(13.0).color(tint))
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .gap(6.0)
                            .items_center()
                            .child(label(name.clone()).color(colors.text).truncate())
                            .child(
                                label(format!(
                                    ":{}",
                                    self.model.context.runner.shared_host_port(name)
                                ))
                                .size(11.0)
                                .mono()
                                .color(colors.text_placeholder),
                            ),
                    )
                    .child(trailing)
                    .into()
            }
        }
    }

    /// The workspace at a glance: how many run, a bar of the split, and the buttons that act on all of them.
    fn summary_card(&self, width: f32) -> Node {
        let colors = theme();
        let counts = self.counts;
        let inner = width - 16.0 - 20.0 - 2.0;
        let status = if counts.busy > 0 {
            format!("{} busy", counts.busy)
        } else {
            format!("{} of {} running", counts.running, counts.total)
        };
        let status = label(status).size(11.5).color(colors.text_muted);
        let status_w = ui::measure(&status.clone().into()).0;
        let ticket =
            (!self.model.context.ticket.is_empty()).then(|| view::tag(&self.model.context.ticket));
        let ticket_w = ticket.as_ref().map_or(0.0, |tag| ui::measure(tag).0 + 6.0);
        let name = label(self.model.context.branch.clone())
            .medium()
            .color(colors.text);
        let natural = ui::measure(&name.clone().into()).0;
        let name_w = natural.min((inner - status_w - ticket_w - 8.0).max(40.0));
        let mut title = div()
            .row()
            .gap(6.0)
            .items_center()
            .child(div().row().w_px(name_w).child(name.truncate()));
        if let Some(ticket) = ticket {
            title = title.child(ticket);
        }
        let title = div()
            .row()
            .items_center()
            .child(div().row().flex(1.0).items_center().child(title))
            .child(status);
        let total = counts.total.max(1) as f32;
        let segment = |count: usize, color: Rgba| -> Option<Node> {
            (count > 0).then(|| {
                div()
                    .w_px((inner * count as f32 / total).max(2.0))
                    .h_px(4.0)
                    .bg(color)
                    .into()
            })
        };
        let mut bar = div()
            .row()
            .w_px(inner)
            .h_px(4.0)
            .rounded(2.0)
            .bg(colors.border_variant);
        for piece in [
            segment(counts.running, colors.success),
            segment(counts.busy, colors.text_accent),
            segment(counts.attention, colors.error),
        ]
        .into_iter()
        .flatten()
        {
            bar = bar.child(piece);
        }
        let legend_item = |color: Rgba, text: String| -> Node {
            div()
                .row()
                .gap(5.0)
                .items_center()
                .child(view::dot(color, 7.0))
                .child(label(text).size(11.5).color(colors.text_muted))
                .into()
        };
        let mut legend = vec![legend_item(
            colors.success,
            format!("{} running", counts.running),
        )];
        if counts.busy > 0 {
            legend.push(legend_item(
                colors.text_accent,
                format!("{} busy", counts.busy),
            ));
        }
        if counts.attention > 0 {
            legend.push(legend_item(
                colors.error,
                format!("{} need attention", counts.attention),
            ));
        }
        if counts.stopped > 0 {
            legend.push(legend_item(
                colors.border,
                format!("{} stopped", counts.stopped),
            ));
        }
        let mut buttons = vec![
            action_button(
                self.base + START_ALL,
                Some(IconKind::Play),
                "Start all",
                Tone::Primary,
                self.hot(self.base + START_ALL),
            ),
            action_button(
                self.base + STOP_ALL,
                Some(IconKind::Stop),
                "Stop all",
                Tone::Plain,
                self.hot(self.base + STOP_ALL),
            ),
        ];
        if counts.attention > 0 {
            buttons.push(action_button(
                self.base + RESTART_FAILED,
                Some(IconKind::RotateCw),
                "Restart failed",
                Tone::Plain,
                self.hot(self.base + RESTART_FAILED),
            ));
        }
        div()
            .col()
            .w_px(width - 16.0)
            .p(10.0)
            .gap(8.0)
            .rounded(7.0)
            .border(1.0, colors.border_variant)
            .bg(Rgba::new(0.0, 0.0, 0.0, 0.08))
            .child(title)
            .child(bar)
            .child(view::pack(legend, inner, 10.0))
            .child(view::pack(buttons, inner, 6.0))
            .into()
    }

    fn attention_card(&self, index: usize, card: &Attention, width: f32) -> Node {
        let colors = theme();
        let inner = width - 16.0 - 18.0 - 2.0;
        let port_conflict = card.crash.is_none() && card.port.is_some();
        let tint = if port_conflict {
            colors.warning
        } else {
            colors.error
        };
        let name = service_title(&card.target);
        let (what, when) = match (&card.crash, card.port) {
            (Some(crash), _) => ("crashed".to_string(), crash.at.map(view::ago)),
            (None, Some(port)) => (format!("cannot bind :{port}"), None),
            (None, None) => ("failed to start".to_string(), None),
        };
        let mut title = div()
            .row()
            .gap(6.0)
            .items_center()
            .child(
                icon(if port_conflict {
                    IconKind::Warning
                } else {
                    IconKind::XCircle
                })
                .size(12.0)
                .color(tint),
            )
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .gap(4.0)
                    .items_center()
                    .child(label(name).medium().color(colors.text))
                    .child(label(what).color(colors.text_muted).truncate()),
            );
        if let Some(when) = when {
            title = title.child(label(when).size(11.0).color(colors.text_placeholder));
        }
        let mut body = div().col().gap(4.0).child(title);
        let detail = match &card.crash {
            Some(crash) => crash.line.clone(),
            None => card.error.as_deref().map(first_line),
        };
        if let Some(detail) = detail {
            body = body.child(
                div().row().w_px(inner).child(
                    label(detail)
                        .size(11.5)
                        .mono()
                        .color(colors.text)
                        .truncate(),
                ),
            );
        }
        if let Some(crash) = &card.crash {
            body = body.child(
                label(format!("Stopped with {}.", crash.exit))
                    .size(11.5)
                    .color(colors.text_muted),
            );
        }
        let button = |action: CardAction, kind: Option<IconKind>, text: &str, tone: Tone| {
            let id = self.card_id(index, action);
            action_button(id, kind, text, tone, self.hot(id))
        };
        let mut buttons = Vec::new();
        if port_conflict {
            buttons.push(button(
                CardAction::NewPort,
                Some(IconKind::ArrowUpRight),
                "Use a new port",
                Tone::Primary,
            ));
        }
        buttons.push(button(
            CardAction::Logs,
            Some(IconKind::File),
            "View logs",
            Tone::Plain,
        ));
        buttons.push(button(
            CardAction::Fix,
            Some(IconKind::Sparkle),
            "Fix with Claude",
            Tone::Agent,
        ));
        if !port_conflict {
            buttons.push(button(
                CardAction::Restart,
                Some(IconKind::RotateCw),
                "",
                Tone::Plain,
            ));
        }
        let hovered = self.hover == Some(self.card_id(index, CardAction::Open));
        div()
            .col()
            .w_px(width - 16.0)
            .p(9.0)
            .gap(6.0)
            .rounded(7.0)
            .border(1.0, tint.alpha(0.3))
            .bg(tint.alpha(if hovered { 0.1 } else { 0.06 }))
            .on_click(self.card_id(index, CardAction::Open))
            .child(body)
            .child(view::pack(buttons, inner, 6.0))
            .into()
    }

    fn filter_bar(&self) -> Node {
        let colors = theme();
        let field = div()
            .row()
            .h_px(26.0)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .rounded(5.0)
            .bg(colors.editor_background)
            .border(
                1.0,
                if self.filter_focused {
                    colors.border_focused
                } else {
                    colors.border_variant
                },
            )
            .on_click(self.base + FILTER)
            .child(
                icon(IconKind::Search)
                    .size(12.0)
                    .color(colors.text_placeholder),
            )
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .items_center()
                    .child(self.filter.render(
                        "Filter by name",
                        self.filter_focused,
                        colors.text,
                        20.0,
                        FieldFont::Ui,
                    )),
            );
        let mut segments = div().row().gap(2.0).items_center();
        for (index, filter) in StatusFilter::ALL.into_iter().enumerate() {
            let id = self.base + STATUS_FILTER + index as u64;
            let count = match filter {
                StatusFilter::All => self.counts.total,
                StatusFilter::Running => self.counts.running + self.counts.busy,
                StatusFilter::Failed => self.counts.attention,
                StatusFilter::Stopped => self.counts.stopped,
            };
            let selected = self.status_filter == filter;
            segments = segments.child(
                div()
                    .row()
                    .h_px(22.0)
                    .px(7.0)
                    .gap(5.0)
                    .items_center()
                    .rounded(4.0)
                    .on_click(id)
                    .bg(if selected {
                        colors.element_selected
                    } else if self.hot(id) {
                        colors.ghost_element_hover
                    } else {
                        Rgba::TRANSPARENT
                    })
                    .child(label(filter.title()).size(12.0).color(if selected {
                        colors.text
                    } else {
                        colors.text_muted
                    }))
                    .child(
                        label(count.to_string())
                            .size(11.0)
                            .color(colors.text_placeholder),
                    ),
            );
        }
        div()
            .col()
            .h_px(FILTER_H)
            .px(8.0)
            .gap(6.0)
            .child(field)
            .child(segments)
            .into()
    }

    /// What scrolls under the filter: the summary card, the cards of what needs attention, then the tree.
    fn blocks(&self, width: f32) -> Vec<(Node, f32)> {
        let mut blocks = Vec::new();
        let mut push = |node: Node| {
            let height = ui::measure(&node).1;
            blocks.push((node, height));
        };
        if self.counts.total > 0 && !self.filtering() {
            push(view::inset(self.summary_card(width)));
            if !self.attention.is_empty() {
                push(view::section("Needs attention", Some(self.attention.len())));
                for (index, card) in self.attention.iter().enumerate() {
                    push(view::inset(self.attention_card(index, card, width)));
                }
            }
        }
        let tree = self.tree_rows();
        if !tree.is_empty() {
            push(view::section("This workspace", None));
        } else {
            let text = if self.filtering() {
                "No services match"
            } else {
                "No services in pom.yml"
            };
            push(
                div()
                    .row()
                    .h_px(ROW_H)
                    .px(12.0)
                    .items_center()
                    .child(label(text).size(12.0).color(theme().text_muted))
                    .into(),
            );
        }
        for index in tree {
            if let Some(row) = self.rows.get(index) {
                blocks.push((self.render_row(index, row), ROW_H));
            }
        }
        blocks
    }

    /// Indices of the rows above the shared zone.
    fn tree_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .take_while(|(_, row)| !is_shared_row(row))
            .map(|(index, _)| index)
            .collect()
    }

    fn shared_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .skip_while(|(_, row)| !is_shared_row(row))
            .map(|(index, _)| index)
            .collect()
    }

    fn fix_with_agent(&mut self, card: &Attention) {
        let name = service_title(&card.target);
        let mut prompt = match &card.crash {
            Some(crash) => format!(
                "The service {name} in this workspace crashed ({}).",
                crash.exit
            ),
            None => format!("The service {name} in this workspace failed to start."),
        };
        if let Some(error) = &card.error {
            prompt.push_str(&format!(" Error: {error}"));
        }
        if let Some(line) = card.crash.as_ref().and_then(|crash| crash.line.as_ref()) {
            prompt.push_str(&format!(" Its output ended with: {line}"));
        }
        let log = self.model.context.runner.holders().crash_log(&card.holder);
        if log.is_file() {
            prompt.push_str(&format!(" The full output is in {}.", log.display()));
        }
        prompt.push_str(" Find the cause and fix it; do not start or stop services yourself.");
        let cwd = if card.target.is_workspace_level() {
            self.root.clone()
        } else {
            self.root.join(&card.target.repo)
        };
        self.requests
            .push(PanelRequest::FixWithAgent(AgentFix { prompt, cwd }));
    }

    fn start_or_stop_all(&mut self, action: Action) {
        let context = self.model.context.clone();
        let Some(config) = context.config() else {
            return;
        };
        for target in context.targets(&config) {
            let holder = context.runner.holder_name(&target);
            let running = self.model.status(&holder) == Status::Running;
            if running == (action == Action::Stop) {
                self.model.run(action, target);
            }
        }
    }

    fn restart_failed(&mut self) {
        for card in self.attention.clone() {
            self.model.run(Action::Restart, card.target);
        }
    }

    fn port(&self, target: &ServiceTarget) -> Option<u16> {
        let config = self.model.context.config()?;
        self.model.context.runner.port(&config, target)
    }

    fn mode(&self, target: &ServiceTarget) -> Option<String> {
        let config = self.model.context.config()?;
        let service = config
            .repos
            .get(&target.repo)?
            .services
            .get(&target.service)?;
        Some(
            self.model
                .context
                .runner
                .mode(&target.repo, &target.service, service),
        )
    }

    fn url(&self, target: &ServiceTarget) -> Option<String> {
        let config = self.model.context.config()?;
        self.model.context.runner.proxy_url(&config, target)
    }

    fn row_targets(&self, row: usize) -> Vec<ServiceTarget> {
        let Some(Row::Group { key, .. }) = self.rows.get(row) else {
            return Vec::new();
        };
        self.model
            .context
            .config()
            .map(|config| {
                self.model
                    .context
                    .targets(&config)
                    .into_iter()
                    .filter(|target| target.repo == *key)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The service's tab: as the preview tab from a row, kept open from an explicit "View logs".
    fn open_tab(&mut self, target: &ServiceTarget, holder: &str, preview: bool) {
        let context = self.model.context.clone();
        let shared = self.model.shared.clone();
        let (target, root) = (target.clone(), self.root.clone());
        let number = self.next_item;
        self.next_item += 1;
        let id = tab::tab_id(holder);
        let open: Box<dyn FnOnce() -> Option<Box<dyn workspace::Item>>> =
            Box::new(move || tab::open_tab(context, shared, target, root, number));
        self.requests.push(if preview {
            PanelRequest::RevealPreview { id, open }
        } else {
            PanelRequest::Reveal { id, open }
        });
    }

    /// Stopping a shared container pulls it from under every workspace, so ask first while services of
    /// other workspaces are running.
    fn run_shared(&mut self, run: SharedRun) {
        if run.action != Action::Stop {
            self.model.run_shared(run);
            return;
        }
        let others = self.other_workspace_services();
        if others == 0 {
            self.model.run_shared(run);
            return;
        }
        let message = if run.name == ALL_SHARED {
            "Stop all shared services?".to_string()
        } else {
            format!("Stop {}?", run.name)
        };
        let noun = if others == 1 { "service" } else { "services" };
        let tag = self.next_item;
        self.next_item += 1;
        self.requests.push(PanelRequest::Prompt {
            tag,
            message,
            detail: Some(format!(
                "Shared services are used by all workspaces; {others} {noun} in other workspaces are still running."
            )),
            buttons: vec!["Stop".to_string(), "Cancel".to_string()],
        });
        self.confirm = Some((tag, run));
    }

    fn other_workspace_services(&self) -> usize {
        let context = &self.model.context;
        let Some(config) = context.config() else {
            return 0;
        };
        let own: HashSet<String> = context
            .targets(&config)
            .iter()
            .map(|target| context.runner.holder_name(target))
            .collect();
        context
            .runner
            .running_holders()
            .into_iter()
            .filter(|holder| !own.contains(holder))
            .count()
    }

    /// Follows a shared container's log in a read-only tab.
    fn open_shared_logs(&mut self, name: &str) {
        let runner = &self.model.context.runner;
        let mut args = vec![
            format!("PATH={}", pom_services::tool_path()),
            "docker".to_string(),
            "compose".to_string(),
            "-f".to_string(),
            runner.compose_file().to_string_lossy().into_owned(),
            "-p".to_string(),
            runner.compose_project(),
            "logs".to_string(),
            "-f".to_string(),
            "--tail".to_string(),
            "200".to_string(),
        ];
        args.push(name.to_string());
        let (root, waker) = (self.root.clone(), self.model.context.waker.clone());
        let item_number = self.next_item;
        self.next_item += 1;
        let item_id = format!("shared-log:{name}");
        let title = format!("{name} (shared)");
        self.requests.push(PanelRequest::Reveal {
            id: item_id.clone(),
            open: Box::new(move || {
                match TerminalItem::command_output(
                    item_number,
                    root,
                    item_id,
                    title,
                    "/usr/bin/env".to_string(),
                    args,
                    Vec::new(),
                    waker,
                ) {
                    Ok(item) => Some(Box::new(item) as Box<dyn workspace::Item>),
                    Err(error) => {
                        eprintln!("services: shared log: {error}");
                        None
                    }
                }
            }),
        });
    }

    fn service_menu(&self, target: &ServiceTarget, holder: &str) -> OpenMenu {
        let context = &self.model.context;
        let config = context.config();
        let state = self.state(holder);
        let running = state == State::Running;
        let mut menu = MenuBuilder::new(self.base + MENU_BASE);
        let mut header = service_title(target);
        if running {
            if let Some(pid) = context.runner.holders().holder_pid(holder) {
                header.push_str(&format!(" - pid {pid}"));
            }
        }
        menu.header(header);
        if running {
            menu.item(
                "Stop",
                MenuAction::Run(Action::Stop),
                IconKind::Stop,
                Some("S"),
            );
            menu.item(
                "Restart",
                MenuAction::Run(Action::Restart),
                IconKind::RotateCw,
                Some("R"),
            );
        } else if state.needs_attention() {
            menu.item(
                "Restart",
                MenuAction::Run(Action::Restart),
                IconKind::RotateCw,
                Some("R"),
            );
        } else {
            menu.push(
                "Start".into(),
                MenuAction::Run(Action::Start),
                Some(IconKind::Play),
                Some("S"),
                matches!(state, State::Busy(_)),
            );
        }
        menu.item(
            "Open in Tab",
            MenuAction::OpenTab,
            IconKind::File,
            Some("Enter"),
        );
        menu.item(
            "View Logs",
            MenuAction::ViewLogs,
            IconKind::Terminal,
            Some("L"),
        );
        let service = config
            .as_ref()
            .and_then(|config| config.repos.get(&target.repo))
            .and_then(|dir| {
                dir.services
                    .get(&target.service)
                    .map(|service| (dir, service))
            });
        if service.is_some_and(|(_, service)| service.has_port()) {
            menu.item(
                "Use a New Port...",
                MenuAction::Run(Action::Relocate),
                IconKind::Server,
                None,
            );
        }
        let mut modes = Vec::new();
        let mut profiles = Vec::new();
        let mut submenus = Vec::new();
        let mut command = String::new();
        if let (Some((dir, service)), Some(config)) = (service, config.as_ref()) {
            modes = service.mode_names();
            let current_mode = context.runner.mode(&target.repo, &target.service, service);
            command = service.active_cmd(&current_mode).trim().to_string();
            profiles = dir.env_profiles(Some(service));
            let current_profile = context.runner.env_profile(config, target);
            menu.sep();
            if modes.len() > 1 {
                let mut sub = MenuBuilder::new(self.base + MENU_BASE + 100);
                for (index, mode) in modes.iter().enumerate() {
                    let item = sub.push(mode.clone(), MenuAction::Mode(index), None, None, false);
                    item.checked = *mode == current_mode;
                    let words: Vec<&str> = service
                        .active_cmd(mode)
                        .split_whitespace()
                        .take(2)
                        .collect();
                    item.hint = Some(words.join(" ").into());
                }
                menu.submenu(
                    workspace::MENU_SUBMENU_BASE,
                    "Mode",
                    IconKind::Grid,
                    &current_mode,
                );
                submenus.push((workspace::MENU_SUBMENU_BASE, sub.items));
            }
            if profiles.len() > 1 {
                let mut sub = MenuBuilder::new(self.base + MENU_BASE + 200);
                for (index, profile) in profiles.iter().enumerate() {
                    let item = sub.push(
                        profile.clone(),
                        MenuAction::Profile(index),
                        None,
                        None,
                        false,
                    );
                    item.checked = *profile == current_profile;
                }
                menu.submenu(
                    workspace::MENU_SUBMENU_BASE + 1,
                    "Env",
                    IconKind::Key,
                    &current_profile,
                );
                submenus.push((workspace::MENU_SUBMENU_BASE + 1, sub.items));
            }
        }
        if self.port(target).is_some() {
            menu.sep();
            menu.push(
                "Open in Browser".into(),
                MenuAction::OpenUrl,
                Some(IconKind::ArrowUpRight),
                None,
                !running,
            );
            menu.item("Copy URL", MenuAction::CopyUrl, IconKind::Copy, None);
        }
        if !command.is_empty() {
            menu.item(
                "Copy Command",
                MenuAction::CopyCommand,
                IconKind::Copy,
                None,
            );
        }
        menu.sep();
        if state.needs_attention() {
            menu.item("Fix with Claude", MenuAction::Fix, IconKind::Sparkle, None);
        } else {
            menu.item(
                "Ask Claude About This Service",
                MenuAction::Ask,
                IconKind::Sparkle,
                None,
            );
        }
        OpenMenu {
            subject: MenuSubject::Service {
                target: target.clone(),
                holder: holder.to_string(),
            },
            items: menu.items,
            submenus,
            modes,
            profiles,
            commands: Vec::new(),
        }
    }

    fn repo_menu(&self, key: &str, row: usize) -> OpenMenu {
        let targets = self.row_targets(row);
        let states: Vec<State> = targets
            .iter()
            .map(|target| self.state(&self.model.context.runner.holder_name(target)))
            .collect();
        let label = if key == WORKSPACE_GROUP {
            "Workspace".to_string()
        } else {
            key.to_string()
        };
        let noun = if targets.len() == 1 {
            "service"
        } else {
            "services"
        };
        let mut menu = MenuBuilder::new(self.base + MENU_BASE);
        menu.header(format!("{label} - {} {noun}", targets.len()));
        menu.push(
            "Start All".into(),
            MenuAction::RepoAll(Action::Start),
            Some(IconKind::Play),
            Some("S"),
            states.iter().all(|state| *state == State::Running),
        );
        menu.push(
            "Stop All".into(),
            MenuAction::RepoAll(Action::Stop),
            Some(IconKind::Stop),
            None,
            !states.contains(&State::Running),
        );
        menu.item(
            "Restart All",
            MenuAction::RepoAll(Action::Restart),
            IconKind::RotateCw,
            Some("R"),
        );
        let commands = if key == WORKSPACE_GROUP {
            Vec::new()
        } else {
            self.repo_commands(Some(key))
        };
        let mut submenus = Vec::new();
        if !commands.is_empty() {
            let mut sub = MenuBuilder::new(self.base + MENU_BASE + 100);
            for (index, command) in commands.iter().enumerate() {
                sub.push(
                    command.label.clone(),
                    MenuAction::RunCommand(index),
                    Some(IconKind::Terminal),
                    None,
                    false,
                );
            }
            menu.sep();
            menu.submenu(
                workspace::MENU_SUBMENU_BASE,
                "Run Task",
                IconKind::Terminal,
                "",
            );
            submenus.push((workspace::MENU_SUBMENU_BASE, sub.items));
        }
        menu.sep();
        if self.environment_button().is_some() {
            menu.item(
                "Environment...",
                MenuAction::Environment,
                IconKind::Key,
                None,
            );
        }
        let open = !self.collapsed.contains(key);
        menu.item(
            if open { "Collapse" } else { "Expand" },
            MenuAction::Fold,
            if open {
                IconKind::ChevronUp
            } else {
                IconKind::ChevronDown
            },
            Some(if open { "Left" } else { "Right" }),
        );
        OpenMenu {
            subject: MenuSubject::Repo {
                key: key.to_string(),
                row,
            },
            items: menu.items,
            submenus,
            modes: Vec::new(),
            profiles: Vec::new(),
            commands,
        }
    }

    fn shared_menu(&self, name: &str) -> OpenMenu {
        let config = self.model.context.config();
        let image = config
            .as_ref()
            .and_then(|config| config.shared_services.get(name))
            .map(|def| def.image.clone())
            .unwrap_or_default();
        let running = self.model.shared_running(name);
        let mut menu = MenuBuilder::new(self.base + MENU_BASE);
        menu.header(if image.is_empty() {
            format!("{name} - shared")
        } else {
            format!("{name} - {image} - shared")
        });
        menu.item(
            "Open in Tab",
            MenuAction::SharedLogs,
            IconKind::File,
            Some("Enter"),
        );
        if running {
            menu.item(
                "Restart",
                MenuAction::Shared(Action::Restart),
                IconKind::RotateCw,
                Some("R"),
            );
        } else {
            menu.item(
                "Start",
                MenuAction::Shared(Action::Start),
                IconKind::Play,
                Some("S"),
            );
        }
        let stop = menu.push(
            "Stop...".into(),
            MenuAction::Shared(Action::Stop),
            Some(IconKind::Stop),
            Some("S"),
            !running,
        );
        stop.danger = true;
        menu.item(
            "View Logs",
            MenuAction::SharedLogs,
            IconKind::Terminal,
            Some("L"),
        );
        menu.sep();
        menu.push(
            "One container for every workspace: stopping it affects all of them.".into(),
            MenuAction::Nothing,
            None,
            None,
            true,
        );
        OpenMenu {
            subject: MenuSubject::Shared {
                name: name.to_string(),
            },
            items: menu.items,
            submenus: Vec::new(),
            modes: Vec::new(),
            profiles: Vec::new(),
            commands: Vec::new(),
        }
    }

    /// The header's Environment button, which the repo menu's "Environment..." opens.
    fn environment_button(&self) -> Option<usize> {
        self.tab_buttons
            .iter()
            .position(|button| button.icon == IconKind::Key)
    }

    fn apply_menu(&mut self, menu: OpenMenu, action: MenuAction) {
        let context = self.model.context.clone();
        match (&menu.subject, action) {
            (_, MenuAction::Nothing) => {}
            (MenuSubject::Service { target, .. }, MenuAction::Run(run)) => {
                self.model.run(run, target.clone())
            }
            (MenuSubject::Service { target, holder }, MenuAction::OpenTab) => {
                self.open_tab(target, holder, false)
            }
            (MenuSubject::Service { target, holder }, MenuAction::ViewLogs) => {
                self.open_tab(target, holder, false)
            }
            (MenuSubject::Service { target, holder }, MenuAction::Fix) => {
                let error = self.model.error(holder);
                let card = Attention {
                    crash: self.model.crash(holder),
                    port: error.as_deref().and_then(port_in),
                    error,
                    target: target.clone(),
                    holder: holder.clone(),
                };
                self.fix_with_agent(&card);
            }
            (MenuSubject::Service { target, .. }, MenuAction::Ask) => {
                let cwd = if target.is_workspace_level() {
                    self.root.clone()
                } else {
                    self.root.join(&target.repo)
                };
                self.requests.push(PanelRequest::FixWithAgent(AgentFix {
                    prompt: format!(
                        "Explain the service {} in this workspace: what it runs, how it is configured in \
                         pom.yml, what it depends on, and anything that looks off.",
                        service_title(target)
                    ),
                    cwd,
                }));
            }
            (MenuSubject::Service { target, .. }, MenuAction::CopyCommand) => {
                let command = context.config().and_then(|config| {
                    let service = config
                        .repos
                        .get(&target.repo)?
                        .services
                        .get(&target.service)?;
                    let mode = context.runner.mode(&target.repo, &target.service, service);
                    Some(service.active_cmd(&mode).trim().to_string())
                });
                if let Some(command) = command {
                    self.requests.push(PanelRequest::Copy(command));
                }
            }
            (MenuSubject::Service { target, .. }, MenuAction::Mode(index)) => {
                let (Some(config), Some(mode)) = (context.config(), menu.modes.get(index)) else {
                    return;
                };
                match context
                    .runner
                    .set_mode(&config, &target.repo, &target.service, mode)
                {
                    Ok(()) => self.restart_if_running(target.clone()),
                    Err(error) => self.requests.push(PanelRequest::Toast(error.to_string())),
                }
            }
            (MenuSubject::Service { target, .. }, MenuAction::Profile(index)) => {
                let (Some(config), Some(profile)) = (context.config(), menu.profiles.get(index))
                else {
                    return;
                };
                match context.runner.set_env_profile(&config, target, profile) {
                    Ok(()) => self.restart_if_running(target.clone()),
                    Err(error) => self.requests.push(PanelRequest::Toast(error.to_string())),
                }
            }
            (MenuSubject::Service { target, .. }, MenuAction::OpenUrl) => {
                if let Some(url) = self.url(target) {
                    self.requests.push(PanelRequest::OpenUrl(url));
                }
            }
            (MenuSubject::Service { target, .. }, MenuAction::CopyUrl) => {
                if let Some(url) = self.url(target) {
                    self.requests.push(PanelRequest::Copy(url));
                }
            }
            (MenuSubject::Repo { row, .. }, MenuAction::RepoAll(run)) => {
                for target in self.row_targets(*row) {
                    let holder = context.runner.holder_name(&target);
                    let running = self.model.status(&holder) == Status::Running;
                    let applies = match run {
                        Action::Start => !running,
                        _ => running,
                    };
                    if applies {
                        self.model.run(run, target);
                    }
                }
            }
            (MenuSubject::Repo { .. }, MenuAction::RunCommand(index)) => {
                if let Some(command) = menu.commands.get(index).cloned() {
                    self.run_command(&command);
                }
            }
            (MenuSubject::Repo { .. }, MenuAction::Environment) => {
                if let Some(index) = self.environment_button() {
                    self.click(self.base + TAB_BUTTON_BASE + index as u64);
                }
            }
            (MenuSubject::Repo { key, .. }, MenuAction::Fold) => self.toggle_group(key.clone()),
            (MenuSubject::Shared { name }, MenuAction::Shared(run)) => self.run_shared(SharedRun {
                name: name.clone(),
                action: run,
            }),
            (MenuSubject::Shared { name }, MenuAction::SharedLogs) => {
                self.open_shared_logs(&name.clone())
            }
            _ => {}
        }
    }

    fn open_menu_for(&mut self, index: usize) -> bool {
        self.menu = match self.rows.get(index).cloned() {
            Some(Row::Service { target, holder }) => Some(self.service_menu(&target, &holder)),
            Some(Row::Group { key, .. }) if key != SHARED_GROUP => {
                Some(self.repo_menu(&key, index))
            }
            Some(Row::Shared { name }) => Some(self.shared_menu(&name)),
            _ => None,
        };
        self.menu.is_some()
    }

    /// Each repo's pre-written commands, for repos checked out in this workspace.
    fn repo_commands(&self, repo: Option<&str>) -> Vec<RepoCommand> {
        let Some(config) = self.model.context.config() else {
            return Vec::new();
        };
        config
            .repos
            .iter()
            .filter(|(name, _)| repo.is_none_or(|repo| repo == name.as_str()))
            .filter(|(name, _)| self.root.join(name).is_dir())
            .flat_map(|(name, dir)| {
                dir.effective_shortcuts()
                    .into_iter()
                    .filter(|shortcut| !shortcut.cmd.trim().is_empty())
                    .map(|shortcut| {
                        let label = [&shortcut.desc, &shortcut.key, &shortcut.cmd]
                            .into_iter()
                            .find(|text| !text.trim().is_empty())
                            .cloned()
                            .unwrap_or_default();
                        RepoCommand {
                            repo: name.clone(),
                            label,
                            cmd: shortcut.cmd,
                        }
                    })
            })
            .collect()
    }

    /// Runs a repo command in a terminal tab, in the repo's worktree with the workspace's env.
    fn run_command(&mut self, command: &RepoCommand) {
        let context = self.model.context.clone();
        let Some(config) = context.config() else {
            return;
        };
        let env = context.runner.workspace_env(&config, &context.branch);
        if let Err(error) = env.write_env_files() {
            self.requests
                .push(PanelRequest::Toast(format!("Env files: {error}")));
        }
        let exports: String = env
            .repo_env(&command.repo)
            .into_iter()
            .map(|(key, value)| format!("export {key}={}; ", pom_services::shell_quote(&value)))
            .collect();
        let worktree = self.root.join(&command.repo);
        let script = format!(
            "export PATH={path}; {exports}cd {dir} && {cmd}; status=$?; printf '\\n[exited with %s]\\n' \"$status\"",
            path = pom_services::shell_quote(pom_services::tool_path()),
            dir = pom_services::shell_quote(&worktree.to_string_lossy()),
            cmd = command.cmd,
        );
        self.requests.push(PanelRequest::RunCommand {
            title: format!("{}: {}", command.repo, command.label),
            cwd: worktree,
            argv: vec!["zsh".into(), "-lc".into(), script],
            env: Vec::new(),
        });
    }

    fn toggle_group(&mut self, key: String) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
    }

    /// A new mode or profile only applies from the next start.
    fn restart_if_running(&mut self, target: ServiceTarget) {
        let holder = self.model.context.runner.holder_name(&target);
        if self.model.status(&holder) == Status::Running {
            self.model.run(Action::Restart, target);
        }
    }
}

impl SidePanelView for ServicesPanel {
    fn kind(&self) -> PaneKind {
        PaneKind::Services
    }

    fn palette_entries(&self) -> Vec<PaletteEntry> {
        let context = &self.model.context;
        let Some(config) = context.config() else {
            return Vec::new();
        };
        let mut entries: Vec<PaletteEntry> = context
            .targets(&config)
            .iter()
            .enumerate()
            .map(|(index, target)| {
                let running =
                    self.model.status(&context.runner.holder_name(target)) == Status::Running;
                PaletteEntry {
                    label: format!(
                        "services: {} {}/{}",
                        if running { "stop" } else { "start" },
                        target.repo,
                        target.service
                    ),
                    id: index as u64 * 2 + u64::from(running),
                }
            })
            .collect();
        entries.extend(
            self.repo_commands(None)
                .into_iter()
                .enumerate()
                .map(|(index, command)| PaletteEntry {
                    label: format!("run: {} {}", command.repo, command.label),
                    id: PALETTE_COMMAND_BASE + index as u64,
                }),
        );
        entries
    }

    fn run_palette_entry(&mut self, id: u64) {
        if let Some(index) = id.checked_sub(PALETTE_COMMAND_BASE) {
            if let Some(command) = self.repo_commands(None).get(index as usize).cloned() {
                self.run_command(&command);
            }
            return;
        }
        let context = self.model.context.clone();
        let Some(config) = context.config() else {
            return;
        };
        if let Some(target) = context.targets(&config).get((id / 2) as usize).cloned() {
            let action = if id % 2 == 1 {
                Action::Stop
            } else {
                Action::Start
            };
            self.model.run(action, target);
        }
    }

    fn render(&mut self, width: f32, height: f32) -> Node {
        if let Ok(mut shared) = self.model.shared.lock() {
            shared.mark_drawn();
        }
        self.viewport_h = height;
        self.rebuild_rows();
        let colors = theme();
        let mut header = div()
            .row()
            .h_px(HEADER_H)
            .pl(12.0)
            .pr(6.0)
            .gap(6.0)
            .items_center()
            .child(label("Services").size(13.0).medium().color(colors.text))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(self.model.context.branch.clone())
                        .size(12.0)
                        .mono()
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            );
        for (index, button) in self.tab_buttons.iter().enumerate() {
            header =
                header.child(self.button(self.base + TAB_BUTTON_BASE + index as u64, button.icon));
        }
        let shared_rows = self.shared_rows();
        let shared_h = (shared_rows.len() as f32 * ROW_H).min(height * 0.4);
        let list_h = (height - HEADER_H - FILTER_H - shared_h - 1.0).max(0.0);
        let blocks = self.blocks(width);
        self.content_h = blocks.iter().map(|(_, block_h)| block_h).sum::<f32>() + 8.0;
        let max_scroll = (self.content_h - list_h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let mut list = div().col().h_px(list_h);
        let (mut top, mut used) = (0.0, 0.0);
        for (node, block_h) in blocks {
            let bottom = top + block_h;
            top = bottom;
            if bottom <= self.scroll {
                continue;
            }
            // Only whole blocks: a partly shown one would paint over the shared zone below.
            if used + block_h > list_h {
                break;
            }
            used += block_h;
            list = list.child(node);
        }
        let mut panel = div()
            .col()
            .w_px(width)
            .h_px(height)
            .child(header)
            .child(self.filter_bar())
            .child(list);
        if !shared_rows.is_empty() {
            let mut zone = div().col().h_px(shared_h).bg(Rgba::new(0.0, 0.0, 0.0, 0.1));
            let fit = (shared_h / ROW_H).floor() as usize;
            for index in shared_rows.into_iter().take(fit) {
                if let Some(row) = self.rows.get(index) {
                    zone = zone.child(self.render_row(index, row));
                }
            }
            panel = panel.child(div().h_px(1.0).bg(colors.border)).child(zone);
        }
        panel.into()
    }

    fn click(&mut self, id: u64) {
        self.filter_focused = id == self.base + FILTER;
        if let Some(button) = self
            .tab_button_index(id)
            .and_then(|index| self.tab_buttons.get(index))
        {
            let open = button.open.clone();
            self.requests.push(PanelRequest::Reveal {
                id: button.id.clone(),
                open: Box::new(move || Some(open())),
            });
            return;
        }
        match id.checked_sub(self.base) {
            Some(FILTER) => return,
            Some(offset) if (STATUS_FILTER..STATUS_FILTER + 4).contains(&offset) => {
                if let Some(filter) = StatusFilter::ALL.get((offset - STATUS_FILTER) as usize) {
                    self.status_filter = *filter;
                    self.scroll = 0.0;
                }
                return;
            }
            Some(START_ALL) => return self.start_or_stop_all(Action::Start),
            Some(STOP_ALL) => return self.start_or_stop_all(Action::Stop),
            Some(RESTART_FAILED) => return self.restart_failed(),
            _ => {}
        }
        if let Some((index, action)) = self.decode_card(id) {
            let Some(card) = self.attention.get(index).cloned() else {
                return;
            };
            match action {
                CardAction::Open => self.open_tab(&card.target, &card.holder, true),
                CardAction::Logs => self.open_tab(&card.target, &card.holder, false),
                CardAction::Fix => self.fix_with_agent(&card),
                CardAction::Restart => self.model.run(Action::Restart, card.target),
                CardAction::NewPort => self.model.run(Action::Relocate, card.target),
            }
            return;
        }
        let Some((index, control)) = self.decode(id) else {
            return;
        };
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        match (row, control) {
            (Row::Group { key, .. }, Control::Row) => self.toggle_group(key),
            (Row::Group { key, .. }, Control::StartAll | Control::StopAll) => {
                let action = if control == Control::StartAll {
                    Action::Start
                } else {
                    Action::Stop
                };
                if key == SHARED_GROUP {
                    self.run_shared(SharedRun {
                        name: ALL_SHARED.to_string(),
                        action,
                    });
                    return;
                }
                for target in self.row_targets(index) {
                    let holder = self.model.context.runner.holder_name(&target);
                    let running = self.model.status(&holder) == Status::Running;
                    if running == (action == Action::Stop) {
                        self.model.run(action, target);
                    }
                }
            }
            (Row::Service { target, holder }, Control::Row) => {
                self.open_tab(&target, &holder, true)
            }
            (Row::Service { target, .. }, Control::Start) => self.model.run(Action::Start, target),
            (Row::Service { target, .. }, Control::Stop) => self.model.run(Action::Stop, target),
            (Row::Service { target, .. }, Control::Restart) => {
                self.model.run(Action::Restart, target)
            }
            (Row::Service { target, holder }, Control::Logs) => {
                self.open_tab(&target, &holder, false)
            }
            (_, Control::More) if self.open_menu_for(index) => {
                self.requests.push(PanelRequest::OpenMenu);
            }
            (Row::Service { target, .. }, Control::OpenUrl) => {
                if let Some(url) = self.url(&target) {
                    self.requests.push(PanelRequest::OpenUrl(url));
                }
            }
            (Row::Shared { name }, Control::Row) => self.open_shared_logs(&name),
            (Row::Shared { name }, Control::Start | Control::Stop | Control::Restart) => {
                let action = match control {
                    Control::Start => Action::Start,
                    Control::Stop => Action::Stop,
                    _ => Action::Restart,
                };
                self.run_shared(SharedRun { name, action });
            }
            _ => {}
        }
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        let id = id.filter(|id| self.ours(*id));
        if self.hover == id {
            return false;
        }
        self.hover = id;
        true
    }

    fn scroll(&mut self, dy: f32) -> bool {
        let max_scroll = (self.content_h - self.viewport_h).max(0.0);
        let next = (self.scroll - dy).clamp(0.0, max_scroll);
        if (next - self.scroll).abs() < 0.01 {
            return false;
        }
        self.scroll = next;
        true
    }

    fn text_focused(&self) -> bool {
        self.filter_focused
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return false;
        }
        self.filter.insert(&typed);
        self.scroll = 0.0;
        true
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape if self.filter.text().is_empty() => self.filter_focused = false,
            EditKey::Escape => self.filter.set_text(""),
            EditKey::Enter => self.filter_focused = false,
            _ => return self.filter.key(key, shift),
        }
        true
    }

    fn blur(&mut self) {
        self.filter_focused = false;
    }

    fn open_menu(&mut self, id: u64) -> bool {
        let Some((index, _)) = self.decode(id) else {
            return false;
        };
        self.open_menu_for(index)
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        self.menu
            .as_ref()
            .map(|menu| menu.items.iter().map(|(item, _)| item.clone()).collect())
            .unwrap_or_default()
    }

    fn submenu_items(&self, id: u64) -> Vec<MenuItem> {
        self.menu
            .as_ref()
            .and_then(|menu| menu.submenus.iter().find(|(parent, _)| *parent == id))
            .map(|(_, items)| items.iter().map(|(item, _)| item.clone()).collect())
            .unwrap_or_default()
    }

    fn menu_action(&mut self, item: u64) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let action = menu
            .items
            .iter()
            .chain(menu.submenus.iter().flat_map(|(_, items)| items.iter()))
            .find(|(entry, _)| entry.id == item)
            .map(|(_, action)| *action);
        if let Some(action) = action {
            self.apply_menu(menu, action);
        }
    }

    fn prompt_answered(&mut self, tag: u64, answer: usize) {
        match self.confirm.take() {
            Some((asked, run)) if asked == tag && answer == 0 => self.model.run_shared(run),
            Some((asked, run)) if asked != tag => self.confirm = Some((asked, run)),
            _ => {}
        }
    }

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        for request in self.model.take_tab_requests() {
            match request {
                model::TabRequest::Fix(target) => {
                    let holder = self.model.context.runner.holder_name(&target);
                    let error = self.model.error(&holder);
                    let card = Attention {
                        crash: self.model.crash(&holder),
                        port: error.as_deref().and_then(port_in),
                        error,
                        target,
                        holder,
                    };
                    self.fix_with_agent(&card);
                }
                model::TabRequest::OpenUrl(url) => self.requests.push(PanelRequest::OpenUrl(url)),
                model::TabRequest::Copy(text) => self.requests.push(PanelRequest::Copy(text)),
                model::TabRequest::OpenFile(path) => {
                    self.requests.push(PanelRequest::OpenFile(path))
                }
            }
        }
        let mut requests = std::mem::take(&mut self.requests);
        requests.extend(
            self.model
                .take_toasts()
                .into_iter()
                .map(PanelRequest::Toast),
        );
        requests
    }
}

fn is_shared_row(row: &Row) -> bool {
    matches!(row, Row::Shared { .. })
        || matches!(row, Row::Group { key, .. } if key == SHARED_GROUP)
}

/// "api > web", or just the name of a workspace-level service.
pub(crate) fn service_title(target: &ServiceTarget) -> String {
    if target.is_workspace_level() {
        target.service.clone()
    } else {
        format!("{} > {}", target.repo, target.service)
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_string()
}

/// The port a start error is about ("port 6006 is in use", "address :6006 already in use").
pub(crate) fn port_in(message: &str) -> Option<u16> {
    let lower = message.to_ascii_lowercase();
    if !lower.contains("port") && !lower.contains("in use") {
        return None;
    }
    lower
        .split(|c: char| !c.is_ascii_digit())
        .filter(|digits| (2..=5).contains(&digits.len()))
        .find_map(|digits| digits.parse::<u16>().ok().filter(|port| *port >= 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_error_names_the_port_it_could_not_bind() {
        assert_eq!(port_in("port 6006 is already in use by node"), Some(6006));
        assert_eq!(
            port_in("listen tcp :3001: address already in use"),
            Some(3001)
        );
        assert_eq!(port_in("command not found: rails"), None);
    }

    #[test]
    fn a_crash_is_told_by_its_last_error_line() {
        let output = "\u{1b}[32mBooting\u{1b}[0m\r\nKeyError: key not found: \"REDIS_URL\"\n  at queue.rb:4\n";
        let clean = model::strip_ansi(output);
        assert_eq!(
            model::telling_line(&clean).as_deref(),
            Some("KeyError: key not found: \"REDIS_URL\"")
        );
        assert_eq!(
            model::telling_line("just\nlines\n").as_deref(),
            Some("lines")
        );
    }
}
