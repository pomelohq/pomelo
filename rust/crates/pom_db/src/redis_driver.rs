//! Redis, the way the previous core browsed it: key prefixes as "keyspaces", a key pattern lists matching keys
//! with type, TTL and a value preview, anything else runs as a command.

use std::collections::BTreeMap;
use std::time::Duration;

use redis::{Connection, Value};

use crate::{QueryResult, Table, TableKind};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Keyspaces come from at most this many keys, so a huge instance still lists quickly.
const KEYSPACE_SCAN_MAX: usize = 20_000;
const PREVIEW_BYTES: usize = 400;
const PREVIEW_ITEMS: isize = 20;

pub(crate) fn connect(url: &str) -> Result<Connection, String> {
    let client = redis::Client::open(url).map_err(|error| error.to_string())?;
    let connection = client
        .get_connection_with_timeout(CONNECT_TIMEOUT)
        .map_err(|error| format!("connect to {url}: {error}"))?;
    for timeout in [
        connection.set_read_timeout(Some(COMMAND_TIMEOUT)),
        connection.set_write_timeout(Some(COMMAND_TIMEOUT)),
    ] {
        timeout.map_err(|error| error.to_string())?;
    }
    Ok(connection)
}

fn scan_page(
    connection: &mut Connection,
    cursor: u64,
    pattern: &str,
    count: usize,
) -> Result<(u64, Vec<String>), String> {
    redis::cmd("SCAN")
        .arg(cursor)
        .arg("MATCH")
        .arg(pattern)
        .arg("COUNT")
        .arg(count)
        .query(connection)
        .map_err(|error| error.to_string())
}

/// The prefix a key belongs to: what comes before its first `:`, or the key itself.
pub(crate) fn namespace(key: &str) -> &str {
    match key.find(':') {
        Some(at) if at > 0 => &key[..at],
        _ => key,
    }
}

pub(crate) fn keyspaces(connection: &mut Connection) -> Result<Vec<Table>, String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut cursor = 0;
    let mut seen = 0;
    loop {
        let (next, keys) = scan_page(connection, cursor, "*", 500)?;
        seen += keys.len();
        for key in &keys {
            *counts.entry(namespace(key).to_string()).or_default() += 1;
        }
        cursor = next;
        if cursor == 0 || seen >= KEYSPACE_SCAN_MAX {
            break;
        }
    }
    Ok(counts
        .into_iter()
        .map(|(name, count)| Table {
            schema: String::new(),
            name,
            kind: TableKind::Keyspace,
            count: Some(count),
        })
        .collect())
}

/// A key pattern, not a command: it has a glob character, or is one word with a `:`.
pub(crate) fn looks_like_pattern(text: &str) -> bool {
    text.contains(['*', '?', '[']) || (!text.contains(' ') && text.contains(':'))
}

pub(crate) fn query(
    connection: &mut Connection,
    text: &str,
    limit: usize,
) -> Result<QueryResult, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(QueryResult::default());
    }
    if looks_like_pattern(text) {
        return scan(connection, text, limit);
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut command = redis::cmd(words[0]);
    for word in &words[1..] {
        command.arg(*word);
    }
    let value: Value = command
        .query(connection)
        .map_err(|error| error.to_string())?;
    Ok(QueryResult {
        columns: vec!["result".into()],
        rows: vec![vec![Some(display(&value))]],
        ..QueryResult::default()
    })
}

fn scan(connection: &mut Connection, pattern: &str, limit: usize) -> Result<QueryResult, String> {
    let mut result = QueryResult {
        columns: ["key", "type", "ttl", "value"].map(str::to_string).to_vec(),
        ..QueryResult::default()
    };
    let mut cursor = 0;
    loop {
        let (next, keys) = scan_page(connection, cursor, pattern, 200)?;
        for key in keys {
            if result.rows.len() >= limit {
                result.truncated = true;
                return Ok(result);
            }
            let kind: String = redis::cmd("TYPE")
                .arg(&key)
                .query(connection)
                .map_err(|error| error.to_string())?;
            let ttl: i64 = redis::cmd("TTL")
                .arg(&key)
                .query(connection)
                .map_err(|error| error.to_string())?;
            let preview = preview(connection, &key, &kind)?;
            result.rows.push(vec![
                Some(key),
                Some(kind),
                Some(if ttl > 0 { format_ttl(ttl) } else { "-".into() }),
                Some(preview),
            ]);
        }
        cursor = next;
        if cursor == 0 {
            return Ok(result);
        }
    }
}

/// `3723` -> `1h2m3s`.
pub(crate) fn format_ttl(seconds: i64) -> String {
    let (hours, minutes, rest) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    let mut text = String::new();
    if hours > 0 {
        text.push_str(&format!("{hours}h"));
    }
    if hours > 0 || minutes > 0 {
        text.push_str(&format!("{minutes}m"));
    }
    text.push_str(&format!("{rest}s"));
    text
}

fn preview(connection: &mut Connection, key: &str, kind: &str) -> Result<String, String> {
    let fetch = |command: &mut redis::Cmd, connection: &mut Connection| -> Result<Value, String> {
        command.query(connection).map_err(|error| error.to_string())
    };
    let value = match kind {
        "string" => fetch(redis::cmd("GET").arg(key), connection)?,
        "list" => fetch(
            redis::cmd("LRANGE").arg(key).arg(0).arg(PREVIEW_ITEMS),
            connection,
        )?,
        "set" => fetch(redis::cmd("SMEMBERS").arg(key), connection)?,
        "zset" => fetch(
            redis::cmd("ZRANGE").arg(key).arg(0).arg(PREVIEW_ITEMS),
            connection,
        )?,
        "hash" => fetch(redis::cmd("HGETALL").arg(key), connection)?,
        _ => return Ok(String::new()),
    };
    let text = if kind == "hash" {
        pairs(&value)
    } else {
        display(&value)
    };
    Ok(truncate(&text))
}

fn truncate(text: &str) -> String {
    if text.len() <= PREVIEW_BYTES {
        return text.to_string();
    }
    let mut end = PREVIEW_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// A hash reply (`f1 v1 f2 v2`) as sorted `f=v` pairs.
fn pairs(value: &Value) -> String {
    let items: Vec<String> = match value {
        Value::Array(items) => items.iter().map(display).collect(),
        Value::Map(entries) => entries
            .iter()
            .flat_map(|(field, value)| [display(field), display(value)])
            .collect(),
        other => return display(other),
    };
    let mut fields: Vec<String> = items
        .chunks(2)
        .map(|pair| format!("{}={}", pair[0], pair.get(1).cloned().unwrap_or_default()))
        .collect();
    fields.sort();
    fields.join(", ")
}

/// A reply as text: nil as `(nil)`, arrays joined with `, `, maps as sorted `k=v`.
pub(crate) fn display(value: &Value) -> String {
    match value {
        Value::Nil => "(nil)".into(),
        Value::Int(number) => number.to_string(),
        Value::BulkString(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        Value::SimpleString(text) => text.clone(),
        Value::Okay => "OK".into(),
        Value::Double(number) => number.to_string(),
        Value::Boolean(flag) => flag.to_string(),
        Value::Array(items) | Value::Set(items) => {
            items.iter().map(display).collect::<Vec<_>>().join(", ")
        }
        Value::Map(entries) => {
            let mut fields: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("{}={}", display(key), display(value)))
                .collect();
            fields.sort();
            fields.join(", ")
        }
        Value::ServerError(error) => format!("error: {}", error.details().unwrap_or_default()),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_group_by_prefix_and_patterns_are_told_from_commands() {
        assert_eq!(namespace("user:1:name"), "user");
        assert_eq!(namespace(":odd"), ":odd");
        assert_eq!(namespace("plain"), "plain");
        assert!(looks_like_pattern("user:*"));
        assert!(looks_like_pattern("session:abc"));
        assert!(!looks_like_pattern("GET user:1"));
        assert!(!looks_like_pattern("DBSIZE"));
    }

    #[test]
    fn replies_read_like_the_previous_core() {
        assert_eq!(format_ttl(3723), "1h2m3s");
        assert_eq!(format_ttl(59), "59s");
        assert_eq!(format_ttl(120), "2m0s");
        let hash = Value::Array(
            ["b", "2", "a", "1"]
                .iter()
                .map(|text| Value::BulkString(text.as_bytes().to_vec()))
                .collect(),
        );
        assert_eq!(pairs(&hash), "a=1, b=2");
        assert_eq!(display(&Value::Nil), "(nil)");
        assert_eq!(display(&hash), "b, 2, a, 1");
        assert_eq!(truncate(&"x".repeat(500)).len(), 403);
    }
}
