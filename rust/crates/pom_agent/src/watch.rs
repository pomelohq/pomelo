use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use notify::{RecursiveMode, Watcher};
use pom_paths::StateDir;

/// Raises a flag and calls `wake` whenever an agent reports a new state; the owner polls
/// `take_changed` and re-reads the states.
pub struct AgentWatcher {
    _watcher: notify::RecommendedWatcher,
    changed: Arc<AtomicBool>,
}

impl AgentWatcher {
    pub fn new(
        state: &StateDir,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> notify::Result<AgentWatcher> {
        let dir = state.path("agents");
        std::fs::create_dir_all(&dir)
            .map_err(|error| notify::Error::generic(&error.to_string()))?;
        let changed = Arc::new(AtomicBool::new(true));
        let flag = changed.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    flag.store(true, Ordering::Release);
                    wake();
                }
            })?;
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;
        Ok(AgentWatcher {
            _watcher: watcher,
            changed,
        })
    }

    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_new_state_file_raises_the_flag() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let watcher = AgentWatcher::new(&state, Arc::new(|| {})).expect("watch");
        assert!(
            watcher.take_changed(),
            "starts changed so the first read happens"
        );
        assert!(!watcher.take_changed());
        std::thread::sleep(Duration::from_millis(100));
        crate::record_hook(
            &state,
            br#"{"cwd":"/p/workspace--main","hook_event_name":"UserPromptSubmit"}"#,
        )
        .expect("hook");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !watcher.take_changed() {
            assert!(Instant::now() < deadline, "no change seen");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
