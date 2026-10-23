//! The Git panel's methods: a workspace's repos, each repo's scan (pushed again whenever it reads differently
//! while a client watches), changes to its index and branch, and reads of its history. Every request names a
//! repo of the workspace, never a folder, and is checked before git runs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use git::access::{self, RepoSource};
use git::scan::RepoScan;
use git::working_copy::CommandError;
use pom_remote::server_api::{
    GitActParams, GitActed, GitReadParams, GitRepoParams, GitScanned, GitWatchParams,
    GIT_SCAN_TOPIC,
};
use pom_remote::LocalSource;
use pom_rpc::{error_code, Publisher, RemoteError};
use serde_json::Value;

use crate::dispatch::{check_branch, check_name, check_op_id, Ops, FAILED, OP_MEMORY};

/// How often a watched workspace's repos are read again, as the panel does while shown.
pub const WATCH_EVERY: Duration = Duration::from_secs(4);
const TICK: Duration = Duration::from_millis(200);
const MAX_WATCHED: usize = 64;

fn invalid(message: impl Into<String>) -> RemoteError {
    RemoteError::new(error_code::INVALID_PARAMS, message)
}

fn to_value(value: impl serde::Serialize) -> Result<Value, RemoteError> {
    serde_json::to_value(value)
        .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The repos of workspace `branch`, with their `origin` read here.
pub fn repos(source: &LocalSource, branch: &str) -> Result<Vec<RepoSource>, RemoteError> {
    check_branch(branch)?;
    source
        .git_repos(branch)
        .map_err(|error| RemoteError::new(FAILED, error.to_string()))
}

/// Repo `name` of workspace `branch`, which must be checked out there.
pub fn repo(source: &LocalSource, branch: &str, name: &str) -> Result<RepoSource, RemoteError> {
    check_name("repo", name)?;
    let found = repos(source, branch)?
        .into_iter()
        .find(|repo| repo.name == name)
        .ok_or_else(|| RemoteError::new(FAILED, format!("{branch} has no repo {name}")))?;
    if !found.root.is_dir() {
        return Err(RemoteError::new(
            FAILED,
            format!("{name} is not checked out in {branch}"),
        ));
    }
    Ok(found)
}

/// Reads every repo side by side, as the panel does.
fn read_all(repos: &[RepoSource]) -> Vec<RepoScan> {
    std::thread::scope(|threads| {
        let handles: Vec<_> = repos
            .iter()
            .map(|repo| threads.spawn(move || RepoScan::read(&repo.root, &repo.default_branch)))
            .collect();
        handles
            .into_iter()
            .zip(repos)
            .map(|(handle, repo)| {
                handle
                    .join()
                    .unwrap_or_else(|_| RepoScan::failed(&repo.root, "reading git failed"))
            })
            .collect()
    })
}

pub struct GitService {
    ops: Ops<Result<(), CommandError>>,
    /// The stop flag of each watch, by connection.
    watches: Mutex<HashMap<u64, Vec<Arc<AtomicBool>>>>,
    every: Mutex<Duration>,
}

impl Default for GitService {
    fn default() -> Self {
        GitService::new()
    }
}

impl GitService {
    pub fn new() -> GitService {
        GitService {
            ops: Ops::new(OP_MEMORY),
            watches: Mutex::new(HashMap::new()),
            every: Mutex::new(WATCH_EVERY),
        }
    }

    /// Reads watched repos this often instead of every four seconds (tests).
    pub fn set_watch_every(&self, every: Duration) {
        *locked(&self.every) = every;
    }

    /// Watches running for clients.
    pub fn watching(&self) -> usize {
        locked(&self.watches)
            .values()
            .flatten()
            .filter(|stop| !stop.load(Ordering::Acquire))
            .count()
    }

    pub fn scan(&self, source: &LocalSource, params: &GitRepoParams) -> Result<Value, RemoteError> {
        let repo = repo(source, &params.branch, &params.repo)?;
        to_value(RepoScan::read(&repo.root, &repo.default_branch))
    }

    pub fn act(&self, source: &LocalSource, params: GitActParams) -> Result<Value, RemoteError> {
        check_op_id(&params.op_id)?;
        params.op.check().map_err(invalid)?;
        let repo = repo(source, &params.branch, &params.repo)?;
        let result = self
            .ops
            .run(&params.op_id, || access::apply(&repo.root, &params.op));
        to_value(GitActed {
            error: result.err(),
        })
    }

    pub fn read(&self, source: &LocalSource, params: GitReadParams) -> Result<Value, RemoteError> {
        params.read.check().map_err(invalid)?;
        let repo = repo(source, &params.branch, &params.repo)?;
        let answer = access::answer(&repo.root, &params.read)
            .map_err(|error| RemoteError::new(FAILED, error))?;
        to_value(answer)
    }

    /// Pushes each repo's scan now and whenever it reads differently, until the subscriber leaves.
    pub fn watch(
        &self,
        connection: u64,
        source: &LocalSource,
        params: GitWatchParams,
        publisher: Publisher,
    ) -> Result<Value, RemoteError> {
        if params.repos.len() > MAX_WATCHED {
            return Err(invalid(format!("at most {MAX_WATCHED} repos")));
        }
        let all = repos(source, &params.branch)?;
        let mut watched = Vec::new();
        for name in &params.repos {
            check_name("repo", name)?;
            match all.iter().find(|repo| &repo.name == name) {
                Some(repo) => watched.push(repo.clone()),
                None => {
                    return Err(RemoteError::new(
                        FAILED,
                        format!("{} has no repo {name}", params.branch),
                    ))
                }
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut watches = locked(&self.watches);
            let list = watches.entry(connection).or_default();
            list.retain(|stop| !stop.load(Ordering::Acquire));
            list.push(stop.clone());
        }
        let every = *locked(&self.every);
        std::thread::Builder::new()
            .name("pom-server-git-watch".into())
            .spawn(move || follow(&watched, &publisher, &stop, every))
            .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))?;
        Ok(Value::Null)
    }

    /// A client went away: its watches stop.
    pub fn disconnect(&self, connection: u64) {
        if let Some(stops) = locked(&self.watches).remove(&connection) {
            for stop in stops {
                stop.store(true, Ordering::Release);
            }
        }
    }
}

fn follow(repos: &[RepoSource], publisher: &Publisher, stop: &AtomicBool, every: Duration) {
    let mut last: Vec<Option<RepoScan>> = vec![None; repos.len()];
    let gone = || stop.load(Ordering::Acquire) || publisher.is_ended();
    while !gone() {
        let started = Instant::now();
        for ((repo, scan), seen) in repos.iter().zip(read_all(repos)).zip(last.iter_mut()) {
            if seen.as_ref() == Some(&scan) {
                continue;
            }
            let pushed = GitScanned {
                repo: repo.name.clone(),
                scan: scan.clone(),
            };
            if publisher.publish(GIT_SCAN_TOPIC, pushed).is_err() {
                stop.store(true, Ordering::Release);
                return;
            }
            *seen = Some(scan);
        }
        while !gone() && started.elapsed() < every {
            std::thread::sleep(TICK);
        }
    }
    stop.store(true, Ordering::Release);
}
