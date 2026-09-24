//! Saved consoles and table tabs, per project (`db-consoles/<session>.json`, the layout the previous app writes):
//! each one's database, SQL text, and a table tab's filter, order and page size.

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleKind {
    #[default]
    Query,
    Table,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Console {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// The database's name.
    #[serde(default, rename = "dbID")]
    pub database: String,
    #[serde(default)]
    pub sql: String,
    #[serde(default)]
    pub kind: ConsoleKind,
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub table: String,
    #[serde(default)]
    pub where_clause: String,
    #[serde(default)]
    pub order_by: String,
    #[serde(default)]
    pub limit: usize,
}

fn path(state: &StateDir, session: &str) -> std::path::PathBuf {
    let name = if session.is_empty() {
        "default"
    } else {
        session
    };
    state.path("db-consoles").join(format!("{name}.json"))
}

/// A missing or unreadable file means no consoles.
pub fn load_consoles(state: &StateDir, session: &str) -> Vec<Console> {
    std::fs::read_to_string(path(state, session))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_consoles(state: &StateDir, session: &str, consoles: &[Console]) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(consoles).map_err(std::io::Error::other)?;
    pom_paths::write_atomic(&path(state, session), text.as_bytes(), 0o644)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_previous_apps_file_and_round_trips() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let state = StateDir::new(temp.path());
        assert!(load_consoles(&state, "demo").is_empty());
        let file = temp.path().join("db-consoles/demo.json");
        std::fs::create_dir_all(file.parent().expect("parent"))?;
        std::fs::write(
            &file,
            r#"[{"id":"A1","title":"query 1","dbID":"demo_api_main","sql":"select 1","kind":"query","schema":"","table":"","whereClause":"","orderBy":"","limit":500},
               {"id":"B2","title":"users","dbID":"demo_api_main","kind":"table","schema":"public","table":"users","whereClause":"id > 3","orderBy":"id","limit":100}]"#,
        )?;
        let consoles = load_consoles(&state, "demo");
        assert_eq!(consoles.len(), 2);
        assert_eq!(consoles[0].database, "demo_api_main");
        assert_eq!(consoles[1].kind, ConsoleKind::Table);
        assert_eq!(consoles[1].where_clause, "id > 3");
        save_consoles(&state, "demo", &consoles)?;
        let text = std::fs::read_to_string(&file)?;
        assert!(
            text.contains("\"dbID\"") && text.contains("\"whereClause\""),
            "{text}"
        );
        assert_eq!(load_consoles(&state, "demo"), consoles);
        Ok(())
    }
}
