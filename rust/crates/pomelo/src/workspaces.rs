//! The app side of creating, renaming and deleting workspaces: opening the forms, queueing the pipelines, and
//! following up when they finish (rescan, switch to the new workspace, warnings as a toast).

use std::sync::Arc;

use winit::window::WindowId;
use workspaces_ui::{
    CreateWorkspace, CreateWorkspaceModal, OpContext, OpKind, RenameWorkspace, RenameWorkspaceModal,
};

use crate::{project_info, App};

/// Claude, found the way the agent launcher finds it, asked for a name.
fn namer() -> workspaces_ui::Namer {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    Arc::new(move |seed: &str, description: &str| {
        let tool_path = pom_services::tool_path();
        let claude = pom_agent::resolve_claude(&home, tool_path);
        pom_agent::suggest_name(&claude, tool_path, seed, description)
    })
}

impl App {
    /// Acts on what the WORKSPACES panel asked for.
    pub(crate) fn handle_workspace_requests(&mut self, id: WindowId) {
        let Some(requests) = self.with_workspace_view(id, |view, _| view.take_workspace_requests())
        else {
            return;
        };
        if requests.new_workspace {
            self.open_create_workspace(id);
        }
        if let Some((index, action)) = requests.row {
            self.workspace_row_action(id, index, action);
        }
        if let Some((op, action)) = requests.op {
            if let Some(main) = self.mains.get(&id) {
                match action {
                    workspace::OpAction::Retry => main.ops.retry(op),
                    workspace::OpAction::Dismiss => main.ops.dismiss(op),
                }
            }
        }
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    pub(crate) fn open_create_workspace(&mut self, id: WindowId) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let Some(config) = project.config.as_ref() else {
            return;
        };
        let repos: Vec<String> = config.repos.keys().cloned().collect();
        let existing: Vec<String> = project
            .workspaces
            .iter()
            .map(|workspace| workspace.branch.clone())
            .collect();
        let modal = CreateWorkspaceModal::new(repos, existing, namer());
        self.with_workspace_view(id, |view, _| view.open_window_modal(Box::new(modal)));
    }

    fn workspace_row_action(&mut self, id: WindowId, index: usize, action: workspace::RowAction) {
        let Some(main) = self.mains.get(&id) else {
            return;
        };
        let Some(project) = main.project.as_ref() else {
            return;
        };
        let Some(target) = project.workspaces.get(index).cloned() else {
            return;
        };
        match action {
            workspace::RowAction::Rename => {
                let current = pom_layout::WorkspaceState::load(&target.path).display_name;
                let modal = RenameWorkspaceModal::new(&target.branch, &current, namer());
                self.with_workspace_view(id, |view, _| view.open_window_modal(Box::new(modal)));
            }
            workspace::RowAction::StopServices => self.stop_workspace_services(id, &target),
            workspace::RowAction::Delete => self.delete_workspace(id, index, &target),
        }
    }

    fn op_context(&self, id: WindowId) -> Option<OpContext> {
        let services = self.mains.get(&id)?.services.as_ref()?;
        let config = services.config.read().ok()?.clone()?;
        Some(OpContext {
            config,
            runner: services.runner.clone(),
            state: pom_paths::StateDir::from_env(),
        })
    }

    fn stop_workspace_services(&mut self, id: WindowId, target: &pom_layout::Workspace) {
        let Some(context) = self.op_context(id) else {
            return;
        };
        let branch = target.branch.clone();
        let is_main = target.is_main;
        std::thread::spawn(move || {
            let services = context.config.repos.iter().flat_map(|(repo, dir)| {
                dir.services
                    .keys()
                    .map(move |service| (repo.clone(), service.clone()))
            });
            let workspace_services = context
                .config
                .workspace_services
                .keys()
                .map(|service| (String::new(), service.clone()));
            for (repo, service) in services.chain(workspace_services) {
                let target = pom_services::ServiceTarget {
                    branch: branch.clone(),
                    is_main,
                    repo,
                    service,
                };
                if let Err(error) = context.runner.stop(&target) {
                    eprintln!("stop {}/{}: {error}", target.repo, target.service);
                }
            }
            ui::wake();
        });
    }

    /// Deleting the workspace this window works in first moves the window to main.
    fn delete_workspace(&mut self, id: WindowId, index: usize, target: &pom_layout::Workspace) {
        let Some(context) = self.op_context(id) else {
            return;
        };
        let (active, main_index, title) = match self.mains.get(&id).and_then(|m| m.project.as_ref())
        {
            Some(project) => (
                project.active_branch() == target.branch,
                project.workspaces.iter().position(|w| w.is_main),
                project_info(project, None).label(index).to_string(),
            ),
            None => return,
        };
        if active {
            if let Some(main_index) = main_index {
                self.activate_workspace(id, main_index);
            }
        }
        if let Some(main) = self.mains.get_mut(&id) {
            main.parked.remove(&target.path);
            main.ops.enqueue(
                OpKind::Delete(pom_workspace::DeleteRequest {
                    branch: target.branch.clone(),
                    from_stage: 0,
                }),
                format!("Deleting {title}"),
                context,
            );
        }
    }

    /// Picks up a closed form, the operations' progress and whatever finished.
    pub(crate) fn poll_workspaces(&mut self, id: WindowId) {
        let result = self.with_workspace_view(id, |view, _| {
            view.tick_window_modal();
            view.take_modal_result()
        });
        if let Some(workspace::ModalResult::Submitted(value)) = result.flatten() {
            if let Some(create) = value.downcast_ref::<CreateWorkspace>() {
                self.queue_create(id, create);
            } else if let Some(rename) = value.downcast_ref::<RenameWorkspace>() {
                self.rename_workspace(id, rename);
            }
        }
        let Some(main) = self.mains.get(&id) else {
            return;
        };
        let ops = main.ops.snapshot();
        let finished = main.ops.take_finished();
        self.with_workspace_view(id, |view, _| view.set_workspace_ops(ops));
        if finished.is_empty() {
            return;
        }
        self.rescan_project(id);
        for done in finished.iter().filter(|done| done.ok) {
            if done.created {
                let index = self
                    .mains
                    .get(&id)
                    .and_then(|main| main.project.as_ref())
                    .and_then(|project| {
                        project
                            .workspaces
                            .iter()
                            .position(|workspace| workspace.branch == done.branch)
                    });
                if let Some(index) = index {
                    self.activate_workspace(id, index);
                }
            }
            if !done.warnings.is_empty() {
                let message = format!(
                    "{} finished with {}: {}",
                    done.branch,
                    if done.warnings.len() == 1 {
                        "a warning".to_string()
                    } else {
                        format!("{} warnings", done.warnings.len())
                    },
                    done.warnings[0].lines().next().unwrap_or_default()
                );
                self.with_workspace_view(id, |view, _| view.show_toast(message, None));
            }
        }
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    fn queue_create(&mut self, id: WindowId, create: &CreateWorkspace) {
        let Some(context) = self.op_context(id) else {
            return;
        };
        let title = if create.display_name.is_empty() {
            create.branch.clone()
        } else {
            create.display_name.clone()
        };
        if let Some(main) = self.mains.get(&id) {
            main.ops.enqueue(
                OpKind::Create {
                    request: pom_workspace::CreateRequest {
                        branch: create.branch.clone(),
                        repos: create.repos.clone(),
                        ..pom_workspace::CreateRequest::default()
                    },
                    display_name: create.display_name.clone(),
                },
                title,
                context,
            );
        }
    }

    fn rename_workspace(&mut self, id: WindowId, rename: &RenameWorkspace) {
        let folder = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .and_then(|project| {
                project
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.branch == rename.branch)
            })
            .map(|workspace| workspace.path.clone());
        let Some(folder) = folder else {
            return;
        };
        let mut state = pom_layout::WorkspaceState::load(&folder);
        state.display_name = rename.display_name.clone();
        if let Err(error) = state.save(&folder) {
            self.with_workspace_view(id, |view, _| {
                view.show_toast(format!("Could not rename: {error}"), None)
            });
            return;
        }
        self.refresh_project_info(id);
    }

    /// Rescans the window's workspaces after one was created or deleted.
    fn rescan_project(&mut self, id: WindowId) {
        let state = pom_paths::StateDir::from_env();
        let rerooted = match self.mains.get_mut(&id) {
            Some(main) => {
                let Some(project) = main.project.as_mut() else {
                    return;
                };
                let before = project.active_root();
                project.reload(&state);
                main.parked.retain(|root, _| root.is_dir());
                project.active_root() != before
            }
            None => return,
        };
        if rerooted {
            self.install_project_views(id);
        } else {
            self.refresh_project_info(id);
        }
    }

    /// Pushes the workspace list (names, running services) to the view.
    pub(crate) fn refresh_project_info(&mut self, id: WindowId) {
        let info = self.mains.get(&id).and_then(|main| {
            let project = main.project.as_ref()?;
            let runner = main
                .services
                .as_ref()
                .map(|services| services.runner.as_ref());
            Some(project_info(project, runner))
        });
        if let Some(info) = info {
            self.with_workspace_view(id, |view, _| view.update_project(info));
        }
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }
}
