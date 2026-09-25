//! Self-update logic for the release bundle. Pure logic/state — the in-app notification/progress view is the
//! separate `auto_update_ui` crate.
//!
//! On launch (background thread) the *production* bundle asks the GitHub Releases API for the newest
//! `v*` release; if it names a higher version than this build it downloads the app tarball, swaps it
//! over the running `Pomelo.app`, and relaunches. The dev bundle (`PomeloDev.app`) and `cargo run` are
//! skipped so they never self-replace. Disable with `POMELO_AUTO_UPDATE=0`; override the source repo with
//! `POMELO_UPDATE_REPO=owner/name` at build time. No HTTP crate: shells out to `curl`/`tar`/`ditto`.

use std::sync::{Mutex, OnceLock};

/// What the updater is currently reporting. Rendered by the `auto_update_ui` crate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    UpdateAvailable(String), // version string, e.g. "0.2.0"
}

fn status_cell() -> &'static Mutex<Status> {
    static CELL: OnceLock<Mutex<Status>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(Status::Idle))
}

/// Current updater status (safe to poll from the render loop).
pub fn status() -> Status {
    status_cell().lock().unwrap().clone()
}

pub(crate) fn set_status(s: Status) {
    *status_cell().lock().unwrap() = s;
}

/// Spawn the background update check. No-op off macOS and for non-release bundles.
pub fn spawn_background_check() {
    #[cfg(target_os = "macos")]
    macos::spawn_background_check();
}

/// The release signing key's public half: the same EdDSA key the previous app's updater trusts.
pub const UPDATE_PUBLIC_KEY: &str = "tGnmpupAySzHVMfQcDqtlMFoxuSLC9Pl6TtF4DmGECY=";

/// Checks `bytes` against a base64 Ed25519 `signature` made with the key whose public half is `public_key`.
pub fn verify(bytes: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = engine
        .decode(public_key)
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("bad update public key")?;
    let signature: [u8; 64] = engine
        .decode(signature)
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("the update's signature is malformed")?;
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|e| format!("update key: {e}"))?;
    key.verify_strict(bytes, &ed25519_dalek::Signature::from_bytes(&signature))
        .map_err(|_| "the update's signature does not match; not installing it".to_string())
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const REPO: &str = match option_env!("POMELO_UPDATE_REPO") {
        Some(r) => r,
        None => "pomelohq/pomelo",
    };
    const TAG_PREFIX: &str = "v";

    pub fn spawn_background_check() {
        if std::env::var("POMELO_AUTO_UPDATE").as_deref() == Ok("0") {
            return;
        }
        let Some(app_root) = production_app_root() else {
            return; // dev bundle or cargo run — never auto-replace
        };
        std::thread::spawn(move || {
            if let Err(e) = check_and_apply(&app_root) {
                eprintln!("[update] {e}");
            }
        });
    }

    /// `.../Pomelo.app` if we're running the production bundle, else None.
    fn production_app_root() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let app = exe.ancestors().nth(3)?; // pomelo -> MacOS -> Contents -> Pomelo.app
        if app.file_name()?.to_str()? == "Pomelo.app" {
            Some(app.to_path_buf())
        } else {
            None
        }
    }

    fn check_and_apply(app_root: &Path) -> Result<(), String> {
        let current = env!("CARGO_PKG_VERSION");
        let body = curl(&format!(
            "https://api.github.com/repos/{REPO}/releases?per_page=20"
        ))?;
        let releases: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("parse releases: {e}"))?;
        let releases = releases.as_array().ok_or("releases not an array")?;

        let mut best: Option<(Vec<u32>, String, String)> = None; // (version, tarball url, signature url)
        for r in releases {
            let tag = r.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
            let Some(ver) = tag.strip_prefix(TAG_PREFIX) else {
                continue;
            };
            let Some(parsed) = parse_version(ver) else {
                continue;
            };
            // Releases from before the app verified updates carry no signature; they are skipped.
            let (Some(url), Some(signature)) =
                (asset_url(r, ".app.tar.gz"), asset_url(r, ".app.tar.gz.sig"))
            else {
                continue;
            };
            if best.as_ref().map(|(bv, _, _)| parsed > *bv).unwrap_or(true) {
                best = Some((parsed, url, signature));
            }
        }

        let Some((latest, url, signature)) = best else {
            return Ok(()); // no rust release yet
        };
        let cur = parse_version(current).ok_or("bad current version")?;
        if latest <= cur {
            return Ok(()); // up to date
        }
        crate::set_status(crate::Status::UpdateAvailable(join_version(&latest)));
        eprintln!(
            "[update] {current} -> {} available, downloading",
            join_version(&latest)
        );
        apply(app_root, &url, &signature)
    }

    fn asset_url(release: &serde_json::Value, suffix: &str) -> Option<String> {
        let assets = release.get("assets")?.as_array()?;
        for a in assets {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.ends_with(suffix) {
                return a
                    .get("browser_download_url")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
        }
        None
    }

    fn apply(app_root: &Path, url: &str, signature_url: &str) -> Result<(), String> {
        let tmp = std::env::temp_dir().join("pomelo-update");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| format!("mktmp: {e}"))?;
        let tarball = tmp.join("update.tar.gz");

        run(Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&tarball)
            .arg(url))?;
        let signature = curl(signature_url)?;
        let bytes = std::fs::read(&tarball).map_err(|e| format!("read update: {e}"))?;
        crate::verify(&bytes, signature.trim(), crate::UPDATE_PUBLIC_KEY)?;
        run(Command::new("tar")
            .arg("-xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(&tmp))?;

        let new_app = tmp.join("Pomelo.app");
        if !new_app.is_dir() {
            return Err("downloaded tarball has no Pomelo.app".into());
        }
        // Swap in place, then relaunch. ditto overwrites the bundle atomically enough for our purposes;
        // the running process keeps its open inode until it exits on the line below.
        let _ = std::fs::remove_dir_all(app_root);
        run(Command::new("ditto").arg(&new_app).arg(app_root))?;
        let _ = Command::new("open").arg(app_root).spawn();
        std::process::exit(0);
    }

    fn curl(url: &str) -> Result<String, String> {
        let out = Command::new("curl")
            .args([
                "-fsSL",
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "User-Agent: pomelo-updater",
                url,
            ])
            .output()
            .map_err(|e| format!("curl spawn: {e}"))?;
        if !out.status.success() {
            return Err(format!("curl {url}: status {}", out.status));
        }
        String::from_utf8(out.stdout).map_err(|e| format!("curl utf8: {e}"))
    }

    fn run(cmd: &mut Command) -> Result<(), String> {
        let status = cmd.status().map_err(|e| format!("spawn {cmd:?}: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{cmd:?}: {status}"))
        }
    }

    fn parse_version(s: &str) -> Option<Vec<u32>> {
        let v: Option<Vec<u32>> = s.split('.').map(|p| p.parse().ok()).collect();
        v.filter(|parts| parts.len() == 3)
    }

    fn join_version(v: &[u32]) -> String {
        v.iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ed25519_dalek::Signer;

    #[test]
    fn only_an_update_signed_by_the_release_key_passes() {
        let engine = base64::engine::general_purpose::STANDARD;
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let public = engine.encode(key.verifying_key().to_bytes());
        let signature = engine.encode(key.sign(b"Pomelo.app tarball").to_bytes());
        assert!(verify(b"Pomelo.app tarball", &signature, &public).is_ok());
        assert!(verify(b"tampered tarball", &signature, &public).is_err());
        assert!(verify(b"Pomelo.app tarball", &signature, UPDATE_PUBLIC_KEY).is_err());
        assert!(verify(b"x", "not base64!", &public).is_err());
    }
}
