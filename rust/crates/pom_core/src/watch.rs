use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use pom_config::FRAGMENT_DIR;

/// Editors save in bursts (write temp, rename, touch); one reload per burst is enough.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Watches a project's `pom.yml` and `pom.d/` and raises a flag, then calls `wake`, once a burst
/// of changes settles. The owner polls `take_changed` on its own thread and reloads.
pub struct ConfigWatcher {
    _watcher: notify::RecommendedWatcher,
    changed: Arc<AtomicBool>,
}

impl ConfigWatcher {
    pub fn new(root: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> notify::Result<ConfigWatcher> {
        let changed = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        let config_path = root.join(pom_config::CONFIG_FILE_NAME);
        let fragments = root.join(FRAGMENT_DIR);
        let relevant = {
            let config_path = config_path.clone();
            let fragments = fragments.clone();
            move |path: &PathBuf| *path == config_path || path.starts_with(&fragments)
        };
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                if event.paths.iter().any(&relevant) {
                    if let Err(error) = sender.send(()) {
                        eprintln!("config watcher stopped: {error}");
                    }
                }
            })?;
        // The root is watched non-recursively: worktrees under it are large and irrelevant here.
        watcher.watch(root, RecursiveMode::NonRecursive)?;
        if fragments.is_dir() {
            watcher.watch(&fragments, RecursiveMode::Recursive)?;
        }
        let flag = changed.clone();
        std::thread::Builder::new()
            .name("pom-config-watch".into())
            .spawn(move || debounce(receiver, flag, wake))
            .map_err(|error| notify::Error::generic(&error.to_string()))?;
        Ok(ConfigWatcher {
            _watcher: watcher,
            changed,
        })
    }

    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

fn debounce(
    receiver: std::sync::mpsc::Receiver<()>,
    changed: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    while receiver.recv().is_ok() {
        let mut quiet_at = Instant::now() + DEBOUNCE;
        loop {
            let timeout = quiet_at.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(timeout) {
                Ok(()) => quiet_at = Instant::now() + DEBOUNCE,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        changed.store(true, Ordering::Release);
        wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn wait_for(condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn burst_of_edits_wakes_once_and_ignores_other_files() -> notify::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        std::fs::write(root.join("pom.yml"), "session: a\n")?;
        std::fs::create_dir_all(root.join("pom.d"))?;
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let watcher = ConfigWatcher::new(
            &root,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        )?;
        // FSEvents may replay the setup writes; let that settle before measuring.
        std::thread::sleep(Duration::from_millis(400));
        watcher.take_changed();
        wakes.store(0, Ordering::SeqCst);

        std::fs::write(root.join("notes.txt"), "x")?;
        std::thread::sleep(Duration::from_millis(400));
        assert!(!watcher.take_changed());

        for index in 0..5 {
            std::fs::write(root.join("pom.yml"), format!("session: a{index}\n"))?;
        }
        assert!(wait_for(|| wakes.load(Ordering::SeqCst) >= 1));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert!(watcher.take_changed());
        assert!(!watcher.take_changed());

        std::fs::write(root.join("pom.d/10-web.yml"), "repos: {}\n")?;
        assert!(wait_for(|| watcher.take_changed()));
        Ok(())
    }
}
