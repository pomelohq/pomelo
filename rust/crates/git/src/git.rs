//! A file's uncommitted changes: its text at HEAD and in the index, read through the git CLI, diffed line by
//! line against the buffer into hunks. A hunk is staged when the index already holds it, i.e. no difference
//! between the index and the buffer overlaps it.

use std::ops::Range;
use std::path::Path;
use std::process::{Command, Stdio};

use imara_diff::{sources::lines, Algorithm, Diff, InternedInput};

mod blame;
mod branch_changes;
pub use blame::{blame, entry_for_row, inline_text, relative_timestamp, BlameEntry};
pub use branch_changes::{
    branch_changes, discard_uncommitted, ChangeStatus, FileChange, RepoChanges,
};

/// What the buffer is compared against. `None` means the file has no version there (new, or not staged).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffBases {
    pub head: Option<String>,
    pub index: Option<String>,
}

fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn text_of(bytes: Vec<u8>) -> Option<String> {
    let text = String::from_utf8(bytes).ok()?;
    // Buffers hold `\n` line breaks only, so the bases must too or every line would differ.
    Some(if text.contains('\r') {
        text.replace("\r\n", "\n")
    } else {
        text
    })
}

/// The file's HEAD and index text, or `None` when it isn't inside a git work tree.
pub fn load_bases(path: &Path) -> Option<DiffBases> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let inside = git(dir, &["rev-parse", "--is-inside-work-tree"])?;
    if inside.trim_ascii() != b"true" {
        return None;
    }
    let show = |spec: String| git(dir, &["show", &spec]).and_then(text_of);
    Some(DiffBases {
        head: show(format!("HEAD:./{name}")),
        index: show(format!(":./{name}")),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    /// Buffer lines the hunk covers; empty (at the line after the removal) for a deletion.
    pub rows: Range<usize>,
    /// Lines of the base it replaces.
    pub base_rows: Range<usize>,
    pub kind: HunkKind,
    pub staged: bool,
}

fn line_count(text: &str) -> usize {
    lines(text).count()
}

/// Changed line ranges between `base` and `text`: (base lines, text lines).
pub fn line_hunks(base: &str, text: &str) -> Vec<(Range<usize>, Range<usize>)> {
    let input = InternedInput::new(lines(base), lines(text));
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    // Slide ambiguous hunks to where git would put them, so the HEAD and index diffs agree on placement.
    diff.postprocess_lines(&input);
    diff.hunks()
        .map(|hunk| {
            (
                hunk.before.start as usize..hunk.before.end as usize,
                hunk.after.start as usize..hunk.after.end as usize,
            )
        })
        .collect()
}

/// Without a base the whole text is one addition.
fn hunks_against(base: Option<&str>, text: &str) -> Vec<(Range<usize>, Range<usize>)> {
    match base {
        Some(base) => line_hunks(base, text),
        None => {
            let count = line_count(text);
            if count == 0 {
                Vec::new()
            } else {
                vec![(0..0, 0..count)]
            }
        }
    }
}

fn rows_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => a.start == b.start,
        (true, false) => b.start <= a.start && a.start <= b.end,
        (false, true) => a.start <= b.start && b.start <= a.end,
        (false, false) => a.start < b.end && b.start < a.end,
    }
}

/// The buffer's changes since HEAD, each marked staged when the index already has it.
pub fn uncommitted_hunks(bases: &DiffBases, text: &str) -> Vec<DiffHunk> {
    let unstaged = hunks_against(bases.index.as_deref(), text);
    hunks_against(bases.head.as_deref(), text)
        .into_iter()
        .map(|(base_rows, rows)| {
            let kind = if rows.is_empty() {
                HunkKind::Deleted
            } else if base_rows.is_empty() {
                HunkKind::Added
            } else {
                HunkKind::Modified
            };
            let staged = !unstaged
                .iter()
                .any(|(_, unstaged_rows)| rows_overlap(unstaged_rows, &rows));
            DiffHunk {
                rows,
                base_rows,
                kind,
                staged,
            }
        })
        .collect()
}

/// Lines `rows` of `text`, line breaks included.
pub fn line_text(text: &str, rows: Range<usize>) -> String {
    lines(text).skip(rows.start).take(rows.len()).collect()
}

/// What the index should hold after staging (or unstaging) `hunks`: `None` when nothing changes,
/// `Some(None)` to drop the file from the index, else its new text. Each hunk maps into the index through the
/// changes the index doesn't have yet; overlapping unstaged changes are taken along, like git does.
pub fn index_after(
    bases: &DiffBases,
    text: &str,
    hunks: &[DiffHunk],
    stage: bool,
) -> Option<Option<String>> {
    let acted: Vec<&DiffHunk> = hunks.iter().filter(|hunk| hunk.staged != stage).collect();
    if acted.is_empty() {
        return None;
    }
    let (Some(index), Some(head)) = (bases.index.as_deref(), bases.head.as_deref()) else {
        // Without both versions the whole file goes in or comes out.
        return Some(if stage {
            Some(text.to_string())
        } else {
            bases.head.clone()
        });
    };
    let unstaged = line_hunks(index, text);
    let index_lines = line_count(index);
    let mut next_unstaged = 0;
    let (mut prev_buffer_end, mut prev_index_end) = (0, 0);
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut pending = acted.into_iter().peekable();
    while let Some(hunk) = pending.next() {
        while let Some((index_rows, buffer_rows)) = unstaged.get(next_unstaged) {
            if buffer_rows.end >= hunk.rows.start {
                break;
            }
            prev_index_end = index_rows.end;
            prev_buffer_end = buffer_rows.end;
            next_unstaged += 1;
        }
        let mut rows = hunk.rows.clone();
        let mut index_start = prev_index_end + rows.start.saturating_sub(prev_buffer_end);
        loop {
            if let Some((index_rows, buffer_rows)) = unstaged.get(next_unstaged) {
                if buffer_rows.start <= rows.end {
                    prev_index_end = index_rows.end;
                    prev_buffer_end = buffer_rows.end;
                    index_start = index_start.min(index_rows.start);
                    rows.start = rows.start.min(buffer_rows.start);
                    rows.end = rows.end.max(buffer_rows.end);
                    next_unstaged += 1;
                    continue;
                }
            }
            if let Some(next) = pending.next_if(|next| next.rows.start <= rows.end) {
                rows.end = rows.end.max(next.rows.end);
                continue;
            }
            break;
        }
        let index_end =
            (prev_index_end + rows.end.saturating_sub(prev_buffer_end)).min(index_lines);
        let index_start = index_start.min(index_end);
        let replacement = if stage {
            line_text(text, rows)
        } else {
            line_text(head, hunk.base_rows.clone())
        };
        match edits.last_mut() {
            Some((last, last_text)) if index_start <= last.end => {
                last.end = last.end.max(index_end);
                last_text.push_str(&replacement);
            }
            _ => edits.push((index_start..index_end, replacement)),
        }
    }
    let index_rows: Vec<&str> = lines(index).collect();
    let mut out = String::with_capacity(index.len());
    let mut row = 0;
    for (range, replacement) in edits {
        out.extend(index_rows.iter().take(range.start).skip(row).copied());
        out.push_str(&replacement);
        row = range.end;
    }
    out.extend(index_rows.iter().skip(row).copied());
    Some(Some(out))
}

pub(crate) fn git_with_input(dir: &Path, args: &[&str], input: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(input).ok()?;
    let output = child.wait_with_output().ok()?;
    output.status.success().then_some(output.stdout)
}

/// Put `text` in the index as the file's staged version, or drop the file from the index for `None`.
pub fn write_index(path: &Path, text: Option<&str>) -> Result<(), String> {
    let dir = path.parent().ok_or("no parent directory")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("unreadable file name")?;
    let Some(text) = text else {
        return git(dir, &["update-index", "--force-remove", "--", name])
            .map(|_| ())
            .ok_or_else(|| format!("could not unstage {name}"));
    };
    let hash = git_with_input(dir, &["hash-object", "-w", "--stdin"], text.as_bytes())
        .ok_or_else(|| format!("could not store {name}"))?;
    let hash = String::from_utf8_lossy(&hash).trim().to_string();
    // Keep an executable bit the index already records.
    let mode = git(dir, &["ls-files", "-s", "--", name])
        .and_then(|listing| {
            String::from_utf8_lossy(&listing)
                .split_whitespace()
                .next()
                .map(str::to_string)
        })
        .unwrap_or_else(|| "100644".to_string());
    // `--cacheinfo` paths are relative to the repository root, not the working directory.
    let prefix = git(dir, &["rev-parse", "--show-prefix"])
        .map(|prefix| String::from_utf8_lossy(&prefix).trim().to_string())
        .unwrap_or_default();
    let info = format!("{mode},{hash},{prefix}{name}");
    git(dir, &["update-index", "--add", "--cacheinfo", &info])
        .map(|_| ())
        .ok_or_else(|| format!("could not stage {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bases(head: Option<&str>, index: Option<&str>) -> DiffBases {
        DiffBases {
            head: head.map(str::to_string),
            index: index.map(str::to_string),
        }
    }

    fn summary(hunks: &[DiffHunk]) -> Vec<(Range<usize>, HunkKind, bool)> {
        hunks
            .iter()
            .map(|h| (h.rows.clone(), h.kind, h.staged))
            .collect()
    }

    #[test]
    fn classifies_added_modified_and_deleted_lines() {
        let head = "a\nb\nc\nd\n";
        let text = "a\nB\nc\nnew\n";
        let hunks = uncommitted_hunks(&bases(Some(head), Some(head)), text);
        assert_eq!(
            summary(&hunks),
            vec![
                (1..2, HunkKind::Modified, false),
                (3..4, HunkKind::Modified, false)
            ]
        );
        let hunks = uncommitted_hunks(&bases(Some(head), Some(head)), "a\nc\nd\n");
        assert_eq!(summary(&hunks), vec![(1..1, HunkKind::Deleted, false)]);
        let hunks = uncommitted_hunks(&bases(Some(head), Some(head)), "a\nb\nx\ny\nc\nd\n");
        assert_eq!(summary(&hunks), vec![(2..4, HunkKind::Added, false)]);
    }

    #[test]
    fn hunks_already_in_the_index_are_staged() {
        let head = "a\nb\nc\n";
        let index = "a\nB\nc\n";
        let text = "a\nB\nc\nd\n";
        let hunks = uncommitted_hunks(&bases(Some(head), Some(index)), text);
        assert_eq!(
            summary(&hunks),
            vec![
                (1..2, HunkKind::Modified, true),
                (3..4, HunkKind::Added, false)
            ]
        );
    }

    #[test]
    fn files_new_to_the_repository_are_one_addition() {
        let hunks = uncommitted_hunks(&bases(None, None), "x\ny\n");
        assert_eq!(summary(&hunks), vec![(0..2, HunkKind::Added, false)]);
        let hunks = uncommitted_hunks(&bases(None, Some("x\ny\n")), "x\ny\n");
        assert_eq!(summary(&hunks), vec![(0..2, HunkKind::Added, true)]);
    }

    fn index_text(head: &str, index: &str, text: &str, stage: bool, pick: usize) -> Option<String> {
        let bases = bases(Some(head), Some(index));
        let hunks = uncommitted_hunks(&bases, text);
        index_after(&bases, text, &hunks[pick..pick + 1], stage).flatten()
    }

    #[test]
    fn staging_one_hunk_leaves_the_others_out_of_the_index() {
        let head = "a\nb\nc\nd\ne\n";
        let text = "a\nB\nc\nd\nE\n";
        assert_eq!(
            index_text(head, head, text, true, 0).as_deref(),
            Some("a\nB\nc\nd\ne\n")
        );
        assert_eq!(
            index_text(head, head, text, true, 1).as_deref(),
            Some("a\nb\nc\nd\nE\n")
        );
        assert_eq!(
            index_text(head, head, "a\nc\nd\ne\n", true, 0).as_deref(),
            Some("a\nc\nd\ne\n")
        );
        assert_eq!(
            index_text(head, head, "a\nb\nx\nc\nd\ne\n", true, 0).as_deref(),
            Some("a\nb\nx\nc\nd\ne\n")
        );
    }

    #[test]
    fn unstaging_puts_head_back_and_already_staged_hunks_are_skipped() {
        let head = "a\nb\nc\n";
        let index = "a\nB\nc\n";
        let text = "a\nB\nc\nd\n";
        assert_eq!(
            index_text(head, index, text, false, 0).as_deref(),
            Some("a\nb\nc\n")
        );
        let bases = bases(Some(head), Some(index));
        let hunks = uncommitted_hunks(&bases, text);
        assert_eq!(index_after(&bases, text, &hunks[0..1], true), None);
        assert_eq!(index_after(&bases, text, &hunks[1..2], false), None);
    }

    #[test]
    fn new_files_stage_whole() {
        let bases = bases(None, None);
        let hunks = uncommitted_hunks(&bases, "x\n");
        assert_eq!(
            index_after(&bases, "x\n", &hunks, true),
            Some(Some("x\n".to_string()))
        );
        let staged = DiffBases {
            head: None,
            index: Some("x\n".to_string()),
        };
        let hunks = uncommitted_hunks(&staged, "x\n");
        assert_eq!(index_after(&staged, "x\n", &hunks, false), Some(None));
    }

    #[test]
    fn reads_bases_from_a_repository() {
        let root = std::env::temp_dir().join(format!("pomelo-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "commit.gpgsign", "false"]);
        let file = root.join("a.txt");
        std::fs::write(&file, "one\r\ntwo\r\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        run(&["add", "a.txt"]);
        let bases = load_bases(&file).unwrap();
        assert_eq!(bases.head.as_deref(), Some("one\ntwo\n"));
        assert_eq!(bases.index.as_deref(), Some("one\ntwo\nthree\n"));
        assert_eq!(
            load_bases(&root.join("missing.txt")).map(|b| b.head),
            Some(None)
        );
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let nested = root.join("sub").join("b.txt");
        std::fs::write(&nested, "x\n").unwrap();
        write_index(&nested, Some("staged\n")).unwrap();
        assert_eq!(
            load_bases(&nested).and_then(|b| b.index).as_deref(),
            Some("staged\n")
        );
        write_index(&nested, None).unwrap();
        assert_eq!(load_bases(&nested).and_then(|b| b.index), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
