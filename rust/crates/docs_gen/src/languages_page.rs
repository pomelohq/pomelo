use editor::highlight::PACKAGED;
use editor::registry::language_registry;
use editor::Lang;

use crate::{code, prose};

const LANGUAGES_INTRO: &str = "\
# Languages

Every language a file can be, how it is highlighted, and the files that are
that language. A language marked **package** opens as plain text until its
package is installed (see [Language packages](/docs/app#language-packages)).

A file is matched by its whole name first, then by its extension or the end
of its path, and when neither matches, by its first line. The `file_types`
setting adds your own matches ([Settings](/reference/settings#languages-tools)).

| Language | Highlighting | Files | First line |
| --- | --- | --- | --- |
";

const SERVERS_INTRO: &str = "\
# Language servers

The language servers Pomelo can start, the languages each one serves and the
folder it runs in. Under each are the programs that can be it, in the order
they are tried; the first one found runs. The `language_servers` setting
names them to reorder them, turn one off with `!name`, or turn on an opt-in
one, which runs only when named there (see
[Language servers](/docs/app#language-servers)).

Pomelo looks for each program on your login shell's `PATH` first. Servers
published to npm are downloaded when missing (with your `node` and `npm`)
into `~/Library/Application Support/Pomelo/languages`.
";

pub fn render_languages() -> Result<String, String> {
    let registry = language_registry()
        .read()
        .map_err(|_| "the language registry is poisoned".to_string())?;
    let packages = grammars::embedded_index().packages;
    let mut languages = Lang::LANGUAGES;
    languages.sort_by_key(|lang| lang.name().to_lowercase());
    let mut page = String::from(LANGUAGES_INTRO);
    for lang in languages {
        let (highlighting, suffixes, first_line) = if PACKAGED.contains(&lang) {
            let package = packages
                .iter()
                .find(|package| package.language == lang.name())
                .ok_or_else(|| format!("{} is packaged but has no package", lang.name()))?;
            (
                "package".to_string(),
                package.path_suffixes.clone(),
                package.first_line_pattern.clone(),
            )
        } else {
            let matcher = registry.matcher(lang).unwrap_or_default();
            let highlighting = if registry.has_grammar(lang) {
                "built in"
            } else {
                "none"
            };
            (
                highlighting.to_string(),
                matcher.path_suffixes,
                matcher
                    .first_line
                    .map(|pattern| pattern.as_str().to_string()),
            )
        };
        let files = if suffixes.is_empty() {
            "-".to_string()
        } else {
            suffixes
                .iter()
                .map(|suffix| code(suffix))
                .collect::<Vec<_>>()
                .join(" ")
        };
        page.push_str(&format!(
            "| {} | {highlighting} | {files} | {} |\n",
            prose(lang.name()),
            first_line.as_deref().map_or_else(|| "-".to_string(), code),
        ));
    }
    Ok(page)
}

pub fn render_servers() -> String {
    let mut adapters: Vec<(lsp::Adapter, Vec<&'static str>)> = Vec::new();
    for lang in Lang::LANGUAGES {
        for (adapter, _) in lsp::default_adapters_for(lang) {
            match adapters
                .iter_mut()
                .find(|(seen, _)| seen.name == adapter.name)
            {
                Some((_, languages)) => languages.push(lang.name()),
                None => adapters.push((adapter, vec![lang.name()])),
            }
        }
    }
    let mut page = String::from(SERVERS_INTRO);
    for (adapter, languages) in adapters {
        let manifests: Vec<String> = adapter
            .manifest
            .names
            .iter()
            .map(|name| code(name))
            .collect();
        let which = if adapter.manifest.outermost {
            "outermost"
        } else {
            "nearest"
        };
        page.push_str(&format!(
            "\n## {}\n\nFor {}. Runs in the folder of the {which} {}, else the workspace folder.\n\n\
             | Name | Program | Opt-in | When missing |\n| --- | --- | --- | --- |\n",
            adapter.name,
            prose(&languages.join(", ")),
            or_list(&manifests),
        ));
        for candidate in adapter.candidates.iter() {
            let program = match &candidate.source {
                Some(source) => std::iter::once("node")
                    .chain([source.script])
                    .chain(candidate.args.iter().copied())
                    .collect::<Vec<_>>()
                    .join(" "),
                None => std::iter::once(candidate.binary)
                    .chain(candidate.args.iter().copied())
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            let missing = match (&candidate.npm, &candidate.source, candidate.install) {
                (Some(package), _, _) => format!("Downloaded from npm ({})", code(package.name)),
                (_, Some(_), _) => "Built once from its source release".to_string(),
                (_, _, Some(install)) => format!("Install it: {}", code(install)),
                _ => "Install it yourself".to_string(),
            };
            page.push_str(&format!(
                "| {} | {} | {} | {missing} |\n",
                code(candidate.name),
                code(&program),
                if candidate.opt_in { "yes" } else { "-" },
            ));
        }
    }
    page
}

fn or_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}
