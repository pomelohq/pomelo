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
    JobStartParams, JobStarted, Progress, StartWaitArgs, EXIT_CANCELLED, PROGRESS_TOPIC,
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
}

/// What a job does, its arguments checked.
enum Work {
    Create(pom_workspace::CreateRequest, String),
    Wake(Workspace),
    Sleep(Workspace, bool),
    StartWait(Workspace, String, Duration),
    Sync(Workspace),
}

impl Jobs {
    pub fn new(memory: Duration) -> Jobs {
        Jobs {
            entries: Mutex::new(Entries::default()),
            memory: Mutex::new(memory),
            next: AtomicU64::new(1),
        }
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
        let work = checked_work(&source, &params.op, &params.args)?;
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
fn checked_work(source: &LocalSource, op: &str, args: &Value) -> Result<Work, RemoteError> {
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
        other => Err(invalid(format!("no job {other}"))),
    }
}

fn run(job: &Job, source: &LocalSource, work: Work) {
    let opened = match source.open_project() {
        Ok(opened) => opened,
        Err(error) => {
            job.stages(&["Reading the project"]);
            return job.fail(0, error.to_string(), EXIT_FAILED);
        }
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
    }
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
