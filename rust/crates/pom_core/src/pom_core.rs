mod watch;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pom_config::{Config, CONFIG_FILE_NAME};
use pom_layout::Workspace;
use pom_paths::{write_atomic, StateDir};
use pom_sessions::{Projects, Sessions};

pub use watch::ConfigWatcher;

/// The previous app remembers the last opened project here; kept so both can hand off to each other.
const LAST_PROJECT_FILE: &str = "last_project";
/// Project root -> the workspace branch last active in it.
const ACTIVE_WORKSPACES_FILE: &str = "active_workspaces.json";

/// A project opened from its `pom.yml`. A config that fails to load or validate still opens: the
/// workspace stays usable and the error is shown until the file is fixed.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub session: String,
    pub config: Option<Config>,
    pub error: Option<ConfigProblem>,
    pub workspaces: Vec<Workspace>,
    /// Branch of the workspace this window works in; empty means the main workspace.
    active: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProblem {
    pub message: String,
    /// 1-based line of the first problem in the root file, when the message names one.
    pub line: Option<u32>,
}

impl Project {
    pub fn open(config_path: &Path, state: &StateDir) -> Project {
        let root = config_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let mut project = Project {
            root,
            config_path: config_path.to_path_buf(),
            session: String::new(),
            config: None,
            error: None,
            workspaces: Vec::new(),
            active: String::new(),
        };
        project.reload(state);
        project.active = remembered_workspace(state, &project.root).unwrap_or_default();
        project
    }

    /// The branch of the active workspace: the chosen one while it still exists on disk, else main.
    pub fn active_branch(&self) -> &str {
        self.active_workspace()
            .map_or_else(|| self.branch(), |workspace| workspace.branch.as_str())
    }

    pub fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|workspace| !self.active.is_empty() && workspace.branch == self.active)
            .or_else(|| self.workspaces.iter().find(|workspace| workspace.is_main))
    }

    /// Where the active workspace's files live: its folder, or the project root while it has none.
    pub fn active_root(&self) -> PathBuf {
        self.active_workspace().map_or_else(
            || pom_layout::workspace_root(&self.root, self.branch(), true),
            |workspace| workspace.path.clone(),
        )
    }

    /// Switch to the workspace of `branch` and remember it for this project; false when it isn't one
    /// of the project's workspaces or is already active.
    pub fn set_active(&mut self, branch: &str, state: &StateDir) -> std::io::Result<bool> {
        if branch == self.active_branch()
            || !self
                .workspaces
                .iter()
                .any(|workspace| workspace.branch == branch)
        {
            return Ok(false);
        }
        self.active = branch.to_string();
        remember_workspace(state, &self.root, branch)?;
        Ok(true)
    }

    /// Re-reads the config and rescans workspaces; returns whether anything the UI shows changed.
    pub fn reload(&mut self, state: &StateDir) -> bool {
        let before = (
            self.error.clone(),
            self.session.clone(),
            self.branch().to_string(),
        );
        let before_workspaces = self.workspace_names();
        match Config::load(&self.config_path) {
            Ok(config) => {
                self.error = config.validate().err().map(|message| ConfigProblem {
                    line: None,
                    message,
                });
                self.session = config.session.clone();
                self.config = Some(config);
            }
            Err(error) => {
                let file = error.path.strip_prefix(&self.root).unwrap_or(&error.path);
                self.error = Some(ConfigProblem {
                    line: first_line_number(&error.message)
                        .filter(|_| error.path == self.config_path),
                    message: format!("{}: {}", file.display(), error.message),
                });
                // Keep the last good config so the rest of the app keeps working mid-edit.
                if self.session.is_empty() {
                    self.session = session_for_root(state, &self.root);
                }
            }
        }
        self.workspaces = pom_layout::scan(&self.root, self.branch(), None);
        (
            self.error.clone(),
            self.session.clone(),
            self.branch().to_string(),
        ) != before
            || self.workspace_names() != before_workspaces
    }

    pub fn branch(&self) -> &str {
        self.config
            .as_ref()
            .map_or("main", Config::global_default_branch)
    }

    fn workspace_names(&self) -> Vec<String> {
        self.workspaces.iter().map(|ws| ws.branch.clone()).collect()
    }

    /// Remember this project as last opened and as the current session, so the next launch (of
    /// either app) comes back to it.
    pub fn record_open(&self, state: &StateDir, now: i64) -> std::io::Result<()> {
        let root = self.root.to_string_lossy();
        write_atomic(&state.path(LAST_PROJECT_FILE), root.as_bytes(), 0o644)?;
        let mut sessions = Sessions::load(state);
        sessions.touch(&self.session, &root, now);
        sessions.save(state)?;
        Projects::register(state, &self.session, &self.root)
    }
}

fn read_active_workspaces(state: &StateDir) -> BTreeMap<String, String> {
    std::fs::read_to_string(state.path(ACTIVE_WORKSPACES_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn remembered_workspace(state: &StateDir, root: &Path) -> Option<String> {
    read_active_workspaces(state).remove(root.to_string_lossy().as_ref())
}

fn remember_workspace(state: &StateDir, root: &Path, branch: &str) -> std::io::Result<()> {
    let mut active = read_active_workspaces(state);
    active.insert(root.to_string_lossy().into_owned(), branch.to_string());
    let text = serde_json::to_string_pretty(&active).map_err(std::io::Error::other)?;
    write_atomic(&state.path(ACTIVE_WORKSPACES_FILE), text.as_bytes(), 0o644)
}

/// `pom.yml` directly inside `dir`, if there is one.
pub fn config_in(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join(CONFIG_FILE_NAME);
    candidate.is_file().then_some(candidate)
}

/// Which project to open at launch: an explicit `POM_CONFIG`, else the last opened project, else
/// the registry's current session. `None` means show the welcome page.
pub fn startup_config(state: &StateDir, explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = explicit.filter(|path| path.is_file()) {
        return Some(explicit.to_path_buf());
    }
    let last = std::fs::read_to_string(state.path(LAST_PROJECT_FILE)).unwrap_or_default();
    let last = last.trim();
    if let Some(config) = (!last.is_empty())
        .then(|| config_in(Path::new(last)))
        .flatten()
    {
        return Some(config);
    }
    let sessions = Sessions::load(state);
    let current = sessions.get(&sessions.current)?;
    config_in(Path::new(&current.path))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub name: String,
    pub path: PathBuf,
    /// Its folder or `pom.yml` is gone, so it can only be removed.
    pub missing: bool,
    pub last_used: i64,
}

/// Known sessions, most recently used first.
pub fn session_entries(state: &StateDir) -> Vec<SessionEntry> {
    let mut entries: Vec<SessionEntry> = Sessions::load(state)
        .sessions
        .into_iter()
        .map(|session| {
            let path = PathBuf::from(&session.path);
            SessionEntry {
                missing: config_in(&path).is_none(),
                name: session.name,
                path,
                last_used: session.last_used,
            }
        })
        .collect();
    entries.sort_by(|a, b| b.last_used.cmp(&a.last_used).then(a.name.cmp(&b.name)));
    entries
}

/// Drops a session from the registries; files on disk are left alone.
pub fn forget_session(state: &StateDir, name: &str) -> std::io::Result<()> {
    let mut sessions = Sessions::load(state);
    sessions.remove(name);
    sessions.save(state)?;
    Projects::unregister(state, name)
}

fn session_for_root(state: &StateDir, root: &Path) -> String {
    let root_text = root.to_string_lossy();
    Sessions::load(state)
        .sessions
        .into_iter()
        .find(|session| session.path == root_text)
        .map(|session| session.name)
        .or_else(|| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

fn first_line_number(message: &str) -> Option<u32> {
    let (_, rest) = message.split_once("line ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        temp: tempfile::TempDir,
        state: StateDir,
    }

    impl Fixture {
        fn new() -> Fixture {
            let temp = tempfile::tempdir().expect("temp dir");
            let state = StateDir::new(temp.path().join("state/pom"));
            Fixture { temp, state }
        }

        fn project(&self, name: &str, config: &str) -> PathBuf {
            let dir = self.temp.path().join(name);
            std::fs::create_dir_all(&dir).expect("project dir");
            std::fs::write(dir.join("pom.yml"), config).expect("write pom.yml");
            dir.join("pom.yml")
        }
    }

    #[test]
    fn opens_a_valid_project_and_scans_workspaces() {
        let fixture = Fixture::new();
        let config = fixture.project("acme", "session: acme\ndefault_branch: trunk\n");
        let root = config.parent().map(Path::to_path_buf).unwrap_or_default();
        std::fs::create_dir_all(root.join("workspace--feat-x/api/.git")).expect("worktree");
        std::fs::create_dir_all(root.join("workspace--trunk/api/.git")).expect("main worktree");
        let project = Project::open(&config, &fixture.state);
        assert_eq!(project.session, "acme");
        assert_eq!(project.branch(), "trunk");
        assert!(project.error.is_none());
        let branches: Vec<&str> = project
            .workspaces
            .iter()
            .map(|w| w.branch.as_str())
            .collect();
        assert_eq!(branches, ["trunk", "feat-x"]);
    }

    #[test]
    fn active_workspace_switches_roots_and_is_remembered() {
        let fixture = Fixture::new();
        let config = fixture.project("acme", "session: acme\n");
        let root = config.parent().map(Path::to_path_buf).unwrap_or_default();
        std::fs::create_dir_all(root.join("api/.git")).expect("legacy main repo");
        std::fs::create_dir_all(root.join("workspace--feat-x/api/.git")).expect("worktree");
        let mut project = Project::open(&config, &fixture.state);
        assert_eq!(project.active_branch(), "main");
        assert_eq!(project.active_root(), root);

        assert!(project
            .set_active("feat-x", &fixture.state)
            .expect("activate"));
        assert!(!project.set_active("feat-x", &fixture.state).expect("again"));
        assert!(!project
            .set_active("ghost", &fixture.state)
            .expect("unknown"));
        assert_eq!(project.active_branch(), "feat-x");
        assert_eq!(project.active_root(), root.join("workspace--feat-x"));

        let reopened = Project::open(&config, &fixture.state);
        assert_eq!(reopened.active_branch(), "feat-x");

        std::fs::remove_dir_all(root.join("workspace--feat-x")).expect("remove worktree");
        let mut project = reopened;
        project.reload(&fixture.state);
        assert_eq!(
            project.active_branch(),
            "main",
            "a removed workspace falls back to main"
        );
    }

    #[test]
    fn broken_config_opens_with_a_located_error() {
        let fixture = Fixture::new();
        let config = fixture.project("acme", "session: acme\nrepos:\n  api:\n    setup: npm i\n");
        let project = Project::open(&config, &fixture.state);
        let error = project.error.clone().expect("config error");
        assert_eq!(error.line, Some(4));
        assert!(
            error.message.contains("cannot unmarshal"),
            "{}",
            error.message
        );
        assert_eq!(project.session, "acme");
    }

    #[test]
    fn validation_failure_is_an_error_without_a_line() {
        let fixture = Fixture::new();
        let config = fixture.project("acme", "repos:\n  r:\n    profiles: [ghost]\n");
        let error = Project::open(&config, &fixture.state).error.expect("error");
        assert!(error.message.contains("ghost"));
        assert_eq!(error.line, None);
    }

    #[test]
    fn reload_keeps_last_good_config_and_reports_change() {
        let fixture = Fixture::new();
        let config = fixture.project("acme", "session: acme\n");
        let mut project = Project::open(&config, &fixture.state);
        assert!(!project.reload(&fixture.state));
        std::fs::write(&config, "session: [broken\n").expect("break config");
        assert!(project.reload(&fixture.state));
        assert!(project.error.is_some());
        assert_eq!(project.session, "acme");
        assert!(project.config.is_some());
        std::fs::write(&config, "session: acme\n").expect("fix config");
        assert!(project.reload(&fixture.state));
        assert!(project.error.is_none());
    }

    #[test]
    fn startup_prefers_explicit_then_last_then_current() {
        let fixture = Fixture::new();
        assert_eq!(startup_config(&fixture.state, None), None);

        let alpha = fixture.project("alpha", "session: alpha\n");
        let beta = fixture.project("beta", "session: beta\n");
        let project = Project::open(&beta, &fixture.state);
        project.record_open(&fixture.state, 10).expect("record");
        assert_eq!(startup_config(&fixture.state, None), Some(beta.clone()));
        assert_eq!(
            startup_config(&fixture.state, Some(&alpha)),
            Some(alpha.clone())
        );

        std::fs::remove_file(fixture.state.path("last_project")).expect("drop last_project");
        assert_eq!(startup_config(&fixture.state, None), Some(beta.clone()));

        std::fs::remove_file(&beta).expect("remove beta config");
        assert_eq!(startup_config(&fixture.state, None), None);
    }

    #[test]
    fn session_list_is_recent_first_and_marks_missing() {
        let fixture = Fixture::new();
        let alpha = fixture.project("alpha", "session: alpha\n");
        let beta = fixture.project("beta", "session: beta\n");
        Project::open(&alpha, &fixture.state)
            .record_open(&fixture.state, 5)
            .expect("record alpha");
        Project::open(&beta, &fixture.state)
            .record_open(&fixture.state, 9)
            .expect("record beta");
        std::fs::remove_file(&alpha).expect("remove alpha config");
        let entries = session_entries(&fixture.state);
        let summary: Vec<(&str, bool)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.missing))
            .collect();
        assert_eq!(summary, [("beta", false), ("alpha", true)]);

        forget_session(&fixture.state, "alpha").expect("forget");
        assert_eq!(session_entries(&fixture.state).len(), 1);
        assert_eq!(Projects::load(&fixture.state).project_dir("alpha"), None);
    }

    #[test]
    fn line_numbers_are_found_in_messages() {
        assert_eq!(first_line_number("parse failed: line 12: bad"), Some(12));
        assert_eq!(first_line_number("no location"), None);
    }
}
