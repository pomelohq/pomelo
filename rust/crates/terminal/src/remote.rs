//! The terminal's grid fed from a holder on another machine, over the project's server connection. Output
//! goes through the same VT parser into the same grid as a local holder; each keystroke goes out at once.

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as BackendEvent, EventListener, WindowSize};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::Processor;
use pom_rpc::pty::{Control, Mode, Output, PtyProvider, Request, Target};
use pom_rpc::StreamEvent;

const EXIT_WAIT: Duration = Duration::from_secs(1);
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
    control: Control,
    detached: Arc<AtomicBool>,
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
        let terminal = options
            .provider
            .attach(&request)
            .map_err(anyhow::Error::msg)?;
        let detached = Arc::new(AtomicBool::new(false));
        let (output, control) = (terminal.output, terminal.control);
        let reading = (control.clone(), detached.clone());
        std::thread::Builder::new()
            .name("terminal-remote".into())
            .spawn(move || read_output(output, reading, &term, &listener))?;
        Ok(RemoteBackend {
            options: RemoteOptions {
                target: Target::Holder(control.holder().to_string()),
                ..options
            },
            control,
            detached,
        })
    }

    pub(crate) fn holder(&self) -> &str {
        self.control.holder()
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) {
        if let Err(error) = self.control.input(bytes) {
            eprintln!("remote terminal input: {error}");
        }
    }

    pub(crate) fn resize(&mut self, size: WindowSize) {
        if let Err(error) = self.control.resize(size.num_cols, size.num_lines) {
            eprintln!("remote terminal resize: {error}");
        }
    }

    pub(crate) fn make_primary(&mut self) {
        if let Err(error) = self.control.make_primary() {
            eprintln!("remote terminal primary: {error}");
        }
    }

    pub(crate) fn detach(&self) {
        if self.detached.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Err(error) = self.control.detach() {
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
    output: Output,
    (control, detached): (Control, Arc<AtomicBool>),
    term: &FairMutex<Term<L>>,
    listener: &L,
) {
    let mut processor: Processor = Processor::new();
    let ended = loop {
        // A synchronized update holds frames until it ends or times out; wake up in time to end it.
        let wait = processor
            .sync_timeout()
            .sync_timeout()
            .map_or(IDLE, |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .max(Duration::from_millis(1))
            });
        match output.recv_timeout(wait) {
            Ok(StreamEvent::Data { offset, bytes }) => {
                processor.advance(&mut *term.lock(), &bytes);
                control.ack(offset + bytes.len() as u64);
                listener.send_event(BackendEvent::Wakeup);
            }
            Ok(StreamEvent::Reset { .. }) => {
                processor = Processor::new();
                processor.advance(&mut *term.lock(), RESET);
                listener.send_event(BackendEvent::Wakeup);
            }
            Ok(StreamEvent::End { reason }) => break reason,
            Err(pom_rpc::Error::Timeout) => {
                processor.stop_sync(&mut *term.lock());
                listener.send_event(BackendEvent::Wakeup);
            }
            Err(error) => break error.to_string(),
        }
    };
    if detached.load(Ordering::Acquire) {
        return;
    }
    if ended == "exit" {
        let crashed = output.exit(EXIT_WAIT).is_some_and(|exit| exit.crashed);
        let status = ExitStatus::from_raw(if crashed { 1 << 8 } else { 0 });
        listener.send_event(BackendEvent::ChildExit(status));
        return;
    }
    let notice = format!("\r\n\x1b[2m[connection to the server lost: {ended}]\x1b[0m\r\n");
    processor.advance(&mut *term.lock(), notice.as_bytes());
    listener.send_event(BackendEvent::Wakeup);
}
