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

/// Deletes every key matching `pattern` (UNLINK, a page of the scan at a time); answers how many went.
pub(crate) fn delete_matching(connection: &mut Connection, pattern: &str) -> Result<u64, String> {
    let mut deleted = 0;
    let mut cursor = 0;
    loop {
        let (next, keys) = scan_page(connection, cursor, pattern, 500)?;
        if !keys.is_empty() {
            let removed: u64 = redis::cmd("UNLINK")
                .arg(&keys)
                .query(connection)
                .map_err(|error| error.to_string())?;
            deleted += removed;
        }
        cursor = next;
        if cursor == 0 {
            return Ok(deleted);
        }
    }
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

/// A key as a keyspace tab lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedisKey {
    pub key: String,
    pub kind: String,
    /// Seconds left; `None` when the key never expires.
    pub ttl: Option<i64>,
}

/// A key's value read the way its type stores it; lists and sets stop at `VALUE_ITEMS`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedisValue {
    Text(String),
    Hash(Vec<(String, String)>),
    List(Vec<String>),
    Set(Vec<String>),
    Sorted(Vec<(String, String)>),
    Missing,
    Other(String),
}

/// How many items a list, set, hash or sorted set is read up to.
pub const VALUE_ITEMS: usize = 1000;

/// Keys matching `pattern` with their type and TTL, at most `limit` (the second part says more exist).
pub(crate) fn keys(
    connection: &mut Connection,
    pattern: &str,
    limit: usize,
) -> Result<(Vec<RedisKey>, bool), String> {
    let mut found = Vec::new();
    let mut cursor = 0;
    loop {
        let (next, page) = scan_page(connection, cursor, pattern, 500)?;
        for key in page {
            if found.len() >= limit {
                return Ok((found, true));
            }
            let kind: String = redis::cmd("TYPE")
                .arg(&key)
                .query(connection)
                .map_err(|error| error.to_string())?;
            let ttl: i64 = redis::cmd("TTL")
                .arg(&key)
                .query(connection)
                .map_err(|error| error.to_string())?;
            found.push(RedisKey {
                key,
                kind,
                ttl: (ttl >= 0).then_some(ttl),
            });
        }
        cursor = next;
        if cursor == 0 {
            found.sort_by(|a, b| a.key.cmp(&b.key));
            return Ok((found, false));
        }
    }
}

fn texts(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) | Value::Set(items) => items.iter().map(display).collect(),
        Value::Nil => Vec::new(),
        other => vec![display(other)],
    }
}

fn paired(value: &Value) -> Vec<(String, String)> {
    match value {
        Value::Map(entries) => entries
            .iter()
            .map(|(key, value)| (display(key), display(value)))
            .collect(),
        other => texts(other)
            .chunks(2)
            .map(|pair| (pair[0].clone(), pair.get(1).cloned().unwrap_or_default()))
            .collect(),
    }
}

pub(crate) fn value(connection: &mut Connection, key: &str) -> Result<RedisValue, String> {
    let run = |command: &mut redis::Cmd, connection: &mut Connection| -> Result<Value, String> {
        command.query(connection).map_err(|error| error.to_string())
    };
    let kind: String = redis::cmd("TYPE")
        .arg(key)
        .query(connection)
        .map_err(|error| error.to_string())?;
    let last = VALUE_ITEMS as isize - 1;
    Ok(match kind.as_str() {
        "none" => RedisValue::Missing,
        "string" => RedisValue::Text(display(&run(redis::cmd("GET").arg(key), connection)?)),
        "list" => RedisValue::List(texts(&run(
            redis::cmd("LRANGE").arg(key).arg(0).arg(last),
            connection,
        )?)),
        "set" => {
            let mut members = texts(&run(redis::cmd("SMEMBERS").arg(key), connection)?);
            members.sort();
            members.truncate(VALUE_ITEMS);
            RedisValue::Set(members)
        }
        "zset" => RedisValue::Sorted(paired(&run(
            redis::cmd("ZRANGE")
                .arg(key)
                .arg(0)
                .arg(last)
                .arg("WITHSCORES"),
            connection,
        )?)),
        "hash" => {
            let mut fields = paired(&run(redis::cmd("HGETALL").arg(key), connection)?);
            fields.truncate(VALUE_ITEMS);
            RedisValue::Hash(fields)
        }
        other => RedisValue::Other(format!("a {other} key; read it in redis-cli")),
    })
}

/// Sets the key's TTL in seconds, or makes it permanent with `None`.
pub(crate) fn expire(
    connection: &mut Connection,
    key: &str,
    seconds: Option<i64>,
) -> Result<(), String> {
    let command = match seconds {
        Some(seconds) => redis::cmd("EXPIRE").arg(key).arg(seconds).clone(),
        None => redis::cmd("PERSIST").arg(key).clone(),
    };
    command
        .query::<Value>(connection)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn delete(connection: &mut Connection, key: &str) -> Result<(), String> {
    redis::cmd("UNLINK")
        .arg(key)
        .query::<u64>(connection)
        .map(|_| ())
        .map_err(|error| error.to_string())
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
