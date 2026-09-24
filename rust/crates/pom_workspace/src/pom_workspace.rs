//! Creating and deleting a branch workspace as a run of named stages. Each stage reports progress as
//! events; a failed run leaves `pipeline-<branch>.json` behind (same format as the previous core) so the
//! app can show where it stopped and resume from that stage.

mod create;
mod delete;
mod git;
mod node_modules;
mod prepare;

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use pom_config::Config;
use pom_paths::StateDir;
use pom_services::ServiceRunner;
use serde::{Deserialize, Serialize};

pub use create::{create, validate_branch_name, CreateRequest, CREATE_STAGES};
pub use delete::{delete, DeleteRequest, DELETE_STAGES};
pub use git::branch_is_safe_to_delete;
pub use prepare::{prepare_main, PrepareRequest};

/// Lines of a failed command's output kept in its warning.
const OUTPUT_TAIL_LINES: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    #[serde(rename = "CreateWorkspace")]
    Create,
    #[serde(rename = "DeleteWorkspace")]
    Delete,
    #[serde(rename = "PrepareMain")]
    PrepareMain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Started {
        operation: Operation,
        stages: Vec<&'static str>,
    },
    StageStarted {
        index: usize,
    },
    Progress {
        index: usize,
        detail: String,
    },
    /// Something went wrong that does not stop the run (a failed setup or seed command).
    Warning {
        index: usize,
        detail: String,
    },
    StageCompleted {
        index: usize,
    },
    StageSkipped {
        index: usize,
    },
    Completed {
        warnings: Vec<String>,
    },
    Failed {
        index: usize,
        error: String,
    },
}

pub type EventSink<'a> = &'a (dyn Fn(Event) + Sync);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineError {
    pub stage: usize,
    pub label: &'static str,
    pub message: String,
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.label, self.message)
    }
}

impl std::error::Error for PipelineError {}

/// What a pipeline works against: the project's config, its service runner and the state folder.
pub struct WorkspaceContext<'a> {
    pub config: &'a Config,
    pub runner: &'a ServiceRunner,
    pub state: &'a StateDir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageStatus {
    Pending,
    Completed,
    Skipped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageEntry {
    pub name: String,
    pub status: StageStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// A run that stopped part-way, kept so it can be resumed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineState {
    pub operation: Operation,
    pub branch: String,
    pub workspace: String,
    pub stages: Vec<StageEntry>,
    pub failed_stage: usize,
}

fn file_key(branch: &str) -> String {
    branch.replace('/', "-")
}

fn state_path(state: &StateDir, branch: &str) -> std::path::PathBuf {
    state.path(format!("pipeline-{}.json", file_key(branch)))
}

fn active_marker(state: &StateDir, branch: &str) -> std::path::PathBuf {
    state.path("active").join(file_key(branch))
}

/// The stopped run of a branch, if one is waiting to be resumed.
pub fn failed_run(state: &StateDir, branch: &str) -> Option<PipelineState> {
    let text = std::fs::read_to_string(state_path(state, branch)).ok()?;
    serde_json::from_str(&text).ok()
}

/// `stage/total label` of a run in progress for the branch.
pub fn active_run(state: &StateDir, branch: &str) -> Option<String> {
    std::fs::read_to_string(active_marker(state, branch)).ok()
}

/// Reports one stage's progress and warnings.
pub(crate) struct StageScope<'a> {
    index: usize,
    sink: EventSink<'a>,
    warnings: &'a Mutex<Vec<String>>,
}

impl StageScope<'_> {
    pub(crate) fn progress(&self, detail: impl Into<String>) {
        (self.sink)(Event::Progress {
            index: self.index,
            detail: detail.into(),
        });
    }

    pub(crate) fn warn(&self, detail: impl Into<String>) {
        let detail = detail.into();
        if let Ok(mut warnings) = self.warnings.lock() {
            warnings.push(detail.clone());
        }
        (self.sink)(Event::Warning {
            index: self.index,
            detail,
        });
    }
}

pub(crate) enum StageResult {
    Done,
    Skipped,
}

pub(crate) struct Run<'a> {
    pub operation: Operation,
    pub branch: &'a str,
    pub workspace: &'a Path,
    pub labels: &'a [&'static str],
    pub from_stage: usize,
    pub state: &'a StateDir,
    pub sink: EventSink<'a>,
    /// Whether a failure is kept in `pipeline-<branch>.json` for resuming.
    pub resumable: bool,
}

impl Run<'_> {
    /// Runs stages `from_stage..`, earlier ones reported as skipped. Stops at the first failure.
    pub(crate) fn execute(
        &self,
        mut stage: impl FnMut(usize, &StageScope<'_>) -> Result<StageResult, String>,
    ) -> Result<Outcome, PipelineError> {
        let warnings = Mutex::new(Vec::new());
        (self.sink)(Event::Started {
            operation: self.operation,
            stages: self.labels.to_vec(),
        });
        for (index, label) in self.labels.iter().enumerate() {
            if index < self.from_stage {
                (self.sink)(Event::StageSkipped { index });
                continue;
            }
            self.mark_active(index);
            (self.sink)(Event::StageStarted { index });
            let scope = StageScope {
                index,
                sink: self.sink,
                warnings: &warnings,
            };
            match stage(index, &scope) {
                Ok(StageResult::Done) => (self.sink)(Event::StageCompleted { index }),
                Ok(StageResult::Skipped) => (self.sink)(Event::StageSkipped { index }),
                Err(message) => {
                    // A first-stage failure changed nothing, so there is nothing to resume.
                    if self.resumable && index > 0 {
                        self.save_failure(index, &message);
                    }
                    self.clear_active();
                    (self.sink)(Event::Failed {
                        index,
                        error: message.clone(),
                    });
                    return Err(PipelineError {
                        stage: index,
                        label,
                        message,
                    });
                }
            }
        }
        self.clear_active();
        if self.resumable {
            if let Err(error) = std::fs::remove_file(state_path(self.state, self.branch)) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("workspace: clear pipeline state: {error}");
                }
            }
        }
        let warnings = warnings.into_inner().unwrap_or_default();
        (self.sink)(Event::Completed {
            warnings: warnings.clone(),
        });
        Ok(Outcome { warnings })
    }

    fn mark_active(&self, index: usize) {
        let marker = active_marker(self.state, self.branch);
        let text = format!("{}/{} {}", index + 1, self.labels.len(), self.labels[index]);
        let written = marker
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&marker, text));
        if let Err(error) = written {
            eprintln!("workspace: mark pipeline active: {error}");
        }
    }

    fn clear_active(&self) {
        if let Err(error) = std::fs::remove_file(active_marker(self.state, self.branch)) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("workspace: clear active marker: {error}");
            }
        }
    }

    fn save_failure(&self, failed: usize, message: &str) {
        let stages = self
            .labels
            .iter()
            .enumerate()
            .map(|(index, name)| StageEntry {
                name: name.to_string(),
                status: match index {
                    _ if index == failed => StageStatus::Failed,
                    _ if index < self.from_stage => StageStatus::Skipped,
                    _ if index < failed => StageStatus::Completed,
                    _ => StageStatus::Pending,
                },
                error: if index == failed {
                    message.to_string()
                } else {
                    String::new()
                },
            })
            .collect();
        let record = PipelineState {
            operation: self.operation,
            branch: self.branch.to_string(),
            workspace: self.workspace.to_string_lossy().into_owned(),
            stages,
            failed_stage: failed,
        };
        let saved = serde_json::to_vec_pretty(&record)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                std::fs::create_dir_all(self.state.root())?;
                std::fs::write(state_path(self.state, self.branch), bytes)
            });
        if let Err(error) = saved {
            eprintln!("workspace: save pipeline state: {error}");
        }
    }
}

/// Runs a shell command in `cwd` with the tool PATH and `env`; on failure, the tail of its output.
pub(crate) fn run_shell(
    login: bool,
    command: &str,
    cwd: &Path,
    env: &[(String, String)],
) -> Result<(), String> {
    let output = Command::new("zsh")
        .arg(if login { "-lc" } else { "-c" })
        .arg(command)
        .current_dir(cwd)
        .env("PATH", pom_services::tool_path())
        .envs(
            env.iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .output()
        .map_err(|error| format!("zsh: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(OUTPUT_TAIL_LINES)..].join("\n");
    Err(match output.status.code() {
        Some(code) => format!("exit {code}\n{tail}"),
        None => format!("killed\n{tail}"),
    })
}

pub(crate) fn repo_alias<'a>(repo: &'a str, config: &'a Config) -> &'a str {
    match config.repos.get(repo) {
        Some(dir) if !dir.alias.is_empty() => &dir.alias,
        _ => repo,
    }
}
