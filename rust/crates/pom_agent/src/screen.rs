//! What an agent's terminal shows now, rendered from the holder's output so text a TUI erased is gone.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

/// Wider than any real window: the agent's UI wraps its own lines, so a wider grid never re-wraps them and
/// its relative cursor moves land where they did on the real screen.
const COLUMNS: usize = 512;
const ROWS: usize = 200;

struct Size;

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        ROWS
    }

    fn screen_lines(&self) -> usize {
        ROWS
    }

    fn columns(&self) -> usize {
        COLUMNS
    }
}

/// The screen's lines down to the cursor, trailing spaces trimmed.
pub(crate) fn rendered_lines(output: &[u8]) -> Vec<String> {
    let mut term = Term::new(Config::default(), &Size, VoidListener);
    let mut processor: Processor = Processor::new();
    processor.advance(&mut term, output);
    let grid = term.grid();
    let last = grid.cursor.point.line.0.max(0) as usize;
    (0..=last.min(ROWS - 1))
        .map(|row| {
            let line = &grid[Line(row as i32)];
            let text: String = (0..COLUMNS).map(|column| line[Column(column)].c).collect();
            text.trim_end().to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_the_ui_erased_is_not_on_the_screen() {
        let output =
            b"first\r\nDo you want to proceed?\r\n> 1. Yes\r\n\x1b[1A\x1b[2K\x1b[1A\x1b[2Kdone\r\n";
        assert_eq!(rendered_lines(output), ["first", "done", ""]);
    }
}
