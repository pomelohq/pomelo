//! Sync on push. GitHub posts a signed `push` to the webhook relay's `/_pom/github`; the delivery is checked
//! against each project's webhook secret, mapped to the workspaces that have the pushed repo on the pushed
//! branch, and each of those syncs once per burst on a worker of its own (see `sync.on_push`).

mod apply;
mod journal;
mod payload;
mod signature;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use pom_config::{Config, OnPush};
use pom_layout::Workspace;
use pom_paths::StateDir;
use pom_proxy::{GithubDelivery, GithubHook, GithubReply};
use pom_ptyhost::SocketDir;
use pom_secrets::SecretStore;
use pom_services::{RunnerOptions, ServiceRunner};

pub use apply::remote_ahead;
pub use journal::{journal_path, last_syncs, now_ms, read_all, recent, utc_text, Outcome, Record};
pub use payload::{Push, Repository};
pub use signature::{sign, verify};

/// Where each project keeps its webhook secret: its own secret store, hidden from `secrets_list`.
pub const SECRET_NAME: &str = "__github_webhook";
/// A burst of pushes to one workspace (a rebase pushed commit by commit, several repos at once) syncs once.
pub const DEBOUNCE: Duration = Duration::from_secs(5);
const LOCK_WAIT: Duration = Duration::from_secs(120);
const MANUAL: &str = "manual";

/// What a sync needs of the machine: where state lives, the holders, and the binary that hosts them.
#[derive(Clone, Debug)]
pub struct Machine {
    pub state: StateDir,
    pub holders: SocketDir,
    pub binary: PathBuf,
}

impl Machine {
    fn runner(&self, root: &Path, config: &Config) -> ServiceRunner {
        ServiceRunner::new(RunnerOptions {
            project_root: root.to_path_buf(),
            session: config.session.clone(),
            state: self.state.clone(),
            holders: self.holders.clone(),
            binary: self.binary.clone(),
            docker: "docker".into(),
        })
    }
}

/// A workspace: its project root and branch.
type Key = (PathBuf, String);

#[derive(Default)]
struct Pending {
    due: Option<Instant>,
    /// Repo key -> the git branch pushed.
    repos: BTreeMap<String, String>,
    deliveries: Vec<String>,
    repository: String,
}

#[derive(Default)]
struct Queue {
    pending: HashMap<Key, Pending>,
    running: HashSet<Key>,
}

struct Inner {
    machine: Machine,
    debounce: Duration,
    queue: Mutex<Queue>,
    changed: Condvar,
}

/// Receives verified pushes and runs each workspace's sync on a worker, one at a time per workspace.
#[derive(Clone)]
pub struct PushSync {
    inner: Arc<Inner>,
}

fn reply(status: u16, body: serde_json::Value, repo: &str, summary: String) -> GithubReply {
    GithubReply {
        status,
        body: body.to_string(),
        repo: repo.to_string(),
        summary,
    }
}

fn origin_of(worktree: &Path) -> Option<String> {
    pom_sync::git::git(
        worktree,
        &["remote", "get-url", "origin"],
        pom_sync::git::LOCAL,
    )
    .ok()
    .filter(|url| !url.is_empty())
    .map(|url| pom_layout::normalize_remote(&url))
}

fn workspaces(root: &Path, config: &Config) -> Vec<Workspace> {
    let known: HashSet<String> = config.repos.keys().cloned().collect();
    pom_layout::scan(root, config.global_default_branch(), Some(&known))
}

/// The repos of the project whose `origin` is one of `remotes`.
fn matching_repos(root: &Path, config: &Config, remotes: &[String]) -> Vec<String> {
    let main = config.global_default_branch();
    let all = workspaces(root, config);
    config
        .repos
        .keys()
        .filter(|repo| {
            let main_worktree = pom_layout::repo_worktree(root, repo, main, true);
            let origin = origin_of(&main_worktree).or_else(|| {
                all.iter()
                    .flat_map(|workspace| workspace.repos.iter())
                    .filter(|entry| &entry.name == *repo)
                    .find_map(|entry| origin_of(&entry.path))
            });
            origin.is_some_and(|origin| remotes.contains(&origin))
        })
        .cloned()
        .collect()
}

fn lock_name(branch: &str) -> String {
    let safe: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("push-{safe}")
}

impl PushSync {
    pub fn start(machine: Machine) -> PushSync {
        PushSync::with_debounce(machine, DEBOUNCE)
    }

    pub fn with_debounce(machine: Machine, debounce: Duration) -> PushSync {
        let sync = PushSync {
            inner: Arc::new(Inner {
                machine,
                debounce,
                queue: Mutex::new(Queue::default()),
                changed: Condvar::new(),
            }),
        };
        let worker = sync.clone();
        let spawned = std::thread::Builder::new()
            .name("push-sync".into())
            .spawn(move || worker.dispatch());
        if let Err(error) = spawned {
            eprintln!("sync on push: cannot start its worker: {error}");
        }
        sync
    }

    /// For the relay: `/_pom/github` deliveries go to [`PushSync::receive`].
    pub fn hook(&self) -> GithubHook {
        let sync = self.clone();
        Arc::new(move |delivery, projects| sync.receive(delivery, projects))
    }

    fn state(&self) -> &StateDir {
        &self.inner.machine.state
    }

    /// Checks the signature against every project's secret, then answers and queues what the event asks for.
    pub fn receive(
        &self,
        delivery: &GithubDelivery,
        projects: &[(PathBuf, Arc<Config>)],
    ) -> GithubReply {
        let mut configured = false;
        let trusted: Vec<&(PathBuf, Arc<Config>)> = projects
            .iter()
            .filter(|(_, config)| {
                let secret = SecretStore::new(self.state().clone(), &config.session)
                    .get(SECRET_NAME)
                    .ok()
                    .flatten();
                configured |= secret.is_some();
                secret.is_some_and(|secret| verify(&secret, &delivery.body, &delivery.signature))
            })
            .collect();
        if !configured {
            return reply(
                403,
                serde_json::json!({"ok": false, "error": "no project has a webhook secret: run `pom github webhook setup` in the project"}),
                "",
                "rejected: no webhook secret".into(),
            );
        }
        if trusted.is_empty() {
            return reply(
                401,
                serde_json::json!({"ok": false, "error": "the X-Hub-Signature-256 signature does not match"}),
                "",
                "rejected: bad signature".into(),
            );
        }
        match delivery.event.as_str() {
            "ping" => {
                let mut record = Record::new("ping", Outcome::Pong);
                record.deliveries.push(delivery.id.clone());
                journal::append(self.state(), &record);
                reply(
                    200,
                    serde_json::json!({"ok": true, "event": "ping"}),
                    "",
                    "ping".into(),
                )
            }
            "push" => self.receive_push(delivery, &trusted),
            other => reply(
                202,
                serde_json::json!({"ok": true, "ignored": other}),
                "",
                format!("{other}: ignored"),
            ),
        }
    }

    fn receive_push(
        &self,
        delivery: &GithubDelivery,
        trusted: &[&(PathBuf, Arc<Config>)],
    ) -> GithubReply {
        let form = delivery
            .content_type
            .starts_with("application/x-www-form-urlencoded");
        let push = match Push::parse(&delivery.body, form) {
            Ok(push) => push,
            Err(error) => {
                return reply(
                    400,
                    serde_json::json!({"ok": false, "error": error}),
                    "",
                    "unreadable push".into(),
                )
            }
        };
        let repository = push.repository.full_name.clone();
        let Some(branch) = push.branch().map(str::to_string) else {
            return reply(
                202,
                serde_json::json!({"ok": true, "ignored": push.reference}),
                &repository,
                format!("{}: not a branch", push.reference),
            );
        };
        let remotes = push.repository.remotes();
        let base = |outcome: Outcome| Record {
            deliveries: vec![delivery.id.clone()],
            repository: repository.clone(),
            branch: branch.clone(),
            after: push.after.clone(),
            ..Record::new("push", outcome)
        };
        let mut reached = Vec::new();
        for (root, config) in trusted {
            let mode = config.on_push();
            for repo in matching_repos(root, config, &remotes) {
                for workspace in workspaces(root, config).into_iter().filter(|workspace| {
                    workspace
                        .repos
                        .iter()
                        .any(|entry| entry.name == repo && entry.branch == branch)
                }) {
                    let outcome = match mode {
                        OnPush::Off => Outcome::Off,
                        _ if push.deleted => Outcome::Deleted,
                        _ => Outcome::Queued,
                    };
                    let record = Record {
                        project: config.session.clone(),
                        root: root.display().to_string(),
                        workspace: workspace.branch.clone(),
                        repo: repo.clone(),
                        detail: format!("sync.on_push: {}", mode.name()),
                        ..base(outcome)
                    };
                    journal::append(self.state(), &record);
                    if outcome == Outcome::Queued {
                        self.enqueue(
                            (root.clone(), workspace.branch.clone()),
                            &repo,
                            &branch,
                            &delivery.id,
                            &repository,
                        );
                    }
                    reached.push(serde_json::json!({
                        "project": config.session,
                        "workspace": workspace.branch,
                        "repo": repo,
                        "action": outcome.name(),
                        "on_push": mode.name(),
                    }));
                }
            }
        }
        if reached.is_empty() {
            journal::append(self.state(), &base(Outcome::NoMatch));
        }
        let summary = format!("push {branch}: {} workspaces", reached.len());
        reply(
            200,
            serde_json::json!({"ok": true, "event": "push", "branch": branch, "workspaces": reached}),
            &repository,
            summary,
        )
    }

    fn enqueue(&self, key: Key, repo: &str, branch: &str, delivery: &str, repository: &str) {
        let Ok(mut queue) = self.inner.queue.lock() else {
            return;
        };
        let debounce = self.inner.debounce;
        let pending = queue.pending.entry(key).or_default();
        pending.due.get_or_insert_with(|| Instant::now() + debounce);
        pending.repos.insert(repo.to_string(), branch.to_string());
        pending.deliveries.push(delivery.to_string());
        pending.repository = repository.to_string();
        self.inner.changed.notify_all();
    }

    fn dispatch(&self) {
        loop {
            let Ok(mut queue) = self.inner.queue.lock() else {
                return;
            };
            let now = Instant::now();
            let ready = queue
                .pending
                .iter()
                .filter(|(key, pending)| {
                    !queue.running.contains(*key) && pending.due.is_some_and(|due| due <= now)
                })
                .map(|(key, _)| key.clone())
                .next();
            if let Some(key) = ready {
                let Some(pending) = queue.pending.remove(&key) else {
                    continue;
                };
                queue.running.insert(key.clone());
                drop(queue);
                let sync = self.clone();
                let spawned = std::thread::Builder::new()
                    .name("push-sync-job".into())
                    .spawn(move || {
                        sync.run(&key, pending);
                        if let Ok(mut queue) = sync.inner.queue.lock() {
                            queue.running.remove(&key);
                        }
                        sync.inner.changed.notify_all();
                    });
                if let Err(error) = spawned {
                    eprintln!("sync on push: cannot start a sync: {error}");
                }
                continue;
            }
            let wait = queue
                .pending
                .values()
                .filter_map(|pending| pending.due)
                .map(|due| due.saturating_duration_since(now))
                .min()
                .unwrap_or(Duration::from_secs(60))
                .max(Duration::from_millis(20));
            if let Err(error) = self.inner.changed.wait_timeout(queue, wait) {
                eprintln!("sync on push: worker stopped: {error}");
                return;
            }
        }
    }

    fn run(&self, (root, branch): &Key, pending: Pending) {
        let template = Record {
            deliveries: pending.deliveries.clone(),
            repository: pending.repository.clone(),
            root: root.display().to_string(),
            workspace: branch.clone(),
            ..Record::new("push", Outcome::Failed)
        };
        for record in sync_workspace(
            &self.inner.machine,
            root,
            branch,
            &pending.repos,
            None,
            &template,
        ) {
            journal::append(self.state(), &record);
        }
    }
}

/// Holds the workspace's sync lock (another process may be pulling it from the app), waiting a while for it.
fn sync_lock(machine: &Machine, session: &str, branch: &str) -> Option<pom_lock::LockGuard> {
    let dir = machine.state.path("sync/locks");
    let started = Instant::now();
    loop {
        match pom_lock::try_acquire(&dir, session, &lock_name(branch)) {
            Ok(Some(guard)) => return Some(guard),
            Ok(None) if started.elapsed() < LOCK_WAIT => {
                std::thread::sleep(Duration::from_millis(250))
            }
            Ok(None) => return None,
            Err(error) => {
                eprintln!("sync on push: lock {}: {error}", dir.display());
                return None;
            }
        }
    }
}

/// Syncs `repos` (repo key -> git branch) of the workspace `branch`; `mode` overrides `sync.on_push`.
fn sync_workspace(
    machine: &Machine,
    root: &Path,
    branch: &str,
    repos: &BTreeMap<String, String>,
    mode: Option<OnPush>,
    template: &Record,
) -> Vec<Record> {
    let failed = |repo: &str, detail: String| Record {
        repo: repo.to_string(),
        detail,
        ..template.clone()
    };
    let config = match Config::load(&root.join(pom_config::CONFIG_FILE_NAME)) {
        Ok(config) => config,
        Err(error) => {
            return repos
                .keys()
                .map(|repo| failed(repo, error.to_string()))
                .collect()
        }
    };
    let Some(_lock) = sync_lock(machine, &config.session, branch) else {
        return repos
            .keys()
            .map(|repo| {
                failed(
                    repo,
                    "another sync of this workspace is still running".into(),
                )
            })
            .collect();
    };
    let Some(workspace) = workspaces(root, &config)
        .into_iter()
        .find(|workspace| workspace.branch == branch)
    else {
        return repos
            .keys()
            .map(|repo| failed(repo, format!("workspace {branch} is gone")))
            .collect();
    };
    let runner = machine.runner(root, &config);
    let job = apply::Job {
        config: &config,
        runner: &runner,
        workspace: &workspace,
        mode: mode.unwrap_or_else(|| config.on_push()),
    };
    repos
        .iter()
        .map(|(repo, git_branch)| {
            let mut record = Record {
                project: config.session.clone(),
                ..Record::new("push", Outcome::Failed)
            };
            record.deliveries = template.deliveries.clone();
            record.repository = template.repository.clone();
            record.root = template.root.clone();
            job.sync(repo, git_branch, &mut record);
            record
        })
        .collect()
}

/// Pulls every repo of the workspace `branch` now, as `sync.on_push: pull` would (the app's Pull button).
pub fn pull_now(machine: &Machine, root: &Path, branch: &str) -> Vec<Record> {
    let config = match Config::load(&root.join(pom_config::CONFIG_FILE_NAME)) {
        Ok(config) => config,
        Err(error) => {
            let mut record = Record::new(MANUAL, Outcome::Failed);
            record.workspace = branch.to_string();
            record.detail = error.to_string();
            return vec![record];
        }
    };
    let repos: BTreeMap<String, String> = workspaces(root, &config)
        .into_iter()
        .find(|workspace| workspace.branch == branch)
        .map(|workspace| {
            workspace
                .repos
                .into_iter()
                .filter(|repo| config.repos.contains_key(&repo.name))
                .filter_map(|repo| match pom_layout::read_head(&repo.path) {
                    Some(pom_layout::Head::Branch(name)) => Some((repo.name, name)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let template = Record {
        deliveries: vec![MANUAL.into()],
        root: root.display().to_string(),
        workspace: branch.to_string(),
        ..Record::new(MANUAL, Outcome::Failed)
    };
    let records = sync_workspace(machine, root, branch, &repos, Some(OnPush::Pull), &template);
    for record in &records {
        journal::append(&machine.state, record);
    }
    records
}

/// The project's webhook secret: made and stored when there is none (or `rotate`); `Some` only when new.
pub fn ensure_secret(
    state: &StateDir,
    session: &str,
    rotate: bool,
) -> Result<Option<String>, String> {
    let store = SecretStore::new(state.clone(), session);
    let current = store.get(SECRET_NAME).map_err(|error| error.to_string())?;
    if current.is_some() && !rotate {
        return Ok(None);
    }
    let secret = pom_secrets::random_hex(32);
    store
        .set(SECRET_NAME, &secret)
        .map_err(|error| error.to_string())?;
    Ok(Some(secret))
}

/// The payload URL to give GitHub, and whether other machines can reach it as configured. Behind a reverse
/// proxy (a public port other than the proxy's own) `hooks.<domain>` is expected to route to the relay.
pub fn payload_url(network: &pom_env::DevNetwork) -> (String, bool) {
    let public = network.public();
    if public.is_local() {
        return (
            format!(
                "http://127.0.0.1:{}{}",
                network.ports.webhook,
                pom_proxy::GITHUB_PATH
            ),
            false,
        );
    }
    let host = format!("hooks.{}", network.domain);
    let base = if network.public_port == network.ports.proxy {
        let scheme = if network.https { "https" } else { "http" };
        format!("{scheme}://{host}:{}", network.ports.webhook)
    } else {
        public.url(&host)
    };
    (format!("{base}{}", pom_proxy::GITHUB_PATH), true)
}
