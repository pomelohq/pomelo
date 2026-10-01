use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const MAX_LOG_LINES: usize = 2000;

/// A server's recent output: what it wrote to stderr and the log messages it sent, oldest dropped first.
#[derive(Default)]
pub struct ServerLog {
    lines: VecDeque<String>,
    total: u64,
    /// A log tab shows it, so a new stderr line wakes the UI.
    pub watched: bool,
}

pub type SharedLog = Arc<Mutex<ServerLog>>;

impl ServerLog {
    pub fn push(&mut self, line: String) {
        if self.lines.len() >= MAX_LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
        self.total += 1;
    }

    /// The lines pushed after the first `seen` (those still kept), and how many were pushed in all.
    pub fn since(&self, seen: u64) -> (Vec<String>, u64) {
        let fresh = (self.total.saturating_sub(seen) as usize).min(self.lines.len());
        let skip = self.lines.len() - fresh;
        (self.lines.iter().skip(skip).cloned().collect(), self.total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_gets_only_what_it_has_not_seen_and_old_lines_drop() {
        let mut log = ServerLog::default();
        log.push("a".into());
        log.push("b".into());
        let (lines, seen) = log.since(0);
        assert_eq!((lines, seen), (vec!["a".to_string(), "b".to_string()], 2));
        log.push("c".into());
        assert_eq!(log.since(seen), (vec!["c".to_string()], 3));
        for index in 0..MAX_LOG_LINES {
            log.push(index.to_string());
        }
        let (lines, _) = log.since(0);
        assert_eq!(lines.len(), MAX_LOG_LINES);
        assert_eq!(lines.first().map(String::as_str), Some("0"));
    }
}
