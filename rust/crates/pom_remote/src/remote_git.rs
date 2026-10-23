//! The Git panel's git on the machine a remote project lives on: each request goes to its server, which runs
//! git there with that machine's own credentials; fetch, pull and push run as jobs that report git's progress.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use git::access::{
    GitAccess, GitAnswer, GitOp, GitRead, RemoteCommand, RepoSource, ScanSink, Watching,
};
use git::scan::RepoScan;
use git::working_copy::{CommandError, CommandOutput};
use pom_rpc::{Connection, Error, PushEvent};

use crate::server_api::{
    method, progress_kind, GitActParams, GitActed, GitReadParams, GitRemoteArgs, GitRemoteOutcome,
    GitRepoParams, GitScanned, GitWatchParams, JobRequest, WorkspaceParams, GIT_SCAN_TOPIC,
};
use crate::server_source::{Inner, ServerSource};
use crate::source::{ACT_TIMEOUT, READ_TIMEOUT};
use crate::{JobEvent, ProjectSource, SourceError};

const FOLLOW_POLL: Duration = Duration::from_millis(500);
const PUSH_TICK: Duration = Duration::from_millis(500);
const RESUBSCRIBE_WAIT: Duration = Duration::from_secs(2);
/// Times a job is followed again after its connection dropped before giving up on hearing how it ended.
const FOLLOW_TRIES: usize = 5;

/// The repos of `branch` as the Git panel lists them, from the project as read on this machine.
pub fn repo_sources(project: &pom_core::Project, branch: &str) -> Vec<RepoSource> {
    let Some(workspace) = project
        .workspaces
        .iter()
        .find(|workspace| workspace.branch == branch)
    else {
        return Vec::new();
    };
    workspace
        .repos
        .iter()
        .map(|repo| RepoSource {
            name: repo.name.clone(),
            root: repo.path.clone(),
            default_branch: project.config.as_ref().map_or_else(
                || "main".to_string(),
                |config| config.default_branch_for(&repo.name).to_string(),
            ),
            expected_branch: branch.to_string(),
            // Kept means on its own branch on purpose, which the panel must not flag.
            kept: repo.expected != branch && repo.branch == repo.expected,
            origin: None,
        })
        .collect()
}

pub struct RemoteGit {
    inner: Arc<Inner>,
    branch: String,
}

impl RemoteGit {
    pub(crate) fn new(inner: Arc<Inner>, branch: &str) -> RemoteGit {
        RemoteGit {
            inner,
            branch: branch.to_string(),
        }
    }

    /// The workspace's repos as its machine lists them, with their `origin`.
    pub fn repos(&self) -> Result<Vec<RepoSource>, SourceError> {
        let params = WorkspaceParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
        };
        self.inner
            .ask_server(method::GIT_REPOS, params, READ_TIMEOUT, "The Git panel")
    }

    fn repo_params(&self, repo: &RepoSource) -> GitRepoParams {
        GitRepoParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            repo: repo.name.clone(),
        }
    }

    fn follow(
        &self,
        source: &ServerSource,
        job: &str,
        progress: &(dyn Fn(&str) + Sync),
    ) -> Result<CommandOutput, CommandError> {
        let mut next = 0;
        let mut lost = 0;
        loop {
            let reason = match source.follow_job(job, next) {
                Ok(feed) => loop {
                    match feed.next(FOLLOW_POLL) {
                        JobEvent::Progress(event) => {
                            lost = 0;
                            next = event.seq + 1;
                            match event.kind.as_str() {
                                progress_kind::LINE => progress(&event.line),
                                progress_kind::DATA => {
                                    let outcome =
                                        event.data.map(serde_json::from_value::<GitRemoteOutcome>);
                                    match outcome {
                                        Some(Ok(GitRemoteOutcome {
                                            error: Some(error), ..
                                        })) => return Err(error),
                                        Some(Ok(GitRemoteOutcome {
                                            output: Some(output),
                                            ..
                                        })) => return Ok(output),
                                        Some(Err(error)) => {
                                            return Err(failure(format!(
                                                "{} answered with something pom here cannot read ({error})",
                                                self.inner.host_name()
                                            )))
                                        }
                                        _ => {}
                                    }
                                }
                                progress_kind::FAILED => return Err(failure(event.line)),
                                _ => {}
                            }
                            if event.done {
                                return Err(failure("git stopped without saying how it went"));
                            }
                        }
                        JobEvent::Idle => {}
                        JobEvent::Ended(reason) => break reason,
                    }
                },
                Err(error) => error.to_string(),
            };
            lost += 1;
            if lost > FOLLOW_TRIES {
                return Err(failure(format!(
                    "lost {} while git ran there ({reason}); look again once it is back",
                    self.inner.host_name()
                )));
            }
            std::thread::sleep(RESUBSCRIBE_WAIT);
        }
    }
}

fn failure(message: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
        output: CommandOutput::default(),
    }
}

impl GitAccess for RemoteGit {
    fn scan(&self, repo: &RepoSource) -> RepoScan {
        self.inner
            .ask_server(
                method::GIT_SCAN,
                self.repo_params(repo),
                READ_TIMEOUT,
                "The Git panel",
            )
            .unwrap_or_else(|error: SourceError| RepoScan::failed(&repo.root, error.to_string()))
    }

    fn act(&self, repo: &RepoSource, op: GitOp) -> Result<(), CommandError> {
        let params = GitActParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            repo: repo.name.clone(),
            op,
            op_id: self.inner.next_op_id(),
        };
        let acted: GitActed = self
            .inner
            .ask_server(method::GIT_ACT, params, ACT_TIMEOUT, "The Git panel")
            .map_err(|error| failure(error.to_string()))?;
        acted.error.map_or(Ok(()), Err)
    }

    fn read(&self, repo: &RepoSource, query: GitRead) -> Result<GitAnswer, String> {
        let params = GitReadParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            repo: repo.name.clone(),
            read: query,
        };
        self.inner
            .ask_server(method::GIT_READ, params, READ_TIMEOUT, "The Git panel")
            .map_err(|error| error.to_string())
    }

    fn remote(
        &self,
        repo: &RepoSource,
        command: RemoteCommand,
        progress: &(dyn Fn(&str) + Sync),
    ) -> Result<CommandOutput, CommandError> {
        let source = ServerSource::from_inner(self.inner.clone());
        let job = JobRequest::GitRemote(GitRemoteArgs {
            branch: self.branch.clone(),
            repo: repo.name.clone(),
            command,
        });
        let id = source
            .start_job(&job, &self.inner.next_op_id())
            .map_err(|error| failure(error.to_string()))?;
        self.follow(&source, &id, progress)
    }

    fn watch(&self, repos: &[RepoSource], changed: ScanSink) -> Option<Watching> {
        let stop = Arc::new(AtomicBool::new(false));
        let watcher = Watcher {
            inner: self.inner.clone(),
            branch: self.branch.clone(),
            names: repos.iter().map(|repo| repo.name.clone()).collect(),
            changed,
            stop: stop.clone(),
        };
        let spawned = std::thread::Builder::new()
            .name("git-watch".into())
            .spawn(move || watcher.run());
        if let Err(error) = spawned {
            eprintln!("git watch: {error}");
            return None;
        }
        Some(Watching::new(move || stop.store(true, Ordering::Release)))
    }

    fn sign_in_advice(&self) -> Option<String> {
        let host = self.inner.host_name();
        Some(format!(
            "git on {host} could not sign in to the remote. It uses {host}'s own git credentials: run pom doctor on {host} to check them."
        ))
    }
}

struct Watcher {
    inner: Arc<Inner>,
    branch: String,
    names: Vec<String>,
    changed: ScanSink,
    stop: Arc<AtomicBool>,
}

impl Watcher {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn run(self) {
        let index: HashMap<String, usize> = self
            .names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.clone(), index))
            .collect();
        let mut first = true;
        while !self.stopped() {
            let connection = if first {
                first = false;
                self.inner.connection().ok().flatten()
            } else {
                self.inner.live_connection()
            };
            if let Some(connection) = connection {
                let reason = self.follow(&connection, &index);
                if !self.stopped() {
                    eprintln!("git watch on {}: {reason}", self.inner.host_name());
                }
            }
            if self.stopped() {
                return;
            }
            std::thread::sleep(RESUBSCRIBE_WAIT);
        }
    }

    fn follow(&self, connection: &Connection, index: &HashMap<String, usize>) -> String {
        let subscription = match connection.subscribe() {
            Ok(subscription) => subscription,
            Err(error) => return error.to_string(),
        };
        let params = GitWatchParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            repos: self.names.clone(),
            channel: subscription.channel(),
        };
        let asked = connection
            .start_request(method::GIT_WATCH, params)
            .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT));
        if let Err(error) = asked {
            return error.to_string();
        }
        loop {
            if self.stopped() {
                return "closed".to_string();
            }
            match subscription.recv_timeout(PUSH_TICK) {
                Ok(PushEvent::Value { topic, value }) if topic == GIT_SCAN_TOPIC => {
                    match serde_json::from_value::<GitScanned>(value) {
                        Ok(scanned) => {
                            if let Some(at) = index.get(&scanned.repo) {
                                (self.changed)(*at, scanned.scan);
                            }
                        }
                        Err(error) => return format!("an unreadable scan: {error}"),
                    }
                }
                Ok(PushEvent::Value { .. }) | Err(Error::Timeout) => {}
                Ok(PushEvent::End { reason }) => return reason,
                Err(error) => return error.to_string(),
            }
        }
    }
}
