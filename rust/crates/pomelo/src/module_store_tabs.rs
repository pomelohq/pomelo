use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::SystemTime;

use module_store::{Options, Request, Store, StoreUpdate};
use module_store_ui::{LocalStore, RemoteStore, StoreAccess, StorePage, StoreState};
use winit::window::WindowId;

use crate::App;

#[derive(Default)]
pub(crate) struct ModuleStoreTabs {
    pages: Vec<(WindowId, module_store_ui::Shared)>,
    work: Option<(WindowId, Receiver<StoreUpdate>)>,
    method: Option<String>,
    sizes: pom_workspace::SizeCache,
}

fn store() -> Store {
    Store::new(&pom_paths::StateDir::from_env())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl App {
    /// How the store reaches project folders here, given the user's fallback: probed once.
    pub(crate) fn module_store_method(&mut self) -> String {
        if let Some(method) = &self.module_store.method {
            return method.clone();
        }
        let supported = store().supported_method(&pom_paths::sessions_root());
        let method = module_store::describe_method(Options::from_settings_file().method(supported))
            .to_string();
        self.module_store.method = Some(method.clone());
        method
    }

    /// Settings changed: the fallback may change what the method line says.
    pub(crate) fn forget_module_store_method(&mut self) {
        self.module_store.method = None;
    }

    pub(crate) fn open_module_store(&mut self, id: WindowId) {
        let remote = self
            .mains
            .get(&id)
            .is_some_and(|main| main.project.is_none() && main.remote.is_some());
        let method = if remote {
            String::new()
        } else {
            self.module_store_method()
        };
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
            if !remote {
                state.method = method;
            }
            state.now = now();
            state.requests.push(Request::Refresh);
            state.changed();
        }
        let root = if remote {
            String::new()
        } else {
            store().root().display().to_string()
        };
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
                    Ok(update) => {
                        let now = now();
                        self.with_store_state(id, |state| state.apply(update, now));
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        open = false;
                        self.with_store_state(id, StoreState::ended);
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
        let Some(access) = self.module_store_access(id) else {
            self.with_store_state(id, |state| {
                state.unavailable = Some(
                    "Open a project in this window to see how its workspaces use the store."
                        .to_string(),
                );
                state.ended();
            });
            return;
        };
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
        let (sender, receiver) = std::sync::mpsc::channel();
        self.module_store.work = Some((id, receiver));
        access.start(
            request,
            previous,
            Box::new(move |update| {
                let open = sender.send(update).is_ok();
                ui::wake();
                open
            }),
        );
    }

    /// The window's store: this machine's for a project open here, the host's for a remote project.
    fn module_store_access(&self, id: WindowId) -> Option<Arc<dyn StoreAccess>> {
        let main = self.mains.get(&id)?;
        if let Some(project) = &main.project {
            let repos = project.config.as_ref().map_or_else(Vec::new, |config| {
                pom_workspace::store_worktrees(config, &project.workspaces)
            });
            let services = main.services.as_ref().and_then(|services| {
                let config = services.config.read().ok()?.clone()?;
                Some((services.runner.clone(), config))
            });
            return Some(Arc::new(LocalStore {
                repos,
                services,
                sizes: self.module_store.sizes.clone(),
            }));
        }
        let remote = main.remote.as_ref()?;
        Some(Arc::new(RemoteStore::new(
            remote.watch.source(),
            &remote.host,
        )))
    }
}
