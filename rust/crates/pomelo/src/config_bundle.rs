//! The app side of config bundles: opening the export / import forms and carrying out what they submit.

use winit::window::WindowId;
use workspaces_ui::{ExportConfig, ExportConfigModal, ImportConfig, ImportConfigModal};

use crate::App;

impl App {
    /// Settings has no window of its own to show a form in, so it goes to the main window last focused.
    pub(crate) fn bundle_window(&self) -> Option<WindowId> {
        self.focused_main
            .filter(|id| self.mains.contains_key(id))
            .or_else(|| self.mains.keys().next().copied())
    }

    pub(crate) fn open_export_config(&mut self, id: WindowId) {
        let Some(session) = self.project_session(id) else {
            self.with_workspace_view(id, |view, _| {
                view.show_toast("Open a project to export its config", None)
            });
            return;
        };
        let secret_count = pom_secrets::SecretStore::new(pom_paths::StateDir::from_env(), &session)
            .names()
            .map_or(0, |names| names.len());
        let modal = ExportConfigModal::new(secret_count);
        self.focus_main_with_modal(id, Box::new(modal));
    }

    pub(crate) fn open_import_config(&mut self, id: WindowId) {
        if self.project_session(id).is_none() {
            self.with_workspace_view(id, |view, _| {
                view.show_toast("Open a project to import a config into it", None)
            });
            return;
        }
        let modal = ImportConfigModal::new(Box::new(choose_bundle_file));
        self.focus_main_with_modal(id, Box::new(modal));
    }

    pub(crate) fn focus_main_with_modal(
        &mut self,
        id: WindowId,
        modal: Box<dyn workspace::WindowModal>,
    ) {
        self.with_workspace_view(id, |view, _| view.open_window_modal(modal));
        if let Some(main) = self.mains.get_mut(&id) {
            main.window.focus_window();
            main.dirty = true;
        }
    }

    fn project_session(&self, id: WindowId) -> Option<String> {
        let project = self.mains.get(&id)?.project.as_ref()?;
        Some(project.session.clone())
    }

    pub(crate) fn export_config(&mut self, id: WindowId, request: &ExportConfig) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let exported = pom_bundle::export(
            &project.config_path,
            &pom_paths::StateDir::from_env(),
            &project.session,
            request.password.as_deref(),
        );
        let message = match exported {
            Err(error) => format!("Export failed: {error}"),
            Ok(export) => match choose_save_path(export.file_name) {
                None => return,
                Some(path) => match std::fs::write(&path, &export.data) {
                    Ok(()) => format!("Exported the config to {}", path.display()),
                    Err(error) => format!("Could not write {}: {error}", path.display()),
                },
            },
        };
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
    }

    pub(crate) fn import_config(&mut self, id: WindowId, request: &ImportConfig) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let state = pom_paths::StateDir::from_env();
        let secrets = if request.store_secrets {
            request.contents.secrets.clone()
        } else {
            Default::default()
        };
        if request.adapt {
            let staged = pom_bundle::apply(
                &project.config_path,
                &state,
                &project.session,
                None,
                &secrets,
            )
            .and_then(|_| pom_bundle::stage_for_adapt(&project.root, &request.contents.config));
            if let Err(error) = staged {
                self.with_workspace_view(id, |view, _| {
                    view.show_toast(format!("Import failed: {error}"), None)
                });
                return;
            }
            let prompt = pom_bundle::adapt_prompt();
            self.open_task_agent(id, |context| {
                pom_agent::claude_task_launch(context, "fixer", &prompt)
            });
            return;
        }
        let yaml = request
            .write_config
            .then_some(request.contents.config.as_str());
        let message = match pom_bundle::apply(
            &project.config_path,
            &state,
            &project.session,
            yaml,
            &secrets,
        ) {
            Err(error) => format!("Import failed: {error}"),
            Ok(applied) => {
                let mut parts = Vec::new();
                if yaml.is_some() {
                    parts.push(if applied.split {
                        "config replaced and split into pom.d".to_string()
                    } else {
                        "config replaced".to_string()
                    });
                }
                if applied.secrets_created > 0 {
                    parts.push(format!("{} secret(s) stored", applied.secrets_created));
                }
                format!("Imported: {}", parts.join(", "))
            }
        };
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
    }
}

#[cfg(target_os = "macos")]
fn choose_bundle_file() -> Option<std::path::PathBuf> {
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{MainThreadMarker, NSString};
    let mtm = MainThreadMarker::new()?;
    let panel = unsafe { NSOpenPanel::openPanel(mtm) };
    unsafe {
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setPrompt(Some(&NSString::from_str("Import")));
        panel.setMessage(Some(&NSString::from_str(
            "Choose a pom.yml or a .pombundle to import",
        )));
    }
    if unsafe { panel.runModal() } != NSModalResponseOK {
        return None;
    }
    let url = unsafe { panel.URL() }?;
    unsafe { url.path() }.map(|path| std::path::PathBuf::from(path.to_string()))
}

#[cfg(not(target_os = "macos"))]
fn choose_bundle_file() -> Option<std::path::PathBuf> {
    None
}

#[cfg(target_os = "macos")]
fn choose_save_path(file_name: &str) -> Option<std::path::PathBuf> {
    use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
    use objc2_foundation::{MainThreadMarker, NSString};
    let mtm = MainThreadMarker::new()?;
    let panel = unsafe { NSSavePanel::savePanel(mtm) };
    unsafe {
        panel.setNameFieldStringValue(&NSString::from_str(file_name));
        panel.setPrompt(Some(&NSString::from_str("Export")));
    }
    if unsafe { panel.runModal() } != NSModalResponseOK {
        return None;
    }
    let url = unsafe { panel.URL() }?;
    unsafe { url.path() }.map(|path| std::path::PathBuf::from(path.to_string()))
}

#[cfg(not(target_os = "macos"))]
fn choose_save_path(_file_name: &str) -> Option<std::path::PathBuf> {
    None
}
