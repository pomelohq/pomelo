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
    dragging_divider: bool,
}

impl App {
    fn scale(&self) -> f32 {
        self.window.as_ref().map(|w| w.scale_factor() as f32).unwrap_or(2.0)
    }
    fn cursor_logical(&self) -> (f32, f32) {
        let s = self.scale();
        (self.cursor.0 as f32 / s, self.cursor.1 as f32 / s)
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
        center_traffic_lights(&window);
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (Some(window), Some(ui)) = (self.window.as_ref(), self.ui.as_mut()) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                ui.resize(size.width, size.height);
                #[cfg(target_os = "macos")]
                center_traffic_lights(window);
                window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.dragging_divider {
                    let x = position.x as f32 / window.scale_factor() as f32;
                    self.layout.set_divider(x);
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let (lx, ly) = self.cursor_logical();
                match state {
                    ElementState::Pressed => {
                        if self.layout.hit_toggle(lx, ly) {
                            self.layout.toggle();
                            window.request_redraw();
                        } else if self.layout.on_divider(lx, ly) {
                            self.dragging_divider = true;
                        }
                    }
                    ElementState::Released => self.dragging_divider = false,
                }
            }
            WindowEvent::RedrawRequested => {
                let (w, h) = ui.size();
                let rects = self.layout.rects(w, h);
                if let Err(e) = ui.render(&rects) {
                    eprintln!("ui render error: {e}");
                }
            }
            _ => {}
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
