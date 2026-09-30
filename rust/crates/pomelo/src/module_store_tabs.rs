use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};

use module_store::{format_size, Method, Options, Store};
use module_store_ui::{Request, StorePage, StoreState};
use winit::window::WindowId;

use crate::App;

#[derive(Default)]
pub(crate) struct ModuleStoreTabs {
    pages: Vec<(WindowId, module_store_ui::Shared)>,
    work: Option<Receiver<Finished>>,
    method: Option<String>,
}

struct Finished {
    entries: Vec<module_store::Entry>,
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

impl App {
    /// How the store reaches project folders here, given the user's fallback: probed once.
    pub(crate) fn module_store_method(&mut self) -> String {
        if let Some(method) = &self.module_store.method {
            return method.clone();
        }
        let supported = store().supported_method(&pom_paths::sessions_root());
        let options = Options::from_settings_file();
        let method = match options.method(supported) {
            Some(Method::Clone) => Method::Clone.label().to_string(),
            Some(method) => format!("{} (no copy-on-write on this drive)", method.label()),
            None => "None: installs run as usual (no copy-on-write on this drive)".to_string(),
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

    pub(crate) fn poll_module_store(&mut self) {
        self.module_store
            .pages
            .retain(|(_, shared)| Rc::strong_count(shared) > 1);
        if let Some(work) = &self.module_store.work {
            match work.try_recv() {
                Ok(finished) => {
                    self.module_store.work = None;
                    let stamp = now();
                    for (id, shared) in &self.module_store.pages {
                        let mut state = shared.borrow_mut();
                        state.entries = finished.entries.clone();
                        state.loaded = true;
                        state.busy = None;
                        state.message = finished.message.clone();
                        state.now = stamp;
                        state.changed();
                        if let Some(main) = self.mains.get_mut(id) {
                            main.dirty = true;
                        }
                    }
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.module_store.work = None,
            }
        }
        let request = self
            .module_store
            .pages
            .iter()
            .find_map(|(_, shared)| {
                let mut state = shared.borrow_mut();
                (!state.requests.is_empty()).then(|| std::mem::take(&mut state.requests))
            })
            .and_then(|requests| requests.into_iter().next());
        let Some(request) = request else {
            return;
        };
        let busy = match &request {
            Request::Refresh => "Measuring the store...".to_string(),
            Request::Delete { repo, .. } => format!("Removing the {repo} copy..."),
            Request::Prune => "Removing unused copies...".to_string(),
            Request::Clear => "Removing every copy...".to_string(),
        };
        for (id, shared) in &self.module_store.pages {
            let mut state = shared.borrow_mut();
            state.busy = Some(busy.clone());
            state.changed();
            if let Some(main) = self.mains.get_mut(id) {
                main.dirty = true;
            }
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        self.module_store.work = Some(receiver);
        std::thread::spawn(move || {
            let store = store();
            let freed = match request {
                Request::Refresh => Ok(None),
                Request::Delete { repo, key } => store.delete(&repo, &key).map(|freed| {
                    Some(format!(
                        "Removed the {repo} copy, freed {}",
                        format_size(freed)
                    ))
                }),
                Request::Prune => store.prune(&Options::from_settings_file()).map(|pruned| {
                    Some(format!(
                        "Pruned {} copies, freed {}",
                        pruned.removed,
                        format_size(pruned.freed)
                    ))
                }),
                Request::Clear => store.clear().map(|pruned| {
                    Some(format!(
                        "Removed {} copies, freed {}",
                        pruned.removed,
                        format_size(pruned.freed)
                    ))
                }),
            };
            let listed = store.list();
            let failure = freed
                .as_ref()
                .err()
                .or(listed.as_ref().err())
                .map(|error| format!("The store could not be read or changed: {error}"));
            let message = failure.or(freed.ok().flatten());
            let entries = listed.unwrap_or_default();
            let delivered = sender.send(Finished { entries, message });
            if delivered.is_ok() {
                ui::wake();
            }
        });
    }
}
