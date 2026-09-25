//! The app side of adding a repo to an open project: the form, the clone and detection off the UI thread,
//! then checking it out in the chosen workspaces.

use std::sync::mpsc::{Receiver, TryRecvError};

use winit::window::WindowId;
use workspaces_ui::{AddRepo, AddRepoModal, CloneRepos, CloneReposModal, OpKind};

use crate::App;

/// Repos being cloned into main: each name with how its clone went.
pub(crate) struct CloningRepos {
    window: WindowId,
    receiver: Receiver<Vec<(String, Result<(), String>)>>,
}

pub(crate) struct AddingRepo {
    window: WindowId,
    request: AddRepo,
    receiver: Receiver<Result<pom_core::AddedRepo, String>>,
}

impl App {
    pub(crate) fn open_add_repo(&mut self, id: WindowId) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let workspaces = project
            .workspaces
            .iter()
            .filter(|workspace| !workspace.is_main)
            .map(|workspace| workspace.branch.clone())
            .collect();
        let existing = project
            .config
            .as_ref()
            .map(|config| config.repos.keys().cloned().collect())
            .unwrap_or_default();
        let modal = AddRepoModal::new(
            workspaces,
            existing,
            Box::new(|| crate::choose_folders(true)),
            crate::claude_installed(),
        );
        self.focus_main_with_modal(id, Box::new(modal));
    }

    /// A workspace lacks repos its config has: check them out there, or for main ask where to clone them from.
    pub(crate) fn add_missing_repos(&mut self, id: WindowId, target: &pom_layout::Workspace) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let missing = project.missing_repos(target);
        if missing.is_empty() {
            return;
        }
        if !target.is_main {
            self.queue_add_repos(id, &target.branch, missing);
            return;
        }
        let repos = missing
            .into_iter()
            .map(|name| {
                let guess = pom_core::guess_remote(project, &name);
                (name, guess)
            })
            .collect();
        let modal = CloneReposModal::new(repos, Box::new(|| crate::choose_folders(true)));
        self.focus_main_with_modal(id, Box::new(modal));
    }

    pub(crate) fn start_clone_repos(&mut self, id: WindowId, clone: &CloneRepos) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.clone()) else {
            return;
        };
        let sources = clone.sources.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let state = pom_paths::StateDir::from_env();
            let results = sources
                .iter()
                .map(|(name, source)| {
                    (
                        name.clone(),
                        pom_core::clone_into_main(&project, name, source, &state),
                    )
                })
                .collect();
            if sender.send(results).is_err() {
                eprintln!("clone repos: the app stopped waiting");
            }
            ui::wake();
        });
        self.with_workspace_view(id, |view, _| {
            view.show_toast("Cloning the missing repos into main...", None)
        });
        self.cloning_repos = Some(CloningRepos {
            window: id,
            receiver,
        });
    }

    pub(crate) fn poll_clone_repos(&mut self) {
        let Some(cloning) = self.cloning_repos.as_ref() else {
            return;
        };
        let results = match cloning.receiver.try_recv() {
            Ok(results) => results,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Vec::new(),
        };
        let Some(CloningRepos { window, .. }) = self.cloning_repos.take() else {
            return;
        };
        let failures: Vec<String> = results
            .iter()
            .filter_map(|(name, result)| {
                result
                    .as_ref()
                    .err()
                    .map(|error| format!("{name}: {error}"))
            })
            .collect();
        let message = if failures.is_empty() {
            "Cloned into main".to_string()
        } else {
            format!("Could not clone {}", failures.join("; "))
        };
        self.with_workspace_view(window, |view, _| view.show_toast(message, None));
        self.refresh_project_now(window);
    }

    /// The Project page's repository rows for the main window Settings works on.
    pub(crate) fn project_page(&self) -> settings_ui::ProjectPage {
        let Some(project) = self
            .bundle_window()
            .and_then(|id| self.mains.get(&id))
            .and_then(|main| main.project.as_ref())
        else {
            return settings_ui::ProjectPage::default();
        };
        let Some(config) = project.config.as_ref() else {
            return settings_ui::ProjectPage {
                session: project.session.clone(),
                repos: Vec::new(),
            };
        };
        let others: Vec<&pom_layout::Workspace> = project
            .workspaces
            .iter()
            .filter(|workspace| !workspace.is_main)
            .collect();
        let main = pom_layout::workspace_root(&project.root, project.branch(), true);
        let repos = config
            .repos
            .iter()
            .map(|(name, dir)| settings_ui::RepoSummary {
                name: name.clone(),
                alias: dir.alias.clone(),
                services: dir.services.len(),
                cloned: main.join(name).is_dir(),
                present: others
                    .iter()
                    .filter(|workspace| workspace.path.join(name).is_dir())
                    .count(),
                workspaces: others.len(),
            })
            .collect();
        settings_ui::ProjectPage {
            session: project.session.clone(),
            repos,
        }
    }

    pub(crate) fn open_rename_alias(&mut self, id: WindowId, repo: &str) {
        let current = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .and_then(|project| project.config.as_ref())
            .and_then(|config| config.repos.get(repo))
            .map(|dir| {
                if dir.alias.is_empty() {
                    repo.to_string()
                } else {
                    dir.alias.clone()
                }
            })
            .unwrap_or_else(|| repo.to_string());
        let modal = workspaces_ui::RenameAliasModal::new(repo, &current);
        self.focus_main_with_modal(id, Box::new(modal));
    }

    pub(crate) fn rename_alias(&mut self, id: WindowId, rename: &workspaces_ui::RenameAlias) {
        let Some(config_path) = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .map(|project| project.config_path.clone())
        else {
            return;
        };
        let message =
            match pom_config::maintain::rename_alias(&config_path, &rename.repo, &rename.alias) {
                Ok(files) => format!(
                    "{} is now {} ({} file{} changed)",
                    rename.repo,
                    rename.alias,
                    files.len(),
                    if files.len() == 1 { "" } else { "s" }
                ),
                Err(error) => format!("Alias not changed: {error}"),
            };
        self.refresh_project_now(id);
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
    }

    pub(crate) fn remove_repo(&mut self, id: WindowId, repo: &str) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.clone()) else {
            return;
        };
        let message = match pom_core::remove_repo(&project, repo) {
            Ok(removed) => {
                let mut parts = vec![format!("Removed {repo}")];
                if !removed.kept_worktrees.is_empty() {
                    parts.push(format!(
                        "kept its worktree with changes in {}",
                        removed.kept_worktrees.join(", ")
                    ));
                }
                if let Some(clone) = removed.main_clone {
                    parts.push(format!("main's clone stays at {}", clone.display()));
                }
                parts.join("; ")
            }
            Err(error) => format!("Could not remove {repo}: {error}"),
        };
        self.refresh_project_now(id);
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
    }

    /// Splits the config into `pom.d`, or normalizes it (which also splits).
    pub(crate) fn tidy_config(&mut self, id: WindowId, normalize: bool) {
        let Some(config_path) = self
            .mains
            .get(&id)
            .and_then(|main| main.project.as_ref())
            .map(|project| project.config_path.clone())
        else {
            return;
        };
        let message = if normalize {
            match pom_config::maintain::normalize(&config_path) {
                Ok(changes) if changes.is_empty() => "The config is already normal".to_string(),
                Ok(changes) => format!("Normalized: {}", changes.join("; ")),
                Err(error) => format!("Could not normalize: {error}"),
            }
        } else {
            match pom_config::maintain::split(&config_path, false) {
                Ok(result) => format!("Split into {} files under pom.d", result.fragments.len()),
                Err(error) => format!("Could not split: {error}"),
            }
        };
        self.refresh_project_now(id);
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
    }

    /// Clones into main the repos the config names but main lacks; other workspaces keep the repos they chose.
    pub(crate) fn clone_missing_repos(&mut self, id: WindowId) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let main_workspace = project
            .workspaces
            .iter()
            .find(|workspace| workspace.is_main && !project.missing_repos(workspace).is_empty())
            .cloned();
        match main_workspace {
            Some(main) => self.add_missing_repos(id, &main),
            None => {
                self.with_workspace_view(id, |view, _| {
                    view.show_toast("Main has every repo of the config", None)
                });
            }
        }
    }

    /// Lets the user pick more of the config's repos for a workspace, which started with the ones it needed.
    pub(crate) fn pick_repos(&mut self, id: WindowId, target: &pom_layout::Workspace) {
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.as_ref()) else {
            return;
        };
        let available = project.missing_repos(target);
        if available.is_empty() {
            let message = format!("{} already has every repo of the config", target.branch);
            self.with_workspace_view(id, |view, _| view.show_toast(message, None));
            return;
        }
        let modal = workspaces_ui::PickReposModal::new(&target.branch, available);
        self.focus_main_with_modal(id, Box::new(modal));
    }

    pub(crate) fn queue_add_repos(&mut self, id: WindowId, branch: &str, repos: Vec<String>) {
        let Some(context) = self.op_context(id) else {
            return;
        };
        let Some(main) = self.mains.get(&id) else {
            return;
        };
        let title = format!("Adding {} to {branch}", repos.join(", "));
        main.ops.enqueue(
            OpKind::AddRepos(pom_workspace::CreateRequest {
                branch: branch.to_string(),
                repos,
                environment: String::new(),
                skip_seed: false,
                from_stage: 0,
            }),
            title,
            context,
        );
    }

    /// Reloads the project and pushes it to services and the window at once, instead of waiting for the
    /// watcher: the next step needs the new config.
    fn refresh_project_now(&mut self, id: WindowId) {
        let state = pom_paths::StateDir::from_env();
        let Some(main) = self.mains.get_mut(&id) else {
            return;
        };
        let Some(project) = main.project.as_mut() else {
            return;
        };
        project.reload(&state);
        if let Some(services) = &main.services {
            services.update_config(project);
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

    pub(crate) fn start_add_repo(&mut self, id: WindowId, request: &AddRepo) {
        if self.adding_repo.is_some() {
            self.with_workspace_view(id, |view, _| {
                view.show_toast("Another repository is still being added", None)
            });
            return;
        }
        let Some(project) = self.mains.get(&id).and_then(|main| main.project.clone()) else {
            return;
        };
        let add = pom_core::AddRepoRequest {
            source: request.source.clone(),
            alias: request.alias.clone(),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = pom_core::add_repo(&project, &add, &pom_paths::StateDir::from_env());
            if sender.send(result).is_err() {
                eprintln!("add repo: the app stopped waiting");
            }
            ui::wake();
        });
        let message = format!(
            "Adding {}: cloning and detecting its services...",
            pom_core::repo_name(&request.source)
        );
        self.with_workspace_view(id, |view, _| view.show_toast(message, None));
        self.adding_repo = Some(AddingRepo {
            window: id,
            request: request.clone(),
            receiver,
        });
    }

    pub(crate) fn poll_add_repo(&mut self) {
        let Some(adding) = self.adding_repo.as_ref() else {
            return;
        };
        let result = match adding.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("stopped unexpectedly".into()),
        };
        let Some(AddingRepo {
            window, request, ..
        }) = self.adding_repo.take()
        else {
            return;
        };
        match result {
            Ok(added) => self.finish_add_repo(window, &request, &added),
            Err(error) => {
                let message = format!("Could not add the repository: {error}");
                self.with_workspace_view(window, |view, _| view.show_toast(message, None));
            }
        }
    }

    /// The config has the repo now: take it in at once (the watcher would too, but the checkouts below need
    /// it), check it out in the chosen workspaces, and show the entry that was written.
    fn finish_add_repo(&mut self, id: WindowId, request: &AddRepo, added: &pom_core::AddedRepo) {
        self.refresh_project_now(id);
        for branch in &request.workspaces {
            self.queue_add_repos(id, branch, vec![added.name.clone()]);
        }
        let file = added.file.clone();
        let message = format!("Added {} to the project", added.name);
        self.with_workspace_view(id, |view, _| {
            view.open_file(&file);
            view.show_toast(message, None);
        });
        if request.use_ai {
            let prompt = wire_prompt(&added.name, &added.file);
            self.open_task_agent(id, |context| {
                pom_agent::claude_task_launch(context, "fixer", &prompt)
            });
        }
    }
}

fn wire_prompt(repo: &str, file: &std::path::Path) -> String {
    format!(
        "The repo `{repo}` was just added to this project; its entry in {} was drafted from what was \
         detected (services, setup). Wire it up: its env (database URLs, other repos' addresses, secrets by \
         name), the shared services it needs, and any setup or migration step. Edit only that entry unless \
         something shared must change, then run config_doctor until it reports no errors.",
        file.display()
    )
}
