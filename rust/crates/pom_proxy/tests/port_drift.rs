//! Every way a service URL ends in "backend not reachable" while the service itself is fine: the proxy must
//! reach the port the service really listens on, whatever the lease files say.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use pom_config::{Config, Dir, Service};
use pom_layout::WorkspaceState;
use pom_ports::{Lease, PortState};
use pom_proxy::{DevProxy, Machine, Ports, ProjectRoute, Serve};

const HOST: &str = "server.api.feat-login.localhost";
const HOLDER: &str = "svc-demo-feat-login-acme-api-server";

#[derive(Default)]
struct World {
    leases: Vec<Lease>,
    listening: Option<u16>,
    holder_alive: bool,
}

#[derive(Clone, Default)]
struct FakeMachine(Arc<Mutex<World>>);

impl FakeMachine {
    fn set(&self, change: impl FnOnce(&mut World)) {
        if let Ok(mut world) = self.0.lock() {
            change(&mut world);
        }
    }
}

impl Machine for FakeMachine {
    fn branches(&self, _project: &ProjectRoute, _config: &Config) -> Vec<String> {
        vec!["main".into(), "feat-login".into()]
    }
    fn leases(&self, session: &str) -> Vec<Lease> {
        self.0
            .lock()
            .map(|world| {
                world
                    .leases
                    .iter()
                    .filter(|lease| lease.session == session)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
    fn live_ports(&self, holder: &str) -> Vec<u16> {
        self.0
            .lock()
            .ok()
            .filter(|world| holder == HOLDER && world.holder_alive)
            .and_then(|world| world.listening)
            .into_iter()
            .collect()
    }
    fn holder_alive(&self, holder: &str) -> bool {
        holder == HOLDER && self.0.lock().is_ok_and(|world| world.holder_alive)
    }
    fn service_envs(&self, _root: &Path, _branch: &str) -> WorkspaceState {
        WorkspaceState::default()
    }
}

fn config() -> Config {
    let mut api = Dir {
        alias: "api".into(),
        ..Dir::default()
    };
    api.services.insert("server".into(), Service::default());
    let mut config = Config {
        session: "demo".into(),
        default_branch: "main".into(),
        ..Config::default()
    };
    config.repos.insert("acme-api".into(), api);
    config
}

fn lease(port: u16, state: PortState) -> Lease {
    Lease {
        key: pom_ports::service_key(&pom_env::port_ws_key("feat-login"), "api~server"),
        port,
        session: "demo".into(),
        state,
        since_ms: 0,
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("free port")
}

/// A port nothing listens on, below `ceiling`, so it sorts first among duplicate leases.
fn dead_port_below(ceiling: u16) -> u16 {
    (20000..ceiling)
        .find(|port| {
            TcpListener::bind(("127.0.0.1", *port)).is_ok()
                && TcpListener::bind(("::1", *port)).is_ok()
        })
        .expect("a dead port")
}

/// Answers every request with `200 ok`.
fn backend(address: &str) -> u16 {
    let address = address
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
        .expect("address");
    let listener = TcpListener::bind(address).expect("bind backend");
    let port = listener.local_addr().expect("local address").port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                if stream.read(&mut buffer).is_ok() {
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                }
            });
        }
    });
    port
}

fn start(machine: FakeMachine) -> (DevProxy, u16) {
    let ports = Ports {
        webhook: free_port(),
        proxy: free_port(),
    };
    let proxy = DevProxy::start(Box::new(machine), ports, Serve::default()).expect("start");
    proxy.set_projects(vec![ProjectRoute {
        root: "/tmp/demo".into(),
        config: Arc::new(RwLock::new(Some(Arc::new(config())))),
    }]);
    (proxy, ports.proxy)
}

fn get(proxy_port: u16, host: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", proxy_port)).expect("connect proxy");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .expect("send");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

fn reaches(scenario: &str, world: World) {
    let (_proxy, proxy_port) = start(FakeMachine(Arc::new(Mutex::new(world))));
    let response = get(proxy_port, HOST, "/");
    assert!(
        response.starts_with("HTTP/1.1 200") && response.ends_with("ok"),
        "{scenario}: {response}"
    );
}

#[test]
fn duplicate_leases_reach_the_one_the_service_runs_on() {
    let service = backend("127.0.0.1:0");
    let stray = dead_port_below(service);
    reaches(
        "an agent left a second, lower lease for the service",
        World {
            leases: vec![
                lease(stray, PortState::Assigned),
                lease(service, PortState::Running),
            ],
            listening: Some(service),
            holder_alive: true,
        },
    );
}

#[test]
fn a_service_that_ignores_its_port_is_still_reached() {
    let service = backend("127.0.0.1:0");
    reaches(
        "the dev server picked its own port instead of $PORT",
        World {
            leases: vec![lease(dead_port_below(65000), PortState::Running)],
            listening: Some(service),
            holder_alive: true,
        },
    );
}

#[test]
fn a_relocated_workspace_still_reaches_the_service_running_on_its_old_port() {
    let service = backend("127.0.0.1:0");
    reaches(
        "resolve_port_conflict leased a new port but the service was not restarted",
        World {
            leases: vec![lease(dead_port_below(65000), PortState::Assigned)],
            listening: Some(service),
            holder_alive: true,
        },
    );
}

#[test]
fn a_service_whose_lease_was_reaped_is_reached_by_its_live_port() {
    let service = backend("127.0.0.1:0");
    reaches(
        "the reaper dropped the lease during a long rebuild",
        World {
            leases: Vec::new(),
            listening: Some(service),
            holder_alive: true,
        },
    );
}

#[test]
fn an_ipv6_only_service_is_reached() {
    let service = backend("[::1]:0");
    reaches(
        "the dev server listens on ::1 only",
        World {
            leases: vec![lease(service, PortState::Running)],
            listening: Some(service),
            holder_alive: true,
        },
    );
}

#[test]
fn a_service_that_comes_back_is_reached_on_the_next_request() {
    let machine = FakeMachine::default();
    let stale = dead_port_below(65000);
    machine.set(|world| world.leases = vec![lease(stale, PortState::Running)]);
    let (_proxy, proxy_port) = start(machine.clone());
    let response = get(proxy_port, HOST, "/");
    assert!(response.starts_with("HTTP/1.1 50"), "{response}");
    let service = backend("127.0.0.1:0");
    machine.set(|world| {
        world.leases = vec![lease(service, PortState::Running)];
        world.listening = Some(service);
        world.holder_alive = true;
    });
    let response = get(proxy_port, HOST, "/");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "a failed request must not pin the dead port in the cache: {response}"
    );
}

#[test]
fn a_stopped_service_says_it_is_not_running() {
    let machine = FakeMachine::default();
    machine.set(|world| world.leases = vec![lease(dead_port_below(65000), PortState::Assigned)]);
    let (_proxy, proxy_port) = start(machine);
    let response = get(proxy_port, HOST, "/");
    assert!(
        !response.contains("backend not reachable"),
        "the cause is known, so say it: {response}"
    );
    assert!(response.contains("not running"), "{response}");
}

#[test]
fn host_routed_requests_are_logged() {
    let service = backend("127.0.0.1:0");
    let machine = FakeMachine::default();
    machine.set(|world| {
        world.leases = vec![lease(service, PortState::Running)];
        world.listening = Some(service);
        world.holder_alive = true;
    });
    let (proxy, proxy_port) = start(machine);
    let response = get(proxy_port, HOST, "/login");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let log = proxy.log(10);
    assert_eq!(
        log.len(),
        1,
        "a request to a service URL shows in Dev Requests"
    );
    assert_eq!(log[0].target, format!("127.0.0.1:{service}"));
}
