use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};

use module_store::{format_size, Method, Options, Overview, RepoWorktrees, Store, Worktree};
use module_store_ui::{Request, StorePage, StoreState};
use winit::window::WindowId;

use crate::App;

#[derive(Default)]
pub(crate) struct ModuleStoreTabs {
    pages: Vec<(WindowId, module_store_ui::Shared)>,
    work: Option<(WindowId, Receiver<Finished>)>,
    method: Option<String>,
}

struct Finished {
    overview: Overview,
    message: Option<String>,
}

fn store() -> Store {
    Store::new(&pom_paths::StateDir::from_env())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Every repo of the project with its worktree in each workspace; Node versions are read later, off
/// the main thread.
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

fn with_node_versions(mut repos: Vec<RepoWorktrees>) -> Vec<RepoWorktrees> {
    for repo in &mut repos {
        let probe = repo
            .worktrees
            .iter()
            .find(|worktree| worktree.is_main)
            .or(repo.worktrees.first());
        repo.node = probe.and_then(|worktree| pom_workspace::node_version(&worktree.path, &[]));
    }
    repos
}

fn node_for(repos: &[RepoWorktrees], repo: &str) -> Option<String> {
    repos
        .iter()
        .find(|found| found.repo == repo)
        .and_then(|found| found.node.clone())
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

    /// Workspaces of this window's project with services running, which a swap would pull files from under.
    fn running_workspace(&self, id: WindowId, workspace: &str) -> bool {
        let Some(app) = self.main_app.as_ref() else {
            return false;
        };
        let Some(main) = self.mains.get(&id) else {
            return false;
        };
        let view = main.entity.read(app.app());
        view.layout().project.as_ref().is_some_and(|project| {
            project
                .workspaces
                .iter()
                .position(|branch| branch == workspace)
                .and_then(|index| project.running.get(index))
                .is_some_and(|running| *running > 0)
        })
    }

    fn show_store_state(&mut self, id: WindowId, change: impl FnOnce(&mut StoreState)) {
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
        if let Some((id, work)) = &self.module_store.work {
            let id = *id;
            match work.try_recv() {
                Ok(finished) => {
                    self.module_store.work = None;
                    self.show_store_state(id, |state| {
                        state.overview = Some(finished.overview);
                        state.busy = None;
                        state.message = finished.message;
                        state.now = now();
                    });
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.module_store.work = None,
            }
        }
        let next = self.module_store.pages.iter().find_map(|(id, shared)| {
            let mut state = shared.borrow_mut();
            (!state.requests.is_empty()).then(|| (*id, state.requests.remove(0)))
        });
        let Some((id, request)) = next else {
            return;
        };
        if let Request::Relink { workspace, .. } = &request {
            if self.running_workspace(id, workspace) {
                let message = format!(
                    "Stop the services of {workspace} first: they are using its node_modules."
                );
                self.show_store_state(id, |state| state.message = Some(message));
                return;
            }
        }
        let Some(repos) = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .map(project_worktrees)
        else {
            return;
        };
        let busy = match &request {
            Request::Refresh => "Looking at every workspace...".to_string(),
            Request::FreeUnused => "Removing unused copies...".to_string(),
            Request::FreeOld { repo } => format!("Removing old {repo} copies..."),
            Request::FreeOthers => "Removing other projects' copies...".to_string(),
            Request::Relink { workspace, .. } => {
                format!("Swapping {workspace} to the shared copy...")
            }
            Request::Keep { repo, .. } => format!("Keeping {repo}'s install in the store..."),
        };
        self.show_store_state(id, |state| state.busy = Some(busy));
        let previous = self
            .module_store
            .pages
            .iter()
            .find(|(window, _)| *window == id)
            .and_then(|(_, shared)| shared.borrow().overview.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        self.module_store.work = Some((id, receiver));
        std::thread::spawn(move || {
            let store = store();
            let repos = with_node_versions(repos);
            let unused_of = |overview: Option<&Overview>, repo: Option<&str>, others: bool| {
                overview.map_or_else(Vec::new, |overview| {
                    let mut copies: Vec<(String, String)> = Vec::new();
                    for entry in overview.unused() {
                        let is_other = !overview.repos.iter().any(|known| known.repo == entry.repo);
                        let wanted = match repo {
                            Some(repo) => entry.repo == repo && !is_other,
                            None => !others || is_other,
                        };
                        if wanted {
                            copies.push((entry.repo.clone(), entry.key.clone()));
                        }
                    }
                    copies
                })
            };
            let done = match &request {
                Request::Refresh => Ok(None),
                Request::FreeUnused => store
                    .delete_all(&unused_of(previous.as_ref(), None, false))
                    .map(|freed| Some(format!("Freed {}", format_size(freed)))),
                Request::FreeOld { repo } => store
                    .delete_all(&unused_of(previous.as_ref(), Some(repo), false))
                    .map(|freed| {
                        Some(format!("Freed {} of old {repo} copies", format_size(freed)))
                    }),
                Request::FreeOthers => store
                    .delete_all(&unused_of(previous.as_ref(), None, true))
                    .map(|freed| Some(format!("Freed {}", format_size(freed)))),
                Request::Relink {
                    repo,
                    workspace,
                    path,
                } => store
                    .relink(repo, path, node_for(&repos, repo).as_deref())
                    .map(|freed| {
                        Some(format!(
                            "{workspace} now uses the shared copy; freed {}",
                            format_size(freed)
                        ))
                    }),
                Request::Keep { repo, path } => store
                    .keep(repo, path, node_for(&repos, repo).as_deref())
                    .map(|()| Some(format!("Kept {repo}'s install; new workspaces get it now"))),
            };
            let overview = store.overview(&repos);
            let failure = done
                .as_ref()
                .err()
                .or(overview.as_ref().err())
                .map(|error| format!("The store could not be read or changed: {error}"));
            let message = failure.or(done.ok().flatten());
            let finished = Finished {
                overview: overview.unwrap_or_default(),
                message,
            };
            if sender.send(finished).is_ok() {
                ui::wake();
            }
        });
    }
}
