//! Postgres over the simple (text) protocol, so every value arrives as text or NULL whatever its type. A query
//! that reads rows runs through a cursor, so a huge table costs only the rows kept.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use pom_services::Endpoint;
use postgres::{Client, NoTls, SimpleQueryMessage};

use crate::{first_keyword, Column, QueryResult, Table, TableKind};

const CURSOR: &str = "pom_browse";
/// Statements a cursor can run (`DECLARE .. FOR` takes only a SELECT or VALUES, which these start).
const CURSOR_KEYWORDS: [&str; 4] = ["select", "with", "values", "table"];

pub(crate) fn connect(
    endpoint: &Endpoint,
    database: &str,
    timeout: Duration,
) -> Result<Client, String> {
    let mut config = postgres::Config::new();
    config
        .host(&endpoint.host)
        .port(endpoint.port)
        .user(&endpoint.user)
        .password(&endpoint.password)
        .dbname(database)
        .connect_timeout(Duration::from_secs(10))
        .options(&format!("-c statement_timeout={}", timeout.as_millis()));
    config.connect(NoTls).map_err(|error| describe(&error))
}

pub(crate) fn database_exists(client: &mut Client, name: &str) -> Result<bool, String> {
    let sql = format!(
        "SELECT 1 FROM pg_database WHERE datname = '{}'",
        name.replace('\'', "''")
    );
    Ok(!rows(client, &sql)?.is_empty())
}

fn describe(error: &postgres::Error) -> String {
    match error.as_db_error() {
        Some(db) => db.message().to_string(),
        // "error connecting to server" alone hides why; the cause (refused, timed out) is its source.
        None => match std::error::Error::source(error) {
            Some(cause) => format!("{error}: {cause}"),
            None => error.to_string(),
        },
    }
}

fn rows(client: &mut Client, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
    let messages = client.simple_query(sql).map_err(|error| describe(&error))?;
    Ok(messages
        .into_iter()
        .filter_map(|message| match message {
            SimpleQueryMessage::Row(row) => Some(
                (0..row.len())
                    .map(|index| row.get(index).map(str::to_string))
                    .collect(),
            ),
            _ => None,
        })
        .collect())
}

const USER_SCHEMAS: &str =
    "table_schema NOT LIKE 'pg\\_%' AND table_schema <> 'information_schema'";
const USER_NAMESPACES: &str = "n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema'";

fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Tables and views, each table with the planner's row estimate (`reltuples`, -1 or 0 before an ANALYZE).
pub(crate) fn tables(client: &mut Client) -> Result<Vec<Table>, String> {
    let sql = format!(
        "SELECT t.table_schema, t.table_name, t.table_type, c.reltuples::bigint \
         FROM information_schema.tables t \
         LEFT JOIN pg_namespace n ON n.nspname = t.table_schema \
         LEFT JOIN pg_class c ON c.relnamespace = n.oid AND c.relname = t.table_name \
         WHERE {USER_SCHEMAS} AND t.table_name NOT LIKE 'pg\\_%' ORDER BY t.table_schema, t.table_name"
    );
    Ok(rows(client, &sql)?
        .into_iter()
        .map(|row| {
            let text = |index: usize| row.get(index).cloned().flatten().unwrap_or_default();
            let view = text(2).to_uppercase().contains("VIEW");
            Table {
                schema: text(0),
                name: text(1),
                kind: if view {
                    TableKind::View
                } else {
                    TableKind::Table
                },
                count: if view {
                    None
                } else {
                    text(3).parse::<i64>().ok().and_then(estimate)
                },
            }
        })
        .collect())
}

fn estimate(reltuples: i64) -> Option<usize> {
    usize::try_from(reltuples).ok()
}

/// A type as people write it: `character varying(255)` -> `varchar(255)`.
pub(crate) fn short_type(full: &str) -> String {
    const NAMES: [(&str, &str); 8] = [
        ("character varying", "varchar"),
        ("timestamp with time zone", "timestamptz"),
        ("timestamp without time zone", "timestamp"),
        ("time with time zone", "timetz"),
        ("time without time zone", "time"),
        ("double precision", "float8"),
        ("character", "char"),
        ("boolean", "bool"),
    ];
    for (long, short) in NAMES {
        if let Some(rest) = full.strip_prefix(long) {
            return format!("{short}{rest}");
        }
    }
    full.to_string()
}

pub(crate) fn columns(client: &mut Client) -> Result<Vec<Column>, String> {
    let sql = format!(
        "SELECT n.nspname, c.relname, a.attname, format_type(a.atttypid, a.atttypmod), \
           EXISTS (SELECT 1 FROM pg_constraint p WHERE p.conrelid = c.oid AND p.contype = 'p' \
                   AND a.attnum = ANY (p.conkey)), \
           (SELECT r.relname FROM pg_constraint f JOIN pg_class r ON r.oid = f.confrelid \
            WHERE f.conrelid = c.oid AND f.contype = 'f' AND a.attnum = ANY (f.conkey) LIMIT 1) \
         FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE a.attnum > 0 AND NOT a.attisdropped AND c.relkind IN ('r', 'v', 'm', 'p', 'f') \
           AND {USER_NAMESPACES} AND c.relname NOT LIKE 'pg\\_%' \
         ORDER BY n.nspname, c.relname, a.attnum"
    );
    Ok(rows(client, &sql)?
        .into_iter()
        .map(|row| {
            let text = |index: usize| row.get(index).cloned().flatten().unwrap_or_default();
            Column {
                schema: text(0),
                table: text(1),
                name: text(2),
                data_type: short_type(&text(3)),
                primary_key: text(4) == "t",
                references: row.get(5).cloned().flatten(),
            }
        })
        .collect())
}

/// The table's definition as SQL: columns with their types, defaults and NOT NULL, then its constraints and
/// indexes; a view's `CREATE VIEW`.
pub(crate) fn table_ddl(client: &mut Client, table: &Table) -> Result<String, String> {
    let schema = if table.schema.is_empty() {
        "public"
    } else {
        &table.schema
    };
    let relation = format!(
        "(SELECT c.oid FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
          WHERE n.nspname = {} AND c.relname = {})",
        literal(schema),
        literal(&table.name)
    );
    if table.kind == TableKind::View {
        let definition = rows(client, &format!("SELECT pg_get_viewdef({relation}, true)"))?;
        let body = definition
            .first()
            .and_then(|row| row.first().cloned().flatten())
            .ok_or_else(|| format!("{} was not found", table.qualified()))?;
        return Ok(format!(
            "CREATE VIEW {} AS\n{}",
            table.sql_name(),
            body.trim_end()
        ));
    }
    let columns = rows(
        client,
        &format!(
            "SELECT a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
               pg_get_expr(d.adbin, d.adrelid) \
             FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = {relation} AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum"
        ),
    )?;
    if columns.is_empty() {
        return Err(format!("{} was not found", table.qualified()));
    }
    let constraints = rows(
        client,
        &format!(
            "SELECT conname, pg_get_constraintdef(oid) FROM pg_constraint \
             WHERE conrelid = {relation} ORDER BY contype, conname"
        ),
    )?;
    let indexes = rows(
        client,
        &format!(
            "SELECT pg_get_indexdef(i.indexrelid) FROM pg_index i \
             WHERE i.indrelid = {relation} AND NOT i.indisprimary \
               AND NOT EXISTS (SELECT 1 FROM pg_constraint k WHERE k.conindid = i.indexrelid) \
             ORDER BY 1"
        ),
    )?;
    let text = |row: &Vec<Option<String>>, index: usize| {
        row.get(index).cloned().flatten().unwrap_or_default()
    };
    let mut lines: Vec<String> = columns
        .iter()
        .map(|row| {
            let mut line = format!(
                "    {} {}",
                crate::quote_identifier(&text(row, 0)),
                text(row, 1)
            );
            let default = text(row, 3);
            if !default.is_empty() {
                line.push_str(&format!(" DEFAULT {default}"));
            }
            if text(row, 2) == "t" {
                line.push_str(" NOT NULL");
            }
            line
        })
        .collect();
    lines.extend(constraints.iter().map(|row| {
        format!(
            "    CONSTRAINT {} {}",
            crate::quote_identifier(&text(row, 0)),
            text(row, 1)
        )
    }));
    let mut ddl = format!(
        "CREATE TABLE {} (\n{}\n);\n",
        table.sql_name(),
        lines.join(",\n")
    );
    for row in &indexes {
        ddl.push_str(&format!("{};\n", text(row, 0)));
    }
    Ok(ddl)
}

/// The last result set (or command) of the messages, at most `limit` rows.
fn collect(messages: Vec<SimpleQueryMessage>, limit: usize) -> QueryResult {
    let mut result = QueryResult::default();
    let mut returned_rows = false;
    for message in messages {
        match message {
            SimpleQueryMessage::RowDescription(columns) => {
                result = QueryResult {
                    columns: columns
                        .iter()
                        .map(|column| column.name().to_string())
                        .collect(),
                    ..QueryResult::default()
                };
                returned_rows = true;
            }
            SimpleQueryMessage::Row(row) => {
                if result.rows.len() >= limit {
                    result.truncated = true;
                    continue;
                }
                result.rows.push(
                    (0..row.len())
                        .map(|index| row.get(index).map(str::to_string))
                        .collect(),
                );
            }
            SimpleQueryMessage::CommandComplete(count) => {
                if !returned_rows {
                    result = QueryResult {
                        rows_affected: Some(count),
                        ..QueryResult::default()
                    };
                }
                returned_rows = false;
            }
            _ => {}
        }
    }
    result
}

pub(crate) fn query(client: &mut Client, sql: &str, limit: usize) -> Result<QueryResult, String> {
    let statement = sql.trim().trim_end_matches(';').trim_end();
    if CURSOR_KEYWORDS.contains(&first_keyword(statement).as_str()) {
        client
            .batch_execute("BEGIN")
            .map_err(|error| describe(&error))?;
        let declared = client.batch_execute(&format!(
            "DECLARE {CURSOR} NO SCROLL CURSOR FOR {statement}"
        ));
        match declared {
            Ok(()) => {
                let fetched = client.simple_query(&format!("FETCH {} FROM {CURSOR}", limit + 1));
                let closed = client.batch_execute("COMMIT");
                let messages = fetched.map_err(|error| describe(&error))?;
                closed.map_err(|error| describe(&error))?;
                let mut result = collect(messages, limit);
                result.rows_affected = None;
                return Ok(result);
            }
            // Not every statement starting like a query can be a cursor (a data-modifying WITH); run it plainly.
            Err(_) => client
                .batch_execute("ROLLBACK")
                .map_err(|error| describe(&error))?,
        }
    }
    let messages = client.simple_query(sql).map_err(|error| describe(&error))?;
    Ok(collect(messages, limit))
}

/// Streams `COPY (sql) TO STDOUT` into the file; empty CSV fields are NULL.
pub(crate) fn export_csv(client: &mut Client, sql: &str, path: &Path) -> Result<u64, String> {
    let statement = sql.trim().trim_end_matches(';').trim_end();
    let mut reader = client
        .copy_out(&format!(
            "COPY ({statement}) TO STDOUT WITH (FORMAT csv, HEADER true)"
        ))
        .map_err(|error| describe(&error))?;
    let mut file = std::io::BufWriter::new(
        std::fs::File::create(path).map_err(|error| format!("{}: {error}", path.display()))?,
    );
    let mut lines: u64 = 0;
    let mut quoted = false;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read =
            std::io::Read::read(&mut reader, &mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        // A newline inside a quoted field is data, not the end of a record.
        for byte in &buffer[..read] {
            match byte {
                b'"' => quoted = !quoted,
                b'\n' if !quoted => lines += 1,
                _ => {}
            }
        }
        file.write_all(&buffer[..read])
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    file.flush()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(lines.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_type_names_read_short() {
        assert_eq!(short_type("character varying(255)"), "varchar(255)");
        assert_eq!(short_type("timestamp with time zone"), "timestamptz");
        assert_eq!(short_type("character(64)"), "char(64)");
        assert_eq!(short_type("bigint"), "bigint");
        assert_eq!(estimate(-1), None);
        assert_eq!(estimate(1284), Some(1284));
    }
}
