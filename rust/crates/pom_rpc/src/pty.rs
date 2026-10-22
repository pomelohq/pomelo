//! Terminals carried by a connection: the `pty.*` requests, their parameters, and the client's end of one
//! attached terminal. A terminal is a stream whose DATA is the holder's output at its absolute offsets, whose
//! INPUT is what the user types, and whose exit comes as a `pty.exit` push.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::connection::{
    Connection, PushEvent, StreamEvent, StreamHandle, StreamReceiver, Subscription,
};
use crate::error::Error;

pub use crate::resuming::{ResumingOutput, ResumingTerminal, TerminalEvent};

pub mod method {
    pub const OPEN: &str = "pty.open";
    pub const ATTACH: &str = "pty.attach";
    pub const RESIZE: &str = "pty.resize";
    pub const PRIMARY: &str = "pty.primary";
    pub const DETACH: &str = "pty.detach";
    pub const CLOSE: &str = "pty.close";
}

/// Every terminal method; a server that lists them all in its HELLO serves terminals.
pub const METHODS: &[&str] = &[
    method::OPEN,
    method::ATTACH,
    method::RESIZE,
    method::PRIMARY,
    method::DETACH,
    method::CLOSE,
];

pub const EXIT_TOPIC: &str = "pty.exit";
/// In a server's HELLO: every terminal of a connection may share one push channel for its events.
pub const SHARED_EVENTS: &str = "pty.shared_events";
pub const MAX_SIZE: u16 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A new shell in the workspace.
    #[default]
    Shell,
    /// The running holder of one of the workspace's services.
    Service,
    /// The running holder of one of the workspace's agent sessions.
    Agent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    #[serde(rename = "rw")]
    ReadWrite,
    /// Watches only: what it types is dropped.
    #[serde(rename = "ro")]
    ReadOnly,
}

/// The server picks the holder's name and command; the client only says what kind of terminal it wants.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenParams {
    pub cfg: String,
    pub branch: String,
    pub kind: Kind,
    /// The agent role of an `agent` terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The `repo/service` of a `service` terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Where a shell starts, inside the workspace; the workspace itself when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Opened {
    pub holder: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AttachParams {
    pub cfg: String,
    pub branch: String,
    pub holder: String,
    /// The stream the client opened for this terminal's output.
    pub channel: u32,
    /// The subscription `pty.exit` is pushed on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<u32>,
    /// `events` serves every terminal of the connection, so it outlives this one.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shared_events: bool,
    /// The output offset to replay from; 0 replays the whole scrollback.
    pub since: u64,
    /// Where this client's input numbering resumes; unset starts a fresh numbering the server picks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_from: Option<u64>,
    /// Tells this client's terminals apart, so each has its own input numbering.
    pub terminal: String,
    pub mode: Mode,
    pub primary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attached {
    pub holder: String,
    /// The offset of the first byte the stream carries.
    pub start: u64,
    /// The holder's output offset when the stream started.
    pub end: u64,
    /// Where this client's input numbering starts.
    pub input_from: u64,
}

/// Later resizes win: the server drops one whose `seq` is older than the last it applied.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResizeParams {
    pub channel: u32,
    pub cols: u16,
    pub rows: u16,
    pub seq: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelParams {
    pub channel: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloseParams {
    pub cfg: String,
    pub branch: String,
    pub holder: String,
}

/// Pushed when a terminal's command ends.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Exit {
    pub holder: String,
    pub crashed: bool,
    /// The terminal's stream, which tells terminals apart on a shared events channel.
    #[serde(skip_serializing_if = "is_zero")]
    pub channel: u32,
}

fn is_zero(channel: &u32) -> bool {
    *channel == 0
}

/// What a terminal view asks a [`PtyProvider`] for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Shell {
        cwd: Option<String>,
    },
    Service(String),
    Agent(String),
    /// A holder already running, by name (a terminal reopened after the app restarted).
    Holder(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub target: Target,
    pub cols: u16,
    pub rows: u16,
    pub mode: Mode,
    pub primary: bool,
    pub claim: Option<String>,
}

/// How the connection to the machine behind a [`PtyProvider`] stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LinkState {
    #[default]
    Connected,
    /// The connection dropped; attempt `attempt` runs in `next_in`.
    Reconnecting { attempt: u32, next_in: Duration },
    /// Reconnecting gave up; only a retry the person asks for tries again.
    Disconnected { reason: String },
}

/// Attaching again to a terminal whose stream dropped with its connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    pub holder: String,
    /// The terminal's id in its first attach, which keys the server's record of the input it took.
    pub terminal: String,
    /// The output offset after the last byte the view applied.
    pub since: u64,
    /// Where the input still unconfirmed starts; `None` starts a fresh numbering.
    pub input_from: Option<u64>,
    /// The server the terminal was attached through; another one means the server restarted.
    pub server: String,
    pub mode: Mode,
    pub primary: bool,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeError {
    /// The machine cannot be reached right now; try again once it can.
    Offline(String),
    /// The terminal is gone (its command ended meanwhile) or cannot be attached again.
    Gone(String),
}

/// Opens terminals on another machine; implemented by whatever owns the connection there.
pub trait PtyProvider: Send + Sync {
    fn attach(&self, request: &Request) -> Result<Terminal, String>;
    /// Ends the holder and its command.
    fn close(&self, holder: &str) -> Result<(), String>;
    /// Attaches again after the connection dropped, on whatever connection now serves.
    fn resume(&self, _resume: &Resume) -> Result<Terminal, ResumeError> {
        Err(ResumeError::Gone(
            "this machine does not resume terminals".into(),
        ))
    }
    fn link(&self) -> LinkState {
        LinkState::Connected
    }
    /// The machine's name, for the bar that says the connection dropped.
    fn host(&self) -> String {
        String::new()
    }
    /// Tries to reconnect now rather than at the next scheduled attempt.
    fn reconnect_now(&self) {}
    /// Bytes of typed input kept until the server has surely written them, to send again after a drop.
    fn unconfirmed_input_limit(&self) -> usize {
        UNCONFIRMED_INPUT_LIMIT
    }
}

pub const UNCONFIRMED_INPUT_LIMIT: usize = 64 * 1024;

type Routes = Arc<Mutex<HashMap<u32, Sender<Exit>>>>;

fn routes(routes: &Routes) -> MutexGuard<'_, HashMap<u32, Sender<Exit>>> {
    routes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How often the router looks up from its wait to notice it was dropped.
const ROUTE_POLL: Duration = Duration::from_millis(500);

/// One push channel for the events of every terminal of a connection, so a terminal costs one channel instead
/// of two. Only for a server whose HELLO lists [`SHARED_EVENTS`].
pub struct SharedEvents {
    channel: u32,
    routes: Routes,
    stop: Arc<AtomicBool>,
}

impl SharedEvents {
    pub fn open(connection: &Connection) -> Result<SharedEvents, Error> {
        let subscription = connection.subscribe()?;
        let channel = subscription.channel();
        let routes: Routes = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_routes, thread_stop) = (routes.clone(), stop.clone());
        std::thread::Builder::new()
            .name("pom-rpc-pty-events".into())
            .spawn(move || route(&subscription, &thread_routes, &thread_stop))
            .map_err(|error| Error::Io(error.to_string()))?;
        Ok(SharedEvents {
            channel,
            routes,
            stop,
        })
    }

    pub fn channel(&self) -> u32 {
        self.channel
    }

    fn register(&self, stream: u32) -> Route {
        let (sender, exits) = mpsc::channel();
        routes(&self.routes).insert(stream, sender);
        Route {
            routes: self.routes.clone(),
            shared: self.channel,
            stream,
            exits,
        }
    }
}

impl Drop for SharedEvents {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// A listener that went away stopped caring on purpose, so a failed hand-off is not an error.
fn deliver(sender: &Sender<Exit>, exit: Exit) -> bool {
    sender.send(exit).is_ok()
}

fn route(subscription: &Subscription, table: &Routes, stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        match subscription.recv_timeout(ROUTE_POLL) {
            Ok(PushEvent::Value { topic, value }) if topic == EXIT_TOPIC => {
                let Ok(exit) = serde_json::from_value::<Exit>(value) else {
                    continue;
                };
                if let Some(sender) = routes(table).remove(&exit.channel) {
                    deliver(&sender, exit);
                }
            }
            Ok(PushEvent::Value { .. }) | Err(Error::Timeout) => {}
            Ok(PushEvent::End { .. }) | Err(_) => break,
        }
    }
    routes(table).clear();
}

/// One terminal's entry on a shared events channel; dropping it leaves the channel.
struct Route {
    routes: Routes,
    shared: u32,
    stream: u32,
    exits: Receiver<Exit>,
}

impl Drop for Route {
    fn drop(&mut self) {
        routes(&self.routes).remove(&self.stream);
    }
}

enum Events {
    Own(Subscription),
    Shared(Route),
}

/// The reading end of an attached terminal: output, resets and the end.
pub struct Output {
    stream: StreamReceiver,
    events: Events,
}

impl Output {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<StreamEvent, Error> {
        self.stream.recv_timeout(timeout)
    }

    /// The `pty.exit` the server pushed, waiting up to `timeout` for it.
    pub fn exit(&self, timeout: Duration) -> Option<Exit> {
        let events = match &self.events {
            Events::Shared(route) => return route.exits.recv_timeout(timeout).ok(),
            Events::Own(events) => events,
        };
        loop {
            match events.recv_timeout(timeout) {
                Ok(PushEvent::Value { topic, value }) if topic == EXIT_TOPIC => {
                    return serde_json::from_value(value).ok()
                }
                Ok(PushEvent::Value { .. }) => {}
                Ok(PushEvent::End { .. }) | Err(_) => return None,
            }
        }
    }
}

/// The controlling end of an attached terminal; cheap to clone and usable from any thread.
#[derive(Clone)]
pub struct Control {
    connection: Connection,
    stream: StreamHandle,
    holder: String,
    terminal: String,
    resizes: std::sync::Arc<AtomicU64>,
}

impl Control {
    pub fn holder(&self) -> &str {
        &self.holder
    }

    /// The id the terminal was attached with, which the server keys its input with.
    pub fn terminal(&self) -> &str {
        &self.terminal
    }

    /// The stream's channel, which `pty.detach` and friends name.
    pub fn channel(&self) -> u32 {
        self.stream.channel()
    }

    pub fn ack(&self, offset: u64) {
        self.stream.ack(offset);
    }

    /// Each call goes out at once as its own INPUT frame, ahead of any queued output.
    pub fn input(&self, bytes: &[u8]) -> Result<(), Error> {
        self.stream.send_input(bytes)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), Error> {
        let params = ResizeParams {
            channel: self.stream.channel(),
            cols,
            rows,
            seq: self.resizes.fetch_add(1, Ordering::AcqRel) + 1,
        };
        self.connection
            .start_request(method::RESIZE, params)
            .map(|pending| pending.forget())
    }

    /// Ends this stream; the holder and its command keep running.
    pub fn detach(&self) -> Result<(), Error> {
        let params = ChannelParams {
            channel: self.stream.channel(),
        };
        self.connection
            .start_request(method::DETACH, params)
            .map(|pending| pending.forget())
    }

    pub fn make_primary(&self) -> Result<(), Error> {
        let params = ChannelParams {
            channel: self.stream.channel(),
        };
        self.connection
            .start_request(method::PRIMARY, params)
            .map(|pending| pending.forget())
    }
}

/// One attached terminal, split so a reader thread owns the output while the view types and resizes.
pub struct Terminal {
    pub output: Output,
    pub control: Control,
    pub attached: Attached,
    /// The id of the server it was attached through, when the provider knows it.
    pub server: String,
}

impl Terminal {
    /// Opens a stream and attaches it to the holder `params` names; the stream and push channel are filled in.
    pub fn attach(
        connection: &Connection,
        params: AttachParams,
        timeout: Duration,
    ) -> Result<Terminal, Error> {
        let stream = connection.open_stream()?;
        let events = connection.subscribe()?;
        Self::finish_attach(connection, stream, Events::Own(events), params, timeout)
    }

    /// Like [`Terminal::attach`], with the exit pushed on the connection's shared events channel.
    pub fn attach_shared(
        connection: &Connection,
        shared: &SharedEvents,
        params: AttachParams,
        timeout: Duration,
    ) -> Result<Terminal, Error> {
        let stream = connection.open_stream()?;
        let route = shared.register(stream.channel());
        Self::finish_attach(connection, stream, Events::Shared(route), params, timeout)
    }

    fn finish_attach(
        connection: &Connection,
        stream: StreamReceiver,
        events: Events,
        mut params: AttachParams,
        timeout: Duration,
    ) -> Result<Terminal, Error> {
        params.channel = stream.channel();
        match &events {
            Events::Own(subscription) => {
                params.events = Some(subscription.channel());
                params.shared_events = false;
            }
            Events::Shared(route) => {
                params.events = Some(route.shared);
                params.shared_events = true;
            }
        }
        let value = connection
            .start_request(method::ATTACH, &params)?
            .wait_timeout(timeout)?;
        let attached: Attached = serde_json::from_value(value)
            .map_err(|error| Error::Protocol(format!("pty.attach answer: {error}")))?;
        stream.set_input_offset(attached.input_from);
        let control = Control {
            connection: connection.clone(),
            stream: stream.handle(),
            holder: attached.holder.clone(),
            terminal: params.terminal.clone(),
            resizes: std::sync::Arc::new(AtomicU64::new(0)),
        };
        Ok(Terminal {
            output: Output { stream, events },
            control,
            attached,
            server: String::new(),
        })
    }
}
