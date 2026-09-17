//! Demo binary: a cross-platform GPU editor window. Opens a winit window, renders a sample buffer on the GPU via
//! `EditorRenderer`, and edits it from the keyboard. Runs on Linux (Vulkan), macOS (Metal) and Windows (DX12).

use std::sync::Arc;

use pomelo_editor_kit::{EditorBuffer, EditorRenderer};
use winit::application::ApplicationHandler;
use std::time::Instant;

use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const SAMPLE: &str = "// pomelo-editor-kit — GPU text, cross-platform (wgpu + winit + glyphon)\nfn main() {\n    println!(\"Hello from the GPU editor\");\n}\n";

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<EditorRenderer>,
    editor: EditorBuffer,
    text: String,
    ext: String,
    name: String,
    mods: winit::keyboard::ModifiersState,
    cursor_pos: (f64, f64),
    mouse_down: bool,
    last_click: Option<Instant>,
    last_click_pos: (f64, f64),
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let title = if self.ext.is_empty() { "Pomelo Editor (GPU)".to_string() } else { format!("Pomelo Editor — {}", self.text.len()) };
        let attrs = Window::default_attributes().with_title(title);
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        self.editor = EditorBuffer::from_str(&self.text);
        let mut renderer = EditorRenderer::new(window.clone()).expect("renderer");
        renderer.set_language(pomelo_editor_kit::Lang::from_ext(&self.ext));
        renderer.set_tab_title(if self.name.is_empty() { "untitled" } else { &self.name });
        self.renderer = Some(renderer);
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
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, text, .. },
                ..
            } => {
                let shift = self.mods.shift_key();
                let cmd = self.mods.super_key() || self.mods.control_key();
                let alt = self.mods.alt_key();
                let mut edited = false;
                match &logical_key {
                    Key::Named(NamedKey::Backspace) => { self.editor.backspace(); edited = true; }
                    Key::Named(NamedKey::Enter) => { self.editor.insert_char('\n'); edited = true; }
                    Key::Named(NamedKey::Space) if !cmd => { self.editor.insert_char(' '); edited = true; }
                    Key::Named(NamedKey::ArrowLeft) => match (alt, cmd, shift) {
                        (true, _, true) => self.editor.extend_word_left(),
                        (true, _, false) => self.editor.move_word_left(),
                        (_, true, true) => self.editor.extend_home(),
                        (_, true, false) => self.editor.move_home(),
                        (_, _, true) => self.editor.extend_left(),
                        _ => self.editor.move_left(),
                    },
                    Key::Named(NamedKey::ArrowRight) => match (alt, cmd, shift) {
                        (true, _, true) => self.editor.extend_word_right(),
                        (true, _, false) => self.editor.move_word_right(),
                        (_, true, true) => self.editor.extend_end(),
                        (_, true, false) => self.editor.move_end(),
                        (_, _, true) => self.editor.extend_right(),
                        _ => self.editor.move_right(),
                    },
                    Key::Named(NamedKey::ArrowUp) => if shift { self.editor.extend_up() } else { self.editor.move_up() },
                    Key::Named(NamedKey::ArrowDown) => if shift { self.editor.extend_down() } else { self.editor.move_down() },
                    Key::Named(NamedKey::Home) => if shift { self.editor.extend_home() } else { self.editor.move_home() },
                    Key::Named(NamedKey::End) => if shift { self.editor.extend_end() } else { self.editor.move_end() },
                    Key::Named(NamedKey::Escape) => self.editor.collapse_cursors(),
                    Key::Character(c) if cmd => match c.as_str() {
                        "z" if shift => { self.editor.redo(); edited = true; }
                        "z" => { self.editor.undo(); edited = true; }
                        "a" => self.editor.select_all(),
                        "d" => self.editor.select_next(),
                        _ => {}
                    },
                    _ => {
                        if !cmd {
                            if let Some(t) = text {
                                for ch in t.chars() {
                                    if !ch.is_control() {
                                        self.editor.insert_char(ch);
                                    }
                                }
                                edited = true;
                            }
                        }
                    }
                }
                if edited {
                    renderer.mark_text_dirty();
                }
                renderer.follow_cursor(&self.editor);
                window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                if self.mouse_down {
                    let scale = window.scale_factor() as f32;
                    let (l, c) = renderer.point_to_line_col(position.x as f32 / scale, position.y as f32 / scale);
                    let off = self.editor.offset_at(l, c);
                    self.editor.extend_cursor(off);
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let scale = window.scale_factor() as f32;
                let (px, py) = self.cursor_pos;
                let (l, c) = renderer.point_to_line_col(px as f32 / scale, py as f32 / scale);
                let off = self.editor.offset_at(l, c);
                match state {
                    ElementState::Pressed => {
                        self.mouse_down = true;
                        let now = Instant::now();
                        let dbl = self.last_click.is_some_and(|t| now.duration_since(t).as_millis() < 400)
                            && (self.last_click_pos.0 - px).abs() < 4.0
                            && (self.last_click_pos.1 - py).abs() < 4.0;
                        if self.mods.super_key() || self.mods.control_key() {
                            self.editor.add_cursor(off);
                        } else if dbl {
                            self.editor.select_word_at(off);
                            self.last_click = None;
                        } else {
                            self.editor.place_cursor(off);
                            self.last_click = Some(now);
                            self.last_click_pos = (px, py);
                        }
                        renderer.set_caret_on(true);
                        window.request_redraw();
                    }
                    ElementState::Released => self.mouse_down = false,
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 24.0, y * 24.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                renderer.scroll_by(dx, dy, &self.editor);
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if self.mouse_down {
                    let scale = window.scale_factor() as f32;
                    let (lx, ly) = (self.cursor_pos.0 as f32 / scale, self.cursor_pos.1 as f32 / scale);
                    renderer.autoscroll_for_drag(&self.editor, ly);
                    let cy = renderer.clamp_drag_y(ly);
                    let (l, c) = renderer.point_to_line_col(lx, cy);
                    let off = self.editor.offset_at(l, c);
                    self.editor.extend_cursor(off);
                }
                if let Err(e) = renderer.render(&self.editor) {
                    eprintln!("render error: {e}");
                }
                if self.mouse_down {
                    window.request_redraw(); // keep autoscrolling while the drag is held
                }
            }
            _ => {}
        }
    }
}

fn main() -> anyhow::Result<()> {
    // Standalone Rust editor: `pomelo-editor-demo [path]`. With a path, open that file and pick the language from its
    // extension; otherwise show the built-in sample. First step of the Swift->Rust app migration.
    let mut app = App::default();
    if let Some(path) = std::env::args().nth(1) {
        app.text = std::fs::read_to_string(&path).unwrap_or_else(|e| format!("// could not read {path}: {e}\n"));
        let p = std::path::Path::new(&path);
        app.ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
        app.name = p.file_name().and_then(|e| e.to_str()).unwrap_or("untitled").to_string();
    } else {
        app.text = SAMPLE.to_string();
        app.ext = "rs".to_string();
        app.name = "sample.rs".to_string();
    }
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut app)?;
    Ok(())
}
