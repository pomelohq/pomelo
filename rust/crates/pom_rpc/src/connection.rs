//! One multiplexed connection over a byte pipe: requests, pushes and credit-limited streams, on three threads
//! (reader, writer, heartbeat) plus one short-lived thread per request being served.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::envelope::{
    decode, error_code, End, Hello, Push, RemoteError, Request, Response, PROTOCOL_VERSION,
};
use crate::error::{CloseReason, Error};
use crate::frame::{read_frame, write_frame, Frame, Kind, ReadError, MAX_DATA};

pub const MAX_CHANNELS: usize = 64;
pub const MAX_PENDING_REQUESTS: usize = 256;
pub const DEFAULT_WINDOW: u64 = 256 * 1024;
const ACK_BATCH: u64 = 32 * 1024;
/// Channel 0 is the connection itself: HELLO, and an END there closes everything.
const CONNECTION_CHANNEL: u32 = 0;

/// Both ends must use the same `window`: the receiver rejects DATA beyond the credit it believes it granted.
#[derive(Debug, Clone)]
pub struct Config {
    pub window: u64,
    pub heartbeat_interval: Duration,
    pub dead_after: Duration,
    pub handshake_timeout: Duration,
    /// How long a closing side waits for its farewell END to reach the pipe before tearing the transport down.
    pub close_grace: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            window: DEFAULT_WINDOW,
            heartbeat_interval: Duration::from_secs(5),
            dead_after: Duration::from_secs(15),
            handshake_timeout: Duration::from_secs(10),
            close_grace: Duration::from_secs(1),
        }
    }
}

type Closer = Box<dyn FnOnce() -> io::Result<()> + Send>;

/// The two halves of a byte pipe, plus a way to tear it down so a reader blocked in `read` wakes up.
pub struct Transport {
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    closer: Option<Closer>,
}

impl Transport {
    pub fn new(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> Self {
        Transport {
            reader: Box::new(reader),
            writer: Box::new(writer),
            closer: None,
        }
    }

    pub fn with_closer(mut self, closer: impl FnOnce() -> io::Result<()> + Send + 'static) -> Self {
        self.closer = Some(Box::new(closer));
        self
    }

    #[cfg(unix)]
    pub fn unix(stream: std::os::unix::net::UnixStream) -> io::Result<Self> {
        let reader = stream.try_clone()?;
        let closer = stream.try_clone()?;
        Ok(Transport::new(reader, stream)
            .with_closer(move || closer.shutdown(std::net::Shutdown::Both)))
    }
}

pub trait RequestHandler: Send + Sync + 'static {
    /// Runs on its own thread, so it may block; long work should poll `context.is_cancelled()`.
    fn handle(&self, request: Request, context: &RequestContext) -> Result<Value, RemoteError>;
}

impl<F> RequestHandler for F
where
    F: Fn(Request, &RequestContext) -> Result<Value, RemoteError> + Send + Sync + 'static,
{
    fn handle(&self, request: Request, context: &RequestContext) -> Result<Value, RemoteError> {
        self(request, context)
    }
}

struct RejectAll;

impl RequestHandler for RejectAll {
    fn handle(&self, request: Request, _context: &RequestContext) -> Result<Value, RemoteError> {
        Err(RemoteError::new(
            error_code::METHOD_NOT_FOUND,
            format!("no handler for {}", request.method),
        ))
    }
}

pub struct RequestContext {
    id: u32,
    cancelled: Arc<AtomicBool>,
    connection: Connection,
}

impl RequestContext {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Open,
    Closed(CloseReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PushEvent {
    Value { topic: String, value: Value },
    End { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Data {
        offset: u64,
        bytes: Vec<u8>,
    },
    /// The sender skipped ahead: the next DATA starts at `offset` and what came before it is gone.
    Reset {
        offset: u64,
    },
    End {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputChunk {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Client,
    Server,
}

impl Role {
    /// The client numbers its ids odd and the server even, so ids from either side never collide.
    fn first_id(self) -> u32 {
        match self {
            Role::Client => 1,
            Role::Server => 2,
        }
    }
}

struct IncomingStream {
    events: Sender<StreamEvent>,
    received_end: Option<u64>,
    acked: u64,
    consumed: u64,
    input_offset: u64,
}

enum Outgoing {
    Data(u64, Vec<u8>),
    Reset(u64),
    End(String),
}

struct OutgoingStream {
    queue: VecDeque<Outgoing>,
    queued_offset: u64,
    acked: u64,
    in_ready: bool,
    input: Sender<InputChunk>,
}

type Accept = Box<dyn FnOnce(&Hello) -> Result<Hello, String> + Send>;

struct State {
    closed: Option<CloseReason>,
    peer_hello: Option<Hello>,
    accept: Option<Accept>,
    last_received: Instant,
    next_id: u32,
    pending: HashMap<u32, Sender<Result<Value, Error>>>,
    serving: HashMap<u32, Arc<AtomicBool>>,
    next_ping: u32,
    pings: HashMap<u32, Sender<()>>,
    subscriptions: HashMap<u32, Sender<PushEvent>>,
    publishers: HashSet<u32>,
    incoming: HashMap<u32, IncomingStream>,
    outgoing: HashMap<u32, OutgoingStream>,
    priority: VecDeque<Frame>,
    control: VecDeque<Frame>,
    ready: VecDeque<u32>,
    farewell: Option<Frame>,
    writer_finished: bool,
    close_error: Option<String>,
}

impl State {
    fn open_channels(&self) -> usize {
        self.incoming.len() + self.outgoing.len() + self.subscriptions.len() + self.publishers.len()
    }

    fn check_open(&self) -> Result<(), Error> {
        match &self.closed {
            Some(reason) => Err(Error::Closed(reason.clone())),
            None => Ok(()),
        }
    }
}

struct Inner {
    state: Mutex<State>,
    writer_wakeup: Condvar,
    changed: Condvar,
    config: Config,
    role: Role,
    handler: Arc<dyn RequestHandler>,
    closer: Mutex<Option<Closer>>,
}

/// Why the reader stops: how to record the close, and what END (if any) to send the peer first.
struct Fault {
    close: CloseReason,
    farewell: Option<String>,
}

impl Fault {
    fn quiet(close: CloseReason) -> Self {
        Fault {
            close,
            farewell: None,
        }
    }

    fn violation(message: impl Into<String>) -> Self {
        let message = message.into();
        Fault {
            farewell: Some(format!("{}: {message}", error_code::PROTOCOL_ERROR)),
            close: CloseReason::Protocol(message),
        }
    }
}

fn protocol(error: Error) -> Fault {
    match error {
        Error::Protocol(message) => Fault::violation(message),
        other => Fault::violation(other.to_string()),
    }
}

/// A listener that went away stopped caring on purpose, so a failed hand-off is not an error.
fn offer<T>(sender: &Sender<T>, value: T) -> bool {
    sender.send(value).is_ok()
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait_changed<'a>(
        &self,
        guard: MutexGuard<'a, State>,
        timeout: Duration,
    ) -> MutexGuard<'a, State> {
        match self.changed.wait_timeout(guard, timeout) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        }
    }

    fn is_peer_id(&self, id: u32) -> bool {
        id != CONNECTION_CHANNEL && id % 2 != self.role.first_id() % 2
    }

    fn was_issued_locally(&self, state: &State, id: u32) -> bool {
        id != CONNECTION_CHANNEL && id % 2 == self.role.first_id() % 2 && id < state.next_id
    }

    fn allocate_id(&self, state: &mut State) -> Result<u32, Error> {
        let id = state.next_id;
        state.next_id = id
            .checked_add(2)
            .ok_or_else(|| Error::Limit("connection ran out of ids".to_string()))?;
        Ok(id)
    }

    fn queue_control(&self, state: &mut State, frame: Frame) {
        state.control.push_back(frame);
        self.writer_wakeup.notify_one();
    }

    fn close_reason(&self) -> CloseReason {
        self.lock().closed.clone().unwrap_or(CloseReason::Local)
    }

    fn shutdown(&self, reason: CloseReason, farewell: Option<String>) {
        let graceful = {
            let mut state = self.lock();
            if state.closed.is_some() {
                return;
            }
            state.closed = Some(reason.clone());
            for (_, sender) in state.pending.drain() {
                offer(&sender, Err(Error::Closed(reason.clone())));
            }
            for (_, cancelled) in state.serving.drain() {
                cancelled.store(true, Ordering::Release);
            }
            state.pings.clear();
            state.subscriptions.clear();
            state.publishers.clear();
            state.incoming.clear();
            state.outgoing.clear();
            state.ready.clear();
            state.priority.clear();
            match farewell {
                Some(reason) => {
                    state.farewell = Frame::json(Kind::End, CONNECTION_CHANNEL, &End { reason })
                        .ok()
                        .or_else(|| {
                            Some(Frame::new(Kind::End, CONNECTION_CHANNEL, b"{}".to_vec()))
                        });
                }
                None => state.control.clear(),
            }
            state.farewell.is_some()
        };
        self.writer_wakeup.notify_all();
        self.changed.notify_all();
        if graceful {
            let deadline = Instant::now() + self.config.close_grace;
            let mut state = self.lock();
            while !state.writer_finished {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                state = self.wait_changed(state, deadline - now);
            }
        }
        self.run_closer();
    }

    fn run_closer(&self) {
        let closer = self
            .closer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(closer) = closer else {
            return;
        };
        if let Err(error) = closer() {
            if error.kind() != io::ErrorKind::NotConnected {
                self.lock().close_error = Some(error.to_string());
            }
        }
    }

    fn fail(&self, fault: Fault) {
        self.shutdown(fault.close, fault.farewell);
    }

    fn handshake(&self, reader: &mut impl Read) -> Result<(), Fault> {
        let frame = match read_frame(reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Err(Fault::quiet(CloseReason::PeerGone)),
            Err(ReadError::Io(error)) => {
                return Err(Fault::quiet(CloseReason::Io(error.to_string())))
            }
            Err(ReadError::Protocol(message)) => return Err(Fault::violation(message)),
        };
        match frame.kind {
            Kind::Hello => {}
            Kind::End if frame.channel == CONNECTION_CHANNEL => {
                let end: End = decode(&frame.payload).map_err(protocol)?;
                return Err(Fault::quiet(CloseReason::Peer(end.reason)));
            }
            other => return Err(Fault::violation(format!("expected HELLO, got {other:?}"))),
        }
        let hello: Hello = decode(&frame.payload).map_err(protocol)?;
        if hello.protocol != PROTOCOL_VERSION {
            let message = format!(
                "{}: peer speaks protocol {}, this side {PROTOCOL_VERSION}",
                error_code::PROTOCOL_MISMATCH,
                hello.protocol
            );
            return Err(Fault {
                close: CloseReason::Peer(message.clone()),
                farewell: Some(message),
            });
        }
        let accept = self.lock().accept.take();
        let reply = match accept {
            Some(decide) => Some(decide(&hello).map_err(|reason| Fault {
                close: CloseReason::Local,
                farewell: Some(reason),
            })?),
            None => None,
        };
        let mut state = self.lock();
        state.last_received = Instant::now();
        if let Some(mut reply) = reply {
            reply.protocol = PROTOCOL_VERSION;
            let frame = Frame::json(Kind::Hello, CONNECTION_CHANNEL, &reply).map_err(protocol)?;
            self.queue_control(&mut state, frame);
        }
        state.peer_hello = Some(hello);
        self.changed.notify_all();
        Ok(())
    }

    fn dispatch(self: &Arc<Self>, frame: Frame) -> Result<(), Fault> {
        let mut guard = self.lock();
        if guard.closed.is_some() {
            return Ok(());
        }
        let state = &mut *guard;
        state.last_received = Instant::now();
        let id = frame.channel;
        match frame.kind {
            Kind::Hello => return Err(Fault::violation("HELLO after the handshake")),
            Kind::Request => {
                let request: Request = decode(&frame.payload).map_err(protocol)?;
                if !self.is_peer_id(id) {
                    return Err(Fault::violation(format!(
                        "request id {id} is not the peer's"
                    )));
                }
                if state.serving.contains_key(&id) {
                    return Err(Fault::violation(format!(
                        "request id {id} already in flight"
                    )));
                }
                if state.serving.len() >= MAX_PENDING_REQUESTS {
                    return Err(Fault::violation(format!(
                        "more than {MAX_PENDING_REQUESTS} requests in flight"
                    )));
                }
                let cancelled = Arc::new(AtomicBool::new(false));
                state.serving.insert(id, cancelled.clone());
                drop(guard);
                self.serve(id, request, cancelled);
            }
            Kind::Response => {
                let response: Response = decode(&frame.payload).map_err(protocol)?;
                match state.pending.remove(&id) {
                    Some(sender) => {
                        offer(&sender, response.into_result().map_err(Error::Remote));
                    }
                    None if !self.was_issued_locally(state, id) => {
                        return Err(Fault::violation(format!(
                            "response to unknown request {id}"
                        )));
                    }
                    // Cancelled or already timed out by the caller: a late answer is dropped.
                    None => {}
                }
            }
            Kind::Cancel => {
                if let Some(cancelled) = state.serving.remove(&id) {
                    cancelled.store(true, Ordering::Release);
                    let response = Response::from_result(Err(RemoteError::new(
                        error_code::CANCELLED,
                        "cancelled by the caller",
                    )));
                    let frame = Frame::json(Kind::Response, id, &response).map_err(protocol)?;
                    self.queue_control(state, frame);
                }
            }
            Kind::Push => {
                let push: Push = decode(&frame.payload).map_err(protocol)?;
                match state.subscriptions.get(&id) {
                    Some(sender) => {
                        let event = PushEvent::Value {
                            topic: push.topic,
                            value: push.value,
                        };
                        if !offer(sender, event) {
                            state.subscriptions.remove(&id);
                        }
                    }
                    None if !self.was_issued_locally(state, id) => {
                        return Err(Fault::violation(format!("push on unknown channel {id}")));
                    }
                    None => {}
                }
            }
            Kind::End => {
                let end: End = decode(&frame.payload).map_err(protocol)?;
                if id == CONNECTION_CHANNEL {
                    drop(guard);
                    self.shutdown(CloseReason::Peer(end.reason), None);
                    return Ok(());
                }
                if let Some(sender) = state.subscriptions.remove(&id) {
                    offer(&sender, PushEvent::End { reason: end.reason });
                } else if let Some(stream) = state.incoming.remove(&id) {
                    offer(&stream.events, StreamEvent::End { reason: end.reason });
                } else if state.outgoing.remove(&id).is_some() || state.publishers.remove(&id) {
                    self.changed.notify_all();
                }
            }
            Kind::Data => {
                let (offset, bytes) = frame.offset_and_bytes().map_err(protocol)?;
                let window = self.config.window;
                let Some(stream) = state.incoming.get_mut(&id) else {
                    if self.was_issued_locally(state, id) {
                        return Ok(());
                    }
                    return Err(Fault::violation(format!("DATA on unknown channel {id}")));
                };
                match stream.received_end {
                    Some(expected) if expected != offset => {
                        return Err(Fault::violation(format!(
                            "DATA at offset {offset} on channel {id}, expected {expected}"
                        )));
                    }
                    Some(_) => {}
                    None => {
                        stream.acked = offset;
                        stream.consumed = offset;
                    }
                }
                let end = offset
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(|| Fault::violation("DATA offset overflows"))?;
                if end - stream.acked > window {
                    return Err(Fault::violation(format!(
                        "DATA on channel {id} exceeds the {window} byte credit window"
                    )));
                }
                stream.received_end = Some(end);
                let event = StreamEvent::Data {
                    offset,
                    bytes: bytes.to_vec(),
                };
                if !offer(&stream.events, event) {
                    state.incoming.remove(&id);
                }
            }
            Kind::Input => {
                let (offset, bytes) = frame.offset_and_bytes().map_err(protocol)?;
                match state.outgoing.get(&id) {
                    Some(stream) => {
                        let chunk = InputChunk {
                            offset,
                            bytes: bytes.to_vec(),
                        };
                        offer(&stream.input, chunk);
                    }
                    None if !self.is_peer_id(id) => {
                        return Err(Fault::violation(format!("INPUT on unknown channel {id}")));
                    }
                    None => {}
                }
            }
            Kind::Ack => {
                let offset = frame.offset().map_err(protocol)?;
                if let Some(stream) = state.outgoing.get_mut(&id) {
                    if offset > stream.queued_offset {
                        return Err(Fault::violation(format!(
                            "ACK of offset {offset} on channel {id}, only {} sent",
                            stream.queued_offset
                        )));
                    }
                    if offset > stream.acked {
                        stream.acked = offset;
                        self.changed.notify_all();
                    }
                } else if !self.is_peer_id(id) {
                    return Err(Fault::violation(format!("ACK on unknown channel {id}")));
                }
            }
            Kind::Reset => {
                let offset = frame.offset().map_err(protocol)?;
                let issued = self.was_issued_locally(state, id);
                match state.incoming.get_mut(&id) {
                    Some(stream) => {
                        stream.received_end = Some(offset);
                        stream.acked = offset;
                        stream.consumed = offset;
                        if !offer(&stream.events, StreamEvent::Reset { offset }) {
                            state.incoming.remove(&id);
                        }
                    }
                    None if !issued => {
                        return Err(Fault::violation(format!("RESET on unknown channel {id}")));
                    }
                    None => {}
                }
            }
            Kind::Ping => {
                state
                    .priority
                    .push_back(Frame::new(Kind::Pong, id, Vec::new()));
                self.writer_wakeup.notify_one();
            }
            Kind::Pong => {
                if let Some(sender) = state.pings.remove(&id) {
                    offer(&sender, ());
                }
            }
        }
        Ok(())
    }

    fn serve(self: &Arc<Self>, id: u32, request: Request, cancelled: Arc<AtomicBool>) {
        let connection = Connection {
            inner: self.clone(),
        };
        let spawned = thread::Builder::new()
            .name("pom-rpc-handler".to_string())
            .spawn(move || {
                let context = RequestContext {
                    id,
                    cancelled,
                    connection,
                };
                let handler = context.connection.inner.handler.clone();
                let result = handler.handle(request, &context);
                context.connection.inner.finish_serving(id, result);
            });
        if let Err(error) = spawned {
            self.finish_serving(
                id,
                Err(RemoteError::new(error_code::INTERNAL, error.to_string())),
            );
        }
    }

    fn finish_serving(&self, id: u32, result: Result<Value, RemoteError>) {
        let mut state = self.lock();
        if state.serving.remove(&id).is_none() {
            return;
        }
        let frame =
            Frame::json(Kind::Response, id, &Response::from_result(result)).or_else(|error| {
                let failure = Response::from_result(Err(RemoteError::new(
                    error_code::INTERNAL,
                    format!("response not sendable: {error}"),
                )));
                Frame::json(Kind::Response, id, &failure)
            });
        match frame {
            Ok(frame) => self.queue_control(&mut state, frame),
            Err(error) => {
                drop(state);
                self.shutdown(CloseReason::Io(error.to_string()), None);
            }
        }
    }

    fn next_frame(&self, state: &mut State) -> Option<Frame> {
        if let Some(frame) = state.priority.pop_front() {
            return Some(frame);
        }
        if let Some(frame) = state.control.pop_front() {
            return Some(frame);
        }
        while let Some(channel) = state.ready.pop_front() {
            let Some(stream) = state.outgoing.get_mut(&channel) else {
                continue;
            };
            stream.in_ready = false;
            let Some(item) = stream.queue.pop_front() else {
                continue;
            };
            let frame = match item {
                Outgoing::Data(offset, mut bytes) => {
                    while let Some(Outgoing::Data(_, next)) = stream.queue.front() {
                        if bytes.len() + next.len() > MAX_DATA {
                            break;
                        }
                        if let Some(Outgoing::Data(_, next)) = stream.queue.pop_front() {
                            bytes.extend_from_slice(&next);
                        }
                    }
                    Frame::bytes(Kind::Data, channel, offset, &bytes)
                }
                Outgoing::Reset(offset) => Ok(Frame::offset_only(Kind::Reset, channel, offset)),
                Outgoing::End(reason) => {
                    state.outgoing.remove(&channel);
                    return Frame::json(Kind::End, channel, &End { reason }).ok();
                }
            };
            if !stream.queue.is_empty() {
                stream.in_ready = true;
                state.ready.push_back(channel);
            }
            if let Ok(frame) = frame {
                return Some(frame);
            }
        }
        None
    }

    fn writer_loop(&self, mut writer: Box<dyn Write + Send>) {
        loop {
            let frame = {
                let mut guard = self.lock();
                loop {
                    let state = &mut *guard;
                    if state.closed.is_some() {
                        let frame = state.control.pop_front().or_else(|| state.farewell.take());
                        if frame.is_none() {
                            state.writer_finished = true;
                            self.changed.notify_all();
                        }
                        break frame;
                    }
                    if let Some(frame) = self.next_frame(state) {
                        break Some(frame);
                    }
                    guard = match self.writer_wakeup.wait(guard) {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                }
            };
            let Some(frame) = frame else {
                return;
            };
            if let Err(error) = write_frame(&mut writer, &frame) {
                self.shutdown(CloseReason::Io(error.to_string()), None);
                let mut state = self.lock();
                state.writer_finished = true;
                self.changed.notify_all();
                return;
            }
        }
    }

    fn reader_loop(self: &Arc<Self>, mut reader: Box<dyn Read + Send>) {
        if let Err(fault) = self.handshake(&mut reader) {
            self.fail(fault);
            return;
        }
        loop {
            match read_frame(&mut reader) {
                Ok(Some(frame)) => {
                    if let Err(fault) = self.dispatch(frame) {
                        self.fail(fault);
                        return;
                    }
                }
                Ok(None) => {
                    self.shutdown(CloseReason::PeerGone, None);
                    return;
                }
                Err(ReadError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    self.shutdown(CloseReason::PeerGone, None);
                    return;
                }
                Err(ReadError::Io(error)) => {
                    self.shutdown(CloseReason::Io(error.to_string()), None);
                    return;
                }
                Err(ReadError::Protocol(message)) => {
                    self.fail(Fault::violation(message));
                    return;
                }
            }
        }
    }

    fn heartbeat_loop(&self) {
        let interval = self.config.heartbeat_interval;
        let dead_after = self.config.dead_after;
        let tick = interval.min(dead_after / 4).max(Duration::from_millis(1));
        let mut last_ping = Instant::now();
        let mut state = self.lock();
        loop {
            if state.closed.is_some() {
                return;
            }
            if state.last_received.elapsed() >= dead_after {
                drop(state);
                self.shutdown(CloseReason::HeartbeatTimeout, None);
                return;
            }
            if last_ping.elapsed() >= interval {
                let id = state.next_ping;
                state.next_ping = id.wrapping_add(1);
                self.queue_control(&mut state, Frame::new(Kind::Ping, id, Vec::new()));
                last_ping = Instant::now();
            }
            state = self.wait_changed(state, tick);
        }
    }
}

/// A cheap handle; clones share one connection. Threads keep it alive until it closes, so call `close`.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl fmt::Debug for Connection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Connection")
            .field("state", &self.state())
            .finish()
    }
}

pub struct ConnectionBuilder {
    transport: Transport,
    config: Config,
    handler: Arc<dyn RequestHandler>,
}

impl ConnectionBuilder {
    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    pub fn handler(mut self, handler: impl RequestHandler) -> Self {
        self.handler = Arc::new(handler);
        self
    }

    /// Sends `hello` and blocks until the peer answers with its own.
    pub fn connect(self, hello: Hello) -> Result<(Connection, Hello), Error> {
        let frame = Frame::json(Kind::Hello, CONNECTION_CHANNEL, &hello)?;
        self.start(Role::Client, Some(frame), None)
    }

    /// Blocks until the client's HELLO arrives; `decide` returns this side's HELLO or a refusal reason.
    pub fn accept(
        self,
        decide: impl FnOnce(&Hello) -> Result<Hello, String> + Send + 'static,
    ) -> Result<(Connection, Hello), Error> {
        self.start(Role::Server, None, Some(Box::new(decide)))
    }

    fn start(
        self,
        role: Role,
        hello: Option<Frame>,
        accept: Option<Accept>,
    ) -> Result<(Connection, Hello), Error> {
        let Transport {
            reader,
            writer,
            closer,
        } = self.transport;
        let state = State {
            closed: None,
            peer_hello: None,
            accept,
            last_received: Instant::now(),
            next_id: role.first_id(),
            pending: HashMap::new(),
            serving: HashMap::new(),
            next_ping: 1,
            pings: HashMap::new(),
            subscriptions: HashMap::new(),
            publishers: HashSet::new(),
            incoming: HashMap::new(),
            outgoing: HashMap::new(),
            priority: VecDeque::new(),
            control: hello.into_iter().collect(),
            ready: VecDeque::new(),
            farewell: None,
            writer_finished: false,
            close_error: None,
        };
        let inner = Arc::new(Inner {
            state: Mutex::new(state),
            writer_wakeup: Condvar::new(),
            changed: Condvar::new(),
            config: self.config,
            role,
            handler: self.handler,
            closer: Mutex::new(closer),
        });
        let connection = Connection {
            inner: inner.clone(),
        };
        connection.spawn("pom-rpc-writer", move |inner| inner.writer_loop(writer))?;
        connection.spawn("pom-rpc-reader", move |inner| inner.reader_loop(reader))?;

        let deadline = Instant::now() + inner.config.handshake_timeout;
        let peer_hello = {
            let mut state = inner.lock();
            loop {
                if let Some(reason) = &state.closed {
                    return Err(Error::Closed(reason.clone()));
                }
                if let Some(hello) = &state.peer_hello {
                    break hello.clone();
                }
                let now = Instant::now();
                if now >= deadline {
                    drop(state);
                    inner.shutdown(CloseReason::HandshakeTimeout, None);
                    return Err(Error::Timeout);
                }
                state = inner.wait_changed(state, deadline - now);
            }
        };
        connection.spawn("pom-rpc-heartbeat", |inner| inner.heartbeat_loop())?;
        Ok((connection, peer_hello))
    }
}

/// An answer still owed by the peer. Dropping it unanswered cancels the request.
pub struct PendingRequest {
    id: u32,
    receiver: Receiver<Result<Value, Error>>,
    inner: Arc<Inner>,
    finished: bool,
}

impl PendingRequest {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn wait(mut self) -> Result<Value, Error> {
        self.finished = true;
        match self.receiver.recv() {
            Ok(result) => result,
            Err(_) => Err(Error::Closed(self.inner.close_reason())),
        }
    }

    pub fn wait_timeout(&mut self, timeout: Duration) -> Result<Value, Error> {
        match self.receiver.recv_timeout(timeout) {
            Ok(result) => {
                self.finished = true;
                result
            }
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(RecvTimeoutError::Disconnected) => {
                self.finished = true;
                Err(Error::Closed(self.inner.close_reason()))
            }
        }
    }

    pub fn cancel(&self) {
        self.canceller().cancel();
    }

    pub fn forget(mut self) {
        self.finished = true;
    }

    /// For cancelling from another thread while this one waits.
    pub fn canceller(&self) -> Canceller {
        Canceller {
            id: self.id,
            inner: self.inner.clone(),
        }
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        if !self.finished {
            self.cancel();
        }
    }
}

#[derive(Clone)]
pub struct Canceller {
    id: u32,
    inner: Arc<Inner>,
}

impl Canceller {
    /// The waiter gets `Error::Cancelled` at once; whatever the peer answers later is dropped.
    pub fn cancel(&self) {
        let mut state = self.inner.lock();
        let Some(sender) = state.pending.remove(&self.id) else {
            return;
        };
        offer(&sender, Err(Error::Cancelled));
        if state.closed.is_none() {
            self.inner
                .queue_control(&mut state, Frame::new(Kind::Cancel, self.id, Vec::new()));
        }
    }
}

pub struct Subscription {
    channel: u32,
    receiver: Receiver<PushEvent>,
    inner: Arc<Inner>,
}

impl Subscription {
    /// Pass this to the peer (in a request) so it can publish on it.
    pub fn channel(&self) -> u32 {
        self.channel
    }

    pub fn recv(&self) -> Result<PushEvent, Error> {
        self.receiver.recv().map_err(|_| ended_error(&self.inner))
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<PushEvent, Error> {
        match self.receiver.recv_timeout(timeout) {
            Ok(event) => Ok(event),
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ended_error(&self.inner)),
        }
    }

    pub fn try_recv(&self) -> Result<Option<PushEvent>, Error> {
        match self.receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ended_error(&self.inner)),
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut state = self.inner.lock();
        if state.subscriptions.remove(&self.channel).is_some() && state.closed.is_none() {
            end_channel(&self.inner, &mut state, self.channel, "unsubscribed");
        }
    }
}

pub struct Publisher {
    channel: u32,
    inner: Arc<Inner>,
    ended: bool,
}

impl Publisher {
    pub fn channel(&self) -> u32 {
        self.channel
    }

    pub fn publish(&self, topic: &str, value: impl Serialize) -> Result<(), Error> {
        let value =
            serde_json::to_value(value).map_err(|error| Error::Protocol(error.to_string()))?;
        let push = Push {
            topic: topic.to_string(),
            value,
        };
        let frame = Frame::json(Kind::Push, self.channel, &push)?;
        let mut state = self.inner.lock();
        state.check_open()?;
        if !state.publishers.contains(&self.channel) {
            return Err(Error::ChannelEnded("the subscriber left".to_string()));
        }
        self.inner.queue_control(&mut state, frame);
        Ok(())
    }

    pub fn is_ended(&self) -> bool {
        !self.inner.lock().publishers.contains(&self.channel)
    }

    pub fn finish(mut self, reason: &str) {
        self.end(reason);
    }

    fn end(&mut self, reason: &str) {
        if self.ended {
            return;
        }
        self.ended = true;
        let mut state = self.inner.lock();
        if state.publishers.remove(&self.channel) && state.closed.is_none() {
            end_channel(&self.inner, &mut state, self.channel, reason);
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.end("closed");
    }
}

/// The receiving end of a stream. Call `ack` as bytes are consumed, or the sender stalls at the window.
pub struct StreamReceiver {
    channel: u32,
    receiver: Receiver<StreamEvent>,
    inner: Arc<Inner>,
}

impl StreamReceiver {
    /// Pass this to the peer (in a request) so it can attach a sender to it.
    pub fn channel(&self) -> u32 {
        self.channel
    }

    pub fn recv(&self) -> Result<StreamEvent, Error> {
        self.receiver.recv().map_err(|_| ended_error(&self.inner))
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<StreamEvent, Error> {
        match self.receiver.recv_timeout(timeout) {
            Ok(event) => Ok(event),
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ended_error(&self.inner)),
        }
    }

    pub fn try_recv(&self) -> Result<Option<StreamEvent>, Error> {
        match self.receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ended_error(&self.inner)),
        }
    }

    /// Marks everything before the absolute `offset` as consumed. ACKs go out in batches, never more than a
    /// quarter window apart, so the sender never waits on a batch.
    pub fn ack(&self, offset: u64) {
        acknowledge(&self.inner, self.channel, offset);
    }

    /// Input numbering starts at 0; set it when resuming so the peer can drop bytes it already has.
    pub fn set_input_offset(&self, offset: u64) {
        if let Some(stream) = self.inner.lock().incoming.get_mut(&self.channel) {
            stream.input_offset = offset;
        }
    }

    /// Sent ahead of stream data: input is small and latency matters more than fairness.
    pub fn send_input(&self, bytes: &[u8]) -> Result<(), Error> {
        send_input(&self.inner, self.channel, bytes)
    }

    pub fn handle(&self) -> StreamHandle {
        StreamHandle {
            channel: self.channel,
            inner: self.inner.clone(),
        }
    }
}

#[derive(Clone)]
pub struct StreamHandle {
    channel: u32,
    inner: Arc<Inner>,
}

impl StreamHandle {
    pub fn channel(&self) -> u32 {
        self.channel
    }

    pub fn ack(&self, offset: u64) {
        acknowledge(&self.inner, self.channel, offset);
    }

    pub fn send_input(&self, bytes: &[u8]) -> Result<(), Error> {
        send_input(&self.inner, self.channel, bytes)
    }
}

fn acknowledge(inner: &Inner, channel: u32, offset: u64) {
    let window = inner.config.window;
    let mut state = inner.lock();
    if state.closed.is_some() {
        return;
    }
    let Some(stream) = state.incoming.get_mut(&channel) else {
        return;
    };
    let offset = offset.min(stream.received_end.unwrap_or(stream.consumed));
    if offset <= stream.consumed {
        return;
    }
    stream.consumed = offset;
    if stream.consumed - stream.acked < ACK_BATCH.min(window / 4).max(1) {
        return;
    }
    stream.acked = stream.consumed;
    let frame = Frame::offset_only(Kind::Ack, channel, stream.acked);
    inner.queue_control(&mut state, frame);
}

fn send_input(inner: &Inner, channel: u32, bytes: &[u8]) -> Result<(), Error> {
    let mut guard = inner.lock();
    guard.check_open()?;
    let state = &mut *guard;
    let Some(stream) = state.incoming.get_mut(&channel) else {
        return Err(Error::ChannelEnded("the stream ended".to_string()));
    };
    let mut frames = Vec::new();
    for chunk in bytes.chunks(MAX_DATA) {
        frames.push(Frame::bytes(
            Kind::Input,
            channel,
            stream.input_offset,
            chunk,
        )?);
        stream.input_offset += chunk.len() as u64;
    }
    state.priority.extend(frames);
    inner.writer_wakeup.notify_one();
    Ok(())
}

impl Drop for StreamReceiver {
    fn drop(&mut self) {
        let mut state = self.inner.lock();
        if state.incoming.remove(&self.channel).is_some() && state.closed.is_none() {
            end_channel(&self.inner, &mut state, self.channel, "closed");
        }
    }
}

/// The sending end of a stream, attached to a channel the peer opened with `open_stream`.
pub struct StreamSender {
    channel: u32,
    inner: Arc<Inner>,
    input: Option<Receiver<InputChunk>>,
    finished: bool,
}

impl StreamSender {
    pub fn channel(&self) -> u32 {
        self.channel
    }

    /// Blocks while the window is full; returns once every byte is queued.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.write_until(bytes, None).map(|_| ())
    }

    /// Queues what the current credit allows without blocking and returns how many bytes that was.
    pub fn try_write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        self.write_until(bytes, Some(Instant::now()))
    }

    fn write_until(&mut self, bytes: &[u8], deadline: Option<Instant>) -> Result<usize, Error> {
        let window = self.inner.config.window;
        let mut rest = bytes;
        let mut guard = self.inner.lock();
        while !rest.is_empty() {
            guard.check_open()?;
            let state = &mut *guard;
            let Some(stream) = state.outgoing.get_mut(&self.channel) else {
                return Err(Error::ChannelEnded("the receiver left".to_string()));
            };
            let available = window.saturating_sub(stream.queued_offset - stream.acked);
            if available == 0 {
                match deadline {
                    Some(deadline) => {
                        let now = Instant::now();
                        if now >= deadline {
                            break;
                        }
                        guard = self.inner.wait_changed(guard, deadline - now);
                    }
                    None => guard = self.inner.wait_changed(guard, Duration::from_secs(1)),
                }
                continue;
            }
            let take = usize::try_from(available)
                .unwrap_or(usize::MAX)
                .min(rest.len())
                .min(MAX_DATA);
            let Some((chunk, remaining)) = rest.split_at_checked(take) else {
                break;
            };
            stream
                .queue
                .push_back(Outgoing::Data(stream.queued_offset, chunk.to_vec()));
            stream.queued_offset += take as u64;
            if !stream.in_ready {
                stream.in_ready = true;
                state.ready.push_back(self.channel);
            }
            self.inner.writer_wakeup.notify_one();
            rest = remaining;
        }
        Ok(bytes.len() - rest.len())
    }

    /// Bytes that can be queued right now without blocking.
    pub fn credit(&self) -> u64 {
        let state = self.inner.lock();
        state
            .outgoing
            .get(&self.channel)
            .map(|stream| {
                self.inner
                    .config
                    .window
                    .saturating_sub(stream.queued_offset - stream.acked)
            })
            .unwrap_or(0)
    }

    /// The absolute offset the next written byte gets.
    pub fn offset(&self) -> u64 {
        let state = self.inner.lock();
        state
            .outgoing
            .get(&self.channel)
            .map(|stream| stream.queued_offset)
            .unwrap_or(0)
    }

    /// Skips ahead to `offset` (bytes the receiver will never get); offsets only move forward.
    pub fn reset(&mut self, offset: u64) -> Result<(), Error> {
        let mut guard = self.inner.lock();
        guard.check_open()?;
        let state = &mut *guard;
        let Some(stream) = state.outgoing.get_mut(&self.channel) else {
            return Err(Error::ChannelEnded("the receiver left".to_string()));
        };
        if offset < stream.queued_offset {
            return Err(Error::Protocol(format!(
                "reset to {offset} moves back from {}",
                stream.queued_offset
            )));
        }
        stream.queue.push_back(Outgoing::Reset(offset));
        stream.queued_offset = offset;
        stream.acked = offset;
        if !stream.in_ready {
            stream.in_ready = true;
            state.ready.push_back(self.channel);
        }
        self.inner.writer_wakeup.notify_one();
        self.inner.changed.notify_all();
        Ok(())
    }

    pub fn recv_input(&self) -> Result<InputChunk, Error> {
        self.input_receiver()?
            .recv()
            .map_err(|_| ended_error(&self.inner))
    }

    pub fn recv_input_timeout(&self, timeout: Duration) -> Result<InputChunk, Error> {
        match self.input_receiver()?.recv_timeout(timeout) {
            Ok(chunk) => Ok(chunk),
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ended_error(&self.inner)),
        }
    }

    pub fn try_recv_input(&self) -> Result<Option<InputChunk>, Error> {
        match self.input_receiver()?.try_recv() {
            Ok(chunk) => Ok(Some(chunk)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ended_error(&self.inner)),
        }
    }

    fn input_receiver(&self) -> Result<&Receiver<InputChunk>, Error> {
        self.input
            .as_ref()
            .ok_or_else(|| Error::ChannelEnded("the input was taken".to_string()))
    }

    pub fn take_input(&mut self) -> Option<InputReceiver> {
        self.input.take().map(|receiver| InputReceiver {
            receiver,
            inner: self.inner.clone(),
        })
    }

    pub fn wait_credit(&self, at_least: u64, timeout: Duration) -> Result<u64, Error> {
        let window = self.inner.config.window;
        let deadline = Instant::now() + timeout;
        let mut guard = self.inner.lock();
        loop {
            guard.check_open()?;
            let Some(stream) = guard.outgoing.get(&self.channel) else {
                return Err(Error::ChannelEnded("the receiver left".to_string()));
            };
            let credit = window.saturating_sub(stream.queued_offset - stream.acked);
            let now = Instant::now();
            if credit >= at_least.min(window) || now >= deadline {
                return Ok(credit);
            }
            guard = self.inner.wait_changed(guard, deadline - now);
        }
    }

    pub fn is_ended(&self) -> bool {
        !self.inner.lock().outgoing.contains_key(&self.channel)
    }

    /// Ends the stream after every queued byte has gone out.
    pub fn finish(mut self, reason: &str) {
        self.end(reason);
    }

    fn end(&mut self, reason: &str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let mut guard = self.inner.lock();
        let state = &mut *guard;
        if state.closed.is_some() {
            return;
        }
        if let Some(stream) = state.outgoing.get_mut(&self.channel) {
            stream.queue.push_back(Outgoing::End(reason.to_string()));
            if !stream.in_ready {
                stream.in_ready = true;
                state.ready.push_back(self.channel);
            }
            self.inner.writer_wakeup.notify_one();
        }
    }
}

impl Drop for StreamSender {
    fn drop(&mut self) {
        self.end("closed");
    }
}

pub struct InputReceiver {
    receiver: Receiver<InputChunk>,
    inner: Arc<Inner>,
}

impl InputReceiver {
    pub fn recv(&self) -> Result<InputChunk, Error> {
        self.receiver.recv().map_err(|_| ended_error(&self.inner))
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<InputChunk, Error> {
        match self.receiver.recv_timeout(timeout) {
            Ok(chunk) => Ok(chunk),
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ended_error(&self.inner)),
        }
    }
}

fn end_channel(inner: &Inner, state: &mut State, channel: u32, reason: &str) {
    let end = End {
        reason: reason.to_string(),
    };
    if let Ok(frame) = Frame::json(Kind::End, channel, &end) {
        inner.queue_control(state, frame);
    }
}

fn ended_error(inner: &Inner) -> Error {
    match &inner.lock().closed {
        Some(reason) => Error::Closed(reason.clone()),
        None => Error::ChannelEnded("ended".to_string()),
    }
}

impl Connection {
    pub fn builder(transport: Transport) -> ConnectionBuilder {
        ConnectionBuilder {
            transport,
            config: Config::default(),
            handler: Arc::new(RejectAll),
        }
    }

    fn spawn(
        &self,
        name: &str,
        body: impl FnOnce(Arc<Inner>) + Send + 'static,
    ) -> Result<(), Error> {
        let inner = self.inner.clone();
        match thread::Builder::new()
            .name(name.to_string())
            .spawn(move || body(inner))
        {
            Ok(_) => Ok(()),
            Err(error) => {
                self.inner
                    .shutdown(CloseReason::Io(error.to_string()), None);
                Err(Error::Io(error.to_string()))
            }
        }
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// Whether both handles are the same connection.
    pub fn ptr_eq(&self, other: &Connection) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn state(&self) -> ConnectionState {
        match &self.inner.lock().closed {
            Some(reason) => ConnectionState::Closed(reason.clone()),
            None => ConnectionState::Open,
        }
    }

    /// Blocks until the connection closes or `timeout` passes; `None` means it is still open.
    pub fn wait_closed(&self, timeout: Duration) -> Option<CloseReason> {
        let deadline = Instant::now() + timeout;
        let mut state = self.inner.lock();
        loop {
            if let Some(reason) = &state.closed {
                return Some(reason.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            state = self.inner.wait_changed(state, deadline - now);
        }
    }

    /// Tells the peer with an END, fails everything pending, then tears the transport down.
    pub fn close(&self) {
        self.inner
            .shutdown(CloseReason::Local, Some("closed".to_string()));
    }

    /// An error from tearing the transport down, kept because nothing else could report it.
    pub fn close_error(&self) -> Option<String> {
        self.inner.lock().close_error.clone()
    }

    pub fn request(&self, method: &str, params: impl Serialize) -> Result<Value, Error> {
        self.start_request(method, params)?.wait()
    }

    pub fn start_request(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<PendingRequest, Error> {
        let params =
            serde_json::to_value(params).map_err(|error| Error::Protocol(error.to_string()))?;
        let request = Request {
            method: method.to_string(),
            params,
        };
        let (sender, receiver) = mpsc::channel();
        let mut state = self.inner.lock();
        state.check_open()?;
        if state.pending.len() >= MAX_PENDING_REQUESTS {
            return Err(Error::Limit(format!(
                "{MAX_PENDING_REQUESTS} requests already in flight"
            )));
        }
        let id = state.next_id;
        let frame = Frame::json(Kind::Request, id, &request)?;
        self.inner.allocate_id(&mut state)?;
        state.pending.insert(id, sender);
        self.inner.queue_control(&mut state, frame);
        Ok(PendingRequest {
            id,
            receiver,
            inner: self.inner.clone(),
            finished: false,
        })
    }

    /// Round trip of one PING, answered ahead of any queued stream data.
    pub fn ping(&self, timeout: Duration) -> Result<Duration, Error> {
        let (sender, receiver) = mpsc::channel();
        let started = Instant::now();
        let id = {
            let mut state = self.inner.lock();
            state.check_open()?;
            let id = state.next_ping;
            state.next_ping = id.wrapping_add(1);
            state.pings.insert(id, sender);
            self.inner
                .queue_control(&mut state, Frame::new(Kind::Ping, id, Vec::new()));
            id
        };
        match receiver.recv_timeout(timeout) {
            Ok(()) => Ok(started.elapsed()),
            Err(RecvTimeoutError::Timeout) => {
                self.inner.lock().pings.remove(&id);
                Err(Error::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(Error::Closed(self.inner.close_reason())),
        }
    }

    pub fn subscribe(&self) -> Result<Subscription, Error> {
        let (sender, receiver) = mpsc::channel();
        let mut state = self.inner.lock();
        state.check_open()?;
        check_channel_room(&state)?;
        let channel = self.inner.allocate_id(&mut state)?;
        state.subscriptions.insert(channel, sender);
        Ok(Subscription {
            channel,
            receiver,
            inner: self.inner.clone(),
        })
    }

    /// Publishes on a channel the peer opened with `subscribe`.
    pub fn publisher(&self, channel: u32) -> Result<Publisher, Error> {
        let mut state = self.inner.lock();
        state.check_open()?;
        self.check_peer_channel(&state, channel)?;
        state.publishers.insert(channel);
        Ok(Publisher {
            channel,
            inner: self.inner.clone(),
            ended: false,
        })
    }

    pub fn open_stream(&self) -> Result<StreamReceiver, Error> {
        let (sender, receiver) = mpsc::channel();
        let mut state = self.inner.lock();
        state.check_open()?;
        check_channel_room(&state)?;
        let channel = self.inner.allocate_id(&mut state)?;
        state.incoming.insert(
            channel,
            IncomingStream {
                events: sender,
                received_end: None,
                acked: 0,
                consumed: 0,
                input_offset: 0,
            },
        );
        Ok(StreamReceiver {
            channel,
            receiver,
            inner: self.inner.clone(),
        })
    }

    /// Sends on a channel the peer opened with `open_stream`; the first byte gets `start_offset`.
    pub fn attach_stream(&self, channel: u32, start_offset: u64) -> Result<StreamSender, Error> {
        let (sender, receiver) = mpsc::channel();
        let mut state = self.inner.lock();
        state.check_open()?;
        self.check_peer_channel(&state, channel)?;
        state.outgoing.insert(
            channel,
            OutgoingStream {
                queue: VecDeque::new(),
                queued_offset: start_offset,
                acked: start_offset,
                in_ready: false,
                input: sender,
            },
        );
        Ok(StreamSender {
            channel,
            inner: self.inner.clone(),
            input: Some(receiver),
            finished: false,
        })
    }

    fn check_peer_channel(&self, state: &State, channel: u32) -> Result<(), Error> {
        if !self.inner.is_peer_id(channel) {
            return Err(Error::Protocol(format!(
                "channel {channel} was not opened by the peer"
            )));
        }
        if state.outgoing.contains_key(&channel) || state.publishers.contains(&channel) {
            return Err(Error::Protocol(format!(
                "channel {channel} is already attached"
            )));
        }
        check_channel_room(state)
    }
}

fn check_channel_room(state: &State) -> Result<(), Error> {
    if state.open_channels() >= MAX_CHANNELS {
        return Err(Error::Limit(format!(
            "{MAX_CHANNELS} channels already open"
        )));
    }
    Ok(())
}
