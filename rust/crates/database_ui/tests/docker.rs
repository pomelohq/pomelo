//! A table tab against a real Postgres in a throwaway container (opt-in: `POM_DOCKER_TEST=1`).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use database_ui::{DatabaseContext, TableItem};
use pom_config::Config;
use pom_db::{Connector, TableKind};
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner};
use workspace::Item;

struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        if let Err(error) = Command::new("docker").args(["rm", "-f", &self.0]).output() {
            eprintln!("remove {}: {error}", self.0);
        }
    }
}

fn settle(item: &mut TableItem) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while item.is_busy() && Instant::now() < deadline {
        item.tick(&|| None);
        std::thread::sleep(Duration::from_millis(20));
    }
    item.tick(&|| None);
}

#[test]
fn a_table_tab_pages_sorts_filters_and_saves_edits() {
    if std::env::var_os("POM_DOCKER_TEST").is_none() {
        eprintln!("skipped: set POM_DOCKER_TEST=1 to run against Docker");
        return;
    }
    let temp = tempfile::tempdir().expect("temp");
    let session = format!("pomdbui{}", std::process::id());
    let root = temp.path().join("project");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(
        root.join("pom.yml"),
        format!("session: {session}\nshared_services:\n  postgres:\n    image: postgres:16-alpine\n    db_user: app\n    db_password: secret\nrepos:\n  api:\n    databases:\n      main: api\n"),
    )
    .expect("pom.yml");
    let config = Arc::new(Config::load(&root.join("pom.yml")).expect("config"));
    let runner = Arc::new(ServiceRunner::new(RunnerOptions {
        project_root: root,
        session: session.clone(),
        state: StateDir::new(temp.path().join("state")),
        holders: SocketDir::new(temp.path().join("s")),
        binary: PathBuf::from("/nonexistent"),
        docker: PathBuf::from("docker"),
    }));
    let database = pom_db::list_databases(&config, "feat").remove(0);
    let port = runner.postgres_endpoint(&config).port;
    let container = Container(format!("{session}-pg"));
    let started = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &container.0,
            "-e",
            "POSTGRES_USER=app",
            "-e",
            "POSTGRES_PASSWORD=secret",
            "-e",
            &format!("POSTGRES_DB={}", database.name),
            "-p",
            &format!("{port}:5432"),
            "postgres:16-alpine",
        ])
        .output()
        .expect("docker");
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );

    let connector = Connector {
        runner: &runner,
        config: &config,
        branch: "feat",
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while connector
        .query(&database, "drop table if exists people; drop table if exists teams; \
            create table teams (id int primary key, name text); \
            insert into teams select g, 'team ' || g from generate_series(1, 3) g; \
            create table people (id int primary key, name text, team_id int references teams(id), prefs jsonb); \
            insert into people select g, 'p' || g, g % 3 + 1, '{\"theme\": \"dark\"}' from generate_series(1, 30) g", 1)
        .is_err()
    {
        assert!(Instant::now() < deadline, "postgres never came up");
        std::thread::sleep(Duration::from_millis(500));
    }

    let config_for_context = config.clone();
    let context = DatabaseContext {
        runner: runner.clone(),
        state: pom_paths::StateDir::new(temp.path().join("state")),
        config: Arc::new(move || Some(config_for_context.clone())),
        branch: "feat".into(),
        workspace_root: temp.path().join("workspace--feat"),
        config_path: temp.path().join("pom.yml"),
        waker: Arc::new(|| {}),
        objects: Arc::new(database_ui::CurlTransport::default()),
    };
    let table = pom_db::Table {
        schema: "public".into(),
        name: "people".into(),
        kind: TableKind::Table,
        count: None,
    };
    let mut item = TableItem::new(context, database.clone(), table);
    settle(&mut item);
    let (rows, total, first) = item.page_summary();
    assert_eq!((rows, total), (30, Some(30)));
    assert_eq!(first.as_deref(), Some("1"));

    item.sort_by_column(0);
    settle(&mut item);
    item.sort_by_column(0);
    settle(&mut item);
    assert_eq!(
        item.page_summary().2.as_deref(),
        Some("30"),
        "descending after two header clicks"
    );

    item.set_filter("id <= 5");
    settle(&mut item);
    assert_eq!(item.page_summary().0, 5);
    assert_eq!(item.page_summary().1, Some(5));

    let structure = item.structure().expect("structure read").clone();
    assert_eq!(structure.primary_key(), ["id"]);
    assert_eq!(
        structure
            .column("team_id")
            .and_then(|column| column.references.clone()),
        Some(("teams".into(), "id".into()))
    );
    assert!(structure.indexes.iter().any(|index| index.primary));
    let teams = connector
        .table_structure(
            &database,
            &pom_db::Table {
                schema: "public".into(),
                name: "teams".into(),
                kind: TableKind::Table,
                count: None,
            },
        )
        .expect("teams");
    assert_eq!(teams.referenced_by[0].table, "people");
    assert_eq!(teams.referenced_by[0].column, "team_id");

    item.set_column_filter("name", "= p3");
    settle(&mut item);
    assert_eq!(
        item.page_summary().0,
        1,
        "the column filter joins the WHERE"
    );
    item.edit_cell(0, 1, Some("Ann O'Neil"));
    item.apply_edits();
    settle(&mut item);
    let saved = connector
        .query(&database, "select name from people where id = 3", 1)
        .expect("read back");
    assert_eq!(saved.rows[0][0].as_deref(), Some("Ann O'Neil"));
    assert!(item.pending_statements().is_empty());

    let failed = connector.apply(
        &database,
        &[
            "update people set name = 'kept' where id = 4".into(),
            "update people set team_id = 99 where id = 5".into(),
        ],
    );
    assert!(failed.is_err(), "the foreign key refuses team 99");
    let untouched = connector
        .query(&database, "select name from people where id = 4", 1)
        .expect("read back");
    assert_eq!(
        untouched.rows[0][0].as_deref(),
        Some("p4"),
        "a failed statement saves none of them"
    );
    drop(container);
}
