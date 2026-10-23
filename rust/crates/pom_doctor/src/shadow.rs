//! A `pom` earlier on the PATH than this one: commands typed in a terminal then run another build, which may
//! lack commands or read state differently.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::{Finding, Severity};

const VERSION_TIMEOUT: Duration = Duration::from_secs(2);

/// The first `pom` the PATH resolves to.
pub fn pom_on_path(path_env: &str) -> Option<PathBuf> {
    path_env
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join("pom"))
        .find(|candidate| candidate.is_file())
}

/// What `<binary> --version` reports (`pom 0.9.4` -> `0.9.4`), within a short timeout.
pub fn pom_version(binary: &Path) -> Option<String> {
    let mut command = Command::new(binary);
    command.arg("--version");
    let output = crate::disk::output_within(&mut command, VERSION_TIMEOUT)?;
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pom "))
        .map(|version| version.trim().to_string())
        .filter(|version| !version.is_empty())
}

fn same_file(one: &Path, other: &Path) -> bool {
    match (one.canonicalize(), other.canonicalize()) {
        (Ok(one), Ok(other)) => one == other,
        _ => one == other,
    }
}

/// A warning when the `pom` on the PATH is not `own` (this build, at `own_version`) and reports another version.
pub fn path_shadow(
    path_env: &str,
    own: &Path,
    own_version: &str,
    version_of: &dyn Fn(&Path) -> Option<String>,
) -> Option<Finding> {
    let found = pom_on_path(path_env)?;
    if same_file(&found, own) {
        return None;
    }
    let found_version = version_of(&found);
    if found_version.as_deref() == Some(own_version) {
        return None;
    }
    let shown = found_version.unwrap_or_else(|| "an unknown version".to_string());
    Some(
        Finding::new(
            "path.shadow",
            Severity::Warn,
            format!("Another pom comes first on the PATH ({shown})"),
        )
        .detail(format!(
            "`pom` in a terminal runs {} ({shown}), not {} ({own_version}); its commands and output may differ",
            found.display(),
            own.display()
        ))
        .fix(format!(
            "remove or rename {found} (or point it at this one: ln -sf {own} {found}), or put {dir} earlier on the PATH",
            found = found.display(),
            own = own.display(),
            dir = own.parent().unwrap_or(own).display()
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_pom(dir: &Path, version: &str) -> PathBuf {
        if let Err(error) = std::fs::create_dir_all(dir) {
            panic!("create {}: {error}", dir.display());
        }
        let path = dir.join("pom");
        let script = format!("#!/bin/sh\necho 'pom {version}'\n");
        if let Err(error) = std::fs::write(&path, script) {
            panic!("write {}: {error}", path.display());
        }
        if let Err(error) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        {
            panic!("chmod {}: {error}", path.display());
        }
        path
    }

    #[test]
    fn a_different_pom_first_on_the_path_is_named_with_both_versions() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let stale = fake_pom(&temp.path().join("local-bin"), "0.9.7");
        let own = fake_pom(&temp.path().join("app"), "0.9.4");
        let path_env = format!(
            "{}:{}",
            temp.path().join("local-bin").display(),
            temp.path().join("app").display()
        );
        assert_eq!(pom_version(&stale).as_deref(), Some("0.9.7"));
        let finding =
            path_shadow(&path_env, &own, "0.9.4", &pom_version).ok_or("no warning for 0.9.7")?;
        assert_eq!(finding.id, "path.shadow");
        assert!(finding.title.contains("0.9.7"), "{}", finding.title);
        assert!(
            finding.detail.contains(&stale.display().to_string())
                && finding.detail.contains(&own.display().to_string())
                && finding.detail.contains("0.9.4"),
            "{}",
            finding.detail
        );
        assert!(finding.fix.contains("ln -sf"), "{}", finding.fix);

        let own_first = format!(
            "{}:{}",
            temp.path().join("app").display(),
            temp.path().join("local-bin").display()
        );
        assert_eq!(path_shadow(&own_first, &own, "0.9.4", &pom_version), None);

        let link_dir = temp.path().join("linked");
        std::fs::create_dir_all(&link_dir).map_err(|error| error.to_string())?;
        std::os::unix::fs::symlink(&own, link_dir.join("pom"))
            .map_err(|error| error.to_string())?;
        let linked = format!(
            "{}:{}",
            link_dir.display(),
            temp.path().join("app").display()
        );
        assert_eq!(
            path_shadow(&linked, &own, "0.9.4", &pom_version),
            None,
            "a link to this binary is this binary"
        );

        let same_version = fake_pom(&temp.path().join("copy"), "0.9.4");
        let copied = format!(
            "{}:{}",
            same_version.parent().ok_or("no parent")?.display(),
            temp.path().join("app").display()
        );
        assert_eq!(
            path_shadow(&copied, &own, "0.9.4", &pom_version),
            None,
            "a copy of the same version is harmless"
        );
        assert_eq!(
            path_shadow("/nonexistent", &own, "0.9.4", &pom_version),
            None
        );
        Ok(())
    }
}
