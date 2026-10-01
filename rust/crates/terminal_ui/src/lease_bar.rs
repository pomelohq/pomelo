//! The strip over an agent tab someone else drives: who drives it, Take over, and a tool call waiting for an
//! approval with Allow and Deny. Laid out like the reference's banner (severity icon, message, actions at the
//! end) and its permission row (Allow with a check, Deny with a cross).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use pom_agent::{PendingApproval, SessionState};
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use ui::{div, icon, label, theme, Div, IconKind, Node, Rgba};

const TAKE_OVER: u64 = 0;
const ALLOW: u64 = 1;
const DENY: u64 = 2;
const IDS: u64 = 4;
/// Lease and approval files are read at most this often while the tab paints.
const REFRESH: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
struct Snapshot {
    driver: Option<String>,
    pending: Option<PendingApproval>,
    session: Option<SessionState>,
}

pub struct LeaseBar {
    base: u64,
    holder: String,
    state: StateDir,
    holders: SocketDir,
    cached: RefCell<(Option<Instant>, Snapshot)>,
}

fn faded(color: Rgba, factor: f32) -> Rgba {
    Rgba::new(color.r, color.g, color.b, color.a * factor)
}

impl LeaseBar {
    pub fn new(holder: &str, state: StateDir, holders: SocketDir) -> LeaseBar {
        LeaseBar {
            base: workspace::toolbar_ids(IDS),
            holder: holder.to_string(),
            state,
            holders,
            cached: RefCell::new((None, Snapshot::default())),
        }
    }

    fn snapshot(&self) -> Snapshot {
        let mut cached = self.cached.borrow_mut();
        if cached.0.is_none_or(|at| at.elapsed() >= REFRESH) {
            let session = pom_agent::session_for_holder(&self.state, &self.holder);
            let pending = session
                .as_ref()
                .and_then(|session| pom_agent::pending_approval(&self.state, &session.session_id));
            *cached = (
                Some(Instant::now()),
                Snapshot {
                    driver: pom_agent::driven_by(&self.state, &self.holder),
                    pending,
                    session,
                },
            );
        }
        cached.1.clone()
    }

    fn forget(&self) {
        self.cached.borrow_mut().0 = None;
    }

    fn action(&self, id: u64, icon_kind: Option<(IconKind, Rgba)>, text: &str) -> Div {
        let colors = theme();
        let mut button = div()
            .row()
            .h_px(22.0)
            .px(4.0)
            .gap(4.0)
            .items_center()
            .rounded(4.0)
            .hover_bg(colors.ghost_element_hover)
            .active_bg(colors.ghost_element_active)
            .on_click(self.base + id);
        if let Some((kind, color)) = icon_kind {
            button = button.child(icon(kind).size(12.0).color(color));
        }
        button.child(label(text).size(12.0).color(colors.text))
    }

    fn banner(
        &self,
        kind: IconKind,
        icon_color: Rgba,
        background: Rgba,
        border: Rgba,
        message: String,
        actions: Div,
    ) -> Div {
        let colors = theme();
        div()
            .row()
            .py(2.0)
            .pl(8.0)
            .pr(4.0)
            .gap(6.0)
            .items_center()
            .rounded(4.0)
            .bg(background)
            .border(1.0, border)
            .child(icon(kind).size(12.0).color(icon_color))
            .child(
                div()
                    .flex(1.0)
                    .child(label(message).size(13.0).color(colors.text)),
            )
            .child(actions)
    }

    /// The strip, or nothing while the person at the app drives the session and nothing waits on them.
    pub fn render(&self, width: f32) -> Option<Node> {
        let snapshot = self.snapshot();
        let colors = theme();
        let mut rows = Vec::new();
        if let Some(driver) = &snapshot.driver {
            rows.push(self.banner(
                IconKind::Eye,
                colors.icon_muted,
                faded(colors.info_background, 0.5),
                faded(colors.border, 0.5),
                format!("Watching - {driver} drives this session"),
                self.action(TAKE_OVER, None, "Take over"),
            ));
        }
        if let Some(pending) = &snapshot.pending {
            let actions = div()
                .row()
                .gap(2.0)
                .child(self.action(ALLOW, Some((IconKind::Check, colors.success)), "Allow"))
                .child(self.action(DENY, Some((IconKind::Close, colors.error)), "Deny"));
            rows.push(self.banner(
                IconKind::Warning,
                colors.warning,
                faded(colors.warning_background, 0.5),
                faded(colors.warning_border, 0.4),
                format!("Waiting for approval: {} {}", pending.tool, pending.summary),
                actions,
            ));
        }
        if rows.is_empty() {
            return None;
        }
        let mut strip = div()
            .col()
            .w_px(width)
            .px(8.0)
            .py(6.0)
            .gap(4.0)
            .bg(colors.toolbar_background);
        for row in rows {
            strip = strip.child(row);
        }
        Some(
            div()
                .col()
                .w_px(width)
                .child(strip)
                .child(div().h_px(1.0).bg(colors.border))
                .into(),
        )
    }

    /// A click on one of its buttons; false when the id is not its own.
    pub fn click(&mut self, id: u64) -> bool {
        let Some(offset) = id.checked_sub(self.base).filter(|offset| *offset < IDS) else {
            return false;
        };
        let snapshot = self.snapshot();
        let Some(session) = snapshot.session.as_ref() else {
            return true;
        };
        let outcome = match offset {
            TAKE_OVER => pom_agent::take_over(&self.state, &self.holders, session),
            ALLOW | DENY => match &snapshot.pending {
                Some(pending) => pom_agent::answer_as_person(
                    &self.state,
                    session,
                    &pending.request,
                    offset == ALLOW,
                ),
                None => Ok(()),
            },
            _ => Ok(()),
        };
        if let Err(error) = outcome {
            eprintln!("agent tab: {error}");
        }
        self.forget();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use pom_agent::{record_event, set_lease, AgentState, Caller, Identity, Lease, SessionEvent};

    #[test]
    fn the_strip_shows_while_someone_else_drives_and_take_over_hands_it_back() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path().join("state"));
        let holders = SocketDir::new(temp.path().join("s"));
        let holder = "ws-myproject-feat-login-claude-reviewer";
        let identity = Identity {
            holder: holder.into(),
            role: "reviewer".into(),
            project: "myproject".into(),
            branch: "feat-login".into(),
            driver: "claude".into(),
        };
        let event = SessionEvent {
            session_id: "b".into(),
            event: "SessionStart".into(),
            state: Some(AgentState::Idle),
            ..SessionEvent::default()
        };
        record_event(&state, &identity, &event).expect("event");
        let mut bar = LeaseBar::new(holder, state.clone(), holders.clone());
        assert!(bar.render(600.0).is_none(), "the person drives: no strip");

        set_lease(
            &state,
            &holders,
            "myproject",
            "feat-login",
            holder,
            "reviewer",
            Lease::for_caller(&Caller::Operator),
        )
        .expect("lease");
        bar.forget();
        assert!(bar.render(600.0).is_some());
        let base = bar.base;
        assert!(bar.click(base + TAKE_OVER));
        assert!(bar.render(600.0).is_none(), "taken over");
        assert!(
            holders.input_lease(holder).is_none(),
            "the person's keys reach the agent again"
        );
        assert!(!bar.click(base + IDS), "not its id");
    }
}
