//! Runs a new project's setup behind the onboarding page: clone and scan off the main thread, the agent CLI
//! in the agent dock, verify, and each repair, feeding progress into the page's shared state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;

use onboarding::{Check, CheckKind, CheckStatus};
use onboarding_ui::{Onboarding, OnboardingPage, Phase, Request, Screen, Shared};
use pom_agent::{AgentCli, AgentState};
use winit::window::WindowId;

use crate::App;

enum Event {
    Scaffold(pom_core::ScaffoldEvent),
    Scaffolded(Result<PathBuf, String>),
    Check(Check),
    Verified,
}

pub(crate) struct OnboardingFlow {
    window: WindowId,
    shared: Shared,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    /// The folder this window had open before, to go back to when the setup is cancelled.
    previous: Option<PathBuf>,
    session: Option<String>,
    agent_item: Option<String>,
    agent_holder: Option<String>,
    /// The agent was seen working since it was last asked for something.
    agent_busy: bool,
    verifying: bool,
    installed: HashMap<String, String>,
    shown_second: u64,
    ticking: Arc<AtomicBool>,
}

fn agents() -> Vec<(AgentCli, bool)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    AgentCli::ALL
        .into_iter()
        .map(|cli| (cli, cli.installed(&home, pom_services::tool_path())))
        .collect()
}

impl App {
    /// The onboarding page in window `id`: the one already under way, or a fresh form.
    pub(crate) fn open_onboarding(&mut self, id: WindowId) {
        if let Some(flow) = self.onboarding.as_ref() {
            if flow.window != id {
                self.with_workspace_view(id, |view, _| {
                    view.show_toast("A project is being created in another window", None)
                });
                return;
            }
            let page = OnboardingPage::new(flow.shared.clone());
            self.with_workspace_view(id, |view, _| view.open_page(Box::new(page)));
            self.mark_dirty(id);
            return;
        }
        let mut state = Onboarding::new(pom_paths::sessions_root(), agents());
        state.form.with_agent &= self.settings.onboard_with_ai;
        let shared: Shared = Rc::new(std::cell::RefCell::new(state));
        let (sender, receiver) = std::sync::mpsc::channel();
        self.onboarding = Some(OnboardingFlow {
            window: id,
            shared: shared.clone(),
            sender,
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            previous: None,
            session: None,
            agent_item: None,
            agent_holder: None,
            agent_busy: false,
            verifying: false,
            installed: HashMap::new(),
            shown_second: 0,
            ticking: Arc::new(AtomicBool::new(false)),
        });
        let page = OnboardingPage::new(shared);
        self.with_workspace_view(id, |view, _| view.open_page(Box::new(page)));
        self.mark_dirty(id);
    }

    fn mark_dirty(&mut self, id: WindowId) {
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    pub(crate) fn poll_onboarding(&mut self) {
        let Some(flow) = self.onboarding.as_ref() else {
            return;
        };
        let window = flow.window;
        let requests = flow.shared.borrow_mut().take_requests();
        let mut changed = !requests.is_empty();
        for request in requests {
            self.onboarding_request(window, request);
        }
        loop {
            let Some(flow) = self.onboarding.as_ref() else {
                return;
            };
            match flow.receiver.try_recv() {
                Ok(event) => {
                    changed = true;
                    self.onboarding_event(window, event);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        changed |= self.follow_agent(window);
        changed |= self.refresh_summary(window);
        let Some(flow) = self.onboarding.as_mut() else {
            return;
        };
        let second = {
            let state = flow.shared.borrow();
            match (&state.run, state.screen) {
                (Some(run), Screen::Progress) => run.started.elapsed().as_secs() + 1,
                _ => 0,
            }
        };
        if second != flow.shown_second {
            flow.shown_second = second;
            changed = true;
        }
        if second > 0 && !flow.ticking.swap(true, Ordering::Relaxed) {
            let ticking = flow.ticking.clone();
            // The elapsed time on the page counts up, so the window wakes each second while a setup runs.
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                ticking.store(false, Ordering::Relaxed);
                ui::wake();
            });
        }
        if changed {
            flow.shared.borrow_mut().changed();
            self.mark_dirty(window);
        }
    }

    fn with_run(&mut self, f: impl FnOnce(&mut onboarding_ui::Run)) {
        if let Some(flow) = self.onboarding.as_ref() {
            if let Some(run) = flow.shared.borrow_mut().run.as_mut() {
                f(run);
            }
        }
    }

    fn run_value<R>(&self, f: impl FnOnce(&onboarding_ui::Run) -> R) -> Option<R> {
        let flow = self.onboarding.as_ref()?;
        let state = flow.shared.borrow();
        state.run.as_ref().map(f)
    }

    fn onboarding_request(&mut self, window: WindowId, request: Request) {
        match request {
            Request::ChooseFolders => {
                let folders = crate::choose_folders(true);
                if let Some(flow) = self.onboarding.as_ref() {
                    let added = flow.shared.borrow_mut().add_folders(&folders);
                    if !folders.is_empty() && added == 0 {
                        self.with_workspace_view(window, |view, _| {
                            view.show_toast("No new git repos in what was chosen", None)
                        });
                    }
                }
            }
            Request::Create => self.onboarding_create(window),
            Request::Close => {
                let busy = self.run_value(|run| run.finished.is_none() && run.error.is_none());
                if busy == Some(true) {
                    return;
                }
                self.with_workspace_view(window, |view, _| view.close_page(onboarding_ui::TAB_ID));
                self.onboarding = None;
            }
            Request::TabClosed => {
                let busy = self.run_value(|run| run.finished.is_none() && run.error.is_none());
                if busy != Some(true) {
                    self.onboarding = None;
                }
            }
            Request::Cancel => self.onboarding_cancel(window),
            Request::TogglePause => {
                let resumed = self.run_value(|run| run.paused).unwrap_or_default();
                self.with_run(|run| run.paused = !run.paused);
                if resumed {
                    self.resume_after_pause(window);
                }
            }
            Request::ToggleAgentCli => self.toggle_agent_cli(window),
            Request::OpenConfig => self.open_project_config(window),
            Request::SkipAgent => {
                self.stop_onboard_agent(window);
                self.with_run(|run| {
                    run.agent_skipped = true;
                    run.agent_shown = false;
                });
                let title = self.run_value(|run| run.agent.title()).unwrap_or_default();
                self.with_workspace_view(window, |view, _| {
                    view.show_toast(
                        format!("{title} stopped. Finishing from the drafted pom.yml"),
                        None,
                    )
                });
                if self.run_value(|run| run.phase) == Some(Phase::Configure) {
                    self.start_verify(window);
                }
            }
            Request::ResumeAgent => {
                self.with_run(|run| {
                    run.agent_skipped = false;
                    run.finding = None;
                    run.checks.clear();
                    run.enter(Phase::Configure);
                });
                self.start_onboard_agent(window);
            }
            Request::ConfigureDone => {
                if self.run_value(|run| run.phase) == Some(Phase::Configure) {
                    self.start_verify(window);
                }
            }
            Request::FixWithAgent => self.fix_with_agent(window),
            Request::CancelFix => {
                self.with_run(|run| {
                    if let Some(finding) = run.finding.as_mut() {
                        finding.fixing = false;
                    }
                });
                if let Some(flow) = self.onboarding.as_mut() {
                    flow.agent_busy = false;
                }
            }
            Request::OpenTerminal { repo } => {
                let dir = self.mains.get(&window).and_then(|main| {
                    let project = main.project.as_ref()?;
                    let config = project.config.as_ref()?;
                    Some(onboarding::main_checkout(&project.root, config, &repo))
                });
                if let Some(dir) = dir {
                    self.with_workspace_view(window, |view, _| view.open_terminal_in(dir));
                }
            }
            Request::SkipService => {
                self.with_run(|run| run.restart_verify_skipping());
                self.start_verify(window);
            }
            Request::RetryVerify => {
                self.with_run(|run| run.restart_verify());
                self.start_verify(window);
            }
            Request::OpenWorkspace => self.open_first_workspace(window),
            Request::StartMain => self.start_main_services(window),
        }
    }

    fn onboarding_create(&mut self, window: WindowId) {
        let Some(flow) = self.onboarding.as_mut() else {
            return;
        };
        let request = {
            let state = flow.shared.borrow();
            let form = &state.form;
            pom_core::ScaffoldRequest {
                name: form.name(),
                root: state.sessions_root.to_string_lossy().into_owned(),
                default_branch: form.default_branch(),
                repos: form
                    .repos
                    .iter()
                    .map(|repo| pom_core::RepoSpec {
                        path: repo.source.clone(),
                        alias: repo.alias.text().trim().to_string(),
                    })
                    .collect(),
                skip_secrets: !form.import_secrets,
            }
        };
        let with_agent = flow.shared.borrow().form.with_agent;
        flow.shared.borrow_mut().start_run();
        flow.cancel = Arc::new(AtomicBool::new(false));
        flow.installed.clear();
        flow.session = Some(request.name.clone());
        let sender = flow.sender.clone();
        let cancel = flow.cancel.clone();
        flow.previous = self
            .mains
            .get(&window)
            .and_then(|main| main.project.as_ref())
            .map(|project| project.root.clone());
        std::thread::spawn(move || {
            let events = sender.clone();
            let result = pom_core::scaffold_session_with(
                &request,
                &pom_paths::StateDir::from_env(),
                &mut |event| {
                    if events.send(Event::Scaffold(event)).is_ok() {
                        ui::wake();
                    }
                },
                &cancel,
            );
            if sender.send(Event::Scaffolded(result)).is_err() {
                eprintln!("onboarding: the app stopped waiting");
            }
            ui::wake();
        });
        if self.settings.onboard_with_ai != with_agent {
            self.settings.onboard_with_ai = with_agent;
            self.with_settings_view(|view, _| view.remember_onboard_with_ai(with_agent));
            if let Err(error) = self.settings.save() {
                eprintln!("could not save settings: {error}");
            }
        }
    }

    fn onboarding_event(&mut self, window: WindowId, event: Event) {
        match event {
            Event::Scaffold(pom_core::ScaffoldEvent::Cloning { repo, percent }) => {
                self.with_run(|run| {
                    if let Some(clone) = run.clone.get_mut(repo) {
                        clone.percent = percent;
                    }
                })
            }
            Event::Scaffold(pom_core::ScaffoldEvent::Cloned { repo, linked }) => {
                self.with_run(|run| {
                    if let Some(clone) = run.clone.get_mut(repo) {
                        clone.percent = 100;
                        clone.linked = Some(linked);
                    }
                    if run.clone.iter().all(|clone| clone.linked.is_some()) {
                        run.enter(Phase::Scan);
                    }
                })
            }
            Event::Scaffold(pom_core::ScaffoldEvent::Scanned(scans)) => self.with_run(|run| {
                run.secrets_imported = scans.iter().map(|scan| scan.env_files).sum();
                run.scans = scans;
            }),
            Event::Scaffolded(Ok(dir)) => self.onboarding_scaffolded(window, dir),
            Event::Scaffolded(Err(error)) if error == pom_core::CANCELLED => {
                if let Some(flow) = self.onboarding.as_ref() {
                    flow.shared.borrow_mut().back_to_form();
                }
                self.with_workspace_view(window, |view, _| {
                    view.show_toast("Stopped. Nothing was created.", None)
                });
            }
            Event::Scaffolded(Err(error)) => self.with_run(|run| run.error = Some(error)),
            Event::Check(check) => {
                if check.status == CheckStatus::Passed && !check.command.is_empty() {
                    if let (Some(flow), CheckKind::Install { repo }) =
                        (self.onboarding.as_mut(), &check.kind)
                    {
                        flow.installed.insert(repo.clone(), check.command.clone());
                    }
                }
                self.with_run(|run| run.record(check));
            }
            Event::Verified => {
                if let Some(flow) = self.onboarding.as_mut() {
                    flow.verifying = false;
                }
                let clean = self
                    .run_value(|run| run.finding.is_none() && run.phase == Phase::Verify)
                    .unwrap_or_default();
                if clean {
                    self.onboarding_done(window);
                }
            }
        }
    }

    fn onboarding_scaffolded(&mut self, window: WindowId, dir: PathBuf) {
        self.refresh_sessions();
        self.open_folder_in(window, &dir);
        if let Some(flow) = self.onboarding.as_ref() {
            let page = OnboardingPage::new(flow.shared.clone());
            self.with_workspace_view(window, |view, _| view.open_page(Box::new(page)));
        }
        self.with_run(|run| {
            if run.phase < Phase::Scan {
                run.enter(Phase::Scan);
            }
            run.enter(Phase::Configure);
        });
        if self.run_value(|run| run.with_agent) == Some(true) {
            self.start_onboard_agent(window);
        } else {
            self.start_verify(window);
        }
    }

    fn start_onboard_agent(&mut self, window: WindowId) {
        let Some(cli) = self.run_value(|run| run.agent) else {
            return;
        };
        let Some(project) = self
            .mains
            .get(&window)
            .and_then(|main| main.project.as_ref())
        else {
            return;
        };
        let (Ok(binary), Some(home)) = (std::env::current_exe(), std::env::var_os("HOME")) else {
            return;
        };
        let home = PathBuf::from(home);
        let state = pom_paths::StateDir::from_env();
        let cwd = project.active_root();
        let branch = project.active_branch().to_string();
        let launch = pom_agent::onboard_launch_with(
            &pom_agent::LaunchContext {
                state: &state,
                home: &home,
                binary: &binary,
                tool_path: pom_services::tool_path(),
                session: &project.session,
                branch: &branch,
                is_main: true,
                cwd: &cwd,
            },
            cli,
        );
        let item = format!("agent:{}", launch.holder);
        if let Some(flow) = self.onboarding.as_mut() {
            flow.agent_item = Some(item);
            flow.agent_holder = Some(launch.holder.clone());
            flow.agent_busy = false;
        }
        self.open_agent_item_with(
            window,
            launch,
            crate::AgentTab {
                icon: Some(ui::IconKind::Sparkle),
                side: true,
                ..crate::AgentTab::default()
            },
        );
        self.with_run(|run| run.agent_shown = true);
    }

    fn stop_onboard_agent(&mut self, window: WindowId) {
        let Some(flow) = self.onboarding.as_mut() else {
            return;
        };
        let item = flow.agent_item.take();
        let holder = flow.agent_holder.take();
        if let Some(item) = item {
            self.with_workspace_view(window, |view, _| view.close_agent_item(&item));
        }
        if let Some(holder) = holder {
            let holders = pom_ptyhost::SocketDir::from_env();
            if let Err(error) = holders.kill_holder(&holder) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("onboarding: stop the agent: {error}");
                }
            }
        }
    }

    fn toggle_agent_cli(&mut self, window: WindowId) {
        let Some(item) = self
            .onboarding
            .as_ref()
            .and_then(|flow| flow.agent_item.clone())
        else {
            return;
        };
        let visible = self
            .with_workspace_view(window, |view, _| view.agent_dock_visible())
            .unwrap_or_default();
        if visible {
            self.with_workspace_view(window, |view, _| view.hide_agent_dock());
        } else {
            self.with_workspace_view(window, |view, _| view.open_agent_item(&item, || None));
        }
        self.with_run(|run| run.agent_shown = !visible);
    }

    /// Watches the agent: once it has worked and gone quiet, configuring (or a fix) is done and verify runs.
    fn follow_agent(&mut self, window: WindowId) -> bool {
        let Some(flow) = self.onboarding.as_ref() else {
            return false;
        };
        let Some(session) = flow.session.clone() else {
            return false;
        };
        let (waiting, cli, paused) = {
            let state = flow.shared.borrow();
            let Some(run) = state.run.as_ref() else {
                return false;
            };
            let fixing = run.finding.as_ref().is_some_and(|finding| finding.fixing);
            (
                run.agent_active() && (run.phase == Phase::Configure || fixing),
                run.agent,
                run.paused,
            )
        };
        if !waiting || cli != AgentCli::Claude || flow.verifying {
            return false;
        }
        let branch = self
            .mains
            .get(&window)
            .and_then(|main| main.project.as_ref())
            .map(|project| project.active_branch().to_string())
            .unwrap_or_default();
        let agent = self.agents.states.get(&(session, branch)).copied();
        let busy = matches!(
            agent,
            Some(AgentState::Thinking | AgentState::ToolUse | AgentState::Compacting)
        );
        let Some(flow) = self.onboarding.as_mut() else {
            return false;
        };
        if busy {
            flow.agent_busy = true;
            return false;
        }
        if !flow.agent_busy || agent != Some(AgentState::Idle) || paused {
            return false;
        }
        flow.agent_busy = false;
        self.with_run(|run| {
            if run.finding.is_some() {
                run.restart_verify();
            }
        });
        self.start_verify(window);
        true
    }

    fn resume_after_pause(&mut self, window: WindowId) {
        let idle = self
            .run_value(|run| run.phase == Phase::Verify && run.finding.is_none())
            .unwrap_or_default();
        let verifying = self.onboarding.as_ref().is_some_and(|flow| flow.verifying);
        if idle && !verifying {
            self.start_verify(window);
        }
    }

    /// What the config sets up, shown under Configure while the agent writes it.
    fn refresh_summary(&mut self, window: WindowId) -> bool {
        let lines = self
            .mains
            .get(&window)
            .and_then(|main| main.project.as_ref())
            .and_then(|project| project.config.as_ref())
            .map(onboarding::summary_lines);
        let Some(lines) = lines else {
            return false;
        };
        let Some(flow) = self.onboarding.as_ref() else {
            return false;
        };
        let mut state = flow.shared.borrow_mut();
        let Some(run) = state.run.as_mut() else {
            return false;
        };
        if run.phase < Phase::Configure || run.summary == lines {
            return false;
        }
        run.summary = lines;
        true
    }

    fn start_verify(&mut self, window: WindowId) {
        let paused = self.run_value(|run| run.paused).unwrap_or_default();
        self.with_run(|run| {
            if run.phase != Phase::Verify {
                run.enter(Phase::Verify);
            }
        });
        if paused {
            return;
        }
        let Some(main) = self.mains.get(&window) else {
            return;
        };
        let (Some(project), Some(services)) = (main.project.as_ref(), main.services.as_ref())
        else {
            return;
        };
        let Some(config) = project.config.clone() else {
            let check = Check {
                kind: CheckKind::Doctor,
                status: CheckStatus::Failed,
                detail: "pom.yml does not load".into(),
                output: project
                    .error
                    .as_ref()
                    .map(|problem| problem.message.clone())
                    .unwrap_or_default(),
                command: String::new(),
            };
            self.with_run(|run| run.record(check));
            return;
        };
        let runner = services.runner.clone();
        let root = project.root.clone();
        let config_path = project.config_path.clone();
        let session = project.session.clone();
        let Some(flow) = self.onboarding.as_mut() else {
            return;
        };
        if flow.verifying {
            return;
        }
        flow.verifying = true;
        let installed = flow.installed.clone();
        let skipped = flow
            .shared
            .borrow()
            .run
            .as_ref()
            .map(|run| run.skipped.clone())
            .unwrap_or_default();
        let sender = flow.sender.clone();
        let cancel = flow.cancel.clone();
        std::thread::spawn(move || {
            let names = pom_secrets::SecretStore::new(pom_paths::StateDir::from_env(), &session)
                .names()
                .unwrap_or_default();
            let path = pom_services::tool_path();
            let has_tool = |tool: &str| pom_doctor::on_path(path, tool);
            let docker_running = || pom_doctor::docker_answers(path);
            let machine = pom_doctor::Machine {
                has_tool: &has_tool,
                docker_running: &docker_running,
            };
            let input = onboarding::VerifyInput {
                config: &config,
                config_path: &config_path,
                root: &root,
                secret_names: &names,
                machine: &machine,
                installed: &installed,
                skipped: &skipped,
                boot_timeout: onboarding::BOOT_TIMEOUT,
                worker_grace: onboarding::WORKER_GRACE,
            };
            let services = onboarding::RunnerServices {
                runner: &runner,
                config: &config,
            };
            let events = sender.clone();
            onboarding::verify(
                &input,
                &services,
                &mut |check| {
                    if events.send(Event::Check(check.clone())).is_ok() {
                        ui::wake();
                    }
                },
                &cancel,
            );
            if sender.send(Event::Verified).is_err() {
                eprintln!("onboarding: the app stopped waiting");
            }
            ui::wake();
        });
    }

    fn fix_with_agent(&mut self, window: WindowId) {
        let Some(check) = self
            .run_value(|run| run.finding.as_ref().map(|finding| finding.check.clone()))
            .flatten()
        else {
            return;
        };
        let Some(item) = self
            .onboarding
            .as_ref()
            .and_then(|flow| flow.agent_item.clone())
        else {
            return;
        };
        let what = match &check.kind {
            CheckKind::Doctor => "config_doctor reports errors".to_string(),
            CheckKind::Install { repo } => format!("the setup of {repo} failed in main"),
            CheckKind::Boot { repo, service } => {
                format!("{repo} > {service} did not boot in main")
            }
        };
        let prompt = format!(
            "Pomelo's verify found a problem: {what}. {}\n\nOutput:\n{}\n\nFix pom.yml (setup, migrate, env wiring) so it passes, loop config_doctor until clean, then say done in one line.",
            check.detail,
            check.output.trim()
        );
        let sent = self
            .with_workspace_view(window, |view, _| view.send_to_agent(&item, &prompt))
            .unwrap_or_default();
        if !sent {
            self.with_workspace_view(window, |view, _| {
                view.show_toast("The agent is not open; open it with Show agent CLI", None)
            });
            return;
        }
        self.with_run(|run| {
            if let Some(finding) = run.finding.as_mut() {
                finding.fixing = true;
                finding.manual = false;
            }
            run.agent_shown = true;
        });
        if let Some(flow) = self.onboarding.as_mut() {
            flow.agent_busy = false;
        }
        let title = self.run_value(|run| run.agent.title()).unwrap_or_default();
        self.with_workspace_view(window, |view, _| {
            view.open_agent_item(&item, || None);
            view.show_toast(format!("Sent to {title} with the output"), None)
        });
    }

    fn onboarding_done(&mut self, window: WindowId) {
        let Some(main) = self.mains.get(&window) else {
            return;
        };
        let Some(config) = main
            .project
            .as_ref()
            .and_then(|project| project.config.clone())
        else {
            return;
        };
        let runner = main
            .services
            .as_ref()
            .map(|services| services.runner.clone());
        let counts = onboarding::counts(&config);
        let (scans, start_shared, first) = self
            .run_value(|run| {
                (
                    run.scans.clone(),
                    run.start_shared,
                    run.first_workspace.clone(),
                )
            })
            .unwrap_or_default();
        let set_up = config
            .repos
            .iter()
            .map(|(repo, dir)| {
                let alias = if dir.alias.is_empty() {
                    repo.clone()
                } else {
                    dir.alias.clone()
                };
                let stack = scans
                    .iter()
                    .find(|scan| scan.name == *repo)
                    .map(|scan| scan.stack.clone())
                    .unwrap_or_default();
                let mut services: Vec<String> = dir.services.keys().cloned().collect();
                let mut text = services.join(", ");
                let mut steps = Vec::new();
                if !dir.effective_migrate().is_empty() {
                    steps.push("migrate");
                }
                if !dir.seed.is_empty() {
                    steps.push("seed");
                }
                if !steps.is_empty() {
                    services.clear();
                    text = format!("{text} - {}", steps.join(", "));
                }
                (alias, stack, text)
            })
            .collect();
        self.with_run(|run| {
            run.counts = counts;
            run.set_up = set_up;
        });
        if let Some(flow) = self.onboarding.as_ref() {
            flow.shared.borrow_mut().finish();
        }
        if start_shared && !config.shared_services.is_empty() {
            if let Some(runner) = runner {
                let config = config.clone();
                std::thread::spawn(move || {
                    if let Err(error) = runner.ensure_shared(&config) {
                        eprintln!("onboarding: shared services: {error}");
                    }
                    ui::wake();
                });
            }
        }
        if let Some(branch) = first {
            self.queue_create(
                window,
                &workspaces_ui::CreateWorkspace {
                    branch,
                    display_name: String::new(),
                    repos: Vec::new(),
                    board: None,
                    repo_branches: Default::default(),
                },
            );
        }
    }

    fn open_first_workspace(&mut self, window: WindowId) {
        let first = self.run_value(|run| run.first_workspace.clone()).flatten();
        let Some(branch) = first else {
            self.open_create_workspace(window);
            return;
        };
        let index = self
            .mains
            .get(&window)
            .and_then(|main| main.project.as_ref())
            .and_then(|project| {
                project
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.branch == branch)
            });
        match index {
            Some(index) => {
                self.activate_workspace(window, index);
                self.with_workspace_view(window, |view, _| view.close_page(onboarding_ui::TAB_ID));
                self.onboarding = None;
            }
            None => {
                self.with_workspace_view(window, |view, _| {
                    view.show_toast(format!("{branch} is still being created"), None)
                });
            }
        }
    }

    fn start_main_services(&mut self, window: WindowId) {
        let Some(main) = self.mains.get(&window) else {
            return;
        };
        let (Some(config), Some(services)) = (
            main.project
                .as_ref()
                .and_then(|project| project.config.clone()),
            main.services.as_ref(),
        ) else {
            return;
        };
        let runner = services.runner.clone();
        std::thread::spawn(move || {
            let branch = config.global_default_branch().to_string();
            for target in pom_services::ServiceRunner::service_targets(&config, &branch, true) {
                if let Err(error) = runner.start(&config, &target) {
                    eprintln!("onboarding: start {}: {error}", target.service);
                }
            }
            ui::wake();
        });
        self.with_workspace_view(window, |view, _| {
            view.run_action(workspace::keymap::Action::FocusServices);
            view.show_toast("Starting main's services...", None)
        });
    }

    /// Stops everything the setup started and removes the project it created, then shows the form again.
    fn onboarding_cancel(&mut self, window: WindowId) {
        let Some(flow) = self.onboarding.as_ref() else {
            return;
        };
        flow.cancel.store(true, Ordering::Relaxed);
        let created = flow.session.clone().filter(|_| {
            flow.shared
                .borrow()
                .run
                .as_ref()
                .is_some_and(|run| run.phase >= Phase::Configure)
        });
        let Some(session) = created else {
            // Clone and scan clean up after themselves once they see the cancel.
            return;
        };
        self.stop_onboard_agent(window);
        let services = self
            .mains
            .get(&window)
            .and_then(|main| main.services.as_ref())
            .map(|services| services.runner.clone());
        let root = self
            .mains
            .get(&window)
            .and_then(|main| main.project.as_ref())
            .map(|project| project.root.clone());
        let previous = self
            .onboarding
            .as_ref()
            .and_then(|flow| flow.previous.clone());
        match previous {
            Some(folder) => self.open_folder_in(window, &folder),
            None => self.open_project_in(window, None),
        }
        let state = pom_paths::StateDir::from_env();
        std::thread::spawn(move || {
            if let Some(runner) = services {
                for holder in runner.running_holders() {
                    if let Err(error) = runner.holders().kill_holder(&holder) {
                        eprintln!("onboarding: stop {holder}: {error}");
                    }
                }
                if let Err(error) = runner.stop_shared() {
                    eprintln!("onboarding: stop shared services: {error}");
                }
            }
            if let Err(error) = pom_core::forget_session(&state, &session) {
                eprintln!("onboarding: forget {session}: {error}");
            }
            if let Err(error) = pom_secrets::SecretStore::new(state, &session).delete_all() {
                eprintln!("onboarding: secrets of {session}: {error}");
            }
            if let Some(root) = root {
                if let Err(error) = std::fs::remove_dir_all(&root) {
                    eprintln!("onboarding: remove {}: {error}", root.display());
                }
            }
            ui::wake();
        });
        self.refresh_sessions();
        if let Some(flow) = self.onboarding.as_mut() {
            flow.session = None;
            flow.shared.borrow_mut().back_to_form();
            let page = OnboardingPage::new(flow.shared.clone());
            self.with_workspace_view(window, |view, _| {
                view.open_page(Box::new(page));
                view.show_toast("Stopped. The session folder was removed.", None)
            });
        }
    }
}

/// What the welcome page says about this Mac, checked off the main thread.
#[derive(Default)]
pub(crate) struct MachineChecks {
    receiver: Option<Receiver<Vec<workspace::MachineCheck>>>,
    checked: bool,
}

fn docker_version(path: &str) -> Option<String> {
    let output = std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .env("PATH", path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !text.is_empty()).then_some(text)
}

fn git_version(path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("--version")
        .env("PATH", path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim().strip_prefix("git version ").map(|version| {
        version
            .split_whitespace()
            .next()
            .unwrap_or(version)
            .to_string()
    })
}

fn machine_checks() -> Vec<workspace::MachineCheck> {
    let path = pom_services::tool_path();
    let docker = pom_doctor::docker_answers(path);
    let git = git_version(path);
    let installed: Vec<&str> = agents()
        .into_iter()
        .filter(|(_, installed)| *installed)
        .map(|(cli, _)| cli.title())
        .collect();
    vec![
        workspace::MachineCheck {
            name: "Docker".into(),
            ok: docker,
            detail: if docker {
                match docker_version(path) {
                    Some(version) => format!("Running ({version})"),
                    None => "Running".into(),
                }
            } else {
                "Docker is not running - shared services cannot start".into()
            },
            fix: (!docker).then(|| "Start Docker".to_string()),
        },
        workspace::MachineCheck {
            name: "git".into(),
            ok: git.is_some(),
            detail: git.unwrap_or_else(|| "git is not installed - repos cannot be cloned".into()),
            fix: None,
        },
        workspace::MachineCheck {
            name: "Agent CLI".into(),
            ok: !installed.is_empty(),
            detail: if installed.is_empty() {
                "None installed - a project is set up by hand".into()
            } else {
                format!(
                    "{} - a project can be set up by an agent",
                    installed.join(", ")
                )
            },
            fix: None,
        },
    ]
}

impl App {
    /// Checks this Mac again (`force`), or for the first time once a window shows the welcome page.
    pub(crate) fn check_machine(&mut self, force: bool) {
        let welcome = self.mains.values().any(|main| main.project.is_none());
        if self.machine.receiver.is_some() || (!force && (self.machine.checked || !welcome)) {
            return;
        }
        self.machine.checked = true;
        let (sender, receiver) = std::sync::mpsc::channel();
        self.machine.receiver = Some(receiver);
        std::thread::spawn(move || {
            if sender.send(machine_checks()).is_ok() {
                ui::wake();
            }
        });
    }

    pub(crate) fn poll_machine(&mut self) {
        self.check_machine(false);
        let Some(receiver) = self.machine.receiver.as_ref() else {
            return;
        };
        let checks = match receiver.try_recv() {
            Ok(checks) => checks,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Vec::new(),
        };
        self.machine.receiver = None;
        let windows: Vec<WindowId> = self.mains.keys().copied().collect();
        for id in windows {
            let checks = checks.clone();
            self.with_workspace_view(id, |view, _| view.set_machine_checks(checks));
            self.mark_dirty(id);
        }
    }

    /// The welcome page's fix for check `index`: Docker is the only one with a fix.
    pub(crate) fn fix_machine(&mut self, index: usize) {
        if index != 0 {
            return;
        }
        let started = ["Docker", "OrbStack"].iter().any(|app| {
            std::process::Command::new("open")
                .args(["-a", app])
                .status()
                .is_ok_and(|status| status.success())
        });
        if !started {
            let windows: Vec<WindowId> = self.mains.keys().copied().collect();
            for id in windows {
                self.with_workspace_view(id, |view, _| {
                    view.show_toast(
                        "Install Docker Desktop or OrbStack to run shared services",
                        None,
                    )
                });
            }
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        self.machine.receiver = Some(receiver);
        std::thread::spawn(move || {
            let path = pom_services::tool_path();
            for _ in 0..30 {
                if pom_doctor::docker_answers(path) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            if sender.send(machine_checks()).is_ok() {
                ui::wake();
            }
        });
    }
}
