//! The Database panel over a scratch project: its databases opened, still connecting, failed, or none at all.
//! Nothing connects; each database shows what it would have answered.

use std::sync::Arc;

use workspace::SidePanelView;

use crate::fixtures::Scratch;
use crate::{Story, StoryDef};

const PROJECT_YML: &str = "session: myproject\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\n  files:\n    image: minio/minio\nrepos:\n  api:\n    databases:\n      dev: \"{{branch.safe}}\"\n      test: \"{{branch.safe}}_test\"\n    env:\n      REDIS_URL: \"{{shared.redis.url}}\"\n      S3_ENDPOINT: \"http://{{shared.files.host}}\"\n  web:\n    shared_services:\n      - redis\n    databases:\n      main: \"web_{{branch.safe}}\"\n";

const NO_DATABASES_YML: &str = "session: myproject\nrepos:\n  api:\n    services:\n      server:\n        cmd: bin/rails server\n  web: {}\n";

/// Object storage that is never reached.
struct Offline;

impl pom_db::object_storage::HttpTransport for Offline {
    fn send(
        &self,
        _request: &pom_db::object_storage::HttpRequest,
    ) -> Result<pom_db::object_storage::HttpResponse, String> {
        Err("offline in the gallery".into())
    }
}

#[derive(Clone, Copy)]
enum State {
    Open,
    Connecting,
    Failed,
    /// The first failure opened to its details.
    FailedDetails,
    /// Every Postgres database of a repo waits on one stopped server.
    ServerDown,
    Folded,
    Empty,
}

struct DatabaseStory {
    panel: database_ui::DatabasePanel,
    // Dropped after the panel, which reads its consoles from here.
    _scratch: Scratch,
}

impl Story for DatabaseStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        self.panel.render(width, height)
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        self.panel.set_hover(id)
    }

    fn scroll(&mut self, dy: f32) -> bool {
        self.panel.scroll(dy)
    }
}

fn table(schema: &str, name: &str, kind: pom_db::TableKind, count: Option<usize>) -> pom_db::Table {
    pom_db::Table {
        schema: schema.into(),
        name: name.into(),
        kind,
        count,
    }
}

fn column(table: &str, name: &str, data_type: &str, primary_key: bool) -> pom_db::Column {
    pom_db::Column {
        schema: "public".into(),
        table: table.into(),
        name: name.into(),
        data_type: data_type.into(),
        primary_key,
        references: (name == "user_id").then(|| "users".to_string()),
    }
}

fn refused(database: &str) -> pom_db::ConnectError {
    pom_db::ConnectError::new(
        pom_db::Engine::Postgres,
        database,
        "localhost",
        5434,
        "postgres",
        "error connecting to server: Connection refused (os error 61)".into(),
    )
}

fn missing(database: &str) -> pom_db::ConnectError {
    pom_db::ConnectError::new(
        pom_db::Engine::Postgres,
        database,
        "localhost",
        5434,
        "postgres",
        format!("db error: FATAL: database \"{database}\" does not exist"),
    )
}

fn fill(panel: &mut database_ui::DatabasePanel, names: &[String], state: State) {
    let Some(first) = names.first() else {
        return;
    };
    match state {
        State::Open => {
            let users = table("public", "users", pom_db::TableKind::Table, Some(1284));
            panel.show_schema(
                first,
                pom_db::Schema {
                    tables: vec![
                        users.clone(),
                        table("public", "login_tokens", pom_db::TableKind::Table, Some(37)),
                        table("public", "orders", pom_db::TableKind::Table, Some(5120)),
                        table("public", "active_users", pom_db::TableKind::View, None),
                    ],
                    columns: vec![
                        column("users", "id", "bigint", true),
                        column("users", "email", "varchar(255)", false),
                        column("users", "name", "varchar(120)", false),
                        column("users", "created_at", "timestamptz", false),
                        column("login_tokens", "user_id", "bigint", false),
                    ],
                },
            );
            panel.expand_table(first, &users);
            panel.show_tables(
                "redis",
                vec![
                    table("", "session", pom_db::TableKind::Keyspace, Some(214)),
                    table("", "cache", pom_db::TableKind::Keyspace, Some(1873)),
                ],
            );
            panel.show_buckets("files", vec!["uploads".into()]);
            let object = |key: &str, size: u64| pom_db::object_storage::ObjectEntry {
                key: key.into(),
                size,
                ..pom_db::object_storage::ObjectEntry::default()
            };
            panel.show_folder(
                "files",
                "uploads",
                "",
                pom_db::object_storage::Listing {
                    prefixes: vec!["avatars/".into()],
                    objects: vec![
                        object("invoice-4412.pdf", 98_304),
                        object("import-2026-09.csv", 3_565_158),
                    ],
                    next: None,
                },
            );
            panel.show_prefix_stats(
                "files",
                "uploads",
                "avatars/",
                pom_db::object_storage::PrefixStats {
                    objects: 2940,
                    bytes: 1_288_490_188,
                    capped: false,
                },
            );
        }
        State::Connecting => panel.show_connecting(first),
        State::Failed | State::FailedDetails => {
            panel.show_failure(first, missing(first), Some("myproject_main".into()));
            if let Some(other) = names.get(1) {
                panel.show_failure(other, refused(other), None);
            }
            if matches!(state, State::FailedDetails) {
                panel.toggle_full_error(first);
            }
        }
        State::ServerDown => {
            for name in names.iter().take(2) {
                panel.show_failure(name, refused(name), None);
            }
        }
        State::Folded | State::Empty => {}
    }
}

fn build(state: State) -> anyhow::Result<Box<dyn Story>> {
    let scratch = Scratch::new("db")?;
    let dir = scratch.path().to_path_buf();
    let yaml = match state {
        State::Empty => NO_DATABASES_YML,
        _ => PROJECT_YML,
    };
    std::fs::write(dir.join("pom.yml"), yaml)?;
    let config = Arc::new(pom_config::Config::load(&dir.join("pom.yml"))?);
    let runner = Arc::new(pom_services::ServiceRunner::new(
        pom_services::RunnerOptions {
            project_root: dir.clone(),
            session: "myproject".into(),
            state: pom_paths::StateDir::new(dir.join("state")),
            holders: pom_ptyhost::SocketDir::new(dir.join("s")),
            binary: "/nonexistent".into(),
            docker: "/nonexistent".into(),
        },
    ));
    let names: Vec<String> = pom_db::list_databases(&config, "feat-login")
        .into_iter()
        .filter(|database| database.engine.is_sql())
        .map(|database| database.name)
        .collect();
    let context = database_ui::DatabaseContext {
        runner,
        state: pom_paths::StateDir::new(dir.join("state")),
        config: Arc::new(move || Some(config.clone())),
        branch: "feat-login".into(),
        workspace_root: dir.join("workspace--feat-login"),
        config_path: dir.join("pom.yml"),
        waker: Arc::new(|| {}),
        objects: Arc::new(Offline),
        choose_files: Arc::new(Vec::new),
    };
    let mut panel = database_ui::DatabasePanel::closed(context);
    fill(&mut panel, &names, state);
    Ok(Box::new(DatabaseStory {
        panel,
        _scratch: scratch,
    }))
}

const PANEL: (f32, f32) = (300.0, 840.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, build| StoryDef {
        id,
        page: "Organisms",
        group: "Database panel",
        title,
        size: PANEL,
        build,
    };
    vec![
        story("database-panel", "Opened", || build(State::Open)),
        story("database-panel-folded", "Folded", || build(State::Folded)),
        story("database-panel-connecting", "Connecting", || {
            build(State::Connecting)
        }),
        story("database-panel-failed", "Failed to connect", || {
            build(State::Failed)
        }),
        story("database-panel-failed-details", "Failure details", || {
            build(State::FailedDetails)
        }),
        story(
            "database-panel-server-down",
            "Server down for every database",
            || build(State::ServerDown),
        ),
        story("database-panel-empty", "No databases", || {
            build(State::Empty)
        }),
    ]
}
