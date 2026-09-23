//! A pane group as a view: the split tree of panes, which one has focus, and what acts on it by pointer or key
//! (tab clicks, the tab bar buttons, dividers, tab dragging, pane commands), plus laying the panes out. Every
//! place that hosts tabs (the editor area, the terminal panel, later other features) owns one and configures its
//! tab bar, its click-id range and how many panes it allows.

use editor::search::Direction;
use ui::{IconKind, Node, Rect, Rgba};

use crate::pane::{render_pane, Pane, PaneClickIds, PaneCommand, TabBarButton, TabBarConfig};
use crate::pane_group::{self, Axis, DividerRef, LeafPlacement, Member, SplitDirection};
use crate::search_bar::{SearchBar, SearchClick, SearchField, Searchable};
use crate::tab_drag::{self, DropTarget, TabDrag, TabDrop};
use crate::{DividerAxis, DividerPlacement, EditKey, Item, PaneBody, PanePlacement};

const TAB_ACTIVATE: u64 = 1_000_000;
const TAB_CLOSE: u64 = 3_000_000;
const BUTTON: u64 = 5_000_000;
const NAV_BACK: u64 = 7_000_000;
const NAV_FORWARD: u64 = 8_000_000;
const SEARCH: u64 = 9_000_000;
const FOLD: u64 = 10_000_000;
const HUNK: u64 = 11_000_000;
const DIVIDER: u64 = 12_000_000;
/// Click ids from `id_base` up to `id_base + ID_SPAN` belong to the group; the owner keeps its own ids outside
/// `[id_base + TAB_ACTIVATE, id_base + ID_SPAN)`.
pub const ID_SPAN: u64 = 13_000_000;
const PANE_STRIDE: u64 = 100_000;
const BUTTON_STRIDE: u64 = 16;
/// An item's gutter gets fold ids from a per-pane base; its change-strip ids sit this far above them.
pub const HUNK_FROM_FOLD: u64 = HUNK - FOLD;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneButtonAction {
    Split(SplitDirection),
    NewItem,
}

#[derive(Clone, Copy)]
pub struct PaneButton {
    pub icon: IconKind,
    pub action: PaneButtonAction,
}

pub struct PaneGroupConfig {
    pub id_base: u64,
    pub show_nav: bool,
    pub buttons: Vec<PaneButton>,
    pub max_panes: usize,
}

/// A click the group leaves to its owner, which decides what the action means for its items.
#[derive(Clone, Debug, PartialEq)]
pub enum GroupClick {
    NotMine,
    Handled,
    Search {
        pane: usize,
        click: SearchClick,
    },
    Button {
        path: Vec<usize>,
        action: PaneButtonAction,
    },
}

pub struct PaneGroupView {
    config: PaneGroupConfig,
    pub group: Member<Pane>,
    /// Path of the focused leaf (empty when the group is a single pane).
    pub active: Vec<usize>,
    next_pane_id: u64,
    /// Rebuilt each layout: pane render index -> its path and screen rect, plus divider metadata by index.
    pane_order: Vec<Vec<usize>>,
    pane_rects: Vec<Rect>,
    divider_order: Vec<DividerRef>,
    hover: Option<u64>,
    tab_drag: Option<TabDrag>,
    foreign_drop: Option<(TabDrop, Rect)>,
    /// The focused pane's content area as last laid out, for placing popovers at its caret.
    focused_content: Option<Rect>,
}

impl PaneGroupView {
    pub fn new(config: PaneGroupConfig) -> Self {
        Self {
            config,
            group: Member::Leaf(Pane::new(0)),
            active: Vec::new(),
            next_pane_id: 1,
            pane_order: Vec::new(),
            pane_rects: Vec::new(),
            divider_order: Vec::new(),
            hover: None,
            tab_drag: None,
            foreign_drop: None,
            focused_content: None,
        }
    }

    pub fn new_pane(&mut self) -> Pane {
        let pane = Pane::new(self.next_pane_id);
        self.next_pane_id += 1;
        pane
    }

    pub fn pane_order(&self) -> &[Vec<usize>] {
        &self.pane_order
    }

    pub fn pane_rects(&self) -> &[Rect] {
        &self.pane_rects
    }

    pub fn focused_content(&self) -> Option<Rect> {
        self.focused_content
    }

    pub fn hover(&self) -> Option<u64> {
        self.hover
    }

    pub fn pane_at(&self, path: &[usize]) -> Option<&Pane> {
        self.group.leaf_at(path)
    }

    /// The focused pane, re-anchoring a stale active path to the first leaf.
    pub fn active_pane_mut(&mut self) -> Option<&mut Pane> {
        if self.group.leaf_at_mut(&self.active).is_none() {
            self.active = self.group.first_leaf_path();
        }
        let path = self.active.clone();
        self.group.leaf_at_mut(&path)
    }

    pub fn active_item_mut(&mut self) -> Option<&mut dyn Item> {
        self.active_pane_mut()?.active_item_mut()
    }

    pub fn active_item(&self) -> Option<&dyn Item> {
        self.pane_at(&self.active)?.active_item()
    }

    fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        self.pane_rects.iter().position(|r| contains(r, x, y))
    }

    pub fn pane_path_at(&self, x: f32, y: f32) -> Option<Vec<usize>> {
        self.pane_order.get(self.index_at(x, y)?).cloned()
    }

    pub fn pane_rect_of(&self, path: &[usize]) -> Option<Rect> {
        let index = self.pane_order.iter().position(|p| p == path)?;
        self.pane_rects.get(index).copied()
    }

    /// The pane under `(x, y)` and the point relative to its body (below the tab bar); `None` over a tab bar.
    pub fn body_point(&self, x: f32, y: f32) -> Option<(Vec<usize>, f32, f32)> {
        let index = self.index_at(x, y)?;
        let rect = *self.pane_rects.get(index)?;
        let path = self.pane_order.get(index)?.clone();
        let header = self.pane_at(&path).map_or(0.0, Pane::header_h);
        let local_y = y - (rect.y + header);
        if local_y < 0.0 {
            return None;
        }
        Some((path, (x - rect.x).max(0.0), local_y))
    }

    pub fn clone_active_of(&mut self, path: &[usize]) -> Option<Box<dyn Item>> {
        self.group.leaf_at(path)?.active_item()?.clone_on_split()
    }

    /// Split the pane at `path` with a new pane holding `item` (empty when `None`) and focus it; refused once the
    /// group holds `max_panes`.
    pub fn split(
        &mut self,
        path: &[usize],
        direction: SplitDirection,
        item: Option<Box<dyn Item>>,
    ) -> Option<Vec<usize>> {
        if self.group.leaf_count() >= self.config.max_panes {
            return None;
        }
        let mut pane = self.new_pane();
        if let Some(item) = item {
            pane.add_item(item);
        }
        let new_path = self.group.split(path, direction, pane).ok()?;
        self.active = new_path.clone();
        Some(new_path)
    }

    /// Close tab `index` in the pane at `path`; an emptied pane leaves the group unless it is the last one.
    pub fn close_tab(&mut self, path: &[usize], index: usize) {
        let Some(pane) = self.group.leaf_at_mut(path) else {
            return;
        };
        pane.close_tab(index);
        if pane.open.is_empty() && self.group.leaf_count() > 1 {
            self.remove_pane(path);
        }
    }

    pub fn remove_pane(&mut self, path: &[usize]) {
        if self.group.remove(path) {
            self.active = self.group.first_leaf_path();
        }
    }

    pub fn is_empty(&self) -> bool {
        let mut empty = true;
        self.group
            .for_each_pane(&mut |pane| empty &= pane.open.is_empty());
        empty
    }

    pub fn for_each_item_mut(&mut self, f: &mut dyn FnMut(&mut dyn Item)) {
        self.group.for_each_pane_mut(&mut |pane| {
            for item in pane.open.iter_mut() {
                f(item.as_mut());
            }
        });
    }

    fn pane_ids(&self, p: usize) -> PaneClickIds {
        let base = self.config.id_base;
        let p = p as u64;
        PaneClickIds {
            tab_activate: base + TAB_ACTIVATE + p * PANE_STRIDE,
            tab_close: base + TAB_CLOSE + p * PANE_STRIDE,
            nav_back: base + NAV_BACK + p,
            nav_forward: base + NAV_FORWARD + p,
            search: base + SEARCH + p * PANE_STRIDE,
        }
    }

    fn tab_bar(&self, p: usize) -> TabBarConfig {
        let base = self.config.id_base + BUTTON + p as u64 * BUTTON_STRIDE;
        TabBarConfig {
            show_nav: self.config.show_nav,
            buttons: self
                .config
                .buttons
                .iter()
                .enumerate()
                .map(|(index, button)| TabBarButton {
                    icon: button.icon,
                    id: base + index as u64,
                })
                .collect(),
        }
    }

    fn render_chrome(&self, pane: &Pane, p: usize, width: f32) -> Node {
        render_pane(pane, &self.tab_bar(p), self.pane_ids(p), self.hover, width)
    }

    /// Lay the panes out in `area`: each pane's chrome and body, and the dividers between them.
    pub fn layout(&mut self, area: Rect) -> (Vec<PanePlacement>, Vec<DividerPlacement>) {
        let mut placements = Vec::new();
        let mut pane_order = Vec::new();
        let mut pane_rects = Vec::new();
        let (leaves, dividers) = self.group.layout(area);
        for (p, leaf) in leaves.into_iter().enumerate() {
            let is_focused = leaf.path == self.active;
            let fold_base = self.config.id_base + FOLD + p as u64 * PANE_STRIDE;
            let Some(pane) = self.group.leaf_at_mut(&leaf.path) else {
                continue;
            };
            let (painted, body) = layout_body(pane, leaf.rect, is_focused, fold_base);
            let chrome = match self.group.leaf_at(&leaf.path) {
                Some(pane) => self.render_chrome(pane, p, leaf.rect.w / ui::ui_text_scale()),
                None => continue,
            };
            placements.push(PanePlacement {
                rect: leaf.rect,
                node: chrome,
                painted,
                ..body
            });
            pane_rects.push(leaf.rect);
            pane_order.push(leaf.path);
        }
        let mut divider_placements = Vec::with_capacity(dividers.len());
        let mut divider_order = Vec::with_capacity(dividers.len());
        for (index, divider) in dividers.into_iter().enumerate() {
            divider_placements.push(DividerPlacement {
                rect: divider.rect,
                id: self.config.id_base + DIVIDER + index as u64,
                axis: divider_axis(divider.reference.axis),
            });
            divider_order.push(divider.reference);
        }
        self.pane_order = pane_order;
        self.pane_rects = pane_rects;
        self.divider_order = divider_order;
        self.focused_content = self.pane_rect_of(&self.active).and_then(|rect| {
            let header = self.pane_at(&self.active)?.header_h();
            Some(Rect::new(
                rect.x,
                rect.y + header,
                rect.w.max(0.0),
                (rect.h - header).max(0.0),
                Rgba::TRANSPARENT,
            ))
        });
        (placements, divider_placements)
    }

    fn offset(&self, id: u64) -> Option<u64> {
        id.checked_sub(self.config.id_base)
            .filter(|offset| (TAB_ACTIVATE..ID_SPAN).contains(offset))
    }

    pub fn owns(&self, id: u64) -> bool {
        self.offset(id).is_some()
    }

    fn active_item_of(&mut self, p: usize) -> Option<&mut dyn Item> {
        let path = self.pane_order.get(p)?.clone();
        self.group.leaf_at_mut(&path)?.active_item_mut()
    }

    pub fn click(&mut self, id: u64) -> GroupClick {
        let Some(offset) = self.offset(id) else {
            return GroupClick::NotMine;
        };
        let per_pane = |base: u64| {
            let n = offset - base;
            ((n / PANE_STRIDE) as usize, n % PANE_STRIDE)
        };
        if offset >= DIVIDER {
            return GroupClick::Handled;
        }
        if offset >= HUNK {
            let (p, line) = per_pane(HUNK);
            if let Some(item) = self.active_item_of(p) {
                item.toggle_diff_hunk(line as usize);
            }
            return GroupClick::Handled;
        }
        if offset >= FOLD {
            let (p, line) = per_pane(FOLD);
            if let Some(item) = self.active_item_of(p) {
                item.toggle_fold(line as usize);
            }
            return GroupClick::Handled;
        }
        if offset >= SEARCH {
            let (pane, within) = per_pane(SEARCH);
            return match SearchClick::from_offset(within) {
                Some(click) => GroupClick::Search { pane, click },
                None => GroupClick::Handled,
            };
        }
        if offset >= NAV_BACK {
            let forward = offset >= NAV_FORWARD;
            let p = (offset - if forward { NAV_FORWARD } else { NAV_BACK }) as usize;
            if let Some(path) = self.pane_order.get(p).cloned() {
                if let Some(pane) = self.group.leaf_at_mut(&path) {
                    if forward {
                        pane.nav_forward();
                    } else {
                        pane.nav_back();
                    }
                    self.active = path;
                }
            }
            return GroupClick::Handled;
        }
        if offset >= BUTTON {
            let n = offset - BUTTON;
            let (p, index) = ((n / BUTTON_STRIDE) as usize, (n % BUTTON_STRIDE) as usize);
            let (Some(path), Some(button)) = (
                self.pane_order.get(p).cloned(),
                self.config.buttons.get(index),
            ) else {
                return GroupClick::Handled;
            };
            self.active = path.clone();
            return GroupClick::Button {
                path,
                action: button.action,
            };
        }
        if offset >= TAB_CLOSE {
            let (p, index) = per_pane(TAB_CLOSE);
            if let Some(path) = self.pane_order.get(p).cloned() {
                self.close_tab(&path, index as usize);
            }
            return GroupClick::Handled;
        }
        let (p, index) = per_pane(TAB_ACTIVATE);
        if let Some(path) = self.pane_order.get(p).cloned() {
            if let Some(pane) = self.group.leaf_at_mut(&path) {
                if (index as usize) < pane.open.len() {
                    pane.activate_user(index as usize);
                    self.active = path;
                }
            }
        }
        GroupClick::Handled
    }

    /// Track the hovered hit id; returns whether that changes a tab (whose close button shows on hover).
    pub fn set_hover(&mut self, id: Option<u64>) -> bool {
        let tab_hit = |group: &Self, hit: Option<u64>| {
            hit.and_then(|id| group.offset(id))
                .is_some_and(|offset| (TAB_ACTIVATE..BUTTON).contains(&offset))
        };
        let changed = self.hover != id && (tab_hit(self, self.hover) || tab_hit(self, id));
        self.hover = id;
        changed
    }

    pub fn focused_search(&mut self) -> Option<(&mut SearchBar, &mut dyn Searchable)> {
        let pane = self.active_pane_mut()?;
        if pane.search.dismissed || pane.search.focus.is_none() {
            return None;
        }
        pane.search_target()
    }

    /// Run a find-bar key command on the focused pane; returns false for keys it does not handle.
    pub fn search_command(&mut self, key: EditKey) -> bool {
        let Some((bar, item)) = self.active_pane_mut().and_then(Pane::search_target) else {
            return false;
        };
        let deployed = !bar.dismissed;
        match key {
            EditKey::DeploySearch => bar.deploy(item, false),
            EditKey::ToggleSearchReplace if deployed => bar.toggle_replace(),
            EditKey::ToggleSearchReplace => bar.deploy(item, true),
            EditKey::SelectNextMatch if deployed => bar.select_match(item, Direction::Next),
            EditKey::SelectPreviousMatch if deployed => bar.select_match(item, Direction::Prev),
            EditKey::SelectAllMatchesInSearch if deployed => bar.select_all_matches(item),
            EditKey::ToggleSearchCaseSensitive if deployed => {
                bar.toggle_option(item, SearchClick::CaseSensitive)
            }
            EditKey::ToggleSearchWholeWord if deployed => {
                bar.toggle_option(item, SearchClick::WholeWord)
            }
            EditKey::ToggleSearchRegex if deployed => bar.toggle_option(item, SearchClick::Regex),
            EditKey::UseSelectionForFind => bar.use_selection_for_find(item),
            _ => return false,
        }
        true
    }

    pub fn search_click(&mut self, p: usize, click: SearchClick) {
        let Some(path) = self.pane_order.get(p).cloned() else {
            return;
        };
        self.active = path.clone();
        let Some((bar, item)) = self.group.leaf_at_mut(&path).and_then(Pane::search_target) else {
            return;
        };
        match click {
            SearchClick::Query => bar.focus = Some(SearchField::Query),
            SearchClick::Replacement => bar.focus = Some(SearchField::Replacement),
            SearchClick::CaseSensitive | SearchClick::WholeWord | SearchClick::Regex => {
                bar.toggle_option(item, click)
            }
            SearchClick::ToggleReplace => bar.toggle_replace(),
            SearchClick::SelectAll => bar.select_all_matches(item),
            SearchClick::Previous => bar.select_match(item, Direction::Prev),
            SearchClick::Next => bar.select_match(item, Direction::Next),
            SearchClick::Close => bar.dismiss(item),
            SearchClick::ReplaceNext => bar.replace_next(item),
            SearchClick::ReplaceAll => bar.replace_all(item),
        }
    }

    pub fn divider_axis(&self, id: u64) -> Option<DividerAxis> {
        let offset = self.offset(id).filter(|offset| *offset >= DIVIDER)?;
        let divider = self.divider_order.get((offset - DIVIDER) as usize)?;
        Some(divider_axis(divider.axis))
    }

    pub fn drag_divider(&mut self, id: u64, x: f32, y: f32) -> bool {
        let Some(divider) = self
            .offset(id)
            .filter(|offset| *offset >= DIVIDER)
            .and_then(|offset| self.divider_order.get((offset - DIVIDER) as usize))
            .cloned()
        else {
            return false;
        };
        self.group.resize_divider(&divider, x, y)
    }

    pub fn pane_in_direction(&self, direction: SplitDirection) -> Option<Vec<usize>> {
        let leaves: Vec<LeafPlacement> = self
            .pane_order
            .iter()
            .zip(&self.pane_rects)
            .map(|(path, rect)| LeafPlacement {
                path: path.clone(),
                rect: *rect,
            })
            .collect();
        pane_group::find_pane_in_direction(&leaves, &self.active, direction, None)
    }

    /// Move focus, swap panes or pick a tab. Splitting depends on what the owner hosts, so `Split` is left to it
    /// (returns false). Returns false when there is nothing to act on.
    pub fn pane_command(&mut self, command: PaneCommand) -> bool {
        match command {
            PaneCommand::Split(_) => false,
            PaneCommand::ActivatePane(direction) => match self.pane_in_direction(direction) {
                Some(path) => {
                    self.active = path;
                    true
                }
                None => false,
            },
            PaneCommand::SwapPane(direction) => {
                let Some(path) = self.pane_in_direction(direction) else {
                    return false;
                };
                if self.group.swap(&self.active, &path) {
                    self.active = path;
                }
                true
            }
            item_command => self
                .active_pane_mut()
                .is_some_and(|pane| pane.apply_item_command(item_command)),
        }
    }

    pub fn is_tab(&self, id: u64) -> bool {
        self.offset(id)
            .is_some_and(|offset| (TAB_ACTIVATE..TAB_CLOSE).contains(&offset))
    }

    fn tab_of(&self, id: u64) -> Option<(usize, usize)> {
        let offset = self.offset(id).filter(|_| self.is_tab(id))? - TAB_ACTIVATE;
        Some((
            (offset / PANE_STRIDE) as usize,
            (offset % PANE_STRIDE) as usize,
        ))
    }

    pub fn begin_tab_drag(&mut self, id: u64) -> bool {
        let Some((p, index)) = self.tab_of(id) else {
            return false;
        };
        self.tab_drag = self
            .pane_order
            .get(p)
            .and_then(|path| self.group.leaf_at(path))
            .and_then(|pane| TabDrag::begin(pane, index));
        self.tab_drag.is_some()
    }

    /// Where a tab drop at `(x, y)` lands, or `None` over no pane. A full group appends instead of splitting.
    fn resolve_drop_at(
        &self,
        x: f32,
        y: f32,
        over: Option<(u64, Rect)>,
    ) -> Option<(TabDrop, Rect)> {
        let full = self.group.leaf_count() >= self.config.max_panes;
        let index = self.index_at(x, y)?;
        let pane = self.group.leaf_at(self.pane_order.get(index)?)?;
        let over_tab = over.and_then(|(id, rect)| {
            let (p, tab) = self.tab_of(id)?;
            (p == index).then_some((tab, rect))
        });
        let (mut target, preview) = tab_drag::resolve_drop(
            *self.pane_rects.get(index)?,
            pane.open.len(),
            x,
            y,
            over_tab,
        );
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
    }

    pub fn update_tab_drag(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool {
        if self.tab_drag.is_none() {
            return false;
        }
        let resolved = self.resolve_drop_at(x, y, over);
        if let Some(drag) = self.tab_drag.as_mut() {
            drag.drop = resolved.map(|(drop, _)| drop);
            drag.preview = resolved.map(|(_, preview)| preview);
        }
        true
    }

    pub fn drop_tab(&mut self) -> bool {
        let Some(drag) = self.tab_drag.take() else {
            return false;
        };
        let Some(drop) = drag.drop else {
            return false;
        };
        let next_id = self.next_pane_id;
        let landed = tab_drag::apply_drop(&mut self.group, &drag, drop, || Pane::new(next_id));
        if self.group.path_of(next_id).is_some() {
            self.next_pane_id += 1;
        }
        match landed {
            Some(path) => self.active = path,
            None if self.group.leaf_at(&self.active).is_none() => {
                self.active = self.group.first_leaf_path()
            }
            None => {}
        }
        true
    }

    pub fn cancel_tab_drag(&mut self) {
        self.tab_drag = None;
    }

    pub fn dragging_tab(&self) -> bool {
        self.tab_drag.is_some()
    }

    pub fn tab_drag_overlay(&self) -> Option<Rect> {
        self.tab_drag
            .as_ref()
            .and_then(|drag| drag.preview)
            .or(self.foreign_drop.map(|(_, preview)| preview))
    }

    pub fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)> {
        Some(self.tab_drag.as_ref()?.ghost())
    }

    pub fn dragged_item(&self) -> Option<&dyn Item> {
        let drag = self.tab_drag.as_ref()?;
        let path = self.group.path_of(drag.source)?;
        self.group
            .leaf_at(&path)?
            .open
            .get(drag.index)
            .map(|item| item.as_ref())
    }

    pub fn take_dragged_item(&mut self) -> Option<Box<dyn Item>> {
        let drag = self.tab_drag.take()?;
        let item = tab_drag::take_item(&mut self.group, &drag)?;
        if self.group.leaf_at(&self.active).is_none() {
            self.active = self.group.first_leaf_path();
        }
        Some(item)
    }

    pub fn update_foreign_drop(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool {
        self.foreign_drop = self.resolve_drop_at(x, y, over);
        self.foreign_drop.is_some()
    }

    pub fn clear_foreign_drop(&mut self) {
        self.foreign_drop = None;
    }

    /// Place an item dragged in from another group where its drop resolved, else in the focused pane.
    pub fn accept_foreign_item(&mut self, item: Box<dyn Item>) {
        let drop = self.foreign_drop.take().map(|(drop, _)| drop);
        let next_id = self.next_pane_id;
        let placed = match drop {
            Some(drop) => tab_drag::insert_item(&mut self.group, drop, item, || Pane::new(next_id)),
            None => Err(Some(item)),
        };
        if self.group.path_of(next_id).is_some() {
            self.next_pane_id += 1;
        }
        match placed {
            Ok(path) => self.active = path,
            Err(Some(item)) => {
                if let Some(pane) = self.active_pane_mut() {
                    pane.add_item(item);
                }
            }
            Err(None) => {}
        }
    }
}

fn contains(rect: &Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

fn divider_axis(axis: Axis) -> DividerAxis {
    match axis {
        Axis::Horizontal => DividerAxis::Horizontal,
        Axis::Vertical => DividerAxis::Vertical,
    }
}

/// The active item's body in a pane at `rect`: painted by the item itself (a terminal), or laid out as text
/// with carets, selections, gutter and scrollbars. Returns the self-painted body, or the placement fields for a
/// text body (chrome and rect are filled in by the caller).
fn layout_body(
    pane: &mut Pane,
    rect: Rect,
    is_focused: bool,
    fold_base: u64,
) -> (Option<(ui::Painted, Rect)>, PanePlacement) {
    if let Some((bar, item)) = pane.search_target() {
        bar.refresh(item);
    }
    let header = pane.header_h();
    let body_rect = Rect::new(
        rect.x,
        rect.y + header,
        rect.w.max(0.0),
        (rect.h - header).max(0.0),
        Rgba::TRANSPARENT,
    );
    let mut placement = PanePlacement {
        rect,
        node: ui::div().into(),
        painted: None,
        body: None,
        back: Vec::new(),
        back_tris: Vec::new(),
        carets: Vec::new(),
        scrollbar: Vec::new(),
        h_scrollbar: Vec::new(),
    };
    let Some(item) = pane.active_item_mut() else {
        return (None, placement);
    };
    if let Some(painted) = item.paint_body(body_rect, is_focused) {
        return (Some((painted, body_rect)), placement);
    }
    item.set_focused(is_focused);
    item.set_body_height(body_rect.h);
    item.set_body_width(body_rect.w);
    placement.back = item.back_rects(body_rect);
    placement.back_tris = item.selection_tris(body_rect);
    placement.carets = item.carets(body_rect);
    placement.scrollbar = item.scrollbar(body_rect);
    placement.h_scrollbar = item.h_scrollbar(body_rect);
    let y_offset = item.body_y_offset();
    let x_offset = item.body_x_offset();
    let gutter_w = item.gutter_w();
    let gutter = item.gutter(fold_base);
    let node = item.render();
    // Text starts right of the fixed gutter and clips to that region, so scrolled glyphs never paint over the
    // line numbers.
    let text_left = (body_rect.x + gutter_w).min(body_rect.x + body_rect.w);
    let text_clip = Rect::new(
        text_left,
        body_rect.y,
        (body_rect.x + body_rect.w - text_left).max(0.0),
        body_rect.h,
        Rgba::TRANSPARENT,
    );
    let gutter_clip = Rect::new(
        body_rect.x,
        body_rect.y,
        (text_left - body_rect.x).max(0.0),
        body_rect.h,
        Rgba::TRANSPARENT,
    );
    placement.body = Some(PaneBody {
        node,
        rect: body_rect,
        y_offset,
        text_left,
        x_offset,
        text_clip,
        gutter,
        gutter_clip,
    });
    (None, placement)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Plain(&'static str);

    impl Item for Plain {
        fn id(&self) -> Option<String> {
            Some(self.0.to_string())
        }
        fn title(&self) -> String {
            self.0.to_string()
        }
        fn render(&mut self) -> Node {
            ui::div().into()
        }
        fn clone_on_split(&self) -> Option<Box<dyn Item>> {
            Some(Box::new(Plain(self.0)))
        }
    }

    const BASE: u64 = 50_000;

    fn view() -> PaneGroupView {
        let mut view = PaneGroupView::new(PaneGroupConfig {
            id_base: BASE,
            show_nav: true,
            buttons: vec![PaneButton {
                icon: IconKind::PanelRight,
                action: PaneButtonAction::Split(SplitDirection::Right),
            }],
            max_panes: 2,
        });
        if let Some(pane) = view.active_pane_mut() {
            pane.add_item(Box::new(Plain("a")));
            pane.add_item(Box::new(Plain("b")));
        }
        view
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT)
    }

    #[test]
    fn clicks_resolve_inside_the_id_range_only() {
        let mut view = view();
        view.layout(area());
        assert_eq!(view.click(BASE + 5), GroupClick::NotMine);
        assert_eq!(view.click(BASE + ID_SPAN), GroupClick::NotMine);
        assert!(view.is_tab(BASE + TAB_ACTIVATE));
        assert_eq!(view.click(BASE + TAB_ACTIVATE), GroupClick::Handled);
        assert_eq!(
            view.active_item().map(|item| item.title()),
            Some("a".into())
        );
        assert_eq!(
            view.click(BASE + BUTTON),
            GroupClick::Button {
                path: Vec::new(),
                action: PaneButtonAction::Split(SplitDirection::Right),
            }
        );
        assert_eq!(view.click(BASE + TAB_CLOSE + 1), GroupClick::Handled);
        assert_eq!(view.pane_at(&[]).map(|pane| pane.open.len()), Some(1));
    }

    #[test]
    fn splits_stop_at_the_pane_limit_and_expose_dividers() {
        let mut view = view();
        let item = view.clone_active_of(&[]);
        assert_eq!(view.split(&[], SplitDirection::Right, item), Some(vec![1]));
        assert!(view.split(&[1], SplitDirection::Down, None).is_none());
        let (panes, dividers) = view.layout(area());
        assert_eq!(panes.len(), 2);
        assert_eq!(dividers.len(), 1);
        assert!(matches!(
            view.divider_axis(BASE + DIVIDER),
            Some(DividerAxis::Vertical | DividerAxis::Horizontal)
        ));
        assert!(view.pane_command(PaneCommand::ActivatePane(SplitDirection::Left)));
        assert_eq!(view.active, vec![0]);
        assert!(!view.pane_command(PaneCommand::Split(SplitDirection::Down)));
    }
}
