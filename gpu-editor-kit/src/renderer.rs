//! GPU renderer: composites the editor buffer's text with `glyphon` on top of `wgpu`. Owns the surface, device and
//! the glyph atlas. Platform-agnostic — wgpu picks Metal/Vulkan/DX12 under the hood.

use std::sync::Arc;

use anyhow::Result;
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea, TextAtlas,
    TextBounds, TextRenderer, Viewport,
};
use wgpu::{
    CompositeAlphaMode, DeviceDescriptor, Instance, LoadOp, MultisampleState, Operations, PresentMode,
    RenderPassColorAttachment, RenderPassDescriptor, RequestAdapterOptions, StoreOp, SurfaceConfiguration,
    TextureFormat, TextureUsages, TextureViewDescriptor,
};
use winit::window::Window;

use crate::EditorBuffer;

const GUTTER_WIDTH: f32 = 52.0;
// Height of the tab bar; the editor content starts below it (like a Zed pane), so a scrolled top line clips under the
// tab bar instead of against the window chrome.
const CONTENT_TOP: f32 = 36.0;

pub struct EditorRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: SurfaceConfiguration,

    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    text_renderer: TextRenderer,
    buffer: Buffer,
    gutter: Buffer,
    tab_title: Buffer,
    title: String,
    title_dirty: bool,
    scale: f32,
    line_height: f32,
    char_width: f32,
    scroll_y: f32,
    scroll_x: f32,
    caret_on: bool,
    text_dirty: bool,
    quad_pipeline: wgpu::RenderPipeline,
    quad_vertices: wgpu::Buffer,
    quad_capacity: usize,
    highlighter: crate::highlight::Highlighter,
    language: crate::highlight::Lang,
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

// sRGB u8 color -> linear f32 quad color. The surface is sRGB, so quad fragment output must be linear to display the
// intended sRGB color (same reason the clear color is linearized).
fn lin(rgb: [u8; 3], a: f32) -> [f32; 4] {
    [
        srgb_to_linear(rgb[0] as f64 / 255.0) as f32,
        srgb_to_linear(rgb[1] as f64 / 255.0) as f32,
        srgb_to_linear(rgb[2] as f64 / 255.0) as f32,
        a,
    ]
}

impl EditorRenderer {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let instance = Instance::default();
        let surface = instance.create_surface(window.clone())?;
        Self::from_surface(instance, surface, size.width, size.height, scale)
    }

    /// Build a renderer that draws into an existing macOS `CAMetalLayer` (for embedding in an AppKit/SwiftUI host).
    ///
    /// # Safety
    /// `layer` must be a valid `CAMetalLayer` pointer that outlives the returned renderer.
    pub unsafe fn from_metal_layer(layer: *mut std::ffi::c_void, width: u32, height: u32, scale: f32) -> Result<Self> {
        let instance = Instance::default();
        let surface = instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))?;
        Self::from_surface(instance, surface, width, height, scale)
    }

    fn from_surface(
        instance: Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<Self> {
        let size = winit::dpi::PhysicalSize::new(width.max(1), height.max(1));
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .ok_or_else(|| anyhow::anyhow!("no GPU adapter"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default(), None))?;

        let format = TextureFormat::Bgra8UnormSrgb;
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: PresentMode::Fifo,
            alpha_mode: CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer = TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);

        let font_size = 13.0;
        let line_height = font_size * 1.4;
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(font_size, line_height));
        buffer.set_size(
            &mut font_system,
            Some(size.width as f32 / scale),
            Some(size.height as f32 / scale),
        );
        let mut gutter = Buffer::new(&mut font_system, Metrics::new(font_size, line_height));
        gutter.set_size(&mut font_system, Some(GUTTER_WIDTH), Some(size.height as f32 / scale));
        let mut tab_title = Buffer::new(&mut font_system, Metrics::new(13.0, CONTENT_TOP));
        tab_title.set_size(&mut font_system, Some(size.width as f32 / scale), Some(CONTENT_TOP));

        let mut probe = Buffer::new(&mut font_system, Metrics::new(font_size, line_height));
        probe.set_text(&mut font_system, "M", Attrs::new().family(Family::Monospace), Shaping::Advanced);
        probe.shape_until_scroll(&mut font_system, false);
        let char_width = probe
            .layout_runs()
            .next()
            .and_then(|r| r.glyphs.first().map(|g| g.w))
            .unwrap_or(font_size * 0.6);

        let quad_capacity = 256;
        let (quad_pipeline, quad_vertices) = Self::build_quads(&device, format, quad_capacity);

        Ok(Self {
            device,
            queue,
            surface,
            config,
            font_system,
            swash_cache,
            atlas,
            viewport,
            text_renderer,
            buffer,
            gutter,
            tab_title,
            title: String::new(),
            title_dirty: true,
            scale,
            line_height,
            char_width,
            scroll_y: 0.0,
            scroll_x: 0.0,
            caret_on: true,
            text_dirty: true,
            quad_pipeline,
            quad_vertices,
            quad_capacity,
            highlighter: crate::highlight::Highlighter::default(),
            language: crate::highlight::Lang::PlainText,
        })
    }

    pub fn set_caret_on(&mut self, on: bool) {
        self.caret_on = on;
    }

    pub fn set_language(&mut self, lang: crate::highlight::Lang) {
        if self.language != lang {
            self.language = lang;
            self.text_dirty = true;
        }
    }

    pub fn mark_text_dirty(&mut self) {
        self.text_dirty = true;
    }

    pub fn set_tab_title(&mut self, title: &str) {
        if self.title != title {
            self.title = title.to_string();
            self.title_dirty = true;
        }
    }

    /// Map a point in logical view coords (top-left origin) to a (line, col) in the text.
    pub fn point_to_line_col(&self, px: f32, py: f32) -> (usize, usize) {
        let y = (py - CONTENT_TOP + self.scroll_y).max(0.0);
        let line = (y / self.line_height).floor() as usize;
        let x = (px - GUTTER_WIDTH + self.scroll_x).max(0.0);
        let col = (x / self.char_width).round() as usize;
        (line, col)
    }

    fn build_quads(
        device: &wgpu::Device,
        format: TextureFormat,
        capacity: usize,
    ) -> (wgpu::RenderPipeline, wgpu::Buffer) {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct VOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
                @vertex fn vs(@location(0) p: vec2<f32>, @location(1) c: vec4<f32>) -> VOut {
                    var o: VOut;
                    o.pos = vec4<f32>(p, 0.0, 1.0);
                    o.color = c;
                    return o;
                }
                @fragment fn fs(in: VOut) -> @location(0) vec4<f32> {
                    return in.color;
                }
                "#
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs",
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 8, shader_location: 1 },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad-verts"),
            size: (capacity * 24) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        (pipeline, vertices)
    }

    // A screen rect (logical px) -> 6 vertices (2 triangles) in NDC, each carrying `color`.
    fn push_rect(&self, out: &mut Vec<f32>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let vw = self.config.width as f32;
        let vh = self.config.height as f32;
        let ndc = |x: f32, y: f32| ((x * self.scale) / vw * 2.0 - 1.0, 1.0 - (y * self.scale) / vh * 2.0);
        let (l, t) = ndc(x, y);
        let (r, b) = ndc(x + w, y + h);
        let mut v = |px: f32, py: f32| {
            out.extend_from_slice(&[px, py, color[0], color[1], color[2], color[3]]);
        };
        v(l, t); v(r, t); v(l, b);
        v(r, t); v(r, b); v(l, b);
    }

    // Clip a content rect to the editor area (below the tab bar, right of the gutter) so selection/caret never paints
    // over the tab strip or the gutter when scrolled.
    fn push_content_rect(&self, out: &mut Vec<f32>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        let top = y.max(CONTENT_TOP);
        let bottom = y + h;
        let left = x.max(GUTTER_WIDTH);
        let right = x + w;
        if bottom > top && right > left {
            self.push_rect(out, left, top, right - left, bottom - top, color);
        }
    }

    fn build_quad_vertices(&self, editor: &EditorBuffer) -> Vec<f32> {
        let mut out = Vec::new();
        let view_w = self.config.width as f32 / self.scale;
        // Tab bar (Zed One Dark): background #2f343e, 1px bottom border #464b57.
        self.push_rect(&mut out, 0.0, 0.0, view_w, CONTENT_TOP, lin([47, 52, 62], 1.0));
        self.push_rect(&mut out, 0.0, CONTENT_TOP - 1.0, view_w, 1.0, lin([70, 75, 87], 1.0));

        let sel = lin([116, 173, 232], 0.24); // player selection #74ade8 @ 0x3d
        if let Some((s, e)) = editor.selection() {
            let (sl, sc) = editor.line_col_of(s);
            let (el, ec) = editor.line_col_of(e);
            for line in sl..=el {
                let c0 = if line == sl { sc } else { 0 };
                let end = if line == el { ec } else { editor.line_len(line) + 1 };
                let x = GUTTER_WIDTH + c0 as f32 * self.char_width - self.scroll_x;
                let w = (end.saturating_sub(c0)) as f32 * self.char_width;
                let y = CONTENT_TOP + line as f32 * self.line_height - self.scroll_y;
                self.push_content_rect(&mut out, x, y, w.max(1.0), self.line_height, sel);
            }
        }
        if self.caret_on {
            let (line, col) = editor.line_col();
            let x = GUTTER_WIDTH + col as f32 * self.char_width - self.scroll_x;
            let y = CONTENT_TOP + line as f32 * self.line_height - self.scroll_y;
            self.push_content_rect(&mut out, x, y, 2.0, self.line_height, lin([116, 173, 232], 1.0));
        }
        out
    }

    fn viewport_height(&self) -> f32 {
        self.config.height as f32 / self.scale
    }

    fn max_scroll(&self, editor: &EditorBuffer) -> f32 {
        let content = editor.rope.len_lines().max(1) as f32 * self.line_height;
        (content + CONTENT_TOP - self.viewport_height()).max(0.0)
    }

    pub fn scroll_by(&mut self, delta_x: f32, delta_y: f32, editor: &EditorBuffer) {
        self.scroll_y = (self.scroll_y - delta_y).clamp(0.0, self.max_scroll(editor));
        // Upper bound clamped in render() where the widest visible line width is known.
        self.scroll_x = (self.scroll_x - delta_x).max(0.0);
    }

    /// Keep the cursor's line inside the viewport after a nav/edit.
    pub fn follow_cursor(&mut self, editor: &EditorBuffer) {
        let (line, _) = editor.line_col();
        let top = line as f32 * self.line_height;
        let bottom = top + self.line_height;
        let view = self.viewport_height() - CONTENT_TOP;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + view {
            self.scroll_y = bottom - view;
        }
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll(editor));
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.text_dirty = true; // reshape re-sizes the text buffers from the new config
    }

    pub fn render(&mut self, editor: &EditorBuffer) -> Result<()> {
        // Reshape text only when it changed — not every frame — so scroll/caret redraws stay cheap on large files.
        if self.text_dirty {
            // Width = None so lines never soft-wrap (Zed scrolls horizontally instead); a wrapped code line would add
            // visual rows that the 1-number-per-line gutter can't match, misaligning line numbers. Height = viewport +
            // overscan so cosmic-text shapes only the visible window (cull) plus a little past the bottom edge.
            let view_h = self.config.height as f32 / self.scale + 3.0 * self.line_height;
            self.buffer.set_size(&mut self.font_system, None, Some(view_h));
            self.gutter.set_size(&mut self.font_system, Some(GUTTER_WIDTH), Some(view_h));

            let text = editor.text();
            let spans = self.highlighter.highlight(&text, self.language);
            let rich: Vec<(&str, Attrs)> = spans
                .iter()
                .map(|s| (s.text.as_str(), Attrs::new().family(Family::Monospace).color(s.color)))
                .collect();
            self.buffer.set_rich_text(
                &mut self.font_system,
                rich,
                Attrs::new().family(Family::Monospace),
                Shaping::Advanced,
            );

            let line_count = editor.rope.len_lines().max(1);
            let numbers: String = (1..=line_count).map(|n| format!("{n}\n")).collect();
            self.gutter.set_text(
                &mut self.font_system,
                &numbers,
                Attrs::new().family(Family::Monospace).color(Color::rgb(78, 90, 95)),
                Shaping::Advanced,
            );
            self.text_dirty = false;
        }
        if self.title_dirty {
            self.tab_title.set_size(&mut self.font_system, Some(self.config.width as f32 / self.scale), Some(CONTENT_TOP));
            self.tab_title.set_text(
                &mut self.font_system,
                &self.title,
                Attrs::new().family(Family::SansSerif).color(Color::rgb(220, 224, 229)),
                Shaping::Advanced,
            );
            self.tab_title.shape_until_scroll(&mut self.font_system, false);
            self.title_dirty = false;
        }

        // Scroll by whole lines through cosmic-text (so it shapes/culls the visible window) and do the sub-line
        // remainder via the text areas' top offset. cosmic-text's layout iterator drops a line once its baseline
        // rises above 0, so a fractional `Scroll::vertical` would cull the top line ~80% of the way up; keeping
        // vertical=0 and shifting `top` avoids that. caret/selection y still use scroll_y directly (equivalent).
        let top_line = (self.scroll_y / self.line_height).floor().max(0.0);
        let scroll_frac = self.scroll_y - top_line * self.line_height;
        let scroll = glyphon::cosmic_text::Scroll { line: top_line as usize, vertical: 0.0, horizontal: 0.0 };
        self.buffer.set_scroll(scroll);
        self.gutter.set_scroll(scroll);
        self.buffer.shape_until_scroll(&mut self.font_system, false);
        self.gutter.shape_until_scroll(&mut self.font_system, false);

        // Clamp horizontal scroll to the widest currently-visible line (Zed-style horizontal scroll, no wrap).
        let widest = self.buffer.layout_runs().map(|r| r.line_w).fold(0.0_f32, f32::max);
        let code_view_w = (self.config.width as f32 / self.scale - GUTTER_WIDTH).max(1.0);
        let max_x = (widest - code_view_w).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, max_x);

        self.viewport.update(
            &self.queue,
            Resolution { width: self.config.width, height: self.config.height },
        );

        // Clip content to below the top padding so a partially-scrolled top line ends at a clean edge instead of
        // bleeding to y=0 (into the title bar / tab strip).
        let clip_top = (CONTENT_TOP * self.scale) as i32;
        let bounds = TextBounds {
            left: 0,
            top: clip_top,
            right: self.config.width as i32,
            bottom: self.config.height as i32,
        };
        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            [
                TextArea {
                    buffer: &self.gutter,
                    left: 8.0 * self.scale,
                    top: (CONTENT_TOP - scroll_frac) * self.scale,
                    scale: self.scale,
                    bounds,
                    default_color: Color::rgb(78, 90, 95),
                    custom_glyphs: &[],
                },
                TextArea {
                    buffer: &self.buffer,
                    left: (GUTTER_WIDTH - self.scroll_x) * self.scale,
                    top: (CONTENT_TOP - scroll_frac) * self.scale,
                    scale: self.scale,
                    // Clip the code to the right of the gutter so horizontally-scrolled text doesn't underlap it.
                    bounds: TextBounds { left: (GUTTER_WIDTH * self.scale) as i32, top: clip_top, right: self.config.width as i32, bottom: self.config.height as i32 },
                    default_color: Color::rgb(172, 178, 190),
                    custom_glyphs: &[],
                },
                TextArea {
                    buffer: &self.tab_title,
                    left: 12.0 * self.scale,
                    top: 0.0,
                    scale: self.scale,
                    bounds: TextBounds { left: 0, top: 0, right: self.config.width as i32, bottom: clip_top },
                    default_color: Color::rgb(220, 224, 229),
                    custom_glyphs: &[],
                },
            ],
            &mut self.swash_cache,
        )?;

        let verts = self.build_quad_vertices(editor);
        let quad_count = verts.len() / 6;
        if quad_count > self.quad_capacity {
            self.quad_capacity = quad_count.next_power_of_two();
            self.quad_vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad-verts"),
                size: (self.quad_capacity * 24) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !verts.is_empty() {
            self.queue.write_buffer(&self.quad_vertices, 0, bytemuck::cast_slice(&verts));
        }

        let frame = self.surface.get_current_texture()?;
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("editor"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        // Zed One Dark editor.background #282c33. The surface is sRGB; wgpu gamma-encodes the clear,
                        // so pass a linearized color.
                        load: LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear(40.0 / 255.0),
                            g: srgb_to_linear(44.0 / 255.0),
                            b: srgb_to_linear(51.0 / 255.0),
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if quad_count > 0 {
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_vertex_buffer(0, self.quad_vertices.slice(..));
                pass.draw(0..quad_count as u32, 0..1);
            }
            self.text_renderer.render(&self.atlas, &self.viewport, &mut pass)?;
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.atlas.trim();
        Ok(())
    }
}
