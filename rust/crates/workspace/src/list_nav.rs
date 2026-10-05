//! Keyboard selection over a list: menus and panel lists share one set of keys (up and down, Tab and shift-Tab,
//! Home and End, cmd-up and cmd-down), skipping rows that cannot be picked.

use crate::EditKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavMove {
    Next,
    Previous,
    First,
    Last,
}

/// The move a key makes in a list, if it is one of the list keys.
pub fn nav_move(key: EditKey, shift: bool) -> Option<NavMove> {
    match key {
        EditKey::Down => Some(NavMove::Next),
        EditKey::Up => Some(NavMove::Previous),
        EditKey::Tab if shift => Some(NavMove::Previous),
        EditKey::Tab => Some(NavMove::Next),
        EditKey::Home | EditKey::DocumentStart | EditKey::PageUp => Some(NavMove::First),
        EditKey::End | EditKey::DocumentEnd | EditKey::PageDown => Some(NavMove::Last),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListNav {
    pub selected: Option<usize>,
}

impl ListNav {
    /// Moves the selection over `len` rows, landing only on rows `selectable` accepts. `wrap` lets Next past the
    /// end go to the first row (menus do; panel lists stop at the ends). Returns whether the selection changed.
    pub fn apply(
        &mut self,
        movement: NavMove,
        len: usize,
        wrap: bool,
        selectable: impl Fn(usize) -> bool,
    ) -> bool {
        let first = (0..len).find(|index| selectable(*index));
        let last = (0..len).rev().find(|index| selectable(*index));
        let next = match (movement, self.selected) {
            (NavMove::First, _) | (NavMove::Next, None) => first,
            (NavMove::Last, _) | (NavMove::Previous, None) => last,
            (NavMove::Next, Some(at)) => ((at + 1)..len)
                .find(|index| selectable(*index))
                .or(if wrap { first } else { Some(at) }),
            (NavMove::Previous, Some(at)) => (0..at.min(len))
                .rev()
                .find(|index| selectable(*index))
                .or(if wrap { last } else { Some(at) }),
        };
        let next = next.filter(|index| *index < len);
        let changed = next != self.selected;
        self.selected = next;
        changed
    }

    /// Keeps the selection inside a list that shrank.
    pub fn clamp(&mut self, len: usize) {
        if let Some(at) = self.selected {
            self.selected = (len > 0).then(|| at.min(len - 1));
        }
    }

    pub fn clear(&mut self) {
        self.selected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_skip_rows_that_cannot_be_picked_and_menus_wrap() {
        let pickable = |index: usize| index != 1 && index != 3;
        let mut nav = ListNav::default();
        assert!(nav.apply(NavMove::Next, 5, true, pickable));
        assert_eq!(nav.selected, Some(0));
        nav.apply(NavMove::Next, 5, true, pickable);
        assert_eq!(nav.selected, Some(2), "row 1 is skipped");
        nav.apply(NavMove::Next, 5, true, pickable);
        assert_eq!(nav.selected, Some(4));
        nav.apply(NavMove::Next, 5, true, pickable);
        assert_eq!(nav.selected, Some(0), "a menu wraps around");
        nav.apply(NavMove::Previous, 5, true, pickable);
        assert_eq!(nav.selected, Some(4));
        nav.apply(NavMove::First, 5, true, pickable);
        assert_eq!(nav.selected, Some(0));
    }

    #[test]
    fn a_panel_list_stops_at_its_ends() {
        let mut nav = ListNav::default();
        nav.apply(NavMove::Last, 3, false, |_| true);
        assert_eq!(nav.selected, Some(2));
        assert!(!nav.apply(NavMove::Next, 3, false, |_| true));
        assert_eq!(nav.selected, Some(2));
        nav.apply(NavMove::Previous, 3, false, |_| true);
        nav.apply(NavMove::Previous, 3, false, |_| true);
        nav.apply(NavMove::Previous, 3, false, |_| true);
        assert_eq!(nav.selected, Some(0));
        nav.clamp(0);
        assert_eq!(nav.selected, None);
    }

    #[test]
    fn the_list_keys() {
        assert_eq!(nav_move(EditKey::Down, false), Some(NavMove::Next));
        assert_eq!(nav_move(EditKey::Tab, true), Some(NavMove::Previous));
        assert_eq!(nav_move(EditKey::DocumentEnd, false), Some(NavMove::Last));
        assert_eq!(nav_move(EditKey::Enter, false), None);
    }
}
