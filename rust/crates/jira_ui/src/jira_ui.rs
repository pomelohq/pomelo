//! A workspace's Jira ticket as a tab: key and status, title, the description and comments as markdown, and
//! the ticket's web links. Opens on what the cache holds and fetches the ticket again in the background.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use markdown::{Markdown, MarkdownLayout, DOCUMENT_STYLE};
use pom_jira::{IssueCache, IssueDetail};
use pom_paths::StateDir;
use terminal::Modifiers;
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::{Item, ItemTick};

const PAD: f32 = 16.0;
const OPEN_IN_JIRA: u64 = 1;
const RELOAD: u64 = 2;
const COPY_LINK: u64 = 3;
const COPIED_FOR: std::time::Duration = std::time::Duration::from_millis(1500);
const WEB_LINK_BASE: u64 = 100;
const DESCRIPTION_LINK_BASE: u64 = 1_000_000;
const COMMENT_LINK_BASE: u64 = 2_000_000;
const COMMENT_LINK_STRIDE: u64 = 10_000;

pub fn item_id(key: &str) -> String {
    format!("ticket:{key}")
}

type Waker = Arc<dyn Fn() + Send + Sync>;

/// A markdown body parsed once, laid out again only when the width changes.
struct Body {
    markdown: Markdown,
    layout: Option<(u32, MarkdownLayout)>,
}

impl Body {
    fn new(source: &str) -> Body {
        Body {
            markdown: Markdown::parse(source),
            layout: None,
        }
    }

    fn render(&mut self, width: f32, link_base: u64) -> Node {
        let key = width.to_bits();
        if self.layout.as_ref().is_none_or(|(at, _)| *at != key) {
            self.layout = Some((key, self.markdown.layout(width, DOCUMENT_STYLE)));
        }
        match &self.layout {
            Some((_, layout)) => {
                layout.render_links(0, layout.line_count(), width, Some(link_base))
            }
            None => div().into(),
        }
    }
}

pub struct TicketItem {
    state: StateDir,
    session: String,
    key: String,
    waker: Waker,
    detail: Option<IssueDetail>,
    /// Jira's status category (`new`, `indeterminate`, `done`), from the WORKSPACES status cache.
    category: String,
    description: Option<Body>,
    comments: Vec<Body>,
    loading: Option<Receiver<Result<IssueDetail, String>>>,
    error: Option<String>,
    scroll: f32,
    content_h: f32,
    body_h: f32,
    hits: Vec<(Rect, u64)>,
    /// A link to hand the clipboard on the next tick, and when the last one was copied.
    copy: Option<String>,
    copied_at: Option<std::time::Instant>,
}

impl TicketItem {
    pub fn new(state: StateDir, session: String, key: String, waker: Waker) -> TicketItem {
        let cache = IssueCache::open(&state, &session);
        let category = cache
            .issue(&key)
            .map(|issue| issue.category.clone())
            .unwrap_or_default();
        let cached = cache.detail(&key).filter(|detail| !detail.key.is_empty());
        let mut item = TicketItem {
            state,
            session,
            key,
            waker,
            detail: None,
            category,
            description: None,
            comments: Vec::new(),
            loading: None,
            error: None,
            scroll: 0.0,
            content_h: 0.0,
            body_h: 0.0,
            hits: Vec::new(),
            copy: None,
            copied_at: None,
        };
        if let Some(detail) = cached {
            item.set_detail(detail);
        }
        item.load();
        item
    }

    fn load(&mut self) {
        if self.loading.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let (state, session, key, waker) = (
            self.state.clone(),
            self.session.clone(),
            self.key.clone(),
            self.waker.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("jira-ticket".into())
            .spawn(move || {
                let answer = match pom_jira::resolve(&state, &session) {
                    Some(client) => client.issue_detail(&key).map_err(|error| error.to_string()),
                    None => Err("Jira is not set up: add the site, email and token in Settings > Integrations".into()),
                };
                if let Ok(detail) = &answer {
                    IssueCache::open(&state, &session).store_detail(detail);
                }
                if sender.send(answer).is_err() {
                    eprintln!("jira: the ticket tab closed before the ticket arrived");
                }
                waker();
            });
        match spawned {
            Ok(_) => self.loading = Some(receiver),
            Err(error) => self.error = Some(format!("Could not load the ticket: {error}")),
        }
    }

    /// Shows `detail` as if Jira had returned it (previews and tests).
    pub fn show(&mut self, detail: IssueDetail, category: &str) {
        self.loading = None;
        self.error = None;
        self.category = category.to_string();
        self.set_detail(detail);
    }

    fn set_detail(&mut self, detail: IssueDetail) {
        let description = detail.description.trim();
        self.description = (!description.is_empty()).then(|| Body::new(description));
        self.comments = detail
            .comments
            .iter()
            .map(|comment| Body::new(comment.body.trim()))
            .collect();
        self.detail = Some(detail);
    }

    fn open(&mut self, url: &str) {
        if url.is_empty() {
            return;
        }
        if let Err(error) = std::process::Command::new("open").arg(url).spawn() {
            self.error = Some(format!("Failed to open {url}: {error}"));
        }
    }

    fn status_color(&self) -> Rgba {
        let colors = theme();
        match self.category.as_str() {
            "done" => colors.success,
            "indeterminate" => colors.text_accent,
            _ => colors.text_muted,
        }
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
        let mut top = div().row().gap(8.0).items_center().child(
            label(self.key.clone())
                .size(13.0)
                .mono()
                .color(colors.text_muted),
        );
        if let Some(detail) = self
            .detail
            .as_ref()
            .filter(|detail| !detail.status.is_empty())
        {
            let color = self.status_color();
            top = top.child(
                div()
                    .row()
                    .h_px(20.0)
                    .px(8.0)
                    .items_center()
                    .rounded(10.0)
                    .bg(Rgba::new(color.r, color.g, color.b, 0.16))
                    .child(label(detail.status.clone()).size(12.0).color(color)),
            );
        }
        top = top.child(div().row().flex(1.0));
        if self
            .detail
            .as_ref()
            .is_some_and(|detail| !detail.url.is_empty())
        {
            let copied = self.copied_at.is_some_and(|at| at.elapsed() < COPIED_FOR);
            top = top.child(button(
                COPY_LINK,
                IconKind::Copy,
                if copied { "Copied" } else { "Copy Link" },
            ));
            top = top.child(button(OPEN_IN_JIRA, IconKind::ArrowUpRight, "Open in Jira"));
        }
        top = top.child(button(
            RELOAD,
            IconKind::RotateCw,
            if self.loading.is_some() {
                "Loading..."
            } else {
                "Reload"
            },
        ));
        let mut column = div().col().gap(6.0).child(top);
        match &self.detail {
            Some(detail) => {
                column = column.child(
                    label(detail.summary.clone())
                        .size(18.0)
                        .color(colors.text)
                        .wrap(width),
                );
            }
            None => {
                let text = if self.loading.is_some() {
                    format!("Loading {} from Jira...", self.key)
                } else {
                    format!("{} could not be loaded", self.key)
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

    fn section(title: String) -> Node {
        label(title).size(11.0).color(theme().text_muted).into()
    }

    fn content(&mut self, width: f32) -> Node {
        let colors = theme();
        let mut column = div().col().gap(14.0).child(self.header(width));
        let Some(detail) = self.detail.clone() else {
            return column.into();
        };
        let description = match self.description.as_mut() {
            Some(body) => body.render(width, DESCRIPTION_LINK_BASE),
            None => label("No description.")
                .size(13.0)
                .color(colors.text_muted)
                .into(),
        };
        column = column
            .child(Self::section("DESCRIPTION".into()))
            .child(description);
        if !detail.web_links.is_empty() {
            let mut links = div().col().gap(2.0);
            for (index, link) in detail.web_links.iter().enumerate() {
                let title = if link.title.is_empty() {
                    link.url.clone()
                } else {
                    link.title.clone()
                };
                links = links.child(
                    div()
                        .row()
                        .h_px(26.0)
                        .gap(8.0)
                        .items_center()
                        .on_click(WEB_LINK_BASE + index as u64)
                        .child(
                            icon(IconKind::ArrowUpRight)
                                .size(12.0)
                                .color(colors.icon_muted),
                        )
                        .child(label(title).size(13.0).color(colors.text_accent).truncate()),
                );
            }
            column = column.child(Self::section("WEB LINKS".into())).child(links);
        }
        column = column.child(Self::section(format!(
            "COMMENTS ({})",
            detail.comments.len()
        )));
        if detail.comments.is_empty() {
            column = column.child(label("No comments.").size(13.0).color(colors.text_muted));
        }
        for (index, comment) in detail.comments.iter().enumerate() {
            let body = match self.comments.get_mut(index) {
                Some(body) => body.render(
                    width - 2.0 * 12.0,
                    COMMENT_LINK_BASE + index as u64 * COMMENT_LINK_STRIDE,
                ),
                None => div().into(),
            };
            column = column.child(
                div()
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
                                label(comment.author.clone())
                                    .size(13.0)
                                    .weight(600)
                                    .color(colors.text),
                            )
                            .child(
                                label(short_time(&comment.created))
                                    .size(12.0)
                                    .color(colors.text_muted),
                            ),
                    )
                    .child(body),
            );
        }
        column.into()
    }

    fn click(&mut self, id: u64) {
        let Some(detail) = self.detail.clone() else {
            if id == RELOAD {
                self.load();
            }
            return;
        };
        let url = match id {
            OPEN_IN_JIRA => Some(detail.url.clone()),
            COPY_LINK => {
                self.copy = Some(detail.url.clone()).filter(|url| !url.is_empty());
                None
            }
            RELOAD => {
                self.load();
                None
            }
            id if id >= COMMENT_LINK_BASE => {
                let offset = id - COMMENT_LINK_BASE;
                let comment = (offset / COMMENT_LINK_STRIDE) as usize;
                let link = (offset % COMMENT_LINK_STRIDE) as usize;
                self.comments
                    .get(comment)
                    .and_then(|body| body.markdown.links().get(link).cloned())
            }
            id if id >= DESCRIPTION_LINK_BASE => self.description.as_ref().and_then(|body| {
                body.markdown
                    .links()
                    .get((id - DESCRIPTION_LINK_BASE) as usize)
                    .cloned()
            }),
            id if id >= WEB_LINK_BASE => detail
                .web_links
                .get((id - WEB_LINK_BASE) as usize)
                .map(|link| link.url.clone()),
            _ => None,
        };
        if let Some(url) = url {
            self.open(&url);
        }
    }
}

/// `2026-09-17T03:53:12.000+0700` as `2026-09-17 03:53`.
fn short_time(stamp: &str) -> String {
    match (stamp.get(..10), stamp.get(11..16)) {
        (Some(date), Some(time)) if stamp.as_bytes().get(10) == Some(&b'T') => {
            format!("{date} {time}")
        }
        _ => stamp.to_string(),
    }
}

impl Item for TicketItem {
    fn id(&self) -> Option<String> {
        Some(item_id(&self.key))
    }

    fn title(&self) -> String {
        match &self.detail {
            Some(detail) if !detail.summary.is_empty() => {
                format!("{} {}", self.key, detail.summary)
            }
            _ => self.key.clone(),
        }
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Ticket)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        self.body_h = body.h / scale;
        let full = body.w / scale;
        let width = (full - 2.0 * PAD).max(120.0);
        let content = self.content(width);
        let tree: Node = div()
            .col()
            .w_px(full)
            .bg(theme().editor_background)
            .child(div().col().w_px(full).p(PAD).child(content))
            .into();
        // The column starts `scroll` above the body; the pane clips what falls outside.
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
            .chain(
                painted
                    .texts
                    .iter()
                    .map(|text| (text.y - shifted.y) / scale + text.size * 1.4),
            )
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
            Some(Err(TryRecvError::Disconnected)) => Err("the ticket lookup stopped".into()),
        };
        self.loading = None;
        match answer {
            Ok(detail) => {
                self.error = None;
                self.set_detail(detail);
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

    fn detail() -> IssueDetail {
        IssueDetail {
            key: "PROJ-101".into(),
            summary: "Flag escalated conversations".into(),
            status: "In Progress".into(),
            url: "https://example.atlassian.net/browse/PROJ-101".into(),
            description:
                "## Background\n\nThe **inbox** needs a flag. See https://example.com/spec.".into(),
            comments: vec![pom_jira::Comment {
                id: "1".into(),
                author: "Ann".into(),
                avatar: String::new(),
                created: "2026-09-17T03:53:12.000+0700".into(),
                body: "Looks *good*".into(),
            }],
            web_links: vec![pom_jira::WebLink {
                title: "Design doc".into(),
                url: "https://example.com/design".into(),
                icon: String::new(),
            }],
        }
    }

    #[test]
    fn shows_status_description_links_and_comments() {
        let temp = tempfile::tempdir().expect("temp");
        let mut item = TicketItem::new(
            StateDir::new(temp.path()),
            "myproject".into(),
            "PROJ-101".into(),
            Arc::new(|| {}),
        );
        item.show(detail(), "indeterminate");
        assert_eq!(item.title(), "PROJ-101 Flag escalated conversations");
        let painted = item
            .paint_body(Rect::new(0.0, 0.0, 800.0, 2000.0, Rgba::TRANSPARENT), true)
            .expect("painted");
        let texts: Vec<&str> = painted
            .texts
            .iter()
            .map(|text| text.text.as_str())
            .collect();
        for expected in [
            "PROJ-101",
            "In Progress",
            "Background",
            "inbox",
            "Design doc",
            "COMMENTS (1)",
            "Ann",
            "2026-09-17 03:53",
            "good",
        ] {
            assert!(texts.contains(&expected), "{expected}: {texts:?}");
        }
        assert!(!texts.iter().any(|text| text.contains("##")), "{texts:?}");
        let ids: Vec<u64> = painted.hits.iter().map(|(_, id)| *id).collect();
        assert!(ids.contains(&OPEN_IN_JIRA));
        assert!(ids.contains(&WEB_LINK_BASE));
        assert!(ids.contains(&DESCRIPTION_LINK_BASE));
        let (copy, _) = painted
            .hits
            .iter()
            .find(|(_, id)| *id == COPY_LINK)
            .expect("copy button");
        item.pointer_down(copy.x + 1.0, copy.y + 1.0, 1, Modifiers::default());
        let tick = item.tick(&|| None);
        assert_eq!(
            tick.clipboard_store.as_deref(),
            Some("https://example.atlassian.net/browse/PROJ-101")
        );
    }

    #[test]
    fn the_cache_shows_the_ticket_before_jira_answers() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        IssueCache::open(&state, "myproject").store_detail(&detail());
        let item = TicketItem::new(
            state,
            "myproject".into(),
            "PROJ-101".into(),
            Arc::new(|| {}),
        );
        assert_eq!(
            item.detail.as_ref().map(|detail| detail.summary.as_str()),
            Some("Flag escalated conversations")
        );
    }

    #[test]
    fn times_read_short() {
        assert_eq!(
            short_time("2026-09-17T03:53:12.000+0700"),
            "2026-09-17 03:53"
        );
        assert_eq!(short_time("yesterday"), "yesterday");
    }
}
