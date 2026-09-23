//! Drawing a terminal's grid snapshot: one row per screen line, cells batched into same-style runs laid on the
//! monospace cell grid, theme ANSI colors (contrast-lifted for legibility), the selection, and the cursor.

mod blocks;
mod contrast;
mod item;
mod panel;

use std::collections::HashMap;

use terminal::{
    Color, Content, CursorShape, Flags, IndexedCell, NamedColor, Palette, Rgb, TermMode,
};
use ui::{div, label, Node, Rect, Rgba, ThemeColors};

pub use contrast::{apca_contrast, ensure_minimum_contrast};
pub use item::TerminalItem;
pub use panel::TerminalPanel;

pub const FONT_SIZE: f32 = 15.0;
/// The "standard" terminal line height (the comfortable one is 1.618).
pub const LINE_HEIGHT: f32 = 1.3;
pub const MINIMUM_CONTRAST: f32 = 45.0;
const DIM_ALPHA: f32 = 0.7;
const BOLD_WEIGHT: u16 = 700;

/// Cell size in design px (the element tree's units); the grid is laid out in whole cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridMetrics {
    pub cell_width: f32,
    pub line_height: f32,
}

impl GridMetrics {
    pub fn measure(font_size: f32, line_height: f32) -> Self {
        let scale = ui::ui_text_scale();
        Self {
            cell_width: ui::measure_text_width("m", font_size, true, 400) / scale,
            line_height: font_size * line_height,
        }
    }

    /// The grid box for a panel of `width` x `height` design px: one cell of left gutter, at least two columns
    /// (a one-column grid misbehaves with wide characters), and whole rows.
    pub fn bounds(&self, width: f32, height: f32) -> terminal::TerminalBounds {
        let rows = (height / self.line_height).floor().max(1.0);
        terminal::TerminalBounds {
            cell_width: self.cell_width,
            line_height: self.line_height,
            width: (width - self.cell_width).max(self.cell_width * 2.0),
            height: rows * self.line_height,
        }
    }
}

fn to_rgb(color: Rgba) -> Rgb {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: byte(color.r),
        g: byte(color.g),
        b: byte(color.b),
    }
}

fn from_rgb(color: Rgb) -> Rgba {
    Rgba::new(
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        1.0,
    )
}

pub fn palette(theme: &ThemeColors) -> Palette {
    Palette {
        ansi: theme.terminal_ansi.map(to_rgb),
        dim: theme.terminal_ansi_dim.map(to_rgb),
        foreground: to_rgb(theme.terminal_foreground),
        background: to_rgb(theme.terminal_background),
        bright_foreground: to_rgb(theme.terminal_bright_foreground),
        cursor: to_rgb(theme.player_cursor),
    }
}

/// A cell color in theme terms. Named colors map to the theme's terminal tokens; indexed ones use the palette
/// (theme ANSI for 0..16, the xterm cube and ramp above); true colors pass through.
pub fn convert_color(color: Color, theme: &ThemeColors, palette: &Palette) -> Rgba {
    match color {
        Color::Named(named) => match named {
            NamedColor::Foreground => theme.terminal_foreground,
            NamedColor::Background => theme.terminal_ansi_background,
            NamedColor::Cursor => theme.player_cursor,
            NamedColor::BrightForeground => theme.terminal_bright_foreground,
            NamedColor::DimForeground => theme.terminal_dim_foreground,
            NamedColor::DimBlack
            | NamedColor::DimRed
            | NamedColor::DimGreen
            | NamedColor::DimYellow
            | NamedColor::DimBlue
            | NamedColor::DimMagenta
            | NamedColor::DimCyan
            | NamedColor::DimWhite => {
                theme.terminal_ansi_dim[named as usize - NamedColor::DimBlack as usize]
            }
            _ => theme.terminal_ansi[(named as usize).min(15)],
        },
        Color::Spec(rgb) => from_rgb(rgb),
        Color::Indexed(index) => from_rgb(palette.color(index as usize)),
    }
}

/// Colors a program asked for exactly (true color, or the 256-color cube/ramp) are left alone; the 16 theme
/// colors get contrast-lifted.
fn is_exact_color(color: Color) -> bool {
    matches!(color, Color::Spec(_) | Color::Indexed(16..=255))
}

/// Box drawing, blocks, shapes and powerline separators join up with neighboring backgrounds, so their exact
/// color matters more than legibility.
fn is_decorative(c: char) -> bool {
    matches!(
        c as u32,
        0x2500..=0x257F
            | 0x2580..=0x259F
            | 0x25A0..=0x25FF
            | 0x1FB00..=0x1FB3B
            | 0xE0B0..=0xE0B7
            | 0xE0B8..=0xE0BF
            | 0xE0C0..=0xE0CA
            | 0xE0CC..=0xE0D1
            | 0xE0D2..=0xE0D7
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RunStyle {
    foreground: Rgba,
    background: Option<Rgba>,
    bold: bool,
    cursor: Option<CursorShape>,
}

struct Run {
    column: usize,
    cells: usize,
    text: String,
    style: RunStyle,
}

/// The grid's element tree plus rects painted over it (block characters, bar/underline cursor), positioned
/// relative to the grid's top-left in design px.
pub struct GridPaint {
    pub node: Node,
    pub overlays: Vec<Rect>,
}

pub struct GridOptions {
    pub focused: bool,
    pub cursor_visible: bool,
    pub minimum_contrast: f32,
}

/// Caches contrast-lifted colors across frames; a screen has few distinct (foreground, background) pairs.
#[derive(Default)]
pub struct GridPainter {
    lifted: HashMap<([u32; 4], [u32; 4]), Rgba>,
}

fn color_key(color: Rgba) -> [u32; 4] {
    [
        color.r.to_bits(),
        color.g.to_bits(),
        color.b.to_bits(),
        color.a.to_bits(),
    ]
}

fn blend(base: Rgba, over: Rgba) -> Rgba {
    let mix = |b: f32, o: f32| b * (1.0 - over.a) + o * over.a;
    Rgba::new(
        mix(base.r, over.r),
        mix(base.g, over.g),
        mix(base.b, over.b),
        1.0,
    )
}

/// Whether `(line, column)` falls in one of the sorted, inclusive search matches.
fn in_search_match(
    matches: &[(terminal::GridPoint, terminal::GridPoint)],
    line: i32,
    column: usize,
) -> bool {
    let point = (line, column);
    let index = matches.partition_point(|(_, end)| (end.line, end.column) < point);
    matches
        .get(index)
        .is_some_and(|(start, _)| (start.line, start.column) <= point)
}

/// Whether the live screen's last row has content, so a full screen sits flush against the bottom edge.
fn bottom_row_occupied(content: &Content) -> bool {
    let bottom = content.screen_lines as i32 - 1 - content.display_offset as i32;
    content.cursor.line >= bottom
        || content
            .cells
            .iter()
            .rev()
            .take_while(|cell| cell.line >= bottom)
            .any(|cell| cell.c != ' ')
}

impl GridPainter {
    fn foreground(
        &mut self,
        cell: &IndexedCell,
        fg: Color,
        bg: Color,
        theme: &ThemeColors,
        palette: &Palette,
        minimum: f32,
    ) -> Rgba {
        let mut color = convert_color(fg, theme, palette);
        if !is_exact_color(fg) && !is_decorative(cell.c) {
            let background = convert_color(bg, theme, palette);
            color = *self
                .lifted
                .entry((color_key(color), color_key(background)))
                .or_insert_with(|| ensure_minimum_contrast(color, background, minimum));
        }
        if cell.flags.contains(Flags::DIM) {
            color.a *= DIM_ALPHA;
        }
        color
    }

    /// The grid as a column of rows (plus the cursor bar/underline, if any, as an overlay rect relative to the
    /// grid's top-left in design px). The caller places it in the panel and paints the background.
    pub fn render(
        &mut self,
        content: &Content,
        metrics: GridMetrics,
        theme: &ThemeColors,
        options: &GridOptions,
    ) -> GridPaint {
        let palette = palette(theme);
        let mut overlays: Vec<Rect> = Vec::new();
        let offset = content.display_offset as i32;
        let screen_lines = content.screen_lines.max(1);
        let cursor_row = content.cursor.line + offset;
        let show_cursor = options.cursor_visible
            && content.cursor.shape != CursorShape::Hidden
            && (0..screen_lines as i32).contains(&cursor_row);
        let cursor_shape = match content.cursor.shape {
            _ if !options.focused => CursorShape::HollowBlock,
            shape => shape,
        };
        let mut rows: Vec<Vec<Run>> = (0..screen_lines).map(|_| Vec::new()).collect();
        let mut extras_before = false;
        for cell in &content.cells {
            let row = cell.line + offset;
            let Some(runs) = usize::try_from(row).ok().and_then(|row| rows.get_mut(row)) else {
                continue;
            };
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            if cell.c == ' ' && extras_before {
                extras_before = false;
                continue;
            }
            extras_before = !cell.zerowidth.is_empty();
            let (mut fg, mut bg) = (cell.fg, cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let is_cursor =
                show_cursor && row == cursor_row && cell.column == content.cursor.column;
            let selected = content.selection.is_some_and(|selection| {
                selection.contains(terminal::GridPoint {
                    line: cell.line,
                    column: cell.column,
                })
            });
            let matched = in_search_match(&content.search_matches, cell.line, cell.column);
            let blank = cell.c == ' ' && cell.zerowidth.is_empty();
            let default_background = matches!(bg, Color::Named(NamedColor::Background));
            if blank && default_background && !selected && !is_cursor && !matched {
                continue;
            }
            let mut background = (!default_background).then(|| convert_color(bg, theme, &palette));
            if matched {
                let base = background.unwrap_or(theme.terminal_background);
                background = Some(blend(base, theme.search_match_background));
            }
            if selected {
                let base = background.unwrap_or(theme.terminal_background);
                background = Some(blend(base, theme.player_selection));
            }
            let mut foreground =
                self.foreground(cell, fg, bg, theme, &palette, options.minimum_contrast);
            let point = terminal::GridPoint {
                line: cell.line,
                column: cell.column,
            };
            if content
                .hovered_link
                .as_ref()
                .is_some_and(|link| link.contains(point))
            {
                foreground = theme.link_text_hover;
                let x = cell.column as f32 * metrics.cell_width;
                let y = row as f32 * metrics.line_height + (metrics.line_height + FONT_SIZE) / 2.0;
                match overlays.last_mut() {
                    Some(last)
                        if last.y == y && (last.x + last.w - x).abs() < 0.01 && last.h == 1.0 =>
                    {
                        last.w += metrics.cell_width;
                    }
                    _ => overlays.push(Rect::new(x, y, metrics.cell_width, 1.0, foreground)),
                }
            }
            let cursor = is_cursor.then_some(cursor_shape);
            if cursor == Some(CursorShape::Block) {
                background = Some(theme.player_cursor);
                foreground = theme.terminal_ansi_background;
            }
            let block = blocks::block_rects(cell.c).filter(|_| cursor != Some(CursorShape::Block));
            if let Some(parts) = &block {
                let sub_width = metrics.cell_width / blocks::SUBCELL_COLUMNS as f32;
                let sub_height = metrics.line_height / blocks::SUBCELL_LINES as f32;
                let cell_x = cell.column as f32 * metrics.cell_width;
                let cell_y = row as f32 * metrics.line_height;
                overlays.extend(
                    parts
                        .iter()
                        .map(|&(column, line, columns, lines, opacity)| {
                            Rect::new(
                                cell_x + column as f32 * sub_width,
                                cell_y + line as f32 * sub_height,
                                columns as f32 * sub_width,
                                lines as f32 * sub_height,
                                foreground.alpha(foreground.a * opacity),
                            )
                        }),
                );
            }
            let style = RunStyle {
                foreground,
                background,
                bold: cell.flags.contains(Flags::BOLD),
                cursor,
            };
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let mut text = if block.is_some() {
                " ".to_string()
            } else {
                cell.c.to_string()
            };
            text.extend(cell.zerowidth.iter());
            let joins = runs.last().is_some_and(|last: &Run| {
                last.style == style
                    && last.column + last.cells == cell.column
                    && width == 1
                    && cursor.is_none()
            });
            match runs.last_mut() {
                Some(last) if joins => {
                    last.cells += 1;
                    last.text.push_str(&text);
                }
                _ => runs.push(Run {
                    column: cell.column,
                    cells: width,
                    text,
                    style,
                }),
            }
        }
        let mut grid = div().col();
        for runs in rows {
            let mut line = div().row().h_px(metrics.line_height).items_center();
            let mut column = 0;
            for run in runs {
                if run.column > column {
                    line =
                        line.child(div().w_px((run.column - column) as f32 * metrics.cell_width));
                }
                let mut text = label(run.text)
                    .size(FONT_SIZE)
                    .mono()
                    .color(run.style.foreground);
                if run.style.bold {
                    text = text.weight(BOLD_WEIGHT);
                }
                let mut segment = div()
                    .row()
                    .items_center()
                    .w_px(run.cells as f32 * metrics.cell_width)
                    .h_px(metrics.line_height)
                    .child(text);
                if let Some(background) = run.style.background {
                    segment = segment.bg(background);
                }
                if run.style.cursor == Some(CursorShape::HollowBlock) {
                    segment = segment.border(1.0, theme.player_cursor);
                }
                line = line.child(segment);
                column = run.column + run.cells;
            }
            grid = grid.child(line);
        }
        let caret = show_cursor
            .then(|| {
                let x = content.cursor.column as f32 * metrics.cell_width;
                let y = cursor_row as f32 * metrics.line_height;
                match cursor_shape {
                    CursorShape::Beam => Some(Rect::new(
                        x,
                        y,
                        2.0,
                        metrics.line_height,
                        theme.player_cursor,
                    )),
                    CursorShape::Underline => Some(Rect::new(
                        x,
                        y + metrics.line_height - 2.0,
                        metrics.cell_width,
                        2.0,
                        theme.player_cursor,
                    )),
                    _ => None,
                }
            })
            .flatten();
        overlays.extend(caret);
        GridPaint {
            node: grid.into(),
            overlays,
        }
    }
}

/// Whether the grid should hug the panel's bottom edge (the alternate screen, or a full live screen), leaving
/// the leftover sub-row space above instead of below.
pub fn anchor_to_bottom(content: &Content) -> bool {
    content.mode.contains(TermMode::ALT_SCREEN)
        || (content.display_offset == 0 && bottom_row_occupied(content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminal::{Cursor, GridPoint, SelectionRange, TerminalBounds};

    fn cell(line: i32, column: usize, c: char) -> IndexedCell {
        IndexedCell {
            line,
            column,
            c,
            fg: Color::Named(NamedColor::Foreground),
            bg: Color::Named(NamedColor::Background),
            flags: Flags::empty(),
            zerowidth: Vec::new(),
        }
    }

    fn content(cells: Vec<IndexedCell>) -> Content {
        Content {
            cells,
            cursor: Cursor {
                line: 1,
                column: 0,
                shape: CursorShape::Block,
            },
            screen_lines: 3,
            columns: 10,
            bounds: TerminalBounds::default(),
            ..Content::default()
        }
    }

    #[test]
    fn named_and_indexed_colors_follow_the_theme() {
        let theme = ui::one_dark();
        let palette = palette(&theme);
        assert_eq!(
            convert_color(Color::Named(NamedColor::Red), &theme, &palette),
            theme.terminal_ansi[1]
        );
        assert_eq!(
            convert_color(Color::Named(NamedColor::BrightBlue), &theme, &palette),
            theme.terminal_ansi[12]
        );
        assert_eq!(
            convert_color(Color::Named(NamedColor::DimCyan), &theme, &palette),
            theme.terminal_ansi_dim[6]
        );
        assert_eq!(
            to_rgb(convert_color(Color::Indexed(196), &theme, &palette)),
            Rgb { r: 255, g: 0, b: 0 }
        );
    }

    #[test]
    fn runs_batch_same_style_cells_and_split_at_cursor_and_selection() {
        let mut cells: Vec<IndexedCell> = "ab cd"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c))
            .collect();
        cells.extend("xyz".chars().enumerate().map(|(i, c)| cell(1, i, c)));
        let mut grid = content(cells);
        grid.selection = Some(SelectionRange {
            start: GridPoint { line: 0, column: 3 },
            end: GridPoint { line: 0, column: 4 },
            is_block: false,
        });
        let metrics = GridMetrics {
            cell_width: 10.0,
            line_height: 20.0,
        };
        let options = GridOptions {
            focused: true,
            cursor_visible: true,
            minimum_contrast: MINIMUM_CONTRAST,
        };
        let theme = ui::one_dark();
        let paint = GridPainter::default().render(&grid, metrics, &theme, &options);
        assert!(paint.overlays.is_empty());
        let painted = ui::render(
            &paint.node,
            Rect::new(0.0, 0.0, 100.0, 60.0, Rgba::TRANSPARENT),
        );
        let texts: Vec<String> = painted.texts.iter().map(|text| text.text.clone()).collect();
        assert_eq!(texts, ["ab", "cd", "x", "yz"]);
        let cursor_bg = painted
            .rects
            .iter()
            .find(|rect| rect.color == theme.player_cursor)
            .map(|rect| (rect.x, rect.y));
        assert_eq!(cursor_bg, Some((0.0, 20.0)));
    }

    #[test]
    fn bars_and_hollow_cursor() {
        let mut grid = content(vec![cell(1, 0, 'q')]);
        grid.cursor.shape = CursorShape::Beam;
        let metrics = GridMetrics {
            cell_width: 10.0,
            line_height: 20.0,
        };
        let theme = ui::one_dark();
        let focused = GridOptions {
            focused: true,
            cursor_visible: true,
            minimum_contrast: 0.0,
        };
        let paint = GridPainter::default().render(&grid, metrics, &theme, &focused);
        assert_eq!(
            paint.overlays.first().map(|rect| (rect.x, rect.y, rect.w)),
            Some((0.0, 20.0, 2.0))
        );
        let unfocused = GridOptions {
            focused: false,
            ..focused
        };
        let paint = GridPainter::default().render(&grid, metrics, &theme, &unfocused);
        assert!(paint.overlays.is_empty());
        let painted = ui::render(
            &paint.node,
            Rect::new(0.0, 0.0, 100.0, 60.0, Rgba::TRANSPARENT),
        );
        assert!(painted
            .rects
            .iter()
            .any(|rect| rect.border > 0.0 && rect.border_color == theme.player_cursor));
    }

    #[test]
    fn grid_bounds_reserve_a_gutter_and_whole_rows() {
        let metrics = GridMetrics {
            cell_width: 9.0,
            line_height: 19.5,
        };
        let bounds = metrics.bounds(909.0, 200.0);
        assert_eq!(bounds.num_columns(), 100);
        assert_eq!(bounds.num_lines(), 10);
    }
}
