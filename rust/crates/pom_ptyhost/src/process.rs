use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};

/// Where holder sockets, pidfiles and crash logs live: `$POM_PTY_SOCK_DIR`, else
/// `$XDG_RUNTIME_DIR/pom-pty`, else `/tmp/pom-pty-<uid>`. Shared with the previous core so either can reach
/// the other's holders. Passed explicitly so tests use a private directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketDir {
    root: PathBuf,
}

const TERM_GRACE: Duration = Duration::from_secs(4);
const QUICK_GRACE: Duration = Duration::from_millis(300);

impl SocketDir {
    pub fn new(root: impl Into<PathBuf>) -> SocketDir {
        SocketDir { root: root.into() }
    }

    pub fn from_env() -> SocketDir {
        if let Some(dir) = std::env::var_os("POM_PTY_SOCK_DIR").filter(|dir| !dir.is_empty()) {
            return SocketDir::new(dir);
        }
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
            return SocketDir::new(PathBuf::from(runtime).join("pom-pty"));
        }
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        SocketDir::new(format!("/tmp/pom-pty-{uid}"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Hashed so long holder names stay under the 104-byte Unix socket path limit.
    fn stem(name: &str) -> String {
        let digest = Sha1::digest(name.as_bytes());
        digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn socket(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.sock", Self::stem(name)))
    }

    pub fn pidfile(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.pid", Self::stem(name)))
    }

    pub fn crash_log(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.crash", Self::stem(name)))
    }

    pub fn holder_pid(&self, name: &str) -> Option<i32> {
        let text = std::fs::read_to_string(self.pidfile(name)).ok()?;
        text.lines().next()?.trim().parse().ok()
    }

    pub fn holder_alive(&self, name: &str) -> bool {
        self.holder_pid(name).is_some_and(process_alive)
    }

    /// Live holders by name; pidfiles of dead ones are cleaned up on the way.
    pub fn holders(&self) -> Vec<(String, i32)> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "pid") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut lines = text.lines();
            let (Some(pid), Some(name)) = (lines.next(), lines.next()) else {
                continue;
            };
            let Ok(pid) = pid.trim().parse::<i32>() else {
                continue;
            };
            if process_alive(pid) {
                out.push((name.to_string(), pid));
            } else {
                remove_quietly(&path);
                remove_quietly(&self.socket(name));
            }
        }
        out.sort();
        out
    }

    /// Terminates a holder and everything under it: TERM, then KILL whatever is left after a grace period.
    pub fn kill_holder(&self, name: &str) -> std::io::Result<()> {
        let pid = self.holder_pid(name).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no holder for {name:?}"),
            )
        })?;
        kill_tree(&descendants(pid), TERM_GRACE);
        // A killed holder never removes its own pidfile, and a recycled pid would then read as alive.
        remove_quietly(&self.pidfile(name));
        remove_quietly(&self.crash_log(name));
        Ok(())
    }

    /// Tears down many disposable holders with one short shared grace, so quitting never stacks per-holder
    /// waits.
    pub fn kill_holders_now(&self, names: &[String]) {
        let mut pids = Vec::new();
        for name in names {
            if let Some(pid) = self.holder_pid(name) {
                pids.extend(descendants(pid));
                remove_quietly(&self.pidfile(name));
                remove_quietly(&self.crash_log(name));
            }
        }
        if !pids.is_empty() {
            kill_tree(&pids, QUICK_GRACE);
        }
    }

    /// Whether the holder's command ended abnormally, and its final output.
    pub fn crash_info(&self, name: &str) -> Option<CrashInfo> {
        let data = std::fs::read(self.crash_log(name)).ok()?;
        let newline = data.iter().position(|&byte| byte == b'\n')?;
        Some(CrashInfo {
            crashed: data.starts_with(b"CRASH"),
            header: String::from_utf8_lossy(&data[..newline]).into_owned(),
            output: data[newline + 1..].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashInfo {
    pub crashed: bool,
    pub header: String,
    pub output: Vec<u8>,
}

pub(crate) fn remove_quietly(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("ptyhost: remove {}: {error}", path.display());
        }
    }
}

/// Alive and not a zombie: `kill(pid, 0)` also succeeds for an exited, unreaped process.
pub fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks for existence and permission.
    let exists = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && !is_zombie(pid)
}

fn is_zombie(pid: i32) -> bool {
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdshortinfo).cast(),
            size,
        )
    };
    if written == size {
        return info.pbsi_status == libc::SZOMB;
    }
    // The kernel has no task info for a zombie: the lookup fails with ESRCH though `kill(pid, 0)` succeeds.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

fn children(pid: i32) -> Vec<i32> {
    let mut buffer = vec![0 as libc::pid_t; 256];
    loop {
        let bytes = (buffer.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer holds `bytes` bytes of pid_t slots.
        let count = unsafe { libc::proc_listchildpids(pid, buffer.as_mut_ptr().cast(), bytes) };
        if count <= 0 {
            return Vec::new();
        }
        let count = count as usize;
        if count < buffer.len() {
            buffer.truncate(count);
            return buffer.into_iter().filter(|&child| child > 0).collect();
        }
        buffer.resize(buffer.len() * 2, 0);
    }
}

/// `root` and every process under it, breadth first.
pub fn descendants(root: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([root]);
    while let Some(pid) = queue.pop_front() {
        if out.contains(&pid) {
            continue;
        }
        out.push(pid);
        queue.extend(children(pid));
    }
    out
}

fn signal_all(pids: &[i32], signal: libc::c_int) {
    for &pid in pids {
        // SAFETY: plain signal delivery; the group form reaches a process group the pid leads.
        unsafe {
            libc::kill(-pid, signal);
            libc::kill(pid, signal);
        }
    }
}

fn kill_tree(pids: &[i32], grace: Duration) {
    signal_all(pids, libc::SIGTERM);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !pids.iter().any(|&pid| process_alive(pid)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    signal_all(pids, libc::SIGKILL);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_match_the_previous_core() {
        let dir = SocketDir::new("/tmp/pom-pty-501");
        // sha1("svc-demo-main-api-web")[..16], same derivation as before.
        let digest = Sha1::digest(b"svc-demo-main-api-web");
        let stem: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            dir.socket("svc-demo-main-api-web"),
            PathBuf::from(format!("/tmp/pom-pty-501/{stem}.sock"))
        );
        assert_eq!(stem.len(), 16);
    }

    #[test]
    fn kills_a_process_tree_and_sees_it_gone() -> std::io::Result<()> {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & sleep 30 & wait"])
            .spawn()?;
        std::thread::sleep(Duration::from_millis(200));
        let pid = child.id() as i32;
        let tree = descendants(pid);
        assert!(tree.len() >= 3, "shell and two sleeps: {tree:?}");
        kill_tree(&tree, Duration::from_secs(2));
        child.wait()?;
        std::thread::sleep(Duration::from_millis(100));
        assert!(tree.iter().all(|&pid| !process_alive(pid)));
        Ok(())
    }

    #[test]
    fn zombie_is_not_alive() -> std::io::Result<()> {
        let child = std::process::Command::new("true").spawn()?;
        let pid = child.id() as i32;
        std::thread::sleep(Duration::from_millis(200));
        assert!(!process_alive(pid), "exited but unreaped");
        drop(child);
        Ok(())
    }

    #[test]
    fn stale_pidfiles_are_dropped_from_the_holder_list() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = SocketDir::new(temp.path());
        std::fs::write(dir.pidfile("ghost"), "999999\nghost\n")?;
        std::fs::write(dir.pidfile("me"), format!("{}\nme\n", std::process::id()))?;
        assert_eq!(
            dir.holders(),
            vec![("me".to_string(), std::process::id() as i32)]
        );
        assert!(!dir.pidfile("ghost").exists());
        Ok(())
    }

    #[test]
    fn crash_log_header_decides_crashed() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = SocketDir::new(temp.path());
        std::fs::write(dir.crash_log("a"), "CRASH\ta - exited: status 1\nboom\n")?;
        let info = dir.crash_info("a");
        assert!(info
            .as_ref()
            .is_some_and(|i| i.crashed && i.output == b"boom\n"));
        std::fs::write(dir.crash_log("b"), "STOP\tb - exited cleanly\n")?;
        assert!(dir.crash_info("b").is_some_and(|i| !i.crashed));
        Ok(())
    }
}
