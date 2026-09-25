//! `pom config ...`: the project's config as files.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use pom_paths::StateDir;
use pom_services::ServiceTarget;

use crate::args::Args;
use crate::{say, Session};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfigCommand {
    Path,
    Edit,
    Split {
        dry: bool,
    },
    Normalize {
        dry: bool,
    },
    Explain {
        target: Option<String>,
        branch: Option<String>,
        env: String,
        json: bool,
        show_secrets: bool,
    },
    Export {
        secrets: bool,
        output: Option<PathBuf>,
    },
    Import {
        file: PathBuf,
        config: bool,
        secrets: bool,
    },
}

impl ConfigCommand {
    /// Explaining resolves ports and databases, so it needs the whole workspace session.
    pub(crate) fn needs_session(&self) -> bool {
        matches!(self, ConfigCommand::Explain { .. })
    }
}

pub(crate) fn parse(words: &[&str]) -> Result<ConfigCommand, String> {
    let (verb, rest) = words.split_first().ok_or(
        "config needs a subcommand (path, edit, split, normalize, explain, export, import)",
    )?;
    let args = Args::parse(rest, &["-o", "--output", "--branch", "--env"])?;
    let none = |args: &Args| args.at_most(0, &format!("config {verb}"));
    match *verb {
        "path" => {
            args.allow(&[])?;
            none(&args)?;
            Ok(ConfigCommand::Path)
        }
        "edit" => {
            args.allow(&[])?;
            none(&args)?;
            Ok(ConfigCommand::Edit)
        }
        "split" | "normalize" => {
            args.allow(&["--dry-run"])?;
            none(&args)?;
            let dry = args.has("--dry-run");
            Ok(if *verb == "split" {
                ConfigCommand::Split { dry }
            } else {
                ConfigCommand::Normalize { dry }
            })
        }
        "explain" => {
            args.allow(&["-o", "--output", "--branch", "--env", "--show-secrets"])?;
            args.at_most(1, "config explain")?;
            Ok(ConfigCommand::Explain {
                target: args.positional.first().cloned(),
                branch: args.value(&["--branch"]),
                env: args.value(&["--env"]).unwrap_or_default(),
                json: args.json()?,
                show_secrets: args.has("--show-secrets"),
            })
        }
        "export" => {
            args.allow(&["--secrets", "-o", "--output"])?;
            if !args.positional.is_empty() {
                return Err("config export takes no arguments (use -o <file>)".into());
            }
            Ok(ConfigCommand::Export {
                secrets: args.has("--secrets"),
                output: args.value(&["-o", "--output"]).map(PathBuf::from),
            })
        }
        "import" => {
            args.allow(&["--config-only", "--secrets-only"])?;
            let [file] = args.positional.as_slice() else {
                return Err("config import needs one file".into());
            };
            if args.has("--config-only") && args.has("--secrets-only") {
                return Err("--config-only and --secrets-only exclude each other".into());
            }
            Ok(ConfigCommand::Import {
                file: PathBuf::from(file),
                config: !args.has("--secrets-only"),
                secrets: !args.has("--config-only"),
            })
        }
        other => Err(format!("unknown config subcommand {other}")),
    }
}

pub(crate) fn execute(
    command: &ConfigCommand,
    config_path: &Path,
    out: &mut dyn Write,
) -> Result<(), String> {
    let state = StateDir::from_env();
    match command {
        ConfigCommand::Path => return say(out, &config_path.display().to_string()),
        ConfigCommand::Edit => return edit(config_path, out),
        ConfigCommand::Split { dry } => {
            let result = pom_config::maintain::split(config_path, *dry)?;
            let verb = if *dry { "would write" } else { "wrote" };
            say(out, &format!("root    {}", result.root.display()))?;
            for fragment in &result.fragments {
                say(out, &format!("{verb}   {}", fragment.display()))?;
            }
            return if *dry {
                say(out, "dry run: nothing written")
            } else {
                say(out, &format!("backup  {}", result.backup.display()))
            };
        }
        ConfigCommand::Normalize { dry: true } => {
            let removed = pom_config::maintain::removed_keys_in(config_path);
            if removed.is_empty() {
                say(
                    out,
                    "no removed keys; normalize would only migrate tokens and split",
                )?;
            } else {
                say(out, &format!("would remove: {}", removed.join(", ")))?;
            }
            return say(out, "dry run: nothing written");
        }
        ConfigCommand::Normalize { dry: false } => {
            let removed = pom_config::maintain::normalize(config_path)?;
            if removed.is_empty() {
                return say(out, "normalized (no removed keys found)");
            }
            return say(out, &format!("normalized; removed {}", removed.join(", ")));
        }
        ConfigCommand::Explain { .. } => {
            return Err("config explain needs a project session".into());
        }
        _ => {}
    }
    let session = session_of(config_path)?;
    match command {
        ConfigCommand::Export { secrets, output } => {
            let password = if *secrets {
                Some(read_password("password to seal the secrets with")?)
            } else {
                None
            };
            let export = pom_bundle::export(config_path, &state, &session, password.as_deref())?;
            match output {
                Some(path) => {
                    std::fs::write(path, &export.data)
                        .map_err(|error| format!("write {}: {error}", path.display()))?;
                    say(out, &format!("wrote {}", path.display()))
                }
                None if export.data.starts_with(pom_bundle::MAGIC) => {
                    Err("a sealed bundle is binary; write it to a file with -o <file>".into())
                }
                None => out
                    .write_all(&export.data)
                    .map_err(|error| error.to_string()),
            }
        }
        ConfigCommand::Import {
            file,
            config,
            secrets,
        } => {
            let data =
                std::fs::read(file).map_err(|error| format!("read {}: {error}", file.display()))?;
            let password = if pom_bundle::is_sealed(&data) {
                read_password("bundle password")?
            } else {
                String::new()
            };
            let contents = pom_bundle::open(&data, &password)?;
            let yaml = config.then_some(contents.config.as_str());
            let no_secrets = Default::default();
            let secret_values = if *secrets {
                &contents.secrets
            } else {
                &no_secrets
            };
            let applied = pom_bundle::apply(config_path, &state, &session, yaml, secret_values)?;
            if yaml.is_some() {
                let tidied = if applied.split {
                    ", split into pom.d"
                } else {
                    ""
                };
                say(
                    out,
                    &format!(
                        "replaced {} (previous kept as pom.yml.bak{tidied})",
                        config_path.display()
                    ),
                )?;
            }
            if applied.secrets_created > 0 {
                say(
                    out,
                    &format!("stored {} secret(s)", applied.secrets_created),
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

impl Session {
    pub(crate) fn config_command(
        &self,
        command: &ConfigCommand,
        out: &mut dyn Write,
    ) -> Result<(), String> {
        let ConfigCommand::Explain {
            target,
            branch,
            env,
            json,
            show_secrets,
        } = command
        else {
            return execute(command, &self.project.config_path, out);
        };
        let branch = branch.clone().unwrap_or_else(|| self.branch.clone());
        let environment = if env.is_empty() { "local" } else { env };
        self.config.validate_environment(env)?;
        let workspace_env = self.runner.workspace_env(&self.config, &branch);
        if let Some((repo, service)) = target.as_deref().and_then(|target| target.split_once('/')) {
            let (repo, service) = self
                .config
                .find_service_entry(&format!("{repo}/{service}"))?;
            let explained = workspace_env
                .explain_service(&repo, &service, env)
                .ok_or_else(|| format!("no service {repo}/{service}"))?;
            let port = self.runner.port(
                &self.config,
                &ServiceTarget {
                    branch: branch.clone(),
                    is_main: branch == self.config.global_default_branch(),
                    repo: repo.clone(),
                    service: service.clone(),
                },
            );
            let shown = |line: &pom_services::EnvLine| {
                if line.secret && !show_secrets {
                    "********".to_string()
                } else {
                    line.value.clone()
                }
            };
            if *json {
                let value = serde_json::json!({
                    "repo": explained.repo,
                    "alias": explained.alias,
                    "service": explained.service,
                    "cmd": explained.cmd,
                    "port": port,
                    "databases": explained.databases,
                    "env": explained.env.iter().map(|line| serde_json::json!({
                        "key": line.key, "value": shown(line), "source": line.source,
                    })).collect::<Vec<_>>(),
                });
                return say(out, &pretty(&value)?);
            }
            say(
                out,
                &format!(
                    "{}/{}  (alias {})\n",
                    explained.repo, explained.service, explained.alias
                ),
            )?;
            say(out, &format!("  cmd   {}", explained.cmd))?;
            say(
                out,
                &format!(
                    "  port  {}",
                    port.map_or_else(
                        || "- (none leased on this branch yet)".to_string(),
                        |port| port.to_string()
                    )
                ),
            )?;
            if !explained.databases.is_empty() {
                say(out, "\nDATABASES")?;
                let rows: Vec<Vec<String>> = explained
                    .databases
                    .iter()
                    .map(|(name, real)| vec![format!("{{{{db.{name}}}}}"), real.clone()])
                    .collect();
                table(out, "  ", &rows)?;
            }
            if !explained.env.is_empty() {
                say(out, "\nENV  (resolved, with where each value comes from)")?;
                let rows: Vec<Vec<String>> = explained
                    .env
                    .iter()
                    .map(|line| vec![line.key.clone(), dash(&shown(line)), line.source.clone()])
                    .collect();
                table(out, "  ", &rows)?;
            }
            return Ok(());
        }
        let shared: Vec<(String, String, u16, String)> = self
            .config
            .shared_services
            .iter()
            .map(|(name, def)| {
                let host = if def.host.is_empty() {
                    "localhost".to_string()
                } else {
                    def.host.clone()
                };
                let creds = if def.db_user.is_empty() {
                    "-".to_string()
                } else {
                    format!("{}:{}", def.db_user, def.db_password)
                };
                (
                    name.clone(),
                    host,
                    self.runner.shared_host_port(name),
                    creds,
                )
            })
            .collect();
        let databases: Vec<(String, Vec<(String, String)>)> = self
            .config
            .repos
            .iter()
            .filter(|(name, dir)| {
                !dir.databases.is_empty()
                    && target
                        .as_deref()
                        .is_none_or(|target| target == name.as_str() || target == dir.alias)
            })
            .map(|(name, dir)| {
                (
                    name.clone(),
                    workspace_env.db_names(dir).into_iter().collect(),
                )
            })
            .collect();
        if *json {
            let value = serde_json::json!({
                "config": self.project.config_path,
                "branch": branch,
                "env": environment,
                "shared": shared.iter().map(|(name, host, port, creds)| serde_json::json!({
                    "name": name, "host": host, "port": port, "creds": creds,
                })).collect::<Vec<_>>(),
                "databases": databases.iter().map(|(repo, names)| serde_json::json!({
                    "repo": repo,
                    "names": names.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
                })).collect::<Vec<_>>(),
            });
            return say(out, &pretty(&value)?);
        }
        say(
            out,
            &format!("Config  {}", self.project.config_path.display()),
        )?;
        say(out, &format!("Branch  {branch}   Env {environment}"))?;
        if !shared.is_empty() {
            say(
                out,
                "\nSHARED SERVICES  {{shared.NAME.host}} {{shared.NAME.port}} {{shared.NAME.url}}",
            )?;
            let mut rows = vec![vec![
                "NAME".to_string(),
                "HOST:PORT".to_string(),
                "CREDS".to_string(),
            ]];
            rows.extend(shared.iter().map(|(name, host, port, creds)| {
                vec![name.clone(), format!("{host}:{port}"), creds.clone()]
            }));
            table(out, "  ", &rows)?;
        }
        if !databases.is_empty() {
            say(
                out,
                "\nDATABASES  {{db.NAME}}, per branch and prefixed with the session",
            )?;
            for (repo, names) in &databases {
                say(out, &format!("  {repo}"))?;
                let rows: Vec<Vec<String>> = names
                    .iter()
                    .map(|(name, real)| vec![format!("{{{{db.{name}}}}}"), real.clone()])
                    .collect();
                table(out, "    ", &rows)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn pretty(value: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|error| error.to_string())
}

pub(crate) fn dash(text: &str) -> String {
    if text.is_empty() {
        "-".to_string()
    } else {
        text.to_string()
    }
}

/// Rows padded into columns two spaces apart; the last column is not padded.
pub(crate) fn table(out: &mut dyn Write, indent: &str, rows: &[Vec<String>]) -> Result<(), String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in rows {
        let mut line = indent.to_string();
        for (column, cell) in row.iter().enumerate() {
            if column + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{cell:width$}  ", width = widths[column]));
            }
        }
        say(out, line.trim_end())?;
    }
    Ok(())
}

/// The session name straight off the file, so a config that fails validation can still be replaced.
fn session_of(config_path: &Path) -> Result<String, String> {
    let document = pom_config::merged_document(config_path).map_err(|error| error.to_string())?;
    document
        .get("session")
        .map(|node| node.text().to_string())
        .filter(|session| !session.is_empty())
        .ok_or_else(|| format!("{} has no session", config_path.display()))
}

/// First line of stdin; on a terminal the typing is hidden.
fn read_password(what: &str) -> Result<String, String> {
    let terminal = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if terminal {
        eprint!("{what}: ");
        set_echo(false);
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if terminal {
        set_echo(true);
        eprintln!();
    }
    read.map_err(|error| format!("read password: {error}"))?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();
    if password.is_empty() {
        return Err(format!("no {what} given (type it, or pipe it on stdin)"));
    }
    Ok(password)
}

fn set_echo(on: bool) {
    let result = std::process::Command::new("stty")
        .arg(if on { "echo" } else { "-echo" })
        .stdin(std::process::Stdio::inherit())
        .status();
    if let Err(error) = result {
        eprintln!("stty: {error}");
    }
}

/// Opens the config in `$VISUAL` / `$EDITOR` (else the default text editor), then says whether it still loads.
fn edit(config_path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let editor = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|editor| !editor.trim().is_empty());
    let status = match editor {
        Some(editor) => {
            let mut words = editor.split_whitespace();
            let program = words.next().unwrap_or("vi");
            std::process::Command::new(program)
                .args(words)
                .arg(config_path)
                .status()
                .map_err(|error| format!("start {program}: {error}"))?
        }
        // `open -W` waits for the editor window so the check below sees the saved file.
        None => std::process::Command::new("open")
            .args(["-W", "-t"])
            .arg(config_path)
            .status()
            .map_err(|error| format!("start open: {error}"))?,
    };
    if !status.success() {
        return Err(format!("the editor exited with {status}"));
    }
    let config = pom_config::Config::load(config_path).map_err(|error| error.message)?;
    config.validate()?;
    say(out, &format!("{} is valid", config_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_and_import_flags() {
        assert_eq!(
            parse(&["export", "--secrets", "-o", "x.pombundle"]),
            Ok(ConfigCommand::Export {
                secrets: true,
                output: Some(PathBuf::from("x.pombundle")),
            })
        );
        assert_eq!(
            parse(&["import", "x.yml", "--config-only"]),
            Ok(ConfigCommand::Import {
                file: PathBuf::from("x.yml"),
                config: true,
                secrets: false,
            })
        );
        assert!(parse(&["import"]).is_err());
        assert!(parse(&["import", "a", "--config-only", "--secrets-only"]).is_err());
        assert!(parse(&["export", "--bogus"]).is_err());
    }
}
