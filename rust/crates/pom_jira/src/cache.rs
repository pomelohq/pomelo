//! Ticket status by key, kept on disk (`cache/jira-<session>.json`, the previous core's version-2 layout) so the
//! WORKSPACES rows show a status right away; entries older than a minute are fetched again.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

use crate::{Client, Issue, IssueDetail};

const VERSION: u32 = 2;
const FRESH_FOR: Duration = Duration::from_secs(60);

#[derive(Default, Serialize, Deserialize)]
struct OnDisk {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    issues: BTreeMap<String, Issue>,
    #[serde(default)]
    detail: BTreeMap<String, serde_json::Value>,
}

pub struct IssueCache {
    path: PathBuf,
    issues: BTreeMap<String, Issue>,
    detail: BTreeMap<String, serde_json::Value>,
    fetched: HashMap<String, Instant>,
}

impl IssueCache {
    pub fn open(state: &StateDir, session: &str) -> IssueCache {
        let path = state.path("cache").join(format!("jira-{session}.json"));
        let disk: OnDisk = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let detail = if disk.version == VERSION {
            disk.detail
        } else {
            BTreeMap::new()
        };
        IssueCache {
            path,
            issues: disk.issues,
            detail,
            fetched: HashMap::new(),
        }
    }

    pub fn issue(&self, key: &str) -> Option<&Issue> {
        self.issues.get(key).filter(|issue| !issue.key.is_empty())
    }

    /// Keys among `keys` not fetched in the last minute (everything read from disk counts as stale).
    pub fn stale(&self, keys: &[String]) -> Vec<String> {
        let now = Instant::now();
        keys.iter()
            .filter(|key| {
                self.fetched
                    .get(*key)
                    .is_none_or(|at| now.duration_since(*at) >= FRESH_FOR)
            })
            .cloned()
            .collect()
    }

    /// Fetches the stale keys' status in one search and saves the cache; returns whether anything was fetched.
    pub fn refresh(&mut self, client: &Client, keys: &[String]) -> bool {
        let stale = self.stale(keys);
        if stale.is_empty() {
            return false;
        }
        match client.search_by_keys(&stale) {
            Ok(issues) => {
                self.record(&stale, issues);
                true
            }
            Err(error) => {
                eprintln!("jira: ticket status: {error}");
                false
            }
        }
    }

    /// Takes in what a search for `asked` returned (keys it did not return stay as they were) and saves.
    pub fn record(&mut self, asked: &[String], issues: Vec<Issue>) {
        let now = Instant::now();
        for key in asked {
            self.fetched.insert(key.clone(), now);
        }
        for issue in issues {
            self.issues.insert(issue.key.clone(), issue);
        }
        self.save();
    }

    pub fn detail(&self, key: &str) -> Option<IssueDetail> {
        serde_json::from_value(self.detail.get(key)?.clone()).ok()
    }

    pub fn store_detail(&mut self, detail: &IssueDetail) {
        let mut value = serde_json::to_value(detail).unwrap_or_default();
        value["configured"] = serde_json::Value::Bool(true);
        self.detail.insert(detail.key.clone(), value);
        self.save();
    }

    fn save(&self) {
        let disk = OnDisk {
            version: VERSION,
            issues: self.issues.clone(),
            detail: self.detail.clone(),
        };
        let written = serde_json::to_vec(&disk)
            .map_err(std::io::Error::other)
            .and_then(|bytes| pom_paths::write_atomic(&self.path, &bytes, 0o644));
        if let Err(error) = written {
            eprintln!("jira: save the ticket cache: {error}");
        }
    }
}
