//! The terminal panel: a pane group of terminal items, driven by the workspace. Panes split, each with its
//! own tabs and find bar; closing the last terminal closes the panel.

use std::path::PathBuf;

use editor::search::Direction;
use terminal::{Keystroke, Modifiers, Waker};
use ui::{label, theme, IconKind, Node, Painted, Rect, Rgba};
use workspace::pane::{render_pane, Pane, PaneClickIds, TabBarButton, TabBarConfig};
use workspace::pane_group::{self, DividerRef, LeafPlacement, Member, SplitDirection};
use workspace::search_bar::{SearchBar, SearchClick, SearchField, SearchSupport};
use workspace::tab_drag::{self, DropTarget, TabDrag, TabDrop};
use workspace::{
    DividerAxis, EditKey, TerminalKeyOutcome, TerminalOpenTarget, TerminalPanelView,
    TerminalSyncOutcome, FUNC_VIEW_BASE, TERMINAL_VIEW_BASE,
};

use crate::item::{Host, TerminalItem};

/// Click ids are laid out per pane (in render order): tabs, then close buttons, then the tab bar buttons and
/// the find bar. Dividers take the block after the last pane's.
const PANE_STRIDE: u64 = 500;
const MAX_PANES: usize = 8;
const TAB_CLOSE_OFFSET: u64 = 100;
const NEW_TERMINAL_OFFSET: u64 = 200;
const SPLIT_RIGHT_OFFSET: u64 = 201;
const SPLIT_DOWN_OFFSET: u64 = 202;
/// Terminal panes hide the history arrows, so their ids point at an unused slot.
const NAV_UNUSED_OFFSET: u64 = 203;
const SEARCH_OFFSET: u64 = 300;
const DIVIDER_BASE: u64 = TERMINAL_VIEW_BASE + PANE_STRIDE * MAX_PANES as u64;

fn terminal_search() -> SearchBar {
    SearchBar::with_support(SearchSupport {
        case: false,
        word: false,
        regex: true,
        replace: false,
        select_all: false,
    })
}

/// Keys for the find bar's text field, from a terminal key press.
fn search_edit_key(keystroke: &Keystroke) -> Option<EditKey> {
    let m = keystroke.modifiers;
    Some(match keystroke.key.as_str() {
        "enter" => EditKey::Enter,
        "escape" => EditKey::Escape,
        "tab" => EditKey::Tab,
        "backspace" if m.cmd => EditKey::DeleteToLineStart,
        "backspace" if m.alt => EditKey::DeleteWordLeft,
        "backspace" => EditKey::Backspace,
        "delete" if m.alt => EditKey::DeleteWordRight,
        "delete" => EditKey::Delete,
        "left" if m.cmd => EditKey::LineStart,
        "left" if m.alt => EditKey::WordLeft,
        "left" => EditKey::Left,
        "right" if m.cmd => EditKey::LineEnd,
        "right" if m.alt => EditKey::WordRight,
        "right" => EditKey::Right,
        "home" => EditKey::Home,
        "end" => EditKey::End,
        "up" => EditKey::Up,
        "down" => EditKey::Down,
        "a" if m.cmd => EditKey::SelectAll,
        "z" if m.cmd && m.shift => EditKey::Redo,
        "z" if m.cmd => EditKey::Undo,
        _ => return None,
    })
}

fn terminal_of(pane: &mut Pane) -> Option<&mut TerminalItem> {
    pane.active_item_mut()?
        .as_any_mut()?
        .downcast_mut::<TerminalItem>()
}

fn divider_axis(axis: pane_group::Axis) -> DividerAxis {
    match axis {
        pane_group::Axis::Horizontal => DividerAxis::Horizontal,
        pane_group::Axis::Vertical => DividerAxis::Vertical,
    }
}

fn contains(rect: &Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

fn blit(painted: &mut Painted, part: Painted) {
    painted.rects.extend(part.rects);
    painted.tris.extend(part.tris);
    painted.texts.extend(part.texts);
    painted.icons.extend(part.icons);
    painted.hits.extend(part.hits);
}

pub struct TerminalPanel {
    root: PathBuf,
    waker: Waker,
    group: Member<Pane>,
    /// Path of the focused pane in the group.
    active: Vec<usize>,
    next_pane_id: u64,
    next_item_id: u64,
    /// Rebuilt each render: leaves in render order (which key the click ids) and the dividers.
    leaves: Vec<LeafPlacement>,
    dividers: Vec<DividerRef>,
    /// The pane a press landed in, so the drag and release go to the same terminal.
    pressed_pane: Option<Vec<usize>>,
    hover: Option<u64>,
    focused: bool,
    spawn_error: Option<String>,
    /// The user closed the last terminal; reported on the next sync so the panel closes like when shells exit.
    closed_last: bool,
    tab_drag: Option<TabDrag>,
}

impl TerminalPanel {
    pub fn new(root: PathBuf, waker: Waker) -> Self {
        Self {
            root,
            waker,
            group: Member::Leaf(Pane {
                search: Box::new(terminal_search()),
                ..Pane::new(0)
            }),
            active: Vec::new(),
            next_pane_id: 1,
            next_item_id: 0,
            leaves: Vec::new(),
            dividers: Vec::new(),
            pressed_pane: None,
            hover: None,
            focused: false,
            spawn_error: None,
            closed_last: false,
            tab_drag: None,
        }
    }

    fn new_pane(&mut self) -> Pane {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        Pane {
            search: Box::new(terminal_search()),
            ..Pane::new(id)
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
        if self.group.leaf_at(&self.active).is_none() {
            self.active = self.group.first_leaf_path();
        }
        self.group.leaf_at_mut(&self.active)
    }

    fn active_terminal(&mut self) -> Option<&mut TerminalItem> {
        terminal_of(self.active_pane()?)
    }

    fn pane_path_at(&self, x: f32, y: f32) -> Option<Vec<usize>> {
        pane_group::leaf_at_point(&self.leaves, x, y).map(|leaf| leaf.path.clone())
    }

    fn terminal_at(&mut self, x: f32, y: f32) -> Option<(Vec<usize>, &mut TerminalItem)> {
        let path = self.pane_path_at(x, y)?;
        let item = terminal_of(self.group.leaf_at_mut(&path)?)?;
        item.body_contains(x, y).then_some((path, item))
    }

    fn ids(p: usize) -> PaneClickIds {
        let base = TERMINAL_VIEW_BASE + p as u64 * PANE_STRIDE;
        PaneClickIds {
            tab_activate: base,
            tab_close: base + TAB_CLOSE_OFFSET,
            nav_back: base + NAV_UNUSED_OFFSET,
            nav_forward: base + NAV_UNUSED_OFFSET,
            search: base + SEARCH_OFFSET,
        }
    }

    fn tab_bar(p: usize) -> TabBarConfig {
        let base = TERMINAL_VIEW_BASE + p as u64 * PANE_STRIDE;
        TabBarConfig {
            show_nav: false,
            buttons: vec![
                TabBarButton {
                    icon: IconKind::Plus,
                    id: base + NEW_TERMINAL_OFFSET,
                },
                TabBarButton {
                    icon: IconKind::PanelRight,
                    id: base + SPLIT_RIGHT_OFFSET,
                },
                TabBarButton {
                    icon: IconKind::PanelBottom,
                    id: base + SPLIT_DOWN_OFFSET,
                },
            ],
        }
    }

    /// Split the pane at `path` with a new shell beside it.
    fn split(&mut self, path: &[usize], direction: SplitDirection) {
        if self.group.leaf_count() >= MAX_PANES {
            return;
        }
        let Some(item) = self.spawn_item(None) else {
            return;
        };
        let mut pane = self.new_pane();
        pane.add_item(Box::new(item));
        if let Some(new_path) = self.group.split(path, direction, pane) {
            self.set_active(new_path);
        }
    }

    fn set_active(&mut self, path: Vec<usize>) {
        if self.active == path {
            return;
        }
        let focused = self.focused;
        if let Some(item) = self.active_terminal() {
            item.focus_changed(false);
        }
        self.active = path;
        if let Some(item) = self.active_terminal() {
            item.focus_changed(focused);
        }
    }

    /// Close tab `index` in the pane at `path`; an emptied pane leaves the group, and the last one closes the
    /// panel.
    fn close_tab(&mut self, path: &[usize], index: usize) {
        let Some(pane) = self.group.leaf_at_mut(path) else {
            return;
        };
        pane.close_tab(index);
        if !pane.open.is_empty() {
            return;
        }
        if self.group.remove(path) {
            self.active = self.group.first_leaf_path();
        } else {
            self.closed_last = true;
        }
    }

    fn search_focused(&mut self) -> bool {
        self.active_pane()
            .is_some_and(|pane| !pane.search.dismissed && pane.search.focus.is_some())
    }

    /// Find-bar commands for the focused pane; returns whether `keystroke` was one.
    fn search_command(&mut self, keystroke: &Keystroke) -> bool {
        let m = keystroke.modifiers;
        let only_cmd = m.cmd && !m.alt && !m.ctrl;
        let Some(pane) = self.active_pane() else {
            return false;
        };
        let dismissed = pane.search.dismissed;
        let Some((bar, item)) = pane.search_target() else {
            return false;
        };
        match keystroke.key.as_str() {
            "f" if only_cmd && !m.shift => {
                if dismissed {
                    bar.deploy(item, false);
                } else {
                    bar.focus = Some(SearchField::Query);
                    bar.query.select_all();
                }
            }
            "g" if only_cmd && !dismissed => {
                let direction = if m.shift {
                    Direction::Prev
                } else {
                    Direction::Next
                };
                bar.select_match(item, direction);
            }
            "x" if m.cmd && m.alt && !m.ctrl && !dismissed => {
                bar.toggle_option(item, SearchClick::Regex);
            }
            _ => return false,
        }
        true
    }

    fn search_click(&mut self, path: &[usize], click: SearchClick) {
        let Some(pane) = self.group.leaf_at_mut(path) else {
            return;
        };
        let Some((bar, item)) = pane.search_target() else {
            return;
        };
        match click {
            SearchClick::Query => bar.focus = Some(SearchField::Query),
            SearchClick::Next => bar.select_match(item, Direction::Next),
            SearchClick::Previous => bar.select_match(item, Direction::Prev),
            SearchClick::Close => bar.dismiss(item),
            SearchClick::Regex => bar.toggle_option(item, click),
            _ => {}
        }
    }

    /// The pane (render index) and tab index a tab click id points at.
    fn tab_of(&self, id: u64) -> Option<(usize, usize)> {
        if id >= DIVIDER_BASE {
            return None;
        }
        let offset = id.checked_sub(TERMINAL_VIEW_BASE)?;
        let within = offset % PANE_STRIDE;
        (within < TAB_CLOSE_OFFSET).then_some(((offset / PANE_STRIDE) as usize, within as usize))
    }

    fn all_terminals(&mut self, f: &mut dyn FnMut(&mut TerminalItem)) {
        self.group.for_each_pane_mut(&mut |pane| {
            for item in pane.open.iter_mut() {
                if let Some(terminal) = item
                    .as_any_mut()
                    .and_then(|any| any.downcast_mut::<TerminalItem>())
                {
                    f(terminal);
                }
            }
        });
    }
}

impl TerminalPanelView for TerminalPanel {
    fn render(&mut self, region: Rect, focused: bool) -> Painted {
        self.focused = focused;
        let scale = ui::ui_text_scale();
        let (leaves, dividers) = self.group.layout(region);
        let mut painted = Painted::default();
        if self.is_empty() {
            let message = self.spawn_error.clone().unwrap_or_default();
            let node = label(message).size(13.0).color(theme().text_muted).into();
            blit(&mut painted, ui::render(&node, region));
        }
        let hover = self.hover;
        let active = self.active.clone();
        for (p, leaf) in leaves.iter().enumerate() {
            let Some(pane) = self.group.leaf_at_mut(&leaf.path) else {
                continue;
            };
            if let Some((bar, item)) = pane.search_target() {
                bar.refresh(item);
            }
            let chrome_h = pane.header_h() * scale;
            let body = Rect::new(
                leaf.rect.x,
                leaf.rect.y + chrome_h,
                leaf.rect.w,
                (leaf.rect.h - chrome_h).max(0.0),
                Rgba::TRANSPARENT,
            );
            let is_active = leaf.path == active;
            if let Some(item) = terminal_of(pane) {
                blit(&mut painted, item.paint(body, focused && is_active));
            }
            let chrome = render_pane(
                pane,
                &Self::tab_bar(p),
                Self::ids(p),
                hover,
                leaf.rect.w / scale,
            );
            blit(&mut painted, ui::render(&chrome, leaf.rect));
        }
        let inset = (pane_group::DIVIDER_GRAB - pane_group::DIVIDER_LINE) / 2.0;
        for (index, divider) in dividers.iter().enumerate() {
            let grab = divider.rect;
            let line = match divider.reference.axis {
                pane_group::Axis::Horizontal => Rect::new(
                    grab.x + inset,
                    grab.y,
                    pane_group::DIVIDER_LINE,
                    grab.h,
                    theme().border,
                ),
                pane_group::Axis::Vertical => Rect::new(
                    grab.x,
                    grab.y + inset,
                    grab.w,
                    pane_group::DIVIDER_LINE,
                    theme().border,
                ),
            };
            painted.rects.push(line);
            painted.hits.push((grab, DIVIDER_BASE + index as u64));
        }
        self.dividers = dividers.into_iter().map(|d| d.reference).collect();
        self.leaves = leaves;
        painted
    }

    fn click(&mut self, id: u64) -> bool {
        let Some(offset) = id.checked_sub(TERMINAL_VIEW_BASE) else {
            return false;
        };
        if id >= DIVIDER_BASE {
            return true;
        }
        let p = (offset / PANE_STRIDE) as usize;
        let within = offset % PANE_STRIDE;
        let path = self
            .leaves
            .get(p)
            .map(|leaf| leaf.path.clone())
            .unwrap_or_else(|| self.active.clone());
        self.set_active(path.clone());
        match within {
            SPLIT_RIGHT_OFFSET => self.split(&path, SplitDirection::Right),
            SPLIT_DOWN_OFFSET => self.split(&path, SplitDirection::Down),
            NEW_TERMINAL_OFFSET => {
                if let Some(item) = self.spawn_item(None) {
                    if let Some(pane) = self.group.leaf_at_mut(&path) {
                        pane.add_item(Box::new(item));
                    }
                }
            }
            offset if offset >= SEARCH_OFFSET => {
                if let Some(click) = SearchClick::from_offset(offset - SEARCH_OFFSET) {
                    self.search_click(&path, click);
                }
            }
            offset if offset >= TAB_CLOSE_OFFSET => {
                self.close_tab(&path, (offset - TAB_CLOSE_OFFSET) as usize);
            }
            index => {
                if let Some(pane) = self.group.leaf_at_mut(&path) {
                    if (index as usize) < pane.open.len() {
                        pane.activate_user(index as usize);
                    }
                }
            }
        }
        true
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        let id = id.filter(|id| (TERMINAL_VIEW_BASE..FUNC_VIEW_BASE).contains(id));
        let changed = self.hover != id;
        self.hover = id;
        changed
    }

    fn divider_axis(&self, id: u64) -> Option<DividerAxis> {
        let index = id.checked_sub(DIVIDER_BASE)? as usize;
        self.dividers.get(index).map(|d| divider_axis(d.axis))
    }

    fn drag_divider(&mut self, id: u64, x: f32, y: f32) -> bool {
        let Some(divider) = id
            .checked_sub(DIVIDER_BASE)
            .and_then(|index| self.dividers.get(index as usize))
            .cloned()
        else {
            return false;
        };
        self.group.resize_divider(&divider, x, y)
    }

    fn is_tab(&self, id: u64) -> bool {
        self.tab_of(id).is_some_and(|(p, index)| {
            self.leaves
                .get(p)
                .and_then(|leaf| self.group.leaf_at(&leaf.path))
                .is_some_and(|pane| index < pane.open.len())
        })
    }

    fn begin_tab_drag(&mut self, id: u64) -> bool {
        let Some((p, index)) = self.tab_of(id) else {
            return false;
        };
        self.tab_drag = self
            .leaves
            .get(p)
            .and_then(|leaf| self.group.leaf_at(&leaf.path))
            .and_then(|pane| TabDrag::begin(pane, index));
        self.tab_drag.is_some()
    }

    fn update_tab_drag(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool {
        if self.tab_drag.is_none() {
            return false;
        }
        let full = self.group.leaf_count() >= MAX_PANES;
        let resolved = self
            .leaves
            .iter()
            .enumerate()
            .find(|(_, leaf)| contains(&leaf.rect, x, y))
            .and_then(|(p, leaf)| {
                let pane = self.group.leaf_at(&leaf.path)?;
                let over_tab = over.and_then(|(id, rect)| {
                    let (tab_pane, index) = self.tab_of(id)?;
                    (tab_pane == p).then_some((index, rect))
                });
                let (mut target, preview) =
                    tab_drag::resolve_drop(leaf.rect, pane.open.len(), x, y, over_tab);
                if full && matches!(target, DropTarget::Split(_)) {
                    target = DropTarget::Append;
                }
                Some((
                    TabDrop {
                        pane: pane.id,
                        target,
                    },
                    preview,
                ))
            });
        if let Some(drag) = self.tab_drag.as_mut() {
            drag.drop = resolved.map(|(drop, _)| drop);
            drag.preview = resolved.map(|(_, preview)| preview);
        }
        true
    }

    fn drop_tab(&mut self) -> bool {
        let Some(drag) = self.tab_drag.take() else {
            return false;
        };
        let Some(drop) = drag.drop else {
            return false;
        };
        let pane = self.new_pane();
        if let Some(path) = tab_drag::apply_drop(&mut self.group, &drag, drop, || pane) {
            self.set_active(path);
        } else if self.group.leaf_at(&self.active).is_none() {
            self.active = self.group.first_leaf_path();
        }
        true
    }

    fn tab_drag_overlay(&self) -> Option<Rect> {
        self.tab_drag.as_ref()?.preview
    }

    fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)> {
        Some(self.tab_drag.as_ref()?.ghost())
    }

    fn grid_contains(&self, x: f32, y: f32) -> bool {
        let Some(leaf) = pane_group::leaf_at_point(&self.leaves, x, y) else {
            return false;
        };
        let Some(pane) = self.group.leaf_at(&leaf.path) else {
            return false;
        };
        let chrome = pane.header_h() * ui::ui_text_scale();
        let body = Rect::new(
            leaf.rect.x,
            leaf.rect.y + chrome,
            leaf.rect.w,
            (leaf.rect.h - chrome).max(0.0),
            Rgba::TRANSPARENT,
        );
        !pane.open.is_empty() && contains(&body, x, y)
    }

    fn mouse_down(&mut self, x: f32, y: f32, click_count: u32, modifiers: Modifiers) -> bool {
        let Some(path) = self.pane_path_at(x, y) else {
            return false;
        };
        self.set_active(path.clone());
        if let Some(pane) = self.group.leaf_at_mut(&path) {
            pane.search.focus = None;
        }
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
        self.group
            .leaf_at_mut(&path)
            .and_then(terminal_of)
            .is_some_and(|item| item.mouse_drag(x, y, modifiers))
    }

    fn mouse_move(&mut self, x: f32, y: f32, modifiers: Modifiers) -> bool {
        let focused = self.focused;
        let active = self.active.clone();
        let under = self.pane_path_at(x, y);
        let paths: Vec<Vec<usize>> = self.leaves.iter().map(|leaf| leaf.path.clone()).collect();
        let mut changed = false;
        for path in paths {
            let Some(item) = self.group.leaf_at_mut(&path).and_then(terminal_of) else {
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
        if let Some(item) = self.group.leaf_at_mut(&path).and_then(terminal_of) {
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
        if self.active_terminal().is_none() {
            return TerminalKeyOutcome::Ignored;
        }
        if self.search_command(keystroke) {
            return TerminalKeyOutcome::Handled;
        }
        if self.search_focused() {
            let m = keystroke.modifiers;
            if m.cmd && keystroke.key == "v" {
                return TerminalKeyOutcome::Paste;
            }
            let Some(pane) = self.active_pane() else {
                return TerminalKeyOutcome::Ignored;
            };
            if m.cmd && keystroke.key == "c" {
                return pane
                    .search
                    .query
                    .selected_text()
                    .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy);
            }
            return match search_edit_key(keystroke) {
                Some(key) => {
                    let keep = pane
                        .search_target()
                        .is_some_and(|(bar, item)| bar.key(item, key, m.shift));
                    if !keep {
                        pane.search.focus = None;
                    }
                    TerminalKeyOutcome::Handled
                }
                None if m.cmd || m.ctrl => TerminalKeyOutcome::Handled,
                None => TerminalKeyOutcome::Ignored,
            };
        }
        self.active_terminal()
            .map_or(TerminalKeyOutcome::Ignored, |item| item.key(keystroke))
    }

    fn text(&mut self, text: &str) {
        if self.search_focused() {
            if let Some((bar, item)) = self.active_pane().and_then(Pane::search_target) {
                bar.input(item, text);
            }
            return;
        }
        if let Some(item) = self.active_terminal() {
            item.text(text);
        }
    }

    fn paste(&mut self, text: &str) {
        if self.search_focused() {
            if let Some((bar, item)) = self.active_pane().and_then(Pane::search_target) {
                bar.input(item, text);
            }
            return;
        }
        if let Some(item) = self.active_terminal() {
            item.paste(text);
        }
    }

    fn focus_changed(&mut self, focused: bool) {
        self.focused = focused;
        if let Some(item) = self.active_terminal() {
            item.focus_changed(focused);
        }
    }

    fn sync(&mut self, clipboard: &dyn Fn() -> Option<String>) -> TerminalSyncOutcome {
        let host = Host {
            palette: crate::palette(&theme()),
            clipboard,
        };
        let mut outcome = TerminalSyncOutcome::default();
        let had_terminals = !self.is_empty();
        let mut exited: Vec<(u64, String)> = Vec::new();
        self.group.for_each_pane_mut(&mut |pane| {
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
            let Some(path) = self.group.path_of(pane_id) else {
                continue;
            };
            let index = self
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
        let mut empty = true;
        self.group
            .for_each_pane(&mut |pane| empty &= pane.open.is_empty());
        empty
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

    fn new_item(&mut self, cwd: Option<PathBuf>) -> Option<Box<dyn workspace::Item>> {
        self.spawn_item(cwd)
            .map(|item| Box::new(item) as Box<dyn workspace::Item>)
    }

    fn link_hovered(&self) -> bool {
        let mut hovered = false;
        self.group.for_each_pane(&mut |pane| {
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
        assert!(panel.search_command(&Keystroke::parse("cmd-f")));
        panel.text("alpha");
        let pane = panel.active_pane().unwrap();
        assert_eq!(pane.search.matches.len(), 2);
        assert_eq!(pane.search.active_match, Some(1));
        let terminal = panel.active_terminal().unwrap().terminal();
        assert_eq!(terminal.selection_text().as_deref(), Some("alpha"));
        let highlighted = &terminal.content().search_matches;
        assert_eq!(highlighted.len(), 2);
        assert_eq!(highlighted[1].0.line - highlighted[0].0.line, 2);
        assert!(panel.search_command(&Keystroke::parse("cmd-g")));
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
        let close_first = TERMINAL_VIEW_BASE + TAB_CLOSE_OFFSET;
        assert!(panel.click(close_first));
        assert!(!panel.sync(&|| None).closed_all);
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
        assert!(panel.click(TERMINAL_VIEW_BASE + SPLIT_RIGHT_OFFSET));
        assert_eq!(panel.group.leaf_count(), 2);
        assert_eq!(panel.active, vec![1]);
        let painted = panel.render(region, true);
        assert!(painted.hits.iter().any(|(_, id)| *id == DIVIDER_BASE));
        assert!(matches!(
            panel.divider_axis(DIVIDER_BASE),
            Some(DividerAxis::Horizontal)
        ));
        assert!(panel.drag_divider(DIVIDER_BASE, 600.0, 10.0));
        assert!(panel.click(TERMINAL_VIEW_BASE + PANE_STRIDE + SPLIT_DOWN_OFFSET));
        assert_eq!(panel.group.leaf_count(), 3);
        panel.render(region, true);
        let second = TERMINAL_VIEW_BASE + PANE_STRIDE;
        assert!(panel.click(second + TAB_CLOSE_OFFSET));
        assert_eq!(panel.group.leaf_count(), 2);
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
        let first_tab = TERMINAL_VIEW_BASE;
        assert!(panel.is_tab(first_tab));
        assert!(panel.begin_tab_drag(first_tab));
        assert!(panel.update_tab_drag(790.0, 250.0, None));
        assert!(panel.tab_drag_overlay().is_some());
        assert!(panel.drop_tab());
        assert_eq!(panel.group.leaf_count(), 2);
        assert_eq!(panel.active, vec![1]);
        panel.render(region, true);
        let right_tab = TERMINAL_VIEW_BASE + PANE_STRIDE;
        assert!(panel.begin_tab_drag(right_tab));
        assert!(panel.update_tab_drag(200.0, 250.0, None));
        assert!(panel.drop_tab());
        assert_eq!(panel.group.leaf_count(), 1);
        assert_eq!(panel.group.leaf_at(&[]).map(|p| p.open.len()), Some(2));
    }
}
