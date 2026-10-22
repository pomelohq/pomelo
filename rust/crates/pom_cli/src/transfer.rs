//! `pom ws export` and `pom ws import`: a workspace moved to another machine as one `.pomws` archive.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pom_paths::StateDir;
use pom_transfer::{ExportOptions, ImportOptions, Manifest, Staging, OUTPUT_SCHEMA};

use crate::args::Args;
use crate::{report, say, Command, Invocation, Session};

pub(crate) const PASSPHRASE_ENV: &str = "POM_BUNDLE_PASSPHRASE";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TransferCommand {
    Export {
        branch: String,
        /// `-` is stdout; none is `<project>-<branch>.pomws` here.
        output: Option<String>,
        databases: bool,
        json: bool,
    },
    Import {
        /// `-` is stdin.
        input: String,
        project: Option<String>,
        branch: Option<String>,
        json: bool,
    },
}

/// Parses what follows `ws` when it is `export` or `import`.
pub(crate) fn parse(words: &[&str]) -> Result<TransferCommand, String> {
    let (verb, rest) = words.split_first().ok_or("ws needs export or import")?;
    match *verb {
        "export" => {
            let args = Args::parse(rest, &["-o", "--output"])?;
            args.allow(&["-o", "--output", "--no-db", "--json"])?;
            let [branch] = args.positional.as_slice() else {
                return Err(
                    "usage: pom ws export <branch> [-o file.pomws] [--no-db] [--json]".into(),
                );
            };
            let output = args
                .value(&["-o", "--output"])
                .filter(|path| !path.is_empty());
            let json = args.has("--json");
            if json && output.as_deref() == Some("-") {
                return Err(
                    "--json prints the report on stdout; write the archive to a file with -o"
                        .into(),
                );
            }
            Ok(TransferCommand::Export {
                branch: branch.clone(),
                output,
                databases: !args.has("--no-db"),
                json,
            })
        }
        "import" => {
            let args = Args::parse(rest, &["--project", "--branch"])?;
            args.allow(&["--project", "--branch", "--json"])?;
            let [input] = args.positional.as_slice() else {
                return Err(
                    "usage: pom ws import <file.pomws | -> [--project name] [--branch new] [--json]"
                        .into(),
                );
            };
            Ok(TransferCommand::Import {
                input: input.clone(),
                project: args.value(&["--project"]).filter(|name| !name.is_empty()),
                branch: args.value(&["--branch"]).filter(|name| !name.is_empty()),
                json: args.has("--json"),
            })
        }
        other => Err(format!("unknown ws command {other}")),
    }
}

pub(crate) fn execute(
    command: &TransferCommand,
    invocation: &Invocation,
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let result = match command {
        TransferCommand::Export {
            branch,
            output,
            databases,
            json,
        } => Session::open(invocation, cwd).and_then(|session| {
            export(
                &session,
                branch,
                output.as_deref(),
                *databases,
                *json,
                cwd,
                out,
                err,
            )
        }),
        TransferCommand::Import {
            input,
            project,
            branch,
            json,
        } => import(
            invocation,
            cwd,
            input,
            project.as_deref(),
            branch.as_deref(),
            *json,
            out,
            err,
        ),
    };
    match result {
        Ok(()) => 0,
        Err(message) => {
            report(err, &format!("error: {message}"));
            1
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn export(
    session: &Session,
    branch: &str,
    output: Option<&str>,
    databases: bool,
    json: bool,
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), String> {
    let context = session.context();
    let err = Mutex::new(err);
    let progress = |line: &str| say_to(&err, &format!("    {line}"));
    let passphrase = || passphrase("passphrase to seal the secrets with", true);
    let options = ExportOptions { databases };
    let (report, target) = match output {
        Some("-") => {
            let report =
                pom_transfer::export(&context, branch, options, &passphrase, &progress, out)?;
            (report, "-".to_string())
        }
        _ => {
            let path = match output {
                Some(path) => cwd.join(path),
                None => cwd.join(format!(
                    "{}-{}.{}",
                    session.config.session,
                    branch.replace('/', "-"),
                    pom_transfer::EXTENSION
                )),
            };
            let partial = path.with_extension("partial");
            let mut file = create_private(&partial)?;
            let exported =
                pom_transfer::export(&context, branch, options, &passphrase, &progress, &mut file);
            let report = match exported {
                Ok(report) => report,
                Err(error) => {
                    if let Err(remove) = std::fs::remove_file(&partial) {
                        say_to(
                            &err,
                            &format!("could not remove {}: {remove}", partial.display()),
                        );
                    }
                    return Err(error);
                }
            };
            std::fs::rename(&partial, &path)
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            (report, path.to_string_lossy().into_owned())
        }
    };
    for warning in &report.warnings {
        say_to(&err, &format!("warning: {warning}"));
    }
    let manifest = &report.manifest;
    if json {
        let document = serde_json::json!({
            "schema": OUTPUT_SCHEMA,
            "action": "export",
            "project": manifest.project,
            "branch": manifest.branch,
            "output": target,
            "bytes": report.bytes,
            "repos": manifest.repos.iter().map(|repo| serde_json::json!({
                "name": repo.name,
                "branch": repo.branch,
                "base": repo.base,
                "head": repo.head,
                "unpushed_commits": repo.unpushed,
                "uncommitted_changes": repo.patch.is_some(),
                "untracked_files": repo.untracked_files,
            })).collect::<Vec<_>>(),
            "databases": manifest.databases.iter().map(|db| serde_json::json!({
                "name": db.name,
                "bytes": db.bytes,
            })).collect::<Vec<_>>(),
            "volumes": manifest.volumes.iter().map(|volume| volume.name.clone()).collect::<Vec<_>>(),
            "secrets": manifest.secrets,
            "warnings": report.warnings,
        });
        return say(out, &document.to_string());
    }
    let mut lines = vec![format!(
        "Exported {} ({}) to {target}",
        manifest.branch,
        size(report.bytes)
    )];
    for repo in &manifest.repos {
        lines.push(format!("  {}: {}", repo.name, repo_summary(repo)));
    }
    if !manifest.databases.is_empty() {
        let names: Vec<&str> = manifest
            .databases
            .iter()
            .map(|db| db.name.as_str())
            .collect();
        lines.push(format!("  databases: {}", names.join(", ")));
    }
    if !manifest.secrets.is_empty() {
        lines.push(format!(
            "  {} secret(s), sealed with the passphrase",
            manifest.secrets.len()
        ));
    }
    // An archive on stdout leaves only stderr for the report.
    if target == "-" {
        for line in lines {
            say_to(&err, &line);
        }
        return Ok(());
    }
    lines.push(format!(
        "  open it on the other machine: pom ws import {target}"
    ));
    for line in lines {
        say(out, &line)?;
    }
    Ok(())
}

fn repo_summary(repo: &pom_transfer::RepoEntry) -> String {
    let mut parts = vec![format!("{} at {}", repo.branch, short(&repo.head))];
    match repo.unpushed {
        0 => {}
        1 => parts.push("1 unpushed commit".into()),
        count => parts.push(format!("{count} unpushed commits")),
    }
    if repo.patch.is_some() {
        parts.push("uncommitted changes".into());
    }
    match repo.untracked_files {
        0 => {}
        1 => parts.push("1 untracked file".into()),
        count => parts.push(format!("{count} untracked files")),
    }
    parts.join(", ")
}

#[allow(clippy::too_many_arguments)]
fn import(
    invocation: &Invocation,
    cwd: &Path,
    input: &str,
    project: Option<&str>,
    branch: Option<&str>,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), String> {
    let state = StateDir::from_env();
    let staging = Staging::new(&state, "import")?;
    let manifest = if input == "-" {
        pom_transfer::unpack(&mut std::io::stdin().lock(), staging.path())?
    } else {
        let path = cwd.join(input);
        let mut file = std::fs::File::open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        pom_transfer::unpack(&mut file, staging.path())?
    };
    let config_path = project_config(
        &state,
        invocation,
        cwd,
        project.unwrap_or(&manifest.project),
    )?;
    let opened = Invocation {
        workspace: None,
        config: Some(config_path),
        command: Command::Help,
    };
    let session = Session::open(&opened, cwd)?;
    let target_branch = branch.unwrap_or(&manifest.branch).to_string();
    let from_stage = match pom_workspace::failed_run(&state, &target_branch) {
        None => 0,
        Some(failed) if failed.operation == pom_workspace::Operation::Import => {
            report(
                err,
                &format!("resuming the import of {target_branch} at stage {}", failed.failed_stage + 1),
            );
            failed.failed_stage
        }
        Some(_) => {
            return Err(format!(
                "a create or delete of {target_branch} stopped part-way; finish it or import with --branch <new>"
            ))
        }
    };
    let options = ImportOptions {
        branch: Some(target_branch.clone()),
        from_stage,
    };
    let context = session.context();
    let plan = pom_transfer::plan_import(&context, &manifest, staging.path(), &options, &|| {
        passphrase("passphrase the secrets were sealed with", false)
    })?;
    for warning in &plan.warnings {
        report(err, &format!("warning: {warning}"));
    }
    let result = crate::workspaces::stream(err, |sink| {
        pom_workspace::import(&context, &plan.request, sink)
    })?;
    let outcome = result.map_err(|error| {
        format!(
            "stage {} ({}) failed: {}\nrun the same import again to resume from there, or `pom ws delete {target_branch}` to remove what it made",
            error.stage + 1,
            error.label,
            error.message
        )
    })?;
    let folder = pom_layout::workspace_folder(&session.project.root, &target_branch);
    let warnings: Vec<String> = plan
        .warnings
        .iter()
        .chain(&outcome.warnings)
        .cloned()
        .collect();
    if json {
        let document = serde_json::json!({
            "schema": OUTPUT_SCHEMA,
            "action": "import",
            "project": session.config.session,
            "branch": target_branch,
            "workspace": folder,
            "from": source_json(&manifest),
            "repos": plan.request.repos.iter().map(|repo| serde_json::json!({
                "name": repo.name,
                "head": repo.head,
            })).collect::<Vec<_>>(),
            "databases": plan.databases.iter().map(|(from, name)| serde_json::json!({
                "from": from,
                "name": name,
            })).collect::<Vec<_>>(),
            "volumes": plan.request.volumes.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>(),
            "secrets": plan.request.secrets.keys().collect::<Vec<_>>(),
            "warnings": warnings,
        });
        return say(out, &document.to_string());
    }
    let warned = match warnings.len() {
        0 => String::new(),
        1 => " with 1 warning".into(),
        count => format!(" with {count} warnings"),
    };
    say(
        out,
        &format!("\nWorkspace {target_branch} imported{warned}"),
    )?;
    say(out, &format!("  cd {}", folder.display()))
}

fn source_json(manifest: &Manifest) -> serde_json::Value {
    serde_json::json!({
        "project": manifest.project,
        "branch": manifest.branch,
        "created_at": manifest.created_at,
        "pom_version": manifest.pom_version,
        "workspace": manifest.source.workspace,
    })
}

/// The project's `pom.yml` here: `-c`, else the registered project of that name, else the one `cwd` is in.
fn project_config(
    state: &StateDir,
    invocation: &Invocation,
    cwd: &Path,
    name: &str,
) -> Result<PathBuf, String> {
    if let Some(path) = &invocation.config {
        return Ok(path.clone());
    }
    let registered = pom_sessions::Projects::load(state)
        .project_dir(name)
        .or_else(|| {
            pom_core::session_entries(state)
                .into_iter()
                .find(|entry| entry.host.is_none() && entry.name == name)
                .map(|entry| entry.path)
        })
        .and_then(|dir| pom_core::config_in(&dir));
    if let Some(path) = registered {
        return Ok(path);
    }
    let here = cwd
        .ancestors()
        .find_map(pom_core::config_in)
        .filter(|path| pom_config::Config::load(path).is_ok_and(|config| config.session == name));
    here.ok_or_else(|| {
        format!(
            "project {name} is not on this machine; bring it over first with `pom project import`, or name another with --project or -c"
        )
    })
}

/// `POM_BUNDLE_PASSPHRASE`, else typed on the terminal (hidden); never an argument, so it stays out of `ps`.
pub(crate) fn passphrase(what: &str, confirm: bool) -> Result<String, String> {
    if let Some(value) = std::env::var(PASSPHRASE_ENV)
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Ok(value);
    }
    let missing =
        || format!("the secrets need a passphrase: set {PASSPHRASE_ENV} or run this on a terminal");
    let mut terminal = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| missing())?;
    let first = prompt(&mut terminal, &format!("{what}: "))?;
    if first.is_empty() {
        return Err(missing());
    }
    if confirm && prompt(&mut terminal, "again: ")? != first {
        return Err("the passphrases differ".into());
    }
    Ok(first)
}

fn prompt(terminal: &mut std::fs::File, text: &str) -> Result<String, String> {
    terminal
        .write_all(text.as_bytes())
        .and_then(|()| terminal.flush())
        .map_err(|error| error.to_string())?;
    set_echo(terminal, false);
    let mut line = String::new();
    let read = std::io::BufReader::new(&*terminal).read_line(&mut line);
    set_echo(terminal, true);
    if let Err(error) = terminal.write_all(b"\n") {
        eprintln!("{error}");
    }
    read.map_err(|error| format!("read passphrase: {error}"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn set_echo(terminal: &std::fs::File, on: bool) {
    let stdin = match terminal.try_clone() {
        Ok(file) => std::process::Stdio::from(file),
        Err(error) => {
            eprintln!("stty: {error}");
            return;
        }
    };
    let result = std::process::Command::new("stty")
        .arg(if on { "echo" } else { "-echo" })
        .stdin(stdin)
        .status();
    if let Err(error) = result {
        eprintln!("stty: {error}");
    }
}

fn create_private(path: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))
}

fn say_to(err: &Mutex<&mut dyn Write>, line: &str) {
    match err.lock() {
        Ok(mut err) => report(&mut **err, line),
        Err(_) => eprintln!("{line}"),
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

fn size(bytes: u64) -> String {
    match bytes {
        0..=1_023 => format!("{bytes} B"),
        1_024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1_024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<TransferCommand, String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        parse(&words)
    }

    #[test]
    fn transfer_commands_parse() {
        assert_eq!(
            parsed("export feat-x -o out.pomws --no-db --json"),
            Ok(TransferCommand::Export {
                branch: "feat-x".into(),
                output: Some("out.pomws".into()),
                databases: false,
                json: true,
            })
        );
        assert_eq!(
            parsed("import - --project shop --branch feat-y"),
            Ok(TransferCommand::Import {
                input: "-".into(),
                project: Some("shop".into()),
                branch: Some("feat-y".into()),
                json: false,
            })
        );
        for bad in [
            "export",
            "export a b",
            "export a -o - --json",
            "import",
            "import a --passphrase x",
        ] {
            assert!(parsed(bad).is_err(), "{bad:?}");
        }
    }
}
