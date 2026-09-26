//! Mapping pointer positions to grid cells and encoding mouse reports for programs that track the mouse.

use std::iter::repeat_n;

use crate::input::Modifiers;
use crate::{TermMode, TerminalBounds};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// A grid cell; `line` is negative in scrollback above the live screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    pub line: i32,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// The cell under `(x, y)` (relative to the grid's top-left) and which half of it, clamped into the grid;
/// past the right or bottom edge counts as the right half so a drag there selects through the line end.
pub fn grid_point_and_side(
    x: f32,
    y: f32,
    bounds: TerminalBounds,
    display_offset: usize,
) -> (GridPoint, Side) {
    let last_column = bounds.num_columns().saturating_sub(1);
    let mut column = (x / bounds.cell_width).max(0.0) as usize;
    let within_cell = x.max(0.0) % bounds.cell_width;
    let mut side = if within_cell > bounds.cell_width / 2.0 {
        Side::Right
    } else {
        Side::Left
    };
    if column > last_column {
        column = last_column;
        side = Side::Right;
    }
    let bottom = i32::try_from(bounds.num_lines().saturating_sub(1)).unwrap_or(i32::MAX);
    let mut line = (y / bounds.line_height) as i32;
    if line > bottom {
        line = bottom;
        side = Side::Right;
    } else if line < 0 {
        side = Side::Left;
    }
    let offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
    (
        GridPoint {
            line: line.saturating_sub(offset),
            column,
        },
        side,
    )
}

enum Format {
    Sgr,
    Normal { utf8: bool },
}

impl Format {
    fn of(mode: TermMode) -> Self {
        if mode.contains(TermMode::SGR_MOUSE) {
            Format::Sgr
        } else {
            Format::Normal {
                utf8: mode.contains(TermMode::UTF8_MOUSE),
            }
        }
    }
}

const LEFT: u8 = 0;
const MIDDLE: u8 = 1;
const RIGHT: u8 = 2;
const MOVE: u8 = 32;
const NO_BUTTON_MOVE: u8 = 35;
const SCROLL_UP: u8 = 64;
const SCROLL_DOWN: u8 = 65;

fn button_code(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => LEFT,
        MouseButton::Middle => MIDDLE,
        MouseButton::Right => RIGHT,
    }
}

fn report(
    point: GridPoint,
    code: u8,
    pressed: bool,
    modifiers: Modifiers,
    format: Format,
) -> Option<Vec<u8>> {
    if point.line < 0 {
        return None;
    }
    let mods = 4 * modifiers.shift as u8 + 8 * modifiers.alt as u8 + 16 * modifiers.ctrl as u8;
    match format {
        Format::Sgr => {
            let last = if pressed { 'M' } else { 'm' };
            Some(
                format!(
                    "\x1b[<{};{};{}{last}",
                    code + mods,
                    point.column + 1,
                    point.line + 1
                )
                .into_bytes(),
            )
        }
        Format::Normal { utf8 } => {
            let code = if pressed { code + mods } else { 3 + mods };
            normal_report(point, code, utf8)
        }
    }
}

fn normal_report(point: GridPoint, code: u8, utf8: bool) -> Option<Vec<u8>> {
    let limit = if utf8 { 2015 } else { 223 };
    let line = point.line as usize;
    if line >= limit || point.column >= limit {
        return None;
    }
    let mut message = vec![0x1b, b'[', b'M', 32 + code];
    let mut encode = |position: usize| {
        if utf8 && position >= 95 {
            let value = 32 + 1 + position;
            message.push((0xC0 + value / 64) as u8);
            message.push((0x80 + (value & 63)) as u8);
        } else {
            message.push((32 + 1 + position) as u8);
        }
    };
    encode(point.column);
    encode(line);
    Some(message)
}

pub fn button_report(
    point: GridPoint,
    button: MouseButton,
    modifiers: Modifiers,
    pressed: bool,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    report(
        point,
        button_code(button),
        pressed,
        modifiers,
        Format::of(mode),
    )
}

/// Motion is reported in any-motion mode, and in drag mode only while a button is held.
pub fn moved_report(
    point: GridPoint,
    held: Option<MouseButton>,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) {
        return None;
    }
    let code = match held {
        Some(button) => MOVE + button_code(button),
        None if mode.contains(TermMode::MOUSE_DRAG) => return None,
        None => NO_BUTTON_MOVE,
    };
    report(point, code, true, modifiers, Format::of(mode))
}

pub fn scroll_reports(
    point: GridPoint,
    lines: i32,
    modifiers: Modifiers,
    mode: TermMode,
) -> Vec<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return Vec::new();
    }
    let code = if lines > 0 { SCROLL_UP } else { SCROLL_DOWN };
    report(point, code, true, modifiers, Format::of(mode))
        .map(|bytes| repeat_n(bytes, lines.unsigned_abs() as usize).collect())
        .unwrap_or_default()
}

/// On the alternate screen with alternate scroll on, the wheel becomes arrow keys.
pub fn alt_scroll(lines: i32) -> Vec<u8> {
    let key = if lines > 0 { b'A' } else { b'B' };
    (0..lines.unsigned_abs())
        .flat_map(|_| [0x1b, b'O', key])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> TerminalBounds {
        TerminalBounds {
            cell_width: 10.0,
            line_height: 20.0,
            width: 100.0,
            height: 100.0,
        }
    }

    #[test]
    fn points_clamp_into_the_grid() {
        let (point, side) = grid_point_and_side(34.0, 45.0, bounds(), 0);
        assert_eq!(point, GridPoint { line: 2, column: 3 });
        assert_eq!(side, Side::Left);
        let (point, side) = grid_point_and_side(500.0, 500.0, bounds(), 3);
        assert_eq!(point, GridPoint { line: 1, column: 9 });
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn reports_in_both_formats() {
        let point = GridPoint { line: 1, column: 4 };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let sgr = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            button_report(point, MouseButton::Left, shift, true, sgr),
            Some(b"\x1b[<4;5;2M".to_vec())
        );
        assert_eq!(
            button_report(point, MouseButton::Left, Modifiers::default(), false, sgr),
            Some(b"\x1b[<0;5;2m".to_vec())
        );
        let normal = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            button_report(
                point,
                MouseButton::Right,
                Modifiers::default(),
                true,
                normal
            ),
            Some(vec![0x1b, b'[', b'M', 34, 37, 34])
        );
        assert_eq!(
            button_report(
                point,
                MouseButton::Left,
                Modifiers::default(),
                true,
                TermMode::empty()
            ),
            None
        );
    }

    #[test]
    fn motion_and_scroll() {
        let point = GridPoint { line: 0, column: 0 };
        let drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert_eq!(moved_report(point, None, Modifiers::default(), drag), None);
        assert_eq!(
            moved_report(point, Some(MouseButton::Left), Modifiers::default(), drag),
            Some(b"\x1b[<32;1;1M".to_vec())
        );
        let click = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            scroll_reports(point, -3, Modifiers::default(), click).len(),
            3
        );
        assert_eq!(alt_scroll(2), b"\x1bOA\x1bOA".to_vec());
    }
}
