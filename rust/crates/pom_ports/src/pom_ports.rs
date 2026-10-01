//! Port leases. Every port handed to a service or shared container is a file `ports.d/<port>` created with
//! O_EXCL, so two processes can never hand out the same port. Each leased key also has one file
//! `keys.d/<hash>` naming its port, linked into place atomically, so the app, the CLI and every agent's MCP
//! server find the same port for a service. The files are the only truth: a manager reads them on every
//! lookup and keeps just its probe bookkeeping in memory.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
/// A port leased up front for a service that is never started is kept this long, so env files written
/// with it stay right for a while.
pub const ASSIGNED_EXPIRY: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const LEASE_MAGIC: &str = "pom-port";
const KEY_MAGIC: &str = "pom-key";
const LEASE_VERSION: &str = "v1";
const CLAIM_ATTEMPTS: usize = 2000;
const KEY_ATTEMPTS: usize = 4;
const LOOPBACKS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

static STAGED: AtomicU64 = AtomicU64::new(0);

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

    fn rank(self) -> u8 {
        match self {
            PortState::Assigned => 0,
            PortState::Starting => 1,
            PortState::Running => 2,
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

/// Of several leases for one key, the one most likely in use: the furthest along, then the newest.
pub fn preferred_lease<'a>(leases: impl IntoIterator<Item = &'a Lease>) -> Option<&'a Lease> {
    leases
        .into_iter()
        .max_by_key(|lease| (lease.state.rank(), lease.since_ms))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct KeyEntry {
    port: u16,
    /// The holder running the key's service, so any process can tell a long build from a dead service.
    holder: String,
}

/// A key file name stable across processes and builds (FNV-1a; the std hasher is seeded per process).
fn key_file_name(session: &str, key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in session.bytes().chain([0]).chain(key.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn format_key_entry(session: &str, key: &str, entry: &KeyEntry) -> String {
    format!(
        "{KEY_MAGIC}\t{LEASE_VERSION}\t{}\t{}\t{session}\t{key}\n",
        entry.port, entry.holder
    )
}

fn parse_key_entry(text: &str, session: &str, key: &str) -> Option<KeyEntry> {
    let fields: Vec<&str> = text.trim_end_matches('\n').split('\t').collect();
    let [magic, version, port, holder, file_session, file_key] = fields.as_slice() else {
        return None;
    };
    if *magic != KEY_MAGIC
        || *version != LEASE_VERSION
        || *file_session != session
        || *file_key != key
    {
        return None;
    }
    Some(KeyEntry {
        port: port.parse().ok()?,
        holder: holder.to_string(),
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

/// Something listens on the port on either loopback: dev servers bind `localhost`, which may be `::1` only.
pub fn loopback_up(port: u16, timeout: Duration) -> bool {
    LOOPBACKS
        .iter()
        .any(|ip| TcpStream::connect_timeout(&SocketAddr::new(*ip, port), timeout).is_ok())
}

impl Probe for SystemProbe {
    fn is_up(&self, port: u16) -> bool {
        loopback_up(port, Duration::from_millis(300))
    }

    fn bindable(&self, port: u16) -> bool {
        LOOPBACKS
            .iter()
            .all(|ip| match TcpListener::bind(SocketAddr::new(*ip, port)) {
                Ok(_) => true,
                // A machine without IPv6 loopback can't hold the port there either.
                Err(error) => error.kind() == std::io::ErrorKind::AddrNotAvailable,
            })
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
    /// Was reachable at some point, so later misses mean it went down rather than never started.
    seen_up: bool,
    /// First failed probe after being up.
    down_since_ms: Option<u64>,
    holder: Option<String>,
}

/// The leases of one session, read from and written to the lease files.
pub struct PortManager {
    session: String,
    dir: PathBuf,
    keys: PathBuf,
    probe: Box<dyn Probe>,
    tracked: Mutex<HashMap<String, Tracked>>,
}

impl PortManager {
    /// Opens the session's leases under `<state>/ports.d`, collapsing any duplicates to one per key.
    pub fn open(state: &StateDir, session: &str, probe: Box<dyn Probe>) -> PortManager {
        let manager = PortManager {
            session: session.to_string(),
            dir: state.path("ports.d"),
            keys: state.path("keys.d"),
            probe,
            tracked: Mutex::new(HashMap::new()),
        };
        manager.repair();
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
        claim: impl Fn(&Self, &mut Lease) -> Option<u16>,
    ) -> Option<u16> {
        for _ in 0..KEY_ATTEMPTS {
            if let Some(lease) = self.lookup(key) {
                return Some(lease.port);
            }
            let mut lease = Lease {
                key: key.to_string(),
                port: 0,
                session: self.session.clone(),
                state: PortState::Assigned,
                since_ms: self.probe.now_ms(),
            };
            let port = claim(self, &mut lease)?;
            let entry = KeyEntry {
                port,
                holder: String::new(),
            };
            match self.link_key(key, &entry) {
                Ok(()) => return Some(port),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if self.read_key(key).is_some_and(|found| found.port == port) {
                        return Some(port);
                    }
                    self.remove_file(port);
                    if let Some(winner) = self.lookup(key) {
                        return Some(winner.port);
                    }
                    self.drop_stale_key(key);
                }
                Err(error) => {
                    eprintln!("ports: key for {port}: {error}");
                    self.remove_file(port);
                    return None;
                }
            }
        }
        None
    }

    fn claim_random(&self, lease: &mut Lease) -> Option<u16> {
        (0..CLAIM_ATTEMPTS).find_map(|_| {
            let port = self.probe.candidate();
            (port >= PORT_LOW)
                .then(|| self.try_claim(lease, port))
                .flatten()
        })
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
        if port == 0 || !self.probe.bindable(port) {
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

    fn key_path(&self, key: &str) -> PathBuf {
        self.keys.join(key_file_name(&self.session, key))
    }

    fn staged_path(&self) -> PathBuf {
        self.keys.join(format!(
            ".staged-{}-{}",
            std::process::id(),
            STAGED.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn read_key(&self, key: &str) -> Option<KeyEntry> {
        let text = std::fs::read_to_string(self.key_path(key)).ok()?;
        parse_key_entry(&text, &self.session, key)
    }

    fn stage_key(&self, key: &str, entry: &KeyEntry) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.keys)?;
        let staged = self.staged_path();
        std::fs::write(&staged, format_key_entry(&self.session, key, entry))?;
        Ok(staged)
    }

    /// Puts the key file in place only if none exists: a hard link is an atomic create-with-content.
    fn link_key(&self, key: &str, entry: &KeyEntry) -> std::io::Result<()> {
        let staged = self.stage_key(key, entry)?;
        let linked = std::fs::hard_link(&staged, self.key_path(key));
        remove_quietly(&staged);
        linked
    }

    fn replace_key(&self, key: &str, entry: &KeyEntry) {
        let replaced = self
            .stage_key(key, entry)
            .and_then(|staged| std::fs::rename(&staged, self.key_path(key)));
        if let Err(error) = replaced {
            eprintln!("ports: key for {}: {error}", entry.port);
        }
    }

    /// Removes a key file whose lease is gone, without removing one another process just put back.
    fn drop_stale_key(&self, key: &str) {
        let path = self.key_path(key);
        let Ok(seen) = std::fs::read_to_string(&path) else {
            return;
        };
        let aside = self.staged_path();
        if std::fs::rename(&path, &aside).is_err() {
            return;
        }
        if std::fs::read_to_string(&aside).ok().as_deref() != Some(seen.as_str()) {
            if let Err(error) = std::fs::hard_link(&aside, &path) {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    eprintln!("ports: restoring a key file: {error}");
                }
            }
        }
        remove_quietly(&aside);
    }

    fn read_lease(&self, port: u16) -> Option<Lease> {
        let text = std::fs::read_to_string(self.dir.join(port.to_string())).ok()?;
        parse_lease(port, &text)
    }

    /// The lease the key file names, while it is still this key's.
    fn lookup(&self, key: &str) -> Option<Lease> {
        let entry = self.read_key(key)?;
        self.read_lease(entry.port)
            .filter(|lease| lease.session == self.session && lease.key == key)
    }

    fn session_leases(&self) -> Vec<Lease> {
        scan_leases(&self.dir)
            .into_iter()
            .filter(|lease| lease.session == self.session)
            .collect()
    }

    /// One lease per key. Duplicates (from older versions, or a process that crashed mid-claim) lose to
    /// the lease the key file names, unless only another one is listening.
    fn repair(&self) {
        let mut by_key: HashMap<String, Vec<Lease>> = HashMap::new();
        for lease in self.session_leases() {
            by_key.entry(lease.key.clone()).or_default().push(lease);
        }
        for (key, leases) in by_key {
            let named = self.read_key(&key);
            let Some(keep) = self.pick(&leases, named.as_ref().map(|entry| entry.port)) else {
                continue;
            };
            for lease in leases.iter().filter(|lease| lease.port != keep) {
                self.remove_file(lease.port);
            }
            if named.as_ref().map(|entry| entry.port) != Some(keep) {
                let holder = named.map(|entry| entry.holder).unwrap_or_default();
                self.replace_key(&key, &KeyEntry { port: keep, holder });
            }
        }
    }

    fn pick(&self, leases: &[Lease], named: Option<u16>) -> Option<u16> {
        if let [only] = leases {
            return Some(only.port);
        }
        let up: Vec<u16> = leases
            .iter()
            .map(|lease| lease.port)
            .filter(|port| self.probe.is_up(*port))
            .collect();
        if let Some(port) = named.filter(|port| leases.iter().any(|lease| lease.port == *port)) {
            if up.is_empty() || up.contains(&port) {
                return Some(port);
            }
        }
        up.first()
            .copied()
            .or_else(|| preferred_lease(leases).map(|lease| lease.port))
    }

    pub fn mark(&self, key: &str, state: PortState) {
        let Some(mut lease) = self.lookup(key) else {
            return;
        };
        if let Ok(mut tracked) = self.tracked.lock() {
            if state == PortState::Running {
                tracked.entry(key.to_string()).or_default().seen_up = true;
            }
        }
        if lease.state == state {
            return;
        }
        lease.state = state;
        if state == PortState::Starting {
            lease.since_ms = self.probe.now_ms();
        }
        self.write(&lease);
    }

    /// Ties a lease to the holder running its service: a slow build keeps its port while the holder lives,
    /// whichever process looks.
    pub fn set_holder(&self, key: &str, holder: &str) {
        if let Ok(mut tracked) = self.tracked.lock() {
            tracked.entry(key.to_string()).or_default().holder = Some(holder.to_string());
        }
        if let Some(mut entry) = self.read_key(key) {
            if entry.holder != holder {
                entry.holder = holder.to_string();
                self.replace_key(key, &entry);
            }
        }
    }

    pub fn release(&self, key: &str) {
        if let Ok(mut tracked) = self.tracked.lock() {
            tracked.remove(key);
        }
        if let Some(lease) = self.lookup(key) {
            self.remove_file(lease.port);
        }
        remove_quietly(&self.key_path(key));
    }

    /// Drops every lease of a workspace (it was deleted), whichever process took them.
    pub fn release_workspace(&self, ws_key: &str) {
        let prefix = format!("{ws_key}\u{1f}");
        let keys: HashSet<String> = self
            .session_leases()
            .into_iter()
            .map(|lease| lease.key)
            .filter(|key| key.starts_with(&prefix))
            .collect();
        for key in keys {
            for lease in self
                .session_leases()
                .iter()
                .filter(|lease| lease.key == key)
            {
                self.remove_file(lease.port);
            }
            if let Ok(mut tracked) = self.tracked.lock() {
                tracked.remove(&key);
            }
            remove_quietly(&self.key_path(&key));
        }
    }

    pub fn bindable(&self, port: u16) -> bool {
        self.probe.bindable(port)
    }

    pub fn port_of(&self, key: &str) -> Option<u16> {
        self.lookup(key).map(|lease| lease.port)
    }

    pub fn leases(&self) -> Vec<Lease> {
        self.session_leases()
    }

    /// Probes every lease of the session, whichever process took it: reachable ones become `running`; one
    /// whose holder lives is kept (a long build or rebuild); otherwise one that was up and stayed down past
    /// the grace, one that never came up within the assign grace, or one never started at all past its
    /// expiry is reclaimed.
    pub fn reap(&self) {
        self.repair();
        let leases = self.session_leases();
        let now = self.probe.now_ms();
        let Ok(mut tracked) = self.tracked.lock() else {
            return;
        };
        let present: HashSet<&str> = leases.iter().map(|lease| lease.key.as_str()).collect();
        tracked.retain(|key, _| present.contains(key.as_str()));
        let mut dead = Vec::new();
        for lease in &leases {
            let entry = tracked.entry(lease.key.clone()).or_default();
            if self.probe.is_up(lease.port) {
                entry.down_since_ms = None;
                entry.seen_up = true;
                if lease.state != PortState::Running {
                    self.write(&Lease {
                        state: PortState::Running,
                        ..lease.clone()
                    });
                }
                continue;
            }
            let holder = entry.holder.clone().or_else(|| {
                self.read_key(&lease.key)
                    .map(|key| key.holder)
                    .filter(|holder| !holder.is_empty())
            });
            if holder.is_some_and(|holder| self.probe.holder_alive(&holder)) {
                entry.down_since_ms = None;
                continue;
            }
            let age = now.saturating_sub(lease.since_ms);
            let expired = if entry.seen_up || lease.state == PortState::Running {
                let since = *entry.down_since_ms.get_or_insert(now);
                now.saturating_sub(since) > REAP_DOWN_GRACE.as_millis() as u64
            } else if lease.state == PortState::Starting {
                age > ASSIGN_GRACE.as_millis() as u64
            } else {
                age > ASSIGNED_EXPIRY.as_millis() as u64
            };
            if expired {
                dead.push(lease.clone());
            }
        }
        for lease in dead {
            tracked.remove(&lease.key);
            self.remove_file(lease.port);
            if self
                .read_key(&lease.key)
                .is_some_and(|entry| entry.port == lease.port)
            {
                remove_quietly(&self.key_path(&lease.key));
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
        remove_quietly(&self.dir.join(port.to_string()));
    }
}

fn remove_quietly(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("ports: {}: {error}", path.display());
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
        // A shared service keeps its well-known port even below the random range.
        assert_eq!(
            manager.acquire_preferred(&shared_key("low", 0), 5432, 3),
            Some(5432)
        );
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

    fn lease_files_for(fixture: &Fixture, key: &str) -> Vec<Lease> {
        scan_leases(&fixture.state.path("ports.d"))
            .into_iter()
            .filter(|lease| lease.key == key)
            .collect()
    }

    fn write_lease(fixture: &Fixture, lease: &Lease) {
        let dir = fixture.state.path("ports.d");
        std::fs::create_dir_all(&dir).expect("ports.d");
        std::fs::write(dir.join(lease.port.to_string()), format_lease(lease)).expect("lease");
    }

    #[test]
    fn two_processes_of_one_session_agree_on_a_services_port() {
        let fixture = Fixture::new();
        let app = fixture.manager("demo");
        let agent = fixture.manager("demo");
        let key = service_key("ws-feat-login", "web~client");
        let port = app.acquire(&key).expect("port");
        assert_eq!(
            agent.acquire(&key),
            Some(port),
            "a process opened before the lease existed must find it, not take a second port"
        );
        assert_eq!(agent.port_of(&key), Some(port));
        assert_eq!(lease_files_for(&fixture, &key).len(), 1);
    }

    #[test]
    fn racing_processes_leave_one_lease_per_service() {
        let fixture = Fixture::new();
        let app = Arc::new(fixture.manager("demo"));
        let agent = Arc::new(fixture.manager("demo"));
        let keys: Vec<String> = (0..20)
            .map(|index| service_key("ws-feat-login", &format!("web~svc{index}")))
            .collect();
        let handles: Vec<_> = keys
            .iter()
            .flat_map(|key| [(app.clone(), key.clone()), (agent.clone(), key.clone())])
            .map(|(manager, key)| std::thread::spawn(move || (key.clone(), manager.acquire(&key))))
            .collect();
        let mut answers: HashMap<String, HashSet<u16>> = HashMap::new();
        for handle in handles {
            if let Ok((key, Some(port))) = handle.join() {
                answers.entry(key).or_default().insert(port);
            }
        }
        for key in &keys {
            assert_eq!(
                answers.get(key).map(HashSet::len),
                Some(1),
                "both processes got the same port for {key}"
            );
            assert_eq!(
                lease_files_for(&fixture, key).len(),
                1,
                "one lease for {key}"
            );
        }
    }

    #[test]
    fn duplicate_leases_on_disk_collapse_to_the_running_one() {
        let fixture = Fixture::new();
        let key = service_key("ws-feat-login", "web~client");
        let stray = Lease {
            key: key.clone(),
            port: 50000,
            session: "demo".into(),
            state: PortState::Assigned,
            since_ms: 0,
        };
        let running = Lease {
            port: 30000,
            state: PortState::Running,
            ..stray.clone()
        };
        write_lease(&fixture, &stray);
        write_lease(&fixture, &running);
        let manager = fixture.manager("demo");
        assert_eq!(
            manager.port_of(&key),
            Some(30000),
            "the lease the service runs on wins, not the highest port"
        );
        assert_eq!(
            lease_files_for(&fixture, &key)
                .iter()
                .map(|lease| lease.port)
                .collect::<Vec<_>>(),
            vec![30000],
            "the stray lease is removed"
        );
    }

    #[test]
    fn a_lease_another_process_released_is_never_handed_out_twice() {
        let fixture = Fixture::new();
        let app = fixture.manager("demo");
        let key = service_key("ws-feat-login", "web~client");
        let port = app.acquire(&key).expect("port");
        // The CLI deletes the workspace; the app still remembers the port.
        fixture.manager("demo").release_workspace("ws-feat-login");
        if let Ok(mut sequence) = fixture.probe.sequence.lock() {
            sequence.push(port);
        }
        let other = fixture
            .manager("other")
            .acquire(&service_key("ws-main", "api~server"))
            .expect("port");
        assert_eq!(other, port, "the freed port was taken by another service");
        assert_ne!(
            app.acquire(&key),
            Some(other),
            "the app must not keep handing out a port whose lease is gone"
        );
    }

    #[test]
    fn an_assigned_lease_nobody_started_is_reclaimed_eventually() {
        let fixture = Fixture::new();
        let key = service_key("ws-feat-login", "web~client");
        let port = fixture.manager("demo").acquire(&key).expect("port");
        let app = fixture.manager("demo");
        fixture.advance(Duration::from_secs(8 * 24 * 60 * 60));
        app.reap();
        assert!(
            !fixture.state.path(format!("ports.d/{port}")).exists(),
            "an assigned lease without a holder must not live forever"
        );
    }

    #[test]
    fn the_reaper_also_sees_leases_written_after_it_opened() {
        let fixture = Fixture::new();
        let app = fixture.manager("demo");
        let key = service_key("ws-feat-login", "web~client");
        let agent = fixture.manager("demo");
        let port = agent.acquire(&key).expect("port");
        agent.mark(&key, PortState::Starting);
        drop(agent);
        fixture.advance(ASSIGN_GRACE + Duration::from_secs(1));
        app.reap();
        assert!(
            !fixture.state.path(format!("ports.d/{port}")).exists(),
            "a dead lease another process left behind is reclaimed"
        );
    }

    #[test]
    fn a_running_service_that_rebuilds_keeps_its_port_while_its_holder_lives() {
        let fixture = Fixture::new();
        let manager = fixture.manager("demo");
        let key = service_key("ws-feat-login", "web~client");
        let holder = "svc-demo-feat-login-web-client";
        let port = manager.acquire(&key).expect("port");
        manager.set_holder(&key, holder);
        if let Ok(mut alive) = fixture.probe.alive.lock() {
            alive.insert(holder.into());
        }
        if let Ok(mut up) = fixture.probe.up.lock() {
            up.insert(port);
        }
        manager.reap();
        if let Ok(mut up) = fixture.probe.up.lock() {
            up.clear();
        }
        manager.reap();
        fixture.advance(REAP_DOWN_GRACE + Duration::from_secs(10));
        manager.reap();
        assert_eq!(
            manager.port_of(&key),
            Some(port),
            "a long rebuild must not cost the service its port"
        );
    }

    #[test]
    fn an_ipv6_only_listener_is_busy_and_up() -> std::io::Result<()> {
        let listener = TcpListener::bind("[::1]:0")?;
        let port = listener.local_addr()?.port();
        let probe = SystemProbe::new(Box::new(|_| false));
        assert!(probe.is_up(port), "a dev server on ::1 is up");
        assert!(!probe.bindable(port), "a port held on ::1 is not free");
        Ok(())
    }

    #[test]
    fn system_probe_sees_a_real_listener() -> std::io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
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
