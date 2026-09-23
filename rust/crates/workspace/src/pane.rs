//! A pane: an ordered set of tabs (any `Item`), the active one, its back/forward history and its find bar,
//! plus the tab bar every pane draws. Owners (the editor center, the terminal panel, later other features)
//! configure which controls the tab bar shows and hand in the click ids for this pane.

use ui::{div, icon, label, material_icon, theme, IconKind, MaterialIcon, Node};

use crate::pane_group::PaneId;
use crate::search_bar::{SearchBar, Searchable};
use crate::Item;

pub const TAB_H: f32 = 32.0;
const MAX_NAVIGATION_HISTORY_LEN: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct NavEntry {
    pub id: String,
    pub cursor: Option<usize>,
    pub scroll: Option<(f32, f32)>,
    pub row: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavMode {
    Normal,
    GoingBack,
    GoingForward,
}

#[derive(Default)]
pub struct Pane {
    pub id: u64,
    pub open: Vec<Box<dyn Item>>,
    pub active: Option<usize>,
    pub back: Vec<NavEntry>,
    pub fwd: Vec<NavEntry>,
    pub search: Box<SearchBar>,
}

impl PaneId for Pane {
    fn pane_id(&self) -> u64 {
        self.id
    }
}

impl Pane {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }

    /// Height of the chrome above the active item's body (tab bar plus the find bar when open); an empty pane
    /// draws no chrome.
    pub fn header_h(&self) -> f32 {
        if self.open.is_empty() {
            0.0
        } else {
            TAB_H + self.search.height()
        }
    }

    /// The find bar with the active item it searches, when that item is searchable.
    pub fn search_target(&mut self) -> Option<(&mut SearchBar, &mut dyn Searchable)> {
        let index = self.active?;
        let item = self.open.get_mut(index)?.searchable()?;
        Some((self.search.as_mut(), item))
    }

    pub fn index_of_id(&self, id: &str) -> Option<usize> {
        self.open.iter().position(|o| o.id().as_deref() == Some(id))
    }

    pub fn nav_entry_for(&self, index: usize) -> Option<NavEntry> {
        let item = self.open.get(index)?;
        let id = item.id()?;
        Some(match item.nav_position() {
            Some((cursor, row, scroll)) => NavEntry {
                id,
                cursor: Some(cursor),
                scroll: Some(scroll),
                row: Some(row),
            },
            None => NavEntry {
                id,
                cursor: None,
                scroll: None,
                row: None,
            },
        })
    }

    pub fn push_nav(&mut self, entry: NavEntry, mode: NavMode) {
        let same = |e: &NavEntry| e.id == entry.id && e.row == entry.row;
        let stack = match mode {
            NavMode::GoingBack => &mut self.fwd,
            NavMode::Normal | NavMode::GoingForward => &mut self.back,
        };
        stack.retain(|e| !same(e));
        if stack.len() >= MAX_NAVIGATION_HISTORY_LEN {
            stack.remove(0);
        }
        stack.push(entry);
        if mode == NavMode::Normal {
            self.fwd.clear();
        }
    }

    fn deactivate_active(&mut self, mode: NavMode) {
        if let Some(entry) = self.active.and_then(|i| self.nav_entry_for(i)) {
            self.push_nav(entry, mode);
        }
    }

    /// Activate tab `index` because the user asked to, recording where they came from.
    pub fn activate_user(&mut self, index: usize) {
        if self.active == Some(index) {
            return;
        }
        self.deactivate_active(NavMode::Normal);
        self.active = Some(index);
    }

    /// Append `item` as a new tab and activate it.
    pub fn add_item(&mut self, item: Box<dyn Item>) {
        self.open.push(item);
        self.activate_user(self.open.len() - 1);
    }

    pub fn can_back(&self) -> bool {
        self.back.iter().any(|e| self.index_of_id(&e.id).is_some())
    }

    pub fn can_forward(&self) -> bool {
        self.fwd.iter().any(|e| self.index_of_id(&e.id).is_some())
    }

    /// Step through history, skipping entries whose tab has closed or that would not move anything.
    fn navigate(&mut self, mode: NavMode) {
        loop {
            let entry = match mode {
                NavMode::GoingBack => self.back.pop(),
                NavMode::GoingForward => self.fwd.pop(),
                NavMode::Normal => None,
            };
            let Some(entry) = entry else {
                return;
            };
            let Some(index) = self.index_of_id(&entry.id) else {
                continue;
            };
            self.deactivate_active(mode);
            let previous = self.active.replace(index);
            let mut navigated = previous != Some(index);
            if let (Some(cursor), Some(scroll), Some(item)) =
                (entry.cursor, entry.scroll, self.open.get_mut(index))
            {
                navigated |= item.navigate_to(cursor, scroll);
            }
            if navigated {
                return;
            }
        }
    }

    pub fn nav_back(&mut self) {
        self.navigate(NavMode::GoingBack);
    }

    pub fn nav_forward(&mut self) {
        self.navigate(NavMode::GoingForward);
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.open.len() {
            return;
        }
        self.open.remove(index);
        self.active = if self.open.is_empty() {
            None
        } else {
            Some(self.active.unwrap_or(0).min(self.open.len() - 1))
        };
    }

    pub fn active_item(&self) -> Option<&dyn Item> {
        self.active
            .and_then(|i| self.open.get(i))
            .map(|b| b.as_ref())
    }

    pub fn active_item_mut(&mut self) -> Option<&mut dyn Item> {
        let index = self.active?;
        self.open.get_mut(index).map(|b| b.as_mut())
    }
}

/// An icon button at the trailing end of the tab bar (split, new, zoom, ...).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TabBarButton {
    pub icon: IconKind,
    pub id: u64,
}

/// What a pane's tab bar shows besides its tabs.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TabBarConfig {
    /// Back/forward through the pane's history at the leading edge.
    pub show_nav: bool,
    pub buttons: Vec<TabBarButton>,
}

/// Click ids for one pane's chrome: tab ids are these bases plus the tab index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneClickIds {
    pub tab_activate: u64,
    pub tab_close: u64,
    pub nav_back: u64,
    pub nav_forward: u64,
    pub search: u64,
}

/// The pane chrome: a tab bar whose full-width bottom border the active tab punches through (so it merges with
/// the body below), then the find bar when open. The body itself is drawn by the owner. An empty pane draws
/// nothing.
pub fn render_pane(
    pane: &Pane,
    config: &TabBarConfig,
    ids: PaneClickIds,
    hover: Option<u64>,
    width: f32,
) -> Node {
    if pane.open.is_empty() {
        return div().col().flex(1.0).into();
    }
    let mut tab_bar = div().row().h_px(TAB_H).bg(theme().tab_bar_background);
    if config.show_nav {
        tab_bar = tab_bar
            .child(nav_button(
                IconKind::ArrowLeft,
                ids.nav_back,
                pane.can_back(),
            ))
            .child(nav_button(
                IconKind::ArrowRight,
                ids.nav_forward,
                pane.can_forward(),
            ))
            .child(div().w_px(1.0).h_px(TAB_H).bg(theme().border));
    }
    for (index, item) in pane.open.iter().enumerate() {
        let is_active = pane.active == Some(index);
        let activate_id = ids.tab_activate + index as u64;
        let close_id = ids.tab_close + index as u64;
        let hovered = hover == Some(activate_id) || hover == Some(close_id);
        let underline = if is_active {
            item.body_background()
        } else {
            theme().border
        };
        // The close slot keeps its width whether or not the glyph shows, so hovering never shifts the tab.
        let mut close_slot = div()
            .w_px(16.0)
            .h_px(16.0)
            .rounded(4.0)
            .items_center()
            .justify_center()
            .on_click(close_id);
        if hovered {
            close_slot =
                close_slot.child(icon(IconKind::Close).size(11.0).color(theme().icon_muted));
        }
        let mut dirty_slot = div().w_px(12.0).h_px(12.0).items_center().justify_center();
        if item.is_dirty() {
            let color = if item.has_conflict() {
                theme().warning
            } else {
                theme().text_accent
            };
            dirty_slot = dirty_slot.child(div().w_px(6.0).h_px(6.0).rounded(3.0).bg(color));
        }
        let glyph: Node = match item.tab_icon() {
            Some(kind) => icon(kind).size(14.0).color(theme().icon_muted).into(),
            None => material_icon(item.icon().unwrap_or(MaterialIcon::Document))
                .size(14.0)
                .into(),
        };
        let content = div()
            .row()
            .flex(1.0)
            .px(10.0)
            .gap(6.0)
            .items_center()
            .child(dirty_slot)
            .child(glyph)
            .child(label(item.title()).size(13.0).color(if is_active {
                theme().text
            } else {
                theme().text_muted
            }))
            .child(close_slot);
        let cell = div()
            .col()
            .h_px(TAB_H)
            .bg(if is_active {
                theme().tab_active_background
            } else {
                theme().tab_inactive_background
            })
            .on_click(activate_id)
            .child(content)
            .child(div().h_px(1.0).bg(underline));
        tab_bar = tab_bar
            .child(cell)
            .child(div().w_px(1.0).h_px(TAB_H).bg(theme().border));
    }
    tab_bar = tab_bar.child(
        div()
            .col()
            .flex(1.0)
            .h_px(TAB_H)
            .child(div().flex(1.0))
            .child(div().h_px(1.0).bg(theme().border)),
    );
    if !config.buttons.is_empty() {
        tab_bar = tab_bar.child(div().w_px(1.0).h_px(TAB_H).bg(theme().border));
        for button in &config.buttons {
            tab_bar = tab_bar.child(tab_bar_button(button.icon, button.id));
        }
    }
    let mut chrome = div().col().flex(1.0).child(tab_bar);
    if !pane.search.dismissed {
        chrome = chrome.child(pane.search.render(ids.search, width));
    }
    chrome.into()
}

fn nav_button(kind: IconKind, id: u64, enabled: bool) -> Node {
    let color = if enabled {
        theme().icon_muted
    } else {
        theme().icon_muted.alpha(0.35)
    };
    let mut inner = div()
        .row()
        .flex(1.0)
        .items_center()
        .justify_center()
        .child(icon(kind).size(15.0).color(color));
    if enabled {
        inner = inner.on_click(id);
    }
    div()
        .col()
        .w_px(26.0)
        .h_px(TAB_H)
        .child(inner)
        .child(div().h_px(1.0).bg(theme().border))
        .into()
}

fn tab_bar_button(kind: IconKind, id: u64) -> Node {
    div()
        .col()
        .w_px(26.0)
        .h_px(TAB_H)
        .child(
            div()
                .row()
                .flex(1.0)
                .items_center()
                .justify_center()
                .on_click(id)
                .child(icon(kind).size(13.0).color(theme().icon_muted)),
        )
        .child(div().h_px(1.0).bg(theme().border))
        .into()
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
        fn tab_icon(&self) -> Option<IconKind> {
            Some(IconKind::Terminal)
        }
        fn render(&mut self) -> Node {
            div().into()
        }
    }

    fn ids() -> PaneClickIds {
        PaneClickIds {
            tab_activate: 100,
            tab_close: 200,
            nav_back: 300,
            nav_forward: 301,
            search: 400,
        }
    }

    #[test]
    fn history_follows_tab_activation_and_skips_closed_tabs() {
        let mut pane = Pane::new(1);
        pane.add_item(Box::new(Plain("a")));
        pane.add_item(Box::new(Plain("b")));
        pane.add_item(Box::new(Plain("c")));
        assert_eq!(pane.active, Some(2));
        assert!(pane.can_back());
        pane.nav_back();
        assert_eq!(pane.active, Some(1));
        assert!(pane.can_forward());
        pane.close_tab(0);
        assert_eq!(pane.active, Some(1));
        pane.nav_back();
        assert_eq!(
            pane.active_item().and_then(|item| item.id()).as_deref(),
            Some("c")
        );
        assert!(pane.search_target().is_none());
    }

    #[test]
    fn tab_bar_shows_only_configured_controls() {
        let mut pane = Pane::new(1);
        pane.add_item(Box::new(Plain("shell")));
        let bare = TabBarConfig::default();
        let painted = ui::render(
            &render_pane(&pane, &bare, ids(), None, 600.0),
            ui::Rect::new(0.0, 0.0, 600.0, 40.0, ui::Rgba::TRANSPARENT),
        );
        let hit_ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
        assert!(hit_ids.contains(&100) && hit_ids.contains(&200));
        assert!(!hit_ids.contains(&300));
        let full = TabBarConfig {
            show_nav: true,
            buttons: vec![TabBarButton {
                icon: IconKind::Plus,
                id: 500,
            }],
        };
        let painted = ui::render(
            &render_pane(&pane, &full, ids(), None, 600.0),
            ui::Rect::new(0.0, 0.0, 600.0, 40.0, ui::Rgba::TRANSPARENT),
        );
        assert!(painted.hits.iter().any(|(_, id)| *id == 500));
        assert!(painted.texts.iter().any(|text| text.text == "shell"));
        let empty = Pane::new(2);
        let painted = ui::render(
            &render_pane(&empty, &full, ids(), None, 600.0),
            ui::Rect::new(0.0, 0.0, 600.0, 40.0, ui::Rgba::TRANSPARENT),
        );
        assert!(painted.hits.is_empty());
    }
}
