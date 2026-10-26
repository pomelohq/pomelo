//! Screens take their colors from the theme: a color written into a feature crate does not follow the theme
//! the person picked.

use std::path::{Path, PathBuf};

/// Where a written color is the point: the toolkit itself, the gallery's own chrome, contrast math, brand logos.
const ALLOWED: &[&str] = &[
    "ui/",
    "stories/",
    "gallery/",
    "ui_snapshot/",
    "terminal_ui/src/contrast.rs",
    "database_ui/src/render.rs",
    "pomelo/src/main.rs",
];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

fn written_color(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with("//") {
        return false;
    }
    if line.contains("Rgba::hex(\"") {
        return true;
    }
    line.split("Rgba::new(").skip(1).any(|args| {
        args.trim_start()
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_digit())
    })
}

#[test]
fn feature_crates_take_colors_from_the_theme() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    sources(&crates, &mut files);
    let mut offenders = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&crates)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.iter().any(|allowed| relative.starts_with(allowed)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Tests may pin exact colors to check the math.
        let code = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for (index, line) in code.lines().enumerate() {
            if written_color(line) {
                offenders.push(format!("{relative}:{}: {}", index + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "colors written outside the theme; use a theme token or a tint from ThemeColors:\n{}",
        offenders.join("\n")
    );
}
