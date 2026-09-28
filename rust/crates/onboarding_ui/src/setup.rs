//! A project being set up, phase by phase, and the summary once it is ready.

use onboarding::{CheckKind, CheckStatus, Segment};
use ui::{div, icon, label, theme, IconKind, Node, Rgba};

use crate::ids::*;
use crate::model::{Onboarding, Phase, Request, Run};
use crate::view::{self, StepMark, Tone};

const LOG_LINES: usize = 8;

pub(crate) fn progress(state: &Onboarding, width: f32, hovered: Option<u64>) -> Node {
    let Some(run) = state.run.as_ref() else {
        return div().into();
    };
    let hot = |id: u64| hovered == Some(id);
    let mut right = Vec::new();
    if run.agent_active() && run.error.is_none() {
        right.push(agent_cli_button(run, hot(AGENT_CLI)));
    }
    if run.error.is_none() {
        right.push(view::button(
            PAUSE,
            None,
            if run.paused { "Resume" } else { "Pause" },
            Tone::Plain,
            hot(PAUSE),
            true,
            None,
        ));
    }
    right.push(view::button(
        STOP,
        None,
        "Cancel",
        Tone::Ghost,
        hot(STOP),
        true,
        None,
    ));
    let sub = format!(
        "{} - {} so far",
        match run.repos.len() {
            1 => "1 repository".to_string(),
            count => format!("{count} repositories"),
        },
        view::seconds_text(run.started.elapsed().as_secs())
    );
    let percent = (run.progress() * 100.0).round() as u32;
    let bar = div()
        .row()
        .items_center()
        .gap(10.0)
        .child(view::progress_bar(run.progress(), None))
        .child(
            label(format!("{percent}%"))
                .size(view::HINT)
                .color(theme().text_placeholder),
        );
    let shown: Vec<Phase> = Phase::ALL
        .into_iter()
        .filter(|phase| *phase != Phase::Repair || run.finding.is_some() || run.repaired)
        .collect();
    let mut timeline = div().col();
    for (index, phase) in shown.iter().enumerate() {
        let last = index + 1 == shown.len();
        timeline = timeline.child(phase_row(run, *phase, last, width, hovered));
    }
    div()
        .col()
        .w_px(width)
        .gap(view::PAGE_GAP)
        .child(view::header(
            &format!("Setting up {}", run.name),
            &sub,
            right,
        ))
        .child(bar)
        .child(timeline)
        .into()
}

fn agent_cli_button(run: &Run, hot: bool) -> Node {
    view::button(
        AGENT_CLI,
        Some(IconKind::Sparkle),
        if run.agent_shown {
            "Hide agent CLI"
        } else {
            "Show agent CLI"
        },
        Tone::Plain,
        hot,
        true,
        None,
    )
}

fn phase_title(run: &Run, phase: Phase) -> String {
    match phase {
        Phase::Clone => "Clone repositories".into(),
        Phase::Scan => "Scan".into(),
        Phase::Configure if run.with_agent => format!("Configure with {}", run.agent.title()),
        Phase::Configure => "Draft pom.yml".into(),
        Phase::Verify => "Verify".into(),
        Phase::Repair => "Repair".into(),
    }
}

fn phase_row(run: &Run, phase: Phase, last: bool, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let reached = run.phase_at[phase.index()].is_some();
    let current = run.phase == phase && run.finished.is_none();
    let failed = (phase == Phase::Clone && run.error.is_some())
        || (phase == Phase::Repair && current && run.finding.is_some());
    let mark = if failed {
        StepMark::Failed
    } else if current {
        StepMark::Working
    } else if reached {
        StepMark::Done
    } else {
        StepMark::Todo(phase.index() + 1)
    };
    let mut rail = div()
        .col()
        .w_px(20.0)
        .items_center()
        .child(view::step_mark(mark, 20.0));
    if !last {
        rail = rail.child(div().w_px(1.0).flex(1.0).bg(colors.border_variant));
    }
    let mut title =
        div()
            .row()
            .items_center()
            .gap(8.0)
            .child(label(phase_title(run, phase)).size(14.0).color(if reached {
                colors.text
            } else {
                colors.text_muted
            }));
    if phase == Phase::Scan {
        title = title.child(view::chip("0 tokens", false));
    }
    if phase == Phase::Repair && run.repaired && run.finding.is_none() {
        title = title.child(label("fixed").size(view::HINT).color(colors.success));
    }
    title = title.child(div().flex(1.0));
    if let Some(seconds) = run.phase_seconds(phase) {
        title = title.child(view::hint(&view::seconds_text(seconds)));
    }
    let detail_w = width - 32.0;
    let mut body = div().col().flex(1.0).pb(16.0).child(title);
    if reached {
        let detail = match phase {
            Phase::Clone => clone_detail(run, hovered),
            Phase::Scan => scan_detail(run, detail_w),
            Phase::Configure => configure_detail(run, hovered),
            Phase::Verify => verify_detail(run, detail_w),
            Phase::Repair => repair_detail(run, hovered),
        };
        if let Some(detail) = detail {
            body = body.child(div().col().pt(8.0).gap(6.0).child(detail));
        }
    }
    div().row().gap(12.0).child(rail).child(body).into()
}

fn clone_detail(run: &Run, hovered: Option<u64>) -> Option<Node> {
    let colors = theme();
    let mut column = div().col().gap(6.0);
    for (repo, clone) in run.repos.iter().zip(&run.clone) {
        let status = match clone.linked {
            Some(true) => "linked".to_string(),
            Some(false) => "cloned".to_string(),
            None => format!("{}%", clone.percent),
        };
        let value = if clone.linked.is_some() {
            1.0
        } else {
            f32::from(clone.percent) / 100.0
        };
        column = column.child(
            div()
                .row()
                .items_center()
                .gap(10.0)
                .child(
                    div().w_px(150.0).child(
                        label(repo.clone())
                            .size(view::TEXT)
                            .mono()
                            .color(colors.text)
                            .truncate(),
                    ),
                )
                .child(view::progress_bar(value, None))
                .child(
                    div()
                        .row()
                        .w_px(90.0)
                        .justify_end()
                        .child(view::hint(&status)),
                ),
        );
    }
    if let Some(error) = &run.error {
        column = column
            .child(finding_box(
                false,
                vec![
                    label("The project was not created")
                        .size(view::TEXT)
                        .color(colors.error)
                        .into(),
                    label(error.clone())
                        .size(view::HINT)
                        .mono()
                        .color(colors.text)
                        .into(),
                ],
            ))
            .child(div().row().child(view::button(
                BACK_TO_FORM,
                None,
                "Back to the form",
                Tone::Plain,
                hovered == Some(BACK_TO_FORM),
                true,
                None,
            )));
    }
    Some(column.into())
}

fn scan_detail(run: &Run, width: f32) -> Option<Node> {
    if run.scans.is_empty() {
        return Some(view::hint("Reading each repo...").into());
    }
    let colors = theme();
    let rows = run
        .scans
        .iter()
        .map(|scan| {
            let name = if scan.alias.is_empty() {
                &scan.name
            } else {
                &scan.alias
            };
            let mut chips: Vec<Node> = scan
                .stack
                .iter()
                .map(|stack| view::chip(stack, true))
                .collect();
            chips.extend(scan.infra.iter().map(|infra| view::chip(infra, false)));
            if chips.is_empty() {
                chips.push(view::chip("nothing detected", false));
            }
            view::row()
                .child(
                    div().w_px(150.0).child(
                        label(name.clone())
                            .size(view::TEXT)
                            .mono()
                            .color(colors.text)
                            .truncate(),
                    ),
                )
                .child(
                    div()
                        .col()
                        .flex(1.0)
                        .child(view::pack(chips, width - 180.0, 5.0)),
                )
                .child(
                    label(format!("{} env", scan.env_files))
                        .size(view::HINT)
                        .color(colors.text_muted),
                )
                .into()
        })
        .collect();
    Some(view::rows(rows))
}

fn log_box(lines: &[Vec<Segment>], empty: &str) -> Node {
    let colors = theme();
    let mut column = div()
        .col()
        .gap(3.0)
        .px(10.0)
        .py(8.0)
        .rounded(6.0)
        .bg(Rgba::new(0.0, 0.0, 0.0, 0.2));
    if lines.is_empty() {
        return column
            .child(
                label(empty.to_string())
                    .size(12.0)
                    .mono()
                    .color(colors.text_muted),
            )
            .into();
    }
    for line in &lines[lines.len().saturating_sub(LOG_LINES)..] {
        let mut row = div().row().items_center();
        for segment in line {
            row = row.child(
                label(segment.text.clone())
                    .size(12.0)
                    .mono()
                    .weight(if segment.strong { 500 } else { 400 })
                    .color(if segment.strong {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .truncate(),
            );
        }
        column = column.child(row);
    }
    column.into()
}

fn configure_detail(run: &Run, hovered: Option<u64>) -> Option<Node> {
    let colors = theme();
    let hot = |id: u64| hovered == Some(id);
    let open_config = view::button(
        OPEN_CONFIG,
        None,
        "Open pom.yml",
        Tone::Plain,
        hot(OPEN_CONFIG),
        true,
        None,
    );
    if !run.agent_active() {
        let text = format!(
            "{}pom.yml was drafted from what the scan found (services, setup commands, shared services, env files). Verify checks the draft; what it cannot fix, you finish in pom.yml.",
            if run.agent_skipped {
                "The agent was stopped. "
            } else {
                ""
            }
        );
        let mut buttons = div().row().gap(6.0).child(open_config);
        if run.agent_skipped {
            buttons = buttons.child(view::button(
                RESUME_AGENT,
                Some(IconKind::Sparkle),
                &format!("Let {} finish it after all", run.agent.title()),
                Tone::Agent,
                hot(RESUME_AGENT),
                true,
                None,
            ));
        }
        let note = div()
            .col()
            .p(10.0)
            .rounded(7.0)
            .border(1.0, colors.border_variant)
            .child(
                label(text)
                    .size(view::SMALL)
                    .color(colors.text_muted)
                    .wrap(640.0),
            );
        return Some(div().col().gap(6.0).child(note).child(buttons).into());
    }
    let reading = format!("{} is reading the repos...", run.agent.title());
    let mut buttons = div()
        .row()
        .items_center()
        .gap(6.0)
        .child(agent_cli_button(run, hot(AGENT_CLI)))
        .child(open_config);
    if run.agent_configuring() {
        buttons = buttons.child(div().flex(1.0));
        if run.agent != pom_agent::AgentCli::Claude {
            buttons = buttons.child(view::button(
                VERIFY_NOW,
                None,
                "It is done - verify",
                Tone::Plain,
                hot(VERIFY_NOW),
                true,
                None,
            ));
        }
        buttons = buttons.child(view::button(
            SKIP_AGENT,
            None,
            "Skip the agent - finish manually",
            Tone::Ghost,
            hot(SKIP_AGENT),
            true,
            None,
        ));
    }
    Some(
        div()
            .col()
            .gap(6.0)
            .child(log_box(&run.summary, &reading))
            .child(buttons)
            .into(),
    )
}

fn verify_detail(run: &Run, width: f32) -> Option<Node> {
    let colors = theme();
    if run.checks.is_empty() {
        return Some(view::hint("Starting...").into());
    }
    let rows = run
        .checks
        .iter()
        .map(|check| {
            let (kind, color) = match check.status {
                CheckStatus::Passed => (IconKind::Check, colors.success),
                CheckStatus::Failed => (IconKind::Close, colors.error),
                CheckStatus::Running => (IconKind::RotateCw, colors.text_accent),
            };
            view::row()
                .child(div().w_px(14.0).child(icon(kind).size(12.0).color(color)))
                .child(
                    div().w_px(170.0).child(
                        label(check.kind.title())
                            .size(view::TEXT)
                            .color(colors.text)
                            .truncate(),
                    ),
                )
                .child(
                    div().row().flex(1.0).child(
                        label(check.detail.clone())
                            .size(12.0)
                            .mono()
                            .color(if check.status == CheckStatus::Failed {
                                colors.error
                            } else {
                                colors.text_muted
                            })
                            .truncate(),
                    ),
                )
                .w_px(width)
                .into()
        })
        .collect();
    Some(view::rows(rows))
}

fn finding_box(fixing: bool, children: Vec<Node>) -> Node {
    let colors = theme();
    let (border, bg) = if fixing {
        (
            view::agent_tint().alpha(0.5),
            view::agent_tint().alpha(0.07),
        )
    } else {
        (colors.error.alpha(0.35), colors.error.alpha(0.07))
    };
    let mut column = div()
        .col()
        .gap(7.0)
        .p(10.0)
        .rounded(7.0)
        .border(1.0, border)
        .bg(bg);
    for child in children {
        column = column.child(child);
    }
    column.into()
}

fn repair_detail(run: &Run, hovered: Option<u64>) -> Option<Node> {
    let colors = theme();
    let finding = run.finding.as_ref()?;
    let hot = |id: u64| hovered == Some(id);
    let check = &finding.check;
    let headline = match &check.kind {
        CheckKind::Doctor => "The config doctor found errors".to_string(),
        CheckKind::Install { repo } => format!("{repo} did not install"),
        CheckKind::Boot { repo, service } if repo.is_empty() => format!("{service} did not boot"),
        CheckKind::Boot { repo, service } => format!("{repo} > {service} did not boot"),
    };
    let tail: Vec<&str> = check
        .output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != check.detail)
        .collect();
    let tail = tail[tail.len().saturating_sub(3)..].join("\n");
    let mut children: Vec<Node> = Vec::new();
    if finding.fixing {
        children.push(
            div()
                .row()
                .items_center()
                .gap(6.0)
                .child(
                    icon(IconKind::RotateCw)
                        .size(12.0)
                        .color(view::agent_tint()),
                )
                .child(
                    label(format!("{} is fixing it...", run.agent.title()))
                        .size(view::TEXT)
                        .color(view::agent_tint()),
                )
                .into(),
        );
    } else {
        children.push(label(headline).size(view::TEXT).color(colors.error).into());
    }
    children.push(
        label(check.detail.clone())
            .size(12.0)
            .mono()
            .color(colors.text)
            .wrap(620.0)
            .into(),
    );
    if finding.fixing {
        children.push(
            view::hint(
                "It reads the output and changes pom.yml; verify runs again when it is done",
            )
            .into(),
        );
        children.push(
            div()
                .row()
                .child(view::button(
                    CANCEL_FIX,
                    None,
                    "Cancel",
                    Tone::Plain,
                    hot(CANCEL_FIX),
                    true,
                    None,
                ))
                .into(),
        );
    } else {
        if !tail.is_empty() {
            children.push(view::hint(&tail).mono().wrap(620.0).into());
        }
        let mut buttons = div().row().gap(6.0);
        if run.agent_active() {
            buttons = buttons.child(view::button(
                FIX_AGENT,
                Some(IconKind::Sparkle),
                &format!("Fix with {}", run.agent.title()),
                Tone::Agent,
                hot(FIX_AGENT),
                true,
                None,
            ));
        }
        buttons = buttons.child(view::button(
            FIX_MANUAL,
            None,
            "Fix manually",
            Tone::Plain,
            hot(FIX_MANUAL) || finding.manual,
            true,
            None,
        ));
        buttons = buttons.child(view::button(
            RETRY,
            None,
            "Verify again",
            Tone::Plain,
            hot(RETRY),
            true,
            None,
        ));
        if matches!(check.kind, CheckKind::Boot { .. }) {
            buttons = buttons.child(view::button(
                SKIP_SERVICE,
                None,
                "Skip this service",
                Tone::Ghost,
                hot(SKIP_SERVICE),
                true,
                None,
            ));
        }
        children.push(buttons.into());
        if finding.manual {
            let mut manual = div()
                .row()
                .items_center()
                .gap(6.0)
                .child(view::hint("Fix it yourself:"))
                .child(view::button(
                    OPEN_CONFIG,
                    Some(IconKind::File),
                    "Open pom.yml",
                    Tone::Plain,
                    hot(OPEN_CONFIG),
                    true,
                    None,
                ));
            if let Some(repo) = finding_repo(&check.kind) {
                manual = manual.child(view::button(
                    OPEN_TERMINAL,
                    Some(IconKind::Terminal),
                    &format!("Open a terminal in {repo}"),
                    Tone::Plain,
                    hot(OPEN_TERMINAL),
                    true,
                    None,
                ));
            }
            children.push(manual.child(view::hint("then Verify again")).into());
        }
    }
    Some(finding_box(finding.fixing, children))
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
    let hot = |id: u64| hovered == Some(id);
    let workspace_title = match &run.first_workspace {
        Some(branch) => format!("Open {branch}"),
        None => "Create a workspace".to_string(),
    };
    let primary = view::button(
        OPEN_WORKSPACE,
        None,
        &workspace_title,
        Tone::Primary,
        hot(OPEN_WORKSPACE),
        true,
        None,
    );
    let stat_w = (width - 30.0) / 4.0;
    let stat = |value: usize, text: &str| -> Node {
        div()
            .col()
            .w_px(stat_w)
            .p(12.0)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .child(
                label(value.to_string())
                    .size(22.0)
                    .weight(600)
                    .color(colors.text),
            )
            .child(
                label(text.to_string())
                    .size(view::HINT)
                    .color(colors.text_muted),
            )
            .into()
    };
    let stats = div()
        .row()
        .gap(10.0)
        .child(stat(run.counts.repos, "repositories"))
        .child(stat(run.counts.services, "services"))
        .child(stat(run.counts.shared, "shared services"))
        .child(stat(run.counts.databases, "databases"));
    let rows: Vec<Node> = run
        .set_up
        .iter()
        .map(|(alias, stack, services)| {
            view::row()
                .child(
                    div().w_px(150.0).child(
                        label(alias.clone())
                            .size(view::TEXT)
                            .mono()
                            .color(colors.text)
                            .truncate(),
                    ),
                )
                .child(div().col().flex(1.0).child(view::pack(
                    stack.iter().map(|stack| view::chip(stack, true)).collect(),
                    width / 2.0,
                    5.0,
                )))
                .child(
                    label(services.clone())
                        .size(view::HINT)
                        .color(colors.text_muted),
                )
                .into()
        })
        .collect();
    let card_w = (width - 20.0) / 3.0;
    let cards = div()
        .row()
        .gap(10.0)
        .child(view::card(
            OPEN_WORKSPACE + 1000,
            IconKind::Branch,
            &workspace_title,
            "A branch of every repo with its own ports and databases, copied from main.",
            card_w,
            hot(OPEN_WORKSPACE + 1000),
        ))
        .child(view::card(
            START_MAIN,
            IconKind::Package,
            "Start main's services",
            "Run main as it is, to compare against your branches.",
            card_w,
            hot(START_MAIN),
        ))
        .child(view::card(
            OPEN_CONFIG,
            IconKind::Folder,
            "Open pom.yml",
            if run.agent_active() {
                "What the agent wrote, with the config doctor's notes."
            } else {
                "The drafted config, with the config doctor's notes."
            },
            card_w,
            hot(OPEN_CONFIG),
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
    let mut column = div()
        .col()
        .w_px(width)
        .gap(view::PAGE_GAP)
        .child(view::header(
            &format!("{} is ready", run.name),
            &match run.skipped.len() {
                0 => "Every repo installs and boots from main".to_string(),
                1 => "Installs and boots from main; 1 service was skipped".to_string(),
                count => format!("Installs and boots from main; {count} services were skipped"),
            },
            vec![primary],
        ))
        .child(stats);
    if !rows.is_empty() {
        column = column.child(view::section(
            "What was set up",
            None,
            vec![view::rows(rows)],
        ));
    }
    column
        .child(view::section("Next", None, vec![cards.into()]))
        .child(view::hint(&format!(
            "{secrets}Took {}.",
            view::seconds_text(took)
        )))
        .into()
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
