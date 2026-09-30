//! Tab-expanded width of every line, stored in blocks that each keep their own maximum so the widest line and
//! the lines that may soft-wrap are found by skipping whole blocks instead of visiting every line.

use std::ops::Range;

use ropey::{Rope, RopeSlice};

use crate::display::MAX_EXPANSION_COLUMN;

const BLOCK_LINES: usize = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineWidth {
    pub columns: usize,
    pub ascii: bool,
}

impl LineWidth {
    /// An upper bound on the line's extent in columns: a non-ASCII glyph may be up to two columns wide.
    pub fn reach(self) -> usize {
        if self.ascii {
            self.columns
        } else {
            self.columns.saturating_mul(2)
        }
    }

    pub fn of_line(line: RopeSlice, tab_size: usize) -> Self {
        let mut measure = Measure::default();
        for chunk in line.chunks() {
            for &byte in chunk.as_bytes() {
                if byte != b'\n' {
                    measure.push(byte, tab_size);
                }
            }
        }
        measure.width()
    }
}

#[derive(Default)]
struct Measure {
    columns: usize,
    byte: usize,
    ascii: bool,
    started: bool,
}

impl Measure {
    fn push(&mut self, byte: u8, tab_size: usize) {
        if !self.started {
            self.started = true;
            self.ascii = true;
        }
        if byte == b'\t' {
            self.columns += if self.byte >= MAX_EXPANSION_COLUMN {
                1
            } else {
                tab_size - self.columns % tab_size
            };
        } else if byte & 0xC0 != 0x80 {
            self.columns += 1;
        }
        if byte >= 0x80 {
            self.ascii = false;
        }
        self.byte += 1;
    }

    fn width(&self) -> LineWidth {
        LineWidth {
            columns: self.columns,
            ascii: !self.started || self.ascii,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Block {
    widths: Vec<LineWidth>,
    widest: usize,
    reach: usize,
}

impl Block {
    fn new(widths: Vec<LineWidth>) -> Self {
        let widest = widths.iter().map(|w| w.columns).max().unwrap_or(0);
        let reach = widths.iter().map(|w| w.reach()).max().unwrap_or(0);
        Self {
            widths,
            widest,
            reach,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LineWidths {
    blocks: Vec<Block>,
    len: usize,
}

impl LineWidths {
    /// One pass over the rope's chunks; lines split only at `\n`, matching the buffer's normalized text.
    pub fn measure(rope: &Rope, tab_size: usize) -> Self {
        let mut measured = Self::default();
        let mut block = Vec::with_capacity(BLOCK_LINES);
        let mut measure = Measure::default();
        for chunk in rope.chunks() {
            for &byte in chunk.as_bytes() {
                if byte == b'\n' {
                    block.push(measure.width());
                    measure = Measure::default();
                    if block.len() == BLOCK_LINES {
                        measured.len += block.len();
                        let full = std::mem::replace(&mut block, Vec::with_capacity(BLOCK_LINES));
                        measured.blocks.push(Block::new(full));
                    }
                } else {
                    measure.push(byte, tab_size);
                }
            }
        }
        block.push(measure.width());
        measured.len += block.len();
        measured.blocks.push(Block::new(block));
        measured
    }

    fn from_widths(widths: Vec<LineWidth>) -> Self {
        let len = widths.len();
        let blocks = widths
            .chunks(BLOCK_LINES)
            .map(|chunk| Block::new(chunk.to_vec()))
            .collect();
        Self { blocks, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, line: usize) -> Option<LineWidth> {
        let (block, start) = self.block_at(line)?;
        self.blocks[block].widths.get(line - start).copied()
    }

    pub fn widest(&self) -> usize {
        self.blocks.iter().map(|b| b.widest).max().unwrap_or(0)
    }

    /// The widest of `lines` whose `reach` stays within `columns`.
    pub fn widest_in(&self, lines: Range<usize>, columns: usize) -> usize {
        let mut widest = 0;
        let mut start = 0;
        for block in &self.blocks {
            let end = start + block.widths.len();
            if end > lines.start && start < lines.end {
                if lines.start <= start && end <= lines.end && block.reach <= columns {
                    widest = widest.max(block.widest);
                } else {
                    let from = lines.start.max(start) - start;
                    let to = lines.end.min(end) - start;
                    for width in &block.widths[from..to] {
                        if width.reach() <= columns {
                            widest = widest.max(width.columns);
                        }
                    }
                }
            }
            if end >= lines.end {
                break;
            }
            start = end;
        }
        widest
    }

    /// The first line at or after `from` whose `reach` exceeds `columns`.
    pub fn next_reaching_past(&self, from: usize, columns: usize) -> Option<usize> {
        let mut start = 0;
        for block in &self.blocks {
            let end = start + block.widths.len();
            if end > from && block.reach > columns {
                let skip = from.saturating_sub(start);
                if let Some(offset) = block.widths[skip..]
                    .iter()
                    .position(|w| w.reach() > columns)
                {
                    return Some(start + skip + offset);
                }
            }
            start = end;
        }
        None
    }

    /// Replace the widths of `lines` with `widths`, rebuilding only the blocks the range touches.
    pub fn splice(&mut self, lines: Range<usize>, widths: Vec<LineWidth>) {
        let start = lines.start.min(self.len);
        let end = lines.end.clamp(start, self.len);
        if self.blocks.is_empty() {
            *self = Self::from_widths(widths);
            return;
        }
        let (first, first_start) = self.block_at(start).unwrap_or((
            self.blocks.len() - 1,
            self.len - self.blocks.last().map_or(0, |b| b.widths.len()),
        ));
        let (last, _) = self
            .block_at(end.saturating_sub(1).max(start))
            .unwrap_or((self.blocks.len() - 1, 0));
        let mut merged: Vec<LineWidth> = Vec::new();
        for block in &self.blocks[first..=last] {
            merged.extend_from_slice(&block.widths);
        }
        let local = start - first_start..end - first_start;
        let removed = local.len();
        let added = widths.len();
        merged.splice(local, widths);
        let rebuilt: Vec<Block> = merged
            .chunks(BLOCK_LINES)
            .map(|chunk| Block::new(chunk.to_vec()))
            .collect();
        self.blocks.splice(first..=last, rebuilt);
        self.len = self.len - removed + added;
    }

    fn block_at(&self, line: usize) -> Option<(usize, usize)> {
        let mut start = 0;
        for (index, block) in self.blocks.iter().enumerate() {
            let end = start + block.widths.len();
            if line < end {
                return Some((index, start));
            }
            start = end;
        }
        None
    }

    /// Continue with `next`, whose first line starts where this text's last line break ended.
    pub fn append_after_break(&mut self, next: LineWidths) {
        if let Some(last) = self.blocks.last_mut() {
            if last.widths.pop().is_some() {
                self.len -= 1;
            }
            *last = Block::new(std::mem::take(&mut last.widths));
            if last.widths.is_empty() {
                self.blocks.pop();
            }
        }
        self.len += next.len;
        self.blocks.extend(next.blocks);
    }

    pub fn iter(&self) -> impl Iterator<Item = LineWidth> + '_ {
        self.blocks.iter().flat_map(|b| b.widths.iter().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths_of(text: &str) -> Vec<usize> {
        LineWidths::measure(&Rope::from_str(text), 4)
            .iter()
            .map(|w| w.columns)
            .collect()
    }

    #[test]
    fn measures_each_line_with_tabs_expanded() {
        assert_eq!(widths_of("a\n\tbb\nccc\n\ndddd\n"), vec![1, 6, 3, 0, 4, 0]);
        assert_eq!(widths_of("ab\tc"), vec![4 + 1]);
        assert_eq!(widths_of("đường"), vec![5]);
    }

    #[test]
    fn a_line_matches_the_whole_rope_pass() {
        let rope = Rope::from_str("x\tyé\n\t\tz\n");
        let whole = LineWidths::measure(&rope, 4);
        for line in 0..rope.len_lines() {
            assert_eq!(
                Some(LineWidth::of_line(rope.line(line), 4)),
                whole.get(line)
            );
        }
        assert!(!whole.get(0).unwrap().ascii);
        assert!(whole.get(1).unwrap().ascii);
    }

    #[test]
    fn splicing_across_blocks_keeps_every_line_in_place() {
        let text: String = (0..2000)
            .map(|i| format!("{}\n", "x".repeat(i % 97)))
            .collect();
        let mut widths = LineWidths::measure(&Rope::from_str(&text), 4);
        let mut plain: Vec<usize> = widths.iter().map(|w| w.columns).collect();
        let fresh = |n: usize| {
            (0..n)
                .map(|i| LineWidth {
                    columns: 1000 + i,
                    ascii: true,
                })
                .collect::<Vec<_>>()
        };
        for (range, count) in [(500..530, 3), (0..0, 700), (1500..2400, 0), (10..11, 1)] {
            let end = range.end.min(plain.len());
            let start = range.start.min(end);
            plain.splice(start..end, fresh(count).iter().map(|w| w.columns));
            widths.splice(range, fresh(count));
            assert_eq!(widths.len(), plain.len());
            assert_eq!(widths.iter().map(|w| w.columns).collect::<Vec<_>>(), plain);
            assert_eq!(widths.widest(), plain.iter().copied().max().unwrap_or(0));
        }
    }

    #[test]
    fn pieces_split_after_a_line_break_join_into_the_whole() {
        let text = "a\n\tbb\nccc\n\ndddd\n";
        for cut in [2, 6, 10, 11, text.len()] {
            let mut joined = LineWidths::measure(&Rope::from_str(&text[..cut]), 4);
            joined.append_after_break(LineWidths::measure(&Rope::from_str(&text[cut..]), 4));
            let whole = LineWidths::measure(&Rope::from_str(text), 4);
            assert_eq!(
                joined.iter().collect::<Vec<_>>(),
                whole.iter().collect::<Vec<_>>(),
                "cut {cut}"
            );
            assert_eq!(joined.len(), whole.len());
        }
    }

    #[test]
    fn finds_lines_that_reach_past_a_width() {
        let widths = LineWidths::measure(&Rope::from_str("aa\nbbbbbb\nc\nddddddd\nééé\n"), 4);
        assert_eq!(widths.next_reaching_past(0, 5), Some(1));
        assert_eq!(widths.next_reaching_past(2, 5), Some(3));
        assert_eq!(widths.next_reaching_past(4, 5), Some(4));
        assert_eq!(widths.next_reaching_past(5, 5), None);
        assert_eq!(widths.widest_in(0..6, 5), 2);
        assert_eq!(widths.widest_in(2..3, 5), 1);
        assert_eq!(widths.widest_in(0..6, 100), 7);
        assert_eq!(widths.widest(), 7);
    }
}
