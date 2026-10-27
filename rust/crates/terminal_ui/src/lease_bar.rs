//! The strip over an agent tab someone else drives: who drives it, Take over, and a tool call waiting for an
//! approval with Allow and Deny, each a banner on the panel's tray.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use pom_agent::PendingApproval;
use pom_paths::StateDir;
use pom_ptyhost::SocketDir;
use ui::{div, label, theme, IconKind, LabelSize, Node, Space};
use workspace::keymap::{key_hint, Action};

const TAKE_OVER: u64 = 0;
const ALLOW: u64 = 1;
const DENY: u64 = 2;
const IDS: u64 = 4;
/// Lease and approval files are read at most this often while the tab paints.
const REFRESH: Duration = Duration::from_secs(1);
/// How often the window repaints while a take over runs, so its spinner turns.
const SPIN_FRAME: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseButton {
    TakeOver,
    Allow,
    Deny,
}

/// Who drives an agent session when it is not the person at the app, and the tool call it waits on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeaseState {
    pub driver: Option<String>,
    pub pending: Option<PendingApproval>,
}

/// Where Take over stands: offered, running since `Taking`'s seconds ago, or failed with the reason.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TakeOver<'a> {
    Ready,
    Taking(f32),
    Failed(&'a str),
}

/// Where a session's lease is read and moved: this machine's state, or the project's machine through its server.
pub trait LeaseSource: Send + Sync {
    fn state(&self) -> LeaseState;
    /// The person at the app takes the session over.
    fn take_over(&self) -> Result<(), String>;
    fn answer(&self, request: &str, allow: bool) -> Result<(), String>;
}

/// A session of this machine, read from the state dir.
pub struct LocalLease {
    holder: String,
    state: StateDir,
    holders: SocketDir,
}

impl LocalLease {
    fn session(&self) -> Result<pom_agent::SessionState, String> {
        pom_agent::session_for_holder(&self.state, &self.holder)
            .ok_or_else(|| format!("{} has no session yet", self.holder))
    }
}

impl LeaseSource for LocalLease {
    fn state(&self) -> LeaseState {
        let pending = pom_agent::session_for_holder(&self.state, &self.holder)
            .and_then(|session| pom_agent::pending_approval(&self.state, &session.session_id));
        LeaseState {
            driver: pom_agent::driven_by(&self.state, &self.holder),
            pending,
        }
    }

    fn take_over(&self) -> Result<(), String> {
        pom_agent::take_over(&self.state, &self.holders, &self.session()?)
            .map_err(|error| error.to_string())
    }

    fn answer(&self, request: &str, allow: bool) -> Result<(), String> {
        pom_agent::answer_as_person(&self.state, &self.session()?, request, allow)
            .map_err(|error| error.to_string())
    }
}

pub struct LeaseBar {
    base: u64,
    source: Arc<dyn LeaseSource>,
    cached: RefCell<(Option<Instant>, LeaseState)>,
    taking: RefCell<Option<Taking>>,
    failed: RefCell<Option<String>>,
}

struct Taking {
    started: Instant,
    done: mpsc::Receiver<Result<(), String>>,
}

/// The approval's actions share its line only when the strip is at least this wide; narrower, they wrap under it.
const ONE_LINE_W: f32 = 612.0;
/// What a failed take over's reason leaves beside it: the tray's inset, the icon and the Take over button.
const FAILED_ROOM: f32 = 180.0;

impl LeaseBar {
    pub fn new(holder: &str, state: StateDir, holders: SocketDir) -> LeaseBar {
        Self::with_source(Box::new(LocalLease {
            holder: holder.to_string(),
            state,
            holders,
        }))
    }

    pub fn with_source(source: Box<dyn LeaseSource>) -> LeaseBar {
        LeaseBar {
            base: workspace::toolbar_ids(IDS),
            source: Arc::from(source),
            cached: RefCell::new((None, LeaseState::default())),
            taking: RefCell::new(None),
            failed: RefCell::new(None),
        }
    }

    fn snapshot(&self) -> LeaseState {
        let mut cached = self.cached.borrow_mut();
        if cached.0.is_none_or(|at| at.elapsed() >= REFRESH) {
            *cached = (Some(Instant::now()), self.source.state());
        }
        cached.1.clone()
    }

    fn forget(&self) {
        self.cached.borrow_mut().0 = None;
    }

    /// Takes in a finished take over: the strip rereads the lease, and keeps the reason when it failed.
    fn settle_take_over(&self) {
        let finished = match self.taking.borrow().as_ref() {
            Some(taking) => match taking.done.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("take over stopped".into())),
            },
            None => None,
        };
        if let Some(result) = finished {
            *self.taking.borrow_mut() = None;
            if let Err(error) = &result {
                eprintln!("agent tab: {error}");
            }
            *self.failed.borrow_mut() = result.err();
            self.forget();
        }
    }

    /// True while a take over runs.
    pub fn busy(&self) -> bool {
        self.settle_take_over();
        self.taking.borrow().is_some()
    }

    /// The strip, or nothing while the person at the app drives the session and nothing waits on them.
    pub fn render(&self, width: f32) -> Option<Node> {
        self.settle_take_over();
        let snapshot = self.snapshot();
        let failed = self.failed.borrow();
        let take_over = match (self.taking.borrow().as_ref(), failed.as_deref()) {
            (Some(taking), _) => TakeOver::Taking(taking.started.elapsed().as_secs_f32()),
            (None, Some(reason)) => TakeOver::Failed(reason),
            (None, None) => TakeOver::Ready,
        };
        strip(
            self.base,
            snapshot.driver.as_deref(),
            take_over,
            snapshot.pending.as_ref(),
            width,
        )
    }

    // Taking over waits on the agent's holder, so it runs off the UI thread and the window keeps painting.
    fn start_take_over(&self) {
        let (sender, done) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let source = self.source.clone();
        let flag = finished.clone();
        std::thread::spawn(move || {
            let result = source.take_over();
            flag.store(true, Ordering::SeqCst);
            if sender.send(result).is_err() {
                eprintln!("agent tab: take over finished after its tab closed");
            }
            ui::wake();
        });
        std::thread::spawn(move || {
            while !finished.load(Ordering::SeqCst) {
                ui::wake();
                std::thread::sleep(SPIN_FRAME);
            }
        });
        *self.failed.borrow_mut() = None;
        *self.taking.borrow_mut() = Some(Taking {
            started: Instant::now(),
            done,
        });
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
        if self.busy() {
            return button == LeaseButton::TakeOver;
        }
        let snapshot = self.snapshot();
        let outcome = match (button, &snapshot.pending) {
            (LeaseButton::TakeOver, _) if snapshot.driver.is_some() => {
                self.start_take_over();
                return true;
            }
            (LeaseButton::Allow | LeaseButton::Deny, Some(pending)) => self
                .source
                .answer(&pending.request, button == LeaseButton::Allow),
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
    take_over: TakeOver<'_>,
    pending: Option<&PendingApproval>,
    width: f32,
) -> Option<Node> {
    let colors = theme();
    let mut cards = Vec::new();
    if let Some(driver) = driver {
        let watching = components::banner(
            components::BannerTone::Info,
            format!("Watching - {driver} drives this session"),
        )
        .icon(IconKind::Eye);
        cards.push(match take_over {
            TakeOver::Ready => {
                watching.action(button(base + TAKE_OVER, None, "Take over", None, false))
            }
            TakeOver::Taking(seconds) => watching.action(
                div()
                    .row()
                    .items_center()
                    .gap(Space::S6)
                    .child(components::spinner(seconds).size(12.0))
                    .child(
                        components::button(base + TAKE_OVER, "Taking over...")
                            .kind(components::ButtonKind::Ghost)
                            .enabled(false),
                    ),
            ),
            TakeOver::Failed(reason) => components::banner(
                components::BannerTone::Warning,
                format!("Could not take over: {reason}"),
            )
            .wrap((width - FAILED_ROOM).max(120.0))
            .detail(format!("Watching - {driver} still drives this session"))
            .action(button(base + TAKE_OVER, None, "Take over", None, false)),
        });
    }
    if let Some(pending) = pending {
        let lead = div()
            .row()
            .gap(Space::S8)
            .items_center()
            .child(components::pill("Waiting for approval").attention())
            .child(
                label(pending.tool.clone())
                    .mono()
                    .size(LabelSize::Small)
                    .weight(700)
                    .color(colors.text),
            );
        cards.push(
            components::banner(components::BannerTone::Warning, pending.summary.clone())
                .lead(lead)
                .mono()
                .action(button(
                    base + ALLOW,
                    Some(IconKind::Check),
                    "Allow",
                    key_hint(Action::AgentAllow),
                    true,
                ))
                .action(button(
                    base + DENY,
                    Some(IconKind::Close),
                    "Deny",
                    key_hint(Action::AgentDeny),
                    false,
                ))
                .stack_actions(width < ONE_LINE_W),
        );
    }
    if cards.is_empty() {
        return None;
    }
    let mut tray = div()
        .col()
        .w_px(width)
        .px(Space::S8)
        .pt(Space::S8)
        .pb(Space::S6)
        .gap(Space::S4);
    for card in cards {
        tray = tray.child(card);
    }
    Some(
        div()
            .col()
            .w_px(width)
            .bg(colors.panel_background)
            .child(tray)
            .child(components::divider())
            .into(),
    )
}

/// A small button: filled with the accent when `primary`, else ghost, with its keys in a chip after the text.
pub(crate) fn button(
    id: u64,
    glyph: Option<IconKind>,
    text: &str,
    keys: Option<String>,
    primary: bool,
) -> Node {
    components::button(id, text)
        .kind(if primary {
            components::ButtonKind::Primary
        } else {
            components::ButtonKind::Ghost
        })
        .icon(glyph)
        .keystroke(keys.as_deref())
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use pom_agent::{record_event, set_lease, AgentState, Caller, Identity, Lease, SessionEvent};

    fn settled(bar: &LeaseBar) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while bar.busy() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!bar.busy(), "take over never finished");
    }

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
        settled(&bar);
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
        settled(&bar);
        assert!(bar.render(600.0).is_none(), "taken over");
        assert!(
            holders.input_lease(holder).is_none(),
            "the person's keys reach the agent again"
        );
        assert!(!bar.click(base + IDS), "not its id");
    }

    struct Refusing;

    impl LeaseSource for Refusing {
        fn state(&self) -> LeaseState {
            LeaseState {
                driver: Some("an orchestrator".into()),
                pending: None,
            }
        }

        fn take_over(&self) -> Result<(), String> {
            std::thread::sleep(Duration::from_millis(50));
            Err("the agent is not running".into())
        }

        fn answer(&self, _request: &str, _allow: bool) -> Result<(), String> {
            Ok(())
        }
    }

    fn texts(bar: &LeaseBar) -> Vec<String> {
        let node = bar.render(600.0).expect("strip");
        ui::render(
            &node,
            ui::Rect::new(0.0, 0.0, 600.0, 200.0, ui::Rgba::TRANSPARENT),
        )
        .texts
        .into_iter()
        .map(|text| text.text)
        .collect()
    }

    #[test]
    fn a_take_over_shows_it_is_running_then_why_it_failed() {
        let mut bar = LeaseBar::with_source(Box::new(Refusing));
        assert!(bar.press(LeaseButton::TakeOver));
        assert!(bar.busy());
        assert!(texts(&bar).contains(&"Taking over...".to_string()));
        assert!(
            bar.press(LeaseButton::TakeOver),
            "a second press waits on the first"
        );
        settled(&bar);
        let after = texts(&bar);
        assert!(after.contains(&"Take over".to_string()), "{after:?}");
        assert!(
            after
                .iter()
                .any(|text| text.contains("the agent is not running")),
            "{after:?}"
        );
    }
}
