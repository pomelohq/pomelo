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
        if let Some((from, to)) = requests.reorder {
            self.reorder_workspace(id, from, to);
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

    fn reorder_workspace(&mut self, id: WindowId, from: usize, to: usize) {
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        let Some(project) = main.project.as_mut() else {
            return;
        };
        match project.move_workspace(from, to, &pom_paths::StateDir::from_env()) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => eprintln!("workspace order not saved: {error}"),
        }
        let runner = main
            .services
            .as_ref()
            .map(|services| services.runner.as_ref());
        let info = crate::project_info(
            project,
            runner,
            main.tickets.as_ref(),
            main.pull_requests.as_ref(),
        );
        self.with_workspace_view(id, |view, _| view.update_project(info));
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
        let mut modal = CreateWorkspaceModal::new(repos, existing, namer());
        let board = (self.settings.jira_board != 0).then_some(self.settings.jira_board);
        if let Some(source) = workspaces_ui::TicketSource::for_session(
            &pom_paths::StateDir::from_env(),
            &project.session,
            board,
            self.settings.jira_only_mine,
        ) {
            modal = modal.with_tickets(source);
        }
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
            workspace::RowAction::UpdateMain => {
                self.queue_main_op(id, OpKind::RefreshMain, workspaces_ui::REFRESH_TITLE)
            }
            workspace::RowAction::PrepareMain => self.queue_main_op(
                id,
                OpKind::PrepareMain(pom_workspace::PrepareRequest::default()),
                "Preparing main",
            ),
            workspace::RowAction::OpenTicket => self.open_ticket(id, &target.branch),
            workspace::RowAction::AddMissingRepos => self.add_missing_repos(id, &target),
            workspace::RowAction::AddRepos => self.pick_repos(id, &target),
        }
    }

    /// The Jira ticket the workspace's branch names, as a tab in the editor area.
    pub(crate) fn open_ticket(&mut self, id: WindowId, branch: &str) {
        let Some(session) = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .map(|project| project.session.clone())
        else {
            return;
        };
        let Some(key) = pom_jira::key_for_branch(branch) else {
            self.with_workspace_view(id, |view, _| {
                view.show_toast(format!("{branch} names no Jira ticket"), None)
            });
            return;
        };
        let item_id = jira_ui::item_id(&key);
        self.with_workspace_view(id, |view, _| {
            view.reveal_center_item(&item_id, || {
                Some(Box::new(jira_ui::TicketItem::new(
                    pom_paths::StateDir::from_env(),
                    session,
                    key,
                    Arc::new(ui::wake),
                )))
            })
        });
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    fn queue_main_op(&mut self, id: WindowId, kind: OpKind, title: &str) {
        let Some(context) = self.op_context(id) else {
            return;
        };
        if let Some(main) = self.mains.get(&id) {
            let already = main.ops.snapshot().iter().any(|op| {
                op.title == title
                    && matches!(
                        op.status,
                        workspace::OpStatus::Queued | workspace::OpStatus::Running
                    )
            });
            if !already {
                main.ops.enqueue(kind, title.to_string(), context);
            }
        }
    }

    pub(crate) fn op_context(&self, id: WindowId) -> Option<OpContext> {
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
                project_info(project, None, None, None)
                    .label(index)
                    .to_string(),
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

    /// Lights the status bar's Services badge while a repo service of the active workspace runs (checked at
    /// most once a second: it lists the holders on disk).
    fn poll_services_badge(&mut self, id: WindowId) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(1);
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        if main.services_checked.is_some_and(|at| at.elapsed() < EVERY) {
            return;
        }
        main.services_checked = Some(std::time::Instant::now());
        let running = match (main.services.as_ref(), main.project.as_ref()) {
            (Some(services), Some(project)) => services
                .runner
                .repo_service_running(project.active_branch()),
            _ => false,
        };
        if self.with_workspace_view(id, |view, _| view.set_services_running(running)) == Some(true)
        {
            if let Some(main) = self.mains.get_mut(&id) {
                main.dirty = true;
            }
        }
    }

    /// Picks up a closed form, the operations' progress and whatever finished.
    pub(crate) fn poll_workspaces(&mut self, id: WindowId) {
        self.poll_doctor(id);
        self.poll_services_badge(id);
        let result = self.with_workspace_view(id, |view, _| {
            view.tick_window_modal();
            view.take_modal_result()
        });
        if let Some(workspace::ModalResult::Submitted(value)) = result.flatten() {
            if let Some(create) = value.downcast_ref::<CreateWorkspace>() {
                self.queue_create(id, create);
            } else if let Some(rename) = value.downcast_ref::<RenameWorkspace>() {
                self.rename_workspace(id, rename);
            } else if let Some(project) = value.downcast_ref::<workspaces_ui::NewProject>() {
                self.start_scaffold(id, project);
            } else if let Some(request) = value.downcast_ref::<workspaces_ui::AddRepo>() {
                self.start_add_repo(id, request);
            } else if let Some(clone) = value.downcast_ref::<workspaces_ui::CloneRepos>() {
                self.start_clone_repos(id, clone);
            } else if let Some(rename) = value.downcast_ref::<workspaces_ui::RenameAlias>() {
                self.rename_alias(id, rename);
            } else if let Some(remove) = value.downcast_ref::<workspaces_ui::RemoveRepo>() {
                self.remove_repo(id, &remove.repo);
            } else if let Some(picked) = value.downcast_ref::<workspaces_ui::PickedRepos>() {
                self.queue_add_repos(id, &picked.branch, picked.repos.clone());
            } else if let Some(export) = value.downcast_ref::<workspaces_ui::ExportConfig>() {
                self.export_config(id, export);
            } else if let Some(import) = value.downcast_ref::<workspaces_ui::ImportConfig>() {
                self.import_config(id, import);
            }
        }
        let tickets_changed = match self.mains.get_mut(&id) {
            Some(main) => {
                let branches: Vec<String> = main
                    .project
                    .iter()
                    .flat_map(|project| project.workspaces.iter().map(|w| w.branch.clone()))
                    .collect();
                let tickets_changed = main.tickets.as_mut().is_some_and(|tickets| {
                    tickets.refresh_if_due(&branches);
                    tickets.take_changed()
                });
                let prs_changed = main.pull_requests.as_ref().is_some_and(|prs| {
                    let workspaces = main
                        .project
                        .iter()
                        .flat_map(|project| project.workspaces.iter())
                        .map(|workspace| pull_request_ui::WorkspaceRepos {
                            branch: workspace.branch.clone(),
                            repos: workspace
                                .repos
                                .iter()
                                .map(|repo| (repo.name.clone(), repo.path.clone()))
                                .collect(),
                        })
                        .collect();
                    prs.refresh_if_due(workspaces);
                    prs.take_changed()
                });
                tickets_changed || prs_changed
            }
            None => false,
        };
        if tickets_changed {
            self.refresh_project_info(id);
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
                    done.title,
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
        if let Some(board) = create
            .board
            .filter(|board| *board != self.settings.jira_board)
        {
            self.settings.jira_board = board;
            self.with_settings_view(|view, _| view.remember_jira_board(board));
            if let Err(error) = self.settings.save() {
                eprintln!("could not save settings: {error}");
            }
        }
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
            Some(project_info(
                project,
                runner,
                main.tickets.as_ref(),
                main.pull_requests.as_ref(),
            ))
        });
        if let Some(info) = info {
            self.with_workspace_view(id, |view, _| view.update_project(info));
        }
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }
}
