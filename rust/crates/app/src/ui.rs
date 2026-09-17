//! Minimal GPU UI layer: fills colored rectangles on a wgpu surface. The layout (top bar, docks, content) is
//! expressed as a list of rects each frame; text/icons come later. Platform-agnostic (Metal/Vulkan/DX12).

use std::sync::Arc;

use anyhow::Result;
use wgpu::{
    CompositeAlphaMode, DeviceDescriptor, Instance, LoadOp, MultisampleState, Operations, PresentMode,
    RenderPassColorAttachment, RenderPassDescriptor, RequestAdapterOptions, StoreOp, SurfaceConfiguration,
    TextureFormat, TextureUsages, TextureViewDescriptor,
};
use winit::window::Window;

/// A colored rectangle in logical pixels (top-left origin).
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [u8; 3],
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub struct UiRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: SurfaceConfiguration,
    scale: f32,
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
    capacity: usize,
}

impl UiRenderer {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let instance = Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .ok_or_else(|| anyhow::anyhow!("no GPU adapter"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor::default(), None))?;

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

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct VOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
                @vertex fn vs(@location(0) p: vec2<f32>, @location(1) c: vec4<f32>) -> VOut {
                    var o: VOut; o.pos = vec4<f32>(p, 0.0, 1.0); o.color = c; return o;
                }
                @fragment fn fs(in: VOut) -> @location(0) vec4<f32> { return in.color; }
                "#
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui"),
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
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let capacity = 256;
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui-verts"),
            size: (capacity * 24) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self { device, queue, surface, config, scale, pipeline, vbuf, capacity })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn size(&self) -> (f32, f32) {
        (self.config.width as f32 / self.scale, self.config.height as f32 / self.scale)
    }

    pub fn render(&mut self, rects: &[Rect]) -> Result<()> {
        let vw = self.config.width as f32;
        let vh = self.config.height as f32;
        let mut verts: Vec<f32> = Vec::with_capacity(rects.len() * 36);
        for r in rects {
            let col = [
                srgb_to_linear(r.color[0] as f64 / 255.0) as f32,
                srgb_to_linear(r.color[1] as f64 / 255.0) as f32,
                srgb_to_linear(r.color[2] as f64 / 255.0) as f32,
                1.0,
            ];
            let ndc = |x: f32, y: f32| ((x * self.scale) / vw * 2.0 - 1.0, 1.0 - (y * self.scale) / vh * 2.0);
            let (l, t) = ndc(r.x, r.y);
            let (rr, b) = ndc(r.x + r.w, r.y + r.h);
            for (px, py) in [(l, t), (rr, t), (l, b), (rr, t), (rr, b), (l, b)] {
                verts.extend_from_slice(&[px, py, col[0], col[1], col[2], col[3]]);
            }
        }
        let quad_count = verts.len() / 6;
        if quad_count > self.capacity {
            self.capacity = quad_count.next_power_of_two();
            self.vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui-verts"),
                size: (self.capacity * 24) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !verts.is_empty() {
            self.queue.write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&verts));
        }

        let frame = self.surface.get_current_texture()?;
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("ui"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Clear(wgpu::Color::BLACK), store: StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if quad_count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, self.vbuf.slice(..));
                pass.draw(0..quad_count as u32, 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}
