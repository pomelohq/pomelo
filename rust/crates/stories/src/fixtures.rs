//! The data stories and scenes draw: scratch projects on disk, a stand-in host, and the window's fixed facts.

pub mod editor;
pub mod forms;
pub mod git;
pub mod host;
pub mod pull_request;
pub mod settings;

use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

/// A folder of its own for one story's files, removed with the story.
pub struct Scratch(std::path::PathBuf);

impl Scratch {
    pub fn new(kind: &str) -> anyhow::Result<Scratch> {
        let dir = std::env::temp_dir().join(format!(
            "pom-story-{kind}-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Scratch(dir))
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Scratch files of this process; removed by whoever owns the run.
pub fn fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/tmp/pomelo-qa-fixture/{}", std::process::id()))
}

pub fn crop_rgba(
    width: u32,
    rgba: &[u8],
    (x, y, w, h): (u32, u32, u32, u32),
) -> (u32, u32, Vec<u8>) {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let start = ((row * width + x) * 4) as usize;
        out.extend_from_slice(rgba.get(start..start + (w * 4) as usize).unwrap_or(&[]));
    }
    (w, h, out)
}

/// The window buttons the OS draws, which a scene stands in for where the app pins them.
pub fn scene_lights() -> Vec<ui::Rect> {
    [
        (0.996, 0.373, 0.341),
        (0.996, 0.737, 0.18),
        (0.157, 0.784, 0.251),
    ]
    .iter()
    .enumerate()
    .map(|(index, (r, g, b))| ui::Rect {
        radius: 6.0,
        ..ui::Rect::new(
            20.0 + index as f32 * 20.0,
            (workspace::TOP_BAR_H - 12.0) / 2.0,
            12.0,
            12.0,
            ui::Rgba::new(*r, *g, *b, 1.0),
        )
    })
    .collect()
}

pub fn window_usage() -> workspace::UsageInfo {
    workspace::UsageInfo {
        account: Some(workspace::UsageAccount {
            name: "Alex".into(),
            email: "alex@example.com".into(),
            plan: "team".into(),
            organization: "myproject".into(),
        }),
        session: Some(workspace::UsageWindow {
            used: 8.0,
            resets: "in 2h 56m".into(),
        }),
        weekly: Some(workspace::UsageWindow {
            used: 26.0,
            resets: "Sat 17:00".into(),
        }),
        note: "Updated 2m ago".into(),
        today: Some(workspace::UsageToday {
            total: "$128".into(),
            sessions: 6,
            by_workspace: Vec::new(),
        }),
        hidden: false,
    }
}
