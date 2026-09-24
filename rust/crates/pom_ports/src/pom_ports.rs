//! Port leases. Every port handed to a service or shared container is a file `ports.d/<port>` created with
//! O_EXCL, so two processes (this app, the CLI, the previous core) can never hand out the same port. A
//! manager keeps the leases of one session in memory and reclaims the ones whose service is gone.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pom_paths::StateDir;

pub const PORT_LOW: u16 = 10000;
pub const PORT_HIGH: u16 = 65535;
/// A service that never came up gives its port back after this long (once its holder is gone too).
pub const ASSIGN_GRACE: Duration = Duration::from_secs(45);
/// A service that was up must stay unreachable this long before its port is reclaimed: a dev server
/// restarting on a code change drops its socket for a moment.
pub const REAP_DOWN_GRACE: Duration = Duration::from_secs(20);
const LEASE_MAGIC: &str = "pom-port";
const LEASE_VERSION: &str = "v1";
const CLAIM_ATTEMPTS: usize = 2000;
const BIND_IP: Ipv4Addr = Ipv4Addr::LOCALHOST;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortState {
    Assigned,
    Starting,
    Running,
}

impl PortState {
    fn as_str(self) -> &'static str {
        match self {
            PortState::Assigned => "assigned",
            PortState::Starting => "starting",
            PortState::Running => "running",
        }
    }

    fn parse(text: &str) -> Option<PortState> {
        match text {
            "assigned" => Some(PortState::Assigned),
            "starting" => Some(PortState::Starting),
            "running" => Some(PortState::Running),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub key: String,
    pub port: u16,
    pub session: String,
    pub state: PortState,
    /// Milliseconds since the epoch when the lease was taken or last (re)started.
    pub since_ms: u64,
}

/// The lease key of a service in a workspace: `<ws key>\x1f<alias>~<service>`.
pub fn service_key(ws_key: &str, service_key: &str) -> String {
    format!("{ws_key}\u{1f}{service_key}")
}

/// The lease key of a shared service's `index`-th port.
pub fn shared_key(name: &str, index: usize) -> String {
    if index == 0 {
        format!("shared\u{1f}{name}")
    } else {
        format!("shared\u{1f}{name}#{index}")
    }
}

pub fn format_lease(lease: &Lease) -> String {
    format!(
        "{LEASE_MAGIC}\t{LEASE_VERSION}\t{}\t{}\t{}\t{}\n",
        lease.session,
        lease.key,
        lease.state.as_str(),
        lease.since_ms
    )
}

pub fn parse_lease(port: u16, text: &str) -> Option<Lease> {
    let fields: Vec<&str> = text.trim_end_matches('\n').split('\t').collect();
    let [magic, version, session, key, state, since] = fields.as_slice() else {
        return None;
    };
    if *magic != LEASE_MAGIC || *version != LEASE_VERSION {
        return None;
    }
    Some(Lease {
        key: key.to_string(),
        port,
        session: session.to_string(),
        state: PortState::parse(state)?,
        since_ms: since.parse().ok()?,
    })
}

/// The machine state a manager consults, behind a trait so tests pin it.
pub trait Probe: Send + Sync {
    /// Something accepts connections on the port.
    fn is_up(&self, port: u16) -> bool;
    /// Nothing holds the port, so a service can listen on it.
    fn bindable(&self, port: u16) -> bool;
    fn holder_alive(&self, holder: &str) -> bool;
    fn now_ms(&self) -> u64;
    /// The next random candidate port in the lease range.
    fn candidate(&self) -> u16;
}

/// Real sockets and clock; holders are checked through the given function.
pub struct SystemProbe {
    pub holder_alive: Box<dyn Fn(&str) -> bool + Send + Sync>,
    random: Mutex<u64>,
}

impl SystemProbe {
    pub fn new(holder_alive: Box<dyn Fn(&str) -> bool + Send + Sync>) -> SystemProbe {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |elapsed| elapsed.as_nanos() as u64)
            ^ u64::from(std::process::id()) << 32;
        SystemProbe {
            holder_alive,
            random: Mutex::new(seed | 1),
        }
    }
}

impl Probe for SystemProbe {
    fn is_up(&self, port: u16) -> bool {
        TcpStream::connect_timeout(
            &SocketAddrV4::new(BIND_IP, port).into(),
            Duration::from_millis(300),
        )
        .is_ok()
    }

    fn bindable(&self, port: u16) -> bool {
        TcpListener::bind(SocketAddrV4::new(BIND_IP, port)).is_ok()
    }

    fn holder_alive(&self, holder: &str) -> bool {
        (self.holder_alive)(holder)
    }

    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    }

    fn candidate(&self) -> u16 {
        let Ok(mut state) = self.random.lock() else {
            return PORT_LOW;
        };
        // xorshift64: plenty to spread candidates over the range.
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        let span = u64::from(PORT_HIGH - PORT_LOW) + 1;
        PORT_LOW + (*state % span) as u16
    }
}

#[derive(Default)]
struct Tracked {
    lease: Option<Lease>,
    /// Was reachable at some point, so later misses mean it went down rather than never started.
    seen_up: bool,
    /// First failed probe after being up.
    down_since_ms: Option<u64>,
    holder: Option<String>,
}

/// The leases of one session. All changes go through the lease files first.
pub struct PortManager {
    session: String,
    dir: PathBuf,
    probe: Box<dyn Probe>,
    tracked: Mutex<HashMap<String, Tracked>>,
}

impl PortManager {
    /// Loads the session's existing leases from `<state>/ports.d`.
    pub fn open(state: &StateDir, session: &str, probe: Box<dyn Probe>) -> PortManager {
        let manager = PortManager {
            session: session.to_string(),
            dir: state.path("ports.d"),
            probe,
            tracked: Mutex::new(HashMap::new()),
        };
        if let Ok(mut tracked) = manager.tracked.lock() {
            for lease in scan_leases(&manager.dir) {
                if lease.session == manager.session {
                    let seen_up = lease.state == PortState::Running;
                    tracked.insert(
                        lease.key.clone(),
                        Tracked {
                            lease: Some(lease),
                            seen_up,
                            ..Tracked::default()
                        },
                    );
                }
            }
        }
        manager
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// The port leased to `key`, taking a random free one first if it has none.
    pub fn acquire(&self, key: &str) -> Option<u16> {
        self.acquire_with(key, |manager, lease| manager.claim_random(lease))
    }

    /// Like `acquire`, but tries `base..=base+span` first (a shared service keeps its usual port when free).
    pub fn acquire_preferred(&self, key: &str, base: u16, span: u16) -> Option<u16> {
        self.acquire_with(key, |manager, lease| {
            manager
                .claim_preferred(lease, base, span)
                .or_else(|| manager.claim_random(lease))
        })
    }

    fn acquire_with(
        &self,
        key: &str,
        claim: impl FnOnce(&Self, &mut Lease) -> Option<u16>,
    ) -> Option<u16> {
        let mut tracked = self.tracked.lock().ok()?;
        if let Some(lease) = tracked.get(key).and_then(|entry| entry.lease.as_ref()) {
            return Some(lease.port);
        }
        let mut lease = Lease {
            key: key.to_string(),
            port: 0,
            session: self.session.clone(),
            state: PortState::Assigned,
            since_ms: self.probe.now_ms(),
        };
        claim(self, &mut lease)?;
        let port = lease.port;
        tracked.entry(key.to_string()).or_default().lease = Some(lease);
        Some(port)
    }

    fn claim_random(&self, lease: &mut Lease) -> Option<u16> {
        (0..CLAIM_ATTEMPTS).find_map(|_| self.try_claim(lease, self.probe.candidate()))
    }

    fn claim_preferred(&self, lease: &mut Lease, base: u16, span: u16) -> Option<u16> {
        if base == 0 {
            return None;
        }
        (0..=span)
            .map_while(|offset| base.checked_add(offset))
            .find_map(|port| self.try_claim(lease, port))
    }

    /// Creates `ports.d/<port>` exclusively; a port another process holds (file or socket) is skipped.
    fn try_claim(&self, lease: &mut Lease, port: u16) -> Option<u16> {
        if port < PORT_LOW || !self.probe.bindable(port) {
            return None;
        }
        if let Err(error) = std::fs::create_dir_all(&self.dir) {
            eprintln!("ports: {}: {error}", self.dir.display());
            return None;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.dir.join(port.to_string()))
            .ok()?;
        lease.port = port;
        if let Err(error) = file.write_all(format_lease(lease).as_bytes()) {
            eprintln!("ports: lease {port}: {error}");
        }
        Some(port)
    }

    pub fn mark(&self, key: &str, state: PortState) {
        let Ok(mut tracked) = self.tracked.lock() else {
            return;
        };
        let Some(entry) = tracked.get_mut(key) else {
            return;
        };
        let Some(lease) = entry.lease.as_mut() else {
            return;
        };
        if lease.state == state {
            return;
        }
        lease.state = state;
        match state {
            PortState::Running => entry.seen_up = true,
            PortState::Starting => lease.since_ms = self.probe.now_ms(),
            PortState::Assigned => {}
        }
        self.write(lease);
    }

    /// Ties a lease to the holder running its service: a slow first build keeps its port while the holder
    /// lives, even past the assign grace.
    pub fn set_holder(&self, key: &str, holder: &str) {
        if let Ok(mut tracked) = self.tracked.lock() {
            if let Some(entry) = tracked.get_mut(key) {
                entry.holder = Some(holder.to_string());
            }
        }
    }

    pub fn release(&self, key: &str) {
        if let Ok(mut tracked) = self.tracked.lock() {
            if let Some(lease) = tracked.remove(key).and_then(|entry| entry.lease) {
                self.remove_file(lease.port);
            }
        }
    }

    /// Drops every lease of a workspace (it was deleted).
    pub fn release_workspace(&self, ws_key: &str) {
        let prefix = format!("{ws_key}\u{1f}");
        if let Ok(mut tracked) = self.tracked.lock() {
            let keys: Vec<String> = tracked
                .keys()
                .filter(|key| key.starts_with(&prefix))
                .cloned()
                .collect();
            for key in keys {
                if let Some(lease) = tracked.remove(&key).and_then(|entry| entry.lease) {
                    self.remove_file(lease.port);
                }
            }
        }
    }

    pub fn port_of(&self, key: &str) -> Option<u16> {
        let tracked = self.tracked.lock().ok()?;
        tracked.get(key)?.lease.as_ref().map(|lease| lease.port)
    }

    pub fn leases(&self) -> Vec<Lease> {
        let mut leases: Vec<Lease> = self
            .tracked
            .lock()
            .map(|tracked| {
                tracked
                    .values()
                    .filter_map(|entry| entry.lease.clone())
                    .collect()
            })
            .unwrap_or_default();
        leases.sort_by_key(|lease| lease.port);
        leases
    }

    /// Probes every lease: reachable ones become `running`; ones that were up and stayed down past the grace,
    /// or never came up within the assign grace with their holder gone, are reclaimed.
    pub fn reap(&self) {
        let Ok(mut tracked) = self.tracked.lock() else {
            return;
        };
        let now = self.probe.now_ms();
        let mut dead = Vec::new();
        for (key, entry) in tracked.iter_mut() {
            let Some(lease) = entry.lease.as_mut() else {
                continue;
            };
            if self.probe.is_up(lease.port) {
                entry.down_since_ms = None;
                if lease.state != PortState::Running {
                    lease.state = PortState::Running;
                    entry.seen_up = true;
                    self.write(lease);
                }
            } else if entry.seen_up || lease.state == PortState::Running {
                let since = *entry.down_since_ms.get_or_insert(now);
                if now.saturating_sub(since) > REAP_DOWN_GRACE.as_millis() as u64 {
                    dead.push(key.clone());
                }
            } else if lease.state == PortState::Starting
                && now.saturating_sub(lease.since_ms) > ASSIGN_GRACE.as_millis() as u64
            {
                let building = entry
                    .holder
                    .as_deref()
                    .is_some_and(|holder| self.probe.holder_alive(holder));
                if !building {
                    dead.push(key.clone());
                }
            }
        }
        for key in dead {
            if let Some(lease) = tracked.remove(&key).and_then(|entry| entry.lease) {
                self.remove_file(lease.port);
            }
        }
    }

    fn write(&self, lease: &Lease) {
        if let Err(error) =
            std::fs::write(self.dir.join(lease.port.to_string()), format_lease(lease))
        {
            eprintln!("ports: lease {}: {error}", lease.port);
        }
    }

    fn remove_file(&self, port: u16) {
        if let Err(error) = std::fs::remove_file(self.dir.join(port.to_string())) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("ports: release {port}: {error}");
            }
        }
    }
}

/// Every readable lease under `dir`, of any session.
pub fn scan_leases(dir: &Path) -> Vec<Lease> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut leases: Vec<Lease> = entries
        .flatten()
        .filter_map(|entry| {
            let port: u16 = entry.file_name().to_str()?.parse().ok()?;
            let text = std::fs::read_to_string(entry.path()).ok()?;
            parse_lease(port, &text)
        })
        .collect();
    leases.sort_by_key(|lease| lease.port);
    leases
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct FakeProbe {
        up: Mutex<HashSet<u16>>,
        busy: Mutex<HashSet<u16>>,
        alive: Mutex<HashSet<String>>,
        clock: AtomicU64,
        sequence: Mutex<Vec<u16>>,
        random: Mutex<u64>,
    }

    impl Probe for Arc<FakeProbe> {
        fn is_up(&self, port: u16) -> bool {
            self.up.lock().is_ok_and(|up| up.contains(&port))
        }
        fn bindable(&self, port: u16) -> bool {
            !self.busy.lock().is_ok_and(|busy| busy.contains(&port))
        }
        fn holder_alive(&self, holder: &str) -> bool {
            self.alive.lock().is_ok_and(|alive| alive.contains(holder))
        }
        fn now_ms(&self) -> u64 {
            self.clock.load(Ordering::SeqCst)
        }
        fn candidate(&self) -> u16 {
            if let Ok(mut sequence) = self.sequence.lock() {
                if !sequence.is_empty() {
                    return sequence.remove(0);
                }
            }
            let Ok(mut state) = self.random.lock() else {
                return PORT_LOW;
            };
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            PORT_LOW + ((*state >> 33) % u64::from(PORT_HIGH - PORT_LOW)) as u16
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        state: StateDir,
        probe: Arc<FakeProbe>,
    }

    impl Fixture {
        fn new() -> Fixture {
            let temp = tempfile::tempdir().expect("temp dir");
            let state = StateDir::new(temp.path().join("pom"));
            Fixture {
                _temp: temp,
                state,
                probe: Arc::new(FakeProbe::default()),
            }
        }

        fn manager(&self, session: &str) -> PortManager {
            PortManager::open(&self.state, session, Box::new(self.probe.clone()))
        }

        fn advance(&self, duration: Duration) {
            self.probe
                .clock
                .fetch_add(duration.as_millis() as u64, Ordering::SeqCst);
        }
    }

    #[test]
    fn lease_format_matches_the_previous_core() {
        let lease = Lease {
            key: service_key("ws-feat", "api~web"),
            port: 12345,
            session: "demo".into(),
            state: PortState::Starting,
            since_ms: 1_700_000_000_000,
        };
        let text = format_lease(&lease);
        assert_eq!(
            text,
            "pom-port\tv1\tdemo\tws-feat\u{1f}api~web\tstarting\t1700000000000\n"
        );
        assert_eq!(parse_lease(12345, &text), Some(lease));
        assert_eq!(parse_lease(1, "garbage"), None);
        assert_eq!(shared_key("postgres", 0), "shared\u{1f}postgres");
        assert_eq!(shared_key("minio", 1), "shared\u{1f}minio#1");
    }

    #[test]
    fn acquire_is_sticky_and_distinct() {
        let fixture = Fixture::new();
        let manager = fixture.manager("demo");
        let first = manager.acquire("ws\u{1f}api").expect("port");
        assert_eq!(manager.acquire("ws\u{1f}api"), Some(first));
        let ports: HashSet<u16> = (0..20)
            .filter_map(|index| manager.acquire(&format!("ws\u{1f}svc{index}")))
            .collect();
        assert_eq!(ports.len(), 20);
        assert!(!ports.contains(&first));
        assert!(ports
            .iter()
            .all(|port| (PORT_LOW..=PORT_HIGH).contains(port)));
        assert!(fixture.state.path(format!("ports.d/{first}")).is_file());
    }

    #[test]
    fn a_port_leased_elsewhere_or_bound_is_skipped() {
        let fixture = Fixture::new();
        let other = fixture.manager("other");
        if let Ok(mut sequence) = fixture.probe.sequence.lock() {
            sequence.extend([20000, 20000, 20001, 20002]);
        }
        assert_eq!(other.acquire("x"), Some(20000));
        if let Ok(mut busy) = fixture.probe.busy.lock() {
            busy.insert(20001);
        }
        let manager = fixture.manager("demo");
        assert_eq!(
            manager.acquire("y"),
            Some(20002),
            "20000 has a lease file, 20001 is bound"
        );
    }

    #[test]
    fn two_managers_racing_never_share_a_port() {
        let fixture = Fixture::new();
        let a = Arc::new(fixture.manager("proc-a"));
        let b = Arc::new(fixture.manager("proc-b"));
        let mut handles = Vec::new();
        for index in 0..15 {
            for (manager, tag) in [(a.clone(), "a"), (b.clone(), "b")] {
                handles.push(std::thread::spawn(move || {
                    manager.acquire(&format!("ws\u{1f}{tag}{index}"))
                }));
            }
        }
        let ports: Vec<u16> = handles
            .into_iter()
            .filter_map(|handle| handle.join().ok().flatten())
            .collect();
        assert_eq!(ports.len(), 30);
        assert_eq!(ports.iter().collect::<HashSet<_>>().len(), 30);
    }

    #[test]
    fn preferred_port_is_used_when_free() {
        let fixture = Fixture::new();
        let manager = fixture.manager("demo");
        assert_eq!(
            manager.acquire_preferred(&shared_key("postgres", 0), 15432, 100),
            Some(15432)
        );
        if let Ok(mut busy) = fixture.probe.busy.lock() {
            busy.insert(16379);
        }
        assert_eq!(
            manager.acquire_preferred(&shared_key("redis", 0), 16379, 100),
            Some(16380)
        );
        // A base below the lease range (e.g. 5432) falls back to a random port.
        let low = manager
            .acquire_preferred(&shared_key("low", 0), 5432, 3)
            .expect("port");
        assert!(low >= PORT_LOW);
    }

    #[test]
    fn a_new_manager_sees_only_its_sessions_leases() {
        let fixture = Fixture::new();
        let first = fixture.manager("demo");
        let port = first.acquire("ws\u{1f}api").expect("port");
        first.mark("ws\u{1f}api", PortState::Running);
        fixture.manager("other").acquire("ws\u{1f}web");
        let reopened = fixture.manager("demo");
        assert_eq!(reopened.port_of("ws\u{1f}api"), Some(port));
        assert_eq!(reopened.leases().len(), 1);
        assert_eq!(reopened.leases()[0].state, PortState::Running);
    }

    #[test]
    fn reap_marks_running_survives_blips_and_reclaims_sustained_downs() {
        let fixture = Fixture::new();
        let manager = fixture.manager("demo");
        let key = "ws\u{1f}web";
        let port = manager.acquire(key).expect("port");
        if let Ok(mut up) = fixture.probe.up.lock() {
            up.insert(port);
        }
        manager.reap();
        assert_eq!(manager.leases()[0].state, PortState::Running);
        if let Ok(mut up) = fixture.probe.up.lock() {
            up.clear();
        }
        manager.reap();
        fixture.advance(Duration::from_secs(5));
        manager.reap();
        assert_eq!(
            manager.port_of(key),
            Some(port),
            "a short blip keeps the port"
        );
        fixture.advance(REAP_DOWN_GRACE + Duration::from_secs(1));
        manager.reap();
        assert_eq!(manager.port_of(key), None);
        assert!(!fixture.state.path(format!("ports.d/{port}")).exists());
    }

    #[test]
    fn a_slow_build_keeps_its_port_while_its_holder_lives() {
        let fixture = Fixture::new();
        let manager = fixture.manager("demo");
        let (building, failed) = ("ws\u{1f}api", "ws\u{1f}worker");
        manager.acquire(building);
        manager.acquire(failed);
        manager.mark(building, PortState::Starting);
        manager.mark(failed, PortState::Starting);
        manager.set_holder(building, "svc-demo-ws-api");
        manager.set_holder(failed, "svc-demo-ws-worker");
        if let Ok(mut alive) = fixture.probe.alive.lock() {
            alive.insert("svc-demo-ws-api".into());
        }
        fixture.advance(ASSIGN_GRACE + Duration::from_secs(1));
        manager.reap();
        assert!(manager.port_of(building).is_some());
        assert!(manager.port_of(failed).is_none());
    }

    #[test]
    fn releasing_a_workspace_drops_only_its_leases() {
        let fixture = Fixture::new();
        let manager = fixture.manager("demo");
        manager.acquire(&service_key("ws-a", "api~web"));
        manager.acquire(&service_key("ws-a", "api~worker"));
        let kept = manager
            .acquire(&service_key("ws-ab", "api~web"))
            .expect("port");
        manager.release_workspace("ws-a");
        let leases = manager.leases();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].port, kept);
        assert_eq!(scan_leases(&fixture.state.path("ports.d")).len(), 1);
    }

    #[test]
    fn system_probe_sees_a_real_listener() -> std::io::Result<()> {
        let listener = TcpListener::bind(SocketAddrV4::new(BIND_IP, 0))?;
        let port = listener.local_addr()?.port();
        let probe = SystemProbe::new(Box::new(|_| false));
        assert!(probe.is_up(port));
        assert!(!probe.bindable(port));
        drop(listener);
        assert!(probe.bindable(port));
        let candidates: HashSet<u16> = (0..50).map(|_| probe.candidate()).collect();
        assert!(candidates.len() > 40, "candidates spread over the range");
        assert!(candidates.iter().all(|port| *port >= PORT_LOW));
        Ok(())
    }
}
