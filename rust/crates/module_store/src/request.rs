use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Holding, Measure, Method, Overview, RepoState, User, Version};

/// What the Store page asks of a store, here or on another machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    #[default]
    Refresh,
    /// Every copy no worktree of this project uses.
    FreeUnused,
    FreeOld {
        repo: String,
    },
    FreeOthers,
    /// Swap a worktree's own install for the stored copy.
    Relink {
        repo: String,
        workspace: String,
        path: PathBuf,
    },
    /// Keep a worktree's own install as the stored copy.
    Keep {
        repo: String,
        path: PathBuf,
    },
    /// Everything `plan` lists, one after another.
    Optimize,
}

impl Request {
    /// What the header says while it runs.
    pub fn working(&self) -> String {
        match self {
            Request::Refresh => "Looking at every workspace".into(),
            Request::FreeUnused => "Removing unused copies".into(),
            Request::FreeOld { repo } => format!("Removing old {repo} copies"),
            Request::FreeOthers => "Removing other projects' copies".into(),
            Request::Relink { workspace, .. } => format!("Swapping {workspace} to the shared copy"),
            Request::Keep { repo, .. } => format!("Keeping {repo}'s install"),
            Request::Optimize => "Optimizing".into(),
        }
    }

    /// It frees or plans from the overview the page showed.
    pub fn reads_overview(&self) -> bool {
        matches!(
            self,
            Request::FreeUnused | Request::FreeOld { .. } | Request::FreeOthers | Request::Optimize
        )
    }
}

/// What a running request reports, in order: the overview after it, then each size as it is measured.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "update", rename_all = "snake_case")]
pub enum StoreUpdate {
    /// Where a store on another machine keeps its copies and how it reaches workspaces there.
    Place {
        root: String,
        method: String,
    },
    Overview {
        overview: Overview,
        /// Workspaces whose services run, as (repo, workspace).
        running: Vec<(String, String)>,
        /// Sizes still to come.
        pending: usize,
        message: Option<String>,
        failed: bool,
        /// The row to highlight.
        flash: Option<String>,
    },
    Measured {
        what: Measure,
        size: u64,
    },
    /// Optimize started its `index`-th step (from 1) of `total`.
    Step {
        request: Request,
        index: usize,
        total: usize,
    },
    /// The request could not run at all.
    Failed {
        message: String,
    },
}

/// How copies reach workspaces with `method`, for the page's header.
pub fn describe_method(method: Option<Method>) -> &'static str {
    match method {
        Some(Method::Clone) => "copy-on-write, workspaces take no extra disk",
        Some(Method::HardLink) => "hard links, workspaces share read-only files",
        Some(Method::Copy) => "copies, each workspace takes the full size",
        None => "off on this drive, installs run as usual",
    }
}

impl Version {
    /// Worktrees with an install of their own.
    pub fn own_users(&self) -> impl Iterator<Item = &User> {
        self.users
            .iter()
            .filter(|user| matches!(user.holding, Holding::Own { .. }))
    }

    /// With no stored copy but an install to keep, keeping it: main's first, else any workspace's.
    pub fn keep_request(&self, repo: &str) -> Option<Request> {
        if self.stored.is_some() {
            return None;
        }
        let installed = |user: &&User| !matches!(user.holding, Holding::NotInstalled);
        let source = self
            .users
            .iter()
            .filter(installed)
            .find(|user| user.is_main)
            .or_else(|| self.users.iter().find(installed))?;
        Some(Request::Keep {
            repo: repo.to_string(),
            path: source.path.clone(),
        })
    }
}

/// What Optimize does, in order: keep an install for each lockfile with no stored copy, move every
/// workspace's own install onto the shared copy, then free the copies no workspace uses.
pub fn plan(overview: &Overview) -> Vec<Request> {
    let mut keeps = Vec::new();
    let mut relinks = Vec::new();
    for repo in &overview.repos {
        let RepoState::Versions { versions, .. } = &repo.state else {
            continue;
        };
        for version in versions {
            let keep = version.keep_request(&repo.repo);
            let source = match &keep {
                Some(Request::Keep { path, .. }) => Some(path.clone()),
                _ => None,
            };
            if version.stored.is_none() && keep.is_none() {
                continue;
            }
            keeps.extend(keep);
            for user in version.own_users() {
                if source.as_ref() != Some(&user.path) {
                    relinks.push(Request::Relink {
                        repo: repo.repo.clone(),
                        workspace: user.workspace.clone(),
                        path: user.path.clone(),
                    });
                }
            }
        }
    }
    let mut steps = keeps;
    steps.extend(relinks);
    if !overview.unused().is_empty() {
        steps.push(Request::FreeUnused);
    }
    steps
}
