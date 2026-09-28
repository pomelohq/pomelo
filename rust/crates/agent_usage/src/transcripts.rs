use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// One reply of an agent: what it read and wrote, in tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    pub session: String,
    pub cwd: PathBuf,
    pub model: String,
    /// Unix seconds.
    pub at: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Turn {
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    /// The context the turn was answered with: everything it read.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

#[derive(Default)]
struct FileState {
    read_to: u64,
    turns: Vec<Turn>,
    /// Message ids already counted: a reply is written once per content block, each with its usage.
    seen: HashSet<String>,
}

/// The turns of every Claude Code transcript, read incrementally: a file is only read past where it was
/// read last time.
#[derive(Default)]
pub struct Transcripts {
    files: HashMap<PathBuf, FileState>,
}

impl Transcripts {
    /// Reads what was added to the transcripts under `projects` touched since `since` (unix seconds).
    pub fn refresh(&mut self, projects: &Path, since: u64) {
        let Ok(folders) = std::fs::read_dir(projects) else {
            return;
        };
        let mut alive = HashSet::new();
        for folder in folders.filter_map(Result::ok) {
            let Ok(entries) = std::fs::read_dir(folder.path()) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path
                    .extension()
                    .is_none_or(|extension| extension != "jsonl")
                {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |elapsed| elapsed.as_secs());
                if modified < since {
                    continue;
                }
                alive.insert(path.clone());
                let state = self.files.entry(path.clone()).or_default();
                if meta.len() < state.read_to {
                    *state = FileState::default();
                }
                if meta.len() > state.read_to {
                    if let Err(error) = read_from(&path, state) {
                        eprintln!("agent usage: read {}: {error}", path.display());
                    }
                }
            }
        }
        self.files.retain(|path, _| alive.contains(path));
    }

    /// Every turn read so far that happened at or after `since`.
    pub fn turns(&self, since: u64) -> Vec<&Turn> {
        self.files
            .values()
            .flat_map(|state| state.turns.iter())
            .filter(|turn| turn.at >= since)
            .collect()
    }
}

fn read_from(path: &Path, state: &mut FileState) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(state.read_to))?;
    let session = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut reader = BufReader::new(file.take(u64::MAX));
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        // A line still being written has no newline yet; it is read again next time.
        if read == 0 || !line.ends_with('\n') {
            break;
        }
        state.read_to += read as u64;
        if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
            continue;
        }
        if let Some((id, turn)) = parse_turn(&line, &session) {
            if state.seen.insert(id) {
                state.turns.push(turn);
            }
        }
    }
    Ok(())
}

fn parse_turn(line: &str, session: &str) -> Option<(String, Turn)> {
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = entry.get("message")?;
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if model.starts_with('<') {
        return None;
    }
    let usage = message.get("usage")?;
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    let parts: Vec<&Value> = match usage.get("iterations").and_then(Value::as_array) {
        Some(iterations) if !iterations.is_empty() => iterations.iter().collect(),
        _ => vec![usage],
    };
    let sum = |key: &str| parts.iter().map(|part| number(part, key)).sum::<u64>();
    let id = message
        .get("id")
        .or_else(|| entry.get("requestId"))
        .or_else(|| entry.get("uuid"))
        .and_then(Value::as_str)?
        .to_string();
    let turn = Turn {
        session: entry
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or(session)
            .to_string(),
        cwd: PathBuf::from(entry.get("cwd").and_then(Value::as_str).unwrap_or_default()),
        model: model.to_string(),
        at: entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(crate::parse_timestamp)
            .unwrap_or(0),
        input: sum("input_tokens"),
        output: sum("output_tokens"),
        cache_read: sum("cache_read_input_tokens"),
        cache_write: sum("cache_creation_input_tokens"),
    };
    Some((id, turn))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, at: &str, usage: &str) -> String {
        format!(
            "{{\"type\":\"assistant\",\"sessionId\":\"s1\",\"cwd\":\"/work/ws\",\"timestamp\":\"{at}\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-opus-5-5\",\"usage\":{usage}}}}}\n"
        )
    }

    #[test]
    fn turns_are_counted_once_and_read_incrementally() {
        let temp = tempfile::tempdir().expect("tempdir");
        let folder = temp.path().join("-work-ws");
        std::fs::create_dir_all(&folder).expect("folder");
        let file = folder.join("s1.jsonl");
        let plain = r#"{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":1000,"cache_creation_input_tokens":20}"#;
        let nested = r#"{"input_tokens":0,"output_tokens":0,"iterations":[{"input_tokens":2,"output_tokens":7,"cache_read_input_tokens":500,"cache_creation_input_tokens":3}]}"#;
        let text = line("m1", "2026-09-28T06:00:00Z", plain)
            + &line("m1", "2026-09-28T06:00:00Z", plain)
            + "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n";
        std::fs::write(&file, &text).expect("write");
        let mut transcripts = Transcripts::default();
        transcripts.refresh(temp.path(), 0);
        let turns = transcripts.turns(0);
        assert_eq!(turns.len(), 1, "a reply's blocks share one usage");
        assert_eq!(turns[0].tokens(), 1035);
        assert_eq!(turns[0].cwd, PathBuf::from("/work/ws"));

        let more = text + &line("m2", "2026-09-28T07:00:00Z", nested) + "{\"type\":\"assist";
        std::fs::write(&file, &more).expect("append");
        transcripts.refresh(temp.path(), 0);
        let mut turns = transcripts.turns(0);
        turns.sort_by_key(|turn| turn.at);
        assert_eq!(turns.len(), 2);
        assert_eq!(
            (turns[1].input, turns[1].output, turns[1].cache_read),
            (2, 7, 500)
        );
        assert_eq!(
            transcripts.turns(1_790_577_000).len(),
            1,
            "filtered by time"
        );
    }
}
