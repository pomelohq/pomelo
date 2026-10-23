//! Buffer search: the query is the project search's, plus moving between matches.

pub use files::search::{Query as SearchQuery, SearchOptions};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Next,
    Prev,
}

/// The match at or around `cursor` (char offsets): one containing it, else the nearest in `direction`.
pub fn active_match_index(
    direction: Direction,
    matches: &[Range<usize>],
    cursor: usize,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let found = matches.binary_search_by(|m| {
        if m.end < cursor {
            std::cmp::Ordering::Less
        } else if m.start > cursor {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    Some(match direction {
        Direction::Prev => match found {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        },
        Direction::Next => match found {
            Ok(i) | Err(i) => i.min(matches.len() - 1),
        },
    })
}

/// The match `count` steps from `cursor` in `direction`, wrapping around the ends.
pub fn match_index_for_direction(
    matches: &[Range<usize>],
    current: usize,
    direction: Direction,
    count: usize,
    cursor: Option<usize>,
) -> usize {
    if count == 0 || matches.is_empty() {
        return current;
    }
    let cursor = cursor.unwrap_or(matches[current].start);
    let first = match direction {
        Direction::Next => matches.iter().position(|m| m.start > cursor).unwrap_or(0),
        Direction::Prev => matches
            .iter()
            .rposition(|m| m.end < cursor)
            .unwrap_or(matches.len() - 1),
    } as isize;
    let steps = count.saturating_sub(1) as isize;
    let len = matches.len() as isize;
    let index = match direction {
        Direction::Next => first + steps,
        Direction::Prev => first - steps,
    };
    index.rem_euclid(len) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_navigation_wraps() {
        let matches = vec![0..1, 5..6, 9..10];
        assert_eq!(active_match_index(Direction::Next, &matches, 3), Some(1));
        assert_eq!(
            match_index_for_direction(&matches, 2, Direction::Next, 1, Some(9)),
            0
        );
        assert_eq!(
            match_index_for_direction(&matches, 0, Direction::Prev, 1, Some(0)),
            2
        );
    }
}
