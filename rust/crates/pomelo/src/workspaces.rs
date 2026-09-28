//! The app side of creating, renaming and deleting workspaces: opening the forms, queueing the pipelines, and
//! following up when they finish (rescan, switch to the new workspace, warnings as a toast).

use std::sync::Arc;

use winit::window::WindowId;
use workspaces_ui::{
    CreateWorkspace, CreateWorkspaceModal, OpContext, OpKind, RenameWorkspace, RenameWorkspaceModal,
};

use crate::{project_info, App};

/// Where the grouped WORKSPACES list keeps its group order and folded groups, as `order=` and `folded=` lines
/// of group keys. Kept out of the settings file, which the settings window writes back from its own copy.
const GROUPS_FILE: &str = "workspace_groups";

fn load_grouping() -> workspace::Grouping {
    let text = std::fs::read_to_string(pom_paths::StateDir::from_env().path(GROUPS_FILE))
        .unwrap_or_default();
    let keys = |name: &str| -> Vec<String> {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(|list| {
                list.split(',')
                    .filter(|key| !key.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    workspace::Grouping::from_keys(&keys("order"), &keys("folded"))
}

fn save_grouping(grouping: &workspace::Grouping) {
    let (order, folded) = grouping.keys();
    let text = format!("order={}\nfolded={}\n", order.join(","), folded.join(","));
    let path = pom_paths::StateDir::from_env().path(GROUPS_FILE);
    if let Err(error) = pom_paths::write_atomic(&path, text.as_bytes(), 0o644) {
        eprintln!("workspaces: save the group layout: {error}");
    }
}

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
    /// Groups every window's WORKSPACES list by ticket status, or lists it flat, as the setting says.
    pub(crate) fn apply_workspace_grouping(&mut self) {
        let grouping = self.settings.group_workspaces.then(load_grouping);
        let ids: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in ids {
            let grouping = grouping.clone();
            self.with_workspace_view(id, |view, _| {
                if view.workspace_grouping() != grouping.as_ref() {
                    view.set_workspace_grouping(grouping);
                }
            });
            if let Some(main) = self.mains.get_mut(&id) {
                main.dirty = true;
            }
        }
    }

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
        if requests.grouping_changed {
            let grouping = self
                .with_workspace_view(id, |view, _| view.workspace_grouping().cloned())
                .flatten();
            if let Some(grouping) = grouping {
                save_grouping(&grouping);
                // Other windows show the same arrangement.
                self.apply_workspace_grouping();
            }
        }
        if let Some((op, action)) = requests.op {
            let failed = self
                .mains
                .get(&id)
                .and_then(|main| main.ops.snapshot().into_iter().find(|view| view.id == op));
            if let Some(main) = self.mains.get(&id) {
                match action {
                    workspace::OpAction::Retry => main.ops.retry(op),
                    workspace::OpAction::Dismiss => main.ops.dismiss(op),
                    workspace::OpAction::Cancel => main.ops.cancel(op),
                    workspace::OpAction::Skip => main.ops.skip(op),
                    workspace::OpAction::FixWithAgent => {}
                }
            }
            if let (workspace::OpAction::FixWithAgent, Some(failed)) = (action, failed) {
                self.open_op_fixer(id, &failed);
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
        let main_checkouts: std::collections::HashMap<String, std::path::PathBuf> = repos
            .iter()
            .map(|repo| {
                let checkout = project
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.is_main)
                    .and_then(|main| main.repos.iter().find(|found| &found.name == repo))
                    .map(|found| found.path.clone())
                    .unwrap_or_else(|| {
                        pom_layout::repo_worktree(
                            &project.root,
                            repo,
                            config.global_default_branch(),
                            true,
                        )
                    });
                (repo.clone(), checkout)
            })
            .collect();
        let mut modal = CreateWorkspaceModal::new(repos, existing, namer())
            .with_branches(branch_source(Arc::new(main_checkouts)));
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
        self.poll_use_branch(id);
        let result = self.with_workspace_view(id, |view, _| {
            view.tick_window_modal();
            view.take_modal_result()
        });
        if let Some(workspace::ModalResult::Submitted(value)) = result.flatten() {
            if let Some(create) = value.downcast_ref::<CreateWorkspace>() {
                self.queue_create(id, create);
            } else if let Some(rename) = value.downcast_ref::<RenameWorkspace>() {
                self.rename_workspace(id, rename);
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
            } else if let Some(choice) = value.downcast_ref::<workspaces_ui::UseBranch>() {
                self.start_use_branch(id, choice);
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
                    // A folder recreated at a deleted workspace's path must not bring back that one's views.
                    if let Some(main) = self.mains.get_mut(&id) {
                        let fresh = main.project.as_ref().and_then(|project| {
                            project
                                .workspaces
                                .get(index)
                                .map(|workspace| workspace.path.clone())
                        });
                        if let Some(fresh) = fresh {
                            main.parked.remove(&fresh);
                        }
                    }
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

    pub(crate) fn queue_create(&mut self, id: WindowId, create: &CreateWorkspace) {
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
                        repo_branches: create.repo_branches.clone(),
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

    /// Opens the form that picks the branch `repo` uses in the active workspace.
    pub(crate) fn open_use_branch(&mut self, id: WindowId, repo: &str) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let Some(active) = project.active_workspace() else {
            return;
        };
        let worktree = active
            .repos
            .iter()
            .find(|found| found.name == repo)
            .map(|found| found.path.clone())
            .unwrap_or_else(|| active.path.join(repo));
        let current = match pom_layout::read_head(&worktree) {
            Some(pom_layout::Head::Branch(branch)) => Some(branch),
            _ => None,
        };
        let checkout = if pom_layout::is_git_repo(&worktree) {
            worktree
        } else {
            let main_checkout = project
                .workspaces
                .iter()
                .find(|workspace| workspace.is_main)
                .and_then(|main| main.repos.iter().find(|found| found.name == repo))
                .map(|found| found.path.clone());
            match main_checkout {
                Some(path) => path,
                None => {
                    let message = format!("{repo} has no checkout to list branches from");
                    self.with_workspace_view(id, |view, _| view.show_toast(message, None));
                    return;
                }
            }
        };
        let folder = active
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let source = branch_source(Arc::new(std::collections::HashMap::from([(
            repo.to_string(),
            checkout,
        )])));
        let modal = workspaces_ui::UseBranchModal::new(
            repo,
            project.active_branch(),
            current.as_deref(),
            &folder,
            source,
        );
        self.with_workspace_view(id, |view, _| view.open_window_modal(Box::new(modal)));
    }

    /// Checks out the branch picked for a repo off the UI thread; the fetch can take up to a minute.
    fn start_use_branch(&mut self, id: WindowId, choice: &workspaces_ui::UseBranch) {
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        if main.use_branch.is_some() {
            self.with_workspace_view(id, |view, _| {
                view.show_toast("Another branch checkout is still running", None)
            });
            return;
        }
        let Some((folder, workspace_branch)) = main.project.as_ref().and_then(|project| {
            let active = project.active_workspace()?;
            Some((active.path.clone(), project.active_branch().to_string()))
        }) else {
            return;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let (repo, branch) = (choice.repo.clone(), choice.branch.clone());
        let spawned = std::thread::Builder::new()
            .name("use-branch".into())
            .spawn(move || {
                let outcome = if branch == workspace_branch {
                    pom_workspace::switch_repo_branch(&folder, &workspace_branch, &repo, &branch)
                } else {
                    pom_workspace::use_another_branch(&folder, &workspace_branch, &repo, &branch)
                };
                let message = match outcome {
                    Ok(()) => format!("{repo} now uses {branch}"),
                    Err(error) => error.lines().next().unwrap_or_default().to_string(),
                };
                if sender.send(message).is_ok() {
                    ui::wake();
                }
            });
        let message = match spawned {
            Ok(_) => {
                main.use_branch = Some(receiver);
                format!("Checking out {} in {}...", choice.branch, choice.repo)
            }
            Err(error) => format!("Could not check out {}: {error}", choice.branch),
        };
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
    }

    fn poll_use_branch(&mut self, id: WindowId) {
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        let message = match main.use_branch.as_ref().map(|receiver| receiver.try_recv()) {
            Some(Ok(message)) => message,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                "The branch checkout stopped".to_string()
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
        };
        main.use_branch = None;
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
        self.rescan_project(id);
    }

    /// Rescans the window's workspaces after one was created or deleted.
    pub(crate) fn rescan_project(&mut self, id: WindowId) {
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

/// Branches come from each repo's main checkout, the one every worktree of it branches off.
fn branch_source(
    main_checkouts: Arc<std::collections::HashMap<String, std::path::PathBuf>>,
) -> workspaces_ui::BranchSource {
    let checkout_of = |checkouts: &std::collections::HashMap<String, std::path::PathBuf>,
                       repo: &str| {
        checkouts
            .get(repo)
            .cloned()
            .ok_or_else(|| format!("{repo} has no checkout in main"))
    };
    let listed = main_checkouts.clone();
    workspaces_ui::BranchSource {
        list: Arc::new(move |repo| {
            let checkout = checkout_of(&listed, repo)?;
            let base = match pom_layout::read_head(&checkout) {
                Some(pom_layout::Head::Branch(branch)) => branch,
                _ => String::new(),
            };
            Ok(workspaces_ui::RepoBranches {
                base,
                branches: pom_workspace::list_branches(&checkout)?,
            })
        }),
        fetch: Arc::new(move |repo| {
            pom_workspace::fetch_origin(&checkout_of(&main_checkouts, repo)?)
        }),
    }
}
