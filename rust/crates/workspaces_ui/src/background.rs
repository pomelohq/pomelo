//! The project's background upkeep, run only by the process holding the session's primary lock (another app
//! instance, or the previous core, may hold it instead): reclaiming dead port leases, refresh-main on its
//! schedule, and auto-push when `pom.yml` turns it on.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_paths::StateDir;
use pom_services::ServiceRunner;

use crate::{OpContext, OpKind, OpQueue};

const TICK: Duration = Duration::from_secs(1);
const REAP_EVERY: Duration = Duration::from_secs(5);
const SCHEDULE_EVERY: Duration = Duration::from_secs(5);
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
pub const REFRESH_TITLE: &str = "Updating main";

/// Stops its thread (and gives up the primary lock) when dropped.
pub struct BackgroundSync {
    stop: Arc<AtomicBool>,
}

impl Drop for BackgroundSync {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub struct BackgroundContext {
    pub runner: Arc<ServiceRunner>,
    pub state: StateDir,
    /// The project's current config (it reloads when `pom.yml` changes).
    pub config: Arc<dyn Fn() -> Option<Arc<Config>> + Send + Sync>,
    pub ops: OpQueue,
}

impl BackgroundSync {
    pub fn start(context: BackgroundContext) -> BackgroundSync {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::spawn(move || run(context, &stopped));
        BackgroundSync { stop }
    }
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn run(context: BackgroundContext, stop: &AtomicBool) {
    let session = context.runner.session().to_string();
    let _primary = match pom_lock::acquire_primary(Path::new(pom_lock::DEFAULT_LOCK_DIR), &session)
    {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            eprintln!("sync: another process owns {session}; leaving background upkeep to it");
            return;
        }
        Err(error) => {
            eprintln!("sync: primary lock for {session}: {error}");
            return;
        }
    };
    let mut reaped = Instant::now();
    let mut scheduled: Option<Instant> = None;
    let mut next_refresh: Option<i64> = None;
    let mut next_push: Option<Instant> = None;
    let mut pruned: Option<Instant> = None;
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(TICK);
        if reaped.elapsed() >= REAP_EVERY {
            reaped = Instant::now();
            context.runner.ports().reap();
        }
        if scheduled.is_some_and(|at| at.elapsed() < SCHEDULE_EVERY) {
            continue;
        }
        scheduled = Some(Instant::now());
        let Some(config) = (context.config)() else {
            continue;
        };
        let schedule = pom_sync::refresh_schedule(&context.state, &session, Some(&config));
        if !schedule.enabled {
            next_refresh = None;
        } else if next_refresh.is_none_or(|at| now_seconds() >= at) {
            // The first check after starting (or turning it on) refreshes at once, then on the clock.
            if !refresh_queued(&context.ops) {
                context.ops.enqueue(
                    OpKind::RefreshMain,
                    REFRESH_TITLE.to_string(),
                    OpContext {
                        config: config.clone(),
                        runner: context.runner.clone(),
                        state: context.state.clone(),
                    },
                );
            }
            next_refresh = Some(pom_sync::next_aligned_run(
                now_seconds(),
                pom_sync::local_utc_offset(),
                schedule.interval_seconds,
            ));
        }
        match pom_sync::auto_push_interval(&config) {
            None => next_push = None,
            Some(interval) => {
                if next_push.is_none_or(|at| Instant::now() >= at) {
                    let prune = pruned.is_none_or(|at| at.elapsed() >= PRUNE_EVERY);
                    if prune {
                        pruned = Some(Instant::now());
                    }
                    let repos: Vec<String> = config.repos.keys().cloned().collect();
                    for failure in pom_sync::auto_push_once(
                        context.runner.project_root(),
                        config.global_default_branch(),
                        &repos,
                        prune,
                    ) {
                        eprintln!("sync: auto-push {failure}");
                    }
                    next_push = Some(Instant::now() + interval);
                }
            }
        }
    }
}

fn refresh_queued(ops: &OpQueue) -> bool {
    ops.snapshot().iter().any(|op| {
        op.title == REFRESH_TITLE
            && matches!(
                op.status,
                workspace::OpStatus::Queued | workspace::OpStatus::Running
            )
    })
}
