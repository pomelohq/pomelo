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
use crate::fallback;
use crate::gate::{ApproveScope, Gate, Refusal};
use crate::identity::Identity;
use crate::lease::{log, read_lease, set_lease, Lease, LeaseClass};
use crate::permission::{
    answer_taken, is_policy_request, pending_permission, PermissionPrompt, ANSWER_BY_HOOK,
    ANSWER_BY_POLICY, ANSWER_BY_TERMINAL,
};
use crate::policy::{write_answer, Answer};
use crate::screen::rendered_lines;
use crate::sessions::{
    events, mark_submitted, now_ms, read_state, record_dialog_answer, record_event, session_dir,
    with_lock, SessionEvent, TurnState,
};
use crate::turns::{turn_meta, turn_spans, turn_was_sent, write_turn_meta, Origin, TurnMeta};

const POLL: Duration = Duration::from_millis(200);
/// How long a submitted prompt may take to show up as a new turn before Enter is pressed once more.
const SUBMIT_RETRY_AFTER: Duration = Duration::from_secs(2);
const SUBMIT_GIVE_UP: Duration = Duration::from_secs(10);
/// Quiet time after a transcript `end_turn` before it counts as the end of a turn no Stop hook closed.
const END_TURN_QUIET: Duration = Duration::from_millis(800);
/// A holder must be missing this many checks in a row before its session counts as dead.
const DEAD_AFTER_MISSES: u32 = 3;
const INTERRUPT_SETTLE: Duration = Duration::from_secs(3);
/// How long an answer may take to reach a prompt pom holds.
const HOOK_TAKE_WAIT: Duration = Duration::from_secs(3);
const DIALOG_CLOSE_WAIT: Duration = Duration::from_secs(3);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(1);
/// How much of the screen the fallback log keeps when no dialog was matched.
const FALLBACK_SEEN_LINES: usize = 6;
/// How long `send --queue` waits for an operator that gave no timeout.
pub const QUEUE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub const NOT_SUBMITTED_EXIT: i32 = 5;
pub const TIMEOUT_EXIT: i32 = 2;
pub const AWAITING_INPUT_EXIT: i32 = 3;
pub const DIED_EXIT: i32 = 4;

/// On whose authority a caller types into a session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SendAs {
    /// As the holder of the session's lease, which must allow the caller.
    #[default]
    LeaseHolder,
    /// Takes the lease first (`--take`); a person's session goes to the caller.
    TakeOver,
    /// As the person who drives the session (`--as-person`), from another client; the lease stays theirs.
    Person,
}

/// What `answer_permission` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PermissionAnswer {
    pub handle: String,
    pub id: String,
    pub allow: bool,
    /// `hook`, `terminal` or `policy`.
    pub via: String,
}

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
    /// The turn a `turn-end` wait waited for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub for_turn: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// For a dead session: `killed`, `user_exited`, `crashed` or `auth_failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(skip)]
    pub exit_code: i32,
}

/// Whether the agent started on the turn sent at `sent_ms`, by its state or by any event since.
fn turn_began(state: &pom_paths::StateDir, session_id: &str, before: u64, sent_ms: u64) -> bool {
    if read_state(state, session_id)
        .is_some_and(|record| record.turn > before || record.turn_state == TurnState::Running)
        || pending_permission(state, session_id).is_some()
    {
        return true;
    }
    events(state, session_id).iter().any(|line| {
        line.t_ms >= sent_ms
            && !(line.event == "Notification"
                && line.notification.as_deref() == Some("idle_prompt"))
    })
}

/// Why the rendered screen makes one more Enter unsafe: a dialog would take it, or the box no longer holds
/// `text`, so the Enter was taken and the turn is only slow to show.
fn retry_blocker_on_screen(
    agent_driver: &dyn crate::driver::AgentDriver,
    lines: &[String],
    text: &str,
) -> Option<&'static str> {
    if agent_driver.shows_choices(lines) {
        return Some("a dialog is on its screen");
    }
    if !agent_driver.holds_unsent(lines, text) {
        return Some("its input box no longer shows the prompt");
    }
    None
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

    /// The lease a caller types under, refused when `send_as` does not give it the right to.
    pub fn typing_lease(
        &self,
        gate: &Gate<'_>,
        view: &SessionView,
        send_as: SendAs,
    ) -> Result<Lease, DriveError> {
        match send_as {
            SendAs::TakeOver => {
                if gate.caller().is_agent() {
                    return Err(DriveError::Invalid(
                        "only a person or an orchestrator may take a session over".into(),
                    ));
                }
                self.take_lease(gate, view)
            }
            SendAs::Person => {
                if gate.caller().is_agent() {
                    return Err(DriveError::Invalid(
                        "an agent cannot act as the person who drives a session".into(),
                    ));
                }
                let lease = read_lease(&self.state, &view.holder);
                if lease.class != LeaseClass::Human {
                    let by = crate::presence::driven_by(&self.state, &view.holder)
                        .unwrap_or_else(|| lease.by.clone());
                    return Err(Refusal::LeaseHeld { by }.into());
                }
                Ok(lease)
            }
            SendAs::LeaseHolder => {
                let lease = read_lease(&self.state, &view.holder);
                lease.allows(gate.caller())?;
                Ok(lease)
            }
        }
    }

    /// Sends one user turn and returns its number once the agent has taken it.
    pub fn send(
        &self,
        gate: &Gate<'_>,
        view: &SessionView,
        text: &str,
        queue: bool,
        send_as: SendAs,
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
        let lease = self.typing_lease(gate, view, send_as)?;
        let (current, reason) = read_state(&self.state, &view.session_id)
            .map(|record| record.status())
            .unwrap_or_else(|| (view.state.clone(), view.reason.clone()));
        let depth = gate.send(
            &view.holder,
            &current,
            reason.as_deref(),
            queue,
            self.caller_depth(gate.caller(), &view.workspace),
        )?;
        let queue_lock = session_dir(&self.state, &view.session_id).join("queue.lock");
        with_lock(&queue_lock, |_| {
            if queue {
                if let Err(error) = self.wait_until_idle(view, timeout.unwrap_or(QUEUE_TIMEOUT)) {
                    return Ok(Err(error));
                }
            }
            let before = read_state(&self.state, &view.session_id).map_or(view.turn, |record| record.turn);
            let origin = match (gate.caller(), send_as) {
                (Caller::Agent { .. }, _) => Origin::Agent,
                (Caller::Operator, SendAs::Person) => Origin::Human,
                (Caller::Operator, _) => Origin::Orchestrator,
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
            if self
                .screen(view)
                .is_some_and(|lines| agent_driver.shows_choices(&lines))
            {
                return Ok(Err(DriveError::Failed(format!(
                    "{} shows a dialog; answer it before sending, or a typed key could answer it",
                    view.handle
                ))));
            }
            let mut connection = pom_ptyhost::connect_writer(&self.holders, &view.holder)?;
            if lease.class != LeaseClass::Human {
                connection.claim(&lease.token)?;
            }
            let sent_ms = now_ms();
            for keys in agent_driver.submit(text) {
                connection.input(&keys.bytes)?;
                std::thread::sleep(Duration::from_millis(keys.pause_ms));
            }
            let started = Instant::now();
            let mut retried = false;
            let mut held_back = None;
            loop {
                let turn = read_state(&self.state, &view.session_id).map_or(before, |record| record.turn);
                if turn > before {
                    if let Err(error) = mark_submitted(&self.state, &identity_of(self, view), &view.session_id, turn) {
                        eprintln!("agent: record turn {turn} of {}: {error}", view.handle);
                    }
                    log(
                        &self.state,
                        &self.project,
                        &view.workspace,
                        json!({"event": "message", "from": sender(gate, send_as), "to": view.holder, "role": view.role, "turn": turn, "depth": depth, "origin": origin, "chars": text.chars().count()}),
                    )?;
                    return Ok(Ok(turn));
                }
                if !retried && started.elapsed() > SUBMIT_RETRY_AFTER {
                    if fallback::person_drives(&lease) {
                        held_back = Some(fallback::WHY_PERSON_DRIVES);
                        retried = true;
                    } else {
                        held_back = self.retry_enter(view, agent_driver, text, (before, sent_ms), &mut connection)?;
                        retried = held_back.is_none();
                    }
                }
                if started.elapsed() > SUBMIT_GIVE_UP {
                    if let Some(why) = held_back {
                        self.record_fallback(view, fallback::SUBMIT_RETRY, &self.screen_tail(view), None, why);
                    }
                    return Ok(Err(DriveError::NotSubmitted(view.handle.clone(), held_back)));
                }
                std::thread::sleep(POLL);
            }
        })
        .map_err(|error| DriveError::Failed(format!("could not send to {}: {error}", view.handle)))?
    }

    fn screen_tail(&self, view: &SessionView) -> Vec<String> {
        self.screen(view)
            .map(|lines| fallback::tail(&lines, FALLBACK_SEEN_LINES))
            .unwrap_or_default()
    }

    /// Presses Enter once more for a prompt still unsent, only while a fresh read right before it still
    /// finds that safe; otherwise why it held back.
    fn retry_enter(
        &self,
        view: &SessionView,
        agent_driver: &dyn crate::driver::AgentDriver,
        text: &str,
        (before, sent_ms): (u64, u64),
        connection: &mut pom_ptyhost::HolderConnection,
    ) -> std::io::Result<Option<&'static str>> {
        if let Some(why) = self.retry_blocker(view, agent_driver, text, before, sent_ms) {
            return Ok(Some(why));
        }
        let seen = self.screen_tail(view);
        let still_safe = |lines: &[String]| {
            (retry_blocker_on_screen(agent_driver, lines, text).is_none()
                && !turn_began(&self.state, &view.session_id, before, sent_ms))
            .then_some(())
        };
        let outcome = fallback::send_if_unchanged(
            || self.screen_lines(view),
            still_safe,
            &(),
            || connection.input(b"\r"),
        )?;
        Ok(match outcome {
            fallback::Recheck::Sent => {
                self.record_fallback(
                    view,
                    fallback::SUBMIT_RETRY,
                    &seen,
                    Some(fallback::key_names(b"\r")),
                    "the prompt still sat unsent in the input box",
                );
                None
            }
            fallback::Recheck::Changed => Some(fallback::WHY_CHANGED),
            fallback::Recheck::Unreadable(_) => Some(fallback::WHY_UNREADABLE),
        })
    }

    fn screen(&self, view: &SessionView) -> Option<Vec<String>> {
        self.screen_lines(view).ok()
    }

    fn screen_lines(&self, view: &SessionView) -> std::io::Result<Vec<String>> {
        pom_ptyhost::snapshot(&self.holders, &view.holder, SNAPSHOT_TIMEOUT)
            .map(|output| rendered_lines(&output))
    }

    /// Why one more Enter could do something other than submit `text`; `None` only when it is safe.
    fn retry_blocker(
        &self,
        view: &SessionView,
        agent_driver: &dyn crate::driver::AgentDriver,
        text: &str,
        before: u64,
        sent_ms: u64,
    ) -> Option<&'static str> {
        if turn_began(&self.state, &view.session_id, before, sent_ms) {
            return Some("the agent already reacted to it");
        }
        let Some(lines) = self.screen(view) else {
            return Some("its screen could not be read");
        };
        retry_blocker_on_screen(agent_driver, &lines, text)
    }

    fn wait_until_idle(&self, view: &SessionView, timeout: Duration) -> Result<(), DriveError> {
        let started = Instant::now();
        loop {
            let (state, reason) = read_state(&self.state, &view.session_id)
                .map(|record| record.status())
                .unwrap_or_else(|| (view.state.clone(), view.reason.clone()));
            if state == "idle" {
                return Ok(());
            }
            if started.elapsed() > timeout {
                let why = reason.map_or_else(String::new, |reason| format!(", {reason}"));
                let waited = match timeout.as_secs() {
                    seconds if seconds >= 60 && seconds % 60 == 0 => {
                        format!("{} min", seconds / 60)
                    }
                    seconds => format!("{seconds} s"),
                };
                return Err(DriveError::Timeout(format!(
                    "--queue gave up after {waited} waiting for {} to become idle: it is still {state}{why}",
                    view.handle
                )));
            }
            std::thread::sleep(POLL);
        }
    }

    /// The turn a `turn-end` wait without `--turn` waits for: the running one; else the one the last send
    /// started while it is still the latest and the session was not reopened since; else the next one.
    pub fn default_wait_turn(&self, view: &SessionView) -> u64 {
        let Some(record) = read_state(&self.state, &view.session_id) else {
            return view.turn + 1;
        };
        if record.turn_state == TurnState::Running {
            return record.turn;
        }
        if record.turn == 0 || !turn_was_sent(&self.state, &view.session_id, record.turn) {
            return record.turn + 1;
        }
        let lines = events(&self.state, &view.session_id);
        let reopened = lines
            .iter()
            .rposition(|line| line.event == "UserPromptSubmit" && line.turn == record.turn)
            .is_none_or(|at| lines[at..].iter().any(reopens));
        if reopened {
            record.turn + 1
        } else {
            record.turn
        }
    }

    /// Blocks until the session reaches `until`, asks for a permission, dies or the wait times out. A
    /// `turn-end` wait ends with exactly turn `turn`, by default `default_wait_turn`. It reads hook state;
    /// it never looks at the screen.
    pub fn wait(
        &self,
        view: &SessionView,
        until: WaitUntil,
        turn: Option<u64>,
        timeout: Option<Duration>,
    ) -> WaitOutcome {
        let started = Instant::now();
        let mut misses = 0;
        let target = turn.unwrap_or_else(|| self.default_wait_turn(view));
        let mut quiet_since: Option<(u64, Instant)> = None;
        loop {
            let record = read_state(&self.state, &view.session_id);
            let (state, current, turn_state) = record
                .as_ref()
                .map_or((view.state.clone(), view.turn, TurnState::None), |record| {
                    (record.status().0, record.turn, record.turn_state)
                });
            let outcome = |reached: &str,
                           stop_reason: Option<&str>,
                           cause: Option<String>,
                           exit_code: i32| WaitOutcome {
                handle: view.handle.clone(),
                reached: reached.into(),
                state: state.clone(),
                turn: current,
                for_turn: (until == WaitUntil::TurnEnd).then_some(target),
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
                    if current == target
                        && matches!(turn_state, TurnState::Ended | TurnState::Cancelled)
                    {
                        let reason = if turn_state == TurnState::Cancelled {
                            "cancelled"
                        } else {
                            "end_turn"
                        };
                        return outcome("turn-end", Some(reason), None, 0);
                    }
                    if current > target {
                        let reason = turn_spans(&events(&self.state, &view.session_id))
                            .into_iter()
                            .find(|span| span.turn == target)
                            .and_then(|span| span.stop_reason)
                            .unwrap_or("end_turn");
                        return outcome("turn-end", Some(reason), None, 0);
                    }
                    if let Some(record) = record.as_ref().filter(|record| {
                        record.turn == target && record.turn_state == TurnState::Running
                    }) {
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
    pub fn interrupt(
        &self,
        gate: &Gate<'_>,
        view: &SessionView,
        send_as: SendAs,
    ) -> Result<(), DriveError> {
        gate.drivable_target(&view.holder)?;
        let lease = self.typing_lease(gate, view, send_as)?;
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

    /// Allows or denies the permission prompt `view`'s session waits on: `id` if given, which must still be the
    /// waiting one. A workspace policy's ask is answered as `approve` does; the agent's own prompt through its
    /// hook while pom holds it. Once the agent draws its dialog instead, pom only denies it, with Esc, and
    /// never while a person drives the session: an allow there is refused.
    pub fn answer_permission(
        &self,
        gate: &Gate<'_>,
        view: &SessionView,
        id: Option<&str>,
        allow: bool,
        reason: &str,
        send_as: SendAs,
    ) -> Result<PermissionAnswer, DriveError> {
        gate.drivable_target(&view.holder)?;
        if view.session_id.is_empty() {
            return Err(DriveError::NotFound(format!(
                "{} has no session yet",
                view.handle
            )));
        }
        let pending = pending_permission(&self.state, &view.session_id);
        let prompt = match (pending, id) {
            (Some(prompt), None) => prompt,
            (Some(prompt), Some(id)) if prompt.id == id => prompt,
            (_, Some(id)) if is_policy_request(&self.state, &view.session_id, id) => {
                PermissionPrompt {
                    id: id.to_string(),
                    answer_by: ANSWER_BY_POLICY.into(),
                    ..PermissionPrompt::default()
                }
            }
            (_, Some(id)) => {
                return Err(DriveError::Stale(format!(
                    "{} does not wait on permission prompt {id}: it was answered, or another replaced it",
                    view.handle
                )))
            }
            (None, None) => {
                return Err(DriveError::NotFound(format!(
                    "{} waits on no permission prompt",
                    view.handle
                )))
            }
        };
        let decided_by = sender(gate, send_as).to_string();
        let answered = |via: &str| PermissionAnswer {
            handle: view.handle.clone(),
            id: prompt.id.clone(),
            allow,
            via: via.to_string(),
        };
        if prompt.answer_by == ANSWER_BY_POLICY {
            let scope = if allow {
                ApproveScope::Once
            } else {
                ApproveScope::Deny
            };
            self.approve(gate, view, &prompt.id, scope)?;
            return Ok(answered(ANSWER_BY_POLICY));
        }
        if prompt.answer_by != ANSWER_BY_HOOK && allow {
            self.record_fallback(
                view,
                fallback::PERMISSION_ALLOW,
                &[],
                None,
                fallback::WHY_ALLOW_NEEDS_HOOK,
            );
            return Err(DriveError::NotTyped {
                code: "allow_needs_hook",
                message: format!(
                    "{} shows its own permission dialog for {}, and pom never types an allow into it: allow it at its terminal (open it in Pomelo, or pom agent takeover), or deny it from here",
                    view.handle, prompt.id
                ),
            });
        }
        let lease = self.typing_lease(gate, view, send_as)?;
        let failed = |error: std::io::Error| DriveError::Failed(error.to_string());
        if prompt.answer_by == ANSWER_BY_HOOK {
            let answer = Answer {
                allow,
                decided_by: decided_by.clone(),
                reason: reason.to_string(),
                always: false,
            };
            write_answer(&self.state, &view.session_id, &prompt.id, &answer).map_err(failed)?;
            if !answer_taken(&self.state, &view.session_id, &prompt.id, HOOK_TAKE_WAIT) {
                return Err(DriveError::Failed(format!(
                    "the prompt {} of {} stopped waiting for an answer before taking this one; read agent ls and answer again",
                    prompt.id, view.handle
                )));
            }
        } else {
            self.deny_dialog(view, &prompt, &lease, &decided_by)?;
            let decision = SessionEvent {
                session_id: view.session_id.clone(),
                event: crate::permission::DECISION_EVENT.into(),
                detail: json!({"request": prompt.id, "allow": false, "decided_by": decided_by, "via": ANSWER_BY_TERMINAL}),
                ..SessionEvent::default()
            };
            if let Err(error) = record_dialog_answer(
                &self.state,
                &identity_of(self, view),
                &decision,
                crate::hooks::AgentState::Thinking,
            ) {
                eprintln!("agent: record {}: {error}", view.handle);
            }
            // The dialog's deny key also stops the turn, and that fires no hook.
            self.record(
                view,
                "Interrupted",
                Some(crate::hooks::AgentState::Idle),
                json!({"stop_reason": "cancelled"}),
            );
        }
        log(
            &self.state,
            &self.project,
            &view.workspace,
            json!({"event": "permission_answer", "role": view.role, "request": prompt.id, "scope": if allow { "once" } else { "deny" }, "decided_by": decided_by, "via": prompt.answer_by}),
        )
        .map_err(failed)?;
        Ok(answered(&prompt.answer_by))
    }

    fn record_fallback(
        &self,
        view: &SessionView,
        action: &str,
        seen: &[String],
        sent: Option<String>,
        why: &str,
    ) {
        fallback::record(
            &self.state,
            &self.project,
            &view.workspace,
            &fallback::Entry {
                holder: &view.holder,
                role: &view.role,
                session_id: &view.session_id,
                action,
                seen,
                sent,
                why,
            },
        );
    }

    /// Declines the agent's permission dialog with its own cancel key, only while no person drives the
    /// session and a fresh read right before the key still shows the dialog for this very prompt; then checks
    /// the dialog closed.
    fn deny_dialog(
        &self,
        view: &SessionView,
        prompt: &PermissionPrompt,
        lease: &Lease,
        decided_by: &str,
    ) -> Result<(), DriveError> {
        let skip = |seen: &[String], why: &str| {
            self.record_fallback(view, fallback::PERMISSION_DENY, seen, None, why);
        };
        if fallback::person_drives(lease) {
            skip(&[], fallback::WHY_PERSON_DRIVES);
            return Err(DriveError::NotTyped {
                code: "person_drives",
                message: format!(
                    "a person drives {}, so pom does not answer its dialog for them; answer it at its terminal",
                    view.handle
                ),
            });
        }
        let agent_driver = driver(&view.driver).ok_or_else(|| Refusal::NotDrivable {
            role: view.role.clone(),
        })?;
        let read = || {
            pom_ptyhost::snapshot(&self.holders, &view.holder, SNAPSHOT_TIMEOUT)
                .map(|output| rendered_lines(&output))
        };
        let lines = read().map_err(|error| {
            skip(&[], fallback::WHY_UNREADABLE);
            DriveError::Failed(format!(
                "could not read the screen of {}: {error}",
                view.handle
            ))
        })?;
        let Some(shown) = agent_driver.permission_dialog(&lines) else {
            skip(
                &fallback::tail(&lines, FALLBACK_SEEN_LINES),
                "no permission dialog on screen",
            );
            return Err(DriveError::Failed(format!(
                "{} does not show its permission dialog yet; answer again in a moment",
                view.handle
            )));
        };
        if pending_permission(&self.state, &view.session_id).map(|now| now.id)
            != Some(prompt.id.clone())
        {
            skip(&shown.content, "the prompt was answered meanwhile");
            return Err(DriveError::Stale(format!(
                "permission prompt {} of {} was answered meanwhile",
                prompt.id, view.handle
            )));
        }
        let mut connection =
            pom_ptyhost::connect_writer(&self.holders, &view.holder).map_err(|error| {
                DriveError::Failed(format!("could not reach {}: {error}", view.handle))
            })?;
        connection
            .claim(&lease.token)
            .map_err(|error| DriveError::Failed(error.to_string()))?;
        let keys = agent_driver.deny_permission();
        let typed = keys
            .iter()
            .map(|keys| fallback::key_names(&keys.bytes))
            .collect::<Vec<_>>()
            .join(" ");
        let outcome = fallback::send_if_unchanged(
            read,
            |lines| agent_driver.permission_dialog(lines),
            &shown,
            || -> Result<(), DriveError> {
                for keys in &keys {
                    connection
                        .input(&keys.bytes)
                        .map_err(|error| DriveError::Failed(error.to_string()))?;
                    std::thread::sleep(Duration::from_millis(keys.pause_ms));
                }
                Ok(())
            },
        )?;
        match outcome {
            fallback::Recheck::Sent => self.record_fallback(
                view,
                fallback::PERMISSION_DENY,
                &shown.content,
                Some(typed),
                &format!("denied by {decided_by}"),
            ),
            fallback::Recheck::Changed => {
                skip(&shown.content, fallback::WHY_CHANGED);
                return Err(DriveError::Failed(format!(
                    "the dialog of {} changed before pom typed into it, so nothing was typed; read agent ls and answer again",
                    view.handle
                )));
            }
            fallback::Recheck::Unreadable(error) => {
                skip(&shown.content, fallback::WHY_UNREADABLE);
                return Err(DriveError::Failed(format!(
                    "could not read the screen of {} again before typing, so nothing was typed: {error}",
                    view.handle
                )));
            }
        }
        let started = Instant::now();
        while started.elapsed() < DIALOG_CLOSE_WAIT {
            std::thread::sleep(POLL);
            let closed = read()
                .map(|lines| agent_driver.permission_dialog(&lines).is_none())
                .map_err(|error| {
                    DriveError::Failed(format!(
                        "could not read the screen of {}: {error}",
                        view.handle
                    ))
                })?;
            if closed {
                return Ok(());
            }
        }
        Err(DriveError::Failed(format!(
            "typed the answer into {} but its dialog is still open; answer it in its terminal",
            view.handle
        )))
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
        crate::launch::add_readable_dirs(&mut merged, &self.home);
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
        let argv = pom_shell::Shell::user().script_argv(&script);
        let group = self.holder_group(&view.workspace);
        pom_ptyhost::spawn_holder(
            &self.holders,
            &pom_ptyhost::SpawnRequest {
                group: Some(&group),
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

/// Who the workspace log says sent a turn.
fn sender<'a>(gate: &'a Gate<'_>, send_as: SendAs) -> &'a str {
    match (gate.caller().holder(), send_as) {
        (Some(holder), _) => holder,
        (None, SendAs::Person) => "person",
        (None, _) => "operator",
    }
}

/// An event after which the session runs a conversation anew: launched, resumed or cleared, never compacted.
fn reopens(line: &crate::sessions::EventLine) -> bool {
    match line.event.as_str() {
        crate::sessions::LAUNCHED_EVENT => true,
        "SessionStart" => line.detail["source"] != "compact",
        _ => false,
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

    fn input_screen(input: &str, dialog: &[&str]) -> Vec<String> {
        let rule = "\u{2500}".repeat(40);
        let mut lines = vec![
            "\u{276F} review the diff please".to_string(),
            "\u{23FA} Reviewed it.".into(),
            String::new(),
        ];
        lines.extend(dialog.iter().map(|line| line.to_string()));
        lines.extend([
            rule.clone(),
            format!("\u{276F}\u{A0}{input}"),
            rule,
            "  ? for shortcuts".into(),
        ]);
        lines
    }

    #[test]
    fn a_second_enter_is_pressed_only_while_the_box_still_holds_the_prompt_and_no_dialog_shows() {
        let claude = driver("claude").expect("claude");
        let text = "review the diff please\nand say what breaks";
        let decide = |lines: &[String]| retry_blocker_on_screen(claude, lines, text);

        assert_eq!(decide(&input_screen("review the diff please", &[])), None);
        assert_eq!(
            decide(&input_screen("[Pasted text #1 +1 lines]", &[])),
            None,
            "a long paste shows as its placeholder"
        );
        assert_eq!(
            decide(&input_screen("", &[])),
            Some("its input box no longer shows the prompt"),
            "the first Enter was taken; only the turn is slow to show"
        );
        assert_eq!(
            decide(&input_screen("Try \"write a test\"", &[])),
            Some("its input box no longer shows the prompt")
        );
        assert_eq!(
            decide(&input_screen("review the diff please", &[])[..2]),
            Some("its input box no longer shows the prompt"),
            "the sent turn's echo above the box does not count as the box"
        );

        let permission = [
            "Bash command",
            "make test",
            "Do you want to proceed?",
            "\u{276F} 1. Yes",
            "  2. Yes, and always allow access to /tmp/myproject from this project",
            "  3. Yes, and switch to auto mode",
            "  4. No",
            "",
            "Esc to cancel - Tab to amend",
        ];
        assert_eq!(
            decide(&input_screen("review the diff please", &permission)),
            Some("a dialog is on its screen"),
            "Enter would take the highlighted yes"
        );
        let unknown = ["Pick one", "\u{276F} 1. Keep going", "  2. Stop"];
        assert_eq!(
            decide(&input_screen("review the diff please", &unknown)),
            Some("a dialog is on its screen"),
            "any highlighted numbered choice counts"
        );
        let trust = [
            "Quick safety check: Is this a project you created or one you trust?",
            "\u{276F} No, exit",
            "  Yes, I trust this folder",
            "",
            "Enter to confirm - Esc to cancel",
        ];
        assert_eq!(
            decide(&trust.map(str::to_string)),
            Some("a dialog is on its screen")
        );
    }

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
            limits: Default::default(),
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
            reason: None,
            turn: 1,
            last_event: String::new(),
            last_event_age_s: 0,
            stale: false,
            ..SessionView::default()
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

    fn live_session(temp: &Path) -> (Drive, SessionView) {
        let drive = Drive {
            state: StateDir::new(temp.join("state")),
            holders: SocketDir::new(temp.join("s")),
            project: "myproject".into(),
            home: temp.into(),
            binary: "/bin/false".into(),
            tool_path: String::new(),
            limits: Default::default(),
        };
        let holder = "ws-myproject-feat-login-claude-reviewer";
        std::fs::create_dir_all(temp.join("s")).expect("dir");
        std::fs::write(
            drive.holders.pidfile(holder),
            format!("{}\n{holder}\n", std::process::id()),
        )
        .expect("pidfile");
        let view = SessionView {
            handle: "feat-login/reviewer".into(),
            workspace: "feat-login".into(),
            role: "reviewer".into(),
            holder: holder.into(),
            driver: "claude".into(),
            session_id: "s".into(),
            alive: true,
            drivable: true,
            ..SessionView::default()
        };
        (drive, view)
    }

    #[test]
    fn a_wait_for_a_later_turn_ignores_the_running_turns_transcript() {
        let temp = tempfile::tempdir().expect("temp");
        let (drive, view) = live_session(temp.path());
        let transcript = temp.path().join("t.jsonl");
        std::fs::write(
            &transcript,
            "{\"type\":\"assistant\",\"message\":{\"content\":[],\"stop_reason\":\"end_turn\"}}\n",
        )
        .expect("transcript");
        let event = SessionEvent {
            session_id: "s".into(),
            event: "UserPromptSubmit".into(),
            state: Some(AgentState::Thinking),
            transcript: transcript.to_string_lossy().into_owned(),
            ..SessionEvent::default()
        };
        record_event(&drive.state, &identity_of(&drive, &view), &event).expect("turn 1");
        let waited = drive.wait(&view, WaitUntil::TurnEnd, Some(2), Some(END_TURN_QUIET * 3));
        assert_eq!(
            (waited.reached.as_str(), waited.for_turn),
            ("timeout", Some(2))
        );
        let current = drive.wait(
            &view,
            WaitUntil::TurnEnd,
            None,
            Some(Duration::from_secs(5)),
        );
        assert_eq!(
            (current.reached.as_str(), current.for_turn),
            ("turn-end", Some(1)),
            "the running turn's transcript still ends it when that is the turn waited for"
        );
    }

    #[test]
    fn an_agent_can_neither_take_a_session_nor_act_as_its_person() {
        let temp = tempfile::tempdir().expect("temp");
        let (drive, view) = live_session(temp.path());
        let gate = Gate::open(
            &drive.state,
            Caller::Agent {
                holder: "ws-myproject-feat-login-claude-raw".into(),
            },
            crate::gate::Workspace {
                project: "myproject".into(),
                branch: "feat-login".into(),
            },
            crate::gate::Limits::OPERATOR,
        )
        .expect("gate");
        for send_as in [SendAs::Person, SendAs::TakeOver] {
            assert!(matches!(
                drive.typing_lease(&gate, &view, send_as),
                Err(DriveError::Invalid(_))
            ));
        }
        assert!(matches!(
            drive.typing_lease(&gate, &view, SendAs::LeaseHolder),
            Err(DriveError::Refused(Refusal::LeaseHeld { .. }))
        ));
    }

    fn ask_at_terminal(drive: &Drive, view: &SessionView) -> String {
        let asked = SessionEvent {
            session_id: view.session_id.clone(),
            event: crate::permission::REQUEST_EVENT.into(),
            state: Some(AgentState::AwaitingInput),
            detail: json!({"request": "p1", "call": "c1", "tool": "Bash", "input": {"command": "rm -rf build"}, "answer_by": ANSWER_BY_TERMINAL}),
            ..SessionEvent::default()
        };
        record_event(&drive.state, &identity_of(drive, view), &asked).expect("asked");
        "p1".into()
    }

    fn fallback_lines(drive: &Drive) -> Vec<Value> {
        crate::lease::log_lines(&drive.state, "myproject", "feat-login")
            .into_iter()
            .filter(|line| line["event"] == fallback::FALLBACK_LOG_EVENT)
            .collect()
    }

    fn hold_lease(drive: &Drive, view: &SessionView, lease: &Lease) {
        let path = crate::lease::lease_path(&drive.state, &view.holder);
        std::fs::create_dir_all(path.parent().expect("dir")).expect("leases");
        pom_paths::write_atomic(&path, &serde_json::to_vec(lease).expect("json"), 0o600)
            .expect("lease");
    }

    fn operator_gate(drive: &Drive) -> Gate<'_> {
        Gate::open(
            &drive.state,
            Caller::Operator,
            crate::gate::Workspace {
                project: "myproject".into(),
                branch: "feat-login".into(),
            },
            crate::gate::Limits::OPERATOR,
        )
        .expect("gate")
    }

    #[test]
    fn an_allow_is_never_typed_into_the_agents_dialog() {
        let temp = tempfile::tempdir().expect("temp");
        let (drive, view) = live_session(temp.path());
        let id = ask_at_terminal(&drive, &view);
        hold_lease(&drive, &view, &Lease::for_caller(&Caller::Operator));
        let gate = operator_gate(&drive);
        for send_as in [SendAs::LeaseHolder, SendAs::Person, SendAs::TakeOver] {
            let refused = drive
                .answer_permission(&gate, &view, Some(&id), true, "", send_as)
                .expect_err("refused");
            assert_eq!(refused.code(), "allow_needs_hook", "{send_as:?}");
            assert!(refused.to_string().contains("never types an allow"));
        }
        let logged = fallback_lines(&drive);
        assert_eq!(logged.len(), 3);
        assert_eq!(logged[0]["action"], fallback::PERMISSION_ALLOW);
        assert_eq!(logged[0]["skipped"], true);
        assert_eq!(logged[0]["why"], fallback::WHY_ALLOW_NEEDS_HOOK);
        assert_eq!(
            pending_permission(&drive.state, &view.session_id).map(|prompt| prompt.id),
            Some(id),
            "the prompt still waits"
        );
    }

    #[test]
    fn a_session_a_person_drives_gets_no_keys_only_a_log_line() {
        let temp = tempfile::tempdir().expect("temp");
        let (drive, view) = live_session(temp.path());
        let id = ask_at_terminal(&drive, &view);
        let gate = operator_gate(&drive);
        let refused = drive
            .answer_permission(&gate, &view, Some(&id), false, "", SendAs::Person)
            .expect_err("refused");
        assert_eq!(refused.code(), "person_drives");
        let logged = fallback_lines(&drive);
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0]["action"], fallback::PERMISSION_DENY);
        assert_eq!(logged[0]["sent"], Value::Null);
        assert_eq!(logged[0]["why"], fallback::WHY_PERSON_DRIVES);
        assert!(
            !crate::lease::log_lines(&drive.state, "myproject", "feat-login")
                .iter()
                .any(|line| line["event"] == "permission_answer"),
            "nothing was answered"
        );
    }

    #[test]
    fn a_deny_that_cannot_read_the_screen_types_nothing_and_says_so() {
        let temp = tempfile::tempdir().expect("temp");
        let (drive, view) = live_session(temp.path());
        let id = ask_at_terminal(&drive, &view);
        hold_lease(&drive, &view, &Lease::for_caller(&Caller::Operator));
        let gate = operator_gate(&drive);
        let failed = drive
            .answer_permission(&gate, &view, Some(&id), false, "", SendAs::LeaseHolder)
            .expect_err("no screen");
        assert_eq!(failed.code(), "failed");
        let logged = fallback_lines(&drive);
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0]["skipped"], true);
        assert_eq!(logged[0]["why"], fallback::WHY_UNREADABLE);
    }
}
