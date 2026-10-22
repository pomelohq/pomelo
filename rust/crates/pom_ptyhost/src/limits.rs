//! Holders of one workspace share a cgroup on Linux, so a CPU and memory cap holds for the workspace as a whole:
//! a systemd user slice per workspace (`pom.slice/pom-<project>.slice/pom-<project>-<workspace>.slice`) with one
//! transient scope per holder in it, or a directory in a delegated cgroup tree named by `POM_CGROUP_ROOT`.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// 100 is one core.
    pub cpu_percent: Option<u32>,
    pub memory_bytes: Option<u64>,
}

impl Limits {
    pub fn is_empty(&self) -> bool {
        self.cpu_percent.is_none() && self.memory_bytes.is_none()
    }
}

/// The workspace a holder belongs to, and the cap its workspace has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HolderGroup {
    pub project: String,
    pub workspace: String,
    pub limits: Limits,
}

impl HolderGroup {
    pub fn new(project: &str, workspace: &str, limits: Limits) -> HolderGroup {
        HolderGroup {
            project: project.to_string(),
            workspace: workspace.to_string(),
            limits,
        }
    }

    /// `project/workspace`, as the holder's pidfile records it; a project name never holds a slash.
    pub fn label(&self) -> String {
        format!("{}/{}", self.project, self.workspace)
    }

    pub fn parse_label(label: &str) -> Option<(String, String)> {
        let (project, workspace) = label.split_once('/')?;
        (!project.is_empty() && !workspace.is_empty())
            .then(|| (project.to_string(), workspace.to_string()))
    }

    /// Dashes nest slices, so the names are escaped and only the two separators make levels.
    pub fn slice_unit(&self) -> String {
        format!(
            "pom-{}-{}.slice",
            unit_escape(&self.project),
            unit_escape(&self.workspace)
        )
    }

    fn scope_unit(&self, holder: &str, nonce: u128) -> String {
        format!(
            "pom-{}-{}-{}-{nonce}.scope",
            unit_escape(&self.project),
            unit_escape(&self.workspace),
            crate::process::SocketDir::stem(holder)
        )
    }

    fn directory_name(&self) -> String {
        format!(
            "pom-{}-{}",
            unit_escape(&self.project),
            unit_escape(&self.workspace)
        )
    }
}

/// systemd's unit-name escaping: anything but `[A-Za-z0-9_.:]` (and a leading dot) becomes `\xNN`.
pub fn unit_escape(text: &str) -> String {
    let mut out = String::new();
    for (index, byte) in text.bytes().enumerate() {
        let plain = byte.is_ascii_alphanumeric()
            || byte == b'_'
            || byte == b':'
            || (byte == b'.' && index > 0);
        if plain {
            out.push(byte as char);
        } else {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
    out
}

/// Where a new holder goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Wherever its spawner runs, as before limits existed.
    Unlimited,
    /// `systemd-run --user --scope` puts it in a scope of the workspace's slice; the argv goes before the holder's.
    Systemd { slice: String, prefix: Vec<String> },
    /// The holder writes its own pid into this `cgroup.procs` before it execs.
    Directory { procs: PathBuf },
}

/// How this machine can cap a workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    None,
    /// The user's systemd manager answers on its private socket.
    Systemd,
    /// A cgroup v2 directory delegated to this user.
    Directory(PathBuf),
}

impl Backend {
    /// `POM_CGROUP=off` turns limits off; `POM_CGROUP_ROOT` names a delegated cgroup directory to use instead of
    /// systemd. Only Linux has cgroups.
    pub fn from_env() -> Backend {
        if !cfg!(target_os = "linux") {
            return Backend::None;
        }
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        if var("POM_CGROUP").is_some_and(|value| value == "off") {
            return Backend::None;
        }
        if let Some(root) = var("POM_CGROUP_ROOT") {
            return Backend::Directory(PathBuf::from(root));
        }
        if systemd_user_available() {
            return Backend::Systemd;
        }
        Backend::None
    }

    pub fn name(&self) -> &'static str {
        match self {
            Backend::None => "none",
            Backend::Systemd => "systemd",
            Backend::Directory(_) => "cgroup",
        }
    }

    /// Sets the workspace's cap and says where its next holder goes. Without limits nothing changes.
    pub fn place(&self, group: &HolderGroup, holder: &str) -> std::io::Result<Placement> {
        if group.limits.is_empty() {
            return Ok(Placement::Unlimited);
        }
        match self {
            Backend::None => Ok(Placement::Unlimited),
            Backend::Systemd => {
                let slice = group.slice_unit();
                let status = Command::new("systemctl")
                    .args(set_property_args(&slice, &group.limits))
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .status()?;
                if !status.success() {
                    return Err(std::io::Error::other(format!(
                        "systemctl --user set-property {slice} failed ({status})"
                    )));
                }
                let nonce = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_millis());
                Ok(Placement::Systemd {
                    prefix: scope_prefix(&slice, &group.scope_unit(holder, nonce)),
                    slice,
                })
            }
            Backend::Directory(root) => {
                let directory = prepare_directory(root, group)?;
                Ok(Placement::Directory {
                    procs: directory.join("cgroup.procs"),
                })
            }
        }
    }
}

fn systemd_user_available() -> bool {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .is_some_and(|dir| Path::new(&dir).join("systemd/private").exists())
}

/// Swap is capped with memory: otherwise a workspace past its cap swaps the whole machine to a crawl instead of
/// being killed.
pub fn set_property_args(slice: &str, limits: &Limits) -> Vec<String> {
    let mut args: Vec<String> = ["--user", "set-property", "--runtime", slice]
        .map(str::to_string)
        .to_vec();
    args.push(match limits.cpu_percent {
        Some(percent) => format!("CPUQuota={percent}%"),
        None => "CPUQuota=".to_string(),
    });
    match limits.memory_bytes {
        Some(bytes) => {
            args.push(format!("MemoryMax={bytes}"));
            args.push("MemorySwapMax=0".to_string());
        }
        None => {
            args.push("MemoryMax=infinity".to_string());
            args.push("MemorySwapMax=infinity".to_string());
        }
    }
    args
}

pub fn scope_prefix(slice: &str, scope: &str) -> Vec<String> {
    vec![
        "systemd-run".to_string(),
        "--user".to_string(),
        "--scope".to_string(),
        "--quiet".to_string(),
        "--collect".to_string(),
        format!("--slice={slice}"),
        format!("--unit={scope}"),
        "--".to_string(),
    ]
}

/// Creates the workspace's directory under `root` with its cap written, enabling the controllers it needs.
pub fn prepare_directory(root: &Path, group: &HolderGroup) -> std::io::Result<PathBuf> {
    let directory = root.join(group.directory_name());
    std::fs::create_dir_all(&directory)?;
    // Already enabled is fine, and a root that refuses still lets the caps below report the real problem.
    if let Err(error) = std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory") {
        eprintln!(
            "ptyhost: enable cpu and memory in {}: {error}",
            root.display()
        );
    }
    let cpu = match group.limits.cpu_percent {
        Some(percent) => format!("{} {CPU_PERIOD}", u64::from(percent) * CPU_PERIOD / 100),
        None => format!("max {CPU_PERIOD}"),
    };
    std::fs::write(directory.join("cpu.max"), cpu)?;
    let memory = group
        .limits
        .memory_bytes
        .map_or_else(|| "max".to_string(), |bytes| bytes.to_string());
    std::fs::write(directory.join("memory.max"), &memory)?;
    let swap = if group.limits.memory_bytes.is_some() {
        "0"
    } else {
        "max"
    };
    // Not every kernel accounts swap; the memory cap still holds without it.
    if let Err(error) = std::fs::write(directory.join("memory.swap.max"), swap) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("ptyhost: {}: memory.swap.max: {error}", directory.display());
        }
    }
    Ok(directory)
}

const CPU_PERIOD: u64 = 100_000;

/// What a cgroup has used, from its `memory.current` and `cpu.stat`, and the caps it has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CgroupUsage {
    pub memory_bytes: u64,
    pub cpu_usec: u64,
    pub memory_max: Option<u64>,
    pub cpu_max_percent: Option<u32>,
}

pub fn read_usage(directory: &Path) -> Option<CgroupUsage> {
    let read = |file: &str| std::fs::read_to_string(directory.join(file)).ok();
    let memory_bytes = read("memory.current")?.trim().parse().ok()?;
    let cpu_usec = read("cpu.stat")?
        .lines()
        .find_map(|line| line.strip_prefix("usage_usec "))?
        .trim()
        .parse()
        .ok()?;
    let memory_max = read("memory.max").and_then(|text| text.trim().parse().ok());
    let cpu_max_percent = read("cpu.max").and_then(|text| {
        let mut fields = text.split_whitespace();
        let quota: u64 = fields.next()?.parse().ok()?;
        let period: u64 = fields.next()?.parse().ok()?;
        (period > 0).then(|| (quota * 100 / period) as u32)
    });
    Some(CgroupUsage {
        memory_bytes,
        cpu_usec,
        memory_max,
        cpu_max_percent,
    })
}

/// The cgroup v2 directory of `pid` under `mount` (`/sys/fs/cgroup`), from `/proc/<pid>/cgroup`.
pub fn cgroup_of(proc_root: &Path, mount: &Path, pid: i32) -> Option<PathBuf> {
    let text = std::fs::read_to_string(proc_root.join(format!("{pid}/cgroup"))).ok()?;
    let path = text.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(mount.join(path.trim_start_matches('/')))
}

/// The workspace cgroup a holder sits in: the parent of its scope when that is a `pom-` slice, or a `pom-`
/// directory itself.
pub fn workspace_cgroup(holder_cgroup: &Path) -> Option<PathBuf> {
    let name = holder_cgroup.file_name()?.to_str()?;
    if name.starts_with("pom-") && name.ends_with(".scope") {
        let parent = holder_cgroup.parent()?;
        let parent_name = parent.file_name()?.to_str()?;
        return (parent_name.starts_with("pom-") && parent_name.ends_with(".slice"))
            .then(|| parent.to_path_buf());
    }
    (name.starts_with("pom-") && !name.ends_with(".slice")).then(|| holder_cgroup.to_path_buf())
}

/// The controllers the cgroup tree a backend uses offers, when it can be read.
pub fn available_controllers(backend: &Backend) -> Option<String> {
    let path = match backend {
        Backend::None => return None,
        Backend::Directory(root) => root.join("cgroup.controllers"),
        Backend::Systemd => {
            // SAFETY: getuid has no preconditions.
            let uid = unsafe { libc::getuid() };
            PathBuf::from(format!(
                "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/cgroup.controllers"
            ))
        }
    };
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
}

/// Whether the user's systemd manager keeps running without a login (`loginctl enable-linger`).
pub fn lingering() -> bool {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .is_ok_and(|user| Path::new("/var/lib/systemd/linger").join(user).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(memory: Option<u64>) -> HolderGroup {
        HolderGroup::new(
            "my-project",
            "feat/login",
            Limits {
                cpu_percent: Some(150),
                memory_bytes: memory,
            },
        )
    }

    #[test]
    fn names_escape_dashes_so_only_pom_levels_nest() {
        assert_eq!(unit_escape("feat/login-x.y"), "feat\\x2flogin\\x2dx.y");
        assert_eq!(unit_escape(".hidden"), "\\x2ehidden");
        assert_eq!(
            group(None).slice_unit(),
            "pom-my\\x2dproject-feat\\x2flogin.slice"
        );
        assert_eq!(
            HolderGroup::parse_label(&group(None).label()),
            Some(("my-project".into(), "feat/login".into()))
        );
    }

    #[test]
    fn systemd_gets_the_slice_caps_and_a_scope_per_holder() {
        let args = set_property_args("pom-a-b.slice", &group(Some(256 << 20)).limits);
        assert_eq!(
            args,
            [
                "--user",
                "set-property",
                "--runtime",
                "pom-a-b.slice",
                "CPUQuota=150%",
                "MemoryMax=268435456",
                "MemorySwapMax=0"
            ]
        );
        let cleared = set_property_args("pom-a-b.slice", &Limits::default());
        assert!(cleared.contains(&"MemoryMax=infinity".to_string()));
        let prefix = scope_prefix("pom-a-b.slice", "pom-a-b-x.scope");
        assert_eq!(prefix[..3], ["systemd-run", "--user", "--scope"]);
        assert!(prefix.contains(&"--slice=pom-a-b.slice".to_string()));
        assert_eq!(prefix.last().map(String::as_str), Some("--"));
    }

    #[test]
    fn a_delegated_directory_gets_the_caps_written() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let placement =
            Backend::Directory(root.path().to_path_buf()).place(&group(Some(1 << 30)), "svc-x")?;
        let directory = root.path().join("pom-my\\x2dproject-feat\\x2flogin");
        assert_eq!(
            placement,
            Placement::Directory {
                procs: directory.join("cgroup.procs")
            }
        );
        let read = |file: &str| std::fs::read_to_string(directory.join(file));
        assert_eq!(read("cpu.max")?, "150000 100000");
        assert_eq!(read("memory.max")?, "1073741824");
        assert_eq!(read("memory.swap.max")?, "0");
        assert_eq!(
            std::fs::read_to_string(root.path().join("cgroup.subtree_control"))?,
            "+cpu +memory"
        );
        let unlimited = HolderGroup::new("p", "w", Limits::default());
        assert_eq!(
            Backend::Directory(root.path().to_path_buf()).place(&unlimited, "svc-x")?,
            Placement::Unlimited
        );
        Ok(())
    }

    #[test]
    fn usage_and_the_workspace_cgroup_are_read_from_files() -> std::io::Result<()> {
        let root = tempfile::tempdir()?;
        let slice = root
            .path()
            .join("user.slice/pom.slice/pom-p.slice/pom-p-w.slice");
        let scope = slice.join("pom-p-w-abc-1.scope");
        std::fs::create_dir_all(&scope)?;
        std::fs::write(slice.join("memory.current"), "1048576\n")?;
        std::fs::write(slice.join("cpu.stat"), "usage_usec 2500000\nuser_usec 1\n")?;
        std::fs::write(slice.join("memory.max"), "max\n")?;
        std::fs::write(slice.join("cpu.max"), "200000 100000\n")?;
        let usage = read_usage(&slice).expect("usage");
        assert_eq!(
            usage,
            CgroupUsage {
                memory_bytes: 1 << 20,
                cpu_usec: 2_500_000,
                memory_max: None,
                cpu_max_percent: Some(200),
            }
        );
        assert_eq!(workspace_cgroup(&scope), Some(slice.clone()));
        assert_eq!(workspace_cgroup(&root.path().join("session-3.scope")), None);
        assert_eq!(
            workspace_cgroup(&root.path().join("pom-p-w")),
            Some(root.path().join("pom-p-w"))
        );
        let proc_root = root.path().join("proc");
        std::fs::create_dir_all(proc_root.join("42"))?;
        std::fs::write(
            proc_root.join("42/cgroup"),
            "0::/user.slice/pom.slice/pom-p.slice/pom-p-w.slice/pom-p-w-abc-1.scope\n",
        )?;
        assert_eq!(cgroup_of(&proc_root, root.path(), 42), Some(scope));
        Ok(())
    }
}
