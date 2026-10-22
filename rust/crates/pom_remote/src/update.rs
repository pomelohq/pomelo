//! Updating pom on a host from the app. The tarball for the host's machine comes from the app's own GitHub
//! release and is checked against that release's checksums here, before a byte reaches the host; it then goes
//! through ssh into a temp folder there, whose install.sh puts pom in place. The host never downloads anything.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::{Host, Ssh};

pub const RELEASES: &str = "https://github.com/pomelohq/pomelo/releases/download";
/// The `sha256sum` list the release publishes for its Linux tarballs.
pub const CHECKSUMS: &str = "checksums-linux.txt";
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

/// Where a version's tarballs are published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The folder of the release's files; only `https://` (and `file://`, for tests) is fetched.
    pub base: String,
}

impl Release {
    pub fn published(version: &str) -> Release {
        Release {
            version: version.to_string(),
            base: format!("{RELEASES}/v{version}"),
        }
    }

    pub fn tarball(&self, arch: &str) -> String {
        format!("pom-{}-linux-{arch}.tar.gz", self.version)
    }
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

/// The sha256 a `sha256sum` list gives `file`.
pub fn checksum_for(list: &str, file: &str) -> Option<String> {
    list.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*');
        (name == file && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

/// Installs `release`'s pom on `host`: the tarball for its machine, checked against the release's checksums
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
    let base = release.base.trim_end_matches('/');
    let tarball_name = release.tarball(arch);
    let checksums = download(&format!("{base}/{CHECKSUMS}"), DOWNLOAD_LIMIT)?;
    let tarball = download(&format!("{base}/{tarball_name}"), DOWNLOAD_LIMIT)?;
    progress(UpdateStep::Verify);
    let expected =
        checksum_for(&String::from_utf8_lossy(&checksums), &tarball_name).ok_or_else(|| {
            format!(
                "{CHECKSUMS} of v{} lists no {tarball_name}",
                release.version
            )
        })?;
    if sha256_hex(&tarball) != expected {
        return Err(format!(
            "{tarball_name} does not match the release's checksum; nothing was installed"
        ));
    }
    progress(UpdateStep::Install);
    run_script(
        ssh,
        host,
        &install_script(&release.version)?,
        &tarball,
        SCRIPT_LIMIT,
    )?;
    Ok(())
}

/// What to run on `host` by hand when the app cannot install pom itself (a build with no release).
pub fn manual_commands(version: &str) -> Vec<String> {
    let base = Release::published(version).base;
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
        assert_eq!(
            Release::published("1.2.3").base,
            "https://github.com/pomelohq/pomelo/releases/download/v1.2.3"
        );
    }

    #[test]
    fn a_checksum_list_is_read_as_sha256sum_writes_it() {
        let hash = "a".repeat(64);
        let list = format!(
            "{hash}  pom-1.2.3-linux-x86_64.tar.gz\n{}  pom-1.2.3-linux-aarch64.tar.gz\n",
            "b".repeat(64)
        );
        assert_eq!(
            checksum_for(&list, "pom-1.2.3-linux-x86_64.tar.gz"),
            Some(hash)
        );
        assert_eq!(checksum_for(&list, "pom-1.2.3-linux-riscv.tar.gz"), None);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn only_https_is_fetched_and_odd_versions_are_refused() {
        assert!(download("http://example.com/x", Duration::from_secs(1)).is_err());
        assert!(install_script("1.2.3; rm -rf /").is_err());
        assert!(install_script("1.2.3")
            .is_ok_and(|script| script.contains("pom-1.2.3/install.sh\" --cli-only")));
    }
}
