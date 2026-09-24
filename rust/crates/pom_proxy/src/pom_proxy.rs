mod machine;
mod routing;
mod server;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub use machine::{listening_port_in_tree, SystemMachine};
pub use routing::{
    branch_labels, host_labels, pick_port, resolve_service_key, rewrite_external_cookie,
    rewrite_local_cookie, ConfigSource, Decision, Logged, Machine, ProjectRoute, Route, Router,
};

const DEFAULT_WEB_PORT: u16 = 8765;
const LOG_CAPACITY: usize = 300;

/// The dev proxy listens two above the base port and the webhook relay one above; `POM_WEB_PORT` moves the
/// base so a dev build can run beside an installed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    pub webhook: u16,
    pub proxy: u16,
}

impl Ports {
    pub fn from_env() -> Ports {
        let base = std::env::var("POM_WEB_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(DEFAULT_WEB_PORT);
        Ports::from_base(base)
    }

    pub fn from_base(base: u16) -> Ports {
        Ports {
            webhook: base.saturating_add(1),
            proxy: base.saturating_add(2),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyLogEntry {
    pub time: String,
    pub method: String,
    pub path: String,
    pub repo: String,
    pub service: String,
    pub profile: String,
    pub target: String,
    pub status: u16,
    pub ms: u64,
}

#[derive(Default)]
pub struct ProxyLog {
    entries: Mutex<VecDeque<ProxyLogEntry>>,
}

impl ProxyLog {
    pub fn add(&self, entry: ProxyLogEntry) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.push_back(entry);
            while entries.len() > LOG_CAPACITY {
                entries.pop_front();
            }
        }
    }

    /// Newest first.
    pub fn snapshot(&self, limit: usize) -> Vec<ProxyLogEntry> {
        self.entries
            .lock()
            .map(|entries| entries.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }
}

fn clock() -> String {
    let now: libc::time_t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as libc::time_t);
    // SAFETY: localtime_r only writes the tm we own; a zeroed tm is a valid initial value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { libc::localtime_r(&now, &mut tm) };
    if converted.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// The running dev proxy and webhook relay. A port someone else already holds (another instance) is skipped
/// rather than fought over.
pub struct DevProxy {
    router: Arc<Router>,
    log: Arc<ProxyLog>,
    ports: Ports,
    proxy_bound: bool,
    webhook_bound: bool,
    _runtime: tokio::runtime::Runtime,
}

impl DevProxy {
    pub fn start(machine: Box<dyn Machine>, ports: Ports) -> std::io::Result<DevProxy> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("pom-proxy")
            .enable_all()
            .build()?;
        let router = Arc::new(Router::new(machine));
        let log = Arc::new(ProxyLog::default());
        let shared = Arc::new(server::Shared::new(router.clone(), log.clone()));
        let _entered = runtime.enter();
        let mut proxy_bound = false;
        for host in ["127.0.0.1", "[::1]"] {
            match bind(&format!("{host}:{}", ports.proxy)) {
                Ok(listener) => {
                    proxy_bound = true;
                    runtime.spawn(server::accept(
                        listener,
                        shared.clone(),
                        server::Kind::Proxy,
                    ));
                }
                Err(error) => eprintln!("dev proxy: {host}:{} unavailable: {error}", ports.proxy),
            }
        }
        if !proxy_bound {
            eprintln!(
                "dev proxy: port {} already in use - skipping; service URLs will not route",
                ports.proxy
            );
        }
        let webhook_bound = match bind(&format!("127.0.0.1:{}", ports.webhook)) {
            Ok(listener) => {
                runtime.spawn(server::accept(listener, shared, server::Kind::Webhook));
                true
            }
            Err(error) => {
                eprintln!(
                    "webhook relay: 127.0.0.1:{} unavailable: {error}",
                    ports.webhook
                );
                false
            }
        };
        drop(_entered);
        Ok(DevProxy {
            router,
            log,
            ports,
            proxy_bound,
            webhook_bound,
            _runtime: runtime,
        })
    }

    pub fn set_projects(&self, projects: Vec<ProjectRoute>) {
        self.router.set_projects(projects);
    }

    pub fn log(&self, limit: usize) -> Vec<ProxyLogEntry> {
        self.log.snapshot(limit)
    }

    pub fn ports(&self) -> Ports {
        self.ports
    }

    pub fn proxy_running(&self) -> bool {
        self.proxy_bound
    }

    pub fn webhook_running(&self) -> bool {
        self.webhook_bound
    }
}

fn bind(address: &str) -> std::io::Result<tokio::net::TcpListener> {
    let listener = std::net::TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}
