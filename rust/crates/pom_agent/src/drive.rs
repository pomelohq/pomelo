//! The operations behind `pom agent` and the agent MCP tools, on one workspace's agent sessions: list,
//! start, stop, read and watch. Every operation that touches a session goes through the gate first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pom_paths::StateDir;
use pom_ptyhost::{SocketDir, SpawnRequest};
use serde::Serialize;
use serde_json::{json, Value};

use crate::driver::{driver, Keystrokes, TrustDialog};
use crate::fallback;
use crate::gate::{Gate, Refusal};
use crate::identity::{holder_role, workspace_prefix};
use crate::launch::{claude_launch_with, cli_launch, fresh_launch, LaunchContext, LaunchOptions};
use crate::screen::rendered_lines;
use crate::sessions::{
    events, now_ms, phase, read_state, record_event, turn_running, workspace_sessions, EventLine,
    SessionEvent, SessionState, TurnState, LAUNCHED_EVENT, NEEDS_TRUST_EVENT, NEEDS_TRUST_STATE,
    SCHEMA, STALE, STARTING_STATE,
};
use crate::turns::{read_turns, settle_transcript, RecordedTurn, SETTLE_CAP, SETTLE_QUIET};

/// A working state no event refreshed for this long is reported stale.
const STALE_AFTER_MS: u64 = 15 * 60 * 1000;
pub(crate) const HOLDER_COLS: u16 = 160;
pub(crate) const HOLDER_ROWS: u16 = 48;
const MCP_SERVER: &str = "pom";
const WORKSPACE_LOG_KEY: &str = "\u{0}workspace";

#[derive(Debug)]
pub enum DriveError {
    Refused(Refusal),
    /// The prompt was typed but the agent never took it as a turn, with why pom held back a second Enter.
    NotSubmitted(String, Option<&'static str>),
    NotFound(String),
    Invalid(String),
    Failed(String),
    /// The agent waits at its "do you trust this folder" prompt and nothing answered it.
    NeedsTrust(String),
    /// A queued send waited its whole timeout for the session to become idle.
    Timeout(String),
    /// An answer names a permission prompt that no longer waits.
    Stale(String),
    /// pom will not type this answer into the agent's terminal: `code` says why.
    NotTyped {
        code: &'static str,
        message: String,
    },
}

/// `pom agent start` found the agent waiting at its folder trust prompt.
pub const NEEDS_TRUST_EXIT: i32 = 7;
/// How long `start` waits for a new session's first hook, answering a trust prompt meanwhile.
const FIRST_HOOK_WAIT: Duration = Duration::from_secs(12);
/// A starting session older than this whose terminal shows the trust prompt is reported as waiting on it.
const TRUST_CHECK_AFTER_S: u64 = 2;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(1);
/// The agent redraws the trust prompt with "No, exit" highlighted again soon after showing it.
const TRUST_SETTLE: Duration = Duration::from_millis(300);
const TRUST_POLL: Duration = Duration::from_millis(50);
const TRUST_ANSWER_WAIT: Duration = Duration::from_secs(10);
const TRUST_KEYS_MAX: usize = 6;

impl DriveError {
    pub fn exit_code(&self) -> i32 {
        match self {
            DriveError::Refused(_) => crate::gate::REFUSED_EXIT,
            DriveError::NotSubmitted(..) => crate::conversation::NOT_SUBMITTED_EXIT,
            DriveError::NeedsTrust(_) => NEEDS_TRUST_EXIT,
            DriveError::Timeout(_) => crate::conversation::TIMEOUT_EXIT,
            DriveError::NotFound(_)
            | DriveError::Invalid(_)
            | DriveError::Failed(_)
            | DriveError::Stale(_)
            | DriveError::NotTyped { .. } => 1,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            DriveError::Refused(refusal) => refusal.code(),
            DriveError::NotSubmitted(..) => "not_submitted",
            DriveError::NotFound(_) => "not_found",
            DriveError::Invalid(_) => "invalid",
            DriveError::Failed(_) => "failed",
            DriveError::NeedsTrust(_) => "needs_trust",
            DriveError::Timeout(_) => "timeout",
            DriveError::Stale(_) => "stale",
            DriveError::NotTyped { code, .. } => code,
        }
    }

    pub fn to_json(&self) -> Value {
        let mut value =
            json!({"schema": SCHEMA, "error": self.code(), "message": self.to_string()});
        if let DriveError::Refused(Refusal::Busy { state, reason }) = self {
            value["state"] = json!(state);
            value["reason"] = json!(reason);
        }
        value
    }
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriveError::Refused(refusal) => write!(formatter, "{refusal}"),
            DriveError::NotSubmitted(handle, None) => write!(
                formatter,
                "the prompt was typed into {handle} but it never started a turn"
            ),
            DriveError::NotSubmitted(handle, Some(why)) => write!(
                formatter,
                "the prompt was typed into {handle} but it never started a turn; pom did not press Enter again because {why}, so look at its terminal before sending again"
            ),
            DriveError::NotFound(message)
            | DriveError::Invalid(message)
            | DriveError::Failed(message)
            | DriveError::NeedsTrust(message)
            | DriveError::Timeout(message)
            | DriveError::Stale(message)
            | DriveError::NotTyped { message, .. } => write!(formatter, "{message}"),
        }
    }
}

impl From<Refusal> for DriveError {
    fn from(refusal: Refusal) -> DriveError {
        DriveError::Refused(refusal)
    }
}

/// One agent session as `ls` reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SessionView {
    pub handle: String,
    pub workspace: String,
    pub role: String,
    pub holder: String,
    pub driver: String,
    pub session_id: String,
    pub alive: bool,
    pub drivable: bool,
    /// One of `sessions::STATES`.
    pub state: String,
    /// One of `sessions::REASONS`, or none.
    pub reason: Option<String>,
    pub turn: u64,
    pub last_event: String,
    pub last_event_age_s: u64,
    pub stale: bool,
    /// A turn is in progress: working, or waiting on a permission inside it.
    pub running: bool,
    /// One of `sessions::PHASES`.
    pub phase: String,
    /// The permission prompt a live session waits on, if any.
    pub pending_permission: Option<crate::permission::PermissionPrompt>,
}

impl SessionView {
    pub fn is_working(&self) -> bool {
        matches!(self.state.as_str(), "thinking" | "tool_use" | "compacting")
    }

    /// Sets `running` and `phase` from `state` and `reason`.
    fn derive_phase(mut self) -> SessionView {
        self.running = turn_running(&self.state, self.reason.as_deref());
        self.phase = phase(&self.state).to_string();
        self
    }
}

/// What a project's agent sessions need from the machine.
pub struct Drive {
    pub state: StateDir,
    pub holders: SocketDir,
    /// The project's session name.
    pub project: String,
    pub home: PathBuf,
    pub binary: PathBuf,
    pub tool_path: String,
    /// pom.yml's `limits:`, which cap the agents' holders with the rest of their workspace.
    pub limits: pom_config::LimitsConfig,
    /// pom.yml's `agents:`; `peer_approve` decides whether an agent may allow another agent's prompt.
    pub agents: pom_config::AgentsConfig,
}

fn view_of(record: &SessionState, alive: bool, drivable: bool, now: u64) -> SessionView {
    let age = now.saturating_sub(record.event_ms);
    let (status, reason) = record.status();
    let working = matches!(status.as_str(), "thinking" | "tool_use" | "compacting");
    let stale = working && alive && age > STALE_AFTER_MS;
    let state = if working && !alive {
        "died".to_string()
    } else if !alive && record.last_event == "SessionEnd" {
        "ended".to_string()
    } else {
        status
    };
    SessionView {
        handle: format!("{}/{}", record.branch, record.role),
        workspace: record.branch.clone(),
        role: record.role.clone(),
        holder: record.holder.clone(),
        driver: record.driver.clone(),
        session_id: record.session_id.clone(),
        alive,
        drivable,
        state,
        reason: if stale { Some(STALE.into()) } else { reason },
        turn: record.turn,
        last_event: record.last_event.clone(),
        last_event_age_s: age / 1000,
        stale,
        ..SessionView::default()
    }
    .derive_phase()
}

/// A handle names a session as `<workspace>/<role>`, or just its role inside the workspace in use.
pub fn split_handle(handle: &str, default_branch: &str) -> (String, String) {
    match handle.rsplit_once('/') {
        Some((branch, role)) if !role.is_empty() => (branch.to_string(), role.to_string()),
        _ => (default_branch.to_string(), handle.to_string()),
    }
}

impl Drive {
    pub(crate) fn holder_group(&self, branch: &str) -> pom_ptyhost::HolderGroup {
        let limit = self.limits.for_workspace(branch);
        pom_ptyhost::HolderGroup::new(
            &self.project,
            branch,
            pom_ptyhost::Limits {
                cpu_percent: limit.cpu_percent(),
                memory_bytes: limit.memory_bytes(),
            },
        )
    }

    fn alive(&self, holder: &str) -> bool {
        self.holders.holder_alive(holder)
    }

    /// The agent sessions of one workspace: every recorded one, plus live agent holders that have not
    /// reported yet (started before this version).
    pub fn list(&self, branch: &str) -> Vec<SessionView> {
        let now = now_ms();
        let mut views: Vec<SessionView> = Vec::new();
        for entry in workspace_sessions(&self.state, &self.project, branch) {
            let Some(record) = read_state(&self.state, &entry.session_id) else {
                continue;
            };
            let role = holder_role(&record.holder, &self.project, branch);
            let mut view = view_of(
                &record,
                self.alive(&record.holder),
                role.is_some_and(|role| role.drivable),
                now,
            );
            if view.alive {
                view.pending_permission =
                    crate::permission::pending_permission(&self.state, &record.session_id);
            }
            // A resumed conversation keeps its id; a relaunched role gets a new one: keep the newest per role.
            match views.iter_mut().find(|known| known.role == view.role) {
                Some(known) if known.alive && !view.alive => {}
                Some(known) => *known = view,
                None => views.push(view),
            }
        }
        for view in &mut views {
            let waiting = view.alive
                && view.state == STARTING_STATE
                && !view.session_id.is_empty()
                && view.last_event_age_s >= TRUST_CHECK_AFTER_S;
            if waiting && self.shows_trust_prompt(&view.holder, &view.driver) {
                self.mark_needs_trust(&view.holder, &view.session_id, branch);
                view.state = NEEDS_TRUST_STATE.into();
                *view = std::mem::take(view).derive_phase();
            }
        }
        let prefix = workspace_prefix(&self.project, branch);
        for (holder, _) in self.holders.holders() {
            if !holder.starts_with(&prefix) || views.iter().any(|view| view.holder == holder) {
                continue;
            }
            let Some(role) = holder_role(&holder, &self.project, branch) else {
                continue;
            };
            views.push(
                SessionView {
                    handle: format!("{branch}/{}", role.role),
                    workspace: branch.to_string(),
                    role: role.role,
                    holder,
                    driver: role.driver.to_string(),
                    session_id: String::new(),
                    alive: true,
                    drivable: role.drivable,
                    state: "unknown".into(),
                    reason: None,
                    turn: 0,
                    last_event: String::new(),
                    last_event_age_s: 0,
                    stale: false,
                    ..SessionView::default()
                }
                .derive_phase(),
            );
        }
        views.sort_by(|a, b| a.role.cmp(&b.role));
        views
    }

    /// Starts another CLI as the workspace's agent for the person (the `agent_command` setting), or finds it
    /// running; it reports through no hooks, so the listing knows it by its holder alone.
    pub fn start_cli(
        &self,
        gate: &Gate<'_>,
        cwd: &Path,
        is_main: bool,
        name: &str,
        command: &str,
    ) -> Result<SessionView, DriveError> {
        if gate.caller().is_agent() {
            return Err(DriveError::Invalid(
                "only a person can start an agent as their own".into(),
            ));
        }
        let workspace = gate.workspace().clone();
        let context = LaunchContext {
            state: &self.state,
            home: &self.home,
            binary: &self.binary,
            tool_path: &self.tool_path,
            session: &self.project,
            branch: &workspace.branch,
            is_main,
            cwd,
        };
        let launch = cli_launch(&context, name, command);
        gate.target(&launch.holder)?;
        if !self.alive(&launch.holder) {
            let group = self.holder_group(&workspace.branch);
            pom_ptyhost::spawn_holder(
                &self.holders,
                &SpawnRequest {
                    group: Some(&group),
                    binary: &self.binary,
                    name: &launch.holder,
                    cwd: &launch.cwd,
                    cols: HOLDER_COLS,
                    rows: HOLDER_ROWS,
                    argv: &launch.argv,
                    env: &[],
                },
            )
            .map_err(|error| DriveError::Failed(format!("could not start {name}: {error}")))?;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !self.alive(&launch.holder) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        self.list(&workspace.branch)
            .into_iter()
            .find(|view| view.holder == launch.holder)
            .ok_or_else(|| DriveError::Failed(format!("{name} did not start")))
    }

    /// The session a handle names, inside the gate's workspace.
    pub fn resolve(&self, gate: &Gate<'_>, handle: &str) -> Result<SessionView, DriveError> {
        let workspace = gate.workspace();
        let (branch, role) = split_handle(handle, &workspace.branch);
        let by_holder = handle.starts_with("ws-");
        if !by_holder && branch != workspace.branch {
            return Err(Refusal::OtherWorkspace {
                allowed: workspace.branch.clone(),
            }
            .into());
        }
        if by_holder {
            gate.target(handle)?;
        }
        self.list(&workspace.branch)
            .into_iter()
            .find(|view| {
                if by_holder {
                    view.holder == handle
                } else {
                    view.role == role
                }
            })
            .ok_or_else(|| {
                DriveError::NotFound(format!(
                    "no agent session {handle} in workspace {}",
                    workspace.branch
                ))
            })
    }

    /// Starts a session, or reuses the running one unless `fresh`. A fresh session runs as role `role`
    /// (`claude-<role>`) on a new conversation of its own.
    pub fn start(
        &self,
        gate: &Gate<'_>,
        cwd: &Path,
        is_main: bool,
        role: &str,
        fresh: bool,
        options: &LaunchOptions,
    ) -> Result<SessionView, DriveError> {
        if let Some(extra) = &options.extra_mcp_config {
            check_extra_mcp(extra)?;
        }
        if options.trust && gate.caller().is_agent() {
            return Err(DriveError::Invalid(
                "only a person or an orchestrator can trust a folder for an agent".into(),
            ));
        }
        if options.for_person && gate.caller().is_agent() {
            return Err(DriveError::Invalid(
                "only a person can start an agent as their own".into(),
            ));
        }
        let options = &LaunchOptions {
            isolated: !options.for_person,
            ..options.clone()
        };
        let workspace = gate.workspace().clone();
        let context = LaunchContext {
            state: &self.state,
            home: &self.home,
            binary: &self.binary,
            tool_path: &self.tool_path,
            session: &self.project,
            branch: &workspace.branch,
            is_main,
            cwd,
        };
        let (launch, session_id) = if fresh {
            if role == "claude" || !role_is_valid(role) {
                return Err(DriveError::Invalid(format!(
                    "a fresh session needs its own --role (lowercase letters, digits, dashes), not {role:?}"
                )));
            }
            fresh_launch(&context, role, options)
        } else if role == "claude" {
            (
                claude_launch_with(&context, options),
                crate::launch::session_id(&crate::launch::main_session_key(&context)),
            )
        } else {
            return self.resolve(gate, role);
        };
        gate.target(&launch.holder)?;
        let mut launched = false;
        if self.alive(&launch.holder) {
            if fresh {
                return Err(DriveError::Invalid(format!(
                    "{role} is already running in {}; stop it or pick another --role",
                    workspace.branch
                )));
            }
        } else {
            if options.trust {
                self.pretrust_folder(&launch.cwd, &workspace.branch, &launch.holder);
            }
            let group = self.holder_group(&workspace.branch);
            pom_ptyhost::spawn_holder(
                &self.holders,
                &SpawnRequest {
                    group: Some(&group),
                    binary: &self.binary,
                    name: &launch.holder,
                    cwd: &launch.cwd,
                    cols: HOLDER_COLS,
                    rows: HOLDER_ROWS,
                    argv: &launch.argv,
                    env: &[],
                },
            )
            .map_err(|error| DriveError::Failed(format!("could not start {role}: {error}")))?;
            // The agent reports its session only from its first hook, which a prompt it shows at start can hold
            // off indefinitely: record what pom launched so ls, takeover and read find it meanwhile.
            let identity = crate::identity::Identity::for_holder(
                &launch.holder,
                &self.project,
                &workspace.branch,
            );
            let launch_event = SessionEvent {
                session_id: session_id.clone(),
                event: LAUNCHED_EVENT.into(),
                transcript: crate::launch::expected_transcript_path(
                    &self.home,
                    &launch.cwd,
                    &session_id,
                )
                .to_string_lossy()
                .into_owned(),
                ..SessionEvent::default()
            };
            if let Err(error) = record_event(&self.state, &identity, &launch_event) {
                eprintln!("agents: recording the launch of {}: {error}", launch.holder);
            }
            launched = true;
            // Whoever starts a session drives it; a person takes it over from the app or with takeover.
            let role_name = holder_role(&launch.holder, &self.project, &workspace.branch)
                .map_or_else(|| role.to_string(), |role| role.role);
            crate::lease::set_lease(
                &self.state,
                &self.holders,
                &self.project,
                &workspace.branch,
                &launch.holder,
                &role_name,
                crate::lease::Lease::for_caller(gate.caller()),
            )
            .map_err(|error| {
                DriveError::Failed(format!("could not take the lease of {role}: {error}"))
            })?;
        }
        let holder = launch.holder.clone();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !self.alive(&holder) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if launched {
            self.await_first_hook(&holder, &session_id, &workspace.branch, options.trust)?;
        }
        let role_name = holder_role(&holder, &self.project, &workspace.branch)
            .map_or_else(|| role.to_string(), |role| role.role);
        Ok(self
            .list(&workspace.branch)
            .into_iter()
            .find(|view| view.holder == holder)
            .unwrap_or(
                SessionView {
                    handle: format!("{}/{role_name}", workspace.branch),
                    workspace: workspace.branch.clone(),
                    role: role_name,
                    holder,
                    driver: crate::identity::CLAUDE_DRIVER.into(),
                    session_id: String::new(),
                    alive: false,
                    drivable: true,
                    state: "starting".into(),
                    reason: None,
                    turn: 0,
                    last_event: String::new(),
                    last_event_age_s: 0,
                    stale: false,
                    ..SessionView::default()
                }
                .derive_phase(),
            ))
    }

    /// Ends a session and its holder.
    pub fn stop(&self, gate: &Gate<'_>, view: &SessionView) -> Result<(), DriveError> {
        gate.target(&view.holder)?;
        if !view.session_id.is_empty() {
            let identity = crate::identity::Identity {
                holder: view.holder.clone(),
                role: view.role.clone(),
                project: self.project.clone(),
                branch: view.workspace.clone(),
                driver: view.driver.clone(),
            };
            let event = crate::sessions::SessionEvent {
                session_id: view.session_id.clone(),
                event: "Killed".into(),
                state: Some(crate::hooks::AgentState::Idle),
                ..crate::sessions::SessionEvent::default()
            };
            if let Err(error) = crate::sessions::record_event(&self.state, &identity, &event) {
                eprintln!("agent: record stop of {}: {error}", view.handle);
            }
        }
        match self.holders.kill_holder(&view.holder) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DriveError::Failed(format!(
                "could not stop {}: {error}",
                view.handle
            ))),
        }
    }

    /// Whether the session's terminal shows the agent's folder trust prompt.
    fn shows_trust_prompt(&self, holder: &str, driver_name: &str) -> bool {
        let Some(agent_driver) = driver(driver_name) else {
            return false;
        };
        pom_ptyhost::snapshot(&self.holders, holder, SNAPSHOT_TIMEOUT)
            .is_ok_and(|screen| agent_driver.waits_for_trust(&screen))
    }

    fn mark_needs_trust(&self, holder: &str, session_id: &str, branch: &str) {
        let identity = crate::identity::Identity::for_holder(holder, &self.project, branch);
        let event = SessionEvent {
            session_id: session_id.to_string(),
            event: NEEDS_TRUST_EVENT.into(),
            ..SessionEvent::default()
        };
        if let Err(error) = record_event(&self.state, &identity, &event) {
            eprintln!("agents: recording that {holder} waits for trust: {error}");
        }
    }

    /// Types into a session as whoever holds its lease.
    fn type_keys(&self, holder: &str, keys: &[Keystrokes]) -> Result<(), DriveError> {
        let fail = |error: std::io::Error| {
            DriveError::Failed(format!("could not type into {holder}: {error}"))
        };
        let lease = crate::lease::read_lease(&self.state, holder);
        let mut connection = pom_ptyhost::connect_writer(&self.holders, holder).map_err(fail)?;
        if lease.class != crate::lease::LeaseClass::Human {
            connection.claim(&lease.token).map_err(fail)?;
        }
        for key in keys {
            connection.input(&key.bytes).map_err(fail)?;
            std::thread::sleep(Duration::from_millis(key.pause_ms));
        }
        Ok(())
    }

    /// Answers the trust prompt with yes one key at a time, each chosen from the settled screen and sent only
    /// while a fresh read still shows that same prompt and no person drives the session.
    fn answer_trust(&self, holder: &str, session_id: &str, branch: &str) -> Result<(), DriveError> {
        let agent_driver = driver(crate::identity::CLAUDE_DRIVER)
            .ok_or_else(|| DriveError::Failed("no driver for the agent".into()))?;
        let unanswered = |why: &str| {
            DriveError::NeedsTrust(format!(
                "{holder} waits at the agent's \"trust this folder\" prompt: {why}; open it in Pomelo (or pom agent takeover) and answer it there"
            ))
        };
        let role =
            holder_role(holder, &self.project, branch).map_or_else(String::new, |role| role.role);
        let note = |seen: &[String], sent: Option<String>, why: &str| {
            fallback::record(
                &self.state,
                &self.project,
                branch,
                &fallback::Entry {
                    holder,
                    role: &role,
                    session_id,
                    action: fallback::TRUST,
                    seen,
                    sent,
                    why,
                },
            );
        };
        let read = || {
            pom_ptyhost::snapshot(&self.holders, holder, SNAPSHOT_TIMEOUT)
                .map(|screen| rendered_lines(&screen))
        };
        let deadline = std::time::Instant::now() + TRUST_ANSWER_WAIT;
        let mut typed = 0;
        let mut still_since: Option<(Option<TrustDialog>, std::time::Instant)> = None;
        while std::time::Instant::now() < deadline {
            let lines = match read() {
                Ok(lines) => lines,
                Err(_) if !self.alive(holder) => return Ok(()),
                Err(error) => {
                    return Err(DriveError::Failed(format!(
                        "could not read the screen of {holder}: {error}"
                    )))
                }
            };
            let dialog = agent_driver.trust_dialog(&lines);
            // The agent keeps animating elsewhere on screen, so only the dialog itself has to hold still.
            let settled = match &still_since {
                Some((seen, since)) if *seen == dialog => since.elapsed() >= TRUST_SETTLE,
                _ => {
                    still_since = Some((dialog.clone(), std::time::Instant::now()));
                    false
                }
            };
            if settled {
                let Some(dialog) = dialog else {
                    if typed > 0 {
                        return Ok(());
                    }
                    return Err(unanswered("its choices could not be read"));
                };
                if typed == TRUST_KEYS_MAX {
                    return Err(unanswered("its highlight would not stay on yes"));
                }
                if fallback::person_drives(&crate::lease::read_lease(&self.state, holder)) {
                    note(&dialog.content, None, fallback::WHY_PERSON_DRIVES);
                    return Err(unanswered("a person drives it"));
                }
                let keys = agent_driver.answer_trust(&dialog);
                let outcome = fallback::send_if_unchanged(
                    read,
                    |lines| agent_driver.trust_dialog(lines),
                    &dialog,
                    || self.type_keys(holder, std::slice::from_ref(&keys)),
                )?;
                match outcome {
                    fallback::Recheck::Sent => {
                        note(
                            &dialog.content,
                            Some(fallback::key_names(&keys.bytes)),
                            "started with --trust and the folder was not trusted through the agent's config",
                        );
                        typed += 1;
                    }
                    fallback::Recheck::Changed => {
                        note(&dialog.content, None, fallback::WHY_CHANGED)
                    }
                    fallback::Recheck::Unreadable(_) => {
                        note(&dialog.content, None, fallback::WHY_UNREADABLE)
                    }
                }
                still_since = None;
            }
            std::thread::sleep(TRUST_POLL);
        }
        Err(unanswered("it did not settle on an answer in time"))
    }

    /// Marks the session's folder trusted in the agent's own config before it starts, so it never draws its
    /// trust prompt; the guarded screen answer stays for when that cannot be written.
    fn pretrust_folder(&self, cwd: &Path, branch: &str, holder: &str) {
        let config = crate::claude::user_config_path(&self.home);
        let line = match crate::claude::accept_folder_trust(&config, cwd) {
            Ok(folder) => {
                json!({"event": "folder_trusted", "holder": holder, "folder": folder, "via": "agent_config"})
            }
            Err(error) => {
                eprintln!("agent: could not mark {} trusted: {error}", cwd.display());
                json!({"event": "folder_trusted", "holder": holder, "folder": cwd, "via": "none", "why": error.to_string()})
            }
        };
        if let Err(error) = crate::lease::log(&self.state, &self.project, branch, line) {
            eprintln!("agent: log the trust of {holder}: {error}");
        }
    }

    /// Waits for a new session's first hook. The agent asks to trust a folder it has not seen before it runs
    /// any: `trust` answers yes, otherwise the session is reported as waiting on it.
    fn await_first_hook(
        &self,
        holder: &str,
        session_id: &str,
        branch: &str,
        trust: bool,
    ) -> Result<(), DriveError> {
        let deadline = std::time::Instant::now() + FIRST_HOOK_WAIT;
        let mut answered = false;
        while std::time::Instant::now() < deadline {
            let state = read_state(&self.state, session_id).map(|record| record.state);
            if state
                .as_deref()
                .is_some_and(|state| state != STARTING_STATE && state != NEEDS_TRUST_STATE)
                || !self.alive(holder)
            {
                return Ok(());
            }
            if !answered && self.shows_trust_prompt(holder, crate::identity::CLAUDE_DRIVER) {
                if !trust {
                    self.mark_needs_trust(holder, session_id, branch);
                    return Err(DriveError::NeedsTrust(format!(
                        "{holder} waits at the agent's \"trust this folder\" prompt: start it again with --trust to answer yes, or open it in Pomelo (or pom agent takeover) and answer it there"
                    )));
                }
                if let Err(error) = self.answer_trust(holder, session_id, branch) {
                    if matches!(error, DriveError::NeedsTrust(_)) {
                        self.mark_needs_trust(holder, session_id, branch);
                    }
                    return Err(error);
                }
                answered = true;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(())
    }

    /// The turns of a session `select` picks.
    pub fn read(
        &self,
        view: &SessionView,
        select: impl Fn(u64) -> bool,
        full: bool,
    ) -> Result<Vec<RecordedTurn>, DriveError> {
        if view.session_id.is_empty() {
            return Err(DriveError::NotFound(format!(
                "{} has not reported a session yet (started before this version, or still starting)",
                view.handle
            )));
        }
        if let Some(record) = read_state(&self.state, &view.session_id) {
            let just_ended = record.turn_state == TurnState::Ended
                && now_ms().saturating_sub(record.event_ms) < SETTLE_CAP.as_millis() as u64;
            if just_ended && !record.transcript.is_empty() {
                settle_transcript(Path::new(&record.transcript), SETTLE_QUIET, SETTLE_CAP);
            }
        }
        Ok(read_turns(
            &self.state,
            &view.session_id,
            select,
            view.alive,
            full,
        ))
    }

    /// New events of a workspace's sessions since `seen` (event count per session), as `watch` lines.
    pub fn new_events(
        &self,
        branch: &str,
        seen: &mut std::collections::HashMap<String, usize>,
    ) -> Vec<Value> {
        let mut lines = Vec::new();
        for entry in workspace_sessions(&self.state, &self.project, branch) {
            let all: Vec<EventLine> = events(&self.state, &entry.session_id);
            let from = seen.get(&entry.session_id).copied().unwrap_or(0);
            for line in all.iter().skip(from) {
                lines.push(watch_line(branch, &entry.role, &entry.session_id, line));
            }
            seen.insert(entry.session_id.clone(), all.len());
        }
        // Lease changes and messages between sessions belong to the workspace, not to one session.
        let log = crate::lease::log_lines(&self.state, &self.project, branch);
        let from = seen.get(WORKSPACE_LOG_KEY).copied().unwrap_or(0);
        for line in log.iter().skip(from) {
            let mut line = line.clone();
            line["schema"] = json!(SCHEMA);
            lines.push(line);
        }
        seen.insert(WORKSPACE_LOG_KEY.to_string(), log.len());
        lines.sort_by_key(|line| line.get("t_ms").and_then(Value::as_u64).unwrap_or(0));
        lines
    }
}

pub fn watch_line(branch: &str, role: &str, session_id: &str, line: &EventLine) -> Value {
    let mut value = json!({
        "schema": SCHEMA,
        "t_ms": line.t_ms,
        "workspace": branch,
        "role": role,
        "session_id": session_id,
        "event": line.event,
        "state": line.state,
        "turn": line.turn,
    });
    let reason = crate::sessions::reason_for(&line.event, &line.state);
    value["reason"] = json!(reason);
    value["running"] = json!(turn_running(&line.state, reason));
    value["phase"] = json!(phase(&line.state));
    if let Some(tool) = &line.tool {
        value["tool"] = json!(tool);
    }
    if !line.detail.is_null() {
        value["detail"] = line.detail.clone();
    }
    value
}

fn role_is_valid(role: &str) -> bool {
    role.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && role
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !role.starts_with("raw")
}

/// An extra MCP config may add servers, never replace pom's own.
fn check_extra_mcp(extra: &str) -> Result<(), DriveError> {
    let text = if extra.trim_start().starts_with('{') {
        extra.to_string()
    } else {
        std::fs::read_to_string(extra)
            .map_err(|error| DriveError::Invalid(format!("--extra-mcp-config {extra}: {error}")))?
    };
    let config: Value = serde_json::from_str(&text)
        .map_err(|error| DriveError::Invalid(format!("--extra-mcp-config is not JSON: {error}")))?;
    let servers = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or_else(|| DriveError::Invalid("--extra-mcp-config has no mcpServers".into()))?;
    if servers.contains_key(MCP_SERVER) {
        return Err(DriveError::Invalid(format!(
            "--extra-mcp-config may not define a server named {MCP_SERVER:?}: it is Pomelo's own"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::caller::Caller;
    use crate::gate::{Limits, Workspace};
    use crate::hooks::AgentState;
    use crate::identity::Identity;
    use crate::sessions::{record_event, SessionEvent};

    fn drive(temp: &Path) -> Drive {
        Drive {
            state: StateDir::new(temp.join("state")),
            holders: SocketDir::new(temp.join("holders")),
            project: "myproject".into(),
            home: temp.join("home"),
            binary: PathBuf::from("/bin/false"),
            tool_path: "/usr/bin:/bin".into(),
            limits: Default::default(),
            agents: Default::default(),
        }
    }

    fn record(
        drive: &Drive,
        branch: &str,
        role: &str,
        holder: &str,
        session: &str,
        event: &str,
        state: AgentState,
    ) {
        let identity = Identity {
            holder: holder.into(),
            role: role.into(),
            project: "myproject".into(),
            branch: branch.into(),
            driver: "claude".into(),
        };
        let event = SessionEvent {
            session_id: session.into(),
            event: event.into(),
            state: Some(state),
            ..SessionEvent::default()
        };
        record_event(&drive.state, &identity, &event).expect("event");
    }

    #[test]
    fn ls_shows_one_workspaces_sessions_with_their_own_state() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        record(
            &drive,
            "feat-login",
            "claude",
            "ws-myproject-feat-login-claude-raw",
            "a",
            "UserPromptSubmit",
            AgentState::Thinking,
        );
        record(
            &drive,
            "feat-login",
            "reviewer",
            "ws-myproject-feat-login-claude-reviewer",
            "b",
            "Stop",
            AgentState::Idle,
        );
        record(
            &drive,
            "feat-signup",
            "claude",
            "ws-myproject-feat-signup-claude-raw",
            "c",
            "Stop",
            AgentState::Idle,
        );
        let views = drive.list("feat-login");
        let summary: Vec<(String, String, bool)> = views
            .iter()
            .map(|view| (view.handle.clone(), view.state.clone(), view.alive))
            .collect();
        assert_eq!(
            summary,
            [
                ("feat-login/claude".to_string(), "died".to_string(), false),
                ("feat-login/reviewer".to_string(), "idle".to_string(), false),
            ],
            "a working session whose holder is gone reads as died; feat-signup's is not listed"
        );
        assert_eq!(views[0].turn, 1);
    }

    #[test]
    fn a_handle_of_another_workspace_or_an_unknown_role_is_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        record(
            &drive,
            "feat-login",
            "claude",
            "ws-myproject-feat-login-claude-raw",
            "a",
            "Stop",
            AgentState::Idle,
        );
        let workspace = Workspace {
            project: "myproject".into(),
            branch: "feat-login".into(),
        };
        let gate =
            Gate::open(&drive.state, Caller::Operator, workspace, Limits::OPERATOR).expect("gate");
        assert_eq!(drive.resolve(&gate, "claude").expect("own").session_id, "a");
        assert_eq!(
            drive
                .resolve(&gate, "feat-login/claude")
                .expect("own")
                .session_id,
            "a"
        );
        assert!(matches!(
            drive.resolve(&gate, "feat-signup/claude"),
            Err(DriveError::Refused(Refusal::OtherWorkspace { .. }))
        ));
        assert!(matches!(
            drive.resolve(&gate, "ws-myproject-feat-signup-claude-raw"),
            Err(DriveError::Refused(Refusal::OtherWorkspace { .. }))
        ));
        assert!(matches!(
            drive.resolve(&gate, "reviewer"),
            Err(DriveError::NotFound(_))
        ));
    }

    #[test]
    fn an_extra_mcp_config_cannot_replace_poms_server() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        let workspace = Workspace {
            project: "myproject".into(),
            branch: "feat-login".into(),
        };
        let gate =
            Gate::open(&drive.state, Caller::Operator, workspace, Limits::OPERATOR).expect("gate");
        let options = LaunchOptions {
            extra_mcp_config: Some(r#"{"mcpServers":{"pom":{"command":"evil"}}}"#.into()),
            ..LaunchOptions::default()
        };
        let refused = drive.start(&gate, temp.path(), false, "reviewer", true, &options);
        assert!(
            matches!(refused, Err(DriveError::Invalid(message)) if message.contains("Pomelo's own"))
        );
        let bad_role = drive.start(
            &gate,
            temp.path(),
            false,
            "claude",
            true,
            &LaunchOptions::default(),
        );
        assert!(matches!(bad_role, Err(DriveError::Invalid(_))));
    }

    #[test]
    fn only_a_person_starts_an_agent_as_their_own() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        let workspace = Workspace {
            project: "myproject".into(),
            branch: "feat-login".into(),
        };
        let caller = Caller::Agent {
            holder: "ws-myproject-feat-login-claude-lead".into(),
        };
        let gate = Gate::open(&drive.state, caller, workspace, Limits::AGENT).expect("gate");
        let options = LaunchOptions {
            for_person: true,
            ..LaunchOptions::default()
        };
        let refused = drive.start(&gate, temp.path(), false, "reviewer", true, &options);
        assert!(
            matches!(refused, Err(DriveError::Invalid(message)) if message.contains("only a person"))
        );
    }

    #[test]
    fn watch_lines_carry_the_workspace_and_role_and_arrive_once() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = drive(temp.path());
        record(
            &drive,
            "feat-login",
            "claude",
            "ws-myproject-feat-login-claude-raw",
            "a",
            "UserPromptSubmit",
            AgentState::Thinking,
        );
        let mut seen = std::collections::HashMap::new();
        let first = drive.new_events("feat-login", &mut seen);
        assert_eq!(first.len(), 1);
        assert_eq!(
            (
                first[0]["workspace"].as_str(),
                first[0]["role"].as_str(),
                first[0]["event"].as_str()
            ),
            (Some("feat-login"), Some("claude"), Some("UserPromptSubmit"))
        );
        record(
            &drive,
            "feat-login",
            "claude",
            "ws-myproject-feat-login-claude-raw",
            "a",
            "PreCompact",
            AgentState::Compacting,
        );
        let next = drive.new_events("feat-login", &mut seen);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0]["state"], "compacting");
        assert!(drive.new_events("feat-login", &mut seen).is_empty());
    }
}
