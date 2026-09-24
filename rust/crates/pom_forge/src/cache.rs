use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pom_paths::StateDir;

use crate::pull_request::{fetch_heads, PrTarget, PullRequest};
use crate::{Client, COOLDOWN};

const FRESH_FOR: Duration = Duration::from_secs(150);

struct Entry {
    pr: Option<PullRequest>,
    /// `None` for entries read from disk: shown at once, refreshed on the next warm.
    fetched: Option<Instant>,
}

/// Each branch's PR as last seen, kept across restarts in the file the previous core used.
pub struct PrCache {
    path: PathBuf,
    entries: HashMap<String, Entry>,
    retry_at: Option<Instant>,
}

impl PrCache {
    pub fn open(state: &StateDir) -> PrCache {
        let path = state.path("cache/pr.json");
        let saved: HashMap<String, PullRequest> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        PrCache {
            path,
            entries: saved
                .into_iter()
                .map(|(key, pr)| {
                    (
                        key,
                        Entry {
                            pr: Some(pr),
                            fetched: None,
                        },
                    )
                })
                .collect(),
            retry_at: None,
        }
    }

    /// `None` when the branch was never looked up; `Some(None)` when it has no PR.
    pub fn get(&self, target: &PrTarget) -> Option<Option<&PullRequest>> {
        self.entries
            .get(&target.key())
            .map(|entry| entry.pr.as_ref())
    }

    pub fn stale(&self, targets: &[PrTarget]) -> Vec<PrTarget> {
        let mut seen = std::collections::HashSet::new();
        targets
            .iter()
            .filter(|target| seen.insert(target.key()))
            .filter(|target| {
                self.entries
                    .get(&target.key())
                    .and_then(|entry| entry.fetched)
                    .is_none_or(|fetched| fetched.elapsed() >= FRESH_FOR)
            })
            .cloned()
            .collect()
    }

    pub fn invalidate(&mut self) {
        for entry in self.entries.values_mut() {
            entry.fetched = None;
        }
        self.retry_at = None;
    }

    /// Looks up the stale targets; returns whether any PR changed. After a failure nothing is asked for 30s.
    pub fn warm(
        &mut self,
        client: &Client,
        targets: &[PrTarget],
    ) -> Result<bool, crate::ForgeError> {
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return Ok(false);
        }
        let stale = self.stale(targets);
        if stale.is_empty() {
            return Ok(false);
        }
        let (found, failure) = fetch_heads(client, &stale);
        let changed = self.record(found);
        if changed {
            self.save();
        }
        match failure {
            Some(error) => {
                self.retry_at = Some(Instant::now() + COOLDOWN);
                Err(error)
            }
            None => Ok(changed),
        }
    }

    fn record(&mut self, found: HashMap<String, Option<PullRequest>>) -> bool {
        let now = Instant::now();
        let mut changed = false;
        for (key, pr) in found {
            let previous = self.entries.insert(
                key,
                Entry {
                    pr: pr.clone(),
                    fetched: Some(now),
                },
            );
            changed |= previous.is_none_or(|previous| previous.pr != pr);
        }
        changed
    }

    fn save(&self) {
        let saved: HashMap<&String, &PullRequest> = self
            .entries
            .iter()
            .filter_map(|(key, entry)| entry.pr.as_ref().map(|pr| (key, pr)))
            .collect();
        let written = serde_json::to_vec(&saved)
            .map_err(std::io::Error::other)
            .and_then(|bytes| pom_paths::write_atomic(&self.path, &bytes, 0o644));
        if let Err(error) = written {
            eprintln!("github: save the PR cache: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{ok, Scripted};
    use crate::Response;

    fn target(head: &str) -> PrTarget {
        PrTarget {
            repo: "web".into(),
            owner: "acme".into(),
            name: "web".into(),
            head: head.into(),
        }
    }

    fn answer(number: u64) -> Response {
        ok(
            &serde_json::json!({ "data": { "p0": { "pullRequests": { "nodes": [
            { "number": number, "title": "Add login", "state": "OPEN" }
        ] } } } })
            .to_string(),
        )
    }

    #[test]
    fn warm_asks_only_for_stale_branches_and_survives_a_restart() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let state = StateDir::new(temp.path());
        let transport = Scripted::default();
        if let Ok(mut answers) = transport.answers.lock() {
            answers.push(answer(7));
        }
        let client = Client::new("t", Box::new(transport.clone()));
        let mut cache = PrCache::open(&state);
        assert!(cache.get(&target("feat")).is_none());
        assert_eq!(cache.warm(&client, &[target("feat")]), Ok(true));
        assert_eq!(
            cache.warm(&client, &[target("feat")]),
            Ok(false),
            "fresh: nothing asked"
        );
        assert_eq!(transport.sent.lock().map(|sent| sent.len()).unwrap_or(0), 1);

        let reopened = PrCache::open(&state);
        assert_eq!(
            reopened.get(&target("feat")).flatten().map(|pr| pr.number),
            Some(7)
        );
        assert_eq!(
            reopened.stale(&[target("feat")]).len(),
            1,
            "disk entries refresh on the next warm"
        );
        Ok(())
    }

    #[test]
    fn a_failure_holds_further_lookups_back() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let transport = Scripted::default();
        if let Ok(mut answers) = transport.answers.lock() {
            answers.push(Response {
                status: 502,
                ..Response::default()
            });
            answers.push(answer(1));
        }
        let client = Client::new("t", Box::new(transport.clone()));
        let mut cache = PrCache::open(&StateDir::new(temp.path()));
        assert!(cache.warm(&client, &[target("feat")]).is_err());
        assert_eq!(cache.warm(&client, &[target("feat")]), Ok(false));
        cache.invalidate();
        assert_eq!(cache.warm(&client, &[target("feat")]), Ok(true));
        Ok(())
    }
}
