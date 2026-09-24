//! `pom ws create|delete|rename|list` and `pom prepare-main`.

use std::io::Write;
use std::sync::mpsc;

use pom_layout::WorkspaceState;
use pom_services::ServiceTarget;
use pom_workspace::{
    CreateRequest, DeleteRequest, Event, EventSink, Outcome, PipelineError, PrepareRequest,
    WorkspaceContext,
};

use crate::{say, Session};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceCommand {
    Create(CreateRequest),
    Delete(DeleteRequest),
    Rename { branch: String, name: String },
    List,
    PrepareMain(PrepareRequest),
}

/// Parses what follows `ws` (or `prepare-main` when `prepare` is set).
pub(crate) fn parse(words: &[&str], prepare: bool) -> Result<WorkspaceCommand, String> {
    let mut positional: Vec<&str> = Vec::new();
    let mut flags: Vec<(&str, Option<&str>)> = Vec::new();
    let mut iter = words.iter();
    while let Some(word) = iter.next() {
        let Some(flag) = word.strip_prefix("--") else {
            positional.push(word);
            continue;
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (flag, None),
        };
        let takes_value = matches!(name, "repos" | "env" | "from-stage");
        let value = match (takes_value, inline) {
            (true, Some(value)) => Some(value),
            (true, None) => Some(*iter.next().ok_or(format!("--{name} needs a value"))?),
            (false, Some(_)) => return Err(format!("--{name} takes no value")),
            (false, None) => None,
        };
        flags.push((name, value));
    }
    let allow = |known: &[&str]| -> Result<(), String> {
        match flags.iter().find(|(name, _)| !known.contains(name)) {
            Some((name, _)) => Err(format!("unknown flag --{name}")),
            None => Ok(()),
        }
    };
    let value = |name: &str| {
        flags
            .iter()
            .find(|(flag, _)| *flag == name)
            .and_then(|(_, value)| *value)
    };
    let has = |name: &str| flags.iter().any(|(flag, _)| *flag == name);
    let from_stage = || -> Result<usize, String> {
        match value("from-stage") {
            None => Ok(0),
            Some(text) => match text.parse::<usize>() {
                Ok(stage) if stage >= 1 => Ok(stage - 1),
                _ => Err(format!(
                    "--from-stage takes a stage number from 1, not {text:?}"
                )),
            },
        }
    };

    if prepare {
        allow(&["no-seed"])?;
        if !positional.is_empty() {
            return Err("prepare-main takes no arguments".into());
        }
        return Ok(WorkspaceCommand::PrepareMain(PrepareRequest {
            skip_seed: has("no-seed"),
        }));
    }
    let (action, rest) = positional
        .split_first()
        .ok_or("ws needs create, delete, rename or list")?;
    match (*action, rest) {
        ("create", [branch]) => {
            allow(&["repos", "env", "no-seed", "from-stage"])?;
            let repos: Vec<String> = value("repos")
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|repo| !repo.is_empty())
                .map(str::to_string)
                .collect();
            if let Some(repo) = repos.iter().find(|repo| repo.contains(':')) {
                return Err(format!(
                    "--repos takes repo names ({repo:?}); every repo checks out the workspace branch"
                ));
            }
            Ok(WorkspaceCommand::Create(CreateRequest {
                branch: branch.to_string(),
                repos,
                environment: value("env").unwrap_or("").to_string(),
                skip_seed: has("no-seed"),
                from_stage: from_stage()?,
            }))
        }
        ("delete", [branch]) => {
            allow(&["from-stage"])?;
            Ok(WorkspaceCommand::Delete(DeleteRequest {
                branch: branch.to_string(),
                from_stage: from_stage()?,
            }))
        }
        ("rename", [branch, name @ ..]) if name.len() <= 1 => {
            allow(&[])?;
            Ok(WorkspaceCommand::Rename {
                branch: branch.to_string(),
                name: name
                    .first()
                    .map_or_else(String::new, |name| name.to_string()),
            })
        }
        ("list", []) => {
            allow(&[])?;
            Ok(WorkspaceCommand::List)
        }
        ("create" | "delete", _) => Err(format!("ws {action} takes one branch")),
        ("rename", _) => Err("ws rename takes a branch and an optional name".into()),
        ("list", _) => Err("ws list takes no arguments".into()),
        (other, _) => Err(format!("unknown ws command {other}")),
    }
}

impl Session {
    pub(crate) fn workspace(
        &self,
        command: &WorkspaceCommand,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        match command {
            WorkspaceCommand::Create(request) => self.create_workspace(request, out),
            WorkspaceCommand::Delete(request) => self.delete_workspace(request, out),
            WorkspaceCommand::Rename { branch, name } => self.rename_workspace(branch, name, out),
            WorkspaceCommand::List => self.list_workspaces(out),
            WorkspaceCommand::PrepareMain(request) => self.prepare_main(request, out),
        }
    }

    fn context(&self) -> WorkspaceContext<'_> {
        WorkspaceContext {
            config: &self.config,
            runner: &self.runner,
            state: &self.state,
        }
    }

    fn create_workspace(&self, request: &CreateRequest, out: &mut dyn Write) -> Result<(), String> {
        self.config.validate_environment(&request.environment)?;
        let context = self.context();
        let result = stream(out, |sink| pom_workspace::create(&context, request, sink))?;
        let folder = pom_layout::workspace_folder(&self.project.root, &request.branch);
        match result {
            Ok(outcome) => {
                say(
                    out,
                    &format!("\nWorkspace {} ready{}", request.branch, warned(&outcome)),
                )?;
                say(out, &format!("  cd {}", folder.display()))
            }
            Err(error) => Err(retry_hint(
                &error,
                &format!("pom ws create {}", request.branch),
            )),
        }
    }

    fn delete_workspace(&self, request: &DeleteRequest, out: &mut dyn Write) -> Result<(), String> {
        let context = self.context();
        match stream(out, |sink| pom_workspace::delete(&context, request, sink))? {
            Ok(outcome) => say(
                out,
                &format!("\nWorkspace {} deleted{}", request.branch, warned(&outcome)),
            ),
            Err(error) => Err(retry_hint(
                &error,
                &format!("pom ws delete {}", request.branch),
            )),
        }
    }

    fn prepare_main(&self, request: &PrepareRequest, out: &mut dyn Write) -> Result<(), String> {
        let context = self.context();
        match stream(out, |sink| {
            pom_workspace::prepare_main(&context, request, sink)
        })? {
            Ok(outcome) => say(
                out,
                &format!(
                    "\nMain prepared{}. New workspaces copy these databases.",
                    warned(&outcome)
                ),
            ),
            Err(error) => Err(error.to_string()),
        }
    }

    fn rename_workspace(
        &self,
        branch: &str,
        name: &str,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let workspace = self
            .project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
            .ok_or_else(|| format!("no workspace {branch}"))?;
        let mut state = WorkspaceState::load(&workspace.path);
        state.display_name = name.trim().to_string();
        state
            .save(&workspace.path)
            .map_err(|error| format!("save {}: {error}", workspace.path.display()))?;
        if state.display_name.is_empty() {
            say(out, &format!("{branch}: name cleared"))
        } else {
            say(out, &format!("{branch}: {}", state.display_name))
        }
    }

    fn list_workspaces(&self, out: &mut dyn Write) -> Result<(), String> {
        if self.project.workspaces.is_empty() {
            return say(out, "no workspaces");
        }
        let width = self
            .project
            .workspaces
            .iter()
            .map(|workspace| workspace.branch.len())
            .max()
            .unwrap_or(0);
        for workspace in &self.project.workspaces {
            let repos: Vec<&str> = workspace
                .repos
                .iter()
                .map(|repo| repo.name.as_str())
                .collect();
            let mut line = format!("{:width$}  {}", workspace.branch, repos.join(", "));
            let name = WorkspaceState::load(&workspace.path).display_name;
            if !name.is_empty() {
                line.push_str(&format!("  \"{name}\""));
            }
            let running = self.running_services(&workspace.branch, workspace.is_main);
            if running > 0 {
                line.push_str(&format!("  {running} running"));
            }
            if let Some(active) = pom_workspace::active_run(&self.state, &workspace.branch) {
                line.push_str(&format!("  in progress: {active}"));
            } else if let Some(failed) = pom_workspace::failed_run(&self.state, &workspace.branch) {
                let verb = match failed.operation {
                    pom_workspace::Operation::Delete => "delete",
                    _ => "create",
                };
                line.push_str(&format!(
                    "  {verb} stopped at stage {} - resume: pom ws {verb} {} --from-stage {}",
                    failed.failed_stage + 1,
                    workspace.branch,
                    failed.failed_stage + 1
                ));
            }
            say(out, &line)?;
        }
        Ok(())
    }

    fn running_services(&self, branch: &str, is_main: bool) -> usize {
        self.config
            .repos
            .iter()
            .flat_map(|(repo, dir)| dir.services.keys().map(move |service| (repo, service)))
            .filter(|(repo, service)| {
                self.runner.is_running(&ServiceTarget {
                    branch: branch.to_string(),
                    is_main,
                    repo: repo.to_string(),
                    service: service.to_string(),
                })
            })
            .count()
    }
}

fn warned(outcome: &Outcome) -> String {
    match outcome.warnings.len() {
        0 => String::new(),
        1 => " with 1 warning".into(),
        count => format!(" with {count} warnings"),
    }
}

fn retry_hint(error: &PipelineError, command: &str) -> String {
    format!(
        "stage {} ({}) failed: {}\nretry from there: {command} --from-stage {}",
        error.stage + 1,
        error.label,
        error.message,
        error.stage + 1
    )
}

/// Runs a pipeline on its own thread and prints its events here as they come.
fn stream<T: Send>(
    out: &mut dyn Write,
    work: impl FnOnce(EventSink<'_>) -> T + Send,
) -> Result<T, String> {
    let (sender, receiver) = mpsc::channel::<Event>();
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            let sink = move |event: Event| {
                if let Err(error) = sender.send(event) {
                    eprintln!("pom: progress lost: {error}");
                }
            };
            work(&sink)
        });
        let mut labels: Vec<&'static str> = Vec::new();
        let mut printed = Ok(());
        for event in receiver {
            if printed.is_ok() {
                printed = print_event(out, &mut labels, event);
            }
        }
        let result = worker
            .join()
            .map_err(|_| "the pipeline crashed".to_string())?;
        printed.map(|()| result)
    })
}

fn print_event(
    out: &mut dyn Write,
    labels: &mut Vec<&'static str>,
    event: Event,
) -> Result<(), String> {
    let label = |index: usize| labels.get(index).copied().unwrap_or("");
    match event {
        Event::Started { stages, .. } => {
            *labels = stages;
            Ok(())
        }
        Event::StageStarted { index } => say(
            out,
            &format!(">>> [{}/{}] {}", index + 1, labels.len(), label(index)),
        ),
        Event::Progress { detail, .. } => say(out, &format!("    {detail}")),
        Event::Warning { detail, .. } => {
            let indented = detail.replace('\n', "\n      ");
            say(out, &format!("    warning: {indented}"))
        }
        Event::StageCompleted { .. } => say(out, "    done"),
        Event::StageSkipped { index } => say(out, &format!("    skipped: {}", label(index))),
        Event::Completed { .. } | Event::Failed { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<WorkspaceCommand, String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        parse(&words, false)
    }

    #[test]
    fn workspace_commands_parse() {
        assert_eq!(
            parsed("create feat-x --repos api,web --env=staging --no-seed --from-stage 4"),
            Ok(WorkspaceCommand::Create(CreateRequest {
                branch: "feat-x".into(),
                repos: vec!["api".into(), "web".into()],
                environment: "staging".into(),
                skip_seed: true,
                from_stage: 3,
            }))
        );
        assert_eq!(
            parsed("delete feat-x"),
            Ok(WorkspaceCommand::Delete(DeleteRequest {
                branch: "feat-x".into(),
                from_stage: 0,
            }))
        );
        assert_eq!(
            parsed("rename feat-x Login"),
            Ok(WorkspaceCommand::Rename {
                branch: "feat-x".into(),
                name: "Login".into(),
            })
        );
        assert_eq!(parsed("list"), Ok(WorkspaceCommand::List));
        assert_eq!(
            parse(&["--no-seed"], true),
            Ok(WorkspaceCommand::PrepareMain(PrepareRequest {
                skip_seed: true
            }))
        );
        for bad in [
            "",
            "create",
            "create a b",
            "create a --from-stage 0",
            "create a --repos",
            "create a --repos api:feat",
            "delete a --env x",
            "list extra",
            "frob",
        ] {
            assert!(parsed(bad).is_err(), "{bad:?}");
        }
    }
}
