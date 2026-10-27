use pom_db::{ConnectError, ConnectErrorKind, Engine};
use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Radius, Rgba, Space};

const PAD: Space = Space::S10;
const GAP: Space = Space::S6;
const FACT_LABEL_W: f32 = 62.0;
const LINE_H: f32 = 24.0;

/// A database that could not be opened, with what is known about fixing it.
pub(crate) struct Failure {
    pub error: ConnectError,
    /// The repo's key in pom.yml; empty for shared Redis.
    pub repo: String,
    /// Main has no database of this name either, so there is nothing to copy from it.
    pub main_absent: bool,
    /// Main's copy of this database, when the server has it.
    pub main_copy: Option<String>,
    pub raw_open: bool,
    /// What is running for it right now ("Creating database...").
    pub busy: Option<String>,
    pub action_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureAction {
    CreateDatabase,
    CopyFromMain,
    StartShared,
    Retry,
    EditConfig,
    ToggleRaw,
    CopyError,
    FixWithAgent,
    More,
}

pub(crate) const ACTION_STRIDE: u64 = 16;

const ALL: [FailureAction; 9] = [
    FailureAction::CreateDatabase,
    FailureAction::CopyFromMain,
    FailureAction::StartShared,
    FailureAction::Retry,
    FailureAction::EditConfig,
    FailureAction::ToggleRaw,
    FailureAction::CopyError,
    FailureAction::FixWithAgent,
    FailureAction::More,
];

impl FailureAction {
    pub fn offset(self) -> u64 {
        ALL.iter().position(|action| *action == self).unwrap_or(0) as u64
    }

    pub fn from_offset(offset: u64) -> Option<FailureAction> {
        ALL.get(offset as usize).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            FailureAction::CreateDatabase => "Create database",
            FailureAction::CopyFromMain => "Copy from main",
            FailureAction::StartShared => "Start shared services",
            FailureAction::Retry => "Retry",
            FailureAction::EditConfig => "Edit pom.yml",
            FailureAction::ToggleRaw => "Details",
            FailureAction::CopyError => "Copy Error",
            FailureAction::FixWithAgent => "Fix with Claude",
            FailureAction::More => "More Fixes",
        }
    }
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Postgres => "Postgres",
        other => other.title(),
    }
}

impl Failure {
    pub fn new(error: ConnectError, repo: String) -> Failure {
        Failure {
            error,
            repo,
            main_absent: false,
            main_copy: None,
            raw_open: false,
            busy: None,
            action_error: None,
        }
    }

    pub fn is_warning(&self) -> bool {
        self.error.kind == ConnectErrorKind::WrongServer
    }

    pub fn title(&self) -> String {
        let error = &self.error;
        let engine = engine_name(error.engine);
        match error.kind {
            ConnectErrorKind::DatabaseMissing => {
                format!("Database {} does not exist", error.database)
            }
            ConnectErrorKind::ServerUnreachable => {
                format!("Can't reach {engine} at {}", error.server())
            }
            ConnectErrorKind::AuthFailed => format!("{engine} rejected user {}", error.user),
            ConnectErrorKind::WrongServer => format!(
                "Port {} answers, but it is not this project's {engine}",
                error.port
            ),
            ConnectErrorKind::Other => format!("Could not open {}", error.database),
        }
    }

    /// The cause in a few words, for the line under the database row, which already names the database.
    pub fn headline(&self) -> String {
        let error = &self.error;
        let engine = engine_name(error.engine);
        match error.kind {
            ConnectErrorKind::DatabaseMissing => "Database does not exist".to_string(),
            ConnectErrorKind::ServerUnreachable => {
                format!("{engine} not answering on :{}", error.port)
            }
            ConnectErrorKind::AuthFailed => format!("{engine} rejected user {}", error.user),
            ConnectErrorKind::WrongServer => {
                format!("Port {} is another project's {engine}", error.port)
            }
            ConnectErrorKind::Other => format!("{engine} refused to open it"),
        }
    }

    /// What the menu beside the primary fix offers: the other fixes, then the agent and the error.
    pub fn more_actions(&self) -> Vec<FailureAction> {
        let mut actions: Vec<FailureAction> = self.actions().into_iter().skip(1).collect();
        actions.extend([FailureAction::FixWithAgent, FailureAction::CopyError]);
        actions
    }

    pub fn why(&self) -> String {
        let error = &self.error;
        let engine = engine_name(error.engine);
        match error.kind {
            ConnectErrorKind::DatabaseMissing => {
                let who = if self.repo.is_empty() {
                    "Services using it".to_string()
                } else {
                    format!("Services of {}", self.repo)
                };
                format!(
                    "{engine} is running and answered, but this workspace's database was never created \
                     (or was dropped). {who} fail the same way until it exists."
                )
            }
            ConnectErrorKind::ServerUnreachable => format!(
                "Nothing answers on that port: the shared {engine} container is stopped, or Docker is not \
                 running."
            ),
            ConnectErrorKind::AuthFailed => "The server is up but the login does not match. It comes from \
                 shared_services in pom.yml (db_user / db_password), which may differ from what the container \
                 was created with."
                .to_string(),
            ConnectErrorKind::WrongServer => format!(
                "Another container ({}) listens where this project's {engine} should be, so the panel talks \
                 to the wrong server. Stop it or give this project another port, then retry.",
                error.container.as_deref().unwrap_or("unknown")
            ),
            ConnectErrorKind::Other => {
                format!("{engine} answered with an error; the full error says why.")
            }
        }
    }

    /// The fixes this kind offers, primary first (the error, copy and agent controls always follow).
    pub fn actions(&self) -> Vec<FailureAction> {
        match self.error.kind {
            ConnectErrorKind::DatabaseMissing => {
                let mut actions = vec![FailureAction::CreateDatabase];
                if self.main_copy.is_some() {
                    actions.push(FailureAction::CopyFromMain);
                }
                actions
            }
            ConnectErrorKind::ServerUnreachable => {
                vec![FailureAction::StartShared, FailureAction::Retry]
            }
            ConnectErrorKind::AuthFailed => vec![FailureAction::EditConfig, FailureAction::Retry],
            ConnectErrorKind::WrongServer => vec![FailureAction::Retry, FailureAction::EditConfig],
            ConnectErrorKind::Other => vec![FailureAction::Retry],
        }
    }

    /// Everything a person or an agent needs to act on it, as text.
    pub fn report(&self) -> String {
        let error = &self.error;
        let mut text = format!(
            "{}\n\nserver: {}\ndatabase: {}\n",
            self.title(),
            error.server(),
            error.database
        );
        if !error.user.is_empty() {
            text.push_str(&format!("user: {}\n", error.user));
        }
        if let Some(container) = &error.container {
            text.push_str(&format!("container: {container}\n"));
        }
        text.push_str(&format!("\n{}", error.raw));
        text
    }

    pub fn agent_prompt(&self) -> String {
        format!(
            "The Pomelo Database panel could not open a database:\n\n{}\n\nFind the cause and fix it. If the \
             fix belongs in the project's pom.yml, say what to change. Do not stop or remove containers of \
             other projects. Tell me when to press Retry in the Database panel.",
            self.report()
        )
    }

    fn facts(&self) -> Vec<(&'static str, String)> {
        let error = &self.error;
        let mut facts = vec![
            ("server", error.server()),
            ("database", error.database.clone()),
        ];
        if !error.user.is_empty() {
            facts.push(("user", error.user.clone()));
        }
        if let Some(container) = &error.container {
            facts.push(("container", container.clone()));
        }
        facts
    }

    /// The line under the database row, `width` wide, and its details while open; `id` gives each control its
    /// click id.
    pub fn render(
        &self,
        width: f32,
        id: impl Fn(FailureAction) -> u64,
        hover: Option<u64>,
    ) -> Node {
        let colors = theme();
        let tone = if self.is_warning() {
            colors.warning
        } else {
            colors.error
        };
        let toggle_id = id(FailureAction::ToggleRaw);
        let toggle_hot = hover == Some(toggle_id);
        let mut cause = div()
            .row()
            .flex(1.0)
            .h_px(LINE_H)
            .gap(Space::S6)
            .px(Space::S4)
            .rounded(Radius::Small)
            .items_center()
            .on_click(toggle_id)
            .child(icon(IconKind::Warning).size(12.0).color(tone))
            .child(
                div().row().flex(1.0).child(
                    label(self.headline())
                        .size(LabelSize::Small)
                        .color(tone)
                        .truncate(),
                ),
            )
            .child(
                icon(if self.raw_open {
                    IconKind::ChevronDown
                } else {
                    IconKind::ChevronRight
                })
                .size(10.0)
                .color(if toggle_hot {
                    colors.icon
                } else {
                    colors.icon_muted
                }),
            );
        if toggle_hot {
            cause = components::with_tooltip(
                cause.bg(colors.ghost_element_hover),
                if self.raw_open {
                    "Hide details"
                } else {
                    "Why, and the full error"
                },
                true,
            );
        }
        // Under the cause, lined up with its text: the panel is too narrow for the cause and a fix on one line.
        let mut fixes = div()
            .row()
            .w_px(width)
            .h_px(LINE_H)
            .pl(Space::Px(4.0 + 12.0 + 6.0))
            .gap(Space::S4)
            .items_center();
        match &self.busy {
            Some(busy) => {
                fixes = fixes
                    .child(icon(IconKind::RotateCw).size(11.0).color(colors.icon_muted))
                    .child(
                        label(busy.clone())
                            .size(LabelSize::Small)
                            .color(colors.text_muted),
                    );
            }
            None => {
                let primary = self
                    .actions()
                    .first()
                    .copied()
                    .unwrap_or(FailureAction::Retry);
                let split =
                    components::split_button(id(primary), id(FailureAction::More), primary.label())
                        .primary()
                        .hovered(hover);
                let main = split.main_half();
                fixes = fixes.child(split.finish(main));
            }
        }
        let mut block = div()
            .col()
            .w_px(width)
            .gap(Space::S2)
            .child(div().row().w_px(width).child(cause))
            .child(fixes);
        if let Some(error) = &self.action_error {
            block = block.child(
                div().row().px(Space::S4).child(
                    label(fix_summary(error))
                        .size(LabelSize::Small)
                        .color(colors.error)
                        .wrap(width - 8.0),
                ),
            );
        }
        if self.raw_open {
            block = block.child(self.details(width, &id, hover));
        }
        block.into()
    }

    /// Why it failed, where it tried, and git's or the server's own words.
    fn details(&self, width: f32, id: &impl Fn(FailureAction) -> u64, hover: Option<u64>) -> Node {
        let colors = theme();
        let inner = (width - 2.0 * PAD.px() - 2.0).max(40.0);
        let mut block = div()
            .col()
            .w_px(width)
            .p(PAD)
            .py(Space::S8)
            .gap(GAP)
            .rounded(Radius::Medium)
            .bg(colors.editor_background)
            .border(1.0, colors.border_variant)
            .child(
                label(self.why())
                    .size(LabelSize::Small)
                    .color(colors.text_muted)
                    .wrap(inner),
            );
        if self.main_absent {
            block = block.child(
                label("Nothing to copy from main: main has no such database either.")
                    .size(LabelSize::Small)
                    .color(colors.text_placeholder)
                    .wrap(inner),
            );
        }
        let mut facts = div().col().gap(Space::S2);
        for (name, value) in self.facts() {
            facts = facts.child(
                div()
                    .row()
                    .gap(Space::S8)
                    .items_center()
                    .child(
                        div().w_px(FACT_LABEL_W).child(
                            label(name)
                                .size(LabelSize::Small)
                                .color(colors.text_placeholder),
                        ),
                    )
                    .child(
                        div().row().flex(1.0).child(
                            label(value)
                                .size(LabelSize::Small)
                                .mono()
                                .color(colors.text)
                                .truncate(),
                        ),
                    ),
            );
        }
        block = block.child(facts);
        let raw = match &self.action_error {
            Some(error) => format!("{}\n\n{}", self.error.raw, output_tail(error)),
            None => self.error.raw.clone(),
        };
        let copy_id = id(FailureAction::CopyError);
        let copy = div()
            .w_px(22.0)
            .h_px(22.0)
            .rounded(Radius::Small)
            .items_center()
            .justify_center()
            .bg(if hover == Some(copy_id) {
                colors.element_hover
            } else {
                Rgba::TRANSPARENT
            })
            .on_click(copy_id)
            .child(icon(IconKind::Copy).size(11.0).color(colors.icon_muted));
        block
            .child(
                div()
                    .row()
                    .w_px(inner)
                    .gap(Space::S6)
                    .child(
                        label(raw)
                            .size(LabelSize::Mini)
                            .mono()
                            .color(colors.text)
                            .wrap(inner - 28.0),
                    )
                    .child(div().flex(1.0))
                    .child(copy),
            )
            .into()
    }
}

/// The line of a failed fix worth reading first: a command's output is mostly a backtrace, and the cause is the
/// line naming what broke (`FATAL: database ... does not exist`), not the first one.
fn fix_summary(error: &str) -> String {
    let first = error.lines().next().unwrap_or_default().trim();
    let cause = error
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != first)
        .find(|line| {
            let lower = line.to_lowercase();
            ["fatal:", "does not exist", "error:", "refused", "failed:"]
                .iter()
                .any(|mark| lower.contains(mark))
        });
    let text = match cause {
        Some(cause) => format!("{first} - {cause}"),
        None => first.to_string(),
    };
    const LIMIT: usize = 240;
    if text.chars().count() > LIMIT {
        format!("{}...", text.chars().take(LIMIT).collect::<String>())
    } else {
        text
    }
}

/// The end of a long command output, where the error usually is.
fn output_tail(error: &str) -> String {
    const LINES: usize = 40;
    let lines: Vec<&str> = error.lines().collect();
    let skipped = lines.len().saturating_sub(LINES);
    let tail = lines[skipped..].join("\n");
    if skipped > 0 {
        format!("... {skipped} earlier lines\n{tail}")
    } else {
        tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_fix_leads_with_the_line_that_names_the_cause() {
        let output = "migrate: exit 1\n/app/lib/model.rb:12:in 'extend'\n/app/lib/model.rb:10:in '<top>'\nCaused by:\nPG::ConnectionBad: connection to server at \"127.0.0.1\", port 5434 failed: FATAL:  database \"shop_tx\" does not exist\n/app/lib/tx.rb:47:in 'block'";
        let summary = fix_summary(output);
        assert!(summary.starts_with("migrate: exit 1 - "), "{summary}");
        assert!(summary.contains("\"shop_tx\" does not exist"), "{summary}");
        assert!(!summary.contains("model.rb"), "{summary}");
        let long: String = (0..100).map(|n| format!("line {n}\n")).collect();
        assert!(output_tail(&long).starts_with("... 60 earlier lines"));
    }

    fn failure(raw: &str) -> Failure {
        Failure::new(
            ConnectError::new(
                Engine::Postgres,
                "myproject_api_feat",
                "localhost",
                5434,
                "postgres",
                raw.to_string(),
            ),
            "api".into(),
        )
    }

    fn texts(failure: &Failure) -> String {
        let node = failure.render(360.0, |action| action.offset(), None);
        ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, 360.0, 900.0, Rgba::TRANSPARENT),
        )
        .texts
        .iter()
        .map(|text| text.text.clone())
        .collect::<Vec<_>>()
        .join("|")
    }

    #[test]
    fn each_kind_reads_as_its_cause() {
        let missing = failure("FATAL: database \"myproject_api_feat\" does not exist");
        assert_eq!(
            missing.title(),
            "Database myproject_api_feat does not exist"
        );
        assert!(missing.why().contains("Services of api"));
        assert_eq!(
            failure("error connecting to server: Connection refused (os error 61)").title(),
            "Can't reach Postgres at localhost:5434"
        );
        assert_eq!(
            failure("password authentication failed for user \"postgres\"").title(),
            "Postgres rejected user postgres"
        );
        let wrong = Failure::new(
            missing
                .error
                .clone()
                .on_foreign_server("other-shared-postgres-1".into()),
            "api".into(),
        );
        assert_eq!(
            wrong.title(),
            "Port 5434 answers, but it is not this project's Postgres"
        );
        assert!(wrong.is_warning());
        assert!(wrong.why().contains("other-shared-postgres-1"));
    }

    #[test]
    fn the_line_says_the_cause_and_the_details_wait_until_opened() {
        let mut missing = failure("FATAL: database \"myproject_api_feat\" does not exist");
        let folded = texts(&missing);
        assert_eq!(
            folded, "Database does not exist|Create database",
            "{folded}"
        );
        missing.raw_open = true;
        let open = texts(&missing);
        for wanted in [
            "Postgres is running and answered",
            "localhost:5434",
            "FATAL: database \"myproject_api_feat\" does not exist",
        ] {
            assert!(open.contains(wanted), "{wanted}: {open}");
        }
    }

    #[test]
    fn each_kind_offers_its_own_fixes() {
        let mut missing = failure("database \"x\" does not exist");
        assert_eq!(missing.actions(), [FailureAction::CreateDatabase]);
        assert_eq!(
            missing.more_actions(),
            [FailureAction::FixWithAgent, FailureAction::CopyError]
        );
        missing.main_copy = Some("myproject_api_main".into());
        assert_eq!(
            missing.more_actions(),
            [
                FailureAction::CopyFromMain,
                FailureAction::FixWithAgent,
                FailureAction::CopyError
            ]
        );
        missing.main_copy = None;
        missing.main_absent = true;
        missing.raw_open = true;
        assert!(texts(&missing).contains("Nothing to copy from main"));
        let down = failure("Connection refused");
        assert_eq!(
            down.actions(),
            [FailureAction::StartShared, FailureAction::Retry]
        );
        assert_eq!(
            texts(&down),
            "Postgres not answering on :5434|Start shared services"
        );
        assert_eq!(
            failure("password authentication failed for user \"postgres\"").actions(),
            [FailureAction::EditConfig, FailureAction::Retry]
        );
        let wrong = Failure::new(
            missing.error.clone().on_foreign_server("other".into()),
            "api".into(),
        );
        assert_eq!(
            wrong.actions(),
            [FailureAction::Retry, FailureAction::EditConfig]
        );
        let mut busy = failure("Connection refused");
        busy.busy = Some("Starting shared services...".into());
        assert_eq!(
            texts(&busy),
            "Postgres not answering on :5434|Starting shared services..."
        );
    }

    #[test]
    fn every_action_has_its_own_offset() {
        for action in ALL {
            assert_eq!(FailureAction::from_offset(action.offset()), Some(action));
            assert!(action.offset() < ACTION_STRIDE);
        }
    }
}
