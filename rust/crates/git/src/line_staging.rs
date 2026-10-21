//! Staging part of a change: only the changed lines a selection covers go in (or come out of) the index,
//! the way a hand-edited patch does. Added lines outside the selection stay out; removed lines are drawn above
//! the change's first row, so they count when the selection reaches that row.

use std::ops::Range;

use crate::{line_count, line_hunks, line_text, DiffBases};

/// One change between two texts: its `base` lines and the `target` lines that replace them.
struct Change {
    base: Range<usize>,
    target: Range<usize>,
}

impl Change {
    fn added_in(&self, selected: &Range<usize>) -> Range<usize> {
        let start = self.target.start.max(selected.start);
        start..self.target.end.min(selected.end).max(start)
    }

    fn removal_selected(&self, selected: &Range<usize>) -> bool {
        !self.base.is_empty()
            && selected.start <= self.target.start
            && (self.target.start < selected.end
                || (self.target.is_empty() && self.target.start == selected.end))
    }

    fn picked(&self, selected: &Range<usize>) -> usize {
        let removed = if self.removal_selected(selected) {
            self.base.len()
        } else {
            0
        };
        self.added_in(selected).len() + removed
    }
}

fn changes(base: Option<&str>, target: &str) -> Vec<Change> {
    let pairs = match base {
        Some(base) => line_hunks(base, target),
        None if line_count(target) == 0 => Vec::new(),
        None => vec![(0..0, 0..line_count(target))],
    };
    pairs
        .into_iter()
        .map(|(base, target)| Change { base, target })
        .collect()
}

/// `base` with, in each change, the selected removals applied and the selected additions put in after what
/// stays of its removed lines.
fn take_lines(base: &str, target: &str, changes: &[Change], selected: &Range<usize>) -> String {
    let mut out = String::with_capacity(base.len());
    let mut row = 0;
    for change in changes {
        out.push_str(&line_text(base, row..change.base.start));
        if !change.removal_selected(selected) {
            out.push_str(&line_text(base, change.base.clone()));
        }
        out.push_str(&line_text(target, change.added_in(selected)));
        row = change.base.end;
    }
    out.push_str(&line_text(base, row..line_count(base)));
    out
}

/// `target` with, in each change, the selected additions dropped and the selected removals put back above them.
fn give_back_lines(
    base: &str,
    target: &str,
    changes: &[Change],
    selected: &Range<usize>,
) -> String {
    let mut out = String::with_capacity(target.len());
    let mut row = 0;
    for change in changes {
        out.push_str(&line_text(target, row..change.target.start));
        if change.removal_selected(selected) {
            out.push_str(&line_text(base, change.base.clone()));
        }
        let added = change.added_in(selected);
        out.push_str(&line_text(target, change.target.start..added.start));
        out.push_str(&line_text(
            target,
            added.end.max(change.target.start)..change.target.end,
        ));
        row = change.target.end;
    }
    out.push_str(&line_text(target, row..line_count(target)));
    out
}

/// Where buffer row `row` sits in the index, going around the buffer's unstaged changes.
fn index_row(unstaged: &[(Range<usize>, Range<usize>)], row: usize) -> usize {
    let mut shift: isize = 0;
    for (index_rows, buffer_rows) in unstaged {
        if buffer_rows.end > row {
            if buffer_rows.start < row {
                return index_rows.end;
            }
            break;
        }
        shift = index_rows.end as isize - buffer_rows.end as isize;
    }
    (row as isize + shift).max(0) as usize
}

/// How many changed lines staging (or unstaging) buffer rows `rows` would move.
pub fn lines_to_stage(bases: &DiffBases, text: &str, rows: Range<usize>, stage: bool) -> usize {
    if stage {
        changes(bases.index.as_deref(), text)
            .iter()
            .map(|change| change.picked(&rows))
            .sum()
    } else {
        let Some(index) = bases.index.as_deref() else {
            return 0;
        };
        let rows = index_rows(index, text, rows);
        changes(bases.head.as_deref(), index)
            .iter()
            .map(|change| change.picked(&rows))
            .sum()
    }
}

fn index_rows(index: &str, text: &str, rows: Range<usize>) -> Range<usize> {
    let unstaged = line_hunks(index, text);
    index_row(&unstaged, rows.start)..index_row(&unstaged, rows.end)
}

/// The index text after staging (or unstaging) only the changed lines within buffer rows `rows`; `None` when
/// the selection holds no changed line.
pub fn index_with_lines(
    bases: &DiffBases,
    text: &str,
    rows: Range<usize>,
    stage: bool,
) -> Option<String> {
    if lines_to_stage(bases, text, rows.clone(), stage) == 0 {
        return None;
    }
    if stage {
        let index = bases.index.as_deref().unwrap_or_default();
        let changes = changes(bases.index.as_deref(), text);
        Some(take_lines(index, text, &changes, &rows))
    } else {
        let index = bases.index.as_deref()?;
        let head = bases.head.as_deref().unwrap_or_default();
        let rows = index_rows(index, text, rows);
        let changes = changes(bases.head.as_deref(), index);
        Some(give_back_lines(head, index, &changes, &rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bases(head: &str, index: &str) -> DiffBases {
        DiffBases {
            head: Some(head.to_string()),
            index: Some(index.to_string()),
        }
    }

    #[test]
    fn stages_only_the_selected_added_lines() {
        let head = "a\nb\n";
        let text = "a\nx\ny\nz\nb\n";
        let staged = index_with_lines(&bases(head, head), text, 2..3, true);
        assert_eq!(staged.as_deref(), Some("a\ny\nb\n"));
        assert_eq!(lines_to_stage(&bases(head, head), text, 2..3, true), 1);
        let staged = index_with_lines(&bases(head, head), text, 1..4, true);
        assert_eq!(staged.as_deref(), Some(text));
    }

    #[test]
    fn removed_lines_go_with_a_selection_reaching_the_first_row() {
        let head = "a\nold\nb\n";
        let text = "a\nnew1\nnew2\nb\n";
        let first = index_with_lines(&bases(head, head), text, 1..2, true);
        assert_eq!(first.as_deref(), Some("a\nnew1\nb\n"));
        assert_eq!(lines_to_stage(&bases(head, head), text, 1..2, true), 2);
        let second = index_with_lines(&bases(head, head), text, 2..3, true);
        assert_eq!(second.as_deref(), Some("a\nold\nnew2\nb\n"));
    }

    #[test]
    fn a_pure_deletion_stages_from_either_neighbouring_row() {
        let head = "a\ngone\nb\n";
        let text = "a\nb\n";
        let above = index_with_lines(&bases(head, head), text, 0..1, true);
        assert_eq!(above.as_deref(), Some("a\nb\n"));
        let below = index_with_lines(&bases(head, head), text, 1..2, true);
        assert_eq!(below.as_deref(), Some("a\nb\n"));
    }

    #[test]
    fn a_selection_without_changes_stages_nothing() {
        let head = "a\nb\nc\n";
        let text = "a\nb\nc\nd\n";
        assert_eq!(index_with_lines(&bases(head, head), text, 0..2, true), None);
        assert_eq!(index_with_lines(&bases(head, text), text, 3..4, true), None);
    }

    #[test]
    fn leaves_other_changes_in_the_index_alone() {
        let head = "a\nb\nc\nd\n";
        let index = "a\nB\nc\nd\n";
        let text = "a\nB\nc\nd\ne\nf\n";
        let staged = index_with_lines(&bases(head, index), text, 5..6, true);
        assert_eq!(staged.as_deref(), Some("a\nB\nc\nd\nf\n"));
    }

    #[test]
    fn unstages_only_the_selected_lines() {
        let head = "a\nb\n";
        let index = "a\nx\ny\nb\n";
        let staged = index_with_lines(&bases(head, index), index, 2..3, false);
        assert_eq!(staged.as_deref(), Some("a\nx\nb\n"));
        let modified = bases("a\nold\nb\n", "a\nnew\nb\n");
        let back = index_with_lines(&modified, "a\nnew\nb\n", 1..2, false);
        assert_eq!(back.as_deref(), Some("a\nold\nb\n"));
    }

    #[test]
    fn unstaging_maps_buffer_rows_past_unstaged_edits() {
        let head = "a\nb\n";
        let index = "a\nb\nc\n";
        let text = "top\na\nb\nc\n";
        assert_eq!(lines_to_stage(&bases(head, index), text, 3..4, false), 1);
        let back = index_with_lines(&bases(head, index), text, 3..4, false);
        assert_eq!(back.as_deref(), Some("a\nb\n"));
    }

    #[test]
    fn new_files_stage_line_by_line() {
        let none = DiffBases::default();
        let staged = index_with_lines(&none, "x\ny\n", 1..2, true);
        assert_eq!(staged.as_deref(), Some("y\n"));
    }
}
