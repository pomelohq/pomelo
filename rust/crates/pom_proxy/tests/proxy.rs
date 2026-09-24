use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use pom_config::{Config, Dir, Service, SharedServiceDef};
use pom_layout::WorkspaceState;
use pom_ports::{Lease, PortState};
use pom_proxy::{DevProxy, Machine, Ports, ProjectRoute};

struct FakeMachine {
    leases: Vec<Lease>,
    envs: HashMap<String, WorkspaceState>,
}

impl Machine for FakeMachine {
    fn branches(&self, _project: &ProjectRoute, _config: &Config) -> Vec<String> {
        vec!["main".into(), "feat-login".into()]
    }
    fn leases(&self, session: &str) -> Vec<Lease> {
        self.leases
            .iter()
            .filter(|lease| lease.session == session)
            .cloned()
            .collect()
    }
    fn live_port(&self, _holder: &str) -> Option<u16> {
        None
    }
    fn holder_alive(&self, _holder: &str) -> bool {
        false
    }
    fn service_envs(&self, _root: &Path, branch: &str) -> WorkspaceState {
        self.envs.get(branch).cloned().unwrap_or_default()
    }
}

fn lease(key: String, port: u16) -> Lease {
    Lease {
        key,
        port,
        session: "demo".into(),
        state: PortState::Running,
        since_ms: 0,
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("free port")
}

/// Answers each request with its request line and headers, and sets a cookie; reports each request it saw.
fn echo_backend(seen: mpsc::Sender<String>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let seen = seen.clone();
            std::thread::spawn(move || serve_echo(stream, seen));
        }
    });
    port
}

fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

fn serve_echo(mut stream: TcpStream, seen: mpsc::Sender<String>) {
    let head = read_head(&mut stream);
    if head.is_empty() {
        return;
    }
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    if stream.read_exact(&mut body).is_err() {
        return;
    }
    let report = format!("{head}{}", String::from_utf8_lossy(&body));
    if seen.send(report.clone()).is_err() {
        return;
    }
    if head.to_ascii_lowercase().contains("upgrade: echo") {
        let handshake =
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: echo\r\nConnection: Upgrade\r\n\r\n";
        if stream.write_all(handshake.as_bytes()).is_err() {
            return;
        }
        let mut buffer = [0u8; 64];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 || stream.write_all(&buffer[..read]).is_err() {
                return;
            }
        }
        return;
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nSet-Cookie: sid=1; Path=/; Secure\r\nConnection: close\r\n\r\n{report}",
        report.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn request(port: u16, raw: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect proxy");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream.write_all(raw.as_bytes()).expect("send");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
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
        .shared_services
        .insert("minio".into(), SharedServiceDef::default());
    config
}

fn start(leases: Vec<Lease>, envs: HashMap<String, WorkspaceState>) -> (DevProxy, Ports) {
    let ports = Ports {
        webhook: free_port(),
        proxy: free_port(),
    };
    let proxy = DevProxy::start(Box::new(FakeMachine { leases, envs }), ports).expect("start");
    proxy.set_projects(vec![ProjectRoute {
        root: "/tmp/demo".into(),
        config: Arc::new(RwLock::new(Some(Arc::new(config())))),
    }]);
    (proxy, ports)
}

fn service_lease(branch: &str, port: u16) -> Lease {
    lease(
        pom_ports::service_key(&pom_env::port_ws_key(branch), "api~server"),
        port,
    )
}

#[test]
fn routes_hosts_and_dev_paths_rewriting_cookies_and_logging() {
    let (sender, seen) = mpsc::channel();
    let backend = echo_backend(sender);
    let (proxy, ports) = start(vec![service_lease("feat-login", backend)], HashMap::new());

    let response = request(
        ports.proxy,
        "GET /health?x=1 HTTP/1.1\r\nHost: server.api.feat-login.localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("GET /health?x=1 HTTP/1.1"), "{response}");
    assert!(
        response.contains("set-cookie: sid=1; Path=/\r\n"),
        "{response}"
    );
    assert!(seen.recv_timeout(Duration::from_secs(5)).is_ok());

    let response = request(
        ports.proxy,
        "GET /_pom_dev/api/server/v1/me HTTP/1.1\r\nHost: web.web.feat-login.localhost:8767\r\nConnection: close\r\n\r\n",
    );
    assert!(response.contains("GET /v1/me HTTP/1.1"), "{response}");
    assert!(
        response.contains("set-cookie: sid=1; Path=/_pom_dev/api/server/\r\n"),
        "{response}"
    );
    let log = proxy.log(10);
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].path, "/_pom_dev/api/server/v1/me");
    assert_eq!((log[0].profile.as_str(), log[0].status), ("local", 200));
    assert_eq!(log[0].target, format!("127.0.0.1:{backend}"));

    let response = request(
        ports.proxy,
        "GET / HTTP/1.1\r\nHost: server.api.main.localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 502"), "{response}");
    assert!(response.contains("no dev-proxy route for main / api/server"));

    let response = request(
        ports.proxy,
        "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
}

#[test]
fn a_remote_profile_sends_dev_paths_to_its_url() {
    let (sender, seen) = mpsc::channel();
    let remote = echo_backend(sender);
    let mut config_envs = HashMap::new();
    let mut state = WorkspaceState::default();
    state.service_envs.insert("web".into(), "staging".into());
    config_envs.insert("feat-login".to_string(), state);
    let ports = Ports {
        webhook: free_port(),
        proxy: free_port(),
    };
    let proxy = DevProxy::start(
        Box::new(FakeMachine {
            leases: Vec::new(),
            envs: config_envs,
        }),
        ports,
    )
    .expect("start");
    let mut config = config();
    config.environments.insert(
        "staging".into(),
        [(
            "api.server".to_string(),
            format!("http://127.0.0.1:{remote}"),
        )]
        .into_iter()
        .collect(),
    );
    proxy.set_projects(vec![ProjectRoute {
        root: "/tmp/demo".into(),
        config: Arc::new(RwLock::new(Some(Arc::new(config)))),
    }]);
    let response = request(
        ports.proxy,
        "GET /_pom_dev/api/server/x HTTP/1.1\r\nHost: web.web.feat-login.localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response.contains("GET /x HTTP/1.1"), "{response}");
    let head = seen
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_default();
    assert!(
        head.contains(&format!("host: 127.0.0.1:{remote}")),
        "{head}"
    );
    assert_eq!(proxy.log(1)[0].profile, "staging");
}

#[test]
fn upgrades_tunnel_both_ways() {
    let (sender, _seen) = mpsc::channel();
    let backend = echo_backend(sender);
    let (_proxy, ports) = start(vec![service_lease("feat-login", backend)], HashMap::new());
    let mut stream = TcpStream::connect(("127.0.0.1", ports.proxy)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream
        .write_all(
            b"GET /hmr HTTP/1.1\r\nHost: server.api.feat-login.localhost\r\nConnection: Upgrade\r\nUpgrade: echo\r\n\r\n",
        )
        .expect("send");
    let head = read_head(&mut stream);
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    stream.write_all(b"ping").expect("write");
    let mut echoed = [0u8; 4];
    stream.read_exact(&mut echoed).expect("echo");
    assert_eq!(&echoed, b"ping");
}

#[test]
fn webhooks_fan_out_to_every_running_workspace() {
    let (sender, seen) = mpsc::channel();
    let main = echo_backend(sender.clone());
    let branch = echo_backend(sender);
    let (_proxy, ports) = start(
        vec![
            service_lease("main", main),
            service_lease("feat-login", branch),
            service_lease("gone", free_port()),
        ],
        HashMap::new(),
    );
    let response = request(
        ports.webhook,
        "POST /api/server/hooks/stripe?id=7 HTTP/1.1\r\nHost: relay\r\nContent-Length: 7\r\nConnection: close\r\n\r\n{\"a\":1}",
    );
    assert!(
        response.ends_with("{\"ok\":true,\"service\":\"api/server\",\"fanout\":2}"),
        "{response}"
    );
    for _ in 0..2 {
        let delivered = seen
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_default();
        assert!(
            delivered.starts_with("POST /hooks/stripe?id=7 HTTP/1.1"),
            "{delivered}"
        );
        assert!(delivered.ends_with("{\"a\":1}"), "{delivered}");
        assert!(!delivered.to_ascii_lowercase().contains("host: relay"));
    }
    let response = request(
        ports.webhook,
        "POST /nope HTTP/1.1\r\nHost: relay\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
}
