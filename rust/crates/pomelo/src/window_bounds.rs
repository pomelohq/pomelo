use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window, WindowAttributes};

const FILE_NAME: &str = "window-bounds.json";
const DEFAULT_SIZE: (f64, f64) = (1280.0, 820.0);
const MIN_VISIBLE: f64 = 64.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowMode {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

/// Logical points; for a maximized or fullscreen window this is the size it returns to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub mode: WindowMode,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    last: Option<WindowBounds>,
    #[serde(default)]
    projects: BTreeMap<PathBuf, WindowBounds>,
}

fn file() -> PathBuf {
    pom_paths::StateDir::from_env().path(FILE_NAME)
}

fn load() -> Stored {
    std::fs::read(file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The project's own bounds when it was open before, else the last window's.
pub fn restore(project: Option<&Path>) -> Option<WindowBounds> {
    let stored = load();
    project
        .and_then(|project| stored.projects.get(project).copied())
        .or(stored.last)
}

pub fn save(project: Option<&Path>, bounds: WindowBounds) {
    let mut stored = load();
    stored.last = Some(bounds);
    if let Some(project) = project {
        stored.projects.insert(project.to_path_buf(), bounds);
    }
    let path = file();
    let written = serde_json::to_vec_pretty(&stored)
        .map_err(std::io::Error::other)
        .and_then(|bytes| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, bytes)
        });
    if let Err(error) = written {
        eprintln!("window bounds: save {}: {error}", path.display());
    }
}

const MIN_SIZE: (f64, f64) = (360.0, 240.0);

/// Size (and position, when it still lands on a connected display) for a new window.
pub fn apply(
    attrs: WindowAttributes,
    bounds: Option<WindowBounds>,
    event_loop: &ActiveEventLoop,
) -> WindowAttributes {
    let attrs = attrs.with_min_inner_size(LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1));
    let Some(bounds) = bounds else {
        return attrs.with_inner_size(LogicalSize::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1));
    };
    let attrs = attrs
        .with_inner_size(LogicalSize::new(bounds.width, bounds.height))
        .with_maximized(bounds.mode == WindowMode::Maximized);
    if on_screen(&bounds, event_loop) {
        attrs.with_position(LogicalPosition::new(bounds.x, bounds.y))
    } else {
        attrs
    }
}

pub fn enter_saved_mode(window: &Window, bounds: Option<WindowBounds>) {
    if bounds.is_some_and(|bounds| bounds.mode == WindowMode::Fullscreen) {
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
    }
}

fn on_screen(bounds: &WindowBounds, event_loop: &ActiveEventLoop) -> bool {
    event_loop.available_monitors().any(|monitor| {
        let scale = monitor.scale_factor();
        let origin = monitor.position().to_logical::<f64>(scale);
        let size = monitor.size().to_logical::<f64>(scale);
        let overlap_x =
            (bounds.x + bounds.width).min(origin.x + size.width) - bounds.x.max(origin.x);
        let overlap_y =
            (bounds.y + bounds.height).min(origin.y + size.height) - bounds.y.max(origin.y);
        overlap_x >= MIN_VISIBLE && overlap_y >= MIN_VISIBLE
    })
}

/// The window's current windowed frame, or `None` while maximized or fullscreen (their frame is not the one
/// to return to).
pub fn windowed_frame(window: &Window) -> Option<(f64, f64, f64, f64)> {
    if window.is_maximized() || window.fullscreen().is_some() {
        return None;
    }
    let scale = window.scale_factor();
    let position = window.outer_position().ok()?.to_logical::<f64>(scale);
    let size = window.inner_size().to_logical::<f64>(scale);
    Some((position.x, position.y, size.width, size.height))
}

pub fn current(window: &Window, windowed: Option<(f64, f64, f64, f64)>) -> Option<WindowBounds> {
    let mode = if window.fullscreen().is_some() {
        WindowMode::Fullscreen
    } else if window.is_maximized() {
        WindowMode::Maximized
    } else {
        WindowMode::Windowed
    };
    let (x, y, width, height) = windowed_frame(window).or(windowed)?;
    Some(WindowBounds {
        mode,
        x,
        y,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_round_trips_with_projects() {
        let bounds = WindowBounds {
            mode: WindowMode::Maximized,
            x: 10.0,
            y: 20.0,
            width: 900.0,
            height: 600.0,
        };
        let mut stored = Stored {
            last: Some(bounds),
            ..Stored::default()
        };
        stored.projects.insert(PathBuf::from("/p/pom.yml"), bounds);
        let bytes = serde_json::to_vec(&stored).unwrap();
        let back: Stored = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.last, Some(bounds));
        assert_eq!(back.projects.get(Path::new("/p/pom.yml")), Some(&bounds));
    }

    #[test]
    fn missing_projects_key_still_loads() {
        let back: Stored = serde_json::from_str(
            r#"{"last":{"mode":"Windowed","x":0,"y":0,"width":800,"height":600}}"#,
        )
        .unwrap();
        assert!(back.projects.is_empty());
        assert_eq!(back.last.map(|b| b.width), Some(800.0));
    }
}
