//! The agent usage page: what the project's agents spent, by workspace, agent or model, and before any ran.

use std::cell::RefCell;
use std::rc::Rc;

use agent_usage_ui::{Group, UsagePage, UsageState};

use crate::page::tab;
use crate::{Story, StoryDef};

fn page(change: impl FnOnce(&mut UsageState)) -> anyhow::Result<Box<dyn Story>> {
    let mut state = agent_usage_ui::preview_state();
    change(&mut state);
    tab(UsagePage::new(Rc::new(RefCell::new(state))))
}

const PAGE: (f32, f32) = (1000.0, 1160.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, size, build| StoryDef {
        id,
        page: "Organisms",
        group: "Usage page",
        title,
        size,
        build,
    };
    vec![
        story("usage-page", "By workspace", PAGE, || page(|_| {})),
        story("usage-page-agents", "By agent", PAGE, || {
            page(|state| state.group = Group::Agent)
        }),
        story("usage-page-models", "By model, 30 days", PAGE, || {
            page(|state| {
                state.group = Group::Model;
                state.period = 30;
            })
        }),
        story("usage-page-no-costs", "Costs hidden", PAGE, || {
            page(|state| state.show_costs = false)
        }),
        story(
            "usage-page-empty",
            "No agent ran yet",
            (1000.0, 640.0),
            || page(|state| state.turns.clear()),
        ),
    ]
}
