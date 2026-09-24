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
    config
        .connect(NoTls)
        .map_err(|error| format!("connect to {database}: {}", describe(&error)))
}

fn describe(error: &postgres::Error) -> String {
    match error.as_db_error() {
        Some(db) => db.message().to_string(),
        None => error.to_string(),
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

pub(crate) fn tables(client: &mut Client) -> Result<Vec<Table>, String> {
    let sql = format!(
        "SELECT table_schema, table_name, table_type FROM information_schema.tables \
         WHERE {USER_SCHEMAS} AND table_name NOT LIKE 'pg\\_%' ORDER BY table_schema, table_name"
    );
    Ok(rows(client, &sql)?
        .into_iter()
        .map(|row| {
            let text = |index: usize| row.get(index).cloned().flatten().unwrap_or_default();
            Table {
                schema: text(0),
                name: text(1),
                kind: if text(2).to_uppercase().contains("VIEW") {
                    TableKind::View
                } else {
                    TableKind::Table
                },
                count: None,
            }
        })
        .collect())
}

pub(crate) fn columns(client: &mut Client) -> Result<Vec<Column>, String> {
    let sql = format!(
        "SELECT table_schema, table_name, column_name, data_type FROM information_schema.columns \
         WHERE {USER_SCHEMAS} AND table_name NOT LIKE 'pg\\_%' \
         ORDER BY table_schema, table_name, ordinal_position"
    );
    Ok(rows(client, &sql)?
        .into_iter()
        .map(|row| {
            let text = |index: usize| row.get(index).cloned().flatten().unwrap_or_default();
            Column {
                schema: text(0),
                table: text(1),
                name: text(2),
                data_type: text(3),
            }
        })
        .collect())
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
