use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use pom_paths::StateDir;

/// Where one server version keeps its socket, pid and lock, and where it logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerPaths {
    pub dir: PathBuf,
    pub socket: PathBuf,
    pub pid: PathBuf,
    pub lock: PathBuf,
    pub log: PathBuf,
}

impl ServerPaths {
    pub fn new(dir: &Path, log_dir: &Path, version: &str) -> ServerPaths {
        ServerPaths {
            dir: dir.to_path_buf(),
            socket: dir.join(format!("{version}.sock")),
            pid: dir.join(format!("{version}.pid")),
            lock: dir.join(format!("{version}.lock")),
            log: log_dir.join(format!("{version}.log")),
        }
    }

    /// `$XDG_RUNTIME_DIR/pom-server`, else `/tmp/pom-server-<uid>`; the log under `$XDG_STATE_HOME/pom/server`.
    pub fn from_env(version: &str) -> ServerPaths {
        let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
            Some(runtime) => PathBuf::from(runtime).join("pom-server"),
            None => PathBuf::from(format!("/tmp/pom-server-{}", crate::peer::current_uid())),
        };
        ServerPaths::new(&dir, &StateDir::from_env().path("server"), version)
    }

    /// Creates the socket dir 0700, or checks an existing one is a real dir this user owns and nobody else
    /// can enter; a shared `/tmp` makes that check the only thing between another user and the socket.
    pub fn ensure_dir(&self) -> io::Result<()> {
        ensure_private_dir(&self.dir, crate::peer::current_uid())
    }
}

pub fn ensure_private_dir(dir: &Path, uid: u32) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::DirBuilder::new().mode(0o700).create(dir)?;
        }
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.file_type().is_dir() {
        return Err(io::Error::other(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    if metadata.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} belongs to another user", dir.display()),
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The versions whose server holds its lock in `dir`, each with the pid its pid file names.
pub fn running_versions(dir: &Path) -> Vec<(String, Option<u32>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut running: Vec<(String, Option<u32>)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = name.strip_suffix(".lock")?.to_string();
            let lock = std::fs::File::open(entry.path()).ok()?;
            match lock.try_lock_shared() {
                Err(std::fs::TryLockError::WouldBlock) => {}
                Ok(()) | Err(std::fs::TryLockError::Error(_)) => return None,
            }
            let pid = std::fs::read_to_string(dir.join(format!("{version}.pid")))
                .ok()
                .and_then(|text| text.trim().parse().ok());
            Some((version, pid))
        })
        .collect();
    running.sort();
    running
}
