//! Keeps the agents' usage current: a background thread reads what the transcripts added and asks for the
//! account's plan limits each minute (backing off when asked too often), and each window gets its chip,
//! its status bar total and, when open, its Agent usage tab.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use agent_usage::{Account, Limits, LimitsError, Turn};
use agent_usage_ui::{AgentKind, Request, UsagePage, UsageState, UsageTurn};
use winit::window::WindowId;

use crate::App;

const PERIOD_DAYS: u64 = 60;
const CHECK_EVERY: Duration = Duration::from_secs(60);

struct Snapshot {
    turns: Vec<Turn>,
    account: Option<Account>,
    limits: Result<Limits, LimitsError>,
    limits_at: Option<u64>,
}

#[derive(Default)]
pub(crate) struct UsageTracker {
    receiver: Option<Receiver<Snapshot>>,
    now: Option<Arc<AtomicBool>>,
    turns: Vec<Turn>,
    account: Option<Account>,
    limits: Option<Limits>,
    limits_error: Option<LimitsError>,
    limits_at: Option<u64>,
    /// Each window's open Agent usage tab.
    pages: Vec<(WindowId, agent_usage_ui::Shared)>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn start(sender: std::sync::mpsc::Sender<Snapshot>, now: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("agent-usage".into())
        .spawn(move || {
            let home = home();
            let projects = home.join(".claude/projects");
            let mut transcripts = agent_usage::Transcripts::default();
            let mut keeper = agent_usage::LimitsKeeper::default();
            let reported_path = pom_paths::StateDir::from_env().path(pom_agent::RATE_LIMITS_FILE);
            loop {
                let since = agent_usage::unix_now().saturating_sub(PERIOD_DAYS * 86_400);
                transcripts.refresh(&projects, since);
                let account = agent_usage::read_account(&home);
                let limits = keeper.update(
                    agent_usage::unix_now(),
                    account.is_some(),
                    agent_usage::reported_limits(&reported_path),
                    now.swap(false, Ordering::Relaxed),
                    || agent_usage::fetch_limits(&home),
                );
                let snapshot = Snapshot {
                    turns: transcripts.turns(since).into_iter().cloned().collect(),
                    account,
                    limits,
                    limits_at: keeper.kept_at(),
                };
                if sender.send(snapshot).is_err() {
                    return;
                }
                ui::wake();
                for _ in 0..CHECK_EVERY.as_secs() {
                    if now.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        })
        .map_err(|error| eprintln!("agent usage: {error}"))
        .ok();
}

/// What a remote window shows of its host's agents: nothing until the host says, and nothing at all from a
/// host whose pom cannot (never this Mac's numbers).
type HostUsage = Option<pom_remote::server_api::UsageReport>;

const HOST_RETRY: Duration = Duration::from_secs(5);

/// Asks a remote window's host for its usage each minute, on a thread that ends with the window.
pub(crate) struct RemoteUsage {
    shared: Arc<std::sync::Mutex<(u64, HostUsage)>>,
    stop: Arc<AtomicBool>,
    now: Arc<AtomicBool>,
    seen: u64,
}

impl Drop for RemoteUsage {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl RemoteUsage {
    pub(crate) fn start(source: Arc<dyn pom_remote::ProjectSource>) -> RemoteUsage {
        let usage = RemoteUsage {
            shared: Arc::default(),
            stop: Arc::default(),
            now: Arc::default(),
            seen: 0,
        };
        let (shared, stop, now) = (usage.shared.clone(), usage.stop.clone(), usage.now.clone());
        let spawned = std::thread::Builder::new()
            .name("host-usage".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let since = agent_usage::local_day_start(agent_usage::unix_now());
                    let next = match source.usage(since) {
                        Ok(report) => Some(report),
                        Err(error) => {
                            eprintln!("host usage: {error}");
                            None
                        }
                    };
                    let known = source.transport().is_some();
                    if let Some(next) = next {
                        let mut state = shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if state.1 != next {
                            *state = (state.0.wrapping_add(1), next);
                            ui::wake();
                        }
                    }
                    let wait = if known { CHECK_EVERY } else { HOST_RETRY };
                    for _ in 0..wait.as_secs() {
                        if stop.load(Ordering::Relaxed) || now.swap(false, Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("host usage: {error}");
        }
        usage
    }

    fn state(&self) -> HostUsage {
        self.shared
            .lock()
            .map(|state| state.1.clone())
            .unwrap_or_default()
    }

    /// Whether the host said something new since the last call.
    fn take_changed(&mut self) -> bool {
        let version = self.shared.lock().map(|state| state.0).unwrap_or_default();
        std::mem::replace(&mut self.seen, version) != version
    }

    fn ask_now(&self) {
        self.now.store(true, Ordering::Relaxed);
    }
}

fn limits_note(
    error: Option<&LimitsError>,
    limits_at: Option<u64>,
    signed_in: bool,
    place: &str,
) -> String {
    match (error, limits_at) {
        (Some(LimitsError::RateLimited), Some(at)) => {
            format!("Updated {} - asked too often, trying again soon", ago(at))
        }
        (Some(LimitsError::RateLimited), None) => {
            format!("The limits service is busy; they show once an agent runs {place}")
        }
        (Some(LimitsError::SignedOut), _) if signed_in => format!(
            "Claude Code's saved sign-in did not work; limits show once an agent runs {place}"
        ),
        (Some(LimitsError::Failed(error)), _) => format!("Could not read the limits: {error}"),
        (_, Some(at)) => format!("Updated {}", ago(at)),
        _ => "Checks every minute".into(),
    }
}

fn shown_account(account: &Account) -> workspace::UsageAccount {
    workspace::UsageAccount {
        name: account.name.clone(),
        email: account.email.clone(),
        plan: account.plan.clone(),
        organization: account.organization.clone(),
    }
}

fn shown_today(summary: agent_usage::DaySummary) -> workspace::UsageToday {
    workspace::UsageToday {
        total: agent_usage_ui::format_cost(summary.cost),
        sessions: summary.sessions,
        by_workspace: summary
            .by_workspace
            .into_iter()
            .map(|(name, cost)| (name, agent_usage_ui::format_cost(cost)))
            .collect(),
    }
}

fn usage_window(used: f32, resets_at: Option<u64>) -> workspace::UsageWindow {
    workspace::UsageWindow {
        used,
        resets: reset_text(resets_at),
    }
}

/// A remote window's chip and today's cost: its host's, as the host reported them.
fn host_usage_info(usage: &HostUsage, host: &str) -> workspace::UsageInfo {
    let Some(report) = usage else {
        return workspace::UsageInfo {
            hidden: true,
            ..workspace::UsageInfo::default()
        };
    };
    let place = format!("on {host}");
    workspace::UsageInfo {
        account: report.account.as_ref().map(shown_account),
        session: report
            .limits
            .map(|limits| usage_window(limits.session.used, limits.session.resets_at)),
        weekly: report
            .limits
            .map(|limits| usage_window(limits.weekly.used, limits.weekly.resets_at)),
        note: format!(
            "{host}: {}",
            limits_note(
                report.limits_error.as_ref(),
                report.limits_at,
                report.account.is_some(),
                &place,
            )
        ),
        today: report.today.clone().map(shown_today),
        hidden: false,
    }
}

/// "in 2h 31m" within a day, else the weekday and time.
fn reset_text(at: Option<u64>) -> String {
    let Some(at) = at else {
        return "not started".into();
    };
    let now = agent_usage::unix_now();
    let left = at.saturating_sub(now);
    if left == 0 {
        return "now".into();
    }
    if left < 86_400 {
        let (hours, minutes) = (left / 3600, (left % 3600) / 60);
        return if hours > 0 {
            format!("in {hours}h {minutes}m")
        } else {
            format!("in {minutes}m")
        };
    }
    let seconds = at as libc::time_t;
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r only writes the struct it is given.
    if unsafe { libc::localtime_r(&seconds, &mut local).is_null() } {
        return String::new();
    }
    let day =
        ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][local.tm_wday.rem_euclid(7) as usize];
    format!("{day} {:02}:{:02}", local.tm_hour, local.tm_min)
}

fn ago(at: u64) -> String {
    let seconds = agent_usage::unix_now().saturating_sub(at);
    match seconds {
        0..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        _ => format!("{}h ago", seconds / 3600),
    }
}

/// The workspace branch a transcript's working folder belongs to, when it is under `root`.
fn branch_of(root: &Path, cwd: &Path) -> Option<String> {
    let relative = cwd.strip_prefix(root).ok()?;
    let first = relative
        .components()
        .next()?
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    first
        .strip_prefix(pom_layout::WORKSPACE_PREFIX)
        .map(str::to_string)
}

impl App {
    pub(crate) fn poll_usage(&mut self) {
        if self.usage.receiver.is_none() {
            let (sender, receiver) = std::sync::mpsc::channel();
            let now = Arc::new(AtomicBool::new(false));
            start(sender, now.clone());
            self.usage.receiver = Some(receiver);
            self.usage.now = Some(now);
        }
        let mut changed = false;
        loop {
            let Some(receiver) = self.usage.receiver.as_ref() else {
                return;
            };
            match receiver.try_recv() {
                Ok(snapshot) => {
                    changed = true;
                    self.usage.turns = snapshot.turns;
                    self.usage.account = snapshot.account;
                    self.usage.limits_at = snapshot.limits_at;
                    match snapshot.limits {
                        Ok(limits) => {
                            self.usage.limits = Some(limits);
                            self.usage.limits_error = None;
                        }
                        Err(error) => {
                            if error == LimitsError::SignedOut {
                                self.usage.limits = None;
                            }
                            self.usage.limits_error = Some(error);
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.usage.receiver = None;
                    break;
                }
            }
        }
        for main in self.mains.values_mut() {
            if let Some(remote) = main.remote.as_mut() {
                changed |= remote.usage.take_changed();
            }
        }
        if changed {
            self.push_usage();
        }
    }

    pub(crate) fn refresh_usage_now(&mut self) {
        if let Some(now) = self.usage.now.as_ref() {
            now.store(true, Ordering::Relaxed);
        }
        for main in self.mains.values() {
            if let Some(remote) = main.remote.as_ref() {
                remote.usage.ask_now();
            }
        }
    }

    fn usage_info(&self, id: WindowId) -> workspace::UsageInfo {
        if let Some(remote) = self.mains.get(&id).and_then(|main| main.remote.as_ref()) {
            return host_usage_info(&remote.usage.state(), &remote.host);
        }
        let tracker = &self.usage;
        let note = limits_note(
            tracker.limits_error.as_ref(),
            tracker.limits_at,
            tracker.account.is_some(),
            "here",
        );
        let root = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .map(|project| project.root.clone());
        let today = agent_usage::local_day(agent_usage::unix_now());
        let todays = tracker
            .turns
            .iter()
            .filter(|turn| agent_usage::local_day(turn.at) == today)
            .filter(|turn| {
                root.as_ref()
                    .is_none_or(|root| branch_of(root, &turn.cwd).is_some())
            });
        let today = agent_usage::day_summary(todays, |turn| {
            root.as_ref()
                .and_then(|root| branch_of(root, &turn.cwd))
                .unwrap_or_else(|| "other projects".into())
        });
        workspace::UsageInfo {
            account: tracker.account.as_ref().map(shown_account),
            session: tracker
                .limits
                .map(|limits| usage_window(limits.session.used, limits.session.resets_at)),
            weekly: tracker
                .limits
                .map(|limits| usage_window(limits.weekly.used, limits.weekly.resets_at)),
            note,
            today: today.map(shown_today),
            hidden: false,
        }
    }

    fn push_usage(&mut self) {
        let windows: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in windows {
            let info = self.usage_info(id);
            let changed = self
                .with_workspace_view(id, |view, _| view.set_usage(info.clone()))
                .unwrap_or_default();
            if changed {
                if let Some(main) = self.mains.get_mut(&id) {
                    main.dirty = true;
                }
            }
            let turns = self.usage_turns(id);
            let titles = self.usage_titles(id);
            for (_, shared) in self.usage.pages.iter().filter(|(window, _)| *window == id) {
                let mut state = shared.borrow_mut();
                state.turns = turns.clone();
                state.titles = titles.clone();
                state.limits = info.clone();
                state.today = agent_usage::local_day(agent_usage::unix_now());
                state.version = state.version.wrapping_add(1);
            }
        }
    }

    /// The window's project's turns, each placed in its workspace and marked main, side or task agent.
    fn usage_turns(&self, id: WindowId) -> Vec<UsageTurn> {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return Vec::new();
        };
        let state = pom_paths::StateDir::from_env();
        let mut kinds: std::collections::HashMap<String, AgentKind> =
            std::collections::HashMap::new();
        for workspace in &project.workspaces {
            kinds.insert(
                pom_agent::main_session_id(&workspace.branch, workspace.is_main),
                AgentKind::Main,
            );
            for record in pom_agent::side_records(&state, &workspace.branch) {
                kinds.insert(record.session, AgentKind::Side);
            }
        }
        self.usage
            .turns
            .iter()
            .filter_map(|turn| {
                let branch = branch_of(&project.root, &turn.cwd)?;
                Some(UsageTurn {
                    workspace: branch,
                    kind: kinds.get(&turn.session).copied().unwrap_or(AgentKind::Task),
                    model: turn.model.clone(),
                    day: agent_usage::local_day(turn.at),
                    session: turn.session.clone(),
                    tokens: turn.tokens(),
                    output: turn.output,
                    cache_read: turn.cache_read,
                    cost: agent_usage::cost(turn),
                })
            })
            .collect()
    }

    fn usage_titles(&self, id: WindowId) -> std::collections::HashMap<String, String> {
        let mut titles = std::collections::HashMap::new();
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return titles;
        };
        let state = pom_paths::StateDir::from_env();
        for workspace in &project.workspaces {
            for record in pom_agent::side_records(&state, &workspace.branch) {
                titles.insert(record.session, record.title);
            }
        }
        titles
    }

    pub(crate) fn open_agent_usage(&mut self, id: WindowId) {
        let shared = match self.usage.pages.iter().find(|(window, _)| *window == id) {
            Some((_, shared)) => shared.clone(),
            None => {
                let shared: agent_usage_ui::Shared = Rc::new(RefCell::new(UsageState::default()));
                self.usage.pages.push((id, shared.clone()));
                shared
            }
        };
        self.push_usage();
        let page = UsagePage::new(shared);
        self.with_workspace_view(id, |view, _| view.open_page(Box::new(page)));
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    /// What the open usage tabs asked for.
    pub(crate) fn poll_usage_pages(&mut self) {
        let requests: Vec<(WindowId, Request)> = self
            .usage
            .pages
            .iter()
            .flat_map(|(id, shared)| {
                std::mem::take(&mut shared.borrow_mut().requests)
                    .into_iter()
                    .map(|request| (*id, request))
            })
            .collect();
        for (id, request) in requests {
            let Request::OpenSession { session, workspace } = request;
            self.open_usage_session(id, &session, &workspace);
        }
        // A tab closed by hand leaves only this window's hold on its state.
        self.usage
            .pages
            .retain(|(_, shared)| Rc::strong_count(shared) > 1);
    }

    fn open_usage_session(&mut self, id: WindowId, session: &str, branch: &str) {
        let index = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .and_then(|project| {
                project
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.branch == branch)
            });
        let Some(index) = index else {
            self.with_workspace_view(id, |view, _| {
                view.show_toast(format!("{branch} is no longer a workspace"), None)
            });
            return;
        };
        self.activate_workspace(id, index);
        let is_main = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .and_then(|project| project.workspaces.get(index))
            .is_some_and(|workspace| workspace.is_main);
        if pom_agent::main_session_id(branch, is_main) == session {
            self.open_agent(id);
            return;
        }
        let state = pom_paths::StateDir::from_env();
        let archived = pom_agent::side_records(&state, branch);
        let holders = pom_ptyhost::SocketDir::from_env();
        let closed: Vec<_> = archived
            .iter()
            .filter(|record| !holders.holder_alive(&record.holder))
            .collect();
        if let Some(position) = closed.iter().position(|record| record.session == session) {
            self.reopen_side_agent(id, position);
            return;
        }
        if let Some(record) = archived.iter().find(|record| record.session == session) {
            let item = format!("agent:{}", record.holder);
            self.with_workspace_view(id, |view, _| view.open_agent_item(&item, false, || None));
            return;
        }
        self.with_workspace_view(id, |view, _| {
            view.show_toast(
                "That was a one-off agent (onboarding or a fix); it cannot be reopened",
                None,
            )
        });
    }
}
