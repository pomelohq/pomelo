//! What a remote window opens here: the host's dev proxy (so this machine's dev proxy can send that host's
//! workspace addresses to it), the shared services of the open workspace, and ports asked for by hand. One thread
//! runs the ssh calls, so the window never waits on them; forwards stop when this is dropped or closed.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::forward::{ForwardKind, ForwardRequest, Forwarder, LocalPort, PortForward};
use crate::watch::{Connection, Snapshot};
use crate::{Host, ProjectSource, Ssh};

/// How often a forward whose port here went away (an ssh that exited) is opened again.
const REAPPLY_EVERY: Duration = Duration::from_secs(5);
pub const GATEWAY_LABEL: &str = "dev proxy";

/// A shared service of the open workspace as the host reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedPort {
    pub name: String,
    pub port: u16,
    pub ready: bool,
}

/// What the dev proxy here needs to send a remote project's workspace addresses to its host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GatewayRoute {
    pub host: String,
    pub session: String,
    pub branches: Vec<String>,
    /// The port here that forwards to the host's dev proxy.
    pub port: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Look {
    connected: bool,
    session: String,
    branch: String,
    branches: Vec<String>,
    shared: Vec<SharedPort>,
}

impl Look {
    fn of(snapshot: &Snapshot) -> Look {
        let list = snapshot.workspaces.as_ref();
        Look {
            connected: snapshot.connection == Connection::Connected,
            session: list.map(|list| list.project.clone()).unwrap_or_default(),
            branch: snapshot.branch.clone(),
            branches: list
                .map(|list| {
                    list.workspaces
                        .iter()
                        .map(|workspace| workspace.branch.clone())
                        .collect()
                })
                .unwrap_or_default(),
            shared: snapshot
                .status
                .as_ref()
                .map(|status| {
                    status
                        .shared
                        .iter()
                        .filter(|shared| shared.port > 0)
                        .map(|shared| SharedPort {
                            name: shared.name.clone(),
                            port: shared.port,
                            ready: shared.ready,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

enum Command {
    Sync(Look),
    Forward(ForwardRequest),
    Stop(u16),
    Close,
}

#[derive(Default)]
struct State {
    connected: bool,
    session: String,
    branch: String,
    branches: Vec<String>,
    /// Remote ports of shared services already tried, so one that failed is not tried on every look.
    tried: HashSet<u16>,
    /// Whether each forwarded shared service answered at the last look.
    ready: BTreeMap<u16, bool>,
    notices: Vec<String>,
    last_sync: Option<Look>,
}

struct Inner {
    forwarder: Forwarder,
    source: Arc<dyn ProjectSource>,
    proxy_port_here: u16,
    state: Mutex<State>,
    version: AtomicU64,
    waker: Arc<dyn Fn() + Send + Sync>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The forwards of one remote window.
pub struct RemotePorts {
    inner: Arc<Inner>,
    commands: Mutex<Option<mpsc::Sender<Command>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

/// Where a remote window's forwards come from and go.
pub struct PortsSetup {
    pub source: Arc<dyn ProjectSource>,
    pub ssh: Ssh,
    pub host: Host,
    pub project: String,
    /// Where `pom host ports` finds this window's record; none in tests that do not look.
    pub record_dir: Option<std::path::PathBuf>,
    /// This machine's dev proxy port, which workspace addresses use here.
    pub proxy_port_here: u16,
    pub waker: Arc<dyn Fn() + Send + Sync>,
}

impl RemotePorts {
    pub fn start(setup: PortsSetup) -> RemotePorts {
        let mut forwarder = Forwarder::new(setup.ssh, setup.host);
        if let Some(dir) = setup.record_dir.as_deref() {
            forwarder = forwarder.recorded_in(dir, &setup.project, &setup.project);
        }
        let inner = Arc::new(Inner {
            forwarder,
            source: setup.source,
            proxy_port_here: setup.proxy_port_here,
            state: Mutex::new(State::default()),
            version: AtomicU64::new(1),
            waker: setup.waker,
        });
        let (sender, receiver) = mpsc::channel();
        let worker_inner = inner.clone();
        let worker = std::thread::Builder::new()
            .name("pom-ports".into())
            .spawn(move || run(&worker_inner, &receiver));
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                eprintln!("ports: could not start: {error}");
                None
            }
        };
        RemotePorts {
            inner,
            commands: Mutex::new(Some(sender)),
            worker: Mutex::new(worker),
        }
    }

    fn send(&self, command: Command) {
        if let Some(sender) = locked(&self.commands).as_ref() {
            if sender.send(command).is_err() {
                eprintln!("ports: the forwarding thread is gone");
            }
        }
    }

    /// Follows what the window's last look found: the connection, the workspaces, the shared services.
    pub fn sync(&self, snapshot: &Snapshot) {
        let sync = Look::of(snapshot);
        {
            let mut state = locked(&self.inner.state);
            if state.last_sync.as_ref() == Some(&sync) {
                return;
            }
            state.last_sync = Some(sync.clone());
        }
        self.send(Command::Sync(sync));
    }

    /// Forwards a port of the host by hand: its own number here when free, or exactly `local`.
    pub fn forward(&self, remote_port: u16, local: Option<u16>) {
        self.send(Command::Forward(ForwardRequest {
            remote_port,
            local: local.map_or(LocalPort::Same, LocalPort::Exact),
            label: remote_port.to_string(),
            kind: ForwardKind::Manual,
        }));
    }

    pub fn stop(&self, remote_port: u16) {
        self.send(Command::Stop(remote_port));
    }

    /// Stops every forward and waits for it; dropping does the same.
    pub fn close(&self) {
        let sender = locked(&self.commands).take();
        if let Some(sender) = sender {
            if sender.send(Command::Close).is_err() {
                eprintln!("ports: the forwarding thread is gone");
            }
        }
        let worker = locked(&self.worker).take();
        if let Some(worker) = worker {
            if worker.join().is_err() {
                eprintln!("ports: the forwarding thread panicked");
            }
        }
    }

    /// Changes whenever a forward or a notice does.
    pub fn version(&self) -> u64 {
        self.inner.version.load(Ordering::Acquire)
    }

    pub fn host(&self) -> &str {
        &self.inner.forwarder.host().name
    }

    /// The workspace the window shows.
    pub fn branch(&self) -> String {
        locked(&self.inner.state).branch.clone()
    }

    pub fn proxy_port_here(&self) -> u16 {
        self.inner.proxy_port_here
    }

    /// Every forward, the host's dev proxy first.
    pub fn forwards(&self) -> Vec<PortForward> {
        let mut forwards = self.inner.forwarder.forwards();
        forwards.sort_by_key(|forward| (forward.kind != ForwardKind::Gateway, forward.remote_port));
        forwards
    }

    pub fn gateway(&self) -> Option<PortForward> {
        self.inner
            .forwarder
            .forwards()
            .into_iter()
            .find(|forward| forward.kind == ForwardKind::Gateway)
    }

    /// The route the dev proxy here takes for this window's workspaces, once the host's dev proxy is forwarded.
    pub fn gateway_route(&self) -> Option<GatewayRoute> {
        let gateway = self.gateway()?;
        let port = self.inner.forwarder.local_port(gateway.remote_port)?;
        let state = locked(&self.inner.state);
        Some(GatewayRoute {
            host: self.host().to_string(),
            session: state.session.clone(),
            branches: state.branches.clone(),
            port,
        })
    }

    /// The browser address of `repo/service` in workspace `branch`: through the dev proxy here, which sends it on
    /// to the host. `None` until the host's dev proxy is forwarded.
    pub fn url_for(&self, repo: &str, service: &str, branch: &str) -> Option<String> {
        self.gateway_route()?;
        let public = pom_env::dev_network().public();
        Some(public.url(&pom_env::service_host(
            service,
            repo,
            branch,
            &public.domain,
        )))
    }

    /// `127.0.0.1:<port here>` of a forwarded shared service, for connection strings shown here.
    pub fn shared_address(&self, name: &str) -> Option<String> {
        self.inner
            .forwarder
            .forwards()
            .into_iter()
            .find(|forward| forward.kind == ForwardKind::Shared && forward.label == name)
            .and_then(|forward| {
                self.inner
                    .forwarder
                    .local_port(forward.remote_port)
                    .map(|port| format!("127.0.0.1:{port}"))
            })
    }

    /// The port here a remote port is forwarded to.
    pub fn local_port(&self, remote_port: u16) -> Option<u16> {
        self.inner.forwarder.local_port(remote_port)
    }

    /// What became reachable since the last call, each said once.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut locked(&self.inner.state).notices)
    }
}

impl Drop for RemotePorts {
    fn drop(&mut self) {
        self.close();
    }
}

impl Inner {
    fn changed(&self) {
        self.version.fetch_add(1, Ordering::AcqRel);
        (self.waker)();
    }

    fn apply(&self, sync: Look) {
        let was_connected = {
            let mut state = locked(&self.state);
            let was = state.connected;
            state.connected = sync.connected;
            if !sync.session.is_empty() {
                state.session = sync.session.clone();
            }
            if !sync.branches.is_empty() {
                state.branches = sync.branches.clone();
            }
            state.branch = sync.branch.clone();
            was
        };
        if !sync.connected {
            if was_connected {
                self.forwarder.mark_lost();
                self.changed();
            }
            return;
        }
        if !was_connected && self.forwarder.reapply() > 0 {
            self.changed();
        }
        if !self
            .forwarder
            .forwards()
            .iter()
            .any(|forward| forward.kind == ForwardKind::Gateway)
        {
            let remote_port = self
                .source
                .dev_proxy_port()
                .unwrap_or_else(|| pom_env::DevPorts::default().proxy);
            if let Err(error) = self.forwarder.forward(&ForwardRequest {
                remote_port,
                local: LocalPort::Any,
                label: GATEWAY_LABEL.to_string(),
                kind: ForwardKind::Gateway,
            }) {
                eprintln!(
                    "ports: the dev proxy of {}: {error}",
                    self.forwarder.host().name
                );
            }
            self.changed();
        }
        self.follow_shared(&sync.shared);
    }

    /// Forwards the shared services that run and stops those that went; says which became reachable here.
    fn follow_shared(&self, shared: &[SharedPort]) {
        let wanted: HashSet<u16> = shared.iter().map(|service| service.port).collect();
        for forward in self.forwarder.forwards() {
            if forward.kind == ForwardKind::Shared && !wanted.contains(&forward.remote_port) {
                self.forwarder.stop(forward.remote_port);
                let mut state = locked(&self.state);
                state.tried.remove(&forward.remote_port);
                state.ready.remove(&forward.remote_port);
                drop(state);
                self.changed();
            }
        }
        let mut reachable = Vec::new();
        for service in shared {
            let first = locked(&self.state).tried.insert(service.port);
            if first {
                if let Err(error) = self.forwarder.forward(&ForwardRequest {
                    remote_port: service.port,
                    local: LocalPort::Same,
                    label: service.name.clone(),
                    kind: ForwardKind::Shared,
                }) {
                    eprintln!(
                        "ports: {} on {}: {error}",
                        service.name,
                        self.forwarder.host().name
                    );
                }
                self.changed();
            }
            let Some(local) = self.forwarder.local_port(service.port) else {
                continue;
            };
            let was_ready = locked(&self.state)
                .ready
                .insert(service.port, service.ready)
                .unwrap_or(false);
            if service.ready && !was_ready {
                reachable.push((service.name.clone(), local));
            }
        }
        if let Some(notice) = reachable_notice(&self.forwarder.host().name, &reachable) {
            locked(&self.state).notices.push(notice);
            self.changed();
        }
    }
}

/// `postgres on dev is reachable here at 127.0.0.1:5432`, or the list of them.
pub fn reachable_notice(host: &str, reachable: &[(String, u16)]) -> Option<String> {
    match reachable {
        [] => None,
        [(name, port)] => Some(format!(
            "{name} on {host} is reachable here at 127.0.0.1:{port}"
        )),
        many => {
            let names: Vec<&str> = many.iter().map(|(name, _)| name.as_str()).collect();
            let addresses: Vec<String> = many
                .iter()
                .map(|(_, port)| format!("127.0.0.1:{port}"))
                .collect();
            Some(format!(
                "{} on {host} are reachable here at {}",
                names.join(", "),
                addresses.join(", ")
            ))
        }
    }
}

fn run(inner: &Inner, commands: &mpsc::Receiver<Command>) {
    loop {
        match commands.recv_timeout(REAPPLY_EVERY) {
            Ok(Command::Sync(sync)) => inner.apply(sync),
            Ok(Command::Forward(request)) => {
                if let Err(error) = inner.forwarder.forward(&request) {
                    eprintln!("ports: {}: {error}", request.remote_port);
                }
                inner.changed();
            }
            Ok(Command::Stop(port)) => {
                inner.forwarder.stop(port);
                locked(&inner.state).tried.remove(&port);
                inner.changed();
            }
            Err(RecvTimeoutError::Timeout) => {
                let connected = locked(&inner.state).connected;
                if connected && inner.forwarder.reapply() > 0 {
                    inner.changed();
                }
            }
            Ok(Command::Close) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    inner.forwarder.close();
    inner.changed();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentTurn, HostUrl, ServiceAction, SourceError, StatusReport, WorkspaceList,
        WorkspaceSummary,
    };
    use std::path::PathBuf;
    use std::time::Instant;

    const FAKE_SSH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-ssh.sh");

    struct Host8767;

    impl ProjectSource for Host8767 {
        fn location(&self) -> String {
            "dev".into()
        }
        fn workspaces(&self) -> Result<WorkspaceList, SourceError> {
            Ok(WorkspaceList::default())
        }
        fn status(&self, _branch: &str) -> Result<StatusReport, SourceError> {
            Ok(StatusReport::default())
        }
        fn logs(&self, _branch: &str, _service: &str) -> Result<String, SourceError> {
            Ok(String::new())
        }
        fn agents(&self, _branch: &str) -> Result<Vec<pom_agent::SessionView>, SourceError> {
            Ok(Vec::new())
        }
        fn agent_turn(&self, _branch: &str, _role: &str) -> Result<Option<AgentTurn>, SourceError> {
            Ok(None)
        }
        fn act(
            &self,
            _branch: &str,
            _action: ServiceAction,
            _target: &str,
        ) -> Result<String, SourceError> {
            Ok(String::new())
        }
        fn dev_proxy_port(&self) -> Option<u16> {
            Some(8767)
        }
    }

    fn free_run(start: u16, length: u16) -> u16 {
        (start..start + 5000)
            .step_by(37)
            .find(|first| (0..length).all(|step| crate::port_free(first + step)))
            .expect("free ports")
    }

    fn snapshot(connection: Connection, shared: &[(&str, u16, bool)]) -> Snapshot {
        Snapshot {
            connection,
            branch: "feat-login".into(),
            workspaces: Some(WorkspaceList {
                project: "myproject".into(),
                workspaces: ["main", "feat-login"]
                    .iter()
                    .map(|branch| WorkspaceSummary {
                        branch: branch.to_string(),
                        ..WorkspaceSummary::default()
                    })
                    .collect(),
                ..WorkspaceList::default()
            }),
            status: Some(StatusReport {
                shared: shared
                    .iter()
                    .map(|(name, port, ready)| pom_services::SharedHealth {
                        name: name.to_string(),
                        up: true,
                        ready: *ready,
                        port: *port,
                    })
                    .collect(),
                ..StatusReport::default()
            }),
            ..Snapshot::default()
        }
    }

    fn wait(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn calls(log: &PathBuf, verb: &str) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with(&format!("-O {verb} ")))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_window_forwards_the_host_proxy_and_shared_services_and_cancels_them_on_close() {
        let temp = tempfile::Builder::new()
            .prefix("pomp")
            .tempdir_in("/tmp")
            .expect("temp");
        let program = PathBuf::from(FAKE_SSH);
        let log = temp.path().join("ssh.log");
        let records = temp.path().join("records");
        let ssh = Ssh::custom(
            program,
            Some(temp.path().join("control")),
            vec![("FAKE_SSH_LOG".into(), log.to_string_lossy().into())],
        );
        let postgres = free_run(41000, 1);
        let ports = RemotePorts::start(PortsSetup {
            source: Arc::new(Host8767),
            ssh,
            host: Host {
                name: "dev".into(),
                url: HostUrl::parse("ssh://dev@dev.example.net").expect("url"),
                pom_path: None,
                transport: None,
            },
            project: "myproject".into(),
            record_dir: Some(records.clone()),
            proxy_port_here: 8767,
            waker: Arc::new(|| {}),
        });

        ports.sync(&snapshot(
            Connection::Connected,
            &[("postgres", postgres, false)],
        ));
        wait("the dev proxy and postgres are forwarded", || {
            ports.forwards().len() == 2 && ports.local_port(postgres).is_some()
        });
        let gateway = ports.gateway_route().expect("gateway");
        assert_eq!(gateway.session, "myproject");
        assert_eq!(gateway.branches, ["main", "feat-login"]);
        assert!(calls(&log, "forward")
            .iter()
            .any(|call| call.contains(":127.0.0.1:8767 ")));
        assert_eq!(
            ports.shared_address("postgres"),
            Some(format!("127.0.0.1:{postgres}"))
        );
        let url = ports.url_for("api", "server", "feat-login").expect("url");
        assert!(
            url.starts_with("http://server.api.feat-login.localhost"),
            "{url}"
        );
        assert!(ports.take_notices().is_empty(), "not reachable yet");

        ports.sync(&snapshot(
            Connection::Connected,
            &[("postgres", postgres, true)],
        ));
        wait("a notice", || {
            !locked(&ports.inner.state).notices.is_empty()
        });
        assert_eq!(
            ports.take_notices(),
            vec![format!(
                "postgres on dev is reachable here at 127.0.0.1:{postgres}"
            )]
        );

        let before = calls(&log, "forward").len();
        ports.sync(&snapshot(
            Connection::Reconnecting {
                attempt: 1,
                next_in: Duration::from_secs(1),
            },
            &[("postgres", postgres, true)],
        ));
        wait("lost", || {
            ports
                .forwards()
                .iter()
                .all(|forward| forward.state == crate::ForwardState::Lost)
        });
        ports.sync(&snapshot(
            Connection::Connected,
            &[("postgres", postgres, true)],
        ));
        wait("forwarded again after the reconnect", || {
            calls(&log, "forward").len() >= before + 2
        });

        ports.forward(9229, None);
        wait("the port asked for", || ports.local_port(9229).is_some());
        ports.sync(&snapshot(Connection::Connected, &[]));
        wait("postgres stopped once it went", || {
            ports.local_port(postgres).is_none()
        });
        assert!(records.join("myproject.json").exists());

        drop(ports);
        let cancels = calls(&log, "cancel");
        assert_eq!(
            cancels.len(),
            3,
            "postgres, then the proxy and 9229 on close: {cancels:?}"
        );
        assert!(!records.join("myproject.json").exists());
    }

    #[test]
    fn notices_name_every_service_that_became_reachable() {
        assert_eq!(reachable_notice("dev", &[]), None);
        assert_eq!(
            reachable_notice("dev", &[("redis".into(), 6379), ("minio".into(), 9000)]).as_deref(),
            Some("redis, minio on dev are reachable here at 127.0.0.1:6379, 127.0.0.1:9000")
        );
    }
}
