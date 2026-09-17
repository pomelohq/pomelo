//! Pomelo editor core — a cross-platform, GPU-rendered code editor (rope model + tree-sitter highlighting +
//! wgpu/glyphon rendering). Used directly by the Rust app (no FFI); platform-agnostic (Metal/Vulkan/DX12).

pub mod buffer;
pub mod highlight;
pub mod renderer;
pub mod theme;

pub use buffer::EditorBuffer;
pub use highlight::Lang;
pub use renderer::EditorRenderer;
pub use theme::Theme;
