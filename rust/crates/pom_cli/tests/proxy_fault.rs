//! `pom proxy fault add|clear` through the real binary against a dev proxy running in this test: the command
//! returns only once the proxy obeys the change, so the very next request meets it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use pom_paths::StateDir;
use pom_proxy::{DevProxy, Ports, ProjectRoute, Serve, SystemMachine};
use serde_json::Value;

const POM: &str = env!("CARGO_BIN_EXE_pom");
const HOST: &str = "web.be.main.localhost";

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomflt")
            .tempdir_in("/tmp")
            .expect("temp");
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join("workspace--main/api/.git")).expect("worktree");
        std::fs::write(
            root.join("pom.yml"),
            "session: demo\ndefault_branch: main\nrepos:\n  api:\n    alias: be\n    services:\n      web: { cmd: \"sleep 100\" }\n",
        )
        .expect("pom.yml");
        Fixture { temp }
    }

    fn root(&self) -> PathBuf {
        self.temp.path().join("project")
    }

    fn state(&self) -> StateDir {
        StateDir::new(self.temp.path().join("state/pom"))
    }

    fn pom(&self, base: u16, args: &[&str]) -> Output {
        Command::new(POM)
            .args(args)
            .current_dir(self.root().join("workspace--main"))
            .env("XDG_STATE_HOME", self.temp.path().join("state"))
            .env("POMELO_CONFIG_DIR", self.temp.path().join("config"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", base.to_string())
            .env("POM_PTY_SOCK_DIR", self.temp.path().join("s"))
            .output()
            .expect("pom")
    }

    fn json(&self, base: u16, args: &[&str]) -> Value {
        let output = self.pom(base, args);
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("one JSON document")
    }

    fn proxy(&self, base: u16) -> DevProxy {
        let machine = SystemMachine {
            state: self.state(),
            holders: pom_ptyhost::SocketDir::new(self.temp.path().join("s")),
            waker: None,
        };
        let proxy = DevProxy::start(Box::new(machine), Ports::from_base(base), Serve::default())
            .expect("proxy");
        let config = pom_config::Config::load(&self.root().join("pom.yml")).expect("config");
        proxy.set_projects(vec![ProjectRoute {
            root: self.root(),
            config: Arc::new(RwLock::new(Some(Arc::new(config)))),
        }]);
        proxy
    }
}

/// A base whose webhook and proxy ports (base + 1, base + 2) are both free.
fn free_base() -> u16 {
    (0..50)
        .find_map(|_| {
            let port = TcpListener::bind("127.0.0.1:0")
                .and_then(|listener| listener.local_addr())
                .ok()?
                .port();
            let base = port.checked_sub(2)?;
            let free = |port: u16| {
                TcpListener::bind(("127.0.0.1", port)).is_ok()
                    && TcpListener::bind(("::1", port)).is_ok()
            };
            (free(base + 1) && free(base + 2)).then_some(base)
        })
        .expect("free ports")
}

fn get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect proxy");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .expect("send");
    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        panic!("read {path}: {error}");
    }
    response
}

// One test: a second one running alongside could take this one's ports for its own proxy.
#[test]
fn a_fault_hits_the_first_request_after_add_returns_and_is_gone_after_clear() {
    let fixture = Fixture::new();
    let base = free_base();

    let unapplied = fixture.json(
        base,
        &[
            "proxy", "fault", "add", "be/web", "--delay", "200ms", "--json",
        ],
    );
    assert_eq!(unapplied["applied"], false, "{unapplied}");
    assert_eq!(unapplied["reason"], "no proxy running", "{unapplied}");
    assert!(Path::new(&pom_proxy::faults_path(&fixture.state())).is_file());
    let id = unapplied["id"].as_str().expect("id");
    let removed = fixture.json(base, &["proxy", "fault", "rm", id, "--json"]);
    assert_eq!(removed["applied"], false, "{removed}");

    let _proxy = fixture.proxy(base);
    let proxy_port = Ports::from_base(base).proxy;
    let before = get(proxy_port, "/v1/orders");
    assert!(!before.contains("injected"), "{before}");

    let added = fixture.json(
        base,
        &[
            "proxy", "fault", "add", "be/web", "--status", "503", "--path", "/v1", "--json",
        ],
    );
    assert_eq!(added["applied"], true, "{added}");
    assert!(added.get("reason").is_none(), "{added}");
    let id = added["id"].as_str().expect("id").to_string();
    let first = get(proxy_port, "/v1/orders");
    assert!(first.starts_with("HTTP/1.1 503"), "{first}");
    assert!(first.contains(&format!("fault {id} injected")), "{first}");

    let cleared = fixture.json(base, &["proxy", "fault", "clear", "-o", "json"]);
    assert_eq!(cleared["applied"], true, "{cleared}");
    assert_eq!(cleared["cleared"], 1, "{cleared}");
    let after = get(proxy_port, "/v1/orders");
    assert!(!after.contains("injected"), "{after}");
}
