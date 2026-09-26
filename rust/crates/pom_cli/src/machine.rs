//! `pom ps` and `pom disk`: what Pomelo costs this machine.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
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

/// A holder with the CPU and memory of its whole process tree, and its children one level down.
fn holder_rows(holders: &SocketDir) -> Vec<Vec<String>> {
    let processes = process_table();
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for (pid, process) in &processes {
        children.entry(process.parent).or_default().push(*pid);
    }
    let tree_total = |root: i32| {
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
    };
    let mut holders = holders.holders();
    holders.sort();
    let mut rows = vec![vec![
        "HOLDER".to_string(),
        "PID".to_string(),
        "CPU%".to_string(),
        "MEM".to_string(),
    ]];
    let (mut all_cpu, mut all_rss) = (0.0, 0);
    for (name, pid) in holders {
        let (cpu, rss) = tree_total(pid);
        all_cpu += cpu;
        all_rss += rss;
        rows.push(vec![
            name,
            pid.to_string(),
            format!("{cpu:.1}"),
            megabytes(rss),
        ]);
        let mut kids = children.get(&pid).cloned().unwrap_or_default();
        kids.sort();
        for kid in kids {
            let (cpu, rss) = tree_total(kid);
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

pub(crate) fn ps(watch: bool, out: &mut dyn Write) -> Result<(), String> {
    let holders = SocketDir::from_env();
    loop {
        let rows = holder_rows(&holders);
        if watch {
            write!(out, "\x1b[H\x1b[2J").map_err(|error| error.to_string())?;
        }
        if rows.len() <= 2 {
            say(out, "no holders running")?;
        } else {
            table(out, "", &rows)?;
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

pub(crate) fn disk(state: &StateDir, out: &mut dyn Write) -> Result<(), String> {
    if let Some(home) = std::env::var_os("HOME") {
        let output = std::process::Command::new("df")
            .arg("-h")
            .arg(&home)
            .output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            let fields: Vec<&str> = text
                .lines()
                .nth(1)
                .unwrap_or("")
                .split_whitespace()
                .collect();
            if fields.len() >= 4 {
                say(
                    out,
                    &format!("disk  used {}  free {}", fields[2], fields[3]),
                )?;
            }
        }
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
