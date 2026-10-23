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
    /// Not to be changed from here: under `.git`, or outside the project config in the main workspace.
    ReadOnly,
}

impl Refusal {
    /// The error code a server answers with.
    pub fn code(self) -> &'static str {
        match self {
            Refusal::Invalid => "invalid_params",
            Refusal::OutsideWorkspace => "outside_workspace",
            Refusal::NotFound => "not_found",
            Refusal::NotAFile => "not_a_file",
            Refusal::ReadOnly => "read_only",
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
            Refusal::ReadOnly => std::io::ErrorKind::PermissionDenied,
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

/// Whether parent folders missing on the way to a new path are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parents {
    Create,
    MustExist,
}

/// A path that is about to be written, made, moved or removed: spelled inside the workspace and not under `.git`.
fn check_change(path: &str) -> Result<(), GuardError> {
    check_spelling(path)?;
    if path.is_empty() {
        return Err(refuse(
            Refusal::Invalid,
            "the workspace itself cannot be changed",
        ));
    }
    if path.split('/').any(|part| part == ".git") {
        return Err(refuse(
            Refusal::ReadOnly,
            "files under .git are not changed from here",
        ));
    }
    Ok(())
}

fn canonical_root(root: &Path) -> Result<PathBuf, GuardError> {
    root.canonicalize()
        .map_err(|error| refuse(Refusal::NotFound, format!("the workspace is gone: {error}")))
}

/// `next` resolved, which must be a folder inside `canonical_root`.
fn inside_folder(canonical_root: &Path, next: &Path) -> Result<PathBuf, GuardError> {
    let resolved = next
        .canonicalize()
        .map_err(|error| refuse(Refusal::NotFound, error.to_string()))?;
    if !resolved.starts_with(canonical_root) {
        return Err(refuse(
            Refusal::OutsideWorkspace,
            "the path leads outside the workspace",
        ));
    }
    if !resolved.is_dir() {
        return Err(refuse(
            Refusal::NotAFile,
            "a file where a folder was expected",
        ));
    }
    Ok(resolved)
}

/// The folder `path` goes in, resolved inside the workspace level by level, with its own name. Missing folders
/// are made one at a time when asked, and each is looked at again once made, so a link swapped in meanwhile
/// cannot lead the rest of the way out.
pub fn resolve_parent(
    root: &Path,
    path: &str,
    parents: Parents,
) -> Result<(PathBuf, String), GuardError> {
    check_change(path)?;
    let canonical_root = canonical_root(root)?;
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default().to_string();
    let mut folder = canonical_root.clone();
    for part in parts {
        let next = folder.join(part);
        match next.symlink_metadata() {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if parents == Parents::MustExist {
                    return Err(refuse(Refusal::NotFound, "no such folder"));
                }
                match std::fs::create_dir(&next) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(refuse(Refusal::NotFound, error.to_string())),
                }
            }
            Err(error) => return Err(refuse(Refusal::NotFound, error.to_string())),
        }
        folder = inside_folder(&canonical_root, &next)?;
    }
    Ok((folder, name))
}

/// Where the contents of `path` are written: the file it names (through a link that stays inside), or a new
/// file in a folder inside the workspace.
pub fn resolve_write(root: &Path, path: &str, parents: Parents) -> Result<PathBuf, GuardError> {
    let (folder, name) = resolve_parent(root, path, parents)?;
    let target = folder.join(&name);
    match target.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(target),
        Err(error) => Err(refuse(Refusal::NotFound, error.to_string())),
        Ok(_) => {
            let resolved = resolve_file(root, path)?;
            let canonical_root = canonical_root(root)?;
            let inside = resolved.strip_prefix(&canonical_root).unwrap_or(&resolved);
            if inside.components().any(|part| part.as_os_str() == ".git") {
                return Err(refuse(
                    Refusal::ReadOnly,
                    "files under .git are not changed from here",
                ));
            }
            Ok(resolved)
        }
    }
}

/// An existing entry about to be moved, copied or removed: the entry itself (a link stays a link), in a folder
/// inside the workspace, and a file, folder or link.
pub fn resolve_entry(root: &Path, path: &str) -> Result<PathBuf, GuardError> {
    let (folder, name) = resolve_parent(root, path, Parents::MustExist)?;
    let entry = folder.join(&name);
    let kind = entry
        .symlink_metadata()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => refuse(Refusal::NotFound, "no such file"),
            _ => refuse(Refusal::NotFound, error.to_string()),
        })?
        .file_type();
    if !kind.is_file() && !kind.is_dir() && !kind.is_symlink() {
        return Err(refuse(Refusal::NotAFile, "not a regular file or folder"));
    }
    Ok(entry)
}

/// A new entry's place, which must still be free.
pub fn resolve_new(root: &Path, path: &str, parents: Parents) -> Result<PathBuf, GuardError> {
    let (folder, name) = resolve_parent(root, path, parents)?;
    let target = folder.join(&name);
    if target.symlink_metadata().is_ok() {
        return Err(refuse(Refusal::Invalid, "something already has that name"));
    }
    Ok(target)
}

/// In the main workspace only the project's config may change; elsewhere anything the guard lets through.
pub fn check_scope(target: &Path, writable_only: Option<&Path>) -> Result<(), GuardError> {
    let Some(allowed) = writable_only else {
        return Ok(());
    };
    let allowed = allowed
        .canonicalize()
        .unwrap_or_else(|_| allowed.to_path_buf());
    if target == allowed {
        return Ok(());
    }
    Err(refuse(
        Refusal::ReadOnly,
        "main is read-only: make changes in a branch workspace",
    ))
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

    #[test]
    fn a_new_file_in_new_folders_is_made_inside_the_workspace() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        assert_eq!(
            resolve_write(root, "a/b/c.rs", Parents::MustExist)
                .err()
                .map(|error| error.refusal),
            Some(Refusal::NotFound)
        );
        let target =
            resolve_write(root, "a/b/c.rs", Parents::Create).map_err(std::io::Error::from)?;
        assert_eq!(target, root.canonicalize()?.join("a/b/c.rs"));
        assert!(root.join("a/b").is_dir());
        assert!(!target.exists(), "only the folders are made");
        std::fs::write(&target, "x")?;
        assert_eq!(
            resolve_write(root, "a/b/c.rs", Parents::MustExist).ok(),
            Some(target)
        );
        Ok(())
    }

    #[test]
    fn nothing_under_git_or_the_workspace_itself_is_changed() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        std::fs::create_dir_all(root.join("api/.git"))?;
        std::fs::write(root.join("api/.git/HEAD"), "ref")?;
        for path in [".git/config", "api/.git/HEAD", "api/.git"] {
            assert_eq!(
                resolve_write(root, path, Parents::Create)
                    .err()
                    .map(|error| error.refusal),
                Some(Refusal::ReadOnly),
                "{path}"
            );
            assert_eq!(
                resolve_entry(root, path).err().map(|error| error.refusal),
                Some(Refusal::ReadOnly),
                "{path}"
            );
        }
        assert_eq!(
            resolve_entry(root, "").err().map(|error| error.refusal),
            Some(Refusal::Invalid)
        );
        assert!(!root.join(".git").exists(), "nothing was made on the way");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_folder_out_of_the_workspace_is_never_written_through() -> std::io::Result<()> {
        let outside = tempfile::tempdir()?;
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        std::os::unix::fs::symlink(outside.path(), root.join("away"))?;
        std::os::unix::fs::symlink(outside.path().join("secret"), root.join("leak"))?;
        std::fs::write(outside.path().join("secret"), "x")?;
        for path in ["away/new.rs", "away/deep/new.rs"] {
            assert_eq!(
                resolve_write(root, path, Parents::Create)
                    .err()
                    .map(|error| error.refusal),
                Some(Refusal::OutsideWorkspace),
                "{path}"
            );
        }
        assert!(
            !outside.path().join("deep").exists(),
            "no folder made out there"
        );
        assert_eq!(
            resolve_write(root, "leak", Parents::Create)
                .err()
                .map(|error| error.refusal),
            Some(Refusal::OutsideWorkspace)
        );
        let link = resolve_entry(root, "leak").map_err(std::io::Error::from)?;
        assert_eq!(
            link,
            root.canonicalize()?.join("leak"),
            "the link itself, not where it points"
        );
        assert_eq!(
            resolve_new(root, "leak", Parents::Create)
                .err()
                .map(|error| error.refusal),
            Some(Refusal::Invalid)
        );
        Ok(())
    }

    #[test]
    fn main_writes_only_its_config() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        std::fs::write(root.join("pom.yml"), "session: x\n")?;
        let config = root.join("pom.yml");
        assert!(check_scope(&config, Some(&config)).is_ok());
        assert_eq!(
            check_scope(&root.join("api.rs"), Some(&config))
                .err()
                .map(|error| error.refusal),
            Some(Refusal::ReadOnly)
        );
        assert!(check_scope(&root.join("api.rs"), None).is_ok());
        Ok(())
    }
}
