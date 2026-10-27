//! The node_modules store page: what each repo's lockfile versions hold and who uses them, while sizes are
//! measured, before anything is read, when it cannot be read, and the store of a host sent over as JSON.

use std::cell::RefCell;
use std::rc::Rc;

use module_store_ui::{StorePage, StoreState, StoreUpdate};

use crate::page::tab;
use crate::{Story, StoryDef};

const ROOT: &str = "~/.local/state/pom/nm-store";

fn page(state: StoreState) -> anyhow::Result<Box<dyn Story>> {
    tab(StorePage::new(Rc::new(RefCell::new(state)), ROOT.into()))
}

fn loading() -> anyhow::Result<Box<dyn Story>> {
    page(StoreState {
        method: "copy-on-write, workspaces take no extra disk".into(),
        ..StoreState::default()
    })
}

fn empty() -> anyhow::Result<Box<dyn Story>> {
    let mut state = module_store_ui::preview_state();
    if let Some(overview) = state.overview.as_mut() {
        overview.repos.clear();
        overview.others.clear();
    }
    state.running.clear();
    page(state)
}

fn failed() -> anyhow::Result<Box<dyn Story>> {
    let mut state = StoreState::default();
    state.apply(
        StoreUpdate::Failed {
            message: "The store could not be read: permission denied (os error 13)".into(),
        },
        0,
    );
    page(state)
}

/// The host's store as its pom server reports it: every update crosses the wire as JSON.
fn on_host() -> anyhow::Result<Box<dyn Story>> {
    let local = module_store_ui::preview_state();
    let overview = local
        .overview
        .ok_or_else(|| anyhow::anyhow!("the preview has no overview"))?;
    let updates = [
        StoreUpdate::Place {
            root: "/home/dev/.local/state/pom/nm-store on dev".into(),
            method: "hardlinks, workspaces share one copy".into(),
        },
        StoreUpdate::Overview {
            overview,
            running: local.running,
            pending: 0,
            message: None,
            failed: false,
            flash: None,
        },
    ];
    let mut state = StoreState::default();
    for update in updates {
        let wire = serde_json::to_string(&update)?;
        state.apply(serde_json::from_str(&wire)?, local.now);
    }
    tab(StorePage::new(Rc::new(RefCell::new(state)), String::new()))
}

fn host_refused() -> anyhow::Result<Box<dyn Story>> {
    let mut state = StoreState::default();
    state.apply(
        StoreUpdate::Failed {
            message: "pom 0.9.6 on dev cannot show its node_modules store here. Update pom there to see it in this window.".into(),
        },
        0,
    );
    tab(StorePage::new(Rc::new(RefCell::new(state)), String::new()))
}

const PAGE: (f32, f32) = (1000.0, 640.0);
const SHORT: (f32, f32) = (1000.0, 220.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, size, build| StoryDef {
        id,
        page: "Organisms",
        group: "Module store",
        title,
        size,
        build,
    };
    vec![
        story("module-store", "Store", PAGE, || {
            tab(module_store_ui::preview_page())
        }),
        story("module-store-measuring", "Measuring sizes", PAGE, || {
            tab(module_store_ui::board_page("measuring"))
        }),
        story("module-store-loading", "Reading lockfiles", SHORT, loading),
        story("module-store-empty", "Nothing stored", SHORT, empty),
        story("module-store-failed", "Could not read", SHORT, failed),
        story("module-store-host", "On a host", PAGE, on_host),
        story(
            "module-store-host-refused",
            "Host pom too old",
            SHORT,
            host_refused,
        ),
    ]
}
