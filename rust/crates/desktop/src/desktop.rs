//! Hands files, folders and links to the desktop environment.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// Opens a file, folder or URL with its default app.
pub fn open(target: impl AsRef<OsStr>) -> std::io::Result<()> {
    spawn(opener().arg(target))
}

/// Shows `path` in the file manager: selected on macOS, its folder elsewhere.
pub fn reveal(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    #[cfg(target_os = "macos")]
    {
        spawn(Command::new("open").arg("-R").arg(path))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let folder = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        open(folder)
    }
}

/// Opens `target` in the named app: an app bundle name on macOS, a command on PATH elsewhere.
pub fn open_with(app: &str, target: impl AsRef<OsStr>) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        spawn(Command::new("open").arg("-a").arg(app).arg(target))
    }
    #[cfg(not(target_os = "macos"))]
    {
        spawn(Command::new(app).arg(target))
    }
}

fn opener() -> Command {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new("xdg-open")
    }
}

// The opener exits once it has handed the target over; waiting on it off-thread keeps it from lingering as a zombie.
fn spawn(command: &mut Command) -> std::io::Result<()> {
    let mut child = command.spawn()?;
    std::thread::Builder::new()
        .name("desktop-open".into())
        .spawn(move || {
            if let Err(error) = child.wait() {
                eprintln!("desktop open: {error}");
            }
        })?;
    Ok(())
}

/// The menu label for `reveal`, named after the platform's file manager.
pub const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else {
    "Reveal in File Manager"
};
