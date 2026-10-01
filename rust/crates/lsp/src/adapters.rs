//! Which server handles a language, and the binaries that can be it: found on the PATH, or for servers
//! published to npm, downloaded (see `install`).

use std::path::Path;

use editor::Lang;

/// A server for some languages, and how to find and start it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    /// With the folder it runs in, the key one running server is shared under.
    pub name: &'static str,
    /// Binaries to look for, in order.
    pub candidates: std::borrow::Cow<'static, [Candidate]>,
    /// Where it runs: the folder holding the file's project manifest.
    pub manifest: Manifest,
}

/// The files that mark a project for a server, and whether the outermost one wins (one server for a
/// monorepo or a Cargo workspace) or the nearest (one per project).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub names: &'static [&'static str],
    pub outermost: bool,
}

const NODE_PROJECT: Manifest = Manifest {
    names: &["package.json", "tsconfig.json", "jsconfig.json"],
    outermost: true,
};

impl Manifest {
    /// The folder a server for `file` runs in: the manifest's, within `workspace`, else `workspace` itself.
    pub fn root_for(&self, file: &Path, workspace: &Path) -> std::path::PathBuf {
        let mut found = None;
        let mut dir = file.parent();
        while let Some(folder) = dir {
            if !folder.starts_with(workspace) {
                break;
            }
            if self.names.iter().any(|name| folder.join(name).is_file()) {
                found = Some(folder);
                if !self.outermost {
                    break;
                }
            }
            if folder == workspace {
                break;
            }
            dir = folder.parent();
        }
        found.unwrap_or(workspace).to_path_buf()
    }
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
    /// The npm package it is downloaded from when it isn't on the PATH.
    pub npm: Option<NpmPackage>,
    /// Left out of `...`: runs only when `language_servers` names it.
    pub opt_in: bool,
    /// The command that installs it, told to the user when it is missing and can't be downloaded.
    pub install: Option<&'static str>,
}

impl Candidate {
    const fn installed_by(self, command: &'static str) -> Self {
        Candidate {
            install: Some(command),
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NpmPackage {
    pub name: &'static str,
    /// The script node runs, relative to the folder the package is installed in.
    pub script: &'static str,
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
        npm: None,
        opt_in: false,
        install: None,
    }
}

/// A server that runs only when the settings name it.
const fn opt_in(
    name: &'static str,
    binary: &'static str,
    args: &'static [&'static str],
) -> Candidate {
    Candidate {
        opt_in: true,
        ..candidate(name, binary, args)
    }
}

const fn from_npm(
    name: &'static str,
    binary: &'static str,
    args: &'static [&'static str],
    package: &'static str,
    script: &'static str,
) -> Candidate {
    Candidate {
        npm: Some(NpmPackage {
            name: package,
            script,
        }),
        ..candidate(name, binary, args)
    }
}

const RUST: Adapter = Adapter {
    name: "rust-analyzer",
    manifest: Manifest {
        names: &["Cargo.toml"],
        outermost: true,
    },
    candidates: std::borrow::Cow::Borrowed(&[Candidate {
        name: "rust-analyzer",
        binary: "rust-analyzer",
        args: &[],
        probe: Some(&["--help"]),
        npm: None,
        opt_in: false,
        install: Some("rustup component add rust-analyzer"),
    }]),
};
const CLANGD: Adapter = Adapter {
    name: "clangd",
    manifest: Manifest {
        names: &["compile_commands.json", "CMakeLists.txt", ".clangd"],
        outermost: false,
    },
    candidates: std::borrow::Cow::Borrowed(&[candidate("clangd", "clangd", &[])]),
};
const GOPLS: Adapter = Adapter {
    name: "gopls",
    manifest: Manifest {
        names: &["go.work", "go.mod"],
        outermost: true,
    },
    candidates: std::borrow::Cow::Borrowed(&[
        candidate("gopls", "gopls", &[]).installed_by("go install golang.org/x/tools/gopls@latest")
    ]),
};
const PYTHON: Adapter = Adapter {
    name: "python",
    manifest: Manifest {
        names: &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
            "pyrightconfig.json",
        ],
        outermost: false,
    },
    candidates: std::borrow::Cow::Borrowed(&[
        from_npm(
            "basedpyright",
            "basedpyright-langserver",
            &["--stdio"],
            "basedpyright",
            "node_modules/basedpyright/langserver.index.js",
        ),
        from_npm(
            "pyright",
            "pyright-langserver",
            &["--stdio"],
            "pyright",
            "node_modules/pyright/langserver.index.js",
        ),
        candidate("ty", "ty", &["server"]).installed_by("uv tool install ty"),
    ]),
};
const RUBY: Adapter = Adapter {
    name: "ruby",
    manifest: Manifest {
        names: &["Gemfile"],
        outermost: false,
    },
    candidates: std::borrow::Cow::Borrowed(&[
        candidate("solargraph", "solargraph", &["stdio"]).installed_by("gem install solargraph"),
        opt_in("ruby-lsp", "ruby-lsp", &[]).installed_by("gem install ruby-lsp"),
    ]),
};
const TAILWIND: Adapter = Adapter {
    name: "tailwindcss",
    manifest: NODE_PROJECT,
    candidates: std::borrow::Cow::Borrowed(&[from_npm(
        "tailwindcss-language-server",
        "tailwindcss-language-server",
        &["--stdio"],
        "@tailwindcss/language-server",
        "node_modules/.bin/tailwindcss-language-server",
    )]),
};
const TYPESCRIPT: Adapter = Adapter {
    name: "typescript",
    manifest: NODE_PROJECT,
    candidates: std::borrow::Cow::Borrowed(&[
        from_npm(
            "vtsls",
            "vtsls",
            &["--stdio"],
            "@vtsls/language-server",
            "node_modules/@vtsls/language-server/bin/vtsls.js",
        ),
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
                    .filter(|candidate| !named(candidate.name) && !candidate.opt_in)
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

/// The adapters for `lang`, each with the servers the settings choose, and the language id its documents are
/// opened with; none when the settings turn its servers off.
pub fn adapters_for(lang: Lang) -> Vec<(Adapter, &'static str)> {
    let choice = choice_for(lang);
    if !choice.enabled {
        return Vec::new();
    }
    default_adapters_for(lang)
        .into_iter()
        .filter_map(|(mut adapter, language_id)| {
            let candidates = chosen(&adapter.candidates, &choice.servers);
            if candidates.is_empty() {
                return None;
            }
            adapter.candidates = std::borrow::Cow::Owned(candidates);
            Some((adapter, language_id))
        })
        .collect()
}

/// The language's main server: the first of its adapters.
pub fn adapter_for(lang: Lang) -> Option<(Adapter, &'static str)> {
    adapters_for(lang).into_iter().next()
}

/// What a server is given at `initialize` (`initializationOptions`), by adapter.
pub fn initialization_options(adapter: &str) -> serde_json::Value {
    if adapter == TAILWIND.name {
        return serde_json::json!({"provideFormatter": true});
    }
    serde_json::Value::Null
}

fn default_adapters_for(lang: Lang) -> Vec<(Adapter, &'static str)> {
    let tailwind = |language_id| (TAILWIND, language_id);
    let mut adapters: Vec<(Adapter, &'static str)> =
        default_adapter_for(lang).into_iter().collect();
    match lang {
        Lang::TypeScript => adapters.push(tailwind("typescript")),
        Lang::Tsx => adapters.push(tailwind("typescriptreact")),
        Lang::JavaScript => adapters.push(tailwind("javascript")),
        Lang::Css => adapters.push(tailwind("css")),
        Lang::Html => adapters.push(tailwind("html")),
        Lang::Svelte => adapters.push(tailwind("svelte")),
        Lang::Php => adapters.push(tailwind("php")),
        _ => {}
    }
    adapters
}

/// What a server is told when it asks for its settings (`workspace/configuration`), by adapter.
pub fn workspace_configuration(adapter: &str, root: &Path) -> serde_json::Value {
    if adapter == TAILWIND.name {
        return serde_json::json!({
            "tailwindCSS": {
                "emmetCompletions": true,
                "includeLanguages": {
                    "html": "html",
                    "css": "css",
                    "javascript": "javascript",
                    "typescript": "typescript",
                    "typescriptreact": "typescriptreact",
                },
            },
        });
    }
    if adapter != TYPESCRIPT.name {
        return serde_json::Value::Null;
    }
    // TypeScript 7+ ships no tsserver.js, which vtsls needs; then it keeps its own.
    let tsdk = [".yarn/sdks/typescript/lib", "node_modules/typescript/lib"]
        .into_iter()
        .find(|tsdk| root.join(tsdk).join("tsserver.js").is_file());
    let config = serde_json::json!({
        "tsdk": tsdk,
        "suggest": {"completeFunctionCalls": true},
        "tsserver": {"maxTsServerMemory": 8192},
    });
    serde_json::json!({
        "typescript": config,
        "javascript": config,
        "vtsls": {
            "experimental": {"completion": {"enableServerSideFuzzyMatch": true, "entriesLimit": 5000}},
            "autoUseWorkspaceTsdk": true,
        },
    })
}

#[cfg(test)]
pub(crate) fn python_candidates() -> Vec<Candidate> {
    PYTHON.candidates.to_vec()
}

fn default_adapter_for(lang: Lang) -> Option<(Adapter, &'static str)> {
    Some(match lang {
        Lang::Rust => (RUST, "rust"),
        Lang::C => (CLANGD, "c"),
        Lang::Cpp => (CLANGD, "cpp"),
        Lang::Go => (GOPLS, "go"),
        Lang::Python => (PYTHON, "python"),
        Lang::Ruby => (RUBY, "ruby"),
        Lang::TypeScript => (TYPESCRIPT, "typescript"),
        Lang::Tsx => (TYPESCRIPT, "typescriptreact"),
        Lang::JavaScript => (TYPESCRIPT, "javascript"),
        _ => return None,
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
    fn ruby_runs_solargraph_and_ruby_lsp_only_when_named() {
        let defaults = &RUBY.candidates;
        let names = |servers: &[&str]| -> Vec<&str> {
            let servers: Vec<String> = servers.iter().map(|name| name.to_string()).collect();
            chosen(defaults, &servers)
                .iter()
                .map(|candidate| candidate.name)
                .collect()
        };
        assert_eq!(names(&["..."]), ["solargraph"]);
        assert_eq!(names(&["ruby-lsp", "..."]), ["ruby-lsp", "solargraph"]);
        let (_, language_id) = adapter_for(Lang::Ruby).unwrap();
        assert_eq!(language_id, "ruby");
    }
}
