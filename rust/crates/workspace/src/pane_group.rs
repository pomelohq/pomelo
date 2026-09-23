//! The pane group: a tree of leaf panes and splits along one axis, sized by per-child flex ratios. Both the
//! editor's center and the terminal panel tile their panes with it. This module owns the structure and its
//! geometry (split, remove, swap, divider drag, hit-testing); what a leaf draws is up to the owner.

use ui::{Rect, Rgba};

pub const HORIZONTAL_MIN_SIZE: f32 = 80.0;
pub const VERTICAL_MIN_SIZE: f32 = 100.0;
/// The visible gap between panes, filled by a 1px line.
pub const DIVIDER_LINE: f32 = 1.0;
/// The pointer grab strip centered on the divider line.
pub const DIVIDER_GRAB: f32 = 5.0;
/// A tab dropped within this fraction of a pane's smaller side splits that edge.
pub const DROP_EDGE: f32 = 0.2;
/// Looking for the neighbouring pane in a direction probes this far past the active pane's edge.
const NEIGHBOUR_PROBE: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    Left,
    Right,
    Up,
    Down,
}

impl SplitDirection {
    pub fn axis(self) -> Axis {
        match self {
            SplitDirection::Left | SplitDirection::Right => Axis::Horizontal,
            SplitDirection::Up | SplitDirection::Down => Axis::Vertical,
        }
    }

    /// Whether the new pane goes after the current one along the axis (right/down) rather than before.
    pub fn increasing(self) -> bool {
        matches!(self, SplitDirection::Right | SplitDirection::Down)
    }

    pub fn opposite(self) -> Self {
        match self {
            SplitDirection::Left => SplitDirection::Right,
            SplitDirection::Right => SplitDirection::Left,
            SplitDirection::Up => SplitDirection::Down,
            SplitDirection::Down => SplitDirection::Up,
        }
    }
}

/// Leaves carry a stable id so a pane can be found again after the tree is restructured around it.
pub trait PaneId {
    fn pane_id(&self) -> u64;
}

pub enum Member<P> {
    Leaf(P),
    Split(Split<P>),
}

/// Flexes sum to `members.len()`, so a child's extent along the axis is `container * flex / len` (equal flexes
/// = equal sizes).
pub struct Split<P> {
    pub axis: Axis,
    pub members: Vec<Member<P>>,
    pub flexes: Vec<f32>,
}

/// A rendered divider: the split that owns it (child-index path), the gap index between child `index` and
/// `index + 1`, and the pixel geometry a drag needs to turn the pointer into flexes without re-walking the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct DividerRef {
    pub split_path: Vec<usize>,
    pub index: usize,
    pub axis: Axis,
    /// Pixels shared by the split's children along its axis (the container minus the divider lines).
    pub container: f32,
    /// The along-axis origin of child `index`, so a drag places the boundary at the absolute pointer position.
    pub child_start: f32,
}

#[derive(Clone)]
pub struct LeafPlacement {
    pub path: Vec<usize>,
    pub rect: Rect,
}

#[derive(Clone)]
pub struct DividerPlacement {
    /// The grab strip (wider than the line, overlapping both panes).
    pub rect: Rect,
    pub reference: DividerRef,
}

/// Rescale `flexes` to sum to their count, preserving ratios; degenerate sums reset to equal weights.
pub fn renormalize(flexes: &mut [f32]) {
    let count = flexes.len() as f32;
    let sum: f32 = flexes.iter().sum();
    if sum > 0.001 {
        let factor = count / sum;
        for flex in flexes.iter_mut() {
            *flex *= factor;
        }
    } else {
        flexes.iter_mut().for_each(|flex| *flex = 1.0);
    }
}

fn snap(rect: Rect) -> Rect {
    Rect::new(
        rect.x.round(),
        rect.y.round(),
        rect.w.round(),
        rect.h.round(),
        Rgba::TRANSPARENT,
    )
}

fn contains(rect: &Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

impl<P> Member<P> {
    pub fn leaf_at(&self, path: &[usize]) -> Option<&P> {
        match self {
            Member::Leaf(pane) => path.is_empty().then_some(pane),
            Member::Split(split) => {
                let (index, rest) = path.split_first()?;
                split.members.get(*index)?.leaf_at(rest)
            }
        }
    }

    pub fn leaf_at_mut(&mut self, path: &[usize]) -> Option<&mut P> {
        match self {
            Member::Leaf(pane) => path.is_empty().then_some(pane),
            Member::Split(split) => {
                let (index, rest) = path.split_first()?;
                split.members.get_mut(*index)?.leaf_at_mut(rest)
            }
        }
    }

    /// The split at `path`; an empty path lands on this node when it is a split.
    pub fn split_at_mut(&mut self, path: &[usize]) -> Option<&mut Split<P>> {
        match self {
            Member::Leaf(_) => None,
            Member::Split(split) => match path.split_first() {
                None => Some(split),
                Some((index, rest)) => split.members.get_mut(*index)?.split_at_mut(rest),
            },
        }
    }

    pub fn first_leaf_path(&self) -> Vec<usize> {
        let mut path = Vec::new();
        let mut current = self;
        while let Member::Split(split) = current {
            path.push(0);
            match split.members.first() {
                Some(member) => current = member,
                None => break,
            }
        }
        path
    }

    pub fn last_leaf_path(&self) -> Vec<usize> {
        let mut path = Vec::new();
        let mut current = self;
        while let Member::Split(split) = current {
            let last = split.members.len().saturating_sub(1);
            path.push(last);
            match split.members.last() {
                Some(member) => current = member,
                None => break,
            }
        }
        path
    }

    pub fn for_each_pane(&self, f: &mut dyn FnMut(&P)) {
        match self {
            Member::Leaf(pane) => f(pane),
            Member::Split(split) => split.members.iter().for_each(|m| m.for_each_pane(f)),
        }
    }

    pub fn for_each_pane_mut(&mut self, f: &mut dyn FnMut(&mut P)) {
        match self {
            Member::Leaf(pane) => f(pane),
            Member::Split(split) => split
                .members
                .iter_mut()
                .for_each(|m| m.for_each_pane_mut(f)),
        }
    }

    pub fn leaf_count(&self) -> usize {
        match self {
            Member::Leaf(_) => 1,
            Member::Split(split) => split.members.iter().map(Member::leaf_count).sum(),
        }
    }

    /// Fold every single-child split into its child, bottom-up, so removals never leave a one-way split.
    pub fn collapse(&mut self) {
        if let Member::Split(split) = self {
            for member in &mut split.members {
                member.collapse();
            }
            if split.members.len() == 1 {
                let only = split.members.remove(0);
                *self = only;
            }
        }
    }

    /// Put `new_pane` beside the leaf at `path`: in the parent's own axis it is inserted next to the leaf,
    /// otherwise the leaf becomes a two-member split. Returns the new pane's path, or the pane back when
    /// `path` is not a leaf.
    pub fn split(
        &mut self,
        path: &[usize],
        direction: SplitDirection,
        new_pane: P,
    ) -> Result<Vec<usize>, P> {
        let axis = direction.axis();
        let after = direction.increasing();
        let pair = |old: Member<P>, new: P| {
            let members = if after {
                vec![old, Member::Leaf(new)]
            } else {
                vec![Member::Leaf(new), old]
            };
            Member::Split(Split {
                axis,
                flexes: vec![1.0; 2],
                members,
            })
        };
        let Some((&index, parent_path)) = path.split_last() else {
            if !matches!(self, Member::Leaf(_)) {
                return Err(new_pane);
            }
            let old = std::mem::replace(self, Member::Split(empty_split(axis)));
            *self = pair(old, new_pane);
            return Ok(vec![usize::from(after)]);
        };
        let Some(parent) = self.split_at_mut(parent_path) else {
            return Err(new_pane);
        };
        if index >= parent.members.len() || !matches!(parent.members[index], Member::Leaf(_)) {
            return Err(new_pane);
        }
        if parent.axis == axis {
            let at = if after { index + 1 } else { index };
            parent.members.insert(at, Member::Leaf(new_pane));
            parent.flexes.insert(at, 1.0);
            renormalize(&mut parent.flexes);
            Ok([parent_path, &[at]].concat())
        } else {
            let old =
                std::mem::replace(&mut parent.members[index], Member::Split(empty_split(axis)));
            parent.members[index] = pair(old, new_pane);
            Ok([parent_path, &[index, usize::from(after)]].concat())
        }
    }

    /// Remove the leaf at `path` (never the sole root) and fold the tree; returns whether anything changed.
    pub fn remove(&mut self, path: &[usize]) -> bool {
        let Some((&index, parent_path)) = path.split_last() else {
            return false;
        };
        let Some(parent) = self.split_at_mut(parent_path) else {
            return false;
        };
        if index >= parent.members.len() {
            return false;
        }
        parent.members.remove(index);
        parent.flexes.remove(index);
        renormalize(&mut parent.flexes);
        self.collapse();
        true
    }

    /// Exchange the leaves at two paths.
    pub fn swap(&mut self, a: &[usize], b: &[usize]) -> bool {
        if a == b || self.leaf_at(a).is_none() || self.leaf_at(b).is_none() {
            return false;
        }
        let Some(first) = self.take_leaf(a) else {
            return false;
        };
        let Some(second) = self.take_leaf(b) else {
            return false;
        };
        self.put_leaf(a, second) && self.put_leaf(b, first)
    }

    fn take_leaf(&mut self, path: &[usize]) -> Option<P> {
        let slot = self.member_at_mut(path)?;
        if !matches!(slot, Member::Leaf(_)) {
            return None;
        }
        match std::mem::replace(slot, Member::Split(empty_split(Axis::Horizontal))) {
            Member::Leaf(pane) => Some(pane),
            Member::Split(_) => None,
        }
    }

    fn put_leaf(&mut self, path: &[usize], pane: P) -> bool {
        match self.member_at_mut(path) {
            Some(slot) => {
                *slot = Member::Leaf(pane);
                true
            }
            None => false,
        }
    }

    fn member_at_mut(&mut self, path: &[usize]) -> Option<&mut Member<P>> {
        match path.split_first() {
            None => Some(self),
            Some((index, rest)) => match self {
                Member::Leaf(_) => None,
                Member::Split(split) => split.members.get_mut(*index)?.member_at_mut(rest),
            },
        }
    }

    /// Lay the tree out in `rect`: each leaf's rect (in render order) and the dividers between siblings. Rects
    /// snap to whole pixels so 1px divider lines stay crisp; siblings share the container minus the lines.
    pub fn layout(&self, rect: Rect) -> (Vec<LeafPlacement>, Vec<DividerPlacement>) {
        let mut leaves = Vec::new();
        let mut dividers = Vec::new();
        let mut path = Vec::new();
        self.layout_into(snap(rect), &mut path, &mut leaves, &mut dividers);
        (leaves, dividers)
    }

    fn layout_into(
        &self,
        rect: Rect,
        path: &mut Vec<usize>,
        leaves: &mut Vec<LeafPlacement>,
        dividers: &mut Vec<DividerPlacement>,
    ) {
        let Member::Split(split) = self else {
            leaves.push(LeafPlacement {
                path: path.clone(),
                rect,
            });
            return;
        };
        let count = split.members.len();
        let (along, base) = match split.axis {
            Axis::Horizontal => (rect.w, rect.x),
            Axis::Vertical => (rect.h, rect.y),
        };
        let available = (along - count.saturating_sub(1) as f32 * DIVIDER_LINE).max(0.0);
        let mut position = 0.0f32;
        for (index, member) in split.members.iter().enumerate() {
            let start = (base + position).round();
            position += available * split.flexes[index] / count as f32;
            let end = (base + position).round();
            let extent = (end - start).max(0.0);
            let child = match split.axis {
                Axis::Horizontal => Rect::new(start, rect.y, extent, rect.h, Rgba::TRANSPARENT),
                Axis::Vertical => Rect::new(rect.x, start, rect.w, extent, Rgba::TRANSPARENT),
            };
            path.push(index);
            member.layout_into(child, path, leaves, dividers);
            path.pop();
            if index + 1 < count {
                let offset = (DIVIDER_GRAB - DIVIDER_LINE) / 2.0;
                let grab = match split.axis {
                    Axis::Horizontal => Rect::new(
                        end - offset,
                        rect.y,
                        DIVIDER_GRAB,
                        rect.h,
                        Rgba::TRANSPARENT,
                    ),
                    Axis::Vertical => Rect::new(
                        rect.x,
                        end - offset,
                        rect.w,
                        DIVIDER_GRAB,
                        Rgba::TRANSPARENT,
                    ),
                };
                dividers.push(DividerPlacement {
                    rect: grab,
                    reference: DividerRef {
                        split_path: path.clone(),
                        index,
                        axis: split.axis,
                        container: available,
                        child_start: start,
                    },
                });
                position += DIVIDER_LINE;
            }
        }
    }

    /// Drag a divider so child `index`'s trailing edge tracks the pointer: the pixel difference is emptied
    /// across successive panes (forward or backward), each clamped to its minimum, so a neighbour that can't
    /// shrink passes the change on instead of stopping the drag. Recomputed from the absolute pointer on every
    /// move, the boundary converges on the cursor over a few events.
    pub fn resize_divider(&mut self, divider: &DividerRef, x: f32, y: f32) -> bool {
        if divider.container <= 1.0 {
            return false;
        }
        let (pointer, min) = match divider.axis {
            Axis::Horizontal => (x, HORIZONTAL_MIN_SIZE),
            Axis::Vertical => (y, VERTICAL_MIN_SIZE),
        };
        let Some(split) = self.split_at_mut(&divider.split_path) else {
            return false;
        };
        let count = split.members.len();
        let index = divider.index;
        if index + 1 >= count {
            return false;
        }
        let container = divider.container;
        let flexes = &mut split.flexes;
        let size = |i: usize, flexes: &[f32]| container * flexes[i] / count as f32;
        if min - 1.0 > size(index, flexes) {
            return false;
        }
        let before = flexes.clone();
        let mut proposed = (pointer - divider.child_start) - size(index, flexes);
        let forward = proposed > 0.0;
        let mut offset = 0usize;
        while proposed.abs() > 0.0 {
            let current = if forward {
                (index + 1 + offset < count).then_some(index + offset)
            } else {
                index.checked_sub(offset)
            };
            let Some(current) = current else {
                break;
            };
            offset += 1;
            let next_target = (size(current + 1, flexes) - proposed).max(min);
            let current_target =
                (size(current, flexes) + size(current + 1, flexes) - next_target).max(min);
            let change = current_target - size(current, flexes);
            let flex_change = change / container;
            flexes[current] += flex_change;
            flexes[current + 1] -= flex_change;
            proposed -= change;
        }
        *flexes != before
    }
}

fn empty_split<P>(axis: Axis) -> Split<P> {
    Split {
        axis,
        members: Vec::new(),
        flexes: Vec::new(),
    }
}

impl<P: PaneId> Member<P> {
    /// The path to the leaf with stable id `id`, if it still exists.
    pub fn path_of(&self, id: u64) -> Option<Vec<usize>> {
        fn walk<P: PaneId>(
            member: &Member<P>,
            id: u64,
            path: &mut Vec<usize>,
        ) -> Option<Vec<usize>> {
            match member {
                Member::Leaf(pane) => (pane.pane_id() == id).then(|| path.clone()),
                Member::Split(split) => {
                    for (index, child) in split.members.iter().enumerate() {
                        path.push(index);
                        let found = walk(child, id, path);
                        path.pop();
                        if found.is_some() {
                            return found;
                        }
                    }
                    None
                }
            }
        }
        walk(self, id, &mut Vec::new())
    }
}

/// The leaf whose rect contains `(x, y)`.
pub fn leaf_at_point(leaves: &[LeafPlacement], x: f32, y: f32) -> Option<&LeafPlacement> {
    leaves.iter().find(|leaf| contains(&leaf.rect, x, y))
}

/// The neighbouring pane in `direction` from the leaf at `active`: probe just past that edge, level with the
/// caret when it is inside the pane, else with the pane's center.
pub fn find_pane_in_direction(
    leaves: &[LeafPlacement],
    active: &[usize],
    direction: SplitDirection,
    caret: Option<(f32, f32)>,
) -> Option<Vec<usize>> {
    let rect = leaves.iter().find(|leaf| leaf.path == active)?.rect;
    let (cx, cy) = match caret {
        Some(point) if contains(&rect, point.0, point.1) => point,
        _ => (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0),
    };
    let (x, y) = match direction {
        SplitDirection::Left => (rect.x - NEIGHBOUR_PROBE, cy),
        SplitDirection::Right => (rect.x + rect.w + NEIGHBOUR_PROBE, cy),
        SplitDirection::Up => (cx, rect.y - NEIGHBOUR_PROBE),
        SplitDirection::Down => (cx, rect.y + rect.h + NEIGHBOUR_PROBE),
    };
    leaf_at_point(leaves, x, y).map(|leaf| leaf.path.clone())
}

/// Where a tab dropped at `(x, y)` in `rect` lands: on an outer edge band, the side it is nearest to (a split);
/// in the interior, `None` (into the pane). Also the preview rect to highlight.
pub fn drop_target(rect: Rect, x: f32, y: f32) -> (Option<SplitDirection>, Rect) {
    let zone = DROP_EDGE * rect.w.min(rect.h);
    let (lx, ly) = (x - rect.x, y - rect.y);
    let on_edge = lx < zone || lx > rect.w - zone || ly < zone || ly > rect.h - zone;
    let direction = on_edge.then(|| {
        let distances = [
            (SplitDirection::Up, ly),
            (SplitDirection::Right, rect.w - lx),
            (SplitDirection::Down, rect.h - ly),
            (SplitDirection::Left, lx),
        ];
        distances
            .iter()
            .copied()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(SplitDirection::Up, |(direction, _)| direction)
    });
    let half = |x, y, w, h| Rect::new(x, y, w, h, Rgba::TRANSPARENT);
    let preview = match direction {
        Some(SplitDirection::Left) => half(rect.x, rect.y, rect.w / 2.0, rect.h),
        Some(SplitDirection::Right) => half(rect.x + rect.w / 2.0, rect.y, rect.w / 2.0, rect.h),
        Some(SplitDirection::Up) => half(rect.x, rect.y, rect.w, rect.h / 2.0),
        Some(SplitDirection::Down) => half(rect.x, rect.y + rect.h / 2.0, rect.w, rect.h / 2.0),
        None => rect,
    };
    (direction, preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    impl PaneId for u64 {
        fn pane_id(&self) -> u64 {
            *self
        }
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0, Rgba::TRANSPARENT)
    }

    #[test]
    fn splits_nest_by_axis_and_collapse_on_remove() {
        let mut group: Member<u64> = Member::Leaf(1);
        assert_eq!(group.split(&[], SplitDirection::Right, 2), Ok(vec![1]));
        assert_eq!(group.split(&[1], SplitDirection::Right, 3), Ok(vec![2]));
        assert_eq!(group.split(&[1], SplitDirection::Down, 4), Ok(vec![1, 1]));
        assert_eq!(group.leaf_count(), 4);
        assert_eq!(group.path_of(4), Some(vec![1, 1]));
        assert_eq!(group.split(&[0], SplitDirection::Left, 5), Ok(vec![0]));
        assert_eq!(group.path_of(1), Some(vec![1]));
        assert!(group.remove(&[2, 0]));
        assert_eq!(group.path_of(4), Some(vec![2]));
        assert_eq!(group.leaf_count(), 4);
        assert!(!group.remove(&[]));
    }

    #[test]
    fn layout_shares_the_container_and_places_dividers() {
        let mut group: Member<u64> = Member::Leaf(1);
        assert!(group.split(&[], SplitDirection::Right, 2).is_ok());
        assert!(group.split(&[1], SplitDirection::Down, 3).is_ok());
        let (leaves, dividers) = group.layout(area());
        assert_eq!(leaves.len(), 3);
        assert_eq!(dividers.len(), 2);
        assert_eq!((leaves[0].rect.x, leaves[0].rect.w), (0.0, 500.0));
        assert_eq!((leaves[1].rect.x, leaves[1].rect.w), (501.0, 499.0));
        assert_eq!((leaves[2].rect.y, leaves[2].rect.h), (301.0, 299.0));
        assert_eq!(dividers[0].reference.axis, Axis::Horizontal);
        assert_eq!(
            dividers[0].rect.x,
            500.0 - (DIVIDER_GRAB - DIVIDER_LINE) / 2.0
        );
        assert_eq!(dividers[1].reference.axis, Axis::Vertical);
        assert_eq!(dividers[1].reference.split_path, vec![1]);
    }

    #[test]
    fn dragging_a_divider_respects_minimum_sizes() {
        let mut group: Member<u64> = Member::Leaf(1);
        assert!(group.split(&[], SplitDirection::Right, 2).is_ok());
        let (_, dividers) = group.layout(area());
        let divider = dividers[0].reference.clone();
        assert!(group.resize_divider(&divider, 700.0, 0.0));
        for _ in 0..40 {
            group.resize_divider(&divider, 700.0, 0.0);
        }
        let (leaves, _) = group.layout(area());
        assert!((leaves[0].rect.w - 700.0).abs() <= 1.0);
        for _ in 0..20 {
            group.resize_divider(&divider, 5.0, 0.0);
        }
        let (leaves, _) = group.layout(area());
        assert!(leaves[0].rect.w >= HORIZONTAL_MIN_SIZE - 1.0);
    }

    #[test]
    fn neighbours_swap_and_drop_zones() {
        let mut group: Member<u64> = Member::Leaf(1);
        assert!(group.split(&[], SplitDirection::Right, 2).is_ok());
        assert!(group.split(&[1], SplitDirection::Down, 3).is_ok());
        let (leaves, _) = group.layout(area());
        assert_eq!(
            find_pane_in_direction(&leaves, &[0], SplitDirection::Right, Some((100.0, 100.0))),
            Some(vec![1, 0])
        );
        assert_eq!(
            find_pane_in_direction(&leaves, &[1, 0], SplitDirection::Down, None),
            Some(vec![1, 1])
        );
        assert_eq!(
            find_pane_in_direction(&leaves, &[0], SplitDirection::Left, None),
            None
        );
        assert!(group.swap(&[0], &[1, 1]));
        assert_eq!(group.path_of(3), Some(vec![0]));
        let rect = leaves[0].rect;
        assert_eq!(drop_target(rect, 10.0, 300.0).0, Some(SplitDirection::Left));
        assert_eq!(
            drop_target(rect, 250.0, 590.0).0,
            Some(SplitDirection::Down)
        );
        assert_eq!(drop_target(rect, 250.0, 300.0).0, None);
    }
}
