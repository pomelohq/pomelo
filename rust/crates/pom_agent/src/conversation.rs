//! Driving a turn: send one prompt, wait for the turn to end, answer a pending approval, interrupt, and move
//! a session between an orchestrator and a person. The turn ends on the agent's Stop hook; the transcript
//! only stands in when no Stop came, once its last reply has said `end_turn` and nothing was written for a
//! moment, because a reply split over several entries carries `end_turn` on each of them.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::caller::Caller;
use crate::drive::{Drive, DriveError, SessionView};
use crate::driver::driver;
use crate::gate::{ApproveScope, Gate, Refusal};
use crate::identity::Identity;
use crate::lease::{log, read_lease, set_lease, Lease, LeaseClass};
use crate::policy::{write_answer, Answer};
use crate::sessions::{
    events, now_ms, read_state, record_event, session_dir, with_lock, SessionEvent, TurnState,
};
use crate::turns::{turn_meta, write_turn_meta, Origin, TurnMeta};

const POLL: Duration = Duration::from_millis(200);
/// How long a submitted prompt may take to show up as a new turn before Enter is pressed once more.
const SUBMIT_RETRY_AFTER: Duration = Duration::from_secs(2);
const SUBMIT_GIVE_UP: Duration = Duration::from_secs(10);
/// Quiet time after a transcript `end_turn` before it counts as the end of a turn no Stop hook closed.
const END_TURN_QUIET: Duration = Duration::from_millis(800);
/// A holder must be missing this many checks in a row before its session counts as dead.
const DEAD_AFTER_MISSES: u32 = 3;
const INTERRUPT_SETTLE: Duration = Duration::from_secs(3);

pub const NOT_SUBMITTED_EXIT: i32 = 5;
pub const TIMEOUT_EXIT: i32 = 2;
pub const AWAITING_INPUT_EXIT: i32 = 3;
pub const DIED_EXIT: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitUntil {
    Idle,
    AwaitingInput,
    TurnEnd,
}

impl WaitUntil {
    pub fn parse(text: &str) -> Option<WaitUntil> {
        Some(match text {
            "idle" => WaitUntil::Idle,
            "awaiting_input" => WaitUntil::AwaitingInput,
            "turn-end" | "turn_end" => WaitUntil::TurnEnd,
            _ => return None,
        })
    }
}

/// How a wait ended; `exit_code` is what `pom agent wait` exits with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WaitOutcome {
    pub handle: String,
    /// `turn-end`, `idle`, `awaiting_input`, `timeout` or `died`.
    pub reached: String,
    pub state: String,
    pub turn: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// For a dead session: `killed`, `user_exited`, `crashed` or `auth_failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(skip)]
    pub exit_code: i32,
}

fn identity_of(drive: &Drive, view: &SessionView) -> Identity {
    Identity {
        holder: view.holder.clone(),
        role: view.role.clone(),
        project: drive.project.clone(),
        branch: view.workspace.clone(),
        driver: view.driver.clone(),
    }
}

impl Drive {
    fn record(
        &self,
        view: &SessionView,
        event: &str,
        state: Option<crate::hooks::AgentState>,
        detail: Value,
    ) {
        if view.session_id.is_empty() {
            return;
        }
        let event = SessionEvent {
            session_id: view.session_id.clone(),
            event: event.into(),
            state,
            detail,
            ..SessionEvent::default()
        };
        if let Err(error) = record_event(&self.state, &identity_of(self, view), &event) {
            eprintln!("agent: record {}: {error}", view.handle);
        }
    }

    /// The depth of the turn the calling agent is in, 0 once that turn is over or for a person.
    fn caller_depth(&self, caller: &Caller, branch: &str) -> u64 {
        let Some(holder) = caller.holder() else {
            return 0;
        };
        let Some(own) = self
            .list(branch)
            .into_iter()
            .find(|view| view.holder == holder)
        else {
            return 0;
        };
        match read_state(&self.state, &own.session_id) {
            Some(record) if record.turn_state == TurnState::Running => {
                turn_meta(&self.state, &own.session_id, record.turn).depth
            }
            _ => 0,
        }
    }

    /// Takes the lease for the caller when it starts a session or asks to take one over.
    pub fn take_lease(&self, gate: &Gate<'_>, view: &SessionView) -> Result<Lease, DriveError> {
        let lease = read_lease(&self.state, &view.holder);
        if lease.class != LeaseClass::Human
            && lease.allows(gate.caller()).is_ok()
            && !lease.token.is_empty()
        {
            return Ok(lease);
        }
        if gate.caller().is_agent() && lease.class == LeaseClass::Human {
            return Err(Refusal::LeaseHeld {
                by: "a person".into(),
            }
            .into());
        }
        set_lease(
            &self.state,
            &self.holders,
            &self.project,
            &view.workspace,
            &view.holder,
            &view.role,
            Lease::for_caller(gate.caller()),
        )
        .map_err(|error| DriveError::Failed(format!("could not take the lease: {error}")))
    }

    /// Sends one user turn and returns its number once the agent has taken it.
    pub fn send(
        &self,
        gate: &Gate<'_>,
        view: &SessionView,
        text: &str,
        queue: bool,
        take: bool,
        timeout: Option<Duration>,
    ) -> Result<u64, DriveError> {
        if text.trim().is_empty() {
            return Err(DriveError::Invalid("nothing to send".into()));
        }
        gate.drivable_target(&view.holder)?;
        let agent_driver = driver(&view.driver).ok_or_else(|| Refusal::NotDrivable {
            role: view.role.clone(),
        })?;
        if view.session_id.is_empty() {
            return Err(DriveError::NotFound(format!(
                "{} has not reported a session yet; it may still be starting",
                view.handle
            )));
        }
        if take && gate.caller().is_agent() {
            return Err(DriveError::Invalid(
                "only a person or an orchestrator may take a session over".into(),
            ));
        }
        let lease = if take {
            self.take_lease(gate, view)?
        } else {
            let lease = read_lease(&self.state, &view.holder);
            lease.allows(gate.caller())?;
            lease
        };
        let current = read_state(&self.state, &view.session_id)
            .map(|record| record.state)
            .unwrap_or_else(|| view.state.clone());
        let depth = gate.send(
            &view.holder,
            &current,
            queue,
            self.caller_depth(gate.caller(), &view.workspace),
        )?;
        let queue_lock = session_dir(&self.state, &view.session_id).join("queue.lock");
        with_lock(&queue_lock, |_| {
            if queue {
                self.wait_until_idle(view, timeout)?;
            }
            let before = read_state(&self.state, &view.session_id).map_or(view.turn, |record| record.turn);
            let origin = match gate.caller() {
                Caller::Agent { .. } => Origin::Agent,
                Caller::Operator => Origin::Orchestrator,
            };
            write_turn_meta(
                &self.state,
                &view.session_id,
                before + 1,
                &TurnMeta {
                    origin,
                    driven_by: gate.caller().holder().map(str::to_string),
                    depth,
                    sent_ms: now_ms(),
                },
            )?;
            let mut connection = pom_ptyhost::connect_writer(&self.holders, &view.holder)?;
            if lease.class != LeaseClass::Human {
                connection.claim(&lease.token)?;
            }
            for keys in agent_driver.submit(text) {
                connection.input(&keys.bytes)?;
                std::thread::sleep(Duration::from_millis(keys.pause_ms));
            }
            let started = Instant::now();
            let mut retried = false;
            loop {
                let turn = read_state(&self.state, &view.session_id).map_or(before, |record| record.turn);
                if turn > before {
                    log(
                        &self.state,
                        &self.project,
                        &view.workspace,
                        json!({"event": "message", "from": gate.caller().holder().unwrap_or("operator"), "to": view.holder, "role": view.role, "turn": turn, "depth": depth, "origin": origin, "chars": text.chars().count()}),
                    )?;
                    return Ok(Ok(turn));
                }
                if !retried && started.elapsed() > SUBMIT_RETRY_AFTER {
                    connection.input(b"\r")?;
                    retried = true;
                }
                if started.elapsed() > SUBMIT_GIVE_UP {
                    return Ok(Err(DriveError::NotSubmitted(view.handle.clone())));
                }
                std::thread::sleep(POLL);
            }
        })
        .map_err(|error| DriveError::Failed(format!("could not send to {}: {error}", view.handle)))?
    }

    fn wait_until_idle(
        &self,
        view: &SessionView,
        timeout: Option<Duration>,
    ) -> std::io::Result<()> {
        let started = Instant::now();
        loop {
            let idle = read_state(&self.state, &view.session_id)
                .is_some_and(|record| record.state == "idle");
            if idle {
                return Ok(());
            }
            if timeout.is_some_and(|timeout| started.elapsed() > timeout) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the session did not become idle",
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Blocks until the session reaches `until` (for turn `turn`, by default the current one), asks for a
    /// permission, dies or the wait times out. It reads hook state; it never looks at the screen.
    pub fn wait(
        &self,
        view: &SessionView,
        until: WaitUntil,
        turn: Option<u64>,
        timeout: Option<Duration>,
    ) -> WaitOutcome {
        let started = Instant::now();
        let mut misses = 0;
        let start_record = read_state(&self.state, &view.session_id);
        let target = turn.unwrap_or_else(|| {
            start_record
                .as_ref()
                .map_or(view.turn, |record| record.turn)
        });
        let mut quiet_since: Option<(u64, Instant)> = None;
        loop {
            let record = read_state(&self.state, &view.session_id);
            let (state, current, turn_state) = record
                .as_ref()
                .map_or((view.state.clone(), view.turn, TurnState::None), |record| {
                    (record.state.clone(), record.turn, record.turn_state)
                });
            let outcome = |reached: &str,
                           stop_reason: Option<&str>,
                           cause: Option<String>,
                           exit_code: i32| WaitOutcome {
                handle: view.handle.clone(),
                reached: reached.into(),
                state: state.clone(),
                turn: current,
                stop_reason: stop_reason.map(str::to_string),
                cause,
                exit_code,
            };
            if self.holders.holder_alive(&view.holder) {
                misses = 0;
            } else {
                misses += 1;
                if misses >= DEAD_AFTER_MISSES {
                    return outcome(
                        "died",
                        Some("died"),
                        Some(self.death_cause(view)),
                        DIED_EXIT,
                    );
                }
            }
            match until {
                WaitUntil::TurnEnd => {
                    if current > target
                        || (current == target
                            && matches!(turn_state, TurnState::Ended | TurnState::Cancelled))
                    {
                        let reason = if current == target && turn_state == TurnState::Cancelled {
                            "cancelled"
                        } else {
                            "end_turn"
                        };
                        return outcome("turn-end", Some(reason), None, 0);
                    }
                    if let Some(record) = record
                        .as_ref()
                        .filter(|record| record.turn_state == TurnState::Running)
                    {
                        if transcript_says_end_turn(Path::new(&record.transcript), &mut quiet_since)
                        {
                            return outcome("turn-end", Some("end_turn"), None, 0);
                        }
                    }
                }
                WaitUntil::Idle if state == "idle" => return outcome("idle", None, None, 0),
                WaitUntil::AwaitingInput if state == "awaiting_input" => {
                    return outcome("awaiting_input", None, None, 0)
                }
                _ => {}
            }
            if state == "awaiting_input" && until != WaitUntil::AwaitingInput {
                return outcome("awaiting_input", None, None, AWAITING_INPUT_EXIT);
            }
            if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
                return outcome("timeout", Some("timeout"), None, TIMEOUT_EXIT);
            }
            std::thread::sleep(POLL);
        }
    }

    /// Why a session's holder is gone.
    pub fn death_cause(&self, view: &SessionView) -> String {
        let lines = events(&self.state, &view.session_id);
        if lines.iter().rev().any(|line| line.event == "Killed") {
            return "killed".into();
        }
        let crash = self.holders.crash_info(&view.holder);
        let output = crash
            .as_ref()
            .map(|crash| String::from_utf8_lossy(&crash.output).to_lowercase())
            .unwrap_or_default();
        let auth = [
            "invalid api key",
            "authentication_error",
            "please run /login",
            "oauth token",
            "401",
        ]
        .iter()
        .any(|marker| output.contains(marker));
        if let Some(end) = lines.iter().rev().find(|line| line.event == "SessionEnd") {
            return if end.detail["reason"] == "logout" || auth {
                "auth_failed".into()
            } else {
                "user_exited".into()
            };
        }
        if auth {
            return "auth_failed".into();
        }
        match crash {
            Some(crash) if !crash.crashed => "user_exited".into(),
            _ => "crashed".into(),
        }
    }

    /// Stops the current turn with the agent's own interrupt key.
    pub fn interrupt(&self, gate: &Gate<'_>, view: &SessionView) -> Result<(), DriveError> {
        gate.drivable_target(&view.holder)?;
        let lease = read_lease(&self.state, &view.holder);
        lease.allows(gate.caller())?;
        let agent_driver = driver(&view.driver).ok_or_else(|| Refusal::NotDrivable {
            role: view.role.clone(),
        })?;
        let mut connection =
            pom_ptyhost::connect_writer(&self.holders, &view.holder).map_err(|error| {
                DriveError::Failed(format!("could not reach {}: {error}", view.handle))
            })?;
        if lease.class != LeaseClass::Human {
            connection
                .claim(&lease.token)
                .map_err(|error| DriveError::Failed(error.to_string()))?;
        }
        connection
            .input(&agent_driver.interrupt())
            .map_err(|error| DriveError::Failed(error.to_string()))?;
        let started = Instant::now();
        let before = read_state(&self.state, &view.session_id).map(|record| record.event_ms);
        while started.elapsed() < INTERRUPT_SETTLE {
            std::thread::sleep(POLL);
            if read_state(&self.state, &view.session_id).map(|record| record.event_ms) != before {
                break;
            }
        }
        self.record(
            view,
            "Interrupted",
            Some(crate::hooks::AgentState::Idle),
            json!({"stop_reason": "cancelled"}),
        );
        Ok(())
    }

    /// Answers a pending approval of `view`'s session.
    pub fn approve(
        &self,
        gate: &Gate<'_>,
        view: &SessionView,
        request: &str,
        scope: ApproveScope,
    ) -> Result<(), DriveError> {
        gate.approve(&view.holder, scope)?;
        if view.session_id.is_empty() {
            return Err(DriveError::NotFound(format!(
                "{} has no session yet",
                view.handle
            )));
        }
        let pending = events(&self.state, &view.session_id)
            .iter()
            .any(|line| line.event == "permission_request" && line.detail["request"] == request);
        if !pending {
            return Err(DriveError::NotFound(format!(
                "no permission request {request} in {}",
                view.handle
            )));
        }
        let decided_by = gate.caller().holder().unwrap_or("operator").to_string();
        let answer = Answer {
            allow: scope != ApproveScope::Deny,
            decided_by: decided_by.clone(),
            reason: String::new(),
            always: scope == ApproveScope::Always,
        };
        write_answer(&self.state, &view.session_id, request, &answer)
            .map_err(|error| DriveError::Failed(error.to_string()))?;
        log(
            &self.state,
            &self.project,
            &view.workspace,
            json!({"event": "permission_answer", "role": view.role, "request": request, "scope": scope, "decided_by": decided_by}),
        )
        .map_err(|error| DriveError::Failed(error.to_string()))
    }

    /// A person takes the session over: the lease moves to them and every client may type again. When the
    /// session is not running, it is reopened on its conversation in a holder for them to attach to.
    pub fn takeover(
        &self,
        gate: &Gate<'_>,
        view: &SessionView,
        cwd: &Path,
        settings: Option<&str>,
        mcp_config: Option<&str>,
    ) -> Result<SessionView, DriveError> {
        if gate.caller().is_agent() {
            return Err(DriveError::Invalid(
                "an agent cannot take a session over for a person".into(),
            ));
        }
        gate.target(&view.holder)?;
        if !self.holders.holder_alive(&view.holder) {
            if view.session_id.is_empty() {
                return Err(DriveError::NotFound(format!(
                    "{} has no conversation to resume",
                    view.handle
                )));
            }
            self.resume_for_person(view, cwd, settings, mcp_config)?;
        }
        let before = read_lease(&self.state, &view.holder);
        let mut person = Lease::person();
        if before.class != LeaseClass::Human {
            person.previous = Some(Box::new(before));
        }
        set_lease(
            &self.state,
            &self.holders,
            &self.project,
            &view.workspace,
            &view.holder,
            &view.role,
            person,
        )
        .map_err(|error| DriveError::Failed(error.to_string()))?;
        Ok(view.clone())
    }

    /// Hands a session a person took over back to whoever drove it before.
    pub fn release(&self, gate: &Gate<'_>, view: &SessionView) -> Result<Lease, DriveError> {
        if gate.caller().is_agent() {
            return Err(DriveError::Invalid(
                "an agent cannot release a person's session".into(),
            ));
        }
        gate.target(&view.holder)?;
        let lease = read_lease(&self.state, &view.holder);
        let next = match lease.previous {
            Some(previous) => *previous,
            None => Lease::for_caller(&Caller::Operator),
        };
        set_lease(
            &self.state,
            &self.holders,
            &self.project,
            &view.workspace,
            &view.holder,
            &view.role,
            next,
        )
        .map_err(|error| DriveError::Failed(error.to_string()))
    }

    fn resume_for_person(
        &self,
        view: &SessionView,
        cwd: &Path,
        settings: Option<&str>,
        mcp_config: Option<&str>,
    ) -> Result<(), DriveError> {
        let claude = crate::launch::resolve_claude(&self.home, &self.tool_path);
        let ours = crate::claude::mcp_config_json(&self.state, &self.binary, &view.workspace);
        let mut mcp = crate::launch::shell_quote(&ours);
        if let Some(extra) = mcp_config {
            mcp.push(' ');
            mcp.push_str(&crate::launch::shell_quote(extra));
        }
        let hooks = crate::hooks::session_hooks(&self.state, &self.binary).unwrap_or(Value::Null);
        let mut merged: Value = settings
            .map(|settings| {
                let text = if settings.trim_start().starts_with('{') {
                    settings.to_string()
                } else {
                    std::fs::read_to_string(settings).unwrap_or_default()
                };
                serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
            })
            .unwrap_or_else(|| json!({}));
        merged["hooks"] = hooks;
        let identity = identity_of(self, view);
        let script = format!(
            "export PATH={path}; export TERM=xterm-256color COLORTERM=truecolor {identity}; unsetopt monitor 2>/dev/null; cd {cwd} && exec {claude} --resume {id} --mcp-config {mcp} --settings {settings}",
            path = crate::launch::shell_quote(&self.tool_path),
            identity = identity.exports(),
            cwd = crate::launch::shell_quote(&cwd.to_string_lossy()),
            claude = crate::launch::shell_quote(&claude),
            id = crate::launch::shell_quote(&view.session_id),
            settings = crate::launch::shell_quote(&merged.to_string()),
        );
        let argv = vec!["zsh".to_string(), "-c".to_string(), script];
        pom_ptyhost::spawn_holder(
            &self.holders,
            &pom_ptyhost::SpawnRequest {
                binary: &self.binary,
                name: &view.holder,
                cwd,
                cols: 160,
                rows: 48,
                argv: &argv,
                env: &[],
            },
        )
        .map_err(|error| DriveError::Failed(format!("could not reopen {}: {error}", view.handle)))
    }
}

/// Whether the transcript's last reply ended its turn and nothing has been written for a moment.
fn transcript_says_end_turn(path: &Path, quiet_since: &mut Option<(u64, Instant)>) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let length = meta.len();
    match quiet_since {
        Some((seen, since)) if *seen == length => {
            if since.elapsed() < END_TURN_QUIET {
                return false;
            }
        }
        _ => {
            *quiet_since = Some((length, Instant::now()));
            return false;
        }
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|entry| {
            matches!(
                entry.get("type").and_then(Value::as_str),
                Some("assistant" | "user")
            )
        })
        .is_some_and(|entry| {
            entry.get("type").and_then(Value::as_str) == Some("assistant")
                && entry
                    .get("message")
                    .and_then(|message| message.get("stop_reason"))
                    .and_then(Value::as_str)
                    == Some("end_turn")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hooks::AgentState;
    use pom_paths::StateDir;
    use pom_ptyhost::SocketDir;

    #[test]
    fn a_transcript_end_turn_ends_the_turn_only_after_it_stays_quiet() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("t.jsonl");
        let reply =
            "{\"type\":\"assistant\",\"message\":{\"content\":[],\"stop_reason\":\"end_turn\"}}\n";
        std::fs::write(&path, reply).expect("transcript");
        let mut quiet = None;
        assert!(
            !transcript_says_end_turn(&path, &mut quiet),
            "first sight starts the quiet clock"
        );
        std::thread::sleep(END_TURN_QUIET + Duration::from_millis(50));
        assert!(transcript_says_end_turn(&path, &mut quiet));

        let split = format!("{reply}{reply}");
        std::fs::write(&path, &split).expect("more of the reply");
        assert!(
            !transcript_says_end_turn(&path, &mut quiet),
            "a new line restarts the clock"
        );
        std::fs::write(&path, format!("{split}{{\"type\":\"user\",\"message\":{{\"content\":[{{\"type\":\"tool_result\"}}]}}}}\n")).expect("tool result");
        std::thread::sleep(END_TURN_QUIET + Duration::from_millis(50));
        transcript_says_end_turn(&path, &mut quiet);
        std::thread::sleep(END_TURN_QUIET + Duration::from_millis(50));
        assert!(
            !transcript_says_end_turn(&path, &mut quiet),
            "the last entry is not a reply"
        );
    }

    #[test]
    fn a_dead_session_says_why() {
        let temp = tempfile::tempdir().expect("temp");
        let drive = Drive {
            state: StateDir::new(temp.path().join("state")),
            holders: SocketDir::new(temp.path().join("s")),
            project: "myproject".into(),
            home: temp.path().into(),
            binary: "/bin/false".into(),
            tool_path: String::new(),
        };
        let view = |session: &str, holder: &str| SessionView {
            handle: format!("feat-login/{session}"),
            workspace: "feat-login".into(),
            role: session.into(),
            holder: holder.into(),
            driver: "claude".into(),
            session_id: session.into(),
            alive: false,
            drivable: true,
            state: "thinking".into(),
            turn: 1,
            last_event: String::new(),
            last_event_age_s: 0,
            stale: false,
        };
        let record = |view: &SessionView, event: &str, detail: Value| {
            drive.record(view, event, Some(AgentState::Idle), detail);
        };
        let killed = view("killed", "ws-myproject-feat-login-claude-a");
        record(&killed, "Killed", Value::Null);
        assert_eq!(drive.death_cause(&killed), "killed");
        let exited = view("exited", "ws-myproject-feat-login-claude-b");
        record(
            &exited,
            "SessionEnd",
            json!({"reason": "prompt_input_exit"}),
        );
        assert_eq!(drive.death_cause(&exited), "user_exited");
        let logout = view("logout", "ws-myproject-feat-login-claude-c");
        record(&logout, "SessionEnd", json!({"reason": "logout"}));
        assert_eq!(drive.death_cause(&logout), "auth_failed");
        let crashed = view("crashed", "ws-myproject-feat-login-claude-d");
        std::fs::create_dir_all(temp.path().join("s")).expect("dir");
        std::fs::write(
            drive.holders.crash_log(&crashed.holder),
            "CRASH\tclaude - exited: 1\nsegfault\n",
        )
        .expect("crash");
        assert_eq!(drive.death_cause(&crashed), "crashed");
        let unauthorized = view("auth", "ws-myproject-feat-login-claude-e");
        std::fs::write(
            drive.holders.crash_log(&unauthorized.holder),
            "CRASH\tclaude - exited: 1\nInvalid API key - Please run /login\n",
        )
        .expect("crash");
        assert_eq!(drive.death_cause(&unauthorized), "auth_failed");
    }
}
