use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use markdown::{MarkdownBody, MarkdownStyle, DOCUMENT_STYLE};
use pom_forge::{PrTarget, PullRequest, TimelineItem, TimelineKind};
use pom_paths::StateDir;
use terminal::Modifiers;
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::{Item, ItemTick};

const PAD: f32 = 24.0;
/// Text past this width gets hard to read, so the overview and checks stop growing with the tab.
const MAX_CONTENT_W: f32 = 760.0;
const CREATE_PR: u64 = 6;
/// The tab reads at the panel's 12px text, so bodies use the document layout scaled down to it.
const BODY_STYLE: MarkdownStyle = MarkdownStyle {
    text_size: 12.0,
    line_height: 18.0,
    paragraph_spacing: 8.0,
    code_block_margin: 8.0,
    heading_margin_top: 12.0,
    code_size: 11.5,
    code_line_height: 18.0,
    ..DOCUMENT_STYLE
};
/// Comments sit in tighter cards than the description.
const COMMENT_STYLE: MarkdownStyle = MarkdownStyle {
    line_height: 14.0,
    ..BODY_STYLE
};
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

    /// Shows the lookup as failed with `error` (previews and tests).
    pub fn show_error(&mut self, error: &str) {
        self.loading = None;
        self.pr = None;
        self.error = Some(error.to_string());
    }

    /// Picks the Checks sub-tab (previews and tests).
    pub fn show_checks(&mut self) {
        self.tab = Tab::Checks;
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
        let (text, color, fill) = match (pr.state.as_str(), pr.is_draft) {
            ("MERGED", _) => ("Merged", colors.text_accent, colors.text_accent.alpha(0.18)),
            ("CLOSED", _) => ("Closed", colors.error, colors.error.alpha(0.16)),
            (_, true) => ("Draft", colors.text_muted, colors.element_hover),
            _ => ("Open", colors.success, colors.success.alpha(0.16)),
        };
        let kind = match pr.state.as_str() {
            "MERGED" => IconKind::Merged,
            "CLOSED" => IconKind::XCircle,
            _ => IconKind::PullRequest,
        };
        div()
            .row()
            .h_px(22.0)
            .px(9.0)
            .gap(5.0)
            .items_center()
            .rounded(12.0)
            .bg(fill)
            .child(icon(kind).size(13.0).color(color))
            .child(label(text).size(12.0).weight(600).color(color))
            .into()
    }

    fn header_button(id: u64, kind: IconKind, text: &str) -> Node {
        let colors = theme();
        div()
            .row()
            .h_px(26.0)
            .px(8.0)
            .gap(5.0)
            .items_center()
            .rounded(4.0)
            .on_click(id)
            .child(icon(kind).size(13.0).color(colors.text_muted))
            .child(label(text.to_string()).size(12.0).color(colors.text_muted))
            .into()
    }

    fn header(&self, width: f32) -> Node {
        let colors = theme();
        let mut top = div().row().gap(10.0).items_center();
        if let Some(pr) = &self.pr {
            top = top
                .child(Self::state_pill(pr))
                .child(
                    label(format!("#{}", pr.number))
                        .size(11.5)
                        .mono()
                        .color(colors.text_placeholder),
                )
                .child(
                    label(self.target.repo.clone())
                        .size(12.0)
                        .color(colors.text_placeholder),
                );
        }
        top = top.child(div().row().flex(1.0));
        if self.pr.is_some() {
            let copied = self.copied_at.is_some_and(|at| at.elapsed() < COPIED_FOR);
            top = top
                .child(Self::header_button(
                    COPY_LINK,
                    IconKind::Copy,
                    if copied { "Copied" } else { "Copy Link" },
                ))
                .child(Self::header_button(
                    OPEN_ON_GITHUB,
                    IconKind::ArrowUpRight,
                    "Open on GitHub",
                ))
                .child(Self::header_button(
                    REFRESH,
                    IconKind::RotateCw,
                    if self.loading.is_some() {
                        "Loading..."
                    } else {
                        "Refresh"
                    },
                ));
        }
        let mut column = div().col().gap(10.0).child(top.pb(2.0));
        if let Some(pr) = &self.pr {
            column = column.child(
                label(pr.title.clone())
                    .size(20.0)
                    .weight(600)
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
                    .h_px(14.0)
                    .gap(12.0)
                    .items_center()
                    .child(
                        label(author.to_string())
                            .size(12.0)
                            .color(colors.text_muted),
                    )
                    .child(
                        label(format!("{} > {}", pr.head_ref_name, pr.base_ref_name))
                            .size(11.5)
                            .mono()
                            .color(colors.text_placeholder),
                    )
                    .child(
                        label(format!("+{}", pr.additions))
                            .size(11.5)
                            .mono()
                            .color(colors.success),
                    )
                    .child(
                        label(format!("-{}", pr.deletions))
                            .size(11.5)
                            .mono()
                            .color(colors.error),
                    ),
            );
            if let Some(error) = &self.error {
                column = column.child(
                    label(error.clone())
                        .size(12.0)
                        .color(colors.error)
                        .wrap(width),
                );
            }
        }
        column.into()
    }

    /// What stands between the PR and a merge, as raised chips: failing checks, requested changes, the merge state.
    fn blockers(pr: &PullRequest) -> Option<Node> {
        if pr.state != "OPEN" {
            return None;
        }
        let colors = theme();
        let mut chips: Vec<(IconKind, Rgba, String)> = Vec::new();
        if pr.checks == "fail" {
            let failing: Vec<&str> = pr
                .status_check_rollup
                .iter()
                .filter(|check| check.result == "fail")
                .map(|check| check.name.as_str())
                .collect();
            chips.push((
                IconKind::XCircle,
                colors.error,
                format!("CI failed: {}", failing.join(", ")),
            ));
        } else if pr.checks == "pending" {
            chips.push((IconKind::Clock, colors.warning, "Checks running".into()));
        }
        let asked: Vec<&str> = pr
            .reviewers
            .iter()
            .filter(|reviewer| reviewer.state == "changes")
            .map(|reviewer| reviewer.name.as_str())
            .collect();
        if !asked.is_empty() {
            chips.push((
                IconKind::Undo,
                colors.error,
                format!("Changes requested by {}", asked.join(", ")),
            ));
        } else if pr.review == "review" {
            chips.push((
                IconKind::Clock,
                colors.text_muted,
                "Waiting for review".into(),
            ));
        }
        if let Some((kind, color, text)) = merge_status(pr) {
            chips.push((kind, color, text));
        }
        if chips.is_empty() {
            return None;
        }
        let mut row = div().row().gap(8.0);
        for (kind, color, text) in chips {
            row = row.child(raised(
                div()
                    .row()
                    .h_px(28.0)
                    .px(10.0)
                    .gap(6.0)
                    .items_center()
                    .child(icon(kind).size(14.0).color(color))
                    .child(label(text).size(12.0).color(colors.text)),
                6.0,
            ));
        }
        Some(row.into())
    }

    fn tabs(&self, checks: usize) -> Node {
        let colors = theme();
        let tab = |id: u64, text: &str, count: Option<usize>, active: bool| {
            let mut segment = div()
                .row()
                .h_px(23.0)
                .px(12.0)
                .gap(6.0)
                .items_center()
                .rounded(4.0)
                .on_click(id)
                .child(label(text.to_string()).size(12.0).color(if active {
                    colors.text
                } else {
                    colors.text_muted
                }));
            if let Some(count) = count {
                segment = segment.child(
                    label(count.to_string())
                        .size(12.0)
                        .color(colors.text_placeholder),
                );
            }
            if active {
                raised(segment, 4.0)
            } else {
                segment.into()
            }
        };
        div()
            .row()
            .child(
                div()
                    .row()
                    .p(2.0)
                    .gap(2.0)
                    .rounded(6.0)
                    .bg(colors.background)
                    .child(tab(
                        OVERVIEW_TAB,
                        "Overview",
                        None,
                        self.tab == Tab::Overview,
                    ))
                    .child(tab(
                        CHECKS_TAB,
                        "Checks",
                        Some(checks),
                        self.tab == Tab::Checks,
                    )),
            )
            .into()
    }

    fn section(title: &str) -> Node {
        div()
            .row()
            .h_px(14.0)
            .items_center()
            .child(
                label(title.to_uppercase())
                    .size(11.0)
                    .weight(600)
                    .color(theme().text_placeholder),
            )
            .into()
    }

    /// A centered message for a tab with no PR to show: still looking, none yet, or the lookup failed.
    fn state_card(&self, width: f32) -> Node {
        let colors = theme();
        if self.loading.is_some() {
            let column = div().col().gap(12.0).child(
                div()
                    .row()
                    .gap(8.0)
                    .items_center()
                    .child(
                        icon(IconKind::LoadCircle)
                            .size(14.0)
                            .color(colors.text_muted),
                    )
                    .child(
                        label(format!("Looking up {} on GitHub...", self.target.head))
                            .size(12.0)
                            .color(colors.text_muted),
                    ),
            );
            let mut lines = div().col().gap(10.0).w_px(420.0).py(4.0);
            for share in [0.6, 0.9, 0.4, 0.75, 0.55] {
                lines = lines.child(
                    div()
                        .w_px(420.0 * share)
                        .h_px(10.0)
                        .rounded(5.0)
                        .bg(colors.element_hover),
                );
            }
            return column.child(lines).into();
        }
        let card_width = width.min(520.0);
        let (kind, color, title, hint) = match &self.error {
            Some(error) => (
                IconKind::Warning,
                colors.error,
                error.clone(),
                "The rest of the Git panel keeps working without it.".to_string(),
            ),
            None => (
                IconKind::PullRequest,
                colors.text,
                format!("No pull request for {} yet", self.target.head),
                "Open one on GitHub; it shows up here on the next refresh.".to_string(),
            ),
        };
        let mut actions = div().row().gap(8.0).items_center();
        if self.error.is_none() {
            actions = actions.child(
                div()
                    .row()
                    .h_px(30.0)
                    .px(8.0)
                    .gap(6.0)
                    .items_center()
                    .rounded(6.0)
                    .border(1.0, colors.text_accent.alpha(0.4))
                    .on_click(CREATE_PR)
                    .child(icon(IconKind::Plus).size(12.0).color(colors.text_accent))
                    .child(
                        label("Create Pull Request")
                            .size(13.0)
                            .color(colors.text_accent),
                    ),
            );
        }
        actions = actions.child(Self::header_button(REFRESH, IconKind::RotateCw, "Refresh"));
        div()
            .row()
            .child(
                div()
                    .col()
                    .w_px(card_width)
                    .p(18.0)
                    .gap(8.0)
                    .rounded(10.0)
                    .bg(colors.panel_background)
                    .child(icon(kind).size(20.0).color(color))
                    .child(
                        label(title)
                            .size(13.0)
                            .weight(600)
                            .color(colors.text)
                            .wrap(card_width - 36.0),
                    )
                    .child(
                        label(hint)
                            .size(13.0)
                            .color(colors.text_placeholder)
                            .wrap(card_width - 36.0),
                    )
                    .child(actions),
            )
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
        Some(parsed.render(width, BODY_STYLE, LINK_BASE))
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
                        COMMENT_STYLE,
                        CONVERSATION_LINK_BASE + index as u64 * CONVERSATION_LINK_STRIDE,
                    )
                })
                .unwrap_or_else(|| div().into())
        };
        let mut column = div().col().gap(8.0);
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
                .gap(6.0)
                .px(12.0)
                .py(10.0)
                .rounded(10.0)
                .bg(colors.panel_background)
                .child(
                    div()
                        .row()
                        .h_px(16.0)
                        .gap(8.0)
                        .items_center()
                        .child(
                            label(item.author.clone())
                                .size(12.0)
                                .weight(600)
                                .color(colors.text),
                        )
                        .child(label(verb).size(12.0).color(color))
                        .child(
                            label(short_time(&item.at))
                                .size(12.0)
                                .color(colors.text_placeholder),
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
                let mut head = div().row().h_px(16.0).gap(10.0).items_center().child(
                    label(place)
                        .size(12.0)
                        .mono()
                        .color(colors.text_muted)
                        .truncate_start(),
                );
                if thread.resolved {
                    head = head.child(label("Resolved").size(12.0).mono().color(colors.success));
                }
                let mut thread_box = div()
                    .col()
                    .gap(6.0)
                    .px(8.0)
                    .py(6.0)
                    .rounded(4.0)
                    .bg(colors.editor_background)
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
                                            .size(12.0)
                                            .color(colors.text_placeholder),
                                    ),
                            )
                            .child(render_next(bodies, card_width - 2.0 * 8.0)),
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
        let section = |title: &str, body: Node| -> Node {
            div()
                .col()
                .gap(6.0)
                .child(Self::section(title))
                .child(body)
                .into()
        };
        let muted = |text: &str| -> Node {
            label(text.to_string())
                .size(12.0)
                .color(colors.text_placeholder)
                .into()
        };
        let mut column = div().col().gap(14.0);
        if let Some((_, _, text)) = merge_status(pr) {
            column = column.child(section(
                "Merge",
                label(text).size(12.0).color(colors.text_muted).into(),
            ));
        }
        let reviewers: Node = if pr.reviewers.is_empty() {
            muted("No reviewers")
        } else {
            let mut list = div().col().gap(6.0);
            for reviewer in &pr.reviewers {
                let (text, color) = match reviewer.state.as_str() {
                    "approved" => ("approved", colors.success),
                    "changes" => ("requested changes", colors.error),
                    "pending" => ("review requested", colors.text_placeholder),
                    _ => ("commented", colors.text_placeholder),
                };
                let initial: String = reviewer
                    .name
                    .chars()
                    .take(1)
                    .flat_map(char::to_uppercase)
                    .collect();
                list = list.child(
                    div()
                        .row()
                        .gap(8.0)
                        .items_center()
                        .child(
                            div()
                                .w_px(18.0)
                                .h_px(18.0)
                                .rounded(9.0)
                                .items_center()
                                .justify_center()
                                .bg(colors.element_hover)
                                .child(
                                    label(initial)
                                        .size(10.0)
                                        .weight(600)
                                        .color(colors.text_muted),
                                ),
                        )
                        .child(label(reviewer.name.clone()).size(12.0).color(colors.text))
                        .child(label(text).size(12.0).color(color)),
                );
            }
            list.into()
        };
        column = column.child(section("Reviewers", reviewers));
        if !pr.labels.is_empty() {
            let mut chips = div().row().gap(4.0);
            for tag in &pr.labels {
                let color = hex_color(&tag.color).unwrap_or(colors.text_muted);
                chips = chips.child(
                    div()
                        .row()
                        .h_px(20.0)
                        .px(8.0)
                        .items_center()
                        .rounded(10.0)
                        .bg(color.alpha(0.2))
                        .child(label(tag.name.clone()).size(12.0).weight(500).color(color)),
                );
            }
            column = column.child(section("Labels", chips.into()));
        }
        column = column.child(section(
            "Description",
            description.unwrap_or_else(|| muted("No description.")),
        ));
        if let Some(conversation) = conversation {
            column = column.child(section(
                &format!("Conversation {}", pr.timeline.len()),
                conversation,
            ));
        }
        column.into()
    }

    fn checks(pr: &PullRequest) -> Node {
        let colors = theme();
        let mut column = div().col().gap(2.0);
        if pr.status_check_rollup.is_empty() {
            return column
                .child(label("No checks").size(12.0).color(colors.text_placeholder))
                .into();
        }
        for (index, check) in pr.status_check_rollup.iter().enumerate() {
            let (kind, color) = match check.result.as_str() {
                "pass" => (IconKind::BadgeCheck, colors.success),
                "fail" => (IconKind::XCircle, colors.error),
                "pending" => (IconKind::Clock, colors.warning),
                _ => (IconKind::Dash, colors.text_disabled),
            };
            let mut row = div()
                .row()
                .h_px(38.0)
                .px(12.0)
                .gap(10.0)
                .items_center()
                .rounded(6.0)
                .bg(colors.panel_background)
                .child(icon(kind).size(14.0).color(color))
                .child(
                    label(check.name.clone())
                        .size(12.0)
                        .weight(500)
                        .color(colors.text),
                );
            if !check.workflow_name.is_empty() {
                row = row.child(
                    label(check.workflow_name.clone())
                        .size(12.0)
                        .color(colors.text_placeholder),
                );
            }
            row = row.child(div().row().flex(1.0));
            let took = if check.result == "pending" {
                Some("running".to_string())
            } else {
                duration(&check.started_at, &check.completed_at)
            };
            if let Some(took) = took {
                row = row.child(label(took).size(12.0).color(colors.text_placeholder));
            }
            if !check.details_url.is_empty() {
                row = row.on_click(CHECK_BASE + index as u64).child(
                    icon(IconKind::ArrowUpRight)
                        .size(12.0)
                        .color(colors.text_placeholder),
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
            CREATE_PR => {
                let url = format!(
                    "https://github.com/{}/{}/compare/{}?expand=1",
                    self.target.owner, self.target.name, self.target.head
                );
                self.open(&url);
            }
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

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `2026-09-17T03:53:12Z` as `Sep 17 03:53`.
fn short_time(stamp: &str) -> String {
    let month = stamp
        .get(5..7)
        .and_then(|month| month.parse::<usize>().ok())
        .and_then(|month| MONTHS.get(month.wrapping_sub(1)));
    let day = stamp.get(8..10).and_then(|day| day.parse::<u32>().ok());
    match (month, day, stamp.get(11..16)) {
        (Some(month), Some(day), Some(time)) if stamp.as_bytes().get(10) == Some(&b'T') => {
            format!("{month} {day} {time}")
        }
        _ => stamp.to_string(),
    }
}

/// Seconds since the epoch for `2026-09-17T03:53:12Z`; days from the civil calendar, so no date crate is needed.
fn epoch_seconds(stamp: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + day_of_year;
    let days = era * 146_097 + of_cycle - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn duration(started: &str, completed: &str) -> Option<String> {
    let seconds = epoch_seconds(completed)? - epoch_seconds(started)?;
    if seconds < 0 {
        return None;
    }
    Some(if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    })
}

/// A card lifted off its layer: a 1px ring and a darker edge under it stand in for a drop shadow.
fn raised(content: ui::Div, radius: f32) -> Node {
    let colors = theme();
    let shadow = match colors.appearance {
        ui::Appearance::Dark => Rgba::new(0.0, 0.0, 0.0, 0.35),
        ui::Appearance::Light => Rgba::new(0.11, 0.09, 0.19, 0.1),
    };
    div()
        .col()
        .pb(1.0)
        .rounded(radius + 1.0)
        .bg(shadow)
        .child(
            div()
                .col()
                .p(1.0)
                .rounded(radius + 1.0)
                .bg(colors.border_variant)
                .child(content.rounded(radius).bg(colors.element_background)),
        )
        .into()
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
        let content_w = width.min(MAX_CONTENT_W);
        let mut column = div().col().w_px(body.w / scale).px(24.0).py(18.0).gap(12.0);
        if self.pr.is_none() {
            column = column.child(self.state_card(width));
        } else {
            column = column.child(self.header(width));
        }
        let (description, conversation) = match self.tab {
            Tab::Overview => (self.description(content_w), self.conversation(content_w)),
            Tab::Checks => (None, None),
        };
        if let Some(pr) = &self.pr {
            if let Some(blockers) = Self::blockers(pr) {
                column = column.child(blockers);
            }
            column = column
                .child(self.tabs(checks))
                .child(div().col().w_px(content_w).child(match self.tab {
                    Tab::Overview => Self::overview(pr, description, conversation),
                    Tab::Checks => Self::checks(pr),
                }));
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

/// Whether the PR can merge into its base, as the forge reports it; `None` while that is still unknown.
fn merge_status(pr: &PullRequest) -> Option<(IconKind, Rgba, String)> {
    if pr.state != "OPEN" {
        return None;
    }
    let colors = theme();
    let base = &pr.base_ref_name;
    if pr.conflict {
        return Some((
            IconKind::Warning,
            colors.error,
            format!("This branch has conflicts with {base} that must be resolved"),
        ));
    }
    match pr.merge_state_status.to_ascii_uppercase().as_str() {
        "BEHIND" => Some((
            IconKind::ArrowDown,
            colors.warning,
            format!("This branch is out of date with {base}"),
        )),
        _ if pr.mergeable.eq_ignore_ascii_case("MERGEABLE") => Some((
            IconKind::Check,
            colors.success,
            format!("No conflicts with {base}"),
        )),
        _ => None,
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
            "CONVERSATION 2",
            "requested changes",
            "query",
            "src/db.rs:12",
            "Resolved",
            "N+1 here",
            "ann",
            "Sep 2 10:00",
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
    fn the_state_pill_and_blockers_follow_the_pull_request() {
        let temp = tempfile::tempdir().expect("temp");
        let texts = |pr: PullRequest| -> Vec<String> {
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
            item.show(Some(pr));
            item.paint_body(Rect::new(0.0, 0.0, 900.0, 1200.0, Rgba::TRANSPARENT), true)
                .expect("painted")
                .texts
                .iter()
                .map(|text| text.text.clone())
                .collect()
        };
        let open = |merge: &str, mergeable: &str| PullRequest {
            number: 7,
            state: "OPEN".into(),
            mergeable: mergeable.into(),
            merge_state_status: merge.into(),
            base_ref_name: "main".into(),
            ..PullRequest::default()
        };
        let conflict = texts(PullRequest {
            conflict: true,
            ..open("DIRTY", "CONFLICTING")
        });
        assert!(conflict.iter().any(|text| text == "Open"));
        assert!(
            conflict.iter().any(|text| text.contains("conflicts")),
            "{conflict:?}"
        );
        let behind = texts(open("BEHIND", "MERGEABLE"));
        assert!(
            behind.iter().any(|text| text.contains("out of date")),
            "{behind:?}"
        );
        let draft = texts(PullRequest {
            is_draft: true,
            ..open("CLEAN", "MERGEABLE")
        });
        assert!(draft.iter().any(|text| text == "Draft"));
        for (state, pill) in [("MERGED", "Merged"), ("CLOSED", "Closed")] {
            let shown = texts(PullRequest {
                state: state.into(),
                ..open("CLEAN", "MERGEABLE")
            });
            assert!(shown.iter().any(|text| text == pill), "{shown:?}");
            assert!(!shown.iter().any(|text| text.contains("conflicts")));
        }
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
