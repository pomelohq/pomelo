use std::cell::RefCell;
use std::rc::Rc;

use module_store::{format_size, Entry};
use terminal::Modifiers;
use ui::{div, label, theme, ButtonStyle, IconKind, LabelSize, Node, Rect, Rgba};
use workspace::{Item, ItemTick};

pub const TAB_ID: &str = "module-store";

const PRUNE: u64 = 1;
const CLEAR: u64 = 2;
const REFRESH: u64 = 3;
const ROW_BASE: u64 = 1_000;
const DELETE_BASE: u64 = 100_000;
const CONTENT_MAX_W: f32 = 980.0;
const PAD_X: f32 = 32.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Refresh,
    Delete { repo: String, key: String },
    Prune,
    Clear,
}

#[derive(Default)]
pub struct StoreState {
    /// How the store reaches project folders on this drive, e.g. "Copy-on-write clone".
    pub method: String,
    pub entries: Vec<Entry>,
    pub loaded: bool,
    /// Work in flight ("Removing copies..."); the actions wait for it.
    pub busy: Option<String>,
    /// The last action's result.
    pub message: Option<String>,
    /// Seconds since the epoch, for "used 2 days ago".
    pub now: u64,
    pub selected: Option<(String, String)>,
    pub requests: Vec<Request>,
    pub version: u64,
}

pub type Shared = Rc<RefCell<StoreState>>;

impl StoreState {
    pub fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }
}

pub fn ago(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ if seconds < 2 * 86_400 => "yesterday".into(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

fn cell(text: impl Into<String>, width: f32, color: Rgba) -> Node {
    div()
        .row()
        .w_px(width)
        .child(
            label(text.into())
                .label_size(LabelSize::Small)
                .color(color)
                .truncate(),
        )
        .into()
}

fn workspaces_label(count: usize) -> String {
    match count {
        0 => "no workspace".into(),
        1 => "1 workspace".into(),
        count => format!("{count} workspaces"),
    }
}

fn header(state: &StoreState) -> Node {
    let colors = theme();
    let total: u64 = state.entries.iter().map(|entry| entry.size).sum();
    let summary = if state.loaded {
        format!(
            "{} copies - {} (files shared with clones are counted in full)",
            state.entries.len(),
            format_size(total)
        )
    } else {
        "Measuring the store...".to_string()
    };
    let idle = state.busy.is_none();
    let action = |id: u64, text: &str| {
        let button = ui::button(id, text, ButtonStyle::Outlined);
        if idle {
            button
        } else {
            ui::button_static(text, ButtonStyle::Outlined)
        }
    };
    let mut column = div()
        .col()
        .gap(8.0)
        .child(
            div()
                .row()
                .items_center()
                .gap(8.0)
                .child(
                    div().row().flex(1.0).child(
                        label("node_modules Store")
                            .label_size(LabelSize::Large)
                            .color(colors.text),
                    ),
                )
                .child(action(REFRESH, "Refresh"))
                .child(action(PRUNE, "Prune"))
                .child(action(CLEAR, "Clear All")),
        )
        .child(workspace::status_line(
            IconKind::Package,
            colors.icon_muted,
            &format!("Import method: {}", state.method),
        ))
        .child(
            label(summary)
                .label_size(LabelSize::Small)
                .color(colors.text_muted),
        );
    let note = state.busy.as_ref().or(state.message.as_ref());
    if let Some(note) = note {
        column = column.child(label(note.clone()).label_size(LabelSize::Small).color(
            if state.busy.is_some() {
                colors.text_accent
            } else {
                colors.text_muted
            },
        ));
    }
    column.into()
}

fn row(state: &StoreState, index: usize, entry: &Entry, hovered: Option<u64>) -> Node {
    let colors = theme();
    let selected = state
        .selected
        .as_ref()
        .is_some_and(|(repo, key)| *repo == entry.repo && *key == entry.key);
    let manager = if entry.manager.is_empty() {
        "-".to_string()
    } else {
        entry.manager.clone()
    };
    let delete_id = DELETE_BASE + index as u64;
    let delete = if state.busy.is_none() {
        ui::button(delete_id, "Delete", ButtonStyle::Subtle)
    } else {
        ui::button_static("Delete", ButtonStyle::Subtle)
    };
    let mut line = div()
        .row()
        .items_center()
        .gap(10.0)
        .px(12.0)
        .h_px(32.0)
        .on_click(ROW_BASE + index as u64)
        .child(
            div().row().flex(1.0).child(
                label(entry.repo.clone())
                    .label_size(LabelSize::Small)
                    .color(colors.text)
                    .truncate(),
            ),
        )
        .child(cell(manager, 48.0, colors.text_muted))
        .child(
            div().row().w_px(76.0).justify_end().child(
                label(format_size(entry.size))
                    .label_size(LabelSize::Small)
                    .color(colors.text),
            ),
        )
        .child(cell(
            format!("used {}", ago(state.now, entry.last_used)),
            120.0,
            colors.text_muted,
        ))
        .child(cell(
            workspaces_label(entry.live_workspaces().len()),
            100.0,
            colors.text_muted,
        ))
        .child(delete);
    if selected {
        line = line.bg(colors.element_selected);
    } else if hovered == Some(ROW_BASE + index as u64) {
        line = line.bg(colors.ghost_element_hover);
    }
    line.into()
}

fn detail_line(name: &str, value: String) -> Node {
    div()
        .row()
        .gap(10.0)
        .child(cell(name, 120.0, theme().text_placeholder))
        .child(
            label(value)
                .label_size(LabelSize::Small)
                .color(theme().text_muted)
                .truncate(),
        )
        .into()
}

fn detail(state: &StoreState, entry: &Entry, root: &str) -> Node {
    let mut column = div()
        .col()
        .gap(6.0)
        .px(12.0)
        .py(10.0)
        .bg(theme().surface_background)
        .child(detail_line("Key", entry.key.clone()))
        .child(detail_line(
            "Path",
            format!("{root}/{}/{}/node_modules", entry.repo, entry.key),
        ))
        .child(detail_line("Stored by", entry.method.label().to_string()))
        .child(detail_line("Kept", ago(state.now, entry.created)));
    let live = entry.live_workspaces();
    if live.is_empty() {
        column = column.child(detail_line("Workspaces", "none still exists".into()));
    }
    for (index, path) in live.iter().enumerate() {
        column = column.child(detail_line(
            if index == 0 { "Workspaces" } else { "" },
            path.display().to_string(),
        ));
    }
    column.into()
}

/// `root` is the store folder, shown in each copy's details.
pub fn render(state: &StoreState, root: &str, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let mut list = div().col().rounded(8.0).border(1.0, colors.border_variant);
    for (index, entry) in state.entries.iter().enumerate() {
        if index > 0 {
            list = list.child(div().h_px(1.0).bg(colors.border_variant));
        }
        list = list.child(row(state, index, entry, hovered));
        let selected = state
            .selected
            .as_ref()
            .is_some_and(|(repo, key)| *repo == entry.repo && *key == entry.key);
        if selected {
            list = list.child(detail(state, entry, root));
        }
    }
    if state.loaded && state.entries.is_empty() {
        list = list.child(
            div()
                .col()
                .gap(4.0)
                .p(16.0)
                .child(label("The store is empty").color(colors.text))
                .child(
                    label("A workspace's node_modules is kept here after its first install, for the next workspace with the same lockfile.")
                        .label_size(LabelSize::Small)
                        .color(colors.text_muted),
                ),
        );
    }
    let content_w = (width - PAD_X * 2.0).clamp(0.0, CONTENT_MAX_W);
    div()
        .col()
        .items_center()
        .py(24.0)
        .child(
            div()
                .col()
                .w_px(content_w)
                .gap(16.0)
                .child(header(state))
                .child(list),
        )
        .into()
}

fn click(state: &mut StoreState, id: u64) {
    let idle = state.busy.is_none();
    match id {
        REFRESH if idle => state.requests.push(Request::Refresh),
        PRUNE if idle => state.requests.push(Request::Prune),
        CLEAR if idle => state.requests.push(Request::Clear),
        id if id >= DELETE_BASE => {
            let Some(entry) = state.entries.get((id - DELETE_BASE) as usize) else {
                return;
            };
            if idle {
                state.requests.push(Request::Delete {
                    repo: entry.repo.clone(),
                    key: entry.key.clone(),
                });
            }
        }
        id if id >= ROW_BASE => {
            let Some(entry) = state.entries.get((id - ROW_BASE) as usize) else {
                return;
            };
            let pick = (entry.repo.clone(), entry.key.clone());
            state.selected = if state.selected.as_ref() == Some(&pick) {
                None
            } else {
                Some(pick)
            };
        }
        _ => return,
    }
    state.changed();
}

pub struct StorePage {
    shared: Shared,
    root: String,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    scroll: f32,
    content_h: f32,
    body_h: f32,
    seen: u64,
}

impl StorePage {
    pub fn new(shared: Shared, root: String) -> StorePage {
        StorePage {
            shared,
            root,
            hits: Vec::new(),
            hovered: None,
            scroll: 0.0,
            content_h: 0.0,
            body_h: 0.0,
            seen: u64::MAX,
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id)
    }

    pub fn click(&mut self, id: u64) {
        click(&mut self.shared.borrow_mut(), id);
    }
}

impl Item for StorePage {
    fn id(&self) -> Option<String> {
        Some(TAB_ID.to_string())
    }

    fn title(&self) -> String {
        "node_modules Store".into()
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Package)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let tree = render(
            &self.shared.borrow(),
            &self.root,
            body.w / scale,
            self.hovered,
        );
        self.content_h = ui::measure(&tree).1 * scale;
        self.body_h = body.h;
        self.scroll = self
            .scroll
            .clamp(0.0, (self.content_h - self.body_h).max(0.0));
        let shifted = Rect::new(
            body.x,
            body.y - self.scroll,
            body.w,
            body.h.max(self.content_h),
            Rgba::TRANSPARENT,
        );
        let painted = ui::render(&tree, shifted);
        self.hits = painted
            .hits
            .iter()
            .copied()
            .filter(|(rect, _)| rect.y + rect.h > body.y && rect.y < body.y + body.h)
            .collect();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        if let Some(id) = self.hit(x, y) {
            self.click(id);
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hovered = self.hit(x, y);
        let moved = hovered != self.hovered;
        self.hovered = hovered;
        moved
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let next = (self.scroll - delta_y).clamp(0.0, (self.content_h - self.body_h).max(0.0));
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let version = self.shared.borrow().version;
        let changed = version != self.seen;
        self.seen = version;
        ItemTick {
            changed,
            ..ItemTick::default()
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// A tab over made-up copies, for headless snapshots.
pub fn preview_page() -> StorePage {
    let now = 1_800_000_000;
    let entry = |repo: &str, manager: &str, gb: f64, days: u64, workspaces: usize| Entry {
        repo: repo.into(),
        key: format!("{:016x}", repo.len() as u64 * 7919 + days),
        manager: manager.into(),
        method: module_store::Method::Clone,
        size: (gb * (1u64 << 30) as f64) as u64,
        created: now - (days + 3) * 86_400,
        last_used: now - days * 86_400,
        workspaces: (0..workspaces)
            .map(|index| std::env::temp_dir().join(format!("preview-ws-{index}")))
            .collect(),
    };
    let entries = vec![
        entry("web", "yarn", 4.2, 0, 0),
        entry("api", "npm", 3.3, 2, 0),
        entry("admin", "yarn", 2.8, 6, 0),
        entry("docs", "bun", 0.6, 12, 0),
    ];
    let state = StoreState {
        method: "Copy-on-write clone".into(),
        selected: Some((entries[1].repo.clone(), entries[1].key.clone())),
        entries,
        loaded: true,
        now,
        message: Some("Freed 1.2 GB".into()),
        ..StoreState::default()
    };
    StorePage::new(
        Rc::new(RefCell::new(state)),
        "~/.local/state/pom/nm-store".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(state: &StoreState) -> Vec<String> {
        let painted = ui::render(
            &render(state, "/store", 1000.0, None),
            Rect::new(0.0, 0.0, 1000.0, 2000.0, Rgba::TRANSPARENT),
        );
        painted.texts.iter().map(|text| text.text.clone()).collect()
    }

    #[test]
    fn actions_are_handed_to_the_app_and_wait_while_busy() {
        let page = preview_page();
        let mut state = page.shared.borrow_mut();
        click(&mut state, DELETE_BASE + 1);
        click(&mut state, PRUNE);
        let api_key = state.entries[1].key.clone();
        assert_eq!(
            state.requests,
            [
                Request::Delete {
                    repo: "api".into(),
                    key: api_key
                },
                Request::Prune
            ]
        );
        state.requests.clear();
        state.busy = Some("Removing copies...".into());
        click(&mut state, CLEAR);
        click(&mut state, DELETE_BASE);
        assert!(state.requests.is_empty());
        assert!(texts(&state)
            .iter()
            .any(|text| text == "Removing copies..."));
    }

    #[test]
    fn a_row_expands_to_its_details_and_totals_are_shown() {
        let page = preview_page();
        let mut state = page.shared.borrow_mut();
        let shown = texts(&state);
        assert!(shown
            .iter()
            .any(|text| text.starts_with("4 copies - 10.9 GB")));
        assert!(shown.iter().any(|text| text.contains("/api/")));
        click(&mut state, ROW_BASE + 1);
        assert_eq!(state.selected, None);
        click(&mut state, ROW_BASE);
        assert_eq!(
            state.selected.as_ref().map(|(repo, _)| repo.as_str()),
            Some("web")
        );
        state.entries.clear();
        assert!(texts(&state)
            .iter()
            .any(|text| text == "The store is empty"));
    }

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(1000, 990), "just now");
        assert_eq!(ago(10_000, 10_000 - 300), "5 min ago");
        assert_eq!(ago(100_000, 100_000 - 7200), "2 h ago");
        assert_eq!(ago(300_000, 300_000 - 100_000), "yesterday");
        assert_eq!(ago(1_000_000, 1_000_000 - 5 * 86_400), "5 days ago");
    }
}
