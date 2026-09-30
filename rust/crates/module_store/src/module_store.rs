//! A shared store of `node_modules`, one copy per lockfile: a new workspace whose lockfile matches gets
//! the store's copy by copy-on-write clone, hard links or a plain copy, whichever the drive allows,
//! instead of a fresh install. pnpm and Yarn's hard-link and Plug'n'Play modes share packages themselves
//! and are left alone.

mod detect;
mod import;
mod overview;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub use detect::{detect, key, node_major, Detection, Manager};
pub use import::{probe, tree_size, Method};
pub use overview::{
    Holding, Measure, Overview, RepoOverview, RepoState, RepoWorktrees, User, Version, Worktree,
};

const STORE_DIR: &str = "nm-store";
const INDEX_FILE: &str = "index.json";
const LOCK_FILE: &str = ".lock";
const TEMP_DIR: &str = ".tmp";
const MODULES: &str = "node_modules";
/// Build caches tools write under node_modules; each workspace keeps its own.
const UNSHARED: [&str; 1] = [".cache"];
const STALE_TEMP: Duration = Duration::from_secs(3600);
const DAY: u64 = 86_400;

pub const ENABLED_KEY: &str = "modules_store_enabled";
pub const FALLBACK_KEY: &str = "modules_fallback";
pub const SIZE_LIMIT_KEY: &str = "modules_size_limit_gb";
pub const UNUSED_DAYS_KEY: &str = "modules_unused_days";

/// What to do where a copy-on-write clone is not possible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fallback {
    #[default]
    HardLink,
    Copy,
    Install,
}

impl Fallback {
    pub const ALL: [Fallback; 3] = [Fallback::HardLink, Fallback::Copy, Fallback::Install];

    pub fn setting(self) -> &'static str {
        match self {
            Fallback::HardLink => "hardlink",
            Fallback::Copy => "copy",
            Fallback::Install => "install",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Fallback::HardLink => "Hard Links",
            Fallback::Copy => "Copy",
            Fallback::Install => "Run Install",
        }
    }

    pub fn from_setting(value: &str) -> Fallback {
        Fallback::ALL
            .into_iter()
            .find(|fallback| fallback.setting() == value)
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub enabled: bool,
    pub fallback: Fallback,
    pub size_limit_gb: u64,
    pub unused_days: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            enabled: true,
            fallback: Fallback::default(),
            size_limit_gb: 20,
            unused_days: 14,
        }
    }
}

impl Options {
    /// The user's choices in `settings.json` (the CLI and workspace creation read them without the app).
    pub fn from_settings_file() -> Options {
        pom_paths::config_dir()
            .map(|dir| dir.join("settings.json"))
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .map_or_else(Options::default, |value| Options::from_settings(&value))
    }

    pub fn from_settings(value: &serde_json::Value) -> Options {
        let defaults = Options::default();
        let number = |key: &str, fallback: u64| {
            value
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(fallback)
        };
        Options {
            enabled: value
                .get(ENABLED_KEY)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(defaults.enabled),
            fallback: value
                .get(FALLBACK_KEY)
                .and_then(serde_json::Value::as_str)
                .map_or(defaults.fallback, Fallback::from_setting),
            size_limit_gb: number(SIZE_LIMIT_KEY, defaults.size_limit_gb),
            unused_days: number(UNUSED_DAYS_KEY, defaults.unused_days),
        }
    }

    /// The method to use given what the drive supports, or `None` to install normally.
    pub fn method(&self, supported: Method) -> Option<Method> {
        match (supported, self.fallback) {
            (Method::Clone, _) => Some(Method::Clone),
            (_, Fallback::Install) => None,
            (Method::HardLink, Fallback::HardLink) => Some(Method::HardLink),
            _ => Some(Method::Copy),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub repo: String,
    pub key: String,
    /// Empty for copies kept before the index existed.
    #[serde(default)]
    pub manager: String,
    #[serde(default)]
    pub method: Method,
    /// Bytes; 0 until measured.
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub last_used: u64,
    /// Worktrees that took this copy.
    #[serde(default)]
    pub workspaces: Vec<PathBuf>,
}

impl Entry {
    /// The worktrees that took this copy and still exist.
    pub fn live_workspaces(&self) -> Vec<&PathBuf> {
        self.workspaces
            .iter()
            .filter(|path| path.is_dir())
            .collect()
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    entries: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restored {
    FromStore(Method),
    /// Nothing to reuse; the install runs as usual.
    Install,
    SelfManaged(&'static str),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub removed: usize,
    pub freed: u64,
}

pub struct Store {
    root: PathBuf,
}

/// Holds the store's lock file open with an exclusive `flock`; released on drop.
struct Lock(fs::File);

impl Lock {
    fn acquire(root: &Path) -> io::Result<Lock> {
        use std::os::unix::io::AsRawFd;
        fs::create_dir_all(root)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(LOCK_FILE))?;
        // SAFETY: the descriptor stays open for the lifetime of `Lock`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Lock(file))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: the descriptor is still open; unlocking an unlocked file is harmless.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn remove_tree(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

impl Store {
    pub fn new(state: &pom_paths::StateDir) -> Store {
        Store {
            root: state.path(STORE_DIR),
        }
    }

    pub fn at(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entry_dir(&self, repo: &str, key: &str) -> PathBuf {
        self.root.join(repo).join(key).join(MODULES)
    }

    fn load(&self) -> Index {
        let mut index: Index = fs::read(self.root.join(INDEX_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        index
            .entries
            .retain(|entry| self.entry_dir(&entry.repo, &entry.key).is_dir());
        self.adopt_unindexed(&mut index);
        index
    }

    /// Copies kept before the index existed (or whose record was lost) join it, so they can be listed
    /// and expire like the rest.
    fn adopt_unindexed(&self, index: &mut Index) {
        let Ok(repos) = fs::read_dir(&self.root) else {
            return;
        };
        for repo in repos.flatten() {
            let repo_name = repo.file_name().to_string_lossy().into_owned();
            if repo_name.starts_with('.') || !repo.path().is_dir() {
                continue;
            }
            let Ok(keys) = fs::read_dir(repo.path()) else {
                continue;
            };
            for key in keys.flatten() {
                let key_name = key.file_name().to_string_lossy().into_owned();
                let known = index
                    .entries
                    .iter()
                    .any(|entry| entry.repo == repo_name && entry.key == key_name);
                if known || !key.path().join(MODULES).is_dir() {
                    continue;
                }
                let modified = key
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |elapsed| elapsed.as_secs());
                index.entries.push(Entry {
                    repo: repo_name.clone(),
                    key: key_name,
                    manager: String::new(),
                    method: Method::Clone,
                    size: 0,
                    created: modified,
                    last_used: modified,
                    workspaces: Vec::new(),
                });
            }
        }
    }

    fn save(&self, index: &Index) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(index).map_err(io::Error::other)?;
        pom_paths::write_atomic(&self.root.join(INDEX_FILE), &bytes, 0o644)
    }

    fn update(&self, change: impl FnOnce(&mut Index)) -> io::Result<()> {
        let _lock = Lock::acquire(&self.root)?;
        let mut index = self.load();
        change(&mut index);
        self.save(&index)
    }

    /// Puts `source` into the store under `repo`/`key` unless it is already there.
    fn fill(
        &self,
        repo: &str,
        key: &str,
        manager: Manager,
        source: &Path,
        method: Method,
    ) -> io::Result<()> {
        let target = self.entry_dir(repo, key);
        if target.is_dir() {
            return Ok(());
        }
        let temp_root = self.root.join(TEMP_DIR);
        fs::create_dir_all(&temp_root)?;
        let temp = temp_root.join(format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let built = import::import(source, &temp.join(MODULES), method, &UNSHARED).and_then(|()| {
            if method == Method::HardLink {
                import::protect(&temp.join(MODULES))?;
            }
            Ok(())
        });
        if let Err(error) = built {
            remove_tree(&temp)?;
            return Err(error);
        }
        let size = tree_size(&temp.join(MODULES));
        let final_dir = self.root.join(repo).join(key);
        fs::create_dir_all(self.root.join(repo))?;
        if let Err(error) = fs::rename(&temp, &final_dir) {
            remove_tree(&temp)?;
            // Another workspace filled the same key first.
            if final_dir.join(MODULES).is_dir() {
                return Ok(());
            }
            return Err(error);
        }
        let stamp = now();
        self.update(|index| {
            index
                .entries
                .retain(|entry| !(entry.repo == repo && entry.key == key));
            index.entries.push(Entry {
                repo: repo.to_string(),
                key: key.to_string(),
                manager: manager.label().to_string(),
                method,
                size,
                created: stamp,
                last_used: stamp,
                workspaces: Vec::new(),
            });
        })
    }

    fn touch(&self, repo: &str, key: &str, workspace: &Path) -> io::Result<()> {
        let stamp = now();
        self.update(|index| {
            if let Some(entry) = index
                .entries
                .iter_mut()
                .find(|entry| entry.repo == repo && entry.key == key)
            {
                entry.last_used = stamp;
                if !entry.workspaces.iter().any(|path| path == workspace) {
                    entry.workspaces.push(workspace.to_path_buf());
                }
            }
        })
    }

    fn key_for(&self, worktree: &Path, node: Option<&str>) -> Result<(Manager, String), Restored> {
        match detect(worktree) {
            Detection::SelfManaged(name) => Err(Restored::SelfManaged(name)),
            Detection::None => Err(Restored::Install),
            Detection::Store { manager, lockfile } => {
                let bytes = fs::read(&lockfile).map_err(|_| Restored::Install)?;
                let patches = detect::patches(worktree);
                let major = node.and_then(node_major);
                Ok((manager, key(&bytes, &patches, manager, major.as_deref())))
            }
        }
    }

    /// Gives a fresh worktree the stored `node_modules` for its lockfile, first keeping main's when main
    /// has the same lockfile. `node` is `node --version` as the workspace's shell reports it.
    pub fn restore(
        &self,
        repo: &str,
        worktree: &Path,
        main: &Path,
        node: Option<&str>,
        options: &Options,
    ) -> io::Result<Restored> {
        let target = worktree.join(MODULES);
        if !options.enabled || target.exists() {
            return Ok(Restored::Install);
        }
        let (manager, key) = match self.key_for(worktree, node) {
            Ok(found) => found,
            Err(restored) => return Ok(restored),
        };
        let Some(method) = options.method(probe(&self.root, worktree)) else {
            return Ok(Restored::Install);
        };
        let stored = self.entry_dir(repo, &key);
        if !stored.is_dir() {
            let main_modules = main.join(MODULES);
            let main_matches = main_modules.is_dir()
                && self
                    .key_for(main, node)
                    .is_ok_and(|(_, main_key)| main_key == key);
            if !main_matches {
                return Ok(Restored::Install);
            }
            // Linking would tie main's own files to the store, so main is always copied in.
            let from_main = match probe(&self.root, main) {
                Method::Clone => Method::Clone,
                _ => Method::Copy,
            };
            self.fill(repo, &key, manager, &main_modules, from_main)?;
        }
        if method == Method::HardLink {
            import::protect(&stored)?;
        }
        import::import(&stored, &target, method, &[])?;
        self.touch(repo, &key, worktree)?;
        Ok(Restored::FromStore(method))
    }

    /// Keeps a freshly installed `node_modules` for the next workspace with the same lockfile.
    pub fn snapshot(
        &self,
        repo: &str,
        worktree: &Path,
        node: Option<&str>,
        options: &Options,
    ) -> io::Result<()> {
        let source = worktree.join(MODULES);
        if !options.enabled || !source.is_dir() {
            return Ok(());
        }
        let Ok((manager, key)) = self.key_for(worktree, node) else {
            return Ok(());
        };
        let Some(method) = options.method(probe(&self.root, worktree)) else {
            return Ok(());
        };
        self.fill(repo, &key, manager, &source, method)?;
        self.touch(repo, &key, worktree)
    }

    /// Every stored copy, most recently used first, measuring any not measured yet.
    /// Every stored copy as recorded, most recently used first; a copy not measured yet has size 0.
    pub fn list_recorded(&self) -> io::Result<Vec<Entry>> {
        let _lock = Lock::acquire(&self.root)?;
        let mut entries = self.load().entries;
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_used));
        Ok(entries)
    }

    /// Records a measured size so later listings need not walk the copy again.
    pub fn record_size(&self, repo: &str, key: &str, size: u64) -> io::Result<()> {
        self.update(|index| {
            if let Some(entry) = index
                .entries
                .iter_mut()
                .find(|entry| entry.repo == repo && entry.key == key)
            {
                entry.size = size;
            }
        })
    }

    pub fn list(&self) -> io::Result<Vec<Entry>> {
        let _lock = Lock::acquire(&self.root)?;
        let mut index = self.load();
        let mut measured = false;
        for entry in index.entries.iter_mut().filter(|entry| entry.size == 0) {
            entry.size = tree_size(&self.entry_dir(&entry.repo, &entry.key));
            measured = true;
        }
        if measured {
            self.save(&index)?;
        }
        index
            .entries
            .sort_by_key(|entry| std::cmp::Reverse(entry.last_used));
        Ok(index.entries)
    }

    pub fn delete(&self, repo: &str, key: &str) -> io::Result<u64> {
        let _lock = Lock::acquire(&self.root)?;
        let mut index = self.load();
        let freed = index
            .entries
            .iter()
            .find(|entry| entry.repo == repo && entry.key == key)
            .map_or(0, |entry| entry.size);
        remove_tree(&self.root.join(repo).join(key))?;
        index
            .entries
            .retain(|entry| !(entry.repo == repo && entry.key == key));
        self.save(&index)?;
        Ok(freed)
    }

    pub fn clear(&self) -> io::Result<Pruned> {
        let entries = self.list()?;
        let mut pruned = Pruned::default();
        for entry in entries {
            pruned.freed += self.delete(&entry.repo, &entry.key)?;
            pruned.removed += 1;
        }
        remove_tree(&self.root.join(TEMP_DIR))?;
        Ok(pruned)
    }

    /// Drops copies unused for `unused_days`, then the least recently used until the store fits
    /// `size_limit_gb`, and temp folders left by an interrupted fill. Workspaces keep their own copies, so
    /// removing a stored one never breaks a workspace.
    pub fn prune(&self, options: &Options) -> io::Result<Pruned> {
        let entries = self.list()?;
        let stamp = now();
        let limit = options.size_limit_gb.saturating_mul(1 << 30);
        let mut total: u64 = entries.iter().map(|entry| entry.size).sum();
        let mut pruned = Pruned::default();
        let mut oldest_first = entries;
        oldest_first.reverse();
        for entry in &oldest_first {
            let expired = options.unused_days > 0
                && stamp.saturating_sub(entry.last_used) > options.unused_days * DAY;
            if expired || (limit > 0 && total > limit) {
                let freed = self.delete(&entry.repo, &entry.key)?;
                total = total.saturating_sub(freed);
                pruned.freed += freed;
                pruned.removed += 1;
            }
        }
        if let Ok(temps) = fs::read_dir(self.root.join(TEMP_DIR)) {
            for temp in temps.flatten() {
                let stale = temp
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.elapsed().ok())
                    .is_some_and(|age| age > STALE_TEMP);
                if stale {
                    remove_tree(&temp.path())?;
                }
            }
        }
        Ok(pruned)
    }

    /// What a workspace folder under `workspaces` would get from this store.
    pub fn supported_method(&self, workspaces: &Path) -> Method {
        probe(&self.root, workspaces)
    }
}

/// `3.2 GB`, `640 MB`.
pub fn format_size(bytes: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const MB: f64 = (1u64 << 20) as f64;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else {
        format!(
            "{:.0} MB",
            (bytes / MB).max(if bytes > 0.0 { 1.0 } else { 0.0 })
        )
    }
}

#[cfg(test)]
mod tests;
