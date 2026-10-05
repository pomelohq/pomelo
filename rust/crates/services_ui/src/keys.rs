use workspace::keymap::Action as KeyAction;
use workspace::list_nav::{self, NavMove};
use workspace::{EditKey, SidePanelView};

use crate::{Control, Row, ServicesPanel, Status, ROW_H};

impl ServicesPanel {
    /// The row the keyboard is on, while the list has the keyboard.
    pub(crate) fn keyboard_row(&self) -> Option<usize> {
        self.keyboard.then_some(self.nav.selected).flatten()
    }

    pub(crate) fn take_keyboard(&mut self, on: bool) {
        self.keyboard = on;
        if on {
            self.filter_focused = false;
            if self.nav.selected.is_none() {
                self.move_selection(NavMove::First);
            }
        }
    }

    fn move_selection(&mut self, movement: NavMove) {
        self.nav.apply(movement, self.rows.len(), false, |_| true);
        self.scroll_to_selected();
    }

    /// Rows above the shared zone scroll under the overview cards; the shared zone stays put.
    fn scroll_to_selected(&mut self) {
        let Some(index) = self.nav.selected else {
            return;
        };
        if !self.tree_rows().contains(&index) {
            return;
        }
        let top = self.preamble_h + index as f32 * ROW_H;
        let list_h = self.list_h;
        if top < self.scroll {
            self.scroll = top;
        } else if top + ROW_H > self.scroll + list_h {
            self.scroll = top + ROW_H - list_h;
        }
    }

    pub(crate) fn nav_key(&mut self, key: EditKey, shift: bool) -> bool {
        let Some(movement) = list_nav::nav_move(key, shift) else {
            return false;
        };
        self.move_selection(movement);
        true
    }

    fn press(&mut self, control: Control) -> bool {
        let Some(index) = self.nav.selected else {
            return false;
        };
        self.click(self.id(index, control));
        true
    }

    fn toggle_running(&mut self) -> bool {
        let control = match self.nav.selected.and_then(|index| self.rows.get(index)) {
            Some(Row::Group { running, .. }) if *running > 0 => Control::StopAll,
            Some(Row::Group { .. }) => Control::StartAll,
            Some(Row::Service { holder, .. }) if self.model.status(holder) == Status::Running => {
                Control::Stop
            }
            Some(Row::Service { .. }) => Control::Start,
            Some(Row::Shared { name }) if self.model.shared_running(name) => Control::Stop,
            Some(Row::Shared { .. }) => Control::Start,
            None => return false,
        };
        self.press(control)
    }

    /// Folds an open group; from a service, goes to the group it sits in.
    fn collapse_selected(&mut self) -> bool {
        let Some(index) = self.nav.selected else {
            return false;
        };
        match self.rows.get(index) {
            Some(Row::Group { key, collapsed, .. }) => {
                if !*collapsed {
                    self.toggle_group(key.clone());
                }
            }
            Some(_) => {
                let group = (0..index)
                    .rev()
                    .find(|above| matches!(self.rows.get(*above), Some(Row::Group { .. })));
                if let Some(group) = group {
                    self.nav.selected = Some(group);
                    self.scroll_to_selected();
                }
            }
            None => return false,
        }
        true
    }

    fn expand_selected(&mut self) -> bool {
        match self.nav.selected.and_then(|index| self.rows.get(index)) {
            Some(Row::Group {
                key,
                collapsed: true,
                ..
            }) => self.toggle_group(key.clone()),
            Some(Row::Group { .. }) => self.move_selection(NavMove::Next),
            _ => return false,
        }
        true
    }

    pub(crate) fn run_key_action(&mut self, action: KeyAction) -> bool {
        let selected = self.nav.selected.and_then(|index| self.rows.get(index));
        let is_service = matches!(selected, Some(Row::Service { .. }));
        let is_shared = matches!(selected, Some(Row::Shared { .. }));
        match action {
            KeyAction::ServicesOpen => self.press(Control::Row),
            KeyAction::ServicesToggleRunning => self.toggle_running(),
            KeyAction::ServicesRestart if is_service || is_shared => self.press(Control::Restart),
            KeyAction::ServicesOpenInBrowser if is_service => self.press(Control::OpenUrl),
            KeyAction::ServicesLogs if is_service => self.press(Control::Logs),
            KeyAction::ServicesLogs if is_shared => self.press(Control::Row),
            KeyAction::ServicesCollapse => self.collapse_selected(),
            KeyAction::ServicesExpand => self.expand_selected(),
            _ => false,
        }
    }
}
