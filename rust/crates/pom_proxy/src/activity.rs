//! When each workspace last got a request through the proxy, kept in `proxy/activity.json` for whatever puts idle
//! workspaces to sleep. A page fires hundreds of requests, so the file is written at most every `WRITE_EVERY`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const ACTIVITY_SCHEMA: &str = "pom.proxy_activity/v1";
const WRITE_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub root: PathBuf,
    pub branch: String,
    pub last_request_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityFile {
    pub schema: String,
    pub workspaces: Vec<ActivityEntry>,
}

pub fn read_activity(path: &Path) -> Vec<ActivityEntry> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ActivityFile>(&bytes).ok())
        .map(|file| file.workspaces)
        .unwrap_or_default()
}

struct Seen {
    last: HashMap<(PathBuf, String), u64>,
    written: Option<Instant>,
}

pub(crate) struct Activity {
    path: Option<PathBuf>,
    seen: Mutex<Seen>,
}

impl Activity {
    pub(crate) fn new(path: Option<PathBuf>) -> Activity {
        let last = path
            .as_deref()
            .map(read_activity)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| ((entry.root, entry.branch), entry.last_request_ms))
            .collect();
        Activity {
            path,
            seen: Mutex::new(Seen {
                last,
                written: None,
            }),
        }
    }

    pub(crate) fn touch(&self, root: &Path, branch: &str, now_ms: u64) {
        let Some(path) = &self.path else {
            return;
        };
        let Ok(mut seen) = self.seen.lock() else {
            return;
        };
        seen.last
            .insert((root.to_path_buf(), branch.to_string()), now_ms);
        if seen
            .written
            .is_some_and(|written| written.elapsed() < WRITE_EVERY)
        {
            return;
        }
        let mut workspaces: Vec<ActivityEntry> = seen
            .last
            .iter()
            .map(|((root, branch), ms)| ActivityEntry {
                root: root.clone(),
                branch: branch.clone(),
                last_request_ms: *ms,
            })
            .collect();
        workspaces.sort_by(|a, b| (&a.root, &a.branch).cmp(&(&b.root, &b.branch)));
        let file = ActivityFile {
            schema: ACTIVITY_SCHEMA.into(),
            workspaces,
        };
        match write_json(path, &file) {
            Ok(()) => seen.written = Some(Instant::now()),
            Err(error) => eprintln!("dev proxy: {}: {error}", path.display()),
        }
    }
}

pub(crate) fn write_json<T: Serialize>(path: &Path, file: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staged = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&staged, serde_json::to_vec_pretty(file)?)?;
    std::fs::rename(&staged, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_written_at_most_every_little_while() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("proxy/activity.json");
        let activity = Activity::new(Some(path.clone()));
        activity.touch(Path::new("/p"), "feat", 1_000);
        activity.touch(Path::new("/p"), "feat", 2_000);
        activity.touch(Path::new("/p"), "main", 3_000);
        assert_eq!(
            read_activity(&path),
            [ActivityEntry {
                root: "/p".into(),
                branch: "feat".into(),
                last_request_ms: 1_000,
            }],
            "the burst after the first write waits"
        );
        let reopened = Activity::new(Some(path.clone()));
        reopened.touch(Path::new("/p"), "main", 4_000);
        let entries = read_activity(&path);
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert_eq!(entries[1].last_request_ms, 4_000);
    }
}
