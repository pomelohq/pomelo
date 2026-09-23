//! What is running in the terminal right now (the PTY's foreground process: its name, arguments and working
//! directory), for the tab title.

use std::path::PathBuf;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    pub name: String,
    pub cwd: PathBuf,
    pub argv: Vec<String>,
}

pub(crate) struct PtyProcessInfo {
    system: System,
    refresh_kind: ProcessRefreshKind,
    pty_fd: i32,
    shell_pid: u32,
    last_foreground: Option<Pid>,
    pub(crate) current: Option<ProcessInfo>,
}

impl PtyProcessInfo {
    pub(crate) fn new(pty_fd: i32, shell_pid: u32) -> Self {
        Self {
            system: System::new(),
            refresh_kind: ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::Always)
                .with_cwd(UpdateKind::Always)
                .with_exe(UpdateKind::Always)
                .without_tasks(),
            pty_fd,
            shell_pid,
            last_foreground: None,
            current: None,
        }
    }

    /// The PTY's foreground process group leader, or the shell before one is set.
    fn foreground_pid(&self) -> Option<Pid> {
        let pid = unsafe { libc::tcgetpgrp(self.pty_fd) };
        if pid > 0 {
            return Some(Pid::from_u32(pid as u32));
        }
        (self.shell_pid > 0).then(|| Pid::from_u32(self.shell_pid))
    }

    /// Re-read the foreground process; returns whether its name or directory changed. The process table is
    /// rebuilt when the foreground process changes so entries for finished commands don't pile up.
    pub(crate) fn refresh(&mut self) -> bool {
        let Some(pid) = self.foreground_pid() else {
            return false;
        };
        if self.last_foreground.replace(pid) != Some(pid) {
            self.system = System::new();
        }
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            self.refresh_kind,
        );
        let loaded = self.system.process(pid).and_then(|process| {
            Some(ProcessInfo {
                name: process.name().to_str()?.to_owned(),
                cwd: process.cwd().map(PathBuf::from).unwrap_or_default(),
                argv: process
                    .cmd()
                    .iter()
                    .filter_map(|arg| arg.to_str().map(ToOwned::to_owned))
                    .collect(),
            })
        });
        let changed = match (&self.current, &loaded) {
            (Some(previous), Some(now)) => previous.name != now.name || previous.cwd != now.cwd,
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.current = loaded;
        }
        changed
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// `"<cwd name> - <process> <args>"`, each part cut to 25 characters when `truncate_parts`.
pub fn title_for(info: &ProcessInfo, truncate_parts: bool) -> String {
    const MAX_CHARS: usize = 25;
    let directory = info
        .cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let process = match info.argv.get(1..) {
        Some(args) if !args.is_empty() => format!("{} {}", info.name, args.join(" ")),
        _ => info.name.clone(),
    };
    if truncate_parts {
        format!(
            "{} - {}",
            truncate(&directory, MAX_CHARS),
            truncate(&process, MAX_CHARS)
        )
    } else {
        format!("{directory} - {process}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_join_directory_and_command() {
        let info = ProcessInfo {
            name: "cargo".into(),
            cwd: PathBuf::from("/home/me/myproject"),
            argv: vec!["cargo".into(), "test".into(), "-p".into(), "api".into()],
        };
        assert_eq!(title_for(&info, true), "myproject - cargo test -p api");
        let long = ProcessInfo {
            name: "a-very-long-process-name-indeed".into(),
            cwd: PathBuf::from("/x"),
            argv: Vec::new(),
        };
        assert_eq!(title_for(&long, true), "x - a-very-long-process-na...");
    }
}
