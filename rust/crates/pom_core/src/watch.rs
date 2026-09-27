use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

/// Editors save in bursts (write temp, rename, touch); one reload per burst is enough.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// Watches a project's `pom.yml` and its `workspace--*` folders and raises a flag per kind, then calls
/// `wake`, once a burst of changes settles. The owner polls `take_changed` / `take_workspaces_changed`.
pub struct ConfigWatcher {
    _watcher: notify::RecommendedWatcher,
    changed: Arc<AtomicBool>,
    workspaces_changed: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
enum Change {
    Config,
    Workspaces,
}

impl ConfigWatcher {
    pub fn new(root: &Path, wake: Arc<dyn Fn() + Send + Sync>) -> notify::Result<ConfigWatcher> {
        let changed = Arc::new(AtomicBool::new(false));
        let workspaces_changed = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::channel::<Change>();
        let config_path = root.join(pom_config::CONFIG_FILE_NAME);
        let project_root = root.to_path_buf();
        // Folders removed or made outside the app (the CLI, a finder) must reach the workspace list too.
        let classify = move |path: &PathBuf| {
            if *path == config_path {
                return Some(Change::Config);
            }
            let is_workspace = path.parent() == Some(project_root.as_path())
                && path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with(pom_layout::WORKSPACE_PREFIX)
                });
            is_workspace.then_some(Change::Workspaces)
        };
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                for change in event.paths.iter().filter_map(&classify) {
                    if let Err(error) = sender.send(change) {
                        eprintln!("config watcher stopped: {error}");
                        return;
                    }
                }
            })?;
        // The root is watched non-recursively: worktrees under it are large and irrelevant here.
        watcher.watch(root, RecursiveMode::NonRecursive)?;
        let flags = (changed.clone(), workspaces_changed.clone());
        std::thread::Builder::new()
            .name("pom-config-watch".into())
            .spawn(move || debounce(receiver, flags, wake))
            .map_err(|error| notify::Error::generic(&error.to_string()))?;
        Ok(ConfigWatcher {
            _watcher: watcher,
            changed,
            workspaces_changed,
        })
    }

    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    pub fn take_workspaces_changed(&self) -> bool {
        self.workspaces_changed.swap(false, Ordering::AcqRel)
    }
}

fn debounce(
    receiver: std::sync::mpsc::Receiver<Change>,
    (config, workspaces): (Arc<AtomicBool>, Arc<AtomicBool>),
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let mark = |change: Change| match change {
        Change::Config => config.store(true, Ordering::Release),
        Change::Workspaces => workspaces.store(true, Ordering::Release),
    };
    while let Ok(first) = receiver.recv() {
        mark(first);
        let mut quiet_at = Instant::now() + DEBOUNCE;
        loop {
            let timeout = quiet_at.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(timeout) {
                Ok(change) => {
                    mark(change);
                    quiet_at = Instant::now() + DEBOUNCE;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
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

        std::fs::create_dir(root.join("workspace--feat-x"))?;
        assert!(wait_for(|| watcher.take_workspaces_changed()));
        assert!(
            !watcher.take_changed(),
            "a new workspace is not a config edit"
        );
        std::fs::remove_dir(root.join("workspace--feat-x"))?;
        assert!(wait_for(|| watcher.take_workspaces_changed()));
        Ok(())
    }
}
