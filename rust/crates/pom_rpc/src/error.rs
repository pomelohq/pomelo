use std::fmt;
use std::io;

use crate::envelope::RemoteError;

/// Why a connection stopped. Every pending request, stream and subscription fails with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// This side called `close`.
    Local,
    /// The peer sent END on the connection channel; carries its reason.
    Peer(String),
    /// The transport hit end of file: the peer process or the pipe is gone.
    PeerGone,
    /// No frame arrived within the configured `dead_after`.
    HeartbeatTimeout,
    /// The handshake did not finish within the configured `handshake_timeout`.
    HandshakeTimeout,
    Io(String),
    /// The peer broke the protocol; it was told so with a `protocol_error` END.
    Protocol(String),
}

impl fmt::Display for CloseReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CloseReason::Local => write!(formatter, "closed locally"),
            CloseReason::Peer(reason) => write!(formatter, "closed by peer: {reason}"),
            CloseReason::PeerGone => write!(formatter, "peer gone"),
            CloseReason::HeartbeatTimeout => write!(formatter, "peer stopped answering"),
            CloseReason::HandshakeTimeout => write!(formatter, "handshake timed out"),
            CloseReason::Io(message) => write!(formatter, "transport error: {message}"),
            CloseReason::Protocol(message) => write!(formatter, "protocol error: {message}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The peer answered with an error.
    Remote(RemoteError),
    Cancelled,
    Closed(CloseReason),
    /// The peer ended this stream or subscription.
    ChannelEnded(String),
    /// A frame or payload that does not follow the protocol.
    Protocol(String),
    /// A per-connection limit (pending requests, open channels, frame size) would be exceeded.
    Limit(String),
    Timeout,
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Remote(error) => write!(formatter, "{}: {}", error.code, error.message),
            Error::Cancelled => write!(formatter, "cancelled"),
            Error::Closed(reason) => write!(formatter, "connection closed ({reason})"),
            Error::ChannelEnded(reason) => write!(formatter, "channel ended: {reason}"),
            Error::Protocol(message) => write!(formatter, "protocol error: {message}"),
            Error::Limit(message) => write!(formatter, "limit exceeded: {message}"),
            Error::Timeout => write!(formatter, "timed out"),
            Error::Io(message) => write!(formatter, "i/o error: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error.to_string())
    }
}
