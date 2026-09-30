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

const LOG_CAPACITY: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    pub webhook: u16,
    pub proxy: u16,
}

impl Ports {
    /// `POM_WEB_PORT`, else the user's settings, else the defaults (shared with service URL templates).
    pub fn from_env() -> Ports {
        let ports = pom_env::dev_ports();
        Ports {
            webhook: ports.webhook,
            proxy: ports.proxy,
        }
    }

    pub fn from_base(base: u16) -> Ports {
        let ports = pom_env::DevPorts::from_base(base);
        Ports {
            webhook: ports.webhook,
            proxy: ports.proxy,
        }
    }
}

/// Which of the two servers to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Serve {
    pub proxy: bool,
    pub webhook: bool,
}

impl Default for Serve {
    fn default() -> Self {
        Serve {
            proxy: true,
            webhook: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RequestKind {
    #[default]
    Proxy,
    Webhook,
}

/// One workspace a webhook was handed to: `status` when it answered, else why it did not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delivery {
    pub workspace: String,
    pub port: u16,
    pub status: Option<u16>,
    pub error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyLogEntry {
    /// Increases by one per logged request, so a view can keep hold of a row as newer ones arrive.
    pub seq: u64,
    pub kind: RequestKind,
    pub time: String,
    pub method: String,
    pub path: String,
    pub repo: String,
    pub service: String,
    pub profile: String,
    pub target: String,
    pub status: u16,
    pub ms: u64,
    pub deliveries: Vec<Delivery>,
}

#[derive(Default)]
pub struct ProxyLog {
    entries: Mutex<(u64, VecDeque<ProxyLogEntry>)>,
}

impl ProxyLog {
    pub fn add(&self, mut entry: ProxyLogEntry) {
        if let Ok(mut guard) = self.entries.lock() {
            let (next, entries) = &mut *guard;
            *next += 1;
            entry.seq = *next;
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
            .map(|guard| guard.1.iter().rev().take(limit).cloned().collect())
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
    serve: Serve,
    _runtime: tokio::runtime::Runtime,
}

impl DevProxy {
    pub fn start(
        machine: Box<dyn Machine>,
        ports: Ports,
        serve: Serve,
    ) -> std::io::Result<DevProxy> {
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
        for host in ["127.0.0.1", "[::1]"].into_iter().filter(|_| serve.proxy) {
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
        if serve.proxy && !proxy_bound {
            eprintln!(
                "dev proxy: port {} already in use - skipping; service URLs will not route",
                ports.proxy
            );
        }
        let webhook_bound = match serve
            .webhook
            .then(|| bind(&format!("127.0.0.1:{}", ports.webhook)))
        {
            None => false,
            Some(Ok(listener)) => {
                runtime.spawn(server::accept(listener, shared, server::Kind::Webhook));
                true
            }
            Some(Err(error)) => {
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
            serve,
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

    pub fn serve(&self) -> Serve {
        self.serve
    }

    pub fn proxy_running(&self) -> bool {
        self.proxy_bound
    }

    pub fn webhook_running(&self) -> bool {
        self.webhook_bound
    }
}

/// Something accepts connections on the local port (the app's proxy, or `pom proxy` in a terminal).
pub fn listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

fn bind(address: &str) -> std::io::Result<tokio::net::TcpListener> {
    let listener = std::net::TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}
