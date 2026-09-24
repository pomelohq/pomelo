
mod console;
mod grid;
mod panel;
mod table_item;

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use pom_config::Config;
use pom_db::{Connector, Database};
use pom_paths::StateDir;
use pom_services::ServiceRunner;

pub use console::{new_console, ConsoleFooter};
pub use grid::{Grid, GridEvent, Region};
pub use panel::DatabasePanel;
pub use table_item::TableItem;

/// What the Database views need: the project's runner (ports, slots), its current config, the workspace's
#[derive(Clone)]
pub struct DatabaseContext {
    pub runner: Arc<ServiceRunner>,
    pub state: StateDir,
    pub config: Arc<dyn Fn() -> Option<Arc<Config>> + Send + Sync>,
    pub branch: String,
    pub waker: Arc<dyn Fn() + Send + Sync>,
}

impl DatabaseContext {
    pub fn consoles(&self) -> Vec<pom_db::Console> {
        pom_db::load_consoles(&self.state, self.runner.session())
    }

    pub fn save_consoles(&self, consoles: &[pom_db::Console]) {
        if let Err(error) = pom_db::save_consoles(&self.state, self.runner.session(), consoles) {
            eprintln!("database: save consoles: {error}");
        }
    }

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

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use super::DatabaseContext;

    pub(crate) struct TestContext {
        pub context: DatabaseContext,
        _dir: tempfile::TempDir,
    }

    pub(crate) fn context() -> TestContext {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(error) => panic!("temp dir: {error}"),
        };
        let pom = dir.path().join("pom.yml");
        let yaml = "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\nrepos:\n  api:\n    databases:\n      main: \"api_{{branch.safe}}\"\n";
        if let Err(error) = std::fs::write(&pom, yaml) {
            panic!("write pom.yml: {error}");
        }
        let config = match pom_config::Config::load(&pom) {
            Ok(config) => Arc::new(config),
            Err(error) => panic!("load pom.yml: {error}"),
        };
        let state = pom_paths::StateDir::new(dir.path().join("state"));
        let runner = Arc::new(pom_services::ServiceRunner::new(
            pom_services::RunnerOptions {
                project_root: dir.path().to_path_buf(),
                session: "myproject".into(),
                state: state.clone(),
                holders: pom_ptyhost::SocketDir::new(dir.path().join("s")),
                binary: "/nonexistent".into(),
                docker: "/nonexistent".into(),
            },
        ));
        TestContext {
            context: DatabaseContext {
                runner,
                state,
                config: Arc::new(move || Some(config.clone())),
                branch: "feat".into(),
                waker: Arc::new(|| {}),
            },
            _dir: dir,
        }
    }
}
