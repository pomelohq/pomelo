//! Database snapshots of a workspace. A snapshot is a copy of each of the workspace's databases kept as a template
//! no one may connect to, so cloning from it never needs to disconnect anyone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use pom_config::Config;
use serde::{Deserialize, Serialize};

use crate::control::{ServiceError, ServiceRunner, ServiceTarget};
use crate::shared::{output_text, Postgres};

/// The snapshot `prepare-main` takes of main's databases; new workspaces clone from it.
pub const MAIN_BASELINE: &str = "main__baseline";
/// The snapshot a workspace takes once it is created and migrated.
pub const WORKSPACE_BASELINE: &str = "ws__baseline";

const POSTGRES_DB: &str = "postgres";
const SNAPSHOT_INFIX: &str = "__snap__";
/// Postgres cuts identifiers at this many bytes.
const PG_NAME_LIMIT: usize = 63;
const HASH_LEN: usize = 8;
const MAX_NAME: usize = 64;
const FILE_COPY_SINCE: u32 = 150_000;
const DROP_FORCE_SINCE: u32 = 130_000;
const DATA_DIR: &str = "/var/lib/postgresql/data";

/// A snapshot name is used inside a quoted identifier, so only plain characters are allowed.
pub fn valid_snapshot_name(name: &str) -> Result<(), String> {
    let plain = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if name.is_empty() || name.len() > MAX_NAME || !plain {
        return Err(format!(
            "snapshot name {name:?}: use 1-{MAX_NAME} letters, digits, - or _"
        ));
    }
    Ok(())
}

fn truncate(name: &str, limit: usize) -> &str {
    let mut end = name.len().min(limit);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// `<db>__snap__<name>`; past Postgres' 63 bytes, the database part is cut and the name becomes a hash of both.
pub fn snapshot_db_name(db: &str, snapshot: &str) -> String {
    let full = format!("{db}{SNAPSHOT_INFIX}{snapshot}");
    if full.len() <= PG_NAME_LIMIT {
        return full;
    }
    let hash = pom_env::branch_hash(&full);
    let hash = truncate(&hash, HASH_LEN);
    let room = PG_NAME_LIMIT - SNAPSHOT_INFIX.len() - hash.len();
    format!("{}{SNAPSHOT_INFIX}{hash}", truncate(db, room))
}

/// The databases `branch` owns, as Postgres names them. A name that does not change with the branch belongs to main
/// too, so another branch never owns it.
pub fn owned_database_names(
    config: &Config,
    branch: &str,
    keep: impl Fn(&str) -> bool,
) -> Vec<String> {
    let main = config.global_default_branch();
    let shared_with_main: Vec<String> = if branch == main {
        Vec::new()
    } else {
        crate::database_names(config, main)
    };
    let mut names: Vec<String> = Vec::new();
    for name in crate::database_names_where(config, branch, keep) {
        if shared_with_main.contains(&name) {
            continue;
        }
        let name = truncate(&name, PG_NAME_LIMIT).to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Statements copying `db` into a snapshot and sealing it as a template no one can connect to.
pub fn snapshot_sql(db: &str, snapshot_db: &str, replace: bool) -> Vec<String> {
    let mut sql = Vec::new();
    if replace {
        sql.extend(drop_snapshot_sql(snapshot_db));
    }
    sql.push(format!(
        "CREATE DATABASE {} TEMPLATE {}",
        quoted(snapshot_db),
        quoted(db)
    ));
    sql.push(format!(
        "ALTER DATABASE {} WITH IS_TEMPLATE true ALLOW_CONNECTIONS false",
        quoted(snapshot_db)
    ));
    sql
}

/// Statements replacing `db` with a copy of its snapshot. `FILE_COPY` (Postgres 15+) copies a big database far
/// faster than the default, which writes every block to the WAL.
pub fn restore_sql(db: &str, snapshot_db: &str, server_version: u32) -> Vec<String> {
    let drop = if server_version >= DROP_FORCE_SINCE {
        format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quoted(db))
    } else {
        format!("DROP DATABASE IF EXISTS {}", quoted(db))
    };
    let strategy = if server_version >= FILE_COPY_SINCE {
        " STRATEGY FILE_COPY"
    } else {
        ""
    };
    vec![
        drop,
        format!(
            "CREATE DATABASE {} TEMPLATE {}{strategy}",
            quoted(db),
            quoted(snapshot_db)
        ),
    ]
}

fn drop_snapshot_sql(snapshot_db: &str) -> Vec<String> {
    vec![
        format!(
            "ALTER DATABASE {} WITH IS_TEMPLATE false",
            quoted(snapshot_db)
        ),
        format!("DROP DATABASE {}", quoted(snapshot_db)),
    ]
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotDb {
    pub db: String,
    pub snapshot_db: String,
    #[serde(default)]
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub created_ms: u64,
    pub databases: Vec<SnapshotDb>,
}

/// `.pom/snapshots.json` in the workspace folder: which databases each snapshot holds, under which names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotIndex {
    #[serde(default)]
    pub snapshots: BTreeMap<String, SnapshotEntry>,
}

impl SnapshotIndex {
    pub fn path(folder: &Path) -> PathBuf {
        folder.join(".pom").join("snapshots.json")
    }

    pub fn load(folder: &Path) -> SnapshotIndex {
        std::fs::read_to_string(Self::path(folder))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, folder: &Path) -> std::io::Result<()> {
        let path = Self::path(folder);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let staged = path.with_extension("json.tmp");
        std::fs::write(&staged, text)?;
        std::fs::rename(staged, path)
    }
}

/// What happened to one database.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DbOutcome {
    pub db: String,
    pub snapshot_db: String,
    pub bytes: u64,
    pub ms: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The result of a snapshot command over a whole workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SnapshotReport {
    pub schema: &'static str,
    pub workspace: String,
    pub snapshot: String,
    pub action: &'static str,
    pub databases: Vec<DbOutcome>,
    pub total_ms: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub services_restarted: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl SnapshotReport {
    fn new(workspace: &str, snapshot: &str, action: &'static str) -> SnapshotReport {
        SnapshotReport {
            schema: "pom.db/v1",
            workspace: workspace.to_string(),
            snapshot: snapshot.to_string(),
            action,
            ..SnapshotReport::default()
        }
    }

    pub fn ok(&self) -> bool {
        self.databases.iter().all(|outcome| outcome.ok)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn sql_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

impl ServiceRunner {
    /// `server_version_num` of the shared Postgres (150004 for 15.4).
    pub fn postgres_version(&self, config: &Config) -> Result<u32, ServiceError> {
        let postgres = self.postgres(config);
        let rows = self.psql_rows(&postgres, "SHOW server_version_num")?;
        rows.trim().parse().map_err(|_| {
            ServiceError::Io(std::io::Error::other(format!(
                "could not read the Postgres version: {}",
                rows.trim()
            )))
        })
    }

    fn database_bytes(&self, postgres: &Postgres, name: &str) -> u64 {
        let sql = format!("SELECT pg_database_size({})", sql_literal(name));
        self.psql_rows(postgres, &sql)
            .ok()
            .and_then(|rows| rows.trim().parse().ok())
            .unwrap_or(0)
    }

    fn run_all(&self, postgres: &Postgres, statements: &[String]) -> Result<(), String> {
        for statement in statements {
            let output = self
                .psql(postgres, POSTGRES_DB, statement)
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(output_text(&output).trim().to_string());
            }
        }
        Ok(())
    }

    /// Snapshot databases that exist in the shared Postgres (names containing `__snap__`).
    pub fn snapshot_databases_present(&self, config: &Config) -> Result<Vec<String>, ServiceError> {
        let postgres = self.postgres(config);
        let sql = format!(
            "SELECT datname FROM pg_database WHERE datistemplate AND position({} in datname) > 0",
            sql_literal(SNAPSHOT_INFIX)
        );
        Ok(self
            .psql_rows(&postgres, &sql)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Free bytes in the shared Postgres' data folder, when it runs in a container of ours.
    fn data_dir_free_bytes(&self, postgres: &Postgres) -> Option<u64> {
        let container = postgres.container.as_ref()?;
        let args = ["exec", container, "df", "-Pk", DATA_DIR].map(str::to_string);
        let output = self.docker(&args).ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let available: u64 = text
            .lines()
            .nth(1)?
            .split_whitespace()
            .nth(3)?
            .parse()
            .ok()?;
        Some(available * 1024)
    }

    /// Copies every database in `dbs` into snapshot `name`, recording it in `folder`'s index.
    pub fn snapshot_workspace(
        &self,
        config: &Config,
        branch: &str,
        folder: &Path,
        dbs: &[String],
        name: &str,
        replace: bool,
    ) -> Result<SnapshotReport, String> {
        valid_snapshot_name(name)?;
        let mut index = SnapshotIndex::load(folder);
        if index.snapshots.contains_key(name) && !replace {
            return Err(format!(
                "snapshot {name} exists in workspace {branch}; pass --replace to take it again"
            ));
        }
        let started = Instant::now();
        let postgres = self.postgres(config);
        let mut report = SnapshotReport::new(branch, name, "snapshot");
        let mut entry = SnapshotEntry {
            created_ms: now_ms(),
            databases: Vec::new(),
        };
        let present = self.snapshot_databases_present(config).unwrap_or_default();
        for db in dbs {
            let snapshot_db = snapshot_db_name(db, name);
            let begin = Instant::now();
            // Postgres copies a database only while no one else is connected to it.
            self.terminate(&postgres, db);
            let statements = snapshot_sql(db, &snapshot_db, present.contains(&snapshot_db));
            let result = self.run_all(&postgres, &statements);
            let bytes = if result.is_ok() {
                self.database_bytes(&postgres, &snapshot_db)
            } else {
                0
            };
            report.databases.push(DbOutcome {
                db: db.clone(),
                snapshot_db: snapshot_db.clone(),
                bytes,
                ms: begin.elapsed().as_millis() as u64,
                ok: result.is_ok(),
                error: result.err(),
            });
            entry.databases.push(SnapshotDb {
                db: db.clone(),
                snapshot_db,
                bytes,
            });
        }
        if report.ok() {
            index.snapshots.insert(name.to_string(), entry);
            index
                .save(folder)
                .map_err(|error| format!("save the snapshot index: {error}"))?;
            let size: u64 = report.databases.iter().map(|outcome| outcome.bytes).sum();
            if let Some(free) = self.data_dir_free_bytes(&postgres) {
                if free < size.saturating_mul(2) {
                    report.warnings.push(format!(
                        "the shared Postgres has {} MB free and this workspace's data takes {} MB: drop old snapshots with `pom db snapshot drop <name>`",
                        free / 1_000_000,
                        size / 1_000_000
                    ));
                }
            }
        }
        report.total_ms = started.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// Replaces each database of the workspace with its copy in snapshot `name`. The workspace's running services
    /// are stopped first and, unless `restart` is false, started again afterwards.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_workspace(
        &self,
        config: &Config,
        branch: &str,
        is_main: bool,
        folder: &Path,
        dbs: &[String],
        name: &str,
        restart: bool,
    ) -> Result<SnapshotReport, String> {
        valid_snapshot_name(name)?;
        let index = SnapshotIndex::load(folder);
        let entry = index
            .snapshots
            .get(name)
            .ok_or_else(|| format!("workspace {branch} has no snapshot {name}"))?;
        let started = Instant::now();
        let version = self
            .postgres_version(config)
            .map_err(|error| error.to_string())?;
        let running = self.stop_running_services(config, branch, is_main)?;
        let postgres = self.postgres(config);
        let mut report = SnapshotReport::new(branch, name, "restore");
        for db in dbs {
            let Some(saved) = entry.databases.iter().find(|saved| saved.db == *db) else {
                report.databases.push(DbOutcome {
                    db: db.clone(),
                    ok: false,
                    error: Some(format!("snapshot {name} has no copy of {db}")),
                    ..DbOutcome::default()
                });
                break;
            };
            let begin = Instant::now();
            self.terminate(&postgres, db);
            let result = self.run_all(&postgres, &restore_sql(db, &saved.snapshot_db, version));
            let failed = result.is_err();
            report.databases.push(DbOutcome {
                db: db.clone(),
                snapshot_db: saved.snapshot_db.clone(),
                bytes: if failed {
                    0
                } else {
                    self.database_bytes(&postgres, db)
                },
                ms: begin.elapsed().as_millis() as u64,
                ok: !failed,
                error: result.err(),
            });
            if failed {
                break;
            }
        }
        if restart {
            report.services_restarted = self.start_services(config, &running, &mut report.warnings);
        }
        report.total_ms = started.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// Drops snapshot `name` of the workspace: its databases and its index entry.
    pub fn drop_workspace_snapshot(
        &self,
        config: &Config,
        branch: &str,
        folder: &Path,
        name: &str,
    ) -> Result<SnapshotReport, String> {
        valid_snapshot_name(name)?;
        let mut index = SnapshotIndex::load(folder);
        let entry = index
            .snapshots
            .remove(name)
            .ok_or_else(|| format!("workspace {branch} has no snapshot {name}"))?;
        let started = Instant::now();
        let postgres = self.postgres(config);
        let mut report = SnapshotReport::new(branch, name, "drop");
        for saved in &entry.databases {
            let begin = Instant::now();
            let result = self.run_all(&postgres, &drop_snapshot_sql(&saved.snapshot_db));
            report.databases.push(DbOutcome {
                db: saved.db.clone(),
                snapshot_db: saved.snapshot_db.clone(),
                bytes: saved.bytes,
                ms: begin.elapsed().as_millis() as u64,
                ok: result.is_ok(),
                error: result.err(),
            });
        }
        index
            .save(folder)
            .map_err(|error| format!("save the snapshot index: {error}"))?;
        report.total_ms = started.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// Drops every snapshot of a workspace that is going away.
    pub fn drop_all_snapshots(&self, config: &Config, folder: &Path) -> Result<(), String> {
        let index = SnapshotIndex::load(folder);
        let postgres = self.postgres(config);
        let mut failures = Vec::new();
        for saved in index.snapshots.values().flat_map(|entry| &entry.databases) {
            if let Err(error) = self.run_all(&postgres, &drop_snapshot_sql(&saved.snapshot_db)) {
                failures.push(format!("{}: {error}", saved.snapshot_db));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    /// Drops snapshot databases by name (orphans whose workspace is gone).
    pub fn drop_snapshot_databases(&self, config: &Config, names: &[String]) -> Result<(), String> {
        let postgres = self.postgres(config);
        let failures: Vec<String> = names
            .iter()
            .filter_map(|name| {
                self.run_all(&postgres, &drop_snapshot_sql(name))
                    .err()
                    .map(|error| format!("{name}: {error}"))
            })
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    /// Stops the workspace's running repo services and returns them, in config order.
    pub fn stop_running_services(
        &self,
        config: &Config,
        branch: &str,
        is_main: bool,
    ) -> Result<Vec<ServiceTarget>, String> {
        let running: Vec<ServiceTarget> = Self::service_targets(config, branch, is_main)
            .into_iter()
            .filter(|target| !target.is_workspace_level() && self.is_running(target))
            .collect();
        for target in &running {
            self.stop(target)
                .map_err(|error| format!("stop {}/{}: {error}", target.repo, target.service))?;
        }
        Ok(running)
    }

    fn start_services(
        &self,
        config: &Config,
        targets: &[ServiceTarget],
        warnings: &mut Vec<String>,
    ) -> Vec<String> {
        let mut started = Vec::new();
        for target in targets {
            let label = format!("{}/{}", target.repo, target.service);
            match self.start(config, target) {
                Ok(_) => started.push(label),
                Err(error) => warnings.push(format!("restart {label}: {error}")),
            }
        }
        started
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_names_fit_postgres_and_stay_distinct() {
        assert_eq!(
            snapshot_db_name("myproject_feat_login", "before-checkout"),
            "myproject_feat_login__snap__before-checkout"
        );
        let long_db = "myproject_".to_string() + &"x".repeat(50);
        let first = snapshot_db_name(&long_db, "step-one");
        let second = snapshot_db_name(&long_db, "step-two");
        assert!(first.len() <= PG_NAME_LIMIT, "{first}");
        assert_ne!(first, second);
        assert!(first.contains(SNAPSHOT_INFIX));
        assert!(valid_snapshot_name("ws__baseline").is_ok());
        assert!(valid_snapshot_name("a\"; DROP").is_err());
        assert!(valid_snapshot_name("").is_err());
    }

    #[test]
    fn restore_uses_file_copy_and_force_only_where_postgres_has_them() {
        let new = restore_sql("db", "db__snap__s", 160_002);
        assert_eq!(new[0], "DROP DATABASE IF EXISTS \"db\" WITH (FORCE)");
        assert_eq!(
            new[1],
            "CREATE DATABASE \"db\" TEMPLATE \"db__snap__s\" STRATEGY FILE_COPY"
        );
        let fourteen = restore_sql("db", "db__snap__s", 140_010);
        assert_eq!(
            fourteen[1],
            "CREATE DATABASE \"db\" TEMPLATE \"db__snap__s\""
        );
        let twelve = restore_sql("db", "db__snap__s", 120_000);
        assert_eq!(twelve[0], "DROP DATABASE IF EXISTS \"db\"");
    }

    #[test]
    fn a_snapshot_is_sealed_and_replacing_drops_the_old_one_first() {
        let sql = snapshot_sql("db", "db__snap__s", true);
        assert_eq!(
            sql[0],
            "ALTER DATABASE \"db__snap__s\" WITH IS_TEMPLATE false"
        );
        assert_eq!(sql[1], "DROP DATABASE \"db__snap__s\"");
        assert_eq!(sql[2], "CREATE DATABASE \"db__snap__s\" TEMPLATE \"db\"");
        assert_eq!(
            sql[3],
            "ALTER DATABASE \"db__snap__s\" WITH IS_TEMPLATE true ALLOW_CONNECTIONS false"
        );
        assert_eq!(snapshot_sql("db", "db__snap__s", false).len(), 2);
    }

    #[test]
    fn a_branch_never_owns_a_database_it_shares_with_main() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("pom.yml");
        std::fs::write(
            &path,
            "session: myproject\ndefault_branch: main\nshared_services:\n  postgres:\n    image: postgres:16\nrepos:\n  api:\n    shared_services:\n      - postgres:\n          db_name: shared_reports\n    databases:\n      main: \"{{branch.safe}}\"\n    services:\n      server:\n        cmd: run\n",
        )
        .expect("pom.yml");
        let config = pom_config::Config::load(&path).expect("config");
        assert_eq!(
            owned_database_names(&config, "feat-login", |_| true),
            vec!["myproject_feat-login".to_string()]
        );
        assert_eq!(
            owned_database_names(&config, "main", |_| true),
            vec!["shared_reports".to_string(), "myproject_main".to_string()]
        );
    }

    #[test]
    fn the_index_round_trips() {
        let folder = tempfile::tempdir().expect("folder");
        let mut index = SnapshotIndex::default();
        index.snapshots.insert(
            "s".into(),
            SnapshotEntry {
                created_ms: 1,
                databases: vec![SnapshotDb {
                    db: "db".into(),
                    snapshot_db: "db__snap__s".into(),
                    bytes: 42,
                }],
            },
        );
        index.save(folder.path()).expect("save");
        assert_eq!(SnapshotIndex::load(folder.path()), index);
    }
}
