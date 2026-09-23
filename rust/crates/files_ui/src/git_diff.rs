//! An open file's uncommitted changes, kept current off the UI thread: the git bases load once (and again after
//! a save), and the hunks recompute whenever the text changes, one computation in flight at a time.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use git::{BlameEntry, DiffBases, DiffHunk};

/// Blame reruns this long after the last edit, not on every keystroke.
const BLAME_DEBOUNCE: Duration = Duration::from_secs(2);
use ropey::Rope;

#[derive(Default)]
pub struct GitDiff {
    bases: Option<Arc<DiffBases>>,
    loading_bases: Option<Receiver<Option<DiffBases>>>,
    hunks: Vec<DiffHunk>,
    /// Buffer version `hunks` were computed for.
    hunks_version: Option<u64>,
    computing: Option<Receiver<(u64, Vec<DiffHunk>)>>,
    path: Option<PathBuf>,
    blame: Vec<BlameEntry>,
    /// Buffer version `blame` reflects, and when the buffer last moved past it.
    blame_version: Option<u64>,
    edited_at: Option<(u64, Instant)>,
    blaming: Option<Receiver<(u64, Option<Vec<BlameEntry>>)>>,
}

impl GitDiff {
    pub fn load(path: PathBuf) -> Self {
        let mut diff = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        diff.reload_bases(path);
        diff
    }

    /// Re-read HEAD and the index, e.g. after a save may have been followed by staging.
    pub fn reload_bases(&mut self, path: PathBuf) {
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            // The receiver is gone only if the file was closed; nothing to report then.
            let _ = sender.send(git::load_bases(&path));
        });
        self.loading_bases = Some(receiver);
    }

    pub fn bases(&self) -> Option<Arc<DiffBases>> {
        self.bases.clone()
    }

    /// Stage `text` for the file (or drop it from the index), then re-read the bases.
    pub fn write_index(&mut self, path: PathBuf, text: Option<String>) {
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            if let Err(error) = git::write_index(&path, text.as_deref()) {
                eprintln!("git: {error}");
            }
            // The receiver is gone only if the file was closed; nothing to report then.
            let _ = sender.send(git::load_bases(&path));
        });
        self.loading_bases = Some(receiver);
    }

    pub fn hunks(&self) -> &[DiffHunk] {
        &self.hunks
    }

    pub fn is_busy(&self) -> bool {
        self.loading_bases.is_some()
            || self.computing.is_some()
            || self.blaming.is_some()
            || self.edited_at.is_some()
    }

    pub fn blame(&self) -> &[BlameEntry] {
        &self.blame
    }

    /// Collect a finished blame, and start one when the text settled after an edit (at once the first time).
    fn poll_blame(&mut self, rope: &Rope, version: u64) -> bool {
        let mut changed = false;
        if let Some(receiver) = self.blaming.as_ref() {
            match receiver.try_recv() {
                Ok((blamed, entries)) => {
                    changed = true;
                    self.blame = entries.unwrap_or_default();
                    self.blame_version = Some(blamed);
                    self.blaming = None;
                }
                Err(TryRecvError::Disconnected) => self.blaming = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.bases.is_none() || self.blaming.is_some() || self.blame_version == Some(version) {
            if self.blame_version == Some(version) {
                self.edited_at = None;
            }
            return changed;
        }
        let due = match self.edited_at {
            Some((seen, at)) if seen == version => at.elapsed() >= BLAME_DEBOUNCE,
            _ => {
                self.edited_at = Some((version, Instant::now()));
                self.blame_version.is_none()
            }
        };
        if let (true, Some(path)) = (due, self.path.clone()) {
            let rope = rope.clone();
            let (sender, receiver) = channel();
            std::thread::spawn(move || {
                let entries = git::blame(&path, &rope.to_string());
                // The receiver is gone only if the file was closed; nothing to report then.
                let _ = sender.send((version, entries));
            });
            self.blaming = Some(receiver);
        }
        changed
    }

    /// Adopt finished work and start a recomputation if the text moved on; returns whether the hunks changed.
    pub fn poll(&mut self, rope: &Rope, version: u64) -> bool {
        let mut changed = false;
        if let Some(receiver) = self.loading_bases.as_ref() {
            match receiver.try_recv() {
                Ok(bases) => {
                    self.bases = bases.map(Arc::new);
                    self.loading_bases = None;
                    self.hunks_version = None;
                    if self.bases.is_none() {
                        changed = !self.hunks.is_empty();
                        self.hunks.clear();
                    }
                }
                Err(TryRecvError::Disconnected) => self.loading_bases = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(receiver) = self.computing.as_ref() {
            match receiver.try_recv() {
                Ok((computed_for, hunks)) => {
                    changed |= hunks != self.hunks;
                    self.hunks = hunks;
                    self.hunks_version = Some(computed_for);
                    self.computing = None;
                }
                Err(TryRecvError::Disconnected) => self.computing = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        changed |= self.poll_blame(rope, version);
        if self.computing.is_none() && self.hunks_version != Some(version) {
            if let Some(bases) = self.bases.clone() {
                let rope = rope.clone();
                let (sender, receiver) = channel();
                std::thread::spawn(move || {
                    let hunks = git::uncommitted_hunks(&bases, &rope.to_string());
                    // The receiver is gone only if the file was closed; nothing to report then.
                    let _ = sender.send((version, hunks));
                });
                self.computing = Some(receiver);
            }
        }
        changed
    }
}
