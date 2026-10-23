//! The check every path from another machine passes before it touches the disk: relative, inside the workspace
//! once symlinks are resolved, and a regular file or folder.

use std::path::{Component, Path, PathBuf};

pub const MAX_PATH: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not a path the protocol allows: absolute, `..`, NUL, backslash, too long.
    Invalid,
    /// It resolves (through a symlink) to somewhere outside the workspace.
    OutsideWorkspace,
    NotFound,
    /// A device, fifo or socket, or a folder where a file was asked for.
    NotAFile,
}

impl Refusal {
    /// The error code a server answers with.
    pub fn code(self) -> &'static str {
        match self {
            Refusal::Invalid => "invalid_params",
            Refusal::OutsideWorkspace => "outside_workspace",
            Refusal::NotFound => "not_found",
            Refusal::NotAFile => "not_a_file",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardError {
    pub refusal: Refusal,
    pub message: String,
}

impl std::fmt::Display for GuardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GuardError {}

impl From<GuardError> for std::io::Error {
    fn from(error: GuardError) -> Self {
        let kind = match error.refusal {
            Refusal::Invalid => std::io::ErrorKind::InvalidInput,
            Refusal::OutsideWorkspace => std::io::ErrorKind::PermissionDenied,
            Refusal::NotFound => std::io::ErrorKind::NotFound,
            Refusal::NotAFile => std::io::ErrorKind::InvalidInput,
        };
        std::io::Error::new(kind, error.message)
    }
}

fn refuse(refusal: Refusal, message: impl Into<String>) -> GuardError {
    GuardError {
        refusal,
        message: message.into(),
    }
}

/// A workspace-relative path as the protocol carries it: '/'-separated, never leaving the root by its spelling.
/// The empty path is the root itself.
pub fn check_spelling(path: &str) -> Result<(), GuardError> {
    if path.len() > MAX_PATH {
        return Err(refuse(Refusal::Invalid, "the path is too long"));
    }
    if path.contains('\0') || path.contains('\\') {
        return Err(refuse(
            Refusal::Invalid,
            "the path has a NUL or a backslash",
        ));
    }
    if path.starts_with('/') {
        return Err(refuse(
            Refusal::Invalid,
            "the path must be relative to the workspace",
        ));
    }
    if path.is_empty() {
        return Ok(());
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(refuse(
            Refusal::Invalid,
            "the path has an empty, . or .. part",
        ));
    }
    Ok(())
}

/// What `path` names inside `root`, symlinks resolved; it must stay within `root` and be a file or folder.
pub fn resolve(root: &Path, path: &str) -> Result<PathBuf, GuardError> {
    check_spelling(path)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| refuse(Refusal::NotFound, format!("the workspace is gone: {error}")))?;
    let joined = if path.is_empty() {
        canonical_root.clone()
    } else {
        canonical_root.join(path)
    };
    let resolved = match joined.canonicalize() {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A dangling symlink is refused as one that escapes: where it points is not ours to say.
            if joined.symlink_metadata().is_ok() {
                return Err(refuse(
                    Refusal::OutsideWorkspace,
                    "the path leads outside the workspace",
                ));
            }
            return Err(refuse(Refusal::NotFound, "no such file"));
        }
        Err(error) => return Err(refuse(Refusal::NotFound, error.to_string())),
    };
    if !resolved.starts_with(&canonical_root) {
        return Err(refuse(
            Refusal::OutsideWorkspace,
            "the path leads outside the workspace",
        ));
    }
    let kind = resolved
        .metadata()
        .map_err(|error| refuse(Refusal::NotFound, error.to_string()))?
        .file_type();
    if !kind.is_file() && !kind.is_dir() {
        return Err(refuse(Refusal::NotAFile, "not a regular file or folder"));
    }
    Ok(resolved)
}

/// [`resolve`] for a path that must be a regular file.
pub fn resolve_file(root: &Path, path: &str) -> Result<PathBuf, GuardError> {
    let resolved = resolve(root, path)?;
    if resolved.is_dir() {
        return Err(refuse(Refusal::NotAFile, "a folder, not a file"));
    }
    Ok(resolved)
}

/// Whether `full` (already inside `canonical_root` by spelling) stays inside it once symlinks are resolved.
pub fn stays_inside(canonical_root: &Path, full: &Path) -> bool {
    full.canonicalize()
        .is_ok_and(|resolved| resolved.starts_with(canonical_root))
}

/// The '/'-separated path of `full` under `root`, or `None` when it is not under it.
pub fn relative(root: &Path, full: &Path) -> Option<String> {
    let rest = full.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in rest.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusal(root: &Path, path: &str) -> Option<Refusal> {
        resolve(root, path).err().map(|error| error.refusal)
    }

    #[test]
    fn spelling_that_could_leave_the_root_is_refused() {
        for path in [
            "../etc/passwd",
            "src/../../x",
            "/etc/passwd",
            "a\\b",
            "a\0b",
            "./a",
            "a//b",
            "a/",
        ] {
            assert_eq!(
                check_spelling(path).err().map(|error| error.refusal),
                Some(Refusal::Invalid),
                "{path:?}"
            );
        }
        assert!(check_spelling(&"a".repeat(MAX_PATH + 1)).is_err());
        assert!(check_spelling("src/main.rs").is_ok());
        assert!(check_spelling("").is_ok());
        assert!(check_spelling("..hidden/a..b").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_workspace_is_refused_and_one_inside_is_followed() -> std::io::Result<()>
    {
        let outside = tempfile::tempdir()?;
        std::fs::write(outside.path().join("secret"), "x")?;
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        std::fs::create_dir(root.join("src"))?;
        std::fs::write(root.join("src/main.rs"), "fn main() {}")?;
        std::os::unix::fs::symlink(outside.path().join("secret"), root.join("leak"))?;
        std::os::unix::fs::symlink(outside.path(), root.join("away"))?;
        std::os::unix::fs::symlink(root.join("src/main.rs"), root.join("alias.rs"))?;
        std::os::unix::fs::symlink(root.join("nowhere"), root.join("dangling"))?;
        assert_eq!(refusal(root, "leak"), Some(Refusal::OutsideWorkspace));
        assert_eq!(
            refusal(root, "away/secret"),
            Some(Refusal::OutsideWorkspace)
        );
        assert_eq!(refusal(root, "dangling"), Some(Refusal::OutsideWorkspace));
        assert_eq!(refusal(root, "missing.rs"), Some(Refusal::NotFound));
        let alias = resolve(root, "alias.rs").map_err(std::io::Error::from)?;
        assert_eq!(alias, root.canonicalize()?.join("src/main.rs"));
        assert!(resolve(root, "").is_ok());
        assert_eq!(
            resolve_file(root, "src").err().map(|error| error.refusal),
            Some(Refusal::NotAFile)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_not_a_file() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let fifo = temp.path().join("pipe");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status()?;
        assert!(status.success());
        assert_eq!(refusal(temp.path(), "pipe"), Some(Refusal::NotAFile));
        Ok(())
    }

    #[test]
    fn relative_paths_use_forward_slashes() {
        let root = Path::new("/work");
        assert_eq!(
            relative(root, Path::new("/work/src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(relative(root, Path::new("/work")), None);
        assert_eq!(relative(root, Path::new("/elsewhere/a")), None);
    }
}
