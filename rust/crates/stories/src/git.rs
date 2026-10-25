//! The Git panel over scratch repositories with their own origins: changes, the branch against origin, its
//! history, and the same panel in the window of a project on a host.

use std::sync::Arc;

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

struct GitStory {
    panel: git_ui::GitPanel,
    // Dropped after the panel, whose scans read these repos.
    _scratch: Scratch,
}

impl Story for GitStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        self.panel.render(width, height)
    }

    fn click(&mut self, id: u64) {
        self.panel.click(id);
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        self.panel.set_hover(id)
    }

    fn scroll(&mut self, dy: f32) -> bool {
        self.panel.scroll(dy)
    }

    fn text(&mut self, text: &str) -> bool {
        self.panel.text(text)
    }

    fn key(&mut self, key: workspace::EditKey, shift: bool) -> bool {
        self.panel.key(key, shift)
    }
}

fn build(repos: Repos, tab: git_ui::Tab, access: Access) -> anyhow::Result<Box<dyn Story>> {
    let scratch = Scratch::new("git")?;
    let root = scratch.path().join("git");
    let roots = match repos {
        Repos::Changes(state) => fixture::create(&root, state)?,
        Repos::Remote(state) => fixture::create_remote(&root, state)?,
    };
    let sources = roots
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
    panel = match access {
        Access::Local => panel,
        Access::Host => panel.with_access(Arc::new(fixture::WireGit)),
        Access::Unserved => panel.with_access(Arc::new(git::access::UnservedGit::new(
            "pom 0.9.6 on dev cannot show its repos here. Update pom there to use Git in this window.",
        ))),
    };
    if matches!(repos, Repos::Remote(_)) {
        panel = panel.with_pull_requests(fixture::pull_requests(
            &roots,
            "feat-login",
            &scratch.path().join("pr-state"),
        )?);
    }
    let (width, height) = PANEL;
    panel.render(width, height);
    panel.wait_for_scan();
    panel.set_tab(tab);
    if tab == git_ui::Tab::Remote {
        for path in [
            "app/controllers/users_controller.rb",
            "spec/requests/users_spec.rb",
        ] {
            panel.mark_reviewed(0, path);
        }
    }
    Ok(Box::new(GitStory {
        panel,
        _scratch: scratch,
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
