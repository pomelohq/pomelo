# pomelo-editor-kit

A cross-platform, GPU-rendered code editor kit (Linux / macOS / Windows) in Rust.

## Why

The app's editor is AppKit `NSTextView` hosted in SwiftUI (macOS only). Profiling showed the stall when many editor
panes resize at once is SwiftUI measuring each hosted editor's fitting size through Auto Layout
(`systemLayoutSizeFittingSize`) every frame. That is a ceiling of hosting OS text views in SwiftUI, and it is why Zed
(GPU text via `gpui` + `wgpu`) stays at 120fps.

This kit takes the same approach and makes it portable:

- **wgpu** — one GPU API mapped to Metal (macOS), Vulkan (Linux), DX12 (Windows).
- **winit** — one windowing / input API across the three platforms.
- **glyphon** — `cosmic-text` shaping + a GPU glyph atlas; text is composited on the GPU, not laid out by the OS.

## Layout

- `src/buffer.rs` — `EditorBuffer`: lines + cursor, editing ops. Platform-independent, no GPU. (Rope comes later.)
- `src/renderer.rs` — `EditorRenderer`: owns the wgpu surface/device + the glyphon atlas; composites the buffer.
- `src/main.rs` — `pomelo-editor-demo`: a winit window that renders + edits a sample buffer on the GPU.

## Run

```
cargo run --bin pomelo-editor-demo
```

## Roadmap

1. Rope buffer + multi-cursor + selection.
2. Syntax highlighting (tree-sitter) coloring the glyph runs.
3. Gutter, scrolling, minimap.
4. Embed into the Swift app (shared surface / FFI) to replace the NSTextView editor pane; the Swift side keeps tabs /
   panes / docks and hands each editor pane to this kit.
5. Windows/Linux app shells once embedding is proven.
