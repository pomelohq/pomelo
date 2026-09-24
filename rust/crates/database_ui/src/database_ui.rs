//! The Database tool window: the active workspace's databases as a tree, and table tabs that page through a
//! table's rows in a grid. Everything talks to the databases on background threads.

mod grid;
mod panel;
mod table_item;

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use pom_config::Config;
use pom_db::{Connector, Database};
use pom_services::ServiceRunner;

pub use grid::{Grid, GridEvent, Region};
pub use panel::DatabasePanel;
pub use table_item::TableItem;

/// What the Database views need: the project's runner (ports, slots), its current config, the workspace's
/// branch, and a way to wake the window when background work lands.
#[derive(Clone)]
pub struct DatabaseContext {
    pub runner: Arc<ServiceRunner>,
    pub config: Arc<dyn Fn() -> Option<Arc<Config>> + Send + Sync>,
    pub branch: String,
    pub waker: Arc<dyn Fn() + Send + Sync>,
}

impl DatabaseContext {
    pub fn databases(&self) -> Vec<Database> {
        (self.config)()
            .map(|config| pom_db::list_databases(&config, &self.branch))
            .unwrap_or_default()
    }

    /// Runs `work` against the databases on a thread; the answer arrives in the returned `Pending`.
    pub fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connector<'_>) -> Result<T, String> + Send + 'static,
    ) -> Pending<T> {
        let (sender, receiver) = mpsc::channel();
        let (runner, config, branch, waker) = (
            self.runner.clone(),
            self.config.clone(),
            self.branch.clone(),
            self.waker.clone(),
        );
        std::thread::spawn(move || {
            let answer = match config() {
                Some(config) => work(&Connector {
                    runner: &runner,
                    config: &config,
                    branch: &branch,
                }),
                None => Err("pom.yml could not be loaded".into()),
            };
            if sender.send(answer).is_err() {
                eprintln!("database: the view closed before its query finished");
            }
            waker();
        });
        Pending(Some(receiver))
    }
}

/// Background work on its way back.
pub struct Pending<T>(Option<Receiver<Result<T, String>>>);

impl<T> Pending<T> {
    pub fn idle() -> Pending<T> {
        Pending(None)
    }

    pub fn busy(&self) -> bool {
        self.0.is_some()
    }

    /// The answer, once, when it has arrived.
    pub fn poll(&mut self) -> Option<Result<T, String>> {
        let answer = match self.0.as_ref()?.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("the query stopped".into()),
        };
        self.0 = None;
        Some(answer)
    }
}
