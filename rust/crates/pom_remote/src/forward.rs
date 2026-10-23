//! Ports of a remote host opened on this machine's loopback with `ssh -L`: on the shared ssh connection when one
//! runs (`ssh -O forward` / `-O cancel`), else on a dedicated `ssh -N`. Only 127.0.0.1 is ever bound here, and
//! nothing opens a port on the host. Forwards live as long as their [`Forwarder`]; the app keeps one per remote
//! window, and records what it forwards in a small file `pom host ports` reads.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{Host, Ssh};

pub const PORTS_SCHEMA: &str = "pom.ports/v1";
/// A port taken here moves to the next free one within this many.
const SPARE_PORTS: u16 = 20;
const TUNNEL_START: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    /// The host's dev proxy, which this machine's dev proxy sends that host's workspace addresses to.
    Gateway,
    /// A shared service (Postgres, Redis, MinIO...) of the open workspace.
    Shared,
    /// A port someone asked for.
    #[default]
    Manual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardState {
    Starting,
    #[default]
    Forwarded,
    /// The connection dropped; it is forwarded again once the host is back.
    Lost,
    Failed,
}

/// Which port here a forward takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalPort {
    /// The host's port number when free here, else the next free one.
    Same,
    /// Exactly this one, or fail.
    Exact(u16),
    /// Any free port.
    Any,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PortForward {
    pub remote_port: u16,
    /// 0 until forwarded.
    pub local_port: u16,
    /// The port asked for here; differs from `local_port` when that one was taken.
    pub wanted_port: u16,
    pub label: String,
    pub kind: ForwardKind,
    pub state: ForwardState,
    /// Why it failed, or that it moved to another port.
    pub message: String,
    /// `shared` (the shared ssh connection) or `tunnel` (an ssh of its own).
    pub via: String,
}

impl PortForward {
    pub fn moved(&self) -> bool {
        self.local_port != 0 && self.wanted_port != 0 && self.local_port != self.wanted_port
    }

    pub fn address(&self) -> String {
        format!("127.0.0.1:{}", self.local_port)
    }
}

/// A forward to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardRequest {
    pub remote_port: u16,
    pub local: LocalPort,
    pub label: String,
    pub kind: ForwardKind,
}

/// What `pom host ports` reads: one file per app window or `pom -H <host> forward`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PortsRecord {
    pub schema: String,
    pub host: String,
    /// The remote project the window shows; empty for the CLI.
    pub project: String,
    pub pid: u32,
    pub forwards: Vec<PortForward>,
}

/// `ssh -L` takes this: loopback on both ends.
pub fn forward_spec(local_port: u16, remote_port: u16) -> String {
    format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}")
}

/// Nothing listens on the loopback port here and it can be bound.
pub fn port_free(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    let answers = [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ]
    .iter()
    .any(|address| TcpStream::connect_timeout(address, PROBE_TIMEOUT).is_ok());
    !answers && port_bindable(port)
}

/// The port can be bound on 127.0.0.1; false while a forward holds it. The probe never listens: a copy of it
/// that a process spawned meanwhile holds until its exec must not keep ssh (SO_REUSEADDR) from binding the port.
fn port_bindable(port: u16) -> bool {
    // SAFETY: a socket this function owns, given only valid pointers and lengths, and closed before it returns.
    unsafe {
        #[cfg(target_os = "linux")]
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        #[cfg(not(target_os = "linux"))]
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return false;
        }
        #[cfg(not(target_os = "linux"))]
        if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) != 0 {
            libc::close(fd);
            return false;
        }
        let reuse: libc::c_int = 1;
        let reusable = libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            (&reuse as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        ) == 0;
        let mut address: libc::sockaddr_in = std::mem::zeroed();
        #[cfg(not(target_os = "linux"))]
        {
            address.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
        }
        address.sin_family = libc::AF_INET as libc::sa_family_t;
        address.sin_port = port.to_be();
        address.sin_addr.s_addr = u32::from(Ipv4Addr::LOCALHOST).to_be();
        let bound = reusable
            && libc::bind(
                fd,
                (&address as *const libc::sockaddr_in).cast(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            ) == 0;
        libc::close(fd);
        bound
    }
}

fn any_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("no free port on this machine: {error}"))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| error.to_string())
}

/// The port a request takes here.
pub fn choose_port(remote_port: u16, local: LocalPort) -> Result<u16, String> {
    match local {
        LocalPort::Exact(port) if port_free(port) => Ok(port),
        LocalPort::Exact(port) => Err(format!("port {port} is in use on this machine")),
        LocalPort::Any => any_free_port(),
        LocalPort::Same => (0..=SPARE_PORTS)
            .filter_map(|step| remote_port.checked_add(step))
            .find(|port| port_free(*port))
            .map_or_else(any_free_port, Ok),
    }
}

/// Where the app and `pom -H <host> forward` record their forwards to `host`.
pub fn ports_dir(state: &pom_paths::StateDir, host: &str) -> PathBuf {
    state.path("forwards").join(pom_env::branch_safe(host))
}

/// The process still runs (a record it left behind when it crashed does not count).
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists and may be signalled.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// What the running app windows and `pom -H <host> forward` commands forward from `host`.
pub fn recorded_ports(state: &pom_paths::StateDir, host: &str) -> Vec<PortsRecord> {
    let Ok(entries) = std::fs::read_dir(ports_dir(state, host)) else {
        return Vec::new();
    };
    let mut records: Vec<PortsRecord> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path()).ok()?;
            serde_json::from_str::<PortsRecord>(&text).ok()
        })
        .filter(|record| record.schema == PORTS_SCHEMA && alive(record.pid))
        .collect();
    records.sort_by(|left, right| left.project.cmp(&right.project));
    records
}

enum Carrier {
    Shared,
    Tunnel(Child),
}

struct Entry {
    forward: PortForward,
    carrier: Option<Carrier>,
}

struct Record {
    path: PathBuf,
    project: String,
}

/// The forwards from one host, closed when it drops.
pub struct Forwarder {
    ssh: Ssh,
    host: Host,
    entries: Mutex<Vec<Entry>>,
    record: Option<Record>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Forwarder {
    pub fn new(ssh: Ssh, host: Host) -> Forwarder {
        Forwarder {
            ssh,
            host,
            entries: Mutex::new(Vec::new()),
            record: None,
        }
    }

    /// Keeps `<dir>/<name>.json` up to date for `pom host ports`, and removes it on close.
    pub fn recorded_in(mut self, dir: &Path, name: &str, project: &str) -> Forwarder {
        self.record = Some(Record {
            path: dir.join(format!("{}.json", pom_env::branch_safe(name))),
            project: project.to_string(),
        });
        self
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn forwards(&self) -> Vec<PortForward> {
        locked(&self.entries)
            .iter()
            .map(|entry| entry.forward.clone())
            .collect()
    }

    /// The port here a forwarded remote port answers on.
    pub fn local_port(&self, remote_port: u16) -> Option<u16> {
        locked(&self.entries)
            .iter()
            .find(|entry| {
                entry.forward.remote_port == remote_port
                    && matches!(
                        entry.forward.state,
                        ForwardState::Forwarded | ForwardState::Lost
                    )
            })
            .map(|entry| entry.forward.local_port)
    }

    /// Forwards `request.remote_port` here; a port already forwarded comes back as it is. Runs ssh, so keep it
    /// off the UI thread.
    pub fn forward(&self, request: &ForwardRequest) -> Result<PortForward, String> {
        let wanted = match request.local {
            LocalPort::Same => request.remote_port,
            LocalPort::Exact(port) => port,
            LocalPort::Any => 0,
        };
        {
            let mut entries = locked(&self.entries);
            if let Some(entry) = entries
                .iter()
                .find(|entry| entry.forward.remote_port == request.remote_port)
            {
                if entry.forward.state != ForwardState::Failed {
                    return Ok(entry.forward.clone());
                }
            }
            entries.retain(|entry| entry.forward.remote_port != request.remote_port);
            entries.push(Entry {
                forward: PortForward {
                    remote_port: request.remote_port,
                    wanted_port: wanted,
                    label: request.label.clone(),
                    kind: request.kind,
                    state: ForwardState::Starting,
                    ..PortForward::default()
                },
                carrier: None,
            });
        }
        let opened = choose_port(request.remote_port, request.local)
            .and_then(|local| Ok((local, self.open(local, request.remote_port)?)));
        let result = {
            let mut entries = locked(&self.entries);
            let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.forward.remote_port == request.remote_port)
            else {
                if let Ok((local, carrier)) = opened {
                    self.close_carrier(local, request.remote_port, carrier);
                }
                return Err(format!("forwarding {} was stopped", request.remote_port));
            };
            match opened {
                Ok((local, carrier)) => {
                    entry.forward.local_port = local;
                    entry.forward.state = ForwardState::Forwarded;
                    entry.forward.via = carrier_name(&carrier).to_string();
                    entry.forward.message = if entry.forward.moved() {
                        format!("port {wanted} is in use on this machine, so it is on {local}")
                    } else {
                        String::new()
                    };
                    entry.carrier = Some(carrier);
                    Ok(entry.forward.clone())
                }
                Err(message) => {
                    entry.forward.state = ForwardState::Failed;
                    entry.forward.message = message.clone();
                    Err(message)
                }
            }
        };
        self.save();
        result
    }

    fn open(&self, local: u16, remote: u16) -> Result<Carrier, String> {
        let spec = forward_spec(local, remote);
        if self.ssh.master_running(&self.host) {
            self.ssh.master_forward(&self.host, "forward", &spec)?;
            return Ok(Carrier::Shared);
        }
        let mut child = self
            .ssh
            .tunnel(&self.host, &spec)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not run ssh: {error}"))?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut said = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        if let Err(error) = std::io::Read::read_to_string(&mut stderr, &mut said) {
                            said = error.to_string();
                        }
                    }
                    let reason = said
                        .lines()
                        .rfind(|line| !line.trim().is_empty())
                        .map(str::trim)
                        .unwrap_or("no message from ssh")
                        .to_string();
                    return Err(format!("ssh -L {spec} exited ({status}): {reason}"));
                }
                Ok(None) if !port_bindable(local) => return Ok(Carrier::Tunnel(child)),
                Ok(None) if started.elapsed() >= TUNNEL_START => {
                    stop_child(&mut child);
                    return Err(format!(
                        "ssh -L {spec} did not listen within {}s",
                        TUNNEL_START.as_secs()
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => return Err(format!("ssh -L {spec}: {error}")),
            }
        }
    }

    fn close_carrier(&self, local: u16, remote: u16, carrier: Carrier) {
        match carrier {
            Carrier::Shared => {
                if let Err(error) =
                    self.ssh
                        .master_forward(&self.host, "cancel", &forward_spec(local, remote))
                {
                    eprintln!("{}: {error}", self.host.name);
                }
            }
            Carrier::Tunnel(mut child) => stop_child(&mut child),
        }
    }

    /// Stops forwarding `remote_port`.
    pub fn stop(&self, remote_port: u16) {
        let removed: Vec<Entry> = {
            let mut entries = locked(&self.entries);
            let (removed, kept) = std::mem::take(&mut *entries)
                .into_iter()
                .partition(|entry| entry.forward.remote_port == remote_port);
            *entries = kept;
            removed
        };
        for entry in removed {
            if let Some(carrier) = entry.carrier {
                self.close_carrier(entry.forward.local_port, remote_port, carrier);
            }
        }
        self.save();
    }

    /// The connection dropped: every forward waits for it to come back.
    pub fn mark_lost(&self) {
        let mut changed = false;
        for entry in locked(&self.entries).iter_mut() {
            if entry.forward.state == ForwardState::Forwarded {
                entry.forward.state = ForwardState::Lost;
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    /// Opens again every forward whose port here no longer holds (after a reconnect, or a tunnel that exited),
    /// on the same port; returns how many it opened.
    pub fn reapply(&self) -> usize {
        let wanted: Vec<(u16, u16)> = {
            let mut entries = locked(&self.entries);
            entries
                .iter_mut()
                .filter(|entry| {
                    matches!(
                        entry.forward.state,
                        ForwardState::Forwarded | ForwardState::Lost
                    )
                })
                .filter_map(|entry| {
                    let tunnel_exited = match &mut entry.carrier {
                        Some(Carrier::Tunnel(child)) => !matches!(child.try_wait(), Ok(None)),
                        _ => false,
                    };
                    let held = !tunnel_exited && !port_bindable(entry.forward.local_port);
                    if held {
                        entry.forward.state = ForwardState::Forwarded;
                        return None;
                    }
                    Some((entry.forward.remote_port, entry.forward.local_port))
                })
                .collect()
        };
        let mut opened = 0;
        for (remote, local) in wanted {
            let result = self.open(local, remote);
            let mut entries = locked(&self.entries);
            let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.forward.remote_port == remote)
            else {
                drop(entries);
                if let Ok(carrier) = result {
                    self.close_carrier(local, remote, carrier);
                }
                continue;
            };
            if let Some(Carrier::Tunnel(mut child)) = entry.carrier.take() {
                stop_child(&mut child);
            }
            match result {
                Ok(carrier) => {
                    entry.forward.via = carrier_name(&carrier).to_string();
                    entry.forward.state = ForwardState::Forwarded;
                    entry.carrier = Some(carrier);
                    opened += 1;
                }
                Err(message) => {
                    entry.forward.state = ForwardState::Lost;
                    entry.forward.message = message;
                }
            }
        }
        self.save();
        opened
    }

    /// Stops every forward and removes the record.
    pub fn close(&self) {
        let entries = std::mem::take(&mut *locked(&self.entries));
        for entry in entries {
            if let Some(carrier) = entry.carrier {
                self.close_carrier(entry.forward.local_port, entry.forward.remote_port, carrier);
            }
        }
        if let Some(record) = &self.record {
            if let Err(error) = std::fs::remove_file(&record.path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("{}: {error}", record.path.display());
                }
            }
        }
    }

    fn save(&self) {
        let Some(record) = &self.record else {
            return;
        };
        let document = PortsRecord {
            schema: PORTS_SCHEMA.to_string(),
            host: self.host.name.clone(),
            project: record.project.clone(),
            pid: std::process::id(),
            forwards: self.forwards(),
        };
        let written = serde_json::to_vec_pretty(&document)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                pom_paths::write_atomic(&record.path, &bytes, 0o644)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = written {
            eprintln!("{}: {error}", record.path.display());
        }
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        self.close();
    }
}

fn carrier_name(carrier: &Carrier) -> &'static str {
    match carrier {
        Carrier::Shared => "shared",
        Carrier::Tunnel(_) => "tunnel",
    }
}

fn stop_child(child: &mut Child) {
    if let Err(error) = child.kill() {
        if error.kind() != std::io::ErrorKind::InvalidInput {
            eprintln!("ssh tunnel: {error}");
        }
    }
    if let Err(error) = child.wait() {
        eprintln!("ssh tunnel: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostUrl;

    const FAKE_SSH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-ssh.sh");

    struct Fixture {
        _temp: tempfile::TempDir,
        log: PathBuf,
        program: PathBuf,
        control: PathBuf,
        records: PathBuf,
    }

    impl Fixture {
        fn new() -> Fixture {
            let temp = tempfile::Builder::new()
                .prefix("pomf")
                .tempdir_in("/tmp")
                .expect("temp");
            Fixture {
                log: temp.path().join("ssh.log"),
                control: temp.path().join("control"),
                records: temp.path().join("records"),
                program: PathBuf::from(FAKE_SSH),
                _temp: temp,
            }
        }

        fn forwarder(&self, master: bool) -> Forwarder {
            let ssh = Ssh::custom(
                self.program.clone(),
                Some(self.control.clone()),
                vec![
                    ("FAKE_SSH_LOG".into(), self.log.to_string_lossy().into()),
                    (
                        "FAKE_MASTER".into(),
                        if master { "0" } else { "255" }.into(),
                    ),
                ],
            );
            let host = Host {
                name: "dev".into(),
                url: HostUrl::parse("ssh://dev@dev.example.net:2222").expect("url"),
                pom_path: None,
                transport: None,
            };
            Forwarder::new(ssh, host).recorded_in(&self.records, "myproject", "myproject")
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(|line| line.trim().to_string())
                .collect()
        }

        fn calls_with(&self, verb: &str) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter(|line| line.starts_with(&format!("-O {verb} ")))
                .collect()
        }
    }

    fn request(remote_port: u16, local: LocalPort) -> ForwardRequest {
        ForwardRequest {
            remote_port,
            local,
            label: "postgres".into(),
            kind: ForwardKind::Shared,
        }
    }

    fn free_run(start: u16, length: u16) -> u16 {
        (start..start + 1500)
            .step_by(37)
            .find(|start| (0..length).all(|step| port_free(start + step)))
            .expect("free ports")
    }

    #[test]
    fn a_forward_rides_the_shared_connection_and_is_cancelled_on_close() {
        let fixture = Fixture::new();
        let port = free_run(25000, 1);
        let forwarder = fixture.forwarder(true);
        let forward = forwarder
            .forward(&request(port, LocalPort::Same))
            .expect("forward");
        assert_eq!(forward.local_port, port);
        assert_eq!(forward.state, ForwardState::Forwarded);
        assert_eq!(forward.via, "shared");
        assert!(!forward.moved());
        let spec = forward_spec(port, port);
        let forwards = fixture.calls_with("forward");
        assert_eq!(forwards.len(), 1);
        let control = format!("ControlPath={}/%C", fixture.control.display());
        assert!(forwards[0].contains(&control), "{}", forwards[0]);
        assert!(
            forwards[0].ends_with(&format!("-L {spec} -p 2222 -- dev@dev.example.net")),
            "{}",
            forwards[0]
        );
        let record = fixture.records.join("myproject.json");
        let saved: PortsRecord =
            serde_json::from_str(&std::fs::read_to_string(&record).expect("record")).expect("json");
        assert_eq!(saved.schema, PORTS_SCHEMA);
        assert_eq!(saved.forwards, vec![forward.clone()]);

        assert_eq!(
            forwarder
                .forward(&request(port, LocalPort::Same))
                .expect("again"),
            forward
        );
        assert_eq!(fixture.calls_with("forward").len(), 1);

        drop(forwarder);
        let cancels = fixture.calls_with("cancel");
        assert_eq!(cancels.len(), 1);
        assert!(
            cancels[0].contains(&format!("-L {spec} ")),
            "{}",
            cancels[0]
        );
        assert!(!record.exists());
    }

    #[test]
    fn a_taken_port_moves_to_the_next_free_one_and_an_exact_one_fails() {
        let fixture = Fixture::new();
        let port = free_run(27000, 3);
        let _taken = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).expect("take");
        let forwarder = fixture.forwarder(true);
        let forward = forwarder
            .forward(&request(port, LocalPort::Same))
            .expect("forward");
        assert_eq!(forward.local_port, port + 1);
        assert_eq!(forward.wanted_port, port);
        assert!(forward.moved());
        assert!(forward.message.contains(&format!("{}", port + 1)));
        assert!(fixture.calls_with("forward")[0].contains(&forward_spec(port + 1, port)));

        let error = forwarder
            .forward(&request(port + 2, LocalPort::Exact(port)))
            .expect_err("taken");
        assert!(error.contains("in use"), "{error}");
        let failed = forwarder.forwards();
        assert_eq!(failed[1].state, ForwardState::Failed);
        assert_eq!(forwarder.local_port(port + 2), None);
        assert_eq!(forwarder.local_port(port), Some(port + 1));
    }

    #[test]
    fn forwards_are_opened_again_after_a_reconnect_and_one_stops_alone() {
        let fixture = Fixture::new();
        let ports = free_run(29000, 2);
        let forwarder = fixture.forwarder(true);
        for port in [ports, ports + 1] {
            forwarder
                .forward(&request(port, LocalPort::Same))
                .expect("forward");
        }
        forwarder.mark_lost();
        assert!(forwarder
            .forwards()
            .iter()
            .all(|forward| forward.state == ForwardState::Lost));
        assert_eq!(forwarder.reapply(), 2);
        assert_eq!(fixture.calls_with("forward").len(), 4);
        assert!(forwarder
            .forwards()
            .iter()
            .all(|forward| forward.state == ForwardState::Forwarded));

        forwarder.stop(ports);
        assert_eq!(fixture.calls_with("cancel").len(), 1);
        assert_eq!(forwarder.forwards().len(), 1);
    }

    #[test]
    fn without_a_shared_connection_a_dedicated_ssh_holds_the_port() {
        let fixture = Fixture::new();
        let port = free_run(31000, 1);
        let forwarder = fixture.forwarder(false);
        let forward = forwarder
            .forward(&request(port, LocalPort::Same))
            .expect("forward");
        assert_eq!(forward.via, "tunnel");
        assert!(!port_bindable(port));
        let tunnel = fixture
            .calls()
            .into_iter()
            .find(|line| line.starts_with("-N "))
            .expect("tunnel call");
        assert!(tunnel.contains("ControlPath=none"), "{tunnel}");
        assert!(tunnel.contains("ExitOnForwardFailure=yes"), "{tunnel}");
        assert!(tunnel.contains(&format!("-L {}", forward_spec(port, port))));
        assert_eq!(forwarder.reapply(), 0);
        forwarder.close();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !port_bindable(port) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(port_bindable(port));
        assert!(fixture.calls_with("cancel").is_empty());
    }

    #[test]
    fn records_of_gone_processes_are_not_listed() {
        let temp = tempfile::tempdir().expect("temp");
        let state = pom_paths::StateDir::new(temp.path());
        let dir = ports_dir(&state, "dev");
        std::fs::create_dir_all(&dir).expect("dir");
        let record = |pid: u32, project: &str| PortsRecord {
            schema: PORTS_SCHEMA.into(),
            host: "dev".into(),
            project: project.into(),
            pid,
            forwards: vec![PortForward {
                remote_port: 5432,
                local_port: 5432,
                ..PortForward::default()
            }],
        };
        for (name, entry) in [
            ("live", record(std::process::id(), "live")),
            ("gone", record(i32::MAX as u32, "gone")),
        ] {
            std::fs::write(
                dir.join(format!("{name}.json")),
                serde_json::to_vec(&entry).expect("json"),
            )
            .expect("write");
        }
        let listed = recorded_ports(&state, "dev");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].project, "live");
        assert!(recorded_ports(&state, "other").is_empty());
    }

    /// Needs an sshd that takes `ssh localhost` without a prompt (the Linux CI job starts one).
    #[test]
    fn a_real_sshd_forward_reaches_a_port_on_the_host() {
        if std::env::var_os("POM_REAL_SSH").is_none() {
            return;
        }
        let host = Host {
            name: "localhost".into(),
            url: HostUrl::parse("ssh://localhost").expect("url"),
            pom_path: None,
            transport: None,
        };
        let ssh = Ssh::system();
        let forwarder = Forwarder::new(ssh.clone(), host.clone());
        assert_eq!(round_trip(&forwarder), "tunnel");
        forwarder.close();

        let control = crate::control_dir().expect("control dir");
        let started = std::process::Command::new("ssh")
            .args([
                "-M",
                "-N",
                "-f",
                "-o",
                "BatchMode=yes",
                "-o",
                "ControlPersist=60",
            ])
            .arg("-o")
            .arg(format!("ControlPath={}/%C", control.display()))
            .arg("localhost")
            .status()
            .expect("master");
        assert!(started.success());
        assert!(ssh.master_running(&host));
        let forwarder = Forwarder::new(ssh, host);
        assert_eq!(round_trip(&forwarder), "shared");
        forwarder.close();
        let stopped = std::process::Command::new("ssh")
            .args(["-O", "exit", "-o"])
            .arg(format!("ControlPath={}/%C", control.display()))
            .arg("localhost")
            .status()
            .expect("exit master");
        assert!(stopped.success());
    }

    /// Forwards a port that answers "pong" and reads it through the forward; returns what carried it.
    fn round_trip(forwarder: &Forwarder) -> String {
        use std::io::{Read, Write};
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listen");
        let remote = listener.local_addr().expect("address").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream.write_all(b"pong").expect("write");
        });
        let forward = forwarder
            .forward(&ForwardRequest {
                remote_port: remote,
                local: LocalPort::Any,
                label: "test".into(),
                kind: ForwardKind::Manual,
            })
            .expect("forward");
        let mut stream =
            TcpStream::connect((Ipv4Addr::LOCALHOST, forward.local_port)).expect("connect");
        let mut answer = String::new();
        stream.read_to_string(&mut answer).expect("read");
        assert_eq!(answer, "pong");
        server.join().expect("server");
        forwarder.stop(remote);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !port_bindable(forward.local_port) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(port_bindable(forward.local_port), "the forward is gone");
        forward.via
    }
}
