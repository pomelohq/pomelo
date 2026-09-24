//! A tiny element tree with flexbox-ish layout, styling and hit-testing, so screens are built declaratively
//! (`div().col().child(label(...))`) instead of hand-placing rects and text. Model (CSS flexbox subset): a
//! div has an explicit `w`/`h` (pixels or auto=content) plus a `grow` factor. The parent lays children along
//! its own axis: `grow` children share leftover main-axis space; the cross axis stretches divs to fill and
//! left/top-aligns (or centers, with `items_center`) content. `render` flattens the tree into the `Rect`/
//! `Text` primitives the GPU layer draws, plus click regions for hit-testing.

use crate::{theme, Rect, Rgba, Text, Tri};

/// Scale a design px value by the active UI text scale, so paddings/sizes zoom with the font size like the
/// reference's rem-based spacing.
fn rem(v: f32) -> f32 {
    v * crate::ui_text_scale()
}

#[derive(Clone, Copy)]
pub enum Len {
    Px(f32),
    Auto,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Row,
    Col,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Justify {
    Start,
    Center,
    End,
    Between,
}

#[derive(Clone)]
pub enum Node {
    Div(Div),
    Label(Label),
    Icon(Icon),
    Anchored(Box<Anchored>),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconKind {
    ChevronRight,
    ChevronDown,
    ChevronUpDown,
    Search,
    Close,
    Check,
    Warning,
    XCircle,
    Plus,
    Folder,
    FolderOpen,
    File,
    Monitor,
    ArrowUpRight,
    ArrowLeft,
    ArrowRight,
    Window,
    Grid,
    Branch,
    Cylinder,
    Ticket,
    Terminal,
    Diamond,
    Sparkle,
    Sidebar,
    PanelRight,
    PanelBottom,
    Maximize,
    Minimize,
    Pin,
    Server,
    Undo,
    ChevronLeft,
    CaseSensitive,
    WholeWord,
    Regex,
    Replace,
    ReplaceNext,
    ReplaceAll,
    SelectAll,
    Quote,
    Command,
    Shift,
    Option,
    Control,
    ArrowUp,
    ArrowDown,
    Return,
    Backspace,
    Tab,
    KeyArrowLeft,
    KeyArrowRight,
    DiffUnified,
    DiffSplit,
    FileGit,
    Play,
    Stop,
    RotateCw,
    SquarePlus,
    SquareDot,
    SquareMinus,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialIcon {
    Rust,
    Go,
    TypeScript,
    React,
    JavaScript,
    Json,
    Markdown,
    Toml,
    Yaml,
    Html,
    Css,
    Sass,
    Python,
    Lock,
    Console,
    Document,
    Image,
    Git,
    NodeJs,
}

#[derive(Clone, Copy)]
pub struct Icon {
    kind: IconKind,
    size: f32,
    color: Rgba,
    material: Option<MaterialIcon>,
}

/// Build an icon of any `kind` at the default size/muted color (chain `.size`/`.color`).
pub fn icon(kind: IconKind) -> Icon {
    Icon {
        kind,
        size: 14.0,
        color: theme().icon_muted,
        material: None,
    }
}

pub fn material_icon(material: MaterialIcon) -> Icon {
    Icon {
        kind: IconKind::File,
        size: 14.0,
        color: theme().icon_muted,
        material: Some(material),
    }
}

pub fn chevron_right() -> Icon {
    Icon {
        kind: IconKind::ChevronRight,
        size: 12.0,
        color: theme().icon_muted,
        material: None,
    }
}

pub fn chevron_down() -> Icon {
    Icon {
        kind: IconKind::ChevronDown,
        size: 12.0,
        color: theme().icon_muted,
        material: None,
    }
}

/// An up/down chevron (like a select control's affordance).
pub fn chevron_up_down() -> Icon {
    Icon {
        kind: IconKind::ChevronUpDown,
        size: 12.0,
        color: theme().icon_muted,
        material: None,
    }
}

/// A magnifying-glass search glyph.
pub fn search_icon() -> Icon {
    Icon {
        kind: IconKind::Search,
        size: 14.0,
        color: theme().icon_muted,
        material: None,
    }
}

/// An X (close/clear) glyph.
pub fn close_icon() -> Icon {
    Icon {
        kind: IconKind::Close,
        size: 14.0,
        color: theme().icon_muted,
        material: None,
    }
}

/// A checkmark glyph (marks the selected item in a menu).
pub fn check_icon() -> Icon {
    Icon {
        kind: IconKind::Check,
        size: 14.0,
        color: theme().icon_accent,
        material: None,
    }
}

/// A plus glyph (e.g. "new" actions).
pub fn plus_icon() -> Icon {
    Icon {
        kind: IconKind::Plus,
        size: 14.0,
        color: theme().icon_muted,
        material: None,
    }
}

/// A folder glyph (e.g. "open" actions).
pub fn folder_icon() -> Icon {
    Icon {
        kind: IconKind::Folder,
        size: 14.0,
        color: theme().icon_muted,
        material: None,
    }
}

/// A monitor/screen glyph (marks a project/session row).
pub fn monitor_icon() -> Icon {
    Icon {
        kind: IconKind::Monitor,
        size: 14.0,
        color: theme().icon_muted,
        material: None,
    }
}

/// An up-right arrow glyph (e.g. "open in new window").
pub fn arrow_up_right_icon() -> Icon {
    Icon {
        kind: IconKind::ArrowUpRight,
        size: 14.0,
        color: theme().icon_muted,
        material: None,
    }
}

/// A window (rounded box) glyph (e.g. "open in this window").
pub fn window_icon() -> Icon {
    Icon {
        kind: IconKind::Window,
        size: 14.0,
        color: theme().icon_muted,
        material: None,
    }
}

impl Icon {
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn color(mut self, c: Rgba) -> Self {
        self.color = c;
        self
    }
}

#[derive(Clone)]
pub struct Div {
    axis: Axis,
    w: Len,
    h: Len,
    grow: f32,
    pad: [f32; 4], // top, right, bottom, left
    gap: f32,
    justify: Justify,
    items_center: bool,
    bg: Option<Rgba>,
    radius: f32,
    border: f32,
    border_color: Rgba,
    click: Option<u64>,
    debug: Option<&'static str>,
    image: Option<u64>,
    children: Vec<Node>,
}

#[derive(Clone)]
pub struct Label {
    pub text: String,
    pub size: f32,
    pub color: Rgba,
    pub mono: bool,
    pub weight: u16,
    /// Wrap width in design px (0 = single line).
    pub wrap: f32,
    /// Shrink when its row overflows and cut the text with a trailing "...".
    pub truncate: bool,
    /// Cut from the front instead ("...rest"), keeping the end of a path visible.
    pub truncate_start: bool,
}

pub fn div() -> Div {
    Div {
        axis: Axis::Row,
        w: Len::Auto,
        h: Len::Auto,
        grow: 0.0,
        pad: [0.0; 4],
        gap: 0.0,
        justify: Justify::Start,
        items_center: false,
        bg: None,
        radius: 0.0,
        border: 0.0,
        border_color: Rgba::TRANSPARENT,
        click: None,
        debug: None,
        image: None,
        children: Vec::new(),
    }
}

pub fn label(text: impl Into<String>) -> Label {
    Label {
        text: text.into(),
        size: 13.0,
        color: theme().text,
        mono: false,
        weight: crate::ui_font_weight(),
        wrap: 0.0,
        truncate: false,
        truncate_start: false,
    }
}

impl Label {
    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
    pub fn color(mut self, c: Rgba) -> Self {
        self.color = c;
        self
    }
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    pub fn weight(mut self, weight: u16) -> Self {
        self.weight = weight;
        self
    }
    pub fn medium(self) -> Self {
        self.weight(500)
    }
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }
    pub fn truncate_start(mut self) -> Self {
        self.truncate = true;
        self.truncate_start = true;
        self
    }
    /// Wrap to `w` design px across multiple lines instead of a single line.
    pub fn wrap(mut self, w: f32) -> Self {
        self.wrap = w;
        self
    }
    fn intrinsic(&self) -> (f32, f32) {
        if self.wrap > 0.0 {
            return crate::measure_wrapped(
                &self.text,
                self.size,
                self.mono,
                self.weight,
                self.wrap,
            );
        }
        (
            crate::measure_text_width(&self.text, self.size, self.mono, self.weight),
            self.size * crate::ui_text_scale() * 1.4,
        )
    }
}

impl Div {
    pub fn row(mut self) -> Self {
        self.axis = Axis::Row;
        self
    }
    pub fn col(mut self) -> Self {
        self.axis = Axis::Col;
        self
    }
    pub fn w_px(mut self, v: f32) -> Self {
        self.w = Len::Px(rem(v));
        self
    }
    pub fn h_px(mut self, v: f32) -> Self {
        self.h = Len::Px(rem(v));
        self
    }
    /// Fill remaining space along the parent's main axis.
    pub fn flex(mut self, v: f32) -> Self {
        self.grow = v;
        self
    }
    pub fn p(mut self, v: f32) -> Self {
        self.pad = [rem(v); 4];
        self
    }
    pub fn px(mut self, v: f32) -> Self {
        self.pad[1] = rem(v);
        self.pad[3] = rem(v);
        self
    }
    pub fn py(mut self, v: f32) -> Self {
        self.pad[0] = rem(v);
        self.pad[2] = rem(v);
        self
    }
    pub fn pt(mut self, v: f32) -> Self {
        self.pad[0] = rem(v);
        self
    }
    pub fn pb(mut self, v: f32) -> Self {
        self.pad[2] = rem(v);
        self
    }
    pub fn pl(mut self, v: f32) -> Self {
        self.pad[3] = rem(v);
        self
    }
    pub fn pr(mut self, v: f32) -> Self {
        self.pad[1] = rem(v);
        self
    }
    pub fn gap(mut self, v: f32) -> Self {
        self.gap = rem(v);
        self
    }
    pub fn justify_between(mut self) -> Self {
        self.justify = Justify::Between;
        self
    }
    pub fn justify_center(mut self) -> Self {
        self.justify = Justify::Center;
        self
    }
    pub fn justify_end(mut self) -> Self {
        self.justify = Justify::End;
        self
    }
    pub fn items_center(mut self) -> Self {
        self.items_center = true;
        self
    }
    pub fn bg(mut self, c: Rgba) -> Self {
        self.bg = Some(c);
        self
    }
    pub fn rounded(mut self, r: f32) -> Self {
        self.radius = rem(r);
        self
    }
    pub fn border(mut self, width: f32, color: Rgba) -> Self {
        self.border = rem(width);
        self.border_color = color;
        self
    }
    pub fn on_click(mut self, id: u64) -> Self {
        self.click = Some(id);
        self
    }
    pub fn image(mut self, id: u64) -> Self {
        self.image = Some(id);
        self
    }
    /// Tag this div's laid-out bounds under `name` so tests can assert layout headlessly (the framework's
    /// `debug_selector`). No effect on drawing.
    pub fn debug(mut self, name: &'static str) -> Self {
        self.debug = Some(name);
        self
    }
    pub fn child(mut self, node: impl Into<Node>) -> Self {
        self.children.push(node.into());
        self
    }
    pub fn children(mut self, nodes: impl IntoIterator<Item = Node>) -> Self {
        self.children.extend(nodes);
        self
    }

    fn intrinsic(&self) -> (f32, f32) {
        let (pt, pr, pb, pl) = (self.pad[0], self.pad[1], self.pad[2], self.pad[3]);
        let (mut main, mut cross) = (0.0f32, 0.0f32);
        let n = self.children.len();
        for (i, c) in self.children.iter().enumerate() {
            let (cw, ch) = intrinsic(c);
            let (cm, cc) = match self.axis {
                Axis::Row => (cw, ch),
                Axis::Col => (ch, cw),
            };
            main += cm;
            if i + 1 < n {
                main += self.gap;
            }
            cross = cross.max(cc);
        }
        match self.axis {
            Axis::Row => (main + pl + pr, cross + pt + pb),
            Axis::Col => (cross + pl + pr, main + pt + pb),
        }
    }
}

/// An overlay element (the framework's `deferred(anchored(...))` idiom): its child is laid out absolutely and painted in
/// a later layer, above the base tree, with an optional scissor clip. Popovers, dropdown panels, tooltips and
/// clipped scroll regions are expressed this way so their text/rects never bleed into the base layer's pass.
/// `priority` orders overlays back-to-front (higher on top). Positions/sizes are in logical px already at the
/// view's scale (not rem-scaled again).
#[derive(Clone)]
pub struct Anchored {
    child: Box<Node>,
    position: Option<(f32, f32)>,
    size: Option<(f32, f32)>,
    priority: usize,
    clip: bool,
    clip_rect: Option<Rect>,
    snap: bool,
}

/// An overlay anchored at an explicit window position (top-left). Chain `.position`, `.size`, `.priority`,
/// `.clip`, `.snap_to_window`.
pub fn anchored() -> Anchored {
    Anchored {
        child: Box::new(div().into()),
        position: None,
        size: None,
        priority: 0,
        clip: false,
        clip_rect: None,
        snap: false,
    }
}

/// An overlay painted above ancestors at the point where it sits in the flow (the framework's `deferred`).
pub fn deferred(child: impl Into<Node>) -> Anchored {
    anchored().child(child)
}

impl Anchored {
    pub fn child(mut self, child: impl Into<Node>) -> Self {
        self.child = Box::new(child.into());
        self
    }
    /// Anchor the child's top-left to this window position; without it the child sits at its flow position.
    pub fn position(mut self, x: f32, y: f32) -> Self {
        self.position = Some((x, y));
        self
    }
    /// Force the overlay's size; without it the child's intrinsic size is used.
    pub fn size(mut self, w: f32, h: f32) -> Self {
        self.size = Some((w, h));
        self
    }
    /// Draw order among overlays; higher paints on top.
    pub fn priority(mut self, p: usize) -> Self {
        self.priority = p;
        self
    }
    /// Scissor the child to its resolved bounds (for a clipped scroll region).
    pub fn clip(mut self) -> Self {
        self.clip = true;
        self
    }
    /// Scissor the child to an explicit rect (logical px) distinct from its layout bounds. Used for a scroll
    /// region whose content is taller than the visible window: place the child at `content_top - scroll` with
    /// the full content height, but clip to the visible band.
    pub fn clip_rect(mut self, x: f32, y: f32, w: f32, h: f32) -> Self {
        self.clip_rect = Some(Rect::new(x, y, w, h, Rgba::TRANSPARENT));
        self
    }
    /// Keep the child inside the window bounds instead of overflowing off-screen.
    pub fn snap_to_window(mut self) -> Self {
        self.snap = true;
        self
    }
}

impl From<Anchored> for Node {
    fn from(a: Anchored) -> Self {
        Node::Anchored(Box::new(a))
    }
}

fn intrinsic(node: &Node) -> (f32, f32) {
    match node {
        Node::Anchored(_) => (0.0, 0.0),
        Node::Div(d) => match (d.w, d.h) {
            (Len::Px(w), Len::Px(h)) => (w, h),
            _ => {
                let (iw, ih) = d.intrinsic();
                (
                    if let Len::Px(w) = d.w { w } else { iw },
                    if let Len::Px(h) = d.h { h } else { ih },
                )
            }
        },
        Node::Label(l) => l.intrinsic(),
        Node::Icon(i) => (rem(i.size), rem(i.size)),
    }
}

/// (explicit-len-along-axis, grow) for a child, from the *parent's* axis.
fn child_main(node: &Node, parent: Axis) -> (Len, f32) {
    match node {
        Node::Div(d) => (if parent == Axis::Row { d.w } else { d.h }, d.grow),
        Node::Label(_) | Node::Icon(_) | Node::Anchored(_) => (Len::Auto, 0.0),
    }
}

fn child_cross_len(node: &Node, parent: Axis) -> Len {
    match node {
        Node::Div(d) => {
            if parent == Axis::Row {
                d.h
            } else {
                d.w
            }
        }
        Node::Label(_) | Node::Icon(_) | Node::Anchored(_) => Len::Auto,
    }
}

/// Leaf content (label/icon) aligns within its slot; containers stretch to fill. An anchored overlay takes no
/// space in flow and is collected separately, so it counts as a leaf here.
fn is_leaf(node: &Node) -> bool {
    matches!(node, Node::Label(_) | Node::Icon(_) | Node::Anchored(_))
}

impl From<Div> for Node {
    fn from(d: Div) -> Self {
        Node::Div(d)
    }
}
impl From<Label> for Node {
    fn from(l: Label) -> Self {
        Node::Label(l)
    }
}
impl From<Icon> for Node {
    fn from(i: Icon) -> Self {
        Node::Icon(i)
    }
}

/// A single icon to draw: a target square (logical px), the SVG it maps to, and the tint color. The renderer
/// rasterizes the SVG to an alpha mask (cached per size) and draws it tinted by `color`.
#[derive(Clone, Copy)]
pub struct IconQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub kind: IconKind,
    pub color: Rgba,
    pub material: Option<MaterialIcon>,
    pub image: Option<u64>,
}

#[derive(Default)]
pub struct Painted {
    pub rects: Vec<Rect>,
    pub tris: Vec<Tri>,
    pub texts: Vec<Text>,
    pub icons: Vec<IconQuad>,
    /// (region, click id) for the app to hit-test, innermost last.
    pub hits: Vec<(Rect, u64)>,
    /// (selector, laid-out bounds) for divs tagged with `.debug(name)`; test-only introspection.
    pub debug_bounds: Vec<(&'static str, Rect)>,
}

/// One painted overlay layer (a popover/dropdown/tooltip or a clipped scroll region), to composite above the
/// base with an optional logical-px scissor `clip`.
pub struct Overlay {
    pub painted: Painted,
    pub clip: Option<Rect>,
}

/// The full result of laying out a view: the base layer plus overlays in back-to-front draw order, and the
/// merged hit regions (base first, then overlays; overlay hits outside their clip are dropped).
#[derive(Default)]
pub struct Frame {
    pub base: Painted,
    pub overlays: Vec<Overlay>,
    pub hits: Vec<(Rect, u64)>,
}

impl Frame {
    /// Bounds of the element tagged `.debug(name)`, searching base then overlays (the framework's `debug_bounds`). For
    /// headless layout assertions in tests.
    pub fn debug_bound(&self, name: &str) -> Option<Rect> {
        self.base
            .debug_bounds
            .iter()
            .chain(
                self.overlays
                    .iter()
                    .flat_map(|o| o.painted.debug_bounds.iter()),
            )
            .find(|(n, _)| *n == name)
            .map(|(_, r)| *r)
    }
}

/// A deferred overlay awaiting paint: its subtree, resolved bounds, clip and draw priority.
struct Pending {
    node: Node,
    area: Rect,
    clip: Option<Rect>,
    priority: usize,
}

/// Lay `node` into `area` (logical px), collecting overlays into their own layers. This is the path the window
/// driver uses so a popover's rects/text form a separate compositing layer above the base.
pub fn paint_frame(node: &Node, area: Rect) -> Frame {
    let viewport = area;
    let mut base = Painted::default();
    let mut pending: Vec<Pending> = Vec::new();
    place(node, area, viewport, &mut base, &mut pending);

    // Process overlays FIFO so nested overlays (a menu opened from a popover) queue after their parent; a stable
    // sort by priority then keeps insertion order within a priority band.
    let mut overlays: Vec<(usize, Overlay)> = Vec::new();
    let mut i = 0;
    while i < pending.len() {
        let p = std::mem::replace(
            &mut pending[i],
            Pending {
                node: Node::Div(div()),
                area,
                clip: None,
                priority: 0,
            },
        );
        i += 1;
        let mut painted = Painted::default();
        place(&p.node, p.area, viewport, &mut painted, &mut pending);
        if let Some(clip) = p.clip {
            painted.hits.retain(|(r, _)| {
                r.y + r.h > clip.y
                    && r.y < clip.y + clip.h
                    && r.x + r.w > clip.x
                    && r.x < clip.x + clip.w
            });
        }
        overlays.push((
            p.priority,
            Overlay {
                painted,
                clip: p.clip,
            },
        ));
    }
    overlays.sort_by_key(|(pri, _)| *pri);

    let mut hits = base.hits.clone();
    let overlays: Vec<Overlay> = overlays
        .into_iter()
        .map(|(_, o)| {
            hits.extend(o.painted.hits.iter().copied());
            o
        })
        .collect();

    Frame {
        base,
        overlays,
        hits,
    }
}

/// Flatten `node` into a single `Painted`. Overlays are appended after the base (kept for simple trees with no
/// overlays; use `paint_frame` when overlays must composite as separate layers).
pub fn render(node: &Node, area: Rect) -> Painted {
    let frame = paint_frame(node, area);
    let mut out = frame.base;
    for o in frame.overlays {
        out.rects.extend(o.painted.rects);
        out.tris.extend(o.painted.tris);
        out.texts.extend(o.painted.texts);
        out.icons.extend(o.painted.icons);
    }
    out.hits = frame.hits;
    out
}

fn place(node: &Node, area: Rect, viewport: Rect, out: &mut Painted, pending: &mut Vec<Pending>) {
    match node {
        Node::Anchored(a) => {
            let (iw, ih) = a.size.unwrap_or_else(|| intrinsic(&a.child));
            let (mut x, mut y) = a.position.unwrap_or((area.x, area.y));
            if a.snap {
                x = x.min(viewport.x + viewport.w - iw).max(viewport.x);
                y = y.min(viewport.y + viewport.h - ih).max(viewport.y);
            }
            let child_area = Rect::new(x, y, iw, ih, Rgba::TRANSPARENT);
            let clip = a.clip_rect.or(if a.clip { Some(child_area) } else { None });
            pending.push(Pending {
                node: (*a.child).clone(),
                area: child_area,
                clip,
                priority: a.priority,
            });
        }
        Node::Label(l) => {
            let (lw, lh) = l.intrinsic();
            let text = if l.truncate_start && lw > area.w + 0.5 {
                truncate_start_to_width(&l.text, area.w, l.size, l.mono, l.weight)
            } else if l.truncate && lw > area.w + 0.5 {
                truncate_to_width(&l.text, area.w, l.size, l.mono, l.weight)
            } else {
                l.text.clone()
            };
            out.texts.push(Text {
                x: area.x,
                y: area.y + (area.h - lh).max(0.0) / 2.0,
                size: l.size,
                color: l.color,
                text,
                mono: l.mono,
                weight: l.weight,
                wrap: l.wrap,
            });
        }
        Node::Div(d) => {
            if d.bg.is_some() || d.border > 0.0 {
                out.rects.push(Rect {
                    x: area.x,
                    y: area.y,
                    w: area.w,
                    h: area.h,
                    color: d.bg.unwrap_or(Rgba::TRANSPARENT),
                    radius: d.radius,
                    border: d.border,
                    border_color: d.border_color,
                });
            }
            if let Some(id) = d.click {
                out.hits.push((area, id));
            }
            if let Some(img) = d.image {
                out.icons.push(IconQuad {
                    x: area.x,
                    y: area.y,
                    w: area.w,
                    h: area.h,
                    kind: IconKind::Close, // unused for images
                    color: Rgba::TRANSPARENT,
                    material: None,
                    image: Some(img),
                });
            }
            if let Some(name) = d.debug {
                out.debug_bounds.push((name, area));
            }
            layout_children(d, area, viewport, out, pending);
        }
        Node::Icon(i) => {
            // Icons are real SVGs, rasterized to an alpha mask and tinted at draw time (in the renderer).
            // Here we only record the target square, centered in `area` at the icon's logical size.
            let s = rem(i.size);
            let cx = area.x + area.w / 2.0;
            let cy = area.y + area.h / 2.0;
            out.icons.push(IconQuad {
                x: cx - s / 2.0,
                y: cy - s / 2.0,
                w: s,
                h: s,
                kind: i.kind,
                color: i.color,
                material: i.material,
                image: None,
            });
        }
    }
}

fn layout_children(
    d: &Div,
    area: Rect,
    viewport: Rect,
    out: &mut Painted,
    pending: &mut Vec<Pending>,
) {
    let (pt, pr, pb, pl) = (d.pad[0], d.pad[1], d.pad[2], d.pad[3]);
    let inner = Rect {
        x: area.x + pl,
        y: area.y + pt,
        w: (area.w - pl - pr).max(0.0),
        h: (area.h - pt - pb).max(0.0),
        color: Rgba::TRANSPARENT,
        radius: 0.0,
        border: 0.0,
        border_color: Rgba::TRANSPARENT,
    };
    let (inner_main, inner_cross) = match d.axis {
        Axis::Row => (inner.w, inner.h),
        Axis::Col => (inner.h, inner.w),
    };

    let n = d.children.len();
    if n == 0 {
        return;
    }
    let total_gap = d.gap * (n.saturating_sub(1)) as f32;

    // Main-axis size per child: grow children share the remainder; others take their explicit/content size.
    let mut mains = vec![0.0f32; n];
    let mut grows = vec![0.0f32; n];
    let mut used = 0.0f32;
    for (i, c) in d.children.iter().enumerate() {
        let (len, grow) = child_main(c, d.axis);
        grows[i] = grow;
        if grow > 0.0 {
            continue;
        }
        let m = match len {
            Len::Px(v) => v,
            Len::Auto => {
                let (iw, ih) = intrinsic(c);
                if d.axis == Axis::Row {
                    iw
                } else {
                    ih
                }
            }
        };
        mains[i] = m;
        used += m;
    }
    let grow_total: f32 = grows.iter().sum();
    let remaining = (inner_main - used - total_gap).max(0.0);
    if grow_total > 0.0 {
        for i in 0..n {
            if grows[i] > 0.0 {
                mains[i] = remaining * grows[i] / grow_total;
            }
        }
    }

    // Truncating labels give back what the row overflows by, in order, before anything spills.
    let mut overflow = mains.iter().sum::<f32>() + total_gap - inner_main;
    if overflow > 0.0 && d.axis == Axis::Row {
        for (i, c) in d.children.iter().enumerate() {
            if overflow <= 0.0 {
                break;
            }
            if matches!(c, Node::Label(l) if l.truncate) {
                let give = overflow.min(mains[i]);
                mains[i] -= give;
                overflow -= give;
            }
        }
    }
    let content_main: f32 = mains.iter().sum::<f32>() + total_gap;
    let (mut cursor, extra_gap) = match d.justify {
        Justify::Start => (0.0, 0.0),
        Justify::Center => ((inner_main - content_main).max(0.0) / 2.0, 0.0),
        Justify::End => ((inner_main - content_main).max(0.0), 0.0),
        Justify::Between => {
            // Do NOT clamp to 0: on overflow the (negative) slack pulls the last child to the right edge and
            // lets earlier children overlap under it, instead of pushing the last child off the container.
            let slack = inner_main - content_main;
            (0.0, if n > 1 { slack / (n - 1) as f32 } else { 0.0 })
        }
    };

    for (i, c) in d.children.iter().enumerate() {
        let main = mains[i];
        // Cross axis: a div with an explicit cross length uses it (centered when items_center, else start);
        // otherwise divs stretch to fill, and labels keep their content height aligned start/center.
        let (cross, cross_off) = match child_cross_len(c, d.axis) {
            Len::Px(v) => (v, align_off(inner_cross, v, d.items_center)),
            Len::Auto => {
                if is_leaf(c) {
                    let (iw, ih) = intrinsic(c);
                    let ic = if d.axis == Axis::Row { ih } else { iw };
                    (ic, align_off(inner_cross, ic, d.items_center))
                } else {
                    (inner_cross, 0.0) // stretch containers to fill the cross axis
                }
            }
        };

        let child_area = match d.axis {
            Axis::Row => Rect {
                x: inner.x + cursor,
                y: inner.y + cross_off,
                w: main,
                h: cross,
                color: Rgba::TRANSPARENT,
                radius: 0.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            },
            Axis::Col => Rect {
                x: inner.x + cross_off,
                y: inner.y + cursor,
                w: cross,
                h: main,
                color: Rgba::TRANSPARENT,
                radius: 0.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            },
        };
        place(c, child_area, viewport, out, pending);
        cursor += main + d.gap + extra_gap;
    }
}

const TRUNCATION_MARK: &str = "...";

/// The longest prefix of `text` that fits `width` with the mark appended (the mark alone if nothing fits).
fn truncate_to_width(text: &str, width: f32, size: f32, mono: bool, weight: u16) -> String {
    let measure = |candidate: &str| crate::measure_text_width(candidate, size, mono, weight);
    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    // Binary search on char boundaries: text widths only grow as characters are added.
    let (mut low, mut high) = (0usize, boundaries.len());
    while low < high {
        let middle = (low + high).div_ceil(2);
        let end = boundaries.get(middle).copied().unwrap_or(text.len());
        if measure(&format!("{}{TRUNCATION_MARK}", text[..end].trim_end())) <= width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let end = boundaries.get(low).copied().unwrap_or(text.len());
    format!("{}{TRUNCATION_MARK}", text[..end].trim_end())
}

fn truncate_start_to_width(text: &str, width: f32, size: f32, mono: bool, weight: u16) -> String {
    let measure = |candidate: &str| crate::measure_text_width(candidate, size, mono, weight);
    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    // The fewest leading characters to drop so the rest plus the mark fits.
    let (mut low, mut high) = (0usize, boundaries.len());
    while low < high {
        let middle = (low + high) / 2;
        let start = boundaries.get(middle).copied().unwrap_or(text.len());
        if measure(&format!("{TRUNCATION_MARK}{}", text[start..].trim_start())) <= width {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let start = boundaries.get(low).copied().unwrap_or(text.len());
    format!("{TRUNCATION_MARK}{}", text[start..].trim_start())
}

fn align_off(container: f32, item: f32, center: bool) -> f32 {
    if center {
        (container - item).max(0.0) / 2.0
    } else {
        0.0
    }
}

impl Painted {
    /// Topmost click id at (`x`, `y`), if any.
    pub fn hit(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
            .map(|(_, id)| *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncating_label_shrinks_to_fit_its_row() {
        let long = "crm-1079-tech-spike-inbound-call-routing-architecture-handoff";
        let tree: Node = div()
            .row()
            .gap(8.0)
            .child(div().w_px(6.0).h_px(6.0))
            .child(label(long).truncate())
            .into();
        let p = render(&tree, Rect::new(0.0, 0.0, 160.0, 28.0, Rgba::TRANSPARENT));
        let text = &p.texts[0].text;
        assert!(text.ends_with("...") && text.len() < long.len(), "{text}");
        let width = crate::measure_text_width(text, 13.0, false, crate::ui_font_weight());
        assert!(width <= 160.0 - 6.0 - 8.0 + 0.5, "{width}");

        let short: Node = div().row().child(label("main").truncate()).into();
        let p = render(&short, Rect::new(0.0, 0.0, 160.0, 28.0, Rgba::TRANSPARENT));
        assert_eq!(p.texts[0].text, "main");

        let tiny = render(&tree, Rect::new(0.0, 0.0, 16.0, 28.0, Rgba::TRANSPARENT));
        assert_eq!(tiny.texts[0].text, "...");
    }

    #[test]
    fn row_grows_child_into_remaining_space() {
        let tree: Node = div()
            .row()
            .w_px(300.0)
            .h_px(40.0)
            .child(div().w_px(100.0))
            .child(div().flex(1.0).bg(Rgba::new(0.1, 0.2, 0.3, 1.0)))
            .into();
        let p = render(
            &tree,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 300.0,
                h: 40.0,
                color: Rgba::TRANSPARENT,
                radius: 0.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            },
        );
        let flexed = p
            .rects
            .iter()
            .find(|r| r.color == Rgba::new(0.1, 0.2, 0.3, 1.0))
            .unwrap();
        assert_eq!(flexed.x, 100.0);
        assert_eq!(flexed.w, 200.0);
        assert_eq!(flexed.h, 40.0); // stretched to fill cross axis
    }

    #[test]
    fn col_stacks_by_height_not_width() {
        let tree: Node = div()
            .col()
            .w_px(200.0)
            .h_px(100.0)
            .child(div().h_px(30.0).bg(Rgba::new(0.9, 0.9, 0.9, 1.0)))
            .child(div().h_px(1.0).bg(Rgba::new(0.5, 0.5, 0.5, 1.0)))
            .into();
        let p = render(
            &tree,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 200.0,
                h: 100.0,
                color: Rgba::TRANSPARENT,
                radius: 0.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            },
        );
        let first = p
            .rects
            .iter()
            .find(|r| r.color == Rgba::new(0.9, 0.9, 0.9, 1.0))
            .unwrap();
        let line = p
            .rects
            .iter()
            .find(|r| r.color == Rgba::new(0.5, 0.5, 0.5, 1.0))
            .unwrap();
        assert_eq!(first.h, 30.0);
        assert_eq!(first.w, 200.0); // stretched full width
        assert_eq!(line.y, 30.0); // stacked right below, not overlapping a huge box
        assert_eq!(line.h, 1.0);
    }

    #[test]
    fn hit_test_finds_clickable() {
        let tree: Node = div().w_px(200.0).h_px(50.0).on_click(7).into();
        let p = render(
            &tree,
            Rect {
                x: 10.0,
                y: 10.0,
                w: 200.0,
                h: 50.0,
                color: Rgba::TRANSPARENT,
                radius: 0.0,
                border: 0.0,
                border_color: Rgba::TRANSPARENT,
            },
        );
        assert_eq!(p.hit(20.0, 20.0), Some(7));
        assert_eq!(p.hit(0.0, 0.0), None);
    }
}
