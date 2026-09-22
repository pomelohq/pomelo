//! Folds: collapsed char ranges carried through edits, plus the indent-based rule for which range a row folds.
//!
//! A fold range starts at the end of its header row and ends either at the closing row's indent (when that row
//! opens with a closing bracket, so `{...}` reads on one line) or at the end of the last non-blank row of the
//! block. Every row after the header up to the row holding the fold's end is hidden from the display.

use crate::buffer::{map_offset, Bias, EditorBuffer};
use crate::syntax::Syntax;
use std::ops::Range;

const CLOSING_BRACKETS: [&str; 3] = ["}", ")", "]"];

/// Byte length of `row`'s leading whitespace, or `None` for a blank row.
fn indent_len(buffer: &EditorBuffer, row: usize) -> Option<usize> {
    if row >= buffer.rope.len_lines() {
        return None;
    }
    let mut len = 0usize;
    for ch in buffer.rope.line(row).chars() {
        match ch {
            ' ' | '\t' => len += 1,
            '\n' | '\r' => return None,
            _ => return Some(len),
        }
    }
    None
}

/// The next non-blank row after `row` is indented further (by raw whitespace length).
pub fn starts_indent(buffer: &EditorBuffer, row: usize) -> bool {
    let Some(indent) = indent_len(buffer, row) else {
        return false;
    };
    for next in row + 1..buffer.rope.len_lines() {
        match indent_len(buffer, next) {
            Some(next_indent) => return next_indent > indent,
            None => continue,
        }
    }
    false
}

/// The char range folding `row` would collapse, if it starts an indented block.
pub fn fold_range_for_row(
    buffer: &EditorBuffer,
    syntax: Option<&Syntax>,
    row: usize,
) -> Option<Range<usize>> {
    if !starts_indent(buffer, row) {
        return None;
    }
    let rope = &buffer.rope;
    let start = rope.line_to_char(row) + buffer.line_len(row);
    let start_indent = indent_len(buffer, row)?;
    let row_bytes = rope.line_to_byte(row)..rope.char_to_byte(start);
    let node_end = syntax.and_then(|s| s.enclosing_node_end(row_bytes));
    let mut closing_row = None;
    for next in row + 1..rope.len_lines() {
        let Some(indent) = indent_len(buffer, next) else {
            continue;
        };
        if indent > start_indent {
            continue;
        }
        // Unindented lines of a multi-line string or comment that belongs to the folded node don't end it.
        let line_start = rope.line_to_byte(next);
        let inside_node = node_end.is_some_and(|end| line_start < end);
        if inside_node && syntax.is_some_and(|s| s.in_string_or_comment(line_start + indent)) {
            continue;
        }
        closing_row = Some(next);
        break;
    }
    let last_non_blank_end = |from: usize| {
        let mut r = from;
        while r > row && indent_len(buffer, r).is_none() {
            r -= 1;
        }
        rope.line_to_char(r) + buffer.line_len(r)
    };
    let end = match closing_row {
        Some(closing) => {
            let indent = indent_len(buffer, closing).unwrap_or(0);
            let content_start = rope.line_to_char(closing) + indent;
            let content: String = rope
                .slice(content_start..rope.line_to_char(closing) + buffer.line_len(closing))
                .chars()
                .take(4)
                .collect();
            if CLOSING_BRACKETS.iter().any(|b| content.starts_with(b)) {
                content_start
            } else {
                last_non_blank_end(closing - 1)
            }
        }
        None => last_non_blank_end(rope.len_lines().saturating_sub(1)),
    };
    (end > start).then_some(start..end)
}

/// Collapsed ranges sorted by start, kept in sync with the buffer.
#[derive(Default)]
pub struct FoldMap {
    folds: Vec<Range<usize>>,
    synced_version: u64,
}

impl FoldMap {
    pub fn is_empty(&self) -> bool {
        self.folds.is_empty()
    }

    /// Carry folds through edits since the last sync: the start sticks after text typed at the header's end,
    /// the end sticks before text typed at the closer. Folds that collapse to nothing are dropped.
    pub fn sync(&mut self, buffer: &EditorBuffer) {
        let version = buffer.version();
        if version == self.synced_version {
            return;
        }
        for batch in buffer.edits_since(self.synced_version) {
            for fold in &mut self.folds {
                fold.start = map_offset(batch, fold.start, Bias::Right);
                fold.end = map_offset(batch, fold.end, Bias::Left);
            }
        }
        self.synced_version = version;
        self.normalize();
    }

    /// Stored separately (overlaps are merged only when computing visible rows), so unfolding an outer block
    /// keeps an inner fold collapsed.
    fn normalize(&mut self) {
        self.folds.retain(|f| f.start < f.end);
        self.folds
            .sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
        self.folds.dedup();
    }

    pub fn fold(&mut self, buffer: &EditorBuffer, range: Range<usize>) {
        self.sync(buffer);
        self.folds.push(range);
        self.normalize();
    }

    /// The fold whose header is `row`.
    pub fn fold_on_row(&self, buffer: &EditorBuffer, row: usize) -> Option<Range<usize>> {
        self.folds
            .iter()
            .find(|f| buffer.rope.char_to_line(f.start) == row)
            .cloned()
    }

    pub fn unfold_row(&mut self, buffer: &EditorBuffer, row: usize) {
        self.folds
            .retain(|f| buffer.rope.char_to_line(f.start) != row);
    }

    /// Folds with overlapping and nested ones merged, sorted by start.
    pub fn merged(&self) -> Vec<Range<usize>> {
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(self.folds.len());
        for fold in &self.folds {
            match merged.last_mut() {
                Some(last) if fold.start < last.end => last.end = last.end.max(fold.end),
                _ => merged.push(fold.clone()),
            }
        }
        merged
    }

    /// Buffer rows in display order: a fold's rows after its header, through the row holding its end, are hidden.
    pub fn visible_rows(&self, buffer: &EditorBuffer) -> Vec<usize> {
        let rows = buffer.rope.len_lines();
        let mut visible = Vec::with_capacity(rows);
        let mut folds = self.folds.iter().peekable();
        let mut row = 0;
        while row < rows {
            visible.push(row);
            let mut next = row + 1;
            while let Some(fold) = folds.peek() {
                let start_row = buffer.rope.char_to_line(fold.start);
                if start_row > row {
                    break;
                }
                if start_row == row {
                    next = next.max(buffer.rope.char_to_line(fold.end) + 1);
                }
                folds.next();
            }
            row = next;
        }
        visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> EditorBuffer {
        EditorBuffer::from_text(text)
    }

    #[test]
    fn folds_block_ending_at_closing_bracket() {
        let b = buffer("fn a() {\n    x;\n\n    y;\n}\nz\n");
        let range = fold_range_for_row(&b, None, 0).unwrap();
        assert_eq!(range.start, "fn a() {".len());
        assert_eq!(range.end, "fn a() {\n    x;\n\n    y;\n".len());
    }

    #[test]
    fn folds_without_closer_stop_at_last_non_blank_row() {
        let b = buffer("def f():\n    x\n    y\n\nz\n");
        let range = fold_range_for_row(&b, None, 0).unwrap();
        assert_eq!(range.end, "def f():\n    x\n    y".len());
    }

    #[test]
    fn non_block_rows_do_not_fold() {
        let b = buffer("a\nb\n");
        assert!(fold_range_for_row(&b, None, 0).is_none());
        assert!(!starts_indent(&b, 1));
    }

    #[test]
    fn visible_rows_hide_through_fold_end() {
        let b = buffer("fn a() {\n    x;\n}\nz\n");
        let mut folds = FoldMap::default();
        folds.fold(&b, fold_range_for_row(&b, None, 0).unwrap());
        assert_eq!(folds.visible_rows(&b), vec![0, 3, 4]);
        assert!(folds.fold_on_row(&b, 0).is_some());
        folds.unfold_row(&b, 0);
        assert_eq!(folds.visible_rows(&b), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn folds_follow_edits() {
        let mut b = buffer("x\nfn a() {\n    y;\n}\n");
        let mut folds = FoldMap::default();
        folds.fold(&b, fold_range_for_row(&b, None, 1).unwrap());
        b.place_cursor(0);
        b.insert_text("new\n");
        folds.sync(&b);
        assert!(folds.fold_on_row(&b, 2).is_some());
        assert_eq!(folds.visible_rows(&b), vec![0, 1, 2, 5]);
    }

    #[test]
    fn nested_folds_survive_outer_unfold() {
        let b = buffer("a {\n  b {\n    c\n  }\n}\n");
        let mut folds = FoldMap::default();
        folds.fold(&b, fold_range_for_row(&b, None, 1).unwrap());
        folds.fold(&b, fold_range_for_row(&b, None, 0).unwrap());
        assert_eq!(folds.visible_rows(&b), vec![0, 5]);
        folds.unfold_row(&b, 0);
        assert_eq!(folds.visible_rows(&b), vec![0, 1, 4, 5]);
    }

    #[test]
    fn unindented_lines_inside_a_string_do_not_end_the_fold() {
        let text = "fn a() {\n    let s = r\"\nraw\n\";\n    y;\n}\n";
        let b = buffer(text);
        let mut syntax = Syntax::new(crate::Lang::Rust).unwrap();
        syntax.sync(&b);
        while syntax.is_parsing() {
            std::thread::sleep(std::time::Duration::from_millis(1));
            syntax.sync(&b);
        }
        let range = fold_range_for_row(&b, Some(&syntax), 0).unwrap();
        assert_eq!(b.rope.char_to_line(range.end), 5);
        let without_syntax = fold_range_for_row(&b, None, 0).unwrap();
        assert_eq!(b.rope.char_to_line(without_syntax.end), 1);
    }

    #[test]
    fn motion_treats_a_fold_as_one_unit() {
        use crate::buffer::Motion;
        let mut b = buffer("fn a() {\n    x;\n}\nz\n");
        let mut folds = FoldMap::default();
        folds.fold(&b, fold_range_for_row(&b, None, 0).unwrap());
        let merged = folds.merged();
        let header_end = "fn a() {".len();
        let closer = "fn a() {\n    x;\n".len();
        b.place_cursor(header_end);
        b.apply_motion(Motion::Right, false, &merged);
        assert_eq!(b.cursor(), closer);
        b.apply_motion(Motion::Left, false, &merged);
        assert_eq!(b.cursor(), header_end);
        b.place_cursor(2);
        b.apply_motion(Motion::Down, false, &merged);
        assert_eq!(b.line_col(), (3, 1));
        b.apply_motion(Motion::Up, false, &merged);
        assert_eq!(b.line_col(), (0, 2));
        b.place_cursor(closer);
        b.apply_motion(Motion::Home, false, &merged);
        assert_eq!(b.cursor(), 0);
        b.apply_motion(Motion::End, false, &merged);
        assert_eq!(b.cursor(), closer + 1);
    }
}
