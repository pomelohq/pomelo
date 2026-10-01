//! Holders as real detached processes (the `pomelo-pty` binary), driven through the client API.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use pom_ptyhost::{Attached, SocketDir, SpawnRequest};

const BINARY: &str = env!("CARGO_BIN_EXE_pomelo-pty");
const TIMEOUT: Duration = Duration::from_secs(5);

struct Holders {
    _temp: tempfile::TempDir,
    dir: SocketDir,
}

impl Holders {
    fn new() -> Holders {
        // Short path: Unix socket paths are limited to 104 bytes.
        let temp = tempfile::Builder::new()
            .prefix("pty")
            .tempdir_in("/tmp")
            .expect("temp dir");
        let dir = SocketDir::new(temp.path());
        Holders { _temp: temp, dir }
    }

    fn spawn(&self, name: &str, argv: &[&str]) {
        let argv: Vec<String> = argv.iter().map(|part| part.to_string()).collect();
        pom_ptyhost::spawn_holder(
            &self.dir,
            &SpawnRequest {
                binary: Path::new(BINARY),
                name,
                cwd: Path::new("/"),
                cols: 100,
                rows: 30,
                argv: &argv,
                env: &[],
            },
        )
        .expect("spawn holder");
        pom_ptyhost::wait_for_holder(&self.dir, name, TIMEOUT).expect("holder socket");
    }

    fn attach(&self, name: &str, since: u64) -> Attached {
        pom_ptyhost::attach(&self.dir, name, since).expect("attach")
    }
}

impl Drop for Holders {
    fn drop(&mut self) {
        let names: Vec<String> = self
            .dir
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        self.dir.kill_holders_now(&names);
    }
}

/// Reads live output until `want` shows up (after whatever the snapshot already held).
fn read_until(attached: &mut Attached, want: &str) -> String {
    let mut seen = String::from_utf8_lossy(&attached.snapshot).into_owned();
    let deadline = Instant::now() + TIMEOUT;
    attached
        .output
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("read timeout");
    let mut buffer = [0u8; 4096];
    while !seen.contains(want) && Instant::now() < deadline {
        match attached.output.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => seen.push_str(&String::from_utf8_lossy(&buffer[..read])),
            Err(_) => {}
        }
    }
    assert!(seen.contains(want), "wanted {want:?}, got {seen:?}");
    seen
}

#[test]
fn a_holder_survives_detach_and_replays_to_the_next_client() {
    let holders = Holders::new();
    holders.spawn("appsh-demo-1", &["cat"]);
    let mut first = holders.attach("appsh-demo-1", 0);
    first.connection.input(b"hello-A\n").expect("input");
    read_until(&mut first, "hello-A");
    drop(first);

    assert!(
        holders.dir.holder_alive("appsh-demo-1"),
        "detaching keeps it running"
    );
    let mut second = holders.attach("appsh-demo-1", 0);
    assert!(String::from_utf8_lossy(&second.snapshot).contains("hello-A"));
    second.connection.input(b"hello-B\n").expect("input");
    read_until(&mut second, "hello-B");

    let resumed = holders.attach("appsh-demo-1", second.end);
    assert!(
        !String::from_utf8_lossy(&resumed.snapshot).contains("hello-A"),
        "resuming from an offset skips what was already seen"
    );
}

#[test]
fn every_client_gets_the_output() {
    let holders = Holders::new();
    holders.spawn("appsh-fan", &["cat"]);
    let mut a = holders.attach("appsh-fan", 0);
    let mut b = holders.attach("appsh-fan", 0);
    a.connection.input(b"broadcast-1\n").expect("input");
    read_until(&mut a, "broadcast-1");
    read_until(&mut b, "broadcast-1");
}

#[test]
fn long_names_fit_the_socket_path() {
    let holders = Holders::new();
    let name = "sh-ws:proj-101-a-very-long-branch-name-for-the-analytics-tab-plus-some-more-length-to-spare";
    holders.spawn(name, &["cat"]);
    let mut attached = holders.attach(name, 0);
    attached.connection.input(b"longname-ok\n").expect("input");
    read_until(&mut attached, "longname-ok");
}

#[test]
fn killing_a_holder_takes_its_tree_down() {
    let holders = Holders::new();
    holders.spawn(
        "svc-demo-main-api-web",
        &["sh", "-c", "sleep 60 & sleep 60"],
    );
    let pid = holders
        .dir
        .holders()
        .first()
        .map(|(_, pid)| *pid)
        .expect("listed");
    std::thread::sleep(Duration::from_millis(200));
    let tree = pom_ptyhost::descendants(pid);
    assert!(tree.len() >= 3, "holder, shell and sleeps: {tree:?}");
    holders
        .dir
        .kill_holder("svc-demo-main-api-web")
        .expect("kill");
    let deadline = Instant::now() + TIMEOUT;
    while tree.iter().any(|&pid| pom_ptyhost::process_alive(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(tree.iter().all(|&pid| !pom_ptyhost::process_alive(pid)));
    assert!(holders.dir.holders().is_empty());
}

#[test]
fn a_failed_command_leaves_a_crash_log_and_cleans_up() {
    let holders = Holders::new();
    holders.spawn(
        "svc-demo-main-api-broken",
        &["sh", "-c", "sleep 0.3; printf failed; exit 3"],
    );
    let deadline = Instant::now() + TIMEOUT;
    while holders.dir.holder_alive("svc-demo-main-api-broken") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(200));
    let info = holders
        .dir
        .crash_info("svc-demo-main-api-broken")
        .expect("crash log");
    assert!(info.crashed, "{}", info.header);
    assert!(String::from_utf8_lossy(&info.output).contains("failed"));
    assert!(!holders.dir.socket("svc-demo-main-api-broken").exists());
    assert!(!holders.dir.pidfile("svc-demo-main-api-broken").exists());
}

#[test]
fn a_one_shot_snapshot_reads_the_scrollback() {
    let holders = Holders::new();
    holders.spawn("svc-peek", &["sh", "-c", "printf peek-me; sleep 30"]);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let snapshot = pom_ptyhost::snapshot(&holders.dir, "svc-peek", TIMEOUT).expect("snapshot");
        if String::from_utf8_lossy(&snapshot).contains("peek-me") {
            break;
        }
        assert!(Instant::now() < deadline, "no output in the snapshot");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn while_a_lease_is_set_only_the_client_that_claimed_it_can_type() {
    let holders = Holders::new();
    holders.spawn("leased", &["/bin/cat"]);
    let mut watcher = holders.attach("leased", 0);
    holders
        .dir
        .set_input_lease("leased", Some("token-1"))
        .expect("lease");
    watcher
        .connection
        .input(b"from-the-viewer\n")
        .expect("viewer input");
    let mut driver = pom_ptyhost::connect_writer(&holders.dir, "leased").expect("writer");
    driver.input(b"before-claim\n").expect("unclaimed");
    driver.claim("token-1").expect("claim");
    driver.input(b"from-the-driver\n").expect("driver input");
    let seen = read_until(&mut watcher, "from-the-driver");
    assert!(!seen.contains("from-the-viewer"), "{seen}");
    assert!(!seen.contains("before-claim"), "{seen}");

    holders
        .dir
        .set_input_lease("leased", None)
        .expect("release");
    watcher
        .connection
        .input(b"viewer-again\n")
        .expect("viewer input");
    read_until(&mut watcher, "viewer-again");
}
