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
    lease_scans: usize,
    process_scans: usize,
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
            .map(|mut world| {
                world.lease_scans += 1;
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
        if let Ok(mut world) = self.0.lock() {
            world.process_scans += 1;
        }
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

/// Reads a request head; closing a socket with unread bytes would reset it instead of ending it.
fn read_head(stream: &mut TcpStream) -> bool {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return false,
        }
    }
    true
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
                if read_head(&mut stream) {
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
    send(proxy_port, host, path, "")
}

fn send(proxy_port: u16, host: &str, path: &str, headers: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", proxy_port)).expect("connect proxy");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{headers}Connection: close\r\n\r\n")
                .as_bytes(),
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
            ..World::default()
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
            ..World::default()
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
            ..World::default()
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
            ..World::default()
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
            ..World::default()
        },
    );
}

/// Serves one request, then stops listening: a dev server that restarts on another port. A connection
/// that sends nothing (the proxy checking the port) does not count.
fn one_shot_backend() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("local address").port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            if read_head(&mut stream) {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
                return;
            }
        }
    });
    port
}

#[test]
fn a_service_that_comes_back_is_reached_without_waiting_out_the_cache() {
    let machine = FakeMachine::default();
    machine.set(|world| world.leases = vec![lease(dead_port_below(65000), PortState::Running)]);
    let (_proxy, proxy_port) = start(machine.clone());
    let response = get(proxy_port, HOST, "/");
    assert!(response.starts_with("HTTP/1.1 50"), "{response}");

    let first = one_shot_backend();
    machine.set(|world| {
        world.leases = vec![lease(first, PortState::Running)];
        world.listening = Some(first);
        world.holder_alive = true;
    });
    std::thread::sleep(Duration::from_millis(600));
    let response = get(proxy_port, HOST, "/");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "a miss is only remembered briefly: {response}"
    );

    let second = backend("127.0.0.1:0");
    machine.set(|world| {
        world.leases = vec![lease(second, PortState::Running)];
        world.listening = Some(second);
    });
    let response = get(proxy_port, HOST, "/");
    assert!(response.starts_with("HTTP/1.1 502"), "{response}");
    let response = get(proxy_port, HOST, "/");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "an address that refused is forgotten at once: {response}"
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
fn host_routed_page_loads_and_failures_are_logged_but_not_every_module() {
    let service = backend("127.0.0.1:0");
    let machine = FakeMachine::default();
    machine.set(|world| {
        world.leases = vec![lease(service, PortState::Running)];
        world.listening = Some(service);
        world.holder_alive = true;
    });
    let (proxy, proxy_port) = start(machine.clone());
    let page = send(proxy_port, HOST, "/login", "Sec-Fetch-Mode: navigate\r\n");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    for module in 0..20 {
        get(proxy_port, HOST, &format!("/src/module{module}.tsx"));
    }
    machine.set(|world| {
        world.leases.clear();
        world.listening = None;
        world.holder_alive = false;
    });
    let missing = get(proxy_port, "server.api.main.localhost", "/src/main.tsx");
    assert!(missing.starts_with("HTTP/1.1 502"), "{missing}");
    let paths: Vec<String> = proxy.log(50).into_iter().map(|entry| entry.path).collect();
    assert_eq!(
        paths,
        ["/src/main.tsx", "/login"],
        "the page load and the failure only"
    );
    assert_eq!(proxy.log(50)[1].target, format!("127.0.0.1:{service}"));
}

/// How a browser loads a client-rendered app: six keep-alive connections to the host, fetching modules
/// back to back. Returns how many came back 200.
fn browser_burst(proxy_port: u16, per_connection: usize) -> usize {
    let handles: Vec<_> = (0..6)
        .map(|connection| {
            std::thread::spawn(move || {
                let Ok(mut stream) = TcpStream::connect(("127.0.0.1", proxy_port)) else {
                    return 0;
                };
                if stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .is_err()
                {
                    return 0;
                }
                let mut loaded = 0;
                for module in 0..per_connection {
                    let request = format!(
                        "GET /src/c{connection}m{module}.tsx HTTP/1.1\r\nHost: {HOST}\r\n\r\n"
                    );
                    if stream.write_all(request.as_bytes()).is_err() {
                        break;
                    }
                    match read_response(&mut stream) {
                        Some(200) => loaded += 1,
                        Some(_) => {}
                        None => break,
                    }
                }
                loaded
            })
        })
        .collect();
    handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .sum()
}

/// The status of one response, after reading its body so the connection can carry the next request.
fn read_response(stream: &mut TcpStream) -> Option<u16> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return None,
        }
    }
    let head = String::from_utf8_lossy(&head);
    let status = head.split(' ').nth(1)?.parse().ok()?;
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    Some(status)
}

/// A client-rendered app fetches hundreds of modules per page load.
#[test]
fn a_burst_of_module_requests_looks_the_service_up_once() {
    let service = backend("127.0.0.1:0");
    let machine = FakeMachine::default();
    machine.set(|world| {
        world.leases = vec![lease(dead_port_below(65000), PortState::Running)];
        world.listening = Some(service);
        world.holder_alive = true;
    });
    let (_proxy, proxy_port) = start(machine.clone());
    let burst = |per_connection: usize| browser_burst(proxy_port, per_connection);
    assert_eq!(burst(50), 300, "every module loads");
    let (leases, processes) = machine
        .0
        .lock()
        .map(|world| (world.lease_scans, world.process_scans))
        .unwrap_or_default();
    assert!(leases <= 2, "lease files read {leases} times for one burst");
    assert!(
        processes <= 2,
        "processes scanned {processes} times for one burst"
    );

    machine.set(|world| {
        world.listening = None;
        world.lease_scans = 0;
        world.process_scans = 0;
    });
    std::thread::sleep(Duration::from_millis(3100));
    assert_eq!(burst(1), 0, "the service is rebuilding");
    let processes = machine.0.lock().map_or(0, |world| world.process_scans);
    assert!(
        processes <= 3,
        "a building service is looked up {processes} times for one burst"
    );
}
