//! Dragging a tab between panes of a pane group: where a drop would land (onto a tab, the tab bar's free end,
//! a pane edge to split, or a pane's middle) and moving the item there, pruning the pane it left.

use ui::{div, icon, label, material_icon, theme, IconKind, MaterialIcon, Node, Rect, Rgba};

use crate::pane::{Pane, TAB_H};
use crate::pane_group::{self, Member, SplitDirection};
use crate::Item;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// Into the pane's tab bar before tab `index` (`index == len` appends).
    Insert(usize),
    /// Split the pane on this side and move the tab into the new pane.
    Split(SplitDirection),
    /// Onto the pane's body: move the tab to the end of that pane.
    Append,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabDrop {
    pub pane: u64,
    pub target: DropTarget,
}

#[derive(Clone, Copy)]
enum Glyph {
    Mono(IconKind),
    File(MaterialIcon),
}

/// A tab being dragged: where it came from (pane id and tab index), how to draw its ghost, and the drop the
/// pointer currently resolves to (`None` when it is over nothing that accepts the tab).
pub struct TabDrag {
    pub source: u64,
    pub index: usize,
    title: String,
    glyph: Glyph,
    pub drop: Option<TabDrop>,
    pub preview: Option<Rect>,
}

impl TabDrag {
    pub fn begin(pane: &Pane, index: usize) -> Option<Self> {
        let item = pane.open.get(index)?;
        let glyph = match item.tab_icon() {
            Some(kind) => Glyph::Mono(kind),
            None => Glyph::File(item.icon().unwrap_or(MaterialIcon::Document)),
        };
        Some(Self {
            source: pane.id,
            index,
            title: item.title(),
            glyph,
            drop: None,
            preview: None,
        })
    }

    /// A floating copy of the dragged tab for the pointer to carry: its node and size.
    pub fn ghost(&self) -> (Node, f32, f32) {
        let width = (self.title.chars().count() as f32 * 7.5 + 52.0).clamp(90.0, 260.0);
        let glyph: Node = match self.glyph {
            Glyph::Mono(kind) => icon(kind).size(14.0).color(theme().icon_muted).into(),
            Glyph::File(kind) => material_icon(kind).size(14.0).into(),
        };
        let node = div()
            .row()
            .items_center()
            .gap(6.0)
            .px(10.0)
            .h_px(TAB_H)
            .rounded(6.0)
            .bg(theme().elevated_surface_background)
            .border(1.0, theme().border)
            .child(glyph)
            .child(label(self.title.clone()).size(13.0).color(theme().text))
            .into();
        (node, width, TAB_H)
    }
}

/// Where a drop at `(x, y)` over a pane at `rect` lands, and the area to highlight. `over_tab` is the tab under
/// the pointer (index and rect) when it is over one; the rest of the tab bar appends; the body splits at an
/// edge band or appends in the middle.
pub fn resolve_drop(
    rect: Rect,
    tab_count: usize,
    x: f32,
    y: f32,
    over_tab: Option<(usize, Rect)>,
) -> (DropTarget, Rect) {
    if let Some((index, tab)) = over_tab {
        return (DropTarget::Insert(index), tab);
    }
    let bar_h = TAB_H * ui::ui_text_scale();
    if y < rect.y + bar_h {
        let bar = Rect::new(rect.x, rect.y, rect.w, bar_h, Rgba::TRANSPARENT);
        return (DropTarget::Insert(tab_count), bar);
    }
    let body = Rect::new(
        rect.x,
        rect.y + bar_h,
        rect.w,
        (rect.h - bar_h).max(0.0),
        Rgba::TRANSPARENT,
    );
    match pane_group::drop_target(body, x, y) {
        (Some(direction), preview) => (DropTarget::Split(direction), preview),
        (None, preview) => (DropTarget::Append, preview),
    }
}

/// Move the dragged tab to its drop. `new_pane` makes the pane for a split. Returns the path of the pane that
/// now holds the tab (the one to focus); an emptied source pane leaves the group unless it is the last one.
pub fn apply_drop(
    group: &mut Member<Pane>,
    drag: &TabDrag,
    drop: TabDrop,
    new_pane: impl FnOnce() -> Pane,
) -> Option<Vec<usize>> {
    let source_path = group.path_of(drag.source)?;
    let same_pane = drop.pane == drag.source;
    if same_pane && drop.target == DropTarget::Append {
        return Some(source_path);
    }
    if same_pane {
        if let DropTarget::Insert(index) = drop.target {
            let pane = group.leaf_at_mut(&source_path)?;
            if drag.index >= pane.open.len() {
                return None;
            }
            let item = pane.open.remove(drag.index);
            let at = if index > drag.index { index - 1 } else { index }.min(pane.open.len());
            pane.open.insert(at, item);
            pane.active = Some(at);
            return Some(source_path);
        }
    }
    let item = detach(group, drag)?;
    let landed = match insert_item(group, drop, item, new_pane) {
        Ok(path) => group.leaf_at(&path).map(|pane| pane.id),
        Err(item) => {
            let pane = group.leaf_at_mut(&source_path)?;
            pane.add_item(item?);
            Some(pane.id)
        }
    };
    prune(group, drag.source);
    landed.and_then(|id| group.path_of(id))
}

fn detach(group: &mut Member<Pane>, drag: &TabDrag) -> Option<Box<dyn Item>> {
    let source_path = group.path_of(drag.source)?;
    let pane = group.leaf_at_mut(&source_path)?;
    if drag.index >= pane.open.len() {
        return None;
    }
    let item = pane.open.remove(drag.index);
    pane.active = if pane.open.is_empty() {
        None
    } else {
        Some(pane.active.unwrap_or(0).min(pane.open.len() - 1))
    };
    Some(item)
}

/// Drop pane `id` from the group if it has no tabs left, unless it is the last pane.
fn prune(group: &mut Member<Pane>, id: u64) {
    if let Some(path) = group.path_of(id) {
        let empty = group
            .leaf_at(&path)
            .is_some_and(|pane| pane.open.is_empty());
        if empty && group.leaf_count() > 1 {
            group.remove(&path);
        }
    }
}

/// Remove the dragged tab from its pane for a move to another group, dropping the pane when it empties
/// (unless it is the last one). Pair with `insert_item` on the other group.
pub fn take_item(group: &mut Member<Pane>, drag: &TabDrag) -> Option<Box<dyn Item>> {
    let item = detach(group, drag)?;
    prune(group, drag.source);
    Some(item)
}

/// Place `item` at `drop` in this group; hands the item back when the target pane is gone.
pub fn insert_item(
    group: &mut Member<Pane>,
    drop: TabDrop,
    item: Box<dyn Item>,
    new_pane: impl FnOnce() -> Pane,
) -> Result<Vec<usize>, Option<Box<dyn Item>>> {
    let Some(target_path) = group.path_of(drop.pane) else {
        return Err(Some(item));
    };
    match drop.target {
        DropTarget::Split(direction) => {
            let mut pane = new_pane();
            pane.add_item(item);
            group
                .split(&target_path, direction, pane)
                .map_err(|mut pane| pane.open.pop())
        }
        DropTarget::Insert(index) => {
            let Some(pane) = group.leaf_at_mut(&target_path) else {
                return Err(Some(item));
            };
            let at = index.min(pane.open.len());
            pane.open.insert(at, item);
            pane.active = Some(at);
            Ok(target_path)
        }
        DropTarget::Append => {
            let Some(pane) = group.leaf_at_mut(&target_path) else {
                return Err(Some(item));
            };
            pane.add_item(item);
            Ok(target_path)
        }
    }
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
            div().into()
        }
    }

    fn pane(id: u64, names: &[&'static str]) -> Pane {
        let mut pane = Pane::new(id);
        for name in names {
            pane.add_item(Box::new(Plain(name)));
        }
        pane
    }

    fn titles(group: &Member<Pane>, path: &[usize]) -> Vec<String> {
        group
            .leaf_at(path)
            .map(|pane| pane.open.iter().map(|item| item.title()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn drop_zones_follow_tab_bar_and_body() {
        let rect = Rect::new(0.0, 0.0, 400.0, 300.0, Rgba::TRANSPARENT);
        let tab = Rect::new(10.0, 0.0, 80.0, 32.0, Rgba::TRANSPARENT);
        assert_eq!(
            resolve_drop(rect, 3, 20.0, 10.0, Some((1, tab))).0,
            DropTarget::Insert(1)
        );
        assert_eq!(
            resolve_drop(rect, 3, 300.0, 10.0, None).0,
            DropTarget::Insert(3)
        );
        assert_eq!(
            resolve_drop(rect, 3, 390.0, 150.0, None).0,
            DropTarget::Split(SplitDirection::Right)
        );
        assert_eq!(
            resolve_drop(rect, 3, 200.0, 160.0, None).0,
            DropTarget::Append
        );
    }

    #[test]
    fn reorder_move_split_and_prune() {
        let mut group = Member::Leaf(pane(1, &["a", "b", "c"]));
        let drag = TabDrag::begin(group.leaf_at(&[]).unwrap(), 0).unwrap();
        let landed = apply_drop(
            &mut group,
            &drag,
            TabDrop {
                pane: 1,
                target: DropTarget::Insert(3),
            },
            || pane(9, &[]),
        );
        assert_eq!(landed, Some(vec![]));
        assert_eq!(titles(&group, &[]), ["b", "c", "a"]);

        let drag = TabDrag::begin(group.leaf_at(&[]).unwrap(), 2).unwrap();
        let landed = apply_drop(
            &mut group,
            &drag,
            TabDrop {
                pane: 1,
                target: DropTarget::Split(SplitDirection::Right),
            },
            || pane(2, &[]),
        );
        assert_eq!(landed, Some(vec![1]));
        assert_eq!(titles(&group, &[0]), ["b", "c"]);
        assert_eq!(titles(&group, &[1]), ["a"]);

        let drag = TabDrag::begin(group.leaf_at(&[1]).unwrap(), 0).unwrap();
        let landed = apply_drop(
            &mut group,
            &drag,
            TabDrop {
                pane: 1,
                target: DropTarget::Insert(0),
            },
            || pane(3, &[]),
        );
        assert_eq!(landed, Some(vec![]));
        assert_eq!(group.leaf_count(), 1);
        assert_eq!(titles(&group, &[]), ["a", "b", "c"]);
    }
}
