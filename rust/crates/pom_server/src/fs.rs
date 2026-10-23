//! A workspace's files for a remote app: one watcher per folder shared by every client that shows it, its tree
//! pushed as a chunked snapshot and then as patches, files read inline or streamed, saves checked against what
//! the client last read, and tree changes run once per op id. Every path passes the guard first.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use files::fs::{Links, INLINE_READ_LIMIT, READ_LIMIT};
use files::guard::{self, GuardError, Parents};
use files::{FileEntry, FileStat, TreePatch};
use pom_remote::server_api::{
    fs_error, method, FsChangeParams, FsFilesParams, FsRead, FsReadParams, FsWriteParams,
    FsWritten, TreeChunk, FS_TREE_TOPIC, WRITE_READY,
};
use pom_rpc::{error_code, Connection, Publisher, RemoteError, StreamSender};
use serde_json::Value;

/// A folder nobody watches any more keeps its watcher this long, for the client that comes right back.
pub const WATCH_LINGER: Duration = Duration::from_secs(60);
pub const CHUNK_ENTRIES: usize = 4000;
/// Below the frame limit with room for the JSON around each entry.
pub const CHUNK_BYTES: usize = 768 * 1024;
pub const MAX_FS_STREAMS: usize = 16;
/// The largest save, matching the largest file that opens.
pub const WRITE_LIMIT: u64 = READ_LIMIT;
const STREAM_PIECE: usize = 32 * 1024;
const TICK: Duration = Duration::from_secs(1);
/// How often a save's sender hears how much has arrived, so it never runs far ahead.
const UPLOAD_REPORT: u64 = 256 * 1024;
/// A save whose text stops coming this long is dropped; the client sends it again.
const UPLOAD_STALL: Duration = Duration::from_secs(30);

/// Where changes may land: the workspace folder, and in the main workspace only the project's config.
pub struct WriteScope {
    pub root: PathBuf,
    pub config: PathBuf,
    pub main: bool,
}

impl WriteScope {
    fn check(&self, target: &Path) -> Result<(), RemoteError> {
        guard::check_scope(target, self.main.then_some(self.config.as_path())).map_err(refused)
    }

    fn is_config(&self, target: &Path) -> bool {
        self.config
            .canonicalize()
            .is_ok_and(|config| config == target)
    }
}

type Outcome = Result<Value, RemoteError>;

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
    Expand(String),
}

/// A folder's watch, kept apart for clients that take gitignored folders lazily and those from before.
type Roots = Arc<Mutex<HashMap<(PathBuf, bool), Sender<Command>>>>;

pub struct FsService {
    roots: Roots,
    linger: Arc<Mutex<Duration>>,
    streams: Arc<AtomicUsize>,
    ops: crate::Ops<Outcome>,
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
            ops: crate::Ops::new(crate::OP_MEMORY),
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
    pub fn watch(&self, root: &Path, lazy: bool, publisher: Publisher) -> Result<(), RemoteError> {
        let root = root
            .canonicalize()
            .map_err(|error| failed(format!("the workspace folder: {error}")))?;
        let key = (root.clone(), lazy);
        let Some(publisher) = join(&locked(&self.roots), &key, publisher) else {
            return Ok(());
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
        // Registering the watches walks the tree; other folders' requests must not wait on it.
        let watcher = if lazy {
            files::TreeWatcher::following(&root, wake)
        } else {
            files::TreeWatcher::new(&root, wake)
        }
        .map_err(|error| failed(format!("watching the workspace: {error}")))?;
        let mut roots = locked(&self.roots);
        let Some(publisher) = join(&roots, &key, publisher) else {
            return Ok(());
        };
        let folder = RootWatch {
            root,
            lazy,
            opened: HashSet::new(),
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
        roots.insert(key, sender);
        Ok(())
    }

    /// Sends the contents of lazy folder `path` to every client following `root`'s tree.
    pub fn expand(&self, root: &Path, path: &str) -> Result<(), RemoteError> {
        let root = root
            .canonicalize()
            .map_err(|error| failed(format!("the workspace folder: {error}")))?;
        let full = guard::resolve(&root, path).map_err(refused)?;
        let Some(path) = guard::relative(&root, &full) else {
            return Err(failed(format!("{path} is not in the workspace")));
        };
        let roots = locked(&self.roots);
        let Some(commands) = roots.get(&(root, true)) else {
            return Err(failed("the tree is not followed"));
        };
        commands
            .send(Command::Expand(path))
            .map_err(|_| failed("the watch went away"))
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

impl FsService {
    /// Saves a file, once per op id: the text inline or streamed, refused when the file is no longer what the
    /// client expects (unless it already holds this very text, as after a save whose answer was lost).
    pub fn write(
        &self,
        connection: &Connection,
        scope: &WriteScope,
        params: FsWriteParams,
    ) -> Result<Value, RemoteError> {
        crate::check_op_id(&params.op_id)?;
        let op_id = params.op_id.clone();
        self.ops.run_keeping(&op_id, || {
            let bytes = match params.text.clone() {
                Some(text) => text.into_bytes(),
                None => match self.receive_upload(connection, &params) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let keep = error.code != fs_error::INCOMPLETE;
                        return (Err(error), keep);
                    }
                },
            };
            (save(scope, &params, &bytes), true)
        })
    }

    /// The save's text as INPUT on its channel, after telling the client it may send; how much has arrived
    /// goes back now and then as an 8-byte count.
    fn receive_upload(
        &self,
        connection: &Connection,
        params: &FsWriteParams,
    ) -> Result<Vec<u8>, RemoteError> {
        if params.size > WRITE_LIMIT {
            return Err(RemoteError::new(
                fs_error::TOO_LARGE,
                format!(
                    "{} MiB is past the {} MiB a save can be",
                    params.size / (1024 * 1024),
                    WRITE_LIMIT / (1024 * 1024)
                ),
            ));
        }
        let Some(mut sender) = attach(connection, params.channel)? else {
            return Err(RemoteError::new(
                error_code::INVALID_PARAMS,
                "a save without its text needs a stream channel",
            ));
        };
        let _slot = self.take_stream()?;
        let Some(input) = sender.take_input() else {
            return Err(RemoteError::new(
                error_code::INTERNAL,
                "the save's input was taken",
            ));
        };
        let incomplete = |why: String| RemoteError::new(fs_error::INCOMPLETE, why);
        sender
            .write(WRITE_READY)
            .map_err(|error| incomplete(error.to_string()))?;
        let size = usize::try_from(params.size).unwrap_or(usize::MAX);
        let mut bytes = Vec::with_capacity(size);
        let mut reported = 0u64;
        while bytes.len() < size {
            let chunk = input
                .recv_timeout(UPLOAD_STALL)
                .map_err(|error| incomplete(format!("the text stopped coming: {error}")))?;
            if chunk.offset != bytes.len() as u64 || bytes.len() + chunk.bytes.len() > size {
                return Err(incomplete("the text came out of order".to_string()));
            }
            bytes.extend_from_slice(&chunk.bytes);
            let received = bytes.len() as u64;
            if received - reported >= UPLOAD_REPORT || received == params.size {
                reported = received;
                sender
                    .write(&received.to_be_bytes())
                    .map_err(|error| incomplete(error.to_string()))?;
            }
        }
        sender.finish("done");
        Ok(bytes)
    }

    /// A change to the tree, once per op id.
    pub fn change(
        &self,
        name: &str,
        scope: &WriteScope,
        params: FsChangeParams,
    ) -> Result<Value, RemoteError> {
        crate::check_op_id(&params.op_id)?;
        self.ops.run(&params.op_id, || {
            change(name, scope, &params).map(|()| Value::Null)
        })
    }
}

fn io_failed(error: std::io::Error) -> RemoteError {
    failed(error.to_string())
}

fn stat_of(metadata: &std::fs::Metadata) -> FileStat {
    FileStat {
        dir: metadata.is_dir(),
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        mtime_ns: metadata
            .modified()
            .map(files::fs::mtime_ns)
            .unwrap_or_default(),
    }
}

fn save(scope: &WriteScope, params: &FsWriteParams, bytes: &[u8]) -> Outcome {
    let target =
        guard::resolve_write(&scope.root, &params.path, Parents::Create).map_err(refused)?;
    scope.check(&target)?;
    if scope.is_config(&target) {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| RemoteError::new(fs_error::REJECTED, "pom.yml must be text"))?;
        pom_config::edit::check_edit(&scope.config, text)
            .map_err(|problem| RemoteError::new(fs_error::REJECTED, problem))?;
    }
    let found = match std::fs::metadata(&target) {
        Ok(metadata) => stat_of(&metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FileStat::default(),
        Err(error) => return Err(io_failed(error)),
    };
    if let Some(expect) = &params.expect {
        let same = found.mtime_ns == expect.mtime_ns && found.size == expect.size;
        if !same {
            let already = std::fs::read(&target).is_ok_and(|current| current == bytes);
            return to_value(FsWritten {
                written: already,
                conflict: !already,
                mtime_ns: found.mtime_ns,
                size: found.size,
            });
        }
    }
    std::fs::write(&target, bytes).map_err(io_failed)?;
    let now = std::fs::metadata(&target).map_err(io_failed)?;
    let now = stat_of(&now);
    to_value(FsWritten {
        written: true,
        conflict: false,
        mtime_ns: now.mtime_ns,
        size: now.size,
    })
}

fn change(name: &str, scope: &WriteScope, params: &FsChangeParams) -> Result<(), RemoteError> {
    let root = &scope.root;
    let path = params.path.as_str();
    match name {
        method::FS_CREATE_FILE => {
            let target = guard::resolve_new(root, path, Parents::Create).map_err(refused)?;
            scope.check(&target)?;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map(|_| ())
                .map_err(io_failed)
        }
        method::FS_CREATE_DIR => {
            let target = guard::resolve_new(root, path, Parents::Create).map_err(refused)?;
            scope.check(&target)?;
            std::fs::create_dir(&target).map_err(io_failed)
        }
        method::FS_RENAME | method::FS_COPY => {
            let source = guard::resolve_entry(root, &params.from).map_err(refused)?;
            scope.check(&source)?;
            let target = guard::resolve_new(root, path, Parents::Create).map_err(refused)?;
            scope.check(&target)?;
            if target.starts_with(&source) {
                return Err(RemoteError::new(
                    error_code::INVALID_PARAMS,
                    "a folder cannot go inside itself",
                ));
            }
            if name == method::FS_RENAME {
                std::fs::rename(&source, &target).map_err(io_failed)
            } else {
                copy_entry(&source, &target).map_err(io_failed)
            }
        }
        method::FS_REMOVE | method::FS_TRASH => {
            let entry = guard::resolve_entry(root, path).map_err(refused)?;
            scope.check(&entry)?;
            if name == method::FS_TRASH {
                return files::trash_path(&entry).map_err(io_failed);
            }
            let kind = entry.symlink_metadata().map_err(io_failed)?.file_type();
            if kind.is_dir() {
                std::fs::remove_dir_all(&entry).map_err(io_failed)
            } else {
                std::fs::remove_file(&entry).map_err(io_failed)
            }
        }
        method::FS_APPEND_GITIGNORE => append_gitignore(scope, path, params.is_dir),
        other => Err(RemoteError::new(
            error_code::METHOD_NOT_FOUND,
            format!("no method {other}"),
        )),
    }
}

/// Copies a file or folder; a link is copied as the link, never as what it points at.
fn copy_entry(from: &Path, to: &Path) -> std::io::Result<()> {
    let kind = from.symlink_metadata()?.file_type();
    if kind.is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(std::fs::read_link(from)?, to);
        #[cfg(not(unix))]
        return Err(std::io::Error::other("links cannot be copied here"));
    }
    if kind.is_dir() {
        std::fs::create_dir(to)?;
        for child in std::fs::read_dir(from)? {
            let child = child?;
            copy_entry(&child.path(), &to.join(child.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ())
}

/// Adds `path` to the .gitignore of the repository holding it, which must be inside the workspace.
fn append_gitignore(scope: &WriteScope, path: &str, is_dir: bool) -> Result<(), RemoteError> {
    let entry = guard::resolve_entry(&scope.root, path).map_err(refused)?;
    let root = guard::resolve(&scope.root, "").map_err(refused)?;
    let Some((work_dir, pattern)) = files::fs::gitignore_line(&entry, is_dir)
        .filter(|(work_dir, _)| work_dir.starts_with(&root))
    else {
        return Err(failed(format!("{path} is not in a repository")));
    };
    let relative = guard::relative(&root, &work_dir.join(".gitignore"))
        .ok_or_else(|| failed("the .gitignore is outside the workspace".to_string()))?;
    let target = guard::resolve_write(&root, &relative, Parents::MustExist).map_err(refused)?;
    scope.check(&target)?;
    let mut contents = match std::fs::read_to_string(&target) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(io_failed(error)),
    };
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(&pattern);
    contents.push('\n');
    std::fs::write(&target, contents).map_err(io_failed)
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

/// Hands `publisher` to the folder's running watch; gives it back when there is none to take it.
fn join(
    roots: &HashMap<(PathBuf, bool), mpsc::Sender<Command>>,
    key: &(PathBuf, bool),
    publisher: Publisher,
) -> Option<Publisher> {
    let Some(commands) = roots.get(key) else {
        return Some(publisher);
    };
    match commands.send(Command::Add(publisher)) {
        Ok(()) => None,
        Err(mpsc::SendError(Command::Add(back))) => Some(back),
        Err(_) => None,
    }
}

/// One watched folder, owned by its thread: everything it pushes goes out in order from here.
struct RootWatch {
    root: PathBuf,
    lazy: bool,
    /// Gitignored folders a client opened, whose contents are in the tree and kept up to date.
    opened: HashSet<String>,
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
        let mut expands = Vec::new();
        let mut found = |watch: &mut RootWatch, batch: Vec<FileEntry>| {
            while let Ok(command) = watch.commands.try_recv() {
                match command {
                    Command::Add(publisher) => watch.add(publisher),
                    Command::Changed => deferred = true,
                    Command::Expand(path) => expands.push(path),
                }
            }
            watch.watch_listed(&batch);
            for entry in &batch {
                watch.entries.insert(entry.path.clone(), entry.clone());
            }
            watch.broadcast(TreeChunk {
                scan_id: watch.scan_id,
                entries: batch,
                ..TreeChunk::default()
            });
            true
        };
        if self.lazy {
            files::fs::walk_lazily(&root, &root, &HashSet::new(), CHUNK_ENTRIES, &mut |batch| {
                found(&mut self, batch)
            });
        } else {
            files::fs::walk_in_batches(&root, &root, Links::Stay, CHUNK_ENTRIES, &mut |batch| {
                found(&mut self, batch)
            });
        }
        self.complete = true;
        self.broadcast(TreeChunk {
            scan_id: self.scan_id,
            last: true,
            ..TreeChunk::default()
        });
        if deferred {
            self.changed();
        }
        for path in expands {
            self.expand(path);
        }
        let mut idle_since: Option<Instant> = None;
        loop {
            match self.commands.recv_timeout(TICK) {
                Ok(Command::Add(publisher)) => self.add(publisher),
                Ok(Command::Changed) => self.changed(),
                Ok(Command::Expand(path)) => self.expand(path),
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
            Ok(Command::Expand(path)) => {
                drop(roots);
                self.expand(path);
                false
            }
            Err(_) => {
                roots.remove(&(self.root.clone(), self.lazy));
                true
            }
        }
    }

    /// Sends a new subscriber everything known so far, as a fresh snapshot.
    fn add(&mut self, publisher: Publisher) {
        if self.send_snapshot(&publisher) {
            self.subscribers.push(publisher);
        }
    }

    fn send_snapshot(&self, publisher: &Publisher) -> bool {
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
                return false;
            }
        }
        true
    }

    /// Walks the whole folder again and sends everyone the new snapshot, as after its own .gitignore changed.
    fn rescan(&mut self) {
        self.entries.clear();
        let opened = self.opened.clone();
        let root = self.root.clone();
        files::fs::walk_lazily(&root, &root, &opened, usize::MAX, &mut |batch| {
            self.watch_listed(&batch);
            for entry in batch {
                self.entries.insert(entry.path.clone(), entry);
            }
            true
        });
        self.scan_id += 1;
        let subscribers = std::mem::take(&mut self.subscribers);
        self.subscribers = subscribers
            .into_iter()
            .filter(|publisher| self.send_snapshot(publisher))
            .collect();
    }

    /// A lazy tree watches only the folders it lists; the full one watched its whole tree at the start.
    fn watch_listed(&self, batch: &[FileEntry]) {
        if !self.lazy {
            return;
        }
        for entry in batch.iter().filter(|entry| entry.is_dir && !entry.lazy) {
            self.watcher.watch_folder(&self.root.join(&entry.path));
        }
    }

    /// Whether `path` lies in a folder whose contents nobody opened, so no client has it.
    fn under_lazy(&self, path: &str) -> bool {
        let mut current = path;
        while let Some((parent, _)) = current.rsplit_once('/') {
            if self.entries.get(parent).is_some_and(|entry| entry.lazy) {
                return true;
            }
            current = parent;
        }
        false
    }

    fn expand(&mut self, path: String) {
        if !self.entries.get(&path).is_some_and(|entry| entry.lazy) {
            return;
        }
        self.opened.insert(path.clone());
        self.watcher.watch_folder(&self.root.join(&path));
        let patch = files::fs::patch_lazily(&self.root, &[path], &self.opened);
        self.watch_listed(&patch.upserted);
        self.publish(patch);
    }

    fn broadcast(&mut self, chunk: TreeChunk) {
        self.subscribers
            .retain(|publisher| publisher.publish(FS_TREE_TOPIC, &chunk).is_ok());
    }

    fn changed(&mut self) {
        let mut changes = self.watcher.take_changes();
        if changes.is_empty() {
            return;
        }
        let mut rescan = false;
        let patch = if self.lazy {
            changes.retain(|path| !self.under_lazy(path));
            let mut rules_changed = Vec::new();
            for path in &changes {
                match path.rsplit_once('/') {
                    Some((folder, ".gitignore")) => rules_changed.push(folder.to_string()),
                    None if path == ".gitignore" => rescan = true,
                    _ => {}
                }
            }
            if rescan {
                self.rescan();
            }
            changes.extend(rules_changed);
            if changes.is_empty() {
                return;
            }
            files::fs::patch_lazily(&self.root, &changes, &self.opened)
        } else {
            files::fs::patch_for(&self.root, &changes, Links::Stay)
        };
        self.publish(patch);
    }

    fn publish(&mut self, patch: TreePatch) {
        let TreePatch {
            upserted,
            removed,
            changed,
        } = patch;
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
            assert!(service.watch(temp.path(), false, publisher).is_ok());
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

    /// The entries of every chunk up to and including the next with `last`.
    fn entries_until_last(subscription: &pom_rpc::Subscription) -> Vec<(String, bool)> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut found = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "no last chunk: {found:?}");
            if let Ok(pom_rpc::PushEvent::Value { value, .. }) =
                subscription.recv_timeout(Duration::from_millis(100))
            {
                let chunk: TreeChunk = serde_json::from_value(value).unwrap_or_default();
                found.extend(
                    chunk
                        .entries
                        .into_iter()
                        .map(|entry| (entry.path, entry.lazy)),
                );
                if chunk.last {
                    found.sort();
                    return found;
                }
            }
        }
    }

    #[test]
    fn a_lazy_tree_sends_an_ignored_folder_once_it_is_opened() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        std::fs::create_dir(root.join(".git"))?;
        std::fs::write(root.join(".gitignore"), "dist/\n")?;
        std::fs::create_dir_all(root.join("dist/assets"))?;
        std::fs::write(root.join("dist/app.js"), "x")?;
        std::fs::write(root.join("main.rs"), "x")?;
        let reject = |_: pom_rpc::Request, _: &pom_rpc::RequestContext| {
            Err(RemoteError::new(error_code::METHOD_NOT_FOUND, "none"))
        };
        let (client, server) = pom_rpc::mock::connected(pom_rpc::Config::default(), reject)
            .map_err(std::io::Error::other)?;
        let service = FsService::new();
        let subscription = client.subscribe().map_err(std::io::Error::other)?;
        let publisher = server
            .publisher(subscription.channel())
            .map_err(std::io::Error::other)?;
        assert!(service.watch(&root, true, publisher).is_ok());
        let owned = |list: &[(&str, bool)]| -> Vec<(String, bool)> {
            list.iter()
                .map(|(path, lazy)| (path.to_string(), *lazy))
                .collect()
        };
        assert_eq!(
            entries_until_last(&subscription),
            owned(&[(".gitignore", false), ("dist", true), ("main.rs", false)])
        );
        assert!(service.expand(&root, "../elsewhere").is_err());
        assert!(service.expand(&root, "dist").is_ok());
        assert_eq!(
            entries_until_last(&subscription),
            owned(&[
                ("dist", false),
                ("dist/app.js", false),
                ("dist/assets", true)
            ])
        );
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("dist/assets/hidden.css"), "x")?;
        std::fs::write(root.join("dist/new.js"), "x")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let entries = entries_until_last(&subscription);
            assert!(
                !entries
                    .iter()
                    .any(|(path, _)| path.starts_with("dist/assets/")),
                "{entries:?}"
            );
            if entries.iter().any(|(path, _)| path == "dist/new.js") {
                break;
            }
            assert!(Instant::now() < deadline, "no patch for dist/new.js");
        }
        Ok(())
    }

    #[test]
    fn main_saves_only_its_config_and_only_one_that_loads() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let config = root.join("pom.yml");
        std::fs::write(&config, "session: demo\n")?;
        std::fs::write(root.join("app.rs"), "fn main() {}\n")?;
        let scope = WriteScope {
            root: root.clone(),
            config: config.clone(),
            main: true,
        };
        let params = |path: &str| FsWriteParams {
            path: path.into(),
            op_id: "op".into(),
            ..FsWriteParams::default()
        };
        let code = |outcome: Outcome| outcome.err().map(|error| error.code);
        assert_eq!(
            code(save(&scope, &params("app.rs"), b"changed")),
            Some("read_only".to_string())
        );
        assert_eq!(
            code(save(&scope, &params("pom.yml"), b"repos: [")),
            Some(fs_error::REJECTED.to_string())
        );
        assert_eq!(std::fs::read_to_string(&config)?, "session: demo\n");
        let saved = save(&scope, &params("pom.yml"), b"session: demo\n# note\n");
        assert!(saved.is_ok(), "{saved:?}");
        assert_eq!(std::fs::read_to_string(&config)?, "session: demo\n# note\n");
        let branch = WriteScope {
            main: false,
            ..scope
        };
        assert!(save(&branch, &params("app.rs"), b"changed").is_ok());
        Ok(())
    }
}
