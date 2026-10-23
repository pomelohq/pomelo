//! The terminal's grid fed from a holder on another machine, over the project's server connection. Output
//! goes through the same VT parser into the same grid as a local holder; each keystroke goes out at once. A
//! connection that drops leaves the screen as it was, refuses typing, and carries on where it stopped once back.

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as BackendEvent, EventListener, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::Processor;
use pom_rpc::pty::{
    LinkState, Mode, PtyProvider, Request, ResumingOutput, ResumingTerminal, Target, TerminalEvent,
};

const IDLE: Duration = Duration::from_millis(250);
/// A full reset, for output that starts over after the server skipped what the ring no longer held.
const RESET: &[u8] = b"\x1bc";

/// Where a remote terminal's shell lives: what to open through `provider`.
#[derive(Clone)]
pub struct RemoteOptions {
    pub provider: Arc<dyn PtyProvider>,
    pub target: Target,
    pub mode: Mode,
}

pub(crate) struct RemoteBackend {
    pub(crate) options: RemoteOptions,
    terminal: ResumingTerminal,
}

impl RemoteBackend {
    pub(crate) fn attach<L: EventListener + Send + 'static>(
        options: RemoteOptions,
        size: WindowSize,
        term: Arc<FairMutex<Term<L>>>,
        listener: L,
    ) -> anyhow::Result<RemoteBackend> {
        let request = Request {
            target: options.target.clone(),
            cols: size.num_cols.max(1),
            rows: size.num_lines.max(1),
            mode: options.mode,
            primary: true,
            claim: None,
        };
        let (terminal, output, _) = ResumingTerminal::attach(options.provider.clone(), &request)
            .map_err(anyhow::Error::msg)?;
        std::thread::Builder::new()
            .name("terminal-remote".into())
            .spawn(move || read_output(output, &term, &listener))?;
        Ok(RemoteBackend {
            options: RemoteOptions {
                target: Target::Holder(terminal.holder().to_string()),
                ..options
            },
            terminal,
        })
    }

    pub(crate) fn holder(&self) -> &str {
        self.terminal.holder()
    }

    /// Typing while the connection is down goes nowhere, so nothing lands later by surprise.
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        if let Err(error) = self.terminal.input(bytes) {
            if self.terminal.link() == LinkState::Connected {
                eprintln!("remote terminal input: {error}");
            }
        }
    }

    pub(crate) fn resize(&mut self, size: WindowSize) {
        if let Err(error) = self.terminal.resize(size.num_cols, size.num_lines) {
            eprintln!("remote terminal resize: {error}");
        }
    }

    pub(crate) fn make_primary(&mut self) {
        if let Err(error) = self.terminal.make_primary() {
            eprintln!("remote terminal primary: {error}");
        }
    }

    pub(crate) fn detach(&self) {
        if let Err(error) = self.terminal.detach() {
            eprintln!("remote terminal detach: {error}");
        }
    }

    /// Ends the holder and its command on the other machine, off this thread.
    pub(crate) fn close(&self) {
        self.detach();
        let (provider, holder) = (self.options.provider.clone(), self.holder().to_string());
        std::thread::spawn(move || {
            if let Err(error) = provider.close(&holder) {
                eprintln!("remote terminal close: {error}");
            }
        });
    }
}

fn read_output<L: EventListener>(
    mut output: ResumingOutput,
    term: &FairMutex<Term<L>>,
    listener: &L,
) {
    let mut processor: Processor = Processor::new();
    loop {
        // A synchronized update holds frames until it ends or times out; wake up in time to end it.
        let wait = processor
            .sync_timeout()
            .sync_timeout()
            .map_or(IDLE, |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1))
            });
        match output.next(wait) {
            TerminalEvent::Data { offset, bytes } => {
                processor.advance(&mut *term.lock(), &bytes);
                output.ack(offset + bytes.len() as u64);
                listener.send_event(BackendEvent::Wakeup);
            }
            TerminalEvent::Reset { .. } => {
                processor = Processor::new();
                processor.advance(&mut *term.lock(), RESET);
                listener.send_event(BackendEvent::Wakeup);
            }
            TerminalEvent::Idle => {
                processor.stop_sync(&mut *term.lock());
                listener.send_event(BackendEvent::Wakeup);
            }
            TerminalEvent::Reconnecting(_) | TerminalEvent::Resumed { .. } => {
                listener.send_event(BackendEvent::Wakeup);
            }
            TerminalEvent::Exit { crashed } => {
                let status = ExitStatus::from_raw(if crashed { 1 << 8 } else { 0 });
                listener.send_event(BackendEvent::ChildExit(status));
                return;
            }
            TerminalEvent::Ended(_) => return,
        }
    }
}
