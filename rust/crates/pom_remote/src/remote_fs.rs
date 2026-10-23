//! A workspace's files on another machine, through its `pom server`: the tree pushed as it changes, files read
//! in one answer or streamed, saves and tree changes sent with an op id and sent again with it after a
//! reconnect, so each happens once.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use files::fs::{
    FileResult, FileStat, ProjectFs, TreeEvent, TreeFeed, TreePatch, WriteError, INLINE_READ_LIMIT,
    READ_LIMIT,
};
use files::guard::Refusal;
use files::FileEntry;
use pom_rpc::{Connection, Error, PushEvent, StreamEvent, StreamReceiver};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::server_api::{
    fs_error, method, FsChangeParams, FsFilesParams, FsPathParams, FsRead, FsReadParams,
    FsWatchParams, FsWriteParams, FsWritten, TreeChunk, FS_TREE_TOPIC, WRITE_READY,
};
use crate::server_source::Inner;
use crate::source::READ_TIMEOUT;

/// How often the tree's reader looks up from its wait to notice it was dropped.
const FEED_TICK: Duration = Duration::from_millis(250);
/// The wait before following the tree again after the session dropped.
const RESUBSCRIBE_WAIT: Duration = Duration::from_millis(500);
/// How often a snapshot still arriving is shown as it stands.
const PARTIAL_EVERY: Duration = Duration::from_millis(500);
/// How long a save waits for the machine to come back before it gives up; the text stays in the editor.
const SAVE_PATIENCE: Duration = Duration::from_secs(30 * 60);
/// How long a change to the tree waits for the machine: the tree shows it meanwhile, so not long.
const CHANGE_PATIENCE: Duration = Duration::from_secs(2 * 60);
const RETRY_WAIT: Duration = Duration::from_millis(500);
/// How long an answer to a save or change may take once everything is sent.
const WRITE_ANSWER: Duration = Duration::from_secs(60);
/// How far a streamed save runs ahead of what the server says has arrived.
const UPLOAD_AHEAD: u64 = 1024 * 1024;
const UPLOAD_PIECE: usize = 32 * 1024;

/// Why one try at a save or change did not finish: worth trying again once the machine is back, or not.
enum Attempt {
    Again(io::Error),
    Final(io::Error),
}

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

impl RemoteFs {
    /// Runs `once` until it finishes or fails for a reason other than the connection, for at most `patience`.
    fn with_retries<T>(
        &self,
        patience: Duration,
        mut once: impl FnMut(&Connection) -> Result<T, Attempt>,
    ) -> io::Result<T> {
        let deadline = Instant::now() + patience;
        loop {
            let tried = match self.inner.connection() {
                Ok(Some(connection)) => once(&connection),
                Ok(None) => Err(Attempt::Final(io::Error::other(format!(
                    "{} is read over plain ssh, which has no files",
                    self.inner.host_name()
                )))),
                Err(error) => Err(Attempt::Again(io::Error::new(
                    io::ErrorKind::NotConnected,
                    error.to_string(),
                ))),
            };
            match tried {
                Ok(value) => return Ok(value),
                Err(Attempt::Final(error)) => return Err(error),
                Err(Attempt::Again(error)) if Instant::now() >= deadline => return Err(error),
                Err(Attempt::Again(_)) => std::thread::sleep(RETRY_WAIT),
            }
        }
    }

    fn attempt_error(&self, error: Error) -> Attempt {
        match error {
            Error::Remote(remote) if remote.code == fs_error::INCOMPLETE => {
                Attempt::Again(io::Error::other(remote.message))
            }
            remote @ Error::Remote(_) => Attempt::Final(io_error(&self.inner, remote)),
            other => Attempt::Again(io_error(&self.inner, other)),
        }
    }

    fn write_params(&self, path: &str, expect: Option<&FileStat>, op_id: &str) -> FsWriteParams {
        FsWriteParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            path: path.to_string(),
            expect: expect.cloned(),
            op_id: op_id.to_string(),
            ..FsWriteParams::default()
        }
    }

    fn write_once(
        &self,
        connection: &Connection,
        path: &str,
        text: &str,
        expect: Option<&FileStat>,
        op_id: &str,
    ) -> Result<FsWritten, Attempt> {
        let mut params = self.write_params(path, expect, op_id);
        if text.len() as u64 <= INLINE_READ_LIMIT {
            params.text = Some(text.to_string());
            let answer = connection
                .start_request(method::FS_WRITE, &params)
                .and_then(|mut pending| pending.wait_timeout(WRITE_ANSWER))
                .map_err(|error| self.attempt_error(error))?;
            return serde_json::from_value(answer).map_err(|error| {
                Attempt::Final(io::Error::new(io::ErrorKind::InvalidData, error))
            });
        }
        let stream = connection
            .open_stream()
            .map_err(|error| self.attempt_error(error))?;
        params.channel = stream.channel();
        params.size = text.len() as u64;
        let mut pending = connection
            .start_request(method::FS_WRITE, &params)
            .map_err(|error| self.attempt_error(error))?;
        let mut upload = Upload::new(&stream);
        loop {
            match pending.wait_timeout(Duration::ZERO) {
                Ok(answer) => {
                    return serde_json::from_value(answer).map_err(|error| {
                        Attempt::Final(io::Error::new(io::ErrorKind::InvalidData, error))
                    })
                }
                Err(Error::Timeout) => {}
                Err(error) => return Err(self.attempt_error(error)),
            }
            if upload.done(text.len()) {
                let answer = pending
                    .wait_timeout(WRITE_ANSWER)
                    .map_err(|error| self.attempt_error(error))?;
                return serde_json::from_value(answer).map_err(|error| {
                    Attempt::Final(io::Error::new(io::ErrorKind::InvalidData, error))
                });
            }
            upload.step(text.as_bytes()).map_err(|error| {
                Attempt::Again(io::Error::new(io::ErrorKind::NotConnected, error))
            })?;
        }
    }

    fn change(&self, name: &str, path: &str, from: &str, is_dir: bool) -> io::Result<()> {
        let params = FsChangeParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            path: path.to_string(),
            from: from.to_string(),
            is_dir,
            op_id: self.inner.next_op_id(),
        };
        self.with_retries(CHANGE_PATIENCE, |connection| {
            connection
                .start_request(name, &params)
                .and_then(|mut pending| pending.wait_timeout(WRITE_ANSWER))
                .map(|_| ())
                .map_err(|error| self.attempt_error(error))
        })
    }
}

/// A save's text going out as INPUT, never more than [`UPLOAD_AHEAD`] past what the server says it has.
struct Upload<'a> {
    stream: &'a StreamReceiver,
    ready: bool,
    sent: usize,
    confirmed: u64,
    /// What the server sent back so far: the ready word, then 8-byte counts.
    heard: Vec<u8>,
}

impl<'a> Upload<'a> {
    fn new(stream: &'a StreamReceiver) -> Upload<'a> {
        Upload {
            stream,
            ready: false,
            sent: 0,
            confirmed: 0,
            heard: Vec::new(),
        }
    }

    fn done(&self, size: usize) -> bool {
        self.ready && self.confirmed >= size as u64
    }

    /// Takes what the server said, then sends what the window allows; waits a little when it allows nothing.
    fn step(&mut self, text: &[u8]) -> Result<(), String> {
        let ahead = self.sent as u64 - self.confirmed.min(self.sent as u64);
        let can_send = self.ready && self.sent < text.len() && ahead < UPLOAD_AHEAD;
        let wait = if can_send {
            Duration::ZERO
        } else {
            FEED_TICK / 5
        };
        match self.stream.recv_timeout(wait) {
            Ok(StreamEvent::Data { bytes, .. }) => {
                self.heard.extend_from_slice(&bytes);
                self.stream.ack(self.heard.len() as u64);
                self.read_heard()?;
            }
            Ok(StreamEvent::End { reason }) => return Err(format!("the save stopped: {reason}")),
            Ok(StreamEvent::Reset { .. }) => return Err("the save skipped ahead".into()),
            Err(Error::Timeout) => {}
            Err(error) => return Err(error.to_string()),
        }
        if !self.ready {
            return Ok(());
        }
        while self.sent < text.len()
            && (self.sent as u64).saturating_sub(self.confirmed) < UPLOAD_AHEAD
        {
            let end = (self.sent + UPLOAD_PIECE).min(text.len());
            let piece = text.get(self.sent..end).unwrap_or_default();
            self.stream
                .send_input(piece)
                .map_err(|error| error.to_string())?;
            self.sent = end;
        }
        Ok(())
    }

    fn read_heard(&mut self) -> Result<(), String> {
        if !self.ready {
            if self.heard.len() < WRITE_READY.len() {
                return Ok(());
            }
            if !self.heard.starts_with(WRITE_READY) {
                return Err("the server did not take the save".into());
            }
            self.ready = true;
        }
        let counts = self.heard.get(WRITE_READY.len()..).unwrap_or_default();
        if let Some(last) = counts.chunks_exact(8).last() {
            let mut count = [0u8; 8];
            count.copy_from_slice(last);
            self.confirmed = u64::from_be_bytes(count);
        }
        Ok(())
    }
}

fn io_error(inner: &Inner, error: Error) -> io::Error {
    match error {
        Error::Remote(error) if error.code == fs_error::TOO_LARGE => io::Error::new(
            io::ErrorKind::InvalidData,
            files::fs::TooLarge(error.message),
        ),
        Error::Remote(error) => {
            let kind = match error.code.as_str() {
                code if code == Refusal::NotFound.code() => io::ErrorKind::NotFound,
                code if code == Refusal::OutsideWorkspace.code() => io::ErrorKind::PermissionDenied,
                code if code == Refusal::NotAFile.code() => io::ErrorKind::InvalidInput,
                code if code == Refusal::Invalid.code() => io::ErrorKind::InvalidInput,
                code if code == Refusal::ReadOnly.code() => io::ErrorKind::PermissionDenied,
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

    /// Waits for the machine while it is away (the editor shows the save as waiting), then sends the save again
    /// with the same op id and the same `expect`: a file changed there meanwhile comes back as a conflict.
    fn write(
        &self,
        path: &str,
        text: &str,
        expect: Option<&FileStat>,
    ) -> Result<FileStat, WriteError> {
        let op_id = self.inner.next_op_id();
        let written = self.with_retries(SAVE_PATIENCE, |connection| {
            self.write_once(connection, path, text, expect, &op_id)
        })?;
        let stat = FileStat {
            dir: false,
            size: written.size,
            mtime_ns: written.mtime_ns,
        };
        if written.conflict {
            return Err(WriteError::Conflict(stat));
        }
        Ok(stat)
    }

    fn create_file(&self, path: &str) -> io::Result<()> {
        self.change(method::FS_CREATE_FILE, path, "", false)
    }

    fn create_dir(&self, path: &str) -> io::Result<()> {
        self.change(method::FS_CREATE_DIR, path, "", false)
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.change(method::FS_RENAME, to, from, false)
    }

    fn copy(&self, from: &str, to: &str) -> io::Result<()> {
        self.change(method::FS_COPY, to, from, false)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        self.change(method::FS_REMOVE, path, "", false)
    }

    fn trash(&self, path: &str) -> io::Result<()> {
        self.change(method::FS_TRASH, path, "", false)
    }

    fn add_to_gitignore(&self, path: &str, is_dir: bool) -> io::Result<()> {
        self.change(method::FS_APPEND_GITIGNORE, path, "", is_dir)
    }

    fn expand(&self, path: &str) -> io::Result<()> {
        let connection = self.connection()?;
        let params = FsPathParams {
            cfg: self.inner.config().to_string(),
            branch: self.branch.clone(),
            path: path.to_string(),
        };
        self.ask::<serde_json::Value>(&connection, method::FS_EXPAND, params)
            .map(|_| ())
    }

    fn online(&self) -> bool {
        self.inner.live_connection().is_some()
    }

    fn host(&self) -> Option<&str> {
        Some(self.inner.host_name())
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
    shown: Option<Instant>,
}

impl Assembly {
    /// The event a chunk makes: the snapshot so far now and then, the whole one at its last chunk, or a patch
    /// once the snapshot is complete.
    fn take(&mut self, chunk: TreeChunk) -> Option<TreeEvent> {
        if chunk.reset {
            self.snapshot = Some(Vec::new());
            self.shown = None;
        }
        let Some(entries) = self.snapshot.as_mut() else {
            return Some(TreeEvent::Patch(TreePatch {
                upserted: chunk.entries,
                removed: chunk.removed,
                changed: chunk.changed,
            }));
        };
        entries.extend(chunk.entries);
        if chunk.last {
            let mut entries = self.snapshot.take().unwrap_or_default();
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            return Some(TreeEvent::Snapshot {
                entries,
                complete: true,
            });
        }
        // Copying the snapshot so far for every chunk grows with the square of a large tree.
        if self.shown.is_some_and(|at| at.elapsed() < PARTIAL_EVERY) {
            return None;
        }
        self.shown = Some(Instant::now());
        let mut sofar = entries.clone();
        sofar.sort_by(|a, b| a.path.cmp(&b.path));
        Some(TreeEvent::Snapshot {
            entries: sofar,
            complete: false,
        })
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
            lazy_ignored: true,
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
                    let Some(event) = assembly.take(chunk) else {
                        continue;
                    };
                    if !self.send(event) {
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
            Some(TreeEvent::Snapshot {
                entries: vec![entry("b")],
                complete: false
            })
        );
        let soon = assembly.take(TreeChunk {
            entries: vec![entry("c")],
            ..TreeChunk::default()
        });
        assert_eq!(soon, None, "the snapshot so far is shown now and then");
        let done = assembly.take(TreeChunk {
            last: true,
            entries: vec![entry("a")],
            ..TreeChunk::default()
        });
        assert_eq!(
            done,
            Some(TreeEvent::Snapshot {
                entries: vec![entry("a"), entry("b"), entry("c")],
                complete: true
            })
        );
        let patch = assembly.take(TreeChunk {
            last: true,
            removed: vec!["a".into()],
            ..TreeChunk::default()
        });
        assert!(matches!(patch, Some(TreeEvent::Patch(patch)) if patch.removed == ["a"]));
        let again = assembly.take(TreeChunk {
            reset: true,
            last: true,
            ..TreeChunk::default()
        });
        assert_eq!(
            again,
            Some(TreeEvent::Snapshot {
                entries: Vec::new(),
                complete: true
            })
        );
    }
}
