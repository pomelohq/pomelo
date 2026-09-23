//! A file's uncommitted changes: its text at HEAD and in the index, read through the git CLI, diffed line by
//! line against the buffer into hunks. A hunk is staged when the index already holds it, i.e. no difference
//! between the index and the buffer overlaps it.

use std::ops::Range;
use std::path::Path;
use std::process::{Command, Stdio};

use imara_diff::{sources::lines, Algorithm, Diff, InternedInput};

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
        let _ = std::fs::remove_dir_all(&root);
    }
}
