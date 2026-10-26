//! The Settings window: its pages, a search across them, and the host tab a remote window adds beside This Mac.
//! Changes stay in the story; nothing reaches the settings file.

use crate::fixtures::settings as fixture;
use crate::{Story, StoryDef};

struct SettingsStory {
    settings: settings::Settings,
    pages: settings_ui::PageState,
    selected: usize,
    expanded: Vec<bool>,
    hovered: Option<u64>,
    search: String,
    searching: bool,
}

impl SettingsStory {
    fn host_shown(&self) -> bool {
        self.pages.host.as_ref().is_some_and(|host| host.shown)
    }

    fn show_tab(&mut self, host_tab: bool) {
        let Some(host) = self.pages.host.as_mut() else {
            return;
        };
        host.shown = host_tab;
        if host_tab && !settings_ui::HOST_CATEGORIES.contains(&self.selected) {
            self.selected = settings_ui::GENERAL;
        }
        if let Some(expanded) = self.expanded.get_mut(self.selected) {
            *expanded = true;
        }
    }
}

impl Story for SettingsStory {
    fn render(&mut self, width: f32, height: f32) -> ui::Node {
        let painted = settings_ui::panel(
            self.selected,
            self.hovered,
            &self.expanded,
            &self.settings,
            &self.pages,
            width,
            height,
            None,
            &self.search,
            self.searching,
        );
        ui::div().w_px(width).h_px(height).painted(painted).into()
    }

    fn click(&mut self, id: u64) {
        self.searching = id == settings_ui::CTRL_SEARCH;
        match id {
            settings_ui::CTRL_TAB_MAC | settings_ui::CTRL_TAB_HOST => {
                self.show_tab(id == settings_ui::CTRL_TAB_HOST);
            }
            settings_ui::CTRL_SEARCH_CLEAR => self.search.clear(),
            settings_ui::NAV_JUMP_BASE..settings_ui::NAV_JUMP_END => {
                let category =
                    ((id - settings_ui::NAV_JUMP_BASE) / settings_ui::NAV_JUMP_STRIDE) as usize;
                self.selected = category;
                if let Some(expanded) = self.expanded.get_mut(category) {
                    *expanded = true;
                }
            }
            _ if (settings_ui::NAV_TOGGLE_BASE
                ..settings_ui::NAV_TOGGLE_BASE + settings_ui::CATEGORY_COUNT as u64)
                .contains(&id) =>
            {
                let category = (id - settings_ui::NAV_TOGGLE_BASE) as usize;
                if let Some(expanded) = self.expanded.get_mut(category) {
                    *expanded = !*expanded;
                }
            }
            // The host's values live on the host; this story has none to change.
            _ if self.host_shown() => {}
            _ => {
                settings_ui::handle_control(id, &mut self.settings);
            }
        }
    }

    fn hover(&mut self, id: Option<u64>) -> bool {
        let changed = self.hovered != id;
        self.hovered = id;
        changed
    }

    fn text(&mut self, text: &str) -> bool {
        if !self.searching {
            return false;
        }
        self.search.push_str(text);
        true
    }

    fn key(&mut self, key: workspace::EditKey, _shift: bool) -> bool {
        if !self.searching || key != workspace::EditKey::Backspace {
            return false;
        }
        self.search.pop().is_some()
    }
}

#[derive(Clone, Copy)]
enum Host {
    None,
    /// Opened from a remote window, on This Mac's tab.
    Mac,
    /// On the host's tab, its settings in this state (see `fixture::host_page`).
    Shown(&'static str),
}

fn build(selected: usize, search: &'static str, host: Host) -> anyhow::Result<Box<dyn Story>> {
    let settings = settings::Settings::default();
    let pages = settings_ui::PageState {
        project: fixture::project_page(!matches!(host, Host::None)),
        general: settings_ui::GeneralPage {
            start_at_login: false,
            version: env!("CARGO_PKG_VERSION").into(),
            updates_apply: false,
            update_note: None,
            update_button: None,
        },
        keymap: settings_ui::KeymapPage {
            rows: workspace::keymap::Action::ALL
                .iter()
                .map(|action| {
                    (
                        action.label().to_string(),
                        action.name().to_string(),
                        workspace::keymap::Keymap::defaults()
                            .binding_for(*action)
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            problems: Vec::new(),
        },
        host: match host {
            Host::None => None,
            Host::Mac => Some(fixture::host_page(false, "")),
            Host::Shown(state) => Some(fixture::host_page(true, state)),
        },
        ..settings_ui::PageState::default()
    };
    Ok(Box::new(SettingsStory {
        settings,
        pages,
        selected,
        expanded: (0..settings_ui::CATEGORY_COUNT)
            .map(|index| index == selected)
            .collect(),
        hovered: None,
        search: search.to_string(),
        searching: false,
    }))
}

const WINDOW: (f32, f32) = (920.0, 760.0);

pub fn stories() -> Vec<StoryDef> {
    let story = |id, title, build| StoryDef {
        id,
        page: "Organisms",
        group: "Settings window",
        title,
        size: WINDOW,
        build,
    };
    vec![
        story("settings-general", "General", || {
            build(settings_ui::GENERAL, "", Host::None)
        }),
        story("settings-appearance", "Appearance", || {
            build(settings_ui::APPEARANCE, "", Host::None)
        }),
        story("settings-search", "Searching for font", || {
            build(settings_ui::APPEARANCE, "font", Host::None)
        }),
        story("settings-mac-tab", "This Mac beside a host", || {
            build(settings_ui::AGENT, "", Host::Mac)
        }),
        story("settings-host-tab", "The host's settings", || {
            build(settings_ui::GENERAL, "", Host::Shown("ready"))
        }),
        story("settings-host-reading", "Reading the host", || {
            build(settings_ui::GENERAL, "", Host::Shown("reading"))
        }),
        story("settings-host-unreachable", "Host unreachable", || {
            build(settings_ui::GENERAL, "", Host::Shown("unreachable"))
        }),
        story("settings-host-refused", "Host refused a value", || {
            build(settings_ui::DEV_SERVICES, "", Host::Shown("refused"))
        }),
    ]
}
