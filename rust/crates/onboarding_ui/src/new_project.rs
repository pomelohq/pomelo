//! The new project form: repositories, who sets it up and how, then a review before creating it.

use pom_agent::AgentCli;
use ui::{div, icon, label, theme, IconKind, Node};
use workspace::text_field::FieldFont;

use crate::ids::*;
use crate::model::{home_relative, AddMode, Field, Form, Onboarding, Request, Step};
use crate::view::{self, Tone};

pub(crate) fn render(state: &Onboarding, width: f32, hovered: Option<u64>) -> Node {
    let form = &state.form;
    let hot = |id: u64| hovered == Some(id);
    let cancel = view::button(
        CANCEL,
        None,
        "Cancel",
        Tone::Ghost,
        hot(CANCEL),
        true,
        Some("esc"),
    );
    let body = match form.step {
        Step::Repositories => repositories(state, width, hovered),
        Step::Setup => setup(state, width, hovered),
        Step::Review => review(state, width),
    };
    let mut footer = div().row().items_center().gap(8.0);
    if form.step != Step::Repositories {
        footer = footer.child(view::button(
            BACK,
            None,
            "Back",
            Tone::Plain,
            hot(BACK),
            true,
            None,
        ));
    }
    footer = footer.child(div().flex(1.0));
    if form.step == Step::Repositories && form.repos.is_empty() {
        footer = footer.child(view::hint("Add at least one repository"));
    }
    footer = footer.child(match form.step {
        Step::Review => view::button(
            CREATE,
            None,
            &format!("Create {}", form.name()),
            Tone::Primary,
            hot(CREATE),
            state.can_create(),
            Some("cmd-enter"),
        ),
        step => view::button(
            NEXT,
            None,
            "Continue",
            Tone::Primary,
            hot(NEXT),
            step_ready(state, step),
            Some("enter"),
        ),
    });
    div()
        .col()
        .w_px(width)
        .gap(view::PAGE_GAP)
        .child(view::header(
            "New project",
            "Repos in, a runnable environment for every branch out",
            vec![cancel],
        ))
        .child(stepper(form.step))
        .child(body)
        .child(view::divider())
        .child(footer)
        .into()
}

fn step_ready(state: &Onboarding, step: Step) -> bool {
    match step {
        Step::Repositories => state.repos_ready(),
        Step::Setup => {
            state.workspace_branch_error().is_none()
                && (!state.form.with_agent || state.agent_installed(state.form.agent))
        }
        Step::Review => state.can_create(),
    }
}

fn stepper(current: Step) -> Node {
    let colors = theme();
    let mut row = div().row().items_center().gap(8.0);
    for (index, step) in Step::ALL.iter().enumerate() {
        if index > 0 {
            row = row.child(div().w_px(30.0).h_px(1.0).bg(colors.border));
        }
        let number = step.number();
        let (mark, color) = if number < current.number() {
            (view::StepMark::Done, colors.text_muted)
        } else if *step == current {
            (view::StepMark::Current(number), colors.text)
        } else {
            (view::StepMark::Todo(number), colors.text_placeholder)
        };
        row = row.child(
            div()
                .row()
                .items_center()
                .gap(7.0)
                .on_click(STEP_BASE + index as u64)
                .child(view::step_mark(mark, 20.0))
                .child(label(step.title()).size(view::TEXT).color(color)),
        );
    }
    row.into()
}

fn labeled(title: &str, field: Node, note: Node) -> ui::Div {
    div()
        .col()
        .gap(5.0)
        .child(
            label(title.to_string())
                .size(view::HINT)
                .color(theme().text_muted),
        )
        .child(field)
        .child(note)
}

fn note(text: &str, error: Option<String>, mono: bool) -> Node {
    match error {
        Some(error) => label(error).size(view::HINT).color(theme().error).into(),
        None if mono => view::hint(text).mono().into(),
        None => view::hint(text).into(),
    }
}

fn text_input(
    form: &Form,
    id: u64,
    field: Field,
    placeholder: &str,
    width: Option<f32>,
    height: f32,
    font: FieldFont,
) -> Node {
    let focused = form.focus == Some(field);
    let text = match field {
        Field::Name => &form.name,
        Field::Branch => &form.branch,
        Field::Url => &form.url,
        Field::WorkspaceBranch => &form.workspace_branch,
        Field::Alias(index) => match form.repos.get(index) {
            Some(repo) => &repo.alias,
            None => &form.name,
        },
    };
    let size = if font == FieldFont::Mono { 12.5 } else { 13.0 };
    let content = div().row().flex(1.0).items_center().child(text.render(
        placeholder,
        focused,
        theme().text,
        size * 1.4,
        font,
    ));
    view::input(id, content.into(), focused, width, height)
}

fn repositories(state: &Onboarding, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let form = &state.form;
    let hot = |id: u64| hovered == Some(id);
    let location = home_relative(&state.sessions_root.join(match form.name().as_str() {
        "" => "...".to_string(),
        name => name.to_string(),
    }));
    let names = div()
        .row()
        .gap(14.0)
        .child(
            labeled(
                "Session name",
                text_input(
                    form,
                    NAME,
                    Field::Name,
                    "myproject",
                    None,
                    30.0,
                    FieldFont::Ui,
                ),
                note(&location, state.name_error(), true),
            )
            .flex(1.0),
        )
        .child(
            labeled(
                "Default branch",
                text_input(
                    form,
                    BRANCH,
                    Field::Branch,
                    "main",
                    Some(200.0),
                    30.0,
                    FieldFont::Mono,
                ),
                note("main's branch in every repo", state.branch_error(), false),
            )
            .w_px(200.0),
        );
    let mode = view::segmented(
        &[
            (MODE_FOLDERS, "Folders", true),
            (MODE_URLS, "Git URLs", true),
        ],
        usize::from(form.add_mode == AddMode::Urls),
    );
    let mut adder = div().row().items_center().gap(8.0).child(mode);
    adder = match form.add_mode {
        AddMode::Folders => adder
            .child(view::button(
                CHOOSE,
                Some(IconKind::Folder),
                "Choose folders...",
                Tone::Plain,
                hot(CHOOSE),
                true,
                None,
            ))
            .child(view::hint(
                "A folder with a .git inside, or a folder of repos",
            )),
        AddMode::Urls => adder
            .child(text_input(
                form,
                URL,
                Field::Url,
                "git@github.com:acme/api.git - paste several at once",
                None,
                30.0,
                FieldFont::Mono,
            ))
            .child(view::button(
                ADD_URL,
                None,
                "Add",
                Tone::Plain,
                hot(ADD_URL),
                !form.url.text().trim().is_empty(),
                None,
            )),
    };
    let list = if form.repos.is_empty() {
        view::rows(vec![view::row()
            .child(view::hint(
                "No repositories yet. Add the folders you already work in, or git URLs to clone.",
            ))
            .into()])
    } else {
        view::rows(
            form.repos
                .iter()
                .enumerate()
                .map(|(index, _)| repo_row(form, index, width, hot(REMOVE_BASE + index as u64)))
                .collect(),
        )
    };
    let count = label(format!("{} added", form.repos.len()))
        .size(view::HINT)
        .color(colors.text_placeholder);
    let mut column = div()
        .col()
        .gap(view::PAGE_GAP)
        .child(names)
        .child(view::section(
            "Repositories",
            Some(count.into()),
            vec![adder.into(), list],
        ));
    let infra = form.infra();
    if !infra.is_empty() {
        let mut chips: Vec<Node> = infra
            .iter()
            .map(|name| {
                div()
                    .row()
                    .h_px(20.0)
                    .px(7.0)
                    .gap(4.0)
                    .items_center()
                    .rounded(4.0)
                    .border(1.0, colors.border_variant)
                    .child(view::dot(colors.text_accent, 8.0))
                    .child(label(name.clone()).size(11.5).color(colors.text))
                    .into()
            })
            .collect();
        chips.push(view::hint("One container of each serves every workspace").into());
        column = column.child(view::section(
            "Shared services found in compose files",
            None,
            vec![view::pack(chips, width, 6.0)],
        ));
    }
    column.into()
}

fn repo_row(form: &Form, index: usize, width: f32, remove_hot: bool) -> Node {
    let colors = theme();
    let repo = &form.repos[index];
    let mut chips: Vec<Node> = Vec::new();
    match &repo.scan {
        Some(scan) => {
            chips.extend(scan.stack.iter().map(|stack| view::chip(stack, true)));
            if let Some(compose) = &scan.compose {
                chips.push(view::chip(compose, false));
            }
            chips.push(view::chip(
                &match scan.env_files {
                    1 => "1 .env file".to_string(),
                    count => format!("{count} .env files"),
                },
                false,
            ));
        }
        None => chips.push(view::chip("detected after cloning", false)),
    }
    let text_w = width - 24.0 - 14.0 - 110.0 - 24.0 - 30.0;
    let source = div()
        .col()
        .flex(1.0)
        .gap(5.0)
        .child(
            label(home_relative(std::path::Path::new(&repo.source)))
                .size(12.0)
                .mono()
                .color(colors.text_muted)
                .truncate_start(),
        )
        .child(view::pack(chips, text_w, 5.0));
    let placeholder = repo
        .scan
        .as_ref()
        .map(|scan| scan.name.clone())
        .unwrap_or_else(|| crate::model::repo_name(&repo.source));
    let remove = div()
        .row()
        .w_px(20.0)
        .h_px(20.0)
        .rounded(3.0)
        .items_center()
        .justify_center()
        .bg(if remove_hot {
            colors.element_hover
        } else {
            ui::Rgba::TRANSPARENT
        })
        .on_click(REMOVE_BASE + index as u64)
        .child(
            icon(IconKind::Close)
                .size(11.0)
                .color(colors.text_placeholder),
        );
    view::row()
        .child(
            icon(if repo.remote() {
                IconKind::Branch
            } else {
                IconKind::Folder
            })
            .size(14.0)
            .color(colors.text_placeholder),
        )
        .child(text_input(
            form,
            ALIAS_BASE + index as u64,
            Field::Alias(index),
            &placeholder,
            Some(110.0),
            24.0,
            FieldFont::Mono,
        ))
        .child(source)
        .child(remove)
        .into()
}

fn setup(state: &Onboarding, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let form = &state.form;
    let any_agent = state.any_agent();
    let agent_on = form.with_agent && any_agent;
    let inner_w = width - 28.0 - 28.0;
    let choice =
        |id: u64, on: bool, kind: IconKind, title: Node, text: &str, extra: Option<Node>| {
            let mut body = div()
                .col()
                .flex(1.0)
                .gap(3.0)
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(8.0)
                        .child(icon(kind).size(14.0).color(colors.icon_muted))
                        .child(title),
                )
                .child(
                    label(text.to_string())
                        .size(view::SMALL)
                        .color(colors.text_muted)
                        .wrap(inner_w),
                );
            if let Some(extra) = extra {
                body = body.child(div().pt(6.0).child(extra));
            }
            let frame = div()
                .row()
                .gap(12.0)
                .p(14.0)
                .rounded(8.0)
                .border(
                    1.0,
                    if on {
                        colors.info_border
                    } else {
                        colors.border_variant
                    },
                )
                .on_click(id)
                .child(div().col().pt(2.0).child(view::radio(on)))
                .child(body);
            let frame = if on {
                frame.bg(colors.text_accent.alpha(0.06))
            } else if hovered == Some(id) {
                frame.bg(colors.ghost_element_hover)
            } else {
                frame
            };
            Node::from(frame)
        };
    let agent_title: Node = div()
        .row()
        .items_center()
        .gap(8.0)
        .child(
            label("Set up with an agent CLI")
                .size(14.0)
                .color(if any_agent {
                    colors.text
                } else {
                    colors.text_muted
                }),
        )
        .child(
            div()
                .row()
                .px(6.0)
                .rounded(4.0)
                .border(1.0, colors.success.alpha(0.45))
                .child(label("recommended").size(11.0).color(colors.success)),
        )
        .into();
    let picker: Node = if any_agent {
        let options: Vec<(u64, &str, bool)> = AgentCli::ALL
            .iter()
            .enumerate()
            .map(|(index, cli)| {
                (
                    AGENT_BASE + index as u64,
                    cli.title(),
                    state.agent_installed(*cli),
                )
            })
            .collect();
        let selected = AgentCli::ALL
            .iter()
            .position(|cli| *cli == form.agent)
            .unwrap_or(0);
        div()
            .row()
            .items_center()
            .gap(8.0)
            .child(
                label("Agent")
                    .size(view::HINT)
                    .color(colors.text_placeholder),
            )
            .child(view::segmented(&options, selected))
            .child(view::hint("only installed CLIs can be picked"))
            .into()
    } else {
        view::hint("Install Claude Code, Codex or Gemini CLI to set up with an agent").into()
    };
    let who = view::section(
        "Who writes pom.yml",
        None,
        vec![
            choice(
                SETUP_AGENT,
                agent_on,
                IconKind::Sparkle,
                agent_title,
                "The agent reads every repo, writes a complete pom.yml (services, setup, migrations, env wiring), then Pomelo checks it installs and boots, and hands problems back to the agent until it is clean. It runs as a CLI in the agent dock: watch it, type to it, or stop it and finish yourself.",
                Some(picker),
            ),
            choice(
                SETUP_MANUAL,
                !agent_on,
                IconKind::Folder,
                label("Set up manually")
                    .size(14.0)
                    .color(colors.text)
                    .into(),
                "Pomelo drafts pom.yml from what it detected (0 tokens) and opens it with the config doctor's findings. You finish it.",
                None,
            ),
        ],
    );
    let infra = form.infra();
    let shared_text = if infra.is_empty() {
        "Whatever pom.yml declares, in one Docker project".to_string()
    } else {
        format!("{} in one Docker project", infra.join(", "))
    };
    let branch_row: Node = div()
        .row()
        .items_center()
        .gap(8.0)
        .child(view::hint("branch"))
        .child(text_input(
            form,
            WORKSPACE_BRANCH,
            Field::WorkspaceBranch,
            "feat-login",
            Some(200.0),
            24.0,
            FieldFont::Mono,
        ))
        .child(match state.workspace_branch_error() {
            Some(error) => label(error).size(view::HINT).color(colors.error).into(),
            None => Node::from(div()),
        })
        .into();
    let options = view::section(
        "Options",
        None,
        vec![
            view::check(
                OPT_SECRETS,
                form.import_secrets,
                "Import gitignored .env values as secrets",
                Some(
                    view::hint(&format!(
                        "{}. Values stay encrypted on this Mac; the agent only sees their names.",
                        match form.env_files() {
                            1 => "1 file".to_string(),
                            count => format!("{count} files"),
                        }
                    ))
                    .into(),
                ),
            ),
            view::check(
                OPT_SHARED,
                form.start_shared,
                "Start the shared services when done",
                Some(view::hint(&shared_text).into()),
            ),
            view::check(
                OPT_WORKSPACE,
                form.first_workspace,
                "Create a first workspace",
                Some(branch_row),
            ),
        ],
    );
    div()
        .col()
        .gap(view::PAGE_GAP)
        .child(who)
        .child(options)
        .into()
}

fn review(state: &Onboarding, width: f32) -> Node {
    let colors = theme();
    let form = &state.form;
    let aliases: Vec<String> = form.repos.iter().map(|repo| form.alias_of(repo)).collect();
    let mut after = Vec::new();
    if form.start_shared {
        after.push("start shared services".to_string());
    }
    if form.first_workspace {
        after.push(format!("create workspace {}", form.workspace_branch()));
    }
    let items = [
        (
            "Create",
            format!(
                "{}, main on {}",
                home_relative(&state.sessions_root.join(form.name())),
                form.default_branch()
            ),
        ),
        ("Clone or link", aliases.join(", ")),
        (
            "Detect",
            "stacks, processes, compose services and env files (no tokens)".to_string(),
        ),
        (
            "Configure",
            if form.with_agent && state.any_agent() {
                format!(
                    "{} writes pom.yml and fixes what the checks find",
                    form.agent.title()
                )
            } else {
                "Pomelo drafts pom.yml for you to finish".to_string()
            },
        ),
        (
            "Secrets",
            if form.import_secrets {
                format!(
                    "{} imported, encrypted",
                    match form.env_files() {
                        1 => "1 .env file".to_string(),
                        count => format!("{count} .env files"),
                    }
                )
            } else {
                "not imported".to_string()
            },
        ),
        (
            "Afterwards",
            if after.is_empty() {
                "nothing else".to_string()
            } else {
                after.join(", ")
            },
        ),
    ];
    let rows = items
        .into_iter()
        .map(|(key, value)| {
            view::row()
                .child(
                    div()
                        .w_px(120.0)
                        .child(label(key).size(view::TEXT).color(colors.text_placeholder)),
                )
                .child(
                    label(value)
                        .size(view::TEXT)
                        .color(colors.text)
                        .wrap(width - 170.0),
                )
                .into()
        })
        .collect();
    div()
        .col()
        .gap(10.0)
        .child(view::rows(rows))
        .child(view::hint(
            "Nothing in your repos changes: Pomelo works in its own worktrees under the session folder.",
        ))
        .into()
}

pub(crate) fn click(state: &mut Onboarding, id: u64) {
    state.form.focus = match id {
        NAME => Some(Field::Name),
        BRANCH => Some(Field::Branch),
        URL => Some(Field::Url),
        WORKSPACE_BRANCH => Some(Field::WorkspaceBranch),
        id if (ALIAS_BASE..ALIAS_BASE + ROW_LIMIT).contains(&id) => {
            Some(Field::Alias((id - ALIAS_BASE) as usize))
        }
        _ => None,
    };
    match id {
        CANCEL => state.requests.push(Request::Close),
        BACK => {
            state.form.step = match state.form.step {
                Step::Review => Step::Setup,
                _ => Step::Repositories,
            }
        }
        NEXT => enter_next(state),
        CREATE if state.can_create() => state.requests.push(Request::Create),
        CHOOSE => state.requests.push(Request::ChooseFolders),
        ADD_URL => {
            state.add_urls();
        }
        MODE_FOLDERS => state.form.add_mode = AddMode::Folders,
        MODE_URLS => {
            state.form.add_mode = AddMode::Urls;
            state.form.focus = Some(Field::Url);
        }
        SETUP_AGENT if state.any_agent() => state.form.with_agent = true,
        SETUP_MANUAL => state.form.with_agent = false,
        OPT_SECRETS => state.form.import_secrets = !state.form.import_secrets,
        OPT_SHARED => state.form.start_shared = !state.form.start_shared,
        OPT_WORKSPACE => state.form.first_workspace = !state.form.first_workspace,
        id if (STEP_BASE..STEP_BASE + 3).contains(&id) => {
            let wanted = Step::ALL[(id - STEP_BASE) as usize];
            if wanted.number() < state.form.step.number() || state.repos_ready() {
                state.form.step = wanted;
            }
        }
        id if (AGENT_BASE..AGENT_BASE + AgentCli::ALL.len() as u64).contains(&id) => {
            let cli = AgentCli::ALL[(id - AGENT_BASE) as usize];
            if state.agent_installed(cli) {
                state.form.agent = cli;
                state.form.with_agent = true;
            }
        }
        id if (REMOVE_BASE..REMOVE_BASE + ROW_LIMIT).contains(&id) => {
            let index = (id - REMOVE_BASE) as usize;
            if index < state.form.repos.len() {
                state.form.repos.remove(index);
            }
        }
        _ => {}
    }
}

fn enter_next(state: &mut Onboarding) {
    let step = state.form.step;
    if !step_ready(state, step) {
        return;
    }
    state.form.step = match step {
        Step::Repositories => Step::Setup,
        _ => Step::Review,
    };
    state.form.focus = None;
}

/// Enter: adds the typed URLs in the URL field, else goes on (or creates, on the last step).
pub(crate) fn enter(state: &mut Onboarding) {
    if state.form.focus == Some(Field::Url) && !state.form.url.text().trim().is_empty() {
        state.add_urls();
        return;
    }
    match state.form.step {
        Step::Review => click(state, CREATE),
        _ => enter_next(state),
    }
}

pub(crate) fn cycle_focus(form: &mut Form, back: bool) {
    let mut order = Vec::new();
    match form.step {
        Step::Repositories => {
            order.extend([Field::Name, Field::Branch]);
            order.extend((0..form.repos.len()).map(Field::Alias));
            if form.add_mode == AddMode::Urls {
                order.push(Field::Url);
            }
        }
        Step::Setup if form.first_workspace => order.push(Field::WorkspaceBranch),
        _ => {}
    }
    if order.is_empty() {
        return;
    }
    let at = form
        .focus
        .and_then(|focus| order.iter().position(|field| *field == focus));
    let next = match (at, back) {
        (None, _) => 0,
        (Some(at), false) => (at + 1) % order.len(),
        (Some(at), true) => (at + order.len() - 1) % order.len(),
    };
    form.focus = Some(order[next]);
}
