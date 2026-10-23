use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use git::access::GitAccess;
pub(crate) use git::scan::RepoScan;

use crate::RepoSource;

#[derive(Clone, Debug, Default)]
pub(crate) struct Scan {
    pub repos: Vec<RepoScan>,
    /// `repo/path` -> fingerprint of the file as it is now, to tell whether a review still holds.
    pub fingerprints: HashMap<String, String>,
    pub loaded: bool,
}

pub(crate) fn review_key(repo: &str, path: &str) -> String {
    format!("{repo}/{path}")
}

/// The last read of each workspace (by its repo roots), so a panel rebuilt on switching back shows it at once
/// and refreshes in the background instead of starting blank.
fn last_scans() -> &'static Mutex<HashMap<Vec<PathBuf>, Arc<Scan>>> {
    static SCANS: OnceLock<Mutex<HashMap<Vec<PathBuf>, Arc<Scan>>>> = OnceLock::new();
    SCANS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn scan_key(sources: &[RepoSource]) -> Vec<PathBuf> {
    sources.iter().map(|source| source.root.clone()).collect()
}

pub(crate) fn remembered_scan(sources: &[RepoSource]) -> Arc<Scan> {
    last_scans()
        .lock()
        .ok()
        .and_then(|last| last.get(&scan_key(sources)).cloned())
        .unwrap_or_default()
}

fn fingerprints_of(sources: &[RepoSource], repos: &[RepoScan]) -> HashMap<String, String> {
    sources
        .iter()
        .zip(repos)
        .flat_map(|(source, repo)| {
            repo.fingerprints
                .iter()
                .map(|(path, print)| (review_key(&source.name, path), print.clone()))
        })
        .collect()
}

/// Reads every repo and publishes the result; wakes the UI only when something changed.
#[derive(Clone)]
pub(crate) struct Scanner {
    pub access: Arc<dyn GitAccess>,
    pub sources: Arc<Vec<RepoSource>>,
    pub scan: Arc<Mutex<Arc<Scan>>>,
    pub scanning: Arc<AtomicBool>,
    pub waker: Arc<dyn Fn() + Send + Sync>,
}

impl Scanner {
    pub fn run(&self) {
        // Repos are read side by side: one after another, a few of them keep the panel waiting over a second.
        let repos: Vec<RepoScan> = std::thread::scope(|threads| {
            let handles: Vec<_> = self
                .sources
                .iter()
                .map(|source| {
                    let access = &self.access;
                    threads.spawn(move || access.scan(source))
                })
                .collect();
            handles
                .into_iter()
                .zip(self.sources.iter())
                .map(|(handle, source)| {
                    handle
                        .join()
                        .unwrap_or_else(|_| RepoScan::failed(&source.root, "reading git failed"))
                })
                .collect()
        });
        let changed = self.publish(repos);
        self.scanning.store(false, Ordering::Release);
        if changed {
            (self.waker)();
        }
    }

    /// One repo's scan as its machine pushed it.
    pub fn absorb(&self, index: usize, repo: RepoScan) {
        let mut repos = match self.scan.lock() {
            Ok(scan) if scan.loaded => scan.repos.clone(),
            _ => return,
        };
        match repos.get_mut(index) {
            Some(slot) => *slot = repo,
            None => return,
        }
        if self.publish(repos) {
            (self.waker)();
        }
    }

    fn publish(&self, repos: Vec<RepoScan>) -> bool {
        let fingerprints = fingerprints_of(&self.sources, &repos);
        self.scan.lock().is_ok_and(|mut scan| {
            let changed = !scan.loaded || scan.repos != repos || scan.fingerprints != fingerprints;
            let next = Arc::new(Scan {
                repos,
                fingerprints,
                loaded: true,
            });
            if let Ok(mut last) = last_scans().lock() {
                last.insert(scan_key(&self.sources), next.clone());
            }
            *scan = next;
            changed
        })
    }
}
