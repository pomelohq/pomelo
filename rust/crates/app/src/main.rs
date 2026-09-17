//! Pomelo app shell (cross-platform, winit + wgpu). This stage builds the LAYOUT: a top bar, a resizable left dock
//! (the workspace-list panel — our addition over Zed) that collapses to an icon rail instead of vanishing, and the
//! content area (colored placeholders for now). Panels/tabs, file tree, terminal and the ported core come next.

mod layout;
mod ui;

use std::sync::Arc;

use layout::Layout;
use ui::UiRenderer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    ui: Option<UiRenderer>,
    layout: Layout,
    cursor: (f64, f64),
    dragging_left: bool,
    dragging_right: bool,
}

impl App {
    fn scale(&self) -> f32 {
        self.window.as_ref().map(|w| w.scale_factor() as f32).unwrap_or(2.0)
    }
    fn cursor_logical(&self) -> (f32, f32) {
        let s = self.scale();
        (self.cursor.0 as f32 / s, self.cursor.1 as f32 / s)
    }
    fn draw(&mut self) {
        if let Some(ui) = self.ui.as_mut() {
            let (w, h) = ui.size();
            let (rects, texts) = self.layout.build(w, h);
            if let Err(e) = ui.render(&rects, &texts) {
                eprintln!("ui render error: {e}");
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes().with_title("Pomelo");
        // Zed-style: manage the title bar ourselves — content fills under a transparent title bar; the traffic lights
        // stay (top-left) and we draw our own top bar behind/around them. No separate native title strip.
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true)
                .with_title_hidden(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        self.ui = Some(UiRenderer::new(window.clone()).expect("ui"));
        #[cfg(target_os = "macos")]
        {
            configure_surface_layer(&window);
            center_traffic_lights(&window);
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.window.is_none() || self.ui.is_none() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.ui.as_mut().unwrap().resize(size.width, size.height);
                #[cfg(target_os = "macos")]
                center_traffic_lights(self.window.as_ref().unwrap());
                // Draw synchronously so the content matches the new size immediately (avoids the compositor
                // stretching the previous frame during live resize, which reads as jitter).
                self.draw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                let s = self.scale();
                let (lx, _ly) = (position.x as f32 / s, position.y as f32 / s);
                if self.dragging_left {
                    self.layout.set_left_divider(lx);
                    self.draw();
                } else if self.dragging_right {
                    let w = self.ui.as_ref().unwrap().size().0;
                    self.layout.set_right_divider(lx, w);
                    self.draw();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let (lx, ly) = self.cursor_logical();
                let w = self.ui.as_ref().unwrap().size().0;
                match state {
                    ElementState::Pressed => {
                        if self.layout.hit_left_toggle(lx, ly) {
                            self.layout.left.collapsed = !self.layout.left.collapsed;
                            self.draw();
                        } else if self.layout.hit_right_toggle(lx, ly, w) {
                            self.layout.right.collapsed = !self.layout.right.collapsed;
                            self.draw();
                        } else if self.layout.on_left_divider(lx, ly) {
                            self.dragging_left = true;
                        } else if self.layout.on_right_divider(lx, ly, w) {
                            self.dragging_right = true;
                        }
                    }
                    ElementState::Released => {
                        self.dragging_left = false;
                        self.dragging_right = false;
                    }
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }
}

// Present the metal drawable inside the layer's transaction, so during a live resize the drawable and the layer
// bounds change atomically instead of the old frame being stretched (which reads as blur/ghosting).
#[cfg(target_os = "macos")]
fn configure_surface_layer(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    unsafe {
        let view = h.ns_view.as_ptr() as *mut AnyObject;
        let layer: *mut AnyObject = msg_send![view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setPresentsWithTransaction: true];
        }
    }
}

// Vertically center the macOS traffic lights inside our taller top bar (they default to a standard ~28px title bar,
// which sits too high). Re-applied on resize because AppKit re-lays them out. Zed does the same.
#[cfg(target_os = "macos")]
fn center_traffic_lights(window: &Window) {
    use objc2_app_kit::{NSView, NSWindowButton};
    use objc2_foundation::NSPoint;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    unsafe {
        let view: &NSView = &*(h.ns_view.as_ptr() as *const NSView);
        let Some(ns_window) = view.window() else { return };
        let btn_h = 14.0_f64;
        let start_x = 19.0_f64;
        let spacing = 20.0_f64;
        for (i, kind) in [
            NSWindowButton::NSWindowCloseButton,
            NSWindowButton::NSWindowMiniaturizeButton,
            NSWindowButton::NSWindowZoomButton,
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(btn) = ns_window.standardWindowButton(kind) {
                if let Some(sv) = btn.superview() {
                    let sv_h = sv.frame().size.height;
                    // Frame origin is bottom-left; place the button so its center is at TOP_BAR_H/2 from the top.
                    let y = sv_h - (layout::TOP_BAR_H as f64 / 2.0) - (btn_h / 2.0);
                    btn.setFrameOrigin(NSPoint::new(start_x + i as f64 * spacing, y));
                }
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}
