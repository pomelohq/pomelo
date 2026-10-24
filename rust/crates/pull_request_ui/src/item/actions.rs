//! What the tab changes on GitHub: merging, review threads (reply, resolve) and submitting a review. Each runs
//! on a thread of its own; replies, resolves and reviews show at once and are undone by a refresh if GitHub
//! refuses them.

use std::sync::mpsc;

use pom_forge::{MergeMethod, Mutation, PullRequest, ReviewEvent, ThreadComment, TimelineItem};
use terminal::Keystroke;
use ui::{div, icon, label, theme, IconKind, Node};
use workspace::{EditKey, PanelRequest, TerminalKeyOutcome};

use super::{raised, PrItem};

pub(super) const MERGE: u64 = 10;
pub(super) const MERGE_METHODS: u64 = 11;
pub(super) const MERGE_CANCEL: u64 = 12;
pub(super) const METHOD_BASE: u64 = 20;
pub(super) const REVIEW_FIELD: u64 = 30;
pub(super) const REVIEW_EVENT_BASE: u64 = 31;
/// Each review thread gets `THREAD_STRIDE` ids from here: its reply field, send, and resolve toggle.
pub(super) const THREAD_BASE: u64 = 500_000;
const THREAD_STRIDE: u64 = 4;
const REPLY_FIELD: u64 = 0;
const SEND_REPLY: u64 = 1;
const TOGGLE_RESOLVED: u64 = 2;

const REVIEW_EVENTS: [ReviewEvent; 3] = [
    ReviewEvent::Comment,
    ReviewEvent::RequestChanges,
    ReviewEvent::Approve,
];
const NO_TOKEN: &str = "No GitHub token: set GH_TOKEN or the session's github secret";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Reply(String),
    Review,
}

/// One change in flight: what to say when it lands, and what to call it if it fails.
pub(super) struct Pending {
    pub answer: mpsc::Receiver<Result<(), String>>,
    pub done: String,
    pub failed: &'static str,
    pub merging: bool,
}

/// "Checks are failing", "Changes were requested" read as one sentence.
fn sentence(parts: &[String]) -> String {
    parts
        .iter()
        .enumerate()
        .map(|(index, part)| {
            if index == 0 {
                return part.clone();
            }
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_lowercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a ready PR has going for it: passing checks, its approvals, a clean merge.
fn ready_summary(pr: &PullRequest) -> String {
    let mut parts = Vec::new();
    if pr.checks == "pass" {
        parts.push("Checks passing".to_string());
    }
    let approvers: Vec<&str> = pr
        .reviewers
        .iter()
        .filter(|reviewer| reviewer.state == "approved")
        .map(|reviewer| reviewer.name.as_str())
        .collect();
    if !approvers.is_empty() {
        parts.push(format!("Approved by {}", approvers.join(", ")));
    }
    if pr.mergeable.eq_ignore_ascii_case("MERGEABLE") {
        parts.push(format!("No conflicts with {}", pr.base_ref_name));
    }
    if parts.is_empty() {
        parts.push(format!(
            "Merges {} into {}",
            pr.head_ref_name, pr.base_ref_name
        ));
    }
    sentence(&parts)
}

impl PrItem {
    fn run(&mut self, mutation: Mutation, done: String, failed: &'static str, merging: bool) {
        if self.pending.is_some() {
            return;
        }
        let (sender, answer) = mpsc::channel();
        let (connect, waker) = (self.connect.clone(), self.waker.clone());
        let spawned = std::thread::Builder::new()
            .name("pull-request-change".into())
            .spawn(move || {
                let result = match connect() {
                    Some(client) => client
                        .mutate(&mutation)
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    None => Err(NO_TOKEN.into()),
                };
                if sender.send(result).is_err() {
                    eprintln!("pull request: the tab closed before GitHub answered");
                }
                waker();
            });
        match spawned {
            Ok(_) => {
                self.pending = Some(Pending {
                    answer,
                    done,
                    failed,
                    merging,
                })
            }
            Err(error) => self
                .requests
                .push(PanelRequest::Toast(format!("{failed}: {error}"))),
        }
    }

    /// Whether a change finished; it reports with a toast and reloads the PR either way.
    pub(super) fn poll_pending(&mut self) -> bool {
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        let result = match pending.answer.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("the request stopped".into()),
        };
        let Some(pending) = self.pending.take() else {
            return false;
        };
        self.requests.push(PanelRequest::Toast(match result {
            Ok(()) => pending.done,
            Err(error) => format!("{}: {error}", pending.failed),
        }));
        self.load();
        true
    }

    /// The merge section after Merge was pressed once, asking to confirm; for the snapshot tool.
    pub fn confirming_merge_box(&mut self, width: f32) -> Option<Node> {
        self.confirm_merge = true;
        let pr = self.pr.clone()?;
        self.merge_box(&pr, width)
    }

    pub(super) fn merge_box(&self, pr: &PullRequest, width: f32) -> Option<Node> {
        if pr.state != "OPEN" {
            return None;
        }
        let colors = theme();
        let blockers = pom_forge::merge_blockers(pr);
        let merging = self.pending.as_ref().is_some_and(|pending| pending.merging);
        let ready = blockers.is_empty();
        let method = self.merge_method;
        let (kind, color, title, detail) = if merging {
            (
                IconKind::LoadCircle,
                colors.text_muted,
                "Merging...".to_string(),
                format!("{} into {}", method.label(), pr.base_ref_name),
            )
        } else if self.confirm_merge && ready {
            (
                IconKind::Warning,
                colors.warning,
                format!("Merge #{} into {}?", pr.number, pr.base_ref_name),
                format!(
                    "{} on GitHub. This cannot be undone from here.",
                    method.label()
                ),
            )
        } else if ready {
            (
                IconKind::BadgeCheck,
                colors.success,
                "Ready to merge".to_string(),
                ready_summary(pr),
            )
        } else {
            (
                IconKind::XCircle,
                colors.error,
                "Merging is blocked".to_string(),
                sentence(&blockers),
            )
        };
        let enabled = ready && !merging && self.pending.is_none();
        let (fill, ink) = if enabled {
            (colors.success, ui::Rgba::new(1.0, 1.0, 1.0, 1.0))
        } else {
            (colors.element_background, colors.text_disabled)
        };
        let text = if self.confirm_merge && enabled {
            format!("Confirm {}", method.label().to_lowercase())
        } else {
            method.label().to_string()
        };
        let mut main = div()
            .row()
            .h_px(30.0)
            .pl(12.0)
            .pr(4.0)
            .items_center()
            .child(label(text).size(12.0).weight(600).color(ink));
        let mut chevron = div()
            .row()
            .h_px(30.0)
            .pl(4.0)
            .pr(12.0)
            .items_center()
            .child(icon(IconKind::ChevronDown).size(12.0).color(ink));
        if enabled {
            main = main.on_click(MERGE);
            chevron = chevron.on_click(MERGE_METHODS);
        }
        let mut actions = div().row().gap(8.0).items_center();
        if self.confirm_merge && enabled {
            actions = actions.child(ui::button(MERGE_CANCEL, "Cancel"));
        }
        actions = actions.child(
            div()
                .row()
                .items_center()
                .rounded(6.0)
                .bg(fill)
                .child(main)
                .child(chevron),
        );
        let text_w = (width - 300.0).max(160.0);
        let row = div()
            .row()
            .w_px(width)
            .px(16.0)
            .py(12.0)
            .gap(12.0)
            .items_center()
            .child(icon(kind).size(16.0).color(color))
            .child(
                div()
                    .col()
                    .flex(1.0)
                    .child(
                        div()
                            .row()
                            .h_px(16.0)
                            .items_center()
                            .child(label(title).size(12.0).weight(600).color(colors.text)),
                    )
                    .child({
                        let detail_label = label(detail.clone())
                            .size(12.0)
                            .color(colors.text_placeholder);
                        // One line keeps the 16px rhythm of the title above; a longer one wraps.
                        if ui::measure_text_width(&detail, 12.0, false, ui::ui_font_weight())
                            / ui::ui_text_scale()
                            <= text_w
                        {
                            Node::from(div().row().h_px(16.0).items_center().child(detail_label))
                        } else {
                            detail_label.wrap(text_w).into()
                        }
                    }),
            )
            .child(actions);
        let mut column = div().col().gap(6.0).child(raised(row, 10.0));
        if self.merge_menu && enabled {
            let mut menu = div().col().p(4.0).w_px(220.0);
            for (index, choice) in MergeMethod::ALL.iter().enumerate() {
                let mark: Node = if *choice == method {
                    icon(IconKind::Check)
                        .size(12.0)
                        .color(colors.text_muted)
                        .into()
                } else {
                    div().w_px(12.0).into()
                };
                menu = menu.child(
                    div()
                        .row()
                        .h_px(28.0)
                        .px(8.0)
                        .gap(8.0)
                        .items_center()
                        .rounded(6.0)
                        .on_click(METHOD_BASE + index as u64)
                        .child(mark)
                        .child(label(choice.label()).size(12.0).color(colors.text)),
                );
            }
            column = column.child(div().row().justify_end().child(raised(menu, 8.0)));
        }
        Some(column.into())
    }

    /// The reply field and resolve toggle under review thread `ordinal` (counted across the whole timeline).
    pub(super) fn thread_controls(
        &self,
        ordinal: usize,
        id: &str,
        resolved: bool,
        width: f32,
    ) -> Node {
        let colors = theme();
        let base = THREAD_BASE + ordinal as u64 * THREAD_STRIDE;
        let focused = self.focus.as_ref() == Some(&Field::Reply(id.to_string()));
        let field: Node = if focused {
            self.reply.render(
                "Reply...",
                true,
                colors.text,
                24.0,
                workspace::text_field::FieldFont::Ui,
            )
        } else {
            label("Reply...")
                .size(12.0)
                .color(colors.text_placeholder)
                .into()
        };
        let busy = self.pending.is_some();
        let mut row = div()
            .row()
            .w_px(width)
            .h_px(36.0)
            .px(8.0)
            .gap(8.0)
            .items_center()
            .rounded(6.0)
            .bg(colors.editor_background)
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .h_px(24.0)
                    .items_center()
                    .on_click(base + REPLY_FIELD)
                    .child(field),
            );
        if focused && !self.reply.text().trim().is_empty() {
            row = row.child(
                ui::button(base + SEND_REPLY, "Reply")
                    .kind(ui::ButtonKind::Primary)
                    .enabled(!busy),
            );
        }
        let toggle = if resolved {
            "Unresolve thread"
        } else {
            "Resolve thread"
        };
        row.child(ui::button(base + TOGGLE_RESOLVED, toggle).enabled(!busy))
            .into()
    }

    pub(super) fn review_box(&mut self, width: f32) -> Option<Node> {
        if self
            .pr
            .as_ref()
            .is_none_or(|pr| pr.state != "OPEN" || pr.id.is_empty())
        {
            return None;
        }
        let colors = theme();
        let focused = self.focus == Some(Field::Review);
        let busy = self.pending.is_some();
        let field = self.review.render(
            "Leave a review",
            focused,
            width - 2.0 * 12.0 - 2.0 * 10.0,
            4,
        );
        let mut buttons = div().row().gap(8.0).items_center();
        for (index, event) in REVIEW_EVENTS.iter().enumerate() {
            buttons = buttons.child(
                ui::button(REVIEW_EVENT_BASE + index as u64, event.label())
                    .kind(if *event == ReviewEvent::Approve {
                        ui::ButtonKind::Primary
                    } else {
                        ui::ButtonKind::Secondary
                    })
                    .enabled(!busy),
            );
        }
        Some(
            div()
                .col()
                .w_px(width)
                .p(12.0)
                .gap(8.0)
                .rounded(10.0)
                .bg(colors.panel_background)
                .child(
                    div()
                        .col()
                        .px(10.0)
                        .py(8.0)
                        .rounded(6.0)
                        .bg(colors.panel_background)
                        .border(
                            1.0,
                            if focused {
                                colors.text_accent
                            } else {
                                colors.border_variant
                            },
                        )
                        .on_click(REVIEW_FIELD)
                        .child(field),
                )
                .child(buttons)
                .into(),
        )
    }

    /// A click on a merge, thread or review control; returns whether `id` was one.
    pub(super) fn click_action(&mut self, id: u64) -> bool {
        match id {
            MERGE => {
                if self.confirm_merge {
                    self.confirm_merge = false;
                    self.merge();
                } else {
                    self.confirm_merge = true;
                    self.merge_menu = false;
                }
            }
            MERGE_METHODS => {
                self.merge_menu = !self.merge_menu;
                self.confirm_merge = false;
            }
            MERGE_CANCEL => self.confirm_merge = false,
            id if (METHOD_BASE..METHOD_BASE + 3).contains(&id) => {
                if let Some(method) = MergeMethod::ALL.get((id - METHOD_BASE) as usize) {
                    self.merge_method = *method;
                }
                self.merge_menu = false;
            }
            REVIEW_FIELD => self.focus = Some(Field::Review),
            id if (REVIEW_EVENT_BASE..REVIEW_EVENT_BASE + 3).contains(&id) => {
                if let Some(event) = REVIEW_EVENTS.get((id - REVIEW_EVENT_BASE) as usize) {
                    self.submit_review(*event);
                }
            }
            id if (THREAD_BASE..super::LINK_BASE).contains(&id) => {
                let offset = id - THREAD_BASE;
                let Some((thread_id, resolved)) = self.thread_at((offset / THREAD_STRIDE) as usize)
                else {
                    return true;
                };
                match offset % THREAD_STRIDE {
                    REPLY_FIELD => {
                        if self.focus != Some(Field::Reply(thread_id.clone())) {
                            self.reply = Default::default();
                            self.reply.set_font_size(12.0);
                        }
                        self.focus = Some(Field::Reply(thread_id));
                    }
                    SEND_REPLY => self.send_reply(&thread_id),
                    TOGGLE_RESOLVED => self.toggle_resolved(&thread_id, !resolved),
                    _ => {}
                }
            }
            _ => return false,
        }
        true
    }

    fn thread_at(&self, ordinal: usize) -> Option<(String, bool)> {
        self.pr
            .as_ref()?
            .timeline
            .iter()
            .flat_map(|item| &item.threads)
            .filter(|thread| !thread.id.is_empty())
            .nth(ordinal)
            .map(|thread| (thread.id.clone(), thread.resolved))
    }

    fn merge(&mut self) {
        let Some(pr) = self.pr.as_ref() else {
            return;
        };
        if !pom_forge::merge_blockers(pr).is_empty() {
            return;
        }
        let done = format!("Merged #{} into {}", pr.number, pr.base_ref_name);
        let mutation = pom_forge::merge(pr, self.merge_method);
        self.run(mutation, done, "Could not merge", true);
    }

    fn send_reply(&mut self, thread_id: &str) {
        let body = self.reply.text().trim().to_string();
        if body.is_empty() || self.pending.is_some() {
            return;
        }
        if let Some(thread) = self.thread_mut(thread_id) {
            thread.comments.push(ThreadComment {
                author: "You".into(),
                body: body.clone(),
                at: String::new(),
            });
        }
        self.reply = Default::default();
        self.focus = None;
        self.run(
            pom_forge::reply(thread_id, &body),
            "Reply posted".into(),
            "Could not reply",
            false,
        );
    }

    fn toggle_resolved(&mut self, thread_id: &str, resolved: bool) {
        if self.pending.is_some() {
            return;
        }
        if let Some(thread) = self.thread_mut(thread_id) {
            thread.resolved = resolved;
        }
        let (done, failed) = if resolved {
            ("Thread resolved", "Could not resolve the thread")
        } else {
            ("Thread reopened", "Could not reopen the thread")
        };
        self.run(
            pom_forge::set_resolved(thread_id, resolved),
            done.into(),
            failed,
            false,
        );
    }

    fn submit_review(&mut self, event: ReviewEvent) {
        let body = self.review.text().trim().to_string();
        if self.pending.is_some() {
            return;
        }
        if body.is_empty() && event != ReviewEvent::Approve {
            self.requests.push(PanelRequest::Toast(format!(
                "Write something to {}",
                event.label().to_lowercase()
            )));
            self.focus = Some(Field::Review);
            return;
        }
        let Some(pr) = self.pr.as_mut() else {
            return;
        };
        let mutation = pom_forge::submit_review(pr, event, &body);
        pr.timeline.push(TimelineItem {
            kind: pom_forge::TimelineKind::Review,
            author: "You".into(),
            body,
            at: String::new(),
            state: match event {
                ReviewEvent::Comment => "COMMENTED",
                ReviewEvent::RequestChanges => "CHANGES_REQUESTED",
                ReviewEvent::Approve => "APPROVED",
            }
            .into(),
            threads: Vec::new(),
        });
        self.review = Default::default();
        self.review.set_font_size(12.0);
        self.focus = None;
        self.run(
            mutation,
            "Review submitted".into(),
            "Could not submit the review",
            false,
        );
    }

    fn thread_mut(&mut self, thread_id: &str) -> Option<&mut pom_forge::ReviewThread> {
        self.pr
            .as_mut()?
            .timeline
            .iter_mut()
            .flat_map(|item| item.threads.iter_mut())
            .find(|thread| thread.id == thread_id)
    }

    pub(super) fn field_keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let Some(focus) = self.focus.clone() else {
            return TerminalKeyOutcome::Ignored;
        };
        let terminal::Modifiers {
            shift, alt, cmd, ..
        } = keystroke.modifiers;
        let key = match keystroke.key.as_str() {
            "escape" => {
                self.focus = None;
                return TerminalKeyOutcome::Handled;
            }
            "enter" => match &focus {
                Field::Reply(thread_id) => {
                    self.send_reply(thread_id);
                    return TerminalKeyOutcome::Handled;
                }
                Field::Review if cmd => {
                    self.submit_review(ReviewEvent::Comment);
                    return TerminalKeyOutcome::Handled;
                }
                Field::Review => EditKey::Enter,
            },
            "v" if cmd => return TerminalKeyOutcome::Paste,
            "left" if cmd => EditKey::Home,
            "right" if cmd => EditKey::End,
            "left" if alt => EditKey::WordLeft,
            "right" if alt => EditKey::WordRight,
            "left" => EditKey::Left,
            "right" => EditKey::Right,
            "up" => EditKey::Up,
            "down" => EditKey::Down,
            "backspace" if cmd => EditKey::DeleteToLineStart,
            "backspace" if alt => EditKey::DeleteWordLeft,
            "backspace" => EditKey::Backspace,
            "delete" => EditKey::Delete,
            "a" if cmd => EditKey::SelectAll,
            "z" if cmd && shift => EditKey::Redo,
            "z" if cmd => EditKey::Undo,
            _ => return TerminalKeyOutcome::Ignored,
        };
        match focus {
            Field::Reply(_) => self.reply.key(key, shift),
            Field::Review => self.review.key(key, shift),
        };
        TerminalKeyOutcome::Handled
    }

    pub(super) fn field_text(&mut self, text: &str) {
        match self.focus {
            Some(Field::Reply(_)) => {
                let line: String = text.chars().filter(|c| !c.is_control()).collect();
                self.reply.insert(&line);
            }
            Some(Field::Review) => {
                let typed: String = text
                    .chars()
                    .filter(|c| *c == '\n' || !c.is_control())
                    .collect();
                self.review.insert(&typed);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use pom_forge::{Client, ForgeError, PrTarget, Response, Transport};
    use pom_paths::StateDir;
    use workspace::Item;

    /// Answers every change with `answer` and keeps what it was sent.
    struct Recorder {
        sent: Arc<Mutex<Vec<serde_json::Value>>>,
        answer: &'static str,
    }

    impl Transport for Recorder {
        fn post(&self, _url: &str, _token: &str, body: &str) -> Result<Response, ForgeError> {
            if let Ok(mut sent) = self.sent.lock() {
                sent.push(serde_json::from_str(body).unwrap_or_default());
            }
            Ok(Response {
                status: 200,
                body: self.answer.to_string(),
                ..Response::default()
            })
        }
    }

    fn item(
        answer: &'static str,
    ) -> (
        PrItem,
        Arc<Mutex<Vec<serde_json::Value>>>,
        tempfile::TempDir,
    ) {
        let temp = tempfile::tempdir().expect("temp");
        let mut item = PrItem::new(
            StateDir::new(temp.path()),
            "myproject".into(),
            Arc::new(|| {}),
            PrTarget {
                repo: "api".into(),
                owner: "acme".into(),
                name: "api".into(),
                head: "feat-login".into(),
            },
            None,
        );
        let sent = Arc::new(Mutex::new(Vec::new()));
        let recorder = sent.clone();
        item.connect = Arc::new(move || {
            Some(Client::new(
                "t",
                Box::new(Recorder {
                    sent: recorder.clone(),
                    answer,
                }),
            ))
        });
        item.show(Some(PullRequest {
            id: "PR_1".into(),
            number: 7,
            state: "OPEN".into(),
            head_ref_oid: "abc".into(),
            base_ref_name: "main".into(),
            checks: "pass".into(),
            review: "approved".into(),
            mergeable: "MERGEABLE".into(),
            timeline: vec![TimelineItem {
                kind: pom_forge::TimelineKind::Review,
                author: "bea".into(),
                state: "CHANGES_REQUESTED".into(),
                threads: vec![pom_forge::ReviewThread {
                    id: "PRRT_1".into(),
                    path: "a.rb".into(),
                    ..pom_forge::ReviewThread::default()
                }],
                ..TimelineItem::default()
            }],
            ..PullRequest::default()
        }));
        (item, sent, temp)
    }

    fn texts(item: &mut PrItem) -> Vec<String> {
        item.paint_body(
            ui::Rect::new(0.0, 0.0, 900.0, 2000.0, ui::Rgba::TRANSPARENT),
            true,
        )
        .expect("painted")
        .texts
        .iter()
        .map(|text| text.text.clone())
        .collect()
    }

    /// Ticks until the change in flight reports, and returns its toast.
    fn landed(item: &mut PrItem) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while item.pending.is_some() && std::time::Instant::now() < deadline {
            item.poll_pending();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match item.take_requests().pop() {
            Some(PanelRequest::Toast(message)) => message,
            _ => String::new(),
        }
    }

    #[test]
    fn merging_asks_to_confirm_then_sends_the_chosen_method() {
        let (mut item, sent, _temp) =
            item(r#"{"data":{"mergePullRequest":{"pullRequest":{"state":"MERGED"}}}}"#);
        assert!(texts(&mut item).iter().any(|text| text == "Ready to merge"));
        assert!(item.click_action(MERGE_METHODS));
        assert!(item.click_action(METHOD_BASE + 2));
        assert!(item.click_action(MERGE));
        assert!(texts(&mut item)
            .iter()
            .any(|text| text == "Confirm rebase and merge"));
        assert!(sent.lock().map(|sent| sent.is_empty()).unwrap_or(false));
        assert!(item.click_action(MERGE));
        assert_eq!(landed(&mut item), "Merged #7 into main");
        let sent = sent.lock().map(|sent| sent.clone()).unwrap_or_default();
        assert_eq!(sent[0]["variables"]["method"], "REBASE");
        assert_eq!(sent[0]["variables"]["head"], "abc");
    }

    #[test]
    fn a_blocked_merge_never_reaches_github() {
        let (mut item, sent, _temp) = item("{}");
        if let Some(pr) = item.pr.as_mut() {
            pr.checks = "fail".into();
        }
        assert!(texts(&mut item)
            .iter()
            .any(|text| text == "Merging is blocked"));
        item.click_action(MERGE);
        item.click_action(MERGE);
        assert!(item.pending.is_none());
        assert!(sent.lock().map(|sent| sent.is_empty()).unwrap_or(false));
    }

    #[test]
    fn a_reply_shows_at_once_and_a_refusal_reports() {
        let (mut item, sent, _temp) =
            item(r#"{"data":null,"errors":[{"message":"Thread is locked"}]}"#);
        assert!(item.click_action(THREAD_BASE + REPLY_FIELD));
        item.field_text("Capped at 100.");
        assert!(texts(&mut item).iter().any(|text| text == "Reply"));
        assert!(item.click_action(THREAD_BASE + SEND_REPLY));
        let comments = &item.pr.as_ref().expect("pr").timeline[0].threads[0].comments;
        assert_eq!(comments[0].body, "Capped at 100.");
        assert_eq!(
            landed(&mut item),
            "Could not reply: GitHub: Thread is locked"
        );
        let sent = sent.lock().map(|sent| sent.clone()).unwrap_or_default();
        assert_eq!(sent[0]["variables"]["thread"], "PRRT_1");
        assert_eq!(sent[0]["variables"]["body"], "Capped at 100.");
    }

    #[test]
    fn resolving_flips_the_thread_and_a_review_needs_words_unless_approving() {
        let (mut item, sent, _temp) = item(r#"{"data":{"ok":true}}"#);
        assert!(item.click_action(THREAD_BASE + TOGGLE_RESOLVED));
        assert!(item.pr.as_ref().expect("pr").timeline[0].threads[0].resolved);
        assert_eq!(landed(&mut item), "Thread resolved");

        assert!(item.click_action(REVIEW_EVENT_BASE + 1));
        assert_eq!(
            landed(&mut item),
            "Write something to request changes",
            "an empty request for changes is not sent"
        );
        assert!(item.click_action(REVIEW_EVENT_BASE + 2));
        assert_eq!(landed(&mut item), "Review submitted");
        let sent = sent.lock().map(|sent| sent.clone()).unwrap_or_default();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["variables"]["event"], "APPROVE");
    }

    #[test]
    fn reasons_read_as_one_sentence() {
        assert_eq!(
            sentence(&["Checks are failing".into(), "Changes were requested".into()]),
            "Checks are failing, changes were requested"
        );
        assert_eq!(sentence(&[]), "");
    }

    #[test]
    fn a_ready_summary_names_checks_approvals_and_the_merge() {
        let pr = PullRequest {
            checks: "pass".into(),
            mergeable: "MERGEABLE".into(),
            base_ref_name: "main".into(),
            reviewers: vec![pom_forge::Reviewer {
                name: "bea".into(),
                state: "approved".into(),
            }],
            ..PullRequest::default()
        };
        assert_eq!(
            ready_summary(&pr),
            "Checks passing, approved by bea, no conflicts with main"
        );
    }
}
