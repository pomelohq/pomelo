//! The terminal panel: a pane group of terminal items, driven by the workspace. Panes split, each with its
//! own tabs and find bar; closing the last terminal closes the panel.

use std::path::PathBuf;

use terminal::{Keystroke, Modifiers, Waker};
use ui::{label, theme, IconKind, Node, Painted, Rect};
use workspace::pane::{Pane, PaneCommand};
use workspace::pane_group::SplitDirection;
use workspace::pane_group_view::{
    GroupClick, PaneButton, PaneButtonAction, PaneGroupConfig, PaneGroupView,
};
use workspace::{
    DividerAxis, EditorLayout, Item, TerminalKeyOutcome, TerminalOpenTarget, TerminalPanelView,
    TerminalSyncOutcome, TERMINAL_VIEW_BASE,
};

use crate::item::{Host, TerminalItem};

const MAX_PANES: usize = 8;

fn terminal_of(pane: &mut Pane) -> Option<&mut TerminalItem> {
    pane.active_item_mut()?
        .as_any_mut()?
        .downcast_mut::<TerminalItem>()
}

pub struct TerminalPanel {
    root: PathBuf,
    waker: Waker,
    panes: PaneGroupView,
    next_item_id: u64,
    /// The pane a press landed in, so the drag and release go to the same terminal.
    pressed_pane: Option<Vec<usize>>,
    focused: bool,
    /// The terminal last told it has focus, so a change of active tab or pane moves focus reporting along.
    focus_holder: Option<String>,
    spawn_error: Option<String>,
    /// The user closed the last terminal; reported on the next sync so the panel closes like when shells exit.
    closed_last: bool,
}

impl TerminalPanel {
    pub fn new(root: PathBuf, waker: Waker) -> Self {
        let mut panes = PaneGroupView::new(PaneGroupConfig {
            id_base: TERMINAL_VIEW_BASE,
            show_nav: false,
            buttons: vec![
                PaneButton {
                    icon: IconKind::Plus,
                    action: PaneButtonAction::NewItem,
                },
                PaneButton {
                    icon: IconKind::PanelRight,
                    action: PaneButtonAction::Split(SplitDirection::Right),
                },
                PaneButton {
                    icon: IconKind::PanelBottom,
                    action: PaneButtonAction::Split(SplitDirection::Down),
                },
            ],
            max_panes: MAX_PANES,
        });
        panes.set_focused(false);
        Self {
            root,
            waker,
            panes,
            next_item_id: 0,
            pressed_pane: None,
            focused: false,
            focus_holder: None,
            spawn_error: None,
            closed_last: false,
        }
    }

    fn spawn_item(&mut self, cwd: Option<PathBuf>) -> Option<TerminalItem> {
        let id = self.next_item_id;
        self.next_item_id += 1;
        match TerminalItem::spawn(id, self.root.clone(), cwd, self.waker.clone()) {
            Ok(item) => {
                self.spawn_error = None;
                Some(item)
            }
            Err(error) => {
                self.spawn_error = Some(format!("Failed to start the shell: {error}"));
                None
            }
        }
    }

    fn active_pane(&mut self) -> Option<&mut Pane> {
        self.panes.active_pane_mut()
    }

    fn active_terminal(&mut self) -> Option<&mut TerminalItem> {
        terminal_of(self.active_pane()?)
    }

    fn terminal_at(&mut self, x: f32, y: f32) -> Option<(Vec<usize>, &mut TerminalItem)> {
        let path = self.panes.pane_path_at(x, y)?;
        let item = terminal_of(self.panes.group.leaf_at_mut(&path)?)?;
        item.body_contains(x, y).then_some((path, item))
    }

    /// Tell terminals when focus moves between them (a tab or pane switch, or the panel gaining focus), so the
    /// shell's focus reporting and the cursor follow.
    fn reconcile_focus(&mut self) {
        let focused = self.focused;
        let holder = if focused {
            self.active_terminal().and_then(|item| item.id())
        } else {
            None
        };
        if holder == self.focus_holder {
            return;
        }
        let previous = self.focus_holder.take();
        self.all_terminals(&mut |item| {
            let id = item.id();
            if id.is_some() && id == previous {
                item.focus_changed(false);
            }
            if id.is_some() && id == holder {
                item.focus_changed(true);
            }
        });
        self.focus_holder = holder;
    }

    /// Split the pane at `path` with a new shell beside it.
    fn split(&mut self, path: &[usize], direction: SplitDirection) {
        if self.panes.group.leaf_count() >= MAX_PANES {
            return;
        }
        let Some(item) = self.spawn_item(None) else {
            return;
        };
        self.panes.split(path, direction, Some(Box::new(item)));
    }

    /// Close tab `index` in the pane at `path`; an emptied pane leaves the group, and the last one closes the
    /// panel.
    fn close_tab(&mut self, path: &[usize], index: usize) {
        self.panes.close_tab(path, index);
        if self.panes.is_empty() {
            self.closed_last = true;
        }
    }

    fn all_terminals(&mut self, f: &mut dyn FnMut(&mut TerminalItem)) {
        self.panes.for_each_item_mut(&mut |item| {
            if let Some(terminal) = item
                .as_any_mut()
                .and_then(|any| any.downcast_mut::<TerminalItem>())
            {
                f(terminal);
            }
        });
    }
}

impl TerminalPanelView for TerminalPanel {
    fn render(&mut self, region: Rect, focused: bool) -> (Painted, EditorLayout) {
        self.focused = focused;
        self.panes.set_focused(focused);
        self.reconcile_focus();
        let mut backdrop = Painted::default();
        if self.is_empty() {
            let message = self.spawn_error.clone().unwrap_or_default();
            let node = label(message).size(13.0).color(theme().text_muted).into();
            backdrop = ui::render(&node, region);
        }
        let (panes, dividers) = self.panes.layout(region);
        (backdrop, EditorLayout { panes, dividers })
    }

    fn click(&mut self, id: u64) -> bool {
        match self.panes.click(id) {
            GroupClick::NotMine => return false,
            GroupClick::Handled => {}
            GroupClick::Search { pane, click } => self.panes.search_click(pane, click),
            GroupClick::Button { path, action } => match action {
                PaneButtonAction::Split(direction) => self.split(&path, direction),
                PaneButtonAction::NewItem => {
                    if let Some(item) = self.spawn_item(None) {
                        if let Some(pane) = self.panes.group.leaf_at_mut(&path) {
                            pane.add_item(Box::new(item));
                        }
                    }
                }
            },
        }
        if self.panes.is_empty() {
            self.closed_last = true;
        }
        self.reconcile_focus();
        true
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        self.panes.set_hover(id)
    }

    fn divider_axis(&self, id: u64) -> Option<DividerAxis> {
        self.panes.divider_axis(id)
    }

    fn drag_divider(&mut self, id: u64, x: f32, y: f32) -> bool {
        self.panes.drag_divider(id, x, y)
    }

    fn is_tab(&self, id: u64) -> bool {
        self.panes.is_tab(id)
    }

    fn begin_tab_drag(&mut self, id: u64) -> bool {
        self.panes.begin_tab_drag(id)
    }

    fn update_tab_drag(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool {
        self.panes.update_tab_drag(x, y, over)
    }

    fn drop_tab(&mut self) -> bool {
        let dropped = self.panes.drop_tab();
        self.reconcile_focus();
        dropped
    }

    fn tab_drag_overlay(&self) -> Option<Rect> {
        self.panes.tab_drag_overlay()
    }

    fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)> {
        self.panes.tab_drag_ghost()
    }

    fn pane_command(&mut self, command: PaneCommand) -> bool {
        let handled = match command {
            PaneCommand::Split(direction) => {
                let path = self.panes.active.clone();
                self.split(&path, direction);
                true
            }
            command => self.panes.pane_command(command),
        };
        self.reconcile_focus();
        handled
    }

    fn dragged_item(&self) -> Option<&dyn Item> {
        self.panes.dragged_item()
    }

    fn take_dragged_item(&mut self) -> Option<Box<dyn Item>> {
        let item = self.panes.take_dragged_item()?;
        if self.is_empty() {
            self.closed_last = true;
        }
        self.reconcile_focus();
        Some(item)
    }

    fn accepts_item(&self, item: &dyn Item) -> bool {
        item.as_any().is_some_and(|any| any.is::<TerminalItem>())
    }

    fn update_foreign_drop(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool {
        self.panes.update_foreign_drop(x, y, over)
    }

    fn clear_foreign_drop(&mut self) {
        self.panes.clear_foreign_drop();
    }

    fn accept_foreign_item(&mut self, item: Box<dyn Item>) {
        self.closed_last = false;
        self.panes.accept_foreign_item(item);
        self.reconcile_focus();
    }

    fn grid_contains(&self, x: f32, y: f32) -> bool {
        self.panes
            .body_point(x, y)
            .and_then(|(path, _, _)| self.panes.pane_at(&path))
            .is_some_and(|pane| !pane.open.is_empty())
    }

    fn mouse_down(&mut self, x: f32, y: f32, click_count: u32, modifiers: Modifiers) -> bool {
        let Some(path) = self.panes.pane_path_at(x, y) else {
            return false;
        };
        self.panes.active = path.clone();
        if let Some(pane) = self.panes.group.leaf_at_mut(&path) {
            pane.search.focus = None;
        }
        self.reconcile_focus();
        let Some((path, item)) = self.terminal_at(x, y) else {
            return false;
        };
        item.mouse_down(x, y, click_count, modifiers);
        self.pressed_pane = Some(path);
        true
    }

    fn mouse_drag(&mut self, x: f32, y: f32, modifiers: Modifiers) -> bool {
        let Some(path) = self.pressed_pane.clone() else {
            return false;
        };
        self.panes
            .group
            .leaf_at_mut(&path)
            .and_then(terminal_of)
            .is_some_and(|item| item.mouse_drag(x, y, modifiers))
    }

    fn mouse_move(&mut self, x: f32, y: f32, modifiers: Modifiers) -> bool {
        let focused = self.focused;
        let active = self.panes.active.clone();
        let under = self.panes.pane_path_at(x, y);
        let mut changed = false;
        for path in self.panes.pane_order().to_vec() {
            let Some(item) = self.panes.group.leaf_at_mut(&path).and_then(terminal_of) else {
                continue;
            };
            if under.as_ref() == Some(&path) || item.hovering_link() {
                changed |= item.mouse_move(x, y, modifiers, focused && path == active);
            }
        }
        changed
    }

    fn mouse_up(&mut self, x: f32, y: f32, modifiers: Modifiers) {
        let Some(path) = self.pressed_pane.take() else {
            return;
        };
        if let Some(item) = self.panes.group.leaf_at_mut(&path).and_then(terminal_of) {
            item.mouse_up(x, y, modifiers);
        }
    }

    fn scroll(&mut self, x: f32, y: f32, delta_y: f32, modifiers: Modifiers) -> bool {
        match self.terminal_at(x, y) {
            Some((_, item)) => {
                item.scroll(x, y, delta_y, modifiers);
                true
            }
            None => false,
        }
    }

    fn key(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        self.panes.item_keystroke(keystroke)
    }

    fn text(&mut self, text: &str) {
        self.panes.item_text(text);
    }

    fn paste(&mut self, text: &str) {
        self.panes.item_paste(text);
    }

    fn focus_changed(&mut self, focused: bool) {
        self.focused = focused;
        self.panes.set_focused(focused);
        self.reconcile_focus();
    }

    fn sync(&mut self, clipboard: &dyn Fn() -> Option<String>) -> TerminalSyncOutcome {
        let host = Host {
            palette: crate::palette(&theme()),
            clipboard,
        };
        let mut outcome = TerminalSyncOutcome::default();
        let had_terminals = !self.is_empty();
        let mut exited: Vec<(u64, String)> = Vec::new();
        self.panes.group.for_each_pane_mut(&mut |pane| {
            let mut pane_changed = false;
            for item in pane.open.iter_mut() {
                let Some(id) = item.id() else {
                    continue;
                };
                let Some(terminal) = item
                    .as_any_mut()
                    .and_then(|any| any.downcast_mut::<TerminalItem>())
                else {
                    continue;
                };
                let result = terminal.sync(&host);
                pane_changed |= result.changed || result.title_changed;
                if result.clipboard_store.is_some() {
                    outcome.clipboard_store = result.clipboard_store;
                }
                if result.close {
                    exited.push((pane.id, id));
                }
            }
            if pane_changed {
                if let Some((bar, item)) = pane.search_target() {
                    if !bar.dismissed {
                        bar.refresh(item);
                    }
                }
            }
            outcome.changed |= pane_changed;
        });
        for (pane_id, item_id) in exited {
            let Some(path) = self.panes.group.path_of(pane_id) else {
                continue;
            };
            let index = self
                .panes
                .group
                .leaf_at(&path)
                .and_then(|pane| pane.index_of_id(&item_id));
            if let Some(index) = index {
                self.close_tab(&path, index);
                outcome.changed = true;
            }
        }
        outcome.closed_all =
            (had_terminals && self.is_empty()) || std::mem::take(&mut self.closed_last);
        outcome
    }

    fn open(&mut self, cwd: Option<PathBuf>) {
        self.closed_last = false;
        let Some(item) = self.spawn_item(cwd) else {
            return;
        };
        if let Some(pane) = self.active_pane() {
            pane.add_item(Box::new(item));
        }
    }

    fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    fn take_open_request(&mut self) -> Option<TerminalOpenTarget> {
        let mut request = None;
        self.all_terminals(&mut |item| {
            if request.is_none() {
                request = item.take_link_request();
            }
        });
        request
    }

    fn new_item(&mut self, cwd: Option<PathBuf>) -> Option<Box<dyn Item>> {
        self.spawn_item(cwd)
            .map(|item| Box::new(item) as Box<dyn Item>)
    }

    fn link_hovered(&self) -> bool {
        let mut hovered = false;
        self.panes.group.for_each_pane(&mut |pane| {
            hovered |= pane.active_item().is_some_and(|item| {
                item.as_any()
                    .and_then(|any| any.downcast_ref::<TerminalItem>())
                    .is_some_and(TerminalItem::hovering_link)
            });
        });
        hovered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminal::{Terminal, TerminalOptions};
    use ui::Rgba;

    fn panel() -> TerminalPanel {
        TerminalPanel::new(std::env::temp_dir(), std::sync::Arc::new(|| {}))
    }

    #[test]
    fn search_finds_newest_match_first_and_cycles() {
        let mut panel = panel();
        let terminal = Terminal::spawn(
            TerminalOptions {
                shell: Some((
                    "/bin/sh".into(),
                    vec![
                        "-c".into(),
                        "printf 'alpha one\\nbeta\\nalpha two'; sleep 5".into(),
                    ],
                )),
                ..TerminalOptions::default()
            },
            std::sync::Arc::new(|| {}),
        )
        .unwrap();
        let item = TerminalItem::with_terminal(0, std::env::temp_dir(), terminal);
        panel.active_pane().unwrap().add_item(Box::new(item));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !panel
            .active_terminal()
            .unwrap()
            .terminal()
            .screen_text()
            .contains("two")
        {
            assert!(std::time::Instant::now() < deadline, "no output");
            panel.sync(&|| None);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            panel.key(&Keystroke::parse("cmd-f")),
            TerminalKeyOutcome::Handled
        );
        panel.text("alpha");
        let pane = panel.active_pane().unwrap();
        assert_eq!(pane.search.matches.len(), 2);
        assert_eq!(pane.search.active_match, Some(1));
        let terminal = panel.active_terminal().unwrap().terminal();
        assert_eq!(terminal.selection_text().as_deref(), Some("alpha"));
        let highlighted = &terminal.content().search_matches;
        assert_eq!(highlighted.len(), 2);
        assert_eq!(highlighted[1].0.line - highlighted[0].0.line, 2);
        assert_eq!(
            panel.key(&Keystroke::parse("cmd-g")),
            TerminalKeyOutcome::Handled
        );
        assert_eq!(panel.active_pane().unwrap().search.active_match, Some(0));
        assert_eq!(
            panel.key(&Keystroke::parse("escape")),
            TerminalKeyOutcome::Handled
        );
        assert!(panel.active_pane().unwrap().search.dismissed);
        let terminal = panel.active_terminal().unwrap().terminal();
        assert!(terminal.content().search_matches.is_empty());
    }

    #[test]
    fn closing_the_last_tab_closes_the_panel() {
        let mut panel = panel();
        panel.open(None);
        panel.open(None);
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        panel.render(region, true);
        let close_first = panel.panes.tab_close_id(0, 0);
        assert!(panel.click(close_first));
        assert!(!panel.sync(&|| None).closed_all);
        panel.render(region, true);
        assert!(panel.click(close_first));
        assert!(panel.is_empty());
        assert!(panel.sync(&|| None).closed_all);
        assert!(!panel.sync(&|| None).closed_all);
    }

    #[test]
    fn panes_split_and_empty_panes_leave_the_group() {
        let mut panel = panel();
        panel.open(None);
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        panel.render(region, true);
        assert!(panel.click(panel.panes.button_id(0, 1)));
        assert_eq!(panel.panes.group.leaf_count(), 2);
        assert_eq!(panel.panes.active, vec![1]);
        let (_, layout) = panel.render(region, true);
        let divider = panel.panes.divider_id(0);
        assert!(layout.dividers.iter().any(|d| d.id == divider));
        assert!(matches!(
            panel.divider_axis(divider),
            Some(DividerAxis::Horizontal)
        ));
        assert!(panel.drag_divider(divider, 600.0, 10.0));
        assert!(panel.click(panel.panes.button_id(1, 2)));
        assert_eq!(panel.panes.group.leaf_count(), 3);
        panel.render(region, true);
        assert!(panel.click(panel.panes.tab_close_id(1, 0)));
        assert_eq!(panel.panes.group.leaf_count(), 2);
        assert!(!panel.is_empty());
        assert!(!panel.sync(&|| None).closed_all);
    }

    #[test]
    fn dragging_a_tab_to_an_edge_splits_and_back_merges() {
        let mut panel = panel();
        panel.open(None);
        panel.open(None);
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        panel.render(region, true);
        let first_tab = panel.panes.tab_id(0, 0);
        assert!(panel.is_tab(first_tab));
        assert!(panel.begin_tab_drag(first_tab));
        assert!(panel.update_tab_drag(790.0, 250.0, None));
        assert!(panel.tab_drag_overlay().is_some());
        assert!(panel.drop_tab());
        assert_eq!(panel.panes.group.leaf_count(), 2);
        assert_eq!(panel.panes.active, vec![1]);
        panel.render(region, true);
        let right_tab = panel.panes.tab_id(1, 0);
        assert!(panel.begin_tab_drag(right_tab));
        assert!(panel.update_tab_drag(200.0, 250.0, None));
        assert!(panel.drop_tab());
        assert_eq!(panel.panes.group.leaf_count(), 1);
        assert_eq!(
            panel.panes.group.leaf_at(&[]).map(|p| p.open.len()),
            Some(2)
        );
    }

    #[test]
    fn pane_keys_split_move_focus_and_swap() {
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        let mut panel = panel();
        panel.open(None);
        panel.render(region, true);
        assert!(panel.pane_command(PaneCommand::Split(SplitDirection::Right)));
        assert_eq!(panel.panes.group.leaf_count(), 2);
        assert_eq!(panel.panes.active, vec![1]);
        panel.render(region, true);
        assert!(!panel.pane_command(PaneCommand::ActivatePane(SplitDirection::Right)));
        assert!(panel.pane_command(PaneCommand::ActivatePane(SplitDirection::Left)));
        assert_eq!(panel.panes.active, vec![0]);
        let left_id = panel.panes.group.leaf_at(&[0]).map(|pane| pane.id);
        assert!(panel.pane_command(PaneCommand::SwapPane(SplitDirection::Right)));
        assert_eq!(panel.panes.active, vec![1]);
        assert_eq!(panel.panes.group.leaf_at(&[1]).map(|pane| pane.id), left_id);
    }

    #[test]
    fn a_dragged_tab_moves_to_another_group_and_back() {
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        let mut source = panel();
        source.open(None);
        source.render(region, true);
        let mut target = panel();
        target.open(None);
        target.render(region, true);

        assert!(source.begin_tab_drag(source.panes.tab_id(0, 0)));
        let accepted = source
            .dragged_item()
            .is_some_and(|item| target.accepts_item(item));
        assert!(accepted);
        assert!(target.update_foreign_drop(790.0, 250.0, None));
        assert!(target.tab_drag_overlay().is_some());
        let Some(item) = source.take_dragged_item() else {
            panic!("dragged item should detach");
        };
        assert!(source.is_empty());
        target.accept_foreign_item(item);
        assert_eq!(target.panes.group.leaf_count(), 2);
        assert_eq!(target.panes.active, vec![1]);
        assert!(target.tab_drag_overlay().is_none());

        target.clear_foreign_drop();
        target.render(region, true);
        assert!(target.begin_tab_drag(target.panes.tab_id(1, 0)));
        let Some(item) = target.take_dragged_item() else {
            panic!("dragged item should detach");
        };
        assert_eq!(target.panes.group.leaf_count(), 1);
        source.accept_foreign_item(item);
        assert!(!source.is_empty());
    }
}
