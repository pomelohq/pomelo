//! The Services panel of a project on another machine: what its last look over ssh found (services, shared
//! services, agents), start/stop/restart, a service's recent output and an agent's last turn. It reads a
//! [`pom_remote::Watch`], which keeps asking while the window is seen.

use pom_remote::{Connection, ServiceAction, Snapshot, Watch};
use ui::{div, icon, label, theme, IconKind, Node, Rgba};
use workspace::{MenuItem, PaneKind, PanelRequest, SidePanelView};

use crate::view::{self, State};

const ROW_H: f32 = 26.0;
const HEADER_H: f32 = 34.0;
const BUTTON: f32 = 20.0;
const LOG_LINE_H: f32 = 15.0;

const CLOSE_DETAIL: u64 = 1;
const ROW_BASE: u64 = 1_000;
const ROW_STRIDE: u64 = 8;
const AGENT_BASE: u64 = 500_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Row,
    Start,
    Stop,
    Restart,
    Logs,
}

impl Control {
    const ALL: [Control; 5] = [
        Control::Row,
        Control::Start,
        Control::Stop,
        Control::Restart,
        Control::Logs,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Detail {
    Logs(String),
    Turn(String),
}

pub struct RemoteServicesPanel {
    watch: Watch,
    base: u64,
    hover: Option<u64>,
    scroll: f32,
    content_h: f32,
    viewport_h: f32,
    detail: Option<Detail>,
    /// `repo/service` of each service row as last drawn, by row index, and whether it runs.
    targets: Vec<(String, bool)>,
    roles: Vec<String>,
    requests: Vec<PanelRequest>,
    next_item: u64,
}

/// Item ids of the terminals this panel opens, clear of the terminal panel's own.
const ITEM_BASE: u64 = 1 << 40;

impl RemoteServicesPanel {
    pub fn new(watch: Watch) -> RemoteServicesPanel {
        RemoteServicesPanel {
            watch,
            base: workspace::side_panel_base(PaneKind::Services),
            hover: None,
            scroll: 0.0,
            content_h: 0.0,
            viewport_h: 0.0,
            detail: None,
            targets: Vec::new(),
            roles: Vec::new(),
            requests: Vec::new(),
            next_item: ITEM_BASE,
        }
    }

    /// The live terminal of a running service, or of an agent, when the host serves terminals.
    fn open_terminal(&mut self, target: terminal::RemoteTarget, title: String) -> bool {
        let Some(provider) = self.watch.terminals() else {
            return false;
        };
        let snapshot = self.watch.snapshot();
        let number = self.next_item;
        self.next_item += 1;
        let service = match target {
            terminal::RemoteTarget::Agent(role) => {
                let watch = self.watch.clone();
                let id = crate::agent_item_id(&snapshot.branch, &role);
                let open = Box::new(move || crate::remote_agent_item(&watch, &role, number));
                self.requests.push(PanelRequest::RevealAgent { id, open });
                return true;
            }
            terminal::RemoteTarget::Service(service) => service,
            terminal::RemoteTarget::Shell { .. } | terminal::RemoteTarget::Holder(_) => {
                return false
            }
        };
        let root = snapshot
            .workspaces
            .as_ref()
            .and_then(|list| {
                list.workspaces
                    .iter()
                    .find(|workspace| workspace.branch == snapshot.branch)
            })
            .map(|workspace| workspace.path.clone())
            .unwrap_or_default();
        let id = format!("service:{}/{service}", snapshot.branch);
        let item_id = id.clone();
        let open = Box::new(move || {
            let remote = terminal::RemoteOptions {
                provider,
                target: terminal::RemoteTarget::Service(service),
                mode: terminal::RemoteMode::ReadWrite,
            };
            match terminal_ui::TerminalItem::remote_console(
                number,
                root,
                item_id,
                title,
                remote,
                false,
                std::sync::Arc::new(ui::wake),
            ) {
                Ok(item) => Some(Box::new(item) as Box<dyn workspace::Item>),
                Err(error) => {
                    eprintln!("remote terminal: {error}");
                    None
                }
            }
        });
        self.requests
            .push(PanelRequest::RevealTerminal { id, open });
        true
    }

    /// Shows `service`'s recent output in place of the list.
    pub fn show_logs(&mut self, service: &str) {
        self.open_detail(Some(Detail::Logs(service.to_string())));
    }

    fn ours(&self, id: u64) -> bool {
        id >= self.base && id < self.base + 10_000_000
    }

    fn hot(&self, id: u64) -> bool {
        self.hover == Some(id)
    }

    fn row_id(&self, index: usize, control: Control) -> u64 {
        let offset = Control::ALL
            .iter()
            .position(|known| *known == control)
            .unwrap_or_default() as u64;
        self.base + ROW_BASE + index as u64 * ROW_STRIDE + offset
    }

    fn decode_row(&self, id: u64) -> Option<(usize, Control)> {
        let offset = id.checked_sub(self.base + ROW_BASE)?;
        if offset >= AGENT_BASE - ROW_BASE {
            return None;
        }
        let control = *Control::ALL.get((offset % ROW_STRIDE) as usize)?;
        Some(((offset / ROW_STRIDE) as usize, control))
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

    fn header(&self, snapshot: &Snapshot) -> Node {
        let colors = theme();
        div()
            .row()
            .h_px(HEADER_H)
            .pl(12.0)
            .pr(6.0)
            .gap(6.0)
            .items_center()
            .child(label("Services").size(13.0).medium().color(colors.text))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(snapshot.branch.clone())
                        .size(12.0)
                        .mono()
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            )
            .into()
    }

    fn service_state(
        &self,
        snapshot: &Snapshot,
        label_text: &str,
        up: bool,
        crashed: bool,
    ) -> State {
        if snapshot.busy.iter().any(|busy| busy == label_text) {
            State::Busy("working")
        } else if up {
            State::Running
        } else if crashed {
            State::Crashed
        } else {
            State::Stopped
        }
    }

    fn rows(&mut self, snapshot: &Snapshot) -> Vec<Node> {
        let colors = theme();
        let mut nodes: Vec<Node> = Vec::new();
        self.targets.clear();
        self.roles.clear();
        let Some(status) = &snapshot.status else {
            let text = match (&snapshot.connection, &snapshot.problem) {
                (_, Some(problem)) => problem.clone(),
                (Connection::Disconnected { .. }, None) => "Nothing read yet.".to_string(),
                _ => format!("Asking {}...", self.watch.location()),
            };
            nodes.push(
                div()
                    .row()
                    .px(12.0)
                    .py(10.0)
                    .child(
                        label(text)
                            .size(12.0)
                            .color(colors.text_placeholder)
                            .truncate(),
                    )
                    .into(),
            );
            return nodes;
        };
        let mut repo = String::new();
        for service in &status.services {
            if service.repo != repo {
                repo = service.repo.clone();
                nodes.push(view::section(&repo, None));
            }
            let name = format!("{}/{}", service.repo, service.service);
            let index = self.targets.len();
            self.targets.push((name.clone(), service.up));
            let state = self.service_state(snapshot, &name, service.up, service.crashed);
            let hovered = Control::ALL
                .iter()
                .any(|control| self.hot(self.row_id(index, *control)));
            let trailing: Node = match (&state, hovered) {
                (State::Busy(text), _) => label(*text).size(11.0).color(colors.text_muted).into(),
                (State::Running, true) => div()
                    .row()
                    .gap(2.0)
                    .child(self.button(self.row_id(index, Control::Restart), IconKind::RotateCw))
                    .child(self.button(self.row_id(index, Control::Logs), IconKind::Terminal))
                    .child(self.button(self.row_id(index, Control::Stop), IconKind::Stop))
                    .into(),
                (_, true) => div()
                    .row()
                    .gap(2.0)
                    .child(self.button(self.row_id(index, Control::Start), IconKind::Play))
                    .child(self.button(self.row_id(index, Control::Logs), IconKind::Terminal))
                    .into(),
                (State::Crashed, false) => label("crashed").size(11.0).color(colors.error).into(),
                (_, false) => match service.port {
                    Some(port) => label(format!(":{port}"))
                        .size(11.0)
                        .mono()
                        .color(colors.text_placeholder)
                        .into(),
                    None => div().into(),
                },
            };
            let mut name_cell = div()
                .row()
                .flex(1.0)
                .gap(6.0)
                .items_center()
                .child(label(service.service.clone()).color(colors.text).truncate());
            if !service.mode.is_empty() {
                name_cell = name_cell.child(view::tag(&service.mode));
            }
            if service.up && !service.ready {
                name_cell =
                    name_cell.child(label("starting").size(11.0).color(colors.text_placeholder));
            }
            let mut row = div()
                .row()
                .h_px(ROW_H)
                .pl(12.0)
                .pr(6.0)
                .gap(6.0)
                .items_center()
                .on_click(self.row_id(index, Control::Row))
                .child(view::status_icon(&state))
                .child(name_cell)
                .child(trailing);
            if hovered {
                row = row.bg(colors.ghost_element_hover);
            }
            nodes.push(row.into());
        }
        if !status.shared.is_empty() {
            nodes.push(view::section("Shared", Some(status.shared.len())));
            for shared in &status.shared {
                let state = match (shared.up, shared.ready) {
                    (true, true) => State::Running,
                    (true, false) => State::Busy("starting"),
                    _ => State::Stopped,
                };
                nodes.push(
                    div()
                        .row()
                        .h_px(ROW_H)
                        .pl(12.0)
                        .pr(10.0)
                        .gap(6.0)
                        .items_center()
                        .child(view::status_icon(&state))
                        .child(
                            div()
                                .row()
                                .flex(1.0)
                                .child(label(shared.name.clone()).color(colors.text).truncate()),
                        )
                        .child(
                            label(format!(":{}", shared.port))
                                .size(11.0)
                                .mono()
                                .color(colors.text_placeholder),
                        )
                        .into(),
                );
            }
        }
        nodes.push(view::section("Agents", Some(snapshot.agents.len())));
        if snapshot.agents.is_empty() {
            nodes.push(
                div()
                    .row()
                    .px(12.0)
                    .h_px(ROW_H)
                    .items_center()
                    .child(
                        label("No agent sessions in this workspace")
                            .size(12.0)
                            .color(colors.text_placeholder),
                    )
                    .into(),
            );
        }
        for (index, agent) in snapshot.agents.iter().enumerate() {
            self.roles.push(agent.role.clone());
            let id = self.base + AGENT_BASE + index as u64;
            let state_text = match &agent.reason {
                Some(reason) => format!("{} ({reason})", agent.state.replace('_', " ")),
                None => agent.state.replace('_', " "),
            };
            let tint = match agent.state.as_str() {
                "thinking" | "tool_use" | "compacting" => colors.text_accent,
                "awaiting_input" => colors.warning,
                "died" => colors.error,
                _ => colors.border,
            };
            let mut row = div()
                .row()
                .h_px(ROW_H)
                .pl(12.0)
                .pr(10.0)
                .gap(6.0)
                .items_center()
                .on_click(id)
                .child(
                    div()
                        .row()
                        .w_px(14.0)
                        .justify_center()
                        .child(view::dot(tint, 8.0)),
                )
                .child(
                    div()
                        .row()
                        .flex(1.0)
                        .gap(6.0)
                        .items_center()
                        .child(label(agent.role.clone()).color(colors.text).truncate())
                        .child(
                            label(state_text)
                                .size(11.0)
                                .color(colors.text_muted)
                                .truncate(),
                        ),
                )
                .child(
                    label(format!("turn {}", agent.turn))
                        .size(11.0)
                        .color(colors.text_placeholder),
                );
            if self.hot(id) {
                row = row.bg(colors.ghost_element_hover);
            }
            nodes.push(row.into());
        }
        nodes
    }

    fn detail_view(&self, detail: &Detail, snapshot: &Snapshot, width: f32, height: f32) -> Node {
        let colors = theme();
        let (title, name) = match detail {
            Detail::Logs(service) => ("Output", service.clone()),
            Detail::Turn(role) => ("Last turn", role.clone()),
        };
        let bar = div()
            .row()
            .h_px(ROW_H)
            .pl(12.0)
            .pr(6.0)
            .gap(6.0)
            .items_center()
            .child(label(title).size(12.0).medium().color(colors.text))
            .child(
                div().row().flex(1.0).child(
                    label(name)
                        .size(12.0)
                        .mono()
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            )
            .child(self.button(self.base + CLOSE_DETAIL, IconKind::Close));
        let body_h = (height - ROW_H).max(0.0);
        let body: Node = match detail {
            Detail::Logs(service) => {
                let text = match &snapshot.logs {
                    Some((shown, Ok(text))) if shown == service => text.clone(),
                    Some((shown, Err(error))) if shown == service => error.clone(),
                    _ => "Reading...".to_string(),
                };
                let fit = (body_h / LOG_LINE_H).floor().max(1.0) as usize;
                let lines: Vec<&str> = text.lines().collect();
                let shown = &lines[lines.len().saturating_sub(fit)..];
                let mut column = div().col().px(12.0).h_px(body_h);
                for line in shown {
                    column = column.child(
                        div().row().h_px(LOG_LINE_H).child(
                            label(strip_escapes(line))
                                .size(11.0)
                                .mono()
                                .color(colors.text_muted)
                                .truncate(),
                        ),
                    );
                }
                column.into()
            }
            Detail::Turn(role) => {
                let wrap = (width - 24.0).max(40.0);
                let mut column = div().col().px(12.0).gap(8.0).h_px(body_h);
                match &snapshot.turn {
                    Some((shown, Ok(Some(turn)))) if shown == role => {
                        column = column
                            .child(
                                label(format!("> {}", turn.prompt))
                                    .size(12.0)
                                    .color(colors.text)
                                    .wrap(wrap),
                            )
                            .child(
                                label(if turn.text.is_empty() {
                                    format!("({})", turn.stop_reason)
                                } else {
                                    turn.text.clone()
                                })
                                .size(12.0)
                                .color(colors.text_muted)
                                .wrap(wrap),
                            );
                    }
                    Some((shown, Ok(None))) if shown == role => {
                        column = column.child(
                            label("No turn yet")
                                .size(12.0)
                                .color(colors.text_placeholder),
                        );
                    }
                    Some((shown, Err(error))) if shown == role => {
                        column = column.child(
                            label(error.clone())
                                .size(12.0)
                                .color(colors.error)
                                .wrap(wrap),
                        );
                    }
                    _ => {
                        column = column.child(
                            label("Reading...")
                                .size(12.0)
                                .color(colors.text_placeholder),
                        );
                    }
                }
                column.into()
            }
        };
        div().col().child(bar).child(body).into()
    }

    fn open_detail(&mut self, detail: Option<Detail>) {
        match &detail {
            Some(Detail::Logs(service)) => {
                self.watch.show_turn(None);
                self.watch.show_logs(Some(service.clone()));
            }
            Some(Detail::Turn(role)) => {
                self.watch.show_logs(None);
                self.watch.show_turn(Some(role.clone()));
            }
            None => {
                self.watch.show_logs(None);
                self.watch.show_turn(None);
            }
        }
        self.detail = detail;
    }
}

pub fn connection_line(snapshot: &Snapshot, location: &str) -> (Rgba, String, bool) {
    let colors = theme();
    match &snapshot.connection {
        Connection::Connecting => (
            colors.text_accent,
            format!("Connecting to {location}..."),
            false,
        ),
        Connection::Connected => {
            let ago = snapshot
                .answered_at
                .map(|at| format!(" - updated {}", view::ago(at)))
                .unwrap_or_default();
            let via = snapshot
                .transport
                .as_ref()
                .map(|transport| format!(" via {}", transport.via.name()))
                .unwrap_or_default();
            (
                colors.success,
                format!("Connected to {location}{via}{ago}"),
                false,
            )
        }
        Connection::Reconnecting { attempt, next_in } => (
            colors.warning,
            reconnecting_line(location, *attempt, *next_in),
            false,
        ),
        Connection::Disconnected { reason } => (
            colors.error,
            format!("Disconnected from {location}: {reason}"),
            true,
        ),
    }
}

/// The status while a dropped session is being reopened.
pub fn reconnecting_line(location: &str, attempt: u32, next_in: std::time::Duration) -> String {
    let when = if next_in.is_zero() {
        String::new()
    } else {
        format!(
            " (attempt {attempt} in {:.0}s)",
            next_in.as_secs_f64().ceil()
        )
    };
    format!("Lost connection to {location}. Reconnecting...{when}")
}

/// Terminal colors and cursor moves a service printed, which a plain label would show as junk.
fn strip_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(char) = chars.next() {
        if char == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() || next == '~' {
                        break;
                    }
                }
            }
            continue;
        }
        if char == '\r' || (char.is_control() && char != '\t') {
            continue;
        }
        out.push(char);
    }
    out
}

impl SidePanelView for RemoteServicesPanel {
    fn kind(&self) -> PaneKind {
        PaneKind::Services
    }

    fn render(&mut self, width: f32, height: f32) -> Node {
        let snapshot = self.watch.snapshot();
        let colors = theme();
        let mut panel = div()
            .col()
            .w_px(width)
            .h_px(height)
            .child(self.header(&snapshot));
        let mut used = HEADER_H;
        if let Some(message) = &snapshot.message {
            panel = panel.child(
                div().row().px(12.0).h_px(ROW_H).items_center().child(
                    label(message.lines().last().unwrap_or_default().to_string())
                        .size(11.5)
                        .color(colors.text_muted)
                        .truncate(),
                ),
            );
            used += ROW_H;
        }
        let body_h = (height - used).max(0.0);
        if let Some(detail) = self.detail.clone() {
            self.viewport_h = 0.0;
            return panel
                .child(self.detail_view(&detail, &snapshot, width, body_h))
                .into();
        }
        let rows = self.rows(&snapshot);
        let row_heights: Vec<f32> = rows.iter().map(|node| ui::measure(node).1).collect();
        self.content_h = row_heights.iter().sum::<f32>() + 8.0;
        self.viewport_h = body_h;
        self.scroll = self.scroll.clamp(0.0, (self.content_h - body_h).max(0.0));
        let mut list = div().col().h_px(body_h);
        let (mut top, mut shown_h) = (0.0, 0.0);
        for (node, row_h) in rows.into_iter().zip(row_heights) {
            let bottom = top + row_h;
            top = bottom;
            if bottom <= self.scroll {
                continue;
            }
            if shown_h + row_h > body_h {
                break;
            }
            shown_h += row_h;
            list = list.child(node);
        }
        panel.child(list).into()
    }

    fn click(&mut self, id: u64) {
        let Some(offset) = id.checked_sub(self.base) else {
            return;
        };
        if offset == CLOSE_DETAIL {
            return self.open_detail(None);
        }
        if let Some(index) = offset.checked_sub(AGENT_BASE) {
            if let Some(role) = self.roles.get(index as usize).cloned() {
                let target = terminal::RemoteTarget::Agent(role.clone());
                if !self.open_terminal(target, role.clone()) {
                    self.open_detail(Some(Detail::Turn(role)));
                }
            }
            return;
        }
        let Some((index, control)) = self.decode_row(id) else {
            return;
        };
        let Some((target, running)) = self.targets.get(index).cloned() else {
            return;
        };
        match control {
            Control::Logs if running => {
                let service = terminal::RemoteTarget::Service(target.clone());
                if !self.open_terminal(service, target.clone()) {
                    self.open_detail(Some(Detail::Logs(target)));
                }
            }
            Control::Row | Control::Logs => self.open_detail(Some(Detail::Logs(target))),
            Control::Start => self.watch.act(ServiceAction::Start, &target),
            Control::Stop => self.watch.act(ServiceAction::Stop, &target),
            Control::Restart => self.watch.act(ServiceAction::Restart, &target),
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

    fn open_menu(&mut self, _id: u64) -> bool {
        false
    }

    fn menu_items(&self) -> Vec<MenuItem> {
        Vec::new()
    }

    fn menu_action(&mut self, _item: u64) {}

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_carriage_returns_leave_the_text() {
        assert_eq!(strip_escapes("\u{1b}[32mGET /\u{1b}[0m 200\r"), "GET / 200");
        assert_eq!(strip_escapes("plain\ttab"), "plain\ttab");
    }

    #[test]
    fn row_ids_round_trip() {
        let watch = Watch::start(
            std::sync::Arc::new(Never),
            "main",
            std::sync::Arc::new(|| {}),
        );
        let panel = RemoteServicesPanel::new(watch);
        for index in [0, 3, 117] {
            for control in Control::ALL {
                assert_eq!(
                    panel.decode_row(panel.row_id(index, control)),
                    Some((index, control))
                );
            }
        }
        assert_eq!(panel.decode_row(panel.base + AGENT_BASE), None);
    }

    struct Never;

    impl pom_remote::ProjectSource for Never {
        fn location(&self) -> String {
            "dev".into()
        }
        fn workspaces(&self) -> Result<pom_remote::WorkspaceList, pom_remote::SourceError> {
            Err(pom_remote::SourceError::Unreachable("test".into()))
        }
        fn status(&self, _: &str) -> Result<pom_remote::StatusReport, pom_remote::SourceError> {
            Err(pom_remote::SourceError::Unreachable("test".into()))
        }
        fn logs(&self, _: &str, _: &str) -> Result<String, pom_remote::SourceError> {
            Err(pom_remote::SourceError::Unreachable("test".into()))
        }
        fn agents(&self, _: &str) -> Result<Vec<pom_agent::SessionView>, pom_remote::SourceError> {
            Ok(Vec::new())
        }
        fn agent_turn(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<pom_remote::AgentTurn>, pom_remote::SourceError> {
            Ok(None)
        }
        fn act(
            &self,
            _: &str,
            _: ServiceAction,
            _: &str,
        ) -> Result<String, pom_remote::SourceError> {
            Ok(String::new())
        }
    }
}
