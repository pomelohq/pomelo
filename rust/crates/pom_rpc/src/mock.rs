//! In-process transports for tests: a socket pair, and a wrapper that injects latency, slow reads and cuts.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::connection::{Config, Connection, RequestHandler, Transport};
use crate::envelope::Hello;
use crate::error::Error;

pub fn pair() -> io::Result<(Transport, Transport)> {
    faulty_pair(Faults::default(), Faults::default())
}

pub fn faulty_pair(client: Faults, server: Faults) -> io::Result<(Transport, Transport)> {
    let (client_stream, server_stream) = UnixStream::pair()?;
    Ok((
        faulty_transport(client_stream, client)?,
        faulty_transport(server_stream, server)?,
    ))
}

pub fn faulty_transport(stream: UnixStream, faults: Faults) -> io::Result<Transport> {
    let closer = stream.try_clone()?;
    let writer = Faulty::new(stream.try_clone()?, faults);
    let reader = writer.sharing(stream);
    Ok(Transport::new(reader, writer)
        .with_closer(move || closer.shutdown(std::net::Shutdown::Both)))
}

/// A connected client and server; the server answers with `handler`.
pub fn connected(
    config: Config,
    handler: impl RequestHandler,
) -> Result<(Connection, Connection), Error> {
    let (client, server) = pair()?;
    connected_over(client, server, config, handler)
}

pub fn connected_over(
    client: Transport,
    server: Transport,
    config: Config,
    handler: impl RequestHandler,
) -> Result<(Connection, Connection), Error> {
    let server_config = config.clone();
    let accepting = thread::Builder::new()
        .name("pom-rpc-mock-accept".to_string())
        .spawn(move || {
            Connection::builder(server)
                .config(server_config)
                .handler(handler)
                .accept(|_| {
                    let mut hello = Hello::new("test");
                    hello.server_id = Some("mock-server".to_string());
                    Ok(hello)
                })
        })?;
    let mut hello = Hello::new("test");
    hello.client_id = Some("mock-client".to_string());
    let client = Connection::builder(client).config(config).connect(hello);
    let server = accepting
        .join()
        .map_err(|_| Error::Io("the accepting thread panicked".to_string()))?;
    Ok((client?.0, server?.0))
}

#[derive(Debug, Clone, Default)]
pub struct Faults {
    /// Slept before every write.
    pub write_delay: Duration,
    /// After this many bytes written, writes and reads fail as if the cable were pulled.
    pub cut_after: Option<u64>,
    /// After this many bytes written, writes are silently swallowed, so the peer hears nothing.
    pub stall_after: Option<u64>,
    /// At most this many bytes per read, to split frames across reads.
    pub read_chunk: Option<usize>,
    /// Slept before every read.
    pub read_delay: Duration,
}

#[derive(Default)]
struct FaultState {
    written: AtomicU64,
    cut: AtomicBool,
}

/// Wraps one half of a pipe; halves made with `sharing` count bytes and cut together.
pub struct Faulty<T> {
    inner: T,
    faults: Faults,
    state: Arc<FaultState>,
}

impl<T> Faulty<T> {
    pub fn new(inner: T, faults: Faults) -> Self {
        Faulty {
            inner,
            faults,
            state: Arc::new(FaultState::default()),
        }
    }

    pub fn sharing<U>(&self, inner: U) -> Faulty<U> {
        Faulty {
            inner,
            faults: self.faults.clone(),
            state: self.state.clone(),
        }
    }

    pub fn is_cut(&self) -> bool {
        self.state.cut.load(Ordering::Acquire)
    }

    fn cut_error(&self) -> io::Error {
        io::Error::new(
            io::ErrorKind::BrokenPipe,
            "connection cut by fault injection",
        )
    }
}

impl<T: Write> Write for Faulty<T> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.is_cut() {
            return Err(self.cut_error());
        }
        if !self.faults.write_delay.is_zero() {
            thread::sleep(self.faults.write_delay);
        }
        let written = self.state.written.load(Ordering::Acquire);
        if let Some(limit) = self.faults.cut_after {
            if written >= limit {
                self.state.cut.store(true, Ordering::Release);
                return Err(self.cut_error());
            }
        }
        if let Some(limit) = self.faults.stall_after {
            if written >= limit {
                self.state
                    .written
                    .fetch_add(buffer.len() as u64, Ordering::AcqRel);
                return Ok(buffer.len());
            }
        }
        let limit = [self.faults.cut_after, self.faults.stall_after]
            .into_iter()
            .flatten()
            .min()
            .map(|limit| usize::try_from(limit - written).unwrap_or(usize::MAX))
            .unwrap_or(buffer.len())
            .min(buffer.len());
        let part = buffer.get(..limit).unwrap_or(buffer);
        let count = self.inner.write(part)?;
        self.state.written.fetch_add(count as u64, Ordering::AcqRel);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.is_cut() {
            return Err(self.cut_error());
        }
        self.inner.flush()
    }
}

impl<T: Read> Read for Faulty<T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.is_cut() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "connection cut by fault injection",
            ));
        }
        if !self.faults.read_delay.is_zero() {
            thread::sleep(self.faults.read_delay);
        }
        let limit = self
            .faults
            .read_chunk
            .unwrap_or(buffer.len())
            .clamp(1, buffer.len().max(1));
        let length = buffer.len();
        let part = buffer.get_mut(..limit.min(length)).unwrap_or(&mut []);
        self.inner.read(part)
    }
}
