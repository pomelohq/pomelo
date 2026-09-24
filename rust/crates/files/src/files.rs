//! Files feature — pure logic ported from the Go core's file endpoints (`internal/core/files.go`): list a
//! workspace's files as a flat entry list, build the nested tree the UI shows, and read a file's contents with
//! text/binary detection. No rendering — the `files_ui` crate draws this.

use std::path::{Path, PathBuf};

/// Directory names never descended into (the Go walk's `skipDirNames` plus `target`: the walk is eager and
/// recursive, and a Rust `target/` has enough files to stall the synchronous rebuild on window open).
const SKIP_DIRS: &[&str] = &[".git", ".pom", "node_modules", ".ddata", "target"];

/// One file or directory, mirroring the Go `FileEntry`. `path` is forward-slash-relative to the workspace root
/// (the Go model also carries a `repo`; a single-root workspace uses `""`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub repo: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

/// A node in the tree the UI renders: a name plus children (empty for files). `path` is the full
/// forward-slash-relative path (the node id in the Swift tree is `repo/path`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Vec<FileNode>,
}

/// Walk `root` recursively and return a flat, sorted entry list (directories included), skipping `SKIP_DIRS`.
/// Faithful to the Go `WalkDir` listing: no hidden-file or gitignore filtering, sorted by path.
pub fn list(root: &Path) -> Vec<FileEntry> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<FileEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = meta.is_dir();
        if is_dir && SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        let full = entry.path();
        let Some(rel) = rel_path(root, &full) else {
            continue;
        };
        out.push(FileEntry {
            repo: String::new(),
            path: rel,
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
        });
        if is_dir {
            walk(root, &full, out);
        }
    }
}

/// How often a running scan hands the UI what it has found so far.
const SCAN_PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// What a background scan has found: the entries so far (sorted like `list`) and whether it is still going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanUpdate {
    pub entries: Vec<FileEntry>,
    pub scanning: bool,
}

#[derive(Default)]
struct ScanState {
    queue: std::collections::VecDeque<PathBuf>,
    active: usize,
    entries: Vec<FileEntry>,
    changed: bool,
    cancelled: bool,
}

impl ScanState {
    fn done(&self) -> bool {
        self.cancelled || (self.queue.is_empty() && self.active == 0)
    }
}

type SharedScan = std::sync::Arc<(std::sync::Mutex<ScanState>, std::sync::Condvar)>;

/// Walks a tree off the UI thread, one worker per CPU pulling directories from a shared queue, and posts
/// progress every `SCAN_PROGRESS_INTERVAL` until done, so a large workspace never blocks a frame. Dropping it
/// stops the walk.
pub struct BackgroundScan {
    shared: SharedScan,
    updates: std::sync::mpsc::Receiver<ScanUpdate>,
}

impl BackgroundScan {
    pub fn start(root: PathBuf, wake: std::sync::Arc<dyn Fn() + Send + Sync>) -> BackgroundScan {
        let shared: SharedScan = std::sync::Arc::new(Default::default());
        if let Ok(mut state) = shared.0.lock() {
            state.queue.push_back(root.clone());
        }
        let workers = std::thread::available_parallelism().map_or(4, |count| count.get());
        for index in 0..workers {
            let (shared, root) = (shared.clone(), root.clone());
            let spawned = std::thread::Builder::new()
                .name(format!("files-scan-{index}"))
                .spawn(move || scan_worker(&root, &shared));
            if let Err(error) = spawned {
                eprintln!("failed to start a file scan worker: {error}");
            }
        }
        let (sender, updates) = std::sync::mpsc::channel();
        let progress = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("files-scan-progress".into())
            .spawn(move || scan_progress(&progress, &sender, &*wake));
        if let Err(error) = spawned {
            eprintln!("failed to start the file scan: {error}");
        }
        BackgroundScan { shared, updates }
    }

    /// The newest update since the last call; earlier ones are superseded and dropped.
    pub fn latest(&self) -> Option<ScanUpdate> {
        self.updates.try_iter().last()
    }

    /// Block until the walk finishes and return everything it found (for tests and tools).
    pub fn wait(self) -> Vec<FileEntry> {
        let mut last = Vec::new();
        for update in self.updates.iter() {
            last = update.entries;
            if !update.scanning {
                break;
            }
        }
        last
    }
}

impl Drop for BackgroundScan {
    fn drop(&mut self) {
        let (lock, wakeup) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            state.cancelled = true;
        }
        wakeup.notify_all();
    }
}

fn scan_worker(root: &Path, shared: &SharedScan) {
    let (lock, wakeup) = &**shared;
    loop {
        let dir = {
            let Ok(mut state) = lock.lock() else {
                return;
            };
            loop {
                if state.done() {
                    wakeup.notify_all();
                    return;
                }
                if let Some(dir) = state.queue.pop_front() {
                    state.active += 1;
                    break dir;
                }
                state = match wakeup.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }
        };
        let mut found = Vec::new();
        let mut subdirs = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let is_dir = meta.is_dir();
                if is_dir && SKIP_DIRS.contains(&name.to_string_lossy().as_ref()) {
                    continue;
                }
                let full = entry.path();
                let Some(path) = rel_path(root, &full) else {
                    continue;
                };
                found.push(FileEntry {
                    repo: String::new(),
                    path,
                    is_dir,
                    size: if is_dir { 0 } else { meta.len() },
                });
                if is_dir {
                    subdirs.push(full);
                }
            }
        }
        let Ok(mut state) = lock.lock() else {
            return;
        };
        state.entries.extend(found);
        state.queue.extend(subdirs);
        state.active -= 1;
        state.changed = true;
        wakeup.notify_all();
    }
}

fn scan_progress(
    shared: &SharedScan,
    sender: &std::sync::mpsc::Sender<ScanUpdate>,
    wake: &(dyn Fn() + Send + Sync),
) {
    let (lock, wakeup) = &**shared;
    loop {
        let (entries, scanning) = {
            let Ok(state) = lock.lock() else {
                return;
            };
            let Ok((mut state, _)) =
                wakeup.wait_timeout_while(state, SCAN_PROGRESS_INTERVAL, |state| !state.done())
            else {
                return;
            };
            if state.cancelled {
                return;
            }
            let scanning = !state.done();
            if scanning && !state.changed {
                continue;
            }
            state.changed = false;
            (state.entries.clone(), scanning)
        };
        let mut entries = entries;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        if sender.send(ScanUpdate { entries, scanning }).is_err() {
            return;
        }
        wake();
        if !scanning {
            return;
        }
    }
}

/// The forward-slash path of `full` relative to `root` (`None` if `full` is not under `root`).
fn rel_path(root: &Path, full: &Path) -> Option<String> {
    let rel = full.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for comp in rel.components() {
        parts.push(comp.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Build the nested tree from a flat entry list, mirroring the Swift `WFileTreeBuilder`: split each path on
/// `/`, create intermediate directory nodes, and sort each level directories-first then by name.
pub fn build_tree(entries: &[FileEntry]) -> Vec<FileNode> {
    let mut roots: Vec<FileNode> = Vec::new();
    for e in entries {
        insert(&mut roots, "", &e.path, e.is_dir);
    }
    sort_level(&mut roots);
    roots
}

fn insert(level: &mut Vec<FileNode>, prefix: &str, rel: &str, is_dir_leaf: bool) {
    let (head, tail) = match rel.split_once('/') {
        Some((h, t)) => (h, Some(t)),
        None => (rel, None),
    };
    if head.is_empty() {
        return;
    }
    let path = if prefix.is_empty() {
        head.to_string()
    } else {
        format!("{prefix}/{head}")
    };
    let idx = match level.iter().position(|n| n.name == head) {
        Some(i) => i,
        None => {
            level.push(FileNode {
                name: head.to_string(),
                path: path.clone(),
                // A node with children (or a non-leaf segment) is a directory.
                is_dir: tail.is_some() || is_dir_leaf,
                children: Vec::new(),
            });
            level.len() - 1
        }
    };
    if let Some(rest) = tail {
        level[idx].is_dir = true;
        insert(&mut level[idx].children, &path, rest, is_dir_leaf);
    }
}

fn sort_level(level: &mut [FileNode]) {
    level.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir) // directories first
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    for node in level.iter_mut() {
        sort_level(&mut node.children);
    }
}

/// A file's contents, mirroring the Go `ReadFile` response: decoded `text` for text files, or `binary` set for
/// non-text (images/blobs the UI can't show as text yet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileContent {
    pub text: Option<String>,
    pub binary: bool,
}

/// Bytes scanned for a NUL when classifying a file as binary (matches the Go head-read threshold).
const BINARY_SNIFF: usize = 8192;

/// Read `root/rel` and classify it as text or binary. A NUL byte in the first `BINARY_SNIFF` bytes marks binary
/// (the Go core's heuristic); otherwise the bytes are decoded lossily as UTF-8.
pub fn read(root: &Path, rel: &str) -> std::io::Result<FileContent> {
    let full = safe_join(root, rel)?;
    let bytes = std::fs::read(&full)?;
    let binary = bytes.iter().take(BINARY_SNIFF).any(|b| *b == 0);
    Ok(FileContent {
        text: if binary {
            None
        } else {
            Some(String::from_utf8_lossy(&bytes).into_owned())
        },
        binary,
    })
}

/// Write `text` to `root/rel`, creating missing parent directories, and return the file's new mtime.
pub fn write(root: &Path, rel: &str, text: &str) -> std::io::Result<std::time::SystemTime> {
    let full = safe_join(root, rel)?;
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&full, text)?;
    std::fs::metadata(&full)?.modified()
}

/// The last-modified time of `root/rel`.
pub fn mtime(root: &Path, rel: &str) -> std::io::Result<std::time::SystemTime> {
    std::fs::metadata(safe_join(root, rel)?)?.modified()
}

/// Join `rel` onto `root`, rejecting `..` traversal so a path can never escape the workspace root.
fn safe_join(root: &Path, rel: &str) -> std::io::Result<PathBuf> {
    if rel.split('/').any(|c| c == ".." || c == ".") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path traversal rejected",
        ));
    }
    Ok(root.join(rel))
}

pub fn create_file(root: &Path, rel: &str) -> std::io::Result<()> {
    let full = safe_join(root, rel)?;
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(full)
        .map(|_| ())
}

pub fn create_dir(root: &Path, rel: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(safe_join(root, rel)?)
}

pub fn rename(root: &Path, from: &str, to: &str) -> std::io::Result<()> {
    let target = safe_join(root, to)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(safe_join(root, from)?, target)
}

pub fn copy_recursive(root: &Path, from: &str, to: &str) -> std::io::Result<()> {
    fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
        if from.is_dir() {
            std::fs::create_dir_all(to)?;
            for entry in std::fs::read_dir(from)? {
                let entry = entry?;
                copy(&entry.path(), &to.join(entry.file_name()))?;
            }
            Ok(())
        } else {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(from, to).map(|_| ())
        }
    }
    copy(&safe_join(root, from)?, &safe_join(root, to)?)
}

pub fn remove(root: &Path, rel: &str) -> std::io::Result<()> {
    let full = safe_join(root, rel)?;
    if full.is_dir() {
        std::fs::remove_dir_all(full)
    } else {
        std::fs::remove_file(full)
    }
}

pub fn trash(root: &Path, rel: &str) -> std::io::Result<()> {
    trash::delete(safe_join(root, rel)?).map_err(std::io::Error::other)
}

pub fn exists(root: &Path, rel: &str) -> bool {
    safe_join(root, rel).is_ok_and(|full| full.symlink_metadata().is_ok())
}

pub fn append_to_gitignore(root: &Path, pattern: &str) -> std::io::Result<()> {
    let path = root.join(".gitignore");
    let mut contents = std::fs::read_to_string(&path).unwrap_or_default();
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    contents.push_str(pattern);
    contents.push('\n');
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("pom-files-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();
        std::fs::write(base.join("Cargo.toml"), b"[package]").unwrap();
        std::fs::write(base.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(base.join(".git/HEAD"), b"ref: x").unwrap();
        std::fs::write(base.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        base
    }

    #[test]
    fn background_scan_finds_what_list_finds() {
        let root = tmp("scan");
        for index in 0..40 {
            let dir = root.join(format!("pkg{index}/src/deep"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("lib.rs"), b"").unwrap();
        }
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        let wakes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = wakes.clone();
        let scan = BackgroundScan::start(
            root.clone(),
            std::sync::Arc::new(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
        );
        assert_eq!(scan.wait(), list(&root));
        assert!(wakes.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    }

    #[test]
    fn background_scan_reports_progress_then_finishes() {
        let root = tmp("scan-progress");
        let scan = BackgroundScan::start(root.clone(), std::sync::Arc::new(|| {}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut last = None;
        while std::time::Instant::now() < deadline {
            if let Some(update) = scan.latest() {
                let finished = !update.scanning;
                last = Some(update);
                if finished {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let last = last.expect("an update arrived");
        assert!(!last.scanning);
        assert_eq!(last.entries, list(&root));
        drop(scan);
    }

    #[test]
    fn missing_root_scans_to_nothing() {
        let scan = BackgroundScan::start(
            PathBuf::from("/nonexistent/pom-scan"),
            std::sync::Arc::new(|| {}),
        );
        assert!(scan.wait().is_empty());
    }

    #[test]
    fn lists_files_skipping_git() {
        let root = tmp("list");
        let entries = list(&root);
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"Cargo.toml"));
        assert!(paths.contains(&"src"));
        assert!(paths.contains(&"src/main.rs"));
        assert!(!paths.iter().any(|p| p.starts_with(".git")));
    }

    #[test]
    fn builds_tree_dirs_first() {
        let root = tmp("tree");
        let tree = build_tree(&list(&root));
        // `src` (dir) sorts before `Cargo.toml` (file) at the top level.
        assert_eq!(tree[0].name, "src");
        assert!(tree[0].is_dir);
        assert_eq!(tree[0].children[0].path, "src/main.rs");
    }

    #[test]
    fn reads_text_and_detects_binary() {
        let root = tmp("read");
        assert_eq!(
            read(&root, "src/main.rs").unwrap().text.as_deref(),
            Some("fn main() {}")
        );
        assert!(read(&root, "blob.bin").unwrap().binary);
        assert!(read(&root, "../escape").is_err());
    }

    #[test]
    fn writes_creating_parents() {
        let root = tmp("write");
        let written = write(&root, "new/dir/a.txt", "hi").unwrap();
        assert_eq!(
            read(&root, "new/dir/a.txt").unwrap().text.as_deref(),
            Some("hi")
        );
        assert_eq!(mtime(&root, "new/dir/a.txt").unwrap(), written);
        assert!(write(&root, "../escape", "x").is_err());
    }
}
