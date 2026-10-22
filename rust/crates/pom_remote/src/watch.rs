//! Keeps a project's state fresh while a window shows it, and runs the start/stop/restart and agent actions it
//! is given, off the UI thread. A machine that pushes changes is followed through its pushes, with a full look
//! now and then in case one went missing; any other is asked again every few seconds.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use pom_agent::SessionView;

use crate::server_api::AgentPresence;
use crate::source::{
    last_turn, AgentTurn, Feed, FeedEvent, ProjectSource, ServiceAction, SourceError, StatusReport,
    TransportInfo, Update, WorkspaceList,
};

pub const POLL_EVERY: Duration = Duration::from_millis(2500);
/// With pushes, a full look this often still catches anything a push missed.
pub const SAFETY_REFRESH: Duration = Duration::from_secs(30);
const LOG_LINES: usize = 400;
/// How often the push reader looks up from its wait to notice it should stop.
const FEED_POLL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub poll: Duration,
    pub safety: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            poll: POLL_EVERY,
            safety: SAFETY_REFRESH,
        }
    }
}

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
    pub transport: Option<TransportInfo>,
    pub branch: String,
    pub workspaces: Option<WorkspaceList>,
    pub status: Option<StatusReport>,
    /// Why the status could not be read although the machine answered.
    pub problem: Option<String>,
    pub agents: Vec<SessionView>,
    /// Who drives each agent when it is not the person here, and what it waits to have approved.
    pub presence: Vec<AgentPresence>,
    /// The service whose output is shown, and its last lines.
    pub logs: Option<(String, Result<String, String>)>,
    /// The agent role whose last turn is shown.
    pub turn: Option<(String, Result<Option<AgentTurn>, String>)>,
    /// Targets with a start/stop/restart under way.
    pub busy: Vec<String>,
    /// What the last start/stop/restart said.
    pub message: Option<String>,
    /// The machine pushes its changes right now.
    pub pushing: bool,
    /// Bumped on every change, so a reader can tell it has something new.
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AgentJob {
    Start(String),
    TakeOver(String),
    Answer {
        holder: String,
        request: String,
        allow: bool,
    },
}

#[derive(Default)]
struct Wanted {
    visible: bool,
    branch: String,
    logs: Option<String>,
    turn: Option<String>,
    actions: VecDeque<(ServiceAction, String)>,
    agent_jobs: VecDeque<AgentJob>,
    /// The pushes stopped, and why.
    feed_lost: Option<String>,
    now: bool,
    stopped: bool,
}

struct Shared {
    wanted: Mutex<Wanted>,
    changed: Condvar,
    snapshot: Mutex<Snapshot>,
    /// Agents this window started, for it to open their tabs.
    started: Mutex<Vec<SessionView>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    fn wanted(&self) -> MutexGuard<'_, Wanted> {
        locked(&self.wanted)
    }

    fn snapshot(&self) -> MutexGuard<'_, Snapshot> {
        locked(&self.snapshot)
    }

    fn update(&self, waker: &(dyn Fn() + Send + Sync), change: impl FnOnce(&mut Snapshot)) {
        {
            let mut snapshot = self.snapshot();
            change(&mut snapshot);
            snapshot.generation += 1;
        }
        waker();
    }

    fn poke(&self, edit: impl FnOnce(&mut Wanted)) {
        edit(&mut self.wanted());
        self.changed.notify_all();
    }
}

/// Ends the polling thread once the last handle goes.
struct Stopper(Arc<Shared>);

impl Drop for Stopper {
    fn drop(&mut self) {
        self.0.poke(|wanted| wanted.stopped = true);
    }
}

/// A handle on one project's watching; clones share it.
#[derive(Clone)]
pub struct Watch {
    shared: Arc<Shared>,
    _stopper: Arc<Stopper>,
    location: String,
    source: Arc<dyn ProjectSource>,
}

impl Watch {
    /// Starts hidden: nothing is asked until [`Watch::set_visible`].
    pub fn start(
        source: Arc<dyn ProjectSource>,
        branch: &str,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> Watch {
        Self::start_with(source, branch, waker, Timing::default())
    }

    pub fn start_with(
        source: Arc<dyn ProjectSource>,
        branch: &str,
        waker: Arc<dyn Fn() + Send + Sync>,
        timing: Timing,
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
            started: Mutex::new(Vec::new()),
        });
        let location = source.location();
        let (thread_shared, thread_source) = (shared.clone(), source.clone());
        let spawned = std::thread::Builder::new()
            .name("pom-remote-watch".into())
            .spawn(move || run(&thread_shared, &thread_source, &waker, timing));
        if let Err(error) = spawned {
            shared.snapshot().connection = Connection::Disconnected {
                reason: format!("could not start watching: {error}"),
            };
        }
        Watch {
            _stopper: Arc::new(Stopper(shared.clone())),
            shared,
            location,
            source,
        }
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    /// Terminals of the shown workspace, when the project's machine opens them over a connection.
    pub fn terminals(&self) -> Option<Arc<dyn pom_rpc::pty::PtyProvider>> {
        let branch = self.shared.snapshot().branch.clone();
        (!branch.is_empty())
            .then(|| self.source.pty(&branch))
            .flatten()
    }

    /// Asks only while the window is seen; showing it again looks at once.
    pub fn set_visible(&self, visible: bool) {
        self.shared.poke(|wanted| {
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
                snapshot.presence.clear();
                snapshot.logs = None;
                snapshot.turn = None;
                snapshot.generation += 1;
            }
        }
        self.shared.poke(|wanted| {
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
        self.shared.poke(|wanted| {
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
        self.shared.poke(|wanted| {
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
        self.shared
            .poke(|wanted| wanted.actions.push_back((action, target.to_string())));
    }

    fn agent_job(&self, job: AgentJob) {
        self.shared.poke(|wanted| wanted.agent_jobs.push_back(job));
    }

    /// Starts agent `role` on the machine for the person here; [`Watch::take_started`] hands it over once up.
    pub fn start_agent(&self, role: &str) {
        {
            let mut snapshot = self.shared.snapshot();
            snapshot.busy.push(agent_busy(role));
            snapshot.generation += 1;
        }
        self.agent_job(AgentJob::Start(role.to_string()));
    }

    pub fn take_over(&self, holder: &str) {
        self.agent_job(AgentJob::TakeOver(holder.to_string()));
    }

    pub fn answer(&self, holder: &str, request: &str, allow: bool) {
        self.agent_job(AgentJob::Answer {
            holder: holder.to_string(),
            request: request.to_string(),
            allow,
        });
    }

    /// Agents [`Watch::start_agent`] started since the last call.
    pub fn take_started(&self) -> Vec<SessionView> {
        std::mem::take(&mut *locked(&self.shared.started))
    }

    /// Who drives the agent in `holder`, as last read.
    pub fn presence(&self, holder: &str) -> Option<AgentPresence> {
        self.shared
            .snapshot()
            .presence
            .iter()
            .find(|presence| presence.holder == holder)
            .cloned()
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
        self.shared.poke(|wanted| wanted.now = true);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot().clone()
    }

    pub fn generation(&self) -> u64 {
        self.shared.snapshot().generation
    }
}

fn agent_busy(role: &str) -> String {
    format!("agent:{role}")
}

/// The pushes being read, and what they follow.
struct Live {
    feed: Arc<dyn Feed>,
    stop: Arc<AtomicBool>,
    followed: (String, Option<String>),
}

impl Live {
    fn end(self) {
        self.stop.store(true, Ordering::Release);
    }
}

enum Job {
    Act(ServiceAction, String, String),
    Agent(AgentJob, String),
    Look {
        branch: String,
        logs: Option<String>,
        turn: Option<String>,
    },
    Logs {
        branch: String,
        service: String,
    },
    OpenFeed {
        branch: String,
        turn: Option<String>,
    },
    Follow {
        branch: String,
        turn: Option<String>,
    },
    DropFeed(Option<String>),
}

#[derive(Default)]
struct Clock {
    look: Option<Instant>,
    logs: Option<Instant>,
    feed_tried: Option<Instant>,
}

fn remaining(at: Option<Instant>, every: Duration) -> Duration {
    at.map_or(Duration::ZERO, |at| every.saturating_sub(at.elapsed()))
}

fn next_job(shared: &Shared, live: Option<&Live>, clock: &Clock, timing: Timing) -> Option<Job> {
    let mut wanted = shared.wanted();
    loop {
        if wanted.stopped {
            return None;
        }
        if let Some((action, target)) = wanted.actions.pop_front() {
            return Some(Job::Act(action, target, wanted.branch.clone()));
        }
        if let Some(job) = wanted.agent_jobs.pop_front() {
            return Some(Job::Agent(job, wanted.branch.clone()));
        }
        if let Some(reason) = wanted.feed_lost.take() {
            wanted.now = true;
            return Some(Job::DropFeed(Some(reason)));
        }
        if !wanted.visible && live.is_some() {
            return Some(Job::DropFeed(None));
        }
        let followed = (wanted.branch.clone(), wanted.turn.clone());
        let mut wait = timing.poll;
        if wanted.visible {
            match live {
                None => {
                    let retry = remaining(clock.feed_tried, timing.poll);
                    if retry.is_zero() {
                        return Some(Job::OpenFeed {
                            branch: followed.0,
                            turn: followed.1,
                        });
                    }
                    wait = wait.min(retry);
                }
                Some(live) if live.followed != followed => {
                    return Some(Job::Follow {
                        branch: followed.0,
                        turn: followed.1,
                    })
                }
                Some(_) => {}
            }
            let every = if live.is_some() {
                timing.safety
            } else {
                timing.poll
            };
            let look_in = remaining(clock.look, every);
            if wanted.now || look_in.is_zero() {
                wanted.now = false;
                return Some(Job::Look {
                    branch: followed.0,
                    logs: wanted.logs.clone(),
                    turn: followed.1,
                });
            }
            wait = wait.min(look_in);
            if let (Some(_), Some(service)) = (live, &wanted.logs) {
                let logs_in = remaining(clock.logs, timing.poll);
                if logs_in.is_zero() {
                    return Some(Job::Logs {
                        branch: followed.0,
                        service: service.clone(),
                    });
                }
                wait = wait.min(logs_in);
            }
        }
        wanted = shared
            .changed
            .wait_timeout(wanted, wait.max(Duration::from_millis(10)))
            .map(|(guard, _)| guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner().0);
    }
}

fn run(
    shared: &Arc<Shared>,
    source: &Arc<dyn ProjectSource>,
    waker: &Arc<dyn Fn() + Send + Sync>,
    timing: Timing,
) {
    let mut live: Option<Live> = None;
    let mut clock = Clock::default();
    while let Some(job) = next_job(shared, live.as_ref(), &clock, timing) {
        match job {
            Job::Act(action, target, branch) => act(
                shared,
                source.as_ref(),
                waker.as_ref(),
                action,
                &target,
                &branch,
            ),
            Job::Agent(job, branch) => agent(shared, source.as_ref(), waker.as_ref(), job, &branch),
            Job::Look { branch, logs, turn } => {
                look(shared, source.as_ref(), waker.as_ref(), &branch, logs, turn);
                clock.look = Some(Instant::now());
                clock.logs = clock.look;
            }
            Job::Logs { branch, service } => {
                let result = source.logs(&branch, &service);
                clock.logs = Some(Instant::now());
                shared.update(waker.as_ref(), |snapshot| {
                    if snapshot.branch == branch {
                        snapshot.logs = Some((service, logs_text(result)));
                    }
                });
            }
            Job::OpenFeed { branch, turn } => {
                clock.feed_tried = Some(Instant::now());
                if let Ok(Some(feed)) = source.events(&branch, turn.as_deref()) {
                    live = Some(read_feed(shared, waker, feed, (branch, turn)));
                    let transport = source.transport();
                    shared.update(waker.as_ref(), |snapshot| {
                        snapshot.pushing = true;
                        snapshot.transport = transport;
                    });
                }
            }
            Job::Follow { branch, turn } => {
                let Some(current) = live.as_mut() else {
                    continue;
                };
                current.followed = (branch.clone(), turn.clone());
                if let Err(error) = current.feed.follow(&branch, turn.as_deref()) {
                    shared.poke(|wanted| wanted.feed_lost = Some(error.to_string()));
                }
            }
            Job::DropFeed(reason) => {
                if let Some(current) = live.take() {
                    current.end();
                }
                clock.feed_tried = reason.is_some().then(Instant::now);
                shared.update(waker.as_ref(), |snapshot| snapshot.pushing = false);
            }
        }
    }
    if let Some(current) = live.take() {
        current.end();
    }
}

/// Reads pushes on a thread of their own into the snapshot until told to stop or the pushes end.
fn read_feed(
    shared: &Arc<Shared>,
    waker: &Arc<dyn Fn() + Send + Sync>,
    feed: Arc<dyn Feed>,
    followed: (String, Option<String>),
) -> Live {
    let stop = Arc::new(AtomicBool::new(false));
    let (thread_shared, thread_waker, thread_feed, thread_stop) =
        (shared.clone(), waker.clone(), feed.clone(), stop.clone());
    let spawned = std::thread::Builder::new()
        .name("pom-remote-pushes".into())
        .spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match thread_feed.next(FEED_POLL) {
                    FeedEvent::Update(update) => {
                        apply(&thread_shared, thread_waker.as_ref(), update)
                    }
                    FeedEvent::Idle => {}
                    FeedEvent::Ended(reason) => {
                        if !thread_stop.load(Ordering::Acquire) {
                            thread_shared.poke(|wanted| wanted.feed_lost = Some(reason));
                        }
                        return;
                    }
                }
            }
        });
    if let Err(error) = spawned {
        shared.poke(|wanted| wanted.feed_lost = Some(error.to_string()));
    }
    Live {
        feed,
        stop,
        followed,
    }
}

fn apply(shared: &Shared, waker: &(dyn Fn() + Send + Sync), update: Update) {
    let turn = shared.wanted().turn.clone();
    shared.update(waker, |snapshot| match update {
        Update::Workspaces(update) => {
            snapshot.connection = Connection::Connected;
            snapshot.answered_at = Some(SystemTime::now());
            match (update.list, update.error) {
                (Some(list), _) => snapshot.workspaces = Some(list),
                (None, Some(error)) => snapshot.problem = Some(error),
                (None, None) => {}
            }
        }
        Update::Status(update) if update.branch == snapshot.branch => {
            match (update.report, update.error) {
                (Some(report), _) => {
                    snapshot.status = Some(report);
                    snapshot.problem = None;
                }
                (None, Some(error)) => snapshot.problem = Some(error),
                (None, None) => {}
            }
        }
        Update::Agents(update) if update.branch == snapshot.branch && update.error.is_none() => {
            snapshot.agents = update.sessions;
            snapshot.presence = update.presence;
        }
        Update::Turn(update)
            if update.branch == snapshot.branch && turn.as_deref() == Some(&update.role) =>
        {
            let result = match (update.document, update.error) {
                (Some(document), _) => Ok(last_turn(&document)),
                (None, error) => Err(error.unwrap_or_default()),
            };
            snapshot.turn = Some((update.role, result));
        }
        Update::Status(_) | Update::Agents(_) | Update::Turn(_) => {}
    });
}

fn act(
    shared: &Shared,
    source: &dyn ProjectSource,
    waker: &(dyn Fn() + Send + Sync),
    action: ServiceAction,
    target: &str,
    branch: &str,
) {
    let result = source.act(branch, action, target);
    let transport = source.transport();
    shared.update(waker, |snapshot| {
        snapshot.transport = transport;
        if let Some(at) = snapshot.busy.iter().position(|busy| busy == target) {
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
    shared.poke(|wanted| wanted.now = true);
}

fn agent(
    shared: &Shared,
    source: &dyn ProjectSource,
    waker: &(dyn Fn() + Send + Sync),
    job: AgentJob,
    branch: &str,
) {
    let message = match &job {
        AgentJob::Start(role) => {
            let result = source.start_agent(branch, role);
            let busy = agent_busy(role);
            let message = match &result {
                Ok(view) => format!("started agent {}", view.role),
                Err(error) => format!("could not start agent {role}: {error}"),
            };
            if let Ok(view) = result {
                locked(&shared.started).push(view);
            }
            shared.update(waker, |snapshot| {
                snapshot.busy.retain(|known| *known != busy);
            });
            message
        }
        AgentJob::TakeOver(holder) => match source.take_over_agent(branch, holder) {
            Ok(()) => "the agent is yours".to_string(),
            Err(error) => format!("could not take the agent over: {error}"),
        },
        AgentJob::Answer {
            holder,
            request,
            allow,
        } => match source.answer_agent(branch, holder, request, *allow) {
            Ok(()) if *allow => "allowed".to_string(),
            Ok(()) => "denied".to_string(),
            Err(error) => format!("could not answer the agent: {error}"),
        },
    };
    shared.update(waker, |snapshot| snapshot.message = Some(message));
    shared.poke(|wanted| wanted.now = true);
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn logs_text(result: Result<String, SourceError>) -> Result<String, String> {
    result
        .map(|text| tail(&text, LOG_LINES))
        .map_err(|error| error.to_string())
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
    let transport = source.transport();
    shared.update(waker, |snapshot| {
        snapshot.transport = transport;
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
            snapshot.logs = Some((service, logs_text(result)));
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
        feed: Option<Arc<FakeFeed>>,
    }

    struct FakeFeed {
        events: Mutex<std::sync::mpsc::Receiver<FeedEvent>>,
        followed: Mutex<Vec<String>>,
    }

    impl Feed for FakeFeed {
        fn next(&self, timeout: Duration) -> FeedEvent {
            match locked(&self.events).recv_timeout(timeout) {
                Ok(event) => event,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => FeedEvent::Idle,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    FeedEvent::Ended("gone".into())
                }
            }
        }
        fn follow(&self, branch: &str, _turn: Option<&str>) -> Result<(), SourceError> {
            locked(&self.followed).push(branch.to_string());
            Ok(())
        }
    }

    fn pushing() -> (Arc<Fake>, std::sync::mpsc::Sender<FeedEvent>, Arc<FakeFeed>) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let feed = Arc::new(FakeFeed {
            events: Mutex::new(receiver),
            followed: Mutex::new(Vec::new()),
        });
        let fake = Arc::new(Fake {
            feed: Some(feed.clone()),
            ..Fake::default()
        });
        (fake, sender, feed)
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
        fn events(
            &self,
            _branch: &str,
            _turn: Option<&str>,
        ) -> Result<Option<Arc<dyn Feed>>, SourceError> {
            Ok(self.feed.clone().map(|feed| feed as Arc<dyn Feed>))
        }
    }

    #[test]
    fn pushes_land_in_the_snapshot_and_polling_stops_but_a_safety_look_still_runs() {
        let (fake, pushes, feed) = pushing();
        let timing = Timing {
            poll: Duration::from_millis(50),
            safety: Duration::from_millis(600),
        };
        let watch = Watch::start_with(fake.clone(), "main", Arc::new(|| {}), timing);
        watch.set_visible(true);
        wait_for(&watch, |snapshot| snapshot.pushing);
        let generation = watch.generation();
        pushes
            .send(FeedEvent::Update(Update::Agents(
                crate::server_api::AgentsUpdate {
                    branch: "main".into(),
                    sessions: vec![SessionView {
                        role: "claude".into(),
                        state: "thinking".into(),
                        ..SessionView::default()
                    }],
                    presence: vec![AgentPresence {
                        holder: "ws-myproject-main-claude".into(),
                        driven_by: Some("an orchestrator".into()),
                        pending: None,
                    }],
                    error: None,
                },
            )))
            .expect("push");
        let snapshot = wait_for(&watch, |snapshot| !snapshot.agents.is_empty());
        assert!(
            snapshot.generation > generation,
            "a push bumps the generation"
        );
        assert_eq!(
            watch
                .presence("ws-myproject-main-claude")
                .and_then(|presence| presence.driven_by)
                .as_deref(),
            Some("an orchestrator")
        );
        pushes
            .send(FeedEvent::Update(Update::Agents(
                crate::server_api::AgentsUpdate {
                    branch: "other".into(),
                    ..Default::default()
                },
            )))
            .expect("push");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            watch.snapshot().agents.len(),
            1,
            "another branch's push is dropped"
        );

        let looks = fake.looks.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        let later = fake.looks.load(Ordering::SeqCst);
        // Polling would look about six times meanwhile; at most the safety look may land.
        assert!(
            later <= looks + 1,
            "no polling while pushes come: {looks} then {later}"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while fake.looks.load(Ordering::SeqCst) <= later {
            assert!(Instant::now() < deadline, "no safety look");
            std::thread::sleep(Duration::from_millis(20));
        }

        watch.set_branch("feat-login");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !locked(&feed.followed).contains(&"feat-login".to_string()) {
            assert!(
                Instant::now() < deadline,
                "the pushes never followed the branch"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        drop(pushes);
        wait_for(&watch, |snapshot| !snapshot.pushing);
    }

    #[test]
    fn a_machine_without_pushes_is_asked_again_every_poll() {
        let fake = Arc::new(Fake::default());
        let timing = Timing {
            poll: Duration::from_millis(50),
            safety: Duration::from_secs(60),
        };
        let watch = Watch::start_with(fake.clone(), "main", Arc::new(|| {}), timing);
        watch.set_visible(true);
        let deadline = Instant::now() + Duration::from_secs(5);
        while fake.looks.load(Ordering::SeqCst) < 4 {
            assert!(Instant::now() < deadline, "not polled");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!watch.snapshot().pushing);
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
