use std::path::PathBuf;
use std::process::Command;

use winit::window::{Window, WindowAttributes};

/// The app id and WM class match the `.desktop` entry, so the shell groups the windows under Pomelo's icon.
pub(crate) fn window_attributes(attributes: WindowAttributes) -> WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    WindowAttributesExtWayland::with_name(attributes, "pomelo", "pomelo")
}

/// The main window draws its own title bar with the window buttons, so the system's is turned off.
pub(crate) fn main_window_attributes(attributes: WindowAttributes) -> WindowAttributes {
    window_attributes(attributes).with_decorations(false)
}

/// Wayland draws a frame when the compositor's frame callback asks for one; drawing ahead of it blocks the
/// thread in the swap until the compositor catches up.
pub(crate) const COMPOSITOR_PACED: bool = true;

pub(crate) fn hide_pointer_until_mouse_moves() {}

pub(crate) fn titlebar_double_click(window: &Window) {
    window.set_maximized(!window.is_maximized());
}

pub(crate) fn is_native_fullscreen(window: &Window) -> bool {
    window.fullscreen().is_some()
}

/// Asks for folders through the desktop's own chooser (zenity on GNOME, kdialog on KDE); none when neither exists.
pub(crate) fn choose_folders(multiple: bool) -> Vec<PathBuf> {
    let title = if multiple {
        "Choose the git repositories of the project"
    } else {
        "Choose a project folder with a pom.yml"
    };
    let mut zenity = Command::new("zenity");
    zenity
        .args(["--file-selection", "--directory", "--separator=\n"])
        .arg(format!("--title={title}"));
    if multiple {
        zenity.arg("--multiple");
    }
    let mut kdialog = Command::new("kdialog");
    kdialog
        .args(["--title", title, "--getexistingdirectory"])
        .arg(std::env::var_os("HOME").unwrap_or_default());
    for mut chooser in [zenity, kdialog] {
        match chooser.output() {
            Ok(output) => {
                return String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from)
                    .collect();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("folder chooser: {error}");
                return Vec::new();
            }
        }
    }
    eprintln!("folder chooser: install zenity or kdialog to pick folders");
    Vec::new()
}

/// The XDG autostart entry that opens the app at login.
fn login_item_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("autostart/pomelo.desktop"))
}

pub(crate) fn start_at_login() -> bool {
    login_item_path().is_some_and(|path| path.exists())
}

pub(crate) fn set_start_at_login(enabled: bool) -> std::io::Result<()> {
    let Some(path) = login_item_path() else {
        return Ok(());
    };
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe()?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Pomelo\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, entry)
}

/// The undecorated window's border band that resizes it, as the cursor (physical pixels) stands on it.
pub(crate) fn resize_edge(
    window: &Window,
    cursor: (f64, f64),
) -> Option<winit::window::ResizeDirection> {
    use winit::window::ResizeDirection;

    if window.is_maximized() || window.fullscreen().is_some() {
        return None;
    }
    let scale = window.scale_factor();
    let size = window.inner_size();
    let (width, height) = (size.width as f64 / scale, size.height as f64 / scale);
    let (x, y) = (cursor.0 / scale, cursor.1 / scale);
    const EDGE: f64 = 5.0;
    const CORNER: f64 = 12.0;
    let (left, right) = (x < CORNER, x > width - CORNER);
    let (top, bottom) = (y < CORNER, y > height - CORNER);
    if (top && left) && (x < EDGE || y < EDGE) {
        return Some(ResizeDirection::NorthWest);
    }
    if (top && right) && (x > width - EDGE || y < EDGE) {
        return Some(ResizeDirection::NorthEast);
    }
    if (bottom && left) && (x < EDGE || y > height - EDGE) {
        return Some(ResizeDirection::SouthWest);
    }
    if (bottom && right) && (x > width - EDGE || y > height - EDGE) {
        return Some(ResizeDirection::SouthEast);
    }
    if y < EDGE {
        Some(ResizeDirection::North)
    } else if y > height - EDGE {
        Some(ResizeDirection::South)
    } else if x < EDGE {
        Some(ResizeDirection::West)
    } else if x > width - EDGE {
        Some(ResizeDirection::East)
    } else {
        None
    }
}

/// The editing keys are laid out for macOS: (command, option, control). Here Ctrl is the shortcut key and moves
/// by word on arrows and deletes, Ctrl+Alt by subword, Ctrl+Space still completes, Shift+Alt+Up/Down adds a
/// cursor, Ctrl+Shift+Alt+Up/Down duplicates the line and Shift+Alt+Left/Right grows or shrinks the selection
/// by syntax node, as Linux editors do; the macOS emacs bindings on Control have no Linux counterpart, and Super
/// belongs to the desktop.
pub(crate) fn editing_modifiers(
    key: &winit::keyboard::Key,
    _super_key: bool,
    alt: bool,
    ctrl: bool,
    shift: bool,
) -> (bool, bool, bool) {
    use winit::keyboard::{Key, NamedKey};

    match key {
        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight) if alt && shift && !ctrl => {
            (false, false, true)
        }
        Key::Named(
            NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::Backspace | NamedKey::Delete,
        ) => (false, ctrl || alt, ctrl && alt),
        Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) if alt && shift => {
            if ctrl {
                (false, true, false)
            } else {
                (true, true, false)
            }
        }
        Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::Space) => (false, alt, ctrl),
        _ => (ctrl, alt, false),
    }
}
