use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pom_paths::{write_atomic, StateDir};
use serde::{Deserialize, Serialize};

const SESSIONS_FILE: &str = "sessions.json";
const PROJECTS_FILE: &str = "registry.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub last_used: i64,
}

/// Known sessions and which one is current (`sessions.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sessions {
    #[serde(default)]
    pub current: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub sessions: Vec<Session>,
}

fn null_as_empty<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

impl Sessions {
    /// A missing or unreadable file is an empty registry, never an error: the app must still boot.
    pub fn load(state: &StateDir) -> Sessions {
        read_json(&state.path(SESSIONS_FILE)).unwrap_or_default()
    }

    pub fn save(&self, state: &StateDir) -> std::io::Result<()> {
        write_json(&state.path(SESSIONS_FILE), self)
    }

    pub fn get(&self, name: &str) -> Option<&Session> {
        self.sessions.iter().find(|session| session.name == name)
    }

    /// The named current session, or else the most recently used one.
    pub fn current_session(&self) -> Option<&Session> {
        self.get(&self.current).or_else(|| {
            self.sessions.iter().reduce(|best, session| {
                if session.last_used > best.last_used {
                    session
                } else {
                    best
                }
            })
        })
    }

    /// Records use of a session (adding it if new) and makes it current.
    pub fn touch(&mut self, name: &str, path: &str, now: i64) {
        match self
            .sessions
            .iter_mut()
            .find(|session| session.name == name)
        {
            Some(session) => {
                session.path = path.to_string();
                session.last_used = now;
            }
            None => self.sessions.push(Session {
                name: name.to_string(),
                path: path.to_string(),
                last_used: now,
            }),
        }
        self.current = name.to_string();
    }

    pub fn remove(&mut self, name: &str) {
        self.sessions.retain(|session| session.name != name);
        if self.current == name {
            self.current.clear();
        }
    }
}

/// Session name to absolute project directory (`registry.json`), used to find a session's
/// config from anywhere (CLI, MCP, holders).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projects {
    #[serde(default, deserialize_with = "null_as_empty_map")]
    pub projects: BTreeMap<String, String>,
}

fn null_as_empty_map<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<BTreeMap<String, String>>::deserialize(deserializer)?.unwrap_or_default())
}

impl Projects {
    pub fn load(state: &StateDir) -> Projects {
        read_json(&state.path(PROJECTS_FILE)).unwrap_or_default()
    }

    pub fn project_dir(&self, session: &str) -> Option<PathBuf> {
        self.projects.get(session).map(PathBuf::from)
    }

    /// Stores the absolute project dir; writes only when it changed.
    pub fn register(state: &StateDir, session: &str, project_dir: &Path) -> std::io::Result<()> {
        let absolute = std::path::absolute(project_dir)?;
        let absolute = absolute.to_string_lossy().into_owned();
        let mut registry = Projects::load(state);
        if registry.projects.get(session) == Some(&absolute) {
            return Ok(());
        }
        registry.projects.insert(session.to_string(), absolute);
        write_json(&state.path(PROJECTS_FILE), &registry)
    }

    pub fn unregister(state: &StateDir, session: &str) -> std::io::Result<()> {
        let mut registry = Projects::load(state);
        if registry.projects.remove(session).is_none() {
            return Ok(());
        }
        write_json(&state.path(PROJECTS_FILE), &registry)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    write_atomic(path, text.as_bytes(), 0o644)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (tempfile::TempDir, StateDir) {
        let temp = tempfile::tempdir().expect("temp dir");
        let state = StateDir::new(temp.path().join("pom"));
        (temp, state)
    }

    #[test]
    fn missing_or_corrupt_file_is_empty() -> std::io::Result<()> {
        let (_temp, state) = state();
        assert_eq!(Sessions::load(&state), Sessions::default());
        std::fs::create_dir_all(state.root())?;
        std::fs::write(state.path("sessions.json"), "{not json")?;
        assert_eq!(Sessions::load(&state), Sessions::default());
        std::fs::write(
            state.path("sessions.json"),
            r#"{"current":"a","sessions":null}"#,
        )?;
        assert_eq!(Sessions::load(&state).current, "a");
        Ok(())
    }

    #[test]
    fn touch_remove_and_current_fallback() -> std::io::Result<()> {
        let (_temp, state) = state();
        let mut sessions = Sessions::default();
        sessions.touch("alpha", "/p/alpha", 10);
        sessions.touch("beta", "/p/beta", 30);
        sessions.touch("alpha", "/p/alpha2", 20);
        assert_eq!(sessions.current, "alpha");
        assert_eq!(sessions.sessions.len(), 2);
        assert_eq!(
            sessions.get("alpha").map(|s| s.path.as_str()),
            Some("/p/alpha2")
        );

        sessions.remove("alpha");
        assert_eq!(sessions.current, "");
        sessions.touch("gamma", "/p/gamma", 5);
        sessions.current.clear();
        assert_eq!(
            sessions.current_session().map(|s| s.name.as_str()),
            Some("beta")
        );

        sessions.save(&state)?;
        assert_eq!(Sessions::load(&state), sessions);
        Ok(())
    }

    #[test]
    fn reads_the_previous_core_format() -> std::io::Result<()> {
        let (_temp, state) = state();
        std::fs::create_dir_all(state.root())?;
        std::fs::write(
            state.path("sessions.json"),
            r#"{
  "current": "demo",
  "sessions": [
    { "name": "demo", "path": "/Users/x/pom/demo", "last_used": 1700000000 }
  ]
}"#,
        )?;
        let sessions = Sessions::load(&state);
        assert_eq!(
            sessions.current_session().map(|s| s.last_used),
            Some(1_700_000_000)
        );
        Ok(())
    }

    #[test]
    fn project_registry_round_trip() -> std::io::Result<()> {
        let (temp, state) = state();
        Projects::register(&state, "demo", temp.path())?;
        assert_eq!(
            Projects::load(&state).project_dir("demo"),
            Some(temp.path().to_path_buf())
        );
        Projects::unregister(&state, "demo")?;
        assert_eq!(Projects::load(&state).project_dir("demo"), None);
        Projects::unregister(&state, "missing")?;
        Ok(())
    }
}
