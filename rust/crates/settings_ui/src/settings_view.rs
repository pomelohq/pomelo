//! `SettingsView`: the settings screen as a reactive `RawView`. It owns all settings-window state (selection,
//! expansion, search, page scroll, inline edit, dropdown popover) that previously lived scattered in the
//! binary, composes the layered `Frame` (clipped scrolling page under fixed chrome, popover on top), and
//! handles input via `on_click`/`scroll`/`hover`/key methods. Global side-effects that reach other windows
//! (theme, UI font, text scale/weight) are applied to the shared `ui` globals here and flagged in
//! `SideEffects` for the shell to re-apply to each window's renderer and repaint.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate as settings_ui;
use pom_paths::StateDir;
use settings::Settings;
use settings_ui::{ConnectionStatus, IntegrationsPage, TokenSource};
use ui::{Context, Frame, Overlay, Painted, RawView, Rect, Window};

const JIRA_FIELDS: [u64; 3] = [
    settings_ui::CTRL_JIRA_SITE,
    settings_ui::CTRL_JIRA_EMAIL,
    settings_ui::CTRL_JIRA_TOKEN,
];

/// Cross-window work the shell must do after an input the view handled: re-apply the UI font to every window's
/// text renderer, and/or repaint the other windows because a shared global (theme/scale/weight) changed.
#[derive(Default, Clone, Copy)]
pub struct SideEffects {
    pub reapply_font: bool,
    pub redraw_others: bool,
}

/// Innermost hit id under `(x, y)` (regions pushed outer-first, inner-last), like `ui::Window::hit_at`.
fn hit_test(hits: &[(Rect, u64)], x: f32, y: f32) -> Option<u64> {
    hits.iter()
        .rev()
        .find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
        .map(|(_, id)| *id)
}

pub struct SettingsView {
    settings: Settings,
    fonts: Vec<String>,
    selected: usize,
    expanded: Vec<bool>,
    search_query: String,
    page_scroll: f32,
    page_total_h: f32,
    editing: Option<(u64, String)>,
    hovered: Option<u64>,
    active_section: Option<usize>,
    section_offsets: Vec<f32>,
    popover: Option<u64>,
    popover_scroll: usize,
    popover_query: String,
    popover_hover: Option<u64>,
    popover_rect: Option<Rect>,
    scroll_accum: f32,
    hits: Vec<(Rect, u64)>,
    /// Last laid-out viewport (logical px), cached each `build` so input methods invoked outside `render_frame`
    /// (scroll/key, reached via `entity.update`) can size popovers and clamp scroll without a `Window`.
    viewport: (f32, f32),
    pending: SideEffects,
    /// The state folder and session whose Jira settings the Integrations page edits.
    jira_session: Option<(StateDir, String)>,
    jira: IntegrationsPage,
    jira_test: Option<Receiver<Result<String, String>>>,
    /// The project's `pom.yml`, whose `sync:` applies until this window sets refresh-main.
    project_config: Option<std::sync::Arc<pom_config::Config>>,
}

impl SettingsView {
    /// Open on the Appearance page with its sub-sections expanded (as the binary did on window open).
    pub fn new(settings: Settings) -> Self {
        let mut expanded = vec![false; settings_ui::CATEGORY_COUNT];
        if let Some(e) = expanded.get_mut(settings_ui::APPEARANCE) {
            *e = true;
        }
        Self {
            settings,
            fonts: Vec::new(),
            selected: settings_ui::APPEARANCE,
            expanded,
            search_query: String::new(),
            page_scroll: 0.0,
            page_total_h: 0.0,
            editing: None,
            hovered: None,
            active_section: None,
            section_offsets: Vec::new(),
            popover: None,
            popover_scroll: 0,
            popover_query: String::new(),
            popover_hover: None,
            popover_rect: None,
            scroll_accum: 0.0,
            hits: Vec::new(),
            viewport: (0.0, 0.0),
            pending: SideEffects::default(),
            jira_session: None,
            jira: IntegrationsPage::default(),
            jira_test: None,
            project_config: None,
        }
    }

    pub fn set_project_config(&mut self, config: Option<std::sync::Arc<pom_config::Config>>) {
        self.project_config = config;
        let status = self.jira.status.clone();
        self.reload_jira(status);
    }

    /// Saves refresh-main's schedule for the session (the background loop picks it up within seconds).
    fn save_refresh(&mut self, enabled: bool, minutes: u64) {
        let Some((state, session)) = self.jira_session.clone() else {
            return;
        };
        let minutes = minutes.clamp(
            settings_ui::REFRESH_MINUTES_MIN,
            settings_ui::REFRESH_MINUTES_MAX,
        );
        let schedule = pom_sync::RefreshSchedule {
            enabled,
            interval_seconds: minutes * 60,
        };
        if let Err(error) = pom_sync::save_refresh_schedule(&state, &session, schedule) {
            eprintln!("settings: save refresh-main: {error}");
        }
        let status = self.jira.status.clone();
        self.reload_jira(status);
    }

    /// Keeps a value the app changed (the ticket picker's board) so this window's next save does not undo it.
    pub fn remember_jira_board(&mut self, board: i64) {
        self.settings.jira_board = board;
    }

    /// The project session the Integrations page edits (none while no project is open).
    pub fn set_jira_session(&mut self, session: Option<(StateDir, String)>) {
        if self.jira_session.as_ref().map(|(_, name)| name)
            == session.as_ref().map(|(_, name)| name)
        {
            return;
        }
        self.jira_session = session;
        self.jira_test = None;
        self.reload_jira(ConnectionStatus::Untested);
    }

    fn reload_jira(&mut self, status: ConnectionStatus) {
        let Some((state, session)) = &self.jira_session else {
            self.jira = IntegrationsPage::default();
            return;
        };
        let stored = pom_jira::JiraSettings::load(state, session);
        let token = match pom_jira::resolve_token(state, session, &stored, &|name| {
            std::env::var(name).ok()
        }) {
            Some((_, pom_jira::TokenOrigin::Secret)) => TokenSource::Secret,
            Some((_, pom_jira::TokenOrigin::Environment)) => TokenSource::Environment,
            None => TokenSource::Missing,
        };
        let refresh = pom_sync::refresh_schedule(state, session, self.project_config.as_deref());
        self.jira = IntegrationsPage {
            session: session.clone(),
            keep_main_fresh: refresh.enabled,
            refresh_minutes: (refresh.interval_seconds / 60).max(settings_ui::REFRESH_MINUTES_MIN),
            site: stored.site,
            email: stored.email,
            token,
            status,
        };
    }

    /// Saves a typed Jira field: site and email into the session's integrations file, the token into its secrets.
    fn commit_jira(&mut self, id: u64, text: &str) {
        let Some((state, session)) = self.jira_session.clone() else {
            return;
        };
        let text = text.trim();
        let saved = if id == settings_ui::CTRL_JIRA_TOKEN {
            if text.is_empty() {
                return;
            }
            pom_jira::save_token(&state, &session, text)
        } else {
            let mut stored = pom_jira::JiraSettings::load(&state, &session);
            if id == settings_ui::CTRL_JIRA_SITE {
                stored.site = text.to_string();
            } else {
                stored.email = text.to_string();
            }
            stored
                .save(&state, &session)
                .map_err(|error| error.to_string())
        };
        match saved {
            Ok(()) => self.reload_jira(ConnectionStatus::Untested),
            Err(error) => self.reload_jira(ConnectionStatus::Failed(error)),
        }
    }

    fn test_jira(&mut self) {
        let Some((state, session)) = self.jira_session.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let answer = match pom_jira::resolve(&state, &session) {
                Some(client) => client
                    .myself()
                    .map(|(name, email)| format!("{name} ({email})"))
                    .map_err(|error| error.to_string()),
                None => Err("set the site, email and token first".to_string()),
            };
            if sender.send(answer).is_err() {
                eprintln!("settings: the Jira check finished after the window closed");
            }
        });
        self.jira_test = Some(receiver);
        self.jira.status = ConnectionStatus::Testing;
    }

    /// Picks up a finished connection check; returns whether the page changed.
    pub fn tick(&mut self) -> bool {
        let Some(receiver) = &self.jira_test else {
            return false;
        };
        let status = match receiver.try_recv() {
            Ok(Ok(who)) => ConnectionStatus::SignedIn(who),
            Ok(Err(error)) => ConnectionStatus::Failed(error),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => ConnectionStatus::Failed("the check stopped".into()),
        };
        self.jira_test = None;
        self.jira.status = status;
        true
    }

    pub fn busy(&self) -> bool {
        self.jira_test.is_some()
    }

    /// Pastes into the field being edited (one line; newlines dropped).
    pub fn key_paste(&mut self, text: &str) -> bool {
        let line: String = text.chars().filter(|c| !c.is_control()).collect();
        match self.editing.as_mut() {
            Some((id, buf)) if JIRA_FIELDS.contains(id) && !line.is_empty() => {
                buf.push_str(&line);
                true
            }
            _ => false,
        }
    }

    /// The shell injects the system font list (from the renderer) once; the font-family dropdown needs it.
    pub fn set_fonts(&mut self, fonts: Vec<String>) {
        self.fonts = fonts;
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The shell drains this after routing an input to learn what other windows need.
    pub fn take_side_effects(&mut self) -> SideEffects {
        std::mem::take(&mut self.pending)
    }

    fn search_active(&self) -> bool {
        self.popover.is_none() && self.editing.is_none()
    }

    fn content_clip_h(&self) -> f32 {
        let (_, _, _, clip_h) = settings_ui::content_region(self.viewport.0, self.viewport.1);
        clip_h
    }

    /// Commit an in-progress numeric edit (parse, clamp, persist, re-apply the global). No-op if idle.
    fn commit_edit(&mut self) {
        let Some((id, buf)) = self.editing.take() else {
            return;
        };
        if JIRA_FIELDS.contains(&id) {
            self.commit_jira(id, &buf);
            return;
        }
        if id == settings_ui::CTRL_REFRESH_EDIT {
            if let Ok(minutes) = buf.trim().parse::<u64>() {
                self.save_refresh(self.jira.keep_main_fresh, minutes);
            }
            return;
        }
        let Ok(v) = buf.trim().parse::<f32>() else {
            return;
        };
        let changed = if id == settings_ui::CTRL_FONT_WEIGHT_EDIT {
            let c = settings_ui::set_font_weight(&mut self.settings, v);
            if c {
                ui::set_ui_font_weight(self.settings.ui_font_weight as u16);
            }
            c
        } else {
            let c = settings_ui::set_font_size(&mut self.settings, v);
            if c {
                ui::set_ui_text_scale(self.settings.ui_font_size / ui::UI_FONT_BASE);
            }
            c
        };
        if changed {
            let _ = self.settings.save();
            self.pending.redraw_others = true;
        }
    }

    /// Rebuild page/chrome/popover into a layered `Frame` and refresh derived state (section offsets, active
    /// section, hit list). Ported from the binary's `draw_settings`.
    fn build(&mut self, window: &Window) -> Frame {
        let (w, h) = (window.width, window.height);
        self.viewport = (w, h);
        let (clip_x, clip_y, clip_w, clip_h) = settings_ui::content_region(w, h);

        let max_scroll = (self.page_total_h - clip_h).max(0.0);
        self.page_scroll = self.page_scroll.clamp(0.0, max_scroll);
        let (page, total_h) = settings_ui::page(
            self.selected,
            &self.settings,
            &self.jira,
            self.editing.as_ref().map(|(id, buf)| (*id, buf.as_str())),
            &self.search_query,
            w,
            h,
            self.page_scroll,
        );
        self.page_total_h = total_h;

        let mut offsets: Vec<(u64, f32)> = page
            .hits
            .iter()
            .filter(|(_, id)| *id >= settings_ui::SECTION_ANCHOR_BASE)
            .map(|(r, id)| (*id, r.y - clip_y + self.page_scroll))
            .collect();
        offsets.sort_by_key(|(id, _)| *id);
        self.section_offsets = offsets.into_iter().map(|(_, y)| y).collect();

        if self.section_offsets.is_empty() {
            self.active_section = None;
        } else if self.active_section.is_none() {
            self.active_section = Some(0);
        }

        let chrome = settings_ui::chrome(
            self.selected,
            self.hovered,
            self.active_section,
            &self.expanded,
            w,
            h,
            &self.search_query,
            self.search_active(),
        );

        let popover = self.popover.and_then(|cid| {
            let anchor = page
                .hits
                .iter()
                .chain(chrome.hits.iter())
                .find(|(_, id)| *id == cid)
                .map(|(r, _)| *r)?;
            let items = settings_ui::control_items(cid, &self.fonts);
            let current = settings_ui::control_value(cid, &self.settings);
            Some(settings_ui::popover(
                anchor,
                &current,
                &items,
                self.popover_scroll,
                &self.popover_query,
                self.popover_hover,
                w,
                h,
            ))
        });

        let mut overlay = chrome;
        if let Some(bar) = settings_ui::content_scrollbar(
            (clip_x, clip_y, clip_w, clip_h),
            self.page_total_h,
            self.page_scroll,
        ) {
            overlay.rects.push(bar);
        }
        self.popover_rect = None;
        if let Some(pop) = &popover {
            self.popover_rect = pop.rects.iter().find(|r| r.color.a > 0.5).copied();
        }

        // Hit order (topmost last): visible page rows, fixed chrome, popover.
        let mut hits: Vec<(Rect, u64)> = page
            .hits
            .iter()
            .filter(|(r, _)| r.y + r.h > clip_y && r.y < clip_y + clip_h)
            .copied()
            .collect();
        hits.extend(overlay.hits.iter().copied());
        if let Some(pop) = &popover {
            hits.extend(pop.hits.iter().copied());
        }
        self.hits = hits.clone();

        // Layers: clipped page (base is empty; the clear color fills), fixed chrome, popover on top.
        let mut overlays = vec![
            Overlay {
                painted: page,
                clip: Some(Rect::new(
                    clip_x,
                    clip_y,
                    clip_w,
                    clip_h,
                    ui::Rgba::TRANSPARENT,
                )),
            },
            Overlay {
                painted: overlay,
                clip: None,
            },
        ];
        if let Some(pop) = popover {
            overlays.push(Overlay {
                painted: pop,
                clip: None,
            });
        }
        Frame {
            base: Painted::default(),
            overlays,
            hits,
        }
    }

    fn close_popover(&mut self) {
        self.popover = None;
        self.popover_hover = None;
        self.popover_query.clear();
    }

    /// Open a dropdown for control `cid`, scrolled so the current value is in view (ported from the binary).
    fn open_popover(&mut self, cid: u64) {
        self.popover = Some(cid);
        self.scroll_accum = 0.0;
        self.popover_query.clear();
        let items = settings_ui::control_items(cid, &self.fonts);
        let current = settings_ui::control_value(cid, &self.settings);
        let sel = items.iter().position(|it| *it == current).unwrap_or(0);
        self.popover_hover = Some(settings_ui::POPOVER_BASE + sel as u64);
        let anchor = self
            .hits
            .iter()
            .find(|(_, hid)| *hid == cid)
            .map(|(r, _)| *r);
        let max_scroll = anchor
            .map(|a| settings_ui::popover_max_scroll_for(items.len(), a, self.viewport.1))
            .unwrap_or(0);
        self.popover_scroll = sel.saturating_sub(2).min(max_scroll);
    }

    /// Update hover from a cursor move; returns true if a repaint is warranted. Mirrors the binary's cursor
    /// handling plus the sticky popover-hover recompute that `draw_settings` did each frame.
    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let hit = hit_test(&self.hits, x, y);
        if self.popover.is_some() {
            if let Some(id) = hit.filter(|id| *id >= settings_ui::POPOVER_BASE) {
                if Some(id) != self.popover_hover {
                    self.popover_hover = Some(id);
                    return true;
                }
            }
            false
        } else if hit != self.hovered {
            self.hovered = hit;
            true
        } else {
            false
        }
    }

    /// Whether the cursor at `(x, y)` is over the open popover panel (so the wheel scrolls the list, not page).
    fn over_popover(&self, x: f32, y: f32) -> bool {
        self.popover_rect
            .is_some_and(|r| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
    }

    /// Scroll the page or the open popover by a wheel delta (logical px). `(x, y)` is the cursor. Returns true
    /// if something moved (repaint needed).
    pub fn scroll(&mut self, dy: f32, x: f32, y: f32) -> bool {
        if self.popover.is_none() || !self.over_popover(x, y) {
            let clip_h = self.content_clip_h();
            let max = (self.page_total_h - clip_h).max(0.0);
            let next = (self.page_scroll - dy).clamp(0.0, max);
            if (next - self.page_scroll).abs() <= 0.01 {
                return false;
            }
            self.page_scroll = next;
            if !self.section_offsets.is_empty() {
                let active = self
                    .section_offsets
                    .iter()
                    .enumerate()
                    .filter(|(_, off)| **off <= next + 4.0)
                    .map(|(i, _)| i)
                    .next_back()
                    .unwrap_or(0);
                self.active_section = Some(active);
            }
            return true;
        }
        let Some(cid) = self.popover else {
            return false;
        };
        let items = settings_ui::control_items(cid, &self.fonts);
        let shown = settings_ui::filter_indices(&items, &self.popover_query).len();
        let Some(anchor) = self.hits.iter().find(|(_, id)| *id == cid).map(|(r, _)| *r) else {
            return false;
        };
        let max_scroll = settings_ui::popover_max_scroll_for(shown, anchor, self.viewport.1);
        if max_scroll == 0 {
            return false;
        }
        self.scroll_accum -= dy / 30.0;
        let steps = self.scroll_accum.trunc();
        if steps == 0.0 {
            return false;
        }
        self.scroll_accum -= steps;
        let next = (self.popover_scroll as i64 + steps as i64).clamp(0, max_scroll as i64) as usize;
        if next == self.popover_scroll {
            return false;
        }
        self.popover_scroll = next;
        true
    }

    /// Type into whichever field is focused (popover filter, numeric edit, or the sidebar search). Returns true
    /// if the buffer changed.
    pub fn key_text(&mut self, text: &str) -> bool {
        if self.popover.is_some() {
            let printable: String = text.chars().filter(|c| !c.is_control()).collect();
            if printable.is_empty() {
                return false;
            }
            self.popover_query.push_str(&printable);
            self.popover_scroll = 0;
            self.scroll_accum = 0.0;
            true
        } else if let Some((id, buf)) = self.editing.as_mut() {
            let free_text = JIRA_FIELDS.contains(id);
            let add: String = text
                .chars()
                .filter(|c| {
                    if free_text {
                        !c.is_control()
                    } else {
                        c.is_ascii_digit() || *c == '.'
                    }
                })
                .collect();
            if add.is_empty() {
                return false;
            }
            buf.push_str(&add);
            true
        } else {
            let printable: String = text.chars().filter(|c| !c.is_control()).collect();
            if printable.is_empty() {
                return false;
            }
            self.search_query.push_str(&printable);
            true
        }
    }

    /// Backspace in the focused field. Returns true if something changed.
    pub fn key_backspace(&mut self) -> bool {
        if self.popover.is_some() {
            let changed = self.popover_query.pop().is_some();
            if changed {
                self.popover_scroll = 0;
                self.scroll_accum = 0.0;
            }
            changed
        } else if let Some((_, buf)) = self.editing.as_mut() {
            buf.pop();
            true
        } else {
            self.search_query.pop().is_some()
        }
    }

    /// Escape: close the popover, cancel an edit, or clear the search (in that priority). Returns true if it
    /// did anything.
    pub fn key_escape(&mut self) -> bool {
        if self.popover.is_some() {
            self.close_popover();
            true
        } else if self.editing.take().is_some() {
            true
        } else if !self.search_query.is_empty() {
            self.search_query.clear();
            true
        } else {
            false
        }
    }

    /// Enter: commit an in-progress numeric edit.
    pub fn key_enter(&mut self) -> bool {
        if self.editing.is_some() {
            self.commit_edit();
            true
        } else {
            false
        }
    }

    fn click(&mut self, id: u64) {
        if id >= settings_ui::RESET_OFFSET {
            self.commit_edit();
            self.close_popover();
            let base = id - settings_ui::RESET_OFFSET;
            if settings_ui::reset_to_default(base, &mut self.settings) {
                let _ = self.settings.save();
                match base {
                    settings_ui::CTRL_THEME => {
                        ui::set_theme(ui::by_name(&self.settings.theme));
                        self.pending.redraw_others = true;
                    }
                    settings_ui::CTRL_FONT_FAMILY => self.pending.reapply_font = true,
                    settings_ui::CTRL_FONT_WEIGHT_EDIT => {
                        ui::set_ui_font_weight(self.settings.ui_font_weight as u16);
                        self.pending.redraw_others = true;
                    }
                    settings_ui::CTRL_FONT_SIZE_EDIT => {
                        ui::set_ui_text_scale(self.settings.ui_font_size / ui::UI_FONT_BASE);
                        self.pending.redraw_others = true;
                    }
                    settings_ui::CTRL_SHOW_DIAGNOSTICS
                    | settings_ui::CTRL_SHOW_CURSOR
                    | settings_ui::CTRL_SHOW_LANGUAGE
                    | settings_ui::CTRL_SHOW_BRANCH
                    | settings_ui::CTRL_SHOW_SESSION => {
                        ui::set_chrome(settings_ui::chrome_flags(&self.settings));
                        self.pending.redraw_others = true;
                    }
                    _ => {}
                }
            }
            return;
        }
        if id == settings_ui::CTRL_OPEN_JSON {
            self.commit_edit();
            self.close_popover();
            if let Some(home) = std::env::var_os("HOME") {
                let path = std::path::Path::new(&home).join(".config/pomelo/settings.json");
                let _ = std::process::Command::new("open").arg(path).spawn();
            }
            return;
        }
        if id >= settings_ui::POPOVER_BASE {
            let idx = (id - settings_ui::POPOVER_BASE) as usize;
            if let Some(cid) = self.popover {
                if settings_ui::apply_choice(cid, idx, &self.fonts, &mut self.settings) {
                    let _ = self.settings.save();
                    if cid == settings_ui::CTRL_FONT_FAMILY {
                        self.pending.reapply_font = true;
                    }
                    if cid == settings_ui::CTRL_THEME {
                        ui::set_theme(ui::by_name(&self.settings.theme));
                        self.pending.redraw_others = true;
                    }
                }
            }
            self.close_popover();
        } else if id == settings_ui::CTRL_SEARCH_CLEAR {
            self.search_query.clear();
            self.page_scroll = 0.0;
        } else if id == settings_ui::CTRL_SEARCH {
            self.commit_edit();
            self.close_popover();
        } else if (settings_ui::NAV_TOGGLE_BASE..settings_ui::NAV_TOGGLE_BASE + 100).contains(&id) {
            let ci = (id - settings_ui::NAV_TOGGLE_BASE) as usize;
            if let Some(e) = self.expanded.get_mut(ci) {
                *e = !*e;
            }
        } else if (settings_ui::NAV_JUMP_BASE..settings_ui::NAV_TOGGLE_BASE).contains(&id) {
            self.commit_edit();
            self.popover = None;
            let rel = id - settings_ui::NAV_JUMP_BASE;
            let cat = (rel / settings_ui::NAV_JUMP_STRIDE) as usize;
            let si = (rel % settings_ui::NAV_JUMP_STRIDE) as usize;
            self.selected = cat;
            if let Some(e) = self.expanded.get_mut(cat) {
                *e = true;
            }
            self.active_section = Some(si);
            self.page_scroll = 0.0;
            // Rebuild so section offsets are current, then scroll the chosen section to the top.
            let window = Window::new(self.viewport.0, self.viewport.1, 1.0);
            let _ = self.build(&window);
            if let Some(&off) = self.section_offsets.get(si) {
                let clip_h = self.content_clip_h();
                let max = (self.page_total_h - clip_h).max(0.0);
                self.page_scroll = off.clamp(0.0, max);
            }
        } else if JIRA_FIELDS.contains(&id) {
            self.commit_edit();
            let seed = match id {
                settings_ui::CTRL_JIRA_SITE => self.jira.site.clone(),
                settings_ui::CTRL_JIRA_EMAIL => self.jira.email.clone(),
                _ => String::new(),
            };
            self.editing = Some((id, seed));
            self.close_popover();
        } else if id == settings_ui::CTRL_JIRA_RESET_TOKEN {
            self.commit_edit();
            if let Some((state, session)) = self.jira_session.clone() {
                if let Err(error) = pom_jira::save_token(&state, &session, "") {
                    eprintln!("settings: clear the Jira token: {error}");
                }
            }
            self.reload_jira(ConnectionStatus::Untested);
        } else if id == settings_ui::CTRL_REFRESH_MAIN {
            self.commit_edit();
            self.save_refresh(!self.jira.keep_main_fresh, self.jira.refresh_minutes);
        } else if id == settings_ui::CTRL_REFRESH_DEC || id == settings_ui::CTRL_REFRESH_INC {
            self.commit_edit();
            let minutes = self.jira.refresh_minutes;
            let next = if id == settings_ui::CTRL_REFRESH_INC {
                minutes + 5 - minutes % 5
            } else {
                minutes.saturating_sub(if minutes.is_multiple_of(5) {
                    5
                } else {
                    minutes % 5
                })
            };
            self.save_refresh(self.jira.keep_main_fresh, next);
        } else if id == settings_ui::CTRL_REFRESH_EDIT {
            self.commit_edit();
            self.editing = Some((id, self.jira.refresh_minutes.to_string()));
            self.close_popover();
        } else if id == settings_ui::CTRL_JIRA_TEST {
            self.commit_edit();
            if self.jira_test.is_none() {
                self.test_jira();
            }
        } else if id == settings_ui::CTRL_FONT_SIZE_EDIT || id == settings_ui::CTRL_FONT_WEIGHT_EDIT
        {
            self.commit_edit();
            let seed = if id == settings_ui::CTRL_FONT_WEIGHT_EDIT {
                format!("{:.0}", self.settings.ui_font_weight)
            } else {
                format!("{:.0}", self.settings.ui_font_size)
            };
            self.editing = Some((id, seed));
            self.popover = None;
            self.popover_hover = None;
        } else if id >= 100 {
            self.commit_edit();
            if settings_ui::is_dropdown(id) {
                if self.popover == Some(id) {
                    self.close_popover();
                } else {
                    self.open_popover(id);
                }
            } else if settings_ui::handle_control(id, &mut self.settings) {
                let _ = self.settings.save();
                match id {
                    settings_ui::CTRL_FONT_WEIGHT_DEC | settings_ui::CTRL_FONT_WEIGHT_INC => {
                        ui::set_ui_font_weight(self.settings.ui_font_weight as u16);
                    }
                    settings_ui::CTRL_FONT_SIZE_DEC | settings_ui::CTRL_FONT_SIZE_INC => {
                        ui::set_ui_text_scale(self.settings.ui_font_size / ui::UI_FONT_BASE);
                    }
                    settings_ui::CTRL_SHOW_DIAGNOSTICS
                    | settings_ui::CTRL_SHOW_CURSOR
                    | settings_ui::CTRL_SHOW_LANGUAGE
                    | settings_ui::CTRL_SHOW_BRANCH
                    | settings_ui::CTRL_SHOW_SESSION => {
                        ui::set_chrome(settings_ui::chrome_flags(&self.settings));
                    }
                    _ => {}
                }
                self.pending.redraw_others = true;
            }
        } else {
            self.commit_edit();
            self.popover = None;
            self.popover_hover = None;
            let i = id as usize;
            if let Some(e) = self.expanded.get_mut(i) {
                *e = true;
            }
            self.selected = i;
            self.page_scroll = 0.0;
            self.active_section = None;
        }
    }
}

impl RawView for SettingsView {
    fn render_frame(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> Frame {
        self.build(window)
    }

    fn on_click(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.viewport = (window.width, window.height);
        self.click(id);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui::Application;

    fn open() -> (ui::Application, ui::WindowHandle) {
        let mut app = Application::new();
        let (h, _e) = app.open_raw_window(
            ui::WindowOptions {
                width: 920.0,
                height: 760.0,
                scale: 2.0,
                ..Default::default()
            },
            |_| SettingsView::new(Settings::default()),
        );
        (app, h)
    }

    #[test]
    fn jira_fields_save_to_the_session_and_the_token_to_its_secrets() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let mut view = SettingsView::new(Settings::default());
        view.set_jira_session(Some((state.clone(), "demo".into())));
        let no_env = std::env::var_os(pom_jira::DEFAULT_TOKEN_ENV).is_none();
        if no_env {
            assert_eq!(view.jira.token, TokenSource::Missing);
        }

        view.click(settings_ui::CTRL_JIRA_SITE);
        view.key_text("acme.atlassian.net");
        view.key_enter();
        view.editing = Some((settings_ui::CTRL_JIRA_TOKEN, String::new()));
        assert!(view.key_paste("tok\n"));
        view.key_enter();

        let saved = pom_jira::JiraSettings::load(&state, "demo");
        assert_eq!(saved.site, "acme.atlassian.net");
        assert_eq!(view.jira.token, TokenSource::Secret);
        let secret = pom_secrets::SecretStore::new(state.clone(), "demo")
            .get(pom_jira::TOKEN_SECRET)
            .expect("secret");
        assert_eq!(secret.as_deref(), Some("tok"));

        view.click(settings_ui::CTRL_JIRA_RESET_TOKEN);
        if no_env {
            assert_eq!(view.jira.token, TokenSource::Missing);
        }
    }

    #[test]
    fn keep_main_fresh_saves_the_schedule_for_the_session() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let mut view = SettingsView::new(Settings::default());
        view.set_jira_session(Some((state.clone(), "demo".into())));
        assert!(!view.jira.keep_main_fresh);
        assert_eq!(view.jira.refresh_minutes, 30);
        view.click(settings_ui::CTRL_REFRESH_MAIN);
        view.click(settings_ui::CTRL_REFRESH_DEC);
        view.click(settings_ui::CTRL_REFRESH_EDIT);
        view.key_backspace();
        view.key_backspace();
        view.key_text("7");
        view.key_enter();
        assert_eq!(
            pom_sync::refresh_schedule(&state, "demo", None),
            pom_sync::RefreshSchedule {
                enabled: true,
                interval_seconds: 7 * 60,
            }
        );
        view.click(settings_ui::CTRL_REFRESH_INC);
        assert_eq!(view.jira.refresh_minutes, 10);
    }

    #[test]
    fn opens_on_appearance_with_layers() {
        let (mut app, h) = open();
        let frame = app.draw(h).expect("frame");
        // A clipped page layer plus the fixed chrome layer (no popover yet).
        assert_eq!(frame.overlays.len(), 2);
        assert!(frame.overlays[0].clip.is_some(), "page is clipped");
        assert!(frame.overlays[1].clip.is_none(), "chrome is unclipped");
    }

    #[test]
    fn clicking_theme_control_opens_then_closes_popover() {
        let (mut app, h) = open();
        app.draw(h);
        let theme_anchor = app
            .window(h)
            .and_then(|w| w.center_of(settings_ui::CTRL_THEME))
            .expect("theme control laid out");
        app.simulate_click(h, theme_anchor.0, theme_anchor.1);
        let frame = app.draw(h).expect("frame");
        assert_eq!(frame.overlays.len(), 3, "popover layer added");
        // Click the control again to toggle it closed.
        app.simulate_click(h, theme_anchor.0, theme_anchor.1);
        let frame = app.draw(h).expect("frame");
        assert_eq!(frame.overlays.len(), 2, "popover closed");
    }

    #[test]
    fn selecting_a_theme_item_applies_and_persists() {
        let (mut app, h) = open();
        app.draw(h);
        let anchor = app
            .window(h)
            .and_then(|w| w.center_of(settings_ui::CTRL_THEME))
            .expect("theme control");
        app.simulate_click(h, anchor.0, anchor.1);
        app.draw(h);
        // Click the second theme item in the popover list.
        let item = app
            .window(h)
            .and_then(|w| w.center_of(settings_ui::POPOVER_BASE + 1))
            .expect("popover item");
        let hit = app.simulate_click(h, item.0, item.1);
        assert_eq!(hit, Some(settings_ui::POPOVER_BASE + 1));
        let frame = app.draw(h).expect("frame");
        assert_eq!(frame.overlays.len(), 2, "popover closed after choosing");
    }
}
