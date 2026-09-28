//! A service's tab: its console under a header (status, name, mode, start/stop), the facts about how it runs,
//! and what went wrong when it crashed. The console follows the service: its live output while it runs, the
//! output it left once it stops.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pom_services::ServiceTarget;
use terminal::{HolderOptions, Keystroke, Modifiers, TerminalOptions, Waker};
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::{EditKey, Item, ItemTick, TerminalKeyOutcome};

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
const OPEN_ENV: u64 = 8;
const FILTER: u64 = 9;
const CLEAR: u64 = 10;
const FOLLOW: u64 = 11;
const PAUSE: u64 = 12;
const WRAP: u64 = 13;
const CLEAR_FILTER: u64 = 14;
const MORE: u64 = 15;
const LOG_ROW_H: f32 = 20.0;
const TIME_W: f32 = 64.0;
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

pub(crate) struct ServiceItem {
    context: Arc<ServicesContext>,
    shared: Arc<Mutex<Shared>>,
    target: ServiceTarget,
    holder: String,
    root: PathBuf,
    base: u64,
    showing: Showing,
    /// Follows the service's output off screen; its lines are what the tab lists.
    terminal: Option<terminal::Terminal>,
    lines: Vec<LogLine>,
    /// Lines shown while paused (the rest wait).
    paused_at: Option<usize>,
    follow: bool,
    wrap: bool,
    filter: TextField,
    filter_focused: bool,
    /// The first line listed when not following (an index into the filtered lines).
    top: usize,
    visible_rows: usize,
    hits: Vec<(Rect, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LogLine {
    /// When the tab saw it arrive; lines that were there before it opened have none.
    time: Option<String>,
    text: String,
}

pub(crate) fn tab_id(holder: &str) -> String {
    format!("service:{holder}")
}

/// The service's tab, following what the service is doing now.
pub(crate) fn open_tab(
    context: Arc<ServicesContext>,
    shared: Arc<Mutex<Shared>>,
    target: ServiceTarget,
    root: PathBuf,
    number: u64,
) -> Option<Box<dyn workspace::Item>> {
    let mut item = ServiceItem::new(context, shared, target, root, number);
    item.respawn();
    Some(Box::new(item))
}

/// The tab as it draws with `lines` of output, `width` x `height` (snapshots).
#[allow(clippy::too_many_arguments)]
pub(crate) fn preview(
    context: Arc<ServicesContext>,
    shared: Arc<Mutex<Shared>>,
    target: ServiceTarget,
    root: PathBuf,
    lines: &[&str],
    filter: &str,
    width: f32,
    height: f32,
) -> Node {
    let mut item = ServiceItem::new(context, shared, target, root, 0);
    item.lines = lines
        .iter()
        .enumerate()
        .map(|(index, text)| LogLine {
            time: Some(format!("15:4{}:{:02}", index / 10, (index * 7) % 60)),
            text: text.to_string(),
        })
        .collect();
    item.filter.set_text(filter);
    item.tree(width, height)
}

impl ServiceItem {
    fn new(
        context: Arc<ServicesContext>,
        shared: Arc<Mutex<Shared>>,
        target: ServiceTarget,
        root: PathBuf,
        number: u64,
    ) -> ServiceItem {
        let holder = context.runner.holder_name(&target);
        ServiceItem {
            base: TOOLBAR_IDS + number * IDS_PER_TAB,
            showing: Showing::Leftover,
            terminal: None,
            lines: Vec::new(),
            paused_at: None,
            follow: true,
            wrap: false,
            filter: {
                let mut field = TextField::default();
                field.set_font_size(12.0);
                field
            },
            filter_focused: false,
            top: 0,
            visible_rows: 0,
            hits: Vec::new(),
            context,
            shared,
            target,
            holder,
            root,
        }
    }

    /// Follows the live output while the service runs, what it left once it stops.
    fn respawn(&mut self) {
        let wanted = match self.state().0 {
            State::Running => Showing::Live,
            State::Busy(_) if self.terminal.is_some() => return,
            _ => Showing::Leftover,
        };
        if self.terminal.is_some() && wanted == self.showing {
            return;
        }
        self.showing = wanted;
        let (options, waker) = self.console(wanted);
        match terminal::Terminal::spawn(options, waker) {
            Ok(mut terminal) => {
                terminal.set_size(terminal::TerminalBounds {
                    cell_width: 8.0,
                    line_height: 16.0,
                    width: 8.0 * 240.0,
                    height: 16.0 * 60.0,
                });
                self.terminal = Some(terminal);
                self.lines.clear();
                self.top = 0;
            }
            Err(error) => eprintln!("services: console: {error}"),
        }
    }

    /// Reads the output again: lines past the ones already listed are stamped with the time they came in.
    fn refresh_lines(&mut self) -> bool {
        let Some(terminal) = self.terminal.as_ref() else {
            return false;
        };
        let output = terminal.output_lines();
        let known = self.lines.len();
        let fresh_start = known > 0;
        if output.len() < known
            || output
                .iter()
                .zip(&self.lines)
                .take(known.min(output.len()).saturating_sub(1))
                .any(|(text, line)| *text != line.text)
        {
            self.lines = output
                .into_iter()
                .map(|text| LogLine { time: None, text })
                .collect();
            return true;
        }
        if output.len() == known && output.last() == self.lines.last().map(|line| &line.text) {
            return false;
        }
        let now = clock_now();
        if let (Some(last), Some(text)) =
            (self.lines.last_mut(), output.get(known.saturating_sub(1)))
        {
            last.text = text.clone();
        }
        for text in output.into_iter().skip(known) {
            self.lines.push(LogLine {
                time: fresh_start.then(|| now.clone()),
                text,
            });
        }
        true
    }

    /// The lines the list shows, as indices: all of them up to the pause, matching the filter.
    fn shown(&self) -> Vec<usize> {
        let end = self
            .paused_at
            .unwrap_or(self.lines.len())
            .min(self.lines.len());
        let query = self.filter.text().to_lowercase();
        (0..end)
            .filter(|index| {
                query.is_empty() || self.lines[*index].text.to_lowercase().contains(&query)
            })
            .collect()
    }

    fn tree(&mut self, width: f32, height: f32) -> Node {
        let colors = theme();
        let (state, crash, error) = self.state();
        let mut top = div()
            .col()
            .w_px(width)
            .child(self.header(&state))
            .child(div().h_px(1.0).bg(colors.border_variant));
        if let Some(trouble) = self.trouble(&state, crash.as_ref(), error.as_deref(), width) {
            top = top.child(trouble);
        }
        let shown = self.shown();
        let top = top
            .child(self.facts(&state, crash.as_ref(), width))
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(self.logs_bar(&state, shown.len()))
            .child(div().h_px(1.0).bg(colors.border_variant));
        let top: Node = top.into();
        let used = ui::measure(&top).1;
        let list_h = (height - used).max(0.0);
        div()
            .col()
            .w_px(width)
            .h_px(height)
            .bg(colors.editor_background)
            .child(top)
            .child(self.log_list(&shown, width, list_h))
            .into()
    }

    fn log_list(&mut self, shown: &[usize], width: f32, height: f32) -> Node {
        let colors = theme();
        let rows = ((height - 8.0) / LOG_ROW_H).floor().max(1.0) as usize;
        self.visible_rows = rows;
        if self.follow || self.top + rows > shown.len() {
            self.top = shown.len().saturating_sub(rows);
        }
        let query = self.filter.text().to_lowercase();
        let text_w = width - 28.0 - TIME_W;
        let mut list = div().col().h_px(height).px(14.0).pt(4.0);
        if shown.is_empty() {
            let note = if !query.is_empty() {
                "No lines match"
            } else if self.state().0 == State::Running {
                "Waiting for output..."
            } else {
                "Not running. Start it to follow its output here."
            };
            return list
                .child(label(note).size(12.5).color(colors.text_placeholder))
                .into();
        }
        let mut used = 0.0;
        for index in shown.iter().skip(self.top) {
            let line = &self.lines[*index];
            let row = log_row(line, &query, text_w, self.wrap);
            let row_h = if self.wrap {
                ui::measure(&row).1.max(LOG_ROW_H)
            } else {
                LOG_ROW_H
            };
            if used + row_h > height - 4.0 {
                break;
            }
            used += row_h;
            list = list.child(row);
        }
        list.into()
    }
}

impl ServiceItem {
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

    fn link(id: u64, text: &str) -> Node {
        div()
            .on_click(id)
            .child(
                label(text.to_string())
                    .size(12.5)
                    .color(theme().text_accent),
            )
            .into()
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
        if running || busy {
            buttons.push(view::disabled_button(Some(IconKind::Play), "Start"));
        } else {
            buttons.push(self.button(START, IconKind::Play, "Start", Tone::Primary));
        }
        if running {
            buttons.push(self.button(STOP, IconKind::Stop, "Stop", Tone::Plain));
            buttons.push(self.button(RESTART, IconKind::RotateCw, "Restart", Tone::Plain));
        }
        if self.url().is_some() {
            if running {
                buttons.push(self.button(
                    OPEN,
                    IconKind::ArrowUpRight,
                    "Open in browser",
                    Tone::Plain,
                ));
            } else {
                buttons.push(view::disabled_button(
                    Some(IconKind::ArrowUpRight),
                    "Open in browser",
                ));
            }
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
        row.child(
            div()
                .row()
                .w_px(24.0)
                .h_px(24.0)
                .items_center()
                .justify_center()
                .rounded(4.0)
                .on_click(self.base + MORE)
                .child(
                    icon(IconKind::Ellipsis)
                        .size(14.0)
                        .color(theme().icon_muted),
                ),
        )
        .into()
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
            if let Some(port) = self.context.runner.port(&config, &self.target) {
                facts.push(Self::fact(
                    "Port",
                    div()
                        .row()
                        .gap(8.0)
                        .items_center()
                        .child(
                            label(format!(":{port}"))
                                .size(12.5)
                                .mono()
                                .color(colors.text),
                        )
                        .child(Self::link(self.base + NEW_PORT, "Change"))
                        .into(),
                ));
            }
            let modes = service.mode_names();
            let current_mode =
                self.context
                    .runner
                    .mode(&self.target.repo, &self.target.service, &service);
            if modes.len() > 1 {
                facts.push(Self::fact(
                    "Mode",
                    self.choice(&modes, &current_mode, MODE_BASE),
                ));
            } else if !current_mode.is_empty() {
                facts.push(Self::fact(
                    "Mode",
                    label(current_mode.clone())
                        .size(12.5)
                        .color(colors.text)
                        .into(),
                ));
            }
            if let Some(dir) = config.repos.get(&self.target.repo) {
                let profiles = dir.env_profiles(Some(&service));
                let current = self.context.runner.env_profile(&config, &self.target);
                if profiles.len() > 1 {
                    facts.push(Self::fact(
                        "Env profile",
                        self.choice(&profiles, &current, PROFILE_BASE),
                    ));
                } else if !current.is_empty() {
                    facts.push(Self::fact(
                        "Env profile",
                        label(current).size(12.5).color(colors.text).into(),
                    ));
                }
                if let Some(file) = dir.env_output.first() {
                    let shown = format!("{}/{}", self.target.repo, file.file);
                    facts.push(Self::fact(
                        "Env file",
                        div()
                            .row()
                            .gap(8.0)
                            .items_center()
                            .child(label(shown).size(12.5).mono().color(colors.text))
                            .child(Self::link(self.base + OPEN_ENV, "Open"))
                            .into(),
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

impl ServiceItem {
    fn click(&mut self, id: u64) {
        let Some(offset) = id
            .checked_sub(self.base)
            .filter(|offset| *offset < IDS_PER_TAB)
        else {
            return;
        };
        self.filter_focused = offset == FILTER;
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
            MORE => self.ask(TabRequest::Menu(self.target.clone())),
            CLEAR => {
                if let Some(terminal) = self.terminal.as_mut() {
                    terminal.clear();
                }
                self.lines.clear();
                self.paused_at = None;
                self.top = 0;
            }
            CLEAR_FILTER => self.filter.set_text(""),
            PAUSE => {
                self.paused_at = match self.paused_at {
                    Some(_) => None,
                    None => Some(self.lines.len()),
                };
            }
            FOLLOW => self.follow = !self.follow,
            WRAP => self.wrap = !self.wrap,
            OPEN_ENV => {
                let file = self.context.config().and_then(|config| {
                    let dir = config.repos.get(&self.target.repo)?;
                    Some(
                        self.root
                            .join(&self.target.repo)
                            .join(&dir.env_output.first()?.file),
                    )
                });
                if let Some(file) = file {
                    self.ask(TabRequest::OpenFile(file));
                }
            }
            offset if (MODE_BASE..PROFILE_BASE).contains(&offset) => {
                self.pick_mode((offset - MODE_BASE) as usize)
            }
            offset if offset >= PROFILE_BASE => self.pick_profile((offset - PROFILE_BASE) as usize),
            _ => {}
        }
    }

    fn edit_key(keystroke: &Keystroke) -> Option<(EditKey, bool)> {
        let Modifiers {
            shift, alt, cmd, ..
        } = keystroke.modifiers;
        let key = match keystroke.key.as_str() {
            "left" if cmd => EditKey::Home,
            "right" if cmd => EditKey::End,
            "left" if alt => EditKey::WordLeft,
            "right" if alt => EditKey::WordRight,
            "left" => EditKey::Left,
            "right" => EditKey::Right,
            "home" => EditKey::Home,
            "end" => EditKey::End,
            "backspace" if cmd => EditKey::DeleteToLineStart,
            "backspace" if alt => EditKey::DeleteWordLeft,
            "backspace" => EditKey::Backspace,
            "delete" => EditKey::Delete,
            "a" if cmd => EditKey::SelectAll,
            _ => return None,
        };
        Some((key, shift))
    }
}

impl Item for ServiceItem {
    fn id(&self) -> Option<String> {
        Some(tab_id(&self.holder))
    }

    fn title(&self) -> String {
        self.target.service.clone()
    }

    fn tab_detail(&self) -> Option<String> {
        (!self.target.is_workspace_level()).then(|| self.target.repo.clone())
    }

    fn tab_dot(&self) -> Option<Rgba> {
        Some(view::state_color(&self.state().0))
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Server)
    }

    /// The body is painted by `paint_body`.
    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let tree = self.tree(body.w / scale, body.h / scale);
        let painted = ui::render(&tree, body);
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn wants_keystrokes(&self) -> bool {
        true
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let cmd = keystroke.modifiers.cmd;
        if cmd && keystroke.key == "f" {
            self.filter_focused = true;
            return TerminalKeyOutcome::Handled;
        }
        if !self.filter_focused {
            return match keystroke.key.as_str() {
                "c" if cmd => self
                    .lines
                    .last()
                    .map_or(TerminalKeyOutcome::Ignored, |line| {
                        TerminalKeyOutcome::Copy(line.text.clone())
                    }),
                _ => TerminalKeyOutcome::Ignored,
            };
        }
        match keystroke.key.as_str() {
            "escape" if self.filter.text().is_empty() => self.filter_focused = false,
            "escape" => self.filter.set_text(""),
            "enter" => self.filter_focused = false,
            "v" if cmd => return TerminalKeyOutcome::Paste,
            _ => match Self::edit_key(keystroke) {
                Some((key, shift)) => {
                    self.filter.key(key, shift);
                }
                None => return TerminalKeyOutcome::Ignored,
            },
        }
        TerminalKeyOutcome::Handled
    }

    fn input_text(&mut self, text: &str) {
        if !self.filter_focused {
            return;
        }
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        self.filter.insert(&typed);
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        if self.filter_focused {
            let line: String = text.chars().filter(|c| !c.is_control()).collect();
            self.filter.insert(&line);
        }
    }

    /// Its buttons can also arrive as window clicks (the window hit-tests the tab's painted ids first).
    fn toolbar_click(&mut self, id: u64) -> bool {
        let ours = id
            .checked_sub(self.base)
            .is_some_and(|offset| offset < IDS_PER_TAB);
        if ours {
            self.click(id);
        }
        ours
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id);
        match hit {
            Some(id) => self.click(id),
            None => self.filter_focused = false,
        }
        true
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let rows = (delta_y / LOG_ROW_H).round() as isize;
        if rows == 0 {
            return false;
        }
        let shown = self.shown().len();
        let last = shown.saturating_sub(self.visible_rows);
        let top = (self.top as isize - rows).clamp(0, last as isize) as usize;
        // Scrolling up leaves the newest lines; reaching the bottom again follows them.
        self.follow = top >= last;
        let moved = top != self.top;
        self.top = top;
        moved
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        self.respawn();
        let host = LogHost {
            palette: terminal_ui::palette(&theme()),
        };
        let synced = self.terminal.as_mut().is_some_and(|terminal| {
            let outcome = terminal.sync(&host);
            outcome.changed || outcome.title_changed
        });
        let changed = synced && self.refresh_lines();
        ItemTick {
            changed: changed || self.state().0 == State::Running,
            ..ItemTick::default()
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

struct LogHost {
    palette: terminal::Palette,
}

impl terminal::TerminalHost for LogHost {
    fn palette(&self) -> &terminal::Palette {
        &self.palette
    }

    fn clipboard_text(&self) -> Option<String> {
        None
    }
}

impl ServiceItem {
    /// The bar over the output: whether it is live, a filter, pause, clear, follow, wrap, and the count.
    fn logs_bar(&self, state: &State, shown: usize) -> Node {
        let colors = theme();
        let live = *state == State::Running && self.paused_at.is_none();
        let mut title = div().row().gap(6.0).items_center().child(
            label("LOGS")
                .size(11.0)
                .weight(600)
                .color(colors.text_placeholder),
        );
        if live {
            title = title
                .child(view::dot(colors.success, 6.0))
                .child(label("live").size(11.5).color(colors.success));
        }
        let mut field = div()
            .row()
            .w_px(280.0)
            .h_px(26.0)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .rounded(5.0)
            .border(
                1.0,
                if self.filter_focused {
                    colors.border_focused
                } else {
                    colors.border_variant
                },
            )
            .bg(colors.editor_background)
            .on_click(self.base + FILTER)
            .child(
                icon(IconKind::Search)
                    .size(12.0)
                    .color(colors.text_placeholder),
            )
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .items_center()
                    .child(self.filter.render(
                        "Filter lines",
                        self.filter_focused,
                        colors.text,
                        20.0,
                        FieldFont::Ui,
                    )),
            );
        if !self.filter.text().is_empty() {
            field = field.child(
                div()
                    .on_click(self.base + CLEAR_FILTER)
                    .child(icon(IconKind::Close).size(11.0).color(colors.icon_muted)),
            );
        }
        let toggle = |id: u64, kind: IconKind, text: String, on: bool| -> Node {
            div()
                .row()
                .h_px(26.0)
                .px(8.0)
                .gap(5.0)
                .items_center()
                .rounded(4.0)
                .on_click(self.base + id)
                .bg(if on {
                    colors.element_selected
                } else {
                    Rgba::TRANSPARENT
                })
                .child(icon(kind).size(12.0).color(if on {
                    colors.icon
                } else {
                    colors.icon_muted
                }))
                .child(label(text).size(12.5).color(if on {
                    colors.text
                } else {
                    colors.text_muted
                }))
                .into()
        };
        let pause = match self.paused_at {
            Some(at) => {
                let waiting = self.lines.len().saturating_sub(at);
                if waiting > 0 {
                    format!("Resume ({waiting} new)")
                } else {
                    "Resume".to_string()
                }
            }
            None => "Pause".to_string(),
        };
        let count = if self.filter.text().is_empty() {
            format!("{shown} lines")
        } else {
            format!("{shown} matching")
        };
        div()
            .row()
            .h_px(40.0)
            .px(14.0)
            .gap(8.0)
            .items_center()
            .child(title)
            .child(div().w_px(4.0))
            .child(field)
            .child(toggle(
                PAUSE,
                if self.paused_at.is_some() {
                    IconKind::Play
                } else {
                    IconKind::Pause
                },
                pause,
                self.paused_at.is_some(),
            ))
            .child(toggle(CLEAR, IconKind::Trash, "Clear".into(), false))
            .child(toggle(
                FOLLOW,
                IconKind::ArrowDown,
                "Follow".into(),
                self.follow,
            ))
            .child(toggle(WRAP, IconKind::Return, "Wrap".into(), self.wrap))
            .child(div().flex(1.0))
            .child(label(count).size(11.5).color(colors.text_placeholder))
            .into()
    }

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

/// One listed line: the time it came in, then its text with what the filter matched highlighted. Errors
/// and warnings keep their color.
fn log_row(line: &LogLine, query: &str, width: f32, wrap: bool) -> Node {
    let colors = theme();
    let lower = line.text.to_lowercase();
    let tint = if ["error", "exception", "fatal", "panic"]
        .iter()
        .any(|word| lower.contains(word))
    {
        colors.error
    } else if lower.contains("warn") || lower.contains("deprecat") || lower.contains("slow") {
        colors.warning
    } else {
        colors.text
    };
    let mut text = div().row().items_center();
    let mut rest = line.text.as_str();
    let mut rest_lower = lower.as_str();
    while !query.is_empty() {
        let Some(at) = rest_lower.find(query) else {
            break;
        };
        let (before, matched) = (&rest[..at], &rest[at..at + query.len()]);
        if !before.is_empty() {
            text = text.child(label(before.to_string()).size(12.5).mono().color(tint));
        }
        text = text.child(
            div().rounded(2.0).bg(colors.warning.alpha(0.3)).child(
                label(matched.to_string())
                    .size(12.5)
                    .mono()
                    .color(colors.text),
            ),
        );
        rest = &rest[at + query.len()..];
        rest_lower = &rest_lower[at + query.len()..];
    }
    let tail = label(rest.to_string()).size(12.5).mono().color(tint);
    let body: Node = if query.is_empty() {
        let whole = label(line.text.clone()).size(12.5).mono().color(tint);
        if wrap {
            whole.wrap(width).into()
        } else {
            whole.truncate().into()
        }
    } else {
        text.child(tail).into()
    };
    div()
        .row()
        .gap(0.0)
        .child(
            div()
                .row()
                .w_px(TIME_W)
                .h_px(LOG_ROW_H)
                .items_center()
                .child(
                    label(line.time.clone().unwrap_or_default())
                        .size(12.5)
                        .mono()
                        .color(colors.text_placeholder),
                ),
        )
        .child(div().row().w_px(width).items_center().child(body))
        .into()
}

/// The local wall clock as "15:41:44".
fn clock_now() -> String {
    let now: libc::time_t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as libc::time_t);
    // SAFETY: localtime_r only writes the tm we own; a zeroed tm is a valid initial value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { libc::localtime_r(&now, &mut tm) };
    if converted.is_null() {
        return String::new();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
