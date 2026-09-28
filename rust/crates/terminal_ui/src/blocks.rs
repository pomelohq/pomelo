//! Block element characters drawn as exact rectangles instead of font glyphs, so bars, QR codes and shaded
//! fills tile seamlessly across cells. Rectangles are in sub-cell units: each cell is 8 columns by 24 lines
//! (the common multiple of eighth blocks and the thirds of sextants).

pub const SUBCELL_COLUMNS: i32 = 8;
pub const SUBCELL_LINES: i32 = 24;

/// `(column, line, columns, lines)` in sub-cell units, plus an opacity for the shade characters.
pub type SubRect = (i32, i32, i32, i32, f32);

fn single_rect(c: char) -> Option<(i32, i32, i32, i32)> {
    let code = c as u32;
    Some(match code {
        0x2580 => (0, 0, 8, 12),
        0x2581..=0x2588 => {
            let eighths = (code - 0x2580) as i32;
            (0, 24 - eighths * 3, 8, eighths * 3)
        }
        0x2589..=0x258F => (0, 0, (0x2590 - code) as i32, 24),
        0x2590 => (4, 0, 4, 24),
        0x2594 => (0, 0, 8, 3),
        0x2595 => (7, 0, 1, 24),
        _ => return None,
    })
}

/// Bit `row * 2 + column` set for each filled quarter.
fn quadrants(c: char) -> Option<u8> {
    Some(match c {
        '\u{2598}' => 0b0001,
        '\u{259D}' => 0b0010,
        '\u{2596}' => 0b0100,
        '\u{2597}' => 0b1000,
        '\u{259A}' => 0b1001,
        '\u{259E}' => 0b0110,
        '\u{259B}' => 0b0111,
        '\u{259C}' => 0b1011,
        '\u{2599}' => 0b1101,
        '\u{259F}' => 0b1110,
        _ => return None,
    })
}

/// Bit `row * 2 + column` set for each filled 2x3 sixth. The sextant block skips the four patterns that already
/// exist as block elements (empty, left half, right half, full).
fn sextants(c: char) -> Option<u8> {
    let offset = (c as u32).checked_sub(0x1FB00)?;
    if offset > 0x3B {
        return None;
    }
    Some((offset + 1 + u32::from(offset >= 20) + u32::from(offset >= 40)) as u8)
}

/// Shades approximated as the foreground at reduced opacity, trading the stipple for seamless coverage.
fn shade(c: char) -> Option<f32> {
    match c {
        '\u{2591}' => Some(0.25),
        '\u{2592}' => Some(0.5),
        '\u{2593}' => Some(0.75),
        _ => None,
    }
}

pub fn block_rects(c: char) -> Option<Vec<SubRect>> {
    if let Some((column, line, columns, lines)) = single_rect(c) {
        return Some(vec![(column, line, columns, lines, 1.0)]);
    }
    let grid = |filled: u8, rows: i32, row_height: i32| {
        (0..rows)
            .flat_map(|row| (0..2).map(move |column| (row, column)))
            .filter(|(row, column)| filled & (1 << (row * 2 + column)) != 0)
            .map(|(row, column)| (column * 4, row * row_height, 4, row_height, 1.0))
            .collect()
    };
    if let Some(filled) = quadrants(c) {
        return Some(grid(filled, 2, 12));
    }
    if let Some(filled) = sextants(c) {
        return Some(grid(filled, 3, 8));
    }
    shade(c).map(|opacity| vec![(0, 0, 8, 24, opacity)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighths_halves_and_quadrants() {
        assert_eq!(block_rects('\u{2588}'), Some(vec![(0, 0, 8, 24, 1.0)]));
        assert_eq!(block_rects('\u{2582}'), Some(vec![(0, 18, 8, 6, 1.0)]));
        assert_eq!(block_rects('\u{258F}'), Some(vec![(0, 0, 1, 24, 1.0)]));
        assert_eq!(block_rects('\u{2590}'), Some(vec![(4, 0, 4, 24, 1.0)]));
        assert_eq!(
            block_rects('\u{259A}'),
            Some(vec![(0, 0, 4, 12, 1.0), (4, 12, 4, 12, 1.0)])
        );
        assert_eq!(block_rects('\u{2592}'), Some(vec![(0, 0, 8, 24, 0.5)]));
        assert_eq!(block_rects('a'), None);
    }

    #[test]
    fn sextants_skip_existing_block_patterns() {
        assert_eq!(block_rects('\u{1FB00}'), Some(vec![(0, 0, 4, 8, 1.0)]));
        assert_eq!(sextants('\u{1FB13}'), Some(20));
        assert_eq!(sextants('\u{1FB14}'), Some(22));
        assert_eq!(sextants('\u{1FB3B}'), Some(62));
    }
}
