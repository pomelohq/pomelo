//! The folder's language servers as the status bar shows them: a row per server for its menu, and the one
//! line of activity (progress, downloads, failures) beside it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use lsp::{LspStore, ServerStatus, ServerSummary};
use workspace::{
    Activity, ActivityClick, ActivityIcon, LanguageServerAction, LanguageServerRow,
    LanguageServers, ServerHealth,
};

/// How long a server's measured memory is reused before `ps` runs again.
const MEMORY_CACHE: Duration = Duration::from_secs(5);

/// What the status bar keeps between frames: statuses dismissed from the activity line, and memory
/// measured lately.
#[derive(Default)]
pub(crate) struct ServerStatusState {
    dismissed: HashSet<(lsp::ServerId, ServerStatus)>,
    memory: RefCell<HashMap<u32, (Instant, Option<u64>)>>,
}

impl ServerStatusState {
    pub(crate) fn summary(&self, store: &LspStore, root: &Path, details: bool) -> LanguageServers {
        let summaries = store.servers();
        let servers = summaries
            .iter()
            .map(|summary| LanguageServerRow {
                group: folder_name(&summary.root),
                name: summary.name.to_string(),
                health: health(summary.status),
                message: summary.message.clone(),
                version: summary.version.clone(),
                memory: summary
                    .process_id
                    .filter(|_| details)
                    .and_then(|pid| self.memory_of(pid))
                    .map(format_memory),
                binary: summary.binary.as_ref().map(binary_label),
                can_stop: matches!(
                    summary.status,
                    ServerStatus::Starting | ServerStatus::Running
                ),
                has_log: store.has_log(summary.key),
            })
            .collect::<Vec<_>>();
        let can_stop_all = servers.iter().any(|server| server.can_stop);
        let transitioning = summaries.iter().any(|summary| {
            matches!(
                summary.status,
                ServerStatus::CheckingForUpdate | ServerStatus::Downloading
            )
        });
        LanguageServers {
            folder: folder_name(root),
            servers,
            activity: self.activity(store, &summaries),
            can_restart_all: !transitioning,
            can_stop_all: can_stop_all && !transitioning,
        }
    }

    /// The line of activity, the first that applies: work in progress, downloads, update checks, failures.
    fn activity(&self, store: &LspStore, summaries: &[ServerSummary]) -> Option<Activity> {
        let work = store.work();
        if let Some(first) = work.first() {
            let mut message = first.title.clone();
            if let Some(percentage) = first.percentage {
                message.push_str(&format!(" ({percentage}%)"));
            }
            if let Some(detail) = &first.message {
                message.push_str(": ");
                message.push_str(detail);
            }
            if work.len() > 1 {
                message.push_str(&format!(" + {} more", work.len() - 1));
            }
            return Some(Activity {
                icon: ActivityIcon::Loading,
                message,
                click: ActivityClick::ListWork,
                cancellable: work
                    .iter()
                    .filter(|work| work.cancellable)
                    .map(|work| work.title.clone())
                    .collect(),
            });
        }
        let names = |status: ServerStatus| -> Vec<&'static str> {
            summaries
                .iter()
                .filter(|summary| {
                    summary.status == status && !self.dismissed.contains(&(summary.key, status))
                })
                .map(|summary| summary.name)
                .collect()
        };
        let line = |icon, message: String, click| {
            Some(Activity {
                icon,
                message,
                click,
                cancellable: Vec::new(),
            })
        };
        let downloading = names(ServerStatus::Downloading);
        if !downloading.is_empty() {
            return line(
                ActivityIcon::Download,
                format!("Downloading {}...", downloading.join(", ")),
                ActivityClick::Dismiss,
            );
        }
        let checking = names(ServerStatus::CheckingForUpdate);
        if !checking.is_empty() {
            return line(
                ActivityIcon::Download,
                format!("Checking for updates to {}...", checking.join(", ")),
                ActivityClick::Dismiss,
            );
        }
        let failed = names(ServerStatus::Failed);
        if !failed.is_empty() {
            return line(
                ActivityIcon::Warning,
                format!("Failed to run {}. Click to show error.", failed.join(", ")),
                ActivityClick::ShowError,
            );
        }
        None
    }

    /// Act on a menu pick or activity click; a message to show comes back as the tab's title and text.
    pub(crate) fn act(
        &mut self,
        store: &mut LspStore,
        action: LanguageServerAction,
    ) -> Option<(String, String)> {
        let summaries = store.servers();
        let key = |index: usize| summaries.get(index).map(|summary| summary.key);
        match action {
            LanguageServerAction::ViewMessage(index) => {
                let summary = summaries.get(index)?;
                return Some(message_tab(summary.name, summary.message.as_deref()?));
            }
            LanguageServerAction::Restart(index) => {
                let key = key(index)?;
                self.dismissed.retain(|(dismissed, _)| *dismissed != key);
                store.restart_server(key);
            }
            LanguageServerAction::Stop(index) => store.stop_server(key(index)?),
            LanguageServerAction::ViewLogs(_) => {}
            LanguageServerAction::RestartAll => {
                self.dismissed.clear();
                store.restart_all();
            }
            LanguageServerAction::StopAll => store.stop_all(),
            LanguageServerAction::CancelWork(index) => {
                let work = store.work();
                let work = work.iter().filter(|work| work.cancellable).nth(index)?;
                store.cancel_work(work.server, &work.token);
            }
            LanguageServerAction::ShowError => {
                let failed = summaries.iter().find(|summary| {
                    summary.status == ServerStatus::Failed
                        && !self
                            .dismissed
                            .contains(&(summary.key, ServerStatus::Failed))
                })?;
                self.dismissed.insert((failed.key, ServerStatus::Failed));
                return Some(message_tab(
                    failed.name,
                    failed.message.as_deref().unwrap_or_default(),
                ));
            }
            LanguageServerAction::DismissActivity => {
                for summary in &summaries {
                    if matches!(
                        summary.status,
                        ServerStatus::Downloading | ServerStatus::CheckingForUpdate
                    ) {
                        self.dismissed.insert((summary.key, summary.status));
                    }
                }
            }
        }
        None
    }

    fn memory_of(&self, pid: u32) -> Option<u64> {
        let now = Instant::now();
        let mut cache = self.memory.borrow_mut();
        if let Some((measured, bytes)) = cache.get(&pid) {
            if now.duration_since(*measured) < MEMORY_CACHE {
                return *bytes;
            }
        }
        let bytes = lsp::process_memory(pid);
        cache.insert(pid, (now, bytes));
        bytes
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn health(status: ServerStatus) -> ServerHealth {
    match status {
        ServerStatus::CheckingForUpdate | ServerStatus::Downloading | ServerStatus::Starting => {
            ServerHealth::Starting
        }
        ServerStatus::Running => ServerHealth::Running,
        ServerStatus::Stopped => ServerHealth::Stopped,
        ServerStatus::Failed => ServerHealth::Error,
    }
}

fn message_tab(name: &str, message: &str) -> (String, String) {
    (
        name.to_string(),
        format!("Language server {name}:\n\n{message}"),
    )
}

fn format_memory(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else {
        format!("{:.1} MB", bytes / (1024.0 * 1024.0))
    }
}

/// What runs a server, home shortened: the script for one run by node or python, else the binary.
fn binary_label(located: &lsp::Located) -> String {
    let program = located
        .binary
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let runtime = ["node", "python"]
        .into_iter()
        .find(|runtime| program.starts_with(runtime));
    let shown = match runtime.and_then(|runtime| {
        let script = located.args.iter().find(|arg| !arg.starts_with('-'))?;
        Some(format!("{script} ({runtime})"))
    }) {
        Some(script) => script,
        None => located.binary.to_string_lossy().into_owned(),
    };
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && shown.starts_with(&home) => {
            format!("~{}", &shown[home.len()..])
        }
        _ => shown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_reads_in_megabytes_until_a_gigabyte() {
        assert_eq!(format_memory(277 * 1024 * 1024 + 400 * 1024), "277.4 MB");
        assert_eq!(format_memory(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn a_node_server_is_shown_by_its_script() {
        let located = lsp::Located {
            name: "vtsls",
            binary: "/usr/local/bin/node".into(),
            args: vec!["/opt/vtsls/bin/vtsls.js".into(), "--stdio".into()],
        };
        assert_eq!(binary_label(&located), "/opt/vtsls/bin/vtsls.js (node)");
        let located = lsp::Located {
            name: "gopls",
            binary: "/opt/go/bin/gopls".into(),
            args: Vec::new(),
        };
        assert_eq!(binary_label(&located), "/opt/go/bin/gopls");
    }
}
