use components::ButtonKind;
use std::cell::RefCell;
use std::rc::Rc;

use pom_proxy::{Delivery, Payload, ProxyLogEntry, RequestKind};
use terminal::{Keystroke, Modifiers};
use ui::{div, label, theme, IconKind, LabelSize, Node, Rect, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::{EditKey, Item, ItemTick, TerminalKeyOutcome};

pub const TAB_ID: &str = "dev-requests";

const RESTART: u64 = 1;
const FILTER: u64 = 2;
const COPY_BODY: u64 = 4;
const KIND_BASE: u64 = 10;
const STATUS_BASE: u64 = 20;
const TAB_BASE: u64 = 30;
const ROW_BASE: u64 = 1_000;
const LIST_SHARE: f32 = 1.4 / 2.4;
const TOOLBAR_H: f32 = 46.0;
const CARD_INSET: f32 = 8.0;
const CARD_GAP: f32 = 6.0;
const FOOT_H: f32 = 36.0;
const DETAIL_PAD: f32 = 14.0;
const BODY_LINES_SHOWN: usize = 300;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KindFilter {
    #[default]
    All,
    Proxy,
    Webhook,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailTab {
    #[default]
    Request,
    FanOut,
    Response,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Restart,
}

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
    /// Newest first, without bodies.
    pub entries: Vec<ProxyLogEntry>,
    pub kind: KindFilter,
    pub errors_only: bool,
    pub filter: TextField,
    pub filter_focused: bool,
    /// The selected row, by its log sequence number.
    pub selected: Option<u64>,
    pub tab: DetailTab,
    /// The selected row's request and response bodies, as the app last fetched them.
    pub payloads: Option<(u64, Payload, Payload)>,
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
            self.changed();
        }
        changed
    }

    /// The bodies the app should fetch: the selection, when not fetched yet or still streaming.
    pub fn wants_payloads(&self) -> Option<u64> {
        let seq = self.selected?;
        match &self.payloads {
            Some((loaded, request, response)) if *loaded == seq => {
                (!request.complete || !response.complete).then_some(seq)
            }
            _ => Some(seq),
        }
    }

    pub fn set_payloads(&mut self, seq: u64, request: Payload, response: Payload) {
        let next = Some((seq, request, response));
        if self.payloads != next {
            self.payloads = next;
            self.changed();
        }
    }

    pub fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    pub fn visible(&self) -> impl Iterator<Item = &ProxyLogEntry> {
        let query = self.filter.text().to_lowercase();
        self.entries.iter().filter(move |entry| {
            let kind = match self.kind {
                KindFilter::All => true,
                KindFilter::Proxy => entry.kind == RequestKind::Proxy,
                KindFilter::Webhook => entry.kind == RequestKind::Webhook,
            };
            let text = query.is_empty()
                || entry.path.to_lowercase().contains(&query)
                || format!("{}/{}", entry.repo, entry.service)
                    .to_lowercase()
                    .contains(&query)
                || entry.method.to_lowercase() == query;
            kind && text && (!self.errors_only || failed(entry))
        })
    }

    fn selected_entry(&self) -> Option<&ProxyLogEntry> {
        let seq = self.selected?;
        self.entries.iter().find(|entry| entry.seq == seq)
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

fn content_type(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.to_ascii_lowercase())
        .unwrap_or_default()
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// The body as text for display and copying: JSON pretty-printed, text as is, `None` for binary.
pub fn body_text(
    payload: &Payload,
    headers: &[(String, String)],
) -> Option<(String, &'static str)> {
    if payload.bytes.is_empty() {
        return Some((String::new(), ""));
    }
    let kind = content_type(headers);
    let text = match std::str::from_utf8(&payload.bytes) {
        Ok(text) => Some(text),
        // A capture cut mid-character is still text.
        Err(error) if error.error_len().is_none() => {
            std::str::from_utf8(&payload.bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    };
    if let Some(text) = text.filter(|text| {
        (kind.contains("json") || text.trim_start().starts_with(['{', '[']))
            && serde_json::from_str::<serde_json::Value>(text).is_ok()
    }) {
        return Some((pretty_json(text), "JSON"));
    }
    let label = if kind.contains("html") {
        "HTML"
    } else if kind.contains("x-www-form-urlencoded") {
        "Form"
    } else {
        "Text"
    };
    text.map(|text| (text.to_string(), label))
}

/// One line of pretty-printed JSON cut into runs, each with the highlight capture it is colored by.
fn json_runs(line: &str) -> Vec<(&str, &'static str)> {
    let mut runs = Vec::new();
    let bytes = line.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let capture = match bytes[at] {
            b' ' | b'\t' => {
                while at < bytes.len() && matches!(bytes[at], b' ' | b'\t') {
                    at += 1;
                }
                ""
            }
            b'"' => {
                at += 1;
                let mut escaped = false;
                while at < bytes.len() {
                    let byte = bytes[at];
                    at += 1;
                    match byte {
                        _ if escaped => escaped = false,
                        b'\\' => escaped = true,
                        b'"' => break,
                        _ => {}
                    }
                }
                let rest = line[at..].trim_start();
                if rest.starts_with(':') {
                    "property.json_key"
                } else {
                    "string"
                }
            }
            b'{' | b'}' | b'[' | b']' => {
                at += 1;
                "punctuation.bracket"
            }
            b',' | b':' => {
                at += 1;
                "punctuation.delimiter"
            }
            _ => {
                while at < bytes.len()
                    && !matches!(
                        bytes[at],
                        b' ' | b'\t' | b',' | b':' | b'"' | b'{' | b'}' | b'[' | b']'
                    )
                {
                    at += 1;
                }
                match &line[start..at] {
                    "true" | "false" => "boolean",
                    "null" => "constant.builtin",
                    word if word.starts_with(|c: char| c == '-' || c.is_ascii_digit()) => "number",
                    _ => "",
                }
            }
        };
        runs.push((&line[start..at], capture));
    }
    runs
}

/// A body line colored like the editor colors JSON.
fn json_line(line: &str, syntax: &editor::Theme) -> Node {
    let mut row = div().row().items_center().h_px(16.0);
    for (text, capture) in json_runs(line) {
        let color = if capture.is_empty() {
            syntax.foreground
        } else {
            syntax.syntax_color(capture)
        };
        let [r, g, b] = color.0;
        let color = Rgba::new(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        );
        row = row.child(small(text.to_string(), color).mono().truncate());
    }
    row.into()
}

/// Re-indents valid JSON keeping its keys in the order they were sent.
fn pretty_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                let close = if c == '{' { '}' } else { ']' };
                while chars.peek().is_some_and(|next| next.is_whitespace()) {
                    chars.next();
                }
                if chars.peek() == Some(&close) {
                    out.push(close);
                    chars.next();
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out
}

fn small(text: impl Into<String>, color: Rgba) -> ui::Label {
    label(text.into()).size(LabelSize::Small).color(color)
}

fn server_dot(name: &str, server: &ServerStatus, served_elsewhere: bool) -> Node {
    let colors = theme();
    let color = if !server.enabled {
        colors.text_disabled
    } else if server.running || served_elsewhere {
        colors.success
    } else {
        colors.error
    };
    div()
        .row()
        .items_center()
        .gap(6.0)
        .child(components::dot(color, 7.0))
        .child(small(format!("{name} :{}", server.port), colors.text_muted))
        .into()
}

fn toolbar(state: &DevRequestsState, hovered: Option<u64>) -> Node {
    let colors = theme();
    let kind = match state.kind {
        KindFilter::All => 0,
        KindFilter::Proxy => 1,
        KindFilter::Webhook => 2,
    };
    let field = components::search_box(FILTER, state.filter_focused)
        .w_px(240.0)
        .child(
            div()
                .row()
                .flex(1.0)
                .items_center()
                .child(state.filter.render(
                    "Filter path or service",
                    state.filter_focused,
                    colors.text,
                    20.0,
                    FieldFont::Ui,
                )),
        );
    let mut bar = components::toolbar()
        .height(TOOLBAR_H)
        .child(
            components::segmented(
                [
                    components::segment(KIND_BASE, "All"),
                    components::segment(KIND_BASE + 1, "Proxy"),
                    components::segment(KIND_BASE + 2, "Webhooks"),
                ],
                kind,
            )
            .hovered(hovered),
        )
        .child(
            components::segmented(
                [
                    components::segment(STATUS_BASE, "Any Status"),
                    components::segment(STATUS_BASE + 1, "Errors"),
                ],
                usize::from(state.errors_only),
            )
            .hovered(hovered),
        )
        .child(field)
        .spacer()
        .child(
            div()
                .row()
                .items_center()
                .gap(12.0)
                .child(server_dot("proxy", &state.proxy, state.served_elsewhere))
                .child(server_dot(
                    "webhooks",
                    &state.webhook,
                    state.served_elsewhere,
                )),
        );
    if !state.served_elsewhere {
        bar = bar.child(
            components::button(RESTART, "Restart")
                .kind(ButtonKind::Ghost)
                .icon(Some(IconKind::RotateCw))
                .hovered(hovered),
        );
    }
    bar.into()
}

fn kind_text(kind: RequestKind) -> ui::Label {
    let colors = theme();
    match kind {
        RequestKind::Proxy => small("PROXY", colors.text_placeholder).mono(),
        RequestKind::Webhook => small("WEBHOOK", colors.info).mono(),
    }
}

fn row(entry: &ProxyLogEntry, hovered: bool, selected: bool) -> Node {
    let colors = theme();
    let fixed = |width: f32, text: ui::Label| div().row().w_px(width).child(text.truncate());
    let mut line = div()
        .row()
        .items_center()
        .gap(10.0)
        .px(12.0)
        .h_px(30.0)
        .on_click(ROW_BASE + entry.seq)
        .child(fixed(
            62.0,
            small(entry.time.clone(), colors.text_muted).mono(),
        ))
        .child(fixed(62.0, kind_text(entry.kind)))
        .child(fixed(40.0, small(entry.method.clone(), colors.text)))
        .child(
            div()
                .row()
                .flex(1.0)
                .child(small(entry.path.clone(), colors.text).mono().truncate()),
        )
        .child(fixed(
            34.0,
            small(entry.status.to_string(), status_color(entry.status)).mono(),
        ))
        .child(
            div()
                .row()
                .w_px(46.0)
                .justify_end()
                .child(small(format!("{} ms", entry.ms), colors.text_muted)),
        );
    if selected {
        line = line.bg(colors.element_selected);
    } else if hovered {
        line = line.bg(colors.ghost_element_hover);
    }
    line.into()
}

fn list(state: &DevRequestsState, hovered: Option<u64>) -> Node {
    let mut column = div().col();
    let mut any = false;
    for entry in state.visible() {
        any = true;
        column = column.child(row(
            entry,
            hovered == Some(ROW_BASE + entry.seq),
            state.selected == Some(entry.seq),
        ));
    }
    if !any {
        let hint = if state.entries.is_empty() {
            "Requests the frontend sends through /_pom_dev/, and webhooks sent to the fan-out port, show up here."
        } else {
            "Nothing matches these filters."
        };
        column = column.child(
            components::empty_state("No requests")
                .icon(IconKind::ArrowUpRight)
                .body(hint, 360.0),
        );
    }
    column.into()
}

fn fact(name: &str, value: impl Into<String>, color: Rgba) -> Node {
    components::key_value(small(name, theme().text_muted), small(value, color), None)
}

fn key_values(rows: Vec<Node>) -> Node {
    let mut column = div().col().gap(6.0).px(DETAIL_PAD).py(6.0);
    for row in rows {
        column = column.child(row);
    }
    column.into()
}

fn section_head(title: Node, action: Option<Node>) -> Node {
    let mut head = div()
        .row()
        .items_center()
        .pl(DETAIL_PAD)
        .pr(8.0)
        .pt(8.0)
        .h_px(32.0)
        .child(div().row().flex(1.0).items_center().gap(8.0).child(title));
    if let Some(action) = action {
        head = head.child(action);
    }
    head.into()
}

fn headers_block(headers: &[(String, String)]) -> Node {
    let colors = theme();
    if headers.is_empty() {
        return key_values(vec![small("No headers", colors.text_muted).into()]);
    }
    let rows = headers
        .iter()
        .map(|(name, value)| {
            let shown = if value == pom_proxy::REDACTED {
                small(value.clone(), colors.text_placeholder)
            } else {
                small(value.clone(), colors.text)
            };
            components::key_value(
                small(name.clone(), colors.text_muted).mono(),
                shown.mono(),
                None,
            )
        })
        .collect();
    key_values(rows)
}

fn body_block(payload: &Payload, headers: &[(String, String)], hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut notes = Vec::new();
    if payload.evicted {
        notes.push("Dropped to keep memory in check; newer requests keep theirs.".to_string());
    } else if payload.truncated() {
        notes.push(format!(
            "Showing the first {} of {}.",
            format_bytes(payload.bytes.len() as u64),
            format_bytes(payload.total)
        ));
    }
    if !payload.complete {
        notes.push("Still streaming.".to_string());
    }
    let shown = body_text(payload, headers);
    let meta = match &shown {
        _ if payload.total == 0 && payload.complete => "Empty".to_string(),
        Some((_, kind)) => format!("{kind} - {}", format_bytes(payload.total)),
        None => format!("Binary - {}", format_bytes(payload.total)),
    };
    let copy = shown
        .as_ref()
        .is_some_and(|(text, _)| !text.is_empty())
        .then(|| {
            components::button(COPY_BODY, "Copy")
                .kind(ButtonKind::Ghost)
                .icon(Some(IconKind::Copy))
                .hovered(hovered)
        });
    let title = div()
        .row()
        .items_center()
        .gap(8.0)
        .child(components::section_header("Body"))
        .child(small(meta, colors.text_placeholder))
        .into();
    let mut column = div().col().child(section_head(title, copy.map(Node::from)));
    for note in notes {
        column = column.child(
            div()
                .row()
                .px(DETAIL_PAD)
                .pt(4.0)
                .child(small(note, colors.text_muted)),
        );
    }
    if let Some((text, kind)) = shown.filter(|(text, _)| !text.is_empty()) {
        let syntax = (kind == "JSON").then(workspace::syntax_theme);
        let mut code = div()
            .col()
            .px(12.0)
            .py(10.0)
            .rounded(4.0)
            .bg(colors.editor_background);
        let lines: Vec<&str> = text.lines().collect();
        for line in lines.iter().take(BODY_LINES_SHOWN) {
            code = code.child(match &syntax {
                Some(syntax) => json_line(line, syntax),
                None => small(line.to_string(), colors.text)
                    .mono()
                    .truncate()
                    .into(),
            });
        }
        if lines.len() > BODY_LINES_SHOWN {
            code = code.child(small(
                format!(
                    "{} more lines - Copy for all of it",
                    lines.len() - BODY_LINES_SHOWN
                ),
                colors.text_placeholder,
            ));
        }
        column = column.child(div().col().px(DETAIL_PAD).py(6.0).child(code));
    }
    column.into()
}

fn delivery_row(delivery: &Delivery) -> Node {
    let colors = theme();
    let (outcome, color) = match delivery.status {
        Some(status) => (
            format!("{status} - {} ms", delivery.ms),
            status_color(status),
        ),
        None => (delivery.error.clone(), colors.error),
    };
    let ok = delivery.status.is_some_and(|status| status < 400);
    div()
        .row()
        .items_center()
        .gap(10.0)
        .h_px(26.0)
        .child(components::dot(
            if ok { colors.success } else { colors.error },
            8.0,
        ))
        .child(
            div()
                .row()
                .flex(1.0)
                .gap(6.0)
                .child(small(delivery.workspace.clone(), colors.text).truncate())
                .child(small(format!(":{}", delivery.port), colors.text_placeholder).mono()),
        )
        .child(small(outcome, color))
        .into()
}

fn webhook_result(entry: &ProxyLogEntry) -> (String, Rgba) {
    let colors = theme();
    let failed = entry
        .deliveries
        .iter()
        .filter(|delivery| delivery.status.is_none_or(|status| status >= 400))
        .count();
    match (entry.deliveries.len(), failed) {
        (0, _) => ("No workspace running it".to_string(), colors.warning),
        (_, 0) => ("Delivered".to_string(), colors.success),
        (total, failed) => (format!("{failed} of {total} failed"), colors.error),
    }
}

fn tabs_for(entry: &ProxyLogEntry) -> Vec<(DetailTab, String)> {
    let mut tabs = vec![(DetailTab::Request, "Request".to_string())];
    if entry.kind == RequestKind::Webhook {
        tabs.push((
            DetailTab::FanOut,
            format!("Fan-out ({})", entry.deliveries.len()),
        ));
    }
    tabs.push((DetailTab::Response, "Response".to_string()));
    tabs
}

fn side(state: &DevRequestsState, hovered: Option<u64>) -> Node {
    let colors = theme();
    let Some(entry) = state.selected_entry() else {
        let mut column = div().col().gap(6.0).p(16.0).child(small(
            "Select a request to see what was sent and what came back.",
            colors.text_muted,
        ));
        if state.served_elsewhere {
            column = column.child(small(
                "`pom proxy` in a terminal is serving these ports; its requests show in that terminal.",
                colors.text_muted,
            ));
        }
        return column.into();
    };
    let webhook = entry.kind == RequestKind::Webhook;
    let (result, result_color) = if webhook {
        webhook_result(entry)
    } else {
        (entry.status.to_string(), status_color(entry.status))
    };
    let mut facts = vec![
        fact(
            "Service",
            format!("{}/{}", entry.repo, entry.service),
            colors.text,
        ),
        fact(
            "Received",
            format!("{} - {} ms", entry.time, entry.ms),
            colors.text,
        ),
    ];
    if !webhook {
        facts.push(fact("Profile", entry.profile.clone(), colors.text_muted));
        facts.push(fact("Target", entry.target.clone(), colors.text_muted));
    }
    let mut column = div().col().child(
        div()
            .col()
            .gap(10.0)
            .px(DETAIL_PAD)
            .pt(14.0)
            .pb(6.0)
            .child(
                div()
                    .row()
                    .items_center()
                    .h_px(16.0)
                    .gap(8.0)
                    .child(kind_text(entry.kind))
                    .child(
                        div().row().flex(1.0).child(
                            small(format!("{} {}", entry.method, entry.path), colors.text)
                                .mono()
                                .truncate(),
                        ),
                    )
                    .child(small(result, result_color)),
            )
            .child(div().col().gap(6.0).py(6.0).children(facts)),
    );
    let tab = if !webhook && state.tab == DetailTab::FanOut {
        DetailTab::Request
    } else {
        state.tab
    };
    let mut strip = div().row().gap(14.0);
    for (index, (kind, text)) in tabs_for(entry).iter().enumerate() {
        let id = TAB_BASE + index as u64;
        let on = *kind == tab;
        let ink = if on || hovered == Some(id) {
            colors.text
        } else {
            colors.text_muted
        };
        strip = strip.child(
            div()
                .col()
                .on_click(id)
                .child(
                    div()
                        .row()
                        .items_center()
                        .px(2.0)
                        .h_px(26.0)
                        .child(small(text.clone(), ink)),
                )
                .child(div().h_px(2.0).bg(if on {
                    colors.text_accent
                } else {
                    Rgba::TRANSPARENT
                })),
        );
    }
    column = column.child(
        div()
            .col()
            .px(DETAIL_PAD)
            .pt(4.0)
            .child(strip)
            .child(components::divider()),
    );
    if tab == DetailTab::FanOut {
        let mut deliveries = div().col().px(DETAIL_PAD).py(6.0);
        if entry.deliveries.is_empty() {
            deliveries = deliveries.child(small(
                "No workspace was running the service when it arrived.",
                colors.text_muted,
            ));
        }
        for delivery in &entry.deliveries {
            deliveries = deliveries.child(delivery_row(delivery));
        }
        column = column.child(deliveries);
    } else {
        let headers = if tab == DetailTab::Response {
            &entry.response_headers
        } else {
            &entry.request_headers
        };
        column = column
            .child(section_head(
                components::section_header("Headers").into(),
                None,
            ))
            .child(headers_block(headers));
        if tab == DetailTab::Response && webhook {
            column = column.child(div().row().px(DETAIL_PAD).child(small(
                "What the relay answered the sender, before fanning out.",
                colors.text_muted,
            )));
        }
        let loaded = state
            .payloads
            .as_ref()
            .filter(|(seq, _, _)| *seq == entry.seq);
        column = match loaded {
            Some((_, request, response)) => column.child(body_block(
                if tab == DetailTab::Response {
                    response
                } else {
                    request
                },
                headers,
                hovered,
            )),
            None => column.child(
                div()
                    .row()
                    .px(DETAIL_PAD)
                    .child(small("Loading the body...", colors.text_muted)),
            ),
        };
    }
    column.into()
}

fn side_foot() -> Node {
    div()
        .row()
        .items_center()
        .px(DETAIL_PAD)
        .h_px(FOOT_H)
        .child(
            small(
                "Kept in memory for this session only.",
                theme().text_placeholder,
            )
            .truncate(),
        )
        .into()
}

/// The text Copy puts on the clipboard for the open tab.
fn copied_body(state: &DevRequestsState) -> Option<String> {
    let entry = state.selected_entry()?;
    let (_, request, response) = state
        .payloads
        .as_ref()
        .filter(|(seq, _, _)| *seq == entry.seq)?;
    let (payload, headers) = match state.tab {
        DetailTab::Response => (response, &entry.response_headers),
        _ => (request, &entry.request_headers),
    };
    body_text(payload, headers).map(|(text, _)| text)
}

fn click(state: &mut DevRequestsState, id: u64) -> Option<String> {
    let mut copied = None;
    state.filter_focused = id == FILTER;
    match id {
        RESTART => state.requests.push(Request::Restart),
        COPY_BODY => copied = copied_body(state),
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
        id if (TAB_BASE..TAB_BASE + 3).contains(&id) => {
            let tab = state.selected_entry().and_then(|entry| {
                tabs_for(entry)
                    .get((id - TAB_BASE) as usize)
                    .map(|(tab, _)| *tab)
            });
            if let Some(tab) = tab {
                state.tab = tab;
            }
        }
        id if id >= ROW_BASE => {
            let seq = id - ROW_BASE;
            if state.selected != Some(seq) {
                state.selected = Some(seq);
                let webhook = state
                    .selected_entry()
                    .is_some_and(|entry| entry.kind == RequestKind::Webhook);
                if state.tab == DetailTab::FanOut && !webhook {
                    state.tab = DetailTab::Request;
                }
            }
        }
        _ => {}
    }
    state.changed();
    copied
}

fn merge(into: &mut ui::Painted, part: ui::Painted) {
    into.rects.extend(part.rects);
    into.tris.extend(part.tris);
    into.texts.extend(part.texts);
    into.icons.extend(part.icons);
    into.hits.extend(part.hits);
}

pub struct DevRequestsPage {
    shared: Shared,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    list: ui::ScrollPane,
    side: ui::ScrollPane,
    seen: u64,
    clipboard: Option<String>,
}

impl DevRequestsPage {
    pub fn new(shared: Shared) -> DevRequestsPage {
        DevRequestsPage {
            shared,
            hits: Vec::new(),
            hovered: None,
            list: ui::ScrollPane::default(),
            side: ui::ScrollPane::default(),
            seen: u64::MAX,
            clipboard: None,
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
        if let Some(text) = click(&mut self.shared.borrow_mut(), id) {
            self.clipboard = Some(text);
        }
    }

    fn edit_key(keystroke: &Keystroke) -> Option<(EditKey, bool)> {
        let Modifiers {
            shift, alt, cmd, ..
        } = keystroke.modifiers;
        let key = match keystroke.key.as_str() {
            "left" if cmd => EditKey::Home,
            "right" if cmd => EditKey::End,
            "left" if alt => EditKey::WordLeft,
            "right" if alt => EditKey::WordRight,
            "left" => EditKey::Left,
            "right" => EditKey::Right,
            "home" => EditKey::Home,
            "end" => EditKey::End,
            "backspace" if cmd => EditKey::DeleteToLineStart,
            "backspace" if alt => EditKey::DeleteWordLeft,
            "backspace" => EditKey::Backspace,
            "delete" => EditKey::Delete,
            "a" if cmd => EditKey::SelectAll,
            _ => return None,
        };
        Some((key, shift))
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
        let (bar, list_tree, side_tree, selected) = {
            let state = self.shared.borrow();
            (
                toolbar(&state, self.hovered),
                list(&state, self.hovered),
                side(&state, self.hovered),
                state.selected_entry().is_some(),
            )
        };
        let bar_h = TOOLBAR_H * scale;
        let inset = CARD_INSET * scale;
        let gap = CARD_GAP * scale;
        let top = body.y + bar_h;
        let cards_h = (body.h - bar_h - inset).max(0.0);
        let cards_w = (body.w - 2.0 * inset - gap).max(0.0);
        let list_w = (cards_w * LIST_SHARE).round();
        let side_x = body.x + inset + list_w + gap;
        let side_w = cards_w - list_w;
        let mut painted = ui::render(
            &bar,
            Rect::new(body.x, body.y, body.w, bar_h, Rgba::TRANSPARENT),
        );
        let card = || div().rounded(6.0).bg(theme().panel_background).into();
        for area in [
            Rect::new(body.x + inset, top, list_w, cards_h, Rgba::TRANSPARENT),
            Rect::new(side_x, top, side_w, cards_h, Rgba::TRANSPARENT),
        ] {
            merge(&mut painted, ui::render(&card(), area));
        }
        let rows = self.list.paint(
            &list_tree,
            Rect::new(body.x + inset, top, list_w, cards_h, Rgba::TRANSPARENT),
        );
        merge(&mut painted, rows);
        let foot_h = if selected { FOOT_H * scale } else { 0.0 };
        let detail = self.side.paint(
            &side_tree,
            Rect::new(
                side_x,
                top,
                side_w,
                (cards_h - foot_h).max(0.0),
                Rgba::TRANSPARENT,
            ),
        );
        merge(&mut painted, detail);
        if selected {
            merge(
                &mut painted,
                ui::render(
                    &side_foot(),
                    Rect::new(
                        side_x,
                        top + cards_h - foot_h,
                        side_w,
                        foot_h,
                        Rgba::TRANSPARENT,
                    ),
                ),
            );
        }
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        match self.hit(x, y) {
            Some(id) => self.click(id),
            None => {
                let mut state = self.shared.borrow_mut();
                if state.filter_focused {
                    state.filter_focused = false;
                    state.changed();
                }
            }
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hovered = self.hit(x, y);
        let moved = hovered != self.hovered;
        self.hovered = hovered;
        moved
    }

    fn pointer_scroll(&mut self, x: f32, y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        if self.side.contains(x, y) {
            self.side.scroll_by(delta_y)
        } else {
            self.list.scroll_by(delta_y)
        }
    }

    fn wants_keystrokes(&self) -> bool {
        self.shared.borrow().filter_focused
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let mut state = self.shared.borrow_mut();
        match keystroke.key.as_str() {
            "escape" | "enter" => state.filter_focused = false,
            "v" if keystroke.modifiers.cmd => return TerminalKeyOutcome::Paste,
            _ => match Self::edit_key(keystroke) {
                Some((key, shift)) => {
                    state.filter.key(key, shift);
                }
                None => return TerminalKeyOutcome::Ignored,
            },
        }
        state.changed();
        TerminalKeyOutcome::Handled
    }

    fn input_text(&mut self, text: &str) {
        let mut state = self.shared.borrow_mut();
        if state.filter_focused {
            let line: String = text.chars().filter(|c| !c.is_control()).collect();
            state.filter.insert(&line);
            state.changed();
        }
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        self.input_text(text);
    }

    fn set_focused(&mut self, focused: bool) {
        if !focused {
            let mut state = self.shared.borrow_mut();
            if state.filter_focused {
                state.filter_focused = false;
                state.changed();
            }
        }
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let version = self.shared.borrow().version;
        let changed = version != self.seen;
        self.seen = version;
        ItemTick {
            changed,
            clipboard_store: self.clipboard.take(),
            ..ItemTick::default()
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// A tab over made-up traffic, for headless snapshots.
pub fn preview_page() -> DevRequestsPage {
    let header = |name: &str, value: &str| (name.to_string(), value.to_string());
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
            ..ProxyLogEntry::default()
        }
    };
    let mut hook = entry(5, RequestKind::Webhook, "POST", "/hooks/stripe", 502, 84);
    hook.target = "2 workspaces".into();
    hook.profile.clear();
    hook.request_headers = vec![
        header("content-type", "application/json"),
        header("stripe-signature", "t=1727671325,v1=5257a8c0d1"),
        header("user-agent", "Stripe/1.0"),
        header("authorization", pom_proxy::REDACTED),
    ];
    hook.response_headers = vec![header("content-type", "application/json")];
    hook.deliveries = vec![
        Delivery {
            workspace: "main".into(),
            port: 4100,
            status: Some(200),
            error: String::new(),
            ms: 38,
        },
        Delivery {
            workspace: "feat-login".into(),
            port: 4131,
            status: None,
            error: "connection refused".into(),
            ms: 2,
        },
    ];
    let body = br#"{"id":"evt_1Q2xYz","type":"invoice.paid","data":{"object":{"id":"in_1Q2xAb","amount_paid":4900,"currency":"usd","customer":"cus_Qw12"}}}"#;
    let payload = |bytes: &[u8]| Payload {
        bytes: bytes.to_vec(),
        total: bytes.len() as u64,
        complete: true,
        evicted: false,
    };
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
        payloads: Some((
            5,
            payload(body),
            payload(br#"{"ok":true,"service":"api/server","fanout":2}"#),
        )),
        ..DevRequestsState::default()
    };
    DevRequestsPage::new(Rc::new(RefCell::new(state)))
}

pub fn board_page(with_requests: bool) -> DevRequestsPage {
    let header = |name: &str, value: &str| (name.to_string(), value.to_string());
    let entry = |seq: u64,
                 time: &str,
                 kind: RequestKind,
                 method: &str,
                 path: &str,
                 status: u16,
                 ms: u64| {
        ProxyLogEntry {
            seq,
            kind,
            time: time.into(),
            method: method.into(),
            path: path.into(),
            repo: "api".into(),
            service: "server".into(),
            profile: "local".into(),
            target: "127.0.0.1:3000".into(),
            status,
            ms,
            ..ProxyLogEntry::default()
        }
    };
    let mut sessions = entry(
        6,
        "15:40:19",
        RequestKind::Proxy,
        "POST",
        "/_pom_dev/api/server/v1/sessions",
        401,
        9,
    );
    sessions.target = "http://server.api.feat-login.localhost:8767/v1/sessions".into();
    sessions.request_headers = vec![
        header("content-type", "application/json"),
        header("origin", "http://app.web.feat-login.localhost:8767"),
        header("cookie", pom_proxy::REDACTED),
    ];
    let body = br#"{"email":"dev@example.com","password":"********"}"#;
    let payload = |bytes: &[u8]| Payload {
        bytes: bytes.to_vec(),
        total: bytes.len() as u64,
        complete: true,
        evicted: false,
    };
    let entries = if with_requests {
        vec![
            entry(
                7,
                "15:40:24",
                RequestKind::Proxy,
                "GET",
                "/_pom_dev/api/server/v1/users/42",
                200,
                6,
            ),
            sessions,
            entry(
                5,
                "15:40:11",
                RequestKind::Proxy,
                "GET",
                "/_pom_dev/api/server/v1/users?page=2",
                200,
                12,
            ),
            entry(
                4,
                "15:39:58",
                RequestKind::Webhook,
                "POST",
                "/hooks/payments",
                200,
                15,
            ),
            entry(
                3,
                "15:39:40",
                RequestKind::Proxy,
                "GET",
                "/_pom_dev/web/app/assets/main.js",
                200,
                18,
            ),
            entry(
                2,
                "15:39:02",
                RequestKind::Webhook,
                "POST",
                "/hooks/payments",
                202,
                21,
            ),
            entry(
                1,
                "15:38:51",
                RequestKind::Proxy,
                "GET",
                "/_pom_dev/api/server/health",
                503,
                24,
            ),
        ]
    } else {
        Vec::new()
    };
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
        selected: with_requests.then_some(6),
        payloads: with_requests.then(|| (6, payload(body), payload(b""))),
        ..DevRequestsState::default()
    };
    DevRequestsPage::new(Rc::new(RefCell::new(state)))
}

#[cfg(test)]
mod tests;
