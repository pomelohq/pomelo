use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use pom_config::{Config, Dir, Service};
use pom_env::{DevNetwork, DevPorts, ProxySettings};
use pom_layout::WorkspaceState;
use pom_ports::{Lease, PortState};
use pom_proxy::{DevProxy, Machine, ProjectRoute, Serve};

struct FakeMachine {
    leases: Vec<Lease>,
}

impl Machine for FakeMachine {
    fn branches(&self, _project: &ProjectRoute, _config: &Config) -> Vec<String> {
        vec!["main".into(), "PROJ-101-login".into()]
    }
    fn leases(&self, session: &str) -> Vec<Lease> {
        self.leases
            .iter()
            .filter(|lease| lease.session == session)
            .cloned()
            .collect()
    }
    fn live_ports(&self, _holder: &str) -> Vec<u16> {
        Vec::new()
    }
    fn holder_alive(&self, _holder: &str) -> bool {
        false
    }
    fn service_envs(&self, _root: &Path, _branch: &str) -> WorkspaceState {
        WorkspaceState::default()
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("free port")
}

/// Echoes each request head in the body and sets one cookie for the dev domain and one for another.
fn backend(seen: mpsc::Sender<String>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("address").port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => break,
                }
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            // The proxy's reachability probe connects without sending anything.
            if head.is_empty() {
                continue;
            }
            if seen.send(head.clone()).is_err() {
                return;
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nSet-Cookie: sid=1; Path=/; Secure; Domain=.dev.example.net; SameSite=None\r\nSet-Cookie: other=2; Path=/; Secure; Domain=.example.org\r\nConnection: close\r\n\r\n{head}",
                head.len()
            );
            if let Err(error) = stream.write_all(response.as_bytes()) {
                eprintln!("backend: {error}");
            }
        }
    });
    port
}

fn request(port: u16, host: &str, extra: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect proxy");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let raw = format!("GET /login HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n");
    stream.write_all(raw.as_bytes()).expect("send");
    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        eprintln!("read: {error}");
    }
    response
}

fn config() -> Config {
    let mut web = Dir::default();
    web.services.insert("web".into(), Service::default());
    let mut config = Config {
        session: "demo".into(),
        default_branch: "main".into(),
        ..Config::default()
    };
    config.repos.insert("web".into(), web);
    config
}

fn network(trusted: &[&str]) -> DevNetwork {
    let ports = DevPorts {
        webhook: free_port(),
        proxy: free_port(),
    };
    DevNetwork::new(
        ports,
        &ProxySettings {
            domain: "dev.example.net".into(),
            public_scheme: "https".into(),
            public_port: 443,
            trusted_proxies: trusted.iter().map(|range| range.to_string()).collect(),
            ..ProxySettings::default()
        },
    )
}

fn start(network: DevNetwork, backend: u16) -> DevProxy {
    let lease = Lease {
        key: pom_ports::service_key(&pom_env::port_ws_key("PROJ-101-login"), "web~web"),
        port: backend,
        session: "demo".into(),
        state: PortState::Running,
        since_ms: 0,
    };
    let proxy = DevProxy::start_on(
        Box::new(FakeMachine {
            leases: vec![lease],
        }),
        network,
        Serve {
            proxy: true,
            webhook: false,
        },
    )
    .expect("start");
    proxy.set_projects(vec![ProjectRoute {
        root: "/tmp/demo".into(),
        config: Arc::new(RwLock::new(Some(Arc::new(config())))),
    }]);
    proxy
}

fn upstream_header(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

#[test]
fn a_trusted_reverse_proxy_reaches_the_workspace_under_its_domain() {
    let (sender, seen) = mpsc::channel();
    let proxy = start(network(&["127.0.0.1"]), backend(sender));
    let response = request(
        proxy.ports().proxy,
        "web.web.proj-101.dev.example.net",
        "X-Forwarded-Proto: https\r\nX-Forwarded-For: 192.0.2.7\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let head = seen.recv_timeout(Duration::from_secs(5)).expect("upstream");
    assert!(head.starts_with("GET /login HTTP/1.1"), "{head}");
    assert_eq!(
        upstream_header(&head, "x-forwarded-host").as_deref(),
        Some("web.web.proj-101.dev.example.net")
    );
    assert_eq!(
        upstream_header(&head, "x-forwarded-proto").as_deref(),
        Some("https")
    );
    assert_eq!(
        upstream_header(&head, "x-forwarded-for").as_deref(),
        Some("192.0.2.7, 127.0.0.1")
    );
    assert!(
        response.contains(
            "set-cookie: sid=1; Path=/; Secure; Domain=.dev.example.net; SameSite=None\r\n"
        ),
        "{response}"
    );
    assert!(
        response.contains("set-cookie: other=2; Path=/; Secure\r\n"),
        "{response}"
    );
}

#[test]
fn an_untrusted_client_cannot_spoof_forwarded_headers() {
    let (sender, seen) = mpsc::channel();
    let proxy = start(network(&["10.0.0.0/8"]), backend(sender));
    let response = request(
        proxy.ports().proxy,
        "web.web.proj-101.dev.example.net",
        "X-Forwarded-Proto: https\r\nX-Forwarded-Host: evil.example.org\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let head = seen.recv_timeout(Duration::from_secs(5)).expect("upstream");
    assert_eq!(
        upstream_header(&head, "x-forwarded-proto").as_deref(),
        Some("http")
    );
    assert_eq!(
        upstream_header(&head, "x-forwarded-host").as_deref(),
        Some("web.web.proj-101.dev.example.net")
    );
}

#[test]
fn localhost_names_keep_the_local_rewrite_under_a_configured_domain() {
    let (sender, seen) = mpsc::channel();
    let proxy = start(network(&["127.0.0.1"]), backend(sender));
    let host = format!("web.web.proj-101.localhost:{}", proxy.ports().proxy);
    let response = request(proxy.ports().proxy, &host, "");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let head = seen.recv_timeout(Duration::from_secs(5)).expect("upstream");
    assert_eq!(upstream_header(&head, "x-forwarded-host"), Some(host));
    assert!(
        response.contains("set-cookie: sid=1; Path=/; Domain=.dev.example.net; SameSite=None\r\n"),
        "{response}"
    );
}

#[test]
fn the_default_network_is_localhost_only() {
    let (sender, seen) = mpsc::channel();
    let ports = DevPorts {
        webhook: free_port(),
        proxy: free_port(),
    };
    let _proxy = start(DevNetwork::local(ports), backend(sender));
    let response = request(
        ports.proxy,
        "web.web.proj-101.dev.example.net",
        "X-Forwarded-Proto: https\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    let host = format!("web.web.proj-101.localhost:{}", ports.proxy);
    let response = request(ports.proxy, &host, "X-Forwarded-Proto: https\r\n");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let head = seen.recv_timeout(Duration::from_secs(5)).expect("upstream");
    assert_eq!(
        upstream_header(&head, "x-forwarded-proto").as_deref(),
        Some("http")
    );
    assert!(
        response.contains("set-cookie: other=2; Path=/; Domain=.example.org\r\n"),
        "{response}"
    );
}
