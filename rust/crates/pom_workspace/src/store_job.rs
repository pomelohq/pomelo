//! One request of the node_modules Store page run against a store: the same steps whether the page's window
//! runs them in place or a project's server runs them on its machine.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use module_store::{
    format_size, Holding, Measure, Overview, RepoRules, RepoState, RepoWorktrees, Request, Store,
    StoreUpdate, Worktree,
};
use pom_config::Config;
use pom_layout::Workspace;
use pom_services::{ServiceRunner, ServiceTarget};

const MEASURE_THREADS: usize = 4;

/// Sizes of workspaces' own installs, by folder, valid while the folder's modification time holds.
pub type SizeCache = Arc<Mutex<HashMap<PathBuf, (Option<SystemTime>, u64)>>>;

/// Where updates go; false once nobody listens.
pub type StoreSink<'a> = &'a (dyn Fn(StoreUpdate) -> bool + Sync);

pub struct StoreJob {
    pub request: Request,
    pub repos: Vec<RepoWorktrees>,
    /// The overview the page showed, which frees and Optimize act on; read afresh when `None`.
    pub previous: Option<Overview>,
    pub services: Option<(Arc<ServiceRunner>, Arc<Config>)>,
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Every repo of the project with its worktree in each workspace.
pub fn store_worktrees(config: &Config, workspaces: &[Workspace]) -> Vec<RepoWorktrees> {
    config
        .repos
        .keys()
        .map(|repo| RepoWorktrees {
            repo: repo.clone(),
            worktrees: workspaces
                .iter()
                .filter_map(|workspace| {
                    let checkout = workspace.repos.iter().find(|found| found.name == *repo)?;
                    Some(Worktree {
                        workspace: workspace.branch.clone(),
                        is_main: workspace.is_main,
                        path: checkout.path.clone(),
                    })
                })
                .collect(),
            node: None,
            setup: crate::modules_rules(config, repo).setup,
        })
        .collect()
}

/// Each repo's Node version as `probe` reads it in its main worktree, asked in parallel.
pub fn with_node_versions(
    mut repos: Vec<RepoWorktrees>,
    probe: &(dyn Fn(&Path) -> Option<String> + Sync),
) -> Vec<RepoWorktrees> {
    let found: Vec<Option<String>> = std::thread::scope(|threads| {
        let asks: Vec<_> = repos
            .iter()
            .map(|repo| {
                let path = repo
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.is_main)
                    .or(repo.worktrees.first())
                    .map(|worktree| worktree.path.clone());
                threads.spawn(move || path.and_then(|path| probe(&path)))
            })
            .collect();
        asks.into_iter()
            .map(|ask| ask.join().unwrap_or(None))
            .collect()
    });
    for (repo, node) in repos.iter_mut().zip(found) {
        repo.node = node;
    }
    repos
}

fn node_for(repos: &[RepoWorktrees], repo: &str) -> Option<String> {
    repos
        .iter()
        .find(|found| found.repo == repo)
        .and_then(|found| found.node.clone())
}

fn rules_for(job: &StoreJob, repo: &str) -> RepoRules {
    job.services
        .as_ref()
        .map_or_else(RepoRules::default, |(_, config)| {
            crate::modules_rules(config, repo)
        })
}

/// `path` is `repo`'s worktree in one of the project's workspaces, so a request never reaches elsewhere.
fn check_worktree(job: &StoreJob, repo: &str, path: &Path) -> io::Result<()> {
    let known = job
        .repos
        .iter()
        .filter(|found| found.repo == repo)
        .flat_map(|found| &found.worktrees)
        .any(|worktree| worktree.path == path);
    if known {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a worktree of {repo}", path.display()),
        ))
    }
}

fn repo_targets(config: &Config, repo: &str, workspace: &str, is_main: bool) -> Vec<ServiceTarget> {
    config.repos.get(repo).map_or_else(Vec::new, |dir| {
        dir.services
            .keys()
            .map(|service| ServiceTarget {
                branch: workspace.to_string(),
                is_main,
                repo: repo.to_string(),
                service: service.clone(),
            })
            .collect()
    })
}

/// Workspaces with a workspace's own copy whose repo services run, which a swap would have to stop.
fn running_owners(
    overview: &Overview,
    services: Option<&(Arc<ServiceRunner>, Arc<Config>)>,
) -> Vec<(String, String)> {
    let Some((runner, config)) = services else {
        return Vec::new();
    };
    let mut running = Vec::new();
    for repo in &overview.repos {
        let RepoState::Versions { versions, .. } = &repo.state else {
            continue;
        };
        for user in versions.iter().flat_map(|version| &version.users) {
            if matches!(user.holding, Holding::Own { .. })
                && repo_targets(config, &repo.repo, &user.workspace, user.is_main)
                    .iter()
                    .any(|target| runner.is_running(target))
            {
                running.push((repo.repo.clone(), user.workspace.clone()));
            }
        }
    }
    running
}

/// The copies `request` frees, from the overview the page showed.
fn unused_copies(overview: Option<&Overview>, request: &Request) -> Vec<(String, String)> {
    let Some(overview) = overview else {
        return Vec::new();
    };
    overview
        .unused()
        .into_iter()
        .filter(|entry| {
            let other = !overview.repos.iter().any(|known| known.repo == entry.repo);
            match request {
                Request::FreeOld { repo } => entry.repo == *repo && !other,
                Request::FreeOthers => other,
                _ => true,
            }
        })
        .map(|entry| (entry.repo.clone(), entry.key.clone()))
        .collect()
}

/// Swaps a workspace's own install for the shared copy, stopping its repo's services around it.
fn relink(
    store: &Store,
    job: &StoreJob,
    repo: &str,
    workspace: &str,
    path: &Path,
) -> io::Result<u64> {
    check_worktree(job, repo, path)?;
    let is_main = job
        .repos
        .iter()
        .flat_map(|found| &found.worktrees)
        .any(|worktree| worktree.path == path && worktree.is_main);
    let stopped: Vec<ServiceTarget> =
        job.services
            .as_ref()
            .map_or_else(Vec::new, |(runner, config)| {
                repo_targets(config, repo, workspace, is_main)
                    .into_iter()
                    .filter(|target| runner.is_running(target))
                    .collect()
            });
    if let Some((runner, _)) = &job.services {
        for target in &stopped {
            runner.stop(target)?;
        }
    }
    let result = store.relink(
        repo,
        path,
        node_for(&job.repos, repo).as_deref(),
        &rules_for(job, repo),
    );
    if let Some((runner, config)) = &job.services {
        for target in &stopped {
            if let Err(error) = runner.start(config, target) {
                eprintln!(
                    "module store: restart {}/{}: {error}",
                    target.repo, target.service
                );
            }
        }
    }
    result
}

fn keep(store: &Store, job: &StoreJob, repo: &str, path: &Path) -> io::Result<()> {
    check_worktree(job, repo, path)?;
    store.keep(
        repo,
        path,
        node_for(&job.repos, repo).as_deref(),
        &rules_for(job, repo),
    )
}

/// One request; returns its result line and the row to highlight.
fn perform(store: &Store, job: &StoreJob) -> (io::Result<Option<String>>, Option<String>) {
    let mut flash = None;
    let done = match &job.request {
        Request::Refresh | Request::Optimize => Ok(None),
        request @ (Request::FreeUnused | Request::FreeOld { .. } | Request::FreeOthers) => store
            .delete_all(&unused_copies(job.previous.as_ref(), request))
            .map(|freed| Some(format!("Freed {}", format_size(freed)))),
        Request::Relink {
            repo,
            workspace,
            path,
        } => relink(store, job, repo, workspace, path).map(|freed| {
            flash = Some(workspace.clone());
            Some(format!(
                "{workspace} now uses the shared copy; freed up to {}",
                format_size(freed)
            ))
        }),
        Request::Keep { repo, path } => keep(store, job, repo, path)
            .map(|()| Some(format!("Kept {repo}'s install; new workspaces get it now"))),
    };
    (done, flash)
}

/// Every step of the plan, reporting each as it starts; failures do not stop the rest.
fn optimize(store: &Store, job: &StoreJob, send: StoreSink<'_>) -> io::Result<Option<String>> {
    let steps = job
        .previous
        .as_ref()
        .map_or_else(Vec::new, module_store::plan);
    let total = steps.len();
    let (mut kept, mut moved, mut freed, mut failed) = (0, 0, 0u64, Vec::new());
    for (index, step) in steps.iter().enumerate() {
        let started = StoreUpdate::Step {
            request: step.clone(),
            index: index + 1,
            total,
        };
        if !send(started) {
            break;
        }
        let result = match step {
            Request::Relink {
                repo,
                workspace,
                path,
            } => relink(store, job, repo, workspace, path).map(|bytes| {
                moved += 1;
                freed += bytes;
            }),
            Request::Keep { repo, path } => keep(store, job, repo, path).map(|()| kept += 1),
            Request::FreeUnused => store
                .delete_all(&unused_copies(job.previous.as_ref(), step))
                .map(|bytes| freed += bytes),
            _ => Ok(()),
        };
        if let Err(error) = result {
            failed.push(format!("{}: {error}", step.working()));
        }
    }
    let mut summary = format!(
        "Optimized: {kept} kept, {moved} moved onto the shared copy, freed up to {}",
        format_size(freed)
    );
    if !failed.is_empty() {
        summary.push_str(&format!(
            "; {} failed ({})",
            failed.len(),
            failed.join("; ")
        ));
    }
    Ok(Some(summary))
}

/// Runs `job`, then sends the overview after it and each size as it is measured.
pub fn run_store_job(store: &Store, mut job: StoreJob, sizes: &SizeCache, send: StoreSink<'_>) {
    if job.previous.is_none() && job.request.reads_overview() {
        job.previous = store.overview(&job.repos).ok();
    }
    let (done, mut flash) = if job.request == Request::Optimize {
        (optimize(store, &job, send), None)
    } else {
        perform(store, &job)
    };
    let overview = store.overview(&job.repos);
    if let (Request::Keep { path, .. }, Ok(overview)) = (&job.request, &overview) {
        flash = overview.repos.iter().find_map(|repo| match &repo.state {
            RepoState::Versions { versions, .. } => versions
                .iter()
                .find(|version| version.users.iter().any(|user| user.path == *path))
                .map(|version| version.key.clone()),
            _ => None,
        });
    }
    let failure = done
        .as_ref()
        .err()
        .or(overview.as_ref().err())
        .map(|error| format!("The store could not be read or changed: {error}"));
    let failed = failure.is_some();
    let message = failure.or(done.ok().flatten());
    let overview = overview.unwrap_or_default();
    let mut pending = store.to_measure(&overview);
    // Own installs unchanged since they were last measured need no walk.
    let cached: Vec<(Measure, u64)> = {
        let known = sizes.lock().map(|guard| guard.clone()).unwrap_or_default();
        pending
            .iter()
            .filter_map(|what| match what {
                Measure::Own { path } => known
                    .get(path)
                    .filter(|(stamp, _)| *stamp == modified(path))
                    .map(|(_, size)| (what.clone(), *size)),
                Measure::Stored { .. } => None,
            })
            .collect()
    };
    pending.retain(|what| !cached.iter().any(|(known, _)| known == what));
    let update = StoreUpdate::Overview {
        running: running_owners(&overview, job.services.as_ref()),
        overview,
        pending: pending.len() + cached.len(),
        message,
        failed,
        flash,
    };
    if !send(update) {
        return;
    }
    for (what, size) in cached {
        if !send(StoreUpdate::Measured { what, size }) {
            return;
        }
    }
    let queue = Mutex::new(pending);
    std::thread::scope(|threads| {
        for _ in 0..MEASURE_THREADS {
            threads.spawn(|| loop {
                let Some(what) = queue.lock().ok().and_then(|mut queue| queue.pop()) else {
                    return;
                };
                let size = match store.measure(&what) {
                    Ok(size) => size,
                    Err(error) => {
                        eprintln!("module store: measure: {error}");
                        0
                    }
                };
                if let Measure::Own { path } = &what {
                    if let Ok(mut known) = sizes.lock() {
                        known.insert(path.clone(), (modified(path), size));
                    }
                }
                if !send(StoreUpdate::Measured { what, size }) {
                    return;
                }
            });
        }
    });
}
