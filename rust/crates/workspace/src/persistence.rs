//! What a pane group looks like on disk: its split tree with each pane's tabs, which pane and tab are active, and
//! whatever each item saves about itself (a file's path and view state, a terminal's directory).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializedMember {
    Split {
        axis: SerializedAxis,
        flexes: Vec<f32>,
        children: Vec<SerializedMember>,
    },
    Pane(SerializedPane),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializedAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SerializedPane {
    /// The pane that had focus in its group.
    pub active: bool,
    pub items: Vec<SerializedItem>,
    pub active_item: Option<usize>,
    /// How many of the first items are pinned tabs.
    #[serde(default)]
    pub pinned_count: usize,
}

/// An item's saved state: `kind` names who can rebuild it, `data` is that item's own format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SerializedItem {
    pub kind: String,
    pub data: serde_json::Value,
}

/// A project's saved panes: the editor area and the terminal panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SerializedWorkspace {
    pub root: std::path::PathBuf,
    pub center: Option<SerializedMember>,
    pub panel: Option<SerializedMember>,
    /// The terminal panel was zoomed (only docks keep their zoom across sessions).
    #[serde(default)]
    pub panel_zoomed: bool,
}

/// Where a project's panes are saved: one file per project root under the config directory.
pub fn workspace_state_path(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file = format!(
        "{name}-{:016x}.json",
        stable_hash(root.as_os_str().as_encoded_bytes())
    );
    Some(
        std::path::PathBuf::from(home)
            .join(".config/pomelo/workspaces")
            .join(file),
    )
}

/// FNV-1a, so the file name for a root stays the same across builds (the std hasher makes no such promise).
fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

pub fn load_workspace(root: &std::path::Path) -> Option<SerializedWorkspace> {
    let text = std::fs::read_to_string(workspace_state_path(root)?).ok()?;
    let saved: SerializedWorkspace = serde_json::from_str(&text).ok()?;
    (saved.root == root).then_some(saved)
}

pub fn save_workspace(json: &str, root: &std::path::Path) -> std::io::Result<()> {
    let Some(path) = workspace_state_path(root) else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_project_root_gets_its_own_stable_file() {
        let a = workspace_state_path(std::path::Path::new("/work/api"));
        let b = workspace_state_path(std::path::Path::new("/work/web"));
        assert_ne!(a, b);
        assert_eq!(a, workspace_state_path(std::path::Path::new("/work/api")));
        let name = a
            .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        assert!(name.starts_with("api-") && name.ends_with(".json"));
        assert_eq!(stable_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn state_saved_before_zoom_and_pins_still_loads() {
        let json = r#"{"root":"/work/api","center":{"pane":{"active":true,"items":[],"active_item":null}},"panel":null}"#;
        let saved: SerializedWorkspace = serde_json::from_str(json).unwrap();
        assert!(!saved.panel_zoomed);
        let Some(SerializedMember::Pane(pane)) = saved.center else {
            panic!("the center pane should load");
        };
        assert_eq!(pane.pinned_count, 0);
    }
}
