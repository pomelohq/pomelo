//! A remote terminal that outlives its connection: when its stream drops it attaches again from the last output
//! offset the view applied, and sends again the input the server may not have written. The same server drops
//! what it already wrote; a restarted server never sees the unconfirmed input, so nothing is typed twice.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::connection::StreamEvent;
use crate::error::Error;
use crate::pty::{
    Attached, Control, LinkState, Mode, Output, PtyProvider, Request, Resume, ResumeError,
};

/// Input kept after it went out, in case the connection drops before the server writes it.
const UNCONFIRMED_LIMIT: usize = 64 * 1024;
const EXIT_WAIT: Duration = Duration::from_secs(1);
/// How long `next` waits before it asks again while the connection is down.
const OFFLINE_WAIT: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Data {
        offset: u64,
        bytes: Vec<u8>,
    },
    /// What follows does not carry on from what came before: the view starts over.
    Reset {
        offset: u64,
    },
    Idle,
    /// The stream dropped with its connection; output goes on once it is back.
    Reconnecting(String),
    /// Attached again; `restarted` when through a new server, which never got the unconfirmed input.
    Resumed {
        restarted: bool,
    },
    Exit {
        crashed: bool,
    },
    /// Detached on purpose: nothing more comes.
    Ended(String),
}

struct State {
    control: Option<Control>,
    server: String,
    /// The output offset after the last byte handed to the view.
    applied: u64,
    /// The input offset the next typed byte gets.
    input_next: u64,
    /// The last input sent, ending at `input_next`.
    unconfirmed: VecDeque<u8>,
    primary: bool,
    cols: u16,
    rows: u16,
    detached: bool,
    finished: bool,
}

struct Shared {
    provider: Arc<dyn PtyProvider>,
    holder: String,
    terminal: String,
    mode: Mode,
    state: Mutex<State>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The controlling end: type, resize and detach from any thread. Cheap to clone.
#[derive(Clone)]
pub struct ResumingTerminal {
    shared: Arc<Shared>,
}

/// The reading end, for one thread: output, and the drops and resumes in between.
pub struct ResumingOutput {
    shared: Arc<Shared>,
    output: Option<Output>,
    pending: VecDeque<TerminalEvent>,
}

impl ResumingTerminal {
    pub fn attach(
        provider: Arc<dyn PtyProvider>,
        request: &Request,
    ) -> Result<(ResumingTerminal, ResumingOutput, Attached), String> {
        let terminal = provider.attach(request)?;
        let attached = terminal.attached.clone();
        let shared = Arc::new(Shared {
            provider,
            holder: attached.holder.clone(),
            terminal: terminal.control.terminal().to_string(),
            mode: request.mode,
            state: Mutex::new(State {
                control: Some(terminal.control),
                server: terminal.server,
                applied: attached.start,
                input_next: attached.input_from,
                unconfirmed: VecDeque::new(),
                primary: request.primary,
                cols: request.cols,
                rows: request.rows,
                detached: false,
                finished: false,
            }),
        });
        let output = ResumingOutput {
            shared: shared.clone(),
            output: Some(terminal.output),
            pending: VecDeque::new(),
        };
        Ok((ResumingTerminal { shared }, output, attached))
    }

    pub fn holder(&self) -> &str {
        &self.shared.holder
    }

    pub fn provider(&self) -> &Arc<dyn PtyProvider> {
        &self.shared.provider
    }

    /// Sends `bytes` at once; refused while the connection is down, so nothing waits to be typed later.
    pub fn input(&self, bytes: &[u8]) -> Result<(), String> {
        if self.shared.provider.link() != LinkState::Connected {
            return Err("reconnecting".to_string());
        }
        let mut state = self.shared.state();
        let control = state.control.clone().ok_or("reconnecting")?;
        control.input(bytes).map_err(|error| error.to_string())?;
        state.input_next += bytes.len() as u64;
        state.unconfirmed.extend(bytes);
        let excess = state.unconfirmed.len().saturating_sub(UNCONFIRMED_LIMIT);
        state.unconfirmed.drain(..excess);
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let mut state = self.shared.state();
        state.cols = cols;
        state.rows = rows;
        match &state.control {
            Some(control) => control
                .resize(cols, rows)
                .map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    pub fn make_primary(&self) -> Result<(), String> {
        let mut state = self.shared.state();
        state.primary = true;
        match &state.control {
            Some(control) => control.make_primary().map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    /// Ends the stream for good; the holder and its command keep running.
    pub fn detach(&self) -> Result<(), String> {
        let mut state = self.shared.state();
        if state.detached {
            return Ok(());
        }
        state.detached = true;
        match &state.control {
            Some(control) => control.detach().map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    /// How the connection stands for this terminal: reconnecting until its stream is attached again.
    pub fn link(&self) -> LinkState {
        let link = self.shared.provider.link();
        let state = self.shared.state();
        match link {
            LinkState::Connected if state.control.is_none() && !state.finished => {
                LinkState::Reconnecting {
                    attempt: 0,
                    next_in: Duration::ZERO,
                }
            }
            other => other,
        }
    }
}

impl ResumingOutput {
    /// Marks output up to `offset` as applied, so the server may send more.
    pub fn ack(&self, offset: u64) {
        if let Some(control) = &self.shared.state().control {
            control.ack(offset);
        }
    }

    pub fn next(&mut self, timeout: Duration) -> TerminalEvent {
        if let Some(event) = self.pending.pop_front() {
            return event;
        }
        let Some(output) = &self.output else {
            return self.resume(timeout);
        };
        match output.recv_timeout(timeout) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                self.shared.state().applied = offset + bytes.len() as u64;
                TerminalEvent::Data { offset, bytes }
            }
            Ok(StreamEvent::Reset { offset }) => {
                self.shared.state().applied = offset;
                TerminalEvent::Reset { offset }
            }
            Ok(StreamEvent::End { reason }) if reason == "exit" => {
                let crashed = output.exit(EXIT_WAIT).is_some_and(|exit| exit.crashed);
                self.finish();
                TerminalEvent::Exit { crashed }
            }
            Ok(StreamEvent::End { reason }) => self.lost(reason),
            Err(Error::Timeout) => TerminalEvent::Idle,
            Err(error) => self.lost(error.to_string()),
        }
    }

    fn finish(&mut self) {
        self.output = None;
        let mut state = self.shared.state();
        state.finished = true;
        state.control = None;
    }

    fn lost(&mut self, reason: String) -> TerminalEvent {
        self.output = None;
        let mut state = self.shared.state();
        state.control = None;
        if state.detached {
            state.finished = true;
            return TerminalEvent::Ended(reason);
        }
        TerminalEvent::Reconnecting(reason)
    }

    fn resume(&mut self, timeout: Duration) -> TerminalEvent {
        let resume = {
            let state = self.shared.state();
            if state.finished {
                return TerminalEvent::Ended("ended".to_string());
            }
            if state.detached {
                drop(state);
                self.finish();
                return TerminalEvent::Ended("detached".to_string());
            }
            Resume {
                holder: self.shared.holder.clone(),
                terminal: self.shared.terminal.clone(),
                since: state.applied,
                input_from: Some(state.input_next - state.unconfirmed.len() as u64),
                server: state.server.clone(),
                mode: self.shared.mode,
                primary: state.primary,
                cols: state.cols,
                rows: state.rows,
            }
        };
        if self.shared.provider.link() != LinkState::Connected {
            std::thread::sleep(timeout.min(OFFLINE_WAIT));
            return TerminalEvent::Idle;
        }
        let terminal = match self.shared.provider.resume(&resume) {
            Ok(terminal) => terminal,
            Err(ResumeError::Offline(_)) => {
                std::thread::sleep(timeout.min(OFFLINE_WAIT));
                return TerminalEvent::Idle;
            }
            Err(ResumeError::Gone(_)) => {
                self.finish();
                return TerminalEvent::Exit { crashed: false };
            }
        };
        let restarted = !resume.server.is_empty() && terminal.server != resume.server;
        let mut state = self.shared.state();
        if restarted {
            state.unconfirmed.clear();
        }
        state.input_next = terminal.attached.input_from + state.unconfirmed.len() as u64;
        if !state.unconfirmed.is_empty() {
            let (front, back) = state.unconfirmed.as_slices();
            let resent = [front, back].concat();
            if let Err(error) = terminal.control.input(&resent) {
                eprintln!("remote terminal: resend input: {error}");
            }
        }
        state.server = terminal.server;
        state.control = Some(terminal.control);
        if state.detached {
            if let Some(control) = &state.control {
                if let Err(error) = control.detach() {
                    eprintln!("remote terminal detach: {error}");
                }
            }
        }
        if terminal.attached.start != resume.since {
            state.applied = terminal.attached.start;
            self.pending.push_back(TerminalEvent::Reset {
                offset: terminal.attached.start,
            });
        }
        drop(state);
        self.output = Some(terminal.output);
        TerminalEvent::Resumed { restarted }
    }
}
