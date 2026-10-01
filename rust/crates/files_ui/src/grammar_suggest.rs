//! The suggestion to install a grammar package when a file of a language the app doesn't highlight becomes
//! the active one, and the installs it starts.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Mutex;

use grammars::{Package, SuggestionCheck};
use workspace::{NoticeLevel, ServerNotice};

use crate::DONT_SHOW_AGAIN;

/// Languages installed or turned down in any window, so the same suggestion shown elsewhere goes away too.
static SETTLED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn settle(language: &str) {
    if let Ok(mut settled) = SETTLED.lock() {
        settled.insert(language.to_string());
    }
}

fn settled(language: &str) -> bool {
    SETTLED
        .lock()
        .is_ok_and(|settled| settled.contains(language))
}

/// The file just made active: its path in the project, the language it was recognized as, and whether it is
/// highlighted.
pub(crate) struct ShownFile<'a> {
    pub path: &'a str,
    pub language: Option<&'static str>,
    pub highlighted: bool,
}

type Install = (Package, Receiver<Result<PathBuf, String>>);

#[derive(Default)]
pub(crate) struct GrammarSuggestions {
    last_shown: Option<String>,
    index: Option<grammars::Index>,
    /// Suggestions waiting on an answer, by notice token.
    offered: HashMap<u64, Package>,
    installing: Vec<Install>,
    withdrawn: Vec<u64>,
}

impl GrammarSuggestions {
    /// Remembers the file's language and offers its grammar when one is published and nothing stands in the
    /// way; once per switch to the file.
    pub(crate) fn file_shown(
        &mut self,
        file: ShownFile,
        next_token: &mut u64,
    ) -> Option<ServerNotice> {
        if self.last_shown.as_deref() == Some(file.path) {
            return None;
        }
        self.last_shown = Some(file.path.to_string());
        if let (Some(language), Some(recent)) = (file.language, grammars::recent_file()) {
            grammars::record_language(&recent, language);
        }
        if !grammars::downloads_on() {
            return None;
        }
        let dir = editor::grammar_packages::grammars_dir()?;
        let index = self.index.get_or_insert_with(|| grammars::load_index(&dir));
        let package = index.package_for(file.path)?.clone();
        let check = SuggestionCheck {
            has_grammar: file.highlighted,
            installed: grammars::is_installed(&dir, &package),
            installing: self
                .installing
                .iter()
                .any(|(installing, _)| installing.id == package.id),
            dismissed: settled(&package.language)
                || grammars::dismissed_file()
                    .is_some_and(|file| grammars::dismissed(&file).contains(&package.language)),
            showing: self
                .offered
                .values()
                .any(|offered| offered.id == package.id),
            downloads_on: true,
        };
        if !check.suggests() {
            return None;
        }
        let token = *next_token;
        *next_token += 1;
        let notice = suggestion(token, &package);
        self.offered.insert(token, package);
        Some(notice)
    }

    /// Acts on the answer to a suggestion; false when `token` is not one.
    pub(crate) fn answer(&mut self, token: u64, action: Option<usize>) -> bool {
        let Some(package) = self.offered.remove(&token) else {
            return false;
        };
        match action {
            Some(0) => self.install(package),
            Some(1) => {
                if let Some(file) = grammars::dismissed_file() {
                    grammars::dismiss(&file, &package.language);
                }
                settle(&package.language);
            }
            _ => {}
        }
        true
    }

    fn install(&mut self, package: Package) {
        let Some(dir) = editor::grammar_packages::grammars_dir() else {
            return;
        };
        let (sender, receiver) = channel();
        let worker = package.clone();
        let spawned = std::thread::Builder::new()
            .name("grammar-install".into())
            .spawn(move || {
                let installed = grammars::download_and_install(&worker, &dir).and_then(|folder| {
                    editor::grammar_packages::register_installed_grammar(&folder).map(|()| folder)
                });
                let failed = installed.is_err();
                if sender.send(installed).is_ok() && failed {
                    editor::registry::notify_languages_changed();
                }
            });
        match spawned {
            Ok(_) => self.installing.push((package, receiver)),
            Err(error) => eprintln!("grammars: start installing {}: {error}", package.language),
        }
    }

    /// Notices for installs that failed; suggestions settled in another window are withdrawn.
    pub(crate) fn poll(&mut self, next_token: &mut u64) -> Vec<ServerNotice> {
        let mut notices = Vec::new();
        self.installing
            .retain(|(package, receiver)| match receiver.try_recv() {
                Err(TryRecvError::Empty) => true,
                Err(TryRecvError::Disconnected) => false,
                Ok(Ok(_)) => {
                    settle(&package.language);
                    false
                }
                Ok(Err(error)) => {
                    eprintln!("grammars: install {}: {error}", package.language);
                    let token = *next_token;
                    *next_token += 1;
                    notices.push(ServerNotice {
                        token,
                        server: format!("Could not install {}", package.language),
                        level: NoticeLevel::Error,
                        message: error,
                        actions: Vec::new(),
                        switch: None,
                    });
                    false
                }
            });
        let done: Vec<u64> = self
            .offered
            .iter()
            .filter(|(_, package)| settled(&package.language))
            .map(|(token, _)| *token)
            .collect();
        for token in done {
            self.offered.remove(&token);
            self.withdrawn.push(token);
        }
        notices
    }

    pub(crate) fn take_withdrawn(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.withdrawn)
    }
}

fn suggestion(token: u64, package: &Package) -> ServerNotice {
    ServerNotice {
        token,
        server: format!("{} is available for this file", package.language),
        level: NoticeLevel::Info,
        message: format!(
            "Install highlighting and outline for {} files ({}).",
            package.language,
            size_label(package.size)
        ),
        actions: vec![
            format!("Install {}", package.language),
            DONT_SHOW_AGAIN.to_string(),
        ],
        switch: None,
    }
}

fn size_label(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suggestion_names_the_language_its_size_and_both_answers() {
        let package = Package {
            id: "ocaml".into(),
            language: "OCaml".into(),
            version: "0.24.2".into(),
            file: "ocaml-0.24.2.tar.gz".into(),
            url: None,
            size: 300 * 1024,
            sha256: String::new(),
            input_hash: None,
            signature: None,
            path_suffixes: vec!["ml".into()],
            first_line_pattern: None,
        };
        let notice = suggestion(4, &package);
        assert_eq!(notice.server, "OCaml is available for this file");
        assert_eq!(
            notice.message,
            "Install highlighting and outline for OCaml files (300 KB)."
        );
        assert_eq!(notice.actions, ["Install OCaml", "Don't show again"]);
        assert_eq!(size_label(5 * 1024 * 1024 + 1), "5.0 MB");
    }

    #[test]
    fn a_settled_language_withdraws_its_suggestion() {
        let mut suggestions = GrammarSuggestions::default();
        let package = Package {
            id: "gleam".into(),
            language: "Gleam".into(),
            version: "1".into(),
            file: "gleam-1.tar.gz".into(),
            url: None,
            size: 1,
            sha256: String::new(),
            input_hash: None,
            signature: None,
            path_suffixes: vec!["gleam".into()],
            first_line_pattern: None,
        };
        suggestions.offered.insert(9, package);
        let mut next = 10;
        assert!(suggestions.poll(&mut next).is_empty());
        assert!(suggestions.take_withdrawn().is_empty());
        settle("Gleam");
        suggestions.poll(&mut next);
        assert_eq!(suggestions.take_withdrawn(), [9]);
        assert!(
            !suggestions.answer(9, Some(0)),
            "a withdrawn suggestion is gone"
        );
    }
}
