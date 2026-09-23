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
}

/// An item's saved state: `kind` names who can rebuild it, `data` is that item's own format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SerializedItem {
    pub kind: String,
    pub data: serde_json::Value,
}
