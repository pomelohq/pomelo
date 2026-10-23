//! Sync on push through the real binary: `pom proxy` serves the relay on its own ports, a signed GitHub
//! `push` for a workspace's branch arrives at `/_pom/github`, and the worktree (a real git clone of a local
//! origin) is fast-forwarded and only the services the change touched restart, or it is left alone.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const POM: &str = env!("CARGO_BIN_EXE_pom");
const SYNC_TIMEOUT: Duration = Duration::from_secs(60);

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Dev")
        .env("GIT_AUTHOR_EMAIL", "dev@example.net")
        .env("GIT_COMMITTER_NAME", "Dev")
        .env("GIT_COMMITTER_EMAIL", "dev@example.net")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("folder");
    }
    std::fs::write(path, text).expect("write");
}

/// Three ports in a row nothing listens on: the base, the relay (base + 1) and the proxy (base + 2).
fn free_base() -> u16 {
    let seed = std::process::id() as u64
        ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    for attempt in 0..500u64 {
        let base = 20000 + ((seed.wrapping_mul(2_654_435_761) + attempt * 7919) % 20000) as u16;
        let held: Vec<TcpListener> = (base..=base + 2)
            .filter_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
            .collect();
        if held.len() == 3 {
            return base;
        }
    }
    panic!("no three free ports in a row");
}

struct Fixture {
    temp: tempfile::TempDir,
    base: u16,
    proxy: Option<Child>,
    secret: String,
}

impl Fixture {
    fn new(on_push: &str) -> Fixture {
        let temp = tempfile::Builder::new()
            .prefix("pomps")
            .tempdir_in("/tmp")
            .expect("temp");
        let top = temp.path();
        let origin = top.join("origin.git");
        std::fs::create_dir_all(&origin).expect("origin");
        git(&origin, &["init", "--bare", "-q", "-b", "main"]);
        let upstream = top.join("upstream");
        std::fs::create_dir_all(&upstream).expect("upstream");
        git(&upstream, &["init", "-q", "-b", "main"]);
        write(&upstream.join("web/index.txt"), "web 1\n");
        write(&upstream.join("worker/main.txt"), "worker 1\n");
        write(&upstream.join("package-lock.json"), "{}\n");
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", "first"]);
        git(
            &upstream,
            &["remote", "add", "origin", origin.to_str().expect("path")],
        );
        git(&upstream, &["push", "-q", "origin", "main"]);
        git(&upstream, &["checkout", "-q", "-b", "feat"]);
        git(&upstream, &["push", "-q", "-u", "origin", "feat"]);

        let root = top.join("project");
        std::fs::create_dir_all(root.join("workspace--main")).expect("main");
        git(
            &root.join("workspace--main"),
            &["clone", "-q", origin.to_str().expect("path"), "api"],
        );
        git(
            &root.join("workspace--main/api"),
            &["worktree", "add", "-q", "../../workspace--feat/api", "feat"],
        );
        std::fs::create_dir_all(top.join("zdot")).expect("zdot");
        let log = top.join("starts.log");
        let setup = top.join("setup.log");
        write(
            &root.join("pom.yml"),
            &format!(
                r#"session: demo
default_branch: main
sync:
  on_push: {on_push}
repos:
  api:
    setup: ["echo setup >> {setup}"]
    services:
      web:
        dir: web
        cmd: "echo web >> {log}; exec sleep 1000"
      worker:
        dir: worker
        cmd: "echo worker >> {log}; exec sleep 1000"
"#,
                setup = setup.display(),
                log = log.display(),
            ),
        );
        let mut fixture = Fixture {
            temp,
            base: free_base(),
            proxy: None,
            secret: String::new(),
        };
        fixture.ok(&["-w", "feat", "start", "api"]);
        let setup = fixture.json(&["github", "webhook", "setup", "--json"]);
        fixture.secret = setup["secret"].as_str().expect("secret").to_string();
        assert_eq!(setup["secret_created"], true);
        assert_eq!(
            setup["payload_url"],
            format!("http://127.0.0.1:{}/_pom/github", fixture.base + 1)
        );
        let again = fixture.json(&["github", "webhook", "setup", "--json"]);
        assert_eq!(again["secret"], serde_json::Value::Null, "shown once");
        fixture.serve();
        fixture
    }

    fn top(&self) -> &Path {
        self.temp.path()
    }

    fn root(&self) -> PathBuf {
        self.top().join("project")
    }

    fn feat(&self) -> PathBuf {
        self.root().join("workspace--feat/api")
    }

    fn upstream(&self) -> PathBuf {
        self.top().join("upstream")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(POM);
        command
            .args(args)
            .current_dir(self.root())
            .env("XDG_STATE_HOME", self.top().join("state"))
            .env("POM_NO_PROXY", "1")
            .env("POM_WEB_PORT", self.base.to_string())
            .env("POM_PTY_SOCK_DIR", self.top().join("s"))
            .env("ZDOTDIR", self.top().join("zdot"))
            .env("HOME", self.top())
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn pom(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("pom")
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.pom(args);
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.ok(args)).expect("json")
    }

    fn serve(&mut self) {
        let child = self
            .command(&["proxy"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("pom proxy");
        self.proxy = Some(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        while TcpStream::connect(("127.0.0.1", self.base + 1)).is_err() {
            assert!(Instant::now() < deadline, "the relay never listened");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// `(status, body)` of a POST to the relay's `/_pom/github`.
    fn deliver(&self, event: &str, id: &str, body: &str, signature: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.base + 1)).expect("relay");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("timeout");
        let request = format!(
            "POST /_pom/github HTTP/1.1\r\nHost: hooks.dev.example.net\r\nContent-Type: application/json\r\nX-GitHub-Event: {event}\r\nX-GitHub-Delivery: {id}\r\nX-Hub-Signature-256: {signature}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("send");
        let mut response = String::new();
        if let Err(error) = stream.read_to_string(&mut response) {
            panic!("read: {error}");
        }
        let status = response
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        (status, body)
    }

    fn push_payload(&self, branch: &str, origin: &str) -> String {
        serde_json::json!({
            "ref": format!("refs/heads/{branch}"),
            "after": "0000000000000000000000000000000000000000",
            "deleted": false,
            "commits": [],
            "repository": {"full_name": "myorg/api", "clone_url": origin},
        })
        .to_string()
    }

    fn origin(&self) -> String {
        self.top().join("origin.git").display().to_string()
    }

    /// A signed push of `branch` of this fixture's origin.
    fn push(&self, id: &str, branch: &str) -> (u16, String) {
        let body = self.push_payload(branch, &self.origin());
        let signature = pom_push::sign(&self.secret, body.as_bytes());
        self.deliver("push", id, &body, &signature)
    }

    /// Commits a change to `file` on GitHub's `feat` and returns the new head.
    fn upstream_commit(&self, file: &str, text: &str) -> String {
        let upstream = self.upstream();
        write(&upstream.join(file), text);
        git(&upstream, &["add", "."]);
        git(&upstream, &["commit", "-q", "-m", file]);
        git(&upstream, &["push", "-q", "origin", "feat"]);
        git(&upstream, &["rev-parse", "HEAD"])
    }

    fn head(&self) -> String {
        git(&self.feat(), &["rev-parse", "HEAD"])
    }

    fn events(&self) -> Vec<serde_json::Value> {
        self.json(&["sync", "status", "--limit", "200", "--json"])["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The sync results (not the queued deliveries) of workspace feat, newest first.
    fn results(&self) -> Vec<serde_json::Value> {
        self.events()
            .into_iter()
            .filter(|event| event["workspace"] == "feat" && event["outcome"] != "queued")
            .collect()
    }

    /// Waits until workspace feat has `count` sync results, and returns the newest.
    fn wait_results(&self, count: usize) -> serde_json::Value {
        let deadline = Instant::now() + SYNC_TIMEOUT;
        loop {
            let results = self.results();
            if results.len() >= count {
                return results[0].clone();
            }
            assert!(
                Instant::now() < deadline,
                "no sync result {count}: {:?}",
                self.events()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn starts(&self, service: &str) -> usize {
        std::fs::read_to_string(self.top().join("starts.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| *line == service)
            .count()
    }

    fn setups(&self) -> usize {
        std::fs::read_to_string(self.top().join("setup.log"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn feat_listing(&self) -> serde_json::Value {
        let listed = self.json(&["ws", "ls", "--json"]);
        listed["workspaces"]
            .as_array()
            .and_then(|list| list.iter().find(|workspace| workspace["branch"] == "feat"))
            .cloned()
            .unwrap_or_default()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut proxy) = self.proxy.take() {
            if let Err(error) = proxy.kill() {
                eprintln!("stop pom proxy: {error}");
            }
            if let Err(error) = proxy.wait() {
                eprintln!("reap pom proxy: {error}");
            }
        }
        let dir = pom_ptyhost::SocketDir::new(self.top().join("s"));
        let names: Vec<String> = dir.holders().into_iter().map(|(name, _)| name).collect();
        dir.kill_holders_now(&names);
    }
}

fn wait_for(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_signed_push_pulls_and_restarts_only_what_it_touched() {
    let fixture = Fixture::new("pull");
    wait_for("both services started", || {
        fixture.starts("web") == 1 && fixture.starts("worker") == 1
    });

    let (status, body) =
        fixture.deliver("ping", "p1", "{}", &pom_push::sign(&fixture.secret, b"{}"));
    assert_eq!(status, 200, "{body}");

    let before = fixture.head();
    let head = fixture.upstream_commit("web/index.txt", "web 2\n");
    let body = fixture.push_payload("feat", &fixture.origin());
    let (status, answer) = fixture.deliver("push", "bad", &body, "sha256=00");
    assert_eq!(status, 401, "{answer}");
    let (status, answer) = fixture.deliver(
        "push",
        "bad2",
        &body,
        &pom_push::sign("not the secret", body.as_bytes()),
    );
    assert_eq!(status, 401, "{answer}");
    let (status, answer) = fixture.push("other-branch", "other");
    assert_eq!(status, 200, "{answer}");
    assert!(answer.contains(r#""workspaces":[]"#), "{answer}");
    let other_repo = fixture.push_payload("feat", "https://github.com/myorg/other.git");
    let (status, answer) = fixture.deliver(
        "push",
        "other-repo",
        &other_repo,
        &pom_push::sign(&fixture.secret, other_repo.as_bytes()),
    );
    assert_eq!(status, 200, "{answer}");
    assert!(answer.contains(r#""workspaces":[]"#), "{answer}");
    // A push is journaled and queued before its reply, so one that left no event for feat never syncs it.
    assert!(
        fixture
            .events()
            .iter()
            .all(|event| event["workspace"] != "feat"),
        "{:?}",
        fixture.events()
    );
    assert_eq!(
        fixture.head(),
        before,
        "nothing but a signed push of feat moves it"
    );
    assert_eq!(
        fixture.feat_listing()["remote_ahead"],
        0,
        "nothing fetched yet"
    );

    let (status, answer) = fixture.push("d1", "feat");
    assert_eq!(status, 200, "{answer}");
    assert!(answer.contains(r#""action":"queued""#), "{answer}");
    let result = fixture.wait_results(1);
    assert_eq!(result["outcome"], "pulled", "{result}");
    assert_eq!(result["commits"], 1);
    assert_eq!(
        result["restarted"],
        serde_json::json!(["api/web"]),
        "{result}"
    );
    assert_eq!(result["setup"], false);
    assert_eq!(fixture.head(), head);
    wait_for("web restarted", || fixture.starts("web") == 2);
    assert_eq!(fixture.starts("worker"), 1, "worker was not touched");
    assert_eq!(fixture.setups(), 0);
    let listing = fixture.feat_listing();
    assert_eq!(listing["remote_ahead"], 0, "{listing}");
    assert_eq!(listing["last_sync"]["outcome"], "pulled", "{listing}");

    let head = fixture.upstream_commit("package-lock.json", "{\"v\": 2}\n");
    fixture.push("d2", "feat");
    let result = fixture.wait_results(2);
    assert_eq!(result["outcome"], "pulled", "{result}");
    assert_eq!(result["setup"], true, "{result}");
    assert_eq!(fixture.head(), head);
    assert_eq!(fixture.setups(), 1);
    wait_for("a new lockfile restarts every service", || {
        fixture.starts("web") == 3 && fixture.starts("worker") == 2
    });

    fixture.ok(&["ws", "sleep", "feat"]);
    let head = fixture.upstream_commit("web/index.txt", "web 3\n");
    fixture.push("d3", "feat");
    let result = fixture.wait_results(3);
    assert_eq!(result["outcome"], "pulled", "{result}");
    assert_eq!(result["restarted"], serde_json::json!([]), "{result}");
    assert_eq!(
        fixture.head(),
        head,
        "a sleeping workspace still fast-forwards"
    );
    assert_eq!(fixture.starts("web"), 3, "and nothing starts");

    let status = fixture.ok(&["sync", "status"]);
    assert!(status.contains("pulled"), "{status}");
    assert!(status.contains("pong"), "{status}");
}

#[test]
fn local_work_is_never_touched() {
    let fixture = Fixture::new("pull");
    let before = fixture.head();
    write(&fixture.feat().join("web/index.txt"), "my edit\n");
    fixture.upstream_commit("web/index.txt", "web 2\n");
    fixture.push("d1", "feat");
    let result = fixture.wait_results(1);
    assert_eq!(result["outcome"], "dirty", "{result}");
    assert_eq!(result["commits"], 1);
    assert_eq!(result["can_pull"], false);
    assert_eq!(fixture.head(), before);
    assert_eq!(
        std::fs::read_to_string(fixture.feat().join("web/index.txt")).expect("edit"),
        "my edit\n"
    );
    assert_eq!(
        fixture.feat_listing()["remote_ahead"],
        1,
        "fetched, not pulled"
    );

    git(&fixture.feat(), &["commit", "-q", "-am", "local"]);
    let local = fixture.head();
    fixture.push("d2", "feat");
    let result = fixture.wait_results(2);
    assert_eq!(result["outcome"], "diverged", "{result}");
    assert_eq!(fixture.head(), local);
    assert_eq!(fixture.starts("web"), 1, "nothing restarted");
}

#[test]
fn notify_only_tells() {
    let fixture = Fixture::new("notify");
    let before = fixture.head();
    fixture.upstream_commit("web/index.txt", "web 2\n");
    fixture.upstream_commit("worker/main.txt", "worker 2\n");
    fixture.push("d1", "feat");
    let result = fixture.wait_results(1);
    assert_eq!(result["outcome"], "notified", "{result}");
    assert_eq!(result["commits"], 2);
    assert_eq!(result["can_pull"], true);
    assert_eq!(fixture.head(), before);
    let listing = fixture.feat_listing();
    assert_eq!(listing["remote_ahead"], 2, "{listing}");
    assert_eq!(listing["last_sync"]["outcome"], "notified", "{listing}");
    assert_eq!(fixture.starts("web"), 1);
}

#[test]
fn a_burst_of_pushes_syncs_once() {
    let fixture = Fixture::new("pull");
    let mut head = String::new();
    for (index, id) in ["b1", "b2", "b3"].iter().enumerate() {
        head = fixture.upstream_commit("web/index.txt", &format!("web {index}\n"));
        let (status, answer) = fixture.push(id, "feat");
        assert_eq!(status, 200, "{answer}");
    }
    let result = fixture.wait_results(1);
    assert_eq!(result["outcome"], "pulled", "{result}");
    assert_eq!(result["deliveries"], serde_json::json!(["b1", "b2", "b3"]));
    assert_eq!(result["commits"], 3);
    assert_eq!(fixture.head(), head);
    assert_eq!(fixture.results().len(), 1, "{:?}", fixture.events());
    wait_for("web restarted once", || fixture.starts("web") == 2);
}
