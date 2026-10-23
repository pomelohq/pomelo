//! Where a project's files are read from: this machine's disk ([`LocalFs`]), or another machine's through the
//! server there. The tree comes as a feed: a snapshot that fills in as the walk goes, then patches as files
//! change.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::search::{FileResult as SearchHit, SearchRequest};
use crate::{BackgroundScan, FileEntry, TreeWatcher, SKIP_DIRS};

/// A read up to this size comes back in its answer; a larger one is streamed.
pub const INLINE_READ_LIMIT: u64 = 512 * 1024;
/// Larger files are refused rather than read whole.
pub const READ_LIMIT: u64 = 64 * 1024 * 1024;
pub const IMAGE_READ_LIMIT: u64 = 32 * 1024 * 1024;
const BINARY_SNIFF: usize = 8192;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileStat {
    pub dir: bool,
    pub size: u64,
    pub mtime_ns: u64,
}

impl FileStat {
    pub fn modified(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_nanos(self.mtime_ns)
    }

    fn of(metadata: &std::fs::Metadata) -> FileStat {
        FileStat {
            dir: metadata.is_dir(),
            size: if metadata.is_dir() { 0 } else { metadata.len() },
            mtime_ns: metadata.modified().map(mtime_ns).unwrap_or_default(),
        }
    }
}

pub fn mtime_ns(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// A file as read: its text (`None` for a binary file) and the modification time and size it had.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileResult {
    pub text: Option<String>,
    pub binary: bool,
    pub mtime_ns: u64,
    pub size: u64,
}

impl FileResult {
    /// Classifies `bytes` the way [`crate::read`] does: a NUL early on means binary.
    pub fn from_bytes(bytes: Vec<u8>, mtime_ns: u64) -> FileResult {
        let size = bytes.len() as u64;
        let binary = is_binary(&bytes);
        FileResult {
            text: (!binary).then(|| match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(invalid) => String::from_utf8_lossy(invalid.as_bytes()).into_owned(),
            }),
            binary,
            mtime_ns,
            size,
        }
    }

    pub fn modified(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_nanos(self.mtime_ns)
    }
}

pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF).any(|byte| *byte == 0)
}

/// A file whose contents may have changed, with what it has now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChangedFile {
    pub path: String,
    pub mtime_ns: u64,
    pub size: u64,
}

/// Entries to add or replace, paths gone with everything under them, and the files whose contents changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreePatch {
    pub upserted: Vec<FileEntry>,
    pub removed: Vec<String>,
    pub changed: Vec<ChangedFile>,
}

impl TreePatch {
    pub fn is_empty(&self) -> bool {
        self.upserted.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeEvent {
    /// Everything found so far, replacing what was there; `complete` once the walk is done.
    Snapshot {
        entries: Vec<FileEntry>,
        complete: bool,
    },
    Patch(TreePatch),
    /// The feed stopped (the machine went away); a fresh snapshot follows once it is back.
    Interrupted(String),
}

/// Why a write did not happen.
#[derive(Debug)]
pub enum WriteError {
    /// The file is no longer what the write expected (it changed since it was read); this is what it is now.
    Conflict(FileStat),
    Failed(io::Error),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Conflict(_) => formatter.write_str("the file changed since it was opened"),
            WriteError::Failed(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<io::Error> for WriteError {
    fn from(error: io::Error) -> Self {
        WriteError::Failed(error)
    }
}

const NO_SEARCH: &str = "Searching these files is not available here";

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "these files cannot be changed from here",
    )
}

pub trait TreeFeed: Send {
    /// What happened since the last call, oldest first.
    fn poll(&mut self) -> Vec<TreeEvent>;
}

/// A workspace's files, wherever they are. Paths are '/'-separated and relative to the workspace.
pub trait ProjectFs: Send + Sync {
    /// The workspace folder, when it is on this machine.
    fn local_root(&self) -> Option<&Path>;
    /// The tree, kept up to date; `wake` runs whenever something new is ready to poll.
    fn watch(&self, wake: Arc<dyn Fn() + Send + Sync>) -> Box<dyn TreeFeed>;
    fn stat(&self, path: &str) -> io::Result<FileStat>;
    fn read(&self, path: &str) -> io::Result<FileResult>;
    /// The whole file when it is no larger than `limit`.
    fn read_bytes(&self, path: &str, limit: u64) -> io::Result<Vec<u8>>;
    /// Every file the finder offers, gitignored ones only when asked.
    fn project_files(&self, include_ignored: bool) -> io::Result<Vec<String>>;
    /// Writes `text` over the file in place. With `expect`, a file that is no longer that one is left as it is
    /// and reported as a conflict.
    fn write(
        &self,
        _path: &str,
        _text: &str,
        _expect: Option<&FileStat>,
    ) -> Result<FileStat, WriteError> {
        Err(WriteError::Failed(unsupported()))
    }
    fn create_file(&self, _path: &str) -> io::Result<()> {
        Err(unsupported())
    }
    fn create_dir(&self, _path: &str) -> io::Result<()> {
        Err(unsupported())
    }
    fn rename(&self, _from: &str, _to: &str) -> io::Result<()> {
        Err(unsupported())
    }
    fn copy(&self, _from: &str, _to: &str) -> io::Result<()> {
        Err(unsupported())
    }
    fn remove(&self, _path: &str) -> io::Result<()> {
        Err(unsupported())
    }
    fn trash(&self, _path: &str) -> io::Result<()> {
        Err(unsupported())
    }
    /// Adds `path` to the .gitignore of the repository it is in.
    fn add_to_gitignore(&self, _path: &str, _is_dir: bool) -> io::Result<()> {
        Err(unsupported())
    }
    /// Runs a project search, handing each file with matches to `on_file` as it is found, until done or `cancel`
    /// is set; returns whether a limit was reached.
    fn search(
        &self,
        _request: &SearchRequest,
        _cancel: &AtomicBool,
        _on_file: &(dyn Fn(SearchHit) + Sync),
    ) -> io::Result<bool> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            NO_SEARCH.to_string(),
        ))
    }
    /// Why these files cannot be searched from here, when they cannot.
    fn search_refusal(&self) -> Option<String> {
        Some(NO_SEARCH.to_string())
    }
    /// Asks for the contents of a lazy folder; they come through the tree's feed.
    fn expand(&self, _path: &str) -> io::Result<()> {
        Ok(())
    }
    /// Whether the machine the files are on can be reached right now.
    fn online(&self) -> bool {
        true
    }
    /// The machine the files are on, when it is another one.
    fn host(&self) -> Option<&str> {
        None
    }
}

/// The repository folder holding `full` (the nearest with a `.git`) and the .gitignore line naming `full` there.
pub fn gitignore_line(full: &Path, is_dir: bool) -> Option<(PathBuf, String)> {
    let work_dir = full.ancestors().find(|dir| dir.join(".git").exists())?;
    let relative = full.strip_prefix(work_dir).ok()?;
    let mut pattern = relative.to_string_lossy().replace('\\', "/");
    if pattern.is_empty() {
        return None;
    }
    if is_dir {
        pattern.push('/');
    }
    Some((work_dir.to_path_buf(), pattern))
}

/// Where a file of `len` bytes splits into `pieces` runs of whole lines: the start of each, then `len`.
pub fn line_bounds(full: &Path, len: u64, pieces: u64) -> io::Result<Vec<u64>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut bounds = vec![0u64];
    let mut file = std::fs::File::open(full)?;
    for index in 1..pieces {
        let previous = bounds.last().copied().unwrap_or(0);
        let nominal = (len * index / pieces).max(previous);
        file.seek(SeekFrom::Start(nominal))?;
        let mut window = [0u8; 64 * 1024];
        let mut at = nominal;
        let boundary = loop {
            let read = file.read(&mut window)?;
            if read == 0 {
                break len;
            }
            if let Some(newline) = window[..read].iter().position(|byte| *byte == b'\n') {
                break at + newline as u64 + 1;
            }
            at += read as u64;
        };
        bounds.push(boundary);
    }
    bounds.push(len);
    Ok(bounds)
}

/// The bytes of `full` from `start` up to `end`.
pub fn read_range(full: &Path, start: u64, end: u64) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(full)?;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0u8; usize::try_from(end.saturating_sub(start)).unwrap_or(0)];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// A file past what can be opened, told apart from other failed reads.
#[derive(Debug)]
pub struct TooLarge(pub String);

impl std::fmt::Display for TooLarge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for TooLarge {}

pub fn too_large(size: u64, limit: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        TooLarge(format!(
            "the file is {} MiB, past the {} MiB that can be opened",
            size / (1024 * 1024),
            limit / (1024 * 1024)
        )),
    )
}

pub fn is_too_large(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<TooLarge>())
}

/// The files on this machine, read in place.
pub struct LocalFs {
    root: PathBuf,
}

impl LocalFs {
    pub fn new(root: PathBuf) -> LocalFs {
        LocalFs { root }
    }
}

impl ProjectFs for LocalFs {
    fn local_root(&self) -> Option<&Path> {
        Some(&self.root)
    }

    fn watch(&self, wake: Arc<dyn Fn() + Send + Sync>) -> Box<dyn TreeFeed> {
        Box::new(LocalFeed::start(self.root.clone(), wake))
    }

    fn stat(&self, path: &str) -> io::Result<FileStat> {
        let full = crate::safe_join(&self.root, path)?;
        Ok(FileStat::of(&std::fs::metadata(full)?))
    }

    fn read(&self, path: &str) -> io::Result<FileResult> {
        let full = crate::safe_join(&self.root, path)?;
        let modified = std::fs::metadata(&full)?.modified().map(mtime_ns)?;
        Ok(FileResult::from_bytes(std::fs::read(&full)?, modified))
    }

    fn read_bytes(&self, path: &str, limit: u64) -> io::Result<Vec<u8>> {
        let full = crate::safe_join(&self.root, path)?;
        let size = std::fs::metadata(&full)?.len();
        if size > limit {
            return Err(too_large(size, limit));
        }
        std::fs::read(full)
    }

    fn project_files(&self, include_ignored: bool) -> io::Result<Vec<String>> {
        Ok(crate::project_files(&self.root, include_ignored))
    }

    /// The editor here watches the disk itself, so `expect` is not checked: a save writes what it has.
    fn write(
        &self,
        path: &str,
        text: &str,
        _expect: Option<&FileStat>,
    ) -> Result<FileStat, WriteError> {
        let modified = crate::write(&self.root, path, text)?;
        Ok(FileStat {
            dir: false,
            size: text.len() as u64,
            mtime_ns: mtime_ns(modified),
        })
    }

    fn create_file(&self, path: &str) -> io::Result<()> {
        crate::create_file(&self.root, path)
    }

    fn create_dir(&self, path: &str) -> io::Result<()> {
        crate::create_dir(&self.root, path)
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        crate::rename(&self.root, from, to)
    }

    fn copy(&self, from: &str, to: &str) -> io::Result<()> {
        crate::copy_recursive(&self.root, from, to)
    }

    fn remove(&self, path: &str) -> io::Result<()> {
        crate::remove(&self.root, path)
    }

    fn trash(&self, path: &str) -> io::Result<()> {
        crate::trash(&self.root, path)
    }

    fn search(
        &self,
        request: &SearchRequest,
        cancel: &AtomicBool,
        on_file: &(dyn Fn(SearchHit) + Sync),
    ) -> io::Result<bool> {
        crate::search::run(&self.root, request, cancel, on_file)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }

    fn search_refusal(&self) -> Option<String> {
        None
    }

    fn add_to_gitignore(&self, path: &str, is_dir: bool) -> io::Result<()> {
        let full = crate::safe_join(&self.root, path)?;
        match gitignore_line(&full, is_dir) {
            Some((work_dir, pattern)) => crate::append_to_gitignore(&work_dir, &pattern),
            None => Ok(()),
        }
    }
}

/// The local tree: a parallel walk, then the watcher's batches turned into patches.
pub struct LocalFeed {
    root: PathBuf,
    scan: Option<BackgroundScan>,
    watcher: Option<TreeWatcher>,
}

impl LocalFeed {
    pub fn start(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> LocalFeed {
        let scan = BackgroundScan::start(root.clone(), wake.clone());
        let watcher = TreeWatcher::new(&root, wake)
            .map_err(|error| eprintln!("file tree watch failed: {error}"))
            .ok();
        LocalFeed {
            root,
            scan: Some(scan),
            watcher,
        }
    }
}

impl TreeFeed for LocalFeed {
    fn poll(&mut self) -> Vec<TreeEvent> {
        let mut events = Vec::new();
        if let Some(update) = self.scan.as_ref().and_then(BackgroundScan::latest) {
            if !update.scanning {
                self.scan = None;
            }
            events.push(TreeEvent::Snapshot {
                entries: update.entries,
                complete: !update.scanning,
            });
        }
        let changes = self
            .watcher
            .as_ref()
            .map(TreeWatcher::take_changes)
            .unwrap_or_default();
        if !changes.is_empty() {
            // Mid-walk the snapshot already holds what changed; open files still look again.
            let patch = if self.scan.is_some() {
                TreePatch {
                    changed: changes
                        .into_iter()
                        .map(|path| ChangedFile {
                            path,
                            ..ChangedFile::default()
                        })
                        .collect(),
                    ..TreePatch::default()
                }
            } else {
                patch_for(&self.root, &changes, Links::Follow)
            };
            events.push(TreeEvent::Patch(patch));
        }
        events
    }
}

/// Whether a walk goes into symlinked folders. A server never does: a link may lead out of the workspace, or
/// round in a loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Links {
    Follow,
    Stay,
}

fn entry_for(root: &Path, full: &Path, links: Links) -> Option<(FileEntry, bool)> {
    let path = crate::guard::relative(root, full)?;
    let metadata = match links {
        Links::Follow => std::fs::metadata(full).ok()?,
        Links::Stay => {
            let own = std::fs::symlink_metadata(full).ok()?;
            if own.file_type().is_symlink() {
                let dir = std::fs::metadata(full).is_ok_and(|target| target.is_dir())
                    && crate::guard::stays_inside(root, full);
                return Some((
                    FileEntry {
                        repo: String::new(),
                        path,
                        is_dir: dir,
                        size: 0,
                        lazy: false,
                    },
                    false,
                ));
            }
            own
        }
    };
    let is_dir = metadata.is_dir();
    let name = full.file_name().map(|name| name.to_string_lossy());
    if is_dir && name.is_some_and(|name| SKIP_DIRS.contains(&name.as_ref())) {
        return None;
    }
    Some((
        FileEntry {
            repo: String::new(),
            path,
            is_dir,
            size: if is_dir { 0 } else { metadata.len() },
            lazy: false,
        },
        is_dir,
    ))
}

/// Walks `dir` under `root` breadth first, handing `batch` entries at a time to `found` until it returns false.
pub fn walk_in_batches(
    root: &Path,
    dir: &Path,
    links: Links,
    batch: usize,
    found: &mut dyn FnMut(Vec<FileEntry>) -> bool,
) -> bool {
    walk(root, dir, links, None, batch, found)
}

/// Like [`walk_in_batches`] without following links, except that a gitignored folder is listed as lazy and not
/// entered unless its path is in `opened`; inside an opened one, every folder is gitignored.
pub fn walk_lazily(
    root: &Path,
    dir: &Path,
    opened: &HashSet<String>,
    batch: usize,
    found: &mut dyn FnMut(Vec<FileEntry>) -> bool,
) -> bool {
    walk(root, dir, Links::Stay, Some(opened), batch, found)
}

fn walk(
    root: &Path,
    dir: &Path,
    links: Links,
    opened: Option<&HashSet<String>>,
    batch: usize,
    found: &mut dyn FnMut(Vec<FileEntry>) -> bool,
) -> bool {
    let start = match opened {
        Some(_) => IgnoreRules::inside(dir),
        None => IgnoreRules::default(),
    };
    let mut queue = std::collections::VecDeque::from([(dir.to_path_buf(), start)]);
    let mut pending = Vec::new();
    while let Some((next, rules)) = queue.pop_front() {
        let Ok(children) = std::fs::read_dir(&next) else {
            continue;
        };
        for child in children.flatten() {
            let full = child.path();
            let Some((mut entry, mut descend)) = entry_for(root, &full, links) else {
                continue;
            };
            let mut child_rules = None;
            if let (Some(opened), true) = (opened, descend) {
                let ignored = rules.ignores(&full, true);
                if ignored && !opened.contains(&entry.path) {
                    entry.lazy = true;
                    descend = false;
                } else {
                    child_rules = Some(rules.enter(&full, ignored));
                }
            }
            pending.push(entry);
            if descend {
                queue.push_back((full, child_rules.unwrap_or_default()));
            }
            if pending.len() >= batch && !found(std::mem::take(&mut pending)) {
                return false;
            }
        }
    }
    pending.is_empty() || found(pending)
}

/// The gitignore rules for what is inside one folder, innermost last, as git applies them.
#[derive(Clone, Default)]
struct IgnoreRules {
    matchers: Vec<Arc<Gitignore>>,
    inside_ignored: bool,
}

fn global_gitignore() -> Option<Arc<Gitignore>> {
    static GLOBAL: std::sync::OnceLock<Option<Arc<Gitignore>>> = std::sync::OnceLock::new();
    GLOBAL
        .get_or_init(|| {
            let (global, _) = Gitignore::global();
            (!global.is_empty()).then(|| Arc::new(global))
        })
        .clone()
}

fn gitignore_at(dir: &Path, file: &Path) -> Option<Arc<Gitignore>> {
    if !file.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(dir);
    if let Some(error) = builder.add(file) {
        eprintln!("reading {}: {error}", file.display());
    }
    let built = builder.build().ok()?;
    (!built.is_empty()).then(|| Arc::new(built))
}

impl IgnoreRules {
    /// The rules for `dir`'s contents, read from the repository holding it down to `dir`.
    fn inside(dir: &Path) -> IgnoreRules {
        let Some(top) = dir.ancestors().find(|folder| folder.join(".git").exists()) else {
            return IgnoreRules::default();
        };
        let Ok(below) = dir.strip_prefix(top) else {
            return IgnoreRules::default();
        };
        let mut rules = IgnoreRules::default().enter(top, false);
        let mut folder = top.to_path_buf();
        for part in below.components() {
            folder.push(part);
            let ignored = rules.ignores(&folder, true);
            rules = rules.enter(&folder, ignored);
        }
        rules
    }

    fn enter(&self, dir: &Path, ignored: bool) -> IgnoreRules {
        if ignored || self.inside_ignored {
            return IgnoreRules {
                matchers: Vec::new(),
                inside_ignored: true,
            };
        }
        let mut rules = if dir.join(".git").exists() {
            let mut fresh = IgnoreRules::default();
            fresh.matchers.extend(global_gitignore());
            fresh
                .matchers
                .extend(gitignore_at(dir, &dir.join(".git/info/exclude")));
            fresh
        } else {
            self.clone()
        };
        rules
            .matchers
            .extend(gitignore_at(dir, &dir.join(".gitignore")));
        rules
    }

    fn ignores(&self, full: &Path, is_dir: bool) -> bool {
        if self.inside_ignored {
            return true;
        }
        for matcher in self.matchers.iter().rev() {
            match matcher.matched(full, is_dir) {
                ignore::Match::Ignore(_) => return true,
                ignore::Match::Whitelist(_) => return false,
                ignore::Match::None => {}
            }
        }
        false
    }
}

/// What changed under `root` at `changed` (paths a watcher reported), read again from disk.
pub fn patch_for(root: &Path, changed: &[String], links: Links) -> TreePatch {
    patch(root, changed, links, None)
}

/// [`patch_for`] for a tree walked with [`walk_lazily`].
pub fn patch_lazily(root: &Path, changed: &[String], opened: &HashSet<String>) -> TreePatch {
    patch(root, changed, Links::Stay, Some(opened))
}

fn patch(
    root: &Path,
    changed: &[String],
    links: Links,
    opened: Option<&HashSet<String>>,
) -> TreePatch {
    let mut changed: Vec<&String> = changed.iter().collect();
    changed.sort();
    changed.dedup();
    let mut tops: Vec<&String> = Vec::new();
    for path in &changed {
        if !tops
            .last()
            .is_some_and(|parent| path.starts_with(&format!("{parent}/")))
        {
            tops.push(path);
        }
    }
    let mut patch = TreePatch::default();
    for path in tops {
        if path.split('/').any(|part| SKIP_DIRS.contains(&part)) {
            continue;
        }
        patch.removed.push(path.clone());
        let full = root.join(path);
        let Some((mut entry, mut descend)) = entry_for(root, &full, links) else {
            continue;
        };
        if let (Some(opened), true, Some(parent)) = (opened, descend, full.parent()) {
            if IgnoreRules::inside(parent).ignores(&full, true) && !opened.contains(path) {
                entry.lazy = true;
                descend = false;
            }
        }
        patch.upserted.push(entry);
        if descend {
            walk(root, &full, links, opened, usize::MAX, &mut |found| {
                patch.upserted.extend(found);
                true
            });
        }
    }
    for path in changed {
        let Ok(metadata) = std::fs::metadata(root.join(path)) else {
            continue;
        };
        if metadata.is_file() {
            patch.changed.push(ChangedFile {
                path: path.clone(),
                mtime_ns: metadata.modified().map(mtime_ns).unwrap_or_default(),
                size: metadata.len(),
            });
        }
    }
    patch
}

/// Applies `patch` to a sorted entry list, keeping it sorted.
pub fn apply_patch(entries: &mut Vec<FileEntry>, patch: &TreePatch) {
    if patch.removed.is_empty() && patch.upserted.is_empty() {
        return;
    }
    if !patch.removed.is_empty() {
        let mut removed: Vec<&str> = patch.removed.iter().map(String::as_str).collect();
        removed.sort_unstable();
        entries.retain(|entry| !under_any(&removed, &entry.path));
    }
    let upserted: std::collections::HashSet<&str> = patch
        .upserted
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    entries.retain(|entry| !upserted.contains(entry.path.as_str()));
    entries.extend(patch.upserted.iter().cloned());
    entries.sort_by(|a, b| a.path.cmp(&b.path));
}

/// Whether `path` is one of the sorted `tops` or lies under one.
fn under_any(tops: &[&str], path: &str) -> bool {
    let mut candidate = path;
    loop {
        if tops.binary_search(&candidate).is_ok() {
            return true;
        }
        match candidate.rsplit_once('/') {
            Some((parent, _)) => candidate = parent,
            None => return false,
        }
    }
}

/// Files that cannot be read where they are, such as on a host whose pom does not serve them: an empty tree, and
/// `reason` for every read.
pub struct UnservedFs {
    reason: String,
}

impl UnservedFs {
    pub fn new(reason: impl Into<String>) -> UnservedFs {
        UnservedFs {
            reason: reason.into(),
        }
    }

    fn refused(&self) -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, self.reason.clone())
    }
}

struct EmptyTree {
    sent: bool,
}

impl TreeFeed for EmptyTree {
    fn poll(&mut self) -> Vec<TreeEvent> {
        if std::mem::replace(&mut self.sent, true) {
            return Vec::new();
        }
        vec![TreeEvent::Snapshot {
            entries: Vec::new(),
            complete: true,
        }]
    }
}

impl ProjectFs for UnservedFs {
    fn local_root(&self) -> Option<&Path> {
        None
    }

    fn watch(&self, _wake: Arc<dyn Fn() + Send + Sync>) -> Box<dyn TreeFeed> {
        Box::new(EmptyTree { sent: false })
    }

    fn search_refusal(&self) -> Option<String> {
        Some(self.reason.clone())
    }

    fn stat(&self, _path: &str) -> io::Result<FileStat> {
        Err(self.refused())
    }

    fn read(&self, _path: &str) -> io::Result<FileResult> {
        Err(self.refused())
    }

    fn read_bytes(&self, _path: &str, _limit: u64) -> io::Result<Vec<u8>> {
        Err(self.refused())
    }

    fn project_files(&self, _include_ignored: bool) -> io::Result<Vec<String>> {
        Err(self.refused())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            repo: String::new(),
            path: path.into(),
            is_dir,
            size: 0,
            lazy: false,
        }
    }

    #[test]
    fn entries_and_reads_round_trip_through_json() -> serde_json::Result<()> {
        let file = FileEntry {
            repo: String::new(),
            path: "src/main.rs".into(),
            is_dir: false,
            size: 12,
            lazy: false,
        };
        let json = serde_json::to_string(&file)?;
        assert_eq!(json, r#"{"path":"src/main.rs","dir":false,"size":12}"#);
        assert_eq!(serde_json::from_str::<FileEntry>(&json)?, file);
        let read = FileResult::from_bytes(b"fn main() {}".to_vec(), 7);
        assert_eq!(
            serde_json::from_str::<FileResult>(&serde_json::to_string(&read)?)?,
            read
        );
        let binary = FileResult::from_bytes(vec![0, 1, 2], 1);
        assert!(binary.binary && binary.text.is_none());
        let stat = FileStat {
            dir: true,
            size: 0,
            mtime_ns: 1_700_000_000_123_456_789,
        };
        assert_eq!(
            serde_json::from_str::<FileStat>(&serde_json::to_string(&stat)?)?,
            stat
        );
        assert_eq!(mtime_ns(stat.modified()), stat.mtime_ns);
        assert_eq!(serde_json::from_str::<FileStat>("{}")?, FileStat::default());
        Ok(())
    }

    #[test]
    fn a_patch_replaces_whole_subtrees() {
        let mut entries = vec![
            entry("a", true),
            entry("a/b", true),
            entry("a/b/c.rs", false),
            entry("ab.rs", false),
            entry("z.rs", false),
        ];
        let patch = TreePatch {
            removed: vec!["a/b".into(), "z.rs".into()],
            upserted: vec![
                entry("a/b", true),
                entry("a/b/d.rs", false),
                entry("n.rs", false),
            ],
            changed: Vec::new(),
        };
        apply_patch(&mut entries, &patch);
        let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(paths, ["a", "a/b", "a/b/d.rs", "ab.rs", "n.rs"]);
    }

    #[test]
    fn local_patches_match_a_fresh_walk() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        std::fs::create_dir_all(root.join("src/old"))?;
        std::fs::write(root.join("src/old/a.rs"), "a")?;
        let mut entries = crate::list(root);
        std::fs::remove_dir_all(root.join("src/old"))?;
        std::fs::create_dir_all(root.join("src/new"))?;
        std::fs::write(root.join("src/new/b.rs"), "bb")?;
        let patch = patch_for(
            root,
            &["src/old".into(), "src/new".into(), "src/new/b.rs".into()],
            Links::Follow,
        );
        assert_eq!(patch.changed.len(), 1);
        assert_eq!(patch.changed[0].size, 2);
        apply_patch(&mut entries, &patch);
        assert_eq!(entries, crate::list(root));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_walk_that_stays_never_follows_a_link_out() -> io::Result<()> {
        let outside = tempfile::tempdir()?;
        std::fs::write(outside.path().join("secret.txt"), "x")?;
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        std::fs::create_dir(root.join("src"))?;
        std::fs::write(root.join("src/a.rs"), "a")?;
        std::os::unix::fs::symlink(outside.path(), root.join("away"))?;
        std::os::unix::fs::symlink(root.join("src"), root.join("alias"))?;
        let mut found = Vec::new();
        assert!(walk_in_batches(
            &root,
            &root,
            Links::Stay,
            2,
            &mut |batch| {
                assert!(batch.len() <= 2);
                found.extend(batch);
                true
            }
        ));
        found.sort_by(|a, b| a.path.cmp(&b.path));
        let listed: Vec<(&str, bool)> = found
            .iter()
            .map(|entry| (entry.path.as_str(), entry.is_dir))
            .collect();
        assert_eq!(
            listed,
            [
                ("alias", true),
                ("away", false),
                ("src", true),
                ("src/a.rs", false)
            ]
        );
        Ok(())
    }

    fn lazy_paths(root: &Path, dir: &Path, opened: &HashSet<String>) -> Vec<(String, bool)> {
        let mut found = Vec::new();
        walk_lazily(root, dir, opened, 3, &mut |batch| {
            found.extend(batch);
            true
        });
        found.sort_by(|a, b| a.path.cmp(&b.path));
        found
            .into_iter()
            .map(|entry| (entry.path, entry.lazy))
            .collect()
    }

    #[test]
    fn a_lazy_walk_lists_ignored_folders_without_entering_them() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let api = root.join("api");
        std::fs::create_dir_all(api.join(".git"))?;
        std::fs::write(api.join(".gitignore"), "dist\n/tmp/\n*.log\n")?;
        std::fs::create_dir_all(api.join("src/dist"))?;
        std::fs::create_dir_all(api.join("dist/assets"))?;
        std::fs::write(api.join("dist/app.js"), "x")?;
        std::fs::write(api.join("dist/assets/a.css"), "x")?;
        std::fs::create_dir_all(api.join("tmp/cache"))?;
        std::fs::write(api.join("src/main.rs"), "x")?;
        std::fs::write(api.join("debug.log"), "x")?;
        std::fs::create_dir_all(root.join("web/dist"))?;
        let lazy = |path: &str| (path.to_string(), true);
        let eager = |path: &str| (path.to_string(), false);
        assert_eq!(
            lazy_paths(&root, &root, &HashSet::new()),
            [
                eager("api"),
                eager("api/.gitignore"),
                eager("api/debug.log"),
                lazy("api/dist"),
                eager("api/src"),
                lazy("api/src/dist"),
                eager("api/src/main.rs"),
                lazy("api/tmp"),
                eager("web"),
                eager("web/dist"),
            ]
        );
        let opened: HashSet<String> = ["api/dist".to_string()].into();
        assert_eq!(
            lazy_paths(&root, &api.join("dist"), &opened),
            [eager("api/dist/app.js"), lazy("api/dist/assets")]
        );
        let patch = patch_lazily(&root, &["api/tmp".into(), "api/dist".into()], &opened);
        let mut upserted: Vec<(&str, bool)> = patch
            .upserted
            .iter()
            .map(|entry| (entry.path.as_str(), entry.lazy))
            .collect();
        upserted.sort();
        assert_eq!(
            upserted,
            [
                ("api/dist", false),
                ("api/dist/app.js", false),
                ("api/dist/assets", true),
                ("api/tmp", true)
            ]
        );
        Ok(())
    }

    #[test]
    fn local_fs_reads_what_the_disk_has() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(temp.path().join("a.txt"), "hello")?;
        let fs = LocalFs::new(temp.path().to_path_buf());
        let read = fs.read("a.txt")?;
        assert_eq!(read.text.as_deref(), Some("hello"));
        assert_eq!(read.size, 5);
        assert_eq!(fs.stat("a.txt")?.mtime_ns, read.mtime_ns);
        assert!(fs.read_bytes("a.txt", 4).is_err());
        assert_eq!(fs.read_bytes("a.txt", 5)?, b"hello");
        assert!(fs.read("../a.txt").is_err());
        Ok(())
    }
}
