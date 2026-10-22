//! Disk space: the filesystems Pomelo writes to (home, its state, Docker's data) and LVM thin pools, each
//! warned about past 80% with the commands that free space.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{Finding, Severity};

pub const FULL_PERCENT: u8 = 80;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

/// What Pomelo keeps on a filesystem: `home`, `state` or `docker`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub device: String,
    pub mount: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Filesystem {
    pub places: Vec<Place>,
    #[serde(flatten)]
    pub usage: Usage,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ThinPool {
    pub name: String,
    pub data_percent: f64,
    pub metadata_percent: Option<f64>,
}

/// The filesystems holding `places`, one entry per mount; a place `usage` cannot read is left out.
pub fn filesystems(places: &[Place], usage: &dyn Fn(&Path) -> Option<Usage>) -> Vec<Filesystem> {
    let mut found: Vec<Filesystem> = Vec::new();
    for place in places {
        let Some(read) = usage(&place.path) else {
            continue;
        };
        match found
            .iter_mut()
            .find(|filesystem| filesystem.usage.mount == read.mount)
        {
            Some(filesystem) => filesystem.places.push(place.clone()),
            None => found.push(Filesystem {
                places: vec![place.clone()],
                usage: read,
            }),
        }
    }
    found
}

/// `df -Pk <path>`: POSIX output, the same on macOS and Linux.
pub fn df_usage(path: &Path) -> Option<Usage> {
    let mut command = Command::new("df");
    command.arg("-Pk").arg(path);
    parse_df(&output_within(&mut command, COMMAND_TIMEOUT)?)
}

pub fn parse_df(text: &str) -> Option<Usage> {
    let line = text.lines().nth(1)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 6 {
        return None;
    }
    let kilobytes = |field: &str| field.parse::<u64>().ok().map(|kb| kb * 1024);
    let size_bytes = kilobytes(fields[1])?;
    let used_bytes = kilobytes(fields[2])?;
    let available_bytes = kilobytes(fields[3])?;
    let used_percent = fields[4].trim_end_matches('%').parse().ok()?;
    Some(Usage {
        device: fields[0].to_string(),
        mount: fields[5..].join(" "),
        size_bytes,
        used_bytes,
        available_bytes,
        used_percent,
    })
}

/// Where Docker keeps images and volumes, from one `docker info` call; `None` when Docker does not answer.
pub fn docker_root(path_env: &str) -> Option<PathBuf> {
    let mut command = Command::new("docker");
    command
        .args(["info", "--format", "{{.DockerRootDir}}"])
        .env("PATH", path_env);
    let root = output_within(&mut command, COMMAND_TIMEOUT)?;
    let root = root.trim();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

/// LVM thin pools and volumes, when `lvs` runs without root; `None` on other systems or without permission.
pub fn thin_pools(path_env: &str) -> Option<Vec<ThinPool>> {
    if !cfg!(target_os = "linux") || !crate::on_path(path_env, "lvs") {
        return None;
    }
    let mut command = Command::new("lvs");
    command
        .args([
            "--noheadings",
            "--separator",
            ",",
            "-o",
            "lv_name,data_percent,metadata_percent",
        ])
        .env("PATH", path_env);
    Some(parse_lvs(&output_within(&mut command, COMMAND_TIMEOUT)?))
}

pub fn parse_lvs(text: &str) -> Vec<ThinPool> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let name = fields.first().filter(|name| !name.is_empty())?;
            let percent = |index: usize| {
                fields
                    .get(index)
                    .and_then(|field| field.parse::<f64>().ok())
            };
            Some(ThinPool {
                name: name.to_string(),
                data_percent: percent(1)?,
                metadata_percent: percent(2),
            })
        })
        .collect()
}

fn cleanup_commands(places: &[Place]) -> Vec<&'static str> {
    let mut commands = Vec::new();
    if places.iter().any(|place| place.name != "docker") {
        commands.extend([
            "pom ws gc --dry-run (then pom ws gc) to delete workspaces whose PRs merged",
            "pom modules prune",
            "pom db clean --dry-run (then pom db clean)",
        ]);
    }
    if places.iter().any(|place| place.name == "docker") {
        commands.extend([
            "docker system df (what uses the space)",
            "docker builder prune",
            "docker image prune -a",
        ]);
    }
    commands
}

/// A warning for each filesystem past [`FULL_PERCENT`], with what to run to free space.
pub fn disk_findings(filesystems: &[Filesystem], pools: &[ThinPool]) -> Vec<Finding> {
    let mut findings = Vec::new();
    for filesystem in filesystems {
        let usage = &filesystem.usage;
        if usage.used_percent <= FULL_PERCENT {
            continue;
        }
        let names: Vec<&str> = filesystem
            .places
            .iter()
            .map(|place| place.name.as_str())
            .collect();
        findings.push(
            Finding::new(
                format!("disk.full:{}", usage.mount),
                Severity::Warn,
                format!(
                    "Disk {}% full: {} ({})",
                    usage.used_percent,
                    usage.mount,
                    names.join(", ")
                ),
            )
            .detail(format!(
                "{} free of {} on {}",
                format_bytes(usage.available_bytes),
                format_bytes(usage.size_bytes),
                usage.device
            ))
            .fix(cleanup_commands(&filesystem.places).join("; ")),
        );
    }
    for pool in pools {
        let metadata = pool.metadata_percent.unwrap_or(0.0);
        let limit = f64::from(FULL_PERCENT);
        if pool.data_percent <= limit && metadata <= limit {
            continue;
        }
        let mut detail = format!("data {:.1}%", pool.data_percent);
        if let Some(metadata) = pool.metadata_percent {
            detail.push_str(&format!(", metadata {metadata:.1}%"));
        }
        findings.push(
            Finding::new(
                format!("disk.thin:{}", pool.name),
                Severity::Warn,
                format!("LVM thin pool {} is filling up", pool.name),
            )
            .detail(format!(
                "{detail}; a full thin pool stops every volume on it"
            ))
            .fix("free space inside its volumes (sudo fstrim -av), or grow it: sudo lvextend -L +20G <vg>/<pool> (metadata: --poolmetadatasize +1G)"),
        );
    }
    findings
}

pub fn format_bytes(bytes: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const MB: f64 = (1u64 << 20) as f64;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.1} GB", value / GB)
    } else {
        format!("{:.0} MB", value / MB)
    }
}

/// Runs `command`, killing it past `timeout`; its stdout when it succeeded.
pub(crate) fn output_within(command: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                if let Err(error) = child.kill() {
                    eprintln!("doctor: stop {command:?}: {error}");
                }
                if let Err(error) = child.wait() {
                    eprintln!("doctor: reap {command:?}: {error}");
                }
                return None;
            }
        }
    };
    let text = reader.join().ok()?.ok()?;
    status.success().then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(mount: &str, percent: u8) -> Usage {
        Usage {
            device: "/dev/sda1".into(),
            mount: mount.into(),
            size_bytes: 100 << 30,
            used_bytes: u64::from(percent) << 30,
            available_bytes: u64::from(100 - percent) << 30,
            used_percent: percent,
        }
    }

    fn place(name: &str, path: &str) -> Place {
        Place {
            name: name.into(),
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn df_output_reads_on_both_systems() {
        let linux = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/mapper/vg-root   102400000  83968000  18432000      82% /\n";
        let read = parse_df(linux).expect("linux");
        assert_eq!((read.used_percent, read.mount.as_str()), (82, "/"));
        assert_eq!(read.available_bytes, 18_432_000 * 1024);
        let macos = "Filesystem     1024-blocks      Used Available Capacity  Mounted on\n/dev/disk3s5    971350180 500000000 400000000    56%    /System/Volumes/Data\n";
        assert_eq!(
            parse_df(macos).map(|read| read.mount),
            Some("/System/Volumes/Data".into())
        );
        assert_eq!(parse_df("garbage"), None);
    }

    #[test]
    fn a_filesystem_past_80_percent_is_warned_about_once_with_its_cleanup() {
        let places = [
            place("home", "/home/me"),
            place("state", "/home/me/.local/state/pom"),
            place("docker", "/var/lib/docker"),
        ];
        let probe = |path: &Path| match path.to_str() {
            Some("/var/lib/docker") => Some(usage("/var", 80)),
            Some(_) => Some(usage("/", 81)),
            None => None,
        };
        let found = filesystems(&places, &probe);
        assert_eq!(found.len(), 2);
        let findings = disk_findings(&found, &[]);
        assert_eq!(findings.len(), 1, "80% is not yet a warning");
        assert_eq!(findings[0].id, "disk.full:/");
        assert_eq!(findings[0].severity, Severity::Warn);
        assert_eq!(findings[0].title, "Disk 81% full: / (home, state)");
        assert!(findings[0].fix.contains("pom ws gc"), "{}", findings[0].fix);
        assert!(!findings[0].fix.contains("docker"), "{}", findings[0].fix);

        let docker_full = |_: &Path| Some(usage("/", 93));
        let findings = disk_findings(&filesystems(&places, &docker_full), &[]);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].fix.contains("docker builder prune"));
        assert!(findings[0].fix.contains("pom modules prune"));
    }

    #[test]
    fn thin_pools_warn_on_data_or_metadata() {
        let pools = parse_lvs(
            "  data,85.20,10.00\n  root,,\n  meta,12.00,81.50\n  ok,50.00,5.00\n  vol,90.00,\n",
        );
        let names: Vec<&str> = pools.iter().map(|pool| pool.name.as_str()).collect();
        assert_eq!(names, ["data", "meta", "ok", "vol"]);
        let warned: Vec<String> = disk_findings(&[], &pools)
            .into_iter()
            .map(|finding| finding.id)
            .collect();
        assert_eq!(
            warned,
            ["disk.thin:data", "disk.thin:meta", "disk.thin:vol"]
        );
    }
}
