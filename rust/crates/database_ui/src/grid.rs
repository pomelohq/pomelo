//! A virtualized data grid after the reference's data table: a header row with muted names and a bottom border,
//! striped rows with a hover tint, 4px cell padding, a row-number column pinned at the left, spreadsheet-style
//! column resizing from an 8px handle at each header edge (double-click restores the width), a clicked cell
//! selected. Only the rows that fit are built; the grid scrolls by whole rows down and by pixels sideways.

use std::time::{Duration, Instant};

use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};

const HEADER_H: f32 = 26.0;
pub const ROW_H: f32 = 22.0;
const CELL_PAD: f32 = 4.0;
const FONT: f32 = 12.0;
const MIN_WIDTH: f32 = 40.0;
/// Content-sized columns stay between these widths until resized by hand.
const AUTO_MIN: f32 = 70.0;
const AUTO_MAX: f32 = 380.0;
const SAMPLE_ROWS: usize = 50;
/// Half of the reference's 8px resize handle, on each side of the edge.
const HANDLE_HALF: f32 = 4.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    HeaderEdge(usize),
    Header(usize),
    Cell(usize, usize),
    RowNumber(usize),
    Outside,
}

/// What a press asked the grid's owner for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridEvent {
    None,
    /// The header of this column was clicked: cycle its sort.
    Sort(usize),
}

pub struct Grid {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    widths: Vec<f32>,
    initial: Vec<f32>,
    first_row: usize,
    scroll_x: f32,
    scroll_carry: f32,
    selected: Option<(usize, usize)>,
    /// A column sorted ascending (`true`) or descending, as the owner applies it.
    pub sort: Option<(usize, bool)>,
    drag: Option<(usize, f32, f32)>,
    hover_row: Option<usize>,
    hover_edge: Option<usize>,
    last_edge_click: Option<(Instant, usize)>,
    area: Rect,
}

impl Default for Grid {
    fn default() -> Grid {
        Grid {
            columns: Vec::new(),
            rows: Vec::new(),
            widths: Vec::new(),
            initial: Vec::new(),
            first_row: 0,
            scroll_x: 0.0,
            scroll_carry: 0.0,
            selected: None,
            sort: None,
            drag: None,
            hover_row: None,
            hover_edge: None,
            last_edge_click: None,
            area: Rect::new(0.0, 0.0, 0.0, 0.0, Rgba::TRANSPARENT),
        }
    }
}

fn scale() -> f32 {
    ui::ui_text_scale()
}

fn text_width(text: &str) -> f32 {
    ui::measure_text_width(text, FONT, true, 400)
}

impl Grid {
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn rows(&self) -> &[Vec<Option<String>>] {
        &self.rows
    }

    /// New contents; column widths are kept when the columns stay the same (the next page of a table).
    pub fn set_data(&mut self, columns: Vec<String>, rows: Vec<Vec<Option<String>>>) {
        let same_columns = columns == self.columns;
        self.columns = columns;
        self.rows = rows;
        self.first_row = 0;
        self.scroll_carry = 0.0;
        self.selected = self
            .selected
            .filter(|(row, column)| *row < self.rows.len() && *column < self.columns.len());
        if !same_columns {
            self.initial = self.auto_widths();
            self.widths = self.initial.clone();
            self.scroll_x = 0.0;
            self.selected = None;
            self.sort = None;
        }
    }

    fn auto_widths(&self) -> Vec<f32> {
        (0..self.columns.len())
            .map(|column| {
                let widest = self
                    .rows
                    .iter()
                    .take(SAMPLE_ROWS)
                    .filter_map(|row| row.get(column).cloned().flatten())
                    .map(|text| text_width(text.lines().next().unwrap_or_default()))
                    .fold(text_width(&self.columns[column]) + 16.0, f32::max);
                (widest + 2.0 * CELL_PAD + 12.0).clamp(AUTO_MIN, AUTO_MAX)
            })
            .collect()
    }

    fn row_number_width(&self) -> f32 {
        let digits = (self.first_row + self.visible_rows()).max(1).to_string();
        (text_width(&digits) + 20.0).max(40.0)
    }

    /// Whole rows that fit below the header.
    pub fn visible_rows(&self) -> usize {
        ((self.area.h / scale() - HEADER_H) / ROW_H)
            .floor()
            .max(0.0) as usize
    }

    fn total_width(&self) -> f32 {
        self.widths.iter().sum()
    }

    fn data_view_width(&self) -> f32 {
        (self.area.w / scale() - self.row_number_width()).max(0.0)
    }

    fn max_first_row(&self) -> usize {
        self.rows.len().saturating_sub(self.visible_rows())
    }

    /// Each column's visible slice `(column, left, width)` in design px from the data area's left edge.
    fn visible_columns(&self) -> Vec<(usize, f32, f32)> {
        let view = self.data_view_width();
        let mut left = -self.scroll_x;
        let mut shown = Vec::new();
        for (column, width) in self.widths.iter().enumerate() {
            let (start, end) = (left.max(0.0), (left + width).min(view));
            if end - start >= 1.0 {
                shown.push((column, start, end - start));
            }
            left += width;
        }
        shown
    }

    /// The column's right edge on screen (logical px), when it is visible.
    fn edge_x(&self, column: usize) -> Option<f32> {
        let right: f32 = self.widths[..=column].iter().sum::<f32>() - self.scroll_x;
        (right >= 0.0 && right <= self.data_view_width())
            .then(|| self.area.x + (self.row_number_width() + right) * scale())
    }

    pub fn region(&self, x: f32, y: f32) -> Region {
        let area = self.area;
        if x < area.x || x >= area.x + area.w || y < area.y || y >= area.y + area.h {
            return Region::Outside;
        }
        let local_y = (y - area.y) / scale();
        if local_y < HEADER_H {
            for column in 0..self.widths.len() {
                if let Some(edge) = self.edge_x(column) {
                    if (x - edge).abs() <= HANDLE_HALF * scale() {
                        return Region::HeaderEdge(column);
                    }
                }
            }
        }
        let local_x = (x - area.x) / scale() - self.row_number_width();
        let row = self.first_row + ((local_y - HEADER_H) / ROW_H).floor().max(0.0) as usize;
        if local_x < 0.0 {
            return if local_y < HEADER_H || row >= self.rows.len() {
                Region::Outside
            } else {
                Region::RowNumber(row)
            };
        }
        let column = self
            .visible_columns()
            .into_iter()
            .find(|(_, left, width)| local_x >= *left && local_x < left + width)
            .map(|(column, _, _)| column);
        match (column, local_y < HEADER_H) {
            (Some(column), true) => Region::Header(column),
            (Some(column), false) if row < self.rows.len() => Region::Cell(row, column),
            _ => Region::Outside,
        }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) -> GridEvent {
        match self.region(x, y) {
            Region::HeaderEdge(column) => {
                let now = Instant::now();
                let double = self.last_edge_click.is_some_and(|(at, clicked)| {
                    clicked == column && now.duration_since(at) < DOUBLE_CLICK
                });
                self.last_edge_click = Some((now, column));
                if double {
                    self.widths[column] = self.initial[column];
                } else {
                    self.drag = Some((column, x, self.widths[column]));
                }
                GridEvent::None
            }
            Region::Header(column) => GridEvent::Sort(column),
            Region::Cell(row, column) => {
                self.selected = Some((row, column));
                GridEvent::None
            }
            Region::RowNumber(row) => {
                self.selected = Some((row, self.selected.map_or(0, |(_, column)| column)));
                GridEvent::None
            }
            Region::Outside => GridEvent::None,
        }
    }

    /// Drags a column edge; returns whether anything moved.
    pub fn pointer_drag(&mut self, x: f32) -> bool {
        let Some((column, start_x, start_width)) = self.drag else {
            return false;
        };
        self.widths[column] = (start_width + (x - start_x) / scale()).max(MIN_WIDTH);
        true
    }

    pub fn pointer_up(&mut self) {
        self.drag = None;
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Hover feedback: the row under the pointer and a highlighted resize edge.
    pub fn pointer_move(&mut self, x: f32, y: f32) -> bool {
        let (row, edge) = match self.region(x, y) {
            Region::Cell(row, _) | Region::RowNumber(row) => (Some(row), None),
            Region::HeaderEdge(column) => (None, Some(column)),
            _ => (None, None),
        };
        let changed = row != self.hover_row || edge != self.hover_edge;
        self.hover_row = row;
        self.hover_edge = edge;
        changed
    }

    /// Scrolls by whole rows (the fraction carries over); returns whether it moved.
    pub fn scroll_rows(&mut self, delta_y: f32) -> bool {
        self.scroll_carry -= delta_y / (ROW_H * scale());
        let steps = self.scroll_carry.trunc();
        self.scroll_carry -= steps;
        let next = (self.first_row as f32 + steps).clamp(0.0, self.max_first_row() as f32) as usize;
        let moved = next != self.first_row;
        self.first_row = next;
        moved
    }

    pub fn scroll_columns(&mut self, delta_x: f32) -> bool {
        let max = (self.total_width() - self.data_view_width()).max(0.0);
        let next = (self.scroll_x - delta_x / scale()).clamp(0.0, max);
        let moved = (next - self.scroll_x).abs() > 0.01;
        self.scroll_x = next;
        moved
    }

    /// Moves the selected cell (arrows, pages), keeping it in view; returns whether it moved.
    pub fn move_selection(&mut self, rows: isize, columns: isize) -> bool {
        if self.rows.is_empty() || self.columns.is_empty() {
            return false;
        }
        let (row, column) = self.selected.unwrap_or((self.first_row, 0));
        let clamp = |value: usize, delta: isize, len: usize| {
            (value as isize + delta).clamp(0, len as isize - 1) as usize
        };
        let next = (
            clamp(row, rows, self.rows.len()),
            clamp(column, columns, self.columns.len()),
        );
        let moved = self.selected != Some(next);
        self.selected = Some(next);
        let visible = self.visible_rows().max(1);
        if next.0 < self.first_row {
            self.first_row = next.0;
        } else if next.0 >= self.first_row + visible {
            self.first_row = next.0 + 1 - visible;
        }
        let left: f32 = self.widths[..next.1].iter().sum();
        let right = left + self.widths[next.1];
        if left < self.scroll_x {
            self.scroll_x = left;
        } else if right > self.scroll_x + self.data_view_width() {
            self.scroll_x = right - self.data_view_width();
        }
        moved
    }

    pub fn page_rows(&self) -> isize {
        self.visible_rows().max(1) as isize
    }

    /// The selected cell's text (empty for NULL).
    pub fn selected_text(&self) -> Option<String> {
        let (row, column) = self.selected?;
        self.rows
            .get(row)
            .and_then(|cells| cells.get(column))
            .map(|cell| cell.clone().unwrap_or_default())
    }

    pub fn selected(&self) -> Option<(usize, usize)> {
        self.selected
    }

    /// The grid laid into `area` (window px).
    pub fn render(&mut self, area: Rect) -> Node {
        self.area = area;
        self.first_row = self.first_row.min(self.max_first_row());
        let max_x = (self.total_width() - self.data_view_width()).max(0.0);
        self.scroll_x = self.scroll_x.min(max_x);
        let colors = theme();
        let number_width = self.row_number_width();
        let columns = self.visible_columns();

        let mut header = div().row().h_px(HEADER_H).items_center().child(
            div()
                .w_px(number_width)
                .h_px(HEADER_H)
                .bg(colors.panel_background),
        );
        for (column, _, width) in &columns {
            let mut name = div()
                .row()
                .items_center()
                .gap(4.0)
                .w_px((width - 1.0).max(0.0))
                .h_px(HEADER_H)
                .px(CELL_PAD)
                .child(
                    div().row().flex(1.0).items_center().child(
                        label(self.columns[*column].clone())
                            .size(FONT)
                            .mono()
                            .color(colors.text_muted)
                            .truncate(),
                    ),
                );
            if let Some((_, ascending)) = self.sort.filter(|(sorted, _)| sorted == column) {
                name = name.child(
                    icon(if ascending {
                        IconKind::ArrowUp
                    } else {
                        IconKind::ArrowDown
                    })
                    .size(10.0)
                    .color(colors.icon_accent),
                );
            }
            let edge = if self.hover_edge == Some(*column)
                || self.drag.is_some_and(|(c, _, _)| c == *column)
            {
                colors.border_focused
            } else {
                colors.border.alpha(0.8)
            };
            header = header
                .child(name)
                .child(div().w_px(1.0).h_px(HEADER_H).bg(edge));
        }
        let mut grid = div()
            .col()
            .child(header)
            .child(div().h_px(1.0).bg(colors.border_variant));

        let last = (self.first_row + self.visible_rows()).min(self.rows.len());
        for row in self.first_row..last {
            let background = if self.hover_row == Some(row) {
                colors.element_hover.alpha(0.6)
            } else if row % 2 == 1 {
                colors.text.alpha(0.05)
            } else {
                Rgba::TRANSPARENT
            };
            let mut line = div().row().h_px(ROW_H).items_center().bg(background).child(
                div()
                    .row()
                    .w_px(number_width)
                    .h_px(ROW_H)
                    .items_center()
                    .justify_center()
                    .bg(colors.panel_background)
                    .child(
                        label((row + 1).to_string())
                            .size(FONT - 1.0)
                            .mono()
                            .color(colors.text_muted),
                    ),
            );
            for (column, _, width) in &columns {
                let value = self.rows[row].get(*column).cloned().flatten();
                let selected = self.selected == Some((row, *column));
                let text = match &value {
                    Some(text) => label(text.lines().next().unwrap_or_default().to_string())
                        .size(FONT)
                        .mono()
                        .color(colors.text)
                        .truncate(),
                    None => label("NULL").size(FONT).mono().color(colors.text_disabled),
                };
                let mut cell = div()
                    .row()
                    .w_px(*width)
                    .h_px(ROW_H)
                    .px(CELL_PAD)
                    .items_center()
                    .child(text);
                if selected {
                    cell = cell.bg(colors.element_selected);
                }
                line = line.child(cell);
            }
            grid = grid.child(line);
        }
        grid.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: usize) -> Grid {
        let mut grid = Grid::default();
        grid.set_data(
            vec!["id".into(), "name".into(), "note".into()],
            (0..rows)
                .map(|row| {
                    vec![
                        Some(row.to_string()),
                        Some(format!("name {row}")),
                        (row % 3 != 0).then(|| "x".repeat(row)),
                    ]
                })
                .collect(),
        );
        grid.render(Rect::new(
            0.0,
            0.0,
            400.0,
            (HEADER_H + 10.0 * ROW_H) * scale(),
            Rgba::TRANSPARENT,
        ));
        grid
    }

    #[test]
    fn only_the_rows_that_fit_are_shown_and_scrolling_moves_by_rows() {
        let mut grid = grid(100);
        assert_eq!(grid.visible_rows(), 10);
        assert!(grid.scroll_rows(-3.0 * ROW_H));
        assert_eq!(grid.first_row, 3);
        assert!(grid.scroll_rows(-1000.0 * ROW_H));
        assert_eq!(grid.first_row, 90, "stops with the last row at the bottom");
        assert!(!grid.scroll_rows(-ROW_H));
        let mut short = self::grid(4);
        assert!(!short.scroll_rows(-5.0 * ROW_H));
    }

    #[test]
    fn widths_follow_content_within_bounds_and_survive_the_next_page() {
        let mut grid = grid(20);
        assert!(grid
            .widths
            .iter()
            .all(|w| (AUTO_MIN..=AUTO_MAX).contains(w)));
        grid.widths[1] = 200.0;
        grid.set_data(grid.columns.clone(), vec![vec![None, None, None]]);
        assert_eq!(grid.widths[1], 200.0, "same columns keep their widths");
        grid.set_data(vec!["other".into()], Vec::new());
        assert_eq!(grid.widths.len(), 1);
    }

    #[test]
    fn edges_resize_and_double_click_restores() {
        let mut grid = grid(5);
        let edge = grid.edge_x(0).expect("first edge");
        assert_eq!(grid.region(edge, 5.0), Region::HeaderEdge(0));
        grid.pointer_down(edge, 5.0);
        assert!(grid.pointer_drag(edge + 50.0 * scale()));
        grid.pointer_up();
        assert!((grid.widths[0] - (grid.initial[0] + 50.0)).abs() < 0.01);
        let edge = grid.edge_x(0).expect("moved edge");
        grid.pointer_down(edge, 5.0);
        grid.pointer_up();
        grid.pointer_down(edge, 5.0);
        assert_eq!(grid.widths[0], grid.initial[0]);
    }

    #[test]
    fn clicks_select_cells_and_headers_ask_to_sort() {
        let mut grid = grid(5);
        let number = grid.row_number_width() * scale();
        let y = (HEADER_H + ROW_H * 1.5) * scale();
        assert_eq!(grid.pointer_down(number + 5.0, y), GridEvent::None);
        assert_eq!(grid.selected(), Some((1, 0)));
        assert_eq!(grid.selected_text().as_deref(), Some("1"));
        assert_eq!(grid.pointer_down(number + 5.0, 5.0), GridEvent::Sort(0));
        assert!(grid.move_selection(1, 2));
        assert_eq!(grid.selected(), Some((2, 2)));
        assert_eq!(grid.selected_text().as_deref(), Some("xx"));
        grid.move_selection(1, 0);
        assert_eq!(
            grid.selected_text().as_deref(),
            Some(""),
            "NULL copies as empty"
        );
    }
}
