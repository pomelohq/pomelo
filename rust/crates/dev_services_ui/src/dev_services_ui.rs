use std::cell::RefCell;
use std::rc::Rc;

use pom_proxy::{Delivery, ProxyLogEntry, RequestKind};
use terminal::Modifiers;
use ui::{div, label, theme, ButtonStyle, IconKind, LabelSize, Node, Rect, Rgba};
use workspace::{Item, ItemTick};

pub const TAB_ID: &str = "dev-requests";

const RESTART: u64 = 1;
const KIND_BASE: u64 = 10;
const STATUS_BASE: u64 = 20;
const ROW_BASE: u64 = 1_000;
const CONTENT_MAX_W: f32 = 980.0;
const PAD_X: f32 = 32.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KindFilter {
    #[default]
    All,
    Proxy,
    Webhook,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Restart,
}

/// One server's line in the header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
}

#[derive(Default)]
pub struct DevRequestsState {
    pub proxy: ServerStatus,
    pub webhook: ServerStatus,
    /// `pom proxy` in a terminal holds the ports; its traffic never reaches this log.
    pub served_elsewhere: bool,
    /// Newest first.
    pub entries: Vec<ProxyLogEntry>,
    pub kind: KindFilter,
    pub errors_only: bool,
    /// The expanded row, by its log sequence number.
    pub selected: Option<u64>,
    pub requests: Vec<Request>,
    pub version: u64,
}

pub type Shared = Rc<RefCell<DevRequestsState>>;

impl DevRequestsState {
    /// Replace the servers' state and log; returns whether anything the tab shows changed.
    pub fn update(
        &mut self,
        proxy: ServerStatus,
        webhook: ServerStatus,
        served_elsewhere: bool,
        entries: Vec<ProxyLogEntry>,
    ) -> bool {
        let changed = self.proxy != proxy
            || self.webhook != webhook
            || self.served_elsewhere != served_elsewhere
            || self.entries != entries;
        if changed {
            self.proxy = proxy;
            self.webhook = webhook;
            self.served_elsewhere = served_elsewhere;
            self.entries = entries;
            self.version = self.version.wrapping_add(1);
        }
        changed
    }

    pub fn visible(&self) -> impl Iterator<Item = &ProxyLogEntry> {
        self.entries.iter().filter(|entry| {
            let kind = match self.kind {
                KindFilter::All => true,
                KindFilter::Proxy => entry.kind == RequestKind::Proxy,
                KindFilter::Webhook => entry.kind == RequestKind::Webhook,
            };
            kind && (!self.errors_only || failed(entry))
        })
    }
}

fn failed(entry: &ProxyLogEntry) -> bool {
    entry.status >= 400 || entry.status == 0
}

fn status_color(status: u16) -> Rgba {
    if (200..400).contains(&status) {
        theme().success
    } else if status >= 500 || status == 0 {
        theme().error
    } else {
        theme().warning
    }
}

fn server_line(name: &str, server: &ServerStatus, served_elsewhere: bool) -> Node {
    let colors = theme();
    let (color, state) = if !server.enabled {
        (colors.text_disabled, "Off")
    } else if server.running || served_elsewhere {
        (colors.success, "Running")
    } else {
        (colors.error, "Stopped")
    };
    let text = format!("{name} - localhost:{} - {state}", server.port);
    workspace::status_line(IconKind::Server, color, &text)
}

fn header(state: &DevRequestsState) -> Node {
    let colors = theme();
    let mut top = div().row().items_center().gap(12.0).child(
        div().row().flex(1.0).child(
            label("Dev Requests")
                .label_size(LabelSize::Large)
                .color(colors.text),
        ),
    );
    if !state.served_elsewhere {
        top = top.child(ui::button(
            RESTART,
            "Restart Servers",
            ButtonStyle::Outlined,
        ));
    }
    let mut column = div().col().gap(8.0).child(top).child(
        div()
            .row()
            .gap(20.0)
            .child(server_line(
                "Reverse proxy",
                &state.proxy,
                state.served_elsewhere,
            ))
            .child(server_line(
                "Webhook fan-out",
                &state.webhook,
                state.served_elsewhere,
            )),
    );
    if state.served_elsewhere {
        column = column.child(
            label("`pom proxy` in a terminal is serving these ports; its requests show in that terminal.")
                .label_size(LabelSize::Small)
                .color(colors.text_muted),
        );
    }
    column.into()
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
            .child(
                label(text.to_string())
                    .label_size(LabelSize::Small)
                    .color(if on { colors.text } else { colors.text_muted }),
            );
        if on {
            part = part.bg(colors.element_selected);
        } else if hovered == Some(*id) {
            part = part.bg(colors.ghost_element_hover);
        }
        row = row.child(part);
    }
    row.into()
}

fn filters(state: &DevRequestsState, hovered: Option<u64>) -> Node {
    let kind = match state.kind {
        KindFilter::All => 0,
        KindFilter::Proxy => 1,
        KindFilter::Webhook => 2,
    };
    div()
        .row()
        .gap(12.0)
        .child(segmented(
            &[
                (KIND_BASE, "All"),
                (KIND_BASE + 1, "Proxy"),
                (KIND_BASE + 2, "Webhooks"),
            ],
            kind,
            hovered,
        ))
        .child(segmented(
            &[(STATUS_BASE, "Any Status"), (STATUS_BASE + 1, "Errors")],
            usize::from(state.errors_only),
            hovered,
        ))
        .into()
}

fn cell(text: impl Into<String>, width: f32, color: Rgba) -> Node {
    div()
        .row()
        .w_px(width)
        .child(
            label(text.into())
                .label_size(LabelSize::Small)
                .color(color)
                .truncate(),
        )
        .into()
}

fn row(entry: &ProxyLogEntry, hovered: bool, selected: bool) -> Node {
    let colors = theme();
    let kind = match entry.kind {
        RequestKind::Proxy => "PROXY",
        RequestKind::Webhook => "WEBHOOK",
    };
    let service = if entry.repo.is_empty() {
        entry.service.clone()
    } else {
        format!("{}/{}", entry.repo, entry.service)
    };
    let mut line = div()
        .row()
        .items_center()
        .gap(10.0)
        .px(12.0)
        .h_px(30.0)
        .on_click(ROW_BASE + entry.seq)
        .child(cell(entry.time.clone(), 60.0, colors.text_muted))
        .child(cell(kind, 62.0, colors.text_placeholder))
        .child(cell(entry.method.clone(), 52.0, colors.text))
        .child(
            div().row().flex(1.0).child(
                label(entry.path.clone())
                    .label_size(LabelSize::Small)
                    .color(colors.text)
                    .mono()
                    .truncate(),
            ),
        )
        .child(cell(service, 150.0, colors.text_muted))
        .child(cell(
            entry.status.to_string(),
            36.0,
            status_color(entry.status),
        ))
        .child(
            div().row().w_px(56.0).justify_end().child(
                label(format!("{} ms", entry.ms))
                    .label_size(LabelSize::Small)
                    .color(colors.text_muted),
            ),
        );
    if selected {
        line = line.bg(colors.element_selected);
    } else if hovered {
        line = line.bg(colors.ghost_element_hover);
    }
    line.into()
}

fn detail_line(name: &str, value: String, color: Rgba) -> Node {
    div()
        .row()
        .gap(10.0)
        .child(cell(name, 120.0, theme().text_placeholder))
        .child(
            label(value)
                .label_size(LabelSize::Small)
                .color(color)
                .truncate(),
        )
        .into()
}

fn delivery_line(delivery: &Delivery) -> Node {
    let colors = theme();
    let (outcome, color) = match delivery.status {
        Some(status) => (status.to_string(), status_color(status)),
        None => (delivery.error.clone(), colors.error),
    };
    detail_line(
        &delivery.workspace,
        format!(":{}  {outcome}", delivery.port),
        color,
    )
}

fn detail(entry: &ProxyLogEntry) -> Node {
    let colors = theme();
    let mut column = div()
        .col()
        .gap(6.0)
        .px(12.0)
        .py(10.0)
        .bg(colors.surface_background)
        .child(detail_line(
            "Path",
            format!("{} {}", entry.method, entry.path),
            colors.text,
        ));
    match entry.kind {
        RequestKind::Proxy => {
            column = column
                .child(detail_line(
                    "Profile",
                    entry.profile.clone(),
                    colors.text_muted,
                ))
                .child(detail_line(
                    "Target",
                    entry.target.clone(),
                    colors.text_muted,
                ));
        }
        RequestKind::Webhook if entry.deliveries.is_empty() => {
            column = column.child(detail_line(
                "Delivered to",
                "No workspace was running the service".into(),
                colors.warning,
            ));
        }
        RequestKind::Webhook => {
            column = column.child(detail_line(
                "Delivered to",
                format!("{} workspaces", entry.deliveries.len()),
                colors.text_muted,
            ));
            for delivery in &entry.deliveries {
                column = column.child(delivery_line(delivery));
            }
        }
    }
    column.into()
}

fn empty(state: &DevRequestsState) -> Node {
    let colors = theme();
    let hint = if state.entries.is_empty() {
        "Requests the frontend sends through /_pom_dev/, and webhooks sent to the fan-out port, show up here."
    } else {
        "Nothing matches these filters."
    };
    div()
        .col()
        .gap(4.0)
        .p(16.0)
        .child(
            label("No requests")
                .label_size(LabelSize::Default)
                .color(colors.text),
        )
        .child(
            label(hint)
                .label_size(LabelSize::Small)
                .color(colors.text_muted),
        )
        .into()
}

pub fn render(state: &DevRequestsState, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut list = div().col().rounded(8.0).border(1.0, colors.border_variant);
    let mut any = false;
    for entry in state.visible() {
        if any {
            list = list.child(div().h_px(1.0).bg(colors.border_variant));
        }
        any = true;
        let selected = state.selected == Some(entry.seq);
        list = list.child(row(entry, hovered == Some(ROW_BASE + entry.seq), selected));
        if selected {
            list = list.child(detail(entry));
        }
    }
    if !any {
        list = list.child(empty(state));
    }
    let content_w = (width - PAD_X * 2.0).clamp(0.0, CONTENT_MAX_W);
    div()
        .col()
        .items_center()
        .py(24.0)
        .child(
            div()
                .col()
                .w_px(content_w)
                .gap(16.0)
                .child(header(state))
                .child(filters(state, hovered))
                .child(list),
        )
        .into()
}

fn click(state: &mut DevRequestsState, id: u64) {
    match id {
        RESTART => state.requests.push(Request::Restart),
        id if (KIND_BASE..KIND_BASE + 3).contains(&id) => {
            state.kind = match id - KIND_BASE {
                1 => KindFilter::Proxy,
                2 => KindFilter::Webhook,
                _ => KindFilter::All,
            };
        }
        id if (STATUS_BASE..STATUS_BASE + 2).contains(&id) => {
            state.errors_only = id == STATUS_BASE + 1;
        }
        id if id >= ROW_BASE => {
            let seq = id - ROW_BASE;
            state.selected = if state.selected == Some(seq) {
                None
            } else {
                Some(seq)
            };
        }
        _ => return,
    }
    state.version = state.version.wrapping_add(1);
}

pub struct DevRequestsPage {
    shared: Shared,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    scroll: f32,
    content_h: f32,
    body_h: f32,
    seen: u64,
}

impl DevRequestsPage {
    pub fn new(shared: Shared) -> DevRequestsPage {
        DevRequestsPage {
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

impl Item for DevRequestsPage {
    fn id(&self) -> Option<String> {
        Some(TAB_ID.to_string())
    }

    fn title(&self) -> String {
        "Dev Requests".into()
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Server)
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

/// A tab over made-up traffic, for headless snapshots.
pub fn preview_page() -> DevRequestsPage {
    let entry = |seq: u64, kind: RequestKind, method: &str, path: &str, status: u16, ms: u64| {
        ProxyLogEntry {
            seq,
            kind,
            time: format!("10:4{}:0{}", seq / 10, seq % 10),
            method: method.into(),
            path: path.into(),
            repo: "api".into(),
            service: "server".into(),
            profile: "local".into(),
            target: "127.0.0.1:4100".into(),
            status,
            ms,
            deliveries: Vec::new(),
        }
    };
    let mut hook = entry(5, RequestKind::Webhook, "POST", "/hooks/stripe", 502, 84);
    hook.target = "2 workspaces".into();
    hook.profile.clear();
    hook.deliveries = vec![
        Delivery {
            workspace: "main".into(),
            port: 4100,
            status: Some(200),
            error: String::new(),
        },
        Delivery {
            workspace: "feat-login".into(),
            port: 4131,
            status: None,
            error: "connection refused".into(),
        },
    ];
    let entries = vec![
        entry(
            6,
            RequestKind::Proxy,
            "GET",
            "/_pom_dev/api/server/v1/me",
            200,
            12,
        ),
        hook,
        entry(
            4,
            RequestKind::Proxy,
            "POST",
            "/_pom_dev/api/server/v1/login",
            401,
            31,
        ),
        entry(
            3,
            RequestKind::Proxy,
            "GET",
            "/_pom_dev/web/app/assets/main.js",
            200,
            4,
        ),
        entry(
            2,
            RequestKind::Proxy,
            "GET",
            "/_pom_dev/api/server/v1/orders",
            503,
            2,
        ),
    ];
    let state = DevRequestsState {
        proxy: ServerStatus {
            enabled: true,
            running: true,
            port: 8767,
        },
        webhook: ServerStatus {
            enabled: true,
            running: true,
            port: 8766,
        },
        entries,
        selected: Some(5),
        ..DevRequestsState::default()
    };
    DevRequestsPage::new(Rc::new(RefCell::new(state)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(state: &DevRequestsState) -> Vec<String> {
        let painted = ui::render(
            &render(state, 1000.0, None),
            Rect::new(0.0, 0.0, 1000.0, 2000.0, Rgba::TRANSPARENT),
        );
        painted.texts.iter().map(|text| text.text.clone()).collect()
    }

    #[test]
    fn filters_narrow_the_list_and_a_row_expands_to_its_deliveries() {
        let page = preview_page();
        let mut state = page.shared.borrow_mut();
        assert_eq!(state.visible().count(), 5);
        click(&mut state, KIND_BASE + 2);
        assert_eq!(state.visible().count(), 1);
        click(&mut state, KIND_BASE);
        click(&mut state, STATUS_BASE + 1);
        let failing: Vec<u64> = state.visible().map(|entry| entry.seq).collect();
        assert_eq!(failing, [5, 4, 2]);
        let shown = texts(&state);
        assert!(shown.iter().any(|text| text == "feat-login"));
        assert!(shown.iter().any(|text| text.contains("connection refused")));
        click(&mut state, ROW_BASE + 5);
        assert_eq!(state.selected, None);
        assert!(!texts(&state).iter().any(|text| text == "feat-login"));
    }

    #[test]
    fn restart_is_handed_to_the_app_unless_a_terminal_serves_the_ports() {
        let mut state = DevRequestsState::default();
        click(&mut state, RESTART);
        assert_eq!(state.requests, [Request::Restart]);
        assert!(texts(&state).iter().any(|text| text == "Restart Servers"));
        state.served_elsewhere = true;
        assert!(!texts(&state).iter().any(|text| text == "Restart Servers"));
    }

    #[test]
    fn update_reports_changes_only() {
        let mut state = DevRequestsState::default();
        let proxy = ServerStatus {
            enabled: true,
            running: true,
            port: 8767,
        };
        assert!(state.update(proxy.clone(), ServerStatus::default(), false, Vec::new()));
        assert!(!state.update(proxy, ServerStatus::default(), false, Vec::new()));
    }
}
