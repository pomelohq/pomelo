//! The new project form: repositories, who sets it up and how, then a review before creating it.

use components::{field, ButtonKind};
use pom_agent::AgentCli;
use ui::{div, label, theme, IconKind, LabelSize, Node, Space};
use workspace::text_field::FieldFont;

use crate::ids::*;
use crate::model::{home_relative, AddMode, Field, Form, Onboarding, Request, Step};
use crate::view;

pub(crate) fn render(state: &Onboarding, width: f32, hovered: Option<u64>) -> Node {
    let form = &state.form;
    let titles: Vec<&str> = Step::ALL.iter().map(|step| step.title()).collect();
    let current = Step::ALL
        .iter()
        .position(|step| *step == form.step)
        .unwrap_or(0);
    let body = match form.step {
        Step::Repositories => repositories(state, width, hovered),
        Step::Setup => setup(state, width),
        Step::Review => review(state, width),
    };
    div()
        .col()
        .w_px(width)
        .gap(Space::Px(view::PAGE_GAP))
        .child(view::steps(&titles, current, false))
        .child(body)
        .into()
}

pub(crate) fn footer(state: &Onboarding) -> Node {
    let form = &state.form;
    let mut buttons: Vec<Node> = Vec::new();
    if form.step == Step::Repositories {
        buttons.push(
            components::button(CANCEL, "Cancel")
                .size(components::ButtonSize::Medium)
                .keystroke(Some("escape"))
                .into(),
        );
    } else {
        buttons.push(
            components::button(BACK, "Back")
                .size(components::ButtonSize::Medium)
                .into(),
        );
    }
    buttons.push(Node::from(match form.step {
        Step::Review => components::button(CREATE, format!("Create {}", form.name()))
            .size(components::ButtonSize::Medium)
            .kind(ButtonKind::Primary)
            .keystroke(Some("cmd-enter"))
            .enabled(state.can_create()),
        step => components::button(NEXT, "Continue")
            .size(components::ButtonSize::Medium)
            .kind(ButtonKind::Primary)
            .keystroke(Some("enter"))
            .enabled(step_ready(state, step)),
    }));
    let note = (form.step == Step::Repositories && form.repos.is_empty())
        .then_some("Add at least one repository");
    view::footer(note, Vec::new(), buttons)
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

fn text_input(
    form: &Form,
    id: u64,
    field: Field,
    placeholder: &str,
    error: bool,
    font: FieldFont,
) -> ui::Div {
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
    view::input(
        id,
        text,
        placeholder,
        form.focus == Some(field),
        error,
        font,
    )
}

fn repositories(state: &Onboarding, width: f32, hovered: Option<u64>) -> Node {
    let form = &state.form;
    let hot = |id: u64| hovered == Some(id);
    let location = home_relative(&state.sessions_root.join(match form.name().as_str() {
        "" => "...".to_string(),
        name => name.to_string(),
    }));
    let name_error = state.name_error();
    let branch_error = state.branch_error();
    let half = (width - view::PAGE_GAP) / 2.0;
    let names = div()
        .row()
        .gap(Space::Px(view::PAGE_GAP))
        .child(
            div().col().w_px(half).child(field(
                "Session name",
                text_input(
                    form,
                    NAME,
                    Field::Name,
                    "myproject",
                    name_error.is_some(),
                    FieldFont::Ui,
                )
                .into(),
                Some(&location),
                name_error.as_deref(),
            )),
        )
        .child(
            div().col().w_px(half).child(field(
                "Default branch",
                text_input(
                    form,
                    BRANCH,
                    Field::Branch,
                    "main",
                    branch_error.is_some(),
                    FieldFont::Mono,
                )
                .into(),
                Some("main's branch in every repo"),
                branch_error.as_deref(),
            )),
        );
    let mode = components::segmented(
        [
            components::segment(MODE_FOLDERS, "Folders"),
            components::segment(MODE_URLS, "Git URLs"),
        ],
        usize::from(form.add_mode == AddMode::Urls),
    )
    .size(components::ButtonSize::Medium);
    let mut adder = div().row().items_center().gap(Space::S8).child(mode);
    let adder_hint = match form.add_mode {
        AddMode::Folders => {
            adder = adder.child(
                components::button(CHOOSE, "Choose folders...")
                    .size(components::ButtonSize::Medium)
                    .icon(Some(IconKind::FolderOpen)),
            );
            "A folder with a .git inside, or a folder of repos"
        }
        AddMode::Urls => {
            adder = adder
                .child(
                    text_input(
                        form,
                        URL,
                        Field::Url,
                        "git@github.com:acme/api.git",
                        false,
                        FieldFont::Mono,
                    )
                    .flex(1.0),
                )
                .child(
                    components::button(ADD_URL, "Add")
                        .size(components::ButtonSize::Medium)
                        .enabled(!form.url.text().trim().is_empty()),
                );
            "Paste one or several git URLs to clone"
        }
    };
    let list = if form.repos.is_empty() {
        view::panel_list(
            vec![div()
                .row()
                .h_px(36.0)
                .px(Space::S10)
                .items_center()
                .child(view::muted(
                    "No repositories yet. Add the folders you already work in, or git URLs to clone.",
                ))
                .into()],
            4.0,
        )
    } else {
        view::panel_list(
            (0..form.repos.len())
                .map(|index| repo_row(form, index, hot(REMOVE_BASE + index as u64)))
                .collect(),
            4.0,
        )
    };
    let mut column = div()
        .col()
        .gap(Space::Px(view::PAGE_GAP))
        .child(view::head(
            "New project",
            Some("Repos in, a runnable environment for every branch out"),
        ))
        .child(names)
        .child(field("Repositories", adder.into(), Some(adder_hint), None))
        .child(list);
    let infra = form.infra();
    if !infra.is_empty() {
        let mut chips = div().row().gap(Space::S6).items_center();
        for name in &infra {
            chips = chips.child(view::chip(name, true));
        }
        column = column.child(field(
            "Shared services found in compose files",
            chips.into(),
            Some("One container of each serves every workspace"),
            None,
        ));
    }
    column.into()
}

fn repo_row(form: &Form, index: usize, remove_hot: bool) -> Node {
    let repo = &form.repos[index];
    let mut chips: Vec<Node> = Vec::new();
    let stack: Vec<String> = match &repo.scan {
        Some(scan) => {
            chips.extend(scan.stack.iter().map(|stack| view::chip(stack, true)));
            if let Some(compose) = &scan.compose {
                chips.push(view::chip(compose, true));
            }
            if scan.env_files > 0 {
                chips.push(view::chip(
                    &match scan.env_files {
                        1 => "1 .env file".to_string(),
                        count => format!("{count} .env files"),
                    },
                    true,
                ));
            }
            scan.stack.clone()
        }
        None => {
            chips.push(view::chip("detected after cloning", false));
            Vec::new()
        }
    };
    let placeholder = repo
        .scan
        .as_ref()
        .map(|scan| scan.name.clone())
        .unwrap_or_else(|| crate::model::repo_name(&repo.source));
    let alias_id = ALIAS_BASE + index as u64;
    // The name is the repo's alias: it reads as plain text and turns into an input when clicked.
    let name: Node = if form.focus == Some(Field::Alias(index)) {
        div()
            .row()
            .child(
                text_input(
                    form,
                    alias_id,
                    Field::Alias(index),
                    &placeholder,
                    false,
                    FieldFont::Mono,
                )
                .w_px(180.0)
                .h_px(22.0),
            )
            .into()
    } else {
        let shown = form.alias_of(repo);
        div()
            .row()
            .on_click(alias_id)
            .child(view::strong_line(if shown.is_empty() {
                &placeholder
            } else {
                &shown
            }))
            .into()
    };
    let mut chip_row = div().row().gap(Space::S6).items_center();
    for chip in chips {
        chip_row = chip_row.child(chip);
    }
    view::repo_row(
        view::repo_icon(&stack, repo.remote()),
        name,
        &home_relative(std::path::Path::new(&repo.source)),
        vec![
            chip_row.into(),
            view::icon_button(REMOVE_BASE + index as u64, IconKind::Close, remove_hot),
        ],
    )
}

fn setup(state: &Onboarding, width: f32) -> Node {
    let colors = theme();
    let form = &state.form;
    let any_agent = state.any_agent();
    let agent_on = form.with_agent && any_agent;
    let text_w = width - 28.0 - 14.0 - 12.0;
    let choice = |id: u64, on: bool, title: Node, text: &str, extra: Option<Node>| -> Node {
        let mut body = div()
            .col()
            .flex(1.0)
            .gap(Space::S8)
            .child(title)
            .child(view::note_paragraph(text, text_w));
        if let Some(extra) = extra {
            body = body.child(extra);
        }
        view::card_frame(on)
            .row()
            .gap(Space::S12)
            .p(Space::S14)
            .on_click(id)
            .child(div().col().pt(Space::S4).child(components::radio(on)))
            .child(body)
            .into()
    };
    let agent_title: Node = div()
        .row()
        .h_px(20.0)
        .items_center()
        .gap(Space::S8)
        .child(
            label("Set up with an agent CLI")
                .size(LabelSize::Medium)
                .medium()
                .color(if any_agent {
                    colors.text
                } else {
                    colors.text_muted
                }),
        )
        .child(components::pill("recommended").color(colors.success))
        .into();
    let picker: Node = if any_agent {
        let options = AgentCli::ALL.iter().enumerate().map(|(index, cli)| {
            components::segment(AGENT_BASE + index as u64, cli.title())
                .enabled(state.agent_installed(*cli))
        });
        let selected = AgentCli::ALL
            .iter()
            .position(|cli| *cli == form.agent)
            .unwrap_or(0);
        field(
            "Agent",
            components::segmented(options, selected)
                .size(components::ButtonSize::Medium)
                .into(),
            Some("only installed CLIs can be picked"),
            None,
        )
    } else {
        view::muted("Install Claude Code, Codex or Gemini CLI to set up with an agent").into()
    };
    let infra = form.infra();
    let shared_text = if infra.is_empty() {
        "Whatever pom.yml declares, in one Docker project".to_string()
    } else {
        format!("{} in one Docker project", infra.join(", "))
    };
    let secrets_text = format!(
        "{}. Values stay encrypted on this machine; the agent only sees their names.",
        match form.env_files() {
            1 => "1 file".to_string(),
            count => format!("{count} files"),
        }
    );
    let option = |id: u64, on: bool, text: &str, detail: Node| -> Node {
        div()
            .col()
            .gap(Space::S4)
            .child(components::checkbox(id, on, text))
            .child(div().col().pl(Space::S24).child(detail))
            .into()
    };
    let branch_error = state.workspace_branch_error();
    let mut branch_row = div().row().items_center().gap(Space::S8).child(
        text_input(
            form,
            WORKSPACE_BRANCH,
            Field::WorkspaceBranch,
            "feat-login",
            branch_error.is_some(),
            FieldFont::Mono,
        )
        .w_px(220.0),
    );
    if let Some(error) = branch_error {
        branch_row = branch_row.child(label(error).size(LabelSize::Small).color(colors.error));
    }
    let options = div()
        .col()
        .gap(Space::S12)
        .child(option(
            OPT_SECRETS,
            form.import_secrets,
            "Import gitignored .env values as secrets",
            view::muted(&secrets_text).into(),
        ))
        .child(option(
            OPT_SHARED,
            form.start_shared,
            "Start the shared services when done",
            view::muted(&shared_text).into(),
        ))
        .child(option(
            OPT_WORKSPACE,
            form.first_workspace,
            "Create a first workspace",
            if form.first_workspace {
                branch_row.into()
            } else {
                view::muted("A branch of every repo, set up right after").into()
            },
        ));
    div()
        .col()
        .gap(Space::Px(view::PAGE_GAP))
        .child(view::head("Who writes pom.yml", None))
        .child(choice(
            SETUP_AGENT,
            agent_on,
            agent_title,
            "The agent reads every repo, writes a complete pom.yml (services, setup, migrations, env wiring), then Pomelo checks it installs and boots, and hands problems back to the agent until it is clean. It runs as a CLI in the agent dock: watch it, type to it, or stop it and finish yourself.",
            Some(picker),
        ))
        .child(choice(
            SETUP_MANUAL,
            !agent_on,
            view::strong_line("Set up manually"),
            "Pomelo drafts pom.yml from what it detected (0 tokens) and opens it with the config doctor's findings. You finish it.",
            None,
        ))
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
    let value_w = width - 8.0 - 20.0 - 120.0 - 12.0;
    let rows = items
        .into_iter()
        .map(|(key, value)| {
            div()
                .row()
                .gap(Space::S12)
                .px(Space::S10)
                .py(Space::S8)
                .child(
                    div().w_px(120.0).child(components::text_line(
                        20.0,
                        label(key)
                            .size(LabelSize::Medium)
                            .color(colors.text_placeholder),
                    )),
                )
                .child(Node::from(
                    components::paragraph(value, value_w)
                        .size(LabelSize::Medium, 20.0)
                        .color(colors.text),
                ))
                .into()
        })
        .collect();
    div()
        .col()
        .gap(Space::Px(view::PAGE_GAP))
        .child(view::head(
            &format!("Create {}", form.name()),
            Some("Nothing in your repos changes: Pomelo works in its own worktrees under the session folder"),
        ))
        .child(view::panel_list(rows, 4.0))
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
