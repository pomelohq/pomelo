//! `SettingsView`: the settings screen as a reactive `RawView`. It owns all settings-window state (selection,
//! expansion, search, page scroll, inline edit, dropdown popover) that previously lived scattered in the
//! binary, composes the layered `Frame` (clipped scrolling page under fixed chrome, popover on top), and
//! handles input via `on_click`/`scroll`/`hover`/key methods. Global side-effects that reach other windows
//! (theme, UI font, text scale/weight) are applied to the shared `ui` globals here and flagged in
//! `SideEffects` for the shell to re-apply to each window's renderer and repaint.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use crate as settings_ui;
use crate::host_tab;
use pom_paths::StateDir;
use serde_json::{Map, Value};
use settings::Settings;
use settings_ui::{
    ConnectionStatus, HostAnswer, HostError, HostLink, HostLoad, HostPage, HostSettingsStore,
    IntegrationsPage, TokenSource,
};
use ui::{Context, Frame, Overlay, Painted, RawView, Rect, Window};

const JIRA_FIELDS: [u64; 3] = [
    settings_ui::CTRL_JIRA_SITE,
    settings_ui::CTRL_JIRA_EMAIL,
    settings_ui::CTRL_JIRA_TOKEN,
];

fn is_free_text(id: u64) -> bool {
    JIRA_FIELDS.contains(&id)
        || id == settings_ui::CTRL_AGENT_COMMAND
        || id == settings_ui::CTRL_TERM_SHELL
        || settings_ui::PROXY_TEXT_FIELDS.contains(&id)
}

const NUMBER_FIELDS: [u64; 10] = [
    settings_ui::CTRL_BUFFER_FONT_EDIT,
    settings_ui::CTRL_TERM_FONT_EDIT,
    settings_ui::CTRL_BUFFER_WEIGHT_EDIT,
    settings_ui::CTRL_TERM_WEIGHT_EDIT,
    settings_ui::CTRL_AGENT_FONT_EDIT,
    settings_ui::CTRL_SCROLLBACK_EDIT,
    settings_ui::CTRL_PROXY_PORT_EDIT,
    settings_ui::CTRL_WEBHOOK_PORT_EDIT,
    settings_ui::CTRL_MODULES_LIMIT_EDIT,
    settings_ui::CTRL_MODULES_DAYS_EDIT,
];

/// Cross-window work the shell must do after an input the view handled: re-apply the UI font to every window's
/// text renderer, and/or repaint the other windows because a shared global (theme/scale/weight) changed.
#[derive(Default, Clone)]
pub struct SideEffects {
    pub reapply_font: bool,
    pub redraw_others: bool,
    pub reinstall_agents: bool,
    pub test_notification: bool,
    pub start_servers: bool,
    pub open_dev_requests: bool,
    pub open_module_store: bool,
    pub play_sound: Option<String>,
    pub toggle_login_item: bool,
    pub check_updates: bool,
    pub edit_keymap: bool,
    pub edit_settings_json: bool,
    pub open_themes: bool,
    /// A theme choice changed: the app resolves and applies the theme (mode, overrides, syntax).
    pub apply_theme: bool,
    pub export_config: bool,
    pub import_config: bool,
    pub edit_project_config: bool,
    pub add_repo: bool,
    pub apply_config: bool,
    /// The repo whose alias to change, or to take out of the project.
    pub rename_repo: Option<String>,
    pub remove_repo: Option<String>,
    /// The host's pom is too old for its tab: update it there.
    pub update_host_pom: bool,
    /// The host's settings.json, to open in the remote editor.
    pub edit_host_settings: Option<String>,
}

/// A request to the host tab's machine, answered on its own thread.
enum HostAsk {
    Read,
    /// The keys changed, and the values before, to put back when the host refuses them.
    Change {
        change: Map<String, Value>,
        before: Box<Settings>,
    },
}

type HostReply = (HostAsk, Result<HostAnswer, HostError>);

/// The project of a remote window, whose Jira settings the Integrations page edits.
#[derive(Clone)]
pub struct RemoteJira {
    pub host: String,
    /// `<project>@<host>`: what this Mac keeps for it.
    pub session: String,
    pub state: StateDir,
    /// The settings on the host; `None` when its pom cannot keep them, so they are kept here under `session`.
    pub store: Option<std::sync::Arc<dyn pom_jira::SetupStore>>,
}

fn token_source(origin: Option<pom_jira::TokenOrigin>) -> TokenSource {
    match origin {
        Some(pom_jira::TokenOrigin::Secret) => TokenSource::Secret,
        Some(pom_jira::TokenOrigin::Environment) => TokenSource::Environment,
        None => TokenSource::Missing,
    }
}

/// Innermost hit id under `(x, y)` (regions pushed outer-first, inner-last), like `ui::Window::hit_at`.
fn hit_test(hits: &[(Rect, u64)], x: f32, y: f32) -> Option<u64> {
    hits.iter()
        .rev()
        .find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
        .map(|(_, id)| *id)
}

pub struct SettingsView {
    settings: Settings,
    fonts: Vec<String>,
    selected: usize,
    expanded: Vec<bool>,
    search_query: String,
    page_scroll: f32,
    /// How far the sidebar's category list is scrolled, how far it can go, and where it shows.
    nav_scroll: f32,
    nav_max: f32,
    nav_clip: Rect,
    page_total_h: f32,
    editing: Option<(u64, String)>,
    hovered: Option<u64>,
    active_section: Option<usize>,
    section_offsets: Vec<f32>,
    popover: Option<u64>,
    popover_scroll: usize,
    popover_query: String,
    popover_hover: Option<u64>,
    popover_rect: Option<Rect>,
    scroll_accum: f32,
    hits: Vec<(Rect, u64)>,
    /// The page control the keyboard is on (Tab moves it); `None` while the keys move between pages.
    key_focus: Option<u64>,
    /// The page's controls in reading order, where they were last laid out, and the page's visible area.
    page_controls: Vec<(Rect, u64)>,
    page_clip: Rect,
    /// Last laid-out viewport (logical px), cached each `build` so input methods invoked outside `render_frame`
    /// (scroll/key, reached via `entity.update`) can size popovers and clamp scroll without a `Window`.
    viewport: (f32, f32),
    pending: SideEffects,
    /// The state folder and session whose Jira settings the Integrations page edits.
    jira_session: Option<(StateDir, String)>,
    /// A remote project's Jira settings, kept on its machine or (an older pom there) here.
    jira_remote: Option<RemoteJira>,
    /// The remote machine's settings on their way back from a read or a change.
    jira_load: Option<Receiver<Result<pom_jira::JiraSetup, String>>>,
    /// The host request before this one: a change must not overtake an earlier one, or its answer drops that edit.
    jira_host_last: Option<std::thread::JoinHandle<()>>,
    pages: settings_ui::PageState,
    jira_test: Option<Receiver<Result<String, String>>>,
    /// The project's `pom.yml`, whose `sync:` applies until this window sets refresh-main.
    project_config: Option<std::sync::Arc<pom_config::Config>>,
    /// The host tab's machine, its answers, how many are still out, and the request before the latest.
    host_store: Option<Arc<dyn HostSettingsStore>>,
    host_replies: (Sender<HostReply>, Receiver<HostReply>),
    host_pending: usize,
    host_last: Option<std::thread::JoinHandle<()>>,
    /// An input on the host tab is being handled: `settings` holds the host's values meanwhile.
    in_host_scope: bool,
}

impl SettingsView {
    /// Open on the Appearance page with its sub-sections expanded (as the binary did on window open).
    pub fn new(settings: Settings) -> Self {
        let mut expanded = vec![false; settings_ui::CATEGORY_COUNT];
        if let Some(e) = expanded.get_mut(settings_ui::APPEARANCE) {
            *e = true;
        }
        Self {
            settings,
            fonts: Vec::new(),
            selected: settings_ui::APPEARANCE,
            expanded,
            search_query: String::new(),
            page_scroll: 0.0,
            nav_scroll: 0.0,
            nav_max: 0.0,
            nav_clip: Rect::new(0.0, 0.0, 0.0, 0.0, ui::Rgba::TRANSPARENT),
            page_total_h: 0.0,
            editing: None,
            hovered: None,
            active_section: None,
            section_offsets: Vec::new(),
            popover: None,
            popover_scroll: 0,
            popover_query: String::new(),
            popover_hover: None,
            popover_rect: None,
            scroll_accum: 0.0,
            hits: Vec::new(),
            key_focus: None,
            page_controls: Vec::new(),
            page_clip: Rect::new(0.0, 0.0, 0.0, 0.0, ui::Rgba::TRANSPARENT),
            viewport: (0.0, 0.0),
            pending: SideEffects::default(),
            jira_session: None,
            jira_remote: None,
            jira_load: None,
            jira_host_last: None,
            pages: settings_ui::PageState::default(),
            jira_test: None,
            project_config: None,
            host_store: None,
            host_replies: mpsc::channel(),
            host_pending: 0,
            host_last: None,
            in_host_scope: false,
        }
    }

    /// Opened from a remote window: a tab for its host beside This Mac, its settings read from there.
    pub fn set_remote_host(&mut self, host: Option<settings_ui::RemoteHost>) {
        match host {
            None => {
                self.pages.host = None;
                self.host_store = None;
            }
            Some(host) => {
                self.pages.host = Some(HostPage::new(&host.name, &host.version));
                self.host_store = Some(host.store);
                self.read_host();
            }
        }
    }

    /// The remote window's session and its host's pom; returns whether the tab changed.
    pub fn set_host_status(&mut self, link: HostLink, version: &str) -> bool {
        let Some(host) = self.pages.host.as_mut() else {
            return false;
        };
        let changed = host.link != link || host.version != version;
        host.link = link;
        host.version = version.to_string();
        changed
    }

    fn host_shown(&self) -> bool {
        self.pages.host.as_ref().is_some_and(|host| host.shown)
    }

    /// The settings the shown tab edits.
    fn active_settings(&self) -> &Settings {
        match self.pages.host.as_ref() {
            Some(host) if host.shown && !self.in_host_scope => &host.settings,
            _ => &self.settings,
        }
    }

    fn read_host(&mut self) {
        if let Some(host) = self.pages.host.as_mut() {
            host.load = HostLoad::Reading;
        }
        self.ask_host_settings(HostAsk::Read, |store| store.read());
    }

    fn ask_host_settings(
        &mut self,
        ask: HostAsk,
        job: impl FnOnce(&dyn HostSettingsStore) -> Result<HostAnswer, HostError> + Send + 'static,
    ) {
        let Some(store) = self.host_store.clone() else {
            return;
        };
        let sender = self.host_replies.0.clone();
        let previous = self.host_last.take();
        self.host_pending += 1;
        self.host_last = Some(std::thread::spawn(move || {
            if let Some(previous) = previous {
                if previous.join().is_err() {
                    eprintln!("settings: an earlier request to the host failed");
                }
            }
            if sender.send((ask, job(&*store))).is_err() {
                eprintln!("settings: the host answered after the window closed");
            }
        }));
    }

    /// Runs an input on the host tab against the host's values, then sends the host what it changed.
    fn in_host<R>(&mut self, act: impl FnOnce(&mut Self) -> R) -> R {
        if self.in_host_scope || !self.host_shown() {
            return act(self);
        }
        let Some(host) = self.pages.host.as_mut() else {
            return act(self);
        };
        std::mem::swap(&mut self.settings, &mut host.settings);
        let before = self.settings.clone();
        self.in_host_scope = true;
        let result = act(self);
        self.in_host_scope = false;
        let after = self.settings.clone();
        if let Some(host) = self.pages.host.as_mut() {
            std::mem::swap(&mut self.settings, &mut host.settings);
        }
        if let Some(change) = host_tab::host_change(&before, &after) {
            if let Some(host) = self.pages.host.as_mut() {
                host.errors.retain(|(key, _)| !change.contains_key(key));
            }
            let values = Value::Object(change.clone());
            let before = Box::new(before);
            self.ask_host_settings(HostAsk::Change { change, before }, move |store| {
                store.change(values)
            });
        }
        result
    }

    /// A value typed on the host tab that this app already knows the host would not take.
    fn refuse_on_host(&mut self, control: u64, message: String) {
        let Some(key) = host_tab::key_of_control(control) else {
            return;
        };
        if let Some(host) = self.pages.host.as_mut() {
            host.errors.retain(|(errored, _)| errored != key);
            host.errors.push((key.to_string(), message));
        }
    }

    /// Takes the host's answers; returns whether the tab changed.
    fn tick_host_settings(&mut self) -> bool {
        let mut changed = false;
        while let Ok((ask, answer)) = self.host_replies.1.try_recv() {
            self.host_pending = self.host_pending.saturating_sub(1);
            changed = true;
            let Some(host) = self.pages.host.as_mut() else {
                continue;
            };
            match (ask, answer) {
                (HostAsk::Read, Ok(answer)) => {
                    host.errors.clear();
                    host.take_answer(answer);
                }
                (HostAsk::Change { .. }, Ok(answer)) => {
                    host.take_answer(answer);
                }
                (HostAsk::Read, Err(error)) => {
                    host.load = match error {
                        HostError::Unreachable(message) => HostLoad::Unreachable(message),
                        HostError::Outdated(message) => HostLoad::Outdated(message),
                        HostError::Refused(message) | HostError::Failed(message) => {
                            HostLoad::Failed(message)
                        }
                    }
                }
                (_, Err(HostError::Outdated(message))) => host.load = HostLoad::Outdated(message),
                (HostAsk::Change { change, before }, Err(error)) => {
                    host_tab::restore(&mut host.settings, &before, &change);
                    let message = error.to_string();
                    for key in change.keys() {
                        let said = message
                            .strip_prefix(&format!("{key}: "))
                            .unwrap_or(&message);
                        host.errors.retain(|(errored, _)| errored != key);
                        host.errors.push((key.clone(), said.to_string()));
                    }
                }
            }
        }
        changed
    }

    /// The settings file was edited by hand; the pages show the new values.
    pub fn replace_settings(&mut self, settings: Settings) {
        self.editing = None;
        self.popover = None;
        self.settings = settings;
    }

    /// The app changed the theme (from a key binding); the page shows it and later saves keep it.
    pub fn set_theme(&mut self, theme: &str, system_dark: bool) {
        self.settings
            .set_active_theme(theme.to_string(), system_dark);
    }

    pub fn set_agent_page(&mut self, agent: settings_ui::AgentPage) -> bool {
        let changed = self.pages.agent != agent;
        self.pages.agent = agent;
        changed
    }

    /// Shows a page of This Mac (by its index in the navbar), as a key binding asks.
    pub fn select_category(&mut self, category: usize) {
        if category < settings_ui::CATEGORY_COUNT {
            self.commit_edit();
            self.close_popover();
            if let Some(host) = self.pages.host.as_mut() {
                host.shown = false;
            }
            self.selected = category;
            self.pages.language = None;
            self.page_scroll = 0.0;
            self.active_section = None;
        }
    }

    pub fn set_general_page(&mut self, general: settings_ui::GeneralPage) -> bool {
        let changed = self.pages.general != general;
        self.pages.general = general;
        changed
    }

    pub fn set_keymap_page(&mut self, keymap: settings_ui::KeymapPage) -> bool {
        let changed = self.pages.keymap != keymap;
        self.pages.keymap = keymap;
        changed
    }

    pub fn set_project_page(&mut self, project: settings_ui::ProjectPage) -> bool {
        let changed = self.pages.project != project;
        self.pages.project = project;
        changed
    }

    fn repo_for(&self, id: u64, base: u64) -> Option<String> {
        let index = id
            .checked_sub(base)
            .filter(|index| *index < settings_ui::CTRL_REPO_LIMIT)?;
        self.pages
            .project
            .repos
            .get(index as usize)
            .map(|repo| repo.name.clone())
    }

    pub fn set_dev_services_page(&mut self, services: settings_ui::DevServicesPage) -> bool {
        let changed = self.pages.dev_services != services;
        self.pages.dev_services = services;
        changed
    }

    fn persist(&mut self) {
        if self.in_host_scope {
            return;
        }
        if let Err(error) = self.settings.save() {
            eprintln!("settings: save: {error}");
        }
    }

    pub fn set_project_config(&mut self, config: Option<std::sync::Arc<pom_config::Config>>) {
        self.project_config = config;
        let status = self.pages.jira.status.clone();
        self.reload_jira(status);
    }

    /// Saves refresh-main's schedule for the session (the background loop picks it up within seconds).
    fn save_refresh(&mut self, enabled: bool, minutes: u64) {
        let Some((state, session)) = self.jira_session.clone() else {
            return;
        };
        let minutes = minutes.clamp(
            settings_ui::REFRESH_MINUTES_MIN,
            settings_ui::REFRESH_MINUTES_MAX,
        );
        let schedule = pom_sync::RefreshSchedule {
            enabled,
            interval_seconds: minutes * 60,
        };
        if let Err(error) = pom_sync::save_refresh_schedule(&state, &session, schedule) {
            eprintln!("settings: save refresh-main: {error}");
        }
        let status = self.pages.jira.status.clone();
        self.reload_jira(status);
    }

    /// Keeps a value the app changed (the ticket picker's board) so this window's next save does not undo it.
    pub fn remember_jira_board(&mut self, board: i64) {
        self.settings.jira_board = board;
    }

    pub fn remember_onboard_with_ai(&mut self, use_ai: bool) {
        self.settings.onboard_with_ai = use_ai;
    }

    /// The project session the Integrations page edits (none while no project is open).
    pub fn set_jira_session(&mut self, session: Option<(StateDir, String)>) {
        if self.jira_session.as_ref().map(|(_, name)| name)
            == session.as_ref().map(|(_, name)| name)
        {
            return;
        }
        self.jira_session = session;
        self.jira_remote = None;
        self.jira_load = None;
        self.jira_test = None;
        self.reload_jira(ConnectionStatus::Untested);
    }

    /// A remote window's project: the Integrations page edits its settings where they are kept.
    pub fn set_jira_remote(&mut self, remote: Option<RemoteJira>) {
        self.jira_test = None;
        self.jira_load = None;
        self.jira_session = remote
            .as_ref()
            .filter(|remote| remote.store.is_none())
            .map(|remote| (remote.state.clone(), remote.session.clone()));
        self.jira_remote = remote;
        self.reload_jira(ConnectionStatus::Untested);
    }

    fn host_store(&self) -> Option<std::sync::Arc<dyn pom_jira::SetupStore>> {
        self.jira_remote.as_ref()?.store.clone()
    }

    /// Where a remote project's settings are kept, and why there.
    fn kept_on(&self) -> Option<(String, String)> {
        let remote = self.jira_remote.as_ref()?;
        Some(match remote.store {
            Some(_) => (
                remote.host.clone(),
                format!("Jira is read on {} with these settings; the token never leaves it.", remote.host),
            ),
            None => (
                "This Mac".into(),
                format!("The pom on {} cannot keep Jira settings yet; update it there to keep them on {}.", remote.host, remote.host),
            ),
        })
    }

    /// Runs `ask` on the host off this thread; its answer fills the page on a later tick.
    fn ask_host(
        &mut self,
        ask: impl FnOnce(&dyn pom_jira::SetupStore) -> Result<pom_jira::JiraSetup, String>
            + Send
            + 'static,
    ) {
        let Some(store) = self.host_store() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let previous = self.jira_host_last.take();
        self.jira_host_last = Some(std::thread::spawn(move || {
            if let Some(previous) = previous {
                if previous.join().is_err() {
                    eprintln!("settings: an earlier Jira request to the host failed");
                }
            }
            if sender.send(ask(&*store)).is_err() {
                eprintln!("settings: a Jira answer arrived after the page moved on");
            }
        }));
        self.jira_load = Some(receiver);
    }

    fn reload_jira(&mut self, status: ConnectionStatus) {
        if let Some(remote) = self
            .jira_remote
            .clone()
            .filter(|remote| remote.store.is_some())
        {
            let previous = std::mem::take(&mut self.pages.jira);
            self.pages.jira = IntegrationsPage {
                session: remote.session,
                kept_on: self.kept_on(),
                status,
                ..previous
            };
            self.ask_host(|store| store.read());
            return;
        }
        let Some((state, session)) = &self.jira_session else {
            self.pages.jira = IntegrationsPage::default();
            return;
        };
        let stored = pom_jira::JiraSettings::load(state, session);
        let token = match pom_jira::resolve_token(state, session, &stored, &|name| {
            std::env::var(name).ok()
        }) {
            Some((_, pom_jira::TokenOrigin::Secret)) => TokenSource::Secret,
            Some((_, pom_jira::TokenOrigin::Environment)) => TokenSource::Environment,
            None => TokenSource::Missing,
        };
        let refresh = pom_sync::refresh_schedule(state, session, self.project_config.as_deref());
        self.pages.jira = IntegrationsPage {
            session: session.clone(),
            kept_on: self.kept_on(),
            keep_main_fresh: refresh.enabled,
            refresh_minutes: (refresh.interval_seconds / 60).max(settings_ui::REFRESH_MINUTES_MIN),
            site: stored.site,
            email: stored.email,
            token,
            status,
        };
    }

    /// Saves a typed Jira field: site and email into the session's integrations file, the token into its secrets.
    fn commit_jira(&mut self, id: u64, text: &str) {
        let text = text.trim();
        if self.host_store().is_some() {
            let change = match id {
                settings_ui::CTRL_JIRA_TOKEN if text.is_empty() => return,
                settings_ui::CTRL_JIRA_TOKEN => pom_jira::SetupChange {
                    token: Some(text.to_string()),
                    ..pom_jira::SetupChange::default()
                },
                settings_ui::CTRL_JIRA_SITE => pom_jira::SetupChange {
                    site: Some(text.to_string()),
                    ..pom_jira::SetupChange::default()
                },
                _ => pom_jira::SetupChange {
                    email: Some(text.to_string()),
                    ..pom_jira::SetupChange::default()
                },
            };
            self.pages.jira.status = ConnectionStatus::Untested;
            self.ask_host(move |store| store.change(&change));
            return;
        }
        let Some((state, session)) = self.jira_session.clone() else {
            return;
        };
        let saved = if id == settings_ui::CTRL_JIRA_TOKEN {
            if text.is_empty() {
                return;
            }
            pom_jira::save_token(&state, &session, text)
        } else {
            let mut stored = pom_jira::JiraSettings::load(&state, &session);
            if id == settings_ui::CTRL_JIRA_SITE {
                stored.site = text.to_string();
            } else {
                stored.email = text.to_string();
            }
            stored
                .save(&state, &session)
                .map_err(|error| error.to_string())
        };
        match saved {
            Ok(()) => self.reload_jira(ConnectionStatus::Untested),
            Err(error) => self.reload_jira(ConnectionStatus::Failed(error)),
        }
    }

    fn test_jira(&mut self) {
        if let Some(store) = self.host_store() {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                if sender.send(store.test()).is_err() {
                    eprintln!("settings: the Jira check finished after the window closed");
                }
            });
            self.jira_test = Some(receiver);
            self.pages.jira.status = ConnectionStatus::Testing;
            return;
        }
        let Some((state, session)) = self.jira_session.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let answer = match pom_jira::resolve(&state, &session) {
                Some(client) => client
                    .myself()
                    .map(|(name, email)| format!("{name} ({email})"))
                    .map_err(|error| error.to_string()),
                None => Err("set the site, email and token first".to_string()),
            };
            if sender.send(answer).is_err() {
                eprintln!("settings: the Jira check finished after the window closed");
            }
        });
        self.jira_test = Some(receiver);
        self.pages.jira.status = ConnectionStatus::Testing;
    }

    /// Picks up a finished connection check; returns whether the page changed.
    pub fn tick(&mut self) -> bool {
        let loaded = self.tick_host() | self.tick_host_settings();
        let Some(receiver) = &self.jira_test else {
            return loaded;
        };
        let status = match receiver.try_recv() {
            Ok(Ok(who)) => ConnectionStatus::SignedIn(who),
            Ok(Err(error)) => ConnectionStatus::Failed(error),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => ConnectionStatus::Failed("the check stopped".into()),
        };
        self.jira_test = None;
        self.pages.jira.status = status;
        true
    }

    /// Picks up the host's settings after a read or a change; returns whether the page changed.
    fn tick_host(&mut self) -> bool {
        let Some(receiver) = &self.jira_load else {
            return false;
        };
        let answer = match receiver.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("the host did not answer".into()),
        };
        self.jira_load = None;
        match answer {
            Ok(setup) => {
                self.pages.jira.site = setup.site;
                self.pages.jira.email = setup.email;
                self.pages.jira.token = token_source(setup.token);
            }
            Err(error) => self.pages.jira.status = ConnectionStatus::Failed(error),
        }
        true
    }

    pub fn busy(&self) -> bool {
        self.jira_test.is_some() || self.jira_load.is_some() || self.host_pending > 0
    }

    /// Pastes into the field being edited (one line; newlines dropped).
    pub fn key_paste(&mut self, text: &str) -> bool {
        let line: String = text.chars().filter(|c| !c.is_control()).collect();
        match self.editing.as_mut() {
            Some((id, buf)) if is_free_text(*id) && !line.is_empty() => {
                buf.push_str(&line);
                true
            }
            _ => false,
        }
    }

    /// The shell injects the system font list (from the renderer) once; the font-family dropdown needs it.
    pub fn set_fonts(&mut self, fonts: Vec<String>) {
        self.fonts = fonts;
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Takes the buttons and dock places a window changed (hidden or moved from a button's menu), so this
    /// view's copy never puts them back.
    pub fn sync_placement(&mut self, from: &Settings) {
        self.settings.agent_hidden = from.agent_hidden;
        self.settings.terminal_hidden = from.terminal_hidden;
        self.settings.func_hidden = from.func_hidden.clone();
        self.settings.sidebar_side = from.sidebar_side.clone();
        self.settings.agent_side = from.agent_side.clone();
        self.settings.terminal_side = from.terminal_side.clone();
        self.settings.func_sides = from.func_sides.clone();
    }

    /// The shell drains this after routing an input to learn what other windows need.
    pub fn take_side_effects(&mut self) -> SideEffects {
        std::mem::take(&mut self.pending)
    }

    fn search_active(&self) -> bool {
        self.popover.is_none() && self.editing.is_none()
    }

    fn content_clip_h(&self) -> f32 {
        let (_, _, _, clip_h) = settings_ui::content_region(self.viewport.0, self.viewport.1);
        clip_h
    }

    /// Commit an in-progress numeric edit (parse, clamp, persist, re-apply the global). No-op if idle.
    fn commit_edit(&mut self) {
        self.in_host(Self::commit_edit_here)
    }

    fn commit_edit_here(&mut self) {
        let Some((id, buf)) = self.editing.take() else {
            return;
        };
        if JIRA_FIELDS.contains(&id) {
            self.commit_jira(id, &buf);
            return;
        }
        if settings_ui::PROXY_TEXT_FIELDS.contains(&id) {
            match settings_ui::set_proxy_text(id, &buf, &mut self.settings) {
                Ok(true) => self.persist(),
                Ok(false) => {}
                Err(error) if self.in_host_scope => self.refuse_on_host(id, error),
                Err(error) => eprintln!("settings: {error}"),
            }
            return;
        }
        if host_tab::is_host_number_edit(id) {
            match buf.trim().parse::<u64>() {
                Ok(typed) => {
                    if let Some((value, _, max)) = host_tab::host_number(id, &mut self.settings) {
                        *value = typed.min(max);
                    }
                }
                Err(_) => self.refuse_on_host(id, format!("{} is not a whole number", buf.trim())),
            }
            return;
        }
        if id == settings_ui::CTRL_TERM_SHELL {
            let shell = buf.trim().to_string();
            if shell != self.settings.terminal_shell {
                self.settings.terminal_shell = shell;
                self.persist();
            }
            return;
        }
        if settings_ui::language_number(id, &self.settings).is_some() {
            if let Ok(value) = buf.trim().parse::<f32>() {
                if settings_ui::set_language_number(id, value, &mut self.settings) {
                    self.persist();
                }
            }
            return;
        }
        if NUMBER_FIELDS.contains(&id) {
            let Ok(value) = buf.trim().parse::<f32>() else {
                return;
            };
            let changed = match id {
                settings_ui::CTRL_BUFFER_FONT_EDIT => {
                    let next = value.clamp(settings_ui::FONT_SIZE_MIN, settings_ui::FONT_SIZE_MAX);
                    std::mem::replace(&mut self.settings.buffer_font_size, next) != next
                }
                settings_ui::CTRL_TERM_FONT_EDIT => {
                    let next = value.clamp(settings_ui::FONT_SIZE_MIN, settings_ui::FONT_SIZE_MAX);
                    std::mem::replace(&mut self.settings.terminal_font_size, next) != next
                }
                settings_ui::CTRL_AGENT_FONT_EDIT => {
                    let next = value.clamp(settings_ui::FONT_SIZE_MIN, settings_ui::FONT_SIZE_MAX);
                    std::mem::replace(&mut self.settings.agent_font_size, next) != next
                }
                settings_ui::CTRL_BUFFER_WEIGHT_EDIT => {
                    let next =
                        value.clamp(settings_ui::FONT_WEIGHT_MIN, settings_ui::FONT_WEIGHT_MAX);
                    std::mem::replace(&mut self.settings.buffer_font_weight, next) != next
                }
                settings_ui::CTRL_TERM_WEIGHT_EDIT => {
                    let next =
                        value.clamp(settings_ui::FONT_WEIGHT_MIN, settings_ui::FONT_WEIGHT_MAX);
                    std::mem::replace(&mut self.settings.terminal_font_weight, next) != next
                }
                settings_ui::CTRL_MODULES_LIMIT_EDIT => {
                    let next = (value.max(1.0) as u64).min(settings_ui::MODULES_LIMIT_MAX_GB);
                    std::mem::replace(&mut self.settings.modules_size_limit_gb, next) != next
                }
                settings_ui::CTRL_MODULES_DAYS_EDIT => {
                    let next = (value.max(0.0) as u64).min(settings_ui::MODULES_DAYS_MAX);
                    std::mem::replace(&mut self.settings.modules_unused_days, next) != next
                }
                settings_ui::CTRL_PROXY_PORT_EDIT | settings_ui::CTRL_WEBHOOK_PORT_EDIT => {
                    let next =
                        value.clamp(f32::from(settings_ui::PORT_MIN), f32::from(u16::MAX)) as u16;
                    let port = if id == settings_ui::CTRL_PROXY_PORT_EDIT {
                        &mut self.settings.dev_proxy_port
                    } else {
                        &mut self.settings.webhook_port
                    };
                    std::mem::replace(port, next) != next
                }
                _ => {
                    let next = (value as u32)
                        .clamp(settings_ui::SCROLLBACK_MIN, settings_ui::SCROLLBACK_MAX);
                    std::mem::replace(&mut self.settings.terminal_scrollback, next) != next
                }
            };
            if changed {
                self.persist();
            }
            return;
        }
        if id == settings_ui::CTRL_AGENT_COMMAND {
            let command = buf.trim();
            let command = if command.is_empty() {
                Settings::default().agent_command
            } else {
                command.to_string()
            };
            if command != self.settings.agent_command {
                self.settings.agent_command = command;
                self.persist();
            }
            return;
        }
        if id == settings_ui::CTRL_REFRESH_EDIT {
            if let Ok(minutes) = buf.trim().parse::<u64>() {
                self.save_refresh(self.pages.jira.keep_main_fresh, minutes);
            }
            return;
        }
        let Ok(v) = buf.trim().parse::<f32>() else {
            return;
        };
        let changed = if id == settings_ui::CTRL_FONT_WEIGHT_EDIT {
            let c = settings_ui::set_font_weight(&mut self.settings, v);
            if c {
                ui::set_ui_font_weight(self.settings.ui_font_weight as u16);
            }
            c
        } else {
            let c = settings_ui::set_font_size(&mut self.settings, v);
            if c {
                ui::set_ui_text_scale(self.settings.ui_font_size / ui::UI_FONT_BASE);
            }
            c
        };
        if changed {
            self.persist();
            self.pending.redraw_others = true;
        }
    }

    /// Rebuild page/chrome/popover into a layered `Frame` and refresh derived state (section offsets, active
    /// section, hit list). Ported from the binary's `draw_settings`.
    fn build(&mut self, window: &Window) -> Frame {
        let (w, h) = (window.width, window.height);
        self.viewport = (w, h);
        let (clip_x, clip_y, clip_w, clip_h) = settings_ui::content_region(w, h);

        let max_scroll = (self.page_total_h - clip_h).max(0.0);
        self.page_scroll = self.page_scroll.clamp(0.0, max_scroll);
        let (page, total_h) = settings_ui::page(
            self.selected,
            &self.settings,
            &self.pages,
            self.editing.as_ref().map(|(id, buf)| (*id, buf.as_str())),
            &self.search_query,
            w,
            h,
            self.page_scroll,
        );
        self.page_total_h = total_h;
        self.page_clip = Rect::new(clip_x, clip_y, clip_w, clip_h, ui::Rgba::TRANSPARENT);
        self.page_controls = Vec::new();
        for (rect, id) in page.hits.iter().copied() {
            let known = self.page_controls.iter().any(|(_, seen)| *seen == id);
            if settings_ui::is_keyboard_control(id) && !known {
                self.page_controls.push((rect, id));
            }
        }
        if self
            .key_focus
            .is_some_and(|id| !self.page_controls.iter().any(|(_, each)| *each == id))
        {
            self.key_focus = None;
        }

        let mut offsets: Vec<(u64, f32)> = page
            .hits
            .iter()
            .filter(|(_, id)| *id >= settings_ui::SECTION_ANCHOR_BASE)
            .map(|(r, id)| (*id, r.y - clip_y + self.page_scroll))
            .collect();
        offsets.sort_by_key(|(id, _)| *id);
        self.section_offsets = offsets.into_iter().map(|(_, y)| y).collect();

        if self.section_offsets.is_empty() {
            self.active_section = None;
        } else if self.active_section.is_none() {
            self.active_section = Some(0);
        }

        let chrome = settings_ui::chrome(
            self.selected,
            self.hovered,
            self.active_section,
            &self.expanded,
            w,
            h,
            &self.search_query,
            self.search_active(),
            self.nav_scroll,
            &self.pages.project,
            self.pages.language,
            self.pages.host.as_ref(),
        );
        self.nav_clip = chrome.nav_clip;
        self.nav_max = (chrome.nav_height - chrome.nav_clip.h).max(0.0);
        if self.nav_scroll > self.nav_max {
            self.nav_scroll = self.nav_max;
        }
        let nav_clip = chrome.nav_clip;
        let mut nav = chrome.nav;
        if let Some(bar) = settings_ui::content_scrollbar(
            (nav_clip.x, nav_clip.y, nav_clip.w, nav_clip.h),
            chrome.nav_height,
            self.nav_scroll,
        ) {
            nav.rects.push(bar);
        }
        let chrome = chrome.fixed;

        let popover = self.popover.and_then(|cid| {
            let anchor = page
                .hits
                .iter()
                .chain(chrome.hits.iter())
                .find(|(_, id)| *id == cid)
                .map(|(r, _)| *r)?;
            let items = settings_ui::control_items(cid, &self.fonts);
            let current = settings_ui::control_value(cid, self.active_settings());
            Some(settings_ui::popover(
                anchor,
                &current,
                &items,
                self.popover_scroll,
                &self.popover_query,
                self.popover_hover,
                w,
                h,
            ))
        });

        let mut overlay = chrome;
        if let Some(ring) = self.key_focus.and_then(|id| {
            self.page_controls
                .iter()
                .find(|(_, each)| *each == id)
                .map(|(rect, _)| *rect)
        }) {
            let focus: ui::Node = ui::div()
                .w_px(ring.w)
                .h_px(ring.h)
                .rounded(4.0)
                .border(1.0, ui::theme().panel_focused_border)
                .into();
            overlay.rects.extend(ui::render(&focus, ring).rects);
        }
        if let Some(bar) = settings_ui::content_scrollbar(
            (clip_x, clip_y, clip_w, clip_h),
            self.page_total_h,
            self.page_scroll,
        ) {
            overlay.rects.push(bar);
        }
        self.popover_rect = None;
        if let Some(pop) = &popover {
            self.popover_rect = pop.rects.iter().find(|r| r.color.a > 0.5).copied();
        }

        // Hit order (topmost last): visible page rows, fixed chrome, popover.
        let mut hits: Vec<(Rect, u64)> = page
            .hits
            .iter()
            .filter(|(r, _)| r.y + r.h > clip_y && r.y < clip_y + clip_h)
            .copied()
            .collect();
        hits.extend(overlay.hits.iter().copied());
        hits.extend(
            nav.hits
                .iter()
                .copied()
                .filter(|(r, _)| r.y + r.h > nav_clip.y && r.y < nav_clip.y + nav_clip.h),
        );
        if let Some(pop) = &popover {
            hits.extend(pop.hits.iter().copied());
        }
        self.hits = hits.clone();

        // Layers: clipped page (base is empty; the clear color fills), fixed chrome, popover on top.
        let mut overlays = vec![
            Overlay {
                painted: page,
                clip: Some(Rect::new(
                    clip_x,
                    clip_y,
                    clip_w,
                    clip_h,
                    ui::Rgba::TRANSPARENT,
                )),
            },
            Overlay {
                painted: overlay,
                clip: None,
            },
            Overlay {
                painted: nav,
                clip: Some(nav_clip),
            },
        ];
        if let Some(pop) = popover {
            overlays.push(Overlay {
                painted: pop,
                clip: None,
            });
        }
        Frame {
            base: Painted::default(),
            overlays,
            hits,
        }
    }

    fn close_popover(&mut self) {
        self.popover = None;
        self.popover_hover = None;
        self.popover_query.clear();
    }

    /// Open a dropdown for control `cid`, scrolled so the current value is in view (ported from the binary).
    fn open_popover(&mut self, cid: u64) {
        self.popover = Some(cid);
        self.scroll_accum = 0.0;
        self.popover_query.clear();
        let items = settings_ui::control_items(cid, &self.fonts);
        let current = settings_ui::control_value(cid, &self.settings);
        let sel = items.iter().position(|it| *it == current).unwrap_or(0);
        self.popover_hover = Some(settings_ui::POPOVER_BASE + sel as u64);
        let anchor = self
            .hits
            .iter()
            .find(|(_, hid)| *hid == cid)
            .map(|(r, _)| *r);
        let max_scroll = anchor
            .map(|a| settings_ui::popover_max_scroll_for(items.len(), a, self.viewport.1))
            .unwrap_or(0);
        self.popover_scroll = sel.saturating_sub(2).min(max_scroll);
    }

    /// Update hover from a cursor move; returns true if a repaint is warranted. Mirrors the binary's cursor
    /// handling plus the sticky popover-hover recompute that `draw_settings` did each frame.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let hit = hit_test(&self.hits, x, y);
        if self.popover.is_some() {
            if let Some(id) = hit.filter(|id| *id >= settings_ui::POPOVER_BASE) {
                if Some(id) != self.popover_hover {
                    self.popover_hover = Some(id);
                    return true;
                }
            }
            false
        } else if hit != self.hovered {
            self.hovered = hit;
            true
        } else {
            false
        }
    }

    /// Whether the cursor at `(x, y)` is over the open popover panel (so the wheel scrolls the list, not page).
    fn over_popover(&self, x: f32, y: f32) -> bool {
        self.popover_rect
            .is_some_and(|r| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
    }

    /// Scroll the page or the open popover by a wheel delta (logical px). `(x, y)` is the cursor. Returns true
    /// if something moved (repaint needed).
    pub fn scroll(&mut self, dy: f32, x: f32, y: f32) -> bool {
        let clip = self.nav_clip;
        if x < clip.x + clip.w && y >= clip.y && self.popover.is_none() {
            let next = (self.nav_scroll - dy).clamp(0.0, self.nav_max);
            if (next - self.nav_scroll).abs() <= 0.01 {
                return false;
            }
            self.nav_scroll = next;
            return true;
        }
        if self.popover.is_none() || !self.over_popover(x, y) {
            let clip_h = self.content_clip_h();
            let max = (self.page_total_h - clip_h).max(0.0);
            let next = (self.page_scroll - dy).clamp(0.0, max);
            if (next - self.page_scroll).abs() <= 0.01 {
                return false;
            }
            self.page_scroll = next;
            if !self.section_offsets.is_empty() {
                let active = self
                    .section_offsets
                    .iter()
                    .enumerate()
                    .filter(|(_, off)| **off <= next + 4.0)
                    .map(|(i, _)| i)
                    .next_back()
                    .unwrap_or(0);
                self.active_section = Some(active);
            }
            return true;
        }
        let Some(cid) = self.popover else {
            return false;
        };
        let items = settings_ui::control_items(cid, &self.fonts);
        let shown = settings_ui::filter_indices(&items, &self.popover_query).len();
        let Some(anchor) = self.hits.iter().find(|(_, id)| *id == cid).map(|(r, _)| *r) else {
            return false;
        };
        let max_scroll = settings_ui::popover_max_scroll_for(shown, anchor, self.viewport.1);
        if max_scroll == 0 {
            return false;
        }
        self.scroll_accum -= dy / 30.0;
        let steps = self.scroll_accum.trunc();
        if steps == 0.0 {
            return false;
        }
        self.scroll_accum -= steps;
        let next = (self.popover_scroll as i64 + steps as i64).clamp(0, max_scroll as i64) as usize;
        if next == self.popover_scroll {
            return false;
        }
        self.popover_scroll = next;
        true
    }

    /// Type into whichever field is focused (popover filter, numeric edit, or the sidebar search). Returns true
    /// if the buffer changed.
    pub fn key_text(&mut self, text: &str) -> bool {
        if text == " " && self.popover.is_none() && self.editing.is_none() {
            if let Some(id) = self.key_focus {
                self.click(id);
                return true;
            }
        }
        if self.popover.is_some() {
            let printable: String = text.chars().filter(|c| !c.is_control()).collect();
            if printable.is_empty() {
                return false;
            }
            self.popover_query.push_str(&printable);
            self.popover_scroll = 0;
            self.scroll_accum = 0.0;
            true
        } else if let Some((id, buf)) = self.editing.as_mut() {
            let free_text = is_free_text(*id);
            let add: String = text
                .chars()
                .filter(|c| {
                    if free_text {
                        !c.is_control()
                    } else {
                        c.is_ascii_digit() || *c == '.'
                    }
                })
                .collect();
            if add.is_empty() {
                return false;
            }
            buf.push_str(&add);
            true
        } else {
            let printable: String = text.chars().filter(|c| !c.is_control()).collect();
            if printable.is_empty() {
                return false;
            }
            self.search_query.push_str(&printable);
            true
        }
    }

    /// Backspace in the focused field. Returns true if something changed.
    pub fn key_backspace(&mut self) -> bool {
        if self.popover.is_some() {
            let changed = self.popover_query.pop().is_some();
            if changed {
                self.popover_scroll = 0;
                self.scroll_accum = 0.0;
            }
            changed
        } else if let Some((_, buf)) = self.editing.as_mut() {
            buf.pop();
            true
        } else {
            self.search_query.pop().is_some()
        }
    }

    /// Escape: close the popover, cancel an edit, or clear the search (in that priority). Returns true if it
    /// did anything.
    pub fn key_escape(&mut self) -> bool {
        if self.popover.is_none() && self.editing.is_none() && self.key_focus.take().is_some() {
            return true;
        }
        if self.popover.is_some() {
            self.close_popover();
            true
        } else if self.editing.take().is_some() {
            true
        } else if !self.search_query.is_empty() {
            self.search_query.clear();
            true
        } else {
            false
        }
    }

    /// Enter: commit an in-progress numeric edit, pick the highlighted dropdown item, or press the control the
    /// keyboard is on.
    pub fn key_enter(&mut self) -> bool {
        if self.editing.is_some() {
            self.commit_edit();
            true
        } else if let Some(item) = self.popover.and(self.popover_hover) {
            self.click(item);
            true
        } else if let Some(id) = self.key_focus {
            self.click(id);
            true
        } else {
            false
        }
    }

    /// Up or down: through a dropdown's items, the page's controls, or else the pages themselves.
    pub fn key_vertical(&mut self, down: bool) -> bool {
        if self.editing.is_some() {
            return false;
        }
        if let Some(cid) = self.popover {
            return self.popover_step(cid, down);
        }
        if self.key_focus.is_some() {
            return self.key_tab(!down);
        }
        let pages = self.pages_shown();
        let at = pages
            .iter()
            .position(|page| *page == self.selected)
            .unwrap_or(0);
        let next = if down {
            (at + 1).min(pages.len().saturating_sub(1))
        } else {
            at.saturating_sub(1)
        };
        let Some(next) = pages
            .get(next)
            .copied()
            .filter(|next| *next != self.selected)
        else {
            return false;
        };
        self.click(next as u64);
        true
    }

    /// The navbar's pages of the shown tab, in order.
    fn pages_shown(&self) -> Vec<usize> {
        if self.host_shown() {
            settings_ui::HOST_CATEGORIES.to_vec()
        } else {
            (0..settings_ui::CATEGORY_COUNT).collect()
        }
    }

    /// Home or End: the first or last page (or dropdown item).
    pub fn key_home_end(&mut self, end: bool) -> bool {
        if self.editing.is_some() || self.key_focus.is_some() {
            return false;
        }
        if let Some(cid) = self.popover {
            let items = settings_ui::control_items(cid, &self.fonts);
            let matches = settings_ui::filter_indices(&items, &self.popover_query);
            let Some(index) = (if end { matches.last() } else { matches.first() }).copied() else {
                return false;
            };
            self.popover_hover = Some(settings_ui::POPOVER_BASE + index as u64);
            self.popover_scroll = if end {
                settings_ui::popover_max_scroll_for(matches.len(), self.page_clip, self.viewport.1)
            } else {
                0
            };
            return true;
        }
        let pages = self.pages_shown();
        let Some(page) = (if end { pages.last() } else { pages.first() }).copied() else {
            return false;
        };
        self.click(page as u64);
        true
    }

    /// Tab: into the page's controls and on to the next (shift: the previous); off either end, back to the pages.
    pub fn key_tab(&mut self, backward: bool) -> bool {
        if self.editing.is_some() || self.popover.is_some() {
            return false;
        }
        let at = self
            .key_focus
            .and_then(|id| self.page_controls.iter().position(|(_, each)| *each == id));
        let next = match (at, backward) {
            (None, false) => Some(0),
            (None, true) => self.page_controls.len().checked_sub(1),
            (Some(at), false) => Some(at + 1).filter(|next| *next < self.page_controls.len()),
            (Some(at), true) => at.checked_sub(1),
        };
        self.key_focus = next
            .and_then(|index| self.page_controls.get(index))
            .map(|(_, id)| *id);
        self.scroll_to_key_focus();
        true
    }

    /// Left: back to moving between pages; right: into the page.
    pub fn key_horizontal(&mut self, right: bool) -> bool {
        if self.editing.is_some() || self.popover.is_some() {
            return false;
        }
        match (right, self.key_focus) {
            (true, None) => self.key_tab(false),
            (false, Some(_)) => {
                self.key_focus = None;
                true
            }
            _ => false,
        }
    }

    fn popover_step(&mut self, cid: u64, down: bool) -> bool {
        let items = settings_ui::control_items(cid, &self.fonts);
        let matches = settings_ui::filter_indices(&items, &self.popover_query);
        if matches.is_empty() {
            return false;
        }
        let at = self.popover_hover.and_then(|id| {
            matches
                .iter()
                .position(|index| settings_ui::POPOVER_BASE + *index as u64 == id)
        });
        let next = match (at, down) {
            (None, _) => 0,
            (Some(at), true) => (at + 1).min(matches.len() - 1),
            (Some(at), false) => at.saturating_sub(1),
        };
        let Some(index) = matches.get(next) else {
            return false;
        };
        self.popover_hover = Some(settings_ui::POPOVER_BASE + *index as u64);
        let shown = settings_ui::popover_visible_cap(self.page_clip, self.viewport.1);
        if next < self.popover_scroll {
            self.popover_scroll = next;
        } else if next >= self.popover_scroll + shown {
            self.popover_scroll = next + 1 - shown;
        }
        true
    }

    fn scroll_to_key_focus(&mut self) {
        let Some(rect) = self.key_focus.and_then(|id| {
            self.page_controls
                .iter()
                .find(|(_, each)| *each == id)
                .map(|(rect, _)| *rect)
        }) else {
            return;
        };
        let clip = self.page_clip;
        if rect.y < clip.y {
            self.page_scroll = (self.page_scroll - (clip.y - rect.y)).max(0.0);
        } else if rect.y + rect.h > clip.y + clip.h {
            self.page_scroll += rect.y + rect.h - (clip.y + clip.h);
        }
    }

    fn click(&mut self, id: u64) {
        match id {
            settings_ui::CTRL_TAB_MAC | settings_ui::CTRL_TAB_HOST => {
                self.show_tab(id == settings_ui::CTRL_TAB_HOST);
                return;
            }
            settings_ui::CTRL_HOST_RETRY => {
                self.read_host();
                return;
            }
            settings_ui::CTRL_HOST_UPDATE_POM => {
                self.pending.update_host_pom = true;
                return;
            }
            settings_ui::CTRL_HOST_OPEN_JSON => {
                self.commit_edit();
                self.close_popover();
                self.pending.edit_host_settings =
                    self.pages.host.as_ref().map(HostPage::settings_path);
                return;
            }
            _ => {}
        }
        self.in_host(|view| view.click_here(id));
    }

    /// This Mac's tab or the host's; a host that could not answer is asked again.
    fn show_tab(&mut self, host_tab: bool) {
        self.commit_edit();
        self.close_popover();
        self.key_focus = None;
        let Some(host) = self.pages.host.as_mut() else {
            return;
        };
        if host.shown == host_tab {
            return;
        }
        host.shown = host_tab;
        let ask_again = host_tab
            && matches!(
                host.load,
                HostLoad::Unreachable(_) | HostLoad::Outdated(_) | HostLoad::Failed(_)
            );
        if host_tab && !settings_ui::HOST_CATEGORIES.contains(&self.selected) {
            self.selected = settings_ui::GENERAL;
        }
        if let Some(expanded) = self.expanded.get_mut(self.selected) {
            *expanded = true;
        }
        self.pages.language = None;
        self.page_scroll = 0.0;
        self.nav_scroll = 0.0;
        self.active_section = None;
        if ask_again {
            self.read_host();
        }
    }

    fn click_here(&mut self, id: u64) {
        if id >= settings_ui::RESET_OFFSET {
            self.commit_edit();
            self.close_popover();
            let base = id - settings_ui::RESET_OFFSET;
            if settings_ui::reset_to_default(base, &mut self.settings) {
                self.persist();
                match base {
                    settings_ui::CTRL_THEME
                    | settings_ui::CTRL_THEME_SELECTION
                    | settings_ui::CTRL_MODE
                    | settings_ui::CTRL_THEME_LIGHT
                    | settings_ui::CTRL_THEME_DARK => {
                        self.pending.apply_theme = true;
                        self.pending.redraw_others = true;
                    }
                    settings_ui::CTRL_FONT_FAMILY => self.pending.reapply_font = true,
                    settings_ui::CTRL_FONT_WEIGHT_EDIT => {
                        ui::set_ui_font_weight(self.settings.ui_font_weight as u16);
                        self.pending.redraw_others = true;
                    }
                    settings_ui::CTRL_FONT_SIZE_EDIT => {
                        ui::set_ui_text_scale(self.settings.ui_font_size / ui::UI_FONT_BASE);
                        self.pending.redraw_others = true;
                    }
                    settings_ui::CTRL_SHOW_DIAGNOSTICS
                    | settings_ui::CTRL_SHOW_CURSOR
                    | settings_ui::CTRL_SHOW_LANGUAGE
                    | settings_ui::CTRL_SHOW_BRANCH
                    | settings_ui::CTRL_SHOW_SESSION => {
                        ui::set_chrome(settings_ui::chrome_flags(&self.settings));
                        self.pending.redraw_others = true;
                    }
                    _ => {}
                }
            }
            return;
        }
        if id == settings_ui::CTRL_OPEN_JSON || settings_ui::is_edit_in_json(id) {
            self.commit_edit();
            self.close_popover();
            self.pending.edit_settings_json = true;
            return;
        }
        if let Some(language) = settings_ui::language_to_open(id) {
            self.show_language(Some(language));
            return;
        }
        if id == settings_ui::CTRL_LANGUAGE_BACK {
            self.show_language(None);
            return;
        }
        if id >= settings_ui::POPOVER_BASE && self.popover.is_some() {
            let idx = (id - settings_ui::POPOVER_BASE) as usize;
            if let Some(cid) = self.popover {
                if settings_ui::apply_choice(cid, idx, &self.fonts, &mut self.settings) {
                    self.persist();
                    if let Some(event) = settings_ui::sound_event(cid) {
                        let sound = self.settings.sound_for(event);
                        if !sound.is_empty() {
                            self.pending.play_sound = Some(sound.to_string());
                        }
                    }
                    if cid == settings_ui::CTRL_FONT_FAMILY {
                        self.pending.reapply_font = true;
                    }
                    if matches!(
                        cid,
                        settings_ui::CTRL_THEME
                            | settings_ui::CTRL_THEME_SELECTION
                            | settings_ui::CTRL_MODE
                            | settings_ui::CTRL_THEME_LIGHT
                            | settings_ui::CTRL_THEME_DARK
                    ) {
                        self.pending.apply_theme = true;
                        self.pending.redraw_others = true;
                    }
                }
            }
            self.close_popover();
        } else if id == settings_ui::CTRL_SEARCH_CLEAR {
            self.search_query.clear();
            self.page_scroll = 0.0;
        } else if id == settings_ui::CTRL_SEARCH {
            self.commit_edit();
            self.close_popover();
        } else if (settings_ui::NAV_TOGGLE_BASE
            ..settings_ui::NAV_TOGGLE_BASE + settings_ui::CATEGORY_COUNT as u64)
            .contains(&id)
        {
            let ci = (id - settings_ui::NAV_TOGGLE_BASE) as usize;
            if let Some(e) = self.expanded.get_mut(ci) {
                *e = !*e;
            }
        } else if (settings_ui::NAV_JUMP_BASE..settings_ui::NAV_JUMP_END).contains(&id) {
            self.commit_edit();
            self.popover = None;
            let rel = id - settings_ui::NAV_JUMP_BASE;
            let cat = (rel / settings_ui::NAV_JUMP_STRIDE) as usize;
            let si = (rel % settings_ui::NAV_JUMP_STRIDE) as usize;
            self.selected = cat;
            self.pages.language = None;
            if let Some(e) = self.expanded.get_mut(cat) {
                *e = true;
            }
            self.active_section = Some(si);
            self.page_scroll = 0.0;
            // Rebuild so section offsets are current, then scroll the chosen section to the top.
            let window = Window::new(self.viewport.0, self.viewport.1, 1.0);
            let _ = self.build(&window);
            if let Some(&off) = self.section_offsets.get(si) {
                let clip_h = self.content_clip_h();
                let max = (self.page_total_h - clip_h).max(0.0);
                self.page_scroll = off.clamp(0.0, max);
            }
        } else if settings_ui::PROXY_TEXT_FIELDS.contains(&id) {
            self.commit_edit();
            self.editing = Some((id, settings_ui::proxy_text(id, &self.settings)));
            self.close_popover();
        } else if id == settings_ui::CTRL_AGENT_COMMAND || id == settings_ui::CTRL_TERM_SHELL {
            self.commit_edit();
            let seed = if id == settings_ui::CTRL_TERM_SHELL {
                self.settings.terminal_shell.clone()
            } else {
                self.settings.agent_command.clone()
            };
            self.editing = Some((id, seed));
            self.close_popover();
        } else if NUMBER_FIELDS.contains(&id) {
            self.commit_edit();
            let seed = match id {
                settings_ui::CTRL_BUFFER_FONT_EDIT => {
                    format!("{:.0}", self.settings.buffer_font_size)
                }
                settings_ui::CTRL_TERM_FONT_EDIT => {
                    format!("{:.0}", self.settings.terminal_font_size)
                }
                settings_ui::CTRL_AGENT_FONT_EDIT => {
                    format!("{:.0}", self.settings.agent_font_size)
                }
                settings_ui::CTRL_BUFFER_WEIGHT_EDIT => {
                    format!("{:.0}", self.settings.buffer_font_weight)
                }
                settings_ui::CTRL_TERM_WEIGHT_EDIT => {
                    format!("{:.0}", self.settings.terminal_font_weight)
                }
                settings_ui::CTRL_MODULES_LIMIT_EDIT => {
                    self.settings.modules_size_limit_gb.to_string()
                }
                settings_ui::CTRL_MODULES_DAYS_EDIT => {
                    self.settings.modules_unused_days.to_string()
                }
                settings_ui::CTRL_PROXY_PORT_EDIT => self.settings.dev_proxy_port.to_string(),
                settings_ui::CTRL_WEBHOOK_PORT_EDIT => self.settings.webhook_port.to_string(),
                _ => self.settings.terminal_scrollback.to_string(),
            };
            self.editing = Some((id, seed));
            self.close_popover();
        } else if id == settings_ui::CTRL_START_AT_LOGIN {
            self.commit_edit();
            self.pending.toggle_login_item = true;
        } else if id == settings_ui::CTRL_CHECK_UPDATES {
            self.commit_edit();
            self.pending.check_updates = true;
        } else if id == settings_ui::CTRL_EDIT_KEYMAP {
            self.commit_edit();
            self.pending.edit_keymap = true;
        } else if id == settings_ui::CTRL_OPEN_THEMES {
            self.commit_edit();
            self.pending.open_themes = true;
        } else if id == settings_ui::CTRL_EXPORT_CONFIG {
            self.commit_edit();
            self.pending.export_config = true;
        } else if id == settings_ui::CTRL_IMPORT_CONFIG {
            self.commit_edit();
            self.pending.import_config = true;
        } else if id == settings_ui::CTRL_EDIT_PROJECT_CONFIG {
            self.commit_edit();
            self.pending.edit_project_config = true;
        } else if id == settings_ui::CTRL_ADD_REPO {
            self.commit_edit();
            self.pending.add_repo = true;
        } else if id == settings_ui::CTRL_APPLY_CONFIG {
            self.commit_edit();
            self.pending.apply_config = true;
        } else if let Some(repo) = self.repo_for(id, settings_ui::CTRL_REPO_RENAME_BASE) {
            self.commit_edit();
            self.pending.rename_repo = Some(repo);
        } else if let Some(repo) = self.repo_for(id, settings_ui::CTRL_REPO_REMOVE_BASE) {
            self.commit_edit();
            self.pending.remove_repo = Some(repo);
        } else if id == settings_ui::CTRL_REINSTALL_AGENTS {
            self.commit_edit();
            self.pending.reinstall_agents = true;
        } else if id == settings_ui::CTRL_TEST_NOTIFICATION {
            self.commit_edit();
            self.pending.test_notification = true;
        } else if id == settings_ui::CTRL_START_SERVERS {
            self.commit_edit();
            self.pending.start_servers = true;
        } else if id == settings_ui::CTRL_OPEN_REQUESTS {
            self.commit_edit();
            self.pending.open_dev_requests = true;
        } else if id == settings_ui::CTRL_OPEN_STORE {
            self.commit_edit();
            self.pending.open_module_store = true;
        } else if JIRA_FIELDS.contains(&id) {
            self.commit_edit();
            let seed = match id {
                settings_ui::CTRL_JIRA_SITE => self.pages.jira.site.clone(),
                settings_ui::CTRL_JIRA_EMAIL => self.pages.jira.email.clone(),
                _ => String::new(),
            };
            self.editing = Some((id, seed));
            self.close_popover();
        } else if id == settings_ui::CTRL_JIRA_RESET_TOKEN && self.host_store().is_some() {
            self.commit_edit();
            self.pages.jira.status = ConnectionStatus::Untested;
            self.ask_host(|store| {
                store.change(&pom_jira::SetupChange {
                    clear_token: true,
                    ..pom_jira::SetupChange::default()
                })
            });
        } else if id == settings_ui::CTRL_JIRA_RESET_TOKEN {
            self.commit_edit();
            if let Some((state, session)) = self.jira_session.clone() {
                if let Err(error) = pom_jira::save_token(&state, &session, "") {
                    eprintln!("settings: clear the Jira token: {error}");
                }
            }
            self.reload_jira(ConnectionStatus::Untested);
        } else if id == settings_ui::CTRL_REFRESH_MAIN {
            self.commit_edit();
            self.save_refresh(
                !self.pages.jira.keep_main_fresh,
                self.pages.jira.refresh_minutes,
            );
        } else if id == settings_ui::CTRL_REFRESH_DEC || id == settings_ui::CTRL_REFRESH_INC {
            self.commit_edit();
            let minutes = self.pages.jira.refresh_minutes;
            let next = if id == settings_ui::CTRL_REFRESH_INC {
                minutes + 5 - minutes % 5
            } else {
                minutes.saturating_sub(if minutes.is_multiple_of(5) {
                    5
                } else {
                    minutes % 5
                })
            };
            self.save_refresh(self.pages.jira.keep_main_fresh, next);
        } else if id == settings_ui::CTRL_REFRESH_EDIT {
            self.commit_edit();
            self.editing = Some((id, self.pages.jira.refresh_minutes.to_string()));
            self.close_popover();
        } else if id == settings_ui::CTRL_JIRA_TEST {
            self.commit_edit();
            if self.jira_test.is_none() {
                self.test_jira();
            }
        } else if id == settings_ui::CTRL_FONT_SIZE_EDIT || id == settings_ui::CTRL_FONT_WEIGHT_EDIT
        {
            self.commit_edit();
            let seed = if id == settings_ui::CTRL_FONT_WEIGHT_EDIT {
                format!("{:.0}", self.settings.ui_font_weight)
            } else {
                format!("{:.0}", self.settings.ui_font_size)
            };
            self.editing = Some((id, seed));
            self.popover = None;
            self.popover_hover = None;
        } else if let Some(seed) = settings_ui::language_number(id, &self.settings) {
            self.commit_edit();
            self.editing = Some((id, seed));
            self.close_popover();
        } else if host_tab::is_host_number_edit(id) {
            self.commit_edit();
            let seed = host_tab::host_number(id, &mut self.settings)
                .map(|(value, _, _)| value.to_string())
                .unwrap_or_default();
            self.editing = Some((id, seed));
            self.close_popover();
        } else if id >= 100 {
            self.commit_edit();
            if settings_ui::is_dropdown(id) {
                if self.popover == Some(id) {
                    self.close_popover();
                } else {
                    self.open_popover(id);
                }
            } else if settings_ui::handle_control(id, &mut self.settings) {
                self.persist();
                match id {
                    settings_ui::CTRL_FONT_WEIGHT_DEC | settings_ui::CTRL_FONT_WEIGHT_INC => {
                        ui::set_ui_font_weight(self.settings.ui_font_weight as u16);
                    }
                    settings_ui::CTRL_FONT_SIZE_DEC | settings_ui::CTRL_FONT_SIZE_INC => {
                        ui::set_ui_text_scale(self.settings.ui_font_size / ui::UI_FONT_BASE);
                    }
                    settings_ui::CTRL_SHOW_DIAGNOSTICS
                    | settings_ui::CTRL_SHOW_CURSOR
                    | settings_ui::CTRL_SHOW_LANGUAGE
                    | settings_ui::CTRL_SHOW_BRANCH
                    | settings_ui::CTRL_SHOW_SESSION => {
                        ui::set_chrome(settings_ui::chrome_flags(&self.settings));
                    }
                    _ => {}
                }
                self.pending.redraw_others = true;
            }
        } else {
            self.commit_edit();
            self.popover = None;
            self.popover_hover = None;
            let i = id as usize;
            if let Some(e) = self.expanded.get_mut(i) {
                *e = true;
            }
            self.selected = i;
            self.pages.language = None;
            self.page_scroll = 0.0;
            self.active_section = None;
        }
    }

    /// Open a language's own page over Languages & Tools, or go back from it.
    fn show_language(&mut self, language: Option<usize>) {
        self.commit_edit();
        self.close_popover();
        self.pages.language = language;
        self.page_scroll = 0.0;
        self.active_section = None;
    }
}

impl RawView for SettingsView {
    fn render_frame(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> Frame {
        self.build(window)
    }

    fn on_click(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.viewport = (window.width, window.height);
        self.click(id);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui::Application;

    fn open() -> (ui::Application, ui::WindowHandle) {
        let mut app = Application::new();
        let (h, _e) = app.open_raw_window(
            ui::WindowOptions {
                width: 920.0,
                height: 760.0,
                scale: 2.0,
                ..Default::default()
            },
            |_| SettingsView::new(Settings::default()),
        );
        (app, h)
    }

    #[test]
    fn jira_fields_save_to_the_session_and_the_token_to_its_secrets() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let mut view = SettingsView::new(Settings::default());
        view.set_jira_session(Some((state.clone(), "demo".into())));
        let no_env = std::env::var_os(pom_jira::DEFAULT_TOKEN_ENV).is_none();
        if no_env {
            assert_eq!(view.pages.jira.token, TokenSource::Missing);
        }

        view.click(settings_ui::CTRL_JIRA_SITE);
        view.key_text("acme.atlassian.net");
        view.key_enter();
        view.editing = Some((settings_ui::CTRL_JIRA_TOKEN, String::new()));
        assert!(view.key_paste("tok\n"));
        view.key_enter();

        let saved = pom_jira::JiraSettings::load(&state, "demo");
        assert_eq!(saved.site, "acme.atlassian.net");
        assert_eq!(view.pages.jira.token, TokenSource::Secret);
        let secret = pom_secrets::SecretStore::new(state.clone(), "demo")
            .get(pom_jira::TOKEN_SECRET)
            .expect("secret");
        assert_eq!(secret.as_deref(), Some("tok"));

        view.click(settings_ui::CTRL_JIRA_RESET_TOKEN);
        if no_env {
            assert_eq!(view.pages.jira.token, TokenSource::Missing);
        }
    }

    #[test]
    fn keep_main_fresh_saves_the_schedule_for_the_session() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let mut view = SettingsView::new(Settings::default());
        view.set_jira_session(Some((state.clone(), "demo".into())));
        assert!(!view.pages.jira.keep_main_fresh);
        assert_eq!(view.pages.jira.refresh_minutes, 30);
        view.click(settings_ui::CTRL_REFRESH_MAIN);
        view.click(settings_ui::CTRL_REFRESH_DEC);
        view.click(settings_ui::CTRL_REFRESH_EDIT);
        view.key_backspace();
        view.key_backspace();
        view.key_text("7");
        view.key_enter();
        assert_eq!(
            pom_sync::refresh_schedule(&state, "demo", None),
            pom_sync::RefreshSchedule {
                enabled: true,
                interval_seconds: 7 * 60,
            }
        );
        view.click(settings_ui::CTRL_REFRESH_INC);
        assert_eq!(view.pages.jira.refresh_minutes, 10);
    }

    #[test]
    fn opens_on_appearance_with_layers() {
        let (mut app, h) = open();
        let frame = app.draw(h).expect("frame");
        // A clipped page layer, the fixed chrome, and the clipped category list (no popover yet).
        assert_eq!(frame.overlays.len(), 3);
        assert!(frame.overlays[0].clip.is_some(), "page is clipped");
        assert!(frame.overlays[1].clip.is_none(), "chrome is unclipped");
        assert!(frame.overlays[2].clip.is_some(), "category list is clipped");
    }

    #[test]
    fn a_short_window_scrolls_the_category_list() {
        let mut app = Application::new();
        let (h, view) = app.open_raw_window(
            ui::WindowOptions {
                width: 920.0,
                height: 360.0,
                scale: 2.0,
                ..Default::default()
            },
            |_| SettingsView::new(Settings::default()),
        );
        let project = settings_ui::PROJECT as u64;
        let visible = |frame: &ui::Frame| frame.hits.iter().any(|(_, id)| *id == project);
        let frame = app.draw(h).expect("frame");
        assert!(
            !visible(&frame),
            "the last category starts below the window"
        );
        let thumb = ui::theme().scrollbar_thumb_background;
        assert!(
            frame.overlays[2]
                .painted
                .rects
                .iter()
                .any(|rect| rect.color == thumb),
            "the category list shows a scrollbar"
        );
        let moved = view.update(app.app_mut(), |v, _| v.scroll(-10_000.0, 20.0, 200.0));
        assert!(moved);
        let frame = app.draw(h).expect("frame");
        assert!(visible(&frame), "scrolled into view");
    }

    #[test]
    fn clicking_theme_control_opens_then_closes_popover() {
        let (mut app, h) = open();
        app.draw(h);
        let theme_anchor = app
            .window(h)
            .and_then(|w| w.center_of(settings_ui::CTRL_THEME))
            .expect("theme control laid out");
        app.simulate_click(h, theme_anchor.0, theme_anchor.1);
        let frame = app.draw(h).expect("frame");
        assert_eq!(frame.overlays.len(), 4, "popover layer added");
        // Click the control again to toggle it closed.
        app.simulate_click(h, theme_anchor.0, theme_anchor.1);
        let frame = app.draw(h).expect("frame");
        assert_eq!(frame.overlays.len(), 3, "popover closed");
    }

    #[test]
    fn selecting_a_theme_item_applies_and_persists() {
        let (mut app, h) = open();
        app.draw(h);
        let anchor = app
            .window(h)
            .and_then(|w| w.center_of(settings_ui::CTRL_THEME))
            .expect("theme control");
        app.simulate_click(h, anchor.0, anchor.1);
        app.draw(h);
        // Click the second theme item in the popover list.
        let item = app
            .window(h)
            .and_then(|w| w.center_of(settings_ui::POPOVER_BASE + 1))
            .expect("popover item");
        let hit = app.simulate_click(h, item.0, item.1);
        assert_eq!(hit, Some(settings_ui::POPOVER_BASE + 1));
        let frame = app.draw(h).expect("frame");
        assert_eq!(frame.overlays.len(), 3, "popover closed after choosing");
    }

    #[test]
    fn action_buttons_hand_their_work_to_the_app() {
        let mut view = SettingsView::new(Settings::default());
        view.click(settings_ui::CTRL_TEST_NOTIFICATION);
        view.click(settings_ui::CTRL_REINSTALL_AGENTS);
        view.click(settings_ui::CTRL_START_SERVERS);
        view.click(settings_ui::CTRL_OPEN_REQUESTS);
        let effects = view.take_side_effects();
        assert!(effects.test_notification && effects.reinstall_agents && effects.start_servers);
        assert!(effects.open_dev_requests);
        view.click(settings_ui::CTRL_OPEN_STORE);
        assert!(view.take_side_effects().open_module_store);
        view.click(settings_ui::CTRL_PROXY_PORT_EDIT);
        assert_eq!(
            view.editing.as_ref().map(|(_, text)| text.as_str()),
            Some("8767")
        );
        view.editing = Some((settings_ui::CTRL_PROXY_PORT_EDIT, "80".into()));
        view.commit_edit();
        assert_eq!(view.settings.dev_proxy_port, settings_ui::PORT_MIN);
        assert!(!view.take_side_effects().test_notification);
        view.click(settings_ui::CTRL_AGENT_COMMAND);
        assert_eq!(
            view.editing,
            Some((settings_ui::CTRL_AGENT_COMMAND, "claude".to_string()))
        );
        assert!(view.key_text("-x"));
        assert_eq!(
            view.editing.as_ref().map(|(_, text)| text.as_str()),
            Some("claude-x")
        );
        assert!(view.set_dev_services_page(settings_ui::DevServicesPage {
            proxy_port: 8767,
            ..Default::default()
        }));
        assert!(!view.set_dev_services_page(settings_ui::DevServicesPage {
            proxy_port: 8767,
            ..Default::default()
        }));
    }

    #[test]
    fn startup_update_and_keymap_buttons_go_to_the_app() {
        let mut view = SettingsView::new(Settings::default());
        view.click(settings_ui::CTRL_START_AT_LOGIN);
        view.click(settings_ui::CTRL_CHECK_UPDATES);
        view.click(settings_ui::CTRL_EDIT_KEYMAP);
        view.click(settings_ui::CTRL_OPEN_JSON);
        view.click(settings_ui::CTRL_EXPORT_CONFIG);
        view.click(settings_ui::CTRL_IMPORT_CONFIG);
        let effects = view.take_side_effects();
        assert!(effects.export_config && effects.import_config);
        assert!(effects.toggle_login_item && effects.check_updates && effects.edit_keymap);
        assert!(effects.edit_settings_json);
        view.click(settings_ui::CTRL_TERM_SHELL);
        assert!(view.key_text("/bin/bash -l"));
        assert_eq!(
            view.editing.as_ref().map(|(_, text)| text.as_str()),
            Some("/bin/bash -l")
        );
        view.click(settings_ui::CTRL_SCROLLBACK_EDIT);
        assert_eq!(
            view.editing.as_ref().map(|(_, text)| text.as_str()),
            Some("10000"),
            "the shell edit was committed and the number field seeded"
        );
        view.select_category(settings_ui::KEYMAP);
        assert_eq!(view.selected, settings_ui::KEYMAP);
    }

    #[test]
    fn the_window_works_from_the_keyboard() {
        let mut view = SettingsView::new(Settings::default());
        let start = view.selected;
        assert!(view.key_vertical(true));
        assert_eq!(view.selected, start + 1, "down opens the next page");
        assert!(view.key_vertical(false));
        assert_eq!(view.selected, start);
        assert!(view.key_home_end(true));
        assert_eq!(view.selected, settings_ui::CATEGORY_COUNT - 1);
        assert!(view.key_home_end(false));
        assert_eq!(view.selected, 0);

        let anywhere = Rect::new(0.0, 0.0, 10.0, 10.0, ui::Rgba::TRANSPARENT);
        view.page_controls = vec![
            (anywhere, settings_ui::CTRL_THEME),
            (anywhere, settings_ui::CTRL_START_AT_LOGIN),
        ];
        assert!(view.key_tab(false));
        assert_eq!(view.key_focus, Some(settings_ui::CTRL_THEME));
        assert!(view.key_enter(), "enter opens the dropdown");
        assert_eq!(view.popover, Some(settings_ui::CTRL_THEME));
        let before = view.popover_hover;
        assert!(view.key_vertical(true));
        assert_ne!(view.popover_hover, before, "down moves through its items");
        let picked = view.popover_hover;
        assert!(view.key_enter());
        assert!(
            view.popover.is_none() && picked.is_some(),
            "enter picks the item"
        );

        assert!(view.key_tab(false));
        assert_eq!(view.key_focus, Some(settings_ui::CTRL_START_AT_LOGIN));
        assert!(view.key_text(" "), "space presses the control");
        assert!(view.take_side_effects().toggle_login_item);
        assert!(view.key_horizontal(false), "left goes back to the pages");
        assert_eq!(view.key_focus, None);
        assert!(view.key_tab(true));
        assert!(
            view.key_escape(),
            "escape leaves the controls before closing anything"
        );
        assert_eq!(view.key_focus, None);
    }

    /// Jira settings kept in memory, as a host's pom keeps them.
    #[derive(Default)]
    struct HostSettings {
        setup: std::sync::Mutex<pom_jira::JiraSetup>,
    }

    impl pom_jira::SetupStore for HostSettings {
        fn read(&self) -> Result<pom_jira::JiraSetup, String> {
            self.setup
                .lock()
                .map(|setup| setup.clone())
                .map_err(|error| error.to_string())
        }

        fn change(&self, change: &pom_jira::SetupChange) -> Result<pom_jira::JiraSetup, String> {
            let mut setup = self.setup.lock().map_err(|error| error.to_string())?;
            if let Some(site) = &change.site {
                setup.site = site.clone();
            }
            if let Some(email) = &change.email {
                setup.email = email.clone();
            }
            if change.token.is_some() {
                setup.token = Some(pom_jira::TokenOrigin::Secret);
            } else if change.clear_token {
                setup.token = None;
            }
            Ok(setup.clone())
        }

        fn test(&self) -> Result<String, String> {
            Ok("Dev (you@example.com)".into())
        }
    }

    fn settle(view: &mut SettingsView) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while view.busy() {
            assert!(
                std::time::Instant::now() < deadline,
                "the host never answered"
            );
            view.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_remote_project_edits_the_settings_on_its_host() {
        let temp = tempfile::tempdir().expect("temp");
        let host = std::sync::Arc::new(HostSettings::default());
        let mut view = SettingsView::new(Settings::default());
        view.set_jira_remote(Some(RemoteJira {
            host: "dev".into(),
            session: "myproject@dev".into(),
            state: StateDir::new(temp.path()),
            store: Some(host.clone()),
        }));
        settle(&mut view);
        assert_eq!(view.pages.jira.session, "myproject@dev");
        assert_eq!(
            view.pages.jira.kept_on.as_ref().map(|kept| kept.0.as_str()),
            Some("dev")
        );
        assert_eq!(view.pages.jira.token, TokenSource::Missing);

        view.click(settings_ui::CTRL_JIRA_SITE);
        view.key_text("example.atlassian.net");
        view.key_enter();
        view.editing = Some((settings_ui::CTRL_JIRA_TOKEN, String::new()));
        assert!(view.key_paste("tok"));
        view.key_enter();
        settle(&mut view);
        assert_eq!(view.pages.jira.site, "example.atlassian.net");
        assert_eq!(view.pages.jira.token, TokenSource::Secret);
        assert_eq!(
            pom_jira::JiraSettings::load(&StateDir::new(temp.path()), "myproject@dev"),
            pom_jira::JiraSettings::default(),
            "nothing kept on this Mac"
        );

        view.click(settings_ui::CTRL_JIRA_TEST);
        settle(&mut view);
        assert_eq!(
            view.pages.jira.status,
            ConnectionStatus::SignedIn("Dev (you@example.com)".into())
        );
        view.click(settings_ui::CTRL_JIRA_RESET_TOKEN);
        settle(&mut view);
        assert_eq!(view.pages.jira.token, TokenSource::Missing);
    }

    /// A host's settings.json in memory, refusing what a host refuses.
    struct HostFile {
        file: std::sync::Mutex<Value>,
        outdated: bool,
    }

    impl HostFile {
        fn new(outdated: bool) -> Arc<HostFile> {
            Arc::new(HostFile {
                file: std::sync::Mutex::new(serde_json::json!({"theme": "kept"})),
                outdated,
            })
        }

        fn answer(file: &Value) -> HostAnswer {
            HostAnswer {
                values: settings::host_values(file),
                keys: settings::HOST_KEYS
                    .iter()
                    .map(|key| key.to_string())
                    .collect(),
                path: "/home/dev/.config/pomelo/settings.json".into(),
            }
        }
    }

    impl HostSettingsStore for HostFile {
        fn read(&self) -> Result<HostAnswer, HostError> {
            if self.outdated {
                return Err(HostError::Outdated(
                    "Changing the settings of dev needs a newer pom on dev; update it there".into(),
                ));
            }
            let file = self
                .file
                .lock()
                .map_err(|error| HostError::Failed(error.to_string()))?;
            Ok(HostFile::answer(&file))
        }

        fn change(&self, values: Value) -> Result<HostAnswer, HostError> {
            let mut file = self
                .file
                .lock()
                .map_err(|error| HostError::Failed(error.to_string()))?;
            settings::apply_host_change(&mut file, &values)
                .map_err(|error| HostError::Refused(error.to_string()))?;
            Ok(HostFile::answer(&file))
        }
    }

    fn remote_host(store: Arc<HostFile>) -> settings_ui::RemoteHost {
        settings_ui::RemoteHost {
            name: "dev".into(),
            version: "0.9.9".into(),
            store,
        }
    }

    #[test]
    fn the_host_tab_changes_the_hosts_settings_and_leaves_this_macs() {
        let file = HostFile::new(false);
        let mut view = SettingsView::new(Settings::default());
        view.set_remote_host(Some(remote_host(file.clone())));
        settle(&mut view);
        view.select_category(settings_ui::KEYMAP);
        view.click(settings_ui::CTRL_TAB_HOST);
        assert!(view.host_shown());
        assert_eq!(view.selected, settings_ui::GENERAL, "a page the host has");
        assert_eq!(
            view.pages.host.as_ref().map(|host| host.load.clone()),
            Some(HostLoad::Ready)
        );

        view.select_category(settings_ui::AGENT);
        view.click(settings_ui::CTRL_TAB_HOST);
        view.click(settings_ui::CTRL_AGENT_COMMAND);
        view.key_backspace();
        view.key_text("x");
        view.key_enter();
        view.click(settings_ui::CTRL_IDLE_EXIT_INC);
        settle(&mut view);
        let host_file = file.file.lock().expect("file").clone();
        assert_eq!(host_file["agent_command"], "claudx");
        assert_eq!(host_file["server"]["idle_exit_minutes"], 35);
        assert_eq!(host_file["theme"], "kept");
        assert_eq!(
            view.settings.agent_command, "claude",
            "this Mac keeps its own"
        );
        assert_eq!(
            view.pages
                .host
                .as_ref()
                .map(|host| host.settings.agent_command.clone()),
            Some("claudx".into())
        );

        view.click(settings_ui::CTRL_PROXY_BIND);
        view.editing = Some((settings_ui::CTRL_PROXY_BIND, "not an address".into()));
        view.key_enter();
        let errors = view
            .pages
            .host
            .as_ref()
            .map(HostPage::control_errors)
            .unwrap_or_default();
        assert_eq!(errors.len(), 1, "said on the row: {errors:?}");
        assert_eq!(errors[0].0, settings_ui::CTRL_PROXY_BIND);

        view.click(settings_ui::CTRL_HOST_OPEN_JSON);
        assert_eq!(
            view.take_side_effects().edit_host_settings.as_deref(),
            Some("/home/dev/.config/pomelo/settings.json")
        );
        view.click(settings_ui::CTRL_TAB_MAC);
        view.click(settings_ui::CTRL_AGENT_COMMAND);
        assert_eq!(
            view.editing.as_ref().map(|(_, text)| text.as_str()),
            Some("claude"),
            "This Mac edits its own again"
        );
    }

    #[test]
    fn a_value_the_host_refuses_goes_back_and_says_why_on_its_row() {
        let file = HostFile::new(false);
        let mut view = SettingsView::new(Settings::default());
        view.set_remote_host(Some(remote_host(file.clone())));
        settle(&mut view);
        view.click(settings_ui::CTRL_TAB_HOST);
        view.editing = Some((settings_ui::CTRL_MODULES_LIMIT_EDIT, "0".into()));
        view.key_enter();
        settle(&mut view);
        let host = view.pages.host.clone().expect("host");
        assert_eq!(
            host.settings.modules_size_limit_gb, 1,
            "clamped as on this Mac"
        );

        let mut change = Map::new();
        change.insert("dev_proxy_port".into(), Value::from(80));
        let before = Box::new(host.settings.clone());
        if let Some(host) = view.pages.host.as_mut() {
            host.settings.dev_proxy_port = 80;
        }
        let values = Value::Object(change.clone());
        view.ask_host_settings(HostAsk::Change { change, before }, move |store| {
            store.change(values)
        });
        settle(&mut view);
        let host = view.pages.host.clone().expect("host");
        assert_eq!(host.settings.dev_proxy_port, 8767, "put back");
        assert_eq!(
            host.control_errors(),
            vec![(
                settings_ui::CTRL_PROXY_PORT_EDIT,
                "80 is not between 1024 and 65535".to_string()
            )]
        );
    }

    #[test]
    fn an_older_pom_on_the_host_asks_for_an_update() {
        let mut view = SettingsView::new(Settings::default());
        view.set_remote_host(Some(remote_host(HostFile::new(true))));
        settle(&mut view);
        assert!(matches!(
            view.pages.host.as_ref().map(|host| &host.load),
            Some(HostLoad::Outdated(_))
        ));
        view.click(settings_ui::CTRL_TAB_HOST);
        settle(&mut view);
        view.click(settings_ui::CTRL_HOST_UPDATE_POM);
        assert!(view.take_side_effects().update_host_pom);
        assert!(view.key_home_end(true));
        assert_eq!(
            view.selected,
            settings_ui::INTEGRATIONS,
            "the host tab's last page"
        );
    }

    #[test]
    fn an_older_host_keeps_the_settings_here_under_its_project() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let mut view = SettingsView::new(Settings::default());
        view.set_jira_remote(Some(RemoteJira {
            host: "dev".into(),
            session: "myproject@dev".into(),
            state: state.clone(),
            store: None,
        }));
        assert_eq!(
            view.pages.jira.kept_on.as_ref().map(|kept| kept.0.as_str()),
            Some("This Mac")
        );
        view.click(settings_ui::CTRL_JIRA_EMAIL);
        view.key_text("you@example.com");
        view.key_enter();
        assert_eq!(
            pom_jira::JiraSettings::load(&state, "myproject@dev").email,
            "you@example.com"
        );
    }
}
