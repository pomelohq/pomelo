//! The Ports tab of a remote window: what of the host is open here, a field to open another port, and where the
//! host's workspace addresses go.

use std::sync::Arc;

use pom_remote::{ForwardKind, ForwardState, PortForward, RemotePorts};
use terminal::{Keystroke, Modifiers};
use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Rect, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::{EditKey, Item, ItemTick, TerminalKeyOutcome};

pub const PORTS_TAB_ID: &str = "remote-ports";

const FIELD: u64 = 1;
const FORWARD: u64 = 2;
const ROW_BASE: u64 = 1_000;
const ROW_STRIDE: u64 = 4;
const OPEN: u64 = 0;
const COPY: u64 = 1;
const STOP: u64 = 2;
const TOOLBAR_H: f32 = 46.0;
const CARD_INSET: f32 = 8.0;
const FOOT_H: f32 = 36.0;
const HEAD_H: f32 = 28.0;
const ROW_H: f32 = 30.0;

/// What the tab shows and does; the window's forwards in the app, a fixed list on a board.
pub trait PortsAccess: Send + Sync {
    fn host(&self) -> String;
    /// The workspace shown, whose shared services are forwarded.
    fn branch(&self) -> String;
    fn proxy_port_here(&self) -> u16;
    fn forwards(&self) -> Vec<PortForward>;
    /// Changes whenever the forwards do.
    fn version(&self) -> u64;
    fn forward(&self, remote_port: u16, local: Option<u16>);
    fn stop(&self, remote_port: u16);
}

impl PortsAccess for RemotePorts {
    fn host(&self) -> String {
        RemotePorts::host(self).to_string()
    }
    fn branch(&self) -> String {
        RemotePorts::branch(self)
    }
    fn proxy_port_here(&self) -> u16 {
        RemotePorts::proxy_port_here(self)
    }
    fn forwards(&self) -> Vec<PortForward> {
        RemotePorts::forwards(self)
    }
    fn version(&self) -> u64 {
        RemotePorts::version(self)
    }
    fn forward(&self, remote_port: u16, local: Option<u16>) {
        RemotePorts::forward(self, remote_port, local);
    }
    fn stop(&self, remote_port: u16) {
        RemotePorts::stop(self, remote_port);
    }
}

/// `<port>` or `<port>:<local>`.
pub fn parse_port(text: &str) -> Option<(u16, Option<u16>)> {
    let port = |text: &str| text.trim().parse::<u16>().ok().filter(|port| *port > 0);
    match text.trim().split_once(':') {
        Some((remote, local)) => Some((port(remote)?, Some(port(local)?))),
        None => Some((port(text)?, None)),
    }
}

fn small(text: impl Into<String>, color: Rgba) -> ui::Label {
    label(text.into()).label_size(LabelSize::Small).color(color)
}

fn head_cell(text: &str) -> ui::Label {
    label(text.to_uppercase())
        .size(11.0)
        .weight(600)
        .color(theme().text_placeholder)
}

fn state_of(forward: &PortForward) -> (&'static str, Rgba, Rgba) {
    let colors = theme();
    match forward.state {
        ForwardState::Failed => ("Failed", colors.error, colors.error),
        ForwardState::Starting => ("Starting", colors.accent_fill, colors.text_muted),
        ForwardState::Lost => ("Reconnecting", colors.warning, colors.warning),
        ForwardState::Forwarded if forward.moved() => ("Moved", colors.warning, colors.warning),
        ForwardState::Forwarded => ("Forwarded", colors.success, colors.text_muted),
    }
}

fn icon_button(id: u64, kind: IconKind, hovered: Option<u64>) -> Node {
    let mut button = div()
        .row()
        .items_center()
        .justify_center()
        .w_px(24.0)
        .h_px(24.0)
        .rounded(6.0)
        .on_click(id)
        .child(icon(kind).size(14.0).color(theme().text));
    if hovered == Some(id) {
        button = button.bg(theme().ghost_element_hover);
    }
    button.into()
}

fn fixed(width: f32, child: impl Into<Node>) -> ui::Div {
    div().row().items_center().w_px(width).child(child)
}

fn row(index: usize, forward: &PortForward, hovered: Option<u64>) -> Node {
    let colors = theme();
    let (state, dot, ink) = state_of(forward);
    let failed = forward.state == ForwardState::Failed;
    let base = ROW_BASE + index as u64 * ROW_STRIDE;
    let here = if forward.local_port == 0 {
        "-".to_string()
    } else {
        forward.address()
    };
    let mut what = div()
        .row()
        .items_center()
        .gap(6.0)
        .flex(1.0)
        .child(small(forward.label.clone(), colors.text).truncate());
    if forward.kind == ForwardKind::Shared {
        what = what.child(
            div()
                .row()
                .h_px(16.0)
                .px(4.0)
                .items_center()
                .rounded(3.0)
                .border(1.0, colors.border_variant)
                .child(
                    label("shared")
                        .size(10.5)
                        .mono()
                        .color(colors.text_placeholder),
                ),
        );
    }
    let mut actions = div().row().items_center().justify_end().gap(4.0).w_px(84.0);
    if !failed {
        actions = actions
            .child(icon_button(base + OPEN, IconKind::ArrowUpRight, hovered))
            .child(icon_button(base + COPY, IconKind::Copy, hovered));
    }
    actions = actions.child(icon_button(
        base + STOP,
        if failed {
            IconKind::Close
        } else {
            IconKind::Stop
        },
        hovered,
    ));
    let line = div()
        .row()
        .items_center()
        .gap(10.0)
        .h_px(ROW_H)
        .pl(12.0)
        .pr(6.0)
        .child(ui::dot(dot, 8.0))
        .child(fixed(
            64.0,
            small(forward.remote_port.to_string(), colors.text).mono(),
        ))
        .child(fixed(140.0, small(here, colors.text).mono()))
        .child(what)
        .child(fixed(96.0, small(state, ink)))
        .child(actions);
    let mut column = div().col().child(line);
    if !forward.message.is_empty() {
        let note = if failed {
            colors.error
        } else {
            colors.text_placeholder
        };
        column = column.child(
            div().col().pl(30.0).pr(12.0).pb(8.0).child(
                div()
                    .row()
                    .h_px(16.0)
                    .items_center()
                    .child(small(forward.message.clone(), note).truncate()),
            ),
        );
    }
    column.into()
}

fn head() -> Node {
    div()
        .row()
        .items_center()
        .gap(10.0)
        .h_px(HEAD_H)
        .pl(12.0)
        .pr(6.0)
        .child(div().w_px(8.0))
        .child(fixed(64.0, head_cell("On host")))
        .child(fixed(140.0, head_cell("Here")))
        .child(div().row().flex(1.0).child(head_cell("What")))
        .child(fixed(96.0, head_cell("State")))
        .child(div().w_px(84.0))
        .into()
}

fn empty(branch: &str, host: &str) -> Node {
    let colors = theme();
    div()
        .col()
        .items_center()
        .gap(8.0)
        .pt(60.0)
        .child(
            icon(IconKind::ArrowUpRight)
                .size(20.0)
                .color(colors.text_placeholder),
        )
        .child(
            label("No ports forwarded")
                .size(13.0)
                .medium()
                .color(colors.text_placeholder),
        )
        .child(workspace::paragraph(
            &format!(
                "Shared services of {branch} are forwarded once they run. Forward any other port of {host} above."
            ),
            12.0,
            16.0,
            360.0,
            colors.text_muted,
        ))
        .into()
}

pub struct PortsPage {
    ports: Arc<dyn PortsAccess>,
    open_url: Arc<dyn Fn(&str)>,
    field: TextField,
    focused: bool,
    invalid: bool,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    scroll: f32,
    content_h: f32,
    view_h: f32,
    seen: u64,
    changed: bool,
    clipboard: Option<String>,
    /// The rows as last drawn, so a click finds its forward.
    shown: Vec<PortForward>,
}

impl PortsPage {
    pub fn new(ports: Arc<dyn PortsAccess>, open_url: Arc<dyn Fn(&str)>) -> PortsPage {
        PortsPage {
            ports,
            open_url,
            field: TextField::default(),
            focused: false,
            invalid: false,
            hits: Vec::new(),
            hovered: None,
            scroll: 0.0,
            content_h: 0.0,
            view_h: 0.0,
            seen: u64::MAX,
            changed: true,
            clipboard: None,
            shown: Vec::new(),
        }
    }

    fn port_on(&self, forward: &PortForward) -> String {
        format!("{}:{}", self.ports.host(), forward.remote_port)
    }

    fn toolbar(&self) -> Node {
        let colors = theme();
        let host = self.ports.host();
        let border = if self.invalid {
            colors.error
        } else if self.focused {
            colors.border_focused
        } else {
            colors.border
        };
        let mut field = div()
            .row()
            .items_center()
            .gap(8.0)
            .w_px(200.0)
            .h_px(30.0)
            .px(10.0)
            .rounded(6.0)
            .border(1.0, border)
            .bg(colors.editor_background)
            .on_click(FIELD)
            .child(
                icon(IconKind::Plus)
                    .size(14.0)
                    .color(colors.text_placeholder),
            )
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .items_center()
                    .child(self.field.render(
                        &format!("Port on {host}"),
                        self.focused,
                        colors.text,
                        20.0,
                        FieldFont::Mono,
                    )),
            );
        if self.focused {
            field = field.ring(3.0, colors.overlay_selected);
        }
        let gateway = self
            .ports
            .forwards()
            .into_iter()
            .find(|forward| forward.kind == ForwardKind::Gateway);
        let here = self.ports.proxy_port_here();
        let (dot, text) = match gateway.as_ref().map(|gateway| gateway.state) {
            Some(ForwardState::Forwarded) => (
                colors.success,
                format!("workspace addresses *.localhost:{here} go to {host}"),
            ),
            Some(ForwardState::Lost) => (
                colors.warning,
                format!("workspace addresses *.localhost:{here} wait for {host}"),
            ),
            Some(ForwardState::Failed) => (
                colors.error,
                format!("workspace addresses do not reach {host}"),
            ),
            _ => (
                colors.text_disabled,
                format!("workspace addresses reach {host} once connected"),
            ),
        };
        div()
            .row()
            .items_center()
            .gap(8.0)
            .px(CARD_INSET)
            .h_px(TOOLBAR_H)
            .child(field)
            .child(ui::button(FORWARD, "Forward").hovered(self.hovered))
            .child(div().flex(1.0))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(6.0)
                    .child(ui::dot(dot, 7.0))
                    .child(small(text, colors.text_muted)),
            )
            .into()
    }

    fn list(&self) -> Node {
        if self.shown.is_empty() {
            return empty(&self.ports.branch(), &self.ports.host());
        }
        let mut column = div().col().child(head());
        for (index, forward) in self.shown.iter().enumerate() {
            column = column.child(row(index, forward, self.hovered));
        }
        column.into()
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

    fn submit(&mut self) {
        match parse_port(&self.field.text()) {
            Some((remote, local)) => {
                self.ports.forward(remote, local);
                self.field.set_text("");
                self.invalid = false;
            }
            None => self.invalid = true,
        }
        self.changed = true;
    }

    pub fn click(&mut self, id: u64) {
        self.focused = id == FIELD;
        self.changed = true;
        match id {
            FORWARD => self.submit(),
            id if id >= ROW_BASE => {
                let index = ((id - ROW_BASE) / ROW_STRIDE) as usize;
                let Some(forward) = self.shown.get(index).cloned() else {
                    return;
                };
                match (id - ROW_BASE) % ROW_STRIDE {
                    OPEN => (self.open_url)(&format!("http://{}", forward.address())),
                    COPY => self.clipboard = Some(forward.address()),
                    STOP => self.ports.stop(forward.remote_port),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The text a row's Copy puts on the clipboard, and what its port is on the host (for tests).
    pub fn describe(&self, index: usize) -> Option<(String, String)> {
        let forward = self.shown.get(index)?;
        Some((forward.address(), self.port_on(forward)))
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

    fn refresh(&mut self) {
        self.shown = self
            .ports
            .forwards()
            .into_iter()
            .filter(|forward| forward.kind != ForwardKind::Gateway)
            .collect();
    }
}

fn merge(into: &mut ui::Painted, part: ui::Painted) {
    into.rects.extend(part.rects);
    into.tris.extend(part.tris);
    into.texts.extend(part.texts);
    into.icons.extend(part.icons);
    into.hits.extend(part.hits);
}

impl Item for PortsPage {
    fn id(&self) -> Option<String> {
        Some(PORTS_TAB_ID.to_string())
    }

    fn title(&self) -> String {
        "Ports".into()
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Server)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        self.refresh();
        let scale = ui::ui_text_scale();
        let bar_h = TOOLBAR_H * scale;
        let inset = CARD_INSET * scale;
        let foot_h = FOOT_H * scale;
        let card = Rect::new(
            body.x + inset,
            body.y + bar_h,
            (body.w - 2.0 * inset).max(0.0),
            (body.h - bar_h - inset).max(0.0),
            Rgba::TRANSPARENT,
        );
        let mut painted = ui::render(
            &self.toolbar(),
            Rect::new(body.x, body.y, body.w, bar_h, Rgba::TRANSPARENT),
        );
        merge(
            &mut painted,
            ui::render(
                &div().rounded(6.0).bg(theme().panel_background).into(),
                card,
            ),
        );
        let list = self.list();
        let list_h = (card.h - foot_h).max(0.0);
        self.content_h = ui::measure(&list).1 * scale;
        self.view_h = list_h;
        self.scroll = self
            .scroll
            .clamp(0.0, (self.content_h - self.view_h).max(0.0));
        let mut rows = ui::render(
            &list,
            Rect::new(
                card.x,
                card.y - self.scroll,
                card.w,
                list_h.max(self.content_h),
                Rgba::TRANSPARENT,
            ),
        );
        rows.hits
            .retain(|(rect, _)| rect.y + rect.h > card.y && rect.y < card.y + list_h);
        merge(&mut painted, rows);
        let foot = div()
            .row()
            .items_center()
            .h_px(FOOT_H)
            .px(14.0)
            .child(small(
                "Forwards run while this window is open and stop when the app closes.",
                theme().text_placeholder,
            ));
        merge(
            &mut painted,
            ui::render(
                &foot.into(),
                Rect::new(card.x, card.y + list_h, card.w, foot_h, Rgba::TRANSPARENT),
            ),
        );
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        match self.hit(x, y) {
            Some(id) => self.click(id),
            None if self.focused => {
                self.focused = false;
                self.changed = true;
            }
            None => {}
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
        let next = (self.scroll - delta_y).clamp(0.0, (self.content_h - self.view_h).max(0.0));
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn wants_keystrokes(&self) -> bool {
        self.focused
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        match keystroke.key.as_str() {
            "enter" => self.submit(),
            "escape" => self.focused = false,
            "v" if keystroke.modifiers.cmd => return TerminalKeyOutcome::Paste,
            _ => match Self::edit_key(keystroke) {
                Some((key, shift)) => {
                    self.field.key(key, shift);
                    self.invalid = false;
                }
                None => return TerminalKeyOutcome::Ignored,
            },
        }
        self.changed = true;
        TerminalKeyOutcome::Handled
    }

    fn input_text(&mut self, text: &str) {
        if self.focused {
            let line: String = text.chars().filter(|c| !c.is_control()).collect();
            self.field.insert(&line);
            self.invalid = false;
            self.changed = true;
        }
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        self.input_text(text);
    }

    fn set_focused(&mut self, focused: bool) {
        if !focused && self.focused {
            self.focused = false;
            self.changed = true;
        }
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let version = self.ports.version();
        let changed = version != self.seen || std::mem::take(&mut self.changed);
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

/// A fixed list for the canvas boards and tests.
pub struct BoardPorts {
    pub host: String,
    pub branch: String,
    pub proxy_port_here: u16,
    pub forwards: std::sync::Mutex<Vec<PortForward>>,
    pub asked: std::sync::Mutex<Vec<(u16, Option<u16>)>>,
    pub stopped: std::sync::Mutex<Vec<u16>>,
}

impl PortsAccess for BoardPorts {
    fn host(&self) -> String {
        self.host.clone()
    }
    fn branch(&self) -> String {
        self.branch.clone()
    }
    fn proxy_port_here(&self) -> u16 {
        self.proxy_port_here
    }
    fn forwards(&self) -> Vec<PortForward> {
        self.forwards
            .lock()
            .map(|forwards| forwards.clone())
            .unwrap_or_default()
    }
    fn version(&self) -> u64 {
        0
    }
    fn forward(&self, remote_port: u16, local: Option<u16>) {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push((remote_port, local));
        }
    }
    fn stop(&self, remote_port: u16) {
        if let Ok(mut stopped) = self.stopped.lock() {
            stopped.push(remote_port);
        }
    }
}

fn forwarded(remote: u16, local: u16, label: &str, kind: ForwardKind) -> PortForward {
    PortForward {
        remote_port: remote,
        local_port: local,
        wanted_port: remote,
        label: label.to_string(),
        kind,
        state: ForwardState::Forwarded,
        message: String::new(),
        via: "shared".into(),
    }
}

/// The boards' forwards: `forwarded`, `moved` (5432 taken here), `error` (3000 asked for exactly) or `empty`.
pub fn board_ports(state: &str) -> BoardPorts {
    let mut forwards = vec![
        forwarded(8767, 52311, "dev proxy", ForwardKind::Gateway),
        forwarded(5432, 5432, "postgres", ForwardKind::Shared),
        forwarded(6379, 6379, "redis", ForwardKind::Shared),
        forwarded(9000, 9000, "minio", ForwardKind::Shared),
        forwarded(9229, 9229, "9229", ForwardKind::Manual),
    ];
    match state {
        "moved" => {
            forwards[1].local_port = 5433;
            forwards[1].message = "port 5432 is in use on this machine, so it is on 5433".into();
        }
        "error" => {
            forwards[4] = PortForward {
                remote_port: 3000,
                wanted_port: 3000,
                label: "3000".into(),
                kind: ForwardKind::Manual,
                state: ForwardState::Failed,
                message: "port 3000 is in use on this machine".into(),
                ..PortForward::default()
            };
        }
        "empty" => forwards.truncate(1),
        _ => {}
    }
    BoardPorts {
        host: "dev".into(),
        branch: "feat-login".into(),
        proxy_port_here: 8767,
        forwards: std::sync::Mutex::new(forwards),
        asked: std::sync::Mutex::new(Vec::new()),
        stopped: std::sync::Mutex::new(Vec::new()),
    }
}

pub fn board_page(state: &str) -> PortsPage {
    PortsPage::new(Arc::new(board_ports(state)), Arc::new(|_: &str| {}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_parse_with_or_without_a_port_here() {
        assert_eq!(parse_port("5432"), Some((5432, None)));
        assert_eq!(parse_port(" 8080:18080 "), Some((8080, Some(18080))));
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("70000"), None);
        assert_eq!(parse_port("web"), None);
        assert_eq!(parse_port("80:"), None);
    }

    #[test]
    fn rows_forward_copy_and_stop_their_own_port() {
        let ports = Arc::new(board_ports("moved"));
        let opened = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = opened.clone();
        let mut page = PortsPage::new(
            ports.clone(),
            Arc::new(move |url: &str| {
                if let Ok(mut opened) = seen.lock() {
                    opened.push(url.to_string());
                }
            }),
        );
        page.refresh();
        assert_eq!(page.shown.len(), 4, "the dev proxy is in the toolbar");
        assert_eq!(
            page.describe(0),
            Some(("127.0.0.1:5433".to_string(), "dev:5432".to_string()))
        );
        page.click(ROW_BASE + OPEN);
        page.click(ROW_BASE + ROW_STRIDE + COPY);
        page.click(ROW_BASE + 3 * ROW_STRIDE + STOP);
        assert_eq!(
            opened
                .lock()
                .map(|opened| opened.clone())
                .unwrap_or_default(),
            vec!["http://127.0.0.1:5433".to_string()]
        );
        assert_eq!(page.clipboard.as_deref(), Some("127.0.0.1:6379"));
        assert_eq!(
            ports
                .stopped
                .lock()
                .map(|stopped| stopped.clone())
                .unwrap_or_default(),
            vec![9229]
        );

        page.click(FIELD);
        page.input_text("8080:18080");
        page.click(FORWARD);
        page.click(FIELD);
        page.input_text("nope");
        page.submit();
        assert!(page.invalid);
        assert_eq!(
            ports
                .asked
                .lock()
                .map(|asked| asked.clone())
                .unwrap_or_default(),
            vec![(8080, Some(18080))]
        );
    }
}
