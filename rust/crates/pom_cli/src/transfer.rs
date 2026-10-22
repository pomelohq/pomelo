//! `pom ws export|import` and `pom project export|import`: a workspace or a project moved to another machine
//! as one archive, written to a file or stdout, or streamed over ssh into `pom ... import -` on a host.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use pom_paths::StateDir;
use pom_remote::{Hosts, Tty, EXIT_UNREACHABLE};
use pom_transfer::{ExportOptions, ImportOptions, Manifest, Staging, OUTPUT_SCHEMA};

use crate::args::Args;
use crate::{report, say, Command, Invocation, Session};

pub(crate) const PASSPHRASE_ENV: &str = "POM_BUNDLE_PASSPHRASE";
/// Tells `import -` that the first line of stdin is the passphrase, ahead of the archive (`--to` sends a one-time
/// one that way, so no secret is ever an argument).
const PASSPHRASE_STDIN: &str = "--passphrase-stdin";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// A file, or `-` for stdout; none is the default name here.
    File(Option<String>),
    /// A host of `hosts.json`, its pom importing what is streamed in.
    Host(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TransferCommand {
    Export {
        branch: String,
        target: Target,
        databases: bool,
        json: bool,
    },
    Import {
        /// `-` is stdin.
        input: String,
        project: Option<String>,
        branch: Option<String>,
        passphrase_stdin: bool,
        json: bool,
    },
    ProjectExport {
        target: Target,
        json: bool,
    },
    ProjectImport {
        input: String,
        directory: Option<String>,
        passphrase_stdin: bool,
        json: bool,
    },
}

fn target(args: &Args, json: bool) -> Result<Target, String> {
    let output = args
        .value(&["-o", "--output"])
        .filter(|path| !path.is_empty());
    let host = args.value(&["--to"]).filter(|host| !host.is_empty());
    match (output, host) {
        (Some(_), Some(_)) => Err("-o and --to exclude each other".into()),
        (_, Some(host)) => Ok(Target::Host(host)),
        (Some(path), None) if path == "-" && json => {
            Err("--json prints the report on stdout; write the archive to a file with -o".into())
        }
        (output, None) => Ok(Target::File(output)),
    }
}

fn input(args: &Args, usage: &str) -> Result<(String, bool), String> {
    let [input] = args.positional.as_slice() else {
        return Err(usage.into());
    };
    let passphrase_stdin = args.has(PASSPHRASE_STDIN);
    if passphrase_stdin && input != "-" {
        return Err(format!(
            "{PASSPHRASE_STDIN} goes with reading the archive from stdin (-)"
        ));
    }
    Ok((input.clone(), passphrase_stdin))
}

/// Parses what follows `ws` (or `project` when `project` is set) for export and import.
pub(crate) fn parse(words: &[&str], project: bool) -> Result<TransferCommand, String> {
    let (verb, rest) = words.split_first().ok_or("missing export or import")?;
    match (*verb, project) {
        ("export", false) => {
            let args = Args::parse(rest, &["-o", "--output", "--to"])?;
            args.allow(&["-o", "--output", "--to", "--no-db", "--json"])?;
            let [branch] = args.positional.as_slice() else {
                return Err(
                    "usage: pom ws export <branch> [-o file.pomws | --to host] [--no-db] [--json]"
                        .into(),
                );
            };
            let json = args.has("--json");
            Ok(TransferCommand::Export {
                branch: branch.clone(),
                target: target(&args, json)?,
                databases: !args.has("--no-db"),
                json,
            })
        }
        ("import", false) => {
            let args = Args::parse(rest, &["--project", "--branch"])?;
            args.allow(&["--project", "--branch", "--json", PASSPHRASE_STDIN])?;
            let (input, passphrase_stdin) = input(
                &args,
                "usage: pom ws import <file.pomws | -> [--project name] [--branch new] [--json]",
            )?;
            Ok(TransferCommand::Import {
                input,
                project: args.value(&["--project"]).filter(|name| !name.is_empty()),
                branch: args.value(&["--branch"]).filter(|name| !name.is_empty()),
                passphrase_stdin,
                json: args.has("--json"),
            })
        }
        ("export", true) => {
            let args = Args::parse(rest, &["-o", "--output", "--to"])?;
            args.allow(&["-o", "--output", "--to", "--json"])?;
            args.at_most(0, "project export")?;
            let json = args.has("--json");
            Ok(TransferCommand::ProjectExport {
                target: target(&args, json)?,
                json,
            })
        }
        ("import", true) => {
            let args = Args::parse(rest, &["--dir"])?;
            args.allow(&["--dir", "--json", PASSPHRASE_STDIN])?;
            let (input, passphrase_stdin) = input(
                &args,
                "usage: pom project import <file.pomproj | -> [--dir path] [--json]",
            )?;
            Ok(TransferCommand::ProjectImport {
                input,
                directory: args.value(&["--dir"]).filter(|dir| !dir.is_empty()),
                passphrase_stdin,
                json: args.has("--json"),
            })
        }
        (other, _) => Err(format!("unknown command {other}")),
    }
}

/// Why a transfer failed, with the exit code to leave with.
pub(crate) struct Failure {
    code: i32,
    message: String,
}

impl From<String> for Failure {
    fn from(message: String) -> Failure {
        Failure { code: 1, message }
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
            target,
            databases,
            json,
        } => Session::open(invocation, cwd)
            .map_err(Failure::from)
            .and_then(|session| export(&session, branch, target, *databases, *json, cwd, out, err)),
        TransferCommand::Import {
            input,
            project,
            branch,
            passphrase_stdin,
            json,
        } => import(
            invocation,
            cwd,
            input,
            project.as_deref(),
            branch.as_deref(),
            *passphrase_stdin,
            *json,
            out,
            err,
        )
        .map_err(Failure::from),
        TransferCommand::ProjectExport { target, json } => {
            project_export(invocation, cwd, target, *json, out, err)
        }
        TransferCommand::ProjectImport {
            input,
            directory,
            passphrase_stdin,
            json,
        } => project_import(
            cwd,
            input,
            directory.as_deref(),
            *passphrase_stdin,
            *json,
            out,
            err,
        )
        .map_err(Failure::from),
    };
    match result {
        Ok(()) => 0,
        Err(failure) => {
            report(err, &format!("error: {}", failure.message));
            failure.code
        }
    }
}

/// Where an archive went, for the report.
enum Written {
    File(String),
    Stdout,
    Host {
        name: String,
        document: serde_json::Value,
    },
}

impl Written {
    fn label(&self) -> String {
        match self {
            Written::File(path) => path.clone(),
            Written::Stdout => "-".into(),
            Written::Host { name, .. } => format!("host {name}"),
        }
    }

    fn remote(&self) -> serde_json::Value {
        match self {
            Written::Host { document, .. } => document.clone(),
            _ => serde_json::Value::Null,
        }
    }
}

/// Runs `write` against the chosen target. For a host, its `pom <remote...> - --passphrase-stdin --json` reads
/// the archive from ssh's stdin, and `write` gets the one-time passphrase that goes ahead of it.
fn write_archive<T>(
    target: &Target,
    default_name: &str,
    cwd: &Path,
    remote: &[&str],
    out: &mut dyn Write,
    write: impl FnOnce(Option<&str>, &mut dyn Write) -> Result<T, String>,
) -> Result<(T, Written), Failure> {
    match target {
        Target::File(Some(path)) if path == "-" => {
            let value = write(None, out)?;
            Ok((value, Written::Stdout))
        }
        Target::File(path) => {
            let path = cwd.join(path.as_deref().unwrap_or(default_name));
            let partial = path.with_extension("partial");
            let mut file = create_private(&partial)?;
            match write(None, &mut file) {
                Ok(value) => {
                    std::fs::rename(&partial, &path)
                        .map_err(|error| format!("write {}: {error}", path.display()))?;
                    Ok((value, Written::File(path.to_string_lossy().into_owned())))
                }
                Err(error) => {
                    if let Err(remove) = std::fs::remove_file(&partial) {
                        eprintln!("could not remove {}: {remove}", partial.display());
                    }
                    Err(error.into())
                }
            }
        }
        Target::Host(name) => stream_to_host(name, remote, write),
    }
}

fn stream_to_host<T>(
    name: &str,
    remote: &[&str],
    write: impl FnOnce(Option<&str>, &mut dyn Write) -> Result<T, String>,
) -> Result<(T, Written), Failure> {
    let host = Hosts::default_path()
        .ok_or_else(|| "no home directory for ~/.config/pomelo".to_string())
        .and_then(|path| Hosts::load(&path)?.get(name))?;
    let mut args: Vec<String> = remote.iter().map(|arg| arg.to_string()).collect();
    args.extend(["-", PASSPHRASE_STDIN, "--json"].map(str::to_string));
    let mut child = pom_remote::ssh_command(&host, &args, Tty::Batch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure {
            code: EXIT_UNREACHABLE,
            message: format!("could not run ssh: {error}"),
        })?;
    let stdout = child.stdout.take();
    let collected = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stdout) = stdout {
            if let Err(error) = stdout.read_to_end(&mut bytes) {
                eprintln!("ssh: {error}");
            }
        }
        bytes
    });
    let stderr = child.stderr.take();
    let relayed = std::thread::spawn(move || {
        let mut last = String::new();
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("  remote: {line}");
                if !line.trim().is_empty() {
                    last = line;
                }
            }
        }
        last
    });
    let secret = one_time_passphrase()?;
    let written = match child.stdin.take() {
        Some(mut stdin) => writeln!(stdin, "{secret}")
            .map_err(|error| format!("ssh: {error}"))
            .and_then(|()| write(Some(&secret), &mut stdin)),
        None => Err("ssh has no stdin".to_string()),
    };
    let status = child.wait().map_err(|error| format!("ssh: {error}"))?;
    let stdout = collected.join().unwrap_or_default();
    let last = relayed.join().unwrap_or_default();
    match status.code() {
        Some(0) => {
            let value = written?;
            let document = serde_json::from_slice(&stdout).map_err(|error| {
                format!(
                    "{name} answered without its report ({error}): {}",
                    String::from_utf8_lossy(&stdout).trim()
                )
            })?;
            Ok((
                value,
                Written::Host {
                    name: name.to_string(),
                    document,
                },
            ))
        }
        Some(255) => Err(Failure {
            code: EXIT_UNREACHABLE,
            message: format!(
                "could not reach host {name} over ssh ({last}); check it with: pom host test {name}"
            ),
        }),
        Some(code) => Err(Failure {
            code,
            message: format!("the import on {name} failed (exit {code}); its output is above"),
        }),
        None => Err("ssh was killed".to_string().into()),
    }
}

/// Seals the secrets for one trip over ssh; the archive never lands on disk with it.
fn one_time_passphrase() -> Result<String, String> {
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(|error| format!("random passphrase: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[allow(clippy::too_many_arguments)]
fn export(
    session: &Session,
    branch: &str,
    target: &Target,
    databases: bool,
    json: bool,
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let context = session.context();
    let options = ExportOptions { databases };
    let default_name = format!(
        "{}-{}.{}",
        session.config.session,
        branch.replace('/', "-"),
        pom_transfer::EXTENSION
    );
    let (report, written) = {
        let err = Mutex::new(&mut *err);
        let progress = |line: &str| say_to(&err, &format!("    {line}"));
        write_archive(
            target,
            &default_name,
            cwd,
            &["ws", "import"],
            out,
            |secret, sink| {
                let passphrase = || match secret {
                    Some(secret) => Ok(secret.to_string()),
                    None => passphrase("passphrase to seal the secrets with", true),
                };
                pom_transfer::export(&context, branch, options, &passphrase, &progress, sink)
            },
        )?
    };
    for warning in &report.warnings {
        crate::report(err, &format!("warning: {warning}"));
    }
    let manifest = &report.manifest;
    if json {
        let document = serde_json::json!({
            "schema": OUTPUT_SCHEMA,
            "action": "export",
            "project": manifest.project,
            "branch": manifest.branch,
            "output": written.label(),
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
            "remote": written.remote(),
        });
        return Ok(say(out, &document.to_string())?);
    }
    let mut lines = vec![format!(
        "Exported {} ({}) to {}",
        manifest.branch,
        size(report.bytes),
        written.label()
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
        lines.push(format!("  {} secret(s), sealed", manifest.secrets.len()));
    }
    match &written {
        // An archive on stdout leaves only stderr for the report.
        Written::Stdout => {
            for line in lines {
                crate::report(err, &line);
            }
            return Ok(());
        }
        Written::File(path) => lines.push(format!(
            "  open it on the other machine: pom ws import {path}"
        )),
        Written::Host { name, document } => lines.push(format!(
            "  imported on {name}: {}",
            document["workspace"].as_str().unwrap_or_default()
        )),
    }
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

/// The archive to read, and the passphrase sent ahead of it on stdin.
fn open_input(
    input: &str,
    cwd: &Path,
    passphrase_stdin: bool,
) -> Result<(Box<dyn Read>, Option<String>), String> {
    if input != "-" {
        let path = cwd.join(input);
        let file = std::fs::File::open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        return Ok((Box::new(file), None));
    }
    let mut reader = BufReader::new(std::io::stdin());
    let mut sent = None;
    if passphrase_stdin {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| format!("read the passphrase: {error}"))?;
        sent = Some(line.trim_end_matches(['\r', '\n']).to_string());
    }
    Ok((Box::new(reader), sent))
}

fn passphrase_or(sent: &Option<String>, what: &str) -> Result<String, String> {
    match sent.as_ref().filter(|sent| !sent.is_empty()) {
        Some(sent) => Ok(sent.clone()),
        None => passphrase(what, false),
    }
}

#[allow(clippy::too_many_arguments)]
fn import(
    invocation: &Invocation,
    cwd: &Path,
    input: &str,
    project: Option<&str>,
    branch: Option<&str>,
    passphrase_stdin: bool,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), String> {
    let state = StateDir::from_env();
    let staging = Staging::new(&state, "import")?;
    let (mut reader, sent) = open_input(input, cwd, passphrase_stdin)?;
    let manifest = pom_transfer::unpack(&mut reader, staging.path())?;
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
        passphrase_or(&sent, "passphrase the secrets were sealed with")
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

fn project_export(
    invocation: &Invocation,
    cwd: &Path,
    target: &Target,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), Failure> {
    let config_path = crate::find_config(invocation.config.as_deref(), cwd)?;
    let state = StateDir::from_env();
    let session = pom_config::Config::load(&config_path)
        .map_err(|error| error.to_string())?
        .session;
    let default_name = format!("{session}.{}", pom_transfer::PROJECT_EXTENSION);
    let (export, written) = write_archive(
        target,
        &default_name,
        cwd,
        &["project", "import"],
        out,
        |secret, sink| {
            let passphrase = || match secret {
                Some(secret) => Ok(secret.to_string()),
                None => passphrase("passphrase to seal the secrets with", true),
            };
            pom_transfer::export_project(&config_path, &state, &passphrase, sink)
        },
    )?;
    for warning in &export.warnings {
        report(err, &format!("warning: {warning}"));
    }
    let manifest = &export.manifest;
    if json {
        let document = serde_json::json!({
            "schema": OUTPUT_SCHEMA,
            "action": "project-export",
            "project": manifest.project,
            "output": written.label(),
            "repos": manifest.repos.iter().map(|repo| serde_json::json!({
                "name": repo.name,
                "remote_url": repo.remote_url,
                "default_branch": repo.default_branch,
            })).collect::<Vec<_>>(),
            "secrets": manifest.secrets,
            "warnings": export.warnings,
            "remote": written.remote(),
        });
        return Ok(say(out, &document.to_string())?);
    }
    let mut lines = vec![format!(
        "Exported project {} ({} repo(s), {} secret(s)) to {}",
        manifest.project,
        manifest.repos.len(),
        manifest.secrets.len(),
        written.label()
    )];
    match &written {
        Written::Stdout => {
            for line in lines {
                report(err, &line);
            }
            return Ok(());
        }
        Written::File(path) => lines.push(format!(
            "  set it up on the other machine: pom project import {path}"
        )),
        Written::Host { name, document } => lines.push(format!(
            "  set up on {name}: {}",
            document["root"].as_str().unwrap_or_default()
        )),
    }
    for line in lines {
        say(out, &line)?;
    }
    Ok(())
}

fn project_import(
    cwd: &Path,
    input: &str,
    directory: Option<&str>,
    passphrase_stdin: bool,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), String> {
    let state = StateDir::from_env();
    let (mut reader, sent) = open_input(input, cwd, passphrase_stdin)?;
    let directory = directory.map(|dir| cwd.join(dir));
    let err = Mutex::new(err);
    let imported = pom_transfer::import_project(
        &mut reader,
        &state,
        directory.as_deref(),
        &|| passphrase_or(&sent, "passphrase the project was sealed with"),
        &|line| say_to(&err, &format!("    {line}")),
    )?;
    for warning in &imported.warnings {
        say_to(&err, &format!("warning: {warning}"));
    }
    if json {
        let document = serde_json::json!({
            "schema": OUTPUT_SCHEMA,
            "action": "project-import",
            "project": imported.manifest.project,
            "root": imported.root,
            "cloned": imported.cloned,
            "secrets_stored": imported.secrets_stored,
            "warnings": imported.warnings,
        });
        return say(out, &document.to_string());
    }
    say(
        out,
        &format!(
            "Project {} set up in {} ({} repo(s) cloned, {} secret(s) stored)",
            imported.manifest.project,
            imported.root.display(),
            imported.cloned.len(),
            imported.secrets_stored
        ),
    )
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

fn say_to<W: Write + ?Sized>(err: &Mutex<&mut W>, line: &str) {
    match err.lock() {
        Ok(mut err) => {
            if let Err(error) = writeln!(err, "{line}") {
                eprintln!("{line} ({error})");
            }
        }
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

    fn parsed(line: &str, project: bool) -> Result<TransferCommand, String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        parse(&words, project)
    }

    #[test]
    fn transfer_commands_parse() {
        assert_eq!(
            parsed("export feat-x -o out.pomws --no-db --json", false),
            Ok(TransferCommand::Export {
                branch: "feat-x".into(),
                target: Target::File(Some("out.pomws".into())),
                databases: false,
                json: true,
            })
        );
        assert_eq!(
            parsed("export feat-x --to dev", false),
            Ok(TransferCommand::Export {
                branch: "feat-x".into(),
                target: Target::Host("dev".into()),
                databases: true,
                json: false,
            })
        );
        assert_eq!(
            parsed(
                "import - --project shop --branch feat-y --passphrase-stdin",
                false
            ),
            Ok(TransferCommand::Import {
                input: "-".into(),
                project: Some("shop".into()),
                branch: Some("feat-y".into()),
                passphrase_stdin: true,
                json: false,
            })
        );
        assert_eq!(
            parsed("export --to dev --json", true),
            Ok(TransferCommand::ProjectExport {
                target: Target::Host("dev".into()),
                json: true,
            })
        );
        assert_eq!(
            parsed("import shop.pomproj --dir here", true),
            Ok(TransferCommand::ProjectImport {
                input: "shop.pomproj".into(),
                directory: Some("here".into()),
                passphrase_stdin: false,
                json: false,
            })
        );
        for (bad, project) in [
            ("export", false),
            ("export a b", false),
            ("export a -o - --json", false),
            ("export a -o x --to dev", false),
            ("import", false),
            ("import a --passphrase x", false),
            ("import a.pomws --passphrase-stdin", false),
            ("export extra", true),
        ] {
            assert!(parsed(bad, project).is_err(), "{bad:?}");
        }
    }
}
