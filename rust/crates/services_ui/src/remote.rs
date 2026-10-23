//! The Services panel's source for a project on another machine: the facts its pom server pushes (read
//! through a [`pom_remote::Watch`]), actions sent there and waited for in the order they were asked, consoles
//! attached to the service's holder there, and its output streamed in.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pom_config::{Config, Service, SharedServiceDef};
use pom_remote::server_api::{
    service_status, JobRequest, ServiceFact, ServicesFacts, SharedFact, TaskArgs,
};
use pom_remote::{
    LogEvent, ProjectSource, ServiceAction, ServiceOffers, ServiceVerb, SourceError, Watch,
};
use pom_services::ServiceTarget;
use terminal::Waker;
use workspace::PanelRequest;

use crate::access::{
    Console, ConsoleRequest, LogLines, RepoCommand, ServiceControl, ServicesAccess,
};
use crate::model::{Action, Crash, Status, ALL_SHARED};

/// How often a shared service's output, or a service's on a server that does not stream it, is read again.
const READ_AGAIN: Duration = Duration::from_millis(2500);
/// How long a stream reader waits for output before it looks whether it should stop.
const STREAM_WAIT: Duration = Duration::from_millis(250);
/// The most lines a tab keeps of a streamed output.
const KEPT_LINES: usize = 10_000;
const SHARED_LINES: usize = 200;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn verb(action: Action) -> ServiceVerb {
    match action {
        Action::Start => ServiceVerb::Start,
        Action::Stop => ServiceVerb::Stop,
        Action::Restart => ServiceVerb::Restart,
        Action::Relocate => ServiceVerb::Relocate,
    }
}

fn epoch(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

type Work = Box<dyn FnOnce() + Send>;

/// One workspace of a project on another machine.
pub struct RemoteServices {
    watch: Watch,
    source: Arc<dyn ProjectSource>,
    branch: String,
    /// The machine's name, for what is said about it.
    host: String,
    parsed: Mutex<Option<(String, Arc<Config>)>>,
    /// Actions run one at a time in the order asked, so a mode picked lands before the restart that follows.
    queue: Mutex<Option<Sender<Work>>>,
    notes: Arc<Mutex<Vec<String>>>,
    tasks: Arc<Mutex<Vec<(JobRequest, String)>>>,
    ports: Option<Arc<pom_remote::RemotePorts>>,
}

impl RemoteServices {
    pub fn new(watch: Watch, branch: &str, host: &str) -> RemoteServices {
        RemoteServices {
            source: watch.source(),
            watch,
            branch: branch.to_string(),
            host: host.to_string(),
            parsed: Mutex::new(None),
            queue: Mutex::new(None),
            notes: Arc::default(),
            tasks: Arc::default(),
            ports: None,
        }
    }

    /// Opens services and shows shared addresses through what `ports` forwards here.
    pub fn with_ports(mut self, ports: Arc<pom_remote::RemotePorts>) -> RemoteServices {
        self.ports = Some(ports);
        self
    }

    /// The port here a port of the machine is forwarded to, else the machine's own.
    fn port_here(&self, remote: u16) -> u16 {
        self.ports
            .as_ref()
            .and_then(|ports| ports.local_port(remote))
            .unwrap_or(remote)
    }

    /// Repo commands asked to run, as jobs for the window to start, with their titles.
    pub fn take_tasks(&self) -> Vec<(JobRequest, String)> {
        std::mem::take(&mut *locked(&self.tasks))
    }

    fn facts(&self) -> Option<Arc<ServicesFacts>> {
        self.watch
            .services()
            .0
            .filter(|facts| facts.branch == self.branch)
    }

    fn offers(&self) -> ServiceOffers {
        self.source
            .transport()
            .map(|transport| transport.services)
            .unwrap_or_default()
    }

    fn service(&self, target: &ServiceTarget) -> Option<ServiceFact> {
        self.facts()?
            .services
            .iter()
            .find(|fact| fact.repo == target.repo && fact.service == target.service)
            .cloned()
    }

    fn by_holder<T>(&self, holder: &str, read: impl Fn(&ServiceFact) -> T) -> Option<T> {
        self.facts()?
            .services
            .iter()
            .find(|fact| fact.holder == holder)
            .map(read)
    }

    fn shared(&self, name: &str) -> Option<SharedFact> {
        self.facts()?
            .shared
            .iter()
            .find(|fact| fact.name == name)
            .cloned()
    }

    fn shared_by_holder<T>(&self, holder: &str, read: impl Fn(&SharedFact) -> T) -> Option<T> {
        self.facts()?
            .shared
            .iter()
            .find(|fact| fact.holder == holder)
            .map(read)
    }

    /// Reads the facts again now, so what an action changed shows before the next push.
    fn refresh(source: &dyn ProjectSource, watch: &Watch, branch: &str) {
        match source.services_facts(branch) {
            Ok(facts) => watch.put_services(facts),
            Err(error) => eprintln!("services: reading the facts again: {error}"),
        }
    }

    /// Runs `work` after every action asked before it.
    fn enqueue(&self, work: Work) {
        let mut queue = locked(&self.queue);
        if queue.is_none() {
            let (sender, receiver) = mpsc::channel::<Work>();
            let spawned = std::thread::Builder::new()
                .name("services-remote".into())
                .spawn(move || {
                    for work in receiver {
                        work();
                    }
                });
            match spawned {
                Ok(_) => *queue = Some(sender),
                Err(error) => {
                    locked(&self.notes).push(format!("Could not run the action: {error}"));
                    return;
                }
            }
        }
        let sent = queue.as_ref().map(|sender| sender.send(work));
        if !matches!(sent, Some(Ok(()))) {
            *queue = None;
            locked(&self.notes).push("Could not run the action".to_string());
        }
    }

    /// Runs `work` in turn and waits for what it says.
    fn in_turn(
        &self,
        work: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        let (done, result) = mpsc::channel();
        self.enqueue(Box::new(move || {
            if done.send(work()).is_err() {
                eprintln!("services: an action finished after its caller left");
            }
        }));
        result
            .recv()
            .unwrap_or_else(|_| Err("the action did not run".to_string()))
    }

    /// Sends a choice in turn, without waiting: a failure is said as a note.
    fn choose(
        &self,
        what: &'static str,
        set: impl FnOnce(&dyn ProjectSource) -> Result<(), SourceError> + Send + 'static,
    ) {
        let (source, watch, branch, notes) = (
            self.source.clone(),
            self.watch.clone(),
            self.branch.clone(),
            self.notes.clone(),
        );
        self.enqueue(Box::new(move || {
            match set(source.as_ref()) {
                Ok(()) => Self::refresh(source.as_ref(), &watch, &branch),
                Err(error) => locked(&notes).push(format!("Could not change the {what}: {error}")),
            }
            ui::wake();
        }));
    }

    fn missing_newer(&self, what: &str) -> String {
        format!("{what} needs a newer pom on {}", self.host)
    }
}

impl ServicesAccess for RemoteServices {
    fn config(&self) -> Option<Arc<Config>> {
        let facts = self.facts()?;
        let mut parsed = locked(&self.parsed);
        if let Some((text, config)) = parsed.as_ref() {
            if *text == facts.config {
                return Some(config.clone());
            }
        }
        let path = facts.root.join(pom_config::CONFIG_FILE_NAME);
        match Config::from_text(&facts.config, &path) {
            Ok(config) => {
                let config = Arc::new(config);
                *parsed = Some((facts.config.clone(), config.clone()));
                Some(config)
            }
            Err(error) => {
                eprintln!("services: {error}");
                parsed.as_ref().map(|(_, config)| config.clone())
            }
        }
    }

    fn holder_name(&self, target: &ServiceTarget) -> String {
        if let Some(fact) = self.service(target) {
            return fact.holder;
        }
        let session = self
            .config()
            .map(|config| config.session.clone())
            .unwrap_or_default();
        pom_services::service_holder_name(&session, target)
    }

    fn status_of(&self, target: &ServiceTarget) -> Status {
        match self.service(target).map(|fact| fact.status) {
            Some(status) if status == service_status::RUNNING => Status::Running,
            Some(status) if status == service_status::CRASHED => Status::Crashed,
            _ => Status::Stopped,
        }
    }

    fn crash(&self, holder: &str) -> Option<Crash> {
        self.by_holder(holder, |fact| fact.crash.clone())
            .flatten()
            .map(|crash| Crash {
                line: crash.line,
                exit: crash.exit,
                at: crash.at.map(epoch),
            })
    }

    fn crash_log(&self, _holder: &str) -> Option<PathBuf> {
        None
    }

    fn shared_running(&self) -> HashSet<String> {
        self.facts()
            .map(|facts| {
                facts
                    .shared
                    .iter()
                    .filter(|fact| fact.running)
                    .map(|fact| fact.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn others_running(&self, _own: &HashSet<String>) -> usize {
        self.facts().map_or(0, |facts| facts.others_running)
    }

    fn port(&self, _config: &Config, target: &ServiceTarget) -> Option<u16> {
        self.service(target)?.port
    }

    fn url(&self, _config: &Config, target: &ServiceTarget) -> Option<String> {
        self.service(target)?.url
    }

    fn browser_url(&self, config: &Config, target: &ServiceTarget) -> Result<String, String> {
        let url = self
            .url(config, target)
            .ok_or_else(|| "the service has no address".to_string())?;
        let alias = config
            .repos
            .get(&target.repo)
            .map(|dir| dir.alias.clone())
            .filter(|alias| !alias.is_empty())
            .unwrap_or_else(|| target.repo.clone());
        let forwarded = self
            .ports
            .as_ref()
            .and_then(|ports| ports.url_for(&alias, &target.service, &self.branch));
        if let Some(forwarded) = forwarded {
            return Ok(forwarded);
        }
        if self.facts().is_some_and(|facts| facts.public_urls) {
            return Ok(url);
        }
        Err(format!(
            "{url} answers on {} only; forward its dev proxy in Ports to open it here",
            self.host
        ))
    }

    fn mode(&self, target: &ServiceTarget, service: &Service) -> String {
        self.service(target)
            .map(|fact| fact.mode)
            .filter(|mode| !mode.is_empty())
            .unwrap_or_else(|| service.mode.clone())
    }

    fn set_mode(&self, _config: &Config, target: &ServiceTarget, mode: &str) -> Result<(), String> {
        if let Some(reason) = self.missing(ServiceControl::Mode) {
            return Err(reason);
        }
        let (branch, repo, service, mode) = (
            self.branch.clone(),
            target.repo.clone(),
            target.service.clone(),
            mode.to_string(),
        );
        self.choose("mode", move |source| {
            source.set_service_mode(&branch, &repo, &service, &mode)
        });
        Ok(())
    }

    fn env_profile(&self, _config: &Config, target: &ServiceTarget) -> String {
        self.service(target)
            .map(|fact| fact.env_profile)
            .unwrap_or_default()
    }

    fn set_env_profile(
        &self,
        _config: &Config,
        target: &ServiceTarget,
        profile: &str,
    ) -> Result<(), String> {
        if let Some(reason) = self.missing(ServiceControl::EnvProfile) {
            return Err(reason);
        }
        let (branch, repo, service, profile) = (
            self.branch.clone(),
            target.repo.clone(),
            target.service.clone(),
            profile.to_string(),
        );
        self.choose("env profile", move |source| {
            source.set_env_profile(&branch, &repo, &service, &profile)
        });
        Ok(())
    }

    fn uptime(&self, holder: &str) -> Option<String> {
        let started = self
            .by_holder(holder, |fact| fact.started_at)
            .or_else(|| self.shared_by_holder(holder, |fact| fact.started_at))
            .flatten()?;
        crate::view::uptime_since(epoch(started))
    }

    fn pid(&self, holder: &str) -> Option<i32> {
        self.by_holder(holder, |fact| fact.pid)
            .or_else(|| self.shared_by_holder(holder, |fact| fact.pid))
            .flatten()
    }

    fn shared_host_port(&self, name: &str) -> u16 {
        self.shared(name)
            .map_or(0, |fact| self.port_here(fact.host_port))
    }

    fn shared_holder(&self, name: &str) -> String {
        self.shared(name)
            .map(|fact| fact.holder)
            .unwrap_or_default()
    }

    fn shared_connection(&self, _config: &Config, _branch: &str, name: &str) -> Option<String> {
        let fact = self.shared(name)?;
        let connection = fact.connection?;
        let here = self
            .ports
            .as_ref()
            .and_then(|ports| ports.shared_address(name));
        Some(match here {
            Some(address) => here_address(&connection, fact.host_port, &address),
            None => connection,
        })
    }

    fn shared_folder(&self, config: &Config, def: &SharedServiceDef) -> String {
        config
            .shared_services
            .iter()
            .find(|(_, known)| *known == def)
            .and_then(|(name, _)| self.shared(name))
            .map(|fact| fact.folder)
            .filter(|folder| !folder.is_empty())
            .unwrap_or_else(|| "project folder".to_string())
    }

    fn run(&self, _config: &Config, action: Action, target: &ServiceTarget) -> Result<(), String> {
        let offers = self.offers();
        let (source, watch, branch) =
            (self.source.clone(), self.watch.clone(), self.branch.clone());
        let (repo, service) = (target.repo.clone(), target.service.clone());
        if offers.run {
            return self.in_turn(move || {
                let result = source
                    .run_service(&branch, &repo, &service, verb(action))
                    .map_err(|error| error.to_string());
                Self::refresh(source.as_ref(), &watch, &branch);
                result
            });
        }
        let older = match action {
            Action::Start => ServiceAction::Start,
            Action::Stop => ServiceAction::Stop,
            Action::Restart => ServiceAction::Restart,
            Action::Relocate => return Err(self.missing_newer("Moving a service to a new port")),
        };
        if target.is_workspace_level() {
            return Err(self.missing_newer("A workspace service"));
        }
        self.in_turn(move || {
            source
                .act(&branch, older, &format!("{repo}/{service}"))
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    fn run_shared(&self, _config: &Config, name: &str, action: Action) -> Result<(), String> {
        if let Some(reason) = self.missing(ServiceControl::Shared) {
            return Err(reason);
        }
        let (source, watch, branch) =
            (self.source.clone(), self.watch.clone(), self.branch.clone());
        let name = if name == ALL_SHARED { "*" } else { name }.to_string();
        self.in_turn(move || {
            let result = source
                .run_shared(&name, verb(action))
                .map_err(|error| error.to_string());
            Self::refresh(source.as_ref(), &watch, &branch);
            result
        })
    }

    fn checked_out(&self, _root: &Path, repo: &str) -> bool {
        self.facts()
            .is_some_and(|facts| facts.repos.iter().any(|known| known == repo))
    }

    fn run_command(
        &self,
        _config: &Config,
        branch: &str,
        _root: &Path,
        command: &RepoCommand,
    ) -> Vec<PanelRequest> {
        if let Some(reason) = self.missing(ServiceControl::Tasks) {
            return vec![PanelRequest::Toast(reason)];
        }
        let request = JobRequest::Task(TaskArgs {
            branch: branch.to_string(),
            repo: command.repo.clone(),
            label: command.label.clone(),
        });
        locked(&self.tasks).push((request, format!("{}: {}", command.repo, command.label)));
        ui::wake();
        Vec::new()
    }

    fn console(&self, request: ConsoleRequest<'_>) -> Console {
        let waker = request.waker.clone();
        if request.target.repo.is_empty() {
            let (source, name) = (self.source.clone(), request.target.service.clone());
            let lines = LineStream::reread(waker, move || {
                source
                    .shared_logs(&name, SHARED_LINES)
                    .map_err(|error| error.to_string())
            });
            return Console {
                lines: Some(LogLines::Stream(lines)),
                shell: None,
            };
        }
        let service = format!("{}/{}", request.target.repo, request.target.service);
        let shell = request
            .live
            .then(|| self.watch.terminals())
            .flatten()
            .and_then(|provider| {
                let remote = terminal::RemoteOptions {
                    provider,
                    target: terminal::RemoteTarget::Service(service.clone()),
                    mode: terminal::RemoteMode::ReadWrite,
                };
                terminal_ui::TerminalItem::remote_console(
                    request.console_id,
                    request.root.to_path_buf(),
                    request.item_id.to_string(),
                    request.target.service.clone(),
                    remote,
                    false,
                    waker.clone(),
                )
                .map_err(|error| eprintln!("services: console: {error}"))
                .ok()
            });
        let lines = match self.source.follow_logs(&self.branch, &service) {
            Ok(Some(feed)) => LineStream::follow(waker, feed),
            _ => {
                let (source, branch) = (self.source.clone(), self.branch.clone());
                LineStream::reread(waker, move || {
                    source
                        .logs(&branch, &service)
                        .map_err(|error| error.to_string())
                })
            }
        };
        Console {
            lines: Some(LogLines::Stream(lines)),
            shell,
        }
    }

    fn missing(&self, control: ServiceControl) -> Option<String> {
        let offers = self.offers();
        let (offered, what) = match control {
            ServiceControl::Relocate => (offers.run, "Moving a service to a new port"),
            ServiceControl::Shared => (offers.shared, "Starting and stopping shared services"),
            ServiceControl::Mode => (offers.mode, "Changing a service's mode"),
            ServiceControl::EnvProfile => (offers.env_profile, "Changing a service's env profile"),
            ServiceControl::Tasks => (offers.tasks, "Running a repo's commands"),
            ServiceControl::SideAgents => {
                return Some(format!(
                    "Agents run on {}: start one in the agent dock",
                    self.host
                ))
            }
        };
        (!offered).then(|| self.missing_newer(what))
    }

    fn unavailable(&self) -> Option<String> {
        if self.facts().is_some() {
            return None;
        }
        if self.source.transport().is_some_and(|transport| {
            transport.via == pom_remote::Via::Ssh || !transport.services.facts
        }) {
            return Some(format!(
                "Services need a newer pom on {}; update it there",
                self.host
            ));
        }
        let (_, problem) = self.watch.services();
        Some(problem.unwrap_or_else(|| format!("Asking {}...", self.host)))
    }

    fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *locked(&self.notes))
    }
}

/// `connection` with the machine's `127.0.0.1:<port>` (or `localhost:<port>`) as `address` here.
fn here_address(connection: &str, port: u16, address: &str) -> String {
    [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
        .iter()
        .fold(connection.to_string(), |text, there| {
            text.replace(there, address)
        })
}

#[derive(Default)]
struct StreamState {
    lines: Vec<String>,
    /// The bytes of the line still being printed.
    partial: Vec<u8>,
    changed: bool,
}

impl StreamState {
    fn push(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if *byte == b'\n' {
                let line = std::mem::take(&mut self.partial);
                self.lines.push(clean(&line));
            } else {
                self.partial.push(*byte);
            }
        }
        let extra = self.lines.len().saturating_sub(KEPT_LINES);
        self.lines.drain(..extra);
        self.changed = true;
    }

    fn replace(&mut self, text: &str) {
        let lines: Vec<String> = text.lines().map(|line| clean(line.as_bytes())).collect();
        if lines != self.lines || !self.partial.is_empty() {
            self.lines = lines;
            self.partial.clear();
            self.changed = true;
        }
    }
}

fn clean(line: &[u8]) -> String {
    pom_services::strip_terminal_codes(&String::from_utf8_lossy(line))
}

/// Output that arrives from elsewhere, as lines: streamed as it prints, or read again every few seconds.
pub struct LineStream {
    state: Arc<Mutex<StreamState>>,
    stop: Arc<AtomicBool>,
}

impl LineStream {
    fn spawn(
        waker: Waker,
        read: impl FnOnce(&Mutex<StreamState>, &AtomicBool) + Send + 'static,
    ) -> LineStream {
        let state = Arc::new(Mutex::new(StreamState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_state, thread_stop) = (state.clone(), stop.clone());
        let spawned = std::thread::Builder::new()
            .name("services-output".into())
            .spawn(move || {
                read(&thread_state, &thread_stop);
                waker();
            });
        if let Err(error) = spawned {
            locked(&state).replace(&format!("Could not read the output: {error}"));
        }
        LineStream { state, stop }
    }

    /// The output `feed` streams, from what the service left or printed so far.
    pub fn follow(waker: Waker, feed: Box<dyn pom_remote::LogFeed>) -> LineStream {
        let wake = waker.clone();
        Self::spawn(waker, move |state, stop| {
            while !stop.load(Ordering::Acquire) {
                match feed.next(STREAM_WAIT) {
                    LogEvent::Data(bytes) => {
                        locked(state).push(&bytes);
                        wake();
                    }
                    LogEvent::StartedOver => {
                        locked(state).replace("");
                        wake();
                    }
                    LogEvent::Skipped | LogEvent::Idle => {}
                    LogEvent::Ended(reason) => {
                        locked(state).push(format!("\n[{reason}]\n").as_bytes());
                        return;
                    }
                }
            }
        })
    }

    /// What `read` answers, read again every few seconds.
    pub fn reread(
        waker: Waker,
        read: impl Fn() -> Result<String, String> + Send + 'static,
    ) -> LineStream {
        let wake = waker.clone();
        Self::spawn(waker, move |state, stop| {
            while !stop.load(Ordering::Acquire) {
                let text = read().unwrap_or_else(|error| error);
                let changed = {
                    let mut state = locked(state);
                    state.replace(&text);
                    state.changed
                };
                if changed {
                    wake();
                }
                let mut waited = Duration::ZERO;
                while waited < READ_AGAIN && !stop.load(Ordering::Acquire) {
                    std::thread::sleep(STREAM_WAIT);
                    waited += STREAM_WAIT;
                }
            }
        })
    }

    pub fn lines(&self) -> Vec<String> {
        let state = locked(&self.state);
        let mut lines = state.lines.clone();
        if !state.partial.is_empty() {
            lines.push(clean(&state.partial));
        }
        lines
    }

    pub fn clear(&mut self) {
        let mut state = locked(&self.state);
        state.lines.clear();
        state.partial.clear();
        state.changed = true;
    }

    /// Whether anything arrived since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut locked(&self.state).changed)
    }
}

impl Drop for LineStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_connection_names_the_port_here() {
        assert_eq!(
            here_address(
                "postgres://pom:pom@127.0.0.1:5432/app",
                5432,
                "127.0.0.1:15432"
            ),
            "postgres://pom:pom@127.0.0.1:15432/app"
        );
        assert_eq!(
            here_address("redis://localhost:6379/2", 6379, "127.0.0.1:16379"),
            "redis://127.0.0.1:16379/2"
        );
    }

    #[test]
    fn streamed_output_reads_as_lines_without_terminal_codes() {
        let mut state = StreamState::default();
        state.push(b"\x1b[32mGET /\x1b[0m 200\r\nhal");
        state.push(b"f a line\n");
        assert_eq!(state.lines, ["GET / 200", "half a line"]);
        state.replace("one\ntwo\n");
        assert_eq!(state.lines, ["one", "two"]);
        assert!(state.changed);
    }
}
