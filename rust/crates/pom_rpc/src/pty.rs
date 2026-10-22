//! Terminals carried by a connection: the `pty.*` requests, their parameters, and the client's end of one
//! attached terminal. A terminal is a stream whose DATA is the holder's output at its absolute offsets, whose
//! INPUT is what the user types, and whose exit comes as a `pty.exit` push.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::connection::{
    Connection, PushEvent, StreamEvent, StreamHandle, StreamReceiver, Subscription,
};
use crate::error::Error;

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

/// Opens terminals on another machine; implemented by whatever owns the connection there.
pub trait PtyProvider: Send + Sync {
    fn attach(&self, request: &Request) -> Result<Terminal, String>;
    /// Ends the holder and its command.
    fn close(&self, holder: &str) -> Result<(), String>;
}

/// The reading end of an attached terminal: output, resets and the end.
pub struct Output {
    stream: StreamReceiver,
    events: Option<Subscription>,
}

impl Output {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<StreamEvent, Error> {
        self.stream.recv_timeout(timeout)
    }

    /// The `pty.exit` the server pushed, waiting up to `timeout` for it.
    pub fn exit(&self, timeout: Duration) -> Option<Exit> {
        let events = self.events.as_ref()?;
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
    resizes: std::sync::Arc<AtomicU64>,
}

impl Control {
    pub fn holder(&self) -> &str {
        &self.holder
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
}

impl Terminal {
    /// Opens a stream and attaches it to the holder `params` names; the stream and push channel are filled in.
    pub fn attach(
        connection: &Connection,
        mut params: AttachParams,
        timeout: Duration,
    ) -> Result<Terminal, Error> {
        let stream = connection.open_stream()?;
        let events = connection.subscribe()?;
        params.channel = stream.channel();
        params.events = Some(events.channel());
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
            resizes: std::sync::Arc::new(AtomicU64::new(0)),
        };
        Ok(Terminal {
            output: Output {
                stream,
                events: Some(events),
            },
            control,
            attached,
        })
    }
}
