//! Creations and deletions run one at a time on a background thread; the WORKSPACES panel shows each as a
//! card, and a failed one stays until retried (from its failed stage) or dismissed.

use std::sync::{Arc, Mutex, MutexGuard};

use pom_config::Config;
use pom_paths::StateDir;
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
    context: OpContext,
    view: WorkspaceOp,
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
        }
    }
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
        // Main's own operations leave its row in place; a created or deleted workspace's row hides meanwhile.
        let branch = match &kind {
            OpKind::Create { request, .. } => request.branch.clone(),
            OpKind::Delete(request) => request.branch.clone(),
            OpKind::RefreshMain | OpKind::PrepareMain(_) => String::new(),
        };
        {
            let mut queue = self.lock();
            queue.next_id += 1;
            let id = queue.next_id;
            queue.ops.push(Op {
                kind,
                context,
                view: WorkspaceOp {
                    id,
                    branch,
                    title,
                    status: OpStatus::Queued,
                    stages: Vec::new(),
                    detail: String::new(),
                    error: String::new(),
                    retryable: true,
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
            if op.view.status != OpStatus::Failed {
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
                OpKind::RefreshMain | OpKind::PrepareMain(_) => {}
            }
            op.view.stages.clear();
            op.view.status = OpStatus::Queued;
            op.view.error.clear();
        }
        self.start_worker();
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
                        (op.view.id, op.kind.clone(), op.context.clone())
                    });
                if next.is_none() {
                    queue.worker = false;
                }
                next
            };
            let Some((id, kind, context)) = next else {
                (self.waker)();
                return;
            };
            (self.waker)();
            self.run(id, &kind, &context);
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
            OpKind::RefreshMain => return,
        };
        let title = self.title_of(id);
        let mut queue = self.lock();
        let (ok, warnings) = match result {
            Ok(outcome) => {
                queue.ops.retain(|op| op.view.id != id);
                (true, outcome.warnings)
            }
            Err(_) => (false, Vec::new()),
        };
        queue.finished.push(Finished {
            title,
            branch,
            created,
            ok,
            warnings,
        });
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
        }
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
}
