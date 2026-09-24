//! The Services panel: a tree of the active workspace's services (workspace-level, then per repo) with a
//! status dot, port and hover controls. Clicking a running service opens its console as a center tab.

mod model;

use std::collections::HashSet;
use std::path::PathBuf;

use pom_services::ServiceTarget;
use terminal_ui::TerminalItem;
use ui::{div, icon, label, theme, IconKind, Node, Rgba};
use workspace::{MenuItem, PaneKind, PanelRequest, SidePanelView};

pub use model::{Action, ServicesContext, SharedConfig, Status};
use model::{Model, WORKSPACE_GROUP};

const ROW_H: f32 = 22.0;
const INDENT: f32 = 16.0;
const HEADER_H: f32 = 28.0;
const BUTTON: f32 = 18.0;
/// Hit ids per row: the row itself plus its controls.
const ROW_STRIDE: u64 = 16;
const MENU_BASE: u64 = 9_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Control {
    Row = 0,
    Start = 1,
    Stop = 2,
    Restart = 3,
    OpenUrl = 4,
    StartAll = 5,
    StopAll = 6,
    NewPort = 7,
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
            Control::NewPort,
        ]
        .get(offset as usize)
        .copied()
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
    },
    Service {
        target: ServiceTarget,
        holder: String,
    },
    Error {
        target: ServiceTarget,
        message: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MenuAction {
    Run(Action),
    Mode(usize),
    Profile(usize),
    OpenUrl,
    CopyUrl,
}

struct OpenMenu {
    target: ServiceTarget,
    items: Vec<(MenuItem, MenuAction)>,
    modes: Vec<String>,
    profiles: Vec<String>,
}

pub struct ServicesPanel {
    model: Model,
    /// Where consoles start (the workspace root).
    root: PathBuf,
    base: u64,
    rows: Vec<Row>,
    collapsed: HashSet<String>,
    hover: Option<u64>,
    scroll: f32,
    viewport_h: f32,
    menu: Option<OpenMenu>,
    requests: Vec<PanelRequest>,
    next_item: u64,
}

impl ServicesPanel {
    pub fn new(context: ServicesContext, root: PathBuf) -> ServicesPanel {
        ServicesPanel {
            model: Model::new(context),
            root,
            base: workspace::side_panel_base(PaneKind::Services),
            rows: Vec::new(),
            collapsed: HashSet::new(),
            hover: None,
            scroll: 0.0,
            viewport_h: 0.0,
            menu: None,
            requests: Vec::new(),
            next_item: 1 << 40,
        }
    }

    /// The last known status of a service (`repo` is `_ws` for a workspace-level one).
    pub fn status(&self, repo: &str, service: &str) -> Status {
        let target = self.model.context.target(repo, service);
        self.model
            .status(&self.model.context.runner.holder_name(&target))
    }

    fn rebuild_rows(&mut self) {
        let context = &self.model.context;
        let Some(config) = context.config() else {
            self.rows.clear();
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
        let mut rows = Vec::new();
        for (key, group_label, targets) in groups {
            let holders: Vec<String> = targets
                .iter()
                .map(|target| context.runner.holder_name(target))
                .collect();
            let running = holders
                .iter()
                .filter(|holder| self.model.status(holder) == Status::Running)
                .count();
            let collapsed = self.collapsed.contains(&key);
            rows.push(Row::Group {
                key,
                label: group_label,
                running,
                total: targets.len(),
                collapsed,
            });
            if collapsed {
                continue;
            }
            for (target, holder) in targets.into_iter().zip(holders) {
                let error = self.model.error(&holder);
                rows.push(Row::Service {
                    target: target.clone(),
                    holder,
                });
                if let Some(message) = error {
                    rows.push(Row::Error { target, message });
                }
            }
        }
        self.rows = rows;
    }

    fn id(&self, row: usize, control: Control) -> u64 {
        self.base + row as u64 * ROW_STRIDE + control as u64
    }

    fn decode(&self, id: u64) -> Option<(usize, Control)> {
        let offset = id.checked_sub(self.base)?;
        if offset >= MENU_BASE {
            return None;
        }
        let row = (offset / ROW_STRIDE) as usize;
        Some((row, Control::from_offset(offset % ROW_STRIDE)?))
    }

    fn hovered_row(&self) -> Option<usize> {
        self.hover
            .and_then(|id| self.decode(id))
            .map(|(row, _)| row)
    }

    fn button(&self, id: u64, kind: IconKind) -> Node {
        let hot = self.hover == Some(id);
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
        let hovered = self.hovered_row() == Some(index);
        let mut body = div()
            .row()
            .h_px(ROW_H)
            .pl(8.0)
            .pr(4.0)
            .items_center()
            .rounded(4.0);
        if hovered {
            body = body.bg(theme().ghost_element_hover);
        }
        match row {
            Row::Group {
                label: text,
                running,
                total,
                collapsed,
                ..
            } => {
                let chevron = if *collapsed {
                    IconKind::ChevronRight
                } else {
                    IconKind::ChevronDown
                };
                let trailing: Node = if hovered {
                    div()
                        .row()
                        .gap(2.0)
                        .child(self.button(self.id(index, Control::StartAll), IconKind::Play))
                        .child(self.button(self.id(index, Control::StopAll), IconKind::Stop))
                        .into()
                } else {
                    label(format!("{running}/{total}"))
                        .size(11.0)
                        .color(theme().text_muted)
                        .into()
                };
                body.on_click(self.id(index, Control::Row))
                    .child(
                        div()
                            .row()
                            .w_px(INDENT)
                            .h_px(ROW_H)
                            .items_center()
                            .justify_center()
                            .child(icon(chevron).size(12.0).color(theme().icon_muted)),
                    )
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .pl(4.0)
                            .items_center()
                            .child(label(text.clone()).color(theme().text).truncate()),
                    )
                    .child(trailing)
                    .into()
            }
            Row::Service { target, holder } => {
                let status = self.model.status(holder);
                let pending = self.model.pending(holder);
                let dot = match status {
                    Status::Running => theme().success,
                    Status::Crashed => theme().error,
                    Status::Stopped => theme().icon_muted,
                };
                let trailing: Node = match (pending, hovered) {
                    (Some(action), _) => label(action.progress_label())
                        .size(11.0)
                        .color(theme().text_muted)
                        .into(),
                    (None, true) if status == Status::Running => {
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
                            .child(self.button(self.id(index, Control::Stop), IconKind::Stop))
                            .into()
                    }
                    (None, true) => self.button(self.id(index, Control::Start), IconKind::Play),
                    (None, false) => {
                        let mut details = Vec::new();
                        if let Some(mode) = self.mode(target).filter(|mode| !mode.is_empty()) {
                            details.push(mode);
                        }
                        if let Some(port) = self.port(target) {
                            details.push(format!(":{port}"));
                        }
                        label(details.join("  "))
                            .size(11.0)
                            .color(theme().text_muted)
                            .into()
                    }
                };
                body.on_click(self.id(index, Control::Row))
                    .child(self.guide_column())
                    .child(
                        div()
                            .row()
                            .w_px(INDENT)
                            .h_px(ROW_H)
                            .items_center()
                            .justify_center()
                            .child(div().w_px(6.0).h_px(6.0).rounded(3.0).bg(dot)),
                    )
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .pl(2.0)
                            .items_center()
                            .child(label(target.service.clone()).color(theme().text).truncate()),
                    )
                    .child(trailing)
                    .into()
            }
            Row::Error { message, .. } => {
                let first_line = message.lines().next().unwrap_or_default().to_string();
                let mut line =
                    div()
                        .row()
                        .child(self.guide_column())
                        .child(div().w_px(INDENT))
                        .child(
                            div().row().flex(1.0).pl(2.0).items_center().child(
                                label(first_line).size(11.0).color(theme().error).truncate(),
                            ),
                        );
                if message.contains("port") {
                    let id = self.id(index, Control::NewPort);
                    let hot = self.hover == Some(id);
                    line = line.child(
                        div()
                            .row()
                            .h_px(ROW_H - 4.0)
                            .px(6.0)
                            .rounded(4.0)
                            .items_center()
                            .on_click(id)
                            .bg(if hot {
                                theme().element_hover
                            } else {
                                Rgba::TRANSPARENT
                            })
                            .child(
                                label("Use a new port")
                                    .size(11.0)
                                    .color(theme().text_accent),
                            ),
                    );
                }
                body.child(line).into()
            }
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
        self.model.context.runner.url(&config, target)
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

    /// A running service's console, or the output a crashed one left.
    fn open_console(&mut self, target: &ServiceTarget, holder: &str) {
        let runner = &self.model.context.runner;
        let holders = runner.holders().clone();
        let title = if target.is_workspace_level() {
            target.service.clone()
        } else {
            format!("{}/{}", target.repo, target.service)
        };
        let (root, waker) = (self.root.clone(), self.model.context.waker.clone());
        let item_number = self.next_item;
        self.next_item += 1;
        let name = holder.to_string();
        match self.model.status(holder) {
            Status::Running => {
                let binary = std::env::current_exe().unwrap_or_default();
                self.requests.push(PanelRequest::Reveal {
                    id: format!("service:{name}"),
                    open: Box::new(move || {
                        let options = terminal::HolderOptions {
                            dir: holders,
                            name,
                            binary,
                            attach_only: true,
                        };
                        let item_id = format!("service:{}", options.name);
                        match TerminalItem::service_console(
                            item_number,
                            root,
                            item_id,
                            title,
                            options,
                            waker,
                        ) {
                            Ok(item) => Some(Box::new(item) as Box<dyn workspace::Item>),
                            Err(error) => {
                                eprintln!("services: console: {error}");
                                None
                            }
                        }
                    }),
                });
            }
            Status::Crashed => {
                let log = holders.crash_log(&name);
                self.requests.push(PanelRequest::Reveal {
                    id: format!("service-log:{name}"),
                    open: Box::new(move || {
                        let item_id = format!("service-log:{name}");
                        match TerminalItem::service_log(
                            item_number,
                            root,
                            item_id,
                            format!("{title} (crashed)"),
                            &log,
                            waker,
                        ) {
                            Ok(item) => Some(Box::new(item) as Box<dyn workspace::Item>),
                            Err(error) => {
                                eprintln!("services: log: {error}");
                                None
                            }
                        }
                    }),
                });
            }
            Status::Stopped => {}
        }
    }

    fn content_height(&self) -> f32 {
        HEADER_H + self.rows.len() as f32 * ROW_H + 8.0
    }

    fn build_menu(&self, target: &ServiceTarget, holder: &str) -> OpenMenu {
        let context = &self.model.context;
        let config = context.config();
        let status = self.model.status(holder);
        let mut items: Vec<(MenuItem, MenuAction)> = Vec::new();
        let mut push = |label: String, action: MenuAction, checked: bool, sep: bool| {
            let id = self.base + MENU_BASE + items.len() as u64;
            items.push((
                MenuItem {
                    id,
                    label: label.into(),
                    checked,
                    sep,
                    disabled: false,
                },
                action,
            ));
        };
        if status == Status::Running {
            push(
                "Restart".into(),
                MenuAction::Run(Action::Restart),
                false,
                false,
            );
            push("Stop".into(), MenuAction::Run(Action::Stop), false, false);
        } else {
            push("Start".into(), MenuAction::Run(Action::Start), false, false);
        }
        let port = self.port(target);
        let service = config
            .as_ref()
            .and_then(|config| config.repos.get(&target.repo))
            .and_then(|dir| {
                dir.services
                    .get(&target.service)
                    .map(|service| (dir, service))
            });
        if service.is_some_and(|(_, service)| service.has_port()) {
            push(
                "Use a New Port".into(),
                MenuAction::Run(Action::Relocate),
                false,
                false,
            );
        }
        let mut modes = Vec::new();
        let mut profiles = Vec::new();
        if let (Some((dir, service)), Some(config)) = (service, config.as_ref()) {
            modes = service.mode_names();
            let current_mode = context.runner.mode(&target.repo, &target.service, service);
            for (index, mode) in modes.iter().enumerate() {
                push(
                    format!("Mode: {mode}"),
                    MenuAction::Mode(index),
                    *mode == current_mode,
                    index == 0,
                );
            }
            profiles = dir.env_profiles(Some(service));
            if profiles.len() > 1 {
                let current = context.runner.env_profile(config, target);
                for (index, profile) in profiles.iter().enumerate() {
                    push(
                        format!("Env: {profile}"),
                        MenuAction::Profile(index),
                        *profile == current,
                        index == 0,
                    );
                }
            }
        }
        if port.is_some() {
            push("Open in Browser".into(), MenuAction::OpenUrl, false, true);
            push("Copy URL".into(), MenuAction::CopyUrl, false, false);
        }
        OpenMenu {
            target: target.clone(),
            items,
            modes,
            profiles,
        }
    }

    fn apply_menu(&mut self, menu: OpenMenu, action: MenuAction) {
        let context = self.model.context.clone();
        let target = menu.target;
        match action {
            MenuAction::Run(run) => self.model.run(run, target),
            MenuAction::Mode(index) => {
                let (Some(config), Some(mode)) = (context.config(), menu.modes.get(index)) else {
                    return;
                };
                match context
                    .runner
                    .set_mode(&config, &target.repo, &target.service, mode)
                {
                    Ok(()) => self.restart_if_running(target),
                    Err(error) => self.requests.push(PanelRequest::Toast(error.to_string())),
                }
            }
            MenuAction::Profile(index) => {
                let (Some(config), Some(profile)) = (context.config(), menu.profiles.get(index))
                else {
                    return;
                };
                match context.runner.set_env_profile(&config, &target, profile) {
                    Ok(()) => self.restart_if_running(target),
                    Err(error) => self.requests.push(PanelRequest::Toast(error.to_string())),
                }
            }
            MenuAction::OpenUrl => {
                if let Some(url) = self.url(&target) {
                    self.requests.push(PanelRequest::OpenUrl(url));
                }
            }
            MenuAction::CopyUrl => {
                if let Some(url) = self.url(&target) {
                    self.requests.push(PanelRequest::Copy(url));
                }
            }
        }
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

    fn render(&mut self, width: f32, height: f32) -> Node {
        if let Ok(mut shared) = self.model.shared.lock() {
            shared.mark_drawn();
        }
        self.viewport_h = height;
        self.rebuild_rows();
        let max_scroll = (self.content_height() - height).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let branch = self.model.context.branch.clone();
        let header = div()
            .row()
            .h_px(HEADER_H)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .child(label("Services").size(12.0).color(theme().text_muted))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(branch)
                        .size(11.0)
                        .color(theme().text_placeholder)
                        .truncate(),
                ),
            );
        let mut list = div().col().px(4.0);
        if self.rows.is_empty() {
            list = list.child(
                div().row().h_px(ROW_H).px(8.0).items_center().child(
                    label("No services in pom.yml")
                        .size(12.0)
                        .color(theme().text_muted),
                ),
            );
        }
        let first = (self.scroll / ROW_H).floor() as usize;
        let visible = (height / ROW_H).ceil() as usize + 2;
        let offset = first as f32 * ROW_H - self.scroll;
        list = list.child(div().h_px(offset.max(0.0)));
        for (index, row) in self.rows.iter().enumerate().skip(first).take(visible) {
            list = list.child(self.render_row(index, row));
        }
        div()
            .col()
            .w_px(width)
            .h_px(height)
            .child(header)
            .child(list)
            .into()
    }

    fn click(&mut self, id: u64) {
        let Some((index, control)) = self.decode(id) else {
            return;
        };
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        match (row, control) {
            (Row::Group { key, .. }, Control::Row) => self.toggle_group(key),
            (Row::Group { .. }, Control::StartAll | Control::StopAll) => {
                let action = if control == Control::StartAll {
                    Action::Start
                } else {
                    Action::Stop
                };
                for target in self.row_targets(index) {
                    let holder = self.model.context.runner.holder_name(&target);
                    let running = self.model.status(&holder) == Status::Running;
                    if running == (action == Action::Stop) {
                        self.model.run(action, target);
                    }
                }
            }
            (Row::Service { target, holder }, Control::Row) => self.open_console(&target, &holder),
            (Row::Service { target, .. }, Control::Start) => self.model.run(Action::Start, target),
            (Row::Service { target, .. }, Control::Stop) => self.model.run(Action::Stop, target),
            (Row::Service { target, .. }, Control::Restart) => {
                self.model.run(Action::Restart, target)
            }
            (Row::Service { target, .. }, Control::OpenUrl) => {
                if let Some(url) = self.url(&target) {
                    self.requests.push(PanelRequest::OpenUrl(url));
                }
            }
            (Row::Error { target, .. }, Control::NewPort) => {
                self.model.run(Action::Relocate, target)
            }
            _ => {}
        }
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        let id = id.filter(|id| self.decode(*id).is_some());
        if self.hover == id {
            return false;
        }
        self.hover = id;
        true
    }

    fn scroll(&mut self, dy: f32) -> bool {
        let max_scroll = (self.content_height() - self.viewport_h).max(0.0);
        let next = (self.scroll - dy).clamp(0.0, max_scroll);
        if (next - self.scroll).abs() < 0.01 {
            return false;
        }
        self.scroll = next;
        true
    }

    fn open_menu(&mut self, id: u64) -> bool {
        let Some((index, _)) = self.decode(id) else {
            return false;
        };
        let Some(Row::Service { target, holder }) = self.rows.get(index).cloned() else {
            return false;
        };
        self.menu = Some(self.build_menu(&target, &holder));
        true
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        self.menu
            .as_ref()
            .map(|menu| menu.items.iter().map(|(item, _)| item.clone()).collect())
            .unwrap_or_default()
    }

    fn menu_action(&mut self, item: u64) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let action = menu
            .items
            .iter()
            .find(|(entry, _)| entry.id == item)
            .map(|(_, action)| *action);
        if let Some(action) = action {
            self.apply_menu(menu, action);
        }
    }

    fn take_requests(&mut self) -> Vec<PanelRequest> {
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
