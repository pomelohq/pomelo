//! The view layer that ties the reactive core (`reactor::App`/`Entity`/`Context`) to the element tree. A
//! `Render` view is an `Entity<V>` that produces a `Node` each frame; `AnyView` type-erases it so a window can
//! hold a root of any concrete type. Mirrors the framework's `Render`/`AnyView`/`Window` split so features port over:
//! `Window` carries the per-window paint/input state, `Context<V>` (from `reactor`) carries reactivity.
//!
//! This module is platform-free: it lays a view out into a `Frame` (base + overlay layers). The winit/wgpu
//! driver that owns the OS window and pumps this on redraw lives in the binary (and grows into a full
//! `Application` driver next).

use crate::{paint_frame, Frame, Node, Rect};
use reactor::{App, Context, Entity, EntityId};
use std::collections::HashMap;

/// A drawable entity: the reactive equivalent of a screen or panel. `Entity<V: Render>` is a view; mutating it
/// through its `Context` and calling `cx.notify()` marks the owning window dirty so it repaints.
pub trait Render: 'static + Sized {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Node;

    /// Handle a click on an element tagged with `.on_click(id)` during render. The `Application` routes a click
    /// to whichever id sits under the cursor. Default: ignore. (Our id-dispatch mirrors the binary's existing
    /// `handle_*_click`; some frameworks capture closures per element instead.)
    fn on_click(&mut self, _id: u64, _window: &mut Window, _cx: &mut Context<Self>) {}
}

/// A view that composes its own layered `Frame` directly instead of a single element tree. The escape hatch for
/// screens whose layout is built at the `Painted`/overlay level (e.g. settings: a clipped scrolling page under
/// fixed chrome with a popover) - the analogue of the framework's `Canvas` raw-paint element, but for a whole view.
pub trait RawView: 'static + Sized {
    fn render_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Frame;
    fn on_click(&mut self, _id: u64, _window: &mut Window, _cx: &mut Context<Self>) {}
}

/// Per-window paint/input state handed to `Render::render`. Holds the logical viewport the view lays out into,
/// the backing scale, and the cursor. Hit regions produced while painting land in `hits` for the driver to
/// route clicks. Kept intentionally small; it grows as the driver moves off the binary.
#[derive(Default)]
pub struct Window {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub cursor: (f32, f32),
    pub hits: Vec<(Rect, u64)>,
}

impl Window {
    pub fn new(width: f32, height: f32, scale: f32) -> Self {
        Self {
            width,
            height,
            scale,
            cursor: (-1.0, -1.0),
            hits: Vec::new(),
        }
    }

    /// The full logical drawing area (origin at top-left).
    pub fn area(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width, self.height, crate::Rgba::TRANSPARENT)
    }

    /// The innermost hit id under the cursor, if any (regions are pushed outer-first, inner-last).
    pub fn hit_at(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|(_, id)| *id)
    }

    /// The laid-out bounds of the region tagged with click id `id` (last match wins), for tests and for a view
    /// that needs a control's anchor rect.
    pub fn rect_of(&self, id: u64) -> Option<Rect> {
        self.hits
            .iter()
            .rev()
            .find(|(_, hid)| *hid == id)
            .map(|(r, _)| *r)
    }

    /// The center point of the region tagged `id`, convenient for `simulate_click`.
    pub fn center_of(&self, id: u64) -> Option<(f32, f32)> {
        self.rect_of(id).map(|r| (r.x + r.w / 2.0, r.y + r.h / 2.0))
    }
}

/// A type-erased root view: an `Entity<V>` plus closures that produce its `Frame` and route `on_click` against
/// the app. Lets a window hold a root of any concrete view type - `Render` (element tree) or `RawView` (direct
/// frame) - uniformly (the framework's `AnyView`).
type PaintFn = Box<dyn FnMut(&mut Window, &mut App) -> Frame>;
type ClickFn = Box<dyn FnMut(u64, &mut Window, &mut App)>;

pub struct AnyView {
    id: EntityId,
    paint: PaintFn,
    click: ClickFn,
}

impl AnyView {
    pub fn new<V: Render>(entity: &Entity<V>) -> Self {
        let paint_entity = entity.clone();
        let click_entity = entity.clone();
        AnyView {
            id: entity.id(),
            paint: Box::new(move |window, app| {
                let node = paint_entity.update(app, |v, cx| v.render(window, cx));
                paint_frame(&node, window.area())
            }),
            click: Box::new(move |id, window, app| {
                click_entity.update(app, |v, cx| v.on_click(id, window, cx))
            }),
        }
    }

    /// A type-erased root from a `RawView` (composes its own `Frame`).
    pub fn raw<V: RawView>(entity: &Entity<V>) -> Self {
        let paint_entity = entity.clone();
        let click_entity = entity.clone();
        AnyView {
            id: entity.id(),
            paint: Box::new(move |window, app| {
                paint_entity.update(app, |v, cx| v.render_frame(window, cx))
            }),
            click: Box::new(move |id, window, app| {
                click_entity.update(app, |v, cx| v.on_click(id, window, cx))
            }),
        }
    }

    pub fn id(&self) -> EntityId {
        self.id
    }

    /// Produce the view's `Frame` (base + overlay layers) for `window`'s current area. Merged hit regions are
    /// mirrored onto `window.hits` so the driver can route input after the frame.
    pub fn paint(&mut self, window: &mut Window, app: &mut App) -> Frame {
        let frame = (self.paint)(window, app);
        window.hits = frame.hits.clone();
        frame
    }
}

/// A handle to a window owned by the `Application`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WindowHandle(u64);

/// How to open a window. The GPU/OS specifics (traffic lights, transparent titlebar) stay in the platform shell.
pub struct WindowOptions {
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: String::new(),
            width: 0.0,
            height: 0.0,
            scale: 1.0,
        }
    }
}

struct WindowSlot {
    view: AnyView,
    window: Window,
}

/// The reactive window manager (the framework's `App` + window registry). Owns the `reactor::App` and every window's
/// root view + per-window state. This half is platform-free and headless-testable: `draw` lays a window out to
/// a `Frame`, and `simulate_move`/`simulate_click` drive input the same way the winit shell will. The shell
/// (winit + wgpu, plus macOS chrome) attaches a `UiRenderer` per handle and pumps `draw`/`simulate_*`.
#[derive(Default)]
pub struct Application {
    app: App,
    next: u64,
    windows: HashMap<WindowHandle, WindowSlot>,
}

impl Application {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    /// Open a window whose root is a new `Render` entity. Returns the handle and the entity so the caller can
    /// update it later; the window retains a strong reference via its `AnyView`.
    pub fn open_window<V: Render>(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Context<V>) -> V,
    ) -> (WindowHandle, Entity<V>) {
        let entity = self.app.new_entity(build);
        let view = AnyView::new(&entity);
        let handle = WindowHandle(self.next);
        self.next += 1;
        self.windows.insert(
            handle,
            WindowSlot {
                view,
                window: Window::new(options.width, options.height, options.scale),
            },
        );
        (handle, entity)
    }

    /// Open a window whose root is a new `RawView` entity (composes its own `Frame`).
    pub fn open_raw_window<V: RawView>(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Context<V>) -> V,
    ) -> (WindowHandle, Entity<V>) {
        let entity = self.app.new_entity(build);
        let view = AnyView::raw(&entity);
        let handle = WindowHandle(self.next);
        self.next += 1;
        self.windows.insert(
            handle,
            WindowSlot {
                view,
                window: Window::new(options.width, options.height, options.scale),
            },
        );
        (handle, entity)
    }

    pub fn close_window(&mut self, handle: WindowHandle) {
        self.windows.remove(&handle);
    }

    pub fn handles(&self) -> Vec<WindowHandle> {
        self.windows.keys().copied().collect()
    }

    pub fn window(&self, handle: WindowHandle) -> Option<&Window> {
        self.windows.get(&handle).map(|s| &s.window)
    }

    /// Resize a window's logical viewport (the shell calls this on a resize event).
    pub fn resize(&mut self, handle: WindowHandle, width: f32, height: f32, scale: f32) {
        if let Some(slot) = self.windows.get_mut(&handle) {
            slot.window.width = width;
            slot.window.height = height;
            slot.window.scale = scale;
        }
    }

    /// Lay a window's view out into a `Frame` (base + overlays). The shell feeds this into `render_frame`.
    pub fn draw(&mut self, handle: WindowHandle) -> Option<Frame> {
        let Application { app, windows, .. } = self;
        let slot = windows.get_mut(&handle)?;
        Some(slot.view.paint(&mut slot.window, app))
    }

    /// Update the cursor for a window (drives hover); returns the hit id now under it, if any.
    pub fn simulate_move(&mut self, handle: WindowHandle, x: f32, y: f32) -> Option<u64> {
        let slot = self.windows.get_mut(&handle)?;
        slot.window.cursor = (x, y);
        slot.window.hit_at(x, y)
    }

    /// Route a click at `(x, y)` to the hit id under it, calling the view's `on_click`. Returns the id hit, if
    /// any. Requires a prior `draw` so the window's hit regions are current.
    pub fn simulate_click(&mut self, handle: WindowHandle, x: f32, y: f32) -> Option<u64> {
        let Application { app, windows, .. } = self;
        let slot = windows.get_mut(&handle)?;
        slot.window.cursor = (x, y);
        let id = slot.window.hit_at(x, y)?;
        (slot.view.click)(id, &mut slot.window, app);
        Some(id)
    }

    /// Whether any entity notified since the last check; the shell redraws its windows when true.
    pub fn take_dirty(&mut self) -> bool {
        self.app.take_dirty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{div, label};

    struct Screen {
        count: i32,
    }

    impl Render for Screen {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Node {
            div()
                .col()
                .child(label(format!("count: {}", self.count)))
                .into()
        }
    }

    fn base_text(f: &Frame) -> String {
        f.base.texts.iter().map(|t| t.text.clone()).collect()
    }

    #[test]
    fn view_repaints_after_notify() {
        let mut app = App::new();
        let screen = app.new_entity(|_| Screen { count: 0 });
        let mut view = AnyView::new(&screen);
        let mut window = Window::new(400.0, 300.0, 2.0);

        let first = view.paint(&mut window, &mut app);
        assert_eq!(base_text(&first), "count: 0");

        screen.update(&mut app, |s, cx| {
            s.count = 7;
            cx.notify();
        });
        assert!(app.take_dirty());

        let second = view.paint(&mut window, &mut app);
        assert_eq!(base_text(&second), "count: 7");
    }

    struct Popover;

    impl Render for Popover {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Node {
            div()
                .child(label("body"))
                .child(
                    crate::anchored()
                        .position(50.0, 40.0)
                        .size(120.0, 80.0)
                        .priority(1)
                        .clip()
                        .child(div().on_click(9).child(label("panel"))),
                )
                .into()
        }
    }

    #[test]
    fn anchored_child_becomes_its_own_layer() {
        let mut app = App::new();
        let view_entity = app.new_entity(|_| Popover);
        let mut view = AnyView::new(&view_entity);
        let mut window = Window::new(400.0, 300.0, 2.0);

        let frame = view.paint(&mut window, &mut app);
        assert_eq!(base_text(&frame), "body");
        assert_eq!(frame.overlays.len(), 1);
        let overlay = &frame.overlays[0];
        let clip = overlay.clip.expect("clipped overlay");
        assert_eq!((clip.x, clip.y, clip.w, clip.h), (50.0, 40.0, 120.0, 80.0));
        assert_eq!(
            overlay
                .painted
                .texts
                .iter()
                .map(|t| t.text.clone())
                .collect::<String>(),
            "panel"
        );
        // The panel's click region lands inside the clip, so it survives into the merged hit list.
        assert_eq!(window.hit_at(60.0, 50.0), Some(9));
    }

    struct Button {
        clicks: i32,
    }

    const BTN: u64 = 42;

    impl Render for Button {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Node {
            div()
                .debug("root")
                .child(
                    div()
                        .w_px(100.0)
                        .h_px(30.0)
                        .debug("button")
                        .on_click(BTN)
                        .child(label(format!("clicks: {}", self.clicks))),
                )
                .into()
        }

        fn on_click(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
            if id == BTN {
                self.clicks += 1;
                cx.notify();
            }
        }
    }

    impl Render for () {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Node {
            div().into()
        }
    }

    #[test]
    fn application_routes_click_to_view() {
        let mut app = Application::new();
        let (h, _btn) = app.open_window(
            WindowOptions {
                width: 200.0,
                height: 100.0,
                scale: 2.0,
                ..Default::default()
            },
            |cx| {
                cx.notify();
                Button { clicks: 0 }
            },
        );
        assert!(app.take_dirty());

        let frame = app.draw(h).expect("frame");
        let btn = frame.debug_bound("button").expect("button bounds");
        assert_eq!((btn.w, btn.h), (100.0, 30.0));

        let hit = app.simulate_click(h, btn.x + 5.0, btn.y + 5.0);
        assert_eq!(hit, Some(BTN));
        assert!(app.take_dirty(), "on_click called cx.notify");

        let frame = app.draw(h).expect("frame");
        let text: String = frame.base.texts.iter().map(|t| t.text.clone()).collect();
        assert_eq!(text, "clicks: 1");
    }

    #[test]
    fn clip_rect_scrolls_content_under_a_fixed_band() {
        // Content taller than the visible band: place it at content_top - scroll, clip to the visible band. A
        // row scrolled above the band is clipped out of the hit list; a row inside stays hittable.
        let scroll = 40.0;
        let (content_top, band_h) = (20.0, 60.0);
        let node: Node = crate::div()
            .child(
                crate::anchored()
                    .position(0.0, content_top - scroll)
                    .size(100.0, 200.0)
                    .clip_rect(0.0, content_top, 100.0, band_h)
                    .priority(1)
                    .child(
                        crate::div()
                            .col()
                            .child(crate::div().h_px(30.0).on_click(1)) // y in [ -20, 10 ): mostly above band
                            .child(crate::div().h_px(30.0).on_click(2)), // y in [ 10, 40 ): inside band
                    ),
            )
            .into();
        let frame = paint_frame(
            &node,
            Rect::new(0.0, 0.0, 200.0, 200.0, crate::Rgba::TRANSPARENT),
        );
        assert_eq!(frame.overlays.len(), 1);
        let clip = frame.overlays[0].clip.expect("clip");
        assert_eq!((clip.y, clip.h), (content_top, band_h));
        // Row 2 sits inside the band and remains hittable; row 1 is scrolled fully above it and is clipped out.
        assert!(frame.hits.iter().any(|(_, id)| *id == 2));
        assert!(!frame.hits.iter().any(|(_, id)| *id == 1));
    }

    struct RawScreen {
        label: String,
    }

    impl RawView for RawScreen {
        fn render_frame(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> Frame {
            // Compose a frame directly: a base label plus one clipped overlay, the way settings does.
            let node: Node = div().child(label(self.label.clone())).into();
            let mut frame = paint_frame(&node, window.area());
            frame.overlays.push(crate::Overlay {
                painted: crate::Painted::default(),
                clip: Some(Rect::new(0.0, 0.0, 10.0, 10.0, crate::Rgba::TRANSPARENT)),
            });
            frame
        }
    }

    #[test]
    fn raw_view_frame_flows_through_application() {
        let mut app = Application::new();
        let (h, _e) = app.open_raw_window(
            WindowOptions {
                width: 100.0,
                height: 50.0,
                scale: 1.0,
                ..Default::default()
            },
            |_| RawScreen {
                label: "raw".into(),
            },
        );
        let frame = app.draw(h).expect("frame");
        assert_eq!(
            frame
                .base
                .texts
                .iter()
                .map(|t| t.text.clone())
                .collect::<String>(),
            "raw"
        );
        assert_eq!(frame.overlays.len(), 1);
    }

    #[test]
    fn windows_are_independent() {
        let mut app = Application::new();
        let (a, _a) = app.open_window(WindowOptions::default(), |_| Button { clicks: 0 });
        let (b, _b) = app.open_window(WindowOptions::default(), |_| ());
        assert_ne!(a, b);
        assert_eq!(app.handles().len(), 2);
        app.close_window(b);
        assert_eq!(app.handles(), vec![a]);
    }
}
