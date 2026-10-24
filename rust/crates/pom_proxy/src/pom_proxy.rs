mod access;
mod activity;
mod error_page;
mod faults;
mod machine;
mod routing;
mod server;

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pom_config::Config;

pub use access::{
    new_proxy_token, proxy_token, refusal, remove_proxy_token, ProxyToken, TokenSource,
    MIN_TOKEN_LENGTH, TOKEN_COOKIE, TOKEN_HEADER, TOKEN_PARAM,
};
pub use activity::{read_activity, ActivityEntry, ActivityFile, ACTIVITY_SCHEMA};
pub use faults::{
    add_fault, clear_faults, faults_path, load_faults, now_ms as faults_now, reload_running_faults,
    remove_fault, Fault, FaultRule, LoadedFaults, ReloadReply,
};
pub use machine::{listening_ports_in_tree, SystemMachine, Waker};
pub use routing::{
    branch_labels, host_labels, host_name, reachable, resolve_service_key, rewrite_external_cookie,
    rewrite_local_cookie, rewrite_public_cookie, ConfigSource, Decision, Logged, Machine, Problem,
    ProblemKind, ProjectRoute, RemoteGateway, Route, Router,
};

const LOG_CAPACITY: usize = 300;
/// Stands in for a credential header's value in the request log.
pub const REDACTED: &str = "[redacted]";

/// Headers that carry credentials: their values never reach the request log.
pub fn sensitive_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    ) || [
        "token", "secret", "password", "api-key", "apikey", "session",
    ]
    .iter()
    .any(|part| name.contains(part))
}

pub const BODY_CAP: usize = 256 * 1024;
const BODY_BUDGET: usize = 64 << 20;

/// A delivery GitHub posted to the relay's `/_pom/github`, as it arrived.
#[derive(Clone, Debug, Default)]
pub struct GithubDelivery {
    /// `X-GitHub-Event`: `push`, `ping`, ...
    pub event: String,
    /// `X-GitHub-Delivery`.
    pub id: String,
    /// `X-Hub-Signature-256`: `sha256=<hex>`.
    pub signature: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// What the relay answers GitHub, and how the delivery shows in the request log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GithubReply {
    pub status: u16,
    /// JSON.
    pub body: String,
    /// `owner/name` of the repository, when the payload was trusted.
    pub repo: String,
    pub summary: String,
}

/// Verifies and acts on a GitHub delivery, given every project the proxy routes (root, config).
pub type GithubHook =
    Arc<dyn Fn(&GithubDelivery, &[(PathBuf, Arc<Config>)]) -> GithubReply + Send + Sync>;

/// The relay path GitHub posts to; repo aliases may not start with `_`, so no repo route shadows it.
pub const GITHUB_PATH: &str = "/_pom/github";

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

#[derive(Default)]
pub struct Initial {
    pub projects: Vec<ProjectRoute>,
    pub github: Option<GithubHook>,
    /// What requests from outside this machine must present; without one a non-loopback bind or a public domain
    /// is refused (see `refusal`).
    pub token: Option<ProxyToken>,
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
    pub ms: u64,
}

/// A body as it passed through: its first `BODY_CAP` bytes, how long it was, and whether it ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Payload {
    pub bytes: Vec<u8>,
    pub total: u64,
    pub complete: bool,
    /// Dropped to stay under the log's memory budget.
    pub evicted: bool,
}

impl Payload {
    pub fn truncated(&self) -> bool {
        self.total > self.bytes.len() as u64
    }
}

/// A body being captured while it streams.
pub type Capture = Arc<Mutex<Payload>>;

pub fn capture_of(bytes: &[u8]) -> Capture {
    let capture = Capture::default();
    record(&capture, bytes);
    finish(&capture);
    capture
}

pub(crate) fn record(capture: &Capture, bytes: &[u8]) {
    if let Ok(mut payload) = capture.lock() {
        payload.total += bytes.len() as u64;
        let room = BODY_CAP.saturating_sub(payload.bytes.len());
        if room > 0 && !payload.evicted {
            payload
                .bytes
                .extend_from_slice(&bytes[..bytes.len().min(room)]);
        }
    }
}

pub(crate) fn finish(capture: &Capture) {
    if let Ok(mut payload) = capture.lock() {
        payload.complete = true;
    }
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
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    /// Body lengths so far (a streamed response keeps growing after the entry is logged).
    pub request_bytes: u64,
    pub response_bytes: u64,
    /// The fault rule that changed this request, if one did.
    pub fault: String,
}

struct Stored {
    entry: ProxyLogEntry,
    request: Capture,
    response: Capture,
}

#[derive(Default)]
pub struct ProxyLog {
    entries: Mutex<(u64, VecDeque<Stored>)>,
}

impl ProxyLog {
    pub fn add(&self, mut entry: ProxyLogEntry, request: Capture, response: Capture) {
        if let Ok(mut guard) = self.entries.lock() {
            let (next, entries) = &mut *guard;
            *next += 1;
            entry.seq = *next;
            entries.push_back(Stored {
                entry,
                request,
                response,
            });
            while entries.len() > LOG_CAPACITY {
                entries.pop_front();
            }
            evict_over_budget(entries, BODY_BUDGET);
        }
    }

    /// Newest first, without bodies (see `payloads`).
    pub fn snapshot(&self, limit: usize) -> Vec<ProxyLogEntry> {
        let size = |capture: &Capture| capture.lock().map_or(0, |payload| payload.total);
        self.entries
            .lock()
            .map(|guard| {
                guard
                    .1
                    .iter()
                    .rev()
                    .take(limit)
                    .map(|stored| ProxyLogEntry {
                        request_bytes: size(&stored.request),
                        response_bytes: size(&stored.response),
                        ..stored.entry.clone()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The request and response bodies of entry `seq`, while it is still in the log.
    pub fn payloads(&self, seq: u64) -> Option<(Payload, Payload)> {
        let guard = self.entries.lock().ok()?;
        let stored = guard.1.iter().find(|stored| stored.entry.seq == seq)?;
        let copy = |capture: &Capture| {
            capture
                .lock()
                .map(|payload| payload.clone())
                .unwrap_or_default()
        };
        Some((copy(&stored.request), copy(&stored.response)))
    }
}

/// Oldest bodies go first once the log holds more than `BODY_BUDGET` bytes of them.
fn evict_over_budget(entries: &mut VecDeque<Stored>, budget: usize) {
    let held = |capture: &Capture| capture.lock().map_or(0, |payload| payload.bytes.len());
    let mut total: usize = entries
        .iter()
        .map(|stored| held(&stored.request) + held(&stored.response))
        .sum();
    for stored in entries.iter() {
        if total <= budget {
            break;
        }
        for capture in [&stored.request, &stored.response] {
            if let Ok(mut payload) = capture.lock() {
                total -= payload.bytes.len();
                payload.bytes = Vec::new();
                payload.evicted = true;
            }
        }
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
    shared: Arc<server::Shared>,
    ports: Ports,
    network: pom_env::DevNetwork,
    proxy_bound: bool,
    webhook_bound: bool,
    serve: Serve,
    _runtime: tokio::runtime::Runtime,
}

impl DevProxy {
    /// On this machine only, under `localhost`.
    pub fn start(
        machine: Box<dyn Machine>,
        ports: Ports,
        serve: Serve,
    ) -> std::io::Result<DevProxy> {
        let network = pom_env::DevNetwork::local(pom_env::DevPorts {
            webhook: ports.webhook,
            proxy: ports.proxy,
        });
        DevProxy::start_on(machine, network, serve)
    }

    /// Listens where `network` says and routes its domain beside `localhost`.
    pub fn start_on(
        machine: Box<dyn Machine>,
        network: pom_env::DevNetwork,
        serve: Serve,
    ) -> std::io::Result<DevProxy> {
        DevProxy::start_with(machine, network, serve, Initial::default())
    }

    pub fn start_with(
        machine: Box<dyn Machine>,
        network: pom_env::DevNetwork,
        serve: Serve,
        initial: Initial,
    ) -> std::io::Result<DevProxy> {
        let ports = Ports {
            webhook: network.ports.webhook,
            proxy: network.ports.proxy,
        };
        if let Some(reason) = refusal(&network, initial.token.as_ref()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                reason,
            ));
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("pom-proxy")
            .enable_all()
            .build()?;
        let initial_token_missing = initial.token.is_none();
        let router = Arc::new(Router::new(machine).with_domain(&network.domain));
        let log = Arc::new(ProxyLog::default());
        let shared = Arc::new(server::Shared::new(
            router.clone(),
            log.clone(),
            network.clone(),
            initial.token.map(|token| token.value),
        ));
        router.set_projects(initial.projects);
        if let Some(hook) = initial.github {
            shared.set_github(hook);
        }
        let _entered = runtime.enter();
        let mut proxy_bound = false;
        let proxy_addresses = listen_addresses(network.bind, ports.proxy, true);
        for address in proxy_addresses.into_iter().filter(|_| serve.proxy) {
            match bind(address) {
                Ok(listener) => {
                    proxy_bound = true;
                    runtime.spawn(server::accept(
                        listener,
                        shared.clone(),
                        server::Kind::Proxy,
                    ));
                }
                Err(error) => eprintln!("dev proxy: {address} unavailable: {error}"),
            }
        }
        if serve.proxy && !proxy_bound {
            eprintln!(
                "dev proxy: port {} already in use - skipping; service URLs will not route",
                ports.proxy
            );
        }
        let mut webhook_bound = false;
        let webhook_addresses = listen_addresses(network.bind, ports.webhook, false);
        for address in webhook_addresses.into_iter().filter(|_| serve.webhook) {
            match bind(address) {
                Ok(listener) => {
                    webhook_bound = true;
                    runtime.spawn(server::accept(
                        listener,
                        shared.clone(),
                        server::Kind::Webhook,
                    ));
                }
                Err(error) => eprintln!("webhook relay: {address} unavailable: {error}"),
            }
        }
        if network.reach().is_some()
            && network.allow_unauthenticated
            && initial_token_missing
            && (proxy_bound || webhook_bound)
        {
            eprintln!("dev proxy: {}", exposure_warning(&network));
        }
        drop(_entered);
        Ok(DevProxy {
            router,
            log,
            shared,
            ports,
            network,
            proxy_bound,
            webhook_bound,
            serve,
            _runtime: runtime,
        })
    }

    pub fn set_projects(&self, projects: Vec<ProjectRoute>) {
        self.router.set_projects(projects);
    }

    /// The remote projects open in the app, whose workspaces go to their host's dev proxy.
    pub fn set_remote_gateways(&self, gateways: Vec<RemoteGateway>) {
        self.router.set_remote_gateways(gateways);
    }

    /// Who handles `/_pom/github` on the relay; without one it answers 404.
    pub fn set_github_hook(&self, hook: GithubHook) {
        self.shared.set_github(hook);
    }

    pub fn log(&self, limit: usize) -> Vec<ProxyLogEntry> {
        self.log.snapshot(limit)
    }

    pub fn payloads(&self, seq: u64) -> Option<(Payload, Payload)> {
        self.log.payloads(seq)
    }

    pub fn ports(&self) -> Ports {
        self.ports
    }

    pub fn network(&self) -> &pom_env::DevNetwork {
        &self.network
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

/// Someone other than this machine's user can reach the proxy with no token: say so.
pub fn exposure_warning(network: &pom_env::DevNetwork) -> String {
    let trust = if network.trusted_proxies.is_empty() {
        " and no proxy.trusted_proxies, so forwarded headers are ignored"
    } else {
        ""
    };
    let reach = if network.exposed() {
        format!(
            "listening on {}, reachable from other machines",
            network.bind
        )
    } else {
        format!(
            "serving {}, reachable through a reverse proxy",
            network.domain
        )
    };
    format!(
        "{reach}{trust}. proxy.allow_unauthenticated is on, so no token is asked for: keep it on a private network (Tailscale, a LAN) or behind an authenticating reverse proxy, or run `pom proxy token new`"
    )
}

/// Loopback stays reachable (the CLI and services on this machine use it) beside a configured address; an
/// unspecified address already covers loopback, and a second bind on the same port would fail.
fn listen_addresses(bind: IpAddr, port: u16, both_loopbacks: bool) -> Vec<SocketAddr> {
    let mut addresses = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)];
    if both_loopbacks {
        addresses.push(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port));
    }
    if bind.is_unspecified() {
        return vec![SocketAddr::new(bind, port)];
    }
    if !bind.is_loopback() {
        addresses.push(SocketAddr::new(bind, port));
    }
    addresses
}

fn bind(address: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    let listener = std::net::TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_headers_are_sensitive_and_signatures_are_not() {
        for name in [
            "Authorization",
            "Cookie",
            "set-cookie",
            "x-api-key",
            "X-Auth-Token",
        ] {
            assert!(sensitive_header(name), "{name}");
        }
        for name in ["stripe-signature", "content-type", "x-hub-signature-256"] {
            assert!(!sensitive_header(name), "{name}");
        }
    }

    #[test]
    fn loopback_binds_as_before_and_other_addresses_add_to_it() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let text = |addresses: Vec<SocketAddr>| -> Vec<String> {
            addresses.iter().map(SocketAddr::to_string).collect()
        };
        assert_eq!(
            text(listen_addresses(loopback, 8767, true)),
            ["127.0.0.1:8767", "[::1]:8767"]
        );
        assert_eq!(
            text(listen_addresses(loopback, 8766, false)),
            ["127.0.0.1:8766"]
        );
        assert_eq!(
            text(listen_addresses("100.64.0.10".parse().unwrap(), 8767, true)),
            ["127.0.0.1:8767", "[::1]:8767", "100.64.0.10:8767"]
        );
        assert_eq!(
            text(listen_addresses("0.0.0.0".parse().unwrap(), 8766, false)),
            ["0.0.0.0:8766"]
        );
    }

    #[test]
    fn bodies_are_capped_and_the_oldest_are_dropped_over_budget() {
        let big = vec![b'x'; BODY_CAP + 10];
        let capture = capture_of(&big);
        let payload = capture.lock().unwrap().clone();
        assert_eq!(payload.bytes.len(), BODY_CAP);
        assert_eq!(payload.total, (BODY_CAP + 10) as u64);
        assert!(payload.truncated() && payload.complete);

        let mut entries: VecDeque<Stored> = (0..3)
            .map(|seq| Stored {
                entry: ProxyLogEntry {
                    seq,
                    ..ProxyLogEntry::default()
                },
                request: capture_of(&[b'r'; 100]),
                response: capture_of(&[b's'; 100]),
            })
            .collect();
        evict_over_budget(&mut entries, 400);
        let evicted: Vec<bool> = entries
            .iter()
            .map(|stored| stored.request.lock().unwrap().evicted)
            .collect();
        assert_eq!(evicted, [true, false, false]);
    }
}
