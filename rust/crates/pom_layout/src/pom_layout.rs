use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub const WORKSPACE_PREFIX: &str = "workspace--";
const WORKSPACE_STATE_FILE: &str = ".pom-workspace.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub name: String,
    pub path: PathBuf,
    /// The branch checked out right now (a short sha when detached, empty when unreadable).
    pub branch: String,
    /// The branch the workspace means it to use: its override, else the workspace branch.
    pub expected: String,
}

/// What a checkout's HEAD points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    /// A commit, as a short sha.
    Detached(String),
}

impl Head {
    pub fn name(&self) -> &str {
        match self {
            Head::Branch(name) | Head::Detached(name) => name,
        }
    }
}

/// Reads HEAD straight from disk, without running git: `.git/HEAD` for a clone, `<gitdir>/HEAD` for a
/// worktree whose `.git` file says `gitdir: <path>`.
pub fn read_head(checkout: &Path) -> Option<Head> {
    let dot_git = checkout.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let pointer = text.lines().find_map(|line| line.strip_prefix("gitdir:"))?;
        checkout.join(pointer.trim())
    };
    parse_head(&std::fs::read_to_string(git_dir.join("HEAD")).ok()?)
}

fn parse_head(text: &str) -> Option<Head> {
    let text = text.trim();
    if let Some(reference) = text.strip_prefix("ref:") {
        let reference = reference.trim();
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        return (!name.is_empty()).then(|| Head::Branch(name.to_string()));
    }
    let is_sha = text.len() >= 7 && text.chars().all(|c| c.is_ascii_hexdigit());
    is_sha.then(|| Head::Detached(text[..7].to_string()))
}

fn repo_at(name: String, path: PathBuf, state: &WorkspaceState, workspace_branch: &str) -> Repo {
    let branch = read_head(&path)
        .map(|head| head.name().to_string())
        .unwrap_or_default();
    let expected = state.repo_branch(&name, workspace_branch).to_string();
    Repo {
        name,
        path,
        branch,
        expected,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub branch: String,
    pub is_main: bool,
    pub path: PathBuf,
    pub repos: Vec<Repo>,
}

/// Folder of a branch's workspace, whether or not it exists yet.
pub fn workspace_folder(project_root: &Path, branch: &str) -> PathBuf {
    project_root.join(format!("{WORKSPACE_PREFIX}{branch}"))
}

/// The main workspace may still live directly in the project root (repos cloned before the
/// `workspace--<default>` layout); every other branch always has its own folder.
pub fn workspace_root(project_root: &Path, branch: &str, is_main: bool) -> PathBuf {
    let folder = workspace_folder(project_root, branch);
    if is_main && !folder.is_dir() {
        return project_root.to_path_buf();
    }
    folder
}

pub fn repo_worktree(project_root: &Path, repo: &str, branch: &str, is_main: bool) -> PathBuf {
    workspace_root(project_root, branch, is_main).join(repo)
}

/// A checkout or a worktree (`.git` is a dir or a file).
pub fn is_git_repo(path: &Path) -> bool {
    std::fs::metadata(path.join(".git")).is_ok_and(|meta| meta.is_dir() || meta.is_file())
}

/// Workspaces on disk: the main one first, then branches sorted by name, each with its git repos
/// sorted by name. Legacy root-level repos count for main only until a `workspace--<default>`
/// folder exists, and only if `known_repos` (when given) lists them.
pub fn scan(
    project_root: &Path,
    default_branch: &str,
    known_repos: Option<&HashSet<String>>,
) -> Vec<Workspace> {
    let Ok(entries) = std::fs::read_dir(project_root) else {
        return Vec::new();
    };
    let mut directories: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect();
    directories.sort();
    let main_folder = format!("{WORKSPACE_PREFIX}{default_branch}");
    let migrated_main = directories.iter().any(|(name, _)| *name == main_folder);

    let mut main = Workspace {
        branch: default_branch.to_string(),
        is_main: true,
        path: project_root.to_path_buf(),
        repos: Vec::new(),
    };
    let main_state = WorkspaceState::load(project_root);
    let mut branches: BTreeMap<String, Workspace> = BTreeMap::new();
    for (name, path) in directories {
        if let Some(branch) = name.strip_prefix(WORKSPACE_PREFIX) {
            let repos = git_repos_in(&path, branch);
            if branch == default_branch {
                main.path = path;
                main.repos.extend(repos);
            } else {
                branches.insert(
                    branch.to_string(),
                    Workspace {
                        branch: branch.to_string(),
                        is_main: false,
                        path,
                        repos,
                    },
                );
            }
            continue;
        }
        let known = known_repos.is_none_or(|known| known.contains(&name));
        if !migrated_main && known && is_git_repo(&path) {
            main.repos
                .push(repo_at(name, path, &main_state, default_branch));
        }
    }

    let mut out = Vec::new();
    if !main.repos.is_empty() {
        main.repos.sort_by(|a, b| a.name.cmp(&b.name));
        out.push(main);
    }
    out.extend(branches.into_values());
    out
}

fn git_repos_in(folder: &Path, workspace_branch: &str) -> Vec<Repo> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let state = WorkspaceState::load(folder);
    let mut repos: Vec<Repo> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| is_git_repo(&entry.path()))
        .map(|entry| {
            repo_at(
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
                &state,
                workspace_branch,
            )
        })
        .collect();
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    repos
}

/// Per-workspace choices kept inside the workspace folder (`.pom-workspace.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceState {
    /// Service key (`repo/svc`) or repo name -> environment profile.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub service_envs: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    /// Repo -> the branch it uses when that is not the workspace branch.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub repo_branches: IndexMap<String, String>,
}

impl WorkspaceState {
    pub fn load(workspace_folder: &Path) -> WorkspaceState {
        std::fs::read_to_string(workspace_folder.join(WORKSPACE_STATE_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, workspace_folder: &Path) -> std::io::Result<()> {
        let mut text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        text.push('\n');
        std::fs::write(workspace_folder.join(WORKSPACE_STATE_FILE), text)
    }

    /// The branch a repo is meant to be on: its override, else the workspace branch.
    pub fn repo_branch<'a>(&'a self, repo: &str, workspace_branch: &'a str) -> &'a str {
        self.repo_branches
            .get(repo)
            .map_or(workspace_branch, String::as_str)
    }

    /// Records the branch a repo uses; the workspace branch itself needs no entry.
    pub fn set_repo_branch(&mut self, repo: &str, branch: &str, workspace_branch: &str) {
        if branch == workspace_branch || branch.is_empty() {
            self.repo_branches.shift_remove(repo);
        } else {
            self.repo_branches
                .insert(repo.to_string(), branch.to_string());
        }
    }

    /// Profile for a service, falling back to its repo's entry.
    pub fn service_env(&self, service_key: &str) -> &str {
        if let Some(env) = self.service_envs.get(service_key) {
            return env;
        }
        match service_key.split_once('/') {
            Some((repo, _)) if !repo.is_empty() => {
                self.service_envs.get(repo).map_or("", String::as_str)
            }
            _ => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_repo(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).expect("mkdir .git");
    }

    fn worktree(path: &Path) {
        std::fs::create_dir_all(path).expect("mkdir worktree");
        std::fs::write(path.join(".git"), "gitdir: /elsewhere\n").expect("write .git file");
    }

    fn summary(workspaces: &[Workspace]) -> Vec<(String, bool, Vec<String>)> {
        workspaces
            .iter()
            .map(|ws| {
                (
                    ws.branch.clone(),
                    ws.is_main,
                    ws.repos.iter().map(|r| r.name.clone()).collect(),
                )
            })
            .collect()
    }

    fn owned(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn legacy_root_repos_form_main() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        git_repo(&root.join("web"));
        git_repo(&root.join("api"));
        std::fs::create_dir_all(root.join("notes")).expect("mkdir");
        worktree(&root.join("workspace--feat-x/api"));
        std::fs::create_dir_all(root.join("workspace--feat-x/plain")).expect("mkdir");
        assert_eq!(
            summary(&scan(root, "main", None)),
            [
                ("main".to_string(), true, owned(&["api", "web"])),
                ("feat-x".to_string(), false, owned(&["api"])),
            ]
        );
        let known: HashSet<String> = ["api".to_string()].into_iter().collect();
        assert_eq!(scan(root, "main", Some(&known))[0].repos.len(), 1);
    }

    #[test]
    fn migrated_main_ignores_root_repos() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        git_repo(&root.join("stale"));
        worktree(&root.join("workspace--main/api"));
        worktree(&root.join("workspace--b/api"));
        worktree(&root.join("workspace--a/api"));
        let workspaces = scan(root, "main", None);
        assert_eq!(
            summary(&workspaces),
            [
                ("main".to_string(), true, owned(&["api"])),
                ("a".to_string(), false, owned(&["api"])),
                ("b".to_string(), false, owned(&["api"])),
            ]
        );
        assert_eq!(workspaces[0].path, root.join("workspace--main"));
        assert!(scan(&root.join("missing"), "main", None).is_empty());
    }

    #[test]
    fn workspace_paths() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        assert_eq!(workspace_root(root, "main", true), root);
        assert_eq!(
            workspace_root(root, "feat", false),
            root.join("workspace--feat")
        );
        std::fs::create_dir_all(root.join("workspace--main")).expect("mkdir");
        assert_eq!(
            repo_worktree(root, "api", "main", true),
            root.join("workspace--main/api")
        );
    }

    #[test]
    fn head_parses_branches_and_detached_commits() {
        assert_eq!(
            parse_head("ref: refs/heads/ana/mail-retry\n"),
            Some(Head::Branch("ana/mail-retry".into()))
        );
        assert_eq!(
            parse_head("4f9c2e1d0b7a8c6e5f4d3c2b1a0f9e8d7c6b5a49\n"),
            Some(Head::Detached("4f9c2e1".into()))
        );
        assert_eq!(parse_head("ref: refs/heads/"), None);
        assert_eq!(parse_head("garbage"), None);
    }

    #[test]
    fn scan_reads_each_repos_branch_and_its_override() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let clone = root.join("workspace--main/api");
        std::fs::create_dir_all(clone.join(".git"))?;
        std::fs::write(clone.join(".git/HEAD"), "ref: refs/heads/main\n")?;
        let git_dir = clone.join(".git/worktrees/api");
        std::fs::create_dir_all(&git_dir)?;
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/ana/mail-retry\n")?;
        let worktree = root.join("workspace--feat-login/api");
        std::fs::create_dir_all(&worktree)?;
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )?;
        let relative = root.join("workspace--feat-login/web");
        std::fs::create_dir_all(relative.join("../.gitdirs/web"))?;
        std::fs::write(relative.join(".git"), "gitdir: ../.gitdirs/web\n")?;
        std::fs::write(
            relative.join("../.gitdirs/web/HEAD"),
            "ref: refs/heads/feat-login\n",
        )?;

        let mut state = WorkspaceState::default();
        state.set_repo_branch("api", "ana/mail-retry", "feat-login");
        state.set_repo_branch("web", "feat-login", "feat-login");
        state.save(&root.join("workspace--feat-login"))?;

        let workspaces = scan(root, "main", None);
        let feat = &workspaces[1];
        let repo = |name: &str| feat.repos.iter().find(|repo| repo.name == name).cloned();
        let api = repo("api").expect("api");
        assert_eq!(
            (api.branch.as_str(), api.expected.as_str()),
            ("ana/mail-retry", "ana/mail-retry")
        );
        let web = repo("web").expect("web");
        assert_eq!(
            (web.branch.as_str(), web.expected.as_str()),
            ("feat-login", "feat-login")
        );
        assert_eq!(workspaces[0].repos[0].branch, "main");

        let loaded = WorkspaceState::load(&root.join("workspace--feat-login"));
        assert_eq!(
            loaded.repo_branches.into_iter().collect::<Vec<_>>(),
            [("api".to_string(), "ana/mail-retry".to_string())],
            "the workspace branch itself is never recorded"
        );
        Ok(())
    }

    #[test]
    fn workspace_state_round_trip_and_fallback() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        assert_eq!(WorkspaceState::load(temp.path()), WorkspaceState::default());
        let mut state = WorkspaceState::default();
        state.service_envs.insert("web".into(), "staging".into());
        state
            .service_envs
            .insert("api/server".into(), "prod".into());
        state.save(temp.path())?;
        let loaded = WorkspaceState::load(temp.path());
        assert_eq!(loaded, state);
        assert_eq!(loaded.service_env("api/server"), "prod");
        assert_eq!(loaded.service_env("web/dev"), "staging");
        assert_eq!(loaded.service_env("api/worker"), "");
        let text = std::fs::read_to_string(temp.path().join(".pom-workspace.json"))?;
        assert!(!text.contains("display_name") && text.ends_with('\n'));
        Ok(())
    }
}
