//! A pane group as a view: the split tree of panes, which one has focus, and what acts on it by pointer or key
//! (tab clicks, the tab bar buttons, dividers, tab dragging, pane commands), plus laying the panes out. Every
//! place that hosts tabs (the editor area, the terminal panel, later other features) owns one and configures its
//! tab bar, its click-id range and how many panes it allows.

use editor::search::Direction;
use ui::{IconKind, Node, Rect, Rgba};

use crate::pane::{
    render_pane, NavEntry, NavMode, Pane, PaneClickIds, PaneCommand, TabBarButton, TabBarConfig,
};
use crate::pane_group::{self, Axis, DividerRef, LeafPlacement, Member, Split, SplitDirection};
use crate::persistence::{SerializedAxis, SerializedItem, SerializedMember, SerializedPane};
use crate::search_bar::{SearchBar, SearchClick, SearchField, Searchable};
use crate::tab_drag::{self, DropTarget, TabDrag, TabDrop};
use crate::{
    ClipboardSlice, CopiedText, DividerAxis, DividerPlacement, EditKey, Item, ItemInput, PaneBody,
    PanePlacement, TerminalKeyOutcome,
};
use terminal::Keystroke;

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
/// A caret jump of at least this many rows within one item records a back/forward history entry.
const MIN_NAVIGATION_HISTORY_ROW_DELTA: usize = 10;
/// An item's gutter gets fold ids from a per-pane base; its change-strip ids sit this far above them.
pub const HUNK_FROM_FOLD: u64 = HUNK - FOLD;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneButtonAction {
    Split(SplitDirection),
    NewItem,
    ToggleZoom,
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
    /// Which dragged items may split a pane at its edge; `None` lets every item split. When restricted, a tab
    /// dragged within the group also cannot split away the only tab of its only pane.
    pub split_filter: Option<fn(&dyn Item) -> bool>,
    /// Zooming shows the whole group (a dock panel) instead of just the zoomed pane (the editor area).
    pub zoom_whole_group: bool,
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
    /// Whether the keyboard is in this group; its active pane then draws as focused.
    focused: bool,
    /// The pane a press on a self-painted item landed in, so its drag and release go to the same item.
    pointer_pane: Option<Vec<usize>>,
    /// What each popover of the last `editor_popovers` shows, so a scroll over it reaches the right item.
    popover_sources: Vec<PopoverSource>,
    /// The pane zoomed to cover the workspace (by id; it stays zoomed while focus is elsewhere).
    zoomed: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
enum PopoverSource {
    Completion,
    CompletionAside,
    Hover { pane: Vec<usize>, index: usize },
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
            focused: true,
            pointer_pane: None,
            popover_sources: Vec::new(),
            zoomed: None,
        }
    }

    /// Zoom the focused pane, or zoom back out. A pane without tabs does not zoom.
    pub fn toggle_zoom(&mut self) {
        if self.zoomed.take().is_some() {
            return;
        }
        let path = self.active.clone();
        self.zoomed = self
            .group
            .leaf_at(&path)
            .filter(|pane| !pane.open.is_empty())
            .map(|pane| pane.id);
    }

    /// Whether the zoom shows now: a zoomed group shows whole, a zoomed pane only while it is the focused one.
    pub fn zoom_shown(&self) -> bool {
        let Some(zoomed) = self.zoomed else {
            return false;
        };
        self.config.zoom_whole_group
            || self
                .pane_at(&self.active)
                .is_some_and(|pane| pane.id == zoomed)
    }

    /// The leaf laid out alone while its zoom shows (for a group that zooms one pane).
    fn zoomed_leaf(&self) -> Option<Vec<usize>> {
        if self.config.zoom_whole_group || !self.zoom_shown() {
            return None;
        }
        self.group.path_of(self.zoomed?)
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn is_focused(&self) -> bool {
        self.focused
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

    /// The group's layout and tabs as saved state.
    pub fn serialize(&self) -> SerializedMember {
        serialize_member(&self.group, &self.active, &mut Vec::new())
    }

    /// Rebuild the group from saved state, making each tab with `make_item` (items it cannot make are skipped).
    /// Panes left without tabs are dropped and a split left with one child gives way to it. Returns false and
    /// keeps the current group when nothing could be restored.
    pub fn restore(
        &mut self,
        saved: &SerializedMember,
        make_item: &mut dyn FnMut(&SerializedItem) -> Option<Box<dyn Item>>,
    ) -> bool {
        let Some((member, active)) = self.restore_member(saved, make_item) else {
            return false;
        };
        self.group = member;
        self.active = active.unwrap_or_else(|| self.group.first_leaf_path());
        true
    }

    fn restore_member(
        &mut self,
        saved: &SerializedMember,
        make_item: &mut dyn FnMut(&SerializedItem) -> Option<Box<dyn Item>>,
    ) -> Option<(Member<Pane>, Option<Vec<usize>>)> {
        match saved {
            SerializedMember::Pane(saved) => {
                let mut pane = self.new_pane();
                let mut active = None;
                for (index, item) in saved.items.iter().enumerate() {
                    if let Some(item) = make_item(item) {
                        if saved.active_item == Some(index) {
                            active = Some(pane.open.len());
                        }
                        pane.open.push(item);
                    }
                }
                if pane.open.is_empty() {
                    return None;
                }
                pane.active = active.or(Some(0));
                Some((Member::Leaf(pane), saved.active.then(Vec::new)))
            }
            SerializedMember::Split {
                axis,
                flexes,
                children,
            } => {
                let mut members = Vec::new();
                let mut kept_flexes = Vec::new();
                let mut active = None;
                for (index, child) in children.iter().enumerate() {
                    let Some((member, child_active)) = self.restore_member(child, make_item) else {
                        continue;
                    };
                    if active.is_none() {
                        active = child_active.map(|mut path| {
                            path.insert(0, members.len());
                            path
                        });
                    }
                    kept_flexes.push(flexes.get(index).copied().unwrap_or(1.0));
                    members.push(member);
                }
                match members.len() {
                    0 => None,
                    1 => {
                        let member = members.pop()?;
                        let active = active.and_then(|path| path.get(1..).map(<[usize]>::to_vec));
                        Some((member, active))
                    }
                    _ => {
                        pane_group::renormalize(&mut kept_flexes);
                        let axis = match axis {
                            SerializedAxis::Horizontal => Axis::Horizontal,
                            SerializedAxis::Vertical => Axis::Vertical,
                        };
                        let split = Split {
                            axis,
                            members,
                            flexes: kept_flexes,
                        };
                        Some((Member::Split(split), active))
                    }
                }
            }
        }
    }

    pub fn refresh_disk_state(&mut self) {
        self.for_each_item_mut(&mut |item| item.refresh_disk_state());
    }

    pub fn is_busy(&self) -> bool {
        let mut busy = false;
        self.group
            .for_each_pane(&mut |pane| busy |= pane.open.iter().any(|item| item.is_busy()));
        busy
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

    /// Click ids by pane render index, as the chrome hands them out.
    pub fn tab_id(&self, p: usize, index: usize) -> u64 {
        self.pane_ids(p).tab_activate + index as u64
    }

    pub fn tab_close_id(&self, p: usize, index: usize) -> u64 {
        self.pane_ids(p).tab_close + index as u64
    }

    pub fn button_id(&self, p: usize, button: usize) -> u64 {
        self.config.id_base + BUTTON + p as u64 * BUTTON_STRIDE + button as u64
    }

    pub fn divider_id(&self, index: usize) -> u64 {
        self.config.id_base + DIVIDER + index as u64
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
                    icon: match button.action {
                        PaneButtonAction::ToggleZoom if self.zoomed.is_some() => IconKind::Minimize,
                        _ => button.icon,
                    },
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
        if self
            .zoomed
            .is_some_and(|id| self.group.path_of(id).is_none())
        {
            self.zoomed = None;
        }
        let (leaves, dividers) = match self.zoomed_leaf() {
            Some(path) => (vec![LeafPlacement { path, rect: area }], Vec::new()),
            None => self.group.layout(area),
        };
        for (p, leaf) in leaves.into_iter().enumerate() {
            let is_focused = self.focused && leaf.path == self.active;
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
        self.focused_content = self.body_rect_of(&self.active);
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
            if button.action == PaneButtonAction::ToggleZoom {
                self.toggle_zoom();
                return GroupClick::Handled;
            }
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

    fn search_focused(&mut self) -> bool {
        self.active_pane_mut()
            .is_some_and(|pane| !pane.search.dismissed && pane.search.focus.is_some())
    }

    /// Find-bar shortcuts for an item that takes raw key presses (a terminal); returns whether it was one.
    fn keystroke_search_command(&mut self, keystroke: &Keystroke) -> bool {
        let m = keystroke.modifiers;
        let only_cmd = m.cmd && !m.alt && !m.ctrl;
        let Some(pane) = self.active_pane_mut() else {
            return false;
        };
        let dismissed = pane.search.dismissed;
        let Some((bar, item)) = pane.search_target() else {
            return false;
        };
        let supported = item.supported_options();
        let option = |key: &str| match key {
            "c" if supported.case => Some(SearchClick::CaseSensitive),
            "w" if supported.word => Some(SearchClick::WholeWord),
            "x" if supported.regex => Some(SearchClick::Regex),
            _ => None,
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
            key if m.cmd && m.alt && !m.ctrl && !dismissed && option(key).is_some() => {
                if let Some(click) = option(key) {
                    bar.toggle_option(item, click);
                }
            }
            _ => return false,
        }
        true
    }

    /// The focused pane and its active item's history entry, taken before an action that may jump the caret.
    pub fn nav_snapshot(&mut self) -> Option<(Vec<usize>, NavEntry)> {
        let path = self.active.clone();
        let pane = self.group.leaf_at(&path)?;
        let entry = pane.nav_entry_for(pane.active?)?;
        Some((path, entry))
    }

    /// Record a history entry when the caret moved far within the same item since `snapshot`.
    pub fn record_nav_jump(&mut self, snapshot: Option<(Vec<usize>, NavEntry)>) {
        let Some((path, before)) = snapshot else {
            return;
        };
        let Some(pane) = self.group.leaf_at_mut(&path) else {
            return;
        };
        let after = pane.active.and_then(|index| pane.nav_entry_for(index));
        let jumped = after.filter(|a| a.id == before.id).is_some_and(|a| {
            matches!((before.row, a.row), (Some(old), Some(new))
                if old.abs_diff(new) >= MIN_NAVIGATION_HISTORY_ROW_DELTA)
        });
        if jumped {
            pane.push_nav(before, NavMode::Normal);
        }
    }

    fn track_nav<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let snapshot = self.nav_snapshot();
        let result = f(self);
        self.record_nav_jump(snapshot);
        result
    }

    /// A key for the focused item without recording a history entry (the caller tracks the jump).
    pub fn editor_key_untracked(&mut self, key: EditKey, shift: bool) -> bool {
        if matches!(key, EditKey::GoBack | EditKey::GoForward) {
            if let Some(pane) = self.active_pane_mut() {
                if key == EditKey::GoBack {
                    pane.nav_back();
                } else {
                    pane.nav_forward();
                }
            }
            return true;
        }
        if self.search_command(key) {
            return true;
        }
        if let Some((bar, item)) = self.focused_search() {
            bar.key(item, key, shift);
            return true;
        }
        let changed = match self.editable_item_mut() {
            Some(item) => {
                item.input_key(key, shift);
                true
            }
            None => false,
        };
        self.refresh_search();
        changed
    }

    /// The body rect (below the chrome) of the pane at `path`, as last laid out.
    pub fn body_rect_of(&self, path: &[usize]) -> Option<Rect> {
        let rect = self.pane_rect_of(path)?;
        let header = self.pane_at(path)?.header_h();
        Some(Rect::new(
            rect.x,
            rect.y + header,
            rect.w.max(0.0),
            (rect.h - header).max(0.0),
            Rgba::TRANSPARENT,
        ))
    }

    fn editable_item_mut(&mut self) -> Option<&mut dyn Item> {
        self.active_item_mut().filter(|item| item.is_editable())
    }

    fn refresh_search(&mut self) {
        if let Some((bar, item)) = self.active_pane_mut().and_then(Pane::search_target) {
            bar.refresh(item);
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
            PaneCommand::ToggleZoom => {
                self.toggle_zoom();
                true
            }
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

    /// Whether dropping `item` at a pane edge may split it: never in a full group, and only for items the
    /// group's split filter allows.
    fn can_split_with(&self, item: &dyn Item, dragged_here: bool) -> bool {
        if self.group.leaf_count() >= self.config.max_panes {
            return false;
        }
        let Some(filter) = self.config.split_filter else {
            return true;
        };
        let can_drag_away = !dragged_here
            || self.group.leaf_count() > 1
            || self
                .tab_drag
                .as_ref()
                .and_then(|drag| self.group.path_of(drag.source))
                .and_then(|path| self.group.leaf_at(&path))
                .is_some_and(|pane| pane.open.len() > 1);
        can_drag_away && filter(item)
    }

    /// Where a tab drop at `(x, y)` lands, or `None` over no pane. Without `can_split` an edge drop goes into
    /// the pane instead.
    fn resolve_drop_at(
        &self,
        x: f32,
        y: f32,
        over: Option<(u64, Rect)>,
        can_split: bool,
    ) -> Option<(TabDrop, Rect)> {
        let index = self.index_at(x, y)?;
        let pane = self.group.leaf_at(self.pane_order.get(index)?)?;
        let over_tab = over.and_then(|(id, rect)| {
            let (p, tab) = self.tab_of(id)?;
            (p == index).then_some((tab, rect))
        });
        let (mut target, mut preview) = tab_drag::resolve_drop(
            *self.pane_rects.get(index)?,
            pane.open.len(),
            x,
            y,
            over_tab,
        );
        if !can_split && matches!(target, DropTarget::Split(_)) {
            target = DropTarget::Append;
            if let Some(body) = self
                .pane_order
                .get(index)
                .and_then(|path| self.body_rect_of(path))
            {
                preview = body;
            }
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
        let can_split = self
            .dragged_item()
            .is_some_and(|item| self.can_split_with(item, true));
        let resolved = self.resolve_drop_at(x, y, over, can_split);
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

    pub fn update_foreign_drop(
        &mut self,
        x: f32,
        y: f32,
        over: Option<(u64, Rect)>,
        item: &dyn Item,
    ) -> bool {
        let can_split = self.can_split_with(item, false);
        self.foreign_drop = self.resolve_drop_at(x, y, over, can_split);
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

/// Keys for the find bar's text field, from a raw key press.
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

impl ItemInput for PaneGroupView {
    fn editor_key(&mut self, key: EditKey, shift: bool) -> bool {
        if matches!(key, EditKey::GoBack | EditKey::GoForward) {
            return self.editor_key_untracked(key, shift);
        }
        self.track_nav(|group| group.editor_key_untracked(key, shift))
    }

    fn editor_text(&mut self, text: &str) -> bool {
        self.track_nav(|group| {
            if let Some((bar, item)) = group.focused_search() {
                bar.input(item, text);
                return true;
            }
            match group.editable_item_mut() {
                Some(item) => {
                    item.input_text(text);
                    true
                }
                None => false,
            }
        })
    }

    fn editor_paste(&mut self, text: &str, slices: Option<&[ClipboardSlice]>) -> bool {
        self.track_nav(|group| {
            if let Some((bar, item)) = group.focused_search() {
                bar.input(item, text);
                return true;
            }
            match group.editable_item_mut() {
                Some(item) => {
                    item.paste(text, slices);
                    true
                }
                None => false,
            }
        })
    }

    fn editor_ime_preedit(&mut self, text: &str, selected: Option<std::ops::Range<usize>>) -> bool {
        match self.editable_item_mut() {
            Some(item) => {
                item.ime_preedit(text, selected);
                true
            }
            None => false,
        }
    }

    fn editor_ime_commit(&mut self, text: &str) -> bool {
        match self.editable_item_mut() {
            Some(item) => {
                item.ime_commit(text);
                true
            }
            None => false,
        }
    }

    fn editor_click(&mut self, x: f32, y: f32, extend: bool) -> bool {
        self.track_nav(|group| {
            let Some((path, local_x, local_y)) = group.body_point(x, y) else {
                return false;
            };
            if ui::modifiers().cmd && !extend {
                group.active = path.clone();
                if group
                    .active_item_mut()
                    .is_some_and(|item| item.cmd_click(local_x, local_y))
                {
                    return true;
                }
            }
            if !extend {
                group.active = path;
            }
            if let Some(pane) = group.active_pane_mut() {
                pane.search.focus = None;
            }
            match group.editable_item_mut() {
                Some(item) => {
                    item.place_cursor(local_x, local_y, extend);
                    true
                }
                None => false,
            }
        })
    }

    fn editor_double_click(&mut self, x: f32, y: f32) -> bool {
        self.track_nav(|group| {
            let Some((path, local_x, local_y)) = group.body_point(x, y) else {
                return false;
            };
            group.active = path;
            match group.editable_item_mut() {
                Some(item) => {
                    item.select_word_at(local_x, local_y);
                    true
                }
                None => false,
            }
        })
    }

    fn editor_drag(&mut self, x: f32, y: f32) -> bool {
        self.track_nav(|group| {
            let Some(body) = group.body_rect_of(&group.active.clone()) else {
                return false;
            };
            match group.editable_item_mut() {
                Some(item) => {
                    item.drag_select(x, y, body);
                    true
                }
                None => false,
            }
        })
    }

    fn editor_hover(&mut self, x: f32, y: f32) -> bool {
        let mut changed = false;
        for path in self.pane_order.clone() {
            let Some(body) = self.body_rect_of(&path) else {
                continue;
            };
            let Some(item) = self
                .group
                .leaf_at_mut(&path)
                .and_then(Pane::active_item_mut)
            else {
                continue;
            };
            let over_gutter =
                x >= body.x && x < body.x + item.gutter_w() && y >= body.y && y < body.y + body.h;
            changed |= item.set_gutter_hovered(over_gutter);
            let local = contains(&body, x, y).then_some((x - body.x, y - body.y));
            changed |= item.pointer_moved(local, (x, y));
        }
        changed
    }

    fn editor_scroll(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        let Some(path) = self.pane_path_at(x, y) else {
            return false;
        };
        match self
            .group
            .leaf_at_mut(&path)
            .and_then(Pane::active_item_mut)
            .filter(|item| item.is_editable())
        {
            Some(item) => {
                let moved_y = item.scroll_by(dy);
                let moved_x = item.scroll_by_x(dx);
                moved_x || moved_y
            }
            None => false,
        }
    }

    fn editor_copy(&self) -> Option<CopiedText> {
        if let Some(pane) = self.pane_at(&self.active) {
            let field = match pane.search.focus {
                Some(SearchField::Query) => Some(&pane.search.query),
                Some(SearchField::Replacement) => Some(&pane.search.replacement),
                None => None,
            };
            if let Some(field) = field.filter(|_| !pane.search.dismissed) {
                return field.selected_text().map(|text| CopiedText {
                    text,
                    slices: Vec::new(),
                });
            }
        }
        self.active_item().and_then(|item| item.copy())
    }

    fn editor_cut(&mut self) -> Option<CopiedText> {
        if let Some((bar, item)) = self.focused_search() {
            let text = match bar.focus {
                Some(SearchField::Replacement) => bar.replacement.selected_text(),
                _ => bar.query.selected_text(),
            }?;
            bar.key(item, EditKey::Backspace, false);
            return Some(CopiedText {
                text,
                slices: Vec::new(),
            });
        }
        self.editable_item_mut().and_then(|item| item.cut())
    }

    fn editor_copy_trimmed(&self) -> Option<CopiedText> {
        self.active_item().and_then(|item| item.copy_trimmed())
    }

    fn editor_selected_text(&self) -> Option<String> {
        self.active_item().and_then(|item| item.selected_text())
    }

    fn editor_right_press(&mut self, x: f32, y: f32) -> bool {
        let Some((path, local_x, local_y)) = self.body_point(x, y) else {
            return false;
        };
        self.active = path;
        match self.editable_item_mut() {
            Some(item) => {
                item.right_press(local_x, local_y);
                true
            }
            None => false,
        }
    }

    fn editor_menu_anchor_at(&self, x: f32, y: f32) -> Option<(Vec<usize>, usize)> {
        let (path, _, local_y) = self.body_point(x, y)?;
        let line = self
            .pane_at(&path)?
            .active_item()?
            .buffer_line_at(local_y)?;
        Some((path, line))
    }

    fn editor_menu_y(&self, path: &[usize], line: usize) -> Option<f32> {
        let body = self.body_rect_of(path)?;
        self.pane_at(path)?.active_item()?.line_screen_y(body, line)
    }

    fn editor_save(&mut self) -> Option<Result<(), String>> {
        self.editable_item_mut().map(|item| item.save())
    }

    fn editor_focused(&self) -> bool {
        self.active_item().is_some_and(|item| item.is_editable())
    }

    fn cursor_position(&self) -> Option<String> {
        self.active_item()?.cursor_status()
    }

    fn editor_popovers(&mut self, viewport: (f32, f32)) -> Vec<(Node, f32, f32)> {
        self.popover_sources.clear();
        let mut popovers = Vec::new();
        let content = self.focused_content;
        let completion =
            content.and_then(|content| self.active_item()?.completion_popover(content));
        if let Some(completion) = completion {
            popovers.push(completion);
            self.popover_sources.push(PopoverSource::Completion);
            let aside =
                content.and_then(|content| self.active_item()?.completion_aside(content, viewport));
            if let Some(aside) = aside {
                popovers.push(aside);
                self.popover_sources.push(PopoverSource::CompletionAside);
            }
        }
        for path in self.pane_order.clone() {
            let Some(body) = self.body_rect_of(&path) else {
                continue;
            };
            let Some(item) = self.pane_at(&path).and_then(Pane::active_item) else {
                continue;
            };
            for (index, popover) in item.hover_popovers(body).into_iter().enumerate() {
                popovers.push(popover);
                self.popover_sources.push(PopoverSource::Hover {
                    pane: path.clone(),
                    index,
                });
            }
        }
        popovers
    }

    fn popover_scroll(&mut self, index: usize, dy: f32) -> bool {
        match self.popover_sources.get(index).cloned() {
            Some(PopoverSource::Completion) => self
                .active_item_mut()
                .is_some_and(|item| item.scroll_completion(dy)),
            Some(PopoverSource::CompletionAside) => self
                .active_item_mut()
                .is_some_and(|item| item.scroll_completion_aside(dy)),
            Some(PopoverSource::Hover { pane, index }) => self
                .group
                .leaf_at_mut(&pane)
                .and_then(Pane::active_item_mut)
                .is_some_and(|item| item.scroll_hover(index, dy)),
            None => false,
        }
    }

    fn popover_click(&mut self, id: u64) -> bool {
        if self
            .active_item_mut()
            .is_some_and(|item| item.popover_click(id))
        {
            return true;
        }
        let mut clicked = false;
        self.for_each_item_mut(&mut |item| clicked = clicked || item.popover_click(id));
        clicked
    }

    fn popover_hover(&mut self, id: Option<u64>) {
        if let Some(item) = self.active_item_mut() {
            item.hover_completion(id);
        }
    }

    fn active_wants_keystrokes(&self) -> bool {
        self.active_item()
            .is_some_and(|item| item.wants_keystrokes())
    }

    /// A raw key press for the focused pane's item (one that wants keystrokes): find-bar shortcuts and editing
    /// while the find bar has focus, otherwise the item's own handling.
    fn item_keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        if !self
            .active_item()
            .is_some_and(|item| item.wants_keystrokes())
        {
            return TerminalKeyOutcome::Ignored;
        }
        if self.keystroke_search_command(keystroke) {
            return TerminalKeyOutcome::Handled;
        }
        if self.search_focused() {
            let m = keystroke.modifiers;
            if m.cmd && keystroke.key == "v" {
                return TerminalKeyOutcome::Paste;
            }
            let Some(pane) = self.active_pane_mut() else {
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
        self.active_item_mut()
            .map_or(TerminalKeyOutcome::Ignored, |item| {
                item.keystroke(keystroke)
            })
    }

    /// Typed text for an item that wants keystrokes, or the find bar while it has focus.
    fn item_text(&mut self, text: &str) {
        if let Some((bar, item)) = self.focused_search() {
            bar.input(item, text);
            return;
        }
        if let Some(item) = self
            .active_item_mut()
            .filter(|item| item.wants_keystrokes())
        {
            item.input_text(text);
        }
    }

    fn item_paste(&mut self, text: &str) {
        if let Some((bar, item)) = self.focused_search() {
            bar.input(item, text);
            return;
        }
        if let Some(item) = self
            .active_item_mut()
            .filter(|item| item.wants_keystrokes())
        {
            item.paste(text, None);
        }
    }

    fn item_focus_changed(&mut self, focused: bool) {
        if let Some(item) = self
            .active_item_mut()
            .filter(|item| item.wants_keystrokes())
        {
            item.set_focused(focused);
        }
    }

    fn item_pointer_down(
        &mut self,
        x: f32,
        y: f32,
        click_count: u32,
        modifiers: terminal::Modifiers,
    ) -> bool {
        let Some(path) = self.pane_path_at(x, y) else {
            return false;
        };
        let handled = self
            .group
            .leaf_at_mut(&path)
            .and_then(Pane::active_item_mut)
            .is_some_and(|item| item.pointer_down(x, y, click_count, modifiers));
        if handled {
            if let Some(pane) = self.group.leaf_at_mut(&path) {
                pane.search.focus = None;
            }
            self.active = path.clone();
            self.pointer_pane = Some(path);
        }
        handled
    }

    fn item_pointer_drag(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) -> bool {
        let Some(path) = self.pointer_pane.clone() else {
            return false;
        };
        self.group
            .leaf_at_mut(&path)
            .and_then(Pane::active_item_mut)
            .is_some_and(|item| item.pointer_drag(x, y, modifiers))
    }

    fn item_pointer_move(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) -> bool {
        let active = self.active.clone();
        let focused = self.focused;
        let mut changed = false;
        for path in self.pane_order.clone() {
            let is_focused = focused && path == active;
            if let Some(item) = self
                .group
                .leaf_at_mut(&path)
                .and_then(Pane::active_item_mut)
            {
                changed |= item.pointer_move(x, y, modifiers, is_focused);
            }
        }
        changed
    }

    fn item_pointer_up(&mut self, x: f32, y: f32, modifiers: terminal::Modifiers) {
        let Some(path) = self.pointer_pane.take() else {
            return;
        };
        if let Some(item) = self
            .group
            .leaf_at_mut(&path)
            .and_then(Pane::active_item_mut)
        {
            item.pointer_up(x, y, modifiers);
        }
    }

    fn item_pointer_scroll(
        &mut self,
        x: f32,
        y: f32,
        delta_y: f32,
        modifiers: terminal::Modifiers,
    ) -> bool {
        let Some(path) = self.pane_path_at(x, y) else {
            return false;
        };
        self.group
            .leaf_at_mut(&path)
            .and_then(Pane::active_item_mut)
            .is_some_and(|item| item.pointer_scroll(x, y, delta_y, modifiers))
    }
}

fn serialize_member(
    member: &Member<Pane>,
    active: &[usize],
    path: &mut Vec<usize>,
) -> SerializedMember {
    match member {
        Member::Leaf(pane) => {
            let mut items = Vec::new();
            let mut active_item = None;
            for (index, item) in pane.open.iter().enumerate() {
                if let Some(saved) = item.serialize() {
                    if pane.active == Some(index) {
                        active_item = Some(items.len());
                    }
                    items.push(saved);
                }
            }
            SerializedMember::Pane(SerializedPane {
                active: path.as_slice() == active,
                items,
                active_item,
            })
        }
        Member::Split(split) => SerializedMember::Split {
            axis: match split.axis {
                Axis::Horizontal => SerializedAxis::Horizontal,
                Axis::Vertical => SerializedAxis::Vertical,
            },
            flexes: split.flexes.clone(),
            children: split
                .members
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    path.push(index);
                    let saved = serialize_member(child, active, path);
                    path.pop();
                    saved
                })
                .collect(),
        },
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
    let background = item.body_background();
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
        background,
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
        fn serialize(&self) -> Option<SerializedItem> {
            Some(SerializedItem {
                kind: "plain".into(),
                data: serde_json::json!(self.0),
            })
        }
    }

    fn make_plain(item: &SerializedItem) -> Option<Box<dyn Item>> {
        let name = match item.data.as_str()? {
            "a" => "a",
            "b" => "b",
            "c" => "c",
            _ => return None,
        };
        Some(Box::new(Plain(name)))
    }

    #[derive(Default)]
    struct Shell {
        typed: String,
    }

    impl Searchable for Shell {
        fn search_version(&self) -> u64 {
            0
        }
        fn find(&self, _query: &editor::search::SearchQuery) -> Vec<std::ops::Range<usize>> {
            Vec::new()
        }
        fn query_suggestion(&self) -> String {
            String::new()
        }
        fn single_cursor(&self) -> Option<usize> {
            None
        }
        fn activate_match(&mut self, _range: std::ops::Range<usize>) {}
        fn select_matches(&mut self, _ranges: &[std::ops::Range<usize>]) {}
        fn replace_match(
            &mut self,
            _query: &editor::search::SearchQuery,
            _range: std::ops::Range<usize>,
        ) {
        }
        fn replace_all(
            &mut self,
            _query: &editor::search::SearchQuery,
            _ranges: &[std::ops::Range<usize>],
        ) {
        }
        fn set_search_highlights(
            &mut self,
            _matches: Vec<std::ops::Range<usize>>,
            _active: Option<usize>,
        ) {
        }
    }

    impl Item for Shell {
        fn title(&self) -> String {
            "shell".into()
        }
        fn render(&mut self) -> Node {
            ui::div().into()
        }
        fn searchable(&mut self) -> Option<&mut dyn Searchable> {
            Some(self)
        }
        fn wants_keystrokes(&self) -> bool {
            true
        }
        fn keystroke(&mut self, _keystroke: &Keystroke) -> TerminalKeyOutcome {
            TerminalKeyOutcome::Handled
        }
        fn input_text(&mut self, text: &str) {
            self.typed.push_str(text);
        }
    }

    #[derive(Default)]
    struct Doc {
        cursor: Option<(f32, f32, bool)>,
        typed: String,
    }

    impl Item for Doc {
        fn title(&self) -> String {
            "doc".into()
        }
        fn render(&mut self) -> Node {
            ui::div().into()
        }
        fn is_editable(&self) -> bool {
            true
        }
        fn place_cursor(&mut self, local_x: f32, local_y: f32, extend: bool) {
            self.cursor = Some((local_x, local_y, extend));
        }
        fn input_text(&mut self, text: &str) {
            self.typed.push_str(text);
        }
        fn as_any(&self) -> Option<&dyn std::any::Any> {
            Some(self)
        }
    }

    fn doc(view: &PaneGroupView) -> Option<&Doc> {
        view.active_item()?.as_any()?.downcast_ref::<Doc>()
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
            split_filter: None,
            zoom_whole_group: false,
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

    #[test]
    fn raw_key_items_get_the_find_bar_in_any_group() {
        let mut view = view();
        if let Some(pane) = view.active_pane_mut() {
            pane.add_item(Box::new(Shell::default()));
        }
        assert_eq!(
            view.item_keystroke(&Keystroke::parse("cmd-f")),
            TerminalKeyOutcome::Handled
        );
        view.item_text("needle");
        let pane = view.pane_at(&[]).map(|pane| pane.search.query.text());
        assert_eq!(pane.as_deref(), Some("needle"));
        assert_eq!(
            view.item_keystroke(&Keystroke::parse("escape")),
            TerminalKeyOutcome::Handled
        );
        view.item_text("ls");
        assert!(view.pane_at(&[]).is_some_and(|pane| pane.search.dismissed));
    }

    #[test]
    fn editor_input_reaches_the_item_under_the_pointer_and_the_focused_one() {
        let mut view = view();
        if let Some(pane) = view.active_pane_mut() {
            pane.add_item(Box::new(Doc::default()));
        }
        view.layout(area());
        let body = view
            .body_rect_of(&[])
            .map(|rect| rect.y)
            .unwrap_or_default();
        assert!(view.editor_click(40.0, body + 30.0, false));
        assert_eq!(
            doc(&view).and_then(|doc| doc.cursor),
            Some((40.0, 30.0, false))
        );
        assert!(view.editor_drag(60.0, body + 50.0));
        assert_eq!(
            doc(&view).and_then(|doc| doc.cursor),
            Some((60.0, 50.0, true))
        );
        assert!(view.editor_text("hi"));
        assert_eq!(doc(&view).map(|doc| doc.typed.as_str()), Some("hi"));
        assert!(view.editor_focused());
        assert!(!view.active_wants_keystrokes());
        assert!(!view.editor_click(40.0, 5.0, false));
    }

    #[test]
    fn saved_layout_comes_back_with_its_tabs_flexes_and_focus() {
        let mut view = view();
        let item = view.clone_active_of(&[]);
        view.split(&[], SplitDirection::Right, item);
        if let Member::Split(split) = &mut view.group {
            split.flexes = vec![0.5, 1.5];
        }
        view.active = vec![0];
        let saved = view.serialize();
        let json = serde_json::to_string(&saved).unwrap();
        let back: SerializedMember = serde_json::from_str(&json).unwrap();
        assert_eq!(back, saved);

        let mut restored = PaneGroupView::new(PaneGroupConfig {
            id_base: BASE,
            show_nav: true,
            buttons: Vec::new(),
            max_panes: 4,
            split_filter: None,
            zoom_whole_group: false,
        });
        assert!(restored.restore(&back, &mut make_plain));
        assert_eq!(restored.group.leaf_count(), 2);
        assert_eq!(restored.active, vec![0]);
        let Member::Split(split) = &restored.group else {
            panic!("the split should come back");
        };
        assert_eq!(split.flexes, vec![0.5, 1.5]);
        let titles: Vec<String> = restored
            .pane_at(&[0])
            .map(|pane| pane.open.iter().map(|item| item.title()).collect())
            .unwrap_or_default();
        assert_eq!(titles, ["a", "b"]);
        assert_eq!(restored.pane_at(&[0]).and_then(|pane| pane.active), Some(1));
    }

    #[test]
    fn panes_whose_tabs_cannot_come_back_are_dropped() {
        let saved = SerializedMember::Split {
            axis: SerializedAxis::Horizontal,
            flexes: vec![1.0, 1.0],
            children: vec![
                SerializedMember::Pane(SerializedPane {
                    active: false,
                    items: vec![SerializedItem {
                        kind: "plain".into(),
                        data: serde_json::json!("gone"),
                    }],
                    active_item: Some(0),
                }),
                SerializedMember::Pane(SerializedPane {
                    active: true,
                    items: vec![SerializedItem {
                        kind: "plain".into(),
                        data: serde_json::json!("c"),
                    }],
                    active_item: Some(0),
                }),
            ],
        };
        let mut view = view();
        assert!(view.restore(&saved, &mut make_plain));
        assert_eq!(view.group.leaf_count(), 1);
        assert_eq!(view.active, Vec::<usize>::new());
        assert_eq!(
            view.active_item().map(|item| item.title()),
            Some("c".into())
        );
        let nothing = SerializedMember::Pane(SerializedPane::default());
        assert!(!view.restore(&nothing, &mut make_plain));
        assert_eq!(
            view.active_item().map(|item| item.title()),
            Some("c".into())
        );
    }

    #[test]
    fn a_zoomed_pane_covers_the_area_only_while_it_has_focus() {
        let mut view = view();
        let item = view.clone_active_of(&[]);
        view.split(&[], SplitDirection::Right, item);
        assert!(view.pane_command(PaneCommand::ToggleZoom));
        assert!(view.zoom_shown());
        let (panes, dividers) = view.layout(area());
        assert_eq!(panes.len(), 1);
        assert!(dividers.is_empty());
        assert_eq!(view.pane_rects().first().map(|rect| rect.w), Some(800.0));

        view.active = vec![0];
        assert!(!view.zoom_shown());
        assert_eq!(view.layout(area()).0.len(), 2);
        view.active = vec![1];
        assert!(view.zoom_shown());

        view.layout(area());
        assert_eq!(view.click(view.tab_close_id(0, 0)), GroupClick::Handled);
        view.layout(area());
        assert!(!view.zoom_shown());
        assert_eq!(view.group.leaf_count(), 1);
    }

    #[test]
    fn a_dock_group_zooms_whole() {
        let mut view = PaneGroupView::new(PaneGroupConfig {
            id_base: BASE,
            show_nav: false,
            buttons: vec![PaneButton {
                icon: IconKind::Maximize,
                action: PaneButtonAction::ToggleZoom,
            }],
            max_panes: 4,
            split_filter: None,
            zoom_whole_group: true,
        });
        if let Some(pane) = view.active_pane_mut() {
            pane.add_item(Box::new(Plain("a")));
        }
        let item = view.clone_active_of(&[]);
        view.split(&[], SplitDirection::Down, item);
        view.layout(area());
        assert_eq!(view.click(view.button_id(1, 0)), GroupClick::Handled);
        view.active = vec![0];
        assert!(view.zoom_shown());
        assert_eq!(view.layout(area()).0.len(), 2);
        assert!(view.pane_command(PaneCommand::ToggleZoom));
        assert!(!view.zoom_shown());
    }
}
