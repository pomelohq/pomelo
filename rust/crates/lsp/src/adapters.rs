//! Which server handles a language: the first of its candidate binaries found on the PATH. Nothing is
//! downloaded; a missing server just means no language features for that language.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use editor::Lang;

/// A server for some languages, and how to find and start it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    /// Also the key one running server is shared under.
    pub name: &'static str,
    /// Binaries to look for, in order.
    pub candidates: std::borrow::Cow<'static, [Candidate]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// What `language_servers` in the settings calls it.
    pub name: &'static str,
    pub binary: &'static str,
    pub args: &'static [&'static str],
    /// Arguments of a trial run that must succeed before the binary is trusted, for launchers that exist
    /// on the PATH without the server behind them (a toolchain proxy whose component isn't installed).
    pub probe: Option<&'static [&'static str]>,
}

const fn candidate(
    name: &'static str,
    binary: &'static str,
    args: &'static [&'static str],
) -> Candidate {
    Candidate {
        name,
        binary,
        args,
        probe: None,
    }
}

const RUST: Adapter = Adapter {
    name: "rust-analyzer",
    candidates: std::borrow::Cow::Borrowed(&[Candidate {
        name: "rust-analyzer",
        binary: "rust-analyzer",
        args: &[],
        probe: Some(&["--help"]),
    }]),
};
const CLANGD: Adapter = Adapter {
    name: "clangd",
    candidates: std::borrow::Cow::Borrowed(&[candidate("clangd", "clangd", &[])]),
};
const GOPLS: Adapter = Adapter {
    name: "gopls",
    candidates: std::borrow::Cow::Borrowed(&[candidate("gopls", "gopls", &[])]),
};
const PYTHON: Adapter = Adapter {
    name: "python",
    candidates: std::borrow::Cow::Borrowed(&[
        candidate("basedpyright", "basedpyright-langserver", &["--stdio"]),
        candidate("pyright", "pyright-langserver", &["--stdio"]),
        candidate("ty", "ty", &["server"]),
    ]),
};
const TYPESCRIPT: Adapter = Adapter {
    name: "typescript",
    candidates: std::borrow::Cow::Borrowed(&[
        candidate("vtsls", "vtsls", &["--stdio"]),
        candidate(
            "typescript-language-server",
            "typescript-language-server",
            &["--stdio"],
        ),
    ]),
};

/// Whether a language uses servers at all, which (`language_servers` syntax), and how it asks them for
/// completions, as the settings say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerChoice {
    pub enabled: bool,
    pub servers: Vec<String>,
    pub completions: bool,
    /// 0 waits for them.
    pub completion_timeout_ms: u64,
}

impl Default for ServerChoice {
    fn default() -> Self {
        ServerChoice {
            enabled: true,
            servers: vec!["...".into()],
            completions: true,
            completion_timeout_ms: 0,
        }
    }
}

type ChoiceOf = dyn Fn(&str) -> ServerChoice + Send + Sync;

static CHOICE: std::sync::RwLock<Option<std::sync::Arc<ChoiceOf>>> = std::sync::RwLock::new(None);

/// How the settings choose a language's servers, by language name ("Rust", "TSX").
pub fn set_server_choice(choice: impl Fn(&str) -> ServerChoice + Send + Sync + 'static) {
    if let Ok(mut slot) = CHOICE.write() {
        *slot = Some(std::sync::Arc::new(choice));
    }
}

pub fn choice_for(lang: Lang) -> ServerChoice {
    let choice = CHOICE.read().ok().and_then(|slot| slot.clone());
    choice.map_or_else(ServerChoice::default, |choice| choice(lang.name()))
}

/// `defaults` ordered and filtered by a `language_servers` list: names listed first, in order, `...` for the
/// defaults not named, `!name` left out; with no `...`, only the named ones.
fn chosen(defaults: &[Candidate], servers: &[String]) -> Vec<Candidate> {
    let named = |name: &str| {
        servers
            .iter()
            .any(|entry| entry == name || entry == &format!("!{name}"))
    };
    let mut out: Vec<Candidate> = Vec::new();
    for entry in servers {
        if entry == "..." {
            out.extend(
                defaults
                    .iter()
                    .filter(|candidate| !named(candidate.name))
                    .cloned(),
            );
        } else if !entry.starts_with('!') {
            if let Some(candidate) = defaults.iter().find(|candidate| candidate.name == entry) {
                out.push(candidate.clone());
            }
        }
    }
    out
}

/// The adapter for `lang`, with the servers the settings choose, and the language id its documents are
/// opened with; none when the settings turn its servers off.
pub fn adapter_for(lang: Lang) -> Option<(Adapter, &'static str)> {
    let (mut adapter, language_id) = default_adapter_for(lang)?;
    let choice = choice_for(lang);
    if !choice.enabled {
        return None;
    }
    let candidates = chosen(&adapter.candidates, &choice.servers);
    if candidates.is_empty() {
        return None;
    }
    adapter.candidates = std::borrow::Cow::Owned(candidates);
    Some((adapter, language_id))
}

fn default_adapter_for(lang: Lang) -> Option<(Adapter, &'static str)> {
    Some(match lang {
        Lang::Rust => (RUST, "rust"),
        Lang::C => (CLANGD, "c"),
        Lang::Cpp => (CLANGD, "cpp"),
        Lang::Go => (GOPLS, "go"),
        Lang::Python => (PYTHON, "python"),
        Lang::TypeScript => (TYPESCRIPT, "typescript"),
        Lang::Tsx => (TYPESCRIPT, "typescriptreact"),
        Lang::JavaScript => (TYPESCRIPT, "javascript"),
        _ => return None,
    })
}

impl Adapter {
    /// The first candidate on `env`'s PATH that passes its probe, with its arguments. May run the probes,
    /// so call it off the UI thread.
    pub fn locate(&self, env: &HashMap<String, String>) -> Option<(PathBuf, Vec<String>)> {
        let path = env.get("PATH")?;
        self.candidates.iter().find_map(|candidate| {
            let found = which(candidate.binary, path)?;
            if let Some(probe) = candidate.probe {
                let works = std::process::Command::new(&found)
                    .args(probe)
                    .env_clear()
                    .envs(env)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
                if !works {
                    eprintln!("lsp: {} is on the PATH but doesn't run", found.display());
                    return None;
                }
            }
            Some((
                found,
                candidate.args.iter().map(|a| a.to_string()).collect(),
            ))
        })
    }
}

fn which(binary: &str, path: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(binary))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_order_and_turn_off_a_languages_servers() {
        let defaults = &PYTHON.candidates;
        let names = |servers: &[&str]| -> Vec<&str> {
            let servers: Vec<String> = servers.iter().map(|name| name.to_string()).collect();
            chosen(defaults, &servers)
                .iter()
                .map(|candidate| candidate.name)
                .collect()
        };
        assert_eq!(names(&["..."]), ["basedpyright", "pyright", "ty"]);
        assert_eq!(names(&["ty", "!basedpyright", "..."]), ["ty", "pyright"]);
        assert_eq!(
            names(&["pyright"]),
            ["pyright"],
            "no ... keeps only the named"
        );
        assert!(names(&["!ty", "unknown"]).is_empty());
    }

    #[test]
    fn languages_share_a_server_and_keep_their_ids() {
        let (ts, ts_id) = adapter_for(Lang::TypeScript).unwrap();
        let (tsx, tsx_id) = adapter_for(Lang::Tsx).unwrap();
        assert_eq!(ts.name, tsx.name);
        assert_eq!((ts_id, tsx_id), ("typescript", "typescriptreact"));
        assert!(adapter_for(Lang::Markdown).is_none());
    }

    #[test]
    fn finds_the_first_executable_candidate() {
        let dir = std::env::temp_dir().join(format!("pomelo-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("pyright-langserver");
        std::fs::write(&binary, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = HashMap::from([(
            "PATH".to_string(),
            format!("/nonexistent:{}", dir.display()),
        )]);
        let (found, args) = PYTHON.locate(&env).unwrap();
        assert_eq!(found, binary);
        assert_eq!(args, vec!["--stdio"]);
        assert!(RUST.locate(&HashMap::new()).is_none());
    }
}
