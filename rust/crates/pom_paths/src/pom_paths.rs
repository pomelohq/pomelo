use std::io::Write;
use std::path::{Path, PathBuf};

/// Root of Pomelo's machine-wide state (`$XDG_STATE_HOME/pom`, else `~/.local/state/pom`).
/// Passed explicitly everywhere so tests can point it at a temp dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        StateDir { root: root.into() }
    }

    pub fn from_env() -> Self {
        let base = match std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
            Some(xdg) => PathBuf::from(xdg),
            None => match home_dir() {
                Some(home) => home.join(".local").join("state"),
                None => PathBuf::from("/tmp"),
            },
        };
        StateDir::new(base.join("pom"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }
}

/// Where new sessions' project folders are created (`$POM_SESSIONS_ROOT`, else `~/pom`).
pub fn sessions_root() -> PathBuf {
    if let Some(root) = std::env::var_os("POM_SESSIONS_ROOT").filter(|v| !v.is_empty()) {
        return PathBuf::from(root);
    }
    home_dir().map_or_else(|| PathBuf::from("/tmp/pom"), |home| home.join("pom"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Writes through a sibling temp file and renames, so a reader never sees a half-written file
/// and a crash mid-write leaves the previous version intact.
pub fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        if let Err(error) = std::fs::remove_file(&temp) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn atomic_write_replaces_and_sets_mode() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("nested/state.json");
        write_atomic(&path, b"one", 0o600)?;
        write_atomic(&path, b"two", 0o600)?;
        assert_eq!(std::fs::read(&path)?, b"two");
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        let leftovers = std::fs::read_dir(temp.path().join("nested"))?.count();
        assert_eq!(leftovers, 1);
        Ok(())
    }

    #[test]
    fn state_paths_join_under_root() {
        let state = StateDir::new("/x/pom");
        assert_eq!(
            state.path("sessions.json"),
            PathBuf::from("/x/pom/sessions.json")
        );
    }
}
