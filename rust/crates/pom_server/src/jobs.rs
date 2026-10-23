//! Work too long for one request (creating, waking or sleeping a workspace, starting and waiting, pulling): a
//! job runs on a thread of its own and numbers its progress, kept for [`JOB_MEMORY`] after it ends so a client
//! that lost its connection follows it again from the last event it saw.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pom_layout::Workspace;
use pom_remote::server_api::{
    job_op, progress_kind, BranchArgs, CreateArgs, JobFollowParams, JobFollowed, JobParams,
    JobStartParams, JobStarted, Progress, StartWaitArgs, StoreArgs, TaskArgs, EXIT_CANCELLED,
    PROGRESS_TOPIC,
};
use pom_remote::{LocalSource, OpenedProject, ProjectSource, ServiceAction};
use pom_rpc::{error_code, Connection, ConnectionState, Publisher, RemoteError};
use pom_services::{ServiceTarget, WaitOutcome};
use pom_workspace::{Event, WorkspaceContext};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dispatch::{check_branch, check_name, check_op_id, FAILED};

pub const JOB_MEMORY: Duration = Duration::from_secs(10 * 60);
const EXIT_FAILED: i32 = 1;
/// The exits `pom start --wait` gives a service that never got ready, and one that stopped while starting.
const EXIT_NOT_READY: i32 = 2;
const EXIT_CRASHED: i32 = 3;
const DEFAULT_WAIT: u64 = 120;
const MAX_WAIT: u64 = 3600;
const MAX_DISPLAY_NAME: usize = 128;
/// How often a waiting follower or start-and-wait looks up to notice a cancel or a client gone.
const POLL: Duration = Duration::from_millis(250);

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(error_code::INVALID_PARAMS, message)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn parsed<T: DeserializeOwned>(args: &Value) -> Result<T, RemoteError> {
    let args = if args.is_null() {
        Value::Object(Default::default())
    } else {
        args.clone()
    };
    serde_json::from_value(args).map_err(|error| invalid(format!("bad args: {error}")))
}

fn check_job_id(id: &str) -> Result<(), RemoteError> {
    check_op_id(id).map_err(|_| invalid(format!("{id:?} is not a job id")))
}

#[derive(Default)]
struct Record {
    events: Vec<Progress>,
    finished: Option<Instant>,
    cancelled: bool,
}

/// One job's progress so far.
pub struct Job {
    id: String,
    op: String,
    record: Mutex<Record>,
    changed: Condvar,
}

impl Job {
    fn push(&self, kind: &str, stage: Option<usize>, line: impl Into<String>) {
        self.push_event(Progress {
            kind: kind.to_string(),
            stage,
            line: line.into(),
            ..Progress::default()
        });
    }

    fn push_event(&self, mut event: Progress) {
        let mut record = locked(&self.record);
        if record.finished.is_some() {
            return;
        }
        event.seq = record.events.len() as u64;
        if event.done {
            record.finished = Some(Instant::now());
        }
        record.events.push(event);
        drop(record);
        self.changed.notify_all();
    }

    fn stages(&self, labels: &[&str]) {
        self.push_event(Progress {
            kind: progress_kind::STAGES.to_string(),
            stages: labels.iter().map(|label| label.to_string()).collect(),
            ..Progress::default()
        });
    }

    fn finish(&self, exit: i32) {
        self.push_event(Progress {
            kind: progress_kind::DONE.to_string(),
            done: true,
            exit: Some(exit),
            ..Progress::default()
        });
    }

    fn fail(&self, stage: usize, line: impl Into<String>, exit: i32) {
        self.push(progress_kind::FAILED, Some(stage), line);
        self.finish(exit);
    }

    fn stopped(&self) -> bool {
        locked(&self.record).cancelled
    }

    /// Fails `stage` as cancelled when a cancel came, so the job goes no further.
    fn cancelled_at(&self, stage: usize) -> bool {
        if !self.stopped() {
            return false;
        }
        self.fail(stage, pom_workspace::CANCELLED, EXIT_CANCELLED);
        true
    }

    fn pipeline(&self, event: Event) {
        let (kind, stage, line) = match event {
            Event::Started { stages, .. } => return self.stages(&stages),
            Event::StageStarted { index } => (progress_kind::STAGE_STARTED, index, String::new()),
            Event::Progress { index, detail } => (progress_kind::LINE, index, detail),
            Event::Warning { index, detail } => (progress_kind::WARNING, index, detail),
            Event::StageCompleted { index } => (progress_kind::STAGE_DONE, index, String::new()),
            Event::StageSkipped { index } => (progress_kind::STAGE_SKIPPED, index, String::new()),
            Event::Failed { index, error } => (progress_kind::FAILED, index, error),
            Event::Completed { .. } => return,
        };
        self.push(kind, Some(stage), line);
    }

    /// The events from `seq` on, waiting up to `wait` for one when there is none yet; and whether it ended.
    fn events_from(&self, seq: u64, wait: Duration) -> (Vec<Progress>, bool) {
        let mut record = locked(&self.record);
        if record.events.len() as u64 <= seq && record.finished.is_none() {
            record = self
                .changed
                .wait_timeout(record, wait)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
        let from = (seq as usize).min(record.events.len());
        (record.events[from..].to_vec(), record.finished.is_some())
    }
}

#[derive(Default)]
struct Entries {
    jobs: HashMap<String, Arc<Job>>,
    by_op_id: HashMap<String, String>,
}

/// Every job of this server, by id and by the op id that started it.
pub struct Jobs {
    entries: Mutex<Entries>,
    memory: Mutex<Duration>,
    next: AtomicU64,
    /// What runs a task's script in place of the user's login shell (tests).
    task_shell: Mutex<Option<Vec<String>>>,
    store_sizes: pom_workspace::SizeCache,
}

/// A repo command to run: where, with which env, and the argv that runs it.
struct Task {
    label: String,
    argv: Vec<String>,
    cwd: std::path::PathBuf,
}

/// What a job does, its arguments checked.
enum Work {
    Create(pom_workspace::CreateRequest, String),
    Wake(Workspace),
    Sleep(Workspace, bool),
    StartWait(Workspace, String, Duration),
    Sync(Workspace),
    Task(Task),
    Store(StoreWork),
}

/// A Store page request, with what runs a script in place of the login shell and the sizes measured before.
struct StoreWork {
    request: module_store::Request,
    shell: Option<Vec<String>>,
    sizes: pom_workspace::SizeCache,
}

impl Jobs {
    pub fn new(memory: Duration) -> Jobs {
        Jobs {
            entries: Mutex::new(Entries::default()),
            memory: Mutex::new(memory),
            next: AtomicU64::new(1),
            task_shell: Mutex::new(None),
            store_sizes: pom_workspace::SizeCache::default(),
        }
    }

    /// Runs tasks as `argv` followed by their script instead of through the user's login shell.
    pub fn set_task_shell(&self, argv: Vec<String>) {
        *locked(&self.task_shell) = Some(argv);
    }

    pub fn set_memory(&self, memory: Duration) {
        *locked(&self.memory) = memory;
    }

    fn entries(&self) -> MutexGuard<'_, Entries> {
        let memory = *locked(&self.memory);
        let mut entries = locked(&self.entries);
        entries.jobs.retain(|_, job| {
            locked(&job.record)
                .finished
                .is_none_or(|finished| finished.elapsed() < memory)
        });
        let Entries { jobs, by_op_id } = &mut *entries;
        by_op_id.retain(|_, id| jobs.contains_key(id));
        entries
    }

    /// Jobs still running.
    pub fn running(&self) -> usize {
        self.entries()
            .jobs
            .values()
            .filter(|job| locked(&job.record).finished.is_none())
            .count()
    }

    pub fn start(
        &self,
        source: LocalSource,
        params: JobStartParams,
    ) -> Result<JobStarted, RemoteError> {
        check_op_id(&params.op_id)?;
        if !job_op::ALL.contains(&params.op.as_str()) {
            return Err(invalid(format!("no job {}", params.op)));
        }
        if let Some(id) = self.entries().by_op_id.get(&params.op_id) {
            return Ok(JobStarted {
                id: id.clone(),
                existing: true,
            });
        }
        let task_shell = locked(&self.task_shell).clone();
        let mut work = checked_work(&source, &params.op, &params.args, task_shell)?;
        if let Work::Store(store) = &mut work {
            store.sizes = self.store_sizes.clone();
        }
        let mut entries = self.entries();
        if let Some(id) = entries.by_op_id.get(&params.op_id) {
            return Ok(JobStarted {
                id: id.clone(),
                existing: true,
            });
        }
        let id = format!("job-{}", self.next.fetch_add(1, Ordering::AcqRel));
        let job = Arc::new(Job {
            id: id.clone(),
            op: params.op.clone(),
            record: Mutex::new(Record::default()),
            changed: Condvar::new(),
        });
        let running = job.clone();
        std::thread::Builder::new()
            .name("pom-server-job".into())
            .spawn(move || run(&running, &source, work))
            .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))?;
        entries.jobs.insert(id.clone(), job);
        entries.by_op_id.insert(params.op_id, id.clone());
        Ok(JobStarted {
            id,
            existing: false,
        })
    }

    fn job(&self, id: &str) -> Result<Arc<Job>, RemoteError> {
        check_job_id(id)?;
        self.entries().jobs.get(id).cloned().ok_or_else(|| {
            RemoteError::new(
                FAILED,
                format!("no job {id} (it ended over ten minutes ago, or never ran here)"),
            )
        })
    }

    /// Pushes the job's events from `since_seq` on the caller's subscription until it ends.
    pub fn follow(
        &self,
        connection: &Connection,
        params: JobFollowParams,
    ) -> Result<JobFollowed, RemoteError> {
        let job = self.job(&params.id)?;
        let publisher = connection
            .publisher(params.channel)
            .map_err(|error| invalid(format!("channel {}: {error}", params.channel)))?;
        let (next_seq, done) = {
            let record = locked(&job.record);
            (record.events.len() as u64, record.finished.is_some())
        };
        let (followed, connection) = (job.clone(), connection.clone());
        std::thread::Builder::new()
            .name("pom-server-job-follow".into())
            .spawn(move || push_progress(&followed, publisher, &connection, params.since_seq))
            .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))?;
        Ok(JobFollowed {
            id: job.id.clone(),
            op: job.op.clone(),
            next_seq,
            done,
        })
    }

    /// Asks the job to stop before its next stage; one that already ended is left as it is.
    pub fn cancel(&self, params: JobParams) -> Result<Value, RemoteError> {
        let job = self.job(&params.id)?;
        locked(&job.record).cancelled = true;
        job.changed.notify_all();
        Ok(Value::Null)
    }
}

fn push_progress(job: &Job, publisher: Publisher, connection: &Connection, since: u64) {
    let mut next = since;
    loop {
        if publisher.is_ended() || connection.state() != ConnectionState::Open {
            return;
        }
        let (events, done) = job.events_from(next, POLL);
        for event in &events {
            if publisher.publish(PROGRESS_TOPIC, event).is_err() {
                return;
            }
            next = event.seq + 1;
        }
        if done && events.is_empty() {
            return publisher.finish("done");
        }
    }
}

fn workspace_of(opened: &OpenedProject, branch: &str) -> Result<Workspace, RemoteError> {
    opened
        .project
        .workspaces
        .iter()
        .find(|workspace| workspace.branch == branch)
        .cloned()
        .ok_or_else(|| RemoteError::new(FAILED, format!("no workspace {branch}")))
}

/// Checks a job's arguments the way the methods check theirs, before anything runs.
fn checked_work(
    source: &LocalSource,
    op: &str,
    args: &Value,
    task_shell: Option<Vec<String>>,
) -> Result<Work, RemoteError> {
    let opened = source
        .open_project()
        .map_err(|error| RemoteError::new(FAILED, error.to_string()))?;
    match op {
        job_op::WS_CREATE => {
            let args: CreateArgs = parsed(args)?;
            check_branch(&args.branch)?;
            for repo in &args.repos {
                check_name("repo", repo)?;
            }
            if !args.environment.is_empty() {
                check_name("environment", &args.environment)?;
            }
            opened
                .config
                .validate_environment(&args.environment)
                .map_err(|error| RemoteError::new(FAILED, error))?;
            if args.display_name.chars().count() > MAX_DISPLAY_NAME
                || args.display_name.chars().any(char::is_control)
            {
                return Err(invalid("display_name is not valid"));
            }
            if args.from_stage >= pom_workspace::CREATE_STAGES.len() {
                return Err(invalid(format!(
                    "from_stage must be below {}",
                    pom_workspace::CREATE_STAGES.len()
                )));
            }
            let request = pom_workspace::CreateRequest {
                branch: args.branch,
                repos: args.repos,
                environment: args.environment,
                fresh_databases: args.fresh_databases,
                from_stage: args.from_stage,
                ..pom_workspace::CreateRequest::default()
            };
            Ok(Work::Create(request, args.display_name.trim().to_string()))
        }
        job_op::WS_WAKE | job_op::WS_SLEEP | job_op::SYNC => {
            let args: BranchArgs = parsed(args)?;
            check_branch(&args.branch)?;
            let workspace = workspace_of(&opened, &args.branch)?;
            Ok(match op {
                job_op::WS_WAKE => Work::Wake(workspace),
                job_op::WS_SLEEP => Work::Sleep(workspace, args.force),
                _ => Work::Sync(workspace),
            })
        }
        job_op::START_WAIT => {
            let args: StartWaitArgs = parsed(args)?;
            check_branch(&args.branch)?;
            check_name("target", &args.target)?;
            let seconds = match args.timeout_seconds {
                0 => DEFAULT_WAIT,
                seconds if seconds <= MAX_WAIT => seconds,
                _ => return Err(invalid(format!("timeout_seconds must be 1..{MAX_WAIT}"))),
            };
            let workspace = workspace_of(&opened, &args.branch)?;
            pom_remote::ordered_services(&opened.config, &args.target, false)
                .map_err(|error| RemoteError::new(FAILED, error))?;
            Ok(Work::StartWait(
                workspace,
                args.target,
                Duration::from_secs(seconds),
            ))
        }
        job_op::TASK => {
            let args: TaskArgs = parsed(args)?;
            check_branch(&args.branch)?;
            check_name("repo", &args.repo)?;
            let workspace = workspace_of(&opened, &args.branch)?;
            checked_task(&opened, &workspace, &args, task_shell).map(Work::Task)
        }
        job_op::STORE => {
            let args: StoreArgs = parsed(args)?;
            checked_store_request(&args.request)?;
            Ok(Work::Store(StoreWork {
                request: args.request,
                shell: task_shell,
                sizes: pom_workspace::SizeCache::default(),
            }))
        }
        other => Err(invalid(format!("no job {other}"))),
    }
}

/// The task is one of the repo's commands in the config, in a repo the workspace has checked out.
fn checked_task(
    opened: &OpenedProject,
    workspace: &Workspace,
    args: &TaskArgs,
    task_shell: Option<Vec<String>>,
) -> Result<Task, RemoteError> {
    let dir = opened
        .config
        .repos
        .get(&args.repo)
        .ok_or_else(|| RemoteError::new(FAILED, format!("no repo {}", args.repo)))?;
    let shortcut = dir
        .effective_shortcuts()
        .into_iter()
        .filter(|shortcut| !shortcut.cmd.trim().is_empty())
        .find(|shortcut| shortcut.label() == args.label)
        .ok_or_else(|| {
            RemoteError::new(
                FAILED,
                format!("{} has no command {:?}", args.repo, args.label),
            )
        })?;
    let worktree = workspace.path.join(&args.repo);
    if !worktree.is_dir() {
        return Err(RemoteError::new(
            FAILED,
            format!("{} is not checked out in {}", args.repo, workspace.branch),
        ));
    }
    let env = opened
        .runner
        .workspace_env(&opened.config, &workspace.branch);
    if let Err(error) = env.write_env_files() {
        eprintln!("pom server: task env files: {error}");
    }
    let exports: String = env
        .repo_env(&args.repo)
        .into_iter()
        .map(|(key, value)| format!("export {key}={}; ", pom_services::shell_quote(&value)))
        .collect();
    let script = format!(
        "exec 2>&1; export PATH={path}; {exports}cd {dir} && {cmd}",
        path = pom_services::shell_quote(pom_services::tool_path()),
        dir = pom_services::shell_quote(&worktree.to_string_lossy()),
        cmd = shortcut.cmd,
    );
    let argv = match task_shell {
        Some(mut argv) => {
            argv.push(script);
            argv
        }
        None => pom_shell::Shell::user().login_argv(&script),
    };
    Ok(Task {
        label: format!("{}: {}", args.repo, args.label),
        argv,
        cwd: worktree,
    })
}

fn run(job: &Job, source: &LocalSource, work: Work) {
    let opened = match source.open_project() {
        Ok(opened) => opened,
        Err(error) => {
            job.stages(&["Reading the project"]);
            return job.fail(0, error.to_string(), EXIT_FAILED);
        }
    };
    let work = match work {
        Work::Store(store) => return run_store(job, source, opened, store),
        other => other,
    };
    let context = WorkspaceContext {
        config: &opened.config,
        runner: &opened.runner,
        state: source.state(),
    };
    match work {
        Work::Create(request, display_name) => create(job, &context, &request, &display_name),
        Work::Wake(workspace) => wake(job, &context, &workspace),
        Work::Sleep(workspace, force) => sleep(job, &context, &workspace, force),
        Work::StartWait(workspace, target, timeout) => {
            start_wait(job, source, &opened, &workspace, &target, timeout)
        }
        Work::Sync(workspace) => sync(job, source, &opened, &workspace),
        Work::Task(task) => run_task(job, &task),
        Work::Store(_) => {}
    }
}

fn checked_store_request(request: &module_store::Request) -> Result<(), RemoteError> {
    use module_store::Request;
    match request {
        Request::FreeOld { repo } => check_name("repo", repo),
        Request::Relink {
            repo,
            workspace,
            path,
        } => {
            check_name("repo", repo)?;
            check_branch(workspace)?;
            check_worktree_path(path)
        }
        Request::Keep { repo, path } => {
            check_name("repo", repo)?;
            check_worktree_path(path)
        }
        Request::Refresh | Request::FreeUnused | Request::FreeOthers | Request::Optimize => Ok(()),
    }
}

/// The job itself only acts on a path that is one of the project's worktrees; this turns away the obvious.
fn check_worktree_path(path: &std::path::Path) -> Result<(), RemoteError> {
    let valid = path.is_absolute()
        && !path
            .components()
            .any(|component| component == std::path::Component::ParentDir);
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("{} is not a worktree", path.display())))
    }
}

/// `node --version` in `path`: through the stand-in shell when one is set, else the user's login shell.
fn node_version(shell: Option<&[String]>, path: &std::path::Path) -> Option<String> {
    let Some((program, args)) = shell.and_then(|argv| argv.split_first()) else {
        return pom_workspace::node_version(path, &[]);
    };
    let output = std::process::Command::new(program)
        .args(args)
        .arg("node --version")
        .current_dir(path)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Reports the store's place, then each [`module_store::StoreUpdate`] as a `data` event.
fn run_store(job: &Job, source: &LocalSource, opened: OpenedProject, work: StoreWork) {
    let store = module_store::Store::new(source.state());
    let supported = store.supported_method(&opened.project.root);
    let method = module_store::Options::from_settings_file().method(supported);
    let send = |update: module_store::StoreUpdate| match serde_json::to_value(&update) {
        Ok(data) => {
            job.push_event(Progress {
                kind: progress_kind::DATA.to_string(),
                data: Some(data),
                ..Progress::default()
            });
            !job.stopped()
        }
        Err(error) => {
            eprintln!("pom server: a store update: {error}");
            true
        }
    };
    send(module_store::StoreUpdate::Place {
        root: store.root().display().to_string(),
        method: module_store::describe_method(method).to_string(),
    });
    let repos = pom_workspace::store_worktrees(&opened.config, &opened.project.workspaces);
    let shell = work.shell.clone();
    let repos =
        pom_workspace::with_node_versions(repos, &|path| node_version(shell.as_deref(), path));
    let request = pom_workspace::StoreJob {
        request: work.request,
        repos,
        previous: None,
        services: Some((Arc::new(opened.runner), Arc::new(opened.config))),
    };
    pom_workspace::run_store_job(&store, request, &work.sizes, &send);
    job.finish(if job.stopped() { EXIT_CANCELLED } else { 0 });
}

fn create(
    job: &Job,
    context: &WorkspaceContext<'_>,
    request: &pom_workspace::CreateRequest,
    display_name: &str,
) {
    let sink = |event: Event| job.pipeline(event);
    let result = pom_workspace::create_until(context, request, &sink, &|| job.stopped());
    match result {
        Ok(_) => {
            if !display_name.is_empty() {
                let folder =
                    pom_layout::workspace_folder(context.runner.project_root(), &request.branch);
                let mut state = pom_layout::WorkspaceState::load(&folder);
                state.display_name = display_name.to_string();
                if let Err(error) = state.save(&folder) {
                    job.push(
                        progress_kind::WARNING,
                        None,
                        format!("could not save the name: {error}"),
                    );
                }
            }
            job.finish(0);
        }
        Err(error) if error.message == pom_workspace::CANCELLED => job.finish(EXIT_CANCELLED),
        Err(_) => job.finish(EXIT_FAILED),
    }
}

fn wake(job: &Job, context: &WorkspaceContext<'_>, workspace: &Workspace) {
    job.stages(&["Starting its services and agents"]);
    if job.cancelled_at(0) {
        return;
    }
    job.push(progress_kind::STAGE_STARTED, Some(0), "");
    let agents = pom_workspace::DriveAgents::new(context);
    let settings = pom_workspace::PowerSettings::from_settings_file();
    match pom_workspace::wake_with_room(context, &agents, workspace, &settings) {
        Ok(pom_workspace::WakeOutcome::Woke { report, made_room }) => {
            if let Some(slept) = made_room {
                job.push(
                    progress_kind::LINE,
                    Some(0),
                    format!("put {slept} to sleep to make room (workspaces.max_running)"),
                );
            }
            if report.already {
                job.push(progress_kind::LINE, Some(0), "it was awake");
            }
            for started in &report.started {
                job.push(progress_kind::LINE, Some(0), format!("started {started}"));
            }
            for agent in &report.agents {
                job.push(
                    progress_kind::LINE,
                    Some(0),
                    format!("started agent {agent}"),
                );
            }
            for failure in &report.failed {
                job.push(
                    progress_kind::WARNING,
                    Some(0),
                    format!("could not start {failure}"),
                );
            }
            match report.failed.len() {
                0 => {
                    job.push(progress_kind::STAGE_DONE, Some(0), "");
                    job.finish(0);
                }
                count => job.fail(0, format!("{count} could not start"), EXIT_FAILED),
            }
        }
        Ok(pom_workspace::WakeOutcome::Queued { running }) => {
            job.push(
                progress_kind::LINE,
                Some(0),
                format!("queued: {running} workspaces already run (workspaces.max_running)"),
            );
            job.push(progress_kind::STAGE_DONE, Some(0), "");
            job.finish(0);
        }
        Err(error) => job.fail(0, error, EXIT_FAILED),
    }
}

fn sleep(job: &Job, context: &WorkspaceContext<'_>, workspace: &Workspace, force: bool) {
    job.stages(&["Stopping its services and agents"]);
    if job.cancelled_at(0) {
        return;
    }
    job.push(progress_kind::STAGE_STARTED, Some(0), "");
    let agents = pom_workspace::DriveAgents::new(context);
    match pom_workspace::sleep(context, &agents, workspace, force) {
        Ok(report) => {
            if report.already {
                job.push(progress_kind::LINE, Some(0), "it was asleep");
            }
            for service in &report.services {
                job.push(progress_kind::LINE, Some(0), format!("stopped {service}"));
            }
            for agent in &report.agents {
                job.push(
                    progress_kind::LINE,
                    Some(0),
                    format!("stopped agent {agent}"),
                );
            }
            job.push(progress_kind::STAGE_DONE, Some(0), "");
            job.finish(0);
        }
        Err(error) => job.fail(0, error.to_string(), EXIT_FAILED),
    }
}

fn start_wait(
    job: &Job,
    source: &LocalSource,
    opened: &OpenedProject,
    workspace: &Workspace,
    target: &str,
    timeout: Duration,
) {
    job.stages(&[&format!("Starting {target}"), "Waiting until ready"]);
    if job.cancelled_at(0) {
        return;
    }
    let started = Instant::now();
    job.push(progress_kind::STAGE_STARTED, Some(0), "");
    match source.act(&workspace.branch, ServiceAction::Start, target) {
        Ok(said) => {
            for line in said.lines().filter(|line| !line.trim().is_empty()) {
                job.push(progress_kind::LINE, Some(0), line);
            }
            job.push(progress_kind::STAGE_DONE, Some(0), "");
        }
        Err(error) => return job.fail(0, error.to_string(), EXIT_FAILED),
    }
    if job.cancelled_at(1) {
        return;
    }
    job.push(progress_kind::STAGE_STARTED, Some(1), "");
    let targets: Vec<ServiceTarget> =
        match pom_remote::ordered_services(&opened.config, target, false) {
            Ok(pairs) => pairs
                .into_iter()
                .map(|(repo, service)| ServiceTarget {
                    branch: workspace.branch.clone(),
                    is_main: workspace.is_main,
                    repo,
                    service,
                })
                .collect(),
            Err(error) => return job.fail(1, error, EXIT_FAILED),
        };
    if targets.iter().any(|target| !target.is_workspace_level())
        && !opened.config.shared_services.is_empty()
    {
        let names: Vec<String> = opened.config.shared_services.keys().cloned().collect();
        let left = timeout.saturating_sub(started.elapsed());
        if let Err(silent) = opened
            .runner
            .wait_shared_ready(&opened.config, &names, left)
        {
            return job.fail(
                1,
                format!("shared {} not ready", silent.join(", ")),
                EXIT_NOT_READY,
            );
        }
    }
    loop {
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return job.fail(
                1,
                format!("not ready after {}s", timeout.as_secs()),
                EXIT_NOT_READY,
            );
        }
        if job.cancelled_at(1) {
            return;
        }
        match opened
            .runner
            .wait_ready(&opened.config, &targets, left.min(POLL * 4))
        {
            WaitOutcome::Ready(services) => {
                for health in services {
                    let port = health
                        .port
                        .map(|port| format!(" :{port}"))
                        .unwrap_or_default();
                    job.push(
                        progress_kind::LINE,
                        Some(1),
                        format!("{}/{} ready{port}", health.repo, health.service),
                    );
                }
                job.push(progress_kind::STAGE_DONE, Some(1), "");
                return job.finish(0);
            }
            WaitOutcome::TimedOut(_) => {}
            WaitOutcome::Crashed { service, tail, .. } => {
                for line in tail.lines().filter(|line| !line.trim().is_empty()) {
                    job.push(progress_kind::WARNING, Some(1), line);
                }
                return job.fail(1, format!("{service} stopped while starting"), EXIT_CRASHED);
            }
        }
    }
}

fn sync(job: &Job, source: &LocalSource, opened: &OpenedProject, workspace: &Workspace) {
    job.stages(&["Pulling from origin"]);
    if job.cancelled_at(0) {
        return;
    }
    job.push(progress_kind::STAGE_STARTED, Some(0), "");
    let machine = pom_push::Machine {
        state: source.state().clone(),
        holders: source.holders().clone(),
        binary: source.binary().to_path_buf(),
    };
    let records = pom_push::pull_now(&machine, &opened.project.root, &workspace.branch);
    let mut failed = 0;
    for record in &records {
        let kind = if record.outcome == pom_push::Outcome::Failed {
            failed += 1;
            progress_kind::WARNING
        } else {
            progress_kind::LINE
        };
        job.push(kind, Some(0), record.message());
    }
    match failed {
        0 => {
            job.push(progress_kind::STAGE_DONE, Some(0), "");
            job.finish(0);
        }
        count => job.fail(0, format!("{count} repo(s) could not pull"), EXIT_FAILED),
    }
}

/// Runs the task's script, each line it prints a step of its one stage; a cancel kills it.
fn run_task(job: &Job, task: &Task) {
    use std::io::BufRead;
    job.stages(&[task.label.as_str()]);
    if job.cancelled_at(0) {
        return;
    }
    job.push(progress_kind::STAGE_STARTED, Some(0), "");
    let Some((program, args)) = task.argv.split_first() else {
        return job.fail(0, "nothing to run", EXIT_FAILED);
    };
    let spawned = std::process::Command::new(program)
        .args(args)
        .current_dir(&task.cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return job.fail(0, format!("could not run it: {error}"), EXIT_FAILED),
    };
    let reader = child.stdout.take().map(|stdout| {
        let lines = std::sync::mpsc::channel::<String>();
        let (sender, receiver) = lines;
        let thread = std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
            {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        (receiver, thread)
    });
    let status = loop {
        if let Some((receiver, _)) = &reader {
            while let Ok(line) = receiver.try_recv() {
                job.push(progress_kind::LINE, Some(0), line);
            }
        }
        if job.stopped() {
            if let Err(error) = child.kill() {
                eprintln!("pom server: stopping a task: {error}");
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => break Err(error),
        }
    };
    if let Some((receiver, thread)) = reader {
        if thread.join().is_err() {
            eprintln!("pom server: a task's reader panicked");
        }
        for line in receiver.try_iter() {
            job.push(progress_kind::LINE, Some(0), line);
        }
    }
    match status {
        _ if job.stopped() => job.fail(0, pom_workspace::CANCELLED, EXIT_CANCELLED),
        Ok(status) if status.success() => {
            job.push(progress_kind::STAGE_DONE, Some(0), "");
            job.finish(0);
        }
        Ok(status) => {
            let code = status.code().unwrap_or(EXIT_FAILED);
            job.fail(0, format!("exited with {code}"), code);
        }
        Err(error) => job.fail(0, error.to_string(), EXIT_FAILED),
    }
}
