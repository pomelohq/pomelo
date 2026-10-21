//! The strip over an agent tab someone else drives: who drives it, Take over, and a tool call waiting for an
//! approval with Allow and Deny, each a raised card on the panel's tray.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use pom_agent::{PendingApproval, SessionState};
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use ui::{div, icon, label, render_keystroke_in, theme, Appearance, Div, IconKind, Node, Rgba};
use workspace::keymap::{key_hint, Action};

const TAKE_OVER: u64 = 0;
const ALLOW: u64 = 1;
const DENY: u64 = 2;
const IDS: u64 = 4;
/// Lease and approval files are read at most this often while the tab paints.
const REFRESH: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseButton {
    TakeOver,
    Allow,
    Deny,
}

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

/// The approval's actions share its line only when the strip is at least this wide; narrower, they wrap under it.
const ONE_LINE_W: f32 = 612.0;

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

    /// The strip, or nothing while the person at the app drives the session and nothing waits on them.
    pub fn render(&self, width: f32) -> Option<Node> {
        let snapshot = self.snapshot();
        strip(
            self.base,
            snapshot.driver.as_deref(),
            snapshot.pending.as_ref(),
            width,
        )
    }

    /// A click on one of its buttons; false when the id is not its own.
    pub fn click(&mut self, id: u64) -> bool {
        let Some(offset) = id.checked_sub(self.base).filter(|offset| *offset < IDS) else {
            return false;
        };
        let button = match offset {
            TAKE_OVER => LeaseButton::TakeOver,
            ALLOW => LeaseButton::Allow,
            DENY => LeaseButton::Deny,
            _ => return true,
        };
        self.press(button);
        true
    }

    /// Takes the session over, or answers the tool call waiting on the person; false when the strip does not
    /// offer it right now.
    pub fn press(&mut self, button: LeaseButton) -> bool {
        let snapshot = self.snapshot();
        let Some(session) = snapshot.session.as_ref() else {
            return false;
        };
        let outcome = match (button, &snapshot.pending) {
            (LeaseButton::TakeOver, _) if snapshot.driver.is_some() => {
                pom_agent::take_over(&self.state, &self.holders, session)
            }
            (LeaseButton::Allow | LeaseButton::Deny, Some(pending)) => pom_agent::answer_as_person(
                &self.state,
                session,
                &pending.request,
                button == LeaseButton::Allow,
            ),
            _ => return false,
        };
        if let Err(error) = outcome {
            eprintln!("agent tab: {error}");
        }
        self.forget();
        true
    }
}

/// The strip a session shows for `driver` and `pending`, its buttons' click ids counted from `base`.
pub fn strip(
    base: u64,
    driver: Option<&str>,
    pending: Option<&PendingApproval>,
    width: f32,
) -> Option<Node> {
    let colors = theme();
    let mut cards = Vec::new();
    if let Some(driver) = driver {
        cards.push(
            div()
                .row()
                .h_px(36.0)
                .pl(12.0)
                .pr(6.0)
                .gap(8.0)
                .items_center()
                .child(
                    icon(IconKind::Eye)
                        .size(14.0)
                        .color(colors.text_placeholder),
                )
                .child(
                    div().row().flex(1.0).child(
                        label(format!("Watching - {driver} drives this session"))
                            .size(12.0)
                            .color(colors.text)
                            .truncate(),
                    ),
                )
                .child(button(base + TAKE_OVER, None, "Take over", None, false)),
        );
    }
    if let Some(pending) = pending {
        let actions = div()
            .row()
            .gap(6.0)
            .child(button(
                base + ALLOW,
                Some(IconKind::Check),
                "Allow",
                key_hint(Action::AgentAllow),
                true,
            ))
            .child(button(
                base + DENY,
                Some(IconKind::Close),
                "Deny",
                key_hint(Action::AgentDeny),
                false,
            ));
        let what = div()
            .row()
            .flex(1.0)
            .h_px(20.0)
            .items_center()
            .child(
                label(format!("{} ", pending.tool))
                    .mono()
                    .size(12.0)
                    .weight(700)
                    .color(colors.text),
            )
            .child(
                div().row().flex(1.0).child(
                    label(pending.summary.clone())
                        .mono()
                        .size(12.0)
                        .color(colors.text)
                        .truncate(),
                ),
            );
        let head = div()
            .row()
            .h_px(20.0)
            .gap(8.0)
            .items_center()
            .child(attention_pill("Waiting for approval"))
            .child(what);
        let card = if width < ONE_LINE_W {
            div()
                .col()
                .p(8.0)
                .pl(12.0)
                .gap(8.0)
                .child(head)
                .child(div().row().justify_end().child(actions))
        } else {
            div()
                .row()
                .p(8.0)
                .pl(12.0)
                .gap(8.0)
                .items_center()
                .child(div().row().flex(1.0).child(head))
                .child(actions)
        };
        cards.push(card);
    }
    if cards.is_empty() {
        return None;
    }
    // Each card sits on a ring with a darker edge under it, the way a raised card casts its shadow.
    let mut tray = div().col().w_px(width).px(7.0).pt(7.0).pb(5.0).gap(3.0);
    for card in cards {
        tray = tray.child(
            div().col().pb(1.0).rounded(11.0).bg(shadow()).child(
                div()
                    .col()
                    .p(1.0)
                    .rounded(11.0)
                    .bg(colors.border)
                    .child(card.rounded(10.0).bg(colors.element_background)),
            ),
        );
    }
    Some(
        div()
            .col()
            .w_px(width)
            .bg(colors.panel_background)
            .child(tray)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .into(),
    )
}

fn shadow() -> Rgba {
    match theme().appearance {
        Appearance::Dark => Rgba::new(0.0, 0.0, 0.0, 0.35),
        Appearance::Light => Rgba::new(0.11, 0.09, 0.19, 0.1),
    }
}

fn attention_pill(text: &str) -> Div {
    let colors = theme();
    let pill = div()
        .row()
        .h_px(20.0)
        .px(7.0)
        .items_center()
        .rounded(10.0)
        .bg(colors.attention)
        .child(label(text).size(11.0).medium().color(colors.on_attention));
    // The yellow is close to white, so on a light card it needs an edge to stand off it.
    match colors.appearance {
        Appearance::Light => pill.border(1.0, colors.on_attention.alpha(0.22)),
        Appearance::Dark => pill,
    }
}

/// A small button: filled with the accent when `primary`, else ghost, with its keys in a chip after the text.
fn button(
    id: u64,
    glyph: Option<IconKind>,
    text: &str,
    keys: Option<String>,
    primary: bool,
) -> Div {
    let colors = theme();
    let (ink, keys_ink, chip) = if primary {
        (
            colors.on_accent,
            colors.on_accent,
            Rgba::new(1.0, 1.0, 1.0, 0.18),
        )
    } else {
        // A translucent wash: the flat hover color matches the card and would vanish on it.
        let chip = match colors.appearance {
            Appearance::Dark => Rgba::new(1.0, 1.0, 1.0, 0.05),
            Appearance::Light => Rgba::new(0.11, 0.09, 0.19, 0.08),
        };
        (colors.text, colors.text_muted, chip)
    };
    let mut button = div()
        .row()
        .h_px(24.0)
        .px(8.0)
        .gap(4.0)
        .items_center()
        .rounded(6.0)
        .on_click(id);
    button = if primary {
        button.bg(colors.accent_fill)
    } else {
        button
            .hover_bg(colors.ghost_element_hover)
            .active_bg(colors.ghost_element_active)
    };
    if let Some(kind) = glyph {
        button = button.child(icon(kind).size(14.0).color(ink));
    }
    button = button.child(label(text).size(12.0).medium().color(ink));
    if let Some(keys) = keys {
        button = button.child(
            div()
                .row()
                .h_px(18.0)
                .px(5.0)
                .items_center()
                .rounded(4.0)
                .bg(chip)
                .child(render_keystroke_in(&keys, 11.0, keys_ink)),
        );
    }
    button
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
        assert!(
            !bar.press(LeaseButton::Allow),
            "no tool call waits, so the key does nothing"
        );
        assert!(bar.press(LeaseButton::TakeOver), "the key takes over");
        assert!(bar.render(600.0).is_none(), "taken over by the key");
        assert!(
            !bar.press(LeaseButton::TakeOver),
            "nothing left to take over"
        );
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
