//! The dev services log: requests through the dev proxy and webhooks fanned out to workspaces, with one opened,
//! filtered to errors, before any arrived, and while the servers are off or held elsewhere.

use std::cell::RefCell;
use std::rc::Rc;

use dev_services_ui::{DevRequestsPage, DevRequestsState, ServerStatus};

use crate::page::tab;
use crate::{Story, StoryDef};

fn page(change: impl FnOnce(&mut DevRequestsState)) -> anyhow::Result<Box<dyn Story>> {
    let mut state = dev_services_ui::preview_state();
    change(&mut state);
    tab(DevRequestsPage::new(Rc::new(RefCell::new(state))))
}

fn stopped(state: &mut DevRequestsState) {
    for server in [&mut state.proxy, &mut state.webhook] {
        *server = ServerStatus {
            running: false,
            ..server.clone()
        };
    }
    state.entries.clear();
    state.selected = None;
    state.payloads = None;
}

const PAGE: (f32, f32) = (1000.0, 760.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, build| StoryDef {
        id,
        page: "Organisms",
        group: "Dev services log",
        title,
        size: PAGE,
        build,
    };
    vec![
        story("dev-services-webhook", "Webhook fanned out", || {
            page(|_| {})
        }),
        story("dev-services-request", "Request opened", || {
            tab(dev_services_ui::board_page(true))
        }),
        story("dev-services-errors", "Errors only", || {
            page(|state| {
                state.errors_only = true;
                state.selected = None;
                state.payloads = None;
            })
        }),
        story("dev-services-empty", "No requests yet", || {
            tab(dev_services_ui::board_page(false))
        }),
        story("dev-services-stopped", "Servers stopped", || page(stopped)),
        story("dev-services-elsewhere", "Held by pom proxy", || {
            page(|state| {
                stopped(state);
                state.served_elsewhere = true;
            })
        }),
    ]
}
