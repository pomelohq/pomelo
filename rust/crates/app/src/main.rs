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
        let attrs = Window::default_attributes().with_title("Pomelo");
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        self.ui = Some(UiRenderer::new(window.clone()).expect("ui"));
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

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}
