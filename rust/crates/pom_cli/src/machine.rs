//! `pom ps` and `pom disk`: what Pomelo costs this machine.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pom_paths::StateDir;
use pom_ptyhost::SocketDir;

use crate::config::table;
use crate::say;

const WATCH_EVERY: Duration = Duration::from_secs(2);

struct Process {
    parent: i32,
    cpu: f64,
    rss_kb: u64,
    command: String,
}

fn process_table() -> HashMap<i32, Process> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,pcpu=,rss=,comm="])
        .output()
    else {
        return HashMap::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let cpu = fields.next()?.parse().ok()?;
            let rss_kb = fields.next()?.parse().ok()?;
            let command = fields.collect::<Vec<_>>().join(" ");
            Some((
                pid,
                Process {
                    parent,
                    cpu,
                    rss_kb,
                    command,
                },
            ))
        })
        .collect()
}

type Children = HashMap<i32, Vec<i32>>;

fn children_of(processes: &HashMap<i32, Process>) -> Children {
    let mut children: Children = HashMap::new();
    for (pid, process) in processes {
        children.entry(process.parent).or_default().push(*pid);
    }
    children
}

/// CPU percent and resident kilobytes of `root` and everything under it.
fn tree_total(processes: &HashMap<i32, Process>, children: &Children, root: i32) -> (f64, u64) {
    let mut stack = vec![root];
    let (mut cpu, mut rss) = (0.0, 0);
    while let Some(pid) = stack.pop() {
        if let Some(process) = processes.get(&pid) {
            cpu += process.cpu;
            rss += process.rss_kb;
        }
        stack.extend(children.get(&pid).into_iter().flatten());
    }
    (cpu, rss)
}

/// A holder with the CPU and memory of its whole process tree, and its children one level down.
fn holder_rows(holders: &[(String, i32)], processes: &HashMap<i32, Process>) -> Vec<Vec<String>> {
    let children = children_of(processes);
    let mut rows = vec![vec![
        "HOLDER".to_string(),
        "PID".to_string(),
        "CPU%".to_string(),
        "MEM".to_string(),
    ]];
    let (mut all_cpu, mut all_rss) = (0.0, 0);
    for (name, pid) in holders {
        let (cpu, rss) = tree_total(processes, &children, *pid);
        all_cpu += cpu;
        all_rss += rss;
        rows.push(vec![
            name.clone(),
            pid.to_string(),
            format!("{cpu:.1}"),
            megabytes(rss),
        ]);
        let mut kids = children.get(pid).cloned().unwrap_or_default();
        kids.sort();
        for kid in kids {
            let (cpu, rss) = tree_total(processes, &children, kid);
            let command = processes
                .get(&kid)
                .map_or("", |process| process.command.as_str());
            let short = short_command(command);
            rows.push(vec![
                format!("  {short}"),
                kid.to_string(),
                format!("{cpu:.1}"),
                megabytes(rss),
            ]);
        }
    }
    rows.push(vec![
        "total".to_string(),
        String::new(),
        format!("{all_cpu:.1}"),
        megabytes(all_rss),
    ]);
    rows
}

/// One holder as `pom ps` sees it: its workspace (from its pidfile) and the workspace cgroup it runs in.
pub(crate) struct HolderSeen {
    pub name: String,
    pub pid: i32,
    pub workspace: Option<(String, String)>,
    pub cgroup: Option<PathBuf>,
    pub cpu: f64,
    pub memory_bytes: u64,
}

/// What one workspace's holders use together.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(crate) struct WorkspaceUsage {
    pub project: String,
    pub workspace: String,
    pub holders: Vec<String>,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub cpu_limit_percent: Option<u32>,
    pub memory_limit_bytes: Option<u64>,
    /// `cgroup` when the kernel's accounting for the workspace cgroup is used, else `processes`.
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<PathBuf>,
}

/// Holders grouped by workspace. A workspace whose holders all sit in one cgroup is measured from it, from two
/// readings `interval` apart (that also counts processes that left a holder's tree); otherwise the holders'
/// process trees are summed.
pub(crate) fn workspace_usage(
    holders: &[HolderSeen],
    first: &HashMap<PathBuf, pom_ptyhost::limits::CgroupUsage>,
    second: &HashMap<PathBuf, pom_ptyhost::limits::CgroupUsage>,
    interval: Duration,
) -> Vec<WorkspaceUsage> {
    let mut grouped: std::collections::BTreeMap<(String, String), Vec<&HolderSeen>> =
        Default::default();
    for holder in holders {
        if let Some(workspace) = &holder.workspace {
            grouped.entry(workspace.clone()).or_default().push(holder);
        }
    }
    grouped
        .into_iter()
        .map(|((project, workspace), members)| {
            let names = members.iter().map(|holder| holder.name.clone()).collect();
            let shared = members
                .first()
                .and_then(|holder| holder.cgroup.clone())
                .filter(|cgroup| {
                    members
                        .iter()
                        .all(|holder| holder.cgroup.as_ref() == Some(cgroup))
                });
            let measured = shared.as_ref().and_then(|cgroup| {
                let (before, after) = (first.get(cgroup)?, second.get(cgroup)?);
                let spent = after.cpu_usec.saturating_sub(before.cpu_usec) as f64;
                let cpu = spent / interval.as_micros().max(1) as f64 * 100.0;
                Some((cpu, *after))
            });
            match measured {
                Some((cpu, usage)) => WorkspaceUsage {
                    project,
                    workspace,
                    holders: names,
                    cpu_percent: cpu,
                    memory_bytes: usage.memory_bytes,
                    cpu_limit_percent: usage.cpu_max_percent,
                    memory_limit_bytes: usage.memory_max,
                    source: "cgroup",
                    cgroup: shared,
                },
                None => WorkspaceUsage {
                    project,
                    workspace,
                    holders: names,
                    cpu_percent: members.iter().map(|holder| holder.cpu).sum(),
                    memory_bytes: members.iter().map(|holder| holder.memory_bytes).sum(),
                    cpu_limit_percent: None,
                    memory_limit_bytes: None,
                    source: "processes",
                    cgroup: None,
                },
            }
        })
        .collect()
}

const CGROUP_SAMPLE: Duration = Duration::from_millis(500);

fn seen_holders(holders: &SocketDir, processes: &HashMap<i32, Process>) -> Vec<HolderSeen> {
    let children = children_of(processes);
    let mut listed = holders.holders();
    listed.sort();
    listed
        .into_iter()
        .map(|(name, pid)| {
            let (cpu, rss) = tree_total(processes, &children, pid);
            let cgroup = pom_ptyhost::limits::cgroup_of(
                Path::new("/proc"),
                Path::new("/sys/fs/cgroup"),
                pid,
            )
            .and_then(|path| pom_ptyhost::limits::workspace_cgroup(&path));
            HolderSeen {
                workspace: holders.holder_group(&name),
                name,
                pid,
                cgroup,
                cpu,
                memory_bytes: rss * 1024,
            }
        })
        .collect()
}

fn measure(holders: &[HolderSeen]) -> Vec<WorkspaceUsage> {
    let cgroups: Vec<PathBuf> = holders
        .iter()
        .filter_map(|holder| holder.cgroup.clone())
        .collect();
    let read = || -> HashMap<PathBuf, pom_ptyhost::limits::CgroupUsage> {
        cgroups
            .iter()
            .filter_map(|cgroup| Some((cgroup.clone(), pom_ptyhost::limits::read_usage(cgroup)?)))
            .collect()
    };
    let first = read();
    if !first.is_empty() {
        std::thread::sleep(CGROUP_SAMPLE);
    }
    let second = read();
    workspace_usage(holders, &first, &second, CGROUP_SAMPLE)
}

fn limit_text(usage: &WorkspaceUsage) -> String {
    let cpu = usage.cpu_limit_percent.map(|percent| format!("{percent}%"));
    let memory = usage
        .memory_limit_bytes
        .map(|bytes| megabytes(bytes / 1024));
    match (cpu, memory) {
        (None, None) => "-".to_string(),
        (cpu, memory) => format!(
            "{} / {}",
            cpu.unwrap_or_else(|| "-".into()),
            memory.unwrap_or_else(|| "-".into())
        ),
    }
}

fn workspace_rows(usage: &[WorkspaceUsage]) -> Vec<Vec<String>> {
    let mut rows = vec![vec![
        "WORKSPACE".to_string(),
        "HOLDERS".to_string(),
        "CPU%".to_string(),
        "MEM".to_string(),
        "LIMIT".to_string(),
        "FROM".to_string(),
    ]];
    for workspace in usage {
        rows.push(vec![
            format!("{}/{}", workspace.project, workspace.workspace),
            workspace.holders.len().to_string(),
            format!("{:.1}", workspace.cpu_percent),
            megabytes(workspace.memory_bytes / 1024),
            limit_text(workspace),
            workspace.source.to_string(),
        ]);
    }
    rows
}

const COMMAND_WIDTH: usize = 48;

/// An executable path shows as its file name; a process that renamed itself keeps its title.
fn short_command(command: &str) -> String {
    let name = if command.starts_with('/') {
        Path::new(command)
            .file_name()
            .map_or(command.to_string(), |name| {
                name.to_string_lossy().into_owned()
            })
    } else {
        command.to_string()
    };
    if name.chars().count() > COMMAND_WIDTH {
        let kept: String = name.chars().take(COMMAND_WIDTH - 3).collect();
        format!("{kept}...")
    } else {
        name
    }
}

fn megabytes(kilobytes: u64) -> String {
    format!("{:.0}M", kilobytes as f64 / 1024.0)
}

pub(crate) fn ps(watch: bool, json: bool, out: &mut dyn Write) -> Result<(), String> {
    let holders = SocketDir::from_env();
    loop {
        let processes = process_table();
        let seen = seen_holders(&holders, &processes);
        let usage = measure(&seen);
        if json {
            let document = serde_json::json!({
                "schema": "pom.ps/v1",
                "workspaces": usage,
                "holders": seen.iter().map(|holder| serde_json::json!({
                    "name": holder.name,
                    "pid": holder.pid,
                    "project": holder.workspace.as_ref().map(|(project, _)| project),
                    "workspace": holder.workspace.as_ref().map(|(_, workspace)| workspace),
                    "cpu_percent": holder.cpu,
                    "memory_bytes": holder.memory_bytes,
                })).collect::<Vec<_>>(),
            });
            return say(out, &document.to_string());
        }
        if watch {
            write!(out, "\x1b[H\x1b[2J").map_err(|error| error.to_string())?;
        }
        let pairs: Vec<(String, i32)> = seen
            .iter()
            .map(|holder| (holder.name.clone(), holder.pid))
            .collect();
        if pairs.is_empty() {
            say(out, "no holders running")?;
        } else {
            if !usage.is_empty() {
                table(out, "", &workspace_rows(&usage))?;
                say(out, "")?;
            }
            table(out, "", &holder_rows(&pairs, &processes))?;
        }
        if !watch {
            return Ok(());
        }
        out.flush().map_err(|error| error.to_string())?;
        std::thread::sleep(WATCH_EVERY);
    }
}

fn size_mb(path: &Path) -> u64 {
    std::process::Command::new("du")
        .arg("-sm")
        .arg(path)
        .output()
        .ok()
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next()
                .and_then(|size| size.parse().ok())
        })
        .unwrap_or(0)
}

fn format_mb(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1}G", mb as f64 / 1024.0)
    } else {
        format!("{mb}M")
    }
}

/// The filesystems of home, the state dir and Docker's data, the LVM thin pools, and the warnings about them.
pub(crate) struct DiskCheck {
    pub filesystems: Vec<pom_doctor::Filesystem>,
    pub pools: Option<Vec<pom_doctor::ThinPool>>,
    pub findings: Vec<pom_doctor::Finding>,
}

pub(crate) fn disk_check(state: &StateDir) -> DiskCheck {
    let path = pom_services::tool_path();
    let mut places = Vec::new();
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        places.push(pom_doctor::Place {
            name: "home".into(),
            path: home.into(),
        });
    }
    places.push(pom_doctor::Place {
        name: "state".into(),
        path: nearest_existing(state.root()),
    });
    if pom_doctor::on_path(path, "docker") {
        if let Some(root) = pom_doctor::docker_root(path) {
            places.push(pom_doctor::Place {
                name: "docker".into(),
                path: root,
            });
        }
    }
    let filesystems = pom_doctor::filesystems(&places, &pom_doctor::df_usage);
    let pools = pom_doctor::thin_pools(path);
    let findings = pom_doctor::disk_findings(&filesystems, pools.as_deref().unwrap_or_default());
    DiskCheck {
        filesystems,
        pools,
        findings,
    }
}

/// The state dir may not exist yet; its filesystem is its nearest existing parent's.
fn nearest_existing(path: &Path) -> std::path::PathBuf {
    path.ancestors()
        .find(|ancestor| ancestor.exists())
        .unwrap_or(path)
        .to_path_buf()
}

pub(crate) fn disk(state: &StateDir, json: bool, out: &mut dyn Write) -> Result<(), String> {
    let check = disk_check(state);
    if json {
        let report = serde_json::json!({
            "schema": "pom.disk/v1",
            "filesystems": check.filesystems,
            "thin_pools": check.pools,
            "findings": check.findings,
            "warn_percent": pom_doctor::FULL_PERCENT,
        });
        return say(
            out,
            &serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?,
        );
    }
    for filesystem in &check.filesystems {
        let usage = &filesystem.usage;
        let names: Vec<&str> = filesystem
            .places
            .iter()
            .map(|place| place.name.as_str())
            .collect();
        say(
            out,
            &format!(
                "disk {}  {}% used  free {}  ({})",
                usage.mount,
                usage.used_percent,
                pom_doctor::format_bytes(usage.available_bytes),
                names.join(", ")
            ),
        )?;
    }
    for finding in &check.findings {
        say(out, &format!("warn  {}", finding.title))?;
        say(out, &format!("      {}", finding.detail))?;
        say(out, &format!("      free space: {}", finding.fix))?;
    }
    let store = std::process::Command::new("pnpm")
        .args(["store", "path"])
        .env("PATH", pom_services::tool_path())
        .output();
    if let Some(output) = store.ok().filter(|output| output.status.success()) {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            say(
                out,
                &format!(
                    "pnpm store  {}  ({path})",
                    format_mb(size_mb(Path::new(&path)))
                ),
            )?;
        }
    }
    let projects = pom_sessions::Projects::load(state).projects;
    if projects.is_empty() {
        return say(out, "\nno registered projects");
    }
    for (session, root) in projects {
        let root = Path::new(&root);
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let workspaces: Vec<_> = entries
            .flatten()
            .filter(|entry| {
                entry.path().is_dir()
                    && entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(pom_layout::WORKSPACE_PREFIX)
            })
            .map(|entry| entry.path())
            .collect();
        say(
            out,
            &format!("\n{session}  ({} workspaces)", workspaces.len()),
        )?;
        let mut per_repo: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
        for workspace in &workspaces {
            for repo in std::fs::read_dir(workspace).into_iter().flatten().flatten() {
                let modules = repo.path().join("node_modules");
                if !modules.is_dir() {
                    continue;
                }
                let entry = per_repo
                    .entry(repo.file_name().to_string_lossy().into_owned())
                    .or_default();
                entry.0 += 1;
                entry.1 += size_mb(&modules);
            }
        }
        if per_repo.is_empty() {
            say(out, "  no node_modules")?;
            continue;
        }
        let mut rows = vec![vec![
            "REPO".to_string(),
            "EACH".to_string(),
            "COPIES".to_string(),
            "TOTAL".to_string(),
        ]];
        let mut sorted: Vec<_> = per_repo.into_iter().collect();
        sorted.sort_by_key(|(_, (_, total))| std::cmp::Reverse(*total));
        let mut grand = 0;
        for (repo, (copies, total)) in sorted {
            grand += total;
            rows.push(vec![
                repo,
                format_mb(total / copies.max(1)),
                copies.to_string(),
                format_mb(total),
            ]);
        }
        rows.push(vec![
            "all".to_string(),
            String::new(),
            String::new(),
            format_mb(grand),
        ]);
        table(out, "  ", &rows)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(
        name: &str,
        workspace: Option<(&str, &str)>,
        cgroup: Option<&str>,
        cpu: f64,
        memory: u64,
    ) -> HolderSeen {
        HolderSeen {
            name: name.into(),
            pid: 1,
            workspace: workspace.map(|(project, workspace)| (project.into(), workspace.into())),
            cgroup: cgroup.map(PathBuf::from),
            cpu,
            memory_bytes: memory,
        }
    }

    #[test]
    fn a_workspace_in_one_cgroup_is_measured_from_it_and_the_rest_from_their_processes() {
        use pom_ptyhost::limits::CgroupUsage;
        let slice = "/sys/fs/cgroup/pom-p-feat.slice";
        let holders = [
            holder(
                "svc-p-feat-api-web",
                Some(("p", "feat")),
                Some(slice),
                5.0,
                100,
            ),
            holder(
                "ws-p-feat-claude-raw",
                Some(("p", "feat")),
                Some(slice),
                5.0,
                100,
            ),
            holder("svc-p-main-api-web", Some(("p", "main")), None, 12.5, 300),
            holder("term-p-main-1", Some(("p", "main")), None, 1.5, 200),
            holder("shr-p-mock", None, None, 50.0, 999),
        ];
        let reading = |usec: u64| CgroupUsage {
            memory_bytes: 256 << 20,
            cpu_usec: usec,
            memory_max: Some(512 << 20),
            cpu_max_percent: Some(200),
        };
        let first = HashMap::from([(PathBuf::from(slice), reading(1_000_000))]);
        let second = HashMap::from([(PathBuf::from(slice), reading(1_500_000))]);
        let usage = workspace_usage(&holders, &first, &second, Duration::from_secs(1));
        assert_eq!(usage.len(), 2, "the shared holder belongs to no workspace");
        let feat = &usage[0];
        assert_eq!((feat.workspace.as_str(), feat.source), ("feat", "cgroup"));
        assert_eq!(feat.cpu_percent, 50.0);
        assert_eq!(feat.memory_bytes, 256 << 20);
        assert_eq!(limit_text(feat), "200% / 512M");
        let main = &usage[1];
        assert_eq!(
            (main.workspace.as_str(), main.source),
            ("main", "processes")
        );
        assert_eq!((main.cpu_percent, main.memory_bytes), (14.0, 500));
        assert_eq!(main.holders.len(), 2);
        assert_eq!(limit_text(main), "-");
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(format_mb(512), "512M");
        assert_eq!(format_mb(1536), "1.5G");
        assert_eq!(megabytes(2048), "2M");
        assert_eq!(short_command("/usr/local/bin/node"), "node");
        assert_eq!(
            short_command("puma 6.4 (tcp://0.0.0.0:3000) [api]"),
            "puma 6.4 (tcp://0.0.0.0:3000) [api]"
        );
        assert!(short_command(&"x".repeat(80)).ends_with("..."));
    }
}
