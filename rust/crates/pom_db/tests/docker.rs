//! Real Postgres and Redis in throwaway containers, on the ports the runner would use. Opt-in (`POM_DOCKER_TEST=1`)
//! since it needs Docker; the containers are removed when the test ends.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use pom_config::Config;
use pom_db::{list_databases, Connector, Engine, TableKind};
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use pom_services::{RunnerOptions, ServiceRunner};

struct Containers(Vec<String>);

impl Drop for Containers {
    fn drop(&mut self) {
        for name in &self.0 {
            if let Err(error) = Command::new("docker").args(["rm", "-f", name]).output() {
                eprintln!("remove {name}: {error}");
            }
        }
    }
}

fn docker(args: &[&str]) {
    let output = Command::new("docker").args(args).output().expect("docker");
    assert!(
        output.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn browses_real_postgres_and_redis() {
    if std::env::var_os("POM_DOCKER_TEST").is_none() {
        eprintln!("skipped: set POM_DOCKER_TEST=1 to run against Docker");
        return;
    }
    let temp = tempfile::tempdir().expect("temp");
    let session = format!("pomdbtest{}", std::process::id());
    let root = temp.path().join("project");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(
        root.join("pom.yml"),
        format!(
            "session: {session}\nshared_services:\n  postgres:\n    image: postgres:16-alpine\n    db_user: app\n    db_password: secret\n  redis:\n    image: redis:7-alpine\nrepos:\n  api:\n    databases:\n      main: \"api_{{{{branch.safe}}}}\"\n"
        ),
    )
    .expect("pom.yml");
    let config = Config::load(&root.join("pom.yml")).expect("config");
    let runner = ServiceRunner::new(RunnerOptions {
        project_root: root,
        session: session.clone(),
        state: StateDir::new(temp.path().join("state")),
        holders: SocketDir::new(temp.path().join("s")),
        binary: PathBuf::from("/nonexistent"),
        docker: PathBuf::from("docker"),
    });
    let databases = list_databases(&config, "feat");
    let postgres = databases
        .iter()
        .find(|db| db.engine == Engine::Postgres)
        .expect("postgres db");
    let redis = databases
        .iter()
        .find(|db| db.engine == Engine::Redis)
        .expect("redis db");

    let pg_port = runner.postgres_endpoint(&config).port;
    let redis_port: u16 = runner
        .redis_url("redis", "feat")
        .trim_start_matches("redis://localhost:")
        .split('/')
        .next()
        .and_then(|port| port.parse().ok())
        .expect("redis port");
    let names = Containers(vec![format!("{session}-pg"), format!("{session}-redis")]);
    docker(&[
        "run",
        "-d",
        "--name",
        &names.0[0],
        "-e",
        "POSTGRES_USER=app",
        "-e",
        "POSTGRES_PASSWORD=secret",
        "-e",
        &format!("POSTGRES_DB={}", postgres.name),
        "-p",
        &format!("{pg_port}:5432"),
        "postgres:16-alpine",
    ]);
    docker(&[
        "run",
        "-d",
        "--name",
        &names.0[1],
        "-p",
        &format!("{redis_port}:6379"),
        "redis:7-alpine",
    ]);

    let connector = Connector {
        runner: &runner,
        config: &config,
        branch: "feat",
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match connector.query(postgres, "select 1", 1) {
            Ok(_) => break,
            Err(error) if Instant::now() < deadline => {
                eprintln!("waiting for postgres: {error}");
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(error) => panic!("postgres never came up: {error}"),
        }
    }

    let setup = connector
        .query(
            postgres,
            "create schema billing; create table users (id int primary key, name text, note text); \
             insert into users values (1, 'ann', null), (2, 'bob', 'NULL'), (3, 'cy', 'two\nlines'); \
             create view billing.names as select name from users",
            10,
        )
        .expect("setup");
    assert!(setup.columns.is_empty());

    let tables = connector.tables(postgres).expect("tables");
    let listed: Vec<(String, TableKind)> = tables.iter().map(|t| (t.qualified(), t.kind)).collect();
    assert_eq!(
        listed,
        [
            ("billing.names".to_string(), TableKind::View),
            ("users".to_string(), TableKind::Table),
        ]
    );
    let columns = connector.columns(postgres).expect("columns");
    assert!(columns
        .iter()
        .any(|c| c.table == "users" && c.name == "note" && c.data_type == "text"));

    let page = connector
        .query(postgres, "select * from users order by id;", 2)
        .expect("page");
    assert_eq!(page.columns, ["id", "name", "note"]);
    assert_eq!(page.rows.len(), 2);
    assert!(page.truncated);
    assert_eq!(page.rows[0][2], None, "NULL is not text");
    assert_eq!(page.rows[1][2].as_deref(), Some("NULL"));

    let changed = connector
        .query(
            postgres,
            "update users set name = upper(name) where id < 3",
            10,
        )
        .expect("update");
    assert_eq!(changed.rows_affected, Some(2));
    let moved = connector
        .query(
            postgres,
            "with gone as (delete from users where id = 3 returning id) select id from gone",
            10,
        )
        .expect("data-modifying with");
    assert_eq!(moved.rows, [[Some("3".to_string())]]);

    let file = temp.path().join("users.csv");
    connector
        .query(postgres, "insert into users values (4, 'di', 'a\nb')", 1)
        .expect("insert");
    let written = connector
        .export_csv(postgres, "select * from users order by id", &file)
        .expect("export");
    assert_eq!(written, 3);
    let csv = std::fs::read_to_string(&file).expect("csv");
    assert!(
        csv.starts_with("id,name,note\n1,ANN,\n2,BOB,NULL\n"),
        "{csv}"
    );

    for key in ["user:1", "user:2", "session:x"] {
        connector
            .query(redis, &format!("SET {key} v-{key}"), 1)
            .expect("set");
    }
    let keyspaces = connector.tables(redis).expect("keyspaces");
    let counts: Vec<(String, Option<usize>)> = keyspaces
        .iter()
        .map(|t| (t.name.clone(), t.count))
        .collect();
    assert_eq!(
        counts,
        [
            ("session".to_string(), Some(1)),
            ("user".to_string(), Some(2))
        ]
    );
    let keys = connector.query(redis, "user:*", 10).expect("scan");
    assert_eq!(keys.columns, ["key", "type", "ttl", "value"]);
    assert_eq!(keys.rows.len(), 2);
    let value = connector.query(redis, "GET user:1", 10).expect("get");
    assert_eq!(value.rows, [[Some("v-user:1".to_string())]]);
    drop(names);
}
