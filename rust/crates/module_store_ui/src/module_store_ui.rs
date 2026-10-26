mod access;
mod remote;

use components::{pill, text_line, ButtonKind};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

pub use access::{LocalStore, StoreAccess, Updates};
use module_store::{format_size, Holding, Overview, RepoState, User, Version};
pub use module_store::{plan, Request, StoreUpdate};
pub use remote::{refusal, RemoteStore};
use terminal::Modifiers;
use ui::{div, label, theme, IconKind, LabelSize, Node, Radius, Rect, Rgba, Space};
use workspace::{Item, ItemTick};

pub const TAB_ID: &str = "module-store";

const REFRESH: u64 = 1;
const FREE_UNUSED: u64 = 2;
const CONFIRM_SWAP: u64 = 3;
const CANCEL_SWAP: u64 = 4;
const OPTIMIZE: u64 = 5;
const CONFIRM_OPTIMIZE: u64 = 6;
const CANCEL_OPTIMIZE: u64 = 7;
const ACTION_BASE: u64 = 1_000;
const LINE_BASE: u64 = 50_000;
const MORE_BASE: u64 = 100_000;
const CONTENT_MAX_W: f32 = 1004.0;
const PAD_X: f32 = 28.0;
const ROW_PAD_X: Space = Space::S14;
const LABEL_W: f32 = 280.0;
const SIZE_W: f32 = 76.0;
const ACTION_W: f32 = 140.0;
const CHIP_GAP: Space = Space::S4;
const TOP_BAR_H: f32 = 2.0;
/// Seconds a finished action's note and highlight stay.
const NOTE_FOR: f32 = 3.0;
const FLASH_FOR: f32 = 1.4;
/// Pulses and shimmers lay the page out again at this rate; the loading bar moves every frame.
const ANIMATION_STEP: f32 = 1.0 / 15.0;

thread_local! {
    /// Laid-out widths of chips and "+N more" labels, by font generation and text.
    static WIDTHS: RefCell<std::collections::HashMap<(u64, String), f32>> =
        RefCell::new(std::collections::HashMap::new());
}

/// The width `node` lays out to, measured once per text and font.
fn width_of(text: &str, node: impl FnOnce() -> Node) -> f32 {
    let key = (ui::font_generation(), text.to_string());
    if let Some(width) = WIDTHS.with(|widths| widths.borrow().get(&key).copied()) {
        return width;
    }
    let width = ui::measure(&node()).0;
    WIDTHS.with(|widths| widths.borrow_mut().insert(key, width));
    width
}

#[derive(Default)]
pub struct StoreState {
    /// How copies reach project folders here, e.g. "copy-on-write, workspaces take no extra disk".
    pub method: String,
    /// `None` until the lockfiles are read.
    pub overview: Option<Overview>,
    /// Sizes still being measured, and how many there were.
    pub measuring: usize,
    pub measure_total: usize,
    /// The request running now.
    pub busy: Option<Request>,
    /// The last result, whether it failed, and when it came (in `clock` seconds).
    pub note: Option<(String, bool, f32)>,
    /// A row to highlight briefly, by version key or workspace, and since when.
    pub flash: Option<(String, f32)>,
    /// A swap waiting for confirmation, shown under its row.
    pub confirm: Option<Request>,
    /// Optimize's plan is shown, waiting for confirmation.
    pub confirm_optimize: bool,
    /// Optimize's step running now and how many there are.
    pub progress: Option<(usize, usize)>,
    /// Workspaces whose services run, as (repo, workspace).
    pub running: Vec<(String, String)>,
    /// Seconds since the epoch, for "changed 2 days ago".
    pub now: u64,
    /// Seconds since the tab opened, for animations.
    pub clock: f32,
    /// Versions whose workspace list is shown in full, by key.
    pub expanded: Vec<String>,
    pub requests: Vec<Request>,
    pub version: u64,
    /// Why this window cannot show a store, in place of the overview.
    pub unavailable: Option<String>,
    /// The store's folder as its source names it, when it is not this machine's.
    pub root: Option<String>,
}

pub type Shared = Rc<RefCell<StoreState>>;

impl StoreState {
    pub fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Something on the page moves: loading, work in flight, or a fading note.
    pub fn animating(&self) -> bool {
        self.loading()
            || self.measuring > 0
            || self.busy.is_some()
            || self
                .note
                .as_ref()
                .is_some_and(|(_, _, at)| self.clock - at < NOTE_FOR)
            || self
                .flash
                .as_ref()
                .is_some_and(|(_, at)| self.clock - at < FLASH_FOR)
    }

    /// Nothing to show yet, and nothing said why.
    pub fn loading(&self) -> bool {
        self.overview.is_none() && self.unavailable.is_none()
    }

    /// Takes in what the running request reports.
    pub fn apply(&mut self, update: StoreUpdate, now: u64) {
        match update {
            StoreUpdate::Place { root, method } => {
                self.root = Some(root);
                self.method = method;
            }
            StoreUpdate::Overview {
                overview,
                running,
                pending,
                message,
                failed,
                flash,
            } => {
                self.overview = Some(overview);
                self.unavailable = None;
                self.running = running;
                self.measuring = pending;
                self.measure_total = pending;
                self.progress = None;
                self.now = now;
                self.finish(message, failed, flash);
            }
            StoreUpdate::Step {
                request,
                index,
                total,
            } => {
                self.busy = Some(request);
                self.progress = Some((index, total));
            }
            StoreUpdate::Measured { what, size } => {
                if let Some(overview) = self.overview.as_mut() {
                    overview.apply(&what, size);
                }
                self.measuring = self.measuring.saturating_sub(1);
            }
            StoreUpdate::Failed { message } => {
                self.measuring = 0;
                self.progress = None;
                if self.overview.is_some() {
                    self.finish(Some(message), true, None);
                } else {
                    self.unavailable = Some(message);
                    self.finish(None, false, None);
                }
            }
        }
        self.changed();
    }

    /// The request's work ended, whatever it last said.
    pub fn ended(&mut self) {
        self.measuring = 0;
        self.busy = None;
        self.progress = None;
        self.changed();
    }

    /// A finished request: its result note and, when it changed a row, that row's highlight.
    pub fn finish(&mut self, message: Option<String>, failed: bool, flash: Option<String>) {
        self.busy = None;
        self.note = message.map(|text| (text, failed, self.clock));
        self.flash = flash.map(|key| (key, self.clock));
        self.changed();
    }
}

fn hit_in(hits: &[(Rect, u64)], x: f32, y: f32) -> Option<u64> {
    hits.iter()
        .rev()
        .find(|(rect, _)| x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h)
        .map(|(_, id)| *id)
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
            if let Some(request) = version.keep_request(&repo.repo) {
                out.push(request);
            }
            if version.stored.is_some() {
                for user in version.own_users() {
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

/// Bytes Optimize frees at most: the swapped installs and the unused copies.
fn plan_frees(overview: &Overview, steps: &[Request]) -> u64 {
    let mut total: u64 = 0;
    for step in steps {
        match step {
            Request::FreeUnused => {
                total += overview
                    .unused()
                    .iter()
                    .map(|entry| entry.size)
                    .sum::<u64>();
            }
            Request::Relink { path, .. } => {
                for repo in &overview.repos {
                    if let RepoState::Versions { versions, .. } = &repo.state {
                        for user in versions.iter().flat_map(|version| &version.users) {
                            if user.path == *path {
                                if let Holding::Own { size: Some(size) } = user.holding {
                                    total += size;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    total
}

const SWAP_POPOVER_W: f32 = 340.0;
const OPTIMIZE_POPOVER_W: f32 = 404.0;

/// The floating card a confirmation sits in, styled like the app's other popovers.
fn popover_frame(width: f32) -> ui::Div {
    components::card()
        .padding(12.0)
        .raised()
        .render()
        .w_px(width)
        .gap(Space::S8)
}

fn confirm_buttons(cancel: u64, confirm: u64, action: &str, hovered: Option<u64>) -> Node {
    div()
        .row()
        .gap(Space::S6)
        .pt(Space::S4)
        .child(div().flex(1.0))
        .child(
            components::button(cancel, "Cancel")
                .keystroke(Some("escape"))
                .hovered(hovered),
        )
        .child(
            components::button(confirm, action)
                .kind(ButtonKind::Primary)
                .keystroke(Some("enter"))
                .hovered(hovered),
        )
        .into()
}

/// The open confirmation, as a card to float by its button.
fn popover(state: &StoreState, hovered: Option<u64>) -> Option<Node> {
    let overview = state.overview.as_ref()?;
    if state.confirm_optimize {
        return Some(optimize_strip(state, overview, hovered));
    }
    let request = state.confirm.as_ref()?;
    let Request::Relink {
        repo,
        workspace,
        path,
    } = request
    else {
        return None;
    };
    let size = overview.repos.iter().find_map(|found| match &found.state {
        RepoState::Versions { versions, .. } => versions
            .iter()
            .flat_map(|version| &version.users)
            .find(|user| user.path == *path)
            .and_then(|user| match user.holding {
                Holding::Own { size } => size,
                _ => None,
            }),
        _ => None,
    });
    let services = state
        .running
        .iter()
        .any(|(running_repo, running)| running_repo == repo && running == workspace);
    Some(confirm_strip(request, size, services, hovered))
}

fn optimize_strip(state: &StoreState, overview: &Overview, hovered: Option<u64>) -> Node {
    let colors = theme();
    let steps = plan(overview);
    let count = |kind: fn(&Request) -> bool| steps.iter().filter(|step| kind(step)).count();
    let keeps = count(|step| matches!(step, Request::Keep { .. }));
    let relinks = count(|step| matches!(step, Request::Relink { .. }));
    let mut parts = Vec::new();
    if keeps > 0 {
        parts.push(plural(
            keeps,
            "install kept as the shared copy",
            "installs kept as the shared copy",
        ));
    }
    if relinks > 0 {
        parts.push(plural(
            relinks,
            "workspace moved onto the shared copy",
            "workspaces moved onto the shared copy",
        ));
    }
    if steps.contains(&Request::FreeUnused) {
        parts.push(plural(
            overview.unused().len(),
            "unused copy removed",
            "unused copies removed",
        ));
    }
    let restarted: Vec<&str> = steps
        .iter()
        .filter_map(|step| match step {
            Request::Relink {
                repo, workspace, ..
            } if state
                .running
                .iter()
                .any(|(running_repo, running)| running_repo == repo && running == workspace) =>
            {
                Some(workspace.as_str())
            }
            _ => None,
        })
        .collect();
    let mut column = popover_frame(OPTIMIZE_POPOVER_W)
        .child(Node::from(
            components::paragraph(
                format!("Optimize: {}", parts.join(", ")),
                OPTIMIZE_POPOVER_W - 24.0,
            )
            .size(LabelSize::Medium, 20.0)
            .color(colors.text),
        ))
        .child(Node::from(
            components::paragraph(
                format!(
                    "Frees up to {}. Each workspace keeps a working node_modules the whole time.",
                    format_size(plan_frees(overview, &steps))
                ),
                OPTIMIZE_POPOVER_W - 24.0,
            )
            .size(LabelSize::Small, 16.0)
            .color(colors.text_muted),
        ));
    if !restarted.is_empty() {
        column = column.child(Node::from(
            components::paragraph(
                format!(
                    "Services are stopped and started again in {}.",
                    restarted.join(", ")
                ),
                OPTIMIZE_POPOVER_W - 24.0,
            )
            .size(LabelSize::Small, 16.0)
            .color(colors.warning),
        ));
    }
    column
        .child(confirm_buttons(
            CANCEL_OPTIMIZE,
            CONFIRM_OPTIMIZE,
            "Optimize",
            hovered,
        ))
        .into()
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
                next += version.own_users().count();
            }
        }
    }
    None
}

fn small(text: impl Into<String>, color: Rgba) -> ui::Label {
    label(text.into()).size(LabelSize::Small).color(color)
}

/// 0..1, repeating every `period` seconds.
fn wave(clock: f32, period: f32) -> f32 {
    if ui::reduce_motion() {
        return 1.0;
    }
    ((clock / period).fract() * std::f32::consts::TAU).sin() * 0.5 + 0.5
}

/// A pulsing dot standing in for a spinner.
fn pulse(clock: f32) -> Node {
    let mut color = theme().text_accent;
    color.a *= 0.35 + 0.65 * wave(clock, 0.9);
    components::dot(color, 7.0).into()
}

fn chip(text: &str, highlight: bool) -> Node {
    let colors = theme();
    let chip = components::chip(text).color(if text == "main" {
        colors.text
    } else {
        colors.text_muted
    });
    if highlight {
        chip.fill(colors.info_background)
            .border(colors.text_accent)
            .into()
    } else {
        chip.into()
    }
}

/// As many chips as fit in `width`, then "+N more" (or all of them, wrapped, with "Show less").
fn chips(
    names: &[String],
    width: f32,
    expanded: bool,
    more_id: u64,
    hovered: Option<u64>,
    highlight: Option<&str>,
) -> Node {
    let colors = theme();
    if names.is_empty() {
        return small("no workspace", colors.text_placeholder).into();
    }
    let toggle = |text: String| {
        let mut part = div()
            .row()
            .px(Space::S8)
            .py(Space::Px(1.0))
            .rounded(Radius::Medium)
            .on_click(more_id)
            .child(label(text).size(LabelSize::Small).color(colors.text_accent));
        if hovered == Some(more_id) {
            part = part.bg(colors.ghost_element_hover);
        }
        part
    };
    let node = |name: &String| chip(name, highlight == Some(name.as_str()));
    if expanded {
        let mut wrap = div().col().gap(CHIP_GAP);
        let mut line = div().row().gap(CHIP_GAP);
        let mut used = 0.0;
        for name in names {
            let part = node(name);
            let w = width_of(name, || chip(name, false));
            if used > 0.0 && used + w > width {
                wrap = wrap.child(line);
                line = div().row().gap(CHIP_GAP);
                used = 0.0;
            }
            used += w + CHIP_GAP.px();
            line = line.child(part);
        }
        return wrap.child(line.child(toggle("Show less".into()))).into();
    }
    let more_text = format!("+{} more", names.len());
    let reserve = width_of(&more_text, || toggle(more_text.clone()).into()) + CHIP_GAP.px();
    let mut row = div().row().items_center().gap(CHIP_GAP);
    let mut used = 0.0;
    let mut shown = 0;
    for (index, name) in names.iter().enumerate() {
        let w = width_of(name, || chip(name, false)) + CHIP_GAP.px();
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
        row = row.child(node(name));
    }
    if shown < names.len() {
        row = row.child(toggle(format!("+{} more", names.len() - shown)));
    }
    row.into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Primary,
    /// Plain text, only while the row is hovered.
    Ghost,
}

struct Button {
    id: u64,
    text: &'static str,
    working: &'static str,
    look: Look,
    /// This row's request is the one running.
    running: bool,
}

fn button(spec: Button, state: &StoreState, hovered: Option<u64>, row_hovered: bool) -> Node {
    let colors = theme();
    if spec.running {
        return div()
            .row()
            .items_center()
            .gap(Space::S6)
            .px(Space::S10)
            .h_px(24.0)
            .child(pulse(state.clock))
            .child(small(spec.working, colors.text_muted))
            .into();
    }
    if spec.look == Look::Ghost && !row_hovered && hovered != Some(spec.id) {
        return div().into();
    }
    let kind = match spec.look {
        Look::Primary => ButtonKind::Secondary,
        Look::Ghost => ButtonKind::Ghost,
    };
    components::button(spec.id, spec.text)
        .kind(kind)
        .enabled(state.busy.is_none())
        .hovered(hovered)
        .into()
}

struct Line {
    id: u64,
    title: String,
    detail: String,
    size: Node,
    users: Node,
    trailing: Node,
    faded: bool,
    flash: f32,
    progress: bool,
}

fn line(line: Line, state: &StoreState, hovered: Option<u64>) -> Node {
    let colors = theme();
    let title = if line.faded {
        colors.text_muted
    } else {
        colors.text
    };
    let detail = colors.text_placeholder;
    let mut row = div()
        .row()
        .items_center()
        .gap(Space::S10)
        .px(ROW_PAD_X)
        .pt(Space::S10)
        .pb(Space::S10)
        .on_click(line.id)
        .child(
            div()
                .col()
                .w_px(LABEL_W)
                .gap(Space::S2)
                .child(text_line(
                    20.0,
                    label(line.title)
                        .size(LabelSize::Medium)
                        .color(title)
                        .truncate(),
                ))
                .child(text_line(16.0, small(line.detail, detail).truncate())),
        )
        .child(div().row().w_px(SIZE_W).justify_end().child(line.size))
        .child(div().row().flex(1.0).child(line.users))
        .child(
            div()
                .row()
                .w_px(ACTION_W)
                .justify_end()
                .child(line.trailing),
        );
    if line.flash > 0.0 {
        let mut tint = colors.success;
        tint.a = 0.12 * line.flash;
        row = row.bg(tint);
    } else if hovered == Some(line.id) {
        row = row.bg(colors.ghost_element_hover);
    }
    let mut column = div().col().child(row);
    if line.progress {
        let offset = (state.clock / 1.3).fract();
        column = column.child(
            div()
                .row()
                .h_px(2.0)
                .child(div().flex(offset.max(0.0001)))
                .child(div().flex(0.3).h_px(2.0).bg(colors.text_accent))
                .child(div().flex((1.0 - offset).max(0.0001))),
        );
    }
    column.into()
}

#[derive(Default)]
struct Ids {
    action: u64,
    line: u64,
    more: u64,
}

impl Ids {
    fn action(&mut self) -> u64 {
        self.action += 1;
        ACTION_BASE + self.action - 1
    }

    fn line(&mut self) -> u64 {
        self.line += 1;
        LINE_BASE + self.line - 1
    }

    fn more(&mut self) -> u64 {
        self.more += 1;
        MORE_BASE + self.more - 1
    }
}

fn size_or_shimmer(state: &StoreState, size: Option<u64>) -> Node {
    match size {
        Some(0) | None if state.measuring > 0 => components::skeleton(60.0, state.clock, 0.0),
        Some(size) => small(format_size(size), theme().text).mono().into(),
        None => small("-", theme().text).mono().into(),
    }
}

fn flash_of(state: &StoreState, key: &str) -> f32 {
    match &state.flash {
        Some((flashed, at)) if flashed == key => (1.0 - (state.clock - at) / FLASH_FOR).max(0.0),
        _ => 0.0,
    }
}

fn confirm_strip(
    request: &Request,
    size: Option<u64>,
    running: bool,
    hovered: Option<u64>,
) -> Node {
    let colors = theme();
    let Request::Relink { workspace, .. } = request else {
        return div().into();
    };
    let freed = size.map_or(String::new(), |size| {
        format!(" Frees up to {}.", format_size(size))
    });
    let mut column = popover_frame(SWAP_POPOVER_W)
        .child(
            Node::from(components::paragraph(format!("Use the shared copy in {workspace}?"), SWAP_POPOVER_W - 24.0).size(LabelSize::Medium, 20.0).color(colors.text)),
        )
        .child(Node::from(components::paragraph(format!(
                "Its node_modules is replaced by a clone of the stored copy; the old folder is kept until the new one is in place.{freed}"
            ), SWAP_POPOVER_W - 24.0).size(LabelSize::Small, 16.0).color(colors.text_muted)));
    if running {
        column = column.child(Node::from(
            components::paragraph(
                "Its services are running: they are stopped first and started again after.",
                SWAP_POPOVER_W - 24.0,
            )
            .size(LabelSize::Small, 16.0)
            .color(colors.warning),
        ));
    }
    column
        .child(confirm_buttons(CANCEL_SWAP, CONFIRM_SWAP, "Swap", hovered))
        .into()
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
    let keep = version.keep_request(repo);
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
    let highlight = state
        .flash
        .as_ref()
        .filter(|(_, at)| state.clock - at < FLASH_FOR)
        .map(|(key, _)| key.as_str());
    let line_id = ids.line();
    let row_hovered = hovered == Some(line_id);
    let users = chips(&listed, users_w, expanded, ids.more(), hovered, highlight);
    let mut progress = false;
    let trailing = match &keep {
        Some(request) => {
            let running = state.busy.as_ref() == Some(request);
            progress |= running;
            button(
                Button {
                    id: ids.action(),
                    text: "Save to Store",
                    working: "Saving",
                    look: Look::Primary,
                    running,
                },
                state,
                hovered,
                row_hovered,
            )
        }
        None if version.stored.is_some() && version.on_main => {
            pill("New workspaces").color(colors.success).into()
        }
        None if version.stored.is_none() => pill("Installs once").color(colors.text_muted).into(),
        None => div().into(),
    };
    let size = match &version.stored {
        Some(entry) => size_or_shimmer(state, Some(entry.size)),
        None => small("-", colors.text).mono().into(),
    };
    let mut rows = vec![line(
        Line {
            id: line_id,
            title,
            detail,
            size,
            users,
            trailing,
            faded: false,
            flash: flash_of(state, &version.key),
            progress,
        },
        state,
        hovered,
    )];
    if version.stored.is_some() {
        let same_as = if version.on_main {
            "main"
        } else {
            first_branch
        };
        for user in version.own_users() {
            let Holding::Own { size } = user.holding else {
                continue;
            };
            let request = Request::Relink {
                repo: repo.to_string(),
                workspace: user.workspace.clone(),
                path: user.path.clone(),
            };
            let running = state.busy.as_ref() == Some(&request);
            let line_id = ids.line();
            let row_hovered = hovered == Some(line_id);
            let users = chips(
                std::slice::from_ref(&user.workspace),
                users_w,
                false,
                ids.more(),
                hovered,
                None,
            );
            let trailing = button(
                Button {
                    id: ids.action(),
                    text: "Use Shared Copy",
                    working: "Swapping",
                    look: Look::Primary,
                    running,
                },
                state,
                hovered,
                row_hovered,
            );
            rows.push(line(
                Line {
                    id: line_id,
                    title: format!("{} has its own copy", user.workspace),
                    detail: format!(
                        "installed on its own; same {} as {same_as}",
                        version.lockfile
                    ),
                    size: size_or_shimmer(state, size),
                    users,
                    trailing,
                    faded: false,
                    flash: 0.0,
                    progress: running,
                },
                state,
                hovered,
            ));
        }
    }
    rows
}

fn group(head: Node, rows: Vec<Node>) -> Node {
    let mut column = components::card().padding(0.0).render().child(
        div()
            .row()
            .items_center()
            .gap(Space::S8)
            .px(ROW_PAD_X)
            .h_px(36.0)
            .child(head),
    );
    for row in rows {
        column = column.child(components::divider()).child(row);
    }
    column.into()
}

fn group_head(name: &str, manager: &str, total: Option<u64>) -> Node {
    let colors = theme();
    let mut head = div().row().items_center().gap(Space::S8).child(
        label(name.to_string())
            .size(LabelSize::Medium)
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
        .px(ROW_PAD_X)
        .pt(Space::S10)
        .pb(Space::S10)
        .items_center()
        .child(text_line(16.0, small(text, theme().text_muted)))
        .into()
}

struct Unused<'a> {
    request: Request,
    title: String,
    detail: &'a str,
    size: u64,
}

fn unused_row(state: &StoreState, unused: Unused<'_>, ids: &mut Ids, hovered: Option<u64>) -> Node {
    let line_id = ids.line();
    let running = state.busy.as_ref() == Some(&unused.request);
    let trailing = button(
        Button {
            id: ids.action(),
            text: "Free",
            working: "Freeing",
            look: Look::Ghost,
            running,
        },
        state,
        hovered,
        hovered == Some(line_id),
    );
    line(
        Line {
            id: line_id,
            title: unused.title,
            detail: unused.detail.to_string(),
            size: size_or_shimmer(state, Some(unused.size)),
            users: small("no workspace", theme().text_placeholder).into(),
            trailing,
            faded: true,
            flash: 0.0,
            progress: running,
        },
        state,
        hovered,
    )
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

fn header(state: &StoreState, hovered: Option<u64>) -> Node {
    let colors = theme();
    let total = state.overview.as_ref().map_or(0, Overview::stored_total);
    let unused: u64 = state.overview.as_ref().map_or(0, |overview| {
        overview.unused().iter().map(|entry| entry.size).sum()
    });
    let summary = match &state.overview {
        None => state
            .unavailable
            .clone()
            .unwrap_or_else(|| "Reading lockfiles".to_string()),
        Some(_) if state.measuring > 0 => format!(
            "measuring {} of {} - {}",
            state.measure_total - state.measuring,
            state.measure_total,
            state.method
        ),
        Some(_) => format!("{} saved - {}", format_size(total), state.method),
    };
    let mut head = components::page_header(
        label("node_modules Store")
            .size(LabelSize::Large)
            .weight(600)
            .color(colors.text),
    )
    .compact()
    .lead(small(summary, colors.text_muted));
    if let Some(request) = &state.busy {
        head = head.lead(
            div()
                .row()
                .items_center()
                .gap(Space::S6)
                .child(pulse(state.clock))
                .child(small(
                    match state.progress {
                        Some((step, steps)) => {
                            format!("Optimizing {step} of {steps} - {}", request.working())
                        }
                        None => request.working(),
                    },
                    colors.text_accent,
                )),
        );
    } else if let Some((text, failed, at)) = &state.note {
        let fade = (1.0 - (state.clock - at - (NOTE_FOR - 0.6)) / 0.6).clamp(0.0, 1.0);
        if fade > 0.0 || *failed {
            let mut color = if *failed {
                colors.error
            } else {
                colors.success
            };
            if !failed {
                color.a *= fade;
            }
            head = head.lead(small(text.clone(), color));
        }
    }
    let idle = state.busy.is_none() && state.overview.is_some();
    head = head.action(
        components::button(REFRESH, "Refresh")
            .kind(ButtonKind::Ghost)
            .icon(Some(IconKind::RotateCw))
            .enabled(state.busy.is_none() && !state.loading())
            .hovered(hovered),
    );
    if unused > 0 && state.measuring == 0 {
        head = head.action(
            components::button(FREE_UNUSED, format!("Free {} unused", format_size(unused)))
                .kind(ButtonKind::Ghost)
                .enabled(idle)
                .hovered(hovered),
        );
    }
    let can_optimize = state.measuring == 0
        && state
            .overview
            .as_ref()
            .is_some_and(|overview| !plan(overview).is_empty());
    if can_optimize {
        head = head.action(
            components::button(OPTIMIZE, "Optimize")
                .kind(ButtonKind::Primary)
                .enabled(idle)
                .hovered(hovered),
        );
    }
    head.into()
}

/// `root` is the store folder, named in the footnote.
pub fn render(state: &StoreState, root: &str, width: f32, hovered: Option<u64>) -> Node {
    let colors = theme();
    let content_w = (width - PAD_X * 2.0).clamp(0.0, CONTENT_MAX_W);
    let users_w =
        (content_w - LABEL_W - SIZE_W - ACTION_W - 3.0 * 10.0 - 2.0 * ROW_PAD_X.px()).max(80.0);
    let mut column = div()
        .col()
        .w_px(content_w)
        .gap(Space::S12)
        .child(header(state, hovered));
    let Some(overview) = &state.overview else {
        return frame(column);
    };
    column = column.child(
        div()
            .row()
            .gap(Space::S10)
            .px(ROW_PAD_X)
            .child(
                div()
                    .row()
                    .w_px(LABEL_W)
                    .child(components::section_header("Lockfile version")),
            )
            .child(
                div()
                    .row()
                    .w_px(SIZE_W)
                    .justify_end()
                    .child(components::section_header("Size")),
            )
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .child(components::section_header("Used by")),
            ),
    );
    let mut ids = Ids::default();
    let measured = state.measuring == 0;
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
                    let unused = Unused {
                        request: Request::FreeOld {
                            repo: repo.repo.clone(),
                        },
                        title: plural(old.len(), "old version", "old versions"),
                        detail: "no workspace uses these lockfiles any more",
                        size: old.iter().map(|entry| entry.size).sum(),
                    };
                    rows.push(unused_row(state, unused, &mut ids, hovered));
                }
                let total = measured.then_some(stored);
                group(group_head(&repo.repo, manager.label(), total), rows)
            }
        };
        column = column.child(node);
    }
    if !overview.others.is_empty() {
        let size = overview.others.iter().map(|entry| entry.size).sum();
        let unused = Unused {
            request: Request::FreeOthers,
            title: plural(overview.others.len(), "copy", "copies"),
            detail: "repos this project does not have",
            size,
        };
        let row = unused_row(state, unused, &mut ids, hovered);
        column = column.child(group(
            group_head("Other projects", "", measured.then_some(size)),
            vec![row],
        ));
    }
    let mut notes = div().col().gap(Space::S4).pt(Space::S4);
    for note in [
        "One copy per lockfile, patches/ folder, Node major version and platform.".to_string(),
        "A new workspace with a matching one gets it in seconds; otherwise it installs once and that install is kept.".to_string(),
        format!("Stored in {root}"),
    ] {
        notes = notes.child(
            div()
                .row()
                .child(small(note, colors.text_placeholder).truncate()),
        );
    }
    frame(column.child(notes))
}

fn frame(column: ui::Div) -> Node {
    div()
        .col()
        .items_center()
        .py(Space::S20)
        .child(column)
        .into()
}

fn click(state: &mut StoreState, id: u64) {
    let idle = state.busy.is_none() && state.overview.is_some();
    match id {
        REFRESH if state.busy.is_none() && !state.loading() => {
            state.requests.push(Request::Refresh)
        }
        FREE_UNUSED if idle => state.requests.push(Request::FreeUnused),
        CANCEL_SWAP => state.confirm = None,
        OPTIMIZE if idle => {
            state.confirm = None;
            state.confirm_optimize = !state.confirm_optimize;
        }
        CANCEL_OPTIMIZE => state.confirm_optimize = false,
        CONFIRM_OPTIMIZE if idle => {
            state.confirm_optimize = false;
            state.requests.push(Request::Optimize);
        }
        CONFIRM_SWAP if idle => {
            if let Some(request) = state.confirm.take() {
                state.requests.push(request);
            }
        }
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
        id if id >= LINE_BASE => return,
        id if id >= ACTION_BASE && idle => {
            let Some(request) = actions(state).into_iter().nth((id - ACTION_BASE) as usize) else {
                return;
            };
            if matches!(request, Request::Relink { .. }) {
                state.confirm = Some(request);
            } else {
                state.confirm = None;
                state.requests.push(request);
            }
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
    opened: Instant,
    painted: Option<(PaintKey, ui::Painted)>,
    /// The button a popover opened from, in window coordinates.
    anchor: Option<Rect>,
    popover_rect: Option<Rect>,
    popover_hits: Vec<(Rect, u64)>,
}

/// What a kept layout was made from; any change lays the page out again.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PaintKey {
    version: u64,
    hovered: Option<u64>,
    scroll: u32,
    body: [u32; 4],
    scale: u32,
    pulse: u64,
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
            opened: Instant::now(),
            painted: None,
            anchor: None,
            popover_rect: None,
            popover_hits: Vec::new(),
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

    /// The row under the pointer, even when a button inside it is what the hit test found.
    fn row_at(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .find(|(rect, id)| {
                (LINE_BASE..MORE_BASE).contains(id)
                    && x >= rect.x
                    && x < rect.x + rect.w
                    && y >= rect.y
                    && y < rect.y + rect.h
            })
            .map(|(_, id)| *id)
    }

    pub fn click(&mut self, id: u64) {
        click(&mut self.shared.borrow_mut(), id);
    }

    fn press(&mut self, id: u64) {
        self.anchor = self
            .hits
            .iter()
            .rev()
            .find(|(_, hit)| *hit == id)
            .map(|(rect, _)| *rect);
        self.click(id);
    }

    pub fn press_optimize(&mut self) {
        self.press(OPTIMIZE);
    }

    fn popover_open(&self) -> bool {
        let state = self.shared.borrow();
        state.confirm.is_some() || state.confirm_optimize
    }

    fn in_popover(&self, x: f32, y: f32) -> bool {
        self.popover_rect.is_some_and(|rect| {
            x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
        })
    }

    fn close_popover(&mut self) {
        let mut state = self.shared.borrow_mut();
        state.confirm = None;
        state.confirm_optimize = false;
        state.changed();
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
        let state = self.shared.borrow();
        // The page is laid out again only when something in it changed or a pulse is due; the loading
        // bar on top moves every frame over the kept layout.
        let key = PaintKey {
            version: state.version,
            hovered: self.hovered,
            scroll: self.scroll.to_bits(),
            body: [body.x, body.y, body.w, body.h].map(f32::to_bits),
            scale: scale.to_bits(),
            pulse: if state.animating() {
                (state.clock / ANIMATION_STEP) as u64
            } else {
                0
            },
        };
        let mut painted = match &self.painted {
            Some((kept, painted)) if *kept == key => painted.clone(),
            _ => {
                let root = state.root.as_deref().unwrap_or(&self.root);
                let tree = render(&state, root, body.w / scale, self.hovered);
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
                self.painted = Some((key, painted.clone()));
                painted
            }
        };
        let loading =
            state.loading() || state.measuring > 0 || state.busy == Some(Request::Refresh);
        if loading {
            let colors = theme();
            let mut track = colors.text_accent;
            track.a = 0.12;
            let h = TOP_BAR_H * scale;
            painted
                .rects
                .push(Rect::new(body.x, body.y, body.w, h, track));
            // Eased so the segment speeds up across the middle and slows at the ends.
            let phase = (state.clock / 1.6).fract();
            let eased = phase * phase * (3.0 - 2.0 * phase);
            let segment = body.w * 0.3;
            let x = body.x - segment + (body.w + segment) * eased;
            let left = x.max(body.x);
            let right = (x + segment).min(body.x + body.w);
            if right > left {
                painted
                    .rects
                    .push(Rect::new(left, body.y, right - left, h, colors.text_accent));
            }
        }
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        if self.popover_open() {
            match hit_in(&self.popover_hits, x, y) {
                Some(id) => self.click(id),
                // A click outside the card dismisses it and does nothing else.
                None if !self.in_popover(x, y) => self.close_popover(),
                None => {}
            }
            return true;
        }
        if let Some(id) = self.hit(x, y) {
            self.press(id);
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        if self.popover_open() {
            let hovered = hit_in(&self.popover_hits, x, y);
            let moved = hovered != self.hovered;
            self.hovered = hovered;
            return moved;
        }
        let hovered = self.hit(x, y);
        // Hovering a row's button keeps the row hovered, so its hover-only actions stay visible.
        let hovered = match hovered {
            Some(id) if !(LINE_BASE..MORE_BASE).contains(&id) => Some(id),
            _ => self.row_at(x, y).or(hovered),
        };
        let moved = hovered != self.hovered;
        self.hovered = hovered;
        moved
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        if self.popover_open() {
            self.close_popover();
        }
        let next = (self.scroll - delta_y).clamp(0.0, (self.content_h - self.body_h).max(0.0));
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn paint_popover(&mut self, body: Rect) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let node = {
            let state = self.shared.borrow();
            popover(&state, self.hovered)
        };
        let (Some(node), Some(anchor)) = (node, self.anchor) else {
            self.popover_hits.clear();
            self.popover_rect = None;
            return None;
        };
        let (w, h) = ui::measure(&node);
        let (w, h) = (w * scale, h * scale);
        let gap = 6.0 * scale;
        let x =
            (anchor.x + anchor.w - w).clamp(body.x + gap, (body.x + body.w - w - gap).max(body.x));
        let below = anchor.y + anchor.h + gap;
        let y = if below + h <= body.y + body.h {
            below
        } else {
            (anchor.y - h - gap).max(body.y)
        };
        let area = Rect::new(x, y, w, h, Rgba::TRANSPARENT);
        let mut painted = ui::render(&node, area);
        let mut rects = ui::elevation_shadow(area, ui::Elevation::Overlay, 1.0);
        rects.append(&mut painted.rects);
        painted.rects = rects;
        self.popover_rect = Some(area);
        self.popover_hits = painted.hits.clone();
        Some(painted)
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let mut state = self.shared.borrow_mut();
        state.clock = self.opened.elapsed().as_secs_f32();
        // Every frame while something moves: the kept layout makes most of them cheap.
        let changed = state.version != self.seen || state.animating();
        self.seen = state.version;
        ItemTick {
            changed,
            ..ItemTick::default()
        }
    }

    fn is_busy(&self) -> bool {
        self.shared.borrow().animating()
    }

    fn animating(&self) -> bool {
        self.shared.borrow().animating()
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
                                    size: Some(1_288_490_188),
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
                                size: Some(1_181_116_006),
                            },
                        )],
                    )],
                    old: vec![entry("admin", "o3", 3.4, 27)],
                },
            },
            RepoOverview {
                repo: "docs".into(),
                state: RepoState::SelfManaged("pnpm".into()),
            },
        ],
        others: vec![entry("billing", "x1", 0.5, 40)],
    };
    let state = StoreState {
        method: "copy-on-write, workspaces take no extra disk".into(),
        overview: Some(overview),
        now,
        running: vec![("web".into(), "feat-pay".into())],
        ..StoreState::default()
    };
    StorePage::new(
        Rc::new(RefCell::new(state)),
        "~/.local/state/pom/nm-store".into(),
    )
}

pub fn board_page(which: &str) -> StorePage {
    use module_store::{Entry, Manager, Method, RepoOverview};
    let measuring = which == "measuring";
    let mb = |size: u64| if measuring { 0 } else { size << 20 };
    let now = 1_800_000_000;
    let day = 86_400;
    let entry = |repo: &str, key: &str, manager: &str, size: u64| Entry {
        repo: repo.into(),
        key: key.into(),
        manager: manager.into(),
        method: Method::Clone,
        size,
        created: now - 30 * day,
        last_used: now - day,
        workspaces: Vec::new(),
    };
    let user = |repo: &str, name: &str, holding: Holding| User {
        workspace: name.into(),
        is_main: name == "main",
        path: PathBuf::from(format!("/work/{name}/{repo}")),
        holding,
    };
    let shared = |repo: &str, names: &[&str]| -> Vec<User> {
        names
            .iter()
            .map(|name| user(repo, name, Holding::Shared))
            .collect()
    };
    let mut web_main = shared(
        "web",
        &[
            "main",
            "feat-login",
            "feat-react",
            "PROJ-101",
            "PROJ-104",
            "fix-cart",
            "fix-tax",
        ],
    );
    web_main.push(user(
        "web",
        "spike-cache",
        Holding::Own {
            size: (!measuring).then(|| mb(975)),
        },
    ));
    let web = RepoOverview {
        repo: "web".into(),
        state: RepoState::Versions {
            manager: Manager::Npm,
            versions: vec![
                Version {
                    key: "w1".into(),
                    lockfile: "package-lock.json".into(),
                    changed: Some(now - 6 * day),
                    on_main: true,
                    stored: Some(entry("web", "w1", "npm", mb(980))),
                    users: web_main,
                },
                Version {
                    key: "w2".into(),
                    lockfile: "package-lock.json".into(),
                    changed: Some(now - 2 * day),
                    on_main: false,
                    stored: Some(entry("web", "w2", "npm", mb(412))),
                    users: shared("web", &["feat-vite"]),
                },
            ],
            old: vec![
                entry("web", "o1", "npm", mb(520)),
                entry("web", "o2", "npm", mb(606)),
            ],
        },
    };
    let worker = RepoOverview {
        repo: "worker".into(),
        state: RepoState::Versions {
            manager: Manager::Yarn,
            versions: vec![
                Version {
                    key: "k1".into(),
                    lockfile: "yarn.lock".into(),
                    changed: Some(now - 21 * day),
                    on_main: true,
                    stored: Some(entry("worker", "k1", "yarn", mb(310))),
                    users: shared("worker", &["main", "feat-login"]),
                },
                Version {
                    key: "k2".into(),
                    lockfile: "yarn.lock".into(),
                    changed: Some(now - 3 * day),
                    on_main: false,
                    stored: None,
                    users: vec![user(
                        "worker",
                        "feat-queue",
                        Holding::Own {
                            size: Some(mb(298)),
                        },
                    )],
                },
            ],
            old: Vec::new(),
        },
    };
    let overview = Overview {
        repos: vec![
            web,
            worker,
            RepoOverview {
                repo: "admin".into(),
                state: RepoState::SelfManaged("pnpm".into()),
            },
            RepoOverview {
                repo: "api".into(),
                state: RepoState::NoLockfile,
            },
        ],
        others: Vec::new(),
    };
    let (left, total) = if measuring { (4, 7) } else { (0, 0) };
    let state = StoreState {
        method: "copy-on-write, workspaces take no extra disk".into(),
        overview: Some(overview),
        measuring: left,
        measure_total: total,
        running: vec![("web".into(), "spike-cache".into())],
        now,
        ..StoreState::default()
    };
    StorePage::new(
        Rc::new(RefCell::new(state)),
        "~/Library/Application Support/Pomelo/modules".into(),
    )
}

#[cfg(test)]
mod tests;
