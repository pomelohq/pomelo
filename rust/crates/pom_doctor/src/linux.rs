//! Linux hosts that are about to hang: Docker nested in an LXC container, a load far past the CPUs, and
//! processes stuck in uninterruptible sleep. Everything is read from `/proc` and `/run` under `root`, so tests
//! point it at a fixture; nothing runs inside a container.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::disk::output_within;
use crate::{Finding, Severity};

const STUCK_FOR: Duration = Duration::from_secs(30);
/// `/proc/<pid>/stat` counts start times in clock ticks; Linux exports USER_HZ = 100 on every architecture.
const TICKS_PER_SECOND: f64 = 100.0;
const LISTED: usize = 8;

/// What `systemd-detect-virt --container` says, `None` without it or outside a container.
pub fn detect_container(path_env: &str) -> Option<String> {
    if !crate::on_path(path_env, "systemd-detect-virt") {
        return None;
    }
    let mut command = Command::new("systemd-detect-virt");
    command.arg("--container").env("PATH", path_env);
    let kind = output_within(&mut command, Duration::from_secs(3))?;
    let kind = kind.trim();
    (!kind.is_empty() && kind != "none").then(|| kind.to_string())
}

/// Whether this machine is an LXC container: by `detected` (systemd-detect-virt), PID 1's environment, or
/// systemd's `/run/systemd/container`.
pub fn in_lxc(root: &Path, detected: Option<&str>) -> bool {
    if detected.is_some_and(|kind| kind.starts_with("lxc")) {
        return true;
    }
    let pid1 = std::fs::read(root.join("proc/1/environ")).unwrap_or_default();
    if pid1
        .split(|byte| *byte == 0)
        .any(|entry| entry.starts_with(b"container=lxc"))
    {
        return true;
    }
    std::fs::read_to_string(root.join("run/systemd/container"))
        .is_ok_and(|kind| kind.trim().starts_with("lxc"))
}

pub fn lxc_docker(in_lxc: bool, has_docker: bool) -> Option<Finding> {
    (in_lxc && has_docker).then(|| {
        Finding::new(
            "linux.lxc_docker",
            Severity::Warn,
            "Docker runs nested inside an LXC container",
        )
        .detail("nested Docker in LXC is known to hang `runc exec` (docker exec, exec healthchecks) when the host kernel misbehaves, which can freeze the whole host")
        .fix("run Pomelo in a VM instead of a container; if it must stay in LXC, give the container `features: nesting=1,keyctl=1`, keep its kernel current, and avoid periodic `docker exec`")
    })
}

/// A warning when the 1-minute load average from `/proc/loadavg` is above twice `cpus`.
pub fn high_load(root: &Path, cpus: usize) -> Option<Finding> {
    let text = std::fs::read_to_string(root.join("proc/loadavg")).ok()?;
    let load: f64 = text.split_whitespace().next()?.parse().ok()?;
    let limit = 2.0 * cpus.max(1) as f64;
    (load > limit).then(|| {
        Finding::new("linux.load", Severity::Warn, "The machine is overloaded")
            .detail(format!(
                "load average {load:.2} over the last minute on {cpus} CPUs (more than twice as many runnable or blocked tasks as CPUs)"
            ))
            .fix("see what runs with `pom ps` and `top`; stop services you do not need (`pom stop`), or give the machine more CPUs")
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Task {
    comm: String,
    state: char,
    start_ticks: u64,
}

fn parse_stat(text: &str) -> Option<Task> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_string();
    let fields: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    let state = fields.first()?.chars().next()?;
    // Field 22 of stat is the start time; fields here start at field 3, the state.
    let start_ticks = fields.get(19)?.parse().ok()?;
    Some(Task {
        comm,
        state,
        start_ticks,
    })
}

fn tasks_in_d(root: &Path) -> HashMap<u32, Task> {
    let Ok(entries) = std::fs::read_dir(root.join("proc")) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let text = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let task = parse_stat(&text)?;
            (task.state == 'D').then_some((pid, task))
        })
        .collect()
}

/// Processes in uninterruptible sleep (`D`) at two scans `pause` apart that started more than 30 s ago,
/// as `(pid, comm)`; a second scan only runs when the first finds one.
pub fn stuck_processes(root: &Path, pause: &mut dyn FnMut()) -> Vec<(u32, String)> {
    let first = tasks_in_d(root);
    if first.is_empty() {
        return Vec::new();
    }
    pause();
    let second = tasks_in_d(root);
    let uptime: Option<f64> = std::fs::read_to_string(root.join("proc/uptime"))
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse().ok());
    let mut stuck: Vec<(u32, String)> = first
        .into_iter()
        .filter(|(pid, task)| {
            second
                .get(pid)
                .is_some_and(|again| again.start_ticks == task.start_ticks)
        })
        .filter(|(_, task)| {
            uptime.is_some_and(|uptime| {
                uptime - task.start_ticks as f64 / TICKS_PER_SECOND > STUCK_FOR.as_secs_f64()
            })
        })
        .map(|(pid, task)| (pid, task.comm))
        .collect();
    stuck.sort();
    stuck
}

pub fn stuck_finding(stuck: &[(u32, String)]) -> Option<Finding> {
    if stuck.is_empty() {
        return None;
    }
    let mut listed: Vec<String> = stuck
        .iter()
        .take(LISTED)
        .map(|(pid, comm)| format!("{pid} ({comm})"))
        .collect();
    if stuck.len() > LISTED {
        listed.push(format!("and {} more", stuck.len() - LISTED));
    }
    Some(
        Finding::new(
            "linux.stuck",
            Severity::Warn,
            format!(
                "{} process(es) stuck in uninterruptible sleep for over 30 s",
                stuck.len()
            ),
        )
        .detail(format!("state D: {}", listed.join(", ")))
        .fix("these wait on the kernel (disk, network storage, a hung runc) and cannot be killed: check `dmesg | tail` and the storage; a stuck runc points at Docker in LXC; a reboot clears them"),
    )
}

/// Every Linux check, on the running machine; nothing on other systems.
pub fn linux_findings(path_env: &str, has_docker: bool) -> Vec<Finding> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let root = Path::new("/");
    let cpus = std::thread::available_parallelism().map_or(1, |count| count.get());
    let detected = detect_container(path_env);
    let mut pause = || std::thread::sleep(Duration::from_secs(1));
    [
        lxc_docker(in_lxc(root, detected.as_deref()), has_docker),
        high_load(root, cpus),
        stuck_finding(&stuck_processes(root, &mut pause)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(root: &Path, relative: &str, text: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("dir");
        }
        std::fs::write(path, text).expect("write");
    }

    fn stat(pid: u32, comm: &str, state: char, start_ticks: u64) -> String {
        format!("{pid} ({comm}) {state} 1 {pid} {pid} 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 {start_ticks} 1000 200 18446744073709551615\n")
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        (temp, root)
    }

    #[test]
    fn lxc_is_found_three_ways_and_only_warns_with_docker() {
        let (_temp, root) = fixture();
        assert!(!in_lxc(&root, None));
        assert!(in_lxc(&root, Some("lxc")));
        write(&root, "proc/1/environ", b"PATH=/bin\0container=lxc\0");
        assert!(in_lxc(&root, None));
        let (_temp, root) = fixture();
        write(&root, "run/systemd/container", b"lxc\n");
        assert!(in_lxc(&root, Some("none")));
        assert!(lxc_docker(true, false).is_none());
        let finding = lxc_docker(true, true).expect("finding");
        assert_eq!(finding.id, "linux.lxc_docker");
        assert!(finding.detail.contains("runc exec"));
    }

    #[test]
    fn load_warns_only_past_twice_the_cpus() {
        let (_temp, root) = fixture();
        assert!(high_load(&root, 4).is_none());
        write(&root, "proc/loadavg", b"8.00 7.50 6.00 3/900 4242\n");
        assert!(high_load(&root, 4).is_none());
        write(&root, "proc/loadavg", b"8.01 7.50 6.00 3/900 4242\n");
        let finding = high_load(&root, 4).expect("finding");
        assert_eq!(finding.id, "linux.load");
        assert!(finding.detail.contains("8.01"), "{}", finding.detail);
    }

    #[test]
    fn only_old_processes_in_d_at_both_scans_count_as_stuck() {
        let (_temp, root) = fixture();
        write(&root, "proc/uptime", b"1000.00 3000.00\n");
        write(
            &root,
            "proc/10/stat",
            stat(10, "runc:[2:INIT]", 'D', 10_000).as_bytes(),
        );
        write(
            &root,
            "proc/11/stat",
            stat(11, "jbd2/sda1-8", 'D', 99_000).as_bytes(),
        );
        write(
            &root,
            "proc/12/stat",
            stat(12, "kworker", 'D', 20_000).as_bytes(),
        );
        write(
            &root,
            "proc/13/stat",
            stat(13, "bash", 'S', 1_000).as_bytes(),
        );
        write(
            &root,
            "proc/14/stat",
            stat(14, "rsync (copy)", 'D', 5_000).as_bytes(),
        );
        let mut pauses = 0;
        let mut pause = || {
            pauses += 1;
            write(
                &root,
                "proc/12/stat",
                stat(12, "kworker", 'S', 20_000).as_bytes(),
            );
            write(
                &root,
                "proc/14/stat",
                stat(14, "rsync (copy)", 'D', 5_000).as_bytes(),
            );
        };
        let stuck = stuck_processes(&root, &mut pause);
        assert_eq!(pauses, 1);
        assert_eq!(
            stuck,
            [
                (10, "runc:[2:INIT]".to_string()),
                (14, "rsync (copy)".to_string())
            ],
            "11 started 10 s ago, 12 woke up"
        );
        let finding = stuck_finding(&stuck).expect("finding");
        assert_eq!(finding.id, "linux.stuck");
        assert_eq!(
            finding.detail,
            "state D: 10 (runc:[2:INIT]), 14 (rsync (copy))"
        );
        assert!(stuck_finding(&[]).is_none());
    }

    #[test]
    fn no_process_in_d_needs_no_second_scan() {
        let (_temp, root) = fixture();
        write(
            &root,
            "proc/13/stat",
            stat(13, "bash", 'S', 1_000).as_bytes(),
        );
        let mut pauses = 0;
        assert!(stuck_processes(&root, &mut || pauses += 1).is_empty());
        assert_eq!(pauses, 0);
    }
}
