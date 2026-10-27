//! The pull request tab of a branch: its overview and checks, and while it looks one up, finds none or fails.

use crate::fixtures::pull_request as fixture;
use crate::page::tab;
use crate::StoryDef;

const TAB: (f32, f32) = (900.0, 780.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, build| StoryDef {
        id,
        page: "Organisms",
        group: "Pull request",
        title,
        size: TAB,
        build,
    };
    vec![
        story("pull-request-overview", "Overview", || {
            tab(fixture::item("overview"))
        }),
        story("pull-request-checks", "Checks", || {
            tab(fixture::item("checks"))
        }),
        story("pull-request-draft", "Draft with nothing written", || {
            tab(fixture::item("empty"))
        }),
        story("pull-request-loading", "Looking it up", || {
            tab(fixture::item("loading"))
        }),
        story("pull-request-none", "No pull request yet", || {
            tab(fixture::item("none"))
        }),
        story("pull-request-error", "Lookup failed", || {
            tab(fixture::item("error"))
        }),
    ]
}
