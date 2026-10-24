use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
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

/// Where the user's app config lives (`$POMELO_CONFIG_DIR`, else `~/.config/pomelo`): settings, keymap,
/// snippets, saved layouts. Tests point the variable at a scratch folder so they never touch the real one.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("POMELO_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    Some(home_dir()?.join(".config").join("pomelo"))
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
    // Per call, not just per process: two threads writing one file would otherwise share a temp
    // file, and the second rename fails once the first has moved it.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = parent.join(format!(".{file_name}.{}.{call}.tmp", std::process::id()));
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

/// Creates `dir` 0700 when missing, then checks it with [`check_private_dir`].
pub fn ensure_private_dir(dir: &Path, uid: u32) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Two holders starting at once both see it missing; the one that loses still uses it.
            match std::fs::DirBuilder::new().mode(0o700).create(dir) {
                Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                    return Err(error)
                }
                _ => {}
            }
        }
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    check_private_dir(dir, uid)
}

/// Checks an existing socket folder is a real directory `uid` owns, tightening it to 0700 if it is looser;
/// in a shared `/tmp` that is all that keeps another user from planting or reaching the sockets inside.
pub fn check_private_dir(dir: &Path, uid: u32) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.file_type().is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    if metadata.uid() != uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} belongs to another user", dir.display()),
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn threads_writing_one_file_at_once_all_succeed() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join(".env.local");
        std::thread::scope(|scope| {
            let writers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..50).try_for_each(|_| write_atomic(&path, b"PORT=3000\n", 0o600))
                    })
                })
                .collect();
            writers
                .into_iter()
                .try_for_each(|writer| writer.join().expect("writer thread"))
        })?;
        assert_eq!(std::fs::read(&path)?, b"PORT=3000\n");
        assert_eq!(std::fs::read_dir(temp.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn threads_creating_one_socket_dir_at_once_all_succeed() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let uid = std::fs::metadata(temp.path())?.uid();
        for round in 0..50 {
            let dir = temp.path().join(format!("sockets-{round}"));
            std::thread::scope(|scope| {
                let creators: Vec<_> = (0..8)
                    .map(|_| scope.spawn(|| ensure_private_dir(&dir, uid)))
                    .collect();
                creators
                    .into_iter()
                    .try_for_each(|creator| creator.join().expect("creator thread"))
            })?;
        }
        Ok(())
    }

    #[test]
    fn a_socket_dir_owned_by_someone_else_or_open_to_others_is_not_trusted() -> std::io::Result<()>
    {
        let temp = tempfile::tempdir()?;
        let uid = std::fs::metadata(temp.path())?.uid();
        let dir = temp.path().join("sockets");
        ensure_private_dir(&dir, uid)?;
        assert_eq!(std::fs::metadata(&dir)?.permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777))?;
        check_private_dir(&dir, uid)?;
        assert_eq!(std::fs::metadata(&dir)?.permissions().mode() & 0o777, 0o700);
        let refused = ensure_private_dir(&dir, uid + 1).expect_err("another owner");
        assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);

        let link = temp.path().join("linked");
        std::os::unix::fs::symlink(&dir, &link)?;
        assert!(check_private_dir(&link, uid).is_err());
        assert_eq!(
            check_private_dir(&temp.path().join("missing"), uid)
                .expect_err("missing")
                .kind(),
            std::io::ErrorKind::NotFound
        );
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
