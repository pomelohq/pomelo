use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use module_store::{
    format_size, Holding, Measure, Method, Options, Overview, RepoState, RepoWorktrees, Store,
    Worktree,
};
use module_store_ui::{Request, StorePage, StoreState};
use pom_services::{ServiceRunner, ServiceTarget};
use winit::window::WindowId;

use crate::App;

const MEASURE_THREADS: usize = 4;

/// Sizes of workspaces' own installs, by folder, valid while the folder's modification time holds.
type SizeCache = Arc<Mutex<HashMap<PathBuf, (Option<SystemTime>, u64)>>>;

#[derive(Default)]
pub(crate) struct ModuleStoreTabs {
    pages: Vec<(WindowId, module_store_ui::Shared)>,
    work: Option<(WindowId, Receiver<Update>)>,
    method: Option<String>,
    sizes: SizeCache,
}

enum Update {
    Overview {
        overview: Overview,
        running: Vec<(String, String)>,
        pending: usize,
        message: Option<String>,
        failed: bool,
        flash: Option<String>,
    },
    Measured(Measure, u64),
}

/// What the worker needs from the window's project.
struct Job {
    request: Request,
    repos: Vec<RepoWorktrees>,
    previous: Option<Overview>,
    services: Option<(Arc<ServiceRunner>, Arc<pom_config::Config>)>,
}

fn store() -> Store {
    Store::new(&pom_paths::StateDir::from_env())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Every repo of the project with its worktree in each workspace.
fn project_worktrees(project: &pom_core::Project) -> Vec<RepoWorktrees> {
    let Some(config) = &project.config else {
        return Vec::new();
    };
    config
        .repos
        .keys()
        .map(|repo| RepoWorktrees {
            repo: repo.clone(),
            worktrees: project
                .workspaces
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
        })
        .collect()
}

/// Each repo's Node version, asked in parallel (a login shell per repo).
fn with_node_versions(mut repos: Vec<RepoWorktrees>) -> Vec<RepoWorktrees> {
    let found: Vec<Option<String>> = std::thread::scope(|threads| {
        let asks: Vec<_> = repos
            .iter()
            .map(|repo| {
                let probe = repo
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.is_main)
                    .or(repo.worktrees.first())
                    .map(|worktree| worktree.path.clone());
                threads
                    .spawn(move || probe.and_then(|path| pom_workspace::node_version(&path, &[])))
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

fn repo_targets(
    config: &pom_config::Config,
    repo: &str,
    workspace: &str,
    is_main: bool,
) -> Vec<ServiceTarget> {
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
    services: Option<&(Arc<ServiceRunner>, Arc<pom_config::Config>)>,
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
    job: &Job,
    repo: &str,
    workspace: &str,
    path: &Path,
) -> std::io::Result<u64> {
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
    let result = store.relink(repo, path, node_for(&job.repos, repo).as_deref());
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

fn run(job: Job, sizes: SizeCache, sender: Sender<Update>) {
    let store = store();
    let repos = with_node_versions(job.repos.clone());
    let job = Job { repos, ..job };
    let mut flash = None;
    let done: std::io::Result<Option<String>> = match &job.request {
        Request::Refresh => Ok(None),
        Request::FreeUnused | Request::FreeOld { .. } | Request::FreeOthers => store
            .delete_all(&unused_copies(job.previous.as_ref(), &job.request))
            .map(|freed| Some(format!("Freed {}", format_size(freed)))),
        Request::Relink {
            repo,
            workspace,
            path,
        } => relink(&store, &job, repo, workspace, path).map(|freed| {
            flash = Some(workspace.clone());
            Some(format!(
                "{workspace} now uses the shared copy; freed up to {}",
                format_size(freed)
            ))
        }),
        Request::Keep { repo, path } => store
            .keep(repo, path, node_for(&job.repos, repo).as_deref())
            .map(|()| Some(format!("Kept {repo}'s install; new workspaces get it now"))),
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
    let update = Update::Overview {
        running: running_owners(&overview, job.services.as_ref()),
        overview,
        pending: pending.len() + cached.len(),
        message,
        failed,
        flash,
    };
    if sender.send(update).is_err() {
        return;
    }
    for (what, size) in cached {
        if sender.send(Update::Measured(what, size)).is_err() {
            return;
        }
    }
    ui::wake();
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
                if sender.send(Update::Measured(what, size)).is_err() {
                    return;
                }
                ui::wake();
            });
        }
    });
}

impl App {
    /// How the store reaches project folders here, given the user's fallback: probed once.
    pub(crate) fn module_store_method(&mut self) -> String {
        if let Some(method) = &self.module_store.method {
            return method.clone();
        }
        let supported = store().supported_method(&pom_paths::sessions_root());
        let method = match Options::from_settings_file().method(supported) {
            Some(Method::Clone) => "copy-on-write, workspaces take no extra disk".to_string(),
            Some(Method::HardLink) => "hard links, workspaces share read-only files".to_string(),
            Some(Method::Copy) => "copies, each workspace takes the full size".to_string(),
            None => "off on this drive, installs run as usual".to_string(),
        };
        self.module_store.method = Some(method.clone());
        method
    }

    /// Settings changed: the fallback may change what the method line says.
    pub(crate) fn forget_module_store_method(&mut self) {
        self.module_store.method = None;
    }

    pub(crate) fn open_module_store(&mut self, id: WindowId) {
        let method = self.module_store_method();
        let shared = match self
            .module_store
            .pages
            .iter()
            .find(|(window, _)| *window == id)
        {
            Some((_, shared)) => shared.clone(),
            None => {
                let shared: module_store_ui::Shared = Rc::new(RefCell::new(StoreState::default()));
                self.module_store.pages.push((id, shared.clone()));
                shared
            }
        };
        {
            let mut state = shared.borrow_mut();
            state.method = method;
            state.now = now();
            state.requests.push(Request::Refresh);
            state.changed();
        }
        let root = store().root().display().to_string();
        let page = StorePage::new(shared, root);
        self.with_workspace_view(id, |view, _| view.open_page(Box::new(page)));
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
        self.poll_module_store();
    }

    fn with_store_state(&mut self, id: WindowId, change: impl FnOnce(&mut StoreState)) {
        if let Some((_, shared)) = self
            .module_store
            .pages
            .iter()
            .find(|(window, _)| *window == id)
        {
            let mut state = shared.borrow_mut();
            change(&mut state);
            state.changed();
        }
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    pub(crate) fn poll_module_store(&mut self) {
        self.module_store
            .pages
            .retain(|(_, shared)| Rc::strong_count(shared) > 1);
        if let Some((id, work)) = self.module_store.work.take() {
            let mut open = true;
            loop {
                match work.try_recv() {
                    Ok(Update::Overview {
                        overview,
                        running,
                        pending,
                        message,
                        failed,
                        flash,
                    }) => self.with_store_state(id, |state| {
                        state.overview = Some(overview);
                        state.running = running;
                        state.measuring = pending;
                        state.measure_total = pending;
                        state.now = now();
                        state.finish(message, failed, flash);
                    }),
                    Ok(Update::Measured(what, size)) => self.with_store_state(id, |state| {
                        if let Some(overview) = state.overview.as_mut() {
                            overview.apply(&what, size);
                        }
                        state.measuring = state.measuring.saturating_sub(1);
                    }),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        open = false;
                        self.with_store_state(id, |state| {
                            state.measuring = 0;
                            state.busy = None;
                        });
                        break;
                    }
                }
            }
            if open {
                self.module_store.work = Some((id, work));
            }
        }
        let busy = self
            .module_store
            .work
            .as_ref()
            .and_then(|(id, _)| {
                self.module_store
                    .pages
                    .iter()
                    .find(|(window, _)| window == id)
                    .map(|(_, shared)| shared.borrow().busy.is_some())
            })
            .unwrap_or(false);
        if busy {
            return;
        }
        let next = self.module_store.pages.iter().find_map(|(id, shared)| {
            let mut state = shared.borrow_mut();
            (!state.requests.is_empty()).then(|| (*id, state.requests.remove(0)))
        });
        let Some((id, request)) = next else {
            return;
        };
        let Some(main) = self.mains.get(&id) else {
            return;
        };
        let Some(repos) = main.project.as_ref().map(project_worktrees) else {
            return;
        };
        let services = main.services.as_ref().and_then(|services| {
            let config = services.config.read().ok()?.clone()?;
            Some((services.runner.clone(), config))
        });
        let previous = self
            .module_store
            .pages
            .iter()
            .find(|(window, _)| *window == id)
            .and_then(|(_, shared)| shared.borrow().overview.clone());
        let busy = request.clone();
        self.with_store_state(id, |state| {
            state.busy = Some(busy);
            state.confirm = None;
        });
        let job = Job {
            request,
            repos,
            previous,
            services,
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        self.module_store.work = Some((id, receiver));
        let sizes = self.module_store.sizes.clone();
        std::thread::spawn(move || run(job, sizes, sender));
    }
}
