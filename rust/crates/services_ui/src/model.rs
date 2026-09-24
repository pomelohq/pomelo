//! What the panel shows, independent of drawing: the workspace's services grouped by repo, their live
//! status, and the actions in flight. Status comes from a background poller and action threads.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_services::{ServiceRunner, ServiceTarget};

/// The project's config, replaced in place when `pom.yml` changes so every panel sees the new one.
pub type SharedConfig = Arc<RwLock<Option<Arc<Config>>>>;

pub(crate) const WORKSPACE_GROUP: &str = "_ws";
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Polling pauses once the panel has not been drawn for this long (hidden, or another workspace).
const VISIBLE_GRACE: Duration = Duration::from_secs(3);

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

/// One workspace of one project: where its services live and how they run.
pub struct ServicesContext {
    pub runner: Arc<ServiceRunner>,
    pub config: SharedConfig,
    pub branch: String,
    pub is_main: bool,
    pub waker: Arc<dyn Fn() + Send + Sync>,
}

impl ServicesContext {
    pub fn config(&self) -> Option<Arc<Config>> {
        self.config.read().ok().and_then(|config| config.clone())
    }

    pub fn target(&self, repo: &str, service: &str) -> ServiceTarget {
        ServiceTarget {
            branch: self.branch.clone(),
            is_main: self.is_main,
            repo: repo.to_string(),
            service: service.to_string(),
        }
    }

    /// Every service of the workspace: workspace-level ones first, then each repo's in config order.
    pub fn targets(&self, config: &Config) -> Vec<ServiceTarget> {
        let workspace = config
            .workspace_services
            .iter()
            .filter(|(_, service)| !service.cmd.is_empty())
            .map(|(name, _)| self.target(WORKSPACE_GROUP, name));
        let repos = config.repos.iter().flat_map(|(repo, dir)| {
            dir.services
                .keys()
                .map(move |service| self.target(repo, service))
        });
        workspace.chain(repos).collect()
    }

    pub fn status_of(&self, target: &ServiceTarget) -> Status {
        let holders = self.runner.holders();
        let name = self.runner.holder_name(target);
        if holders.holder_alive(&name) {
            Status::Running
        } else if holders.crash_info(&name).is_some_and(|info| info.crashed) {
            Status::Crashed
        } else {
            Status::Stopped
        }
    }
}

/// State shared between the panel, its poller and its action threads, keyed by holder name.
#[derive(Default)]
pub struct Shared {
    pub status: HashMap<String, Status>,
    pub pending: HashMap<String, Action>,
    pub errors: HashMap<String, String>,
    pub toasts: Vec<String>,
    drawn_at: Option<Instant>,
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
    pub context: Arc<ServicesContext>,
    pub shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

impl Model {
    pub fn new(context: ServicesContext) -> Model {
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
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL_INTERVAL);
                    let visible = shared.lock().is_ok_and(|shared| shared.visible());
                    if visible && refresh(&context, &shared) {
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

    /// Runs `action` on a background thread (stopping waits for the process tree to exit); the row shows
    /// progress until it finishes. A second action on a busy service is ignored.
    pub fn run(&self, action: Action, target: ServiceTarget) {
        let Some(config) = self.context.config() else {
            return;
        };
        let holder = self.context.runner.holder_name(&target);
        {
            let Ok(mut shared) = self.shared.lock() else {
                return;
            };
            if shared.pending.contains_key(&holder) {
                return;
            }
            shared.pending.insert(holder.clone(), action);
            shared.errors.remove(&holder);
        }
        (self.context.waker)();
        let (context, shared) = (self.context.clone(), self.shared.clone());
        let busy_key = holder.clone();
        let spawned = std::thread::Builder::new()
            .name("services-action".into())
            .spawn(move || {
                let runner = &context.runner;
                let result = match action {
                    Action::Start => runner.start(&config, &target).map(|_| ()),
                    Action::Restart => runner.restart(&config, &target).map(|_| ()),
                    Action::Relocate => runner.relocate(&config, &target).map(|_| ()),
                    Action::Stop => runner.stop(&target).map_err(Into::into),
                };
                let status = context.status_of(&target);
                if let Ok(mut shared) = shared.lock() {
                    shared.pending.remove(&holder);
                    shared.status.insert(holder.clone(), status);
                    if let Err(error) = result {
                        shared.errors.insert(holder, error.to_string());
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

impl Drop for Model {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn refresh(context: &ServicesContext, shared: &Mutex<Shared>) -> bool {
    let Some(config) = context.config() else {
        return false;
    };
    let fresh: HashMap<String, Status> = context
        .targets(&config)
        .iter()
        .map(|target| {
            (
                context.runner.holder_name(target),
                context.status_of(target),
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
    shared.status = fresh;
    true
}
