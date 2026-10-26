//! The host tab of Settings opened from a remote window: the settings pom on that machine reads, read and changed
//! through its pom server, beside the This Mac tab.

use std::sync::Arc;

use serde_json::{Map, Value};
use settings::Settings;
use ui::{div, label, theme, LabelSize, Node, Radius};

use super::{
    control_value, dim_c, is_default, jira_items, proxy_text, Control, IntegrationsPage, Page,
    PageItem, SettingRow, AGENT, CTRL_AGENT_COMMAND, CTRL_MODULES_DAYS_DEC, CTRL_MODULES_DAYS_EDIT,
    CTRL_MODULES_DAYS_INC, CTRL_MODULES_ENABLED, CTRL_MODULES_FALLBACK, CTRL_MODULES_LIMIT_DEC,
    CTRL_MODULES_LIMIT_EDIT, CTRL_MODULES_LIMIT_INC, CTRL_PROXY_BIND, CTRL_PROXY_DOMAIN,
    CTRL_PROXY_PORT_DEC, CTRL_PROXY_PORT_EDIT, CTRL_PROXY_PORT_INC, CTRL_PROXY_PUBLIC_PORT,
    CTRL_PROXY_SCHEME, CTRL_PROXY_TRUSTED, CTRL_TERM_SHELL, CTRL_WEBHOOK_PORT_DEC,
    CTRL_WEBHOOK_PORT_EDIT, CTRL_WEBHOOK_PORT_INC, DEV_SERVICES, GENERAL, INTEGRATIONS, TERMINAL,
};

pub const CTRL_TAB_MAC: u64 = 392;
pub const CTRL_TAB_HOST: u64 = 393;
pub const CTRL_HOST_RETRY: u64 = 394;
pub const CTRL_HOST_UPDATE_POM: u64 = 395;
pub const CTRL_HOST_OPEN_JSON: u64 = 396;
pub const CTRL_IDLE_SLEEP_DEC: u64 = 600;
pub const CTRL_IDLE_SLEEP_INC: u64 = 601;
pub const CTRL_IDLE_SLEEP_EDIT: u64 = 602;
pub const CTRL_MAX_RUNNING_DEC: u64 = 603;
pub const CTRL_MAX_RUNNING_INC: u64 = 604;
pub const CTRL_MAX_RUNNING_EDIT: u64 = 605;
pub const CTRL_IDLE_EXIT_DEC: u64 = 606;
pub const CTRL_IDLE_EXIT_INC: u64 = 607;
pub const CTRL_IDLE_EXIT_EDIT: u64 = 608;
pub const CTRL_RETENTION_DEC: u64 = 609;
pub const CTRL_RETENTION_INC: u64 = 610;
pub const CTRL_RETENTION_EDIT: u64 = 611;

/// The host tab's pages, in its navbar's order.
pub const HOST_CATEGORIES: [usize; 5] = [GENERAL, AGENT, TERMINAL, DEV_SERVICES, INTEGRATIONS];

/// Where the host keeps them when its pom does not say.
pub const HOST_SETTINGS_FALLBACK: &str = "~/.config/pomelo/settings.json";

/// The settings rows a host key is edited with: its error shows on the row of this control.
const KEY_CONTROLS: [(&str, u64); 17] = [
    ("agent_command", CTRL_AGENT_COMMAND),
    ("terminal_shell", CTRL_TERM_SHELL),
    ("modules_store_enabled", CTRL_MODULES_ENABLED),
    ("modules_fallback", CTRL_MODULES_FALLBACK),
    ("modules_size_limit_gb", CTRL_MODULES_LIMIT_EDIT),
    ("modules_unused_days", CTRL_MODULES_DAYS_EDIT),
    ("dev_proxy_port", CTRL_PROXY_PORT_EDIT),
    ("webhook_port", CTRL_WEBHOOK_PORT_EDIT),
    ("proxy.bind", CTRL_PROXY_BIND),
    ("proxy.domain", CTRL_PROXY_DOMAIN),
    ("proxy.public_scheme", CTRL_PROXY_SCHEME),
    ("proxy.public_port", CTRL_PROXY_PUBLIC_PORT),
    ("proxy.trusted_proxies", CTRL_PROXY_TRUSTED),
    ("server.idle_exit_minutes", CTRL_IDLE_EXIT_EDIT),
    ("server.reconnect_retention_minutes", CTRL_RETENTION_EDIT),
    ("workspaces.idle_sleep_minutes", CTRL_IDLE_SLEEP_EDIT),
    ("workspaces.max_running", CTRL_MAX_RUNNING_EDIT),
];

pub fn key_of_control(id: u64) -> Option<&'static str> {
    KEY_CONTROLS
        .iter()
        .find(|(_, control)| *control == id)
        .map(|(key, _)| *key)
}

/// Minutes a host's server and workspaces wait at most: a week.
pub const HOST_MINUTES_MAX: u64 = 10_080;
pub const HOST_RUNNING_MAX: u64 = 64;

/// A whole-number host setting a stepper edits, by any of its three controls: its value, step and upper bound.
pub fn host_number(id: u64, s: &mut Settings) -> Option<(&mut u64, i64, u64)> {
    Some(match id {
        CTRL_IDLE_SLEEP_DEC..=CTRL_IDLE_SLEEP_EDIT => {
            (&mut s.workspaces.idle_sleep_minutes, 5, HOST_MINUTES_MAX)
        }
        CTRL_MAX_RUNNING_DEC..=CTRL_MAX_RUNNING_EDIT => {
            (&mut s.workspaces.max_running, 1, HOST_RUNNING_MAX)
        }
        CTRL_IDLE_EXIT_DEC..=CTRL_IDLE_EXIT_EDIT => {
            (&mut s.server.idle_exit_minutes, 5, HOST_MINUTES_MAX)
        }
        CTRL_RETENTION_DEC..=CTRL_RETENTION_EDIT => (
            &mut s.server.reconnect_retention_minutes,
            1,
            HOST_MINUTES_MAX,
        ),
        _ => return None,
    })
}

pub fn is_host_number_edit(id: u64) -> bool {
    matches!(
        id,
        CTRL_IDLE_SLEEP_EDIT | CTRL_MAX_RUNNING_EDIT | CTRL_IDLE_EXIT_EDIT | CTRL_RETENTION_EDIT
    )
}

/// Steps a host number by its dec or inc control; true when it changed.
pub fn step_host_number(id: u64, s: &mut Settings) -> bool {
    let up = matches!(
        id,
        CTRL_IDLE_SLEEP_INC | CTRL_MAX_RUNNING_INC | CTRL_IDLE_EXIT_INC | CTRL_RETENTION_INC
    );
    let down = matches!(
        id,
        CTRL_IDLE_SLEEP_DEC | CTRL_MAX_RUNNING_DEC | CTRL_IDLE_EXIT_DEC | CTRL_RETENTION_DEC
    );
    let Some((value, step, max)) = host_number(id, s).filter(|_| up || down) else {
        return false;
    };
    super::step_u64(value, if up { step } else { -step }, 0, max)
}

/// The default of the host number an edit control shows, compared and reset by it.
fn host_number_default(id: u64) -> Option<u64> {
    let mut defaults = Settings::default();
    host_number(id, &mut defaults).map(|(value, _, _)| *value)
}

pub fn host_number_is_default(id: u64, s: &Settings) -> Option<bool> {
    let mut current = s.clone();
    let now = host_number(id, &mut current).map(|(value, _, _)| *value)?;
    Some(Some(now) == host_number_default(id))
}

pub fn reset_host_number(id: u64, s: &mut Settings) -> Option<bool> {
    let default = host_number_default(id)?;
    let (value, _, _) = host_number(id, s)?;
    Some(std::mem::replace(value, default) != default)
}

/// What a host answered `settings.get` / `settings.set` with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostAnswer {
    /// Its host keys, nested as settings.json nests them, defaults filled.
    pub values: Value,
    /// The dotted keys it takes.
    pub keys: Vec<String>,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    Unreachable(String),
    /// Its pom has no settings.get / settings.set.
    Outdated(String),
    /// It does not take the value (the message names why).
    Refused(String),
    Failed(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Unreachable(message)
            | HostError::Outdated(message)
            | HostError::Refused(message)
            | HostError::Failed(message) => formatter.write_str(message),
        }
    }
}

/// A host's own settings, kept there.
pub trait HostSettingsStore: Send + Sync {
    fn read(&self) -> Result<HostAnswer, HostError>;
    /// Lays the dotted keys of `values` over the host's settings.json.
    fn change(&self, values: Value) -> Result<HostAnswer, HostError>;
}

/// The machine a remote window's project runs on, for the host tab.
#[derive(Clone)]
pub struct RemoteHost {
    pub name: String,
    /// Its pom's version, empty when not known yet.
    pub version: String,
    pub store: Arc<dyn HostSettingsStore>,
}

/// The remote window's session to its host, as the tab's dot shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostLink {
    Connecting,
    #[default]
    Connected,
    Reconnecting,
    Disconnected,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HostLoad {
    #[default]
    Reading,
    Ready,
    Unreachable(String),
    Outdated(String),
    Failed(String),
}

/// The host tab's state: which tab is shown, and the host's settings as last read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostPage {
    pub name: String,
    /// The host tab is shown, not This Mac.
    pub shown: bool,
    pub load: HostLoad,
    /// The host's values (defaults until read).
    pub settings: Settings,
    pub keys: Vec<String>,
    pub path: String,
    pub version: String,
    pub link: HostLink,
    /// A change the host did not take: the dotted key and why.
    pub errors: Vec<(String, String)>,
}

impl HostPage {
    pub fn new(name: &str, version: &str) -> HostPage {
        HostPage {
            name: name.to_string(),
            version: version.to_string(),
            ..HostPage::default()
        }
    }

    /// Whether the tab shows `key`: what the host lists once read, every key while reading.
    pub fn takes(&self, key: &str) -> bool {
        match self.load {
            HostLoad::Ready => self.keys.iter().any(|known| known == key),
            _ => settings::HOST_KEYS.contains(&key),
        }
    }

    /// Takes an answer of the host; false when its values are not settings this app reads.
    pub fn take_answer(&mut self, answer: HostAnswer) -> bool {
        match serde_json::from_value::<Settings>(answer.values) {
            Ok(values) => {
                self.settings = values;
                self.keys = answer.keys;
                self.path = answer.path;
                self.load = HostLoad::Ready;
                true
            }
            Err(error) => {
                self.load = HostLoad::Failed(format!(
                    "{} answered with settings this app cannot read ({error})",
                    self.name
                ));
                false
            }
        }
    }

    /// The row errors keyed by the control they show on.
    pub fn control_errors(&self) -> Vec<(u64, String)> {
        self.errors
            .iter()
            .filter_map(|(key, message)| {
                KEY_CONTROLS
                    .iter()
                    .find(|(known, _)| known == key)
                    .map(|(_, control)| (*control, message.clone()))
            })
            .collect()
    }

    pub fn settings_path(&self) -> String {
        if self.path.is_empty() {
            HOST_SETTINGS_FALLBACK.to_string()
        } else {
            self.path.clone()
        }
    }
}

fn lookup<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.')
        .try_fold(value, |value, part| value.get(part))
}

/// The host keys whose values differ between `before` and `after`, dotted, as a `settings.set` change.
pub fn host_change(before: &Settings, after: &Settings) -> Option<Map<String, Value>> {
    let before = serde_json::to_value(before).ok()?;
    let after = serde_json::to_value(after).ok()?;
    let mut change = Map::new();
    for key in settings::HOST_KEYS {
        let now = lookup(&after, key);
        if lookup(&before, key) != now {
            if let Some(now) = now {
                change.insert(key.to_string(), now.clone());
            }
        }
    }
    (!change.is_empty()).then_some(change)
}

/// Puts the keys of `change` back to their values in `from`.
pub fn restore(target: &mut Settings, from: &Settings, change: &Map<String, Value>) {
    let (Ok(mut current), Ok(old)) = (serde_json::to_value(&*target), serde_json::to_value(from))
    else {
        return;
    };
    for key in change.keys() {
        let Some(value) = lookup(&old, key).cloned() else {
            continue;
        };
        if let Some(slot) = current.pointer_mut(&format!("/{}", key.replace('.', "/"))) {
            *slot = value;
        }
    }
    match serde_json::from_value(current) {
        Ok(restored) => *target = restored,
        Err(error) => eprintln!("settings: restore host values: {error}"),
    }
}

/// This Mac's tab says this under each section the host has its own of.
pub fn mac_note(host: &str) -> String {
    format!("Applies to projects on this Mac; for {host} use its tab")
}

/// The sections of a This Mac page the host has its own of.
pub fn mac_only_sections(category: usize) -> &'static [&'static str] {
    match category {
        AGENT => &["Command"],
        TERMINAL => &["Shell"],
        DEV_SERVICES => &[
            "Reverse Proxy",
            "Remote Access",
            "Webhook Fan-out",
            "Shared node_modules",
        ],
        _ => &[],
    }
}

fn stepper(dec: u64, inc: u64, edit: u64, value: String) -> Control {
    Control::Stepper {
        dec,
        inc,
        edit,
        value,
    }
}

fn text(id: u64, value: String, placeholder: &'static str) -> Control {
    Control::TextInput {
        id,
        value,
        placeholder,
        masked: false,
    }
}

struct Rows<'a> {
    host: &'a HostPage,
    items: Vec<PageItem>,
    section: Option<(&'static str, Vec<PageItem>)>,
}

impl<'a> Rows<'a> {
    fn new(host: &'a HostPage) -> Self {
        Rows {
            host,
            items: Vec::new(),
            section: None,
        }
    }

    fn close(&mut self) {
        if let Some((name, rows)) = self.section.take() {
            if !rows.is_empty() {
                self.items.push(PageItem::Header(name));
                self.items.extend(rows);
            }
        }
    }

    fn header(&mut self, name: &'static str) -> &mut Self {
        self.close();
        self.section = Some((name, Vec::new()));
        self
    }

    fn row(
        &mut self,
        key: &str,
        title: &'static str,
        description: String,
        control: Control,
    ) -> &mut Self {
        if !self.host.takes(key) {
            return self;
        }
        let ready = self.host.load == HostLoad::Ready;
        let reset = KEY_CONTROLS
            .iter()
            .find(|(known, _)| *known == key)
            .map(|(_, id)| *id)
            .filter(|id| ready && !is_default(*id, &self.host.settings));
        self.push(SettingRow {
            title: title.into(),
            description: description.into(),
            control: if ready { control } else { Control::Placeholder },
            reset,
        })
    }

    fn push(&mut self, row: SettingRow) -> &mut Self {
        if let Some((_, rows)) = self.section.as_mut() {
            rows.push(PageItem::Row(row));
        }
        self
    }

    fn done(mut self, title: &'static str) -> Page {
        self.close();
        Page {
            title,
            items: self.items,
        }
    }
}

/// The host tab's page for `category`, with only the keys the host takes.
pub fn host_page(category: usize, host: &HostPage, jira: &IntegrationsPage) -> Option<Page> {
    let name = host.name.as_str();
    let s = &host.settings;
    let mut rows = Rows::new(host);
    Some(match category {
        GENERAL => {
            rows.header("Workspaces")
                .row(
                    "workspaces.idle_sleep_minutes",
                    "Sleep When Idle",
                    format!("Minutes without a dev proxy request or a working agent before a workspace on {name} sleeps; 0 never."),
                    stepper(CTRL_IDLE_SLEEP_DEC, CTRL_IDLE_SLEEP_INC, CTRL_IDLE_SLEEP_EDIT, s.workspaces.idle_sleep_minutes.to_string()),
                )
                .row(
                    "workspaces.max_running",
                    "Running at Once",
                    format!("How many workspaces may run at once on {name}; 0 any number."),
                    stepper(CTRL_MAX_RUNNING_DEC, CTRL_MAX_RUNNING_INC, CTRL_MAX_RUNNING_EDIT, s.workspaces.max_running.to_string()),
                )
                .header("Server")
                .row(
                    "server.idle_exit_minutes",
                    "Idle Exit",
                    format!("Minutes pom server on {name} keeps running with no window connected before it exits; 0 never."),
                    stepper(CTRL_IDLE_EXIT_DEC, CTRL_IDLE_EXIT_INC, CTRL_IDLE_EXIT_EDIT, s.server.idle_exit_minutes.to_string()),
                )
                .row(
                    "server.reconnect_retention_minutes",
                    "Reconnect Window",
                    format!("Minutes {name} keeps a lost window's terminals, so reconnecting carries them on."),
                    stepper(CTRL_RETENTION_DEC, CTRL_RETENTION_INC, CTRL_RETENTION_EDIT, s.server.reconnect_retention_minutes.to_string()),
                );
            if !host.version.is_empty() {
                rows.push(SettingRow {
                    title: format!("pom on {name}").into(),
                    description: "The pom that serves this window and keeps these settings.".into(),
                    control: Control::Value {
                        text: host.version.clone(),
                    },
                    reset: None,
                });
            }
            rows.done("General")
        }
        AGENT => {
            rows.header("Command").row(
                "agent_command",
                "Agent Command",
                format!("The AI CLI the Agent button opens in a workspace on {name}."),
                text(CTRL_AGENT_COMMAND, s.agent_command.clone(), "claude"),
            );
            rows.done("Agent")
        }
        TERMINAL => {
            rows.header("Shell").row(
                "terminal_shell",
                "Shell",
                format!("What new terminals on {name} run; empty runs the login shell there."),
                text(CTRL_TERM_SHELL, s.terminal_shell.clone(), "login shell"),
            );
            rows.done("Terminal")
        }
        DEV_SERVICES => {
            let bind = if s.proxy.bind_ip().is_loopback() {
                format!("Only {name} itself can reach the proxy and the webhook relay. A private address lets another machine in; they stay on loopback too.")
            } else {
                "Other machines can reach the proxy and the webhook relay, and must present the proxy token (pom proxy token new). Without a token the proxy does not start, unless proxy.allow_unauthenticated is on.".to_string()
            };
            rows.header("Reverse Proxy")
                .row(
                    "dev_proxy_port",
                    "Port",
                    format!("The proxy's port on {name}; restart running services there after changing it."),
                    stepper(CTRL_PROXY_PORT_DEC, CTRL_PROXY_PORT_INC, CTRL_PROXY_PORT_EDIT, s.dev_proxy_port.to_string()),
                )
                .header("Remote Access")
                .row("proxy.bind", "Bind Address", bind, text(CTRL_PROXY_BIND, s.proxy.bind.clone(), "127.0.0.1"))
                .row(
                    "proxy.domain",
                    "Domain",
                    "Service hosts end in it, beside localhost: <service>.<repo>.<ticket or branch>.<domain>. A domain other than localhost needs the proxy token for every request to it.".to_string(),
                    text(CTRL_PROXY_DOMAIN, s.proxy.domain.clone(), "localhost"),
                )
                .row(
                    "proxy.public_scheme",
                    "Public Scheme",
                    "HTTPS when a reverse proxy in front terminates TLS: service URLs use it and cookies keep Secure.".to_string(),
                    Control::Dropdown {
                        id: CTRL_PROXY_SCHEME,
                        value: control_value(CTRL_PROXY_SCHEME, s),
                    },
                )
                .row(
                    "proxy.public_port",
                    "Public Port",
                    "The port in service URLs; empty uses the proxy's port, and 443 with HTTPS (80 with HTTP) leaves it out.".to_string(),
                    text(CTRL_PROXY_PUBLIC_PORT, proxy_text(CTRL_PROXY_PUBLIC_PORT, s), "proxy port"),
                )
                .row(
                    "proxy.trusted_proxies",
                    "Trusted Proxies",
                    "Addresses or ranges, comma separated, whose X-Forwarded-Proto and X-Forwarded-Host are believed; anyone else's are replaced.".to_string(),
                    text(CTRL_PROXY_TRUSTED, proxy_text(CTRL_PROXY_TRUSTED, s), "none"),
                )
                .header("Webhook Fan-out")
                .row(
                    "webhook_port",
                    "Port",
                    format!("Where the webhook relay on {name} listens: point an external webhook at it, /<repo>/<service>."),
                    stepper(CTRL_WEBHOOK_PORT_DEC, CTRL_WEBHOOK_PORT_INC, CTRL_WEBHOOK_PORT_EDIT, s.webhook_port.to_string()),
                )
                .header("Shared node_modules")
                .row(
                    "modules_store_enabled",
                    "Enabled",
                    format!("A new workspace on {name} whose lockfile matches a stored one takes its node_modules instead of installing."),
                    Control::Toggle {
                        id: CTRL_MODULES_ENABLED,
                        on: s.modules_store_enabled,
                    },
                )
                .row(
                    "modules_fallback",
                    "When Cloning Is Unsupported",
                    format!("What {name} does when its drive cannot clone: hard links share the files read-only, a copy takes the full size again, Run Install skips the store."),
                    Control::Dropdown {
                        id: CTRL_MODULES_FALLBACK,
                        value: control_value(CTRL_MODULES_FALLBACK, s),
                    },
                )
                .row(
                    "modules_size_limit_gb",
                    "Size Limit",
                    format!("GB kept at most on {name}; the least recently used copies go first."),
                    stepper(CTRL_MODULES_LIMIT_DEC, CTRL_MODULES_LIMIT_INC, CTRL_MODULES_LIMIT_EDIT, s.modules_size_limit_gb.to_string()),
                )
                .row(
                    "modules_unused_days",
                    "Remove Unused After",
                    "Days a copy is kept without a workspace taking it (0 keeps them until the size limit).".to_string(),
                    stepper(CTRL_MODULES_DAYS_DEC, CTRL_MODULES_DAYS_INC, CTRL_MODULES_DAYS_EDIT, s.modules_unused_days.to_string()),
                );
            rows.done("Dev Services")
        }
        INTEGRATIONS => Page {
            title: "Integrations",
            items: jira_items(jira),
        },
        _ => return None,
    })
}

/// The two tabs in the title strip: This Mac, and the host wearing its session's dot.
pub fn tabs(host: &HostPage) -> Node {
    let colors = theme();
    let dot = match (&host.load, host.link) {
        (HostLoad::Reading, _) | (_, HostLink::Connecting) => colors.text_accent,
        (_, HostLink::Connected) => colors.success,
        (_, HostLink::Reconnecting) => colors.warning,
        (_, HostLink::Disconnected) => colors.error,
    };
    components::segmented(
        [
            components::segment(CTRL_TAB_MAC, "This Mac").icon(Some(ui::IconKind::Monitor)),
            components::segment(CTRL_TAB_HOST, host.name.clone())
                .icon(Some(ui::IconKind::Server))
                .dot(dot),
        ],
        usize::from(host.shown),
    )
    .into()
}

/// What the host tab says under a page's title while the host's settings are not shown.
pub fn state_line(host: &HostPage) -> Option<Node> {
    let colors = theme();
    let name = &host.name;
    let line =
        |icon: ui::IconKind, ink: ui::Rgba, text: String, button: Option<(u64, &str)>| -> Node {
            let mut row = div()
                .row()
                .pt(14.0)
                .pb(4.0)
                .gap(8.0)
                .items_center()
                .child(ui::icon(icon).size(14.0).color(ink))
                .child(label(text).size(LabelSize::Small).color(dim_c()).truncate())
                .child(div().flex(1.0));
            if let Some((id, text)) = button {
                row = row.child(components::button(id, text));
            }
            row.into()
        };
    match &host.load {
        HostLoad::Ready => None,
        HostLoad::Reading => Some(line(
            ui::IconKind::LoadCircle,
            colors.text_placeholder,
            format!("Reading the settings on {name}..."),
            None,
        )),
        HostLoad::Unreachable(error) => Some(line(
            ui::IconKind::XCircle,
            colors.error,
            format!("Could not reach {name}: {error}"),
            Some((CTRL_HOST_RETRY, "Retry")),
        )),
        HostLoad::Failed(error) => Some(line(
            ui::IconKind::XCircle,
            colors.error,
            error.clone(),
            Some((CTRL_HOST_RETRY, "Retry")),
        )),
        HostLoad::Outdated(_) => {
            let pom = if host.version.is_empty() {
                format!("pom on {name}")
            } else {
                format!("pom {} on {name}", host.version)
            };
            let banner = div()
                .row()
                .pl(14.0)
                .pr(10.0)
                .py(10.0)
                .gap(10.0)
                .items_center()
                .rounded(Radius::Large)
                .bg(colors.element_background)
                .child(
                    ui::icon(ui::IconKind::Warning)
                        .size(16.0)
                        .color(colors.warning),
                )
                .child(
                    label(format!(
                        "{pom} cannot show its settings here. Update it to change them in this window."
                    ))
                    .size(LabelSize::Small)
                    .color(dim_c())
                    .truncate(),
                )
                .child(div().flex(1.0))
                .child(components::button(CTRL_HOST_UPDATE_POM, format!("Update pom on {name}")));
            Some(
                div()
                    .col()
                    .pt(14.0)
                    .gap(10.0)
                    .child(banner)
                    .child(
                        label(format!(
                            "Until then, {name} reads them from {} there.",
                            host.settings_path()
                        ))
                        .size(LabelSize::Small)
                        .color(colors.text_placeholder),
                    )
                    .into(),
            )
        }
    }
}

/// A section's note under its header: where it applies.
pub fn note(text: &str, under_header: bool) -> Node {
    let ink = theme().text_placeholder;
    let note = div()
        .row()
        .h_px(super::SMALL_LINE_HEIGHT)
        .gap(6.0)
        .items_center()
        .child(ui::icon(ui::IconKind::Info).size(12.0).color(ink))
        .child(label(text.to_string()).size(LabelSize::Small).color(ink));
    if under_header {
        div().col().pb(4.0).child(note).into()
    } else {
        div().col().pt(3.0).child(note).into()
    }
}

/// The message a host gave for a row, under its description.
pub fn row_error(text: &str, wrap: f32) -> Node {
    label(text.to_string())
        .size(LabelSize::Small)
        .line_height(super::SMALL_LINE_HEIGHT)
        .color(theme().error)
        .wrap(wrap)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_host_key_but_the_token_ones_has_a_row() {
        let shown: Vec<&str> = KEY_CONTROLS.iter().map(|(key, _)| *key).collect();
        for key in settings::HOST_KEYS {
            assert!(
                shown.contains(key)
                    || ["proxy.token_env", "proxy.allow_unauthenticated"].contains(key),
                "{key} has no row"
            );
        }
    }

    #[test]
    fn a_change_names_only_the_host_keys_that_moved() {
        let before = Settings::default();
        let mut after = before.clone();
        after.proxy.bind = "0.0.0.0".into();
        after.theme = "Other".into();
        after.workspaces.max_running = 3;
        let change = host_change(&before, &after).expect("a change");
        assert_eq!(change.len(), 2);
        assert_eq!(change["proxy.bind"], "0.0.0.0");
        assert_eq!(change["workspaces.max_running"], 3);
        assert!(host_change(&before, &before).is_none());

        restore(&mut after, &before, &change);
        assert_eq!(after.proxy.bind, before.proxy.bind);
        assert_eq!(after.workspaces.max_running, 0);
        assert_eq!(after.theme, "Other", "keys outside the change stay");
    }

    #[test]
    fn the_tab_shows_only_what_the_host_lists() {
        let mut host = HostPage::new("dev", "0.9.9");
        let all = host_page(DEV_SERVICES, &host, &IntegrationsPage::default()).expect("page");
        assert!(
            all.items.iter().all(|item| match item {
                PageItem::Row(row) => matches!(row.control, Control::Placeholder),
                _ => true,
            }),
            "values wait for the host"
        );
        host.take_answer(HostAnswer {
            values: serde_json::json!({}),
            keys: vec!["dev_proxy_port".into()],
            path: String::new(),
        });
        let page = host_page(DEV_SERVICES, &host, &IntegrationsPage::default()).expect("page");
        let headers: Vec<&str> = page
            .items
            .iter()
            .filter_map(|item| match item {
                PageItem::Header(name) => Some(*name),
                _ => None,
            })
            .collect();
        assert_eq!(headers, ["Reverse Proxy"]);
        assert_eq!(host.settings_path(), HOST_SETTINGS_FALLBACK);
    }
}
