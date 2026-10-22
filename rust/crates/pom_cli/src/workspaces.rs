//! `pom ws create|delete|rename|list|gc` and `pom prepare-main`.

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
    Rename {
        branch: String,
        name: String,
    },
    List {
        json: bool,
    },
    Gc(crate::gc::GcArgs),
    PrepareMain(PrepareRequest),
    Sleep {
        branch: String,
        force: bool,
        json: bool,
    },
    Wake {
        branch: String,
        json: bool,
    },
}

/// Parses what follows `ws` (or `prepare-main` when `prepare` is set).
pub(crate) fn parse(words: &[&str], prepare: bool) -> Result<WorkspaceCommand, String> {
    let mut positional: Vec<&str> = Vec::new();
    let mut flags: Vec<(&str, Option<&str>)> = Vec::new();
    let mut iter = words.iter();
    while let Some(word) = iter.next() {
        if *word == "-o" {
            flags.push(("output", Some(*iter.next().ok_or("-o needs a value")?)));
            continue;
        }
        let Some(flag) = word.strip_prefix("--") else {
            positional.push(word);
            continue;
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (flag, None),
        };
        let takes_value = matches!(name, "repos" | "env" | "from-stage" | "output");
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
        .ok_or("ws needs create, delete, rename, list, gc, sleep or wake")?;
    match (*action, rest) {
        ("create", [branch]) => {
            allow(&["repos", "env", "no-seed", "fresh-db", "from-stage"])?;
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
                fresh_databases: has("fresh-db"),
                from_stage: from_stage()?,
                repo_branches: Default::default(),
            }))
        }
        ("delete", [branch]) => {
            allow(&["from-stage"])?;
            Ok(WorkspaceCommand::Delete(DeleteRequest {
                branch: branch.to_string(),
                from_stage: from_stage()?,
                ..DeleteRequest::default()
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
        ("list" | "ls", []) => {
            allow(&["json", "output"])?;
            let json = match value("output") {
                None | Some("table") => has("json"),
                Some("json") => true,
                Some(other) => {
                    return Err(format!("unknown output format {other} (table or json)"))
                }
            };
            Ok(WorkspaceCommand::List { json })
        }
        ("gc", []) => {
            allow(&["merged", "closed", "yes", "dry-run", "json", "force"])?;
            Ok(WorkspaceCommand::Gc(crate::gc::GcArgs {
                merged: has("merged"),
                closed: has("closed"),
                yes: has("yes"),
                dry_run: has("dry-run"),
                json: has("json"),
                force: has("force"),
            }))
        }
        ("sleep", [branch]) => {
            allow(&["force", "json"])?;
            Ok(WorkspaceCommand::Sleep {
                branch: branch.to_string(),
                force: has("force"),
                json: has("json"),
            })
        }
        ("wake", [branch]) => {
            allow(&["json"])?;
            Ok(WorkspaceCommand::Wake {
                branch: branch.to_string(),
                json: has("json"),
            })
        }
        ("sleep" | "wake", _) => Err(format!("ws {action} takes one branch")),
        ("gc", _) => Err("ws gc takes no arguments".into()),
        ("create" | "delete", _) => Err(format!("ws {action} takes one branch")),
        ("rename", _) => Err("ws rename takes a branch and an optional name".into()),
        ("list" | "ls", _) => Err("ws ls takes no arguments".into()),
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
            WorkspaceCommand::List { json: false } => self.list_workspaces(out),
            WorkspaceCommand::List { json: true } => self.list_workspaces_json(out),
            WorkspaceCommand::Gc(args) => self.gc(args, out),
            WorkspaceCommand::PrepareMain(request) => self.prepare_main(request, out),
            WorkspaceCommand::Sleep {
                branch,
                force,
                json,
            } => self.sleep_workspace(branch, *force, *json, out),
            WorkspaceCommand::Wake { branch, json } => self.wake_workspace(branch, *json, out),
        }
    }

    pub(crate) fn context(&self) -> WorkspaceContext<'_> {
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
            let state = WorkspaceState::load(&workspace.path);
            if !state.display_name.is_empty() {
                line.push_str(&format!("  \"{}\"", state.display_name));
            }
            if !state.state.is_running() {
                line.push_str(&format!("  {}", state.state.name()));
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
                    pom_workspace::Operation::Import => "import",
                    _ => "create",
                };
                if verb == "import" {
                    line.push_str(&format!(
                        "  import stopped at stage {} - resume: run the same pom ws import again",
                        failed.failed_stage + 1
                    ));
                } else {
                    line.push_str(&format!(
                        "  {verb} stopped at stage {} - resume: pom ws {verb} {} --from-stage {}",
                        failed.failed_stage + 1,
                        workspace.branch,
                        failed.failed_stage + 1
                    ));
                }
            }
            say(out, &line)?;
        }
        Ok(())
    }

    /// `ws ls --json`: each workspace with its folder, repos, display name, running services and any
    /// create/delete still in progress or stopped part-way.
    fn list_workspaces_json(&self, out: &mut dyn Write) -> Result<(), String> {
        let list =
            pom_remote::workspace_list(&self.project, &self.config, &self.runner, &self.state);
        say(
            out,
            &serde_json::to_string(&list).map_err(|error| error.to_string())?,
        )
    }

    fn find_workspace(&self, branch: &str) -> Result<&pom_layout::Workspace, String> {
        self.project
            .workspaces
            .iter()
            .find(|workspace| workspace.branch == branch)
            .ok_or_else(|| format!("no workspace {branch}"))
    }

    fn sleep_workspace(
        &self,
        branch: &str,
        force: bool,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let workspace = self.find_workspace(branch)?;
        let context = self.context();
        let agents = pom_workspace::DriveAgents::new(&context);
        let report = pom_workspace::sleep(&context, &agents, workspace, force)
            .map_err(|error| error.to_string())?;
        if json {
            let document = serde_json::json!({
                "schema": "pom.ws_sleep/v1",
                "workspace": branch,
                "state": "sleeping",
                "already": report.already,
                "stopped_services": report.services,
                "stopped_agents": report.agents,
            });
            return say(out, &document.to_string());
        }
        if report.already {
            return say(out, &format!("{branch} is already asleep"));
        }
        say(
            out,
            &format!(
                "{branch} is asleep: stopped {} service(s) and {} agent session(s); its databases, worktrees and ports are kept (`pom ws wake {branch}`)",
                report.services.len(),
                report.agents.len()
            ),
        )
    }

    fn wake_workspace(&self, branch: &str, json: bool, out: &mut dyn Write) -> Result<(), String> {
        let workspace = self.find_workspace(branch)?;
        let context = self.context();
        let agents = pom_workspace::DriveAgents::new(&context);
        let settings = pom_workspace::PowerSettings::from_settings_file();
        let (report, made_room) =
            match pom_workspace::wake_with_room(&context, &agents, workspace, &settings)? {
                pom_workspace::WakeOutcome::Woke { report, made_room } => (report, made_room),
                pom_workspace::WakeOutcome::Queued { running } => {
                    return queued(out, branch, running, settings.max_running, json);
                }
            };
        if let Some(slept) = made_room.as_ref().filter(|_| !json) {
            say(
                out,
                &format!("put {slept} to sleep to make room (workspaces.max_running)"),
            )?;
        }
        if json {
            let document = serde_json::json!({
                "schema": "pom.ws_wake/v1",
                "workspace": branch,
                "state": "running",
                "already": report.already,
                "started_services": report.started,
                "started_agents": report.agents,
                "failed": report.failed,
                "made_room": made_room,
            });
            say(out, &document.to_string())?;
        } else if report.already {
            say(out, &format!("{branch} is not asleep"))?;
        } else {
            say(
                out,
                &format!(
                    "{branch} is awake: started {} service(s) and {} agent session(s)",
                    report.started.len(),
                    report.agents.len()
                ),
            )?;
            for failure in &report.failed {
                say(out, &format!("  could not start {failure}"))?;
            }
        }
        if report.failed.is_empty() {
            Ok(())
        } else {
            Err(format!("{} could not start", report.failed.len()))
        }
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

/// A workspace that waits for a slot under `workspaces.max_running`.
pub(crate) fn queued(
    out: &mut dyn Write,
    branch: &str,
    running: usize,
    max_running: usize,
    json: bool,
) -> Result<(), String> {
    if json {
        let document = serde_json::json!({
            "schema": "pom.ws_wake/v1",
            "workspace": branch,
            "state": "queued",
            "running": running,
            "max_running": max_running,
        });
        return say(out, &document.to_string());
    }
    say(
        out,
        &format!(
            "{branch} is queued: {running} workspaces run with agents at work and workspaces.max_running is {max_running}; it starts once one sleeps"
        ),
    )
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
pub(crate) fn stream<T: Send>(
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
            parsed(
                "create feat-x --repos api,web --env=staging --no-seed --fresh-db --from-stage 4"
            ),
            Ok(WorkspaceCommand::Create(CreateRequest {
                branch: "feat-x".into(),
                repos: vec!["api".into(), "web".into()],
                environment: "staging".into(),
                skip_seed: true,
                fresh_databases: true,
                from_stage: 3,
                repo_branches: Default::default(),
            }))
        );
        assert_eq!(
            parsed("delete feat-x"),
            Ok(WorkspaceCommand::Delete(DeleteRequest {
                branch: "feat-x".into(),
                from_stage: 0,
                ..DeleteRequest::default()
            }))
        );
        assert_eq!(
            parsed("rename feat-x Login"),
            Ok(WorkspaceCommand::Rename {
                branch: "feat-x".into(),
                name: "Login".into(),
            })
        );
        assert_eq!(parsed("list"), Ok(WorkspaceCommand::List { json: false }));
        assert_eq!(
            parsed("ls --json"),
            Ok(WorkspaceCommand::List { json: true })
        );
        assert_eq!(
            parsed("ls -o json"),
            Ok(WorkspaceCommand::List { json: true })
        );
        assert!(parsed("ls -o yaml").is_err());
        assert_eq!(
            parsed("gc --closed --yes --json"),
            Ok(WorkspaceCommand::Gc(crate::gc::GcArgs {
                closed: true,
                yes: true,
                json: true,
                ..Default::default()
            }))
        );
        assert!(parsed("gc feat").is_err());
        assert_eq!(
            parsed("sleep feat-x --force --json"),
            Ok(WorkspaceCommand::Sleep {
                branch: "feat-x".into(),
                force: true,
                json: true,
            })
        );
        assert_eq!(
            parsed("wake feat-x"),
            Ok(WorkspaceCommand::Wake {
                branch: "feat-x".into(),
                json: false,
            })
        );
        assert!(parsed("wake").is_err());
        assert!(parsed("wake a --force").is_err());
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
