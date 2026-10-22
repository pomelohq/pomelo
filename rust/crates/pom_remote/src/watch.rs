//! Keeps a project's state fresh by asking its [`ProjectSource`] again every few seconds while a window shows
//! it, and runs the start/stop/restart it is given, off the UI thread. There is no stream yet: each look is a
//! new request.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use pom_agent::SessionView;

use crate::source::{
    AgentTurn, ProjectSource, ServiceAction, SourceError, StatusReport, WorkspaceList,
};

pub const POLL_EVERY: Duration = Duration::from_millis(2500);
const LOG_LINES: usize = 400;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Connecting,
    Connected,
    /// The last look failed to reach the machine; what was read before stays shown.
    Disconnected {
        reason: String,
    },
}

/// What was last read, for one workspace of the project.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub connection: Connection,
    /// When the machine last answered.
    pub answered_at: Option<SystemTime>,
    pub branch: String,
    pub workspaces: Option<WorkspaceList>,
    pub status: Option<StatusReport>,
    /// Why the status could not be read although the machine answered.
    pub problem: Option<String>,
    pub agents: Vec<SessionView>,
    /// The service whose output is shown, and its last lines.
    pub logs: Option<(String, Result<String, String>)>,
    /// The agent role whose last turn is shown.
    pub turn: Option<(String, Result<Option<AgentTurn>, String>)>,
    /// Targets with a start/stop/restart under way.
    pub busy: Vec<String>,
    /// What the last start/stop/restart said.
    pub message: Option<String>,
    /// Bumped on every change, so a reader can tell it has something new.
    pub generation: u64,
}

#[derive(Default)]
struct Wanted {
    visible: bool,
    branch: String,
    logs: Option<String>,
    turn: Option<String>,
    actions: VecDeque<(ServiceAction, String)>,
    now: bool,
    stopped: bool,
}

struct Shared {
    wanted: Mutex<Wanted>,
    changed: Condvar,
    snapshot: Mutex<Snapshot>,
}

impl Shared {
    fn wanted(&self) -> MutexGuard<'_, Wanted> {
        self.wanted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn snapshot(&self) -> MutexGuard<'_, Snapshot> {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn update(&self, waker: &(dyn Fn() + Send + Sync), change: impl FnOnce(&mut Snapshot)) {
        {
            let mut snapshot = self.snapshot();
            change(&mut snapshot);
            snapshot.generation += 1;
        }
        waker();
    }
}

/// Ends the polling thread once the last handle goes.
struct Stopper(Arc<Shared>);

impl Drop for Stopper {
    fn drop(&mut self) {
        self.0.wanted().stopped = true;
        self.0.changed.notify_all();
    }
}

/// A handle on one project's polling; clones share it.
#[derive(Clone)]
pub struct Watch {
    shared: Arc<Shared>,
    _stopper: Arc<Stopper>,
    location: String,
}

impl Watch {
    /// Starts hidden: nothing is asked until [`Watch::set_visible`].
    pub fn start(
        source: Arc<dyn ProjectSource>,
        branch: &str,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> Watch {
        let shared = Arc::new(Shared {
            wanted: Mutex::new(Wanted {
                branch: branch.to_string(),
                now: true,
                ..Wanted::default()
            }),
            changed: Condvar::new(),
            snapshot: Mutex::new(Snapshot {
                branch: branch.to_string(),
                ..Snapshot::default()
            }),
        });
        let location = source.location();
        let thread_shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("pom-remote-watch".into())
            .spawn(move || run(&thread_shared, source.as_ref(), waker.as_ref()));
        if let Err(error) = spawned {
            shared.snapshot().connection = Connection::Disconnected {
                reason: format!("could not start watching: {error}"),
            };
        }
        Watch {
            _stopper: Arc::new(Stopper(shared.clone())),
            shared,
            location,
        }
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    fn change(&self, edit: impl FnOnce(&mut Wanted)) {
        edit(&mut self.shared.wanted());
        self.shared.changed.notify_all();
    }

    /// Polls only while the window is seen; showing it again looks at once.
    pub fn set_visible(&self, visible: bool) {
        self.change(|wanted| {
            if visible && !wanted.visible {
                wanted.now = true;
            }
            wanted.visible = visible;
        });
    }

    pub fn set_branch(&self, branch: &str) {
        {
            let mut snapshot = self.shared.snapshot();
            if snapshot.branch != branch {
                snapshot.branch = branch.to_string();
                snapshot.status = None;
                snapshot.problem = None;
                snapshot.agents.clear();
                snapshot.logs = None;
                snapshot.turn = None;
                snapshot.generation += 1;
            }
        }
        self.change(|wanted| {
            wanted.branch = branch.to_string();
            wanted.logs = None;
            wanted.turn = None;
            wanted.now = true;
        });
    }

    pub fn show_logs(&self, service: Option<String>) {
        if service.is_none() {
            let mut snapshot = self.shared.snapshot();
            snapshot.logs = None;
            snapshot.generation += 1;
        }
        self.change(|wanted| {
            wanted.now |= service.is_some();
            wanted.logs = service;
        });
    }

    pub fn show_turn(&self, role: Option<String>) {
        if role.is_none() {
            let mut snapshot = self.shared.snapshot();
            snapshot.turn = None;
            snapshot.generation += 1;
        }
        self.change(|wanted| {
            wanted.now |= role.is_some();
            wanted.turn = role;
        });
    }

    pub fn act(&self, action: ServiceAction, target: &str) {
        {
            let mut snapshot = self.shared.snapshot();
            snapshot.busy.push(target.to_string());
            snapshot.generation += 1;
        }
        self.change(|wanted| wanted.actions.push_back((action, target.to_string())));
    }

    /// Looks again now (a Reconnect).
    pub fn refresh_now(&self) {
        {
            let mut snapshot = self.shared.snapshot();
            if matches!(snapshot.connection, Connection::Disconnected { .. }) {
                snapshot.connection = Connection::Connecting;
                snapshot.generation += 1;
            }
        }
        self.change(|wanted| wanted.now = true);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot().clone()
    }

    pub fn generation(&self) -> u64 {
        self.shared.snapshot().generation
    }
}

enum Job {
    Act(ServiceAction, String, String),
    Look {
        branch: String,
        logs: Option<String>,
        turn: Option<String>,
    },
}

fn next_job(shared: &Shared, last_look: Option<Instant>) -> Option<Job> {
    let mut wanted = shared.wanted();
    loop {
        if wanted.stopped {
            return None;
        }
        if let Some((action, target)) = wanted.actions.pop_front() {
            return Some(Job::Act(action, target, wanted.branch.clone()));
        }
        let due = last_look.is_none_or(|at| at.elapsed() >= POLL_EVERY);
        if wanted.visible && (wanted.now || due) {
            wanted.now = false;
            return Some(Job::Look {
                branch: wanted.branch.clone(),
                logs: wanted.logs.clone(),
                turn: wanted.turn.clone(),
            });
        }
        let wait = match (wanted.visible, last_look) {
            (true, Some(at)) => POLL_EVERY.saturating_sub(at.elapsed()),
            _ => POLL_EVERY,
        };
        wanted = shared
            .changed
            .wait_timeout(wanted, wait.max(Duration::from_millis(10)))
            .map(|(guard, _)| guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner().0);
    }
}

fn run(shared: &Shared, source: &dyn ProjectSource, waker: &(dyn Fn() + Send + Sync)) {
    let mut last_look: Option<Instant> = None;
    while let Some(job) = next_job(shared, last_look) {
        match job {
            Job::Act(action, target, branch) => {
                let result = source.act(&branch, action, &target);
                shared.update(waker, |snapshot| {
                    if let Some(at) = snapshot.busy.iter().position(|busy| *busy == target) {
                        snapshot.busy.remove(at);
                    }
                    snapshot.message = Some(match &result {
                        Ok(said) if said.trim().is_empty() => {
                            format!("{} {target}: nothing to do", action.verb())
                        }
                        Ok(said) => said.trim().to_string(),
                        Err(error) => format!("could not {} {target}: {error}", action.verb()),
                    });
                    if let Err(SourceError::Unreachable(reason)) = &result {
                        snapshot.connection = Connection::Disconnected {
                            reason: reason.clone(),
                        };
                    }
                });
                shared.wanted().now = true;
            }
            Job::Look { branch, logs, turn } => {
                look(shared, source, waker, &branch, logs, turn);
                last_look = Some(Instant::now());
            }
        }
    }
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn look(
    shared: &Shared,
    source: &dyn ProjectSource,
    waker: &(dyn Fn() + Send + Sync),
    branch: &str,
    logs: Option<String>,
    turn: Option<String>,
) {
    let mut unreachable: Option<String> = None;
    let mut note = |error: &SourceError| {
        if let SourceError::Unreachable(reason) = error {
            unreachable.get_or_insert_with(|| reason.clone());
        }
    };
    let workspaces = source.workspaces();
    if let Err(error) = &workspaces {
        note(error);
    }
    // One unreachable answer is enough: asking the rest would only wait out the same timeout again. Before
    // a workspace is picked (the first look learns which is main) only the list is asked for.
    let ask_more = !matches!(workspaces, Err(SourceError::Unreachable(_))) && !branch.is_empty();
    let status = ask_more.then(|| source.status(branch));
    let agents = ask_more.then(|| source.agents(branch));
    let logs = logs
        .filter(|_| ask_more)
        .map(|service| (service.clone(), source.logs(branch, &service)));
    let turn = turn
        .filter(|_| ask_more)
        .map(|role| (role.clone(), source.agent_turn(branch, &role)));
    for error in [
        status.as_ref().and_then(|result| result.as_ref().err()),
        agents.as_ref().and_then(|result| result.as_ref().err()),
    ]
    .into_iter()
    .flatten()
    {
        note(error);
    }
    shared.update(waker, |snapshot| {
        if snapshot.branch != branch {
            return;
        }
        match unreachable {
            Some(reason) => {
                snapshot.connection = Connection::Disconnected { reason };
                return;
            }
            None => {
                snapshot.connection = Connection::Connected;
                snapshot.answered_at = Some(SystemTime::now());
            }
        }
        match workspaces {
            Ok(list) => snapshot.workspaces = Some(list),
            Err(error) => snapshot.problem = Some(error.to_string()),
        }
        match status {
            Some(Ok(report)) => {
                snapshot.status = Some(report);
                snapshot.problem = None;
            }
            Some(Err(error)) => snapshot.problem = Some(error.to_string()),
            None => {}
        }
        if let Some(Ok(agents)) = agents {
            snapshot.agents = agents;
        }
        if let Some((service, result)) = logs {
            snapshot.logs = Some((
                service,
                result
                    .map(|text| tail(&text, LOG_LINES))
                    .map_err(|error| error.to_string()),
            ));
        }
        if let Some((role, result)) = turn {
            snapshot.turn = Some((role, result.map_err(|error| error.to_string())));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::WorkspaceSummary;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Fake {
        down: AtomicBool,
        looks: AtomicUsize,
        acted: Mutex<Vec<String>>,
    }

    impl ProjectSource for Fake {
        fn location(&self) -> String {
            "dev (dev@dev.example.net)".into()
        }
        fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
            self.looks.fetch_add(1, Ordering::SeqCst);
            if self.down.load(Ordering::SeqCst) {
                return Err(SourceError::Unreachable("Connection refused".into()));
            }
            Ok(WorkspaceList {
                project: "myproject".into(),
                workspaces: vec![WorkspaceSummary {
                    branch: "main".into(),
                    is_main: true,
                    ..WorkspaceSummary::default()
                }],
                ..WorkspaceList::default()
            })
        }
        fn status(&self, branch: &str) -> Result<StatusReport, SourceError> {
            Ok(StatusReport {
                workspace: branch.into(),
                ..StatusReport::default()
            })
        }
        fn logs(&self, _branch: &str, service: &str) -> Result<String, SourceError> {
            Ok((0..1000)
                .map(|line| format!("{service} {line}\n"))
                .collect())
        }
        fn agents(&self, _branch: &str) -> Result<Vec<SessionView>, SourceError> {
            Ok(Vec::new())
        }
        fn agent_turn(&self, _: &str, _: &str) -> Result<Option<AgentTurn>, SourceError> {
            Ok(None)
        }
        fn act(
            &self,
            branch: &str,
            action: ServiceAction,
            target: &str,
        ) -> Result<String, SourceError> {
            self.acted
                .lock()
                .map_err(|_| SourceError::Failed("lock".into()))?
                .push(format!("{branch} {} {target}", action.verb()));
            Ok(format!("started {target}"))
        }
    }

    fn wait_for(watch: &Watch, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = watch.snapshot();
            if done(&snapshot) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "timed out at {snapshot:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn nothing_is_asked_while_hidden_and_a_shown_window_looks_at_once() {
        let fake = Arc::new(Fake::default());
        let watch = Watch::start(fake.clone(), "main", Arc::new(|| {}));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(fake.looks.load(Ordering::SeqCst), 0);
        watch.set_visible(true);
        let snapshot = wait_for(&watch, |snapshot| {
            snapshot.connection == Connection::Connected
        });
        assert_eq!(
            snapshot.workspaces.map(|list| list.project).as_deref(),
            Some("myproject")
        );
        watch.set_visible(false);
        let looks = fake.looks.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(fake.looks.load(Ordering::SeqCst), looks);
    }

    #[test]
    fn losing_the_machine_keeps_what_was_read_and_a_retry_reconnects() {
        let fake = Arc::new(Fake::default());
        let watch = Watch::start(fake.clone(), "main", Arc::new(|| {}));
        watch.set_visible(true);
        wait_for(&watch, |snapshot| {
            snapshot.connection == Connection::Connected
        });
        fake.down.store(true, Ordering::SeqCst);
        watch.refresh_now();
        let down = wait_for(&watch, |snapshot| {
            matches!(snapshot.connection, Connection::Disconnected { .. })
        });
        assert!(down.workspaces.is_some());
        fake.down.store(false, Ordering::SeqCst);
        watch.refresh_now();
        wait_for(&watch, |snapshot| {
            snapshot.connection == Connection::Connected
        });
    }

    #[test]
    fn actions_run_on_the_shown_workspace_and_logs_keep_their_tail() {
        let fake = Arc::new(Fake::default());
        let watch = Watch::start(fake.clone(), "main", Arc::new(|| {}));
        watch.set_visible(true);
        watch.set_branch("feat-login");
        watch.act(ServiceAction::Start, "api/web");
        watch.show_logs(Some("api/web".into()));
        let snapshot = wait_for(&watch, |snapshot| {
            snapshot.message.is_some() && snapshot.logs.is_some() && snapshot.busy.is_empty()
        });
        assert_eq!(snapshot.message.as_deref(), Some("started api/web"));
        let acted = fake
            .acted
            .lock()
            .map(|acted| acted.clone())
            .unwrap_or_default();
        assert_eq!(acted, vec!["feat-login start api/web".to_string()]);
        let (service, logs) = snapshot.logs.expect("logs");
        assert_eq!(service, "api/web");
        let logs = logs.expect("text");
        assert_eq!(logs.lines().count(), LOG_LINES);
        assert!(logs.ends_with("api/web 999"));
    }
}
