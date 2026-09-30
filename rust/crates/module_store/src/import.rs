use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How a tree gets from the store into a workspace (or back).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    /// Copy-on-write: the copy shares blocks until either side writes (APFS, Btrfs, XFS).
    #[default]
    Clone,
    /// Every file is the store's file under a second name (any filesystem, same volume).
    HardLink,
    Copy,
}

impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Method::Clone => "Copy-on-write clone",
            Method::HardLink => "Hard links",
            Method::Copy => "Copy",
        }
    }
}

/// The best method between the store (`from`) and a workspace folder (`to`), found by trying it on a
/// throwaway file rather than guessing from the filesystem name.
pub fn probe(from: &Path, to: &Path) -> Method {
    let stamp = format!(".pom-probe-{}", std::process::id());
    let source = from.join(&stamp);
    let target = to.join(&stamp);
    if fs::create_dir_all(from).is_err() || fs::write(&source, b"probe").is_err() {
        return Method::Copy;
    }
    let method = if clone_file(&source, &target).is_ok() {
        Method::Clone
    } else if fs::hard_link(&source, &target).is_ok() {
        Method::HardLink
    } else {
        Method::Copy
    };
    for path in [&source, &target] {
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("module store: remove {}: {error}", path.display());
            }
        }
    }
    method
}

/// Recreate the tree at `source` as `target` (which must not exist), skipping top-level entries named in
/// `skip` (build caches that should not be shared).
pub fn import(source: &Path, target: &Path, method: Method, skip: &[&str]) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(target_os = "macos")]
    if method == Method::Clone {
        clone_file(source, target)?;
        for name in skip {
            let cache = target.join(name);
            if cache.exists() {
                fs::remove_dir_all(&cache)?;
            }
        }
        return Ok(());
    }
    walk(source, target, method, skip)
}

fn walk(source: &Path, target: &Path, method: Method, skip: &[&str]) -> io::Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if skip.iter().any(|skipped| name == *skipped) {
            continue;
        }
        let from = entry.path();
        let to = target.join(&name);
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&from)?, &to)?;
        } else if kind.is_dir() {
            walk(&from, &to, method, &[])?;
        } else if kind.is_file() {
            match method {
                Method::Clone => clone_file(&from, &to)?,
                Method::HardLink => fs::hard_link(&from, &to)?,
                Method::Copy => {
                    fs::copy(&from, &to)?;
                }
            }
        }
    }
    let permissions = fs::metadata(source)?.permissions();
    fs::set_permissions(target, permissions)
}

#[cfg(target_os = "macos")]
fn clone_file(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
    let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
    // SAFETY: both are valid NUL-terminated paths; clonefile only reads them.
    let result = unsafe { libc::clonefile(source.as_ptr(), target.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn clone_file(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::io::AsRawFd;
    let from = fs::File::open(source)?;
    let mode = from.metadata()?.permissions().mode();
    let to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    // SAFETY: both descriptors are open for the duration of the call.
    let result = unsafe { libc::ioctl(to.as_raw_fd(), libc::FICLONE, from.as_raw_fd()) };
    if result != 0 {
        let error = io::Error::last_os_error();
        drop(to);
        if let Err(cleanup) = fs::remove_file(target) {
            eprintln!("module store: remove {}: {cleanup}", target.display());
        }
        return Err(error);
    }
    fs::set_permissions(target, fs::Permissions::from_mode(mode))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clone_file(_source: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copy-on-write clones are not supported here",
    ))
}

/// Total bytes of the regular files under `root` (each hard-linked file counted once per name).
pub fn tree_size(root: &Path) -> u64 {
    let mut total = 0;
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                total += entry.metadata().map_or(0, |meta| meta.len());
            }
        }
    }
    total
}

/// Make every file under `root` read-only, so a tool that edits a hard-linked file in place fails loudly
/// instead of changing it for every workspace that shares it.
pub fn protect(root: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let path = entry.path();
                let mut permissions = fs::metadata(&path)?.permissions();
                permissions.set_mode(permissions.mode() & !0o222);
                fs::set_permissions(&path, permissions)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn sample(root: &Path) {
        fs::create_dir_all(root.join("pkg/lib")).unwrap();
        fs::create_dir_all(root.join(".cache/babel")).unwrap();
        fs::write(root.join("pkg/index.js"), "module.exports = 1").unwrap();
        fs::write(root.join("pkg/lib/a.js"), "a").unwrap();
        fs::write(root.join(".cache/babel/x"), "cache").unwrap();
        fs::create_dir_all(root.join(".bin")).unwrap();
        std::os::unix::fs::symlink("../pkg/index.js", root.join(".bin/pkg")).unwrap();
    }

    #[test]
    fn every_method_recreates_the_tree_without_the_skipped_cache() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        sample(&source);
        for method in [Method::Clone, Method::HardLink, Method::Copy] {
            let target = dir.path().join(format!("{method:?}"));
            if method == Method::Clone && probe(dir.path(), dir.path()) != Method::Clone {
                continue;
            }
            import(&source, &target, method, &[".cache"]).unwrap();
            assert_eq!(
                fs::read_to_string(target.join("pkg/lib/a.js")).unwrap(),
                "a",
                "{method:?}"
            );
            assert!(!target.join(".cache").exists(), "{method:?}");
            assert_eq!(
                fs::read_link(target.join(".bin/pkg")).unwrap(),
                Path::new("../pkg/index.js"),
                "{method:?}"
            );
            let linked = fs::metadata(target.join("pkg/index.js")).unwrap().ino()
                == fs::metadata(source.join("pkg/index.js")).unwrap().ino();
            assert_eq!(linked, method == Method::HardLink, "{method:?}");
        }
    }

    #[test]
    fn protect_makes_files_read_only_but_still_removable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tree");
        sample(&root);
        protect(&root).unwrap();
        assert!(fs::write(root.join("pkg/index.js"), "changed").is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tree_size_adds_up_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        sample(dir.path());
        assert_eq!(tree_size(dir.path()), 18 + 1 + 5);
    }
}
