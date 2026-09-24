use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

/// Saves and checkouts land as bursts of events; the tree takes them in one batch once they settle.
const LATENCY: Duration = Duration::from_millis(100);

/// Watches a tree and collects the relative paths that changed; `wake` runs once per settled batch.
pub struct TreeWatcher {
    _watcher: notify::RecommendedWatcher,
    pending: Arc<Mutex<Vec<String>>>,
}

impl TreeWatcher {
    pub fn new(root: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> notify::Result<TreeWatcher> {
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
        watcher.watch(root, RecursiveMode::Recursive)?;
        let pending = Arc::new(Mutex::new(Vec::new()));
        let (root, collected) = (root.to_path_buf(), pending.clone());
        std::thread::Builder::new()
            .name("files-watch".into())
            .spawn(move || collect(&root, &receiver, &collected, &*wake))
            .map_err(|error| notify::Error::generic(&error.to_string()))?;
        Ok(TreeWatcher {
            _watcher: watcher,
            pending,
        })
    }

    /// The changed paths since the last call (relative, forward-slash), unsorted and possibly repeated.
    pub fn take_changes(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
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
    pending: &Mutex<Vec<String>>,
    wake: &(dyn Fn() + Send + Sync),
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
                    shared.append(&mut batch);
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
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while Instant::now() < deadline && !seen.iter().any(|path: &String| path == "src/new.rs") {
            seen.extend(watcher.take_changes());
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(seen.iter().any(|path| path == "src/new.rs"), "{seen:?}");
        Ok(())
    }
}
