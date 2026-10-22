//! Creations and deletions run one at a time on a background thread; the WORKSPACES panel shows each as a
//! card, and a failed one stays until retried (from its failed stage) or dismissed.

use std::sync::{Arc, Mutex, MutexGuard};

use std::time::Duration;

use pom_config::Config;
use pom_paths::StateDir;
use pom_remote::server_api::{progress_kind, JobRequest, Progress, EXIT_CANCELLED};
use pom_remote::{JobEvent, ProjectSource, SourceError};
use pom_services::ServiceRunner;
use pom_sync::RepoState;
use pom_workspace::{CreateRequest, DeleteRequest, Event, PrepareRequest, WorkspaceContext};
use workspace::{OpStatus, StageState, WorkspaceOp};

/// What an operation works against, captured when it is queued.
#[derive(Clone)]
pub struct OpContext {
    pub config: Arc<Config>,
    pub runner: Arc<ServiceRunner>,
    pub state: StateDir,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpKind {
    Create {
        request: CreateRequest,
        display_name: String,
    },
    Delete(DeleteRequest),
    /// Pull every repo of main from origin and migrate what moved.
    RefreshMain,
    /// Reset main's databases, migrate and seed.
    PrepareMain(PrepareRequest),
    /// Check out repos the config gained into an existing workspace; its row stays while this runs.
    AddRepos(CreateRequest),
    /// A job run on the project's host.
    Remote(JobRequest),
}

/// How long a remote job's progress may stay away (the host or the connection gone) before its card fails.
const RECONNECT_TRIES: u32 = 15;
const RECONNECT_WAIT: Duration = Duration::from_secs(2);
const FOLLOW_POLL: Duration = Duration::from_millis(250);

/// Where an operation runs: here with the project's context, or on its host through its source.
#[derive(Clone)]
enum Place {
    Local(OpContext),
    Remote(Arc<dyn ProjectSource>),
}

/// An operation that ended, for the app to follow up (switch to a new workspace, show warnings, rescan).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub title: String,
    pub branch: String,
    pub created: bool,
    pub ok: bool,
    pub warnings: Vec<String>,
}

struct Op {
    kind: OpKind,
    place: Place,
    view: WorkspaceOp,
    /// The host's id of a remote job once it started.
    job: Option<String>,
    cancel: bool,
}

#[derive(Default)]
struct Queue {
    ops: Vec<Op>,
    next_id: u64,
    worker: bool,
    finished: Vec<Finished>,
}

#[derive(Clone)]
pub struct OpQueue {
    queue: Arc<Mutex<Queue>>,
    waker: Arc<dyn Fn() + Send + Sync>,
}

/// Folds one repo's refresh state into the card, whose stages are the repos.
pub fn apply_repo_state(view: &mut WorkspaceOp, repo: &str, state: &RepoState) {
    let stage = match state {
        RepoState::Pending => StageState::Pending,
        RepoState::Pulling | RepoState::Migrating => StageState::Running,
        RepoState::NoChange | RepoState::Updated => StageState::Done,
        RepoState::Skipped(_) => StageState::Skipped,
        RepoState::Failed(_) => StageState::Failed,
    };
    let label = match state {
        RepoState::Skipped(why) => format!("{repo}: skipped ({why})"),
        RepoState::NoChange => format!("{repo}: up to date"),
        RepoState::Updated => format!("{repo}: updated"),
        RepoState::Migrating => format!("{repo}: migrating"),
        _ => repo.to_string(),
    };
    view.status = OpStatus::Running;
    let at = view
        .stages
        .iter()
        .position(|(name, _)| name == repo || name.starts_with(&format!("{repo}: ")));
    match at {
        Some(at) => view.stages[at] = (label, stage),
        None => view.stages.push((label, stage)),
    }
    if let RepoState::Failed(error) = state {
        view.detail = format!("{repo}: {}", error.lines().next().unwrap_or_default());
    }
}

/// Folds one pipeline event into the card.
pub fn apply_event(view: &mut WorkspaceOp, event: &Event) {
    let mut set = |index: usize, state: StageState| {
        if let Some((_, stage)) = view.stages.get_mut(index) {
            *stage = state;
        }
    };
    match event {
        Event::Started { stages, .. } => {
            view.stages = stages
                .iter()
                .map(|label| (label.to_string(), StageState::Pending))
                .collect();
            view.status = OpStatus::Running;
            view.error.clear();
        }
        Event::StageStarted { index } => {
            set(*index, StageState::Running);
            view.detail.clear();
        }
        Event::Progress { detail, .. } => view.detail = detail.clone(),
        Event::Warning { detail, .. } => {
            view.detail = detail.lines().next().unwrap_or_default().to_string();
        }
        Event::StageCompleted { index } => set(*index, StageState::Done),
        Event::StageSkipped { index } => set(*index, StageState::Skipped),
        Event::Completed { .. } => view.status = OpStatus::Done,
        Event::Failed { index, error } => {
            set(*index, StageState::Failed);
            view.status = OpStatus::Failed;
            view.error = error.lines().next().unwrap_or_default().to_string();
            view.log = error.clone();
        }
    }
}

/// Folds one event of a remote job into the card.
pub fn apply_progress(view: &mut WorkspaceOp, progress: &Progress) {
    let mut set = |stage: Option<usize>, state: StageState| {
        if let Some((_, current)) = stage.and_then(|index| view.stages.get_mut(index)) {
            *current = state;
        }
    };
    match progress.kind.as_str() {
        progress_kind::STAGES => {
            view.stages = progress
                .stages
                .iter()
                .map(|label| (label.clone(), StageState::Pending))
                .collect();
            view.status = OpStatus::Running;
            view.error.clear();
        }
        progress_kind::STAGE_STARTED => {
            set(progress.stage, StageState::Running);
            view.status = OpStatus::Running;
            view.detail.clear();
        }
        progress_kind::STAGE_DONE => set(progress.stage, StageState::Done),
        progress_kind::STAGE_SKIPPED => set(progress.stage, StageState::Skipped),
        progress_kind::LINE | progress_kind::WARNING => {
            view.detail = progress.line.lines().next().unwrap_or_default().to_string();
        }
        progress_kind::FAILED => {
            set(progress.stage, StageState::Failed);
            view.status = OpStatus::Failed;
            view.error = progress.line.lines().next().unwrap_or_default().to_string();
            view.log = progress.line.clone();
        }
        progress_kind::DONE => match progress.exit {
            Some(0) => view.status = OpStatus::Done,
            exit => {
                view.status = OpStatus::Failed;
                if view.error.is_empty() {
                    view.error = match exit {
                        Some(EXIT_CANCELLED) => "cancelled".to_string(),
                        Some(code) => format!("exited {code}"),
                        None => "ended without an exit".to_string(),
                    };
                }
            }
        },
        _ => {}
    }
}

/// What skipping a failed stage leaves for later, for the stages a run can do without.
fn skip_note(kind: &OpKind, stage: usize) -> &'static str {
    match (kind, stage) {
        (OpKind::Create { .. } | OpKind::AddRepos(_), 2) => {
            "no shared services or databases until they start"
        }
        (OpKind::Delete(_), 0) => "its services may keep running",
        (OpKind::Delete(_), 2) => "pre-delete commands won't run",
        _ => "",
    }
}

/// The checkout a failure names (`web: ...`), in the workspace when it has one there, else main's; for a
/// branch checked out elsewhere, that other checkout.
fn fix_dir(kind: &OpKind, context: &OpContext, error: &str) -> String {
    if let Some(elsewhere) = pom_workspace::checked_out_elsewhere(error) {
        return elsewhere.to_string_lossy().into_owned();
    }
    let Some(repo) = error
        .split(':')
        .next()
        .map(str::trim)
        .filter(|repo| context.config.repos.contains_key(*repo))
    else {
        return String::new();
    };
    let root = context.runner.project_root();
    let main_branch = context.config.global_default_branch();
    let main = pom_layout::repo_worktree(root, repo, main_branch, true);
    let branch = match kind {
        OpKind::Create { request, .. } | OpKind::AddRepos(request) => Some(&request.branch),
        OpKind::Delete(request) => Some(&request.branch),
        OpKind::RefreshMain | OpKind::PrepareMain(_) | OpKind::Remote(_) => None,
    };
    let own = branch
        .map(|branch| pom_layout::workspace_folder(root, branch).join(repo))
        .filter(|dir| dir.is_dir());
    own.unwrap_or(main).to_string_lossy().into_owned()
}

impl OpQueue {
    pub fn new(waker: Arc<dyn Fn() + Send + Sync>) -> OpQueue {
        OpQueue {
            queue: Arc::new(Mutex::new(Queue::default())),
            waker,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Queues an operation; `title` is what its card says.
    pub fn enqueue(&self, kind: OpKind, title: String, context: OpContext) {
        self.push(kind, title, Place::Local(context));
    }

    /// Queues a job to run on the project's host, its card fed by the host's progress.
    pub fn enqueue_remote(
        &self,
        request: JobRequest,
        title: String,
        source: Arc<dyn ProjectSource>,
    ) {
        self.push(OpKind::Remote(request), title, Place::Remote(source));
    }

    fn push(&self, kind: OpKind, title: String, place: Place) {
        // Main's own operations leave its row in place; a created or deleted workspace's row hides meanwhile.
        let branch = match &kind {
            OpKind::Create { request, .. } => request.branch.clone(),
            OpKind::Delete(request) => request.branch.clone(),
            OpKind::Remote(JobRequest::Create(args)) => args.branch.clone(),
            OpKind::RefreshMain
            | OpKind::PrepareMain(_)
            | OpKind::AddRepos(_)
            | OpKind::Remote(_) => String::new(),
        };
        let remote = matches!(place, Place::Remote(_));
        let quiet = matches!(kind, OpKind::RefreshMain);
        {
            let mut queue = self.lock();
            if quiet {
                // A fresh run supersedes the last failed one still shown on main.
                queue
                    .ops
                    .retain(|op| !(op.view.quiet && op.view.status == OpStatus::Failed));
            }
            queue.next_id += 1;
            let id = queue.next_id;
            queue.ops.push(Op {
                kind,
                place,
                job: None,
                cancel: false,
                view: WorkspaceOp {
                    id,
                    branch,
                    title,
                    status: OpStatus::Queued,
                    stages: Vec::new(),
                    detail: String::new(),
                    error: String::new(),
                    retryable: true,
                    quiet,
                    remote,
                    ..Default::default()
                },
            });
        }
        self.start_worker();
        (self.waker)();
    }

    /// Runs a failed operation again from the stage it failed at.
    pub fn retry(&self, id: u64) {
        {
            let mut queue = self.lock();
            let Some(op) = queue.ops.iter_mut().find(|op| op.view.id == id) else {
                return;
            };
            // A remote job shows its failure a moment before its last event ends the follow.
            if op.view.status != OpStatus::Failed || op.job.is_some() {
                return;
            }
            let failed = op
                .view
                .stages
                .iter()
                .position(|(_, state)| *state == StageState::Failed)
                .unwrap_or(0);
            match &mut op.kind {
                OpKind::Create { request, .. } => request.from_stage = failed,
                OpKind::Delete(request) => request.from_stage = failed,
                OpKind::AddRepos(request) => request.from_stage = failed,
                OpKind::Remote(JobRequest::Create(args)) => args.from_stage = failed,
                OpKind::RefreshMain | OpKind::PrepareMain(_) | OpKind::Remote(_) => {}
            }
            op.job = None;
            op.cancel = false;
            op.view.stages.clear();
            op.view.status = OpStatus::Queued;
            op.view.error.clear();
            op.view.log.clear();
            op.view.skip.clear();
        }
        self.start_worker();
        (self.waker)();
    }

    /// Resumes a failed operation past its failed stage, when that stage can be done without.
    pub fn skip(&self, id: u64) {
        {
            let mut queue = self.lock();
            let Some(op) = queue.ops.iter_mut().find(|op| op.view.id == id) else {
                return;
            };
            if op.view.status != OpStatus::Failed || op.view.skip.is_empty() {
                return;
            }
            let Some(failed) = op
                .view
                .stages
                .iter()
                .position(|(_, state)| *state == StageState::Failed)
            else {
                return;
            };
            match &mut op.kind {
                OpKind::Create { request, .. } | OpKind::AddRepos(request) => {
                    request.from_stage = failed + 1
                }
                OpKind::Delete(request) => request.from_stage = failed + 1,
                OpKind::RefreshMain | OpKind::PrepareMain(_) | OpKind::Remote(_) => return,
            }
            op.view.stages.clear();
            op.view.status = OpStatus::Queued;
            op.view.error.clear();
            op.view.log.clear();
            op.view.skip.clear();
        }
        self.start_worker();
        (self.waker)();
    }

    /// Drops an operation that is still waiting its turn, or asks a running remote job to stop.
    pub fn cancel(&self, id: u64) {
        {
            let mut queue = self.lock();
            queue
                .ops
                .retain(|op| op.view.id != id || op.view.status != OpStatus::Queued);
            if let Some(op) = queue.ops.iter_mut().find(|op| {
                op.view.id == id && op.view.remote && op.view.status == OpStatus::Running
            }) {
                op.cancel = true;
                op.view.detail = "Cancelling...".to_string();
            }
        }
        (self.waker)();
    }

    pub fn dismiss(&self, id: u64) {
        self.lock()
            .ops
            .retain(|op| op.view.id != id || op.view.status != OpStatus::Failed);
        (self.waker)();
    }

    /// The cards to show, oldest first.
    pub fn snapshot(&self) -> Vec<WorkspaceOp> {
        self.lock().ops.iter().map(|op| op.view.clone()).collect()
    }

    pub fn take_finished(&self) -> Vec<Finished> {
        std::mem::take(&mut self.lock().finished)
    }

    pub fn busy(&self) -> bool {
        self.lock()
            .ops
            .iter()
            .any(|op| matches!(op.view.status, OpStatus::Queued | OpStatus::Running))
    }

    fn start_worker(&self) {
        {
            let mut queue = self.lock();
            if queue.worker {
                return;
            }
            queue.worker = true;
        }
        let this = self.clone();
        std::thread::spawn(move || this.work());
    }

    fn work(&self) {
        loop {
            let next = {
                let mut queue = self.lock();
                let next = queue
                    .ops
                    .iter_mut()
                    .find(|op| op.view.status == OpStatus::Queued)
                    .map(|op| {
                        op.view.status = OpStatus::Running;
                        (op.view.id, op.kind.clone(), op.place.clone())
                    });
                if next.is_none() {
                    queue.worker = false;
                }
                next
            };
            let Some((id, kind, place)) = next else {
                (self.waker)();
                return;
            };
            (self.waker)();
            match (place, &kind) {
                (Place::Local(context), _) => self.run(id, &kind, &context),
                (Place::Remote(source), OpKind::Remote(request)) => {
                    self.run_remote(id, request, &source)
                }
                (Place::Remote(_), _) => self.finish_remote(id, "", false, Err("not a job".into())),
            }
        }
    }

    fn run(&self, id: u64, kind: &OpKind, context: &OpContext) {
        let workspace_context = WorkspaceContext {
            config: &context.config,
            runner: &context.runner,
            state: &context.state,
        };
        let sink = |event: Event| {
            if let Some(op) = self.lock().ops.iter_mut().find(|op| op.view.id == id) {
                apply_event(&mut op.view, &event);
            }
            (self.waker)();
        };
        if let OpKind::RefreshMain = kind {
            self.run_refresh(id, context);
            return;
        }
        let (branch, created, result) = match kind {
            OpKind::Create {
                request,
                display_name,
            } => {
                let result = pom_workspace::create(&workspace_context, request, &sink);
                if result.is_ok() && !display_name.is_empty() {
                    save_display_name(context, &request.branch, display_name);
                }
                (request.branch.clone(), true, result)
            }
            OpKind::Delete(request) => (
                request.branch.clone(),
                false,
                pom_workspace::delete(&workspace_context, request, &sink),
            ),
            OpKind::PrepareMain(request) => (
                context.config.global_default_branch().to_string(),
                false,
                pom_workspace::prepare_main(&workspace_context, request, &sink),
            ),
            OpKind::AddRepos(request) => (
                request.branch.clone(),
                false,
                pom_workspace::create(&workspace_context, request, &sink),
            ),
            OpKind::RefreshMain | OpKind::Remote(_) => return,
        };
        let title = self.title_of(id);
        let mut queue = self.lock();
        let (ok, warnings) = match result {
            Ok(outcome) => {
                queue.ops.retain(|op| op.view.id != id);
                (true, outcome.warnings)
            }
            Err(error) => {
                if let Some(op) = queue.ops.iter_mut().find(|op| op.view.id == id) {
                    op.view.fix_dir = fix_dir(kind, context, &error.message);
                    op.view.skip = skip_note(kind, error.stage).to_string();
                }
                (false, Vec::new())
            }
        };
        queue.finished.push(Finished {
            title,
            branch,
            created,
            ok,
            warnings,
        });
    }

    fn run_remote(&self, id: u64, request: &JobRequest, source: &Arc<dyn ProjectSource>) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let op_id = format!("app-{}-{nanos:x}-{id}", std::process::id());
        let result = match source.start_job(request, &op_id) {
            Ok(job) => {
                if let Some(op) = self.lock().ops.iter_mut().find(|op| op.view.id == id) {
                    op.job = Some(job.clone());
                }
                self.follow_remote(id, &job, source.as_ref())
            }
            Err(error) => Err(error.to_string()),
        };
        let created = matches!(request, JobRequest::Create(_));
        self.finish_remote(id, request.branch(), created, result);
    }

    fn cancel_asked(&self, id: u64) -> bool {
        self.lock()
            .ops
            .iter()
            .any(|op| op.view.id == id && op.cancel)
    }

    /// Feeds the card from the job's progress until it ends; a lost connection follows again from the last
    /// event seen. The warnings on success, or why it failed.
    fn follow_remote(
        &self,
        id: u64,
        job: &str,
        source: &dyn ProjectSource,
    ) -> Result<Vec<String>, String> {
        let mut next = 0;
        let mut warnings = Vec::new();
        let mut lost = 0;
        let mut cancel_sent = false;
        let mut last_error: String;
        loop {
            match source.follow_job(job, next) {
                Ok(feed) => loop {
                    if !cancel_sent && self.cancel_asked(id) {
                        cancel_sent = true;
                        if let Err(error) = source.cancel_job(job) {
                            eprintln!("cancel {job}: {error}");
                        }
                    }
                    match feed.next(FOLLOW_POLL) {
                        JobEvent::Progress(progress) => {
                            lost = 0;
                            next = progress.seq + 1;
                            if progress.kind == progress_kind::WARNING {
                                warnings.push(progress.line.clone());
                            }
                            if let Some(op) = self.lock().ops.iter_mut().find(|op| op.view.id == id)
                            {
                                apply_progress(&mut op.view, &progress);
                            }
                            (self.waker)();
                            if progress.done {
                                return match progress.exit {
                                    Some(0) => Ok(warnings),
                                    Some(EXIT_CANCELLED) => Err("cancelled".into()),
                                    Some(code) => Err(format!("exited {code}")),
                                    None => Err("ended without an exit".into()),
                                };
                            }
                        }
                        JobEvent::Idle => {}
                        JobEvent::Ended(reason) => {
                            last_error = reason;
                            break;
                        }
                    }
                },
                Err(SourceError::Failed(message)) => return Err(message),
                Err(SourceError::Unreachable(reason)) => last_error = reason,
            }
            // The session coming back on its own is no lost try: the server keeps the job's events meanwhile.
            let reconnecting = source.link().is_some_and(|report| {
                matches!(report.state, pom_remote::LinkState::Reconnecting { .. })
            });
            if !reconnecting {
                lost += 1;
            }
            if lost > RECONNECT_TRIES {
                return Err(format!("lost the host: {last_error}"));
            }
            std::thread::sleep(RECONNECT_WAIT);
        }
    }

    fn finish_remote(
        &self,
        id: u64,
        branch: &str,
        created: bool,
        result: Result<Vec<String>, String>,
    ) {
        let title = self.title_of(id);
        let mut queue = self.lock();
        let (ok, warnings) = match result {
            Ok(warnings) => {
                queue.ops.retain(|op| op.view.id != id);
                (true, warnings)
            }
            Err(error) => {
                let shown = queue
                    .ops
                    .iter()
                    .any(|op| op.view.id == id && op.view.status == OpStatus::Failed);
                if !shown {
                    fail(&mut queue, id, &error);
                }
                if let Some(op) = queue.ops.iter_mut().find(|op| op.view.id == id) {
                    op.job = None;
                }
                (false, Vec::new())
            }
        };
        queue.finished.push(Finished {
            title,
            branch: branch.to_string(),
            created,
            ok,
            warnings,
        });
        drop(queue);
        (self.waker)();
    }

    fn title_of(&self, id: u64) -> String {
        self.lock()
            .ops
            .iter()
            .find(|op| op.view.id == id)
            .map(|op| op.view.title.clone())
            .unwrap_or_default()
    }

    fn run_refresh(&self, id: u64, context: &OpContext) {
        let progress = |repo: &str, state: &RepoState| {
            if let Some(op) = self.lock().ops.iter_mut().find(|op| op.view.id == id) {
                apply_repo_state(&mut op.view, repo, state);
            }
            (self.waker)();
        };
        let lock_dir = std::path::Path::new(pom_lock::DEFAULT_LOCK_DIR);
        let result = pom_sync::refresh_main(
            &pom_sync::RefreshContext {
                config: &context.config,
                runner: &context.runner,
                lock_dir,
            },
            &progress,
        );
        let title = self.title_of(id);
        let mut queue = self.lock();
        let (ok, warnings) = match result {
            Ok(repos) => {
                let failures: Vec<String> = repos
                    .iter()
                    .filter_map(|(repo, state)| match state {
                        RepoState::Failed(error) => Some(format!("{repo}: {error}")),
                        _ => None,
                    })
                    .collect();
                let skipped = repos.iter().filter_map(|(repo, state)| match state {
                    RepoState::Skipped(why) => Some(format!("{repo}: skipped ({why})")),
                    _ => None,
                });
                let warnings: Vec<String> = failures.iter().cloned().chain(skipped).collect();
                match failures.first() {
                    None => {
                        queue.ops.retain(|op| op.view.id != id);
                        (true, warnings)
                    }
                    Some(first) => {
                        fail(&mut queue, id, first);
                        if let Some(op) = queue.ops.iter_mut().find(|op| op.view.id == id) {
                            op.view.fix_dir = fix_dir(&OpKind::RefreshMain, context, first);
                        }
                        (false, warnings)
                    }
                }
            }
            Err(error) => {
                fail(&mut queue, id, &error);
                (false, Vec::new())
            }
        };
        queue.finished.push(Finished {
            title,
            branch: String::new(),
            created: false,
            ok,
            warnings,
        });
    }
}

fn fail(queue: &mut Queue, id: u64, error: &str) {
    if let Some(op) = queue.ops.iter_mut().find(|op| op.view.id == id) {
        op.view.status = OpStatus::Failed;
        op.view.error = error.lines().next().unwrap_or_default().to_string();
        op.view.log = error.to_string();
    }
}

fn save_display_name(context: &OpContext, branch: &str, name: &str) {
    let folder = pom_layout::workspace_folder(context.runner.project_root(), branch);
    let mut state = pom_layout::WorkspaceState::load(&folder);
    state.display_name = name.to_string();
    if let Err(error) = state.save(&folder) {
        eprintln!("workspaces: save the name of {branch}: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> WorkspaceOp {
        WorkspaceOp {
            id: 1,
            branch: "feat".into(),
            title: "feat".into(),
            status: OpStatus::Queued,
            stages: Vec::new(),
            detail: String::new(),
            error: String::new(),
            retryable: true,
            quiet: false,
            ..Default::default()
        }
    }

    #[test]
    fn only_stages_a_workspace_can_do_without_are_skippable() {
        let create = OpKind::Create {
            request: CreateRequest::default(),
            display_name: String::new(),
        };
        assert!(!skip_note(&create, 2).is_empty(), "shared services");
        assert!(skip_note(&create, 3).is_empty(), "worktrees");
        assert!(skip_note(&OpKind::RefreshMain, 0).is_empty());
    }

    #[test]
    fn refresh_states_become_repo_stages() {
        let mut view = card();
        apply_repo_state(&mut view, "api", &RepoState::Pending);
        apply_repo_state(&mut view, "web", &RepoState::Pending);
        apply_repo_state(&mut view, "api", &RepoState::Pulling);
        apply_repo_state(&mut view, "api", &RepoState::Updated);
        apply_repo_state(
            &mut view,
            "web",
            &RepoState::Skipped("uncommitted changes".into()),
        );
        assert_eq!(
            view.stages,
            [
                ("api: updated".to_string(), StageState::Done),
                (
                    "web: skipped (uncommitted changes)".to_string(),
                    StageState::Skipped
                ),
            ]
        );
    }

    #[test]
    fn events_move_the_card_through_its_stages() {
        let mut view = card();
        apply_event(
            &mut view,
            &Event::Started {
                operation: pom_workspace::Operation::Create,
                stages: vec!["One", "Two", "Three"],
            },
        );
        assert_eq!(view.status, OpStatus::Running);
        apply_event(&mut view, &Event::StageSkipped { index: 0 });
        apply_event(&mut view, &Event::StageStarted { index: 1 });
        apply_event(
            &mut view,
            &Event::Progress {
                index: 1,
                detail: "worktree: api".into(),
            },
        );
        assert_eq!(view.detail, "worktree: api");
        apply_event(
            &mut view,
            &Event::Failed {
                index: 1,
                error: "web: git worktree add failed\nmore".into(),
            },
        );
        assert_eq!(view.status, OpStatus::Failed);
        assert_eq!(view.error, "web: git worktree add failed");
        let states: Vec<StageState> = view.stages.iter().map(|(_, state)| *state).collect();
        assert_eq!(
            states,
            [StageState::Skipped, StageState::Failed, StageState::Pending]
        );
    }

    struct Host {
        events: Arc<Mutex<std::sync::mpsc::Receiver<Option<Progress>>>>,
        follows: Mutex<Vec<u64>>,
        started: Mutex<Vec<(JobRequest, String)>>,
        cancelled: Mutex<Vec<String>>,
    }

    struct HostFeed(Arc<Mutex<std::sync::mpsc::Receiver<Option<Progress>>>>);

    impl pom_remote::JobFeed for HostFeed {
        fn next(&self, timeout: Duration) -> JobEvent {
            match self.0.lock().map(|events| events.recv_timeout(timeout)) {
                Ok(Ok(Some(progress))) => JobEvent::Progress(progress),
                Ok(Ok(None)) => JobEvent::Ended("connection lost".into()),
                _ => JobEvent::Idle,
            }
        }
    }

    impl ProjectSource for Host {
        fn location(&self) -> String {
            "dev".into()
        }
        fn workspaces(&self) -> Result<pom_remote::WorkspaceList, SourceError> {
            Ok(Default::default())
        }
        fn status(&self, _: &str) -> Result<pom_remote::StatusReport, SourceError> {
            Ok(Default::default())
        }
        fn logs(&self, _: &str, _: &str) -> Result<String, SourceError> {
            Ok(String::new())
        }
        fn agents(&self, _: &str) -> Result<Vec<pom_agent::SessionView>, SourceError> {
            Ok(Vec::new())
        }
        fn agent_turn(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<pom_remote::AgentTurn>, SourceError> {
            Ok(None)
        }
        fn act(
            &self,
            _: &str,
            _: pom_remote::ServiceAction,
            _: &str,
        ) -> Result<String, SourceError> {
            Ok(String::new())
        }
        fn start_job(&self, request: &JobRequest, op_id: &str) -> Result<String, SourceError> {
            if let Ok(mut started) = self.started.lock() {
                started.push((request.clone(), op_id.to_string()));
            }
            Ok("job-1".into())
        }
        fn follow_job(
            &self,
            _: &str,
            since_seq: u64,
        ) -> Result<Box<dyn pom_remote::JobFeed>, SourceError> {
            if let Ok(mut follows) = self.follows.lock() {
                follows.push(since_seq);
            }
            Ok(Box::new(HostFeed(self.events.clone())))
        }
        fn cancel_job(&self, id: &str) -> Result<(), SourceError> {
            if let Ok(mut cancelled) = self.cancelled.lock() {
                cancelled.push(id.to_string());
            }
            Ok(())
        }
    }

    fn event(seq: u64, kind: &str, stage: Option<usize>) -> Option<Progress> {
        Some(Progress {
            seq,
            kind: kind.into(),
            stage,
            ..Progress::default()
        })
    }

    fn wait(queue: &OpQueue, done: impl Fn(&[WorkspaceOp]) -> bool) -> Vec<WorkspaceOp> {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let ops = queue.snapshot();
            if done(&ops) {
                return ops;
            }
            assert!(std::time::Instant::now() < deadline, "{ops:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_remote_job_feeds_its_card_follows_again_after_a_drop_and_cancels_on_the_host() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let host = Arc::new(Host {
            events: Arc::new(Mutex::new(receiver)),
            follows: Mutex::new(Vec::new()),
            started: Mutex::new(Vec::new()),
            cancelled: Mutex::new(Vec::new()),
        });
        let queue = OpQueue::new(Arc::new(|| {}));
        let request = JobRequest::Create(pom_remote::server_api::CreateArgs {
            branch: "feat-login".into(),
            ..Default::default()
        });
        queue.enqueue_remote(request, "Login page".into(), host.clone());
        let stages = Progress {
            seq: 0,
            kind: progress_kind::STAGES.into(),
            stages: vec!["One".into(), "Two".into(), "Three".into()],
            ..Progress::default()
        };
        for progress in [
            Some(stages),
            event(1, progress_kind::STAGE_STARTED, Some(0)),
            None,
            event(2, progress_kind::STAGE_DONE, Some(0)),
            event(3, progress_kind::STAGE_STARTED, Some(1)),
        ] {
            sender.send(progress).expect("send");
        }
        let ops = wait(&queue, |ops| {
            ops.first().is_some_and(|op| {
                op.stages
                    .get(1)
                    .is_some_and(|stage| stage.1 == StageState::Running)
            })
        });
        assert!(ops[0].remote && ops[0].status == OpStatus::Running);
        assert_eq!(ops[0].branch, "feat-login");
        assert_eq!(
            host.follows.lock().map(|f| f.clone()).unwrap_or_default(),
            [0, 2]
        );

        queue.cancel(ops[0].id);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while host.cancelled.lock().map(|c| c.is_empty()).unwrap_or(true) {
            assert!(
                std::time::Instant::now() < deadline,
                "no cancel reached the host"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut failed = event(4, progress_kind::FAILED, Some(2));
        if let Some(failed) = failed.as_mut() {
            failed.line = "cancelled".into();
        }
        sender.send(failed).expect("send");
        let mut done = event(5, progress_kind::DONE, None);
        if let Some(done) = done.as_mut() {
            done.done = true;
            done.exit = Some(EXIT_CANCELLED);
        }
        sender.send(done).expect("send");
        let ops = wait(&queue, |ops| {
            ops.first().is_some_and(|op| op.status == OpStatus::Failed)
        });
        assert_eq!(ops[0].error, "cancelled");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut finished = queue.take_finished();
        while finished.is_empty() {
            assert!(std::time::Instant::now() < deadline, "never finished");
            std::thread::sleep(Duration::from_millis(10));
            finished = queue.take_finished();
        }
        assert_eq!(finished.len(), 1);
        assert!(!finished[0].ok && finished[0].created);

        queue.retry(ops[0].id);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while host.started.lock().map(|s| s.len()).unwrap_or(0) < 2 {
            assert!(std::time::Instant::now() < deadline, "not retried");
            std::thread::sleep(Duration::from_millis(10));
        }
        let started = host.started.lock().map(|s| s.clone()).unwrap_or_default();
        match &started[1].0 {
            JobRequest::Create(args) => {
                assert_eq!(args.from_stage, 2, "resumes at the stopped stage")
            }
            other => panic!("{other:?}"),
        }
        assert_ne!(started[0].1, started[1].1, "a retry is a new start");
    }
}
