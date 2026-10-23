//! A workspace's files on another machine, read through its `pom server`: the tree pushed as it changes, files
//! read in one answer or streamed. Read only for now.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use files::fs::{FileResult, FileStat, ProjectFs, TreeEvent, TreeFeed, TreePatch, READ_LIMIT};
use files::guard::Refusal;
use files::FileEntry;
use pom_rpc::{Connection, Error, PushEvent, StreamEvent, StreamReceiver};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::server_api::{
    fs_error, method, FsFilesParams, FsPathParams, FsRead, FsReadParams, FsWatchParams, TreeChunk,
    FS_TREE_TOPIC,
};
use crate::server_source::Inner;
use crate::source::READ_TIMEOUT;

/// How often the tree's reader looks up from its wait to notice it was dropped.
const FEED_TICK: Duration = Duration::from_millis(250);
/// The wait before following the tree again after the session dropped.
const RESUBSCRIBE_WAIT: Duration = Duration::from_millis(500);

pub struct RemoteFs {
    inner: Arc<Inner>,
    branch: String,
}

impl RemoteFs {
    pub(crate) fn new(inner: Arc<Inner>, branch: &str) -> RemoteFs {
        RemoteFs {
            inner,
            branch: branch.to_string(),
        }
    }

    fn connection(&self) -> io::Result<Connection> {
        match self.inner.connection() {
            Ok(Some(connection)) => Ok(connection),
            Ok(None) => Err(io::Error::other(format!(
                "{} is read over plain ssh, which has no files",
                self.inner.host_name()
            ))),
            Err(error) => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                error.to_string(),
            )),
        }
    }

    fn ask<T: DeserializeOwned>(
        &self,
        connection: &Connection,
        name: &str,
        params: impl Serialize,
    ) -> io::Result<T> {
        let value = connection
            .start_request(name, params)
            .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT))
            .map_err(|error| io_error(&self.inner, error))?;
        serde_json::from_value(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {error}")))
    }

    fn read_params(&self, path: &str, channel: u32, limit: u64, raw: bool) -> FsReadParams {
        FsReadParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            path: path.to_string(),
            channel,
            limit,
            raw,
        }
    }

    /// Reads `path` whole, inline or over a stream as the server picks.
    fn fetch(&self, path: &str, limit: u64, raw: bool) -> io::Result<(FsRead, Vec<u8>)> {
        let connection = self.connection()?;
        let stream = connection
            .open_stream()
            .map_err(|error| io_error(&self.inner, error))?;
        let params = self.read_params(path, stream.channel(), limit, raw);
        let answer: FsRead = self.ask(&connection, method::FS_READ, params)?;
        if !answer.streamed {
            return Ok((answer, Vec::new()));
        }
        let bytes = receive(&stream, Some(answer.size))?;
        Ok((answer, bytes))
    }
}

fn io_error(inner: &Inner, error: Error) -> io::Error {
    match error {
        Error::Remote(error) => {
            let kind = match error.code.as_str() {
                code if code == Refusal::NotFound.code() => io::ErrorKind::NotFound,
                code if code == Refusal::OutsideWorkspace.code() => io::ErrorKind::PermissionDenied,
                code if code == Refusal::NotAFile.code() => io::ErrorKind::InvalidInput,
                code if code == Refusal::Invalid.code() => io::ErrorKind::InvalidInput,
                fs_error::TOO_LARGE => io::ErrorKind::InvalidData,
                _ => io::ErrorKind::Other,
            };
            io::Error::new(kind, error.message)
        }
        Error::Timeout => io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "no answer from {} after {}s",
                inner.host_name(),
                READ_TIMEOUT.as_secs()
            ),
        ),
        other => io::Error::new(
            io::ErrorKind::NotConnected,
            format!("{} stopped answering: {other}", inner.host_name()),
        ),
    }
}

/// Everything sent on `stream` until it ends: in order, nothing missing or twice, `size` bytes when known.
fn receive(stream: &StreamReceiver, size: Option<u64>) -> io::Result<Vec<u8>> {
    let capacity = size.map_or(0, |size| usize::try_from(size).unwrap_or(0));
    let mut bytes = Vec::with_capacity(capacity);
    loop {
        match stream.recv_timeout(READ_TIMEOUT) {
            Ok(StreamEvent::Data {
                offset,
                bytes: data,
            }) => {
                if offset != bytes.len() as u64 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("the read skipped from {} to {offset}", bytes.len()),
                    ));
                }
                bytes.extend_from_slice(&data);
                stream.ack(bytes.len() as u64);
            }
            Ok(StreamEvent::End { reason }) if reason == "done" => break,
            Ok(StreamEvent::End { reason }) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("the read stopped: {reason}"),
                ))
            }
            Ok(StreamEvent::Reset { .. }) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the read skipped ahead",
                ))
            }
            Err(Error::Timeout) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the read stalled"))
            }
            Err(error) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    error.to_string(),
                ))
            }
        }
    }
    if size.is_some_and(|size| size != bytes.len() as u64) {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("the read ended after {} bytes", bytes.len()),
        ));
    }
    Ok(bytes)
}

impl ProjectFs for RemoteFs {
    fn local_root(&self) -> Option<&Path> {
        None
    }

    fn watch(&self, wake: Arc<dyn Fn() + Send + Sync>) -> Box<dyn TreeFeed> {
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let follower = TreeFollower {
            inner: self.inner.clone(),
            branch: self.branch.clone(),
            sender,
            stop: stop.clone(),
            wake,
        };
        let spawned = std::thread::Builder::new()
            .name("remote-file-tree".into())
            .spawn(move || follower.run());
        if let Err(error) = spawned {
            eprintln!("following the remote file tree: {error}");
        }
        Box::new(RemoteFeed { receiver, stop })
    }

    fn stat(&self, path: &str) -> io::Result<FileStat> {
        let connection = self.connection()?;
        let params = FsPathParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            path: path.to_string(),
        };
        self.ask(&connection, method::FS_STAT, params)
    }

    fn read(&self, path: &str) -> io::Result<FileResult> {
        let (answer, bytes) = self.fetch(path, READ_LIMIT, false)?;
        if !answer.streamed {
            return Ok(FileResult {
                text: answer.text,
                binary: answer.binary,
                mtime_ns: answer.mtime_ns,
                size: answer.size,
            });
        }
        let mut read = FileResult::from_bytes(bytes, answer.mtime_ns);
        if answer.binary {
            read.text = None;
            read.binary = true;
        }
        Ok(read)
    }

    fn read_bytes(&self, path: &str, limit: u64) -> io::Result<Vec<u8>> {
        let (answer, bytes) = self.fetch(path, limit, true)?;
        if !answer.streamed {
            return Ok(answer.text.unwrap_or_default().into_bytes());
        }
        Ok(bytes)
    }

    fn project_files(&self, include_ignored: bool) -> io::Result<Vec<String>> {
        let connection = self.connection()?;
        let stream = connection
            .open_stream()
            .map_err(|error| io_error(&self.inner, error))?;
        let params = FsFilesParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            include_ignored,
            channel: stream.channel(),
        };
        self.ask::<serde_json::Value>(&connection, method::FS_FILES, params)?;
        let bytes = receive(&stream, None)?;
        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }
}

/// The tree as a thread follows it; dropping it stops the thread.
struct RemoteFeed {
    receiver: mpsc::Receiver<TreeEvent>,
    stop: Arc<AtomicBool>,
}

impl Drop for RemoteFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl TreeFeed for RemoteFeed {
    fn poll(&mut self) -> Vec<TreeEvent> {
        let mut events: Vec<TreeEvent> = Vec::new();
        for event in self.receiver.try_iter() {
            // A snapshot in progress is superseded by the next one, so only the newest is built into a tree.
            if matches!(event, TreeEvent::Snapshot { .. })
                && matches!(events.last(), Some(TreeEvent::Snapshot { .. }))
            {
                events.pop();
            }
            events.push(event);
        }
        events
    }
}

struct TreeFollower {
    inner: Arc<Inner>,
    branch: String,
    sender: mpsc::Sender<TreeEvent>,
    stop: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

/// A snapshot as its chunks arrive, until the last.
#[derive(Default)]
struct Assembly {
    snapshot: Option<Vec<FileEntry>>,
}

impl Assembly {
    /// The event a chunk makes: the snapshot so far, or a patch once the snapshot is complete.
    fn take(&mut self, chunk: TreeChunk) -> TreeEvent {
        if chunk.reset {
            self.snapshot = Some(Vec::new());
        }
        let Some(entries) = self.snapshot.as_mut() else {
            return TreeEvent::Patch(TreePatch {
                upserted: chunk.entries,
                removed: chunk.removed,
                changed: chunk.changed,
            });
        };
        entries.extend(chunk.entries);
        if chunk.last {
            let mut entries = self.snapshot.take().unwrap_or_default();
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            return TreeEvent::Snapshot {
                entries,
                complete: true,
            };
        }
        let mut sofar = entries.clone();
        sofar.sort_by(|a, b| a.path.cmp(&b.path));
        TreeEvent::Snapshot {
            entries: sofar,
            complete: false,
        }
    }
}

impl TreeFollower {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn send(&self, event: TreeEvent) -> bool {
        if self.sender.send(event).is_err() {
            return false;
        }
        (self.wake)();
        true
    }

    fn run(self) {
        let mut first = true;
        while !self.stopped() {
            let connection = if first {
                first = false;
                self.inner.connection().ok().flatten()
            } else {
                self.inner.live_connection()
            };
            let reason = match connection {
                Some(connection) => self.follow(&connection),
                None => format!("reconnecting to {}", self.inner.host_name()),
            };
            if self.stopped() || !self.send(TreeEvent::Interrupted(reason)) {
                return;
            }
            std::thread::sleep(RESUBSCRIBE_WAIT);
        }
    }

    /// Follows the tree on `connection` until it ends; returns why.
    fn follow(&self, connection: &Connection) -> String {
        let subscription = match connection.subscribe() {
            Ok(subscription) => subscription,
            Err(error) => return error.to_string(),
        };
        let params = FsWatchParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            channel: subscription.channel(),
        };
        let asked = connection
            .start_request(method::FS_WATCH, params)
            .and_then(|mut pending| pending.wait_timeout(READ_TIMEOUT));
        if let Err(error) = asked {
            return io_error(&self.inner, error).to_string();
        }
        let mut assembly = Assembly::default();
        loop {
            if self.stopped() {
                return "closed".to_string();
            }
            match subscription.recv_timeout(FEED_TICK) {
                Ok(PushEvent::Value { topic, value }) if topic == FS_TREE_TOPIC => {
                    let chunk: TreeChunk = match serde_json::from_value(value) {
                        Ok(chunk) => chunk,
                        Err(error) => return format!("an unreadable tree: {error}"),
                    };
                    if !self.send(assembly.take(chunk)) {
                        return "closed".to_string();
                    }
                }
                Ok(PushEvent::Value { .. }) | Err(Error::Timeout) => {}
                Ok(PushEvent::End { reason }) => return reason,
                Err(error) => return error.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            ..FileEntry::default()
        }
    }

    #[test]
    fn chunks_build_a_snapshot_then_patches() {
        let mut assembly = Assembly::default();
        let first = assembly.take(TreeChunk {
            reset: true,
            entries: vec![entry("b")],
            ..TreeChunk::default()
        });
        assert_eq!(
            first,
            TreeEvent::Snapshot {
                entries: vec![entry("b")],
                complete: false
            }
        );
        let done = assembly.take(TreeChunk {
            last: true,
            entries: vec![entry("a")],
            ..TreeChunk::default()
        });
        assert_eq!(
            done,
            TreeEvent::Snapshot {
                entries: vec![entry("a"), entry("b")],
                complete: true
            }
        );
        let patch = assembly.take(TreeChunk {
            last: true,
            removed: vec!["a".into()],
            ..TreeChunk::default()
        });
        assert!(matches!(patch, TreeEvent::Patch(patch) if patch.removed == ["a"]));
        let again = assembly.take(TreeChunk {
            reset: true,
            last: true,
            ..TreeChunk::default()
        });
        assert_eq!(
            again,
            TreeEvent::Snapshot {
                entries: Vec::new(),
                complete: true
            }
        );
    }
}
