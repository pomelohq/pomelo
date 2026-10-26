//! The Git panel over scratch repositories with their own origins: changes, the branch against origin, its
//! history, and the same panel in the window of a project on a host.

use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use workspace::SidePanelView;

use crate::fixtures::git as fixture;
use crate::fixtures::Scratch;
use crate::{Story, StoryDef};

#[derive(Clone, Copy)]
enum Repos {
    Changes(fixture::State),
    Remote(fixture::RemoteState),
}

#[derive(Clone, Copy)]
enum Access {
    Local,
    /// Every scan and answer crosses the wire as JSON, as a remote window receives them.
    Host,
    /// The host's pom is too old to serve its repos.
    Unserved,
}

struct Made {
    scratch: Scratch,
    roots: Vec<PathBuf>,
}

/// Makes its repos on a thread of its own, since a few dozen git runs would hold the gallery's frame for a second.
struct GitStory {
    repos: Repos,
    tab: git_ui::Tab,
    access: Access,
    making: Option<mpsc::Receiver<anyhow::Result<Made>>>,
    panel: Option<git_ui::GitPanel>,
    scanned: bool,
    failed: Option<String>,
    // Dropped after the panel, whose scans read these repos.
    _scratch: Option<Scratch>,
}

impl GitStory {
    fn advance(&mut self) {
        if let Some(making) = &self.making {
            let made = match making.try_recv() {
                Ok(made) => made,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err(anyhow::anyhow!("making the repos stopped"))
                }
            };
            self.making = None;
            if let Err(error) = made.and_then(|made| self.open(made)) {
                self.failed = Some(error.to_string());
            }
        }
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        if self.scanned || panel.scanning() {
            return;
        }
        self.scanned = true;
        if self.tab == git_ui::Tab::Remote {
            for path in [
                "app/controllers/users_controller.rb",
                "spec/requests/users_spec.rb",
            ] {
                panel.mark_reviewed(0, path);
            }
        }
    }

    fn open(&mut self, made: Made) -> anyhow::Result<()> {
        let sources = made
            .roots
            .iter()
            .map(|root| git_ui::RepoSource {
                name: root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                root: root.clone(),
                default_branch: "main".into(),
                expected_branch: "feat-login".into(),
                kept: false,
                origin: None,
            })
            .collect();
        let mut panel = git_ui::GitPanel::new(sources, None, Arc::new(|| {}));
        panel = match self.access {
            Access::Local => panel,
            Access::Host => panel.with_access(Arc::new(fixture::WireGit)),
            Access::Unserved => panel.with_access(Arc::new(git::access::UnservedGit::new(
                "pom 0.9.6 on dev cannot show its repos here. Update pom there to use Git in this window.",
            ))),
        };
        if matches!(self.repos, Repos::Remote(_)) {
            panel = panel.with_pull_requests(fixture::pull_requests(
                &made.roots,
                "feat-login",
                &made.scratch.path().join("pr-state"),
            )?);
        }
        let (width, height) = PANEL;
        panel.render(width, height);
        panel.set_tab(self.tab);
        self.panel = Some(panel);
        self._scratch = Some(made.scratch);
        Ok(())
    }
}

impl Story for GitStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        self.advance();
        match (&mut self.panel, &self.failed) {
            (Some(panel), _) => panel.render(width, height),
            (None, Some(error)) => ui::label(error.clone()).color(ui::theme().error).into(),
            (None, None) => ui::div().into(),
        }
    }

    fn pending(&mut self) -> bool {
        self.advance();
        self.failed.is_none() && !self.scanned
    }

    fn click(&mut self, id: u64) {
        if let Some(panel) = self.panel.as_mut() {
            panel.click(id);
        }
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        self.panel.as_mut().is_some_and(|panel| panel.set_hover(id))
    }

    fn scroll(&mut self, dy: f32) -> bool {
        self.panel.as_mut().is_some_and(|panel| panel.scroll(dy))
    }

    fn text(&mut self, text: &str) -> bool {
        self.panel.as_mut().is_some_and(|panel| panel.text(text))
    }

    fn key(&mut self, key: workspace::EditKey, shift: bool) -> bool {
        self.panel
            .as_mut()
            .is_some_and(|panel| panel.key(key, shift))
    }
}

fn build(repos: Repos, tab: git_ui::Tab, access: Access) -> anyhow::Result<Box<dyn Story>> {
    let (done, making) = mpsc::channel();
    std::thread::Builder::new()
        .name("git-story".into())
        .spawn(move || {
            let made = Scratch::new("git").and_then(|scratch| {
                let root = scratch.path().join("git");
                let roots = match repos {
                    Repos::Changes(state) => fixture::create(&root, state)?,
                    Repos::Remote(state) => fixture::create_remote(&root, state)?,
                };
                Ok(Made { scratch, roots })
            });
            // The story may be gone by now; its repos then go with the scratch.
            let _ = done.send(made);
        })?;
    Ok(Box::new(GitStory {
        repos,
        tab,
        access,
        making: Some(making),
        panel: None,
        scanned: false,
        failed: None,
        _scratch: None,
    }))
}

const PANEL: (f32, f32) = (330.0, 820.0);

pub fn stories() -> Vec<StoryDef> {
    use fixture::{RemoteState, State};
    use git_ui::Tab;
    let story = |id, title, build| StoryDef {
        id,
        page: "Organisms",
        group: "Git panel",
        title,
        size: PANEL,
        build,
    };
    vec![
        story("git-panel-changes", "Changes", || {
            build(Repos::Changes(State::Staged), Tab::Changes, Access::Local)
        }),
        story("git-panel-unstaged", "Nothing staged", || {
            build(Repos::Changes(State::Unstaged), Tab::Changes, Access::Local)
        }),
        story("git-panel-remote", "Remote", || {
            build(
                Repos::Remote(RemoteState::Differ),
                Tab::Remote,
                Access::Local,
            )
        }),
        story("git-panel-in-sync", "In sync with origin", || {
            build(
                Repos::Remote(RemoteState::InSync),
                Tab::Remote,
                Access::Local,
            )
        }),
        story("git-panel-history", "History", || {
            build(
                Repos::Remote(RemoteState::Differ),
                Tab::History,
                Access::Local,
            )
        }),
        story("git-panel-host", "On a host", || {
            build(Repos::Changes(State::Staged), Tab::Changes, Access::Host)
        }),
        story("git-panel-host-unserved", "Host pom too old", || {
            build(
                Repos::Changes(State::Unstaged),
                Tab::Changes,
                Access::Unserved,
            )
        }),
    ]
}
