//! A service's tab: its console under a header (status, name, mode, start/stop), the facts about how it runs,
//! and what went wrong when it crashed. The console follows the service: its live output while it runs, the
//! output it left once it stops.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pom_services::ServiceTarget;
use terminal::{HolderOptions, TerminalOptions, Waker};
use terminal_ui::{ConsoleToolbar, TerminalItem};
use ui::{div, icon, label, theme, IconKind, Node};

use crate::model::{run_action, Action, Crash, ServicesContext, Shared, Status, TabRequest};
use crate::view::{self, action_button, State, Tone};

const START: u64 = 0;
const STOP: u64 = 1;
const RESTART: u64 = 2;
const OPEN: u64 = 3;
const FIX: u64 = 4;
const NEW_PORT: u64 = 5;
const COPY_URL: u64 = 6;
const COPY_COMMAND: u64 = 7;
const MODE_BASE: u64 = 16;
const PROFILE_BASE: u64 = 32;
const IDS_PER_TAB: u64 = 64;
/// Toolbar ids sit far above every panel's and pane group's ids.
const TOOLBAR_IDS: u64 = 1 << 50;

/// What the console of a service's tab shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Showing {
    Live,
    Leftover,
}

pub(crate) struct ServiceTab {
    context: Arc<ServicesContext>,
    shared: Arc<Mutex<Shared>>,
    target: ServiceTarget,
    holder: String,
    root: PathBuf,
    base: u64,
    showing: Showing,
}

pub(crate) fn tab_id(holder: &str) -> String {
    format!("service:{holder}")
}

/// The service's tab, its console showing what the service is doing now.
pub(crate) fn open_tab(
    context: Arc<ServicesContext>,
    shared: Arc<Mutex<Shared>>,
    target: ServiceTarget,
    root: PathBuf,
    number: u64,
) -> Option<Box<dyn workspace::Item>> {
    let holder = context.runner.holder_name(&target);
    let running = context.status_of(&target) == Status::Running;
    let tab = ServiceTab {
        base: TOOLBAR_IDS + number * IDS_PER_TAB,
        showing: if running {
            Showing::Live
        } else {
            Showing::Leftover
        },
        context,
        shared,
        target,
        holder,
        root,
    };
    let (options, waker) = tab.console(tab.showing);
    let terminal = match terminal::Terminal::spawn(options, waker) {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("services: console: {error}");
            return None;
        }
    };
    let item = TerminalItem::with_terminal(number, tab.root.clone(), terminal)
        .into_console(tab_id(&tab.holder), crate::service_title(&tab.target));
    Some(Box::new(item.with_toolbar(Box::new(tab))))
}

/// The header and facts a service's tab would show, without its console (snapshots).
pub(crate) fn toolbar_preview(
    context: Arc<ServicesContext>,
    shared: Arc<Mutex<Shared>>,
    target: ServiceTarget,
    root: PathBuf,
    width: f32,
) -> Node {
    let holder = context.runner.holder_name(&target);
    ServiceTab {
        context,
        shared,
        target,
        holder,
        root,
        base: TOOLBAR_IDS,
        showing: Showing::Leftover,
    }
    .render(width)
}

impl ServiceTab {
    fn console(&self, showing: Showing) -> (TerminalOptions, Waker) {
        let runner = &self.context.runner;
        let holders = runner.holders().clone();
        let options = match showing {
            Showing::Live => TerminalOptions {
                working_directory: Some(self.root.clone()),
                holder: Some(HolderOptions {
                    dir: holders,
                    name: self.holder.clone(),
                    binary: std::env::current_exe().unwrap_or_default(),
                    attach_only: true,
                }),
                ..TerminalOptions::default()
            },
            Showing::Leftover => {
                let log = holders.crash_log(&self.holder);
                let shell = if log.is_file() {
                    (
                        "/usr/bin/tail".to_string(),
                        vec![
                            "-n".to_string(),
                            "+2".to_string(),
                            log.to_string_lossy().into_owned(),
                        ],
                    )
                } else {
                    (
                        "/bin/echo".to_string(),
                        vec!["Not running. Start it to follow its output here.".to_string()],
                    )
                };
                TerminalOptions {
                    shell: Some(shell),
                    working_directory: Some(self.root.clone()),
                    ..TerminalOptions::default()
                }
            }
        };
        (options, self.context.waker.clone())
    }

    fn state(&self) -> (State, Option<Crash>, Option<String>) {
        let Ok(shared) = self.shared.lock() else {
            return (State::Stopped, None, None);
        };
        let error = shared.errors.get(&self.holder).cloned();
        let crash = shared.crashes.get(&self.holder).cloned();
        let status = shared
            .status
            .get(&self.holder)
            .copied()
            .unwrap_or(Status::Stopped);
        let state = match (shared.pending.get(&self.holder), status) {
            (Some(action), _) => State::Busy(action.progress_label()),
            (None, Status::Running) => State::Running,
            (None, Status::Crashed) => State::Crashed,
            (None, Status::Stopped) => match &error {
                Some(message) => State::Failed {
                    port: crate::port_in(message),
                },
                None => State::Stopped,
            },
        };
        (state, crash, error)
    }

    fn service(&self) -> Option<(Arc<pom_config::Config>, pom_config::Service)> {
        let config = self.context.config()?;
        let service = if self.target.is_workspace_level() {
            config.workspace_services.get(&self.target.service)?.clone()
        } else {
            config
                .repos
                .get(&self.target.repo)?
                .services
                .get(&self.target.service)?
                .clone()
        };
        Some((config, service))
    }

    fn url(&self) -> Option<String> {
        let config = self.context.config()?;
        self.context.runner.proxy_url(&config, &self.target)
    }

    fn run(&self, action: Action) {
        run_action(&self.context, &self.shared, action, self.target.clone());
    }

    fn ask(&self, request: TabRequest) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.tab_requests.push(request);
        }
        (self.context.waker)();
    }

    fn button(&self, offset: u64, kind: IconKind, text: &str, tone: Tone) -> Node {
        action_button(self.base + offset, Some(kind), text, tone, false)
    }

    fn fact(title: &str, value: Node) -> Node {
        div()
            .col()
            .gap(3.0)
            .child(
                label(title.to_string())
                    .size(11.0)
                    .color(theme().text_placeholder),
            )
            .child(value)
            .into()
    }

    /// A choice among `options` with `current` pressed; each option clicks at `base + index`.
    fn choice(&self, options: &[String], current: &str, base: u64) -> Node {
        let colors = theme();
        let mut row = div()
            .row()
            .gap(2.0)
            .p(2.0)
            .rounded(4.0)
            .bg(colors.element_background);
        for (index, option) in options.iter().enumerate() {
            let pressed = option == current;
            row = row.child(
                div()
                    .row()
                    .h_px(18.0)
                    .px(6.0)
                    .items_center()
                    .rounded(3.0)
                    .on_click(self.base + base + index as u64)
                    .bg(if pressed {
                        colors.element_selected
                    } else {
                        ui::Rgba::TRANSPARENT
                    })
                    .child(label(option.clone()).size(12.0).color(if pressed {
                        colors.text
                    } else {
                        colors.text_muted
                    })),
            );
        }
        row.into()
    }

    fn uptime(&self) -> Option<String> {
        let pidfile = self.context.runner.holders().pidfile(&self.holder);
        let started = std::fs::metadata(pidfile)
            .and_then(|meta| meta.modified())
            .ok()?;
        let seconds = started.elapsed().ok()?.as_secs();
        Some(match seconds {
            0..=59 => format!("{seconds}s"),
            60..=3599 => format!("{}m {}s", seconds / 60, seconds % 60),
            3600..=86_399 => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
            _ => format!("{}d {}h", seconds / 86_400, (seconds % 86_400) / 3600),
        })
    }

    fn header(&self, state: &State) -> Node {
        let colors = theme();
        let running = *state == State::Running;
        let busy = matches!(state, State::Busy(_));
        let mut title = div()
            .row()
            .flex(1.0)
            .gap(8.0)
            .items_center()
            .child(view::status_icon(state));
        title = if self.target.is_workspace_level() {
            title.child(
                label(self.target.service.clone())
                    .size(14.0)
                    .medium()
                    .color(colors.text),
            )
        } else {
            title
                .child(
                    label(self.target.repo.clone())
                        .size(14.0)
                        .color(colors.text_muted),
                )
                .child(label(">").size(14.0).color(colors.text_placeholder))
                .child(
                    label(self.target.service.clone())
                        .size(14.0)
                        .medium()
                        .color(colors.text),
                )
        };
        if let Some(mode) = self
            .service()
            .map(|(_, service)| {
                self.context
                    .runner
                    .mode(&self.target.repo, &self.target.service, &service)
            })
            .filter(|mode| !mode.is_empty())
        {
            title = title.child(view::tag(&mode));
        }
        let mut buttons = Vec::new();
        if !busy {
            if running {
                buttons.push(self.button(STOP, IconKind::Stop, "Stop", Tone::Plain));
                buttons.push(self.button(RESTART, IconKind::RotateCw, "Restart", Tone::Plain));
            } else {
                buttons.push(self.button(START, IconKind::Play, "Start", Tone::Primary));
            }
        }
        if running && self.url().is_some() {
            buttons.push(self.button(OPEN, IconKind::ArrowUpRight, "Open in browser", Tone::Plain));
        }
        let mut row = div()
            .row()
            .h_px(44.0)
            .px(14.0)
            .gap(6.0)
            .items_center()
            .child(title);
        for button in buttons {
            row = row.child(button);
        }
        row.into()
    }

    fn trouble(
        &self,
        state: &State,
        crash: Option<&Crash>,
        error: Option<&str>,
        width: f32,
    ) -> Option<Node> {
        if !state.needs_attention() {
            return None;
        }
        let colors = theme();
        let port = match state {
            State::Failed { port } => *port,
            _ => None,
        };
        let tint = if port.is_some() {
            colors.warning
        } else {
            colors.error
        };
        let inner = width - 28.0 - 22.0;
        let heading = match (crash, port) {
            (Some(crash), _) => match crash.at {
                Some(at) => format!("Crashed {} - {}", view::ago(at), crash.exit),
                None => format!("Crashed - {}", crash.exit),
            },
            (None, Some(port)) => format!("Port {port} is already in use"),
            (None, None) => "Could not start".to_string(),
        };
        let mut block = div()
            .col()
            .w_px(width - 28.0)
            .p(10.0)
            .gap(7.0)
            .rounded(6.0)
            .border(1.0, tint.alpha(0.3))
            .bg(tint.alpha(0.06))
            .child(
                div()
                    .row()
                    .gap(6.0)
                    .items_center()
                    .child(
                        icon(if port.is_some() {
                            IconKind::Warning
                        } else {
                            IconKind::XCircle
                        })
                        .size(12.0)
                        .color(tint),
                    )
                    .child(label(heading).color(tint)),
            );
        let detail = crash
            .and_then(|crash| crash.line.clone())
            .or_else(|| error.map(str::to_string));
        if let Some(detail) = detail {
            block = block.child(
                div()
                    .row()
                    .w_px(inner)
                    .p(8.0)
                    .rounded(4.0)
                    .bg(ui::Rgba::new(0.0, 0.0, 0.0, 0.2))
                    .child(
                        label(detail)
                            .size(12.0)
                            .mono()
                            .color(colors.text)
                            .wrap(inner - 16.0),
                    ),
            );
        }
        let mut buttons = Vec::new();
        if port.is_some() {
            buttons.push(self.button(
                NEW_PORT,
                IconKind::ArrowUpRight,
                "Use a new port",
                Tone::Primary,
            ));
        }
        buttons.push(self.button(FIX, IconKind::Sparkle, "Fix with Claude", Tone::Agent));
        buttons.push(self.button(RESTART, IconKind::RotateCw, "Restart", Tone::Plain));
        block = block.child(view::pack(buttons, inner, 6.0));
        Some(div().row().px(14.0).pt(10.0).child(block).into())
    }

    fn facts(&self, state: &State, crash: Option<&Crash>, width: f32) -> Node {
        let colors = theme();
        let status: Node = match state {
            State::Running => {
                let mut text = "Running".to_string();
                if let Some(uptime) = self.uptime() {
                    text.push_str(&format!(" - up {uptime}"));
                }
                label(text).size(12.5).color(colors.success).into()
            }
            State::Busy(text) => label(*text).size(12.5).color(colors.text_accent).into(),
            State::Crashed => {
                let text = match crash {
                    Some(crash) => format!("Crashed - {}", crash.exit),
                    None => "Crashed".to_string(),
                };
                label(text).size(12.5).color(colors.error).into()
            }
            State::Failed { .. } => label("Could not start")
                .size(12.5)
                .color(colors.warning)
                .into(),
            State::Stopped => label("Stopped").size(12.5).color(colors.text_muted).into(),
        };
        let mut facts = vec![Self::fact("Status", status)];
        if *state == State::Running {
            if let Some(pid) = self.context.runner.holders().holder_pid(&self.holder) {
                facts.push(Self::fact(
                    "PID",
                    label(pid.to_string())
                        .size(12.5)
                        .mono()
                        .color(colors.text)
                        .into(),
                ));
            }
        }
        if let Some(url) = self.url() {
            facts.push(Self::fact(
                "URL",
                div()
                    .row()
                    .gap(6.0)
                    .items_center()
                    .child(label(url).size(12.5).mono().color(colors.text_accent))
                    .child(
                        div()
                            .on_click(self.base + COPY_URL)
                            .child(icon(IconKind::Copy).size(11.0).color(colors.icon_muted)),
                    )
                    .into(),
            ));
        }
        if let Some((config, service)) = self.service() {
            let modes = service.mode_names();
            if modes.len() > 1 {
                let current =
                    self.context
                        .runner
                        .mode(&self.target.repo, &self.target.service, &service);
                facts.push(Self::fact("Mode", self.choice(&modes, &current, MODE_BASE)));
            }
            if let Some(dir) = config.repos.get(&self.target.repo) {
                let profiles = dir.env_profiles(Some(&service));
                if profiles.len() > 1 {
                    let current = self.context.runner.env_profile(&config, &self.target);
                    facts.push(Self::fact(
                        "Env profile",
                        self.choice(&profiles, &current, PROFILE_BASE),
                    ));
                }
            }
            let mode = self
                .context
                .runner
                .mode(&self.target.repo, &self.target.service, &service);
            let command = service.active_cmd(&mode).trim().to_string();
            if !command.is_empty() {
                facts.push(Self::fact(
                    "Command",
                    div()
                        .row()
                        .gap(6.0)
                        .items_center()
                        .child({
                            let text = label(command).size(12.5).mono().color(colors.text);
                            let natural = ui::measure(&text.clone().into()).0;
                            div()
                                .row()
                                .w_px(natural.min(width - 60.0))
                                .child(text.truncate())
                        })
                        .child(
                            div()
                                .on_click(self.base + COPY_COMMAND)
                                .child(icon(IconKind::Copy).size(11.0).color(colors.icon_muted)),
                        )
                        .into(),
                ));
            }
        }
        div()
            .col()
            .px(14.0)
            .pt(10.0)
            .pb(11.0)
            .child(view::pack(facts, width - 28.0, 22.0))
            .into()
    }
}

impl ConsoleToolbar for ServiceTab {
    fn render(&self, width: f32) -> Node {
        let colors = theme();
        let (state, crash, error) = self.state();
        let mut column = div()
            .col()
            .w_px(width)
            .bg(colors.editor_background)
            .child(self.header(&state))
            .child(div().h_px(1.0).bg(colors.border_variant));
        if let Some(trouble) = self.trouble(&state, crash.as_ref(), error.as_deref(), width) {
            column = column.child(trouble);
        }
        column
            .child(self.facts(&state, crash.as_ref(), width))
            .child(div().h_px(1.0).bg(colors.border))
            .into()
    }

    fn click(&mut self, id: u64) -> bool {
        let Some(offset) = id
            .checked_sub(self.base)
            .filter(|offset| *offset < IDS_PER_TAB)
        else {
            return false;
        };
        match offset {
            START => self.run(Action::Start),
            STOP => self.run(Action::Stop),
            RESTART => self.run(Action::Restart),
            NEW_PORT => self.run(Action::Relocate),
            OPEN => {
                if let Some(url) = self.url() {
                    self.ask(TabRequest::OpenUrl(url));
                }
            }
            COPY_URL => {
                if let Some(url) = self.url() {
                    self.ask(TabRequest::Copy(url));
                }
            }
            COPY_COMMAND => {
                if let Some((_, service)) = self.service() {
                    let mode =
                        self.context
                            .runner
                            .mode(&self.target.repo, &self.target.service, &service);
                    self.ask(TabRequest::Copy(
                        service.active_cmd(&mode).trim().to_string(),
                    ));
                }
            }
            FIX => self.ask(TabRequest::Fix(self.target.clone())),
            offset if (MODE_BASE..PROFILE_BASE).contains(&offset) => {
                self.pick_mode((offset - MODE_BASE) as usize)
            }
            offset if offset >= PROFILE_BASE => self.pick_profile((offset - PROFILE_BASE) as usize),
            _ => {}
        }
        true
    }

    fn take_respawn(&mut self) -> Option<(TerminalOptions, Waker)> {
        let (state, _, _) = self.state();
        let wanted = match state {
            State::Running => Showing::Live,
            State::Crashed | State::Failed { .. } | State::Stopped => Showing::Leftover,
            State::Busy(_) => return None,
        };
        if wanted == self.showing {
            return None;
        }
        self.showing = wanted;
        Some(self.console(wanted))
    }
}

impl ServiceTab {
    fn restart_if_running(&self) {
        if self.state().0 == State::Running {
            self.run(Action::Restart);
        }
    }

    fn pick_mode(&self, index: usize) {
        let Some((config, service)) = self.service() else {
            return;
        };
        let Some(mode) = service.mode_names().get(index).cloned() else {
            return;
        };
        match self
            .context
            .runner
            .set_mode(&config, &self.target.repo, &self.target.service, &mode)
        {
            Ok(()) => self.restart_if_running(),
            Err(error) => self.toast(error.to_string()),
        }
    }

    fn pick_profile(&self, index: usize) {
        let Some((config, service)) = self.service() else {
            return;
        };
        let Some(dir) = config.repos.get(&self.target.repo) else {
            return;
        };
        let Some(profile) = dir.env_profiles(Some(&service)).get(index).cloned() else {
            return;
        };
        match self
            .context
            .runner
            .set_env_profile(&config, &self.target, &profile)
        {
            Ok(()) => self.restart_if_running(),
            Err(error) => self.toast(error.to_string()),
        }
    }

    fn toast(&self, text: String) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.toasts.push(text);
        }
    }
}
