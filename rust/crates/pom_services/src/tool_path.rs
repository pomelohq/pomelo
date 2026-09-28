//! Version managers (nvm, fnm, volta, asdf, ...) only put their bins on PATH from an interactive
//! `.zshrc`, so services and the hooks they run under `/bin/sh` would not find `node` and friends.
//! We read the managers' own directories instead of sourcing any rc file: deterministic, and it
//! cannot trigger a macOS permission prompt from a prompt plugin.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The current PATH with the well-known tool dirs that exist prepended. Computed once.
pub fn tool_path() -> &'static str {
    static TOOL_PATH: OnceLock<String> = OnceLock::new();
    TOOL_PATH.get_or_init(|| {
        let env = |key: &str| std::env::var(key).ok().filter(|value| !value.is_empty());
        build_tool_path(
            &env("PATH").unwrap_or_default(),
            &env("HOME").map(PathBuf::from).unwrap_or_default(),
            &env,
        )
    })
}

fn build_tool_path(base: &str, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> String {
    let env_or = |key: &str, fallback: PathBuf| env(key).map_or(fallback, PathBuf::from);
    let candidates = [
        env("PNPM_HOME").map(PathBuf::from),
        Some(home.join("Library/pnpm")),
        Some(env_or("VOLTA_HOME", home.join(".volta")).join("bin")),
        Some(home.join(".bun/bin")),
        Some(env_or("XDG_DATA_HOME", home.join(".local/share")).join("fnm")),
        Some(env_or("ASDF_DATA_DIR", home.join(".asdf")).join("shims")),
        Some(env_or("RBENV_ROOT", home.join(".rbenv")).join("shims")),
        Some(env_or("PYENV_ROOT", home.join(".pyenv")).join("shims")),
        nvm_default_bin(&env_or("NVM_DIR", home.join(".nvm"))),
        Some(home.join(".cargo/bin")),
        Some(PathBuf::from("/opt/homebrew/bin")),
        Some(PathBuf::from("/usr/local/bin")),
    ];
    let mut seen: Vec<String> = base.split(':').map(str::to_string).collect();
    let mut prepend = Vec::new();
    for dir in candidates.into_iter().flatten() {
        let text = dir.to_string_lossy().into_owned();
        if text.is_empty() || seen.contains(&text) || !dir.is_dir() {
            continue;
        }
        seen.push(text.clone());
        prepend.push(text);
    }
    if prepend.is_empty() {
        return base.to_string();
    }
    format!("{}:{base}", prepend.join(":"))
}

/// nvm's `default` alias when it names an installed version, else the highest installed one.
fn nvm_default_bin(nvm_dir: &Path) -> Option<PathBuf> {
    let versions_dir = nvm_dir.join("versions/node");
    let mut versions: Vec<String> = std::fs::read_dir(&versions_dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with('v'))
        .collect();
    if let Ok(alias) = std::fs::read_to_string(nvm_dir.join("alias/default")) {
        let alias = alias.trim();
        let want = if alias.starts_with('v') {
            alias.to_string()
        } else {
            format!("v{alias}")
        };
        if versions.contains(&want) {
            return Some(versions_dir.join(want).join("bin"));
        }
    }
    versions.sort_by_key(|version| std::cmp::Reverse(semver(version)));
    versions
        .first()
        .map(|version| versions_dir.join(version).join("bin"))
}

fn semver(version: &str) -> [u32; 3] {
    let mut out = [0; 3];
    for (slot, part) in out
        .iter_mut()
        .zip(version.trim_start_matches('v').splitn(3, '.'))
    {
        *slot = part
            .chars()
            .map_while(|c| c.to_digit(10))
            .fold(0, |number, digit| number * 10 + digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepends_existing_manager_dirs_once() {
        let temp = tempfile::tempdir().expect("temp");
        let home = temp.path();
        for dir in [".volta/bin", ".cargo/bin", ".nvm/versions/node/v18.2.0/bin"] {
            std::fs::create_dir_all(home.join(dir)).expect("mkdir");
        }
        std::fs::create_dir_all(home.join(".nvm/versions/node/v20.11.1/bin")).expect("mkdir");
        let cargo = home.join(".cargo/bin").to_string_lossy().into_owned();
        let base = format!("/usr/bin:{cargo}");
        let path = build_tool_path(&base, home, &|_| None);
        let volta = home.join(".volta/bin").to_string_lossy().into_owned();
        let node = home
            .join(".nvm/versions/node/v20.11.1/bin")
            .to_string_lossy()
            .into_owned();
        assert!(path.starts_with(&format!("{volta}:{node}:")), "{path}");
        assert!(path.ends_with(&base), "{path}");
        assert_eq!(path.matches(&cargo).count(), 1, "{path}");
    }

    #[test]
    fn nvm_default_alias_wins_over_newest() {
        let temp = tempfile::tempdir().expect("temp");
        for version in ["v18.2.0", "v20.1.0"] {
            std::fs::create_dir_all(temp.path().join("versions/node").join(version))
                .expect("mkdir");
        }
        std::fs::create_dir_all(temp.path().join("alias")).expect("mkdir");
        std::fs::write(temp.path().join("alias/default"), "18.2.0\n").expect("alias");
        let bin = nvm_default_bin(temp.path()).expect("bin");
        assert!(bin.ends_with("v18.2.0/bin"), "{}", bin.display());
    }

    #[test]
    fn semver_orders_numerically() {
        assert!(semver("v10.0.0") > semver("v9.9.9"));
        assert_eq!(semver("v1.2"), [1, 2, 0]);
    }
}
