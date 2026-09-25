mod compose;
mod emit;
mod monorepo;
mod rules;

use std::path::Path;

pub use compose::{parse_compose, ComposeService, ServiceKind};
pub use emit::{emit, emit_repo, RepoDetection};
pub use monorepo::detect_repo;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedRun {
    /// server | worker
    pub kind: String,
    pub cmd: String,
}

/// How one app in a repo runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackFacts {
    /// Repo-relative folder of a monorepo member; empty for the repo root.
    pub dir: String,
    pub rule_id: String,
    pub language: String,
    pub framework: String,
    pub package_manager: String,
    pub install: String,
    pub run: Vec<ResolvedRun>,
    pub setup: Vec<String>,
    pub port: u16,
    /// global-shared | per-project
    pub dep_cache: String,
}

/// The best-matching stack for the folder, if any rule matches.
pub fn detect(repo: &Path) -> Option<StackFacts> {
    rules::detect_with(&rules::load_rules(), repo, repo)
}
