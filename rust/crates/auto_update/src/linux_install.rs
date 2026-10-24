//! Installs from the Linux tarballs, shared by the app's updater and `pom self-update`: a signed tarball is
//! unpacked and its own install.sh puts it in place, so an update installs exactly as a first install does.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::{verify, Artifact, Offer, UPDATE_PUBLIC_KEY};

/// install.sh writes it, so an install it made can tell itself apart from a package manager's.
pub const INSTALL_MARKER: &str = "share/pomelo/installed-by";

/// The release arch of this machine.
pub fn arch() -> Result<&'static str, String> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Ok("aarch64"),
        other => Err(format!("no build is published for {other} machines")),
    }
}

pub fn app_artifact() -> Result<Artifact, String> {
    arch().map(Artifact::LinuxApp)
}

pub fn cli_artifact() -> Result<Artifact, String> {
    arch().map(Artifact::LinuxCli)
}

/// The prefix install.sh put `exe` under (`<prefix>/bin/<name>`), when it may replace itself there. One a
/// package manager installed, or a copy elsewhere, is updated the way it was installed.
pub fn self_managed_prefix(exe: &Path, name: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let not_ours = || {
        format!(
            "{} was not installed by install.sh; update it the way it was installed",
            exe.display()
        )
    };
    if exe.file_name().and_then(|file| file.to_str()) != Some(name) {
        return Err(not_ours());
    }
    let bin = exe.parent().ok_or_else(not_ours)?;
    if bin.file_name().and_then(|dir| dir.to_str()) != Some("bin") {
        return Err(not_ours());
    }
    let prefix = bin.parent().ok_or_else(not_ours)?;
    let installs_before_the_marker = home.is_some_and(|home| prefix == home.join(".local"));
    if prefix.join(INSTALL_MARKER).is_file() || installs_before_the_marker {
        Ok(prefix.to_path_buf())
    } else {
        Err(not_ours())
    }
}

/// Fetches `url` with curl over HTTPS only (redirects too), within `limit`.
pub fn fetch(url: &str, limit: Duration) -> Result<Vec<u8>, String> {
    let mut command = Command::new("curl");
    command
        .args(["-fsSL", "--connect-timeout", "20", "--max-time"])
        .arg(limit.as_secs().to_string())
        .args(["-H", "User-Agent: pomelo-updater"]);
    if url.starts_with("https://") {
        command.args(["--proto", "=https", "--proto-redir", "=https"]);
    } else if url.starts_with("file://") {
        command.args(["--proto", "=file"]);
    } else {
        return Err(format!("{url} is not an https:// address"));
    }
    let output = command
        .arg(url)
        .output()
        .map_err(|error| format!("could not run curl: {error}"))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        return Err(match reason.trim() {
            "" => format!("could not download {url}"),
            reason => format!("could not download {url}: {reason}"),
        });
    }
    Ok(output.stdout)
}

/// `offer`'s download, refused unless its `.sig` verifies against the release key.
pub fn fetch_verified(offer: &Offer, limit: Duration) -> Result<Vec<u8>, String> {
    let bytes = fetch(&offer.url, limit)?;
    let signature = fetch(&offer.signature_url, Duration::from_secs(60))?;
    verify(
        &bytes,
        &String::from_utf8_lossy(&signature),
        UPDATE_PUBLIC_KEY,
    )
    .map_err(|error| format!("{}: {error}; nothing was installed", offer.file_name))?;
    Ok(bytes)
}

/// Unpacks a tarball into `into`; the folder in it that holds install.sh.
pub fn unpack(tarball: &Path, into: &Path) -> Result<PathBuf, String> {
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(into)
        .output()
        .map_err(|error| format!("could not run tar: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not unpack {}: {}",
            tarball.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let entries = std::fs::read_dir(into).map_err(|e| format!("read {}: {e}", into.display()))?;
    entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|dir| dir.join("install.sh").is_file())
        .ok_or_else(|| format!("{} has no install.sh", tarball.display()))
}

/// Runs an unpacked tarball's install.sh into `prefix`.
pub fn run_install(unpacked: &Path, prefix: &Path, cli_only: bool) -> Result<(), String> {
    let mut command = Command::new(unpacked.join("install.sh"));
    command.env("PREFIX", prefix).stdin(Stdio::null());
    if cli_only {
        command.arg("--cli-only");
    }
    let output = command
        .output()
        .map_err(|error| format!("could not run install.sh: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "install.sh failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_install_sh_installed_replaces_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let prefix = dir.path().join("opt");
        std::fs::create_dir_all(prefix.join("bin")).expect("bin");
        let pom = prefix.join("bin/pom");
        assert!(self_managed_prefix(&pom, "pom", None).is_err());
        std::fs::create_dir_all(prefix.join("share/pomelo")).expect("share");
        std::fs::write(prefix.join(INSTALL_MARKER), "install.sh\n").expect("marker");
        assert_eq!(self_managed_prefix(&pom, "pom", None), Ok(prefix.clone()));
        assert!(self_managed_prefix(&prefix.join("bin/pomelo"), "pom", None).is_err());
        assert!(self_managed_prefix(Path::new("/usr/bin/pom"), "pom", None).is_err());
        let home = dir.path().join("home");
        assert_eq!(
            self_managed_prefix(&home.join(".local/bin/pom"), "pom", Some(&home)),
            Ok(home.join(".local"))
        );
    }

    #[test]
    fn a_tarball_unpacks_and_its_install_sh_runs_into_the_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stage = dir.path().join("stage/pom-1.2.3");
        std::fs::create_dir_all(&stage).expect("stage");
        let script = stage.join("install.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nset -e\nmkdir -p \"$PREFIX/bin\"\necho \"$@\" > \"$PREFIX/bin/args\"\n",
        )
        .expect("script");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let tarball = dir.path().join("pom.tar.gz");
        let packed = Command::new("tar")
            .arg("-czf")
            .arg(&tarball)
            .arg("-C")
            .arg(dir.path().join("stage"))
            .arg("pom-1.2.3")
            .status()
            .expect("tar");
        assert!(packed.success());
        let into = dir.path().join("unpacked");
        std::fs::create_dir_all(&into).expect("unpacked");
        let unpacked = unpack(&tarball, &into).expect("unpack");
        assert!(unpacked.ends_with("pom-1.2.3"));
        let prefix = dir.path().join("prefix");
        run_install(&unpacked, &prefix, true).expect("install");
        assert_eq!(
            std::fs::read_to_string(prefix.join("bin/args")).ok(),
            Some("--cli-only\n".into())
        );
        assert!(fetch("http://example.com/x", Duration::from_secs(1)).is_err());
    }
}
