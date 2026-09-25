//! `pom completion bash|zsh|fish`: shell completion for commands and their subcommands.

use std::io::Write;

/// Every command with its subcommands (none for most).
pub(crate) const COMMANDS: &[(&str, &[&str])] = &[
    ("start", &[]),
    ("stop", &[]),
    ("restart", &[]),
    ("status", &[]),
    ("logs", &[]),
    ("attach", &[]),
    ("ports", &[]),
    ("url", &[]),
    ("proxy", &[]),
    ("run", &[]),
    ("commands", &[]),
    ("env", &["ls", "get", "set", "unset"]),
    ("db", &["create", "drop", "reset", "clean"]),
    (
        "config",
        &["path", "split", "normalize", "explain", "export", "import"],
    ),
    ("ws", &["create", "delete", "rename", "list"]),
    ("workspace", &["create", "delete", "rename", "list"]),
    ("get", &["workspaces"]),
    ("describe", &["workspace"]),
    ("apply", &[]),
    ("prepare-main", &[]),
    ("refresh", &[]),
    ("release", &[]),
    ("ps", &[]),
    ("disk", &[]),
    ("doctor", &[]),
    ("init", &[]),
    ("onboard", &[]),
    ("mcp", &[]),
    ("completion", &["bash", "zsh", "fish"]),
    ("version", &[]),
    ("help", &[]),
];

pub(crate) fn script(shell: &str) -> Result<String, String> {
    let names: Vec<&str> = COMMANDS.iter().map(|(name, _)| *name).collect();
    let with_subcommands = COMMANDS.iter().filter(|(_, subs)| !subs.is_empty());
    match shell {
        "bash" => {
            let mut cases = String::new();
            for (name, subs) in with_subcommands {
                cases.push_str(&format!(
                    "    {name}) COMPREPLY=($(compgen -W \"{}\" -- \"$current\")); return ;;\n",
                    subs.join(" ")
                ));
            }
            Ok(format!(
                "_pom() {{\n  local current=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  if [ \"$COMP_CWORD\" -eq 1 ]; then\n    COMPREPLY=($(compgen -W \"{}\" -- \"$current\"))\n    return\n  fi\n  case \"${{COMP_WORDS[1]}}\" in\n{cases}  esac\n  COMPREPLY=($(compgen -f -- \"$current\"))\n}}\ncomplete -F _pom pom\n",
                names.join(" ")
            ))
        }
        "zsh" => {
            let mut cases = String::new();
            for (name, subs) in with_subcommands {
                cases.push_str(&format!("    {name}) compadd -- {} ;;\n", subs.join(" ")));
            }
            Ok(format!(
                "#compdef pom\n_pom() {{\n  if (( CURRENT == 2 )); then\n    compadd -- {}\n    return\n  fi\n  case \"${{words[2]}}\" in\n{cases}    *) _files ;;\n  esac\n}}\ncompdef _pom pom\n",
                names.join(" ")
            ))
        }
        "fish" => {
            let mut lines = format!(
                "complete -c pom -f -n '__fish_use_subcommand' -a '{}'\n",
                names.join(" ")
            );
            for (name, subs) in with_subcommands {
                lines.push_str(&format!(
                    "complete -c pom -f -n '__fish_seen_subcommand_from {name}' -a '{}'\n",
                    subs.join(" ")
                ));
            }
            Ok(lines)
        }
        other => Err(format!("no completion for {other} (bash, zsh or fish)")),
    }
}

pub(crate) fn print(shell: &str, out: &mut dyn Write) -> Result<(), String> {
    let text = script(shell)?;
    out.write_all(text.as_bytes())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_list_commands_and_subcommands() {
        let bash = script("bash").expect("bash");
        assert!(bash.contains("complete -F _pom pom"));
        assert!(bash.contains("config) COMPREPLY=($(compgen -W \"path split"));
        let zsh = script("zsh").expect("zsh");
        assert!(zsh.starts_with("#compdef pom"));
        assert!(zsh.contains("db) compadd -- create drop reset clean"));
        let fish = script("fish").expect("fish");
        assert!(fish.contains("__fish_seen_subcommand_from env' -a 'ls get set unset'"));
        assert!(script("tcsh").is_err());
        assert!(bash.is_ascii() && zsh.is_ascii() && fish.is_ascii());
    }
}
