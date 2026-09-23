//! Pomelo app shell (cross-platform, winit + wgpu). This stage builds the LAYOUT: a top bar, a resizable left dock
//! (the workspace-list panel — our addition) that collapses to an icon rail instead of vanishing, and the
//! content area (colored placeholders for now). Panels/tabs, file tree, terminal and the ported core come next.
//! This crate is the composition root only: the framework lives in `ui`, the
//! layout/dock system in `workspace`, self-update in `auto_update`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use settings::Settings;
use ui::UiRenderer;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, Window, WindowId};
use workspace::{DockPosition, EditKey, Layout};

enum EditorInput {
    Key(EditKey),
    Text(String),
}

/// Apply the persisted dock layout (widths, collapsed state, and the side/hidden of every button) onto a fresh
/// `Layout`, so the user's arrangement is restored on launch and for windows opened later.
fn apply_dock_settings(s: &Settings, layout: &mut Layout) {
    layout.left.width = s.left_dock_width;
    layout.left.collapsed = s.left_dock_collapsed;
    layout.right.width = s.right_dock_width;
    layout.right.collapsed = s.right_dock_collapsed;
    layout.bottom.collapsed = s.bottom_dock_collapsed;
    layout.sidebar_side = DockPosition::from_side(&s.sidebar_side);
    layout.agent_side = DockPosition::from_side(&s.agent_side);
    layout.terminal_side = DockPosition::from_side(&s.terminal_side);
    layout.agent_hidden = s.agent_hidden;
    layout.terminal_hidden = s.terminal_hidden;
    for (i, side) in s.func_sides.iter().enumerate() {
        if let Some(slot) = layout.func_side.get_mut(i) {
            *slot = DockPosition::from_side(side);
        }
    }
    for (i, hidden) in s.func_hidden.iter().enumerate() {
        if let Some(slot) = layout.func_hidden.get_mut(i) {
            *slot = *hidden;
        }
    }
}

fn files_root() -> std::path::PathBuf {
    use std::path::{Path, PathBuf};
    if let Some(p) = std::env::var_os("POMELO_FILES_ROOT") {
        return PathBuf::from(p);
    }
    if let Ok(cwd) = std::env::current_dir() {
        if cwd != Path::new("/") {
            return cwd;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.as_path();
        while let Some(parent) = dir.parent() {
            if parent.join("Cargo.toml").is_file() {
                return parent.to_path_buf();
            }
            dir = parent;
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn install_features(layout: &mut Layout) {
    layout.files_view = Some(Box::new(files_ui::FilesView::new(files_root())));
}

/// Read the current dock layout back into settings for persistence.
fn read_dock_settings(s: &mut Settings, layout: &Layout) {
    s.left_dock_width = layout.left.width;
    s.right_dock_width = layout.right.width;
    s.left_dock_collapsed = layout.left.collapsed;
    s.right_dock_collapsed = layout.right.collapsed;
    s.bottom_dock_collapsed = layout.bottom.collapsed;
    s.sidebar_side = layout.sidebar_side.as_str().into();
    s.agent_side = layout.agent_side.as_str().into();
    s.terminal_side = layout.terminal_side.as_str().into();
    s.agent_hidden = layout.agent_hidden;
    s.terminal_hidden = layout.terminal_hidden;
    s.func_sides = layout
        .func_side
        .iter()
        .map(|d| d.as_str().to_string())
        .collect();
    s.func_hidden = layout.func_hidden.clone();
}

/// One on-screen main window: its winit handle + GPU renderer, plus the framework `WindowHandle`/`WorkspaceView`
/// entity it drives. Many can coexist (one per session opened in a new window).
struct MainWindow {
    window: Arc<Window>,
    ui: UiRenderer,
    handle: ui::WindowHandle,
    entity: ui::Entity<workspace::WorkspaceView>,
    cursor: (f64, f64),
    dirty: bool,
}

#[derive(Default)]
struct App {
    // All main windows share one reactive `Application` (the framework's single App, many windows); each `MainWindow`
    // drives a `WorkspaceView` entity. The binary owns the winit windows + GPU renderers.
    main_app: Option<ui::Application>,
    mains: std::collections::HashMap<WindowId, MainWindow>,
    settings: Settings,
    // Settings is its own OS window (opened on Cmd+,), not an overlay on the main window. Its state + input
    // live in a reactive `SettingsView` driven by a `ui::Application`; the binary only owns the winit window +
    // GPU renderer and pumps draw/input into the framework.
    settings_window: Option<Arc<Window>>,
    settings_ui: Option<UiRenderer>,
    settings_app: Option<ui::Application>,
    settings_handle: Option<ui::WindowHandle>,
    settings_entity: Option<ui::Entity<settings_ui::SettingsView>>,
    caret_last_toggle: Option<Instant>,
    /// Cmd+K was pressed; the next key completes a two-stroke editor binding.
    pending_cmd_k: bool,
    settings_dirty: bool,
    settings_cursor: (f64, f64),
    super_down: bool,
    shift_down: bool,
    alt_down: bool,
    ctrl_down: bool,
}

impl App {
    /// Create a real OS window hosting a `WorkspaceView` for `layout`, wire its renderer + macOS chrome, and
    /// register it. Returns its `WindowId`. Used both for the first window and for "Open in new window".
    fn new_main_window(&mut self, event_loop: &ActiveEventLoop, layout: Layout) -> WindowId {
        let mut attrs = Window::default_attributes()
            .with_title("Pomelo")
            .with_inner_size(LogicalSize::new(
                self.settings.window_width,
                self.settings.window_height,
            ));
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        // Input methods (e.g. Vietnamese Telex) compose through Ime events; plain keys still arrive as input.
        window.set_ime_allowed(true);
        let id = window.id();
        let mut renderer = UiRenderer::new(window.clone()).expect("ui");
        renderer.set_ui_font(&self.settings.ui_font);
        let (w, h) = renderer.size();
        let scale = window.scale_factor() as f32;
        #[cfg(target_os = "macos")]
        {
            center_traffic_lights(&window);
            pin_layer_top_left(&window);
        }
        let app = self.main_app.get_or_insert_with(ui::Application::new);
        let (handle, entity) = app.open_raw_window::<workspace::WorkspaceView>(
            ui::WindowOptions {
                title: "Pomelo".into(),
                width: w,
                height: h,
                scale,
            },
            move |_| workspace::WorkspaceView::new(layout),
        );
        self.mains.insert(
            id,
            MainWindow {
                window,
                ui: renderer,
                handle,
                entity,
                cursor: (0.0, 0.0),
                dirty: false,
            },
        );
        id
    }

    fn draw_main(&mut self, id: WindowId) {
        let Some(app) = self.main_app.as_mut() else {
            return;
        };
        let Some(m) = self.mains.get_mut(&id) else {
            return;
        };
        let (w, h) = m.ui.size();
        let scale = m.window.scale_factor() as f32;
        app.resize(m.handle, w, h, scale);
        let Some(frame) = app.draw(m.handle) else {
            return;
        };
        let black = ui::Rgba::new(0.0, 0.0, 0.0, 1.0);
        let mut layers: Vec<ui::Layer> = Vec::with_capacity(frame.overlays.len() + 1);
        layers.push((
            frame.base.rects.as_slice(),
            frame.base.tris.as_slice(),
            frame.base.texts.as_slice(),
            frame.base.icons.as_slice(),
            None,
        ));
        for o in &frame.overlays {
            let clip = o.clip.map(|r| (r.x, r.y, r.w, r.h));
            layers.push((
                o.painted.rects.as_slice(),
                o.painted.tris.as_slice(),
                o.painted.texts.as_slice(),
                o.painted.icons.as_slice(),
                clip,
            ));
        }
        if let Err(e) = m.ui.render_frame(black, &layers) {
            eprintln!("ui render error: {e}");
        }
    }

    fn draw_settings(&mut self) {
        // Sync the winit window's logical size + scale into the view, draw its Frame, then composite: layer 0
        // clears, each overlay (clipped page, fixed chrome, popover) draws above it.
        let (w, h) = match self.settings_ui.as_ref() {
            Some(ui) => ui.size(),
            None => return,
        };
        let scale = self
            .settings_window
            .as_ref()
            .map(|win| win.scale_factor() as f32)
            .unwrap_or(2.0);
        let (Some(app), Some(handle)) = (self.settings_app.as_mut(), self.settings_handle) else {
            return;
        };
        app.resize(handle, w, h, scale);
        let Some(frame) = app.draw(handle) else {
            return;
        };
        let clear = ui::theme().editor_background;
        let mut layers: Vec<ui::Layer> = Vec::with_capacity(frame.overlays.len() + 1);
        layers.push((
            frame.base.rects.as_slice(),
            frame.base.tris.as_slice(),
            frame.base.texts.as_slice(),
            frame.base.icons.as_slice(),
            None,
        ));
        for o in &frame.overlays {
            let clip = o.clip.map(|r| (r.x, r.y, r.w, r.h));
            layers.push((
                o.painted.rects.as_slice(),
                o.painted.tris.as_slice(),
                o.painted.texts.as_slice(),
                o.painted.icons.as_slice(),
                clip,
            ));
        }
        if let Some(ui) = self.settings_ui.as_mut() {
            if let Err(e) = ui.render_frame(clear, &layers) {
                eprintln!("settings render error: {e}");
            }
        }
    }

    /// Toggle the Settings window: open a real second window, or close it if already open.
    fn toggle_settings(&mut self, event_loop: &ActiveEventLoop) {
        if self.settings_window.is_some() {
            self.settings_ui = None;
            self.settings_window = None;
            self.settings_app = None;
            self.settings_handle = None;
            self.settings_entity = None;
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("Settings")
            .with_inner_size(LogicalSize::new(920.0, 760.0));
        // Self-managed title bar like the main window: content fills under transparent titlebar, traffic
        // lights stay top-left, no native title strip. The settings page insets its top for them.
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true);
        }
        let win = Arc::new(event_loop.create_window(attrs).expect("settings window"));
        let renderer = UiRenderer::new(win.clone()).expect("settings ui");
        #[cfg(target_os = "macos")]
        pin_layer_top_left(&win);
        let (w, h) = renderer.size();
        let scale = win.scale_factor() as f32;
        let fonts = renderer.font_families();
        self.settings_ui = Some(renderer);

        // The settings window is a real framework window: its state + input live in a `SettingsView` entity
        // driven by a `ui::Application`; the binary only pumps draw/input into it.
        let mut app = ui::Application::new();
        let settings = self.settings.clone();
        let (handle, entity) = app.open_raw_window::<settings_ui::SettingsView>(
            ui::WindowOptions {
                title: "Settings".into(),
                width: w,
                height: h,
                scale,
            },
            move |_| {
                let mut view = settings_ui::SettingsView::new(settings);
                view.set_fonts(fonts);
                view
            },
        );
        self.settings_app = Some(app);
        self.settings_handle = Some(handle);
        self.settings_entity = Some(entity);
        self.settings_window = Some(win);
        self.apply_ui_font();
        self.draw_settings();
    }

    /// Apply the configured UI font (".PomeloSans"/".PomeloMono"/a family name) to every window's renderer.
    fn apply_ui_font(&mut self) {
        let font = self.settings.ui_font.clone();
        for m in self.mains.values_mut() {
            m.ui.set_ui_font(&font);
        }
        if let Some(u) = self.settings_ui.as_mut() {
            u.set_ui_font(&font);
        }
    }

    /// Apply the configured theme to the global palette the whole UI reads (selected by name).
    fn apply_theme(&self) {
        ui::set_theme(ui::by_name(&self.settings.theme));
    }

    /// Force the caret visible and restart its blink cycle (called on keystrokes so it doesn't blink off mid-type).
    fn reset_caret(&mut self) {
        ui::set_caret_phase(true);
        self.caret_last_toggle = Some(Instant::now());
    }

    /// Push the configured UI font size into the global text scale so all interface text zooms with it.
    fn apply_font_scale(&self) {
        ui::set_ui_text_scale(self.settings.ui_font_size / ui::UI_FONT_BASE);
    }

    /// Push the configured UI font weight into the global so all interface text is shaped at it.
    fn apply_font_weight(&self) {
        ui::set_ui_font_weight(self.settings.ui_font_weight as u16);
    }

    /// After routing an input to the settings view, apply the cross-window side-effects it flagged: re-apply
    /// the UI font to every renderer, and/or repaint the main window because a shared global changed. Also
    /// mirror the view's settings back so the binary persists them and repaints the settings window.
    fn sync_settings_side_effects(&mut self) {
        let Some(entity) = self.settings_entity.clone() else {
            return;
        };
        let Some(app) = self.settings_app.as_mut() else {
            return;
        };
        let effects = entity.update(app.app_mut(), |v, _| v.take_side_effects());
        self.settings = entity.read(app.app()).settings().clone();
        if effects.reapply_font {
            self.apply_ui_font();
        }
        if effects.reapply_font || effects.redraw_others {
            self.mark_all_mains_dirty();
        }
        // Any view mutation (click/scroll/key) may have changed the frame; repaint the settings window.
        self.settings_dirty = true;
    }

    fn mark_all_mains_dirty(&mut self) {
        for m in self.mains.values_mut() {
            m.dirty = true;
        }
    }

    /// Run `f` against the settings view entity, scoping the app+entity borrow so callers can then touch other
    /// fields (draw, side-effects). Returns None if the settings window is closed.
    fn with_settings_view<R>(
        &mut self,
        f: impl FnOnce(&mut settings_ui::SettingsView, &mut ui::Context<settings_ui::SettingsView>) -> R,
    ) -> Option<R> {
        let entity = self.settings_entity.clone()?;
        let app = self.settings_app.as_mut()?;
        Some(entity.update(app.app_mut(), f))
    }

    fn settings_simulate_move(&mut self, x: f32, y: f32) -> Option<u64> {
        let handle = self.settings_handle?;
        self.settings_app
            .as_mut()
            .and_then(|a| a.simulate_move(handle, x, y))
    }

    fn settings_simulate_click(&mut self, x: f32, y: f32) {
        if let (Some(app), Some(handle)) = (self.settings_app.as_mut(), self.settings_handle) {
            app.simulate_click(handle, x, y);
        }
    }

    /// Run `f` against a specific main window's `WorkspaceView`, scoping the app+entity borrow. None if gone.
    fn with_workspace_view<R>(
        &mut self,
        id: WindowId,
        f: impl FnOnce(&mut workspace::WorkspaceView, &mut ui::Context<workspace::WorkspaceView>) -> R,
    ) -> Option<R> {
        let entity = self.mains.get(&id)?.entity.clone();
        let app = self.main_app.as_mut()?;
        Some(entity.update(app.app_mut(), f))
    }

    /// After routing an input to a main view, apply the effects it flagged: persist dock geometry, and open a
    /// new window for a session ("Open in new window") as a real second OS window.
    fn sync_workspace_effects(&mut self, id: WindowId, event_loop: &ActiveEventLoop) {
        let Some(entity) = self.mains.get(&id).map(|m| m.entity.clone()) else {
            return;
        };
        let Some(app) = self.main_app.as_mut() else {
            return;
        };
        let effects = entity.update(app.app_mut(), |v, _| v.take_effects());
        if effects.persist {
            let view = entity.read(app.app());
            read_dock_settings(&mut self.settings, view.layout());
            drop(view);
            let _ = self.settings.save();
        }
        if let Some(session) = effects.open_new_window {
            // Real multi-window: clone the current dock layout and open another OS window for the session.
            let mut layout = Layout::default();
            apply_dock_settings(&self.settings, &mut layout);
            install_features(&mut layout);
            if session < layout.sessions.len() {
                layout.current_session = session;
            }
            self.new_main_window(event_loop, layout);
        }
        if let Some(m) = self.mains.get_mut(&id) {
            m.dirty = true;
        }
    }

    /// Whether a given main window's session menu is open (drives caret blink + wheel routing).
    fn main_menu_open(&self, id: WindowId) -> bool {
        match (self.mains.get(&id), self.main_app.as_ref()) {
            (Some(m), Some(a)) => m.entity.read(a.app()).menu_open(),
            _ => false,
        }
    }

    /// Whether any main window has its session menu open (for the caret-blink wake condition).
    fn any_menu_open(&self) -> bool {
        let Some(app) = self.main_app.as_ref() else {
            return false;
        };
        self.mains
            .values()
            .any(|m| m.entity.read(app.app()).menu_open())
    }

    /// Persist dock geometry read from any one main view (on quit / dock change).
    fn persist_settings(&mut self) {
        if let (Some(app), Some(m)) = (self.main_app.as_ref(), self.mains.values().next()) {
            let view = m.entity.read(app.app());
            read_dock_settings(&mut self.settings, view.layout());
        }
        let _ = self.settings.save();
    }
}

const CARET_BLINK: Duration = Duration::from_millis(500);

impl ApplicationHandler for App {
    // Blink the settings caret: while the settings window is open, wake on a timer to toggle caret visibility.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Coalesce per-window redraws (scroll/hover bursts) into one per iteration.
        let dirty: Vec<WindowId> = self
            .mains
            .iter()
            .filter(|(_, m)| m.dirty)
            .map(|(id, _)| *id)
            .collect();
        for id in dirty {
            if let Some(m) = self.mains.get_mut(&id) {
                m.dirty = false;
            }
            self.draw_main(id);
        }
        let ticking: Vec<WindowId> = match self.main_app.as_ref() {
            Some(a) => self
                .mains
                .iter()
                .filter(|(_, m)| m.entity.read(a.app()).ticking())
                .map(|(id, _)| *id)
                .collect(),
            None => Vec::new(),
        };
        for id in &ticking {
            self.draw_main(*id);
        }
        let editor_windows: Vec<WindowId> = match self.main_app.as_ref() {
            Some(a) => self
                .mains
                .iter()
                .filter(|(_, m)| m.entity.read(a.app()).editor_focused())
                .map(|(id, _)| *id)
                .collect(),
            None => Vec::new(),
        };
        let needs_caret =
            self.settings_window.is_some() || self.any_menu_open() || !editor_windows.is_empty();
        if !needs_caret && ticking.is_empty() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        // Coalesce hot-path updates (scroll, hover) into one redraw per loop iteration so a burst of trackpad
        // wheel events doesn't trigger a redraw storm.
        if self.settings_dirty {
            self.settings_dirty = false;
            self.draw_settings();
        }
        let now = Instant::now();
        let mut wake = if ticking.is_empty() {
            None
        } else {
            Some(now + Duration::from_millis(33))
        };
        if needs_caret {
            let last = *self.caret_last_toggle.get_or_insert(now);
            let caret_wake = if now.duration_since(last) >= CARET_BLINK {
                self.caret_last_toggle = Some(now);
                ui::set_caret_phase(!ui::caret_phase());
                if self.settings_window.is_some() {
                    self.draw_settings();
                }
                let menu_windows: Vec<WindowId> = self
                    .mains
                    .keys()
                    .copied()
                    .filter(|id| self.main_menu_open(*id))
                    .collect();
                for id in menu_windows {
                    self.draw_main(id);
                }
                for id in &editor_windows {
                    self.draw_main(*id);
                }
                now + CARET_BLINK
            } else {
                last + CARET_BLINK
            };
            wake = Some(wake.map_or(caret_wake, |w| w.min(caret_wake)));
        }
        match wake {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.mains.is_empty() {
            return;
        }
        self.settings = Settings::load();
        self.apply_theme();
        self.apply_font_scale();
        self.apply_font_weight();
        ui::set_chrome(settings_ui::chrome_flags(&self.settings));
        #[cfg(target_os = "macos")]
        set_dock_icon();
        auto_update::spawn_background_check();

        let mut layout = Layout::default();
        apply_dock_settings(&self.settings, &mut layout);
        install_features(&mut layout);
        self.new_main_window(event_loop, layout);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.mains.is_empty() {
            return;
        }
        // Cmd+, toggles the Settings window from anywhere; Esc closes it if focused.
        if let WindowEvent::ModifiersChanged(mods) = &event {
            self.super_down = mods.state().super_key();
            self.shift_down = mods.state().shift_key();
            self.alt_down = mods.state().alt_key();
            self.ctrl_down = mods.state().control_key();
        }
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed {
                let toggle = matches!(&ke.logical_key, Key::Character(c) if c.as_str() == ",")
                    && self.super_down;
                let esc_on_settings = ke.logical_key == Key::Named(NamedKey::Escape)
                    && self.settings_window.as_ref().map(|w| w.id()) == Some(id);
                if toggle {
                    self.toggle_settings(event_loop);
                    return;
                }
                if esc_on_settings {
                    self.settings_ui = None;
                    self.settings_window = None;
                    return;
                }
            }
        }
        // Type into a main window's open session-menu search field.
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed
                && self.mains.contains_key(&id)
                && self.main_menu_open(id)
            {
                self.reset_caret();
                let changed = match &ke.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        self.with_workspace_view(id, |v, _| v.key_escape())
                    }
                    Key::Named(NamedKey::Backspace) => {
                        self.with_workspace_view(id, |v, _| v.key_backspace())
                    }
                    _ => {
                        let text = ke.text.as_ref().map(|t| t.to_string());
                        match text {
                            Some(t) => Some(
                                self.with_workspace_view(id, |v, _| v.key_text(&t)) == Some(true),
                            ),
                            None => Some(false),
                        }
                    }
                };
                if changed == Some(true) {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
                return;
            }
        }
        if let WindowEvent::Ime(ime) = &event {
            if self.mains.contains_key(&id)
                && self.with_workspace_view(id, |v, _| v.editor_focused()) == Some(true)
            {
                self.reset_caret();
                let changed = match ime {
                    Ime::Preedit(text, cursor) => {
                        let selected = cursor.map(|(start, end)| {
                            text[..start.min(text.len())].chars().count()
                                ..text[..end.min(text.len())].chars().count()
                        });
                        self.with_workspace_view(id, |v, _| v.editor_ime_preedit(text, selected))
                    }
                    Ime::Commit(text) => {
                        self.with_workspace_view(id, |v, _| v.editor_ime_commit(text))
                    }
                    Ime::Enabled | Ime::Disabled => None,
                };
                if changed == Some(true) {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
                return;
            }
        }
        if let WindowEvent::KeyboardInput { event: ke, .. } = &event {
            if ke.state == ElementState::Pressed
                && self.mains.contains_key(&id)
                && self.with_workspace_view(id, |v, _| v.editor_focused()) == Some(true)
            {
                let (shift, cmd, alt, ctrl) = (
                    self.shift_down,
                    self.super_down,
                    self.alt_down,
                    self.ctrl_down,
                );
                if cmd {
                    if let Key::Character(c) = &ke.logical_key {
                        match c.as_str() {
                            "s" => {
                                self.with_workspace_view(id, |v, _| {
                                    if let Some(Err(reason)) = v.editor_save() {
                                        v.show_toast(reason, None);
                                    }
                                });
                                if let Some(m) = self.mains.get_mut(&id) {
                                    m.dirty = true;
                                }
                                return;
                            }
                            "c" => {
                                self.with_workspace_view(id, |v, _| v.editor_copy_to_clipboard());
                                return;
                            }
                            "x" | "v" => {
                                self.reset_caret();
                                let changed = self.with_workspace_view(id, |v, _| {
                                    if c.as_str() == "x" {
                                        v.editor_cut_to_clipboard()
                                    } else {
                                        v.editor_paste_from_clipboard()
                                    }
                                });
                                if changed == Some(true) {
                                    if let Some(m) = self.mains.get_mut(&id) {
                                        m.dirty = true;
                                    }
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                let after_cmd_k = std::mem::take(&mut self.pending_cmd_k);
                if cmd
                    && !shift
                    && matches!(&ke.logical_key, Key::Character(c) if c.as_str() == "k")
                {
                    self.pending_cmd_k = true;
                    return;
                }
                let key = |k| Some(EditorInput::Key(k));
                let input: Option<EditorInput> = match &ke.logical_key {
                    Key::Character(c) if after_cmd_k && !cmd => match c.as_str() {
                        "z" => key(EditKey::ToggleSoftWrap),
                        _ => None,
                    },
                    Key::Named(NamedKey::ArrowRight) if cmd && alt => {
                        key(EditKey::SetPickerPreviewRight)
                    }
                    Key::Named(NamedKey::ArrowLeft) if cmd => key(EditKey::Home),
                    Key::Named(NamedKey::ArrowRight) if cmd => key(EditKey::End),
                    Key::Named(NamedKey::ArrowUp) if cmd && alt => key(EditKey::AddCursorAbove),
                    Key::Named(NamedKey::ArrowDown) if cmd && alt => key(EditKey::AddCursorBelow),
                    Key::Named(NamedKey::ArrowRight) if cmd && ctrl => {
                        key(EditKey::SelectLargerSyntaxNode)
                    }
                    Key::Named(NamedKey::ArrowLeft) if cmd && ctrl => {
                        key(EditKey::SelectSmallerSyntaxNode)
                    }
                    Key::Named(NamedKey::ArrowRight) if ctrl && shift => {
                        key(EditKey::SelectLargerSyntaxNode)
                    }
                    Key::Named(NamedKey::ArrowLeft) if ctrl && shift => {
                        key(EditKey::SelectSmallerSyntaxNode)
                    }
                    Key::Named(NamedKey::ArrowUp) if alt && shift => key(EditKey::DuplicateLineUp),
                    Key::Named(NamedKey::ArrowDown) if alt && shift => {
                        key(EditKey::DuplicateLineDown)
                    }
                    Key::Named(NamedKey::ArrowUp) if alt => key(EditKey::MoveLineUp),
                    Key::Named(NamedKey::ArrowDown) if alt => key(EditKey::MoveLineDown),
                    Key::Named(NamedKey::ArrowUp) if cmd => key(EditKey::DocumentStart),
                    Key::Named(NamedKey::ArrowDown) if cmd => key(EditKey::DocumentEnd),
                    Key::Named(NamedKey::Home) if cmd => key(EditKey::DocumentStart),
                    Key::Named(NamedKey::End) if cmd => key(EditKey::DocumentEnd),
                    Key::Named(NamedKey::ArrowLeft) if ctrl && alt => key(EditKey::SubwordLeft),
                    Key::Named(NamedKey::ArrowRight) if ctrl && alt => key(EditKey::SubwordRight),
                    Key::Named(NamedKey::ArrowLeft) if alt => key(EditKey::WordLeft),
                    Key::Named(NamedKey::ArrowRight) if alt => key(EditKey::WordRight),
                    Key::Named(NamedKey::ArrowLeft) => key(EditKey::Left),
                    Key::Named(NamedKey::ArrowRight) => key(EditKey::Right),
                    Key::Named(NamedKey::ArrowUp) => key(EditKey::Up),
                    Key::Named(NamedKey::ArrowDown) => key(EditKey::Down),
                    Key::Named(NamedKey::Home) => key(EditKey::Home),
                    Key::Named(NamedKey::End) => key(EditKey::End),
                    Key::Named(NamedKey::F8) if cmd && shift => key(EditKey::GoToPreviousHunk),
                    Key::Named(NamedKey::F8) if cmd => key(EditKey::GoToHunk),
                    Key::Named(NamedKey::Space) if ctrl && !cmd => key(EditKey::ShowCompletions),
                    Key::Named(NamedKey::PageUp) => key(EditKey::PageUp),
                    Key::Named(NamedKey::PageDown) => key(EditKey::PageDown),
                    Key::Named(NamedKey::Backspace) if cmd => key(EditKey::DeleteToLineStart),
                    Key::Named(NamedKey::Backspace) if ctrl && alt => {
                        key(EditKey::DeleteSubwordLeft)
                    }
                    Key::Named(NamedKey::Backspace) if alt => key(EditKey::DeleteWordLeft),
                    Key::Named(NamedKey::Backspace) => key(EditKey::Backspace),
                    Key::Named(NamedKey::Delete) if cmd => key(EditKey::DeleteToLineEnd),
                    Key::Named(NamedKey::Delete) if ctrl && alt => key(EditKey::DeleteSubwordRight),
                    Key::Named(NamedKey::Delete) if alt => key(EditKey::DeleteWordRight),
                    Key::Named(NamedKey::Delete) => key(EditKey::Delete),
                    Key::Named(NamedKey::Enter) if alt => key(EditKey::SelectAllMatchesInSearch),
                    Key::Named(NamedKey::Enter) if cmd => key(EditKey::ReplaceAll),
                    Key::Named(NamedKey::Enter) => key(EditKey::Enter),
                    Key::Named(NamedKey::Escape) => key(EditKey::Escape),
                    // Emacs-style Control bindings macOS text fields share.
                    Key::Character(c) if ctrl && !cmd && !alt => match c.as_str() {
                        "a" => key(EditKey::LineStart),
                        "e" => key(EditKey::LineEnd),
                        "b" => key(EditKey::Left),
                        "f" => key(EditKey::Right),
                        "p" => key(EditKey::Up),
                        "n" => key(EditKey::Down),
                        "h" => key(EditKey::Backspace),
                        "d" => key(EditKey::Delete),
                        "w" => key(EditKey::DeleteWordLeft),
                        "t" => key(EditKey::Transpose),
                        "g" => key(EditKey::ToggleGoToLine),
                        "-" => key(EditKey::GoBack),
                        "_" => key(EditKey::GoForward),
                        "j" => key(EditKey::JoinLines),
                        "m" => key(EditKey::MoveToEnclosingBracket),
                        _ => None,
                    },
                    Key::Named(NamedKey::Tab) if shift => key(EditKey::Outdent),
                    Key::Named(NamedKey::Tab) => key(EditKey::Tab),
                    Key::Character(_) if cmd && alt => match ke.key_without_modifiers() {
                        Key::Character(c) => match c.as_str() {
                            "c" => key(EditKey::ToggleSearchCaseSensitive),
                            "p" => key(EditKey::TogglePickerPreview),
                            "z" => key(EditKey::GitRestore),
                            "y" => key(EditKey::ToggleStaged),
                            "w" => key(EditKey::ToggleSearchWholeWord),
                            "x" => key(EditKey::ToggleSearchRegex),
                            _ => None,
                        },
                        _ => None,
                    },
                    Key::Character(c) if cmd => match c.as_str() {
                        "f" => key(EditKey::DeploySearch),
                        "g" | "G" if shift => key(EditKey::SelectPreviousMatch),
                        "g" => key(EditKey::SelectNextMatch),
                        "h" | "H" if shift => key(EditKey::ToggleSearchReplace),
                        "e" => key(EditKey::UseSelectionForFind),
                        "z" | "Z" if shift => key(EditKey::Redo),
                        "y" | "Y" if shift => key(EditKey::UnstageAndNext),
                        "y" => key(EditKey::StageAndNext),
                        "\"" => key(EditKey::ExpandAllDiffHunks),
                        "'" => key(EditKey::ToggleSelectedDiffHunks),
                        "k" | "K" if shift => key(EditKey::DeleteLine),
                        "l" | "L" if shift => key(EditKey::SelectAllMatches),
                        "p" | "P" if shift && !ctrl => key(EditKey::ToggleCommandPalette),
                        "o" | "O" if shift => key(EditKey::ToggleOutline),
                        "p" if ctrl => key(EditKey::AddCursorAboveRow),
                        "n" if ctrl => key(EditKey::AddCursorBelowRow),
                        "d" if !ctrl => key(EditKey::SelectNext),
                        "z" => key(EditKey::Undo),
                        "a" => key(EditKey::SelectAll),
                        "[" => key(EditKey::Outdent),
                        "]" => key(EditKey::Indent),
                        "/" => key(EditKey::ToggleComments),
                        "|" | "\\" if shift => key(EditKey::MoveToEnclosingBracket),
                        _ => None, // leave copy/paste/other Cmd shortcuts to their own handlers
                    },
                    _ if cmd => None,
                    _ => ke.text.as_ref().map(|t| EditorInput::Text(t.to_string())),
                };
                if let Some(input) = input {
                    self.reset_caret();
                    let changed = match input {
                        EditorInput::Key(k) => {
                            self.with_workspace_view(id, |v, _| v.editor_key(k, shift))
                        }
                        EditorInput::Text(t) => {
                            self.with_workspace_view(id, |v, _| v.editor_text(&t))
                        }
                    };
                    if changed == Some(true) {
                        if let Some(m) = self.mains.get_mut(&id) {
                            m.dirty = true;
                        }
                    }
                    return;
                }
            }
        }

        // Route the rest to whichever window the event belongs to.
        if self.settings_window.as_ref().map(|w| w.id()) == Some(id) {
            let scale = self
                .settings_window
                .as_ref()
                .map(|w| w.scale_factor() as f32)
                .unwrap_or(2.0);
            match event {
                WindowEvent::CloseRequested => {
                    self.settings_ui = None;
                    self.settings_window = None;
                    self.settings_app = None;
                    self.settings_handle = None;
                    self.settings_entity = None;
                }
                WindowEvent::Resized(size) => {
                    if let Some(ui) = self.settings_ui.as_mut() {
                        ui.resize(size.width, size.height);
                    }
                    self.draw_settings();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    self.settings_cursor = (position.x, position.y);
                    let (lx, ly) = (position.x as f32 / scale, position.y as f32 / scale);
                    let hit = self.settings_simulate_move(lx, ly);
                    if let Some(win) = self.settings_window.as_ref() {
                        win.set_cursor(if hit.is_some() {
                            CursorIcon::Pointer
                        } else {
                            CursorIcon::Default
                        });
                    }
                    if self.with_settings_view(|v, _| v.hover(lx, ly)) == Some(true) {
                        self.settings_dirty = true;
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let (cx, cy) = (
                        self.settings_cursor.0 as f32 / scale,
                        self.settings_cursor.1 as f32 / scale,
                    );
                    let dy = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y * 30.0,
                        winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    };
                    if self.with_settings_view(|v, _| v.scroll(dy, cx, cy)) == Some(true) {
                        self.settings_dirty = true;
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    let (lx, ly) = (
                        self.settings_cursor.0 as f32 / scale,
                        self.settings_cursor.1 as f32 / scale,
                    );
                    self.settings_simulate_click(lx, ly);
                    self.sync_settings_side_effects();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state != ElementState::Pressed {
                        return;
                    }
                    self.reset_caret();
                    let changed = match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            self.with_settings_view(|v, _| v.key_escape())
                        }
                        Key::Named(NamedKey::Enter) => {
                            self.with_settings_view(|v, _| v.key_enter())
                        }
                        Key::Named(NamedKey::Backspace) => {
                            self.with_settings_view(|v, _| v.key_backspace())
                        }
                        _ => {
                            let text = event.text.as_ref().map(|t| t.to_string());
                            text.map(|t| {
                                self.with_settings_view(|v, _| v.key_text(&t)) == Some(true)
                            })
                            .map(Some)
                            .unwrap_or(Some(false))
                        }
                    };
                    if changed == Some(true) {
                        self.sync_settings_side_effects();
                    }
                }
                WindowEvent::RedrawRequested => self.draw_settings(),
                _ => {}
            }
            return;
        }

        // A main window event.
        if !self.mains.contains_key(&id) {
            return;
        }
        let scale = self
            .mains
            .get(&id)
            .map(|m| m.window.scale_factor() as f32)
            .unwrap_or(2.0);
        match event {
            WindowEvent::CloseRequested => {
                self.persist_settings();
                if let Some(m) = self.mains.remove(&id) {
                    if let Some(app) = self.main_app.as_mut() {
                        app.close_window(m.handle);
                    }
                }
                if self.mains.is_empty() {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(m) = self.mains.get_mut(&id) {
                    m.ui.resize(size.width, size.height);
                    #[cfg(target_os = "macos")]
                    {
                        pin_layer_top_left(&m.window);
                        center_traffic_lights(&m.window);
                    }
                }
                // Remember the window size (persisted on quit / dock change), not on every resize event.
                self.settings.window_width = size.width as f32 / scale;
                self.settings.window_height = size.height as f32 / scale;
                self.draw_main(id);
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(m) = self.mains.get_mut(&id) {
                    m.cursor = (position.x, position.y);
                }
                let (lx, ly) = (position.x as f32 / scale, position.y as f32 / scale);
                let dragging = match (self.mains.get(&id), self.main_app.as_ref()) {
                    (Some(m), Some(a)) => m.entity.read(a.app()).dragging(),
                    _ => false,
                };
                if self.with_workspace_view(id, |v, _| v.mouse_move(lx, ly)) == Some(true) {
                    // A live divider drag repaints immediately for smoothness; hover is coalesced.
                    if dragging {
                        self.reset_caret();
                        self.draw_main(id);
                    } else if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
                let resize = self
                    .with_workspace_view(id, |v, _| v.resize_cursor_at(lx, ly))
                    .flatten();
                let over = self
                    .with_workspace_view(id, |v, _| v.hit_at(lx, ly))
                    .flatten()
                    .is_some();
                if let Some(m) = self.mains.get(&id) {
                    m.window.set_cursor(match resize {
                        Some(workspace::ResizeCursor::Horizontal) => CursorIcon::EwResize,
                        Some(workspace::ResizeCursor::Vertical) => CursorIcon::NsResize,
                        None if over => CursorIcon::Pointer,
                        None => CursorIcon::Default,
                    });
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let (lx, ly) = match self.mains.get(&id) {
                    Some(m) => (m.cursor.0 as f32 / scale, m.cursor.1 as f32 / scale),
                    None => return,
                };
                match state {
                    ElementState::Pressed => {
                        self.with_workspace_view(id, |v, _| v.mouse_down(lx, ly));
                        self.reset_caret();
                        self.sync_workspace_effects(id, event_loop);
                    }
                    ElementState::Released => {
                        self.with_workspace_view(id, |v, _| v.mouse_up());
                        self.sync_workspace_effects(id, event_loop);
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                let (lx, ly) = match self.mains.get(&id) {
                    Some(m) => (m.cursor.0 as f32 / scale, m.cursor.1 as f32 / scale),
                    None => return,
                };
                self.reset_caret();
                if self.with_workspace_view(id, |v, _| v.right_click(lx, ly)) == Some(true) {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
            }
            WindowEvent::Focused(true) => {
                self.with_workspace_view(id, |v, _| v.refresh_disk_state());
                self.reset_caret();
                if let Some(m) = self.mains.get_mut(&id) {
                    m.dirty = true;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => (x * 30.0, y * 30.0),
                    winit::event::MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let (lx, ly) = match self.mains.get(&id) {
                    Some(m) => (m.cursor.0 as f32 / scale, m.cursor.1 as f32 / scale),
                    None => return,
                };
                if self.with_workspace_view(id, |v, _| v.scroll(lx, ly, dx, dy)) == Some(true) {
                    if let Some(m) = self.mains.get_mut(&id) {
                        m.dirty = true;
                    }
                }
            }
            WindowEvent::RedrawRequested => self.draw_main(id),
            _ => {}
        }
    }
}

#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    const ICON: &[u8] = include_bytes!("../assets/AppIcon.icns");
    unsafe {
        let data: *mut AnyObject = msg_send![
            class!(NSData),
            dataWithBytes: ICON.as_ptr() as *const std::ffi::c_void,
            length: ICON.len(),
        ];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if image.is_null() {
            eprintln!(
                "[icon] NSImage init from icns FAILED (data_len={})",
                ICON.len()
            );
            return;
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        if std::env::var("POMELO_ICON_LOG").is_ok() {
            let valid: bool = msg_send![image, isValid];
            eprintln!("[icon] set applicationIconImage, image_valid={valid} app={app:p}");
        }
    }
}

#[cfg(target_os = "macos")]
fn pin_layer_top_left(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSString;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    unsafe {
        let view = h.ns_view.as_ptr() as *mut AnyObject;
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() {
            return;
        }
        let gravity = NSString::from_str("topLeft");
        let _: () = msg_send![layer, setContentsGravity: &*gravity];
    }
}

// Center the macOS traffic lights in our taller top bar: resize the titlebar container
// (two levels up) to a fixed height pinned to the window top, then place the buttons at a constant offset within it,
// so they don't drift when the window is resized.
#[cfg(target_os = "macos")]
fn center_traffic_lights(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_app_kit::{NSView, NSWindowButton};
    use objc2_foundation::NSPoint;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    unsafe {
        let view: &NSView = &*(h.ns_view.as_ptr() as *const NSView);
        let Some(ns_window) = view.window() else {
            return;
        };
        // Re-fetch the buttons every time — AppKit can recreate the standard buttons on layout passes.
        let Some(close) = ns_window.standardWindowButton(NSWindowButton::NSWindowCloseButton)
        else {
            return;
        };
        let Some(minimize) =
            ns_window.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton)
        else {
            return;
        };
        let Some(zoom) = ns_window.standardWindowButton(NSWindowButton::NSWindowZoomButton) else {
            return;
        };
        // The titlebar container is TWO levels up (button -> widget container -> titlebar view). Resizing the wrong
        // one (the immediate superview) breaks/hides the buttons.
        let Some(button_container) = close.superview() else {
            return;
        };
        let Some(titlebar) = button_container.superview() else {
            return;
        };

        let catx = AnyClass::get("CATransaction");
        if let Some(catx) = catx {
            let _: () = msg_send![catx, begin];
            let _: () = msg_send![catx, setDisableActions: true];
        }

        let window_h = ns_window.frame().size.height;
        let close_f = close.frame();
        let btn_w = close_f.size.width;
        let btn_h = close_f.size.height;
        let btn_pad = (minimize.frame().origin.x - close_f.origin.x - btn_w).max(0.0);
        let pos_x = 19.0_f64;
        let pos_y = ((workspace::TOP_BAR_H as f64 - btn_h) / 2.0).max(0.0); // vertical padding -> centers in the top bar
        let container_h = btn_h + 2.0 * pos_y;

        // Pin the titlebar container to the top of the window at a fixed height (constant across resizes -> stable).
        let mut tf = titlebar.frame();
        tf.size.height = container_h;
        tf.origin.y = window_h - container_h;
        let _: () = msg_send![&*titlebar, setFrame: tf];

        let min_x = pos_x + btn_w + btn_pad;
        let zoom_x = min_x + btn_w + btn_pad;
        close.setFrameOrigin(NSPoint::new(pos_x, pos_y));
        minimize.setFrameOrigin(NSPoint::new(min_x, pos_y));
        zoom.setFrameOrigin(NSPoint::new(zoom_x, pos_y));
        let _: () = msg_send![&*titlebar, updateTrackingAreas];

        if std::env::var("POMELO_RESIZE_LOG").is_ok() {
            eprintln!(
                "[resize] window_h={window_h:.1} container_h={container_h:.1} titlebar.y={:.1} close=({pos_x:.1},{pos_y:.1}) btn={btn_w:.1}x{btn_h:.1}",
                tf.origin.y
            );
        }

        if let Some(catx) = catx {
            let _: () = msg_send![catx, commit];
        }
    }
}

fn main() -> anyhow::Result<()> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let line = format!("panic at {loc}: {info}\n");
        if let Some(home) = std::env::var_os("HOME") {
            let path = std::path::Path::new(&home).join("Library/Logs/pomelo-panic.log");
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = f.write_all(line.as_bytes());
            }
        }
        eprintln!("{line}");
        previous(info);
    }));

    let event_loop = EventLoop::new()?;
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}
