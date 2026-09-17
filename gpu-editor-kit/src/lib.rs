//! pomelo-editor-kit: a cross-platform, GPU-rendered code editor kit.
//!
//! Why this exists: the app's editor is currently AppKit `NSTextView` (macOS only) hosted in SwiftUI, whose per-frame
//! Auto Layout measurement stalls when many editor panes resize at once. Zed avoids this with a GPU text renderer
//! (gpui + wgpu). This kit is the same idea, portable to Linux/macOS/Windows:
//!
//!   - `wgpu`   -> one GPU API over Metal (macOS), Vulkan (Linux), DX12 (Windows).
//!   - `winit`  -> one windowing/input API across the three platforms.
//!   - `glyphon`-> cosmic-text shaping + a GPU glyph atlas; text is composited on the GPU, not laid out by the OS.
//!
//! The kit is a plain library (`EditorBuffer` model + `EditorRenderer`) plus a `winit` demo binary. Embedding it into
//! the existing Swift app is a later step (render into a shared surface / FFI); the first goal is a self-contained
//! GPU editor that runs on all three platforms.

pub mod buffer;
pub mod renderer;

pub use buffer::{Cursor, EditorBuffer};
pub use renderer::EditorRenderer;
