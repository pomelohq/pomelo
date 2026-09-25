use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use markdown::{MarkdownBody, DOCUMENT_STYLE};
use pom_forge::{PrTarget, PullRequest, TimelineItem, TimelineKind};
use pom_paths::StateDir;
use terminal::Modifiers;
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::{Item, ItemTick, PrSeverity};

const PAD: f32 = 16.0;
const OPEN_ON_GITHUB: u64 = 1;
const REFRESH: u64 = 2;
const OVERVIEW_TAB: u64 = 3;
const CHECKS_TAB: u64 = 4;
const CHECK_BASE: u64 = 100;
const COPY_LINK: u64 = 5;
const COPIED_FOR: std::time::Duration = std::time::Duration::from_millis(1500);
const LINK_BASE: u64 = 1_000_000;
const CONVERSATION_LINK_BASE: u64 = 2_000_000;
const CONVERSATION_LINK_STRIDE: u64 = 10_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Checks,
}

pub(crate) fn item_id(target: &PrTarget) -> String {
    format!("pr:{}:{}", target.repo, target.head)
}

pub struct PrItem {
    state: StateDir,
    session: String,
    waker: Arc<dyn Fn() + Send + Sync>,
    target: PrTarget,
    pr: Option<PullRequest>,
    loading: Option<Receiver<Result<Option<PullRequest>, String>>>,
    error: Option<String>,
    tab: Tab,
    scroll: f32,
    content_h: f32,
    body_h: f32,
    hits: Vec<(Rect, u64)>,
    /// The body parsed once, and laid out again only when the width changes.
    description: Option<(String, MarkdownBody)>,
    /// The timeline it was built from, and every written body in it (items, then their thread comments).
    conversation: Option<(Vec<TimelineItem>, Vec<MarkdownBody>)>,
    /// A link to hand the clipboard on the next tick, and when the last one was copied.
    copy: Option<String>,
    copied_at: Option<std::time::Instant>,
}

impl PrItem {
    pub fn new(
        state: StateDir,
        session: String,
        waker: Arc<dyn Fn() + Send + Sync>,
        target: PrTarget,
        cached: Option<PullRequest>,
    ) -> PrItem {
        let mut item = PrItem {
            state,
            session,
            waker,
            target,
            pr: cached,
            loading: None,
            error: None,
            tab: Tab::Overview,
            scroll: 0.0,
            content_h: 0.0,
            body_h: 0.0,
            hits: Vec::new(),
            description: None,
            conversation: None,
            copy: None,
            copied_at: None,
        };
        item.load();
        item
    }

    fn load(&mut self) {
        if self.loading.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let (state, session, target, waker) = (
            self.state.clone(),
            self.session.clone(),
            self.target.clone(),
            self.waker.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("pull-request".into())
            .spawn(move || {
                let answer = match pom_forge::resolve(&state, &session) {
                    Some(client) => {
                        pom_forge::fetch_detail(&client, &target).map_err(|error| error.to_string())
                    }
                    None => {
                        Err("No GitHub token: set GH_TOKEN or the session's github secret".into())
                    }
                };
                if sender.send(answer).is_err() {
                    eprintln!("pull request: the tab closed before its details arrived");
                }
                waker();
            });
        if spawned.is_ok() {
            self.loading = Some(receiver);
        }
    }

    /// Shows `pr` as if GitHub had returned it (previews and tests).
    pub fn show(&mut self, pr: Option<PullRequest>) {
        self.loading = None;
        self.error = None;
        self.pr = pr;
    }

    fn open(&mut self, url: &str) {
        if url.is_empty() {
            return;
        }
        if let Err(error) = std::process::Command::new("open").arg(url).spawn() {
            self.error = Some(format!("Failed to open {url}: {error}"));
        }
    }

    fn state_pill(pr: &PullRequest) -> Node {
        let colors = theme();
        let (text, color) = match (pr.state.as_str(), pr.is_draft) {
            ("MERGED", _) => ("Merged", PrSeverity::Merged.color()),
            ("CLOSED", _) => ("Closed", colors.error),
            (_, true) => ("Draft", colors.text_muted),
            _ => ("Open", colors.success),
        };
        div()
            .row()
            .h_px(20.0)
            .px(8.0)
            .gap(4.0)
            .items_center()
            .rounded(10.0)
            .bg(Rgba::new(color.r, color.g, color.b, 0.16))
            .child(
                icon(if pr.state == "MERGED" {
                    IconKind::Merged
                } else {
                    IconKind::PullRequest
                })
                .size(12.0)
                .color(color),
            )
            .child(label(text).size(12.0).color(color))
            .into()
    }

    fn header(&self, width: f32) -> Node {
        let colors = theme();
        let button = |id: u64, kind: IconKind, text: &str| {
            div()
                .row()
                .h_px(24.0)
                .px(8.0)
                .gap(4.0)
                .items_center()
                .rounded(4.0)
                .border(1.0, colors.border_variant)
                .on_click(id)
                .child(icon(kind).size(12.0).color(colors.icon_muted))
                .child(label(text.to_string()).size(12.0).color(colors.text))
        };
        let mut top = div().row().gap(8.0).items_center();
        if let Some(pr) = &self.pr {
            top = top.child(Self::state_pill(pr)).child(
                label(format!("#{}", pr.number))
                    .size(13.0)
                    .color(colors.text_muted),
            );
        }
        top = top.child(div().row().flex(1.0));
        if self.pr.is_some() {
            let copied = self.copied_at.is_some_and(|at| at.elapsed() < COPIED_FOR);
            top = top.child(button(
                COPY_LINK,
                IconKind::Copy,
                if copied { "Copied" } else { "Copy Link" },
            ));
            top = top.child(button(
                OPEN_ON_GITHUB,
                IconKind::ArrowUpRight,
                "Open on GitHub",
            ));
        }
        top = top.child(button(
            REFRESH,
            IconKind::RotateCw,
            if self.loading.is_some() {
                "Loading..."
            } else {
                "Refresh"
            },
        ));
        let mut column = div().col().gap(6.0).child(top);
        match &self.pr {
            Some(pr) => {
                column = column.child(
                    label(pr.title.clone())
                        .size(18.0)
                        .color(colors.text)
                        .wrap(width),
                );
                let author = pr
                    .author
                    .as_ref()
                    .map_or("", |author| author.login.as_str());
                column = column.child(
                    div()
                        .row()
                        .gap(10.0)
                        .items_center()
                        .child(
                            label(author.to_string())
                                .size(12.0)
                                .color(colors.text_muted),
                        )
                        .child(
                            label(format!("{} > {}", pr.head_ref_name, pr.base_ref_name))
                                .size(12.0)
                                .mono()
                                .color(colors.text_muted),
                        )
                        .child(
                            label(format!("+{}", pr.additions))
                                .size(12.0)
                                .color(colors.version_control_added),
                        )
                        .child(
                            label(format!("-{}", pr.deletions))
                                .size(12.0)
                                .color(colors.version_control_deleted),
                        ),
                );
            }
            None => {
                let text = if self.loading.is_some() {
                    format!("Looking up {} on GitHub...", self.target.head)
                } else {
                    format!("No pull request for {} yet", self.target.head)
                };
                column = column.child(label(text).size(14.0).color(colors.text_muted));
            }
        }
        if let Some(error) = &self.error {
            column = column.child(
                label(error.clone())
                    .size(12.0)
                    .color(colors.error)
                    .wrap(width),
            );
        }
        column.into()
    }

    fn tabs(&self, checks: usize) -> Node {
        let colors = theme();
        let tab = |id: u64, text: String, active: bool| {
            div()
                .col()
                .gap(6.0)
                .on_click(id)
                .child(label(text).size(13.0).color(if active {
                    colors.text
                } else {
                    colors.text_muted
                }))
                .child(div().h_px(2.0).bg(if active {
                    colors.text_accent
                } else {
                    Rgba::TRANSPARENT
                }))
        };
        div()
            .col()
            .child(
                div()
                    .row()
                    .gap(16.0)
                    .child(tab(
                        OVERVIEW_TAB,
                        "Overview".into(),
                        self.tab == Tab::Overview,
                    ))
                    .child(tab(
                        CHECKS_TAB,
                        format!("Checks {checks}"),
                        self.tab == Tab::Checks,
                    )),
            )
            .child(div().h_px(1.0).bg(colors.border_variant))
            .into()
    }

    fn section(title: &str) -> Node {
        label(title.to_string())
            .size(11.0)
            .color(theme().text_muted)
            .into()
    }

    fn description(&mut self, width: f32) -> Option<Node> {
        let body = self.pr.as_ref()?.body.trim().replace("\r\n", "\n");
        if body.is_empty() {
            return None;
        }
        if self
            .description
            .as_ref()
            .is_none_or(|(source, _)| *source != body)
        {
            self.description = Some((body.clone(), MarkdownBody::new(&body)));
        }
        let (_, parsed) = self.description.as_mut()?;
        Some(parsed.render(width, DOCUMENT_STYLE, LINK_BASE))
    }

    /// Reviews (with their inline threads) and comments after the description, oldest first.
    fn conversation(&mut self, width: f32) -> Option<Node> {
        let timeline = self.pr.as_ref()?.timeline.clone();
        if timeline.is_empty() {
            return None;
        }
        if self
            .conversation
            .as_ref()
            .is_none_or(|(built, _)| *built != timeline)
        {
            let mut bodies = Vec::new();
            for item in &timeline {
                if !item.body.trim().is_empty() {
                    bodies.push(MarkdownBody::new(item.body.trim()));
                }
                for thread in &item.threads {
                    for comment in &thread.comments {
                        bodies.push(MarkdownBody::new(comment.body.trim()));
                    }
                }
            }
            self.conversation = Some((timeline.clone(), bodies));
        }
        let (_, bodies) = self.conversation.as_mut()?;
        let colors = theme();
        let card_width = width - 2.0 * 12.0;
        let mut next = 0usize;
        let mut render_next = |bodies: &mut Vec<MarkdownBody>, width: f32| {
            let index = next;
            next += 1;
            bodies
                .get_mut(index)
                .map(|body| {
                    body.render(
                        width,
                        DOCUMENT_STYLE,
                        CONVERSATION_LINK_BASE + index as u64 * CONVERSATION_LINK_STRIDE,
                    )
                })
                .unwrap_or_else(|| div().into())
        };
        let mut column = div().col().gap(10.0);
        for item in &timeline {
            let (verb, color) = match (item.kind, item.state.as_str()) {
                (TimelineKind::Review, "APPROVED") => ("approved", colors.success),
                (TimelineKind::Review, "CHANGES_REQUESTED") => ("requested changes", colors.error),
                (TimelineKind::Review, "DISMISSED") => ("review dismissed", colors.text_muted),
                (TimelineKind::Review, _) => ("reviewed", colors.text_muted),
                (TimelineKind::Inline, _) => ("commented on the code", colors.text_muted),
                (TimelineKind::Comment, _) => ("commented", colors.text_muted),
            };
            let mut card = div()
                .col()
                .gap(8.0)
                .p(12.0)
                .rounded(6.0)
                .border(1.0, colors.border_variant)
                .child(
                    div()
                        .row()
                        .gap(8.0)
                        .items_center()
                        .child(
                            label(item.author.clone())
                                .size(13.0)
                                .weight(600)
                                .color(colors.text),
                        )
                        .child(label(verb).size(12.0).color(color))
                        .child(
                            label(short_time(&item.at))
                                .size(12.0)
                                .color(colors.text_muted),
                        ),
                );
            if !item.body.trim().is_empty() {
                card = card.child(render_next(bodies, card_width));
            }
            for thread in &item.threads {
                let place = match thread.line {
                    Some(line) => format!("{}:{line}", thread.path),
                    None => thread.path.clone(),
                };
                let mut head = div().row().gap(8.0).items_center().child(
                    div().row().flex(1.0).items_center().child(
                        label(place)
                            .size(12.0)
                            .mono()
                            .color(colors.text_muted)
                            .truncate_start(),
                    ),
                );
                if thread.resolved {
                    head = head.child(label("Resolved").size(11.0).color(colors.success));
                }
                let mut thread_box = div()
                    .col()
                    .gap(8.0)
                    .p(10.0)
                    .rounded(4.0)
                    .bg(colors.panel_background)
                    .child(head);
                for comment in &thread.comments {
                    thread_box = thread_box.child(
                        div()
                            .col()
                            .gap(4.0)
                            .child(
                                div()
                                    .row()
                                    .gap(8.0)
                                    .items_center()
                                    .child(
                                        label(comment.author.clone())
                                            .size(12.0)
                                            .weight(600)
                                            .color(colors.text),
                                    )
                                    .child(
                                        label(short_time(&comment.at))
                                            .size(11.0)
                                            .color(colors.text_muted),
                                    ),
                            )
                            .child(render_next(bodies, card_width - 2.0 * 10.0)),
                    );
                }
                card = card.child(thread_box);
            }
            column = column.child(card);
        }
        Some(column.into())
    }

    fn overview(pr: &PullRequest, description: Option<Node>, conversation: Option<Node>) -> Node {
        let colors = theme();
        let mut column = div().col().gap(8.0).child(Self::section("REVIEWERS"));
        if pr.reviewers.is_empty() {
            column = column.child(label("No reviewers").size(13.0).color(colors.text_muted));
        }
        for reviewer in &pr.reviewers {
            let (kind, color, text) = match reviewer.state.as_str() {
                "approved" => (IconKind::Check, colors.success, "approved"),
                "changes" => (IconKind::Warning, colors.error, "requested changes"),
                "pending" => (IconKind::Clock, colors.warning, "review requested"),
                _ => (IconKind::Eye, colors.text_muted, "commented"),
            };
            column = column.child(
                div()
                    .row()
                    .gap(8.0)
                    .items_center()
                    .child(icon(kind).size(14.0).color(color))
                    .child(label(reviewer.name.clone()).size(13.0).color(colors.text))
                    .child(label(text).size(12.0).color(colors.text_muted)),
            );
        }
        if !pr.labels.is_empty() {
            let mut chips = div().row().gap(6.0);
            for tag in &pr.labels {
                let color = hex_color(&tag.color).unwrap_or(colors.text_muted);
                chips = chips.child(
                    div()
                        .row()
                        .h_px(20.0)
                        .px(8.0)
                        .items_center()
                        .rounded(10.0)
                        .bg(Rgba::new(color.r, color.g, color.b, 0.2))
                        .child(label(tag.name.clone()).size(12.0).color(colors.text)),
                );
            }
            column = column
                .child(div().h_px(8.0))
                .child(Self::section("LABELS"))
                .child(chips);
        }
        column = column
            .child(div().h_px(8.0))
            .child(Self::section("DESCRIPTION"));
        column = column.child(description.unwrap_or_else(|| {
            label("No description.")
                .size(13.0)
                .color(colors.text_muted)
                .into()
        }));
        if let Some(conversation) = conversation {
            column = column
                .child(div().h_px(8.0))
                .child(Self::section(&format!(
                    "CONVERSATION ({})",
                    pr.timeline.len()
                )))
                .child(conversation);
        }
        column.into()
    }

    fn checks(pr: &PullRequest) -> Node {
        let colors = theme();
        let mut column = div().col().gap(2.0);
        if pr.status_check_rollup.is_empty() {
            return column
                .child(label("No checks").size(13.0).color(colors.text_muted))
                .into();
        }
        for (index, check) in pr.status_check_rollup.iter().enumerate() {
            let (kind, color) = match check.result.as_str() {
                "pass" => (IconKind::Check, colors.success),
                "fail" => (IconKind::XCircle, colors.error),
                "pending" => (IconKind::Clock, colors.warning),
                _ => (IconKind::Dash, colors.text_disabled),
            };
            let mut row = div()
                .row()
                .h_px(30.0)
                .px(8.0)
                .gap(8.0)
                .items_center()
                .rounded(4.0)
                .child(icon(kind).size(14.0).color(color))
                .child(label(check.name.clone()).size(13.0).color(colors.text));
            if !check.workflow_name.is_empty() {
                row = row.child(
                    label(check.workflow_name.clone())
                        .size(12.0)
                        .color(colors.text_muted),
                );
            }
            row = row.child(div().row().flex(1.0));
            if !check.details_url.is_empty() {
                row = row.on_click(CHECK_BASE + index as u64).child(
                    icon(IconKind::ArrowUpRight)
                        .size(12.0)
                        .color(colors.icon_muted),
                );
            }
            column = column.child(row);
        }
        column.into()
    }

    fn click(&mut self, id: u64) {
        match id {
            OPEN_ON_GITHUB => {
                if let Some(url) = self.pr.as_ref().map(|pr| pr.url.clone()) {
                    self.open(&url);
                }
            }
            REFRESH => self.load(),
            COPY_LINK => {
                self.copy = self
                    .pr
                    .as_ref()
                    .map(|pr| pr.url.clone())
                    .filter(|url| !url.is_empty());
            }
            id if id >= CONVERSATION_LINK_BASE => {
                let offset = id - CONVERSATION_LINK_BASE;
                let url = self.conversation.as_ref().and_then(|(_, bodies)| {
                    bodies
                        .get((offset / CONVERSATION_LINK_STRIDE) as usize)?
                        .link((offset % CONVERSATION_LINK_STRIDE) as usize)
                        .map(str::to_string)
                });
                if let Some(url) = url {
                    self.open(&url);
                }
            }
            id if id >= LINK_BASE => {
                let url = self.description.as_ref().and_then(|(_, parsed)| {
                    parsed.link((id - LINK_BASE) as usize).map(str::to_string)
                });
                if let Some(url) = url {
                    self.open(&url);
                }
            }
            OVERVIEW_TAB => self.tab = Tab::Overview,
            CHECKS_TAB => self.tab = Tab::Checks,
            id if id >= CHECK_BASE => {
                let url = self
                    .pr
                    .as_ref()
                    .and_then(|pr| pr.status_check_rollup.get((id - CHECK_BASE) as usize))
                    .map(|check| check.details_url.clone());
                if let Some(url) = url {
                    self.open(&url);
                }
            }
            _ => {}
        }
    }
}

/// `2026-09-17T03:53:12Z` as `2026-09-17 03:53`.
fn short_time(stamp: &str) -> String {
    match (stamp.get(..10), stamp.get(11..16)) {
        (Some(date), Some(time)) if stamp.as_bytes().get(10) == Some(&b'T') => {
            format!("{date} {time}")
        }
        _ => stamp.to_string(),
    }
}

fn hex_color(hex: &str) -> Option<Rgba> {
    let hex = hex.trim_start_matches('#');
    let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some(Rgba::new(
        f32::from(channel(0)?) / 255.0,
        f32::from(channel(2)?) / 255.0,
        f32::from(channel(4)?) / 255.0,
        1.0,
    ))
}

impl Item for PrItem {
    fn id(&self) -> Option<String> {
        Some(item_id(&self.target))
    }

    fn title(&self) -> String {
        match &self.pr {
            Some(pr) => format!("#{} {}", pr.number, pr.title),
            None => format!("PR {} [{}]", self.target.head, self.target.repo),
        }
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::PullRequest)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        self.body_h = body.h / scale;
        let width = (body.w / scale - 2.0 * PAD).max(120.0);
        let checks = self
            .pr
            .as_ref()
            .map_or(0, |pr| pr.status_check_rollup.len());
        let mut column = div()
            .col()
            .w_px(body.w / scale)
            .p(PAD)
            .gap(14.0)
            .child(self.header(width));
        let (description, conversation) = match self.tab {
            Tab::Overview => (self.description(width), self.conversation(width)),
            Tab::Checks => (None, None),
        };
        if let Some(pr) = &self.pr {
            column = column.child(self.tabs(checks)).child(match self.tab {
                Tab::Overview => Self::overview(pr, description, conversation),
                Tab::Checks => Self::checks(pr),
            });
        }
        let tree: Node = div()
            .col()
            .w_px(body.w / scale)
            .bg(theme().editor_background)
            .child(column)
            .into();
        let shifted = Rect::new(
            body.x,
            body.y - self.scroll * scale,
            body.w,
            body.h + self.scroll * scale,
            Rgba::TRANSPARENT,
        );
        let painted = ui::render(&tree, shifted);
        self.content_h = painted
            .rects
            .iter()
            .map(|rect| (rect.y + rect.h - shifted.y) / scale)
            .fold(0.0, f32::max);
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id);
        if let Some(id) = hit {
            self.click(id);
        }
        true
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let max = (self.content_h + PAD - self.body_h).max(0.0);
        let next = (self.scroll - delta_y).clamp(0.0, max);
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        if let Some(url) = self.copy.take() {
            self.copied_at = Some(std::time::Instant::now());
            return ItemTick {
                changed: true,
                clipboard_store: Some(url),
                ..ItemTick::default()
            };
        }
        let label_expired = self.copied_at.is_some_and(|at| at.elapsed() >= COPIED_FOR);
        if label_expired {
            self.copied_at = None;
        }
        let answer = match self.loading.as_ref().map(Receiver::try_recv) {
            Some(Ok(answer)) => answer,
            Some(Err(TryRecvError::Empty)) | None => {
                return ItemTick {
                    changed: label_expired,
                    ..ItemTick::default()
                }
            }
            Some(Err(TryRecvError::Disconnected)) => Err("the lookup stopped".into()),
        };
        self.loading = None;
        match answer {
            Ok(pr) => {
                self.error = None;
                self.pr = pr;
            }
            Err(error) => self.error = Some(error),
        }
        ItemTick {
            changed: true,
            ..ItemTick::default()
        }
    }

    fn is_busy(&self) -> bool {
        self.loading.is_some() || self.copied_at.is_some()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_conversation_follows_the_description() {
        let temp = tempfile::tempdir().expect("temp");
        let mut item = PrItem::new(
            StateDir::new(temp.path()),
            "myproject".into(),
            Arc::new(|| {}),
            PrTarget {
                repo: "web".into(),
                owner: "acme".into(),
                name: "web".into(),
                head: "feat".into(),
            },
            None,
        );
        item.show(Some(PullRequest {
            number: 7,
            title: "Login".into(),
            state: "OPEN".into(),
            timeline: vec![
                TimelineItem {
                    kind: TimelineKind::Review,
                    author: "bea".into(),
                    body: "Fix the **query**".into(),
                    at: "2026-09-01T09:00:00Z".into(),
                    state: "CHANGES_REQUESTED".into(),
                    threads: vec![pom_forge::ReviewThread {
                        path: "src/db.rs".into(),
                        line: Some(12),
                        resolved: true,
                        comments: vec![pom_forge::ThreadComment {
                            author: "bea".into(),
                            body: "N+1 here".into(),
                            at: "2026-09-01T09:00:00Z".into(),
                        }],
                    }],
                },
                TimelineItem {
                    kind: TimelineKind::Comment,
                    author: "ann".into(),
                    body: "See [notes](https://example.com/notes)".into(),
                    at: "2026-09-02T10:00:00Z".into(),
                    ..TimelineItem::default()
                },
            ],
            ..PullRequest::default()
        }));
        let painted = item
            .paint_body(Rect::new(0.0, 0.0, 800.0, 3000.0, Rgba::TRANSPARENT), true)
            .expect("painted");
        let texts: Vec<&str> = painted
            .texts
            .iter()
            .map(|text| text.text.as_str())
            .collect();
        for expected in [
            "CONVERSATION (2)",
            "requested changes",
            "query",
            "src/db.rs:12",
            "Resolved",
            "N+1 here",
            "ann",
            "2026-09-02 10:00",
            "notes",
        ] {
            assert!(texts.contains(&expected), "{expected}: {texts:?}");
        }
        assert!(painted
            .hits
            .iter()
            .any(|(_, id)| *id >= CONVERSATION_LINK_BASE));
    }

    #[test]
    fn the_description_renders_as_markdown_with_clickable_links() {
        let temp = tempfile::tempdir().expect("temp");
        let mut item = PrItem::new(
            StateDir::new(temp.path()),
            "myproject".into(),
            Arc::new(|| {}),
            PrTarget {
                repo: "web".into(),
                owner: "acme".into(),
                name: "web".into(),
                head: "feat".into(),
            },
            None,
        );
        item.show(Some(PullRequest {
            number: 7,
            title: "Login".into(),
            state: "OPEN".into(),
            url: "https://github.com/acme/web/pull/7".into(),
            body: "### Related ticket\r\n[PROJ-101](https://example.com/PROJ-101)\r\n\r\nSome **bold** text".into(),
            ..PullRequest::default()
        }));
        let painted = item
            .paint_body(Rect::new(0.0, 0.0, 800.0, 2000.0, Rgba::TRANSPARENT), true)
            .expect("painted");
        let texts: Vec<&str> = painted
            .texts
            .iter()
            .map(|text| text.text.as_str())
            .collect();
        assert!(texts.contains(&"Related ticket"), "{texts:?}");
        assert!(texts.contains(&"bold"), "{texts:?}");
        assert!(!texts.iter().any(|text| text.contains("###")), "{texts:?}");
        assert!(painted.hits.iter().any(|(_, id)| *id == LINK_BASE));
        let (copy, _) = painted
            .hits
            .iter()
            .find(|(_, id)| *id == COPY_LINK)
            .expect("copy button");
        item.pointer_down(copy.x + 1.0, copy.y + 1.0, 1, Modifiers::default());
        assert_eq!(
            item.tick(&|| None).clipboard_store.as_deref(),
            Some("https://github.com/acme/web/pull/7")
        );
    }
}
