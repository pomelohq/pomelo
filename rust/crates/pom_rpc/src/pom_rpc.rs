//! The protocol between the app and a long-lived `pom server` on another machine, carried over any byte pipe
//! (an ssh session in production, a socket pair in tests).

mod connection;
mod envelope;
mod error;
mod frame;
#[cfg(unix)]
pub mod mock;
pub mod pty;
mod resuming;

pub use connection::{
    Canceller, Config, Connection, ConnectionBuilder, ConnectionState, InputChunk, InputReceiver,
    PendingRequest, Publisher, PushEvent, RequestContext, RequestHandler, StreamEvent,
    StreamHandle, StreamReceiver, StreamSender, Subscription, Transport, DEFAULT_WINDOW,
    MAX_CHANNELS, MAX_PENDING_REQUESTS,
};
pub use envelope::{
    agreed_protocol, decode, error_code, End, Hello, Mismatch, Push, RemoteError, Request,
    Response, MIN_PROTOCOL, PROTOCOL_VERSION,
};
pub use error::{CloseReason, Error};
pub use frame::{read_frame, write_frame, Frame, Kind, ReadError, MAX_DATA, MAX_FRAME};
