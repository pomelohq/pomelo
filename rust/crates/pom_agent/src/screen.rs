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

struct Size {
    columns: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// The screen's lines down to its last drawn line or the cursor, whichever is lower, trailing spaces
/// trimmed.
pub(crate) fn rendered_lines(output: &[u8]) -> Vec<String> {
    render(output, COLUMNS, ROWS)
}

fn render(output: &[u8], columns: usize, rows: usize) -> Vec<String> {
    let mut term = Term::new(Config::default(), &Size { columns, rows }, VoidListener);
    let mut processor: Processor = Processor::new();
    processor.advance(&mut term, output);
    let grid = term.grid();
    let mut lines: Vec<String> = (0..rows)
        .map(|row| {
            let line = &grid[Line(row as i32)];
            let text: String = (0..columns).map(|column| line[Column(column)].c).collect();
            text.trim_end().to_string()
        })
        .collect();
    // The agent parks its cursor inside a dialog, above the choices it is still showing.
    let cursor = grid.cursor.point.line.0.max(0) as usize;
    let last_drawn = lines.iter().rposition(|line| !line.is_empty()).unwrap_or(0);
    lines.truncate(cursor.max(last_drawn).min(rows - 1) + 1);
    lines
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn text_the_ui_erased_is_not_on_the_screen() {
        let output =
            b"first\r\nDo you want to proceed?\r\n> 1. Yes\r\n\x1b[1A\x1b[2K\x1b[1A\x1b[2Kdone\r\n";
        assert_eq!(rendered_lines(output), ["first", "done", ""]);
    }

    #[test]
    fn lines_drawn_below_the_parked_cursor_are_on_the_screen() {
        let output = b"top\r\n\x1b[3Gmiddle\r\n\x1b[3Gbottom\r\n\x1b[2A";
        assert_eq!(rendered_lines(output), ["top", "  middle", "  bottom"]);
    }

    /// Renders at the size the holder gives a session no app window is attached to.
    pub(crate) fn rendered_at_holder_size(output: &[u8]) -> Vec<String> {
        render(
            output,
            crate::drive::HOLDER_COLS as usize,
            crate::drive::HOLDER_ROWS as usize,
        )
    }
}
