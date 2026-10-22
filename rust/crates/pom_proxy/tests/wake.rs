//! A request for a sleeping workspace gets the waiting page and wakes it, once for a burst of requests.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use pom_config::{Config, Dir, Service};
use pom_layout::{RunState, WorkspaceState};
use pom_ports::{Lease, PortState};
use pom_proxy::{DevProxy, Machine, Ports, ProjectRoute, Serve};

struct SleepyMachine {
    leases: Vec<Lease>,
    wakes: Arc<Mutex<Vec<String>>>,
}

impl Machine for SleepyMachine {
    fn branches(&self, _project: &ProjectRoute, _config: &Config) -> Vec<String> {
        vec!["main".into(), "feat-login".into()]
    }
    fn leases(&self, _session: &str) -> Vec<Lease> {
        self.leases.clone()
    }
    fn live_ports(&self, _holder: &str) -> Vec<u16> {
        Vec::new()
    }
    fn holder_alive(&self, _holder: &str) -> bool {
        false
    }
    fn service_envs(&self, _root: &Path, branch: &str) -> WorkspaceState {
        WorkspaceState {
            state: if branch == "feat-login" {
                RunState::Sleeping
            } else {
                RunState::Running
            },
            ..WorkspaceState::default()
        }
    }
    fn wake(&self, project_root: &Path, branch: &str) {
        self.wakes
            .lock()
            .expect("wakes")
            .push(format!("{}:{branch}", project_root.display()));
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("free port")
}

fn request(port: u16, host: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect proxy");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let raw =
        format!("GET / HTTP/1.1\r\nHost: {host}\r\nAccept: text/html\r\nConnection: close\r\n\r\n");
    stream.write_all(raw.as_bytes()).expect("send");
    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        panic!("read: {error}");
    }
    response
}

fn lease(branch: &str, port: u16) -> Lease {
    Lease {
        key: pom_ports::service_key(&pom_env::port_ws_key(branch), "api~server"),
        port,
        session: "demo".into(),
        state: PortState::Running,
        since_ms: 0,
    }
}

#[test]
fn a_sleeping_workspace_shows_the_waiting_page_and_wakes_once() {
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
    let wakes = Arc::new(Mutex::new(Vec::new()));
    let ports = Ports {
        webhook: free_port(),
        proxy: free_port(),
    };
    // Leased ports nothing listens on: the services are stopped, as in a workspace that sleeps.
    let proxy = DevProxy::start(
        Box::new(SleepyMachine {
            leases: vec![lease("feat-login", free_port()), lease("main", free_port())],
            wakes: wakes.clone(),
        }),
        ports,
        Serve::default(),
    )
    .expect("start");
    proxy.set_projects(vec![ProjectRoute {
        root: "/tmp/demo".into(),
        config: Arc::new(RwLock::new(Some(Arc::new(config)))),
    }]);

    for _ in 0..3 {
        let response = request(ports.proxy, "server.api.feat-login.localhost");
        assert!(response.starts_with("HTTP/1.1 503"), "{response}");
        assert!(response.contains("Waking feat-login"), "{response}");
        assert!(response.contains("location.reload()"), "{response}");
    }
    assert_eq!(*wakes.lock().expect("wakes"), ["/tmp/demo:feat-login"]);

    let response = request(ports.proxy, "server.api.main.localhost");
    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(response.contains("is not running"), "{response}");
    assert_eq!(
        wakes.lock().expect("wakes").len(),
        1,
        "a running workspace is not woken"
    );
}
