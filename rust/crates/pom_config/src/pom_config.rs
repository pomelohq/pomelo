mod decode;
mod depgraph;
mod include;
mod lookup;
pub mod maintain;
mod presets;
mod schema;
mod validate;
pub mod yaml_node;

use std::path::{Path, PathBuf};

pub use depgraph::{CycleError, DepGraph};
pub use include::{fragment_files, FRAGMENT_DIR};
pub use lookup::ResolvedService;
pub use schema::{
    CodeAgentsConfig, Config, Dir, EnvFileEntry, HealthCheck, Preset, Service, SharedServiceDef,
    SharedServiceRef, Shortcut, SyncConfig, UiConfig, DEFAULT_ENV_FILE, DEFAULT_SESSION,
};

use crate::yaml_node::Node;

pub const CONFIG_FILE_NAME: &str = "pom.yml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for LoadError {}

impl Config {
    /// Parses `path` plus every `pom.d` fragment beside it, then fills preset and well-known
    /// defaults. Validation is separate so callers can still show a config that fails it.
    pub fn load(path: &Path) -> Result<Config, LoadError> {
        let root = merged_document(path)?;
        let mut decoder = decode::Decoder::default();
        let mut config = Config::decode(&root, &mut decoder);
        if !decoder.errors.is_empty() {
            return Err(LoadError {
                path: path.to_path_buf(),
                message: format!("unmarshal errors:\n  {}", decoder.errors.join("\n  ")),
            });
        }
        config.apply_well_known_defaults();
        config.apply_presets();
        config.apply_workspace_presets();
        Ok(config)
    }
}

/// The root document with fragments merged in, before any typing.
pub fn merged_document(path: &Path) -> Result<Node, LoadError> {
    let error = |path: &Path, message: String| LoadError {
        path: path.to_path_buf(),
        message,
    };
    let mut root = read_document(path)?.unwrap_or_else(Node::empty_mapping);
    let config_dir = path.parent().unwrap_or(Path::new("."));
    let fragments = fragment_files(config_dir)
        .map_err(|e| error(config_dir, format!("read {FRAGMENT_DIR}: {e}")))?;
    if fragments.is_empty() {
        return Ok(root);
    }
    if !root.is_mapping() {
        return Err(error(path, "root is not a mapping".into()));
    }
    for fragment in fragments {
        match read_document(&fragment)? {
            Some(document) if document.is_mapping() => include::merge_mapping(&mut root, document),
            _ => {}
        }
    }
    Ok(root)
}

fn read_document(path: &Path) -> Result<Option<Node>, LoadError> {
    let source = std::fs::read_to_string(path).map_err(|e| LoadError {
        path: path.to_path_buf(),
        message: format!("read failed: {e}"),
    })?;
    yaml_node::parse(&source).map_err(|e| LoadError {
        path: path.to_path_buf(),
        message: format!("parse failed: {e}"),
    })
}

/// Walks up from `start` to the nearest directory holding a `pom.yml`.
pub fn find_config_from(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(CONFIG_FILE_NAME))
        .find(|candidate| candidate.is_file())
}
