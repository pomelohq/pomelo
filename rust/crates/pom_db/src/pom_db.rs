//! A workspace's databases, to browse: the Postgres databases its repos declare (named for the branch) and the
//! shared services (Redis at the workspace's slot, object storage, and the rest listed by engine). Each call
//! connects, works and disconnects, like the previous core; values keep NULL apart from the text "NULL".

mod connect_error;
mod consoles;
mod engine;
pub mod object_storage;
mod postgres_driver;
mod redis_driver;
pub mod sigv4;
mod statements;
mod structure;

use std::path::Path;
use std::time::Duration;

use pom_config::Config;
use pom_services::ServiceRunner;
use serde::{Deserialize, Serialize};

pub use connect_error::{classify, ConnectError, ConnectErrorKind};
pub use consoles::{load_consoles, save_consoles, Console, ConsoleKind};
pub use engine::Engine;
pub use object_storage::ObjectStore;
pub use redis_driver::{RedisKey, RedisValue, VALUE_ITEMS};
pub use statements::{first_keyword, statement_at, statement_ranges};
pub use structure::{
    combined_filter, filter_condition, quote_literal, reference_count_query, referenced_row_query,
    update_statement, update_statement_lines, ColumnInfo, Index, Reference, TableStructure,
};

pub const DEFAULT_LIMIT: usize = 500;
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_MINIO_PORT: u16 = 9000;
const DEFAULT_MINIO_KEY: &str = "minioadmin";

/// The name repo databases carry in `Database::repo` for a shared service.
pub const SHARED: &str = "shared";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Database {
    /// The real database name (Postgres) or the shared service's name.
    pub name: String,
    pub engine: Engine,
    /// The repo alias that declares it, or `shared`.
    pub repo: String,
    /// What the repo calls it (its `databases:` key), or the service's name.
    pub label: String,
    /// For a shared service: the repos (aliases) whose config refers to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<String>,
}

impl Database {
    pub fn is_shared(&self) -> bool {
        self.repo == SHARED
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableKind {
    Table,
    View,
    /// A Redis key prefix (`user:*`).
    Keyspace,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub schema: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: TableKind,
    /// Keys under a keyspace; the planner's row estimate for a table (unknown until it was analyzed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

impl Table {
    /// `name`, or `schema.name` outside `public`.
    pub fn qualified(&self) -> String {
        if self.schema.is_empty() || self.schema == "public" {
            self.name.clone()
        } else {
            format!("{}.{}", self.schema, self.name)
        }
    }

    /// The identifier to put in SQL, each part double-quoted.
    pub fn sql_name(&self) -> String {
        if self.schema.is_empty() {
            quote_identifier(&self.name)
        } else {
            format!(
                "{}.{}",
                quote_identifier(&self.schema),
                quote_identifier(&self.name)
            )
        }
    }
}

pub fn quote_identifier(part: &str) -> String {
    format!("\"{}\"", part.replace('"', "\"\""))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub schema: String,
    pub table: String,
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub primary_key: bool,
    /// The table a foreign key on this column points at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<String>,
}

/// What opening a database finds: its tables (or key prefixes) and every table's columns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schema {
    pub tables: Vec<Table>,
    pub columns: Vec<Column>,
}

impl Schema {
    pub fn columns_of<'a>(&'a self, table: &'a Table) -> impl Iterator<Item = &'a Column> + 'a {
        self.columns
            .iter()
            .filter(move |column| column.table == table.name && column.schema == table.schema)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    /// `None` is SQL NULL.
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows existed than the limit allowed.
    pub truncated: bool,
    /// For a statement that returns no rows: how many it changed.
    pub rows_affected: Option<u64>,
}

/// How a client logs in to a database: what `psql` or `redis-cli` needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Login {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    /// The database name (Postgres) or number (Redis).
    pub database: String,
}

impl Login {
    /// `postgres://user:password@host:port/db` or `redis://host:port/db`.
    pub fn url(&self, engine: Engine) -> String {
        let escape = |text: &str| sigv4::uri_encode(text, false);
        match engine {
            Engine::Redis => format!("redis://{}:{}/{}", self.host, self.port, self.database),
            _ if self.password.is_empty() => format!(
                "postgres://{}@{}:{}/{}",
                escape(&self.user),
                self.host,
                self.port,
                escape(&self.database)
            ),
            _ => format!(
                "postgres://{}:{}@{}:{}/{}",
                escape(&self.user),
                escape(&self.password),
                self.host,
                self.port,
                escape(&self.database)
            ),
        }
    }
}

fn alias<'a>(key: &'a str, dir: &'a pom_config::Dir) -> &'a str {
    if dir.alias.is_empty() {
        key
    } else {
        &dir.alias
    }
}

/// Whether a template refers to shared service `name` (`{{shared.name}}`, `{{shared.name.url}}`).
fn refers_to(text: &str, name: &str) -> bool {
    pom_env::template::refs(text).iter().any(|key| {
        key.strip_prefix("shared.")
            .is_some_and(|rest| rest == name || rest.starts_with(&format!("{name}.")))
    })
}

/// The repos (aliases, config order) whose config uses shared service `name`: its `shared_services:` list or a
/// `{{shared.name...}}` template in their env, services or env files.
pub fn service_users(config: &Config, name: &str) -> Vec<String> {
    config
        .repos
        .iter()
        .filter(|(_, dir)| {
            dir.shared_refs.iter().any(|shared| shared.name == name)
                || dir.env.values().any(|value| refers_to(value, name))
                || dir.own_env.values().any(|value| refers_to(value, name))
                || dir
                    .env_output
                    .iter()
                    .any(|file| file.env.values().any(|value| refers_to(value, name)))
                || dir.services.values().any(|service| {
                    service.env.values().any(|value| refers_to(value, name))
                        || refers_to(&service.cmd, name)
                })
        })
        .map(|(key, dir)| alias(key, dir).to_string())
        .collect()
}

/// The databases a workspace on `branch` can browse, in config order: each repo's `databases:`, then the shared
/// services other than Postgres (whose databases are the repos' own), each with the repos that use it.
pub fn list_databases(config: &Config, branch: &str) -> Vec<Database> {
    let mut databases = Vec::new();
    for (repo, dir) in &config.repos {
        for (label, template) in &dir.databases {
            databases.push(Database {
                name: format!(
                    "{}_{}",
                    config.session,
                    pom_env::resolve_branch_tokens(template, branch)
                ),
                engine: Engine::Postgres,
                repo: alias(repo, dir).to_string(),
                label: label.clone(),
                used_by: Vec::new(),
            });
        }
    }
    for (name, def) in &config.shared_services {
        let engine = Engine::of_service(name, def);
        if engine == Engine::Postgres {
            continue;
        }
        databases.push(Database {
            name: name.clone(),
            engine,
            repo: SHARED.into(),
            label: name.clone(),
            used_by: service_users(config, name),
        });
    }
    databases
}

/// How to reach a workspace's databases: the project's runner (ports, slots), its config and the branch.
pub struct Connector<'a> {
    pub runner: &'a ServiceRunner,
    pub config: &'a Config,
    pub branch: &'a str,
}

fn unsupported(database: &Database) -> String {
    format!(
        "{} is a {} service; the panel cannot browse it",
        database.name,
        database.engine.title()
    )
}

impl Connector<'_> {
    fn postgres(&self, database: &str, timeout: Duration) -> Result<postgres::Client, String> {
        let endpoint = self.runner.postgres_endpoint(self.config);
        postgres_driver::connect(&endpoint, database, timeout)
            .map_err(|error| format!("connect to {database}: {error}"))
    }

    /// The database's tables and columns (key prefixes for Redis), or why it could not be opened: the cause,
    /// the login used and the full text.
    pub fn open(&self, database: &Database) -> Result<Schema, Box<ConnectError>> {
        match database.engine {
            Engine::Postgres => {
                let endpoint = self.runner.postgres_endpoint(self.config);
                let failure = |raw: String| {
                    ConnectError::new(
                        Engine::Postgres,
                        &database.name,
                        &endpoint.host,
                        endpoint.port,
                        &endpoint.user,
                        raw,
                    )
                };
                let mut client = postgres_driver::connect(&endpoint, &database.name, LIST_TIMEOUT)
                    .map_err(|raw| Box::new(self.explain(failure(raw))))?;
                let listed = postgres_driver::tables(&mut client).and_then(|tables| {
                    Ok(Schema {
                        tables,
                        columns: postgres_driver::columns(&mut client)?,
                    })
                });
                listed.map_err(|raw| {
                    Box::new(ConnectError {
                        kind: ConnectErrorKind::Other,
                        ..failure(raw)
                    })
                })
            }
            Engine::Redis => {
                let url = self.runner.redis_url(&database.name, self.branch);
                let (host, port) = redis_address(&url);
                let failure = |raw: String| {
                    ConnectError::new(Engine::Redis, &database.name, &host, port, "", raw)
                };
                let mut connection =
                    redis_driver::connect(&url).map_err(|raw| Box::new(failure(raw)))?;
                redis_driver::keyspaces(&mut connection)
                    .map(|tables| Schema {
                        tables,
                        columns: Vec::new(),
                    })
                    .map_err(|raw| {
                        Box::new(ConnectError {
                            kind: ConnectErrorKind::Other,
                            ..failure(raw)
                        })
                    })
            }
            _ => Err(Box::new(ConnectError::new(
                database.engine,
                &database.name,
                "",
                0,
                "",
                unsupported(database),
            ))),
        }
    }

    fn explain(&self, error: ConnectError) -> ConnectError {
        if !error.server_answered() {
            return error;
        }
        match self.runner.port_owner(error.port) {
            Some(owner) if !self.runner.owns(&owner) => error.on_foreign_server(owner.container),
            _ => error,
        }
    }

    /// Creates just this database in the shared Postgres.
    pub fn create_database(&self, name: &str) -> Result<(), String> {
        self.runner
            .create_databases(self.config, &[name.to_string()])
            .map_err(|error| error.to_string())
    }

    /// Drops the database and creates it again, empty.
    pub fn reset_database(&self, name: &str) -> Result<(), String> {
        self.runner
            .drop_databases(self.config, &[name.to_string()])
            .map_err(|error| error.to_string())?;
        self.create_database(name)
    }

    /// Replaces `target` with a copy of `template`.
    pub fn copy_database(&self, template: &str, target: &str) -> Result<(), String> {
        self.runner
            .clone_database(self.config, template, target)
            .map_err(|error| error.to_string())
    }

    pub fn start_shared(&self) -> Result<(), String> {
        self.runner
            .ensure_shared(self.config)
            .map_err(|error| error.to_string())
    }

    /// Whether the shared Postgres has a database of this name.
    pub fn database_exists(&self, name: &str) -> Result<bool, String> {
        postgres_driver::database_exists(&mut self.postgres("postgres", LIST_TIMEOUT)?, name)
    }

    fn redis(&self, name: &str) -> Result<redis::Connection, String> {
        redis_driver::connect(&self.runner.redis_url(name, self.branch))
    }

    /// How `psql` or `redis-cli` logs in to it; `None` for engines without a client here.
    pub fn login(&self, database: &Database) -> Option<Login> {
        match database.engine {
            Engine::Postgres => {
                let endpoint = self.runner.postgres_endpoint(self.config);
                Some(Login {
                    host: endpoint.host,
                    port: endpoint.port,
                    user: endpoint.user,
                    password: endpoint.password,
                    database: database.name.clone(),
                })
            }
            Engine::Redis => {
                let url = self.runner.redis_url(&database.name, self.branch);
                let (host, port) = redis_address(&url);
                Some(Login {
                    host,
                    port,
                    user: String::new(),
                    password: String::new(),
                    database: url.rsplit('/').next().unwrap_or("0").to_string(),
                })
            }
            _ => None,
        }
    }

    /// The shared object storage `name`, at its published port with its root keys.
    pub fn object_store(&self, name: &str) -> ObjectStore {
        let def = self.config.shared_services.get(name);
        let pick = |keys: &[&str], fallback: Option<&String>| {
            def.and_then(|def| {
                keys.iter()
                    .find_map(|key| def.environment.get(*key))
                    .or(fallback)
                    .filter(|value| !value.is_empty())
                    .cloned()
            })
            .unwrap_or_else(|| DEFAULT_MINIO_KEY.to_string())
        };
        let host = def
            .map(|def| def.host.clone())
            .filter(|host| !host.is_empty())
            .unwrap_or_else(|| "localhost".into());
        let port = match self.runner.shared_host_port(name) {
            0 => DEFAULT_MINIO_PORT,
            port => port,
        };
        ObjectStore::new(
            &host,
            port,
            sigv4::Credentials {
                access_key: pick(
                    &["MINIO_ROOT_USER", "MINIO_ACCESS_KEY"],
                    def.map(|def| &def.db_user),
                ),
                secret_key: pick(
                    &["MINIO_ROOT_PASSWORD", "MINIO_SECRET_KEY"],
                    def.map(|def| &def.db_password),
                ),
                region: "us-east-1".into(),
            },
        )
    }

    pub fn tables(&self, database: &Database) -> Result<Vec<Table>, String> {
        match database.engine {
            Engine::Postgres => {
                postgres_driver::tables(&mut self.postgres(&database.name, LIST_TIMEOUT)?)
            }
            Engine::Redis => redis_driver::keyspaces(&mut self.redis(&database.name)?),
            _ => Err(unsupported(database)),
        }
    }

    pub fn columns(&self, database: &Database) -> Result<Vec<Column>, String> {
        match database.engine {
            Engine::Postgres => {
                postgres_driver::columns(&mut self.postgres(&database.name, LIST_TIMEOUT)?)
            }
            Engine::Redis => Ok(Vec::new()),
            _ => Err(unsupported(database)),
        }
    }

    /// `CREATE TABLE` (or `CREATE VIEW`) text for the table, with its constraints and indexes.
    pub fn table_ddl(&self, database: &Database, table: &Table) -> Result<String, String> {
        match database.engine {
            Engine::Postgres => {
                postgres_driver::table_ddl(&mut self.postgres(&database.name, LIST_TIMEOUT)?, table)
            }
            _ => Err(unsupported(database)),
        }
    }

    pub fn table_structure(
        &self,
        database: &Database,
        table: &Table,
    ) -> Result<TableStructure, String> {
        match database.engine {
            Engine::Postgres => {
                postgres_driver::structure(&mut self.postgres(&database.name, LIST_TIMEOUT)?, table)
            }
            _ => Err(unsupported(database)),
        }
    }

    /// Runs the statements in one transaction; answers how many rows they changed.
    pub fn apply(&self, database: &Database, statements: &[String]) -> Result<u64, String> {
        match database.engine {
            Engine::Postgres => postgres_driver::apply(
                &mut self.postgres(&database.name, QUERY_TIMEOUT)?,
                statements,
            ),
            _ => Err(unsupported(database)),
        }
    }

    /// Redis keys matching `pattern` with type and TTL, at most `limit` (and whether more exist).
    pub fn redis_keys(
        &self,
        database: &Database,
        pattern: &str,
        limit: usize,
    ) -> Result<(Vec<RedisKey>, bool), String> {
        match database.engine {
            Engine::Redis => redis_driver::keys(&mut self.redis(&database.name)?, pattern, limit),
            _ => Err(unsupported(database)),
        }
    }

    pub fn redis_value(&self, database: &Database, key: &str) -> Result<RedisValue, String> {
        match database.engine {
            Engine::Redis => redis_driver::value(&mut self.redis(&database.name)?, key),
            _ => Err(unsupported(database)),
        }
    }

    /// Sets a Redis key's TTL, or keeps it forever with `None`.
    pub fn redis_expire(
        &self,
        database: &Database,
        key: &str,
        seconds: Option<i64>,
    ) -> Result<(), String> {
        match database.engine {
            Engine::Redis => redis_driver::expire(&mut self.redis(&database.name)?, key, seconds),
            _ => Err(unsupported(database)),
        }
    }

    pub fn redis_delete(&self, database: &Database, key: &str) -> Result<(), String> {
        match database.engine {
            Engine::Redis => redis_driver::delete(&mut self.redis(&database.name)?, key),
            _ => Err(unsupported(database)),
        }
    }

    /// Deletes the Redis keys matching `pattern`; answers how many went.
    pub fn delete_keys(&self, database: &Database, pattern: &str) -> Result<u64, String> {
        match database.engine {
            Engine::Redis => {
                redis_driver::delete_matching(&mut self.redis(&database.name)?, pattern)
            }
            _ => Err(unsupported(database)),
        }
    }

    /// Runs SQL (Postgres) or a command or key pattern (Redis), keeping at most `limit` rows.
    pub fn query(
        &self,
        database: &Database,
        text: &str,
        limit: usize,
    ) -> Result<QueryResult, String> {
        let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
        match database.engine {
            Engine::Postgres => postgres_driver::query(
                &mut self.postgres(&database.name, QUERY_TIMEOUT)?,
                text,
                limit,
            ),
            Engine::Redis => redis_driver::query(&mut self.redis(&database.name)?, text, limit),
            _ => Err(unsupported(database)),
        }
    }

    /// Writes a query's whole result to a CSV file (header first); returns the rows written.
    pub fn export_csv(&self, database: &Database, sql: &str, path: &Path) -> Result<u64, String> {
        match database.engine {
            Engine::Postgres => postgres_driver::export_csv(
                &mut self.postgres(&database.name, EXPORT_TIMEOUT)?,
                sql,
                path,
            ),
            _ => Err("CSV export is for Postgres only".into()),
        }
    }
}

/// `host` and `port` of a `redis://host:port/db` URL.
fn redis_address(url: &str) -> (String, u16) {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(0)),
        None => (authority.to_string(), 0),
    }
}

/// The config entry of the repo a database belongs to: its key and its definition.
pub fn repo_of<'a>(
    config: &'a Config,
    database: &Database,
) -> Option<(&'a str, &'a pom_config::Dir)> {
    config
        .repos
        .iter()
        .find(|(key, dir)| alias(key, dir) == database.repo)
        .map(|(key, dir)| (key.as_str(), dir))
}

/// The same database in the main workspace (what a new workspace copies), when it is a different one.
pub fn main_database(config: &Config, database: &Database) -> Option<String> {
    if database.engine != Engine::Postgres {
        return None;
    }
    let (_, dir) = repo_of(config, database)?;
    let template = dir.databases.get(&database.label)?;
    // Named after the main workspace's branch, as creating a workspace does, not a repo's own default branch.
    let main = format!(
        "{}_{}",
        config.session,
        pom_env::resolve_branch_tokens(template, config.global_default_branch())
    );
    (main != database.name).then_some(main)
}

/// Every row of the table the filter keeps, in the typed order (what an export writes).
pub fn table_select(table: &Table, filter: &str, order: &str) -> String {
    let mut sql = format!("SELECT * FROM {}", table.sql_name());
    if !filter.trim().is_empty() {
        sql.push_str(&format!(" WHERE {}", filter.trim()));
    }
    if !order.trim().is_empty() {
        sql.push_str(&format!(" ORDER BY {}", order.trim()));
    }
    sql
}

/// The table browser's query: `SELECT *` with the typed filter and order, one page at `offset`.
pub fn table_query(
    table: &Table,
    filter: &str,
    order: &str,
    limit: usize,
    offset: usize,
) -> String {
    let mut sql = table_select(table, filter, order);
    sql.push_str(&format!(" LIMIT {limit}"));
    if offset > 0 {
        sql.push_str(&format!(" OFFSET {offset}"));
    }
    sql
}

/// Rows the table browser's filter matches, for "1-500 of N".
pub fn count_query(table: &Table, filter: &str) -> String {
    let mut sql = format!("SELECT count(*) FROM {}", table.sql_name());
    if !filter.trim().is_empty() {
        sql.push_str(&format!(" WHERE {}", filter.trim()));
    }
    sql
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("pom.yml");
        std::fs::write(&path, text).expect("pom.yml");
        Config::load(&path).expect("config")
    }

    #[test]
    fn databases_follow_the_branch_and_the_config_order() {
        let config = config(
            "session: demo\nshared_services:\n  cache:\n    type: redis\n    image: redis:7\n  redis:\n    image: redis:7\n  pg:\n    image: postgres:16\nrepos:\n  web:\n    alias: front\n    databases:\n      main: \"web_{{branch.safe}}\"\n  api:\n    databases:\n      main: \"api_{{branch.safe}}\"\n      audit: audit\n",
        );
        let names: Vec<(String, Engine, String, String)> = list_databases(&config, "feat/x")
            .into_iter()
            .map(|db| (db.name, db.engine, db.repo, db.label))
            .collect();
        assert_eq!(
            names,
            [
                (
                    "demo_web_feat_x".into(),
                    Engine::Postgres,
                    "front".into(),
                    "main".into()
                ),
                (
                    "demo_api_feat_x".into(),
                    Engine::Postgres,
                    "api".into(),
                    "main".into()
                ),
                (
                    "demo_audit".into(),
                    Engine::Postgres,
                    "api".into(),
                    "audit".into()
                ),
                (
                    "cache".into(),
                    Engine::Redis,
                    "shared".into(),
                    "cache".into()
                ),
                (
                    "redis".into(),
                    Engine::Redis,
                    "shared".into(),
                    "redis".into()
                ),
            ]
        );
    }

    #[test]
    fn shared_services_know_the_repos_that_use_them() {
        let config = config(
            "session: demo\nshared_services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\n  files:\n    image: minio/minio\n  queue:\n    image: rabbitmq:3\nrepos:\n  api:\n    databases:\n      dev: \"api_{{branch.safe}}\"\n    env:\n      REDIS_URL: \"redis://{{shared.redis.host}}:{{shared.redis.port}}\"\n      S3_URL: \"http://{{shared.files.host}}\"\n  web:\n    alias: front\n    shared_services:\n      - redis\n    services:\n      app:\n        cmd: npm start\n        env:\n          CACHE: \"{{shared.redis.url}}\"\n  search:\n    env:\n      NOTE: \"{{shared.redisx.host}}\"\n",
        );
        let databases = list_databases(&config, "feat");
        let shared: Vec<(&str, Engine, Vec<String>)> = databases
            .iter()
            .filter(|database| database.is_shared())
            .map(|database| {
                (
                    database.name.as_str(),
                    database.engine,
                    database.used_by.clone(),
                )
            })
            .collect();
        assert_eq!(
            shared,
            [
                (
                    "redis",
                    Engine::Redis,
                    vec!["api".to_string(), "front".to_string()]
                ),
                ("files", Engine::Minio, vec!["api".to_string()]),
                ("queue", Engine::Rabbitmq, Vec::new()),
            ]
        );
    }

    #[test]
    fn a_login_reads_as_a_connection_url() {
        let login = Login {
            host: "localhost".into(),
            port: 5434,
            user: "postgres".into(),
            password: "p@ss".into(),
            database: "demo_api".into(),
        };
        assert_eq!(
            login.url(Engine::Postgres),
            "postgres://postgres:p%40ss@localhost:5434/demo_api"
        );
        let redis = Login {
            port: 6390,
            database: "3".into(),
            ..login
        };
        assert_eq!(redis.url(Engine::Redis), "redis://localhost:6390/3");
    }

    #[test]
    fn a_database_knows_its_repo_and_its_main_copy() {
        let config = config(
            "session: demo\ndefault_branch: main\nrepos:\n  web:\n    alias: front\n    default_branch: master\n    databases:\n      main: \"web_{{branch.safe}}\"\n      audit: audit\n",
        );
        let databases = list_databases(&config, "feat/x");
        assert_eq!(
            repo_of(&config, &databases[0]).map(|(key, _)| key),
            Some("web")
        );
        assert_eq!(
            main_database(&config, &databases[0]).as_deref(),
            Some("demo_web_main")
        );
        assert_eq!(main_database(&config, &databases[1]), None);
        assert_eq!(
            redis_address("redis://localhost:6390/3"),
            ("localhost".to_string(), 6390)
        );
    }

    #[test]
    fn browser_queries_quote_the_table_and_page() {
        let table = Table {
            schema: "billing".into(),
            name: "odd\"name".into(),
            kind: TableKind::Table,
            count: None,
        };
        assert_eq!(table.qualified(), "billing.odd\"name");
        assert_eq!(
            table_query(&table, " id > 3 ", "id desc", 100, 200),
            "SELECT * FROM \"billing\".\"odd\"\"name\" WHERE id > 3 ORDER BY id desc LIMIT 100 OFFSET 200"
        );
        assert_eq!(
            count_query(&table, ""),
            "SELECT count(*) FROM \"billing\".\"odd\"\"name\""
        );
        let public = Table {
            schema: "public".into(),
            ..table
        };
        assert_eq!(public.qualified(), "odd\"name");
    }
}
