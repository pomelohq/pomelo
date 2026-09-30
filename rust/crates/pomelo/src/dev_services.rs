use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dev_services_ui::{DevRequestsPage, DevRequestsState, Request, ServerStatus};
use winit::window::WindowId;

use crate::App;

const REFRESH_EVERY: Duration = Duration::from_millis(500);
const ENTRIES_SHOWN: usize = 300;

#[derive(Default)]
pub(crate) struct DevRequestsTabs {
    pages: Vec<(WindowId, dev_services_ui::Shared)>,
    refreshed: Option<Instant>,
}

impl App {
    pub(crate) fn desired_serve(&self) -> pom_proxy::Serve {
        pom_proxy::Serve {
            proxy: self.settings.dev_proxy_enabled,
            webhook: self.settings.webhook_enabled,
        }
    }

    /// Restart the servers when Settings turned one on or off or moved a port.
    pub(crate) fn apply_dev_services_settings(&mut self) {
        let Some(proxy) = &self.dev_proxy else {
            self.sync_dev_proxy();
            return;
        };
        if proxy.serve() != self.desired_serve() || proxy.ports() != pom_proxy::Ports::from_env() {
            self.restart_dev_proxy();
            self.dev_requests.refreshed = None;
        }
    }

    fn server_statuses(&self) -> (ServerStatus, ServerStatus, bool) {
        let ports = self
            .dev_proxy
            .as_ref()
            .map_or_else(pom_proxy::Ports::from_env, pom_proxy::DevProxy::ports);
        let proxy_running = self
            .dev_proxy
            .as_ref()
            .is_some_and(pom_proxy::DevProxy::proxy_running);
        let webhook_running = self
            .dev_proxy
            .as_ref()
            .is_some_and(pom_proxy::DevProxy::webhook_running);
        let served_elsewhere = !proxy_running && pom_proxy::listening(ports.proxy);
        (
            ServerStatus {
                enabled: self.settings.dev_proxy_enabled,
                running: proxy_running,
                port: ports.proxy,
            },
            ServerStatus {
                enabled: self.settings.webhook_enabled,
                running: webhook_running,
                port: ports.webhook,
            },
            served_elsewhere,
        )
    }

    pub(crate) fn dev_services_page(&self) -> settings_ui::DevServicesPage {
        let (proxy, webhook, served_elsewhere) = self.server_statuses();
        settings_ui::DevServicesPage {
            proxy_running: proxy.running,
            webhook_running: webhook.running,
            proxy_port: proxy.port,
            webhook_port: webhook.port,
            served_elsewhere,
            port_from_env: std::env::var_os("POM_WEB_PORT").is_some(),
            modules_method: String::new(),
        }
    }

    pub(crate) fn open_dev_requests(&mut self, id: WindowId) {
        let shared = match self
            .dev_requests
            .pages
            .iter()
            .find(|(window, _)| *window == id)
        {
            Some((_, shared)) => shared.clone(),
            None => {
                let shared: dev_services_ui::Shared =
                    Rc::new(RefCell::new(DevRequestsState::default()));
                self.dev_requests.pages.push((id, shared.clone()));
                shared
            }
        };
        self.dev_requests.refreshed = None;
        self.poll_dev_requests();
        let page = DevRequestsPage::new(shared);
        self.with_workspace_view(id, |view, _| view.open_page(Box::new(page)));
        if let Some(main) = self.mains.get_mut(&id) {
            main.dirty = true;
        }
    }

    pub(crate) fn poll_dev_requests(&mut self) {
        let restart = self.dev_requests.pages.iter().any(|(_, shared)| {
            let mut state = shared.borrow_mut();
            let asked = state.requests.contains(&Request::Restart);
            state.requests.clear();
            asked
        });
        if restart {
            self.restart_dev_proxy();
            self.dev_requests.refreshed = None;
        }
        // A tab closed by hand leaves only this list's hold on its state.
        self.dev_requests
            .pages
            .retain(|(_, shared)| Rc::strong_count(shared) > 1);
        if let Some(proxy) = &self.dev_proxy {
            for (id, shared) in &self.dev_requests.pages {
                let wanted = shared.borrow().wants_payloads();
                let Some((request, response)) = wanted.and_then(|seq| proxy.payloads(seq)) else {
                    continue;
                };
                if let Some(seq) = wanted {
                    let before = shared.borrow().version;
                    shared.borrow_mut().set_payloads(seq, request, response);
                    if shared.borrow().version != before {
                        if let Some(main) = self.mains.get_mut(id) {
                            main.dirty = true;
                        }
                    }
                }
            }
        }
        if self.dev_requests.pages.is_empty()
            || self
                .dev_requests
                .refreshed
                .is_some_and(|at| at.elapsed() < REFRESH_EVERY)
        {
            return;
        }
        self.dev_requests.refreshed = Some(Instant::now());
        let (proxy, webhook, served_elsewhere) = self.server_statuses();
        let entries = self
            .dev_proxy
            .as_ref()
            .map(|proxy| proxy.log(ENTRIES_SHOWN))
            .unwrap_or_default();
        let changed: Vec<WindowId> = self
            .dev_requests
            .pages
            .iter()
            .filter(|(_, shared)| {
                shared.borrow_mut().update(
                    proxy.clone(),
                    webhook.clone(),
                    served_elsewhere,
                    entries.clone(),
                )
            })
            .map(|(id, _)| *id)
            .collect();
        for id in changed {
            if let Some(main) = self.mains.get_mut(&id) {
                main.dirty = true;
            }
        }
    }
}
