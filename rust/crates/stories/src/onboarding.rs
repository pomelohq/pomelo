//! The new project page from the repos it starts from to a working setup: the form's steps, the run, a failed
//! check being fixed and the page once it is done.

use crate::page::tab;
use crate::StoryDef;

const PAGE: (f32, f32) = (1000.0, 900.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, build| StoryDef {
        id,
        page: "Organisms",
        group: "Onboarding",
        title,
        size: PAGE,
        build,
    };
    vec![
        story("onboarding-repos", "Repos", || {
            tab(onboarding_ui::preview_page("repos"))
        }),
        story("onboarding-setup", "Setup", || {
            tab(onboarding_ui::preview_page("setup"))
        }),
        story("onboarding-review", "Review", || {
            tab(onboarding_ui::preview_page("review"))
        }),
        story("onboarding-progress", "Setting up", || {
            tab(onboarding_ui::preview_page("progress"))
        }),
        story("onboarding-failed", "A check failed", || {
            tab(onboarding_ui::preview_page("failed"))
        }),
        story("onboarding-fixing", "Fixing a check", || {
            tab(onboarding_ui::preview_page("fixing"))
        }),
        story("onboarding-skipped", "Agent skipped", || {
            tab(onboarding_ui::preview_page("skipped"))
        }),
        story("onboarding-done", "Done", || {
            tab(onboarding_ui::preview_page("done"))
        }),
    ]
}
