use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use module_store::{format_size, Holding, Overview, RepoState, User, Version};
use terminal::Modifiers;
use ui::{div, label, theme, ButtonStyle, IconKind, LabelSize, Node, Rect, Rgba};
use workspace::{Item, ItemTick};

pub const TAB_ID: &str = "module-store";

const REFRESH: u64 = 1;
const FREE_UNUSED: u64 = 2;
const ACTION_BASE: u64 = 1_000;
const MORE_BASE: u64 = 100_000;
const CONTENT_MAX_W: f32 = 1040.0;
const PAD_X: f32 = 32.0;
const LABEL_W: f32 = 250.0;
const SIZE_W: f32 = 76.0;
const ACTION_W: f32 = 150.0;
const CHIP_GAP: f32 = 4.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Refresh,
    /// Every copy no worktree of this project uses.
    FreeUnused,
    FreeOld {
        repo: String,
    },
    FreeOthers,
    /// Swap a worktree's own install for the stored copy.
    Relink {
        repo: String,
        workspace: String,
        path: PathBuf,
    },
    /// Keep a worktree's own install as the stored copy.
    Keep {
        repo: String,
        path: PathBuf,
    },
}

#[derive(Default)]
pub struct StoreState {
    /// How copies reach project folders here, e.g. "copy-on-write, workspaces take no extra disk".
    pub method: String,
    /// `None` while the first measurement runs.
    pub overview: Option<Overview>,
    pub busy: Option<String>,
    pub message: Option<String>,
    /// Seconds since the epoch, for "changed 2 days ago".
    pub now: u64,
    /// Versions whose workspace list is shown in full, by key.
    pub expanded: Vec<String>,
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

fn own_users(version: &Version) -> impl Iterator<Item = &User> {
    version
        .users
        .iter()
        .filter(|user| matches!(user.holding, Holding::Own { .. }))
}

/// A version with no stored copy but an install to keep: main's first, else any workspace's.
fn keep_request(repo: &str, version: &Version) -> Option<Request> {
    if version.stored.is_some() {
        return None;
    }
    let installed = |user: &&User| !matches!(user.holding, Holding::NotInstalled);
    let source = version
        .users
        .iter()
        .filter(installed)
        .find(|user| user.is_main)
        .or_else(|| version.users.iter().find(installed))?;
    Some(Request::Keep {
        repo: repo.to_string(),
        path: source.path.clone(),
    })
}

/// Every button on the page in render order, so a click id maps back to what it asks for.
fn actions(state: &StoreState) -> Vec<Request> {
    let mut out = Vec::new();
    let Some(overview) = &state.overview else {
        return out;
    };
    for repo in &overview.repos {
        let RepoState::Versions { versions, old, .. } = &repo.state else {
            continue;
        };
        for version in versions {
            if let Some(request) = keep_request(&repo.repo, version) {
                out.push(request);
            }
            if version.stored.is_some() {
                for user in own_users(version) {
                    out.push(Request::Relink {
                        repo: repo.repo.clone(),
                        workspace: user.workspace.clone(),
                        path: user.path.clone(),
                    });
                }
            }
        }
        if !old.is_empty() {
            out.push(Request::FreeOld {
                repo: repo.repo.clone(),
            });
        }
    }
    if !overview.others.is_empty() {
        out.push(Request::FreeOthers);
    }
    out
}

/// The version a "+N more" chip belongs to, walking versions in render order (each version's own-copy
/// rows take a chip id too).
fn more_key(state: &StoreState, index: usize) -> Option<String> {
    let overview = state.overview.as_ref()?;
    let mut next = 0;
    for repo in &overview.repos {
        let RepoState::Versions { versions, .. } = &repo.state else {
            continue;
        };
        for version in versions {
            if next == index {
                return Some(version.key.clone());
            }
            next += 1;
            if version.stored.is_some() {
                next += own_users(version).count();
            }
        }
    }
    None
}

fn small(text: impl Into<String>, color: Rgba) -> ui::Label {
    label(text.into()).label_size(LabelSize::Small).color(color)
}

fn section_label(text: &str) -> ui::Label {
    label(text.to_uppercase())
        .size(11.0)
        .weight(600)
        .color(theme().text_placeholder)
}

fn pill(text: &str, color: Rgba) -> Node {
    div()
        .row()
        .px(9.0)
        .py(2.0)
        .rounded(10.0)
        .border(1.0, color.alpha(0.45))
        .child(small(text, color))
        .into()
}

fn chip(text: &str) -> Node {
    div()
        .row()
        .px(7.0)
        .py(1.0)
        .rounded(5.0)
        .border(1.0, theme().border_variant)
        .child(label(text.to_string()).size(11.5).color(theme().text))
        .into()
}

/// As many chips as fit in `width`, then "+N more" (or all of them, wrapped, with "Show less").
fn chips(names: &[String], width: f32, expanded: bool, more_id: u64, hovered: Option<u64>) -> Node {
    let colors = theme();
    if names.is_empty() {
        return small("no workspace", colors.text_placeholder).into();
    }
    let toggle = |text: String| {
        let mut part = div()
            .row()
            .px(7.0)
            .py(1.0)
            .rounded(5.0)
            .on_click(more_id)
            .child(label(text).size(11.5).color(colors.text_accent));
        if hovered == Some(more_id) {
            part = part.bg(colors.ghost_element_hover);
        }
        part
    };
    if expanded {
        let mut wrap = div().col().gap(CHIP_GAP);
        let mut line = div().row().gap(CHIP_GAP);
        let mut used = 0.0;
        for name in names {
            let node = chip(name);
            let w = ui::measure(&node).0;
            if used > 0.0 && used + w > width {
                wrap = wrap.child(line);
                line = div().row().gap(CHIP_GAP);
                used = 0.0;
            }
            used += w + CHIP_GAP;
            line = line.child(node);
        }
        return wrap.child(line.child(toggle("Show less".into()))).into();
    }
    let reserve = ui::measure(&toggle(format!("+{} more", names.len())).into()).0 + CHIP_GAP;
    let mut row = div().row().items_center().gap(CHIP_GAP);
    let mut used = 0.0;
    let mut shown = 0;
    for (index, name) in names.iter().enumerate() {
        let node = chip(name);
        let w = ui::measure(&node).0 + CHIP_GAP;
        let room = if index + 1 == names.len() {
            width
        } else {
            width - reserve
        };
        if used + w > room {
            break;
        }
        used += w;
        shown += 1;
        row = row.child(node);
    }
    if shown < names.len() {
        row = row.child(toggle(format!("+{} more", names.len() - shown)));
    }
    row.into()
}

struct Line {
    title: String,
    detail: String,
    size: String,
    users: Node,
    trailing: Option<Node>,
    faded: bool,
}

fn line(line: Line) -> Node {
    let colors = theme();
    let (title, detail) = if line.faded {
        (colors.text_muted, colors.text_placeholder)
    } else {
        (colors.text, colors.text_muted)
    };
    let mut right = div().row().w_px(ACTION_W).justify_end();
    if let Some(trailing) = line.trailing {
        right = right.child(trailing);
    }
    div()
        .row()
        .items_center()
        .gap(10.0)
        .px(12.0)
        .py(7.0)
        .child(
            div()
                .col()
                .w_px(LABEL_W)
                .gap(1.0)
                .child(
                    div()
                        .row()
                        .child(label(line.title).size(13.0).color(title).truncate()),
                )
                .child(div().row().child(small(line.detail, detail).truncate())),
        )
        .child(
            div()
                .row()
                .w_px(SIZE_W)
                .justify_end()
                .child(small(line.size, title)),
        )
        .child(div().row().flex(1.0).child(line.users))
        .child(right)
        .into()
}

fn action_button(id: u64, text: &str, idle: bool, hovered: Option<u64>) -> Node {
    if !idle {
        return ui::button_static(text, ButtonStyle::Outlined).into();
    }
    let mut button = ui::button(id, text, ButtonStyle::Outlined);
    if hovered == Some(id) {
        button = button.bg(theme().ghost_element_hover);
    }
    button.into()
}

#[derive(Default)]
struct Ids {
    action: u64,
    more: u64,
}

impl Ids {
    fn action(&mut self) -> u64 {
        self.action += 1;
        ACTION_BASE + self.action - 1
    }

    fn more(&mut self) -> u64 {
        self.more += 1;
        MORE_BASE + self.more - 1
    }
}

fn version_rows(
    state: &StoreState,
    repo: &str,
    version: &Version,
    users_w: f32,
    ids: &mut Ids,
    hovered: Option<u64>,
) -> Vec<Node> {
    let colors = theme();
    let idle = state.busy.is_none();
    let first_branch = version
        .users
        .iter()
        .find(|user| !user.is_main)
        .map_or("a branch", |user| user.workspace.as_str());
    let title = if version.on_main {
        "Current on main".to_string()
    } else {
        format!("Changed on {first_branch}")
    };
    let keep = keep_request(repo, version);
    let detail = match (&version.stored, &keep) {
        (Some(_), _) => version
            .changed
            .map(|at| format!("{} changed {}", version.lockfile, ago(state.now, at)))
            .unwrap_or_else(|| version.lockfile.clone()),
        (None, Some(_)) => "no saved copy yet; its own install can be kept".to_string(),
        (None, None) => "no saved copy yet; the next install is kept".to_string(),
    };
    let listed: Vec<String> = version
        .users
        .iter()
        .filter(|user| version.stored.is_none() || !matches!(user.holding, Holding::Own { .. }))
        .map(|user| user.workspace.clone())
        .collect();
    let expanded = state.expanded.contains(&version.key);
    let users = chips(&listed, users_w, expanded, ids.more(), hovered);
    let trailing = if keep.is_some() {
        Some(action_button(ids.action(), "Save to Store", idle, hovered))
    } else if version.stored.is_some() && version.on_main {
        Some(pill("New workspaces", colors.success))
    } else if version.stored.is_none() {
        Some(pill("Installs once", colors.text_muted))
    } else {
        None
    };
    let mut rows = vec![line(Line {
        title,
        detail,
        size: version
            .stored
            .as_ref()
            .map_or("-".to_string(), |entry| format_size(entry.size)),
        users,
        trailing,
        faded: false,
    })];
    if version.stored.is_some() {
        let same_as = if version.on_main {
            "main"
        } else {
            first_branch
        };
        for user in own_users(version) {
            let Holding::Own { size } = user.holding else {
                continue;
            };
            rows.push(line(Line {
                title: format!("{} has its own copy", user.workspace),
                detail: format!(
                    "installed on its own; same {} as {same_as}",
                    version.lockfile
                ),
                size: format_size(size),
                users: chips(
                    std::slice::from_ref(&user.workspace),
                    users_w,
                    false,
                    ids.more(),
                    hovered,
                ),
                trailing: Some(action_button(
                    ids.action(),
                    "Use Shared Copy",
                    idle,
                    hovered,
                )),
                faded: false,
            }));
        }
    }
    rows
}

fn group(head: Node, rows: Vec<Node>) -> Node {
    let colors = theme();
    let mut column = div()
        .col()
        .rounded(8.0)
        .border(1.0, colors.border_variant)
        .child(
            div()
                .row()
                .items_center()
                .gap(8.0)
                .px(14.0)
                .h_px(36.0)
                .bg(colors.panel_background)
                .child(head),
        );
    for row in rows {
        column = column
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(row);
    }
    column.into()
}

fn group_head(name: &str, manager: &str, total: Option<u64>) -> Node {
    let colors = theme();
    let mut head = div().row().items_center().gap(8.0).child(
        label(name.to_string())
            .size(13.0)
            .weight(600)
            .color(colors.text),
    );
    if !manager.is_empty() {
        head = head.child(small(manager, colors.text_muted));
    }
    if let Some(total) = total.filter(|total| *total > 0) {
        head = head.child(small(
            format!("- {}", format_size(total)),
            colors.text_muted,
        ));
    }
    head.into()
}

fn note_row(text: String) -> Node {
    div()
        .row()
        .px(12.0)
        .h_px(34.0)
        .items_center()
        .child(small(text, theme().text_muted))
        .into()
}

fn unused_row(
    title: String,
    detail: &str,
    size: u64,
    id: u64,
    idle: bool,
    hovered: Option<u64>,
) -> Node {
    line(Line {
        title,
        detail: detail.to_string(),
        size: format_size(size),
        users: small("no workspace", theme().text_placeholder).into(),
        trailing: Some(action_button(id, "Free", idle, hovered)),
        faded: true,
    })
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// `root` is the store folder, named in the footnote.
pub fn render(state: &StoreState, root: &str, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let content_w = (width - PAD_X * 2.0).clamp(0.0, CONTENT_MAX_W);
    let users_w = (content_w - LABEL_W - SIZE_W - ACTION_W - 3.0 * 10.0 - 24.0).max(80.0);
    let idle = state.busy.is_none();
    let total = state.overview.as_ref().map_or(0, Overview::stored_total);
    let unused: u64 = state.overview.as_ref().map_or(0, |overview| {
        overview.unused().iter().map(|entry| entry.size).sum()
    });
    let summary = if state.overview.is_some() {
        format!("{} saved - {}", format_size(total), state.method)
    } else {
        state.method.clone()
    };
    let mut head = div()
        .row()
        .items_center()
        .gap(10.0)
        .child(
            label("node_modules Store")
                .label_size(LabelSize::Large)
                .color(colors.text),
        )
        .child(small(summary, colors.text_muted))
        .child(div().flex(1.0))
        .child(action_button(REFRESH, "Refresh", idle, hovered));
    if unused > 0 {
        head = head.child(action_button(
            FREE_UNUSED,
            &format!("Free {} Unused", format_size(unused)),
            idle,
            hovered,
        ));
    }
    let mut column = div().col().w_px(content_w).gap(14.0).child(head);
    if let Some(note) = state.busy.as_ref().or(state.message.as_ref()) {
        let color = if state.busy.is_some() {
            colors.text_accent
        } else {
            colors.text_muted
        };
        column = column.child(small(note.clone(), color));
    }
    let Some(overview) = &state.overview else {
        return frame(column.child(small("Looking at every workspace...", colors.text_muted)));
    };
    column = column.child(
        div()
            .row()
            .gap(10.0)
            .px(12.0)
            .child(
                div()
                    .row()
                    .w_px(LABEL_W)
                    .child(section_label("Lockfile version")),
            )
            .child(
                div()
                    .row()
                    .w_px(SIZE_W)
                    .justify_end()
                    .child(section_label("Size")),
            )
            .child(div().row().flex(1.0).child(section_label("Used by"))),
    );
    let mut ids = Ids::default();
    for repo in &overview.repos {
        let node = match &repo.state {
            RepoState::SelfManaged(name) => group(
                group_head(&repo.repo, name, None),
                vec![note_row(format!(
                    "{name} shares packages itself; Pomelo leaves it alone."
                ))],
            ),
            RepoState::NoLockfile => group(
                group_head(&repo.repo, "", None),
                vec![note_row(
                    "No package-lock.json, yarn.lock or bun.lock in its worktrees.".into(),
                )],
            ),
            RepoState::Versions {
                manager,
                versions,
                old,
            } => {
                let stored: u64 = versions
                    .iter()
                    .filter_map(|version| version.stored.as_ref())
                    .chain(old.iter())
                    .map(|entry| entry.size)
                    .sum();
                let mut rows = Vec::new();
                for version in versions {
                    rows.extend(version_rows(
                        state, &repo.repo, version, users_w, &mut ids, hovered,
                    ));
                }
                if !old.is_empty() {
                    rows.push(unused_row(
                        plural(old.len(), "old version", "old versions"),
                        "no workspace uses these lockfiles any more",
                        old.iter().map(|entry| entry.size).sum(),
                        ids.action(),
                        idle,
                        hovered,
                    ));
                }
                group(group_head(&repo.repo, manager.label(), Some(stored)), rows)
            }
        };
        column = column.child(node);
    }
    if !overview.others.is_empty() {
        let size = overview.others.iter().map(|entry| entry.size).sum();
        column = column.child(group(
            group_head("Other projects", "", Some(size)),
            vec![unused_row(
                plural(overview.others.len(), "copy", "copies"),
                "repos this project does not have",
                size,
                ids.action(),
                idle,
                hovered,
            )],
        ));
    }
    for note in [
        "One copy per lockfile, patches/ folder, Node major version and platform.".to_string(),
        "A new workspace with a matching one gets it in seconds; otherwise it installs once and that install is kept.".to_string(),
        format!("Stored in {root}"),
    ] {
        column = column.child(
            div()
                .row()
                .child(small(note, colors.text_placeholder).truncate()),
        );
    }
    frame(column)
}

fn frame(column: ui::Div) -> Node {
    div().col().items_center().py(24.0).child(column).into()
}

fn click(state: &mut StoreState, id: u64) {
    let idle = state.busy.is_none();
    match id {
        REFRESH if idle => state.requests.push(Request::Refresh),
        FREE_UNUSED if idle => state.requests.push(Request::FreeUnused),
        id if id >= MORE_BASE => {
            let Some(key) = more_key(state, (id - MORE_BASE) as usize) else {
                return;
            };
            match state.expanded.iter().position(|open| *open == key) {
                Some(index) => {
                    state.expanded.remove(index);
                }
                None => state.expanded.push(key),
            }
        }
        id if id >= ACTION_BASE && idle => {
            let Some(request) = actions(state).into_iter().nth((id - ACTION_BASE) as usize) else {
                return;
            };
            state.requests.push(request);
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

/// A tab over a made-up project, for headless snapshots.
pub fn preview_page() -> StorePage {
    use module_store::{Entry, Manager, Method, RepoOverview};
    let now = 1_800_000_000;
    let day = 86_400;
    let entry = |repo: &str, key: &str, gb: f64, days: u64| Entry {
        repo: repo.into(),
        key: key.into(),
        manager: "npm".into(),
        method: Method::Clone,
        size: (gb * (1u64 << 30) as f64) as u64,
        created: now - (days + 3) * day,
        last_used: now - days * day,
        workspaces: Vec::new(),
    };
    let user = |name: &str, holding: Holding| User {
        workspace: name.into(),
        is_main: name == "main",
        path: PathBuf::from(format!("/work/{name}/api")),
        holding,
    };
    let version = |key: &str,
                   lockfile: &str,
                   days: u64,
                   on_main: bool,
                   stored: Option<Entry>,
                   users: Vec<User>| Version {
        key: key.into(),
        lockfile: lockfile.into(),
        changed: Some(now - days * day),
        on_main,
        stored,
        users,
    };
    let many = [
        "main",
        "feat-login",
        "feat-pay",
        "PROJ-101",
        "PROJ-102",
        "PROJ-104",
        "PROJ-107",
        "PROJ-110",
        "PROJ-111",
        "fix-cart",
        "fix-tax",
        "spike-cache",
    ];
    let overview = Overview {
        repos: vec![
            RepoOverview {
                repo: "api".into(),
                state: RepoState::Versions {
                    manager: Manager::Npm,
                    versions: vec![
                        version(
                            "k1",
                            "package-lock.json",
                            12,
                            true,
                            Some(entry("api", "k1", 1.4, 0)),
                            many.iter()
                                .map(|name| user(name, Holding::Shared))
                                .collect(),
                        ),
                        version(
                            "k2",
                            "package-lock.json",
                            3,
                            false,
                            Some(entry("api", "k2", 1.5, 1)),
                            vec![user("feat-react", Holding::Shared)],
                        ),
                    ],
                    old: vec![entry("api", "o1", 0.9, 20), entry("api", "o2", 1.2, 30)],
                },
            },
            RepoOverview {
                repo: "web".into(),
                state: RepoState::Versions {
                    manager: Manager::Yarn,
                    versions: vec![version(
                        "w1",
                        "yarn.lock",
                        2,
                        true,
                        Some(entry("web", "w1", 1.2, 0)),
                        vec![
                            user("main", Holding::Shared),
                            user("feat-login", Holding::Shared),
                            user(
                                "feat-pay",
                                Holding::Own {
                                    size: 1_288_490_188,
                                },
                            ),
                        ],
                    )],
                    old: Vec::new(),
                },
            },
            RepoOverview {
                repo: "admin".into(),
                state: RepoState::Versions {
                    manager: Manager::Yarn,
                    versions: vec![version(
                        "a1",
                        "yarn.lock",
                        5,
                        true,
                        None,
                        vec![user(
                            "main",
                            Holding::Own {
                                size: 1_181_116_006,
                            },
                        )],
                    )],
                    old: vec![entry("admin", "o3", 3.4, 27)],
                },
            },
            RepoOverview {
                repo: "docs".into(),
                state: RepoState::SelfManaged("pnpm"),
            },
        ],
        others: vec![entry("billing", "x1", 0.5, 40)],
    };
    let state = StoreState {
        method: "copy-on-write, workspaces take no extra disk".into(),
        overview: Some(overview),
        now,
        ..StoreState::default()
    };
    StorePage::new(
        Rc::new(RefCell::new(state)),
        "~/.local/state/pom/nm-store".into(),
    )
}

#[cfg(test)]
mod tests;
