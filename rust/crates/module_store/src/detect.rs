use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Manager {
    Npm,
    Yarn,
    Bun,
}

impl Manager {
    pub fn label(self) -> &'static str {
        match self {
            Manager::Npm => "npm",
            Manager::Yarn => "yarn",
            Manager::Bun => "bun",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detection {
    Store {
        manager: Manager,
        lockfile: PathBuf,
    },
    /// The package manager shares packages itself (pnpm's store, Yarn's hard links or Plug'n'Play).
    SelfManaged(&'static str),
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Installer {
    Pnpm,
    Yarn,
    Npm,
    Bun,
}

impl Installer {
    const ORDER: [Installer; 4] = [
        Installer::Pnpm,
        Installer::Yarn,
        Installer::Npm,
        Installer::Bun,
    ];

    fn from_name(name: &str) -> Option<Installer> {
        match name {
            "pnpm" => Some(Installer::Pnpm),
            "yarn" => Some(Installer::Yarn),
            "npm" => Some(Installer::Npm),
            "bun" => Some(Installer::Bun),
            _ => None,
        }
    }

    fn lockfile(self, worktree: &Path) -> Option<PathBuf> {
        let names: &[&str] = match self {
            Installer::Pnpm => &["pnpm-lock.yaml"],
            Installer::Yarn => &["yarn.lock"],
            Installer::Npm => &["package-lock.json"],
            Installer::Bun => &["bun.lock", "bun.lockb"],
        };
        names
            .iter()
            .map(|name| worktree.join(name))
            .find(|path| path.is_file())
    }

    fn installs_with(self, argument: Option<&str>) -> bool {
        match (self, argument) {
            (Installer::Yarn, None | Some("install")) => true,
            (Installer::Yarn, Some(flag)) => flag.starts_with('-'),
            (Installer::Npm, Some(command)) => matches!(command, "ci" | "install" | "i"),
            (Installer::Pnpm | Installer::Bun, Some(command)) => {
                matches!(command, "install" | "i")
            }
            _ => false,
        }
    }
}

/// The package manager the first install command in `setup` runs (`yarn`, `npm ci`, `pnpm i`...).
fn setup_installer(setup: &[String]) -> Option<Installer> {
    setup.iter().find_map(|step| {
        step.split(['&', '|', ';', '\n', '(', ')'])
            .find_map(|command| {
                let mut words = command.split_whitespace();
                let installer = Installer::from_name(words.next()?)?;
                installer.installs_with(words.next()).then_some(installer)
            })
    })
}

/// `"packageManager": "yarn@1.22.19"` in package.json.
fn declared_installer(worktree: &Path) -> Option<Installer> {
    let bytes = std::fs::read(worktree.join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let declared = manifest.get("packageManager")?.as_str()?;
    Installer::from_name(declared.split('@').next()?.trim())
}

/// Which manager owns `worktree`'s node_modules. With more than one lockfile, the install command in the
/// repo's `setup` steps decides, then `packageManager` in package.json, then pnpm, Yarn, npm, Bun.
pub fn detect(worktree: &Path, setup: &[String]) -> Detection {
    let present: Vec<(Installer, PathBuf)> = Installer::ORDER
        .into_iter()
        .filter_map(|installer| Some((installer, installer.lockfile(worktree)?)))
        .collect();
    let preferred = || {
        let wanted = setup_installer(setup)
            .filter(|wanted| present.iter().any(|(installer, _)| installer == wanted));
        let wanted = wanted.or_else(|| {
            declared_installer(worktree)
                .filter(|wanted| present.iter().any(|(installer, _)| installer == wanted))
        })?;
        present.iter().find(|(installer, _)| *installer == wanted)
    };
    let chosen = if present.len() > 1 { preferred() } else { None };
    let Some((installer, lockfile)) = chosen.or(present.first()).cloned() else {
        return Detection::None;
    };
    let manager = match installer {
        Installer::Pnpm => return Detection::SelfManaged("pnpm"),
        Installer::Yarn => return yarn(worktree, lockfile),
        Installer::Npm => Manager::Npm,
        Installer::Bun => Manager::Bun,
    };
    Detection::Store { manager, lockfile }
}

fn yarn(worktree: &Path, lockfile: PathBuf) -> Detection {
    if let Ok(rc) = std::fs::read_to_string(worktree.join(".yarnrc.yml")) {
        let setting = |key: &str| {
            rc.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.trim() == key).then(|| value.trim().trim_matches(['"', '\'']).to_string())
            })
        };
        // Yarn 2+ installs Plug'n'Play (no node_modules) unless told otherwise.
        if setting("nodeLinker").as_deref() != Some("node-modules") {
            return Detection::SelfManaged("yarn plug'n'play");
        }
        if setting("nmMode").is_some_and(|mode| mode.starts_with("hardlinks")) {
            return Detection::SelfManaged("yarn hard links");
        }
    }
    Detection::Store {
        manager: Manager::Yarn,
        lockfile,
    }
}

/// Same lockfile, patches, package manager, Node major version and platform -> the same `node_modules`,
/// native builds and applied patches included.
pub fn key(lockfile: &[u8], patches: &[u8], manager: Manager, node_major: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    for part in [
        lockfile,
        patches,
        manager.label().as_bytes(),
        node_major.unwrap_or("").as_bytes(),
        std::env::consts::OS.as_bytes(),
        std::env::consts::ARCH.as_bytes(),
    ] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The `patches/` folder patch-package applies on install, as bytes to key on: a branch that adds a patch
/// must not get a copy without it (the shared files are read-only, so applying it would fail).
pub fn patches(worktree: &Path) -> Vec<u8> {
    let mut files = Vec::new();
    let mut pending = vec![worktree.join("patches")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                files.push((path, bytes));
            }
        }
    }
    files.sort();
    let mut out = Vec::new();
    for (path, bytes) in files {
        if let Ok(relative) = path.strip_prefix(worktree) {
            out.extend_from_slice(relative.to_string_lossy().as_bytes());
        }
        out.push(0);
        out.extend_from_slice(&bytes);
    }
    out
}

/// `v20.11.1` -> `20`.
pub fn node_major(version: &str) -> Option<String> {
    let major = version.trim().trim_start_matches('v').split('.').next()?;
    (!major.is_empty() && major.chars().all(|c| c.is_ascii_digit())).then(|| major.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_managers_and_leaves_self_sharing_ones_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(detect(root, &[]), Detection::None);
        std::fs::write(root.join("package-lock.json"), "{}").unwrap();
        assert!(matches!(
            detect(root, &[]),
            Detection::Store {
                manager: Manager::Npm,
                ..
            }
        ));
        std::fs::write(root.join("yarn.lock"), "").unwrap();
        assert!(matches!(
            detect(root, &[]),
            Detection::Store {
                manager: Manager::Yarn,
                ..
            }
        ));
        std::fs::write(
            root.join(".yarnrc.yml"),
            "yarnPath: .yarn/releases/yarn.cjs\n",
        )
        .unwrap();
        assert_eq!(
            detect(root, &[]),
            Detection::SelfManaged("yarn plug'n'play")
        );
        std::fs::write(root.join(".yarnrc.yml"), "nodeLinker: node-modules\n").unwrap();
        assert!(matches!(detect(root, &[]), Detection::Store { .. }));
        std::fs::write(
            root.join(".yarnrc.yml"),
            "nodeLinker: \"node-modules\"\nnmMode: hardlinks-global\n",
        )
        .unwrap();
        assert_eq!(detect(root, &[]), Detection::SelfManaged("yarn hard links"));
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect(root, &[]), Detection::SelfManaged("pnpm"));
    }

    #[test]
    fn with_several_lockfiles_the_setup_install_command_then_package_json_decides() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(root.join("yarn.lock"), "").unwrap();
        std::fs::write(root.join("package-lock.json"), "{}").unwrap();
        let steps = |steps: &[&str]| {
            steps
                .iter()
                .map(|step| step.to_string())
                .collect::<Vec<_>>()
        };
        let manager = |setup: &[String]| match detect(root, setup) {
            Detection::Store { manager, .. } => manager.label(),
            Detection::SelfManaged(name) => name,
            Detection::None => "none",
        };
        assert_eq!(manager(&[]), "pnpm");
        assert_eq!(manager(&steps(&["yarn install --frozen-lockfile"])), "yarn");
        assert_eq!(manager(&steps(&["cd web && yarn", "yarn build"])), "yarn");
        assert_eq!(manager(&steps(&["yarn build", "npm ci"])), "npm");
        assert_eq!(manager(&steps(&["npm run build; pnpm i"])), "pnpm");
        assert_eq!(manager(&steps(&["bun install"])), "pnpm", "no bun lockfile");

        std::fs::write(
            root.join("package.json"),
            r#"{"name": "web", "packageManager": "yarn@1.22.19"}"#,
        )
        .unwrap();
        assert_eq!(manager(&[]), "yarn");
        assert_eq!(manager(&steps(&["npm install"])), "npm");

        std::fs::remove_file(root.join("pnpm-lock.yaml")).unwrap();
        std::fs::remove_file(root.join("yarn.lock")).unwrap();
        assert_eq!(manager(&steps(&["yarn"])), "npm", "a single lockfile wins");
    }

    #[test]
    fn patches_are_read_in_a_stable_order() {
        let dir = tempfile::tempdir().unwrap();
        assert!(patches(dir.path()).is_empty());
        std::fs::create_dir_all(dir.path().join("patches/nested")).unwrap();
        std::fs::write(dir.path().join("patches/b.patch"), "b").unwrap();
        std::fs::write(dir.path().join("patches/nested/a.patch"), "a").unwrap();
        let first = patches(dir.path());
        assert_eq!(first, patches(dir.path()));
        std::fs::write(dir.path().join("patches/b.patch"), "changed").unwrap();
        assert_ne!(first, patches(dir.path()));
    }

    #[test]
    fn the_key_changes_with_the_node_major_and_the_manager() {
        let base = key(b"lock", b"", Manager::Npm, Some("20"));
        assert_eq!(base.len(), 16);
        assert_eq!(base, key(b"lock", b"", Manager::Npm, Some("20")));
        assert_ne!(base, key(b"lock", b"", Manager::Npm, Some("22")));
        assert_ne!(base, key(b"lock", b"", Manager::Bun, Some("20")));
        assert_ne!(base, key(b"lock2", b"", Manager::Npm, Some("20")));
        assert_ne!(base, key(b"lock", b"p", Manager::Npm, Some("20")));
        assert_eq!(node_major("v20.11.1\n").as_deref(), Some("20"));
        assert_eq!(node_major("command not found"), None);
    }
}
