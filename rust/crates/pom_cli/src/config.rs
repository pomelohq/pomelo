//! `pom config ...`: the project's config as files.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use pom_paths::StateDir;

use crate::say;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfigCommand {
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

pub(crate) fn parse(words: &[&str]) -> Result<ConfigCommand, String> {
    let (verb, rest) = words
        .split_first()
        .ok_or("config needs a subcommand (export, import)")?;
    let mut positional: Vec<&str> = Vec::new();
    let mut flags: Vec<&str> = Vec::new();
    let mut output = None;
    let mut iter = rest.iter();
    while let Some(word) = iter.next() {
        match *word {
            "-o" | "--output" => {
                output = Some(PathBuf::from(*iter.next().ok_or("-o needs a path")?));
            }
            flag if flag.starts_with("--") => flags.push(flag),
            word => positional.push(word),
        }
    }
    let allow = |known: &[&str]| match flags.iter().find(|flag| !known.contains(flag)) {
        Some(flag) => Err(format!("unknown flag {flag}")),
        None => Ok(()),
    };
    let has = |flag: &str| flags.contains(&flag);
    match *verb {
        "export" => {
            allow(&["--secrets"])?;
            if !positional.is_empty() {
                return Err("config export takes no arguments (use -o <file>)".into());
            }
            Ok(ConfigCommand::Export {
                secrets: has("--secrets"),
                output,
            })
        }
        "import" => {
            allow(&["--config-only", "--secrets-only"])?;
            if output.is_some() {
                return Err("config import takes no -o".into());
            }
            let [file] = positional.as_slice() else {
                return Err("config import needs one file".into());
            };
            if has("--config-only") && has("--secrets-only") {
                return Err("--config-only and --secrets-only exclude each other".into());
            }
            Ok(ConfigCommand::Import {
                file: PathBuf::from(file),
                config: !has("--secrets-only"),
                secrets: !has("--config-only"),
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
    }
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
