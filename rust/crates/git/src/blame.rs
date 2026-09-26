//! Who last changed each line: `git blame --incremental` over the buffer's current text, and the relative
//! timestamps shown beside a line ("3 days ago").

use std::collections::HashMap;
use std::path::Path;

use time::{OffsetDateTime, UtcOffset};

/// A run of lines last changed by one commit. Lines not committed yet have no entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlameEntry {
    pub sha: String,
    /// First buffer line (0-based) and how many follow.
    pub start_row: usize,
    pub line_count: usize,
    pub author: Option<String>,
    pub author_time: Option<i64>,
    pub summary: Option<String>,
}

/// Blame the file as `contents` reads (unsaved edits count as not committed). `None` outside a repository.
pub fn blame(path: &Path, contents: &str) -> Option<Vec<BlameEntry>> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let output = crate::git_with_input(
        dir,
        &["blame", "--incremental", "--contents", "-", "--", name],
        contents.as_bytes(),
    )?;
    Some(parse_incremental(&String::from_utf8_lossy(&output)))
}

/// Each entry starts `<sha> <source line> <result line> <count>`; the commit's details follow only the first
/// time its sha appears, and every entry ends with `filename`.
fn parse_incremental(output: &str) -> Vec<BlameEntry> {
    let mut entries = Vec::new();
    let mut known: HashMap<String, BlameEntry> = HashMap::new();
    let mut current: Option<BlameEntry> = None;
    for line in output.lines() {
        let Some(entry) = current.as_mut() else {
            let mut fields = line.split(' ');
            let (Some(sha), Some(_), Some(result_line), Some(count)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(result_line), Ok(count)) = (result_line.parse::<usize>(), count.parse()) else {
                continue;
            };
            let mut entry = known.get(sha).cloned().unwrap_or_default();
            entry.sha = sha.to_string();
            entry.start_row = result_line.saturating_sub(1);
            entry.line_count = count;
            current = Some(entry);
            continue;
        };
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let committed = entry.sha.bytes().any(|b| b != b'0');
        match key {
            "author" if committed => entry.author = Some(value.to_string()),
            "author-time" if committed => entry.author_time = value.parse().ok(),
            "summary" if committed => entry.summary = Some(value.to_string()),
            "filename" => {
                if let Some(done) = current.take() {
                    if done.sha.bytes().any(|b| b != b'0') {
                        known.insert(done.sha.clone(), done.clone());
                        entries.push(done);
                    }
                }
            }
            _ => {}
        }
    }
    entries.sort_by_key(|entry| entry.start_row);
    entries
}

/// The entry covering buffer line `row`.
pub fn entry_for_row(entries: &[BlameEntry], row: usize) -> Option<&BlameEntry> {
    let index = entries.partition_point(|entry| entry.start_row <= row);
    let entry = entries.get(index.checked_sub(1)?)?;
    (row < entry.start_row + entry.line_count).then_some(entry)
}

/// `author, when`, the way the line annotation reads.
pub fn inline_text(entry: &BlameEntry) -> String {
    let when = entry
        .author_time
        .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok())
        .map(|t| {
            let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
            relative_timestamp(
                t.to_offset(offset),
                OffsetDateTime::now_utc().to_offset(offset),
            )
        })
        .unwrap_or_else(|| "Error parsing date".to_string());
    format!("{}, {when}", entry.author.as_deref().unwrap_or_default())
}

/// Minutes and hours within the day, then days, weeks, months, and years (years with months under five).
pub fn relative_timestamp(timestamp: OffsetDateTime, reference: OffsetDateTime) -> String {
    let minutes = (reference - timestamp).whole_minutes();
    match minutes {
        0 => return "Just now".to_string(),
        1 => return "1 minute ago".to_string(),
        2..=59 => return format!("{minutes} minutes ago"),
        _ => {}
    }
    let hours = (reference - timestamp).whole_hours();
    match hours {
        1 => return "1 hour ago".to_string(),
        2..=23 => return format!("{hours} hours ago"),
        _ => {}
    }
    let days = (reference.date() - timestamp.date()).whole_days();
    match days {
        0 => return "Today".to_string(),
        1 => return "Yesterday".to_string(),
        2..=6 => return format!("{days} days ago"),
        _ => {}
    }
    let weeks = (reference.date() - timestamp.date()).whole_weeks();
    match weeks {
        1 => return "1 week ago".to_string(),
        2..=4 => return format!("{weeks} weeks ago"),
        _ => {}
    }
    let months = month_difference(timestamp, reference);
    match months {
        0..=1 => "1 month ago".to_string(),
        2..=11 => format!("{months} months ago"),
        12..60 => {
            let (years, months) = (months / 12, months % 12);
            let year_unit = if years == 1 { "year" } else { "years" };
            match months {
                0 => format!("{years} {year_unit} ago"),
                1 => format!("{years} {year_unit}, 1 month ago"),
                _ => format!("{years} {year_unit}, {months} months ago"),
            }
        }
        _ => format!("{} years ago", (months + 6) / 12),
    }
}

fn month_difference(timestamp: OffsetDateTime, reference: OffsetDateTime) -> usize {
    let (from_month, to_month) = (
        u8::from(timestamp.month()) as i64,
        u8::from(reference.month()) as i64,
    );
    let years = (reference.year() - timestamp.year()) as i64;
    (years * 12 + to_month - from_month).max(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn parses_incremental_output_reusing_commit_details() {
        let output = "\
aaaa000000000000000000000000000000000001 1 1 2
author Ann
author-time 1709741400
summary First
filename a.txt
0000000000000000000000000000000000000000 3 3 1
author External file (--contents)
filename a.txt
aaaa000000000000000000000000000000000001 4 4 1
filename a.txt
";
        let entries = parse_incremental(output);
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].start_row, entries[0].line_count), (0, 2));
        assert_eq!(entries[1].author.as_deref(), Some("Ann"));
        assert_eq!(entry_for_row(&entries, 1).map(|e| e.start_row), Some(0));
        assert_eq!(entry_for_row(&entries, 2), None);
        assert_eq!(entry_for_row(&entries, 3).map(|e| e.start_row), Some(3));
    }

    #[test]
    fn relative_timestamps_step_through_units() {
        let now = datetime!(2026-09-23 12:00 UTC);
        let at = |t: OffsetDateTime| relative_timestamp(t, now);
        assert_eq!(at(datetime!(2026-09-23 11:59:30 UTC)), "Just now");
        assert_eq!(at(datetime!(2026-09-23 11:15 UTC)), "45 minutes ago");
        assert_eq!(at(datetime!(2026-09-23 07:00 UTC)), "5 hours ago");
        assert_eq!(at(datetime!(2026-09-22 08:00 UTC)), "Yesterday");
        assert_eq!(at(datetime!(2026-09-19 08:00 UTC)), "4 days ago");
        assert_eq!(at(datetime!(2026-09-09 08:00 UTC)), "2 weeks ago");
        assert_eq!(at(datetime!(2026-06-01 08:00 UTC)), "3 months ago");
        assert_eq!(at(datetime!(2024-11-01 08:00 UTC)), "1 year, 10 months ago");
        assert_eq!(at(datetime!(2019-01-01 08:00 UTC)), "8 years ago");
    }
}
