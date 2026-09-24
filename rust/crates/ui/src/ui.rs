//! Minimal GPU UI layer: fills colored rectangles on a wgpu surface. The layout (top bar, docks, content) is
//! expressed as a list of rects each frame; text/icons come later. Platform-agnostic (Metal/Vulkan/DX12).

mod app;
mod components;
mod element;
mod theme;
pub use app::*;
pub use components::*;
pub use element::*;
pub use theme::*;

// Re-export the reactive core so view crates depend on `ui` alone (the framework exposes these from one crate).
pub use reactor::{App, Context, Entity, Global, Subscription, WeakEntity};

use std::sync::Arc;

use anyhow::Result;
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};
use wgpu::util::DeviceExt;
use wgpu::{
    CompositeAlphaMode, DeviceDescriptor, Instance, LoadOp, MultisampleState, Operations,
    PresentMode, RenderPassColorAttachment, RenderPassDescriptor, RequestAdapterOptions, StoreOp,
    SurfaceConfiguration, TextureFormat, TextureUsages, TextureViewDescriptor,
};
use winit::window::Window as WinitWindow;

/// A colored rectangle in logical pixels (top-left origin). `radius` rounds the corners (0 = sharp);
/// `border` draws a `border`-px-wide ring in `border_color` inside the edge (0 = no border).
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: Rgba,
    pub radius: f32,
    pub border: f32,
    pub border_color: Rgba,
}

impl Rect {
    /// A wavy line `thickness` thick along `w`, as under diagnostics; drawn in a box three times as tall.
    pub fn wavy_underline(x: f32, y: f32, w: f32, thickness: f32, color: Rgba) -> Self {
        Rect {
            x,
            y,
            w,
            h: thickness * 3.0,
            color,
            // A negative radius tells the shader to draw the wave instead of a box.
            radius: -thickness,
            border: 0.0,
            border_color: Rgba::TRANSPARENT,
        }
    }

    pub fn new(x: f32, y: f32, w: f32, h: f32, color: Rgba) -> Self {
        Self {
            x,
            y,
            w,
            h,
            color,
            radius: 0.0,
            border: 0.0,
            border_color: Rgba::TRANSPARENT,
        }
    }
}

/// A filled triangle in logical pixels (for small icons like the disclosure/dropdown chevron).
#[derive(Clone, Copy)]
pub struct Tri {
    pub p: [[f32; 2]; 3],
    pub color: Rgba,
}

/// A run of text in logical pixels (top-left origin). `mono` renders it in the mono UI font (`.PomeloMono`)
/// instead of the active family — used for section headers.
#[derive(Clone)]
pub struct Text {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: Rgba,
    pub text: String,
    pub mono: bool,
    /// OpenType weight (400 = regular, 500 = medium, 700 = bold). Most UI text is regular, matching the
    /// reference; only emphasized runs bump this.
    pub weight: u16,
    /// Wrap width in logical (design) px; `0` = single line. Used for wrapping setting descriptions.
    pub wrap: f32,
}

/// A scissor rectangle `(x, y, w, h)` in physical pixels; `None` means draw unclipped.
pub type Clip = Option<(f32, f32, f32, f32)>;

/// One back-to-front compositing layer for `render_frame`: its rects, triangles, text runs, icons, and clip.
pub type Layer<'a> = (&'a [Rect], &'a [Tri], &'a [Text], &'a [IconQuad], Clip);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum IconTexKey {
    Mono(IconKind, u32),
    Material(MaterialIcon, u32),
    Image(u64),
}

type ImageData = (u32, u32, Vec<u8>);

static IMAGES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u64, ImageData>>> =
    std::sync::OnceLock::new();

fn images() -> &'static std::sync::Mutex<std::collections::HashMap<u64, ImageData>> {
    IMAGES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub fn set_image(id: u64, width: u32, height: u32, rgba: Vec<u8>) {
    if let Ok(mut m) = images().lock() {
        m.insert(id, (width, height, rgba));
    }
}

pub fn has_image(id: u64) -> bool {
    images()
        .lock()
        .map(|m| m.contains_key(&id))
        .unwrap_or(false)
}

pub fn selection_path(
    rows: &[(f32, f32)],
    top_y: f32,
    line_h: f32,
    radius: f32,
    color: Rgba,
) -> Vec<Tri> {
    if rows.is_empty() {
        return Vec::new();
    }
    if rows.len() >= 2 && rows[0].0 > rows[1].1 {
        let mut out = selection_band(&rows[0..1], top_y, line_h, radius, color);
        out.extend(selection_band(
            &rows[1..],
            top_y + line_h,
            line_h,
            radius,
            color,
        ));
        return out;
    }
    selection_band(rows, top_y, line_h, radius, color)
}

fn selection_band(
    rows: &[(f32, f32)],
    top_y: f32,
    line_h: f32,
    radius: f32,
    color: Rgba,
) -> Vec<Tri> {
    let n = rows.len();
    if n == 0 {
        return Vec::new();
    }
    let y = |i: usize| top_y + i as f32 * line_h;
    let mut ring: Vec<(f32, f32)> = Vec::with_capacity(n * 4);
    ring.push((rows[0].0, y(0)));
    ring.push((rows[0].1, y(0)));
    for i in 0..n {
        ring.push((rows[i].1, y(i + 1)));
        if i + 1 < n {
            ring.push((rows[i + 1].1, y(i + 1)));
        }
    }
    ring.push((rows[n - 1].0, y(n)));
    for i in (0..n).rev() {
        ring.push((rows[i].0, y(i)));
        if i > 0 {
            ring.push((rows[i - 1].0, y(i)));
        }
    }
    dedup_ring(&mut ring);
    let rounded = round_ring(&ring, radius);
    earclip(&rounded, color)
}

fn dedup_ring(ring: &mut Vec<(f32, f32)>) {
    ring.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3);
    while ring.len() >= 2 {
        let (f, l) = (ring[0], ring[ring.len() - 1]);
        if (f.0 - l.0).abs() < 1e-3 && (f.1 - l.1).abs() < 1e-3 {
            ring.pop();
        } else {
            break;
        }
    }
}

fn round_ring(ring: &[(f32, f32)], radius: f32) -> Vec<(f32, f32)> {
    let n = ring.len();
    if n < 3 || radius <= 0.0 {
        return ring.to_vec();
    }
    const SEG: usize = 3;
    let mut out = Vec::with_capacity(n * (SEG + 1));
    for i in 0..n {
        let p = ring[(i + n - 1) % n];
        let v = ring[i];
        let nx = ring[(i + 1) % n];
        let to_p = (p.0 - v.0, p.1 - v.1);
        let to_n = (nx.0 - v.0, nx.1 - v.1);
        let len_p = (to_p.0 * to_p.0 + to_p.1 * to_p.1).sqrt();
        let len_n = (to_n.0 * to_n.0 + to_n.1 * to_n.1).sqrt();
        if len_p < 1e-3 || len_n < 1e-3 {
            out.push(v);
            continue;
        }
        let rp = radius.min(len_p * 0.5);
        let rn = radius.min(len_n * 0.5);
        let a = (v.0 + to_p.0 / len_p * rp, v.1 + to_p.1 / len_p * rp);
        let b = (v.0 + to_n.0 / len_n * rn, v.1 + to_n.1 / len_n * rn);
        out.push(a);
        for s in 1..SEG {
            let t = s as f32 / SEG as f32;
            let mt = 1.0 - t;
            out.push((
                mt * mt * a.0 + 2.0 * mt * t * v.0 + t * t * b.0,
                mt * mt * a.1 + 2.0 * mt * t * v.1 + t * t * b.1,
            ));
        }
        out.push(b);
    }
    out
}

fn signed_area(poly: &[(f32, f32)]) -> f32 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        a += p.0 * q.1 - q.0 * p.1;
    }
    a * 0.5
}

fn earclip(poly: &[(f32, f32)], color: Rgba) -> Vec<Tri> {
    if poly.len() < 3 {
        return Vec::new();
    }
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    if signed_area(poly) < 0.0 {
        idx.reverse();
    }
    let cross = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    };
    let in_tri = |p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        let d1 = cross(a, b, p);
        let d2 = cross(b, c, p);
        let d3 = cross(c, a, p);
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(neg && pos)
    };
    let mut tris = Vec::new();
    let mut guard = 0;
    while idx.len() >= 3 && guard < 20_000 {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let i0 = idx[(i + m - 1) % m];
            let i1 = idx[i];
            let i2 = idx[(i + 1) % m];
            let (a, b, c) = (poly[i0], poly[i1], poly[i2]);
            if cross(a, b, c) <= 0.0 {
                continue; // reflex/collinear -> not an ear
            }
            let mut contains = false;
            for &j in &idx {
                if j == i0 || j == i1 || j == i2 {
                    continue;
                }
                if in_tri(poly[j], a, b, c) {
                    contains = true;
                    break;
                }
            }
            if contains {
                continue;
            }
            tris.push(Tri {
                p: [[a.0, a.1], [b.0, b.1], [c.0, c.1]],
                color,
            });
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    tris
}

type IconDraw = (IconTexKey, u32, u32);

fn ui_config(width: u32, height: u32) -> SurfaceConfiguration {
    SurfaceConfiguration {
        usage: TextureUsages::RENDER_ATTACHMENT,
        format: TextureFormat::Bgra8UnormSrgb,
        width: width.max(1),
        height: height.max(1),
        present_mode: PresentMode::Fifo,
        alpha_mode: CompositeAlphaMode::Auto,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    }
}

fn glyph_color(c: Rgba) -> Color {
    let [r, g, b, a] = c.to_u8();
    Color::rgba(r, g, b, a)
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
    // Windowed rendering targets a surface; headless (for UI snapshot tests) targets an offscreen texture.
    surface: Option<wgpu::Surface<'static>>,
    offscreen: Option<wgpu::Texture>,
    config: SurfaceConfiguration,
    scale: f32,
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
    capacity: usize,
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    // One text renderer per composited layer (base, overlay, top, ...); grown on demand.
    text_renderers: Vec<TextRenderer>,
    shaped_cache: std::collections::HashMap<u64, Buffer>,
    ui_font: Option<String>,     // active family used for rendering
    sans_family: Option<String>, // resolves ".PomeloSans"
    mono_family: Option<String>, // resolves ".PomeloMono"
    // Icons: real SVGs rasterized to an alpha mask (cached per size) and drawn tinted.
    icon_pipeline: wgpu::RenderPipeline,
    icon_bgl: wgpu::BindGroupLayout,
    icon_sampler: wgpu::Sampler,
    icon_cache: std::collections::HashMap<IconTexKey, wgpu::BindGroup>,
    msaa: wgpu::TextureView,
}

const SAMPLES: u32 = 4;

fn make_msaa(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: TextureFormat,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("ui-msaa"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&TextureViewDescriptor::default())
}

// Bundle our UI font (IBM Plex, OFL) so labels render crisp and identical on every machine, instead of a
// system fallback whose optical size is wrong at small sizes. We register all static weight faces under the
// same family so `Attrs::weight` picks the matching face (cosmic-text 0.12 does not instantiate a variable
// font's `wght` axis, so a single variable file would be stuck at Regular). Returns the registered family name.
/// The corner radius in physical px, or the negated wave thickness for a wavy underline.
fn shader_radius(r: &Rect, scale: f32, half_w: f32, half_h: f32) -> f32 {
    if r.radius < 0.0 {
        r.radius * scale
    } else {
        (r.radius * scale).min(half_w).min(half_h)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

static MODIFIERS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_modifiers(modifiers: Modifiers) {
    let bits = u8::from(modifiers.cmd)
        | u8::from(modifiers.shift) << 1
        | u8::from(modifiers.alt) << 2
        | u8::from(modifiers.ctrl) << 3;
    MODIFIERS.store(bits, std::sync::atomic::Ordering::Relaxed);
}

pub fn modifiers() -> Modifiers {
    let bits = MODIFIERS.load(std::sync::atomic::Ordering::Relaxed);
    Modifiers {
        cmd: bits & 1 != 0,
        shift: bits & 2 != 0,
        alt: bits & 4 != 0,
        ctrl: bits & 8 != 0,
    }
}

static WAKER: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// Install how background work asks the event loop for a frame.
pub fn set_waker(waker: impl Fn() + Send + Sync + 'static) {
    if WAKER.set(Box::new(waker)).is_err() {
        eprintln!("ui: waker already installed");
    }
}

/// Ask for a frame from any thread, e.g. when a background result arrived.
pub fn wake() {
    if let Some(waker) = WAKER.get() {
        waker();
    }
}

fn load_font(font_system: &mut FontSystem, faces: &[&'static [u8]]) -> Option<String> {
    use glyphon::fontdb::Source;
    let mut first: Option<String> = None;
    for bytes in faces {
        let ids = font_system
            .db_mut()
            .load_font_source(Source::Binary(Arc::new(*bytes)));
        if first.is_none() {
            if let Some(id) = ids.first() {
                first = font_system
                    .db()
                    .face(*id)
                    .and_then(|f| f.families.first().map(|(n, _)| n.clone()));
            }
        }
    }
    first
}

/// Bundle our UI fonts (IBM Plex Sans + Mono, OFL) and return their real family names. We expose them under
/// the aliases ".PomeloSans" / ".PomeloMono". All static weights
/// (Thin 100 .. Bold 700) are registered so the Font Weight control spans the family's real range.
fn load_ui_fonts(font_system: &mut FontSystem) -> (Option<String>, Option<String>) {
    let sans = load_font(
        font_system,
        &[
            include_bytes!("../assets/IBMPlexSans-Thin.ttf"),
            include_bytes!("../assets/IBMPlexSans-ExtraLight.ttf"),
            include_bytes!("../assets/IBMPlexSans-Light.ttf"),
            include_bytes!("../assets/IBMPlexSans-Regular.ttf"),
            include_bytes!("../assets/IBMPlexSans-Medium.ttf"),
            include_bytes!("../assets/IBMPlexSans-SemiBold.ttf"),
            include_bytes!("../assets/IBMPlexSans-Bold.ttf"),
        ],
    );
    let mono = load_font(
        font_system,
        &[
            include_bytes!("../assets/Lilex-Regular.ttf"),
            include_bytes!("../assets/Lilex-Bold.ttf"),
        ],
    );
    theme::set_bundled_fonts(sans.clone(), mono.clone());
    (sans, mono)
}

struct Measurer {
    font_system: FontSystem,
    sans: Option<String>,
    mono: Option<String>,
    // Shaping a buffer per label per frame is expensive and dominates layout while scrolling; cache widths
    // keyed by (text, quarter-px size, mono) so repeated labels across frames are free.
    cache: std::collections::HashMap<(String, u32, bool), f32>,
    // Wrapped (width, height) keyed additionally by wrap width (half-px buckets).
    wrap_cache: std::collections::HashMap<(String, u32, bool, u32), (f32, f32)>,
    glyph_cache: std::collections::HashMap<(String, u32, bool), GlyphOffsets>,
}

/// Each glyph's starting byte index into the shaped text with its x offset, plus the total advance.
pub type GlyphOffsets = (std::rc::Rc<[(usize, f32)]>, f32);

thread_local! {
    // A standalone font context (our bundled fonts only) used to measure text widths during element layout,
    // so containers size to the actual shaped glyphs instead of a crude average-char estimate.
    static MEASURER: std::cell::RefCell<Option<Measurer>> = const { std::cell::RefCell::new(None) };
}

/// Width in logical px of `text` shaped at `size` in the sans (or mono) UI font. Used by the element tree's
/// intrinsic sizing so a label's box matches its real glyph extent.
pub fn measure_text_width(text: &str, size: f32, mono: bool, weight: u16) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let size = size * theme::ui_text_scale();
    MEASURER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let m = slot.get_or_insert_with(|| {
            // System fonts too (not just bundled Plex): the user can pick any installed family as the UI font,
            // and measurement must shape in that same family or laid-out boxes won't match the rendered glyphs.
            let mut font_system = FontSystem::new();
            let (sans, mono) = load_ui_fonts(&mut font_system);
            Measurer {
                font_system,
                sans,
                mono,
                cache: std::collections::HashMap::new(),
                wrap_cache: std::collections::HashMap::new(),
                glyph_cache: std::collections::HashMap::new(),
            }
        });
        let family_name = if mono {
            m.mono.clone()
        } else {
            theme::active_ui_font().or_else(|| m.sans.clone())
        };
        let weight = theme::snap_weight(family_name.as_deref(), weight);
        let key = (
            format!(
                "{}\u{0}{}\u{0}{}",
                family_name.as_deref().unwrap_or(""),
                weight,
                text
            ),
            (size * 4.0).round() as u32,
            mono,
        );
        if let Some(w) = m.cache.get(&key) {
            return *w;
        }
        let family = match &family_name {
            Some(name) => Family::Name(name),
            None => Family::SansSerif,
        };
        let mut buffer = Buffer::new(&mut m.font_system, Metrics::new(size, size * 1.3));
        buffer.set_size(&mut m.font_system, None, None);
        buffer.set_text(
            &mut m.font_system,
            text,
            Attrs::new().family(family).weight(Weight(weight)),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut m.font_system, false);
        let width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0_f32, f32::max);
        m.cache.insert(key, width);
        width
    })
}

/// How far the mono (editor) font reaches below its baseline at `size`, in logical px.
pub fn mono_descent(size: f32) -> f32 {
    let size = size * theme::ui_text_scale();
    MEASURER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let m = slot.get_or_insert_with(|| {
            let mut font_system = FontSystem::new();
            let (sans, mono) = load_ui_fonts(&mut font_system);
            Measurer {
                font_system,
                sans,
                mono,
                cache: std::collections::HashMap::new(),
                wrap_cache: std::collections::HashMap::new(),
                glyph_cache: std::collections::HashMap::new(),
            }
        });
        let Some(name) = m.mono.clone() else {
            return 0.0;
        };
        let id = m.font_system.db().query(&glyphon::fontdb::Query {
            families: &[glyphon::fontdb::Family::Name(&name)],
            ..Default::default()
        });
        let Some(font) = id.and_then(|id| m.font_system.get_font(id)) else {
            return 0.0;
        };
        let metrics = font.as_swash().metrics(&[]);
        if metrics.units_per_em == 0 {
            return 0.0;
        }
        metrics.descent / metrics.units_per_em as f32 * size
    })
}

/// Glyph offsets of `text` shaped exactly like a single-line label (same font, size, weight, scale as
/// `measure_text_width`): each glyph's starting byte index with its x, and the total advance. A ligature is one
/// glyph, so indices inside it have no entry of their own.
pub fn measure_glyphs(text: &str, size: f32, mono: bool, weight: u16) -> GlyphOffsets {
    if text.is_empty() {
        return (std::rc::Rc::from(Vec::new()), 0.0);
    }
    let size = size * theme::ui_text_scale();
    MEASURER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let m = slot.get_or_insert_with(|| {
            let mut font_system = FontSystem::new();
            let (sans, mono) = load_ui_fonts(&mut font_system);
            Measurer {
                font_system,
                sans,
                mono,
                cache: std::collections::HashMap::new(),
                wrap_cache: std::collections::HashMap::new(),
                glyph_cache: std::collections::HashMap::new(),
            }
        });
        let family_name = if mono {
            m.mono.clone()
        } else {
            theme::active_ui_font().or_else(|| m.sans.clone())
        };
        let weight = theme::snap_weight(family_name.as_deref(), weight);
        let key = (
            format!(
                "{}\u{0}{}\u{0}{}",
                family_name.as_deref().unwrap_or(""),
                weight,
                text
            ),
            (size * 4.0).round() as u32,
            mono,
        );
        if let Some(hit) = m.glyph_cache.get(&key) {
            return hit.clone();
        }
        let family = match &family_name {
            Some(name) => Family::Name(name),
            None => Family::SansSerif,
        };
        let mut buffer = Buffer::new(&mut m.font_system, Metrics::new(size, size * 1.3));
        buffer.set_size(&mut m.font_system, None, None);
        buffer.set_text(
            &mut m.font_system,
            text,
            Attrs::new().family(family).weight(Weight(weight)),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut m.font_system, false);
        let mut glyphs = Vec::new();
        let mut width = 0.0_f32;
        for run in buffer.layout_runs() {
            glyphs.extend(run.glyphs.iter().map(|g| (g.start, g.x)));
            width = width.max(run.line_w);
        }
        glyphs.sort_by_key(|(start, _)| *start);
        let offsets: GlyphOffsets = (std::rc::Rc::from(glyphs), width);
        m.glyph_cache.insert(key, offsets.clone());
        offsets
    })
}

/// Shaped size of `text` wrapped to `max_w` logical (design) px at `size`, as (widest line, total height).
/// Used to lay out multi-line setting descriptions.
pub fn measure_wrapped(text: &str, size: f32, mono: bool, weight: u16, max_w: f32) -> (f32, f32) {
    let scale = theme::ui_text_scale();
    let size = size * scale;
    let max_w = max_w * scale;
    if text.is_empty() || max_w <= 0.0 {
        return (0.0, size * 1.4);
    }
    MEASURER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let m = slot.get_or_insert_with(|| {
            // System fonts too (not just bundled Plex): the user can pick any installed family as the UI font,
            // and measurement must shape in that same family or laid-out boxes won't match the rendered glyphs.
            let mut font_system = FontSystem::new();
            let (sans, mono) = load_ui_fonts(&mut font_system);
            Measurer {
                font_system,
                sans,
                mono,
                cache: std::collections::HashMap::new(),
                wrap_cache: std::collections::HashMap::new(),
                glyph_cache: std::collections::HashMap::new(),
            }
        });
        let family_name = if mono {
            m.mono.clone()
        } else {
            theme::active_ui_font().or_else(|| m.sans.clone())
        };
        let weight = theme::snap_weight(family_name.as_deref(), weight);
        let key = (
            format!(
                "{}\u{0}{}\u{0}{}",
                family_name.as_deref().unwrap_or(""),
                weight,
                text
            ),
            (size * 4.0).round() as u32,
            mono,
            (max_w * 2.0).round() as u32,
        );
        if let Some(v) = m.wrap_cache.get(&key) {
            return *v;
        }
        let family = match &family_name {
            Some(name) => Family::Name(name),
            None => Family::SansSerif,
        };
        let mut buffer = Buffer::new(&mut m.font_system, Metrics::new(size, size * 1.3));
        buffer.set_size(&mut m.font_system, Some(max_w), None);
        buffer.set_text(
            &mut m.font_system,
            text,
            Attrs::new().family(family).weight(Weight(weight)),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut m.font_system, false);
        let mut lines = 0usize;
        let mut w = 0.0_f32;
        for run in buffer.layout_runs() {
            lines += 1;
            w = w.max(run.line_w);
        }
        let out = (w, lines.max(1) as f32 * size * 1.5);
        m.wrap_cache.insert(key, out);
        out
    })
}

/// The SVG source for each icon (Lucide, ISC/MIT — see `assets/icons/LICENSES`), bundled into the binary.
fn icon_svg(kind: IconKind) -> &'static [u8] {
    macro_rules! svg {
        ($f:literal) => {
            include_bytes!(concat!("../assets/icons/", $f))
        };
    }
    match kind {
        IconKind::ChevronRight => svg!("chevron_right.svg"),
        IconKind::ChevronDown => svg!("chevron_down.svg"),
        IconKind::ChevronUpDown => svg!("chevron_up_down.svg"),
        IconKind::Search => svg!("search.svg"),
        IconKind::Close => svg!("close.svg"),
        IconKind::Check => svg!("check.svg"),
        IconKind::Warning => svg!("warning.svg"),
        IconKind::XCircle => svg!("x_circle.svg"),
        IconKind::Plus => svg!("plus.svg"),
        IconKind::Folder => svg!("folder.svg"),
        IconKind::FolderOpen => svg!("folder_open.svg"),
        IconKind::File => svg!("file.svg"),
        IconKind::Monitor => svg!("monitor.svg"),
        IconKind::ArrowUpRight => svg!("arrow_up_right.svg"),
        IconKind::ArrowLeft => svg!("arrow_left.svg"),
        IconKind::ArrowRight => svg!("arrow_right.svg"),
        IconKind::Window => svg!("window.svg"),
        IconKind::Grid => svg!("grid.svg"),
        IconKind::Branch => svg!("branch.svg"),
        IconKind::Cylinder => svg!("cylinder.svg"),
        IconKind::Table => svg!("table.svg"),
        IconKind::Eye => svg!("eye.svg"),
        IconKind::Key => svg!("key.svg"),
        IconKind::Dash => svg!("dash.svg"),
        IconKind::ChevronUp => svg!("chevron_up.svg"),
        IconKind::ExpandUp => svg!("expand_up.svg"),
        IconKind::PullRequest => svg!("pull_request.svg"),
        IconKind::Merged => svg!("merged.svg"),
        IconKind::Clock => svg!("clock.svg"),
        IconKind::Filter => svg!("filter.svg"),
        IconKind::Copy => svg!("copy.svg"),
        IconKind::Trash => svg!("trash.svg"),
        IconKind::Ticket => svg!("ticket.svg"),
        IconKind::Terminal => svg!("terminal.svg"),
        IconKind::Diamond => svg!("diamond.svg"),
        IconKind::Sparkle => svg!("sparkle.svg"),
        IconKind::Sidebar => svg!("sidebar.svg"),
        IconKind::PanelRight => svg!("panel_right.svg"),
        IconKind::PanelBottom => svg!("panel_bottom.svg"),
        IconKind::Maximize => svg!("maximize.svg"),
        IconKind::Minimize => svg!("minimize.svg"),
        IconKind::Pin => svg!("pin.svg"),
        IconKind::Server => svg!("server.svg"),
        IconKind::Undo => svg!("undo.svg"),
        IconKind::ChevronLeft => svg!("chevron_left.svg"),
        IconKind::CaseSensitive => svg!("case_sensitive.svg"),
        IconKind::WholeWord => svg!("whole_word.svg"),
        IconKind::Regex => svg!("regex.svg"),
        IconKind::Replace => svg!("replace.svg"),
        IconKind::ReplaceNext => svg!("replace_next.svg"),
        IconKind::ReplaceAll => svg!("replace_all.svg"),
        IconKind::SelectAll => svg!("select_all.svg"),
        IconKind::Quote => svg!("quote.svg"),
        IconKind::Command => svg!("command.svg"),
        IconKind::Shift => svg!("shift.svg"),
        IconKind::Option => svg!("option.svg"),
        IconKind::Control => svg!("control.svg"),
        IconKind::ArrowUp => svg!("arrow_up.svg"),
        IconKind::ArrowDown => svg!("arrow_down.svg"),
        IconKind::Return => svg!("return.svg"),
        IconKind::Backspace => svg!("backspace.svg"),
        IconKind::Tab => svg!("tab.svg"),
        IconKind::KeyArrowLeft => svg!("key_arrow_left.svg"),
        IconKind::KeyArrowRight => svg!("key_arrow_right.svg"),
        IconKind::DiffUnified => svg!("diff_unified.svg"),
        IconKind::DiffSplit => svg!("diff_split.svg"),
        IconKind::FileGit => svg!("file_git.svg"),
        IconKind::Play => svg!("play.svg"),
        IconKind::Stop => svg!("stop.svg"),
        IconKind::RotateCw => svg!("rotate_cw.svg"),
        IconKind::SquarePlus => svg!("square_plus.svg"),
        IconKind::SquareDot => svg!("square_dot.svg"),
        IconKind::SquareMinus => svg!("square_minus.svg"),
    }
}

/// Rasterize an icon's SVG to a `px`-by-`px` alpha coverage mask (one byte per pixel). Returns all-zero on any
/// parse/render failure so a bad asset degrades to a blank icon rather than crashing.
fn rasterize_icon(kind: IconKind, px: u32) -> Vec<u8> {
    let blank = || vec![0u8; (px * px) as usize];
    let opt = resvg::usvg::Options::default();
    let Ok(tree) = resvg::usvg::Tree::from_data(icon_svg(kind), &opt) else {
        return blank();
    };
    let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(px, px) else {
        return blank();
    };
    let size = tree.size();
    let scale = (px as f32 / size.width()).min(px as f32 / size.height());
    // Center the (possibly non-square) drawing in the square pixmap.
    let tx = (px as f32 - size.width() * scale) / 2.0;
    let ty = (px as f32 - size.height() * scale) / 2.0;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // tiny-skia stores premultiplied RGBA; the alpha channel is the coverage we tint later.
    pixmap.data().chunks_exact(4).map(|p| p[3]).collect()
}

fn material_svg(m: MaterialIcon) -> &'static [u8] {
    macro_rules! svg {
        ($f:literal) => {
            include_bytes!(concat!("../assets/icons/material/", $f))
        };
    }
    match m {
        MaterialIcon::Rust => svg!("rust.svg"),
        MaterialIcon::Go => svg!("go.svg"),
        MaterialIcon::TypeScript => svg!("typescript.svg"),
        MaterialIcon::React => svg!("react.svg"),
        MaterialIcon::JavaScript => svg!("javascript.svg"),
        MaterialIcon::Json => svg!("json.svg"),
        MaterialIcon::Markdown => svg!("markdown.svg"),
        MaterialIcon::Toml => svg!("toml.svg"),
        MaterialIcon::Yaml => svg!("yaml.svg"),
        MaterialIcon::Html => svg!("html.svg"),
        MaterialIcon::Css => svg!("css.svg"),
        MaterialIcon::Sass => svg!("sass.svg"),
        MaterialIcon::Python => svg!("python.svg"),
        MaterialIcon::Lock => svg!("lock.svg"),
        MaterialIcon::Console => svg!("console.svg"),
        MaterialIcon::Document => svg!("document.svg"),
        MaterialIcon::Image => svg!("image.svg"),
        MaterialIcon::Git => svg!("git.svg"),
        MaterialIcon::NodeJs => svg!("nodejs.svg"),
    }
}

fn rasterize_material(m: MaterialIcon, px: u32) -> Vec<u8> {
    let blank = || vec![0u8; (px * px * 4) as usize];
    let opt = resvg::usvg::Options::default();
    let Ok(tree) = resvg::usvg::Tree::from_data(material_svg(m), &opt) else {
        return blank();
    };
    let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(px, px) else {
        return blank();
    };
    let size = tree.size();
    let scale = (px as f32 / size.width()).min(px as f32 / size.height());
    let tx = (px as f32 - size.width() * scale) / 2.0;
    let ty = (px as f32 - size.height() * scale) / 2.0;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for p in pixmap.data().chunks_exact(4) {
        let a = p[3];
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let un = |c: u8| ((c as u32 * 255 + (a as u32) / 2) / a as u32).min(255) as u8;
            out.extend_from_slice(&[un(p[0]), un(p[1]), un(p[2]), a]);
        }
    }
    out
}

impl UiRenderer {
    pub fn new(window: Arc<WinitWindow>) -> Result<Self> {
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let instance = Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .ok_or_else(|| anyhow::anyhow!("no GPU adapter"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default(), None))?;

        let config = ui_config(size.width, size.height);
        surface.configure(&device, &config);
        Self::from_parts(device, queue, config, scale, Some(surface), None)
    }

    /// Headless renderer that draws to an offscreen texture (for the UI snapshot tool). Sizes are physical.
    pub fn new_headless(width: u32, height: u32, scale: f32) -> Result<Self> {
        let instance = Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default()))
                .ok_or_else(|| anyhow::anyhow!("no GPU adapter"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default(), None))?;
        let config = ui_config(width, height);
        let offscreen = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui-offscreen"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self::from_parts(device, queue, config, scale, None, Some(offscreen))
    }

    fn from_parts(
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: SurfaceConfiguration,
        scale: f32,
        surface: Option<wgpu::Surface<'static>>,
        offscreen: Option<wgpu::Texture>,
    ) -> Result<Self> {
        let format = config.format;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct VOut {
                    @builtin(position) pos: vec4<f32>,
                    @location(0) color: vec4<f32>,
                    @location(1) local: vec2<f32>,
                    @location(2) hsize: vec2<f32>,
                    @location(3) radius: f32,
                    @location(4) bcolor: vec4<f32>,
                    @location(5) bwidth: f32,
                };
                @vertex fn vs(
                    @location(0) p: vec2<f32>,
                    @location(1) c: vec4<f32>,
                    @location(2) local: vec2<f32>,
                    @location(3) hsize: vec2<f32>,
                    @location(4) radius: f32,
                    @location(5) bcolor: vec4<f32>,
                    @location(6) bwidth: f32,
                ) -> VOut {
                    var o: VOut;
                    o.pos = vec4<f32>(p, 0.0, 1.0);
                    o.color = c; o.local = local; o.hsize = hsize; o.radius = radius;
                    o.bcolor = bcolor; o.bwidth = bwidth;
                    return o;
                }
                fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
                    let q = abs(p) - b + vec2<f32>(r, r);
                    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - r;
                }
                @fragment fn fs(in: VOut) -> @location(0) vec4<f32> {
                    if (in.radius < 0.0) {
                        // Wavy underline: distance to a sine through the box's middle, two waves per
                        // thickness-scaled height, peaking at 0.8 of the thickness.
                        let thickness = -in.radius;
                        let height = in.hsize.y * 2.0;
                        let st = vec2<f32>((in.local.x + in.hsize.x) / height, (in.local.y + in.hsize.y) / height - 0.5);
                        let frequency = 3.14159265 * 2.0 * thickness / height;
                        let amplitude = thickness * 0.8 / height;
                        let sine = sin(st.x * frequency) * amplitude;
                        let slope = cos(st.x * frequency) * amplitude * frequency;
                        let distance = (st.y - sine) / sqrt(1.0 + slope * slope) * height;
                        let half = thickness * 0.5;
                        let alpha = clamp(0.5 - max(-(distance + half), distance - half), 0.0, 1.0);
                        return vec4<f32>(in.color.rgb, in.color.a * alpha);
                    }
                    let d = sd_round_box(in.local, in.hsize, in.radius);
                    let aa = fwidth(d);
                    let cov = 1.0 - smoothstep(0.0, aa, d);
                    // Border ring: pixels within `bwidth` of the edge use the border color. The inner AA
                    // straddles the -bwidth boundary (not spilling fully inward), so a 1px border reads as a
                    // crisp hairline instead of a ~2px fuzzy band.
                    let edge = select(0.0, smoothstep(-in.bwidth - aa * 0.5, -in.bwidth + aa * 0.5, d), in.bwidth > 0.0);
                    let rgb = mix(in.color.rgb, in.bcolor.rgb, edge);
                    // Alpha must follow the border in the ring, else a transparent fill (ghost buttons) hides
                    // the border entirely.
                    let a = mix(in.color.a, in.bcolor.a, edge);
                    return vec4<f32>(rgb, a * cov);
                }
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
                    array_stride: 64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 32,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 40,
                            shader_location: 4,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 44,
                            shader_location: 5,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 60,
                            shader_location: 6,
                        },
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
            multisample: MultisampleState {
                count: SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });
        let capacity = 256;
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui-verts"),
            size: (capacity * 64) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut font_system = FontSystem::new();
        let (sans_family, mono_family) = load_ui_fonts(&mut font_system);
        let ui_font = sans_family.clone();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderers = (0..4)
            .map(|_| {
                TextRenderer::new(
                    &mut atlas,
                    &device,
                    MultisampleState {
                        count: SAMPLES,
                        mask: !0,
                        alpha_to_coverage_enabled: false,
                    },
                    None,
                )
            })
            .collect();

        // Icon pipeline: a textured quad sampling an alpha mask (R8), tinted by the per-vertex color.
        let icon_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui-icon"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct VOut {
                    @builtin(position) pos: vec4<f32>,
                    @location(0) uv: vec2<f32>,
                    @location(1) color: vec4<f32>,
                    @location(2) mode: f32,
                };
                @vertex fn vs(
                    @location(0) p: vec2<f32>,
                    @location(1) uv: vec2<f32>,
                    @location(2) c: vec4<f32>,
                    @location(3) mode: f32,
                ) -> VOut {
                    var o: VOut;
                    o.pos = vec4<f32>(p, 0.0, 1.0);
                    o.uv = uv; o.color = c; o.mode = mode;
                    return o;
                }
                @group(0) @binding(0) var tex: texture_2d<f32>;
                @group(0) @binding(1) var samp: sampler;
                @fragment fn fs(in: VOut) -> @location(0) vec4<f32> {
                    let t = textureSample(tex, samp, in.uv);
                    if (in.mode < 0.5) {
                        return vec4<f32>(in.color.rgb, in.color.a * t.r);
                    }
                    return t;
                }
                "#
                .into(),
            ),
        });
        let icon_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui-icon-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let icon_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui-icon-layout"),
            bind_group_layouts: &[&icon_bgl],
            push_constant_ranges: &[],
        });
        let icon_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui-icon"),
            layout: Some(&icon_layout),
            vertex: wgpu::VertexState {
                module: &icon_shader,
                entry_point: "vs",
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 36,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 32,
                            shader_location: 3,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &icon_shader,
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
            multisample: MultisampleState {
                count: SAMPLES,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });
        let icon_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ui-icon-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let msaa = make_msaa(&device, config.width, config.height, format);
        Ok(Self {
            device,
            queue,
            surface,
            offscreen,
            config,
            scale,
            msaa,
            pipeline,
            vbuf,
            capacity,
            font_system,
            swash_cache,
            atlas,
            viewport,
            text_renderers,
            shaped_cache: std::collections::HashMap::new(),
            ui_font,
            sans_family,
            mono_family,
            icon_pipeline,
            icon_bgl,
            icon_sampler,
            icon_cache: std::collections::HashMap::new(),
        })
    }

    fn ensure_tex(&mut self, key: IconTexKey) {
        if self.icon_cache.contains_key(&key) {
            return;
        }
        let (width, height, format, bytes, bytes_per_row) = match key {
            IconTexKey::Mono(kind, px) => (
                px,
                px,
                wgpu::TextureFormat::R8Unorm,
                rasterize_icon(kind, px),
                px,
            ),
            IconTexKey::Material(m, px) => (
                px,
                px,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                rasterize_material(m, px),
                px * 4,
            ),
            IconTexKey::Image(id) => match images().lock().ok().and_then(|m| m.get(&id).cloned()) {
                Some((w, h, rgba)) => (w, h, wgpu::TextureFormat::Rgba8UnormSrgb, rgba, w * 4),
                None => return,
            },
        };
        if width == 0 || height == 0 {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui-icon-tex"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui-icon-bg"),
            layout: &self.icon_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.icon_sampler),
                },
            ],
        });
        self.icon_cache.insert(key, bind_group);
    }

    /// Set the active UI font family. Accepts the ".PomeloSans"/".PomeloMono" aliases or any real family
    /// name; unknown names fall back to the sans font so text never disappears.
    pub fn set_ui_font(&mut self, name: &str) {
        self.ui_font = match name {
            ".PomeloSans" => self.sans_family.clone(),
            ".PomeloMono" => self.mono_family.clone(),
            other => {
                let known = self
                    .font_system
                    .db()
                    .faces()
                    .any(|f| f.families.iter().any(|(n, _)| n == other));
                if known {
                    Some(other.to_string())
                } else {
                    self.sans_family.clone()
                }
            }
        };
        theme::set_active_ui_font(self.ui_font.clone());
        self.shaped_cache.clear();
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        self.msaa = make_msaa(
            &self.device,
            self.config.width,
            self.config.height,
            self.config.format,
        );
    }

    pub fn size(&self) -> (f32, f32) {
        (
            self.config.width as f32 / self.scale,
            self.config.height as f32 / self.scale,
        )
    }

    pub fn render(&mut self, rects: &[Rect], tris: &[Tri], texts: &[Text]) -> Result<()> {
        let vw = self.config.width as f32;
        let vh = self.config.height as f32;
        const FLOATS_PER_VERT: usize = 16; // pos2 + color4 + local2 + hsize2 + radius1 + bcolor4 + bwidth1
        let mut verts: Vec<f32> =
            Vec::with_capacity((rects.len() + tris.len()) * 6 * FLOATS_PER_VERT);
        let s = self.scale;
        let ndc = |x: f32, y: f32| ((x * s) / vw * 2.0 - 1.0, 1.0 - (y * s) / vh * 2.0);
        let lin = |c: Rgba| {
            [
                srgb_to_linear(c.r as f64) as f32,
                srgb_to_linear(c.g as f64) as f32,
                srgb_to_linear(c.b as f64) as f32,
                c.a,
            ]
        };
        for r in rects {
            let col = lin(r.color);
            let bcol = lin(r.border_color);
            let bw = r.border * s;
            let (l, t) = ndc(r.x, r.y);
            let (rr, b) = ndc(r.x + r.w, r.y + r.h);
            // SDF params in physical pixels: half-extent, and the corner-relative local coord per vertex.
            let hw = r.w * s / 2.0;
            let hh = r.h * s / 2.0;
            let rad = shader_radius(r, s, hw, hh);
            let corners = [
                (l, t, -hw, -hh),
                (rr, t, hw, -hh),
                (l, b, -hw, hh),
                (rr, t, hw, -hh),
                (rr, b, hw, hh),
                (l, b, -hw, hh),
            ];
            for (px, py, lx, ly) in corners {
                verts.extend_from_slice(&[
                    px, py, col[0], col[1], col[2], col[3], lx, ly, hw, hh, rad, bcol[0], bcol[1],
                    bcol[2], bcol[3], bw,
                ]);
            }
        }
        // Triangles: solid fill via the same pipeline (huge hsize + local 0 -> SDF always inside), no border.
        for tr in tris {
            let col = lin(tr.color);
            for [x, y] in tr.p {
                let (px, py) = ndc(x, y);
                verts.extend_from_slice(&[
                    px, py, col[0], col[1], col[2], col[3], 0.0, 0.0, 1.0e6, 1.0e6, 0.0, 0.0, 0.0,
                    0.0, 0.0, 0.0,
                ]);
            }
        }
        let vert_count = verts.len() / FLOATS_PER_VERT;
        if vert_count > self.capacity {
            self.capacity = vert_count.next_power_of_two();
            self.vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui-verts"),
                size: (self.capacity * 64) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !verts.is_empty() {
            self.queue
                .write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&verts));
        }

        // Shape each text run into its own buffer, then prepare the glyph atlas.
        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: self.config.width as i32,
            bottom: self.config.height as i32,
        };
        let sans = self.ui_font.clone();
        let mono = self.mono_family.clone().or_else(|| sans.clone());
        let mut buffers: Vec<Buffer> = Vec::with_capacity(texts.len());
        for t in texts {
            let fam = if t.mono { &mono } else { &sans };
            let family = match fam {
                Some(name) => Family::Name(name),
                None => Family::SansSerif,
            };
            let sz = t.size * theme::ui_text_scale();
            let wrap_w = if t.wrap > 0.0 {
                t.wrap * theme::ui_text_scale()
            } else {
                vw / self.scale
            };
            let weight = theme::snap_weight(fam.as_deref(), t.weight);
            let mut buf = Buffer::new(&mut self.font_system, Metrics::new(sz, sz * 1.3));
            buf.set_size(&mut self.font_system, Some(wrap_w), None);
            buf.set_text(
                &mut self.font_system,
                &t.text,
                Attrs::new()
                    .family(family)
                    .weight(Weight(weight))
                    .color(glyph_color(t.color)),
                Shaping::Advanced,
            );
            buf.shape_until_scroll(&mut self.font_system, false);
            buffers.push(buf);
        }
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let areas: Vec<TextArea> = texts
            .iter()
            .zip(&buffers)
            .map(|(t, buf)| TextArea {
                buffer: buf,
                // Snap to whole device pixels so glyphs aren't blurred by sub-pixel positioning.
                left: (t.x * self.scale).round(),
                top: (t.y * self.scale).round(),
                scale: self.scale,
                bounds,
                default_color: glyph_color(t.color),
                custom_glyphs: &[],
            })
            .collect();
        self.text_renderers[0].prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        )?;

        let frame = match &self.surface {
            Some(surface) => Some(surface.get_current_texture()?),
            None => None,
        };
        let target = match (&frame, &self.offscreen) {
            (Some(f), _) => &f.texture,
            (None, Some(tex)) => tex,
            (None, None) => return Ok(()),
        };
        let view = target.create_view(&TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("ui"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &self.msaa,
                    resolve_target: Some(&view),
                    ops: Operations {
                        load: LoadOp::Clear(wgpu::Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if vert_count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, self.vbuf.slice(..));
                pass.draw(0..vert_count as u32, 0..1);
            }
            self.text_renderers[0].render(&self.atlas, &self.viewport, &mut pass)?;
        }
        self.queue.submit(Some(encoder.finish()));
        if let Some(frame) = frame {
            frame.present();
        }
        self.atlas.trim();
        Ok(())
    }

    /// Read the offscreen framebuffer back as RGBA8 (headless only). Call after `render`.
    pub fn read_rgba(&self) -> Result<(u32, u32, Vec<u8>)> {
        let tex = self
            .offscreen
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("read_rgba requires a headless renderer"))?;
        let (w, h) = (self.config.width, self.config.height);
        let bytes_per_row = (w * 4).div_ceil(256) * 256; // wgpu requires 256-byte-aligned rows
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bytes_per_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &buf,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::Maintain::Wait);
        let data = slice.get_mapped_range();
        // De-pad rows and swap BGRA -> RGBA.
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let start = (row * bytes_per_row) as usize;
            for px in 0..w as usize {
                let p = start + px * 4;
                out.extend_from_slice(&[data[p + 2], data[p + 1], data[p], data[p + 3]]);
            }
        }
        drop(data);
        buf.unmap();
        Ok((w, h, out))
    }

    /// Font family names for a picker: our ".PomeloSans"/".PomeloMono" aliases first, then the system fonts.
    pub fn font_families(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .font_system
            .db()
            .faces()
            .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
            .filter(|n| !n.starts_with('.')) // hide private system fonts
            .collect();
        v.sort();
        v.dedup();
        let mut out = vec![".PomeloSans".to_string(), ".PomeloMono".to_string()];
        out.extend(v);
        out
    }

    /// Draw `base` then `overlay` in one frame (two passes), so the overlay floats above without the base's
    /// text bleeding through (all text draws after all rects within a single pass). Used for popovers.
    pub fn render_layered(
        &mut self,
        base: (&[Rect], &[Tri], &[Text]),
        overlay: (&[Rect], &[Tri], &[Text]),
    ) -> Result<()> {
        self.render_layered_clip(base, overlay, None, None, Rgba::new(0.0, 0.0, 0.0, 1.0))
    }

    /// Like `render_layered`, but scissors the base layer to `base_clip` and/or the overlay layer to
    /// `overlay_clip` (both logical px) and clears to `clear`. `overlay_clip` lets a scrolling list draw
    /// smoothly inside fixed chrome (e.g. a menu whose list scrolls between a fixed search and footer).
    pub fn render_layered_clip(
        &mut self,
        base: (&[Rect], &[Tri], &[Text]),
        overlay: (&[Rect], &[Tri], &[Text]),
        base_clip: Option<(f32, f32, f32, f32)>,
        overlay_clip: Option<(f32, f32, f32, f32)>,
        clear: Rgba,
    ) -> Result<()> {
        self.render_frame(
            clear,
            &[
                (base.0, base.1, base.2, &[], base_clip),
                (overlay.0, overlay.1, overlay.2, &[], overlay_clip),
            ],
        )
    }

    /// Three-layer render (base, overlay, optional unclipped `top`), kept for callers that want a tooltip layer.
    pub fn render_layers(
        &mut self,
        base: (&[Rect], &[Tri], &[Text]),
        overlay: (&[Rect], &[Tri], &[Text]),
        top: Option<(&[Rect], &[Tri], &[Text])>,
        base_clip: Option<(f32, f32, f32, f32)>,
        overlay_clip: Option<(f32, f32, f32, f32)>,
        clear: Rgba,
    ) -> Result<()> {
        let mut layers: Vec<Layer> = vec![
            (base.0, base.1, base.2, &[], base_clip),
            (overlay.0, overlay.1, overlay.2, &[], overlay_clip),
        ];
        if let Some(t) = top {
            layers.push((t.0, t.1, t.2, &[], None));
        }
        self.render_frame(clear, &layers)
    }

    /// Composite N layers back-to-front into one frame: layer 0 clears to `clear`, later layers load over it;
    /// each layer draws its shapes+text with an optional scissor clip and its own text renderer. This is how a
    /// scrolling menu list (clipped) sits above the body yet below an unclipped tooltip.
    pub fn render_frame(&mut self, clear: Rgba, layers: &[Layer]) -> Result<()> {
        const FPV: usize = 16;
        let mut verts: Vec<f32> = Vec::new();
        let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(layers.len());
        for (rects, tris, _, _, _) in layers {
            let start = verts.len() / FPV;
            verts.extend(self.verts_for(rects, tris));
            ranges.push((start, verts.len() / FPV));
        }
        // Icons: rasterize/cache each needed (icon, px) mask, then build a textured-quad vertex buffer with a
        // per-layer draw list of (bind-group key, vertex range).
        const IPV: usize = 9; // pos2 + uv2 + color4 + mode1
        let mut icon_verts: Vec<f32> = Vec::new();
        let mut icon_draws: Vec<Vec<IconDraw>> = Vec::with_capacity(layers.len());
        {
            let vw = self.config.width as f32;
            let vh = self.config.height as f32;
            let s = self.scale;
            for (_, _, _, icons, _) in layers {
                let mut draws = Vec::new();
                for q in icons.iter() {
                    let px = (q.w * s).round().max(1.0) as u32;
                    let (key, mode, c, rect) = if let Some(id) = q.image {
                        let (iw, ih) = images()
                            .lock()
                            .ok()
                            .and_then(|m| m.get(&id).map(|(w, h, _)| (*w as f32, *h as f32)))
                            .unwrap_or((q.w.max(1.0), q.h.max(1.0)));
                        let fit = (q.w / iw).min(q.h / ih).clamp(0.0, 1.0);
                        let (fw, fh) = (iw * fit, ih * fit);
                        let rect = (q.x + (q.w - fw) / 2.0, q.y + (q.h - fh) / 2.0, fw, fh);
                        (IconTexKey::Image(id), 1.0f32, [1.0, 1.0, 1.0, 1.0], rect)
                    } else if let Some(m) = q.material {
                        (
                            IconTexKey::Material(m, px),
                            1.0f32,
                            [1.0, 1.0, 1.0, 1.0],
                            (q.x, q.y, q.w, q.h),
                        )
                    } else {
                        (
                            IconTexKey::Mono(q.kind, px),
                            0.0,
                            [
                                srgb_to_linear(q.color.r as f64) as f32,
                                srgb_to_linear(q.color.g as f64) as f32,
                                srgb_to_linear(q.color.b as f64) as f32,
                                q.color.a,
                            ],
                            (q.x, q.y, q.w, q.h),
                        )
                    };
                    self.ensure_tex(key);
                    let ndc = |x: f32, y: f32| ((x * s) / vw * 2.0 - 1.0, 1.0 - (y * s) / vh * 2.0);
                    let (l, t) = ndc(rect.0, rect.1);
                    let (r, b) = ndc(rect.0 + rect.2, rect.1 + rect.3);
                    let start = (icon_verts.len() / IPV) as u32;
                    for (px_, py_, u, v) in [
                        (l, t, 0.0, 0.0),
                        (r, t, 1.0, 0.0),
                        (l, b, 0.0, 1.0),
                        (r, t, 1.0, 0.0),
                        (r, b, 1.0, 1.0),
                        (l, b, 0.0, 1.0),
                    ] {
                        icon_verts
                            .extend_from_slice(&[px_, py_, u, v, c[0], c[1], c[2], c[3], mode]);
                    }
                    let end = (icon_verts.len() / IPV) as u32;
                    draws.push((key, start, end));
                }
                icon_draws.push(draws);
            }
        }
        let icon_vbuf = if icon_verts.is_empty() {
            None
        } else {
            Some(
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("ui-icon-verts"),
                        contents: bytemuck::cast_slice(&icon_verts),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
            )
        };
        let total_vc = verts.len() / FPV;
        if total_vc > self.capacity {
            self.capacity = total_vc.next_power_of_two();
            self.vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui-verts"),
                size: (self.capacity * 64) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !verts.is_empty() {
            self.queue
                .write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&verts));
        }
        // Prepare all layers' text before any pass, so the atlas is final and every bind group is valid.
        for (i, (_, _, texts, _, _)) in layers.iter().enumerate() {
            self.prepare_layer(texts, i as u8)?;
        }

        let frame = match &self.surface {
            Some(surface) => Some(surface.get_current_texture()?),
            None => None,
        };
        let target = match (&frame, &self.offscreen) {
            (Some(f), _) => &f.texture,
            (None, Some(tex)) => tex,
            (None, None) => return Ok(()),
        };
        let view = target.create_view(&TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for (i, (_, _, _, _, clip)) in layers.iter().enumerate() {
            let load = if i == 0 {
                LoadOp::Clear(wgpu::Color {
                    r: srgb_to_linear(clear.r as f64),
                    g: srgb_to_linear(clear.g as f64),
                    b: srgb_to_linear(clear.b as f64),
                    a: clear.a as f64,
                })
            } else {
                LoadOp::Load
            };
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("ui-layer"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &self.msaa,
                    resolve_target: Some(&view),
                    ops: Operations {
                        load,
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some((cx, cy, cw, ch)) = *clip {
                let s = self.scale;
                let px = |v: f32| (v * s).max(0.0) as u32;
                let x = px(cx).min(self.config.width);
                let y = px(cy).min(self.config.height);
                let w = px(cw).min(self.config.width.saturating_sub(x));
                let h = px(ch).min(self.config.height.saturating_sub(y));
                // An empty clip hides the whole layer; wgpu rejects a zero-size scissor, so skip drawing.
                if w == 0 || h == 0 {
                    continue;
                }
                pass.set_scissor_rect(x, y, w, h);
            }
            let (start, end) = ranges[i];
            if end > start {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, self.vbuf.slice(..));
                pass.draw(start as u32..end as u32, 0..1);
            }
            // Icons for this layer: one draw per icon, binding its cached alpha mask.
            if let Some(ibuf) = &icon_vbuf {
                if !icon_draws[i].is_empty() {
                    pass.set_pipeline(&self.icon_pipeline);
                    pass.set_vertex_buffer(0, ibuf.slice(..));
                    for (key, vs, ve) in &icon_draws[i] {
                        if let Some(bg) = self.icon_cache.get(key) {
                            pass.set_bind_group(0, bg, &[]);
                            pass.draw(*vs..*ve, 0..1);
                        }
                    }
                }
            }
            self.text_renderers[i].render(&self.atlas, &self.viewport, &mut pass)?;
        }
        self.queue.submit(Some(encoder.finish()));
        if let Some(frame) = frame {
            frame.present();
        }
        self.atlas.trim();
        Ok(())
    }

    fn verts_for(&self, rects: &[Rect], tris: &[Tri]) -> Vec<f32> {
        let vw = self.config.width as f32;
        let vh = self.config.height as f32;
        let s = self.scale;
        let ndc = |x: f32, y: f32| ((x * s) / vw * 2.0 - 1.0, 1.0 - (y * s) / vh * 2.0);
        let lin = |c: Rgba| {
            [
                srgb_to_linear(c.r as f64) as f32,
                srgb_to_linear(c.g as f64) as f32,
                srgb_to_linear(c.b as f64) as f32,
                c.a,
            ]
        };
        let mut verts: Vec<f32> = Vec::with_capacity((rects.len() + tris.len()) * 6 * 16);
        for r in rects {
            let col = lin(r.color);
            let bcol = lin(r.border_color);
            let bw = r.border * s;
            let (l, t) = ndc(r.x, r.y);
            let (rr, b) = ndc(r.x + r.w, r.y + r.h);
            let hw = r.w * s / 2.0;
            let hh = r.h * s / 2.0;
            let rad = shader_radius(r, s, hw, hh);
            for (px, py, lx, ly) in [
                (l, t, -hw, -hh),
                (rr, t, hw, -hh),
                (l, b, -hw, hh),
                (rr, t, hw, -hh),
                (rr, b, hw, hh),
                (l, b, -hw, hh),
            ] {
                verts.extend_from_slice(&[
                    px, py, col[0], col[1], col[2], col[3], lx, ly, hw, hh, rad, bcol[0], bcol[1],
                    bcol[2], bcol[3], bw,
                ]);
            }
        }
        for tr in tris {
            let col = lin(tr.color);
            for [x, y] in tr.p {
                let (px, py) = ndc(x, y);
                verts.extend_from_slice(&[
                    px, py, col[0], col[1], col[2], col[3], 0.0, 0.0, 1.0e6, 1.0e6, 0.0, 0.0, 0.0,
                    0.0, 0.0, 0.0,
                ]);
            }
        }
        verts
    }

    fn prepare_layer(&mut self, texts: &[Text], layer: u8) -> Result<()> {
        let bounds = TextBounds {
            left: 0,
            top: 0,
            right: self.config.width as i32,
            bottom: self.config.height as i32,
        };
        let sans = self.ui_font.clone();
        let mono = self.mono_family.clone().or_else(|| sans.clone());
        let scale_ui = theme::ui_text_scale();

        if self.shaped_cache.len() > 20_000 {
            self.shaped_cache.clear();
        }

        let mut keys: Vec<u64> = Vec::with_capacity(texts.len());
        for t in texts {
            let fam = if t.mono { &mono } else { &sans };
            let sz = t.size * scale_ui;
            let weight = theme::snap_weight(fam.as_deref(), t.weight);
            let wrap_w = if t.wrap > 0.0 {
                Some(t.wrap * scale_ui)
            } else {
                None
            };
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                t.text.hash(&mut h);
                sz.to_bits().hash(&mut h);
                weight.hash(&mut h);
                t.mono.hash(&mut h);
                wrap_w.map(f32::to_bits).hash(&mut h);
                fam.hash(&mut h);
                h.finish()
            };
            keys.push(key);
            if !self.shaped_cache.contains_key(&key) {
                let family = match fam {
                    Some(name) => Family::Name(name),
                    None => Family::SansSerif,
                };
                let mut buf = Buffer::new(&mut self.font_system, Metrics::new(sz, sz * 1.3));
                buf.set_size(&mut self.font_system, wrap_w, None);
                buf.set_text(
                    &mut self.font_system,
                    &t.text,
                    Attrs::new().family(family).weight(Weight(weight)),
                    Shaping::Advanced,
                );
                buf.shape_until_scroll(&mut self.font_system, false);
                self.shaped_cache.insert(key, buf);
            }
        }
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        while self.text_renderers.len() <= layer as usize {
            self.text_renderers.push(TextRenderer::new(
                &mut self.atlas,
                &self.device,
                MultisampleState {
                    count: SAMPLES,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                None,
            ));
        }
        let areas: Vec<TextArea> = texts
            .iter()
            .zip(&keys)
            .map(|(t, key)| TextArea {
                buffer: &self.shaped_cache[key],
                left: (t.x * self.scale).round(),
                top: (t.y * self.scale).round(),
                scale: self.scale,
                bounds,
                default_color: glyph_color(t.color),
                custom_glyphs: &[],
            })
            .collect();
        let renderer = &mut self.text_renderers[layer as usize];
        renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_glyph_offsets() {
        let (glyphs, width) = measure_glyphs("abc", 14.0, true, 400);
        assert_eq!(
            glyphs.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(glyphs.windows(2).all(|w| w[0].1 < w[1].1));
        assert!((width - measure_text_width("abc", 14.0, true, 400)).abs() < 0.01);
    }
}
