//! A workspace's databases, to browse: the Postgres databases its repos declare (named for the branch) and the
//! shared Redis instances (at the workspace's slot). Each call connects, works and disconnects, like the
//! previous core; values keep NULL apart from the text "NULL".

mod connect_error;
mod consoles;
mod postgres_driver;
mod redis_driver;
mod statements;

use std::path::Path;
use std::time::Duration;

use pom_config::Config;
use pom_services::ServiceRunner;
use serde::{Deserialize, Serialize};

pub use connect_error::{classify, ConnectError, ConnectErrorKind};
pub use consoles::{load_consoles, save_consoles, Console, ConsoleKind};
pub use statements::{first_keyword, statement_at, statement_ranges};

pub const DEFAULT_LIMIT: usize = 500;
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const MIGRATE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Postgres,
    Redis,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Database {
    /// The real database name (Postgres) or the shared service's name (Redis).
    pub name: String,
    pub engine: Engine,
    /// The repo alias that declares it, or `shared`.
    pub repo: String,
    /// What the repo calls it (its `databases:` key).
    pub label: String,
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
    /// Keys under a keyspace; unknown for tables.
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
        let quote = |part: &str| format!("\"{}\"", part.replace('"', "\"\""));
        if self.schema.is_empty() {
            quote(&self.name)
        } else {
            format!("{}.{}", quote(&self.schema), quote(&self.name))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub schema: String,
    pub table: String,
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
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

/// The databases a workspace on `branch` can browse, in config order: each repo's `databases:`, then the shared
/// Redis services.
pub fn list_databases(config: &Config, branch: &str) -> Vec<Database> {
    let mut databases = Vec::new();
    for (repo, dir) in &config.repos {
        let alias = if dir.alias.is_empty() {
            repo
        } else {
            &dir.alias
        };
        for (label, template) in &dir.databases {
            databases.push(Database {
                name: format!(
                    "{}_{}",
                    config.session,
                    pom_env::resolve_branch_tokens(template, branch)
                ),
                engine: Engine::Postgres,
                repo: alias.clone(),
                label: label.clone(),
            });
        }
    }
    for (name, def) in &config.shared_services {
        if def.kind == "redis" || (def.kind.is_empty() && name == "redis") {
            databases.push(Database {
                name: name.clone(),
                engine: Engine::Redis,
                repo: "shared".into(),
                label: name.clone(),
            });
        }
    }
    databases
}

/// How to reach a workspace's databases: the project's runner (ports, slots), its config and the branch.
pub struct Connector<'a> {
    pub runner: &'a ServiceRunner,
    pub config: &'a Config,
    pub branch: &'a str,
}

impl Connector<'_> {
    fn postgres(&self, database: &str, timeout: Duration) -> Result<postgres::Client, String> {
        let endpoint = self.runner.postgres_endpoint(self.config);
        postgres_driver::connect(&endpoint, database, timeout)
            .map_err(|error| format!("connect to {database}: {error}"))
    }

    /// The database's tables, or why it could not be opened: the cause, the login used and the full text.
    pub fn open(&self, database: &Database) -> Result<Vec<Table>, Box<ConnectError>> {
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
                postgres_driver::tables(&mut client).map_err(|raw| {
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
                redis_driver::keyspaces(&mut connection).map_err(|raw| {
                    Box::new(ConnectError {
                        kind: ConnectErrorKind::Other,
                        ..failure(raw)
                    })
                })
            }
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

    /// Runs `repo`'s migrate commands in `checkout` with the workspace's env for it.
    pub fn migrate(&self, repo: &str, checkout: &Path) -> Result<(), String> {
        let steps = self
            .config
            .repos
            .get(repo)
            .map(pom_config::Dir::effective_migrate)
            .unwrap_or_default();
        if steps.is_empty() {
            return Err(format!("{repo} has no migrate command in pom.yml"));
        }
        let env = self.runner.workspace_env(self.config, self.branch);
        env.write_env_files()
            .map_err(|error| format!("write env files: {error}"))?;
        let mut command = std::process::Command::new("zsh");
        command
            .args(["-lc", &steps.join(" && ")])
            .current_dir(checkout)
            .env("PATH", pom_services::tool_path())
            .envs(env.repo_env(repo));
        pom_services::run_within(&mut command, MIGRATE_TIMEOUT)
            .map(|_| ())
            .map_err(|error| format!("migrate: {error}"))
    }

    /// Whether the shared Postgres has a database of this name.
    pub fn database_exists(&self, name: &str) -> Result<bool, String> {
        postgres_driver::database_exists(&mut self.postgres("postgres", LIST_TIMEOUT)?, name)
    }

    fn redis(&self, name: &str) -> Result<redis::Connection, String> {
        redis_driver::connect(&self.runner.redis_url(name, self.branch))
    }

    pub fn tables(&self, database: &Database) -> Result<Vec<Table>, String> {
        match database.engine {
            Engine::Postgres => {
                postgres_driver::tables(&mut self.postgres(&database.name, LIST_TIMEOUT)?)
            }
            Engine::Redis => redis_driver::keyspaces(&mut self.redis(&database.name)?),
        }
    }

    pub fn columns(&self, database: &Database) -> Result<Vec<Column>, String> {
        match database.engine {
            Engine::Postgres => {
                postgres_driver::columns(&mut self.postgres(&database.name, LIST_TIMEOUT)?)
            }
            Engine::Redis => Ok(Vec::new()),
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
            Engine::Redis => Err("CSV export is for Postgres only".into()),
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
        .find(|(key, dir)| {
            let alias = if dir.alias.is_empty() {
                key.as_str()
            } else {
                dir.alias.as_str()
            };
            alias == database.repo
        })
        .map(|(key, dir)| (key.as_str(), dir))
}

/// The same database in the main workspace (what a new workspace copies), when it is a different one.
pub fn main_database(config: &Config, database: &Database) -> Option<String> {
    if database.engine != Engine::Postgres {
        return None;
    }
    let (key, dir) = repo_of(config, database)?;
    let template = dir.databases.get(&database.label)?;
    let main = format!(
        "{}_{}",
        config.session,
        pom_env::resolve_branch_tokens(template, config.default_branch_for(key))
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
    fn a_database_knows_its_repo_and_its_main_copy() {
        let config = config(
            "session: demo\ndefault_branch: main\nrepos:\n  web:\n    alias: front\n    databases:\n      main: \"web_{{branch.safe}}\"\n      audit: audit\n",
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
