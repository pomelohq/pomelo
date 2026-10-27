//! Whole app windows as one board each: the main window over the editor fixture, and the window of a project
//! on a host with its editor, services, terminal and agents. The app lays the window out and paints it; the
//! board shows that frame.

use std::time::{Duration, Instant};

use crate::fixtures::host::HostWindow;
use crate::fixtures::window::MainWindow;
use crate::{Story, StoryDef};

const WINDOW: (f32, f32) = (1440.0, 900.0);
/// Long enough for a host's terminal and files to arrive over the stand-in wire.
const SETTLING: Duration = Duration::from_millis(800);

enum Source {
    Main(MainWindow),
    Host(HostWindow),
}

struct WindowStory {
    source: Source,
    frame: Option<ui::Frame>,
    opened: Instant,
    live: bool,
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
        match &self.frame {
            Some(frame) => board.painted(frame.base.clone()).into(),
            None => board.into(),
        }
    }

    fn overlays(&mut self, area: ui::Rect) -> Vec<ui::Overlay> {
        let Some(frame) = &self.frame else {
            return Vec::new();
        };
        // The window paints from its own top-left; the board sits at `area`.
        frame
            .overlays
            .iter()
            .map(|overlay| ui::Overlay {
                painted: ui::render(
                    &ui::div()
                        .w_px(area.w)
                        .h_px(area.h)
                        .painted(overlay.painted.clone())
                        .into(),
                    area,
                ),
                clip: overlay.clip.map(|clip| {
                    ui::Rect::new(area.x + clip.x, area.y + clip.y, clip.w, clip.h, clip.color)
                }),
            })
            .collect()
    }

    fn pending(&mut self) -> bool {
        self.live && self.opened.elapsed() < SETTLING
    }
}

fn story(source: Source, live: bool) -> anyhow::Result<Box<dyn Story>> {
    let mut story = WindowStory {
        source,
        frame: None,
        opened: Instant::now(),
        live,
    };
    story.draw();
    Ok(Box::new(story))
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
