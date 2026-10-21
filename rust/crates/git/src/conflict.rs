//! Merge conflicts left in a file's text: each `<<<<<<<` .. `>>>>>>>` region with its two sides (and the common
//! base, for diff3-style markers), and the text that keeps one side, the other, or both.

use std::ops::Range;

/// One conflicted region, in lines of the text it was parsed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// From the `<<<<<<<` line through the `>>>>>>>` line.
    pub rows: Range<usize>,
    pub ours: Range<usize>,
    pub theirs: Range<usize>,
    pub base: Option<Range<usize>>,
    pub ours_branch: String,
    pub theirs_branch: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
    Both,
}

/// Every complete conflict, top to bottom. A `<<<<<<<` inside an unfinished region starts over from it, and a
/// region without its closing marker is not a conflict.
pub fn conflicts(text: &str) -> Vec<Conflict> {
    let mut found = Vec::new();
    let mut start: Option<(usize, String)> = None;
    let mut ours_end = None;
    let mut base_start = None;
    let mut base_end = None;
    let mut theirs_start = None;
    for (row, line) in text.lines().enumerate() {
        if let Some(branch) = line.strip_prefix("<<<<<<<") {
            start = Some((row, branch.trim().to_string()));
            (ours_end, base_start, base_end, theirs_start) = (None, None, None, None);
        } else if line.starts_with("|||||||") && start.is_some() && ours_end.is_none() {
            ours_end = Some(row);
            base_start = Some(row + 1);
        } else if line.starts_with("=======") && start.is_some() && theirs_start.is_none() {
            if ours_end.is_none() {
                ours_end = Some(row);
            } else if base_start.is_some() {
                base_end = Some(row);
            }
            theirs_start = Some(row + 1);
        } else if let Some(branch) = line.strip_prefix(">>>>>>>") {
            let (Some((first, ours_branch)), Some(ours_to), Some(theirs_from)) =
                (start.take(), ours_end, theirs_start)
            else {
                continue;
            };
            let theirs_branch = branch.trim().to_string();
            found.push(Conflict {
                rows: first..row + 1,
                ours: first + 1..ours_to,
                theirs: theirs_from..row,
                base: base_start.zip(base_end).map(|(from, to)| from..to),
                ours_branch: if ours_branch.is_empty() {
                    "HEAD".into()
                } else {
                    ours_branch
                },
                theirs_branch: if theirs_branch.is_empty() {
                    "theirs".into()
                } else {
                    theirs_branch
                },
            });
            (ours_end, base_start, base_end, theirs_start) = (None, None, None, None);
        }
    }
    found
}

/// The lines that replace `conflict.rows` when `side` wins: one side's lines, or ours then theirs.
pub fn resolution(text: &str, conflict: &Conflict, side: Side) -> String {
    let kept = |rows: &Range<usize>| crate::line_text(text, rows.clone());
    match side {
        Side::Ours => kept(&conflict.ours),
        Side::Theirs => kept(&conflict.theirs),
        Side::Both => kept(&conflict.ours) + &kept(&conflict.theirs),
    }
}

/// `text` with `conflict` settled for `side`.
pub fn resolve(text: &str, conflict: &Conflict, side: Side) -> String {
    let before = crate::line_text(text, 0..conflict.rows.start);
    let after = crate::line_text(text, conflict.rows.end..usize::MAX);
    before + &resolution(text, conflict, side) + &after
}

/// What a side agent is asked when it merges conflicted files for the user.
pub fn merge_prompt(paths: &[String]) -> String {
    let files = paths.join(", ");
    let noun = if paths.len() == 1 {
        "file has"
    } else {
        "files have"
    };
    format!(
        "These {noun} unresolved merge conflicts: {files}. For each conflict, read both sides and the code around \
         it, then rewrite the region so it keeps the intent of both changes, removing every conflict marker. Do \
         not stage, commit, or run git commands that change the index; the user marks the files resolved."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_WAY: &str = "def perform(mail_id)\n<<<<<<< HEAD\n  retry_on Error, wait: 30\n=======\n  retry_on Error, attempts: 5\n>>>>>>> origin/main\n  deliver(mail_id)\n";

    #[test]
    fn a_two_way_conflict_has_both_sides_and_their_branches() {
        let found = conflicts(TWO_WAY);
        assert_eq!(
            found,
            vec![Conflict {
                rows: 1..6,
                ours: 2..3,
                theirs: 4..5,
                base: None,
                ours_branch: "HEAD".into(),
                theirs_branch: "origin/main".into(),
            }]
        );
    }

    #[test]
    fn diff3_markers_carry_the_base() {
        let text = "<<<<<<< ours\na\n||||||| base\nb\n=======\nc\n>>>>>>> theirs\n";
        let found = conflicts(text);
        assert_eq!(found[0].ours, 1..2);
        assert_eq!(found[0].base, Some(3..4));
        assert_eq!(found[0].theirs, 5..6);
        assert_eq!(resolve(text, &found[0], Side::Theirs), "c\n");
    }

    #[test]
    fn each_side_replaces_the_whole_region() {
        let conflict = &conflicts(TWO_WAY)[0];
        assert_eq!(
            resolve(TWO_WAY, conflict, Side::Ours),
            "def perform(mail_id)\n  retry_on Error, wait: 30\n  deliver(mail_id)\n"
        );
        assert_eq!(
            resolve(TWO_WAY, conflict, Side::Theirs),
            "def perform(mail_id)\n  retry_on Error, attempts: 5\n  deliver(mail_id)\n"
        );
        assert_eq!(
            resolve(TWO_WAY, conflict, Side::Both),
            "def perform(mail_id)\n  retry_on Error, wait: 30\n  retry_on Error, attempts: 5\n  deliver(mail_id)\n"
        );
    }

    #[test]
    fn resolving_one_conflict_leaves_the_next_in_place() {
        let text = format!("{TWO_WAY}{TWO_WAY}");
        let found = conflicts(&text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].rows, 8..13);
        let once = resolve(&text, &found[0], Side::Ours);
        assert_eq!(conflicts(&once).len(), 1);
        let twice = resolve(&once, &conflicts(&once)[0], Side::Theirs);
        assert!(conflicts(&twice).is_empty());
    }

    #[test]
    fn unfinished_or_restarted_regions_are_not_conflicts() {
        assert!(conflicts("<<<<<<< HEAD\na\n=======\nb\n").is_empty());
        assert!(conflicts("a\n=======\nb\n>>>>>>> x\n").is_empty());
        let restarted = "<<<<<<< stale\nx\n<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\n";
        assert_eq!(conflicts(restarted)[0].rows, 2..7);
    }

    #[test]
    fn a_conflict_on_the_last_line_without_a_newline_resolves() {
        let text = "<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x";
        let found = conflicts(text);
        assert_eq!(resolve(text, &found[0], Side::Theirs), "b\n");
    }

    #[test]
    fn the_merge_prompt_names_every_file_and_keeps_git_to_the_user() {
        let prompt = merge_prompt(&["app/a.rb".into(), "app/b.rb".into()]);
        assert!(prompt.contains("app/a.rb, app/b.rb"));
        assert!(prompt.contains("files have"));
        assert!(prompt.contains("Do not stage"));
    }
}
