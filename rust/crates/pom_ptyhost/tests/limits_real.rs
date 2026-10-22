//! Workspace caps against a real systemd user manager. Opt-in (`POM_REAL_CGROUP=1`): it needs a Linux login
//! session with cgroup v2, which CI containers and macOS lack.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pom_ptyhost::limits::{cgroup_of, workspace_cgroup, Backend};
use pom_ptyhost::{HolderGroup, Limits, SocketDir, SpawnRequest};

const BINARY: &str = env!("CARGO_BIN_EXE_pomelo-pty");
const MEMORY_CAP: u64 = 256 << 20;

fn spawn(dir: &SocketDir, name: &str, group: &HolderGroup, script: &str) {
    let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];
    pom_ptyhost::spawn_holder(
        dir,
        &SpawnRequest {
            binary: Path::new(BINARY),
            name,
            cwd: Path::new("/"),
            cols: 80,
            rows: 24,
            argv: &argv,
            env: &[],
            group: Some(group),
        },
    )
    .expect("spawn holder");
    pom_ptyhost::wait_for_holder(dir, name, Duration::from_secs(10)).expect("holder socket");
}

fn holder_cgroup(dir: &SocketDir, name: &str) -> PathBuf {
    let pid = dir.holder_pid(name).expect("holder pid");
    let own = cgroup_of(Path::new("/proc"), Path::new("/sys/fs/cgroup"), pid).expect("cgroup");
    workspace_cgroup(&own).unwrap_or_else(|| panic!("{name} is not in a workspace slice: {own:?}"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .trim()
        .to_string()
}

#[test]
fn a_workspace_over_its_memory_cap_is_killed_inside_its_cgroup_while_others_run_on() {
    if std::env::var_os("POM_REAL_CGROUP").is_none() {
        eprintln!("skipped: set POM_REAL_CGROUP=1 on Linux with a systemd user session");
        return;
    }
    assert_eq!(
        Backend::from_env(),
        Backend::Systemd,
        "POM_REAL_CGROUP is set but no systemd user manager answers"
    );
    let temp = tempfile::Builder::new()
        .prefix("pty")
        .tempdir_in("/tmp")
        .expect("temp");
    let dir = SocketDir::new(temp.path());
    let project = format!("pomtest{}", std::process::id());
    let heavy = HolderGroup::new(
        &project,
        "feat/heavy",
        Limits {
            cpu_percent: Some(50),
            memory_bytes: Some(MEMORY_CAP),
        },
    );
    let calm = HolderGroup::new(
        &project,
        "feat-calm",
        Limits {
            cpu_percent: None,
            memory_bytes: Some(1 << 30),
        },
    );
    spawn(&dir, "svc-calm", &calm, "sleep 120");
    spawn(&dir, "svc-steady", &heavy, "sleep 120");

    let slice = holder_cgroup(&dir, "svc-steady");
    assert_eq!(
        slice.file_name().and_then(|name| name.to_str()),
        Some(heavy.slice_unit().as_str())
    );
    assert!(
        slice
            .ancestors()
            .any(|ancestor| ancestor.file_name().is_some_and(|name| name == "pom.slice")),
        "{slice:?} is under pom.slice"
    );
    assert_eq!(read(&slice.join("memory.max")), MEMORY_CAP.to_string());
    assert_eq!(read(&slice.join("cpu.max")), "50000 100000");
    assert_ne!(holder_cgroup(&dir, "svc-calm"), slice);
    assert_eq!(
        dir.holder_group("svc-steady"),
        Some((project.clone(), "feat/heavy".to_string()))
    );

    spawn(
        &dir,
        "svc-hog",
        &heavy,
        "exec python3 -c 'chunks = []\nwhile True: chunks.append(bytearray(16 << 20))'",
    );
    assert_eq!(holder_cgroup(&dir, "svc-hog"), slice);
    let deadline = Instant::now() + Duration::from_secs(60);
    while dir.holder_alive("svc-hog") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!dir.holder_alive("svc-hog"), "the hog is still running");
    let events = read(&slice.join("memory.events"));
    let kills: u64 = events
        .lines()
        .find_map(|line| line.strip_prefix("oom_kill "))
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0);
    assert!(kills > 0, "memory.events: {events}");
    assert!(
        dir.holder_alive("svc-steady"),
        "the rest of the workspace kept running"
    );
    assert!(
        dir.holder_alive("svc-calm"),
        "another workspace kept running"
    );

    let names: Vec<String> = dir.holders().into_iter().map(|(name, _)| name).collect();
    dir.kill_holders_now(&names);
}
