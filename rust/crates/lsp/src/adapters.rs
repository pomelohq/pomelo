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
    pub candidates: &'static [Candidate],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub binary: &'static str,
    pub args: &'static [&'static str],
    /// Arguments of a trial run that must succeed before the binary is trusted, for launchers that exist
    /// on the PATH without the server behind them (a toolchain proxy whose component isn't installed).
    pub probe: Option<&'static [&'static str]>,
}

const fn candidate(binary: &'static str, args: &'static [&'static str]) -> Candidate {
    Candidate {
        binary,
        args,
        probe: None,
    }
}

const RUST: Adapter = Adapter {
    name: "rust-analyzer",
    candidates: &[Candidate {
        binary: "rust-analyzer",
        args: &[],
        probe: Some(&["--help"]),
    }],
};
const CLANGD: Adapter = Adapter {
    name: "clangd",
    candidates: &[candidate("clangd", &[])],
};
const GOPLS: Adapter = Adapter {
    name: "gopls",
    candidates: &[candidate("gopls", &[])],
};
const PYTHON: Adapter = Adapter {
    name: "python",
    candidates: &[
        candidate("basedpyright-langserver", &["--stdio"]),
        candidate("pyright-langserver", &["--stdio"]),
        candidate("ty", &["server"]),
    ],
};
const TYPESCRIPT: Adapter = Adapter {
    name: "typescript",
    candidates: &[
        candidate("vtsls", &["--stdio"]),
        candidate("typescript-language-server", &["--stdio"]),
    ],
};

/// The adapter for `lang` and the language id its documents are opened with.
pub fn adapter_for(lang: Lang) -> Option<(Adapter, &'static str)> {
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
