use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const DEFAULT_LOCK_DIR: &str = "/tmp/pom";
const PRIMARY: &str = "primary";

/// An exclusive advisory lock on `<dir>/<session>_<name>.lock`, held until dropped. The OS
/// releases it if the process dies, so a crash never leaves a stale lock behind.
#[derive(Debug)]
pub struct LockGuard {
    file: File,
    path: PathBuf,
}

impl LockGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Err(error) = self.file.unlock() {
            eprintln!("unlock {}: {error}", self.path.display());
        }
    }
}

pub fn lock_path(dir: &Path, session: &str, name: &str) -> PathBuf {
    dir.join(format!("{session}_{name}.lock"))
}

/// Non-blocking; `Ok(None)` means another process holds it.
pub fn try_acquire(dir: &Path, session: &str, name: &str) -> std::io::Result<Option<LockGuard>> {
    std::fs::create_dir_all(dir)?;
    let path = lock_path(dir, session, name);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(LockGuard { file, path })),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

/// The one process per session that runs background loops (reaper, auto-push, refresh-main).
/// The holder's pid is written into the file for diagnostics.
pub fn acquire_primary(dir: &Path, session: &str) -> std::io::Result<Option<LockGuard>> {
    let Some(mut guard) = try_acquire(dir, session, PRIMARY)? else {
        return Ok(None);
    };
    guard.file.set_len(0)?;
    writeln!(guard.file, "{}", std::process::id())?;
    Ok(Some(guard))
}

/// Removes every lock file of a session (used when a session is deleted).
pub fn remove_all(dir: &Path, session: &str) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let prefix = format!("{session}_");
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name.ends_with(".lock") {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_is_refused_until_release() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let first = try_acquire(temp.path(), "demo", "svc")?;
        assert!(first.is_some());
        assert!(try_acquire(temp.path(), "demo", "svc")?.is_none());
        assert!(try_acquire(temp.path(), "demo", "other")?.is_some());
        drop(first);
        assert!(try_acquire(temp.path(), "demo", "svc")?.is_some());
        Ok(())
    }

    #[test]
    fn primary_records_pid() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let guard = acquire_primary(temp.path(), "demo")?;
        let pid = std::fs::read_to_string(lock_path(temp.path(), "demo", "primary"))?;
        assert_eq!(pid.trim(), std::process::id().to_string());
        assert!(acquire_primary(temp.path(), "demo")?.is_none());
        drop(guard);
        Ok(())
    }

    #[test]
    fn lock_is_exclusive_across_processes() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let _guard = try_acquire(temp.path(), "demo", "svc")?;
        let path = lock_path(temp.path(), "demo", "svc");
        // Same flock the previous core takes, from another process: must fail while held.
        let status = std::process::Command::new("/usr/bin/python3")
            .args([
                "-c",
                "import fcntl,sys; f=open(sys.argv[1]); fcntl.flock(f, fcntl.LOCK_EX|fcntl.LOCK_NB)",
            ])
            .arg(&path)
            .stderr(std::process::Stdio::null())
            .status()?;
        assert!(!status.success());
        Ok(())
    }

    #[test]
    fn remove_all_only_touches_that_session() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        for name in [
            "demo_a.lock",
            "demo_primary.lock",
            "demo2_a.lock",
            "demo_note.txt",
        ] {
            std::fs::write(temp.path().join(name), "")?;
        }
        remove_all(temp.path(), "demo")?;
        let mut left: Vec<String> = std::fs::read_dir(temp.path())?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["demo2_a.lock", "demo_note.txt"]);
        remove_all(&temp.path().join("missing"), "demo")?;
        Ok(())
    }
}
