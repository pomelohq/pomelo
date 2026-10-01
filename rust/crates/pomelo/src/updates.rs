//! Wires the updater into the windows: pushes its state to each title bar and the Settings page, runs what
//! their update controls ask for, and says so after an update installed.

use std::sync::mpsc::{Receiver, TryRecvError};

use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::App;

type NotesResult = (WindowId, String, Result<String, String>);

#[derive(Default)]
pub(crate) struct UpdateTracker {
    shown: Option<auto_update::Snapshot>,
    waker_installed: bool,
    notes: Option<Receiver<NotesResult>>,
}

impl UpdateTracker {
    pub(crate) fn waker_installed(&self) -> bool {
        self.waker_installed
    }

    pub(crate) fn install_waker(&mut self) {
        self.waker_installed = true;
        auto_update::set_waker(ui::wake);
    }
}

impl App {
    pub(crate) fn poll_updates(&mut self) {
        if self.mains.is_empty() {
            return;
        }
        if !self.updates.waker_installed {
            self.updates.install_waker();
        }
        auto_update::set_automatic_checks(self.settings.auto_update);
        let snapshot = auto_update::snapshot();
        if self.updates.shown.as_ref() != Some(&snapshot) {
            let info = auto_update_ui::update_info(&snapshot);
            self.updates.shown = Some(snapshot);
            let windows: Vec<WindowId> = self.mains.keys().copied().collect();
            for id in windows {
                let changed = self
                    .with_workspace_view(id, |view, _| view.set_update(info.clone()))
                    .unwrap_or_default();
                if changed {
                    if let Some(main) = self.mains.get_mut(&id) {
                        main.dirty = true;
                    }
                }
            }
            self.settings_pages_at = None;
        }
        self.announce_fix();
        self.poll_release_notes();
    }

    /// After a crash at startup, a newer release that is ready says so in every window.
    fn announce_fix(&mut self) {
        if self.fix_announced || !self.launch.recovering() {
            return;
        }
        let auto_update::Status::Ready { version, .. } = auto_update::snapshot().status else {
            return;
        };
        self.fix_announced = true;
        let message = format!("Pomelo {version} is downloaded. Restart to use it.");
        let windows: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in windows {
            let (title, message) = ("A fixed version is ready".to_string(), message.clone());
            self.with_workspace_view(id, |view, _| view.notify_update_ready(title, message));
            if let Some(main) = self.mains.get_mut(&id) {
                main.dirty = true;
            }
        }
    }

    /// The Settings > General update row, for the page state.
    pub(crate) fn update_settings_row(&self) -> auto_update_ui::SettingsRow {
        auto_update_ui::settings_row(&auto_update::snapshot(), std::time::SystemTime::now())
    }

    /// On the first launch after an update, say so in `id`.
    pub(crate) fn announce_update(&mut self, id: WindowId) {
        let Some(version) = auto_update::take_just_updated() else {
            return;
        };
        let (title, message, button) = auto_update_ui::updated_notification(&version);
        self.with_workspace_view(id, |view, _| {
            view.notify_release_notes(title, message, button)
        });
    }

    /// Settings' update button: check, or restart into a ready update.
    pub(crate) fn settings_update_clicked(&mut self) {
        match auto_update_ui::settings_action(&auto_update::snapshot()) {
            workspace::UpdateAction::Restart => self.restart_into_update(),
            _ => auto_update::check(auto_update::CheckKind::Manual),
        }
    }

    pub(crate) fn run_update_action(
        &mut self,
        id: WindowId,
        action: workspace::UpdateAction,
        event_loop: &ActiveEventLoop,
    ) {
        match action {
            workspace::UpdateAction::Check => auto_update::check(auto_update::CheckKind::Manual),
            workspace::UpdateAction::Dismiss => auto_update::dismiss(),
            workspace::UpdateAction::Restart => self.restart_into_update(),
            workspace::UpdateAction::OpenDetails => {
                if self.settings_window.is_none() {
                    self.toggle_settings(event_loop);
                }
                self.settings_pages_at = None;
                self.refresh_settings_pages();
                self.with_settings_view(|view, _| view.select_category(settings_ui::GENERAL));
                self.settings_dirty = true;
            }
            workspace::UpdateAction::ReleaseNotes => self.fetch_release_notes(id),
        }
    }

    fn restart_into_update(&mut self) {
        let windows: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in windows {
            self.with_workspace_view(id, |view, _| view.persist_panes(true));
        }
        self.persist_settings();
        // The restart exits without the usual quit, which would otherwise leave the marker of a crash.
        if let Some(marker) = &self.run_marker {
            crate::recovery::end(marker);
        }
        let Err(error) = auto_update::restart();
        eprintln!("[update] restart: {error}");
    }

    fn fetch_release_notes(&mut self, id: WindowId) {
        let (sender, receiver) = std::sync::mpsc::channel();
        let version = auto_update::current_version().to_string();
        let spawned = std::thread::Builder::new()
            .name("release-notes".into())
            .spawn(move || {
                let notes = auto_update::release_notes(&version);
                if sender.send((id, version, notes)).is_ok() {
                    ui::wake();
                }
            });
        match spawned {
            Ok(_) => self.updates.notes = Some(receiver),
            Err(error) => {
                eprintln!("[update] release notes: {error}");
                open_url(&auto_update::release_url(auto_update::current_version()));
            }
        }
    }

    fn poll_release_notes(&mut self) {
        let Some(receiver) = self.updates.notes.as_ref() else {
            return;
        };
        let (id, version, notes) = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.updates.notes = None;
                return;
            }
        };
        self.updates.notes = None;
        let opened = match notes.and_then(|body| write_release_notes(&version, &body)) {
            Ok(path) => self
                .with_workspace_view(id, |view, _| view.open_markdown_preview(&path))
                .unwrap_or_default(),
            Err(error) => {
                eprintln!("[update] release notes: {error}");
                false
            }
        };
        if opened {
            if let Some(main) = self.mains.get_mut(&id) {
                main.dirty = true;
            }
        } else {
            open_url(&auto_update::release_url(&version));
        }
    }
}

fn write_release_notes(version: &str, body: &str) -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join("pomelo-release-notes");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(format!("Pomelo {version} release notes.md"));
    let markdown =
        auto_update_ui::release_notes_markdown(version, body, &auto_update::releases_url());
    std::fs::write(&path, markdown).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

fn open_url(url: &str) {
    if let Err(error) = std::process::Command::new("/usr/bin/open").arg(url).spawn() {
        eprintln!("[update] open {url}: {error}");
    }
}
