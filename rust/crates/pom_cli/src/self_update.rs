//! `pom self-update`: replace a pom (and the Linux app beside it) that install.sh installed with a newer signed
//! build of its channel, or an exact version. install.sh's `--channel` and `--version` run it too.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use auto_update::{linux_install, Channel, Offer, Version};

use crate::args::Args;

const DOWNLOAD_LIMIT: Duration = Duration::from_secs(600);
const USAGE: &str =
    "usage: pom self-update [--channel stable|nightly] [--version <v>] [--check] [--prefix <dir>] [--app | --cli-only]";

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SelfUpdate {
    pub channel: Option<String>,
    pub version: Option<String>,
    pub check: bool,
    /// Install into this prefix instead of the running pom's own (install.sh passes its own).
    pub prefix: Option<PathBuf>,
    /// The app and pom, or pom alone; unset follows what the prefix has.
    pub app: Option<bool>,
}

pub(crate) fn parse(words: &[&str]) -> Result<SelfUpdate, String> {
    let args = Args::parse(words, &["--channel", "--version", "--prefix"])?;
    args.allow(&[
        "--channel",
        "--version",
        "--prefix",
        "--check",
        "--app",
        "--cli-only",
    ])
    .map_err(|error| format!("{error}\n{USAGE}"))?;
    args.at_most(0, "self-update")?;
    if let Some(channel) = args.value(&["--channel"]) {
        Channel::parse(&channel)
            .ok_or_else(|| format!("unknown channel {channel} (stable or nightly)"))?;
    }
    if let Some(version) = args.value(&["--version"]) {
        Version::parse(version.trim_start_matches('v'))
            .ok_or_else(|| format!("{version} is not a version"))?;
    }
    if args.has("--app") && args.has("--cli-only") {
        return Err(format!("--app and --cli-only exclude each other\n{USAGE}"));
    }
    Ok(SelfUpdate {
        channel: args.value(&["--channel"]),
        version: args
            .value(&["--version"])
            .map(|version| version.trim_start_matches('v').to_string()),
        check: args.has("--check"),
        prefix: args.value(&["--prefix"]).map(PathBuf::from),
        app: match (args.has("--app"), args.has("--cli-only")) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        },
    })
}

/// Which download to install: the exact version asked for, else the newest of the channel when it is newer.
pub(crate) fn choose(
    offers: &[Offer],
    channel: Channel,
    wanted: Option<&Version>,
    current: &Version,
) -> Result<Option<Offer>, String> {
    if let Some(wanted) = wanted {
        return offers
            .iter()
            .find(|offer| &offer.version == wanted)
            .cloned()
            .map(Some)
            .ok_or_else(|| {
                format!("no signed build of {wanted} is published (a nightly stays for 7 nights)")
            });
    }
    let stable_only =
        |offer: &&Offer| channel == Channel::Nightly || !offer.version.is_prerelease();
    Ok(offers
        .iter()
        .filter(stable_only)
        .max_by(|left, right| left.version.cmp(&right.version))
        .filter(|offer| &offer.version > current)
        .cloned())
}

pub(crate) fn execute(request: &SelfUpdate, out: &mut dyn Write) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err(
            "pom comes with Pomelo.app on macOS, and updates with it (Settings > General)".into(),
        );
    }
    let prefix = match &request.prefix {
        Some(prefix) => prefix.clone(),
        None => {
            let exe = std::env::current_exe().map_err(|error| format!("where pom is: {error}"))?;
            let home = std::env::var_os("HOME").map(PathBuf::from);
            linux_install::self_managed_prefix(&exe, "pom", home.as_deref())?
        }
    };
    let channel_name = request
        .channel
        .clone()
        .unwrap_or_else(|| settings::Settings::load().update_channel().to_string());
    let channel = Channel::parse(&channel_name).unwrap_or_default();
    let with_app = request
        .app
        .unwrap_or_else(|| prefix.join("bin/pomelo").is_file());
    let artifact = if with_app {
        linux_install::app_artifact()?
    } else {
        linux_install::cli_artifact()?
    };
    let wanted = request.version.as_deref().and_then(Version::parse);
    // An exact version may be a nightly whichever channel is followed.
    let listed = if wanted.is_some() {
        Channel::Nightly
    } else {
        channel
    };
    let releases = auto_update::fetch_releases(listed)?;
    let offers = auto_update::offers(&releases, listed, artifact);
    let built = crate::VERSION.split('+').next().unwrap_or_default();
    let current = Version::parse(built).ok_or("this pom has no version")?;
    let Some(offer) = choose(&offers, channel, wanted.as_ref(), &current)? else {
        writeln!(
            out,
            "pom {current} is the newest on the {channel_name} channel"
        )
        .map_err(|error| error.to_string())?;
        return Ok(());
    };
    let what = if with_app { "Pomelo and pom" } else { "pom" };
    if request.check {
        writeln!(
            out,
            "{what} {} would replace {current} under {} ({})",
            offer.version,
            prefix.display(),
            offer.file_name
        )
        .map_err(|error| error.to_string())?;
        return Ok(());
    }
    writeln!(out, "downloading {}", offer.file_name).map_err(|error| error.to_string())?;
    let bytes = linux_install::fetch_verified(&offer, DOWNLOAD_LIMIT)?;
    let dir = std::env::temp_dir().join(format!("pom-self-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let installed = (|| {
        let tarball = dir.join(&offer.file_name);
        std::fs::write(&tarball, &bytes)
            .map_err(|e| format!("write {}: {e}", tarball.display()))?;
        let unpacked = linux_install::unpack(&tarball, &dir)?;
        linux_install::run_install(&unpacked, &prefix, !with_app)
    })();
    if let Err(error) = std::fs::remove_dir_all(&dir) {
        eprintln!("self-update: remove {}: {error}", dir.display());
    }
    installed?;
    writeln!(
        out,
        "{what} {} installed under {} (signature verified); a running `pom server` keeps {current} until it restarts: pom server stop",
        offer.version,
        prefix.display()
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    fn offer(text: &str) -> Offer {
        Offer {
            version: version(text),
            file_name: format!("pom-{text}-linux-x86_64.tar.gz"),
            url: String::new(),
            signature_url: String::new(),
            size: None,
        }
    }

    #[test]
    fn flags_parse_and_conflicts_are_refused() {
        assert_eq!(parse(&[]), Ok(SelfUpdate::default()));
        let request = parse(&["--channel", "nightly", "--version", "v0.9.9", "--check"]);
        assert_eq!(
            request.map(|r| (r.channel, r.version, r.check)),
            Ok((Some("nightly".into()), Some("0.9.9".into()), true))
        );
        assert!(parse(&["--channel", "beta"]).is_err());
        assert!(parse(&["--version", "latest"]).is_err());
        assert!(parse(&["--app", "--cli-only"]).is_err());
        assert_eq!(parse(&["--cli-only"]).map(|r| r.app), Ok(Some(false)));
        assert!(parse(&["now"]).is_err());
    }

    #[test]
    fn the_newest_newer_build_or_the_exact_version_is_chosen() {
        let offers = vec![
            offer("0.9.8"),
            offer("0.9.9-nightly.20261023"),
            offer("0.9.9-nightly.20261024"),
        ];
        let current = version("0.9.8");
        let pick = |channel, wanted: Option<&str>| {
            choose(&offers, channel, wanted.map(version).as_ref(), &current)
                .map(|offer| offer.map(|offer| offer.version.to_string()))
        };
        assert_eq!(pick(Channel::Stable, None), Ok(None));
        assert_eq!(
            pick(Channel::Nightly, None),
            Ok(Some("0.9.9-nightly.20261024".into()))
        );
        assert_eq!(
            pick(Channel::Stable, Some("0.9.9-nightly.20261023")),
            Ok(Some("0.9.9-nightly.20261023".into()))
        );
        assert!(pick(Channel::Stable, Some("0.9.9-nightly.20261001")).is_err());
    }
}
