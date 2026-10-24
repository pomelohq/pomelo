use stories::{Story, StoryDef};

pub const GAP: f32 = 80.0;
pub const ROW_GAP: f32 = 160.0;
/// Room above a group's boards for its title and note, inside its frame.
pub const GROUP_HEADER: f32 = 88.0;
/// How far a group's dashed frame sits outside its header and boards.
pub const FRAME_PAD: f32 = 32.0;
pub const BOARD_TITLE: f32 = 22.0;
const GROUP_GAP: f32 = 160.0;
/// Groups of small boards share a shelf up to this width, so a page of atoms is not one tall column.
const SHELF: f32 = 2400.0;

pub struct Board {
    pub def: StoryDef,
    pub x: f32,
    pub y: f32,
    pub story: Option<Box<dyn Story>>,
    pub error: Option<String>,
    pub image: Option<Raster>,
    pub stale: bool,
}

pub struct Raster {
    pub id: u64,
    pub scale: f32,
}

pub struct Group {
    pub title: &'static str,
    pub note: Option<&'static str>,
    pub left: f32,
    pub right: f32,
    /// Where the header starts, above the boards.
    pub head: f32,
    pub bottom: f32,
}

impl Board {
    pub fn w(&self) -> f32 {
        self.def.size.0
    }

    pub fn h(&self) -> f32 {
        self.def.size.1
    }

    pub fn story(&mut self) -> Option<&mut Box<dyn Story>> {
        if self.story.is_none() && self.error.is_none() {
            match (self.def.build)() {
                Ok(story) => self.story = Some(story),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        self.story.as_mut()
    }
}

impl Drop for Board {
    fn drop(&mut self) {
        if let Some(raster) = self.image.take() {
            ui::release_image(raster.id);
        }
    }
}

pub fn pages(defs: &[StoryDef]) -> Vec<&'static str> {
    let mut pages: Vec<&'static str> = Vec::new();
    for def in defs {
        if !pages.contains(&def.page) {
            pages.push(def.page);
        }
    }
    pages
}

/// A page's stories by group, each group one row in registry order; narrow groups share a shelf.
pub fn lay_out(defs: &[StoryDef], page: &str) -> (Vec<Board>, Vec<Group>) {
    let mut boards = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut order: Vec<&'static str> = Vec::new();
    for def in defs.iter().filter(|def| def.page == page) {
        if !order.contains(&def.group) {
            order.push(def.group);
        }
    }
    let (mut shelf_x, mut shelf_y, mut shelf_bottom) = (0.0_f32, 0.0_f32, 0.0_f32);
    for group in order {
        let members: Vec<&StoryDef> = defs
            .iter()
            .filter(|def| def.page == page && def.group == group)
            .collect();
        let width = members.iter().map(|def| def.size.0).sum::<f32>()
            + GAP * members.len().saturating_sub(1) as f32;
        let tallest = members.iter().map(|def| def.size.1).fold(0.0, f32::max);
        if shelf_x > 0.0 && shelf_x + width > SHELF {
            shelf_x = 0.0;
            shelf_y = shelf_bottom + ROW_GAP + FRAME_PAD * 2.0;
        }
        let top = shelf_y + GROUP_HEADER + BOARD_TITLE;
        let mut x = shelf_x;
        for def in members {
            boards.push(Board {
                def: *def,
                x,
                y: top,
                story: None,
                error: None,
                image: None,
                stale: true,
            });
            x += def.size.0 + GAP;
        }
        groups.push(Group {
            title: group,
            note: stories::group_note(group),
            left: shelf_x,
            right: shelf_x + width,
            head: shelf_y,
            bottom: top + tallest,
        });
        shelf_bottom = shelf_bottom.max(top + tallest);
        shelf_x += width + GROUP_GAP + FRAME_PAD * 2.0;
    }
    (boards, groups)
}

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 4.0;

impl Default for Camera {
    fn default() -> Self {
        Camera {
            x: 40.0,
            y: 40.0,
            zoom: 0.5,
        }
    }
}

impl Camera {
    pub fn to_screen(self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.zoom, self.y + y * self.zoom)
    }

    pub fn zoom_at(&mut self, factor: f32, sx: f32, sy: f32) {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let (wx, wy) = ((sx - self.x) / self.zoom, (sy - self.y) / self.zoom);
        self.zoom = zoom;
        self.x = sx - wx * zoom;
        self.y = sy - wy * zoom;
    }

    /// Frames a world box inside a `w` by `h` view with a margin, never past 100%.
    pub fn fit(bounds: (f32, f32, f32, f32), w: f32, h: f32) -> Camera {
        let (bx, by, bw, bh) = bounds;
        let margin = 48.0;
        let zoom = ((w - margin * 2.0) / bw.max(1.0))
            .min((h - margin * 2.0) / bh.max(1.0))
            .clamp(MIN_ZOOM, 1.0);
        Camera {
            x: (w - bw * zoom) / 2.0 - bx * zoom,
            y: (h - bh * zoom) / 2.0 - by * zoom,
            zoom,
        }
    }
}

pub fn bounds(boards: &[Board]) -> (f32, f32, f32, f32) {
    let right = boards
        .iter()
        .map(|board| board.x + board.w())
        .fold(0.0, f32::max);
    let bottom = boards
        .iter()
        .map(|board| board.y + board.h())
        .fold(0.0, f32::max);
    (0.0, 0.0, right.max(1.0), bottom.max(1.0))
}

/// Draws boards off screen and hands their pixels to the window renderer as images.
pub struct Rasterizer {
    renderer: Option<ui::UiRenderer>,
    next_id: u64,
}

// Story images live in their own id range so they never collide with images the panels load.
const FIRST_IMAGE: u64 = 0x6a11_0000_0000_0000;

impl Rasterizer {
    pub fn new() -> Self {
        Rasterizer {
            renderer: None,
            next_id: FIRST_IMAGE,
        }
    }

    pub fn draw(&mut self, board: &mut Board, scale: f32) -> anyhow::Result<()> {
        let (w, h) = (board.w(), board.h());
        let (pw, ph) = ((w * scale).ceil() as u32, (h * scale).ceil() as u32);
        if self.renderer.is_none() {
            self.renderer = Some(ui::UiRenderer::new_headless(pw.max(1), ph.max(1), scale)?);
        }
        let Some(story) = board.story() else {
            return Ok(());
        };
        let painted = stories::paint(story.as_mut(), w, h);
        let Some(renderer) = self.renderer.as_mut() else {
            return Ok(());
        };
        renderer.set_scale(scale);
        renderer.resize(pw.max(1), ph.max(1));
        renderer.render_frame(
            ui::theme().panel_background,
            &[(
                painted.rects.as_slice(),
                painted.tris.as_slice(),
                painted.texts.as_slice(),
                painted.icons.as_slice(),
                None,
            )],
        )?;
        let (iw, ih, rgba) = renderer.read_rgba()?;
        let id = self.next_id;
        self.next_id += 1;
        ui::retain_image(id);
        ui::set_image(id, iw, ih, rgba);
        if let Some(old) = board.image.replace(Raster { id, scale }) {
            ui::release_image(old.id);
        }
        board.stale = false;
        Ok(())
    }
}
