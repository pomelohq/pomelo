//! The user's snippet files, `<scope>.json` per language plus `snippets.json` for all, re-read when they change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use editor::highlight::Lang;
use editor::snippet::{parse_snippet_file, snippet_scope, SnippetDefinition, GLOBAL_SCOPE};

/// Completions ask on every keystroke; the directory is looked at no more often than this.
const RESCAN_INTERVAL: Duration = Duration::from_secs(1);

pub fn default_dir() -> Option<PathBuf> {
    Some(pom_paths::config_dir()?.join("snippets"))
}

#[derive(Default)]
struct DirCache {
    scanned_at: Option<Instant>,
    files: HashMap<String, (Option<SystemTime>, Vec<Arc<SnippetDefinition>>)>,
}

static CACHES: LazyLock<Mutex<HashMap<PathBuf, DirCache>>> = LazyLock::new(Mutex::default);

impl DirCache {
    fn rescan(&mut self, dir: &Path) {
        if self
            .scanned_at
            .is_some_and(|at| at.elapsed() < RESCAN_INTERVAL)
        {
            return;
        }
        self.scanned_at = Some(Instant::now());
        let Ok(entries) = std::fs::read_dir(dir) else {
            self.files.clear();
            return;
        };
        let mut seen = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let stem = stem.to_string();
            let modified = entry.metadata().and_then(|m| m.modified()).ok();
            seen.push(stem.clone());
            if self
                .files
                .get(&stem)
                .is_some_and(|(cached, _)| *cached == modified && modified.is_some())
            {
                continue;
            }
            let snippets = match std::fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))
                .and_then(|json| parse_snippet_file(&json, &path))
            {
                Ok(snippets) => snippets.into_iter().map(Arc::new).collect(),
                Err(error) => {
                    eprintln!("snippets: {error}");
                    Vec::new()
                }
            };
            self.files.insert(stem, (modified, snippets));
        }
        self.files.retain(|stem, _| seen.contains(stem));
    }
}

/// Snippets for `lang` from `dir`: the language's own first, then the global ones.
pub fn snippets_for(dir: &Path, lang: Lang) -> Vec<Arc<SnippetDefinition>> {
    let Ok(mut caches) = CACHES.lock() else {
        return Vec::new();
    };
    let cache = caches.entry(dir.to_path_buf()).or_default();
    cache.rescan(dir);
    [snippet_scope(lang), GLOBAL_SCOPE]
        .iter()
        .filter_map(|scope| cache.files.get(*scope))
        .flat_map(|(_, snippets)| snippets.iter().cloned())
        .collect()
}
