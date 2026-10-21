use std::collections::HashSet;
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
    pending: Arc<Mutex<HashSet<String>>>,
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
        let pending = Arc::new(Mutex::new(HashSet::new()));
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

    pub fn take_changes(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|mut pending| pending.drain().collect())
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
    pending: &Mutex<HashSet<String>>,
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
            collect(Path::new("/work"), &receiver, &collected, &|| {});
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
}
