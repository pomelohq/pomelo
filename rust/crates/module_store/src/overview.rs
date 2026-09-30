use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::{
    detect, import, key, node_major, tree_size, Detection, Entry, Manager, Method, Store, MODULES,
};

/// Something whose size is not known yet.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Measure {
    Stored {
        repo: String,
        key: String,
    },
    /// A workspace's own node_modules folder.
    Own {
        path: PathBuf,
    },
}

impl Overview {
    /// Fills in a size `measure` found.
    pub fn apply(&mut self, what: &Measure, size: u64) {
        let mut stored = |entry: &mut Entry| {
            if let Measure::Stored { repo, key } = what {
                if entry.repo == *repo && entry.key == *key {
                    entry.size = size;
                }
            }
        };
        self.others.iter_mut().for_each(&mut stored);
        for repo in &mut self.repos {
            let RepoState::Versions { versions, old, .. } = &mut repo.state else {
                continue;
            };
            old.iter_mut().for_each(&mut stored);
            for version in versions {
                if let Some(entry) = version.stored.as_mut() {
                    stored(entry);
                }
                for user in &mut version.users {
                    if let (Measure::Own { path }, Holding::Own { size: known }) =
                        (what, &mut user.holding)
                    {
                        if user.path.join(MODULES) == *path {
                            *known = Some(size);
                        }
                    }
                }
            }
        }
    }
}

/// A repo's worktree in one workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub workspace: String,
    pub is_main: bool,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoWorktrees {
    pub repo: String,
    pub worktrees: Vec<Worktree>,
    /// `node --version` as the repo's shell reports it.
    pub node: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holding {
    /// Its node_modules came from (or went into) the store.
    Shared,
    /// Installed on its own: a copy of its own, `None` until measured.
    Own {
        size: Option<u64>,
    },
    NotInstalled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub workspace: String,
    pub is_main: bool,
    pub path: PathBuf,
    pub holding: Holding,
}

/// One lockfile version of a repo: the worktrees on it and the stored copy, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub key: String,
    pub lockfile: String,
    /// When the lockfile last changed (seconds since the epoch).
    pub changed: Option<u64>,
    /// Main's current lockfile: new workspaces branch from it.
    pub on_main: bool,
    pub stored: Option<Entry>,
    pub users: Vec<User>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoState {
    SelfManaged(&'static str),
    NoLockfile,
    Versions {
        manager: Manager,
        versions: Vec<Version>,
        /// Stored copies no worktree's lockfile matches any more.
        old: Vec<Entry>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoOverview {
    pub repo: String,
    pub state: RepoState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overview {
    pub repos: Vec<RepoOverview>,
    /// Copies of repos this project does not have (other projects, or renamed repos).
    pub others: Vec<Entry>,
}

impl Overview {
    pub fn stored_total(&self) -> u64 {
        let mut total: u64 = self.others.iter().map(|entry| entry.size).sum();
        for repo in &self.repos {
            if let RepoState::Versions { versions, old, .. } = &repo.state {
                total += old.iter().map(|entry| entry.size).sum::<u64>();
                total += versions
                    .iter()
                    .filter_map(|version| version.stored.as_ref())
                    .map(|entry| entry.size)
                    .sum::<u64>();
            }
        }
        total
    }

    /// Every copy no worktree of this project uses: old versions and other projects' copies.
    pub fn unused(&self) -> Vec<&Entry> {
        let mut unused: Vec<&Entry> = self.others.iter().collect();
        for repo in &self.repos {
            if let RepoState::Versions { old, .. } = &repo.state {
                unused.extend(old.iter());
            }
        }
        unused
    }
}

fn modified(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs())
}

fn worktree_key(path: &Path, node: Option<&str>) -> Option<(Manager, String, PathBuf)> {
    let Detection::Store { manager, lockfile } = detect(path) else {
        return None;
    };
    let bytes = fs::read(&lockfile).ok()?;
    let patches = crate::detect::patches(path);
    let major = node.and_then(node_major);
    Some((
        manager,
        key(&bytes, &patches, manager, major.as_deref()),
        lockfile,
    ))
}

impl Store {
    /// How each repo's worktrees relate to the stored copies, from lockfiles and the index alone: sizes
    /// not measured yet are left for `to_measure`.
    pub fn overview(&self, repos: &[RepoWorktrees]) -> io::Result<Overview> {
        let entries = self.list_recorded()?;
        let mut overview = Overview::default();
        for input in repos {
            overview.repos.push(RepoOverview {
                repo: input.repo.clone(),
                state: self.repo_state(input, &entries),
            });
        }
        overview.others = entries
            .into_iter()
            .filter(|entry| !repos.iter().any(|input| input.repo == entry.repo))
            .collect();
        Ok(overview)
    }

    fn repo_state(&self, input: &RepoWorktrees, entries: &[Entry]) -> RepoState {
        let main = input.worktrees.iter().find(|worktree| worktree.is_main);
        let probe = main.or(input.worktrees.first());
        if let Some(Detection::SelfManaged(name)) = probe.map(|worktree| detect(&worktree.path)) {
            return RepoState::SelfManaged(name);
        }
        let node = input.node.as_deref();
        let mut manager = None;
        let mut versions: BTreeMap<String, Version> = BTreeMap::new();
        for worktree in &input.worktrees {
            let Some((found, key, lockfile)) = worktree_key(&worktree.path, node) else {
                continue;
            };
            manager.get_or_insert(found);
            let stored = entries
                .iter()
                .find(|entry| entry.repo == input.repo && entry.key == key)
                .cloned();
            let installed = worktree.path.join(MODULES).is_dir();
            let recorded = stored
                .as_ref()
                .is_some_and(|entry| entry.workspaces.contains(&worktree.path));
            let holding = if !installed {
                Holding::NotInstalled
            } else if recorded || (worktree.is_main && stored.is_some()) {
                // Main is where a copy is first taken from, so it shares with the store already.
                Holding::Shared
            } else {
                Holding::Own { size: None }
            };
            let version = versions.entry(key.clone()).or_insert_with(|| Version {
                key,
                lockfile: lockfile
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                changed: modified(&lockfile),
                on_main: false,
                stored,
                users: Vec::new(),
            });
            version.on_main |= worktree.is_main;
            version.users.push(User {
                workspace: worktree.workspace.clone(),
                is_main: worktree.is_main,
                path: worktree.path.clone(),
                holding,
            });
        }
        let Some(manager) = manager else {
            return RepoState::NoLockfile;
        };
        let old = entries
            .iter()
            .filter(|entry| entry.repo == input.repo && !versions.contains_key(&entry.key))
            .cloned()
            .collect();
        let mut versions: Vec<Version> = versions.into_values().collect();
        // Main's version first, then the most used.
        versions.sort_by_key(|version| (!version.on_main, std::cmp::Reverse(version.users.len())));
        RepoState::Versions {
            manager,
            versions,
            old,
        }
    }

    /// Swaps a worktree's own node_modules for a clone of the stored copy of its lockfile; returns the
    /// bytes its own copy took. The old folder is kept until the new one is in place.
    pub fn relink(&self, repo: &str, worktree: &Path, node: Option<&str>) -> io::Result<u64> {
        let Some((_, key, _)) = worktree_key(worktree, node) else {
            return Err(io::Error::other("no lockfile the store keeps"));
        };
        let stored = self.entry_dir(repo, &key);
        if !stored.is_dir() {
            return Err(io::Error::other("no stored copy matches its lockfile"));
        }
        let target = worktree.join(MODULES);
        let freed = import::tree_size(&target);
        let aside = worktree.join(format!(".node_modules.pom-old-{}", std::process::id()));
        if target.exists() {
            fs::rename(&target, &aside)?;
        }
        let method = match import::probe(self.root(), worktree) {
            Method::Clone => Method::Clone,
            Method::HardLink => {
                import::protect(&stored)?;
                Method::HardLink
            }
            Method::Copy => Method::Copy,
        };
        if let Err(error) = import::import(&stored, &target, method, &[]) {
            if target.exists() {
                fs::remove_dir_all(&target)?;
            }
            if aside.exists() {
                fs::rename(&aside, &target)?;
            }
            return Err(error);
        }
        if aside.exists() {
            fs::remove_dir_all(&aside)?;
        }
        self.touch(repo, &key, worktree)?;
        Ok(freed)
    }

    /// Keeps a worktree's own install as the stored copy of its lockfile. Its files are cloned or
    /// copied, never linked, so the worktree stays writable.
    pub fn keep(&self, repo: &str, worktree: &Path, node: Option<&str>) -> io::Result<()> {
        let Some((manager, key, _)) = worktree_key(worktree, node) else {
            return Err(io::Error::other("no lockfile the store keeps"));
        };
        let source = worktree.join(MODULES);
        if !source.is_dir() {
            return Err(io::Error::other("it has no node_modules to keep"));
        }
        let method = match import::probe(self.root(), worktree) {
            Method::Clone => Method::Clone,
            _ => Method::Copy,
        };
        self.fill(repo, &key, manager, &source, method)?;
        self.touch(repo, &key, worktree)
    }

    /// Everything in `overview` still without a size: stored copies and workspaces' own installs.
    pub fn to_measure(&self, overview: &Overview) -> Vec<Measure> {
        let mut out = Vec::new();
        let mut entry = |entry: &Entry| {
            if entry.size == 0 {
                out.push(Measure::Stored {
                    repo: entry.repo.clone(),
                    key: entry.key.clone(),
                });
            }
        };
        for repo in &overview.repos {
            if let RepoState::Versions { versions, old, .. } = &repo.state {
                old.iter().for_each(&mut entry);
                for version in versions {
                    if let Some(stored) = &version.stored {
                        entry(stored);
                    }
                }
            }
        }
        overview.others.iter().for_each(&mut entry);
        for repo in &overview.repos {
            if let RepoState::Versions { versions, .. } = &repo.state {
                for version in versions {
                    for user in &version.users {
                        if matches!(user.holding, Holding::Own { size: None }) {
                            out.push(Measure::Own {
                                path: user.path.join(MODULES),
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// Walks one thing to measure; a stored copy's size is kept in the index.
    pub fn measure(&self, what: &Measure) -> io::Result<u64> {
        match what {
            Measure::Stored { repo, key } => {
                let size = tree_size(&self.entry_dir(repo, key));
                self.record_size(repo, key, size)?;
                Ok(size)
            }
            Measure::Own { path } => Ok(tree_size(path)),
        }
    }

    /// Removes the given copies; returns how many bytes they held.
    pub fn delete_all(&self, copies: &[(String, String)]) -> io::Result<u64> {
        let mut freed = 0;
        for (repo, key) in copies {
            freed += self.delete(repo, key)?;
        }
        Ok(freed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Options;

    fn worktree(root: &Path, name: &str, lock: &str, installed: bool) -> Worktree {
        let path = root.join(name).join("api");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("package-lock.json"), lock).unwrap();
        if installed {
            fs::create_dir_all(path.join("node_modules/pad")).unwrap();
            fs::write(path.join("node_modules/pad/index.js"), "pad").unwrap();
        }
        Worktree {
            workspace: name.into(),
            is_main: name == "main",
            path,
        }
    }

    #[test]
    fn worktrees_group_by_lockfile_with_old_copies_and_own_installs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("store"));
        let options = Options {
            fallback: crate::Fallback::Copy,
            ..Options::default()
        };
        let main = worktree(dir.path(), "main", "v1", true);
        let login = worktree(dir.path(), "feat-login", "v1", false);
        store
            .restore("api", &login.path, &main.path, None, &options)
            .unwrap();
        let own = worktree(dir.path(), "feat-pay", "v1", true);
        let react = worktree(dir.path(), "feat-react", "v2", false);
        let stale = dir.path().join("stale/node_modules");
        fs::create_dir_all(stale.join("x")).unwrap();
        fs::write(stale.join("x/i.js"), "old").unwrap();
        store
            .fill("api", "oldkey", Manager::Npm, &stale, Method::Copy)
            .unwrap();
        store
            .fill("web", "webkey", Manager::Npm, &stale, Method::Copy)
            .unwrap();
        let overview = store
            .overview(&[RepoWorktrees {
                repo: "api".into(),
                worktrees: vec![main.clone(), login.clone(), own.clone(), react.clone()],
                node: None,
            }])
            .unwrap();
        let RepoState::Versions { versions, old, .. } = &overview.repos[0].state else {
            panic!("{overview:?}");
        };
        assert_eq!(versions.len(), 2);
        let current = &versions[0];
        assert!(current.on_main && current.stored.is_some());
        let holding = |name: &str| {
            current
                .users
                .iter()
                .find(|user| user.workspace == name)
                .map(|user| user.holding.clone())
        };
        assert_eq!(holding("main"), Some(Holding::Shared));
        assert_eq!(holding("feat-login"), Some(Holding::Shared));
        assert_eq!(holding("feat-pay"), Some(Holding::Own { size: None }));
        let mut measured = overview.clone();
        let pending = store.to_measure(&measured);
        assert_eq!(
            pending.len(),
            1,
            "stored copies were measured when kept: {pending:?}"
        );
        let size = store.measure(&pending[0]).unwrap();
        measured.apply(&pending[0], size);
        let RepoState::Versions {
            versions: after, ..
        } = &measured.repos[0].state
        else {
            panic!();
        };
        assert!(after[0]
            .users
            .iter()
            .any(|user| user.holding == Holding::Own { size: Some(3) }));
        assert_eq!(versions[1].users[0].holding, Holding::NotInstalled);
        assert!(versions[1].stored.is_none());
        assert_eq!(old.len(), 1);
        assert_eq!(overview.others.len(), 1);
        assert_eq!(overview.unused().len(), 2);

        assert_eq!(store.relink("api", &own.path, None).unwrap(), 3);
        let after = store
            .overview(&[RepoWorktrees {
                repo: "api".into(),
                worktrees: vec![own.clone()],
                node: None,
            }])
            .unwrap();
        let RepoState::Versions { versions, .. } = &after.repos[0].state else {
            panic!();
        };
        assert_eq!(versions[0].users[0].holding, Holding::Shared);
        assert!(!own
            .path
            .read_dir()
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains("pom-old")));
    }

    #[test]
    fn keep_stores_a_worktrees_install_without_linking_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("store"));
        let main = worktree(dir.path(), "main", "v1", true);
        store.keep("api", &main.path, None).unwrap();
        let entries = store.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(fs::write(main.path.join("node_modules/pad/index.js"), "mine").is_ok());
        assert!(store
            .relink("api", &dir.path().join("nowhere"), None)
            .is_err());
        assert_eq!(
            store
                .delete_all(&[("api".into(), entries[0].key.clone())])
                .unwrap(),
            3
        );
    }
}
