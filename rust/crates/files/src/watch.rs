use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

use crate::SKIP_DIRS;

/// Saves and checkouts land as bursts of events; the tree takes them in one batch once they settle.
const LATENCY: Duration = Duration::from_millis(100);

/// inotify needs one watch per folder, so on Linux folders are watched one by one and the ones in
/// `SKIP_DIRS` never are; FSEvents watches a whole tree with one stream.
const PER_FOLDER: bool = cfg!(target_os = "linux");

/// Watches a tree and collects the relative paths that changed; `wake` runs once per settled batch.
pub struct TreeWatcher {
    watcher: Arc<Mutex<notify::RecommendedWatcher>>,
    pending: Arc<Mutex<HashSet<String>>>,
}

impl TreeWatcher {
    /// Watches every folder of `root` but the skipped ones.
    pub fn new(root: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> notify::Result<TreeWatcher> {
        let watcher = Self::start(root, wake)?;
        if PER_FOLDER {
            watch_tree(&watcher.watcher, root, root);
        }
        Ok(watcher)
    }

    /// Watches only `root` itself on Linux; the caller adds each folder it lists with [`Self::watch_folder`].
    pub fn following(
        root: &Path,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> notify::Result<TreeWatcher> {
        let watcher = Self::start(root, wake)?;
        watcher.watch_folder(root);
        Ok(watcher)
    }

    fn start(root: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> notify::Result<TreeWatcher> {
        let (sender, receiver) = std::sync::mpsc::channel::<Vec<PathBuf>>();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) if !event.kind.is_access() => {
                    if sender.send(event.paths).is_err() {
                        eprintln!("file tree watcher stopped");
                    }
                }
                Ok(_) => {}
                Err(error) => eprintln!("file tree watcher: {error}"),
            })?;
        if !PER_FOLDER {
            watcher.watch(root, RecursiveMode::Recursive)?;
        }
        let watcher = Arc::new(Mutex::new(watcher));
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let (root, collected, adder) = (root.to_path_buf(), pending.clone(), watcher.clone());
        std::thread::Builder::new()
            .name("files-watch".into())
            .spawn(move || {
                let added = |path: &Path| {
                    if PER_FOLDER && path.is_dir() && !path.is_symlink() {
                        watch_tree(&adder, &root, path);
                    }
                };
                collect(&root, &receiver, &collected, &*wake, &added)
            })
            .map_err(|error| notify::Error::generic(&error.to_string()))?;
        Ok(TreeWatcher { watcher, pending })
    }

    /// Watches the folder at `path` alone (not what is inside it); a no-op where one watch covers the tree.
    pub fn watch_folder(&self, path: &Path) {
        if !PER_FOLDER {
            return;
        }
        if let Ok(mut watcher) = self.watcher.lock() {
            if let Err(error) = watcher.watch(path, RecursiveMode::NonRecursive) {
                eprintln!("file tree watcher: {}: {error}", path.display());
            }
        }
    }

    pub fn take_changes(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|mut pending| pending.drain().collect())
            .unwrap_or_default()
    }
}

fn skipped(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|rel| {
        rel.components()
            .any(|part| SKIP_DIRS.contains(&part.as_os_str().to_string_lossy().as_ref()))
    })
}

/// Watches `from` and every folder under it, leaving out skipped folders and links.
fn watch_tree(watcher: &Mutex<notify::RecommendedWatcher>, root: &Path, from: &Path) {
    if skipped(root, from) {
        return;
    }
    let Ok(mut watcher) = watcher.lock() else {
        return;
    };
    let mut queue = std::collections::VecDeque::from([from.to_path_buf()]);
    while let Some(folder) = queue.pop_front() {
        if let Err(error) = watcher.watch(&folder, RecursiveMode::NonRecursive) {
            eprintln!("file tree watcher: {}: {error}", folder.display());
            if matches!(error.kind, notify::ErrorKind::MaxFilesWatch) {
                return;
            }
            continue;
        }
        let Ok(children) = std::fs::read_dir(&folder) else {
            continue;
        };
        for child in children.flatten() {
            let is_folder = child.file_type().is_ok_and(|kind| kind.is_dir());
            let name = child.file_name();
            if is_folder && !SKIP_DIRS.contains(&name.to_string_lossy().as_ref()) {
                queue.push_back(child.path());
            }
        }
    }
}

fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty()
        || parts.first().is_some_and(|first| first == ".git")
        || parts.iter().any(|part| part == ".git")
    {
        return None;
    }
    Some(parts.join("/"))
}

fn collect(
    root: &Path,
    receiver: &Receiver<Vec<PathBuf>>,
    pending: &Mutex<HashSet<String>>,
    wake: &(dyn Fn() + Send + Sync),
    added: &dyn Fn(&Path),
) {
    // FSEvents reports the real path; a root reached through a symlink (/tmp) must be compared in that form.
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut batch: Vec<String> = Vec::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let wait = deadline.map_or(Duration::from_secs(3600), |at| {
            at.saturating_duration_since(Instant::now())
        });
        match receiver.recv_timeout(wait) {
            Ok(paths) => {
                for path in &paths {
                    added(path);
                }
                batch.extend(paths.iter().filter_map(|path| {
                    relative(root, path).or_else(|| relative(&canonical, path))
                }));
                deadline.get_or_insert_with(|| Instant::now() + LATENCY);
            }
            Err(RecvTimeoutError::Timeout) => {
                deadline = None;
                if batch.is_empty() {
                    continue;
                }
                if let Ok(mut shared) = pending.lock() {
                    shared.extend(batch.drain(..));
                }
                batch.clear();
                wake();
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_under_git_are_ignored() {
        let root = Path::new("/work");
        assert_eq!(
            relative(root, Path::new("/work/src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(relative(root, Path::new("/work/.git/index")), None);
        assert_eq!(relative(root, Path::new("/work/api/.git/HEAD")), None);
        assert_eq!(relative(root, Path::new("/elsewhere/a")), None);
    }

    #[test]
    fn a_path_changed_many_times_is_held_once() {
        let (sender, receiver) = std::sync::mpsc::channel::<Vec<PathBuf>>();
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let collected = pending.clone();
        let collector = std::thread::spawn(move || {
            collect(Path::new("/work"), &receiver, &collected, &|| {}, &|_| {});
        });
        for round in 0..3 {
            for _ in 0..1000 {
                let sent = sender.send(vec![PathBuf::from("/work/log/development.log")]);
                assert!(sent.is_ok(), "round {round}");
            }
            std::thread::sleep(LATENCY * 3);
        }
        drop(sender);
        assert!(collector.join().is_ok());
        let held = pending.lock().map(|set| set.len()).unwrap_or_default();
        assert_eq!(held, 1);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    fn a_new_file_is_reported_once_changes_settle() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let watcher =
            TreeWatcher::new(temp.path(), Arc::new(|| {})).map_err(std::io::Error::other)?;
        std::thread::sleep(Duration::from_millis(300));
        std::fs::create_dir_all(temp.path().join("src"))?;
        std::fs::write(temp.path().join("src/new.rs"), "x")?;
        // inotify watches a new folder only once it exists, so a file made right away may come as its folder.
        let reported =
            |path: &String| path == "src/new.rs" || (cfg!(target_os = "linux") && path == "src");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while Instant::now() < deadline && !seen.iter().any(reported) {
            seen.extend(watcher.take_changes());
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(seen.iter().any(reported), "{seen:?}");
        Ok(())
    }

    fn changes_within(watcher: &TreeWatcher, wait: Duration) -> Vec<String> {
        let deadline = Instant::now() + wait;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            seen.extend(watcher.take_changes());
            std::thread::sleep(Duration::from_millis(50));
        }
        seen
    }

    #[test]
    fn a_folder_made_later_is_watched_too() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let watcher =
            TreeWatcher::new(temp.path(), Arc::new(|| {})).map_err(std::io::Error::other)?;
        std::fs::create_dir_all(temp.path().join("src/deep"))?;
        changes_within(&watcher, Duration::from_millis(500));
        std::fs::write(temp.path().join("src/deep/late.rs"), "x")?;
        let seen = changes_within(&watcher, Duration::from_secs(2));
        assert!(
            seen.iter().any(|path| path == "src/deep/late.rs"),
            "{seen:?}"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn skipped_folders_take_no_watches() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir_all(temp.path().join("node_modules/pkg"))?;
        let watcher =
            TreeWatcher::new(temp.path(), Arc::new(|| {})).map_err(std::io::Error::other)?;
        std::fs::write(temp.path().join("node_modules/pkg/index.js"), "x")?;
        let seen = changes_within(&watcher, Duration::from_millis(800));
        assert!(
            seen.iter()
                .all(|path| !path.starts_with("node_modules/pkg/")),
            "{seen:?}"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_following_watch_sees_only_the_folders_it_was_given() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir_all(temp.path().join("src"))?;
        std::fs::create_dir_all(temp.path().join("dist"))?;
        let watcher =
            TreeWatcher::following(temp.path(), Arc::new(|| {})).map_err(std::io::Error::other)?;
        watcher.watch_folder(&temp.path().join("src"));
        std::fs::write(temp.path().join("src/a.rs"), "x")?;
        std::fs::write(temp.path().join("dist/b.js"), "x")?;
        let seen = changes_within(&watcher, Duration::from_millis(800));
        assert!(seen.iter().any(|path| path == "src/a.rs"), "{seen:?}");
        assert!(seen.iter().all(|path| path != "dist/b.js"), "{seen:?}");
        Ok(())
    }
}
