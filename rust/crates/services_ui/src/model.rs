//! What the panel shows, independent of drawing: the workspace's services grouped by repo, their live
//! status, and the actions in flight. Status comes from a background poller and action threads.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_services::{ServiceRunner, ServiceTarget};

use crate::access::{LocalServices, ServicesAccess};
#[cfg(test)]
pub(crate) use pom_services::{strip_terminal_codes as strip_ansi, telling_line};

/// The project's config, replaced in place when `pom.yml` changes so every panel sees the new one.
pub type SharedConfig = Arc<RwLock<Option<Arc<Config>>>>;

pub(crate) const WORKSPACE_GROUP: &str = "_ws";
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Polling pauses once the panel has not been drawn for this long (hidden, or another workspace).
const VISIBLE_GRACE: Duration = Duration::from_secs(3);
/// Asking Docker is a subprocess, so shared containers are checked only every this many polls.
const SHARED_POLL_EVERY: u32 = 5;
pub(crate) const ALL_SHARED: &str = "*";

/// A shared-service action: one container, or the whole stack when `name` is `ALL_SHARED`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedRun {
    pub name: String,
    pub action: Action,
}

pub(crate) fn shared_key(name: &str) -> String {
    format!("shared:{name}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Crashed,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Relocate,
}

impl Action {
    pub fn progress_label(self) -> &'static str {
        match self {
            Action::Start => "starting...",
            Action::Stop => "stopping...",
            Action::Restart | Action::Relocate => "restarting...",
        }
    }
}

/// One workspace of one project on this machine: where its services live and how they run.
pub struct ServicesContext {
    pub runner: Arc<ServiceRunner>,
    pub config: SharedConfig,
    pub branch: String,
    /// The ticket the workspace's branch names, empty when it names none.
    pub ticket: String,
    pub is_main: bool,
    pub waker: Arc<dyn Fn() + Send + Sync>,
}

impl ServicesContext {
    pub fn config(&self) -> Option<Arc<Config>> {
        self.config.read().ok().and_then(|config| config.clone())
    }

    pub fn target(&self, repo: &str, service: &str) -> ServiceTarget {
        target(&self.branch, self.is_main, repo, service)
    }

    /// Every service of the workspace: workspace-level ones first, then each repo's in config order.
    pub fn targets(&self, config: &Config) -> Vec<ServiceTarget> {
        targets(&self.branch, self.is_main, config)
    }

    pub fn status_of(&self, target: &ServiceTarget) -> Status {
        self.local().status_of(target)
    }

    fn local(&self) -> LocalServices {
        LocalServices {
            runner: self.runner.clone(),
            config: self.config.clone(),
        }
    }
}

/// The workspace a panel shows, and where its facts come from.
pub struct WorkspaceInfo {
    pub branch: String,
    /// The ticket the workspace's branch names, empty when it names none.
    pub ticket: String,
    pub is_main: bool,
}

/// What the panel and its tabs share: the workspace, its source and how to ask for a redraw.
pub struct Context {
    pub access: Arc<dyn ServicesAccess>,
    pub branch: String,
    pub ticket: String,
    pub is_main: bool,
    pub waker: Arc<dyn Fn() + Send + Sync>,
}

impl Context {
    pub fn new(
        access: Arc<dyn ServicesAccess>,
        workspace: WorkspaceInfo,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> Context {
        Context {
            access,
            branch: workspace.branch,
            ticket: workspace.ticket,
            is_main: workspace.is_main,
            waker,
        }
    }

    pub fn config(&self) -> Option<Arc<Config>> {
        self.access.config()
    }

    pub fn target(&self, repo: &str, service: &str) -> ServiceTarget {
        target(&self.branch, self.is_main, repo, service)
    }

    pub fn targets(&self, config: &Config) -> Vec<ServiceTarget> {
        targets(&self.branch, self.is_main, config)
    }

    pub fn holder_name(&self, target: &ServiceTarget) -> String {
        self.access.holder_name(target)
    }
}

impl From<ServicesContext> for Context {
    fn from(context: ServicesContext) -> Context {
        Context {
            access: Arc::new(context.local()),
            branch: context.branch,
            ticket: context.ticket,
            is_main: context.is_main,
            waker: context.waker,
        }
    }
}

/// Anything a tab or panel can be built from: the shared context, or a local one of its own (tests).
pub(crate) trait IntoContext {
    fn into_context(self) -> Arc<Context>;
}

impl IntoContext for Arc<Context> {
    fn into_context(self) -> Arc<Context> {
        self
    }
}

impl IntoContext for Arc<ServicesContext> {
    fn into_context(self) -> Arc<Context> {
        Arc::new(Context {
            access: Arc::new(self.local()),
            branch: self.branch.clone(),
            ticket: self.ticket.clone(),
            is_main: self.is_main,
            waker: self.waker.clone(),
        })
    }
}

fn target(branch: &str, is_main: bool, repo: &str, service: &str) -> ServiceTarget {
    ServiceTarget {
        branch: branch.to_string(),
        is_main,
        repo: repo.to_string(),
        service: service.to_string(),
    }
}

fn targets(branch: &str, is_main: bool, config: &Config) -> Vec<ServiceTarget> {
    let workspace = config
        .workspace_services
        .iter()
        .filter(|(_, service)| !service.cmd.is_empty())
        .map(|(name, _)| target(branch, is_main, WORKSPACE_GROUP, name));
    let repos = config.repos.iter().flat_map(|(repo, dir)| {
        dir.services
            .keys()
            .map(move |service| target(branch, is_main, repo, service))
    });
    workspace.chain(repos).collect()
}

/// State shared between the panel, its poller and its action threads, keyed by holder name.
#[derive(Default)]
pub struct Shared {
    pub status: HashMap<String, Status>,
    pub pending: HashMap<String, Action>,
    pub errors: HashMap<String, String>,
    pub toasts: Vec<String>,
    pub shared_running: HashSet<String>,
    /// What each crashed service left behind, read once when it is seen crashed.
    pub crashes: HashMap<String, Crash>,
    /// What a service's tab asked the panel to do (it cannot talk to the workspace itself).
    pub tab_requests: Vec<TabRequest>,
    drawn_at: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabRequest {
    Fix(ServiceTarget),
    /// Open the service in the browser, as the panel opens it.
    OpenUrl(ServiceTarget),
    Copy(String),
    OpenFile(std::path::PathBuf),
    /// The service's menu, under the tab's "..." button.
    Menu(ServiceTarget),
    /// Start, stop or restart a shared container, asking first as the panel does.
    Shared(SharedRun),
    /// A shared container's menu, under its tab's "..." button.
    SharedMenu(String),
}

/// How a service died: the line of its output that most likely says why, how it exited, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crash {
    pub line: Option<String>,
    pub exit: String,
    pub at: Option<std::time::SystemTime>,
}

impl Shared {
    pub fn mark_drawn(&mut self) {
        self.drawn_at = Some(Instant::now());
    }

    fn visible(&self) -> bool {
        self.drawn_at
            .is_some_and(|drawn| drawn.elapsed() < VISIBLE_GRACE)
    }
}

pub struct Model {
    pub context: Arc<Context>,
    pub shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

impl Model {
    pub fn new(context: Context) -> Model {
        let model = Model {
            context: Arc::new(context),
            shared: Arc::new(Mutex::new(Shared::default())),
            stop: Arc::new(AtomicBool::new(false)),
        };
        model.refresh();
        model.spawn_poller();
        model
    }

    /// Re-reads every service's status now; returns whether anything changed.
    pub fn refresh(&self) -> bool {
        refresh(&self.context, &self.shared)
    }

    fn spawn_poller(&self) {
        let (context, shared, stop) =
            (self.context.clone(), self.shared.clone(), self.stop.clone());
        let spawned = std::thread::Builder::new()
            .name("services-poll".into())
            .spawn(move || {
                if refresh_shared(&context, &shared) {
                    (context.waker)();
                }
                let mut polls: u32 = 0;
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL_INTERVAL);
                    polls = polls.wrapping_add(1);
                    if !shared.lock().is_ok_and(|shared| shared.visible()) {
                        continue;
                    }
                    let mut changed = refresh(&context, &shared);
                    if polls.is_multiple_of(SHARED_POLL_EVERY) {
                        changed |= refresh_shared(&context, &shared);
                    }
                    if changed {
                        (context.waker)();
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("services: poller: {error}");
        }
    }

    pub fn status(&self, holder: &str) -> Status {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.status.get(holder).copied())
            .unwrap_or(Status::Stopped)
    }

    pub fn pending(&self, holder: &str) -> Option<Action> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.pending.get(holder).copied())
    }

    pub fn error(&self, holder: &str) -> Option<String> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.errors.get(holder).cloned())
    }

    pub fn run(&self, action: Action, target: ServiceTarget) {
        run_action(&self.context, &self.shared, action, target);
    }

    pub fn take_tab_requests(&self) -> Vec<TabRequest> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.tab_requests))
            .unwrap_or_default()
    }

    pub fn crash(&self, holder: &str) -> Option<Crash> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.crashes.get(holder).cloned())
    }

    pub fn shared_running(&self, name: &str) -> bool {
        self.shared
            .lock()
            .is_ok_and(|shared| shared.shared_running.contains(name))
    }

    /// Like `run`, for shared containers (Docker can take a while to pull and start them).
    pub fn run_shared(&self, run: SharedRun) {
        let Some(config) = self.context.config() else {
            return;
        };
        let key = shared_key(&run.name);
        {
            let Ok(mut shared) = self.shared.lock() else {
                return;
            };
            if shared.pending.contains_key(&key) {
                return;
            }
            shared.pending.insert(key.clone(), run.action);
            shared.errors.remove(&key);
        }
        (self.context.waker)();
        let (context, shared) = (self.context.clone(), self.shared.clone());
        let busy_key = key.clone();
        let spawned = std::thread::Builder::new()
            .name("services-shared".into())
            .spawn(move || {
                let result = context.access.run_shared(&config, &run.name, run.action);
                refresh_shared(&context, &shared);
                if let Ok(mut shared) = shared.lock() {
                    shared.pending.remove(&key);
                    if let Err(error) = result {
                        shared.errors.insert(key, error);
                    }
                }
                (context.waker)();
            });
        if let Err(error) = spawned {
            if let Ok(mut shared) = self.shared.lock() {
                shared.pending.remove(&busy_key);
                shared
                    .toasts
                    .push(format!("Could not run the action: {error}"));
            }
        }
    }

    pub fn take_toasts(&self) -> Vec<String> {
        self.shared
            .lock()
            .map(|mut shared| std::mem::take(&mut shared.toasts))
            .unwrap_or_default()
    }
}

/// Runs `action` on a background thread (stopping waits for the process tree to exit); the row shows progress
/// until it finishes. A second action on a busy service is ignored.
pub(crate) fn run_action(
    context: &Arc<Context>,
    state: &Arc<Mutex<Shared>>,
    action: Action,
    target: ServiceTarget,
) {
    let Some(config) = context.config() else {
        return;
    };
    let holder = context.holder_name(&target);
    {
        let Ok(mut shared) = state.lock() else {
            return;
        };
        if shared.pending.contains_key(&holder) {
            return;
        }
        shared.pending.insert(holder.clone(), action);
        shared.errors.remove(&holder);
    }
    (context.waker)();
    let (context, shared) = (context.clone(), state.clone());
    let busy_key = holder.clone();
    let spawned = std::thread::Builder::new()
        .name("services-action".into())
        .spawn(move || {
            let result = context.access.run(&config, action, &target);
            let status = context.access.status_of(&target);
            if let Ok(mut shared) = shared.lock() {
                shared.pending.remove(&holder);
                shared.status.insert(holder.clone(), status);
                if let Err(error) = result {
                    shared.errors.insert(holder, error);
                }
            }
            (context.waker)();
        });
    if let Err(error) = spawned {
        if let Ok(mut shared) = state.lock() {
            shared.pending.remove(&busy_key);
            shared
                .toasts
                .push(format!("Could not run the action: {error}"));
        }
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn refresh_shared(context: &Context, shared: &Mutex<Shared>) -> bool {
    let has_shared = context
        .config()
        .is_some_and(|config| !config.shared_services.is_empty());
    if !has_shared {
        return false;
    }
    let running = context.access.shared_running();
    let Ok(mut shared) = shared.lock() else {
        return false;
    };
    if shared.shared_running == running {
        return false;
    }
    shared.shared_running = running;
    true
}

fn refresh(context: &Context, shared: &Mutex<Shared>) -> bool {
    let Some(config) = context.config() else {
        return false;
    };
    let fresh: HashMap<String, Status> = context
        .targets(&config)
        .iter()
        .map(|target| {
            (
                context.access.holder_name(target),
                context.access.status_of(target),
            )
        })
        .collect();
    let Ok(mut shared) = shared.lock() else {
        return false;
    };
    if shared.status == fresh {
        return false;
    }
    // A service that came back up no longer has the error of its last failed start.
    let recovered: HashSet<String> = fresh
        .iter()
        .filter(|(_, status)| **status == Status::Running)
        .map(|(holder, _)| holder.clone())
        .collect();
    shared
        .errors
        .retain(|holder, _| !recovered.contains(holder));
    let crashed: Vec<String> = fresh
        .iter()
        .filter(|(_, status)| **status == Status::Crashed)
        .map(|(holder, _)| holder.clone())
        .collect();
    shared.crashes.retain(|holder, _| crashed.contains(holder));
    for holder in crashed {
        if let std::collections::hash_map::Entry::Vacant(slot) = shared.crashes.entry(holder) {
            if let Some(crash) = context.access.crash(slot.key()) {
                slot.insert(crash);
            }
        }
    }
    shared.status = fresh;
    true
}

/// A crash as its holder recorded it: the header naming how it exited, and the output it left.
pub(crate) fn crash_from(header: &str, output: &[u8], at: Option<std::time::SystemTime>) -> Crash {
    Crash {
        line: pom_services::crash_line(output),
        exit: pom_services::crash_exit(header),
        at,
    }
}
