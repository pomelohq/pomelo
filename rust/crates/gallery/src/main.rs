//! Every story on an endless canvas, drawn by the app's own code. Pan with the wheel or a space-drag, zoom
//! with a pinch or cmd-wheel, double-click a story to step into it and use it, Escape to step out.

mod chrome;
mod scene;

use std::sync::Arc;
use std::time::{Duration, Instant};

use scene::{Board, Camera, Group, Rasterizer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{CursorIcon, Window, WindowId};
use workspace::EditKey;

// Zooming re-rasterizes only once the camera has been still this long.
const SETTLE: Duration = Duration::from_millis(150);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
// A story is rasterized at no more than this many device pixels per point, however far in you zoom.
const MAX_RASTER: f32 = 4.0;
// The renderer draws no finer than half a device pixel per point.
const MIN_RASTER: f32 = 0.5;
const GRID: f32 = 24.0;
const MIN_WINDOW: (f64, f64) = (640.0, 400.0);
// Rasterizing past this in one frame would drop it; the rest wait for the next.
const RASTER_BUDGET: Duration = Duration::from_millis(10);

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Saved {
    page: String,
    theme: String,
    camera: Option<Camera>,
    focused: Option<String>,
    window: Option<WindowFrame>,
}

/// The window's place and size in points, so the gallery opens where it was left.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct WindowFrame {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn saved_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".local/state/pom/gallery.json"))
}

fn load_saved() -> Saved {
    saved_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

struct Gallery {
    window: Option<Arc<Window>>,
    renderer: Option<ui::UiRenderer>,
    rasterizer: Rasterizer,
    defs: Vec<stories::StoryDef>,
    pages: Vec<&'static str>,
    page: usize,
    boards: Vec<Board>,
    groups: Vec<Group>,
    camera: Camera,
    moved_at: Instant,
    cursor: (f32, f32),
    modifiers: ModifiersState,
    space: bool,
    dragging: Option<(f32, f32)>,
    last_click: Option<(Instant, usize)>,
    focused: Option<usize>,
    focused_hits: Vec<(ui::Rect, u64)>,
    hovered: Option<u64>,
    chrome: bool,
    chrome_hits: Vec<(ui::Rect, u64)>,
    themes: Vec<(&'static str, ui::Appearance)>,
    theme: &'static str,
    pending_focus: Option<String>,
    saved_frame: Option<WindowFrame>,
    /// The camera or window changed since the last save; saved once they settle, so a killed gallery keeps them.
    unsaved: bool,
    collapsed: Vec<&'static str>,
    menu: Option<Menu>,
    story_menu: Option<StoryMenu>,
    chrome_hovered: Option<u64>,
    layers_scroll: f32,
    /// Where the layer list sits, top to bottom, so the wheel over it scrolls it instead of the canvas.
    layers_area: (f32, f32),
    inspect: Inspector,
}

/// The inspect mode: the open board's elements as last painted, the one under the pointer and the one pinned.
#[derive(Default)]
struct Inspector {
    on: bool,
    elements: Vec<ui::inspect::Inspected>,
    origin: (f32, f32),
    hovered: Option<usize>,
    picked: Option<usize>,
}

impl Inspector {
    fn shown(&self) -> Option<usize> {
        self.picked
            .or(self.hovered)
            .filter(|index| *index < self.elements.len())
    }
}

/// A focused story's context menu, opened at the pointer by a right click.
struct StoryMenu {
    x: f32,
    y: f32,
    items: Vec<components::MenuItem>,
    submenu: Option<(u64, Vec<components::MenuItem>)>,
    hovered: Option<u64>,
    hits: Vec<(ui::Rect, u64)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Menu {
    Pages,
    Themes,
}

impl Gallery {
    fn new() -> Self {
        let saved = load_saved();
        let defs = stories::all();
        let pages = scene::pages(&defs);
        let page = pages
            .iter()
            .position(|name| *name == saved.page)
            .unwrap_or(0);
        let themes = ui::builtin_theme_names();
        let theme = themes
            .iter()
            .map(|(name, _)| *name)
            .find(|name| *name == saved.theme)
            .unwrap_or("Pomelo Dark");
        ui::set_theme(ui::by_name(theme));
        let mut gallery = Gallery {
            window: None,
            renderer: None,
            rasterizer: Rasterizer::new(),
            defs,
            pages,
            page,
            boards: Vec::new(),
            groups: Vec::new(),
            camera: saved.camera.unwrap_or_default(),
            moved_at: Instant::now(),
            cursor: (0.0, 0.0),
            modifiers: ModifiersState::empty(),
            space: false,
            dragging: None,
            last_click: None,
            focused: None,
            focused_hits: Vec::new(),
            hovered: None,
            chrome: true,
            chrome_hits: Vec::new(),
            themes,
            theme,
            pending_focus: saved.focused,
            saved_frame: saved.window,
            unsaved: false,
            collapsed: Vec::new(),
            menu: None,
            story_menu: None,
            chrome_hovered: None,
            layers_scroll: 0.0,
            layers_area: (0.0, 0.0),
            inspect: Inspector::default(),
        };
        gallery.open_page(page, saved.camera.is_none());
        gallery
    }

    fn window_frame(&self) -> Option<WindowFrame> {
        let window = self.window.as_ref()?;
        let scale = window.scale_factor();
        let size = window.inner_size().to_logical::<f64>(scale);
        let position = window.outer_position().ok()?.to_logical::<f64>(scale);
        Some(WindowFrame {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        })
    }

    fn save(&mut self) {
        self.unsaved = false;
        if let Some(frame) = self.window_frame() {
            self.saved_frame = Some(frame);
        }
        let saved = Saved {
            page: self
                .pages
                .get(self.page)
                .copied()
                .unwrap_or_default()
                .into(),
            theme: self.theme.into(),
            camera: Some(self.camera),
            focused: self
                .focused
                .and_then(|index| self.boards.get(index))
                .map(|board| board.def.id.into()),
            window: self.saved_frame,
        };
        let Some(path) = saved_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(dir) {
                eprintln!("gallery: {}: {error}", dir.display());
                return;
            }
        }
        match serde_json::to_vec_pretty(&saved) {
            Ok(bytes) => {
                if let Err(error) = std::fs::write(&path, bytes) {
                    eprintln!("gallery: {}: {error}", path.display());
                }
            }
            Err(error) => eprintln!("gallery: {error}"),
        }
    }

    fn open_page(&mut self, page: usize, fit: bool) {
        self.page = page.min(self.pages.len().saturating_sub(1));
        self.focused = None;
        let Some(name) = self.pages.get(self.page) else {
            return;
        };
        let (boards, groups) = scene::lay_out(&self.defs, name);
        self.boards = boards;
        self.groups = groups;
        if fit {
            self.fit();
        }
    }

    fn view_size(&self) -> (f32, f32) {
        self.renderer
            .as_ref()
            .map(|r| r.size())
            .unwrap_or((1440.0, 900.0))
    }

    fn canvas(&self) -> ui::Rect {
        let (w, h) = self.view_size();
        let (left, right) = if self.chrome {
            (chrome::LEFT, chrome::RIGHT)
        } else {
            (0.0, 0.0)
        };
        ui::Rect::new(
            left,
            0.0,
            (w - left - right).max(1.0),
            h,
            ui::Rgba::TRANSPARENT,
        )
    }

    fn fit(&mut self) {
        let canvas = self.canvas();
        self.camera = Camera::fit(scene::bounds(&self.boards), canvas.w, canvas.h);
        self.moved();
    }

    fn moved(&mut self) {
        self.moved_at = Instant::now();
        self.unsaved = true;
    }

    fn settled(&self) -> bool {
        self.moved_at.elapsed() >= SETTLE
    }

    fn scale(&self) -> f32 {
        self.window
            .as_ref()
            .map(|w| w.scale_factor() as f32)
            .unwrap_or(2.0)
    }

    fn redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The board under a point of the canvas (canvas-local coordinates).
    fn board_at(&self, x: f32, y: f32) -> Option<usize> {
        self.boards.iter().position(|board| {
            let (sx, sy) = self.camera.to_screen(board.x, board.y);
            let (w, h) = (board.w() * self.camera.zoom, board.h() * self.camera.zoom);
            x >= sx && x < sx + w && y >= sy && y < sy + h
        })
    }

    fn focus(&mut self, index: usize) {
        let canvas = self.canvas();
        let Some(board) = self.boards.get(index) else {
            return;
        };
        let x = ((canvas.w - board.w()) / 2.0).max(32.0) - board.x;
        let y = ((canvas.h - board.h()) / 2.0).max(32.0) - board.y;
        self.camera = Camera { x, y, zoom: 1.0 };
        self.focused = Some(index);
        self.moved();
    }

    fn unfocus(&mut self) {
        if let Some(board) = self
            .focused
            .take()
            .and_then(|index| self.boards.get_mut(index))
        {
            board.stale = true;
        }
        self.focused_hits.clear();
        self.hovered = None;
        self.inspect.elements.clear();
        self.inspect.hovered = None;
        self.inspect.picked = None;
    }

    fn toggle_inspect(&mut self) {
        self.inspect = Inspector {
            on: !self.inspect.on,
            ..Inspector::default()
        };
        if self.inspect.on {
            self.hovered = None;
        }
        self.hover();
        self.redraw();
    }

    fn set_theme(&mut self, name: &'static str) {
        self.theme = name;
        ui::set_theme(ui::by_name(name));
        for board in &mut self.boards {
            board.stale = true;
        }
        self.save();
    }

    fn raster_scale(&self) -> f32 {
        (self.scale() * self.camera.zoom).clamp(MIN_RASTER, MAX_RASTER)
    }

    /// Rasters go one power of two past what the zoom needs, so zooming in a little stays sharp without a redraw.
    fn raster_target(&self) -> f32 {
        self.raster_scale()
            .log2()
            .ceil()
            .exp2()
            .clamp(MIN_RASTER, MAX_RASTER)
    }

    /// Re-rasterizes a few visible boards a frame, nearest the canvas center first; true while more wait.
    fn rasterize(&mut self) -> bool {
        let canvas = self.canvas();
        let needed = self.raster_scale();
        let wanted = self.raster_target();
        let settled = self.settled();
        let mut filling = self
            .focused
            .and_then(|index| self.boards.get(index))
            .and_then(|board| board.story.as_ref())
            .is_some_and(|story| story.animating());
        let mut waiting: Vec<(f32, usize)> = self
            .boards
            .iter_mut()
            .enumerate()
            .filter(|(index, _)| Some(*index) != self.focused)
            .filter_map(|(index, board)| {
                let (sx, sy) = self.camera.to_screen(board.x, board.y);
                let (w, h) = (board.w() * self.camera.zoom, board.h() * self.camera.zoom);
                let visible = sx + w >= 0.0 && sy + h >= 0.0 && sx <= canvas.w && sy <= canvas.h;
                if !visible || board.error.is_some() {
                    return None;
                }
                if board.story.as_ref().is_some_and(|story| story.animating()) {
                    filling = true;
                    board.stale = true;
                }
                if board.story.as_mut().is_some_and(|story| story.pending()) {
                    filling = true;
                    board.stale = true;
                    if board.image.is_some() {
                        return None;
                    }
                }
                let off_scale = board.image.as_ref().is_none_or(|raster| {
                    raster.scale < needed * 0.95 || raster.scale > wanted * 2.0
                });
                (board.stale || board.image.is_none() || (settled && off_scale)).then(|| {
                    let dx = sx + w / 2.0 - canvas.w / 2.0;
                    let dy = sy + h / 2.0 - canvas.h / 2.0;
                    (dx * dx + dy * dy, index)
                })
            })
            .collect();
        waiting.sort_by(|a, b| a.0.total_cmp(&b.0));
        let started = Instant::now();
        let mut drawn = 0;
        for (_, index) in &waiting {
            if drawn > 0 && started.elapsed() >= RASTER_BUDGET {
                break;
            }
            if let Some(board) = self.boards.get_mut(*index) {
                if let Err(error) = self.rasterizer.draw(board, wanted) {
                    board.error = Some(error.to_string());
                }
            }
            drawn += 1;
        }
        filling || waiting.len() > drawn
    }

    fn draw(&mut self) {
        let more = self.rasterize();
        let Some((w, h)) = self.renderer.as_ref().map(|r| r.size()) else {
            return;
        };
        let canvas = self.canvas();
        let theme = ui::theme();
        let zoom = self.camera.zoom;

        let mut base = ui::Painted::default();
        base.rects.push(ui::Rect::new(
            canvas.x,
            0.0,
            canvas.w,
            canvas.h,
            canvas_color(&theme),
        ));
        let mut step = GRID * zoom;
        while step < 12.0 {
            step *= 2.0;
        }
        let dot = ui::Rgba::new(
            theme.text_muted.r,
            theme.text_muted.g,
            theme.text_muted.b,
            0.22,
        );
        let mut gy = self.camera.y.rem_euclid(step);
        while gy < canvas.h {
            let mut gx = self.camera.x.rem_euclid(step);
            while gx < canvas.w {
                base.rects
                    .push(ui::Rect::new(canvas.x + gx - 1.0, gy - 1.0, 2.0, 2.0, dot));
                gx += step;
            }
            gy += step;
        }
        // Titles keep their screen size, so at low zoom the note goes first and then the board titles.
        let frame_color = ui::Rgba::new(
            theme.text_muted.r,
            theme.text_muted.g,
            theme.text_muted.b,
            0.35,
        );
        let mut board_title_floor = Vec::with_capacity(self.groups.len());
        for group in &self.groups {
            if let Some(head) = group.section_head {
                let (sx, sy) = self.camera.to_screen(group.left, head - scene::FRAME_PAD);
                base.append(ui::render(
                    &chrome::section_title(group.section),
                    ui::Rect::new(canvas.x + sx, sy, 600.0, 36.0, ui::Rgba::TRANSPARENT),
                ));
            }
            let (x0, y0) = self
                .camera
                .to_screen(group.left - scene::FRAME_PAD, group.head - scene::FRAME_PAD);
            let (x1, y1) = self.camera.to_screen(
                group.right + scene::FRAME_PAD,
                group.bottom + scene::FRAME_PAD,
            );
            dashed_frame(&mut base, canvas.x + x0, y0, x1 - x0, y1 - y0, frame_color);
            let (sx, sy) = self.camera.to_screen(group.left, group.head);
            let width = ((group.right - group.left) * zoom).clamp(220.0, 560.0);
            let mut bottom = sy + 24.0;
            base.append(ui::render(
                &chrome::group_title(group.title),
                ui::Rect::new(canvas.x + sx, sy, width, 24.0, ui::Rgba::TRANSPARENT),
            ));
            if let Some(note) = group.note.filter(|_| scene::GROUP_HEADER * zoom >= 64.0) {
                base.append(ui::render(
                    &chrome::group_note(note, width),
                    ui::Rect::new(canvas.x + sx, sy + 26.0, width, 36.0, ui::Rgba::TRANSPARENT),
                ));
                bottom = sy + 64.0;
            }
            board_title_floor.push((group.title, bottom));
        }
        for (index, board) in self.boards.iter().enumerate() {
            let (sx, sy) = self.camera.to_screen(board.x, board.y);
            let (bx, by, bw, bh) = (canvas.x + sx, sy, board.w() * zoom, board.h() * zoom);
            if bx > canvas.x + canvas.w || by > canvas.h || bx + bw < canvas.x || by + bh < -40.0 {
                continue;
            }
            let floor = board_title_floor
                .iter()
                .find(|(title, _)| *title == board.def.group)
                .map_or(f32::MIN, |(_, floor)| *floor);
            if bw >= 48.0 && by - 20.0 >= floor {
                let note = board.error.as_deref();
                base.append(ui::render(
                    &chrome::board_title(board.def.title, note, bw - 8.0),
                    ui::Rect::new(bx, by - 20.0, bw, 16.0, ui::Rgba::TRANSPARENT),
                ));
            }
            // Rects have no blur, so the shadow is a few thin square steps that fade out under the frame.
            for (spread, alpha) in [(3.0, 0.04), (2.0, 0.06), (1.0, 0.1)] {
                base.rects.push(ui::Rect::new(
                    bx - spread,
                    by - spread + 1.0,
                    bw + spread * 2.0,
                    bh + spread * 2.0,
                    ui::Rgba::new(0.0, 0.0, 0.0, alpha),
                ));
            }
            let mut frame = ui::Rect::new(
                bx - 1.0,
                by - 1.0,
                bw + 2.0,
                bh + 2.0,
                theme.panel_background,
            );
            frame.border = if self.focused == Some(index) {
                2.0
            } else {
                1.0
            };
            frame.border_color = if self.focused == Some(index) {
                theme.border_focused
            } else {
                theme.border
            };
            base.rects.push(frame);
            if self.focused == Some(index) {
                continue;
            }
            if let Some(raster) = &board.image {
                base.icons.push(ui::IconQuad {
                    x: bx,
                    y: by,
                    w: bw,
                    h: bh,
                    kind: ui::IconKind::Close,
                    color: ui::Rgba::TRANSPARENT,
                    material: None,
                    image: Some(raster.id),
                    stretch: true,
                });
            }
        }

        let mut live = ui::Painted::default();
        let mut live_clip = None;
        let mut live_overlays: Vec<ui::Overlay> = Vec::new();
        if let Some(index) = self.focused {
            let canvas_x = canvas.x;
            let camera = self.camera;
            let hovered = self.hovered;
            let inspecting = self.inspect.on;
            let mut inspected = Vec::new();
            if let Some(board) = self.boards.get_mut(index) {
                let (sx, sy) = camera.to_screen(board.x, board.y);
                let area = ui::Rect::new(
                    canvas_x + sx,
                    sy,
                    board.w(),
                    board.h(),
                    ui::Rgba::TRANSPARENT,
                );
                if let Some(story) = board.story() {
                    let node = ui::div()
                        .bg(ui::theme().panel_background)
                        .child(story.render(area.w, area.h))
                        .into();
                    let paint = || {
                        ui::paint_frame_interactive(
                            &node,
                            area,
                            ui::Interaction {
                                hovered,
                                pressed: None,
                            },
                        )
                    };
                    let frame = if inspecting {
                        let (frame, elements) = ui::inspect::inspect(paint);
                        inspected = elements;
                        frame
                    } else {
                        paint()
                    };
                    live = frame.base;
                    live.hits = frame.hits;
                    live_overlays = frame.overlays;
                    live_overlays.extend(story.overlays(area));
                }
                let x0 = area.x.max(canvas.x);
                let y0 = area.y.max(0.0);
                let x1 = (area.x + area.w).min(canvas.x + canvas.w);
                let y1 = (area.y + area.h).min(canvas.h);
                live_clip = Some((x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)));
                let visible = ui::Rect::new(
                    x0,
                    y0,
                    (x1 - x0).max(0.0),
                    (y1 - y0).max(0.0),
                    ui::Rgba::TRANSPARENT,
                );
                for overlay in &mut live_overlays {
                    overlay.clip = Some(match overlay.clip {
                        Some(clip) => intersect(clip, visible),
                        None => visible,
                    });
                }
                self.inspect.origin = (area.x, area.y);
            }
            if inspecting {
                self.inspect.elements = inspected;
            }
        }
        self.focused_hits = live.hits.clone();
        for layer in &live_overlays {
            self.focused_hits.extend(layer.painted.hits.iter().copied());
        }

        let mut side = ui::Painted::default();
        let mut list = ui::Painted::default();
        let mut list_clip = None;
        let mut overlay = ui::Painted::default();
        if self.chrome {
            let (top, layers) = chrome::left(
                &self.pages,
                self.page,
                &self.groups,
                &self.boards,
                self.focused,
                &self.collapsed,
            );
            let interaction = ui::Interaction {
                hovered: self.chrome_hovered,
                pressed: None,
            };
            side.rects.push(ui::Rect::new(
                0.0,
                0.0,
                chrome::LEFT,
                h,
                theme.panel_background,
            ));
            let top_h = ui::measure(&top).1.ceil();
            side.append(ui::render_interactive(
                &top,
                ui::Rect::new(0.0, 0.0, chrome::LEFT, top_h, ui::Rgba::TRANSPARENT),
                interaction,
            ));
            let layers_h = ui::measure(&layers).1.ceil();
            let visible = (h - top_h).max(0.0);
            self.layers_scroll = self.layers_scroll.clamp(0.0, (layers_h - visible).max(0.0));
            list = ui::render_interactive(
                &layers,
                ui::Rect::new(
                    0.0,
                    top_h - self.layers_scroll,
                    chrome::LEFT,
                    layers_h,
                    ui::Rgba::TRANSPARENT,
                ),
                interaction,
            );
            list.hits.retain_mut(|(rect, _)| {
                let y0 = rect.y.max(top_h);
                let y1 = (rect.y + rect.h).min(h);
                rect.y = y0;
                rect.h = y1 - y0;
                rect.h > 0.0
            });
            list_clip = Some((0.0, top_h, chrome::LEFT, visible));
            self.layers_area = (top_h, h);
            let inspector = self.inspect.on.then(|| {
                let shown = self.inspect.shown();
                let element = shown.and_then(|index| self.inspect.elements.get(index));
                chrome::inspector(
                    element,
                    self.inspect.origin,
                    element.is_some_and(|element| element.parent.is_some()),
                    shown.is_some_and(|index| {
                        !ui::inspect::children(&self.inspect.elements, index).is_empty()
                    }),
                )
            });
            let right = chrome::right(h, &self.themes, self.theme, zoom, inspector);
            side.append(ui::render_interactive(
                &right,
                ui::Rect::new(
                    w - chrome::RIGHT,
                    0.0,
                    chrome::RIGHT,
                    h,
                    ui::Rgba::TRANSPARENT,
                ),
                interaction,
            ));
            side.rects
                .push(ui::Rect::new(chrome::LEFT - 1.0, 0.0, 1.0, h, theme.border));
            side.rects
                .push(ui::Rect::new(w - chrome::RIGHT, 0.0, 1.0, h, theme.border));
        } else {
            let mini = chrome::mini(self.pages.get(self.page).copied().unwrap_or_default());
            let (mini_w, mini_h) = ui::measure(&mini);
            let (mini_w, mini_h) = (mini_w.ceil(), mini_h.ceil());
            let interaction = ui::Interaction {
                hovered: self.chrome_hovered,
                pressed: None,
            };
            side.append(ui::render_interactive(
                &mini,
                ui::Rect::new(12.0, 12.0, mini_w, mini_h, ui::Rgba::TRANSPARENT),
                interaction,
            ));
            let pill = chrome::theme_pill(self.theme);
            let (pill_w, pill_h) = ui::measure(&pill);
            let pill_x = 12.0 + mini_w + 8.0;
            side.append(ui::render_interactive(
                &pill,
                ui::Rect::new(
                    pill_x,
                    12.0,
                    pill_w.ceil(),
                    pill_h.ceil(),
                    ui::Rgba::TRANSPARENT,
                ),
                interaction,
            ));
            let menu = match self.menu {
                Some(Menu::Pages) => Some((
                    chrome::page_menu(&self.pages, self.page, self.chrome_hovered),
                    chrome::PAGES,
                )),
                Some(Menu::Themes) => Some((
                    chrome::theme_menu(&self.themes, self.theme, self.chrome_hovered),
                    chrome::THEMES,
                )),
                None => None,
            };
            let anchor = |id: u64| {
                side.hits
                    .iter()
                    .find(|(_, hit)| *hit == id)
                    .map_or(12.0, |(rect, _)| rect.x - 6.0)
            };
            if let Some((menu, x)) = menu.map(|(menu, id)| (menu, anchor(id))) {
                let (menu_w, menu_h) = ui::measure(&menu);
                overlay.append(ui::render_interactive(
                    &menu,
                    ui::Rect::new(
                        x,
                        12.0 + mini_h + 6.0,
                        menu_w.ceil(),
                        menu_h.ceil(),
                        ui::Rgba::TRANSPARENT,
                    ),
                    interaction,
                ));
            }
        }
        if self.chrome_hovered == Some(chrome::HIDE) {
            if let Some((button, _)) = side.hits.iter().find(|(_, id)| *id == chrome::HIDE) {
                let tip = chrome::shortcut_tip(if self.chrome {
                    "Hide panels"
                } else {
                    "Show panels"
                });
                let (tip_w, tip_h) = ui::measure(&tip);
                let x = (button.x + button.w / 2.0 - tip_w / 2.0).clamp(8.0, w - tip_w - 8.0);
                let tip = ui::render(
                    &tip,
                    ui::Rect::new(
                        x,
                        button.y + button.h + 8.0,
                        tip_w.ceil(),
                        tip_h.ceil(),
                        ui::Rgba::TRANSPARENT,
                    ),
                );
                overlay.append(tip);
            }
        }
        if let Some(menu) = self.story_menu.as_mut() {
            let (card, menu_w, menu_h) = workspace::context_menu_card(&menu.items, menu.hovered);
            let x = menu.x.min(w - menu_w - 6.0).max(6.0);
            let y = menu.y.min(h - menu_h - 6.0).max(6.0);
            let main = ui::render_interactive(
                &card,
                ui::Rect::new(x, y, menu_w, menu_h, ui::Rgba::TRANSPARENT),
                ui::Interaction {
                    hovered: menu.hovered,
                    pressed: None,
                },
            );
            menu.hits = main.hits.clone();
            let anchor = menu.submenu.as_ref().and_then(|(id, _)| {
                main.hits
                    .iter()
                    .find(|(_, hit)| hit == id)
                    .map(|(rect, _)| *rect)
            });
            overlay.append(main);
            if let (Some((_, items)), Some(row)) = (&menu.submenu, anchor) {
                let (card, sub_w, sub_h) = workspace::context_menu_card(items, menu.hovered);
                let sub_x = if x + menu_w + sub_w + 8.0 <= w {
                    x + menu_w + 2.0
                } else {
                    (x - sub_w - 2.0).max(6.0)
                };
                let sub_y = (row.y - 4.0).min(h - sub_h - 6.0).max(6.0);
                let sub = ui::render_interactive(
                    &card,
                    ui::Rect::new(sub_x, sub_y, sub_w, sub_h, ui::Rgba::TRANSPARENT),
                    ui::Interaction {
                        hovered: menu.hovered,
                        pressed: None,
                    },
                );
                menu.hits.extend(sub.hits.iter().copied());
                overlay.append(sub);
            }
        }
        let mut highlight = ui::Painted::default();
        if self.inspect.on {
            highlight_element(&mut highlight, &self.inspect, &theme);
        }
        self.chrome_hits = side.hits.clone();
        self.chrome_hits.extend(list.hits.iter().copied());
        self.chrome_hits.extend(overlay.hits.iter().copied());

        let canvas_clip = Some((canvas.x, 0.0, canvas.w, canvas.h));
        let mut layers: Vec<ui::Layer> = vec![
            (
                base.rects.as_slice(),
                base.tris.as_slice(),
                base.texts.as_slice(),
                base.icons.as_slice(),
                canvas_clip,
            ),
            (
                live.rects.as_slice(),
                live.tris.as_slice(),
                live.texts.as_slice(),
                live.icons.as_slice(),
                live_clip,
            ),
            (
                side.rects.as_slice(),
                side.tris.as_slice(),
                side.texts.as_slice(),
                side.icons.as_slice(),
                None,
            ),
            (
                list.rects.as_slice(),
                list.tris.as_slice(),
                list.texts.as_slice(),
                list.icons.as_slice(),
                list_clip,
            ),
        ];
        layers.extend(stories::layers(&live_overlays));
        layers.push((
            highlight.rects.as_slice(),
            highlight.tris.as_slice(),
            highlight.texts.as_slice(),
            highlight.icons.as_slice(),
            live_clip,
        ));
        layers.push((
            overlay.rects.as_slice(),
            overlay.tris.as_slice(),
            overlay.texts.as_slice(),
            overlay.icons.as_slice(),
            None,
        ));
        if let Some(renderer) = self.renderer.as_mut() {
            if let Err(error) = renderer.render_frame(theme.background, &layers) {
                eprintln!("gallery: draw: {error}");
            }
        }
        if more || !self.settled() {
            self.redraw();
        } else if self.unsaved {
            self.save();
        }
    }

    fn click(&mut self) {
        let (x, y) = self.cursor;
        if let Some(menu) = self.story_menu.as_ref() {
            let hit = ui::topmost_hit(&menu.hits, x, y);
            if let Some(id) = hit.filter(|id| workspace::is_submenu(*id)) {
                self.open_submenu(id);
                return;
            }
            self.story_menu = None;
            if let (Some(id), Some(story)) = (
                hit,
                self.focused
                    .and_then(|index| self.boards.get_mut(index))
                    .and_then(Board::story),
            ) {
                story.menu_action(id);
            }
            self.redraw();
            return;
        }
        if let Some(id) = ui::topmost_hit(&self.chrome_hits, x, y) {
            self.chrome_click(id);
            return;
        }
        if self.menu.take().is_some() {
            self.redraw();
            return;
        }
        let canvas = self.canvas();
        if self.inspect.on && self.focused.is_some() {
            if let Some(at) = ui::inspect::element_at(&self.inspect.elements, x, y) {
                self.inspect.picked = Some(at);
                self.redraw();
                return;
            }
        }
        if let Some(index) = self.focused {
            if let Some(id) = ui::topmost_hit(&self.focused_hits, x, y) {
                if let Some(story) = self.boards.get_mut(index).and_then(Board::story) {
                    story.click(id);
                }
                return;
            }
            if self.board_at(x - canvas.x, y) == Some(index) {
                return;
            }
            self.unfocus();
        }
        let Some(index) = self.board_at(x - canvas.x, y) else {
            self.last_click = None;
            return;
        };
        let now = Instant::now();
        let double = self
            .last_click
            .is_some_and(|(at, last)| last == index && now.duration_since(at) < DOUBLE_CLICK);
        self.last_click = Some((now, index));
        if double {
            self.focus(index);
        }
    }

    fn right_click(&mut self) {
        let (x, y) = self.cursor;
        self.story_menu = None;
        let canvas = self.canvas();
        let Some(index) = self.focused else {
            // A board takes clicks only once focused, so a right click on one focuses it first.
            if let Some(index) = self.board_at(x - canvas.x, y) {
                self.focus(index);
            }
            return;
        };
        let Some(id) = ui::topmost_hit(&self.focused_hits, x, y) else {
            return;
        };
        let Some(story) = self.boards.get_mut(index).and_then(Board::story) else {
            return;
        };
        let items = story.context_menu(id);
        if !items.is_empty() {
            self.story_menu = Some(StoryMenu {
                x,
                y,
                items,
                submenu: None,
                hovered: None,
                hits: Vec::new(),
            });
        }
    }

    fn open_submenu(&mut self, id: u64) {
        let items = self
            .focused
            .and_then(|index| self.boards.get_mut(index))
            .and_then(Board::story)
            .map(|story| story.submenu(id))
            .unwrap_or_default();
        if let Some(menu) = self.story_menu.as_mut() {
            if menu.submenu.as_ref().map(|(open, _)| *open) != Some(id) {
                menu.submenu = (!items.is_empty()).then_some((id, items));
            }
        }
    }

    fn chrome_click(&mut self, id: u64) {
        match chrome::decode(id) {
            Some(chrome::Click::Page(page)) => {
                self.menu = None;
                self.open_page(page, true);
                self.save();
            }
            Some(chrome::Click::Board(index)) => self.focus(index),
            Some(chrome::Click::Theme(index)) => {
                if let Some((name, _)) = self.themes.get(index).copied() {
                    self.set_theme(name);
                }
            }
            Some(chrome::Click::Fit) => {
                self.unfocus();
                self.fit();
            }
            Some(chrome::Click::Actual) => {
                let canvas = self.canvas();
                self.camera
                    .zoom_at(1.0 / self.camera.zoom, canvas.w / 2.0, canvas.h / 2.0);
                self.moved();
            }
            Some(chrome::Click::ZoomIn) => {
                let canvas = self.canvas();
                self.zoom(1.25, canvas.w / 2.0, canvas.h / 2.0);
            }
            Some(chrome::Click::ZoomOut) => {
                let canvas = self.canvas();
                self.zoom(0.8, canvas.w / 2.0, canvas.h / 2.0);
            }
            Some(chrome::Click::Hide) => self.toggle_chrome(),
            Some(chrome::Click::Inspect) => self.toggle_inspect(),
            Some(chrome::Click::Themes) => self.toggle_menu(Menu::Themes),
            Some(chrome::Click::Pages) => self.toggle_menu(Menu::Pages),
            Some(chrome::Click::Group(index)) => {
                if let Some(title) = self.groups.get(index).map(|group| group.title) {
                    if let Some(at) = self.collapsed.iter().position(|name| *name == title) {
                        self.collapsed.remove(at);
                    } else {
                        self.collapsed.push(title);
                    }
                }
            }
            None => {}
        }
    }

    fn hover(&mut self) {
        let (x, y) = self.cursor;
        if let Some(menu) = self.story_menu.as_mut() {
            let hovered = ui::topmost_hit(&menu.hits, x, y);
            if hovered != menu.hovered {
                menu.hovered = hovered;
                if let Some(id) = hovered.filter(|id| workspace::is_submenu(*id)) {
                    self.open_submenu(id);
                }
                self.redraw();
            }
            if let Some(window) = &self.window {
                window.set_cursor(CursorIcon::Default);
            }
            return;
        }
        let chrome_hovered = ui::topmost_hit(&self.chrome_hits, x, y);
        let mut pointer = chrome_hovered.is_some();
        if chrome_hovered != self.chrome_hovered {
            self.chrome_hovered = chrome_hovered;
            self.redraw();
        }
        if self.inspect.on {
            let at = self
                .focused
                .and(ui::inspect::element_at(&self.inspect.elements, x, y));
            if at != self.inspect.hovered {
                self.inspect.hovered = at;
                self.redraw();
            }
        } else if let Some(index) = self.focused {
            let id = ui::topmost_hit(&self.focused_hits, x, y);
            pointer |= id.is_some();
            if id != self.hovered {
                self.hovered = id;
                if let Some(story) = self.boards.get_mut(index).and_then(Board::story) {
                    story.hover(id);
                }
                self.redraw();
            }
        }
        if let Some(window) = &self.window {
            window.set_cursor(if self.space || self.dragging.is_some() {
                CursorIcon::Grab
            } else if self.inspect.hovered.is_some() && chrome_hovered.is_none() {
                CursorIcon::Crosshair
            } else if pointer {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
        }
    }

    fn wheel(&mut self, dx: f32, dy: f32) {
        let canvas = self.canvas();
        let (x, y) = self.cursor;
        if self.chrome && x < chrome::LEFT {
            if (self.layers_area.0..self.layers_area.1).contains(&y) {
                self.layers_scroll -= dy;
                self.redraw();
            }
            return;
        }
        if self.modifiers.super_key() || self.modifiers.control_key() {
            self.zoom(1.0 + dy * 0.01, x - canvas.x, y);
            return;
        }
        if let Some(index) = self.focused {
            if self.board_at(x - canvas.x, y) == Some(index) {
                if let Some(story) = self.boards.get_mut(index).and_then(Board::story) {
                    story.scroll(dy);
                }
                self.redraw();
                return;
            }
        }
        self.camera.x += dx;
        self.camera.y += dy;
        self.moved();
        self.redraw();
    }

    fn zoom(&mut self, factor: f32, x: f32, y: f32) {
        if self.focused.is_some() {
            self.unfocus();
        }
        self.camera.zoom_at(factor, x, y);
        self.moved();
        self.redraw();
    }

    fn toggle_chrome(&mut self) {
        self.chrome = !self.chrome;
        self.menu = None;
    }

    /// Arrows walk the open menu in the order it lists its rows, applying each one as they land on it.
    fn step_menu(&mut self, by: isize) {
        let step = |at: usize, len: usize| (at as isize + by).clamp(0, len as isize - 1) as usize;
        match self.menu {
            Some(Menu::Pages) if !self.pages.is_empty() => {
                let page = step(self.page, self.pages.len());
                if page != self.page {
                    self.open_page(page, true);
                    self.save();
                }
            }
            Some(Menu::Themes) => {
                let order: Vec<&'static str> = [ui::Appearance::Dark, ui::Appearance::Light]
                    .iter()
                    .flat_map(|appearance| {
                        self.themes
                            .iter()
                            .filter(move |(_, kind)| kind == appearance)
                            .map(|(name, _)| *name)
                    })
                    .collect();
                let next = order
                    .iter()
                    .position(|name| *name == self.theme)
                    .and_then(|at| order.get(step(at, order.len())).copied());
                if let Some(next) = next.filter(|next| *next != self.theme) {
                    self.set_theme(next);
                }
            }
            _ => {}
        }
    }

    fn toggle_menu(&mut self, menu: Menu) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
    }

    fn key(&mut self, event: &winit::event::KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        if let Key::Named(NamedKey::Space) = event.logical_key {
            if self.focused.is_none() {
                self.space = pressed;
                self.hover();
                return;
            }
        }
        if !pressed {
            return;
        }
        if event.logical_key == Key::Named(NamedKey::Escape) && self.story_menu.take().is_some() {
            return;
        }
        let command = self.modifiers.super_key() || self.modifiers.control_key();
        match &event.logical_key {
            _ if command
                && self.modifiers.shift_key()
                && event.physical_key == PhysicalKey::Code(KeyCode::Backslash) =>
            {
                self.toggle_chrome();
                self.redraw();
                return;
            }
            Key::Character(c) if command && c.eq_ignore_ascii_case("i") => {
                self.toggle_inspect();
                return;
            }
            Key::Named(NamedKey::Escape) if self.inspect.on => {
                if self.inspect.picked.take().is_none() {
                    self.toggle_inspect();
                }
                self.redraw();
                return;
            }
            Key::Named(step @ (NamedKey::ArrowUp | NamedKey::ArrowDown))
                if self.inspect.shown().is_some() && self.menu.is_none() =>
            {
                let elements = &self.inspect.elements;
                let shown = self.inspect.shown();
                let next = shown.and_then(|index| match step {
                    NamedKey::ArrowUp => elements.get(index).and_then(|element| element.parent),
                    _ => ui::inspect::children(elements, index).first().copied(),
                });
                if next.is_some() {
                    self.inspect.picked = next;
                    self.redraw();
                }
                return;
            }
            Key::Character(c) if command && c.as_str() == "0" => {
                let canvas = self.canvas();
                self.zoom(1.0 / self.camera.zoom, canvas.w / 2.0, canvas.h / 2.0);
                return;
            }
            Key::Character(c) if self.modifiers.shift_key() && c.as_str() == "!" => {
                self.unfocus();
                self.fit();
                return;
            }
            Key::Named(step @ (NamedKey::ArrowUp | NamedKey::ArrowDown)) if self.menu.is_some() => {
                self.step_menu(if *step == NamedKey::ArrowUp { -1 } else { 1 });
                self.redraw();
                return;
            }
            Key::Named(NamedKey::Enter | NamedKey::Escape) if self.menu.is_some() => {
                self.menu = None;
                self.redraw();
                return;
            }
            Key::Named(NamedKey::Escape) if self.focused.is_some() => {
                self.unfocus();
                return;
            }
            _ => {}
        }
        let Some(index) = self.focused else {
            return;
        };
        let shift = self.modifiers.shift_key();
        let Some(story) = self.boards.get_mut(index).and_then(Board::story) else {
            return;
        };
        let edit = match &event.logical_key {
            Key::Named(NamedKey::ArrowLeft) => Some(EditKey::Left),
            Key::Named(NamedKey::ArrowRight) => Some(EditKey::Right),
            Key::Named(NamedKey::ArrowUp) => Some(EditKey::Up),
            Key::Named(NamedKey::ArrowDown) => Some(EditKey::Down),
            Key::Named(NamedKey::Home) => Some(EditKey::Home),
            Key::Named(NamedKey::End) => Some(EditKey::End),
            Key::Named(NamedKey::Backspace) => Some(EditKey::Backspace),
            Key::Named(NamedKey::Delete) => Some(EditKey::Delete),
            _ => None,
        };
        match edit {
            Some(key) => {
                story.key(key, shift);
            }
            None if !command => {
                if let Some(text) = event.text.as_ref() {
                    story.text(text);
                }
            }
            None => {}
        }
    }
}

/// The canvas sits a step darker than the app's own surfaces (lighter on a light theme), so every story stands out.
fn canvas_color(theme: &ui::ThemeColors) -> ui::Rgba {
    let base = theme.background;
    let toward = if matches!(theme.appearance, ui::Appearance::Light) {
        0.07
    } else {
        0.38
    };
    ui::Rgba::new(
        base.r * (1.0 - toward),
        base.g * (1.0 - toward),
        base.b * (1.0 - toward),
        1.0,
    )
}

impl ApplicationHandler for Gallery {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        #[cfg(target_os = "macos")]
        set_dock_icon();
        let mut attributes = Window::default_attributes().with_title("Pomelo Gallery");
        attributes = match self.saved_frame {
            Some(frame) => attributes
                .with_inner_size(LogicalSize::new(
                    frame.width.max(MIN_WINDOW.0),
                    frame.height.max(MIN_WINDOW.1),
                ))
                .with_position(LogicalPosition::new(frame.x, frame.y)),
            None => attributes.with_inner_size(LogicalSize::new(1440.0, 900.0)),
        };
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("gallery window"),
        );
        self.renderer = Some(ui::UiRenderer::new(window.clone()).expect("gallery renderer"));
        self.window = Some(window);
        if let Some(id) = self.pending_focus.take() {
            if let Some(index) = self.boards.iter().position(|board| board.def.id == id) {
                self.focus(index);
            }
        }
        self.redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        self.redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let scale = self.scale();
        match event {
            WindowEvent::Moved(_) => {
                self.moved();
                self.redraw();
            }
            WindowEvent::CloseRequested => {
                self.save();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                self.moved();
                self.redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let (Some(renderer), Some(window)) =
                    (self.renderer.as_mut(), self.window.as_ref())
                {
                    renderer.set_scale(scale_factor as f32);
                    let size = window.inner_size();
                    renderer.resize(size.width, size.height);
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x as f32 / scale, position.y as f32 / scale);
                if let Some((last_x, last_y)) = self.dragging {
                    self.camera.x += x - last_x;
                    self.camera.y += y - last_y;
                    self.dragging = Some((x, y));
                    self.moved();
                    self.redraw();
                }
                self.cursor = (x, y);
                self.hover();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 30.0, y * 30.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / scale, p.y as f32 / scale),
                };
                self.wheel(dx, dy);
            }
            WindowEvent::PinchGesture { delta, .. } => {
                let canvas = self.canvas();
                self.zoom(1.0 + delta as f32, self.cursor.0 - canvas.x, self.cursor.1);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pan =
                    button == MouseButton::Middle || (button == MouseButton::Left && self.space);
                match (state, pan) {
                    (ElementState::Pressed, true) => self.dragging = Some(self.cursor),
                    (ElementState::Released, true) => {
                        self.dragging = None;
                        self.save();
                    }
                    (ElementState::Pressed, false) if button == MouseButton::Left => self.click(),
                    (ElementState::Pressed, false) if button == MouseButton::Right => {
                        self.right_click()
                    }
                    _ => {}
                }
                self.hover();
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.key(&event);
                self.redraw();
            }
            WindowEvent::RedrawRequested => {
                let was_moving = !self.settled();
                self.draw();
                if was_moving && self.settled() {
                    self.save();
                }
            }
            _ => {}
        }
    }
}

/// Rects have no dash style, so the frame is laid out of short pieces along each edge.
/// Tints the inspected element the way browser devtools do: content blue, padding green, gaps purple.
fn highlight_element(out: &mut ui::Painted, inspect: &Inspector, theme: &ui::ThemeColors) {
    let elements = &inspect.elements;
    let Some(shown) = inspect.hovered.or(inspect.picked) else {
        return;
    };
    let Some(element) = elements.get(shown) else {
        return;
    };
    let content = ui::Rgba::new(0.36, 0.62, 0.95, 0.30);
    let padding_tint = ui::Rgba::new(0.45, 0.78, 0.42, 0.38);
    let gap_tint = ui::Rgba::new(0.72, 0.45, 0.92, 0.38);
    let rect = element.rect;
    match &element.what {
        ui::inspect::Inspect::Box {
            padding: [top, right, bottom, left],
            gap,
            row,
            ..
        } => {
            let inner = ui::Rect::new(
                rect.x + left,
                rect.y + top,
                (rect.w - left - right).max(0.0),
                (rect.h - top - bottom).max(0.0),
                content,
            );
            let bands = [
                (rect.x, rect.y, rect.w, *top),
                (rect.x, inner.y + inner.h, rect.w, *bottom),
                (rect.x, inner.y, *left, inner.h),
                (inner.x + inner.w, inner.y, *right, inner.h),
            ];
            for (x, y, w, h) in bands {
                if w > 0.0 && h > 0.0 {
                    out.rects.push(ui::Rect::new(x, y, w, h, padding_tint));
                }
            }
            out.rects.push(inner);
            if *gap > 0.0 {
                let children = ui::inspect::children(elements, shown);
                for pair in children.windows(2) {
                    let (Some(a), Some(b)) = (elements.get(pair[0]), elements.get(pair[1])) else {
                        continue;
                    };
                    let (a, b) = (a.rect, b.rect);
                    let band = if *row {
                        let x = a.x + a.w;
                        ui::Rect::new(x, inner.y, b.x - x, inner.h, gap_tint)
                    } else {
                        let y = a.y + a.h;
                        ui::Rect::new(inner.x, y, inner.w, b.y - y, gap_tint)
                    };
                    if band.w > 0.0 && band.h > 0.0 {
                        out.rects.push(band);
                    }
                }
            }
        }
        _ => out
            .rects
            .push(ui::Rect::new(rect.x, rect.y, rect.w, rect.h, content)),
    }
    for (index, color) in [
        (Some(shown), theme.text_accent),
        (
            inspect.picked.filter(|picked| *picked != shown),
            theme.border_focused,
        ),
    ] {
        let Some(outlined) = index.and_then(|index| elements.get(index)) else {
            continue;
        };
        let mut outline = ui::Rect::new(
            outlined.rect.x,
            outlined.rect.y,
            outlined.rect.w,
            outlined.rect.h,
            ui::Rgba::TRANSPARENT,
        );
        outline.border = 1.0;
        outline.border_color = color;
        out.rects.push(outline);
    }
    let tag: ui::Node = ui::div()
        .h_px(18.0)
        .px(ui::Space::S6)
        .items_center()
        .rounded(ui::Radius::Small)
        .bg(theme.text_accent)
        .child(
            ui::label(format!("{} x {}", rect.w.round(), rect.h.round()))
                .size(ui::LabelSize::XSmall)
                .mono()
                .color(theme.background),
        )
        .into();
    let (tag_w, tag_h) = ui::measure(&tag);
    let board_bottom = elements
        .first()
        .map_or(f32::MAX, |root| root.rect.y + root.rect.h);
    let below = rect.y + rect.h + 4.0;
    let y = if below + tag_h <= board_bottom {
        below
    } else {
        rect.y - tag_h - 4.0
    };
    out.append(ui::render(
        &tag,
        ui::Rect::new(rect.x, y, tag_w.ceil(), tag_h.ceil(), ui::Rgba::TRANSPARENT),
    ));
}

fn dashed_frame(painted: &mut ui::Painted, x: f32, y: f32, w: f32, h: f32, color: ui::Rgba) {
    const DASH: f32 = 6.0;
    const GAP: f32 = 4.0;
    let mut along = 0.0;
    while along < w {
        let length = DASH.min(w - along);
        painted
            .rects
            .push(ui::Rect::new(x + along, y, length, 1.0, color));
        painted
            .rects
            .push(ui::Rect::new(x + along, y + h - 1.0, length, 1.0, color));
        along += DASH + GAP;
    }
    along = 0.0;
    while along < h {
        let length = DASH.min(h - along);
        painted
            .rects
            .push(ui::Rect::new(x, y + along, 1.0, length, color));
        painted
            .rects
            .push(ui::Rect::new(x + w - 1.0, y + along, 1.0, length, color));
        along += DASH + GAP;
    }
}

/// A bare binary has no bundle, so the Dock would show a blank document without this.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    const ICON: &[u8] = include_bytes!("../assets/gallery-icon.png");
    // SAFETY: plain AppKit class messages on the main thread; a null image is checked before use.
    unsafe {
        let data: *mut AnyObject = msg_send![
            class!(NSData),
            dataWithBytes: ICON.as_ptr() as *const std::ffi::c_void,
            length: ICON.len(),
        ];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if image.is_null() {
            eprintln!("gallery: the Dock icon did not load");
            return;
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
    }
}

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let proxy = event_loop.create_proxy();
    ui::set_waker(move || {
        if let Err(error) = proxy.send_event(()) {
            eprintln!("gallery: wake after the window closed: {error}");
        }
    });
    let mut gallery = Gallery::new();
    event_loop.run_app(&mut gallery)?;
    Ok(())
}

fn intersect(a: ui::Rect, b: ui::Rect) -> ui::Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let w = ((a.x + a.w).min(b.x + b.w) - x).max(0.0);
    let h = ((a.y + a.h).min(b.y + b.h) - y).max(0.0);
    ui::Rect::new(x, y, w, h, ui::Rgba::TRANSPARENT)
}
