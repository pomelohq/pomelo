//! Whole app windows as one board each: the main window over the editor fixture, and the window of a project
//! on a host with its editor, services, terminal and agents. The app lays the window out and paints it; the
//! board shows that frame.

use std::time::{Duration, Instant};

use crate::fixtures::host::HostWindow;
use crate::fixtures::window::MainWindow;
use crate::{Story, StoryDef};

pub const WINDOW: (f32, f32) = (1440.0, 900.0);
/// Long enough for a host's terminal and files to arrive over the stand-in wire.
const SETTLING: Duration = Duration::from_millis(800);

pub enum Source {
    Main(MainWindow),
    Host(HostWindow),
}

struct WindowStory {
    source: Source,
    frame: Option<ui::Frame>,
    opened: Instant,
    settling: Duration,
    /// The part of the window the board shows; the whole window when none.
    crop: ui::Rect,
}

impl WindowStory {
    fn draw(&mut self) {
        let frame = match &mut self.source {
            Source::Main(window) => window.frame(),
            Source::Host(window) => window.frame(),
        };
        if frame.is_some() {
            self.frame = frame;
        }
    }
}

impl Story for WindowStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        self.draw();
        let board = ui::div().col().w_px(width).h_px(height);
        let Some(frame) = &self.frame else {
            return board.into();
        };
        let window = ui::Rect::new(
            -self.crop.x,
            -self.crop.y,
            WINDOW.0,
            WINDOW.1,
            ui::Rgba::TRANSPARENT,
        );
        board
            .painted(ui::render(
                &ui::div().painted(frame.base.clone()).into(),
                window,
            ))
            .into()
    }

    fn overlays(&mut self, area: ui::Rect) -> Vec<ui::Overlay> {
        let Some(frame) = &self.frame else {
            return Vec::new();
        };
        // The window paints from its own top-left; the board sits at `area` and shows the crop of it.
        let (dx, dy) = (area.x - self.crop.x, area.y - self.crop.y);
        let window = ui::Rect::new(dx, dy, WINDOW.0, WINDOW.1, ui::Rgba::TRANSPARENT);
        frame
            .overlays
            .iter()
            .map(|overlay| {
                let clip = overlay
                    .clip
                    .map(|clip| ui::Rect::new(dx + clip.x, dy + clip.y, clip.w, clip.h, clip.color))
                    .unwrap_or(area);
                ui::Overlay {
                    painted: ui::render(&ui::div().painted(overlay.painted.clone()).into(), window),
                    clip: Some(intersect(clip, area)),
                }
            })
            .collect()
    }

    fn pending(&mut self) -> bool {
        let pending = self.opened.elapsed() < self.settling;
        if pending {
            self.draw();
        }
        pending
    }
}

fn intersect(a: ui::Rect, b: ui::Rect) -> ui::Rect {
    let (x, y) = (a.x.max(b.x), a.y.max(b.y));
    let w = ((a.x + a.w).min(b.x + b.w) - x).max(0.0);
    let h = ((a.y + a.h).min(b.y + b.h) - y).max(0.0);
    ui::Rect::new(x, y, w, h, a.color)
}

/// A board over `crop` of a window, redrawn for `settling` while its content arrives.
pub fn cropped(
    source: Source,
    crop: (f32, f32, f32, f32),
    settling: Duration,
) -> anyhow::Result<Box<dyn Story>> {
    let (x, y, w, h) = crop;
    let mut story = WindowStory {
        source,
        frame: None,
        opened: Instant::now(),
        settling,
        crop: ui::Rect::new(x, y, w, h, ui::Rgba::TRANSPARENT),
    };
    story.draw();
    Ok(Box::new(story))
}

fn story(source: Source, live: bool) -> anyhow::Result<Box<dyn Story>> {
    let settling = if live { SETTLING } else { Duration::ZERO };
    cropped(source, (0.0, 0.0, WINDOW.0, WINDOW.1), settling)
}

fn main_window() -> anyhow::Result<Box<dyn Story>> {
    story(Source::Main(MainWindow::open(WINDOW, 1.0)?), false)
}

fn host_window(mode: &str) -> anyhow::Result<Box<dyn Story>> {
    let window = HostWindow::open(mode, WINDOW, 1.0)?;
    let live = window.live();
    story(Source::Host(window), live)
}

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, build| StoryDef {
        id,
        page: "Pages",
        group: "Windows",
        title,
        size: WINDOW,
        build,
    };
    vec![
        story("window-main", "Main window", main_window),
        story("window-host-editor", "Editor on a host", || {
            host_window("files-open")
        }),
        story("window-host-services", "Services on a host", || {
            host_window("connected")
        }),
        story("window-host-terminal", "Terminal on a host", || {
            host_window("terminal")
        }),
        story("window-host-agents", "Agents on a host", || {
            host_window("agents")
        }),
    ]
}
