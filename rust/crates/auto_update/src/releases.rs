//! What GitHub Releases offers for a channel: versions compared by semver, the rolling `nightly` prerelease read
//! by its assets' names, and the signature every download is checked against.

use std::cmp::Ordering;
use std::fmt;

pub const REPO: &str = match option_env!("POMELO_UPDATE_REPO") {
    Some(repo) => repo,
    None => "pomelohq/pomelo",
};
/// The one prerelease every nightly build is published into, its assets named by version.
pub const NIGHTLY_TAG: &str = "nightly";
const TAG_PREFIX: &str = "v";

/// The release signing key's public half: the same EdDSA key the previous app's updater trusts.
pub const UPDATE_PUBLIC_KEY: &str = "tGnmpupAySzHVMfQcDqtlMFoxuSLC9Pl6TtF4DmGECY=";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Channel {
    /// Releases cut with `make patch|minor|major`; prereleases are never offered.
    #[default]
    Stable,
    /// The newest of a nightly build of main and a stable release, so a stable release cut later still wins.
    Nightly,
}

impl Channel {
    pub fn parse(text: &str) -> Option<Channel> {
        match text {
            "stable" => Some(Channel::Stable),
            "nightly" => Some(Channel::Nightly),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nightly => "nightly",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Identifier {
    Number(u64),
    Text(String),
}

impl Ord for Identifier {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Identifier::Number(left), Identifier::Number(right)) => left.cmp(right),
            (Identifier::Number(_), Identifier::Text(_)) => Ordering::Less,
            (Identifier::Text(_), Identifier::Number(_)) => Ordering::Greater,
            (Identifier::Text(left), Identifier::Text(right)) => left.cmp(right),
        }
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A semver version (`0.9.9`, `0.9.9-nightly.20261024`); build metadata is not accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<Identifier>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let numbers: Vec<u64> = core.split('.').map(numeric).collect::<Option<Vec<u64>>>()?;
        let core: [u64; 3] = numbers.try_into().ok()?;
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|part| {
                    if part.is_empty()
                        || !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    {
                        None
                    } else if part.chars().all(|c| c.is_ascii_digit()) {
                        numeric(part).map(Identifier::Number)
                    } else {
                        Some(Identifier::Text(part.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Version { core, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    pub fn is_nightly(&self) -> bool {
        matches!(self.pre.first(), Some(Identifier::Text(text)) if text == NIGHTLY_TAG)
    }

    /// The release tag this version is published under.
    pub fn tag(&self) -> String {
        if self.is_nightly() {
            NIGHTLY_TAG.to_string()
        } else {
            format!("{TAG_PREFIX}{self}")
        }
    }
}

fn numeric(text: &str) -> Option<u64> {
    let leading_zero = text.len() > 1 && text.starts_with('0');
    if text.is_empty() || leading_zero || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.core;
        write!(f, "{major}.{minor}.{patch}")?;
        for (index, part) in self.pre.iter().enumerate() {
            f.write_str(if index == 0 { "-" } else { "." })?;
            match part {
                Identifier::Number(number) => write!(f, "{number}")?,
                Identifier::Text(text) => f.write_str(text)?,
            }
        }
        Ok(())
    }
}

/// A download a release offers, by the file name each version gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Artifact {
    /// `Pomelo-<v>.app.tar.gz`, what the macOS app swaps itself for.
    MacApp,
    /// `Pomelo-<v>-linux-<arch>.tar.gz`, the Linux app and pom with install.sh.
    LinuxApp(&'static str),
    /// `pom-<v>-linux-<arch>.tar.gz`, pom alone with install.sh.
    LinuxCli(&'static str),
}

impl Artifact {
    pub fn file_name(self, version: &str) -> String {
        match self {
            Artifact::MacApp => format!("Pomelo-{version}.app.tar.gz"),
            Artifact::LinuxApp(arch) => format!("Pomelo-{version}-linux-{arch}.tar.gz"),
            Artifact::LinuxCli(arch) => format!("pom-{version}-linux-{arch}.tar.gz"),
        }
    }

    /// The version a file of this artifact is named for.
    pub fn version_in(self, name: &str) -> Option<Version> {
        let middle = match self {
            Artifact::MacApp => name.strip_prefix("Pomelo-")?.strip_suffix(".app.tar.gz")?,
            Artifact::LinuxApp(arch) => name
                .strip_prefix("Pomelo-")?
                .strip_suffix(".tar.gz")?
                .strip_suffix(arch)?
                .strip_suffix("-linux-")?,
            Artifact::LinuxCli(arch) => name
                .strip_prefix("pom-")?
                .strip_suffix(".tar.gz")?
                .strip_suffix(arch)?
                .strip_suffix("-linux-")?,
        };
        Version::parse(middle)
    }
}

/// One version's download and its signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub version: Version,
    pub file_name: String,
    pub url: String,
    pub signature_url: String,
    pub size: Option<u64>,
}

fn assets(release: &serde_json::Value) -> impl Iterator<Item = &serde_json::Value> {
    release
        .get("assets")
        .and_then(|assets| assets.as_array())
        .into_iter()
        .flatten()
}

fn name_of(asset: &serde_json::Value) -> Option<&str> {
    asset.get("name").and_then(|name| name.as_str())
}

fn url_of(asset: &serde_json::Value) -> Option<String> {
    asset
        .get("browser_download_url")
        .and_then(|url| url.as_str())
        .map(String::from)
}

fn offer(
    release: &serde_json::Value,
    asset: &serde_json::Value,
    version: Version,
) -> Option<Offer> {
    let file_name = name_of(asset)?.to_string();
    let signature_name = format!("{file_name}.sig");
    let signature_url = assets(release)
        .find(|candidate| name_of(candidate) == Some(signature_name.as_str()))
        .and_then(url_of)?;
    Some(Offer {
        version,
        url: url_of(asset)?,
        signature_url,
        size: asset.get("size").and_then(|size| size.as_u64()),
        file_name,
    })
}

/// Every signed download of `artifact` that `channel` may install, from a `/releases` listing.
pub fn offers(releases: &serde_json::Value, channel: Channel, artifact: Artifact) -> Vec<Offer> {
    let mut found = Vec::new();
    for release in releases.as_array().into_iter().flatten() {
        let flag = |key: &str| release.get(key).and_then(|v| v.as_bool()) == Some(true);
        if flag("draft") {
            continue;
        }
        let tag = release
            .get("tag_name")
            .and_then(|tag| tag.as_str())
            .unwrap_or_default();
        if tag == NIGHTLY_TAG {
            if channel != Channel::Nightly {
                continue;
            }
            for asset in assets(release) {
                let version = name_of(asset)
                    .and_then(|name| artifact.version_in(name))
                    .filter(Version::is_nightly);
                if let Some(offer) = version.and_then(|version| offer(release, asset, version)) {
                    found.push(offer);
                }
            }
            continue;
        }
        let Some(version) = tag.strip_prefix(TAG_PREFIX).and_then(Version::parse) else {
            continue;
        };
        if channel == Channel::Stable && (version.is_prerelease() || flag("prerelease")) {
            continue;
        }
        let file_name = artifact.file_name(&version.to_string());
        let asset = assets(release).find(|asset| name_of(asset) == Some(file_name.as_str()));
        if let Some(offer) = asset.and_then(|asset| offer(release, asset, version)) {
            found.push(offer);
        }
    }
    found
}

/// The highest version `channel` offers.
pub fn newest(releases: &serde_json::Value, channel: Channel, artifact: Artifact) -> Option<Offer> {
    offers(releases, channel, artifact)
        .into_iter()
        .max_by(|left, right| left.version.cmp(&right.version))
}

pub fn releases_api(per_page: u32) -> String {
    format!("https://api.github.com/repos/{REPO}/releases?per_page={per_page}")
}

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
        .decode(signature.trim())
        .ok()
        .and_then(|raw| raw.try_into().ok())
        .ok_or("the update's signature is malformed")?;
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|e| format!("update key: {e}"))?;
    key.verify_strict(bytes, &ed25519_dalek::Signature::from_bytes(&signature))
        .map_err(|_| "the download's signature did not verify".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} parses"))
    }

    #[test]
    fn versions_order_by_semver_and_a_release_ranks_above_its_nightlies() {
        assert!(version("0.10.0") > version("0.9.9"));
        assert!(version("0.9.10") > version("0.9.9"));
        assert!(version("0.9.9") > version("0.9.9-nightly.20261024"));
        assert!(version("0.9.9-nightly.20261025") > version("0.9.9-nightly.20261024"));
        assert!(version("0.9.9-nightly.20261024") > version("0.9.8"));
        assert!(version("1.0.0-alpha.2") > version("1.0.0-alpha.1"));
        assert!(version("1.0.0-alpha.beta") > version("1.0.0-alpha.1"));
        assert_eq!(
            version("0.9.9-nightly.20261024").to_string(),
            "0.9.9-nightly.20261024"
        );
        for bad in [
            "0.10",
            "0.1.x",
            "01.2.3",
            "1.2.3-",
            "1.2.3-a..b",
            "1.2.3+build",
            "",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert!(version("0.9.9-nightly.20261024").is_nightly());
        assert!(!version("0.9.9-rc.1").is_nightly());
        assert_eq!(version("0.9.9-nightly.20261024").tag(), "nightly");
        assert_eq!(version("0.9.9").tag(), "v0.9.9");
    }

    #[test]
    fn artifact_names_carry_their_version() {
        assert_eq!(
            Artifact::MacApp.version_in("Pomelo-0.9.9-nightly.20261024.app.tar.gz"),
            Some(version("0.9.9-nightly.20261024"))
        );
        assert_eq!(
            Artifact::LinuxCli("x86_64").version_in("pom-0.9.9-linux-x86_64.tar.gz"),
            Some(version("0.9.9"))
        );
        assert_eq!(
            Artifact::LinuxCli("x86_64").version_in("pom-0.9.9-linux-aarch64.tar.gz"),
            None
        );
        assert_eq!(
            Artifact::LinuxApp("aarch64").file_name("0.9.9"),
            "Pomelo-0.9.9-linux-aarch64.tar.gz"
        );
        assert_eq!(Artifact::MacApp.version_in("Pomelo-0.9.9.dmg"), None);
    }

    fn asset(name: &str) -> serde_json::Value {
        serde_json::json!({"name": name, "browser_download_url": format!("https://x/{name}"), "size": 10})
    }

    fn listing() -> serde_json::Value {
        let signed = |version: &str| {
            let name = Artifact::MacApp.file_name(version);
            vec![asset(&name), asset(&format!("{name}.sig"))]
        };
        let nightly: Vec<serde_json::Value> = ["0.9.9-nightly.20261023", "0.9.9-nightly.20261024"]
            .iter()
            .flat_map(|version| signed(version))
            .chain([asset("Pomelo-0.9.9-nightly.20261025.app.tar.gz")])
            .collect();
        serde_json::json!([
            {"tag_name": "nightly", "prerelease": true, "assets": nightly},
            {"tag_name": "v0.9.8", "assets": signed("0.9.8")},
            {"tag_name": "v0.9.10", "draft": true, "assets": signed("0.9.10")},
            {"tag_name": "v0.9.7", "assets": [asset("Pomelo-0.9.7.app.tar.gz")]},
            {"tag_name": "grammars", "assets": [asset("index.json")]}
        ])
    }

    #[test]
    fn stable_ignores_nightlies_and_nightly_takes_the_highest_signed_build() {
        let stable = newest(&listing(), Channel::Stable, Artifact::MacApp).map(|o| o.version);
        assert_eq!(stable, Some(version("0.9.8")));
        let nightly = newest(&listing(), Channel::Nightly, Artifact::MacApp);
        assert_eq!(
            nightly.as_ref().map(|offer| offer.version.clone()),
            Some(version("0.9.9-nightly.20261024")),
            "the unsigned 20261025 build is skipped"
        );
        assert_eq!(
            nightly.map(|offer| offer.signature_url),
            Some("https://x/Pomelo-0.9.9-nightly.20261024.app.tar.gz.sig".to_string())
        );
    }

    #[test]
    fn a_stable_release_cut_after_a_nightly_wins_on_the_nightly_channel() {
        let mut releases = listing();
        let name = Artifact::MacApp.file_name("0.9.9");
        if let Some(list) = releases.as_array_mut() {
            list.push(serde_json::json!({"tag_name": "v0.9.9", "assets": [asset(&name), asset(&format!("{name}.sig"))]}));
        }
        let nightly = newest(&releases, Channel::Nightly, Artifact::MacApp).map(|o| o.version);
        assert_eq!(nightly, Some(version("0.9.9")));
    }

    #[test]
    fn channels_read_from_settings() {
        assert_eq!(Channel::parse("nightly"), Some(Channel::Nightly));
        assert_eq!(Channel::parse("stable"), Some(Channel::Stable));
        assert_eq!(Channel::parse(""), None);
        assert_eq!(Channel::Nightly.as_str(), "nightly");
    }
}
