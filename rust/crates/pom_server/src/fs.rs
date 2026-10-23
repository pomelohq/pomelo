//! A workspace's files for a remote app, read only: one watcher per folder shared by every client that shows it,
//! its tree pushed as a chunked snapshot and then as patches, files read inline or streamed. Every path passes
//! the guard first.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use files::fs::{Links, INLINE_READ_LIMIT, READ_LIMIT};
use files::guard::{self, GuardError};
use files::{FileEntry, FileStat, TreePatch};
use pom_remote::server_api::{
    fs_error, FsFilesParams, FsRead, FsReadParams, TreeChunk, FS_TREE_TOPIC,
};
use pom_rpc::{error_code, Connection, Publisher, RemoteError, StreamSender};
use serde_json::Value;

/// A folder nobody watches any more keeps its watcher this long, for the client that comes right back.
pub const WATCH_LINGER: Duration = Duration::from_secs(60);
pub const CHUNK_ENTRIES: usize = 4000;
/// Below the frame limit with room for the JSON around each entry.
pub const CHUNK_BYTES: usize = 768 * 1024;
pub const MAX_FS_STREAMS: usize = 16;
const STREAM_PIECE: usize = 32 * 1024;
const TICK: Duration = Duration::from_secs(1);

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn refused(error: GuardError) -> RemoteError {
    RemoteError::new(error.refusal.code(), error.message)
}

fn failed(message: impl Into<String>) -> RemoteError {
    RemoteError::new(crate::FAILED, message)
}

enum Command {
    Add(Publisher),
    Changed,
}

type Roots = Arc<Mutex<HashMap<PathBuf, Sender<Command>>>>;

pub struct FsService {
    roots: Roots,
    linger: Arc<Mutex<Duration>>,
    streams: Arc<AtomicUsize>,
}

impl Default for FsService {
    fn default() -> Self {
        FsService::new()
    }
}

/// Counts a stream until it ends, so a client cannot hold more than [`MAX_FS_STREAMS`].
struct StreamSlot(Arc<AtomicUsize>);

impl Drop for StreamSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl FsService {
    pub fn new() -> FsService {
        FsService {
            roots: Arc::new(Mutex::new(HashMap::new())),
            linger: Arc::new(Mutex::new(WATCH_LINGER)),
            streams: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn set_linger(&self, linger: Duration) {
        *locked(&self.linger) = linger;
    }

    /// Folders with a watcher, in use or lingering.
    pub fn watched(&self) -> usize {
        locked(&self.roots).len()
    }

    /// Pushes `root`'s tree on `publisher`: a snapshot, then patches as files change.
    pub fn watch(&self, root: &Path, publisher: Publisher) -> Result<(), RemoteError> {
        let root = root
            .canonicalize()
            .map_err(|error| failed(format!("the workspace folder: {error}")))?;
        let mut roots = locked(&self.roots);
        let mut publisher = Some(publisher);
        if let Some(commands) = roots.get(&root) {
            if let Some(given) = publisher.take() {
                match commands.send(Command::Add(given)) {
                    Ok(()) => return Ok(()),
                    Err(mpsc::SendError(Command::Add(back))) => publisher = Some(back),
                    Err(_) => {}
                }
            }
        }
        let Some(publisher) = publisher else {
            return Err(failed("the watch went away"));
        };
        let (sender, commands) = mpsc::channel();
        let wake = {
            let sender = sender.clone();
            Arc::new(move || {
                if sender.send(Command::Changed).is_err() {
                    eprintln!("pom server: a file watch stopped");
                }
            })
        };
        let watcher = files::TreeWatcher::new(&root, wake)
            .map_err(|error| failed(format!("watching the workspace: {error}")))?;
        let folder = RootWatch {
            root: root.clone(),
            watcher,
            commands,
            roots: self.roots.clone(),
            linger: self.linger.clone(),
            subscribers: Vec::new(),
            entries: BTreeMap::new(),
            scan_id: 0,
            complete: false,
        };
        std::thread::Builder::new()
            .name("pom-server-files".into())
            .spawn(move || folder.run(publisher))
            .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))?;
        roots.insert(root, sender);
        Ok(())
    }

    pub fn stat(&self, root: &Path, path: &str) -> Result<FileStat, RemoteError> {
        let full = guard::resolve(root, path).map_err(refused)?;
        let metadata = std::fs::metadata(&full).map_err(|error| failed(error.to_string()))?;
        Ok(FileStat {
            dir: metadata.is_dir(),
            size: if metadata.is_dir() { 0 } else { metadata.len() },
            mtime_ns: metadata
                .modified()
                .map(files::fs::mtime_ns)
                .unwrap_or_default(),
        })
    }

    fn take_stream(&self) -> Result<StreamSlot, RemoteError> {
        let taken = self.streams.fetch_add(1, Ordering::AcqRel);
        let slot = StreamSlot(self.streams.clone());
        if taken >= MAX_FS_STREAMS {
            return Err(RemoteError::new(
                fs_error::BUSY,
                format!("{MAX_FS_STREAMS} file streams already run"),
            ));
        }
        Ok(slot)
    }

    /// A small text file comes in the answer; anything else, or anything `raw`, over the client's stream.
    pub fn read(
        &self,
        connection: &Connection,
        root: &Path,
        params: &FsReadParams,
    ) -> Result<Value, RemoteError> {
        let full = guard::resolve_file(root, &params.path).map_err(refused)?;
        let mut file = std::fs::File::open(&full).map_err(|error| failed(error.to_string()))?;
        let metadata = file.metadata().map_err(|error| failed(error.to_string()))?;
        let limit = match params.limit {
            0 => READ_LIMIT,
            asked => asked.min(READ_LIMIT),
        };
        let size = metadata.len();
        if size > limit {
            return Err(RemoteError::new(
                fs_error::TOO_LARGE,
                files::fs::too_large(size, limit).to_string(),
            ));
        }
        let mtime_ns = metadata
            .modified()
            .map(files::fs::mtime_ns)
            .unwrap_or_default();
        let mut sender = attach(connection, params.channel)?;
        if !params.raw && size <= INLINE_READ_LIMIT {
            let mut bytes = Vec::new();
            file.by_ref()
                .take(INLINE_READ_LIMIT)
                .read_to_end(&mut bytes)
                .map_err(|error| failed(error.to_string()))?;
            if let Some(sender) = sender.take() {
                sender.finish("inline");
            }
            let read = files::FileResult::from_bytes(bytes, mtime_ns);
            return to_value(FsRead {
                size: read.size,
                text: read.text,
                binary: read.binary,
                mtime_ns,
                streamed: false,
            });
        }
        let Some(sender) = sender else {
            return Err(RemoteError::new(
                error_code::INVALID_PARAMS,
                "a file this large needs a stream channel",
            ));
        };
        let slot = self.take_stream()?;
        let mut head = vec![0; 8192.min(size as usize)];
        file.read_exact(&mut head)
            .map_err(|error| failed(error.to_string()))?;
        let binary = files::fs::is_binary(&head);
        std::thread::Builder::new()
            .name("pom-server-read".into())
            .spawn(move || {
                stream_file(sender, head, file, size);
                drop(slot);
            })
            .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))?;
        to_value(FsRead {
            text: None,
            binary,
            mtime_ns,
            size,
            streamed: true,
        })
    }

    /// The finder's list over the client's stream, one path per line.
    pub fn files(
        &self,
        connection: &Connection,
        root: &Path,
        params: &FsFilesParams,
    ) -> Result<Value, RemoteError> {
        let root = guard::resolve(root, "").map_err(refused)?;
        let Some(sender) = attach(connection, params.channel)? else {
            return Err(RemoteError::new(
                error_code::INVALID_PARAMS,
                "the list needs a stream channel",
            ));
        };
        let slot = self.take_stream()?;
        let include_ignored = params.include_ignored;
        std::thread::Builder::new()
            .name("pom-server-file-list".into())
            .spawn(move || {
                let listed = files::project_files(&root, include_ignored);
                let mut text = listed.join("\n");
                if !text.is_empty() {
                    text.push('\n');
                }
                stream_bytes(sender, text.as_bytes());
                drop(slot);
            })
            .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))?;
        Ok(Value::Null)
    }
}

fn to_value(value: impl serde::Serialize) -> Result<Value, RemoteError> {
    serde_json::to_value(value)
        .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))
}

fn attach(connection: &Connection, channel: u32) -> Result<Option<StreamSender>, RemoteError> {
    if channel == 0 {
        return Ok(None);
    }
    connection
        .attach_stream(channel, 0)
        .map(Some)
        .map_err(|error| {
            RemoteError::new(
                error_code::INVALID_PARAMS,
                format!("channel {channel}: {error}"),
            )
        })
}

fn stream_bytes(mut sender: StreamSender, bytes: &[u8]) {
    for piece in bytes.chunks(STREAM_PIECE) {
        if sender.write(piece).is_err() {
            return;
        }
    }
    sender.finish("done");
}

/// Sends `head` and then the rest of `file`, never past the `size` the answer gave.
fn stream_file(mut sender: StreamSender, head: Vec<u8>, mut file: std::fs::File, size: u64) {
    if sender.write(&head).is_err() {
        return;
    }
    let mut left = size.saturating_sub(head.len() as u64);
    let mut piece = vec![0; STREAM_PIECE];
    while left > 0 {
        let want = piece.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let Some(buffer) = piece.get_mut(..want) else {
            return;
        };
        match file.read(buffer) {
            Ok(0) => break,
            Ok(read) => {
                let Some(bytes) = buffer.get(..read) else {
                    return;
                };
                if sender.write(bytes).is_err() {
                    return;
                }
                left -= read as u64;
            }
            Err(_) => return,
        }
    }
    sender.finish("done");
}

/// One watched folder, owned by its thread: everything it pushes goes out in order from here.
struct RootWatch {
    root: PathBuf,
    watcher: files::TreeWatcher,
    commands: Receiver<Command>,
    roots: Roots,
    linger: Arc<Mutex<Duration>>,
    subscribers: Vec<Publisher>,
    entries: BTreeMap<String, FileEntry>,
    scan_id: u64,
    complete: bool,
}

/// Splits entries into pushes below [`CHUNK_ENTRIES`] and [`CHUNK_BYTES`].
fn chunks(entries: Vec<FileEntry>) -> Vec<Vec<FileEntry>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    let mut bytes = 0;
    for entry in entries {
        let size = entry.path.len() * 2 + 48;
        if !current.is_empty() && (current.len() >= CHUNK_ENTRIES || bytes + size > CHUNK_BYTES) {
            out.push(std::mem::take(&mut current));
            bytes = 0;
        }
        bytes += size;
        current.push(entry);
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
}

impl RootWatch {
    fn run(mut self, first: Publisher) {
        self.add(first);
        let root = self.root.clone();
        let mut deferred = false;
        files::fs::walk_in_batches(&root, &root, Links::Stay, CHUNK_ENTRIES, &mut |batch| {
            while let Ok(command) = self.commands.try_recv() {
                match command {
                    Command::Add(publisher) => self.add(publisher),
                    Command::Changed => deferred = true,
                }
            }
            for entry in &batch {
                self.entries.insert(entry.path.clone(), entry.clone());
            }
            self.broadcast(TreeChunk {
                scan_id: self.scan_id,
                entries: batch,
                ..TreeChunk::default()
            });
            true
        });
        self.complete = true;
        self.broadcast(TreeChunk {
            scan_id: self.scan_id,
            last: true,
            ..TreeChunk::default()
        });
        if deferred {
            self.changed();
        }
        let mut idle_since: Option<Instant> = None;
        loop {
            match self.commands.recv_timeout(TICK) {
                Ok(Command::Add(publisher)) => self.add(publisher),
                Ok(Command::Changed) => self.changed(),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.subscribers.retain(|publisher| !publisher.is_ended());
            if !self.subscribers.is_empty() {
                idle_since = None;
                continue;
            }
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= *locked(&self.linger) && self.leave_if_idle() {
                return;
            }
        }
    }

    /// Leaves the shared map, unless a client asked for this folder meanwhile.
    fn leave_if_idle(&mut self) -> bool {
        let mut roots = locked(&self.roots);
        match self.commands.try_recv() {
            Ok(Command::Add(publisher)) => {
                drop(roots);
                self.add(publisher);
                false
            }
            Ok(Command::Changed) => {
                drop(roots);
                self.changed();
                false
            }
            Err(_) => {
                roots.remove(&self.root);
                true
            }
        }
    }

    /// Sends a new subscriber everything known so far, as a fresh snapshot.
    fn add(&mut self, publisher: Publisher) {
        let entries: Vec<FileEntry> = self.entries.values().cloned().collect();
        let pieces = chunks(entries);
        let count = pieces.len();
        for (index, piece) in pieces.into_iter().enumerate() {
            let chunk = TreeChunk {
                scan_id: self.scan_id,
                reset: index == 0,
                last: self.complete && index + 1 == count,
                entries: piece,
                ..TreeChunk::default()
            };
            if publisher.publish(FS_TREE_TOPIC, &chunk).is_err() {
                return;
            }
        }
        self.subscribers.push(publisher);
    }

    fn broadcast(&mut self, chunk: TreeChunk) {
        self.subscribers
            .retain(|publisher| publisher.publish(FS_TREE_TOPIC, &chunk).is_ok());
    }

    fn changed(&mut self) {
        let changes = self.watcher.take_changes();
        if changes.is_empty() {
            return;
        }
        let TreePatch {
            upserted,
            removed,
            changed,
        } = files::fs::patch_for(&self.root, &changes, Links::Stay);
        for path in &removed {
            let under = format!("{path}/");
            self.entries.remove(path);
            let inside: Vec<String> = self
                .entries
                .range(under.clone()..)
                .take_while(|(key, _)| key.starts_with(&under))
                .map(|(key, _)| key.clone())
                .collect();
            for key in inside {
                self.entries.remove(&key);
            }
        }
        for entry in &upserted {
            self.entries.insert(entry.path.clone(), entry.clone());
        }
        self.scan_id += 1;
        let pieces = chunks(upserted);
        let count = pieces.len();
        let mut removed = Some(removed);
        let mut changed = Some(changed);
        for (index, piece) in pieces.into_iter().enumerate() {
            self.broadcast(TreeChunk {
                scan_id: self.scan_id,
                reset: false,
                last: index + 1 == count,
                entries: piece,
                removed: removed.take().unwrap_or_default(),
                changed: changed.take().unwrap_or_default(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn entry(path: String) -> FileEntry {
        FileEntry {
            path,
            ..FileEntry::default()
        }
    }

    #[test]
    fn chunks_stay_under_both_limits() {
        let many: Vec<FileEntry> = (0..9001).map(|index| entry(format!("f{index}"))).collect();
        let pieces = chunks(many);
        assert_eq!(
            pieces.iter().map(Vec::len).collect::<Vec<_>>(),
            [4000, 4000, 1001]
        );
        let long: Vec<FileEntry> = (0..400)
            .map(|index| entry(format!("{index}{}", "x".repeat(4000))))
            .collect();
        for piece in chunks(long) {
            let json = serde_json::to_vec(&piece).unwrap_or_default();
            assert!(json.len() < pom_rpc::MAX_FRAME, "{}", json.len());
        }
        assert_eq!(
            chunks(Vec::new()).len(),
            1,
            "an empty tree still sends a chunk"
        );
    }

    fn last_chunk(subscription: &pom_rpc::Subscription) -> TreeChunk {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "no complete snapshot");
            if let Ok(pom_rpc::PushEvent::Value { value, .. }) =
                subscription.recv_timeout(Duration::from_millis(100))
            {
                let chunk: TreeChunk = serde_json::from_value(value).unwrap_or_default();
                if chunk.last {
                    return chunk;
                }
            }
        }
    }

    #[test]
    fn clients_of_one_folder_share_a_watch_that_goes_a_while_after_the_last() -> std::io::Result<()>
    {
        let temp = tempfile::tempdir()?;
        std::fs::write(temp.path().join("a.rs"), "a")?;
        let reject = |_: pom_rpc::Request, _: &pom_rpc::RequestContext| {
            Err(RemoteError::new(error_code::METHOD_NOT_FOUND, "none"))
        };
        let (client, server) = pom_rpc::mock::connected(pom_rpc::Config::default(), reject)
            .map_err(std::io::Error::other)?;
        let service = FsService::new();
        service.set_linger(Duration::from_millis(200));
        let first = client.subscribe().map_err(std::io::Error::other)?;
        let second = client.subscribe().map_err(std::io::Error::other)?;
        for subscription in [&first, &second] {
            let publisher = server
                .publisher(subscription.channel())
                .map_err(std::io::Error::other)?;
            assert!(service.watch(temp.path(), publisher).is_ok());
        }
        assert_eq!(service.watched(), 1);
        for subscription in [&first, &second] {
            let entries = last_chunk(subscription).entries;
            assert!(entries.is_empty() || entries.iter().any(|entry| entry.path == "a.rs"));
        }
        drop(first);
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(service.watched(), 1, "one client still watches");
        drop(second);
        let deadline = Instant::now() + Duration::from_secs(10);
        while service.watched() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            service.watched(),
            0,
            "gone once nobody watched for the linger"
        );
        Ok(())
    }
}
