//! A project being set up, phase by phase, and the summary once it is ready.

use components::{text_line, ButtonKind};
use onboarding::{CheckKind, CheckStatus};
use ui::{div, icon, label, theme, IconKind, LabelSize, Node};

use crate::ids::*;
use crate::model::{Onboarding, Phase, Request, Run};
use crate::view;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PhaseState {
    Done,
    Running,
    Pending,
    Failed,
}

pub(crate) fn progress(state: &Onboarding, width: f32, _hovered: Option<u64>) -> Node {
    let Some(run) = state.run.as_ref() else {
        return div().into();
    };
    let shown: Vec<Phase> = Phase::ALL
        .into_iter()
        .filter(|phase| *phase != Phase::Repair || run.finding.is_some() || run.repaired)
        .collect();
    let rows = shown
        .iter()
        .map(|phase| {
            let (state, note) = phase_state(run, *phase);
            phase_row(&phase_title(run, *phase), state, note.as_deref())
        })
        .collect();
    let mut column = div()
        .col()
        .w_px(width)
        .gap(view::PAGE_GAP)
        .child(view::head(
            &format!("Setting up {}", run.name),
            Some("Clone, scan, configure, then verify until every repo installs and boots."),
        ))
        .child(view::panel_list(rows, 6.0));
    if let Some(error) = &run.error {
        column = column.child(problem_card(
            "The project was not created",
            error,
            None,
            vec![components::button(BACK_TO_FORM, "Back to the form")
                .size(components::ButtonSize::Medium)
                .into()],
            width,
        ));
    } else if let Some(card) = finding_card(run, width) {
        column = column.child(card);
    }
    column.into()
}

pub(crate) fn progress_footer(state: &Onboarding) -> Node {
    let Some(run) = state.run.as_ref() else {
        return div().into();
    };
    let mut leading = Vec::new();
    let mut buttons = Vec::new();
    if run.error.is_none() {
        if run.agent_configuring() {
            if run.agent != pom_agent::AgentCli::Claude {
                leading.push(
                    components::button(VERIFY_NOW, "It is done - verify")
                        .size(components::ButtonSize::Medium)
                        .kind(ButtonKind::Ghost)
                        .into(),
                );
            }
            leading.push(
                components::button(SKIP_AGENT, "Skip the agent")
                    .size(components::ButtonSize::Medium)
                    .kind(ButtonKind::Ghost)
                    .into(),
            );
        }
        if run.agent_skipped {
            leading.push(
                components::button(RESUME_AGENT, format!("Let {} finish it", run.agent.title()))
                    .size(components::ButtonSize::Medium)
                    .kind(ButtonKind::Ghost)
                    .into(),
            );
        }
        if run.agent_active() {
            buttons.push(
                components::button(
                    AGENT_CLI,
                    if run.agent_shown {
                        "Hide agent CLI"
                    } else {
                        "Show agent CLI"
                    },
                )
                .size(components::ButtonSize::Medium)
                .kind(ButtonKind::Ghost)
                .into(),
            );
        }
        buttons.push(
            components::button(PAUSE, if run.paused { "Resume" } else { "Pause" })
                .size(components::ButtonSize::Medium)
                .kind(ButtonKind::Ghost)
                .into(),
        );
    }
    buttons.push(
        components::button(STOP, "Cancel")
            .size(components::ButtonSize::Medium)
            .into(),
    );
    view::footer(None, leading, buttons)
}

fn phase_title(run: &Run, phase: Phase) -> String {
    match phase {
        Phase::Clone => "Clone repositories".into(),
        Phase::Scan => "Scan".into(),
        Phase::Configure if run.with_agent && !run.agent_skipped => {
            format!("Configure with {}", run.agent.title())
        }
        Phase::Configure => "Draft pom.yml".into(),
        Phase::Verify => "Verify".into(),
        Phase::Repair => "Repair".into(),
    }
}

fn names(list: &[&str]) -> String {
    match list {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn check_headline(kind: &CheckKind) -> String {
    match kind {
        CheckKind::Doctor => "the config doctor found errors".to_string(),
        CheckKind::Install { repo } => format!("{repo} did not install"),
        CheckKind::Boot { repo, service } if repo.is_empty() => format!("{service} did not boot"),
        CheckKind::Boot { repo, service } => format!("{repo} > {service} did not boot"),
    }
}

/// How far a phase got, and the one line that says what it found or is doing.
fn phase_state(run: &Run, phase: Phase) -> (PhaseState, Option<String>) {
    let reached = run.phase_at[phase.index()].is_some();
    let current = run.phase == phase && run.finished.is_none();
    match phase {
        Phase::Clone => {
            if let Some(error) = &run.error {
                let first = error.lines().next().unwrap_or_default().to_string();
                return (PhaseState::Failed, Some(first));
            }
            let mut cloned = Vec::new();
            let mut linked = Vec::new();
            let mut done = 0;
            for (repo, clone) in run.repos.iter().zip(&run.clone) {
                match clone.linked {
                    Some(true) => linked.push(repo.as_str()),
                    Some(false) => cloned.push(repo.as_str()),
                    None => {}
                }
                if clone.linked.is_some() {
                    done += 1;
                }
            }
            if current {
                let percent = (run.progress() * 100.0).round() as u32;
                return (
                    PhaseState::Running,
                    Some(format!("{done} of {} - {percent}%", run.repos.len())),
                );
            }
            let mut parts = Vec::new();
            if !cloned.is_empty() {
                parts.push(format!("{} cloned", names(&cloned)));
            }
            if !linked.is_empty() {
                parts.push(format!("{} linked", names(&linked)));
            }
            (PhaseState::Done, Some(parts.join(", ")))
        }
        _ if !reached => (PhaseState::Pending, None),
        Phase::Scan if current => (PhaseState::Running, Some("reading each repo".into())),
        Phase::Scan => {
            let mut shared: Vec<&str> = run
                .scans
                .iter()
                .flat_map(|scan| scan.infra.iter().map(String::as_str))
                .collect();
            shared.sort_unstable();
            shared.dedup();
            let repos = match run.scans.len() {
                1 => "1 repo".to_string(),
                count => format!("{count} repos"),
            };
            let shared = match shared.len() {
                1 => "1 shared service".to_string(),
                count => format!("{count} shared services"),
            };
            (PhaseState::Done, Some(format!("{repos}, {shared}")))
        }
        Phase::Configure if current => {
            let latest = run.summary.last().map(|line| {
                line.iter()
                    .map(|segment| segment.text.as_str())
                    .collect::<String>()
            });
            let note = latest.unwrap_or_else(|| {
                if run.agent_active() {
                    format!("{} is reading the repos", run.agent.title())
                } else {
                    "drafting from what the scan found".to_string()
                }
            });
            (PhaseState::Running, Some(note))
        }
        Phase::Configure if run.agent_skipped => (
            PhaseState::Done,
            Some("drafted from the scan; you finish it".into()),
        ),
        Phase::Configure => (PhaseState::Done, None),
        Phase::Verify => {
            let failed = run
                .checks
                .iter()
                .find(|check| check.status == CheckStatus::Failed);
            if let Some(check) = failed {
                return (PhaseState::Failed, Some(check_headline(&check.kind)));
            }
            if current {
                let passed = run
                    .checks
                    .iter()
                    .filter(|check| check.status == CheckStatus::Passed)
                    .count();
                return (
                    PhaseState::Running,
                    Some(format!("{passed} of {} checks passed", run.checks.len())),
                );
            }
            (
                PhaseState::Done,
                Some("every repo installs and boots".into()),
            )
        }
        Phase::Repair => match &run.finding {
            Some(finding) if finding.fixing => (
                PhaseState::Running,
                Some(format!("{} is fixing it", run.agent.title())),
            ),
            Some(_) => (PhaseState::Pending, Some("waiting for you".into())),
            None => (PhaseState::Done, Some("fixed".into())),
        },
    }
}

fn phase_row(title: &str, state: PhaseState, note: Option<&str>) -> Node {
    let colors = theme();
    let (kind, tint, ink) = match state {
        PhaseState::Done => (IconKind::Check, colors.success, colors.text_muted),
        PhaseState::Running => (IconKind::LoadCircle, colors.text_accent, colors.text),
        PhaseState::Pending => (
            IconKind::Circle,
            colors.text_placeholder,
            colors.text_placeholder,
        ),
        PhaseState::Failed => (IconKind::XCircle, colors.error, colors.error),
    };
    let mut row = div()
        .row()
        .h_px(34.0)
        .px(8.0)
        .gap(10.0)
        .items_center()
        .rounded(4.0)
        .child(icon(kind).size(14.0).color(tint))
        .child(
            div().row().flex(1.0).items_center().child(
                label(title.to_string())
                    .size(LabelSize::Medium)
                    .color(ink)
                    .truncate(),
            ),
        );
    if state == PhaseState::Running {
        row = row.bg(colors.element_selected);
    }
    if let Some(note) = note.filter(|note| !note.is_empty()) {
        row = row.child(view::muted(note).truncate());
    }
    row.into()
}

/// A raised card about what went wrong: what failed, its output, and what to do about it.
fn problem_card(
    title: &str,
    output: &str,
    hint: Option<&str>,
    buttons: Vec<Node>,
    width: f32,
) -> Node {
    let colors = theme();
    let mut log = div()
        .col()
        .px(12.0)
        .py(10.0)
        .rounded(4.0)
        .bg(colors.editor_background);
    for line in output.lines() {
        log = log.child(text_line(
            16.0,
            label(line.to_string())
                .size(LabelSize::Small)
                .mono()
                .color(colors.text_muted)
                .truncate(),
        ));
    }
    let mut column = components::card()
        .render()
        .gap(10.0)
        .child(
            div()
                .row()
                .h_px(20.0)
                .items_center()
                .gap(8.0)
                .child(icon(IconKind::BadgeAlert).size(16.0).color(colors.error))
                .child(view::strong_line(title)),
        )
        .child(log);
    if let Some(hint) = hint {
        column = column.child(view::note_paragraph(hint, width - 28.0));
    }
    let mut row = div().row().items_center().gap(8.0);
    for button in buttons {
        row = row.child(button);
    }
    column.child(row).into()
}

fn finding_card(run: &Run, width: f32) -> Option<Node> {
    let finding = run.finding.as_ref()?;
    let check = &finding.check;
    let mut lines: Vec<&str> = check
        .output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if !check.detail.is_empty() && !lines.contains(&check.detail.as_str()) {
        lines.push(&check.detail);
    }
    let output = lines[lines.len().saturating_sub(4)..].join("\n");
    let title = match check.kind {
        CheckKind::Doctor => "The config doctor found errors".to_string(),
        _ => check_headline(&check.kind),
    };
    let mut buttons = Vec::new();
    if finding.fixing {
        buttons.push(
            components::button(CANCEL_FIX, "Cancel")
                .size(components::ButtonSize::Medium)
                .into(),
        );
    } else {
        if run.agent_active() {
            buttons.push(
                components::button(FIX_AGENT, format!("Fix with {}", run.agent.title()))
                    .size(components::ButtonSize::Medium)
                    .kind(ButtonKind::Agent)
                    .icon(Some(IconKind::Sparkle))
                    .into(),
            );
        }
        buttons.push(
            components::button(FIX_MANUAL, "Fix manually")
                .size(components::ButtonSize::Medium)
                .into(),
        );
        buttons.push(
            components::button(RETRY, "Verify again")
                .size(components::ButtonSize::Medium)
                .kind(ButtonKind::Ghost)
                .into(),
        );
        if matches!(check.kind, CheckKind::Boot { .. }) {
            buttons.push(
                components::button(SKIP_SERVICE, "Skip this service")
                    .size(components::ButtonSize::Medium)
                    .kind(ButtonKind::Ghost)
                    .into(),
            );
        }
    }
    let hint = if finding.fixing {
        Some("It reads the output and changes pom.yml; verify runs again when it is done")
    } else if finding.manual {
        None
    } else {
        Some("Fix it here or let the agent change pom.yml, then verify again")
    };
    let card = problem_card(&title, &output, hint, buttons, width);
    if !finding.manual || finding.fixing {
        return Some(card);
    }
    let mut manual = div()
        .row()
        .items_center()
        .gap(8.0)
        .child(view::muted("Fix it yourself:"))
        .child(
            components::button(OPEN_CONFIG, "Open pom.yml")
                .size(components::ButtonSize::Medium)
                .icon(Some(IconKind::File)),
        );
    if let Some(repo) = finding_repo(&check.kind) {
        manual = manual.child(
            components::button(OPEN_TERMINAL, format!("Open a terminal in {repo}"))
                .size(components::ButtonSize::Medium)
                .icon(Some(IconKind::Terminal)),
        );
    }
    Some(
        div()
            .col()
            .gap(10.0)
            .child(card)
            .child(manual.child(view::muted("then Verify again")))
            .into(),
    )
}

fn finding_repo(kind: &CheckKind) -> Option<&str> {
    match kind {
        CheckKind::Install { repo } | CheckKind::Boot { repo, .. } if !repo.is_empty() => {
            Some(repo)
        }
        _ => None,
    }
}

pub(crate) fn done(state: &Onboarding, width: f32, hovered: Option<u64>) -> Node {
    let Some(run) = state.run.as_ref() else {
        return div().into();
    };
    let colors = theme();
    let titles: Vec<&str> = crate::model::Step::ALL
        .iter()
        .map(|step| step.title())
        .collect();
    let workspace_title = workspace_title(run);
    let stat_w = (width - 3.0 * 12.0) / 4.0;
    let stat = |value: usize, text: &str| -> Node {
        view::card_frame(false)
            .col()
            .w_px(stat_w)
            .gap(2.0)
            .px(14.0)
            .py(12.0)
            .child(text_line(
                28.0,
                label(value.to_string())
                    .size_px(20.0)
                    .weight(600)
                    .color(colors.text),
            ))
            .child(text_line(16.0, view::muted(text)))
            .into()
    };
    let stats = div()
        .row()
        .gap(12.0)
        .child(stat(run.counts.repos, "repositories"))
        .child(stat(run.counts.services, "services"))
        .child(stat(run.counts.shared, "shared services"))
        .child(stat(run.counts.databases, "databases"));
    let rows: Vec<Node> = run
        .set_up
        .iter()
        .map(|(alias, stack, services)| {
            view::repo_row(
                view::repo_icon(stack, false),
                view::strong_line(alias),
                &stack.join(", "),
                vec![view::muted(services).into()],
            )
        })
        .collect();
    let card_w = (width - 2.0 * 12.0) / 3.0;
    let card = |id: u64, lead: Node, title: &str, text: &str, primary: bool| -> Node {
        let mut frame = view::card_frame(primary)
            .col()
            .w_px(card_w)
            .gap(6.0)
            .p(14.0)
            .on_click(id)
            .child(lead)
            .child(view::strong_line(title))
            .child(view::note_paragraph(text, card_w - 28.0));
        if hovered == Some(id) && !primary {
            frame = frame.bg(colors.element_hover);
        }
        frame.into()
    };
    let cards = div()
        .row()
        .gap(12.0)
        .child(card(
            OPEN_WORKSPACE + 1000,
            icon(IconKind::Branch)
                .size(16.0)
                .color(colors.text_accent)
                .into(),
            &workspace_title,
            "A branch of every repo with its own ports and databases, copied from main.",
            true,
        ))
        .child(card(
            START_MAIN,
            icon(IconKind::Play)
                .size(16.0)
                .color(colors.text_muted)
                .into(),
            "Start main's services",
            "Run main as it is, to compare against your branches.",
            false,
        ))
        .child(card(
            OPEN_CONFIG,
            ui::material_icon(ui::MaterialIcon::Yaml).size(16.0).into(),
            "Open pom.yml",
            if run.agent_active() {
                "What the agent wrote, with the config doctor's notes."
            } else {
                "The drafted config, with the config doctor's notes."
            },
            false,
        ));
    let took = run
        .finished
        .map(|finished| finished.saturating_duration_since(run.started).as_secs())
        .unwrap_or_default();
    let secrets = match run.secrets_imported {
        0 => String::new(),
        1 => "1 .env file became secrets. ".to_string(),
        count => format!("{count} .env files became secrets. "),
    };
    let sub = match run.skipped.len() {
        0 => "Every repo installs and boots from main".to_string(),
        1 => "Installs and boots from main; 1 service was skipped".to_string(),
        count => format!("Installs and boots from main; {count} services were skipped"),
    };
    let mut column = div()
        .col()
        .w_px(width)
        .gap(view::PAGE_GAP)
        .child(view::steps(&titles, titles.len(), true))
        .child(view::head(&format!("{} is ready", run.name), Some(&sub)))
        .child(stats);
    if !rows.is_empty() {
        column = column.child(components::field(
            "What was set up",
            view::panel_list(rows, 4.0),
            None,
            None,
        ));
    }
    column
        .child(components::field("Next", cards.into(), None, None))
        .child(text_line(
            16.0,
            view::muted(&format!("{secrets}Took {}.", view::seconds_text(took))),
        ))
        .into()
}

fn workspace_title(run: &Run) -> String {
    match &run.first_workspace {
        Some(branch) => format!("Open {branch}"),
        None => "Create a workspace".to_string(),
    }
}

pub(crate) fn done_footer(state: &Onboarding) -> Node {
    let Some(run) = state.run.as_ref() else {
        return div().into();
    };
    view::footer(
        None,
        Vec::new(),
        vec![components::button(OPEN_WORKSPACE, workspace_title(run))
            .size(components::ButtonSize::Medium)
            .kind(ButtonKind::Primary)
            .keystroke(Some("enter"))
            .into()],
    )
}

pub(crate) fn click(state: &mut Onboarding, id: u64) {
    let request = match id {
        PAUSE => Request::TogglePause,
        STOP => Request::Cancel,
        AGENT_CLI => Request::ToggleAgentCli,
        OPEN_CONFIG => Request::OpenConfig,
        SKIP_AGENT => Request::SkipAgent,
        RESUME_AGENT => Request::ResumeAgent,
        VERIFY_NOW => Request::ConfigureDone,
        FIX_AGENT => Request::FixWithAgent,
        CANCEL_FIX => Request::CancelFix,
        FIX_MANUAL => {
            if let Some(finding) = state.run.as_mut().and_then(|run| run.finding.as_mut()) {
                finding.manual = !finding.manual;
            }
            return;
        }
        OPEN_TERMINAL => {
            let repo = state
                .run
                .as_ref()
                .and_then(|run| run.finding.as_ref())
                .and_then(|finding| finding_repo(&finding.check.kind).map(str::to_string));
            match repo {
                Some(repo) => Request::OpenTerminal { repo },
                None => return,
            }
        }
        SKIP_SERVICE => Request::SkipService,
        RETRY => Request::RetryVerify,
        START_MAIN => Request::StartMain,
        CLOSE => Request::Close,
        BACK_TO_FORM => {
            state.back_to_form();
            return;
        }
        id if id == OPEN_WORKSPACE || id == OPEN_WORKSPACE + 1000 => Request::OpenWorkspace,
        _ => return,
    };
    state.requests.push(request);
}
