//! Updating pom on a host from the app. The tarball for the host's machine is the app's own version when it is
//! still published (a stable release, or a nightly the nightly release keeps), else the newest nightly; its
//! signature is checked here against the release key, before a byte reaches the host. It then goes through ssh
//! into a temp folder there, whose install.sh puts pom in place. The host never downloads anything.
//!
//! An app and a host's pom of the same version work together fully. Of different versions, the app uses only
//! what the server's HELLO declares, and offers this update once their major or minor version differs and the
//! host is older; a newer host is never taken back.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use auto_update::{Artifact, Channel, Offer, Version};

use crate::{Host, Ssh};

pub const RELEASES: &str = "https://github.com/pomelohq/pomelo/releases/download";
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(300);
const SCRIPT_LIMIT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStep {
    /// Asking the host what machine it is.
    Machine,
    Download,
    Verify,
    Install,
    /// Stopping the old server and reconnecting to the new one.
    Restart,
}

impl UpdateStep {
    pub fn label(self) -> &'static str {
        match self {
            UpdateStep::Machine => "checking the host's machine",
            UpdateStep::Download => "downloading",
            UpdateStep::Verify => "checking the download",
            UpdateStep::Install => "installing",
            UpdateStep::Restart => "restarting pom server",
        }
    }
}

/// Where the pom for a version comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// GitHub Releases: the version's own release, or the kept builds of the nightly release.
    GitHub,
    /// A folder holding the version's tarball and its `.sig`; only `https://` (and `file://`, for tests).
    Folder(String),
}

/// The pom a host should get: `version` (the app's), signed by `public_key`'s key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub source: Source,
    pub public_key: String,
}

impl Release {
    pub fn published(version: &str) -> Release {
        Release {
            version: version.to_string(),
            source: Source::GitHub,
            public_key: auto_update::UPDATE_PUBLIC_KEY.to_string(),
        }
    }

    pub fn tarball(&self, arch: &str) -> String {
        format!("pom-{}-linux-{arch}.tar.gz", self.version)
    }

    /// The signed download to install for `arch`.
    pub fn resolve(&self, arch: &'static str) -> Result<Offer, String> {
        let file_name = self.tarball(arch);
        match &self.source {
            Source::Folder(base) => {
                let base = base.trim_end_matches('/');
                Ok(Offer {
                    version: Version::parse(&self.version)
                        .ok_or_else(|| format!("{:?} is not a version", self.version))?,
                    url: format!("{base}/{file_name}"),
                    signature_url: format!("{base}/{file_name}.sig"),
                    size: None,
                    file_name,
                })
            }
            Source::GitHub => {
                let releases = auto_update::fetch_releases(Channel::Nightly)?;
                let offers =
                    auto_update::offers(&releases, Channel::Nightly, Artifact::LinuxCli(arch));
                pick(&offers, &self.version).ok_or_else(|| {
                    format!("no signed {file_name} is published; update pom on the host by hand")
                })
            }
        }
    }
}

/// The app's own version when it is published, else for a nightly app the newest nightly: a nightly leaves the
/// nightly release after 7 nights, and the newest one speaks the same protocol as far as HELLO says.
pub fn pick(offers: &[Offer], wanted: &str) -> Option<Offer> {
    let wanted = Version::parse(wanted)?;
    if let Some(exact) = offers.iter().find(|offer| offer.version == wanted) {
        return Some(exact.clone());
    }
    if !wanted.is_nightly() {
        return None;
    }
    offers
        .iter()
        .filter(|offer| offer.version.is_nightly())
        .max_by(|left, right| left.version.cmp(&right.version))
        .cloned()
}

/// The release arch for `uname -sm` output; only Linux hosts have a pom tarball.
pub fn linux_arch(uname: &str) -> Result<&'static str, String> {
    let mut words = uname.split_whitespace();
    let (system, machine) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    if system != "Linux" {
        return Err(format!(
            "the host runs {system}; pom is published for Linux hosts only"
        ));
    }
    match machine {
        "x86_64" | "amd64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("aarch64"),
        other => Err(format!("no pom is published for {other} machines")),
    }
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '-' | '+'))
}

/// Fetches `url` with curl, over HTTPS only (redirects too), within `limit`.
pub fn download(url: &str, limit: Duration) -> Result<Vec<u8>, String> {
    let mut command = Command::new("curl");
    command
        .arg("-fsSL")
        .arg("--connect-timeout")
        .arg("15")
        .arg("--max-time")
        .arg(limit.as_secs().to_string());
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

/// Runs `script` on `host`, feeding it `input`; its stdout when it exits 0.
pub fn run_script(
    ssh: &Ssh,
    host: &Host,
    script: &str,
    input: &[u8],
    limit: Duration,
) -> Result<String, String> {
    let mut child = ssh
        .script(host, script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run ssh: {error}"))?;
    let missing = || "ssh has no pipes".to_string();
    let mut stdin = child.stdin.take().ok_or_else(missing)?;
    let mut stdout = child.stdout.take().ok_or_else(missing)?;
    let mut stderr = child.stderr.take().ok_or_else(missing)?;
    let input = input.to_vec();
    let feeder = std::thread::spawn(move || stdin.write_all(&input));
    let out = std::thread::spawn(move || {
        let mut text = Vec::new();
        stdout.read_to_end(&mut text).map(|_| text)
    });
    let err = std::thread::spawn(move || {
        let mut text = Vec::new();
        stderr.read_to_end(&mut text).map(|_| text)
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                if let Err(error) = child.kill() {
                    eprintln!("update: stopping ssh: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("update: reaping ssh: {error}");
                }
                return Err(format!(
                    "{} did not finish in {}s",
                    host.name,
                    limit.as_secs()
                ));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let fed = feeder
        .join()
        .map_err(|_| "the input thread failed".to_string())?;
    let stdout = out
        .join()
        .map_err(|_| "the output thread failed".to_string())?
        .map_err(|error| error.to_string())?;
    let stderr = err
        .join()
        .map_err(|_| "the output thread failed".to_string())?
        .map_err(|error| error.to_string())?;
    let said = String::from_utf8_lossy(&stderr).trim().to_string();
    if !status.success() {
        return Err(match (status.code(), said.is_empty()) {
            (Some(255), _) => format!("could not reach {}: {said}", host.name),
            (_, true) => format!("{} exited with {status}", host.name),
            (_, false) => said,
        });
    }
    if let Err(error) = fed {
        return Err(format!("sending to {}: {error}", host.name));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// What the host runs: the tarball on stdin unpacked into a temp folder, its pom installed, the folder gone.
pub fn install_script(version: &str) -> Result<String, String> {
    if !valid_version(version) {
        return Err(format!("{version:?} is not a version"));
    }
    Ok(format!(
        "set -e; dir=$(mktemp -d); trap 'rm -rf \"$dir\"' EXIT; tar -xzf - -C \"$dir\"; \"$dir/pom-{version}/install.sh\" --cli-only"
    ))
}

/// Installs `release`'s pom on `host`: the tarball for its machine, refused unless its signature verifies,
/// before it leaves this machine. The running server is left to the caller.
pub fn install_pom(
    ssh: &Ssh,
    host: &Host,
    release: &Release,
    progress: &dyn Fn(UpdateStep),
) -> Result<(), String> {
    progress(UpdateStep::Machine);
    let uname = run_script(ssh, host, "uname -sm", b"", SCRIPT_LIMIT)?;
    let arch = linux_arch(uname.trim())?;
    progress(UpdateStep::Download);
    let offer = release.resolve(arch)?;
    let tarball = download(&offer.url, DOWNLOAD_LIMIT)?;
    let signature = download(&offer.signature_url, DOWNLOAD_LIMIT)?;
    progress(UpdateStep::Verify);
    auto_update::verify(
        &tarball,
        &String::from_utf8_lossy(&signature),
        &release.public_key,
    )
    .map_err(|error| format!("{}: {error}; nothing was installed", offer.file_name))?;
    progress(UpdateStep::Install);
    run_script(
        ssh,
        host,
        &install_script(&offer.version.to_string())?,
        &tarball,
        SCRIPT_LIMIT,
    )?;
    Ok(())
}

/// What to run on `host` by hand when the app cannot install pom itself (a build with no release).
pub fn manual_commands(version: &str) -> Vec<String> {
    let tag = Version::parse(version).map_or_else(|| format!("v{version}"), |parsed| parsed.tag());
    let base = format!("{RELEASES}/{tag}");
    vec![
        format!("curl -fLO {base}/pom-{version}-linux-$(uname -m).tar.gz"),
        format!(
            "tar -xzf pom-{version}-linux-*.tar.gz && pom server stop && pom-{version}/install.sh --cli-only"
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_machine_picks_the_tarball() {
        assert_eq!(linux_arch("Linux x86_64"), Ok("x86_64"));
        assert_eq!(linux_arch("Linux aarch64\n"), Ok("aarch64"));
        assert!(linux_arch("Darwin arm64").is_err());
        assert!(linux_arch("Linux riscv64").is_err());
        assert_eq!(
            Release::published("1.2.3").tarball("x86_64"),
            "pom-1.2.3-linux-x86_64.tar.gz"
        );
        assert!(manual_commands("1.2.3")[0]
            .contains("https://github.com/pomelohq/pomelo/releases/download/v1.2.3/"));
        assert!(manual_commands("1.2.4-nightly.20261024")[0]
            .contains("/releases/download/nightly/pom-1.2.4-nightly.20261024-linux-"));
    }

    fn offer(version: &str) -> Offer {
        Offer {
            version: Version::parse(version).unwrap_or_else(|| panic!("{version}")),
            file_name: format!("pom-{version}-linux-x86_64.tar.gz"),
            url: String::new(),
            signature_url: String::new(),
            size: None,
        }
    }

    #[test]
    fn the_apps_own_version_wins_and_a_nightly_gone_from_the_release_takes_the_newest() {
        let offers = vec![
            offer("0.9.8"),
            offer("0.9.9-nightly.20261023"),
            offer("0.9.9-nightly.20261024"),
        ];
        let picked = |wanted: &str| pick(&offers, wanted).map(|offer| offer.version.to_string());
        assert_eq!(picked("0.9.8"), Some("0.9.8".into()));
        assert_eq!(
            picked("0.9.9-nightly.20261023"),
            Some("0.9.9-nightly.20261023".into())
        );
        assert_eq!(
            picked("0.9.9-nightly.20261001"),
            Some("0.9.9-nightly.20261024".into())
        );
        assert_eq!(picked("0.9.7"), None);
        assert_eq!(picked("not a version"), None);
    }

    #[test]
    fn only_https_is_fetched_and_odd_versions_are_refused() {
        assert!(download("http://example.com/x", Duration::from_secs(1)).is_err());
        assert!(install_script("1.2.3; rm -rf /").is_err());
        assert!(install_script("1.2.3")
            .is_ok_and(|script| script.contains("pom-1.2.3/install.sh\" --cli-only")));
    }
}
