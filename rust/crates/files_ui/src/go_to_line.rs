//! Ctrl+G: a modal that jumps the caret to `line[:column]` or `+n`/`-n` lines from the current one, previewing
//! the target row while typing and restoring the scroll when cancelled.

use crate::text_field::{TextField, INPUT_FONT};
use ui::{div, label, theme, Node};

pub const WIDTH: f32 = 384.0;

pub struct GoToLine {
    pub field: TextField,
    /// The caret's position when opened, 1-based.
    current_line: usize,
    current_column: usize,
    line_count: usize,
    /// Scroll to restore when the modal closes without confirming.
    pub prev_scroll: Option<(f32, f32)>,
}

impl GoToLine {
    pub fn new(line: usize, column: usize, line_count: usize, scroll: (f32, f32)) -> Self {
        Self {
            field: TextField::default(),
            current_line: line,
            current_column: column,
            line_count,
            prev_scroll: Some(scroll),
        }
    }

    fn placeholder(&self) -> String {
        format!("{}:{}", self.current_line, self.current_column)
    }

    /// Tab fills an empty field with the placeholder.
    pub fn accept_placeholder(&mut self) {
        if self.field.text().is_empty() {
            self.field.set_text(&self.placeholder());
            self.field.move_to_end();
        }
    }

    /// `+n`/`f n` forward or `-n`/`b n` back from the current line.
    fn relative_offset(&self) -> Option<i64> {
        let input = self.field.text();
        let trimmed = input.trim();
        let mut direction = None;
        let mut number_start = 0;
        for (i, c) in trimmed.char_indices() {
            match c {
                '+' | 'f' | 'F' | '-' | 'b' | 'B' => {
                    direction = Some(c);
                    number_start = i + c.len_utf8();
                }
                _ => break,
            }
        }
        let direction = direction?;
        let rest = &trimmed[number_start..];
        let value: i64 = rest.split(':').next().unwrap_or(rest).trim().parse().ok()?;
        match direction {
            '+' | 'f' | 'F' => Some(value),
            _ => Some(-value),
        }
    }

    fn line_and_column(&self) -> Option<(usize, Option<usize>)> {
        let input = self.field.text();
        let mut parts = input.splitn(2, ':').map(str::trim);
        let line = parts.next()?.parse().ok()?;
        let column = parts.next().and_then(|c| c.parse().ok());
        Some((line, column))
    }

    fn relative_target(&self, offset: i64) -> usize {
        if offset >= 0 {
            self.current_line.saturating_add(offset as usize)
        } else {
            self.current_line
                .saturating_sub(offset.unsigned_abs() as usize)
        }
    }

    /// The 0-based (row, char column) the query points at, if it parses.
    pub fn target(&self) -> Option<(usize, usize)> {
        let (line, column) = match self.relative_offset() {
            Some(offset) => (self.relative_target(offset), None),
            None => self.line_and_column()?,
        };
        Some((
            line.saturating_sub(1),
            column.unwrap_or(0).saturating_sub(1),
        ))
    }

    fn help_text(&self) -> String {
        if let Some(offset) = self.relative_offset() {
            return format!(
                "Go to line {} ({offset:+} from current)",
                self.relative_target(offset)
            );
        }
        match self.line_and_column() {
            Some((line, Some(column))) => format!("Go to line {line}, character {column}"),
            Some((line, None)) => format!("Go to line {line}"),
            None => format!(
                "Current Line: {} of {} (column {})",
                self.current_line, self.line_count, self.current_column
            ),
        }
    }

    pub fn render(&self) -> Node {
        let colors = theme();
        div()
            .col()
            .w_px(WIDTH)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .bg(colors.elevated_surface_background)
            .child(div().px(8.0).py(4.0).child(self.field.render(
                &self.placeholder(),
                true,
                colors.text,
                INPUT_FONT * 1.6,
            )))
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(
                div()
                    .row()
                    .px(8.0)
                    .py(4.0)
                    .child(label(self.help_text()).size(14.0).color(colors.text_muted)),
            )
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modal(query: &str) -> GoToLine {
        let mut m = GoToLine::new(10, 3, 50, (0.0, 0.0));
        m.field.insert(query);
        m
    }

    #[test]
    fn parses_absolute_and_relative_targets() {
        assert_eq!(modal("12").target(), Some((11, 0)));
        assert_eq!(modal("12:5").target(), Some((11, 4)));
        assert_eq!(modal("+5").target(), Some((14, 0)));
        assert_eq!(modal("b3").target(), Some((6, 0)));
        assert_eq!(modal("x").target(), None);
    }

    #[test]
    fn help_text_describes_the_query() {
        assert_eq!(modal("").help_text(), "Current Line: 10 of 50 (column 3)");
        assert_eq!(modal("-2").help_text(), "Go to line 8 (-2 from current)");
        assert_eq!(modal("7:2").help_text(), "Go to line 7, character 2");
    }

    #[test]
    fn tab_fills_the_placeholder() {
        let mut m = modal("");
        m.accept_placeholder();
        assert_eq!(m.field.text(), "10:3");
    }
}
