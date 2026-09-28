mod console;
mod failure;
mod grid;
mod menu;
mod object_item;
mod panel;
mod render;
mod table_item;
mod tree;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use pom_config::Config;
use pom_db::object_storage::HttpTransport;
use pom_db::{Connector, Database};
use pom_paths::StateDir;
use pom_services::ServiceRunner;

pub use console::{new_console, ConsoleFooter};
pub use grid::{Grid, GridEvent, Region};
pub use object_item::ObjectItem;
pub use panel::DatabasePanel;
pub use pom_db::object_storage::CurlTransport;
pub use table_item::TableItem;

/// What the Database views need: the project's runner (ports, slots), its current config, the workspace's
#[derive(Clone)]
pub struct DatabaseContext {
    pub runner: Arc<ServiceRunner>,
    pub state: StateDir,
    pub config: Arc<dyn Fn() -> Option<Arc<Config>> + Send + Sync>,
    pub branch: String,
    /// The active workspace's folder; a repo's checkout is the folder named for it inside.
    pub workspace_root: PathBuf,
    pub config_path: PathBuf,
    pub waker: Arc<dyn Fn() + Send + Sync>,
    /// How object storage is reached (curl in the app, canned answers in tests).
    pub objects: Arc<dyn HttpTransport>,
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

    /// The repos' names as databases carry them (aliases), in config order.
    pub fn repos(&self) -> Vec<String> {
        (self.config)()
            .map(|config| {
                config
                    .repos
                    .iter()
                    .map(|(key, dir)| {
                        if dir.alias.is_empty() {
                            key.clone()
                        } else {
                            dir.alias.clone()
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Runs `work` here and now, for what needs no network (logins, presigned links).
    pub fn run_now<T>(&self, work: impl FnOnce(&Connector<'_>) -> T) -> Option<T> {
        let config = (self.config)()?;
        Some(work(&Connector {
            runner: &self.runner,
            config: &config,
            branch: &self.branch,
        }))
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
    use std::sync::{Arc, Mutex};

    use pom_db::object_storage::{HttpRequest, HttpResponse, HttpTransport};

    use super::DatabaseContext;

    /// Answers queued bodies in order and keeps the URLs asked for.
    pub(crate) struct FakeTransport {
        answers: Mutex<Vec<String>>,
        asked: Mutex<Vec<String>>,
    }

    impl FakeTransport {
        pub(crate) fn answering(bodies: &[&str]) -> FakeTransport {
            FakeTransport {
                answers: Mutex::new(bodies.iter().map(|body| body.to_string()).collect()),
                asked: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn urls(&self) -> Vec<String> {
            self.asked
                .lock()
                .map(|asked| asked.clone())
                .unwrap_or_default()
        }
    }

    impl HttpTransport for FakeTransport {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
            self.asked
                .lock()
                .map_err(|error| error.to_string())?
                .push(request.url.clone());
            let mut answers = self.answers.lock().map_err(|error| error.to_string())?;
            if answers.is_empty() {
                return Err("no more answers".into());
            }
            Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: answers.remove(0).into_bytes(),
            })
        }
    }

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
        let yaml = "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\n  files:\n    image: minio/minio\n  queue:\n    image: rabbitmq:3\nrepos:\n  api:\n    databases:\n      main: \"api_{{branch.safe}}\"\n    env:\n      REDIS_URL: \"{{shared.redis.url}}\"\n      S3_HOST: \"{{shared.files.host}}\"\n  web:\n    shared_services:\n      - redis\n    commands:\n      migrate: npm run migrate\n    databases:\n      main: \"web_{{branch.safe}}\"\n";
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
                workspace_root: dir.path().join("workspace--feat"),
                config_path: pom,
                waker: Arc::new(|| {}),
                objects: Arc::new(pom_db::object_storage::CurlTransport {
                    program: "/nonexistent".into(),
                }),
            },
            _dir: dir,
        }
    }
}
