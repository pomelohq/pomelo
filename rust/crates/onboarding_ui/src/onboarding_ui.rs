//! The onboarding page: a tab in the editor area that creates a project (repositories, setup, review),
//! then follows its setup (clone, scan, configure, verify, repair) to a summary of what is ready.

mod model;
mod new_project;
mod preview;
mod setup;
mod view;

use std::cell::RefCell;
use std::rc::Rc;

use terminal::{Keystroke, Modifiers};
use ui::{div, theme, IconKind, Node, Rect, Rgba};
use workspace::text_field::TextField;
use workspace::{EditKey, Item, ItemTick, TerminalKeyOutcome};

pub use model::{
    home_relative, repo_name, AddMode, CloneProgress, Field, Finding, Form, Onboarding, Phase,
    RepoRow, Request, Run, Screen, Step,
};

pub use preview::preview_page;

pub const TAB_ID: &str = "onboarding";

pub(crate) mod ids {
    pub const CANCEL: u64 = 1;
    pub const BACK: u64 = 2;
    pub const NEXT: u64 = 3;
    pub const CREATE: u64 = 4;
    pub const NAME: u64 = 5;
    pub const BRANCH: u64 = 6;
    pub const URL: u64 = 7;
    pub const ADD_URL: u64 = 8;
    pub const CHOOSE: u64 = 9;
    pub const MODE_FOLDERS: u64 = 10;
    pub const MODE_URLS: u64 = 11;
    pub const SETUP_AGENT: u64 = 12;
    pub const SETUP_MANUAL: u64 = 13;
    pub const OPT_SECRETS: u64 = 14;
    pub const OPT_SHARED: u64 = 15;
    pub const OPT_WORKSPACE: u64 = 16;
    pub const WORKSPACE_BRANCH: u64 = 17;
    pub const PAUSE: u64 = 20;
    pub const STOP: u64 = 21;
    pub const AGENT_CLI: u64 = 22;
    pub const OPEN_CONFIG: u64 = 23;
    pub const SKIP_AGENT: u64 = 24;
    pub const RESUME_AGENT: u64 = 25;
    pub const VERIFY_NOW: u64 = 26;
    pub const FIX_AGENT: u64 = 27;
    pub const FIX_MANUAL: u64 = 28;
    pub const SKIP_SERVICE: u64 = 29;
    pub const CANCEL_FIX: u64 = 30;
    pub const RETRY: u64 = 31;
    pub const OPEN_WORKSPACE: u64 = 32;
    pub const START_MAIN: u64 = 33;
    pub const CLOSE: u64 = 34;
    pub const BACK_TO_FORM: u64 = 35;
    pub const OPEN_TERMINAL: u64 = 36;
    pub const STEP_BASE: u64 = 40;
    pub const AGENT_BASE: u64 = 50;
    pub const ALIAS_BASE: u64 = 100;
    pub const REMOVE_BASE: u64 = 300;
    pub const ROW_LIMIT: u64 = 100;
}

pub type Shared = Rc<RefCell<Onboarding>>;

/// The tab; its state lives in `Shared` so the app can feed progress in and the tab can be opened again
/// in the new project's window.
pub struct OnboardingPage {
    shared: Shared,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    scroll: f32,
    content_h: f32,
    body_h: f32,
    seen: (u64, u64, bool),
}

impl OnboardingPage {
    pub fn new(shared: Shared) -> OnboardingPage {
        OnboardingPage {
            shared,
            hits: Vec::new(),
            hovered: None,
            scroll: 0.0,
            content_h: 0.0,
            body_h: 0.0,
            seen: (u64::MAX, 0, false),
        }
    }

    /// The page as it is drawn at `width`, for tests and snapshots.
    pub fn tree(&self, width: f32) -> Node {
        let state = self.shared.borrow();
        let content_w = (width - 2.0 * view::PAGE_PAD_X).clamp(200.0, view::CONTENT_MAX_W);
        let body = match state.screen {
            Screen::NewProject => new_project::render(&state, content_w, self.hovered),
            Screen::Progress => setup::progress(&state, content_w, self.hovered),
            Screen::Done => setup::done(&state, content_w, self.hovered),
        };
        div()
            .col()
            .w_px(width)
            .bg(theme().editor_background)
            .pt(view::PAGE_PAD_TOP)
            .pb(view::PAGE_PAD_BOTTOM)
            .items_center()
            .child(body)
            .into()
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
        let mut state = self.shared.borrow_mut();
        let screen = state.screen;
        match screen {
            Screen::NewProject => new_project::click(&mut state, id),
            Screen::Progress | Screen::Done => setup::click(&mut state, id),
        }
        state.changed();
    }

    fn edit_key(keystroke: &Keystroke) -> Option<(EditKey, bool)> {
        let Modifiers {
            shift, alt, cmd, ..
        } = keystroke.modifiers;
        let key = match keystroke.key.as_str() {
            "left" if cmd => EditKey::Home,
            "right" if cmd => EditKey::End,
            "left" if alt => EditKey::WordLeft,
            "right" if alt => EditKey::WordRight,
            "left" => EditKey::Left,
            "right" => EditKey::Right,
            "home" => EditKey::Home,
            "end" => EditKey::End,
            "backspace" if cmd => EditKey::DeleteToLineStart,
            "backspace" if alt => EditKey::DeleteWordLeft,
            "backspace" => EditKey::Backspace,
            "delete" => EditKey::Delete,
            "a" if cmd => EditKey::SelectAll,
            _ => return None,
        };
        Some((key, shift))
    }
}

/// The focused text field of the form, if any.
pub(crate) fn focused_field(form: &mut Form) -> Option<&mut TextField> {
    Some(match form.focus? {
        Field::Name => &mut form.name,
        Field::Branch => &mut form.branch,
        Field::Url => &mut form.url,
        Field::WorkspaceBranch => &mut form.workspace_branch,
        Field::Alias(index) => &mut form.repos.get_mut(index)?.alias,
    })
}

impl Item for OnboardingPage {
    fn id(&self) -> Option<String> {
        Some(TAB_ID.to_string())
    }

    fn title(&self) -> String {
        let state = self.shared.borrow();
        let name = state
            .run
            .as_ref()
            .map(|run| run.name.clone())
            .unwrap_or_default();
        match state.screen {
            Screen::NewProject => "New project".into(),
            Screen::Progress => format!("Setting up {name}"),
            Screen::Done => format!("{name} is ready"),
        }
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(match self.shared.borrow().screen {
            Screen::Progress => IconKind::RotateCw,
            _ => IconKind::Package,
        })
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let tree = self.tree(body.w / scale);
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

    fn wants_keystrokes(&self) -> bool {
        true
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let cmd = keystroke.modifiers.cmd;
        let mut state = self.shared.borrow_mut();
        if state.screen != Screen::NewProject {
            return TerminalKeyOutcome::Ignored;
        }
        let outcome = match keystroke.key.as_str() {
            "enter" if cmd => {
                new_project::click(&mut state, ids::CREATE);
                TerminalKeyOutcome::Handled
            }
            "enter" => {
                new_project::enter(&mut state);
                TerminalKeyOutcome::Handled
            }
            "escape" => {
                state.requests.push(Request::Close);
                TerminalKeyOutcome::Handled
            }
            "tab" => {
                new_project::cycle_focus(&mut state.form, keystroke.modifiers.shift);
                TerminalKeyOutcome::Handled
            }
            "v" if cmd => TerminalKeyOutcome::Paste,
            "c" if cmd => focused_field(&mut state.form)
                .and_then(|field| field.selected_text())
                .map_or(TerminalKeyOutcome::Handled, TerminalKeyOutcome::Copy),
            _ => match (Self::edit_key(keystroke), focused_field(&mut state.form)) {
                (Some((key, shift)), Some(field)) => {
                    field.key(key, shift);
                    TerminalKeyOutcome::Handled
                }
                _ => TerminalKeyOutcome::Ignored,
            },
        };
        state.changed();
        outcome
    }

    fn input_text(&mut self, text: &str) {
        let mut state = self.shared.borrow_mut();
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if let Some(field) = focused_field(&mut state.form) {
            field.insert(&typed);
        }
        state.changed();
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        let mut state = self.shared.borrow_mut();
        if state.form.focus == Some(Field::Url) && text.trim().contains(char::is_whitespace) {
            let joined = format!("{} {text}", state.form.url.text());
            state.form.url.set_text(&joined);
            state.add_urls();
        } else if let Some(field) = focused_field(&mut state.form) {
            let line: String = text.chars().filter(|c| !c.is_control()).collect();
            field.insert(&line);
        }
        state.changed();
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        match self.hit(x, y) {
            Some(id) => self.click(id),
            None => {
                let mut state = self.shared.borrow_mut();
                state.form.focus = None;
                state.changed();
            }
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
        let state = self.shared.borrow();
        // A running setup shows elapsed seconds, so it repaints once a second on its own.
        let second = match (&state.run, state.screen) {
            (Some(run), Screen::Progress) => run.started.elapsed().as_secs() + 1,
            _ => 0,
        };
        let seen = (state.version, second, ui::caret_phase());
        let changed = seen != self.seen;
        self.seen = seen;
        ItemTick {
            changed,
            ..ItemTick::default()
        }
    }

    fn closed(&mut self) {
        if let Ok(mut state) = self.shared.try_borrow_mut() {
            state.requests.push(Request::TabClosed);
        }
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests;
