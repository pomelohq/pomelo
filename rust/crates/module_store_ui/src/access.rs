//! Where the Store page's requests run: this machine's store and the window's project, or a project's
//! server on another machine. Both report the same updates, so the page draws and behaves alike.

use std::sync::Arc;

use module_store::{Overview, RepoWorktrees, Request, Store, StoreUpdate};
use pom_config::Config;
use pom_services::ServiceRunner;
use pom_workspace::{SizeCache, StoreJob};

/// Takes each update; false once nobody listens.
pub type Updates = Box<dyn Fn(StoreUpdate) -> bool + Send + Sync>;

pub trait StoreAccess: Send + Sync {
    /// Runs `request` off the calling thread, reporting through `send` and dropping it when done.
    /// `previous` is the overview the page showed, which frees and Optimize act on.
    fn start(&self, request: Request, previous: Option<Overview>, send: Updates);
}

/// This machine's store, for a project open here.
pub struct LocalStore {
    pub repos: Vec<RepoWorktrees>,
    pub services: Option<(Arc<ServiceRunner>, Arc<Config>)>,
    pub sizes: SizeCache,
}

impl StoreAccess for LocalStore {
    fn start(&self, request: Request, previous: Option<Overview>, send: Updates) {
        let repos = self.repos.clone();
        let services = self.services.clone();
        let sizes = self.sizes.clone();
        std::thread::spawn(move || {
            let store = Store::new(&pom_paths::StateDir::from_env());
            let repos = pom_workspace::with_node_versions(repos, &|path| {
                pom_workspace::node_version(path, &[])
            });
            let job = StoreJob {
                request,
                repos,
                previous,
                services,
            };
            pom_workspace::run_store_job(&store, job, &sizes, &*send);
        });
    }
}
