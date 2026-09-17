//! Demo binary: a cross-platform GPU editor window. Opens a winit window, renders a sample buffer on the GPU via
//! `EditorRenderer`, and edits it from the keyboard. Runs on Linux (Vulkan), macOS (Metal) and Windows (DX12).

use std::sync::Arc;

use pomelo_editor_kit::{EditorBuffer, EditorRenderer};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const SAMPLE: &str = "// pomelo-editor-kit — GPU text, cross-platform (wgpu + winit + glyphon)\nfn main() {\n    println!(\"Hello from the GPU editor\");\n}\n";

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<EditorRenderer>,
    editor: EditorBuffer,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Pomelo Editor (GPU)");
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        self.editor = EditorBuffer::from_str(SAMPLE);
        self.renderer = Some(EditorRenderer::new(window.clone()).expect("renderer"));
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (Some(window), Some(renderer)) = (self.window.as_ref(), self.renderer.as_mut()) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                renderer.resize(size.width, size.height);
                window.request_redraw();
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, text, .. },
                ..
            } => {
                match logical_key {
                    Key::Named(NamedKey::Backspace) => self.editor.backspace(),
                    Key::Named(NamedKey::Enter) => self.editor.insert_char('\n'),
                    Key::Named(NamedKey::Space) => self.editor.insert_char(' '),
                    Key::Named(NamedKey::ArrowLeft) => self.editor.move_left(),
                    Key::Named(NamedKey::ArrowRight) => self.editor.move_right(),
                    Key::Named(NamedKey::ArrowUp) => self.editor.move_up(),
                    Key::Named(NamedKey::ArrowDown) => self.editor.move_down(),
                    _ => {
                        if let Some(t) = text {
                            for ch in t.chars() {
                                if !ch.is_control() {
                                    self.editor.insert_char(ch);
                                }
                            }
                        }
                    }
                }
                renderer.follow_cursor(&self.editor);
                window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 24.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                renderer.scroll_by(dy, &self.editor);
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = renderer.render(&self.editor) {
                    eprintln!("render error: {e}");
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
