//! The terminal panel: a pane group of terminal items, driven by the workspace. Panes split, each with its
//! own tabs and find bar; closing the last terminal closes the panel.

use std::path::{Path, PathBuf};

use terminal::Waker;
use ui::{label, theme, IconKind, Node, Painted, Rect};
use workspace::pane::{Pane, PaneCommand};
use workspace::pane_group::SplitDirection;
use workspace::pane_group_view::{
    GroupClick, PaneButton, PaneButtonAction, PaneGroupConfig, PaneGroupView,
};
use workspace::persistence::{SerializedItem, SerializedMember};
use workspace::{
    DividerAxis, EditorLayout, Item, TerminalOpenTarget, TerminalPanelView, TerminalSyncOutcome,
    AGENT_VIEW_BASE, TERMINAL_VIEW_BASE,
};

use crate::item::{Host, TerminalItem};

const MAX_PANES: usize = 8;

fn terminal_of(pane: &mut Pane) -> Option<&mut TerminalItem> {
    pane.active_item_mut()?
        .as_any_mut()?
        .downcast_mut::<TerminalItem>()
}

/// A new shell in `cwd` (the project root when `None`), noting the failure for the empty panel to show.
#[derive(Clone, Debug)]
pub struct HolderScope {
    pub dir: pom_ptyhost::SocketDir,
    pub binary: PathBuf,
    pub prefix: String,
}

impl HolderScope {
    fn new_name(&self, id: u64) -> String {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        format!("{}-{millis}-{id}", self.prefix)
    }

    fn options(&self, name: String) -> terminal::HolderOptions {
        terminal::HolderOptions {
            dir: self.dir.clone(),
            name,
            binary: self.binary.clone(),
            attach_only: false,
        }
    }

    fn owns(&self, name: &str) -> bool {
        name.strip_prefix(&self.prefix)
            .is_some_and(|rest| rest.starts_with('-'))
    }
}

fn spawn_terminal(
    root: &Path,
    waker: &Waker,
    next_item_id: &mut u64,
    spawn_error: &mut Option<String>,
    cwd: Option<PathBuf>,
    holder: Option<terminal::HolderOptions>,
) -> Option<TerminalItem> {
    let id = *next_item_id;
    *next_item_id += 1;
    match TerminalItem::spawn(id, root.to_path_buf(), cwd, waker.clone(), holder) {
        Ok(item) => {
            *spawn_error = None;
            Some(item)
        }
        Err(error) => {
            *spawn_error = Some(format!("Failed to start the shell: {error}"));
            None
        }
    }
}

pub struct TerminalPanel {
    root: PathBuf,
    waker: Waker,
    holders: Option<HolderScope>,
    panes: PaneGroupView,
    next_item_id: u64,
    focused: bool,
    /// The terminal last told it has focus, so a change of active tab or pane moves focus reporting along.
    focus_holder: Option<String>,
    spawn_error: Option<String>,
    /// The user closed the last terminal; reported on the next sync so the panel closes like when shells exit.
    closed_last: bool,
}

impl TerminalPanel {
    pub fn new(root: PathBuf, waker: Waker) -> Self {
        Self::with_panes(root, waker, Self::panel_panes())
    }

    /// The agent dock's panel: one pane of agent sessions, which are opened for it rather than from it.
    pub fn agent_dock(root: PathBuf, waker: Waker) -> Self {
        let mut panes = PaneGroupView::new(PaneGroupConfig {
            id_base: AGENT_VIEW_BASE,
            show_nav: false,
            buttons: Vec::new(),
            max_panes: 1,
            split_filter: None,
            zoom_whole_group: true,
        });
        panes.set_focused(false);
        Self::with_panes(root, waker, panes)
    }

    fn panel_panes() -> PaneGroupView {
        let mut panes = PaneGroupView::new(PaneGroupConfig {
            id_base: TERMINAL_VIEW_BASE,
            show_nav: false,
            buttons: vec![
                PaneButton {
                    icon: IconKind::Plus,
                    action: PaneButtonAction::NewItem,
                },
                PaneButton {
                    icon: IconKind::PanelRight,
                    action: PaneButtonAction::Split(SplitDirection::Right),
                },
                PaneButton {
                    icon: IconKind::PanelBottom,
                    action: PaneButtonAction::Split(SplitDirection::Down),
                },
                PaneButton {
                    icon: IconKind::Maximize,
                    action: PaneButtonAction::ToggleZoom,
                },
            ],
            max_panes: MAX_PANES,
            split_filter: Some(|item| item.as_any().is_some_and(|any| any.is::<TerminalItem>())),
            zoom_whole_group: true,
        });
        panes.set_focused(false);
        panes
    }

    fn with_panes(root: PathBuf, waker: Waker, panes: PaneGroupView) -> Self {
        Self {
            root,
            waker,
            holders: None,
            panes,
            next_item_id: 0,
            focused: false,
            focus_holder: None,
            spawn_error: None,
            closed_last: false,
        }
    }

    pub fn with_holders(root: PathBuf, waker: Waker, scope: HolderScope) -> Self {
        let mut panel = Self::new(root, waker);
        panel.holders = Some(scope);
        panel
    }

    fn spawn_item(&mut self, cwd: Option<PathBuf>) -> Option<TerminalItem> {
        let holder = self
            .holders
            .as_ref()
            .map(|scope| scope.options(scope.new_name(self.next_item_id)));
        spawn_terminal(
            &self.root,
            &self.waker,
            &mut self.next_item_id,
            &mut self.spawn_error,
            cwd,
            holder,
        )
    }

    fn reap_unclaimed(&self, kept: &[String]) {
        let Some(scope) = self.holders.clone() else {
            return;
        };
        let unclaimed: Vec<String> = scope
            .dir
            .holders()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| scope.owns(name) && !kept.contains(name))
            .collect();
        if !unclaimed.is_empty() {
            std::thread::spawn(move || scope.dir.kill_holders_now(&unclaimed));
        }
    }

    fn active_pane(&mut self) -> Option<&mut Pane> {
        self.panes.active_pane_mut()
    }

    fn active_terminal(&mut self) -> Option<&mut TerminalItem> {
        terminal_of(self.active_pane()?)
    }

    /// Tell terminals when focus moves between them (a tab or pane switch, or the panel gaining focus), so the
    /// shell's focus reporting and the cursor follow.
    fn reconcile_focus(&mut self) {
        let focused = self.focused;
        let holder = if focused {
            self.active_terminal().and_then(|item| item.id())
        } else {
            None
        };
        if holder == self.focus_holder {
            return;
        }
        let previous = self.focus_holder.take();
        self.all_terminals(&mut |item| {
            let id = item.id();
            if id.is_some() && id == previous {
                item.focus_changed(false);
            }
            if id.is_some() && id == holder {
                item.focus_changed(true);
            }
        });
        self.focus_holder = holder;
    }

    /// Split the pane at `path` with a new shell beside it.
    fn split(&mut self, path: &[usize], direction: SplitDirection) {
        if self.panes.group.leaf_count() >= MAX_PANES {
            return;
        }
        let Some(item) = self.spawn_item(None) else {
            return;
        };
        self.panes.split(path, direction, Some(Box::new(item)));
    }

    /// Close tab `index` in the pane at `path`; an emptied pane leaves the group, and the last one closes the
    /// panel.
    fn close_tab(&mut self, path: &[usize], index: usize) {
        self.panes.close_tab(path, index);
        if self.panes.is_empty() {
            self.closed_last = true;
        }
    }

    fn all_terminals(&mut self, f: &mut dyn FnMut(&mut TerminalItem)) {
        self.panes.for_each_item_mut(&mut |item| {
            if let Some(terminal) = item
                .as_any_mut()
                .and_then(|any| any.downcast_mut::<TerminalItem>())
            {
                f(terminal);
            }
        });
    }
}

impl TerminalPanelView for TerminalPanel {
    fn panes(&mut self) -> &mut PaneGroupView {
        &mut self.panes
    }

    fn panes_ref(&self) -> &PaneGroupView {
        &self.panes
    }

    fn save_panes(&self) -> SerializedMember {
        self.panes.serialize()
    }

    fn restore_panes(&mut self, saved: &SerializedMember) -> bool {
        let (root, waker, holders) = (&self.root, &self.waker, &self.holders);
        let (next_item_id, spawn_error) = (&mut self.next_item_id, &mut self.spawn_error);
        let mut kept: Vec<String> = Vec::new();
        let mut make_item = |item: &SerializedItem| -> Option<Box<dyn Item>> {
            let cwd = crate::item::saved_cwd(item)?;
            let cwd = cwd.is_dir().then_some(cwd);
            let holder = holders.as_ref().map(|scope| {
                let name = crate::item::saved_holder(item)
                    .filter(|name| scope.owns(name) && scope.dir.holder_alive(name))
                    .unwrap_or_else(|| scope.new_name(*next_item_id));
                kept.push(name.clone());
                scope.options(name)
            });
            spawn_terminal(root, waker, next_item_id, spawn_error, cwd, holder)
                .map(|terminal| Box::new(terminal) as Box<dyn Item>)
        };
        let restored = self.panes.restore(saved, &mut make_item);
        self.reap_unclaimed(&kept);
        restored
    }

    fn render(&mut self, region: Rect, focused: bool) -> (Painted, EditorLayout) {
        self.focused = focused;
        self.panes.set_focused(focused);
        self.reconcile_focus();
        let mut backdrop = Painted::default();
        if self.is_empty() {
            let message = self.spawn_error.clone().unwrap_or_default();
            let node = label(message).size(13.0).color(theme().text_muted).into();
            backdrop = ui::render(&node, region);
        }
        let (panes, dividers) = self.panes.layout(region);
        (backdrop, EditorLayout { panes, dividers })
    }

    fn click(&mut self, id: u64) -> bool {
        match self.panes.click(id) {
            GroupClick::NotMine => return false,
            GroupClick::Handled => {}
            GroupClick::Search { pane, click } => self.panes.search_click(pane, click),
            GroupClick::Button { path, action } => match action {
                PaneButtonAction::Split(direction) => self.split(&path, direction),
                PaneButtonAction::NewItem => {
                    if let Some(item) = self.spawn_item(None) {
                        if let Some(pane) = self.panes.group.leaf_at_mut(&path) {
                            pane.add_item(Box::new(item));
                        }
                    }
                }
                PaneButtonAction::ToggleZoom => self.panes.toggle_zoom(),
            },
        }
        if self.panes.is_empty() {
            self.closed_last = true;
        }
        self.reconcile_focus();
        true
    }

    fn set_hover(&mut self, id: Option<u64>) -> bool {
        self.panes.set_hover(id)
    }

    fn divider_axis(&self, id: u64) -> Option<DividerAxis> {
        self.panes.divider_axis(id)
    }

    fn drag_divider(&mut self, id: u64, x: f32, y: f32) -> bool {
        self.panes.drag_divider(id, x, y)
    }

    fn is_tab(&self, id: u64) -> bool {
        self.panes.is_tab(id)
    }

    fn begin_tab_drag(&mut self, id: u64) -> bool {
        self.panes.begin_tab_drag(id)
    }

    fn update_tab_drag(&mut self, x: f32, y: f32, over: Option<(u64, Rect)>) -> bool {
        self.panes.update_tab_drag(x, y, over)
    }

    fn drop_tab(&mut self) -> bool {
        let dropped = self.panes.drop_tab();
        self.reconcile_focus();
        dropped
    }

    fn tab_drag_overlay(&self) -> Option<Rect> {
        self.panes.tab_drag_overlay()
    }

    fn tab_drag_ghost(&self) -> Option<(Node, f32, f32)> {
        self.panes.tab_drag_ghost()
    }

    fn pane_command(&mut self, command: PaneCommand) -> bool {
        let handled = match command {
            PaneCommand::Split(direction) => {
                let path = self.panes.active.clone();
                self.split(&path, direction);
                true
            }
            command => self.panes.pane_command(command),
        };
        self.reconcile_focus();
        handled
    }

    fn dragged_item(&self) -> Option<&dyn Item> {
        self.panes.dragged_item()
    }

    fn take_dragged_item(&mut self) -> Option<Box<dyn Item>> {
        let item = self.panes.take_dragged_item()?;
        if self.is_empty() {
            self.closed_last = true;
        }
        self.reconcile_focus();
        Some(item)
    }

    fn accepts_item(&self, _item: &dyn Item) -> bool {
        true
    }

    fn update_foreign_drop(
        &mut self,
        x: f32,
        y: f32,
        over: Option<(u64, Rect)>,
        item: &dyn Item,
    ) -> bool {
        self.panes.update_foreign_drop(x, y, over, item)
    }

    fn clear_foreign_drop(&mut self) {
        self.panes.clear_foreign_drop();
    }

    fn accept_foreign_item(&mut self, item: Box<dyn Item>) {
        self.closed_last = false;
        self.panes.accept_foreign_item(item);
        self.reconcile_focus();
    }

    fn focus_changed(&mut self, focused: bool) {
        self.focused = focused;
        self.panes.set_focused(focused);
        self.reconcile_focus();
    }

    fn sync(&mut self, clipboard: &dyn Fn() -> Option<String>) -> TerminalSyncOutcome {
        let host = Host {
            palette: crate::palette(&theme()),
            clipboard,
        };
        let mut outcome = TerminalSyncOutcome::default();
        let had_terminals = !self.is_empty();
        let mut exited: Vec<(u64, String)> = Vec::new();
        self.panes.group.for_each_pane_mut(&mut |pane| {
            let mut pane_changed = false;
            for item in pane.open.iter_mut() {
                let Some(id) = item.id() else {
                    continue;
                };
                let Some(terminal) = item
                    .as_any_mut()
                    .and_then(|any| any.downcast_mut::<TerminalItem>())
                else {
                    continue;
                };
                let result = terminal.sync(&host);
                pane_changed |= result.changed || result.title_changed;
                if result.clipboard_store.is_some() {
                    outcome.clipboard_store = result.clipboard_store;
                }
                if result.close {
                    exited.push((pane.id, id));
                }
            }
            if pane_changed {
                if let Some((bar, item)) = pane.search_target() {
                    if !bar.dismissed {
                        bar.refresh(item);
                    }
                }
            }
            outcome.changed |= pane_changed;
        });
        for (pane_id, item_id) in exited {
            let Some(path) = self.panes.group.path_of(pane_id) else {
                continue;
            };
            let index = self
                .panes
                .group
                .leaf_at(&path)
                .and_then(|pane| pane.index_of_id(&item_id));
            if let Some(index) = index {
                self.close_tab(&path, index);
                outcome.changed = true;
            }
        }
        outcome.closed_all =
            (had_terminals && self.is_empty()) || std::mem::take(&mut self.closed_last);
        outcome
    }

    fn open(&mut self, cwd: Option<PathBuf>) {
        self.closed_last = false;
        let Some(item) = self.spawn_item(cwd) else {
            return;
        };
        if let Some(pane) = self.active_pane() {
            pane.add_item(Box::new(item));
        }
    }

    fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    fn take_open_request(&mut self) -> Option<TerminalOpenTarget> {
        let mut request = None;
        self.all_terminals(&mut |item| {
            if request.is_none() {
                request = item.take_link_request();
            }
        });
        request
    }

    fn new_item(&mut self, cwd: Option<PathBuf>) -> Option<Box<dyn Item>> {
        self.spawn_item(cwd)
            .map(|item| Box::new(item) as Box<dyn Item>)
    }

    fn command_item(
        &mut self,
        title: String,
        cwd: PathBuf,
        argv: Vec<String>,
    ) -> Option<Box<dyn Item>> {
        let id = self.next_item_id;
        self.next_item_id += 1;
        let mut argv = argv.into_iter();
        let program = argv.next()?;
        match TerminalItem::command_output(
            id,
            cwd,
            format!("command:{id}"),
            title,
            program.clone(),
            argv.collect(),
            self.waker.clone(),
        ) {
            Ok(item) => Some(Box::new(item)),
            Err(error) => {
                eprintln!("terminal: start {program:?}: {error}");
                None
            }
        }
    }

    fn link_hovered(&self) -> bool {
        let mut hovered = false;
        self.panes.group.for_each_pane(&mut |pane| {
            hovered |= pane.active_item().is_some_and(|item| {
                item.as_any()
                    .and_then(|any| any.downcast_ref::<TerminalItem>())
                    .is_some_and(TerminalItem::hovering_link)
            });
        });
        hovered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminal::Keystroke;
    use terminal::{Terminal, TerminalOptions};
    use ui::Rgba;
    use workspace::{ItemInput, TerminalKeyOutcome};

    fn panel() -> TerminalPanel {
        // The default login shell goes through `login`, which outlives the test and keeps its pty.
        crate::set_terminal_defaults(15.0, "/bin/sh", 0);
        TerminalPanel::new(std::env::temp_dir(), std::sync::Arc::new(|| {}))
    }

    #[test]
    fn holder_process_entry() {
        let Ok(spec) = std::env::var("POMELO_TEST_HOLDER") else {
            return;
        };
        let Some((dir, name)) = spec.split_once('|') else {
            return;
        };
        let started = pom_ptyhost::listen_and_serve(
            &pom_ptyhost::SocketDir::new(dir),
            name,
            pom_ptyhost::StartOptions {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    format!("printf shell-of-{name}; exec cat"),
                ],
                dir: PathBuf::from("/"),
                env: vec![("PATH".into(), "/usr/bin:/bin".into())],
                cols: 80,
                rows: 24,
                on_exit: None,
            },
        );
        if let Ok(session) = started {
            session.wait();
        }
        std::process::exit(0);
    }

    fn start_holder(dir: &pom_ptyhost::SocketDir, name: &str) -> std::process::Child {
        let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "panel::tests::holder_process_entry",
                "--exact",
                "--nocapture",
            ])
            .env(
                "POMELO_TEST_HOLDER",
                format!("{}|{name}", dir.root().display()),
            )
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("holder process");
        pom_ptyhost::wait_for_holder(dir, name, std::time::Duration::from_secs(10))
            .expect("holder up");
        child
    }

    #[test]
    fn closing_a_tab_ends_its_shell() {
        let temp = tempfile::Builder::new()
            .prefix("pty")
            .tempdir_in("/tmp")
            .expect("temp");
        let dir = pom_ptyhost::SocketDir::new(temp.path());
        let mut holder = start_holder(&dir, "term-close-main-1");
        let scope = HolderScope {
            dir: dir.clone(),
            binary: PathBuf::from("/nonexistent"),
            prefix: "term-close-main".into(),
        };
        let mut panel =
            TerminalPanel::with_holders(std::env::temp_dir(), std::sync::Arc::new(|| {}), scope);
        let saved = SerializedMember::Pane(workspace::persistence::SerializedPane {
            active: true,
            items: vec![SerializedItem {
                kind: crate::item::TERMINAL_KIND.into(),
                data: serde_json::json!({ "cwd": "/", "holder": "term-close-main-1" }),
            }],
            active_item: Some(0),
            pinned_count: 0,
        });
        assert!(panel.restore_panes(&saved));
        panel.panes.close_tab(&[], 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while dir.holder_alive("term-close-main-1") && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!dir.holder_alive("term-close-main-1"));
        holder.wait().expect("reap holder process");
    }

    #[test]
    fn restored_tabs_reattach_and_unclaimed_shells_are_reaped() {
        let temp = tempfile::Builder::new()
            .prefix("pty")
            .tempdir_in("/tmp")
            .expect("temp");
        let dir = pom_ptyhost::SocketDir::new(temp.path());
        let mut kept = start_holder(&dir, "term-demo-main-1");
        let mut stray = start_holder(&dir, "term-demo-main-2");
        let mut other = start_holder(&dir, "term-other-main-1");
        let scope = HolderScope {
            dir: dir.clone(),
            binary: PathBuf::from("/nonexistent"),
            prefix: "term-demo-main".into(),
        };
        let mut panel =
            TerminalPanel::with_holders(std::env::temp_dir(), std::sync::Arc::new(|| {}), scope);
        let saved = SerializedMember::Pane(workspace::persistence::SerializedPane {
            active: true,
            items: vec![SerializedItem {
                kind: crate::item::TERMINAL_KIND.into(),
                data: serde_json::json!({ "cwd": "/", "holder": "term-demo-main-1" }),
            }],
            active_item: Some(0),
            pinned_count: 0,
        });
        assert!(panel.restore_panes(&saved));
        let reattached = panel
            .active_terminal()
            .and_then(|item| item.terminal().holder().map(|holder| holder.name.clone()));
        assert_eq!(reattached.as_deref(), Some("term-demo-main-1"));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while dir.holder_alive("term-demo-main-2") && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            !dir.holder_alive("term-demo-main-2"),
            "the unclaimed shell is reaped"
        );
        assert!(
            dir.holder_alive("term-demo-main-1"),
            "the restored tab's shell keeps running"
        );
        assert!(
            dir.holder_alive("term-other-main-1"),
            "another workspace's shells are left alone"
        );

        drop(panel);
        for name in ["term-demo-main-1", "term-other-main-1"] {
            dir.kill_holder(name).expect("kill");
        }
        for child in [&mut kept, &mut stray, &mut other] {
            child.wait().expect("reap holder process");
        }
    }

    #[test]
    fn search_finds_newest_match_first_and_cycles() {
        let mut panel = panel();
        let terminal = Terminal::spawn(
            TerminalOptions {
                shell: Some((
                    "/bin/sh".into(),
                    vec![
                        "-c".into(),
                        "printf 'alpha one\\nbeta\\nalpha two'; sleep 5".into(),
                    ],
                )),
                ..TerminalOptions::default()
            },
            std::sync::Arc::new(|| {}),
        )
        .unwrap();
        let item = TerminalItem::with_terminal(0, std::env::temp_dir(), terminal);
        panel.active_pane().unwrap().add_item(Box::new(item));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !panel
            .active_terminal()
            .unwrap()
            .terminal()
            .screen_text()
            .contains("two")
        {
            assert!(std::time::Instant::now() < deadline, "no output");
            panel.sync(&|| None);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            panel.panes.item_keystroke(&Keystroke::parse("cmd-f")),
            TerminalKeyOutcome::Handled
        );
        panel.panes.item_text("alpha");
        let pane = panel.active_pane().unwrap();
        assert_eq!(pane.search.matches.len(), 2);
        assert_eq!(pane.search.active_match, Some(1));
        let terminal = panel.active_terminal().unwrap().terminal();
        assert_eq!(terminal.selection_text().as_deref(), Some("alpha"));
        let highlighted = &terminal.content().search_matches;
        assert_eq!(highlighted.len(), 2);
        assert_eq!(highlighted[1].0.line - highlighted[0].0.line, 2);
        assert_eq!(
            panel.panes.item_keystroke(&Keystroke::parse("cmd-g")),
            TerminalKeyOutcome::Handled
        );
        assert_eq!(panel.active_pane().unwrap().search.active_match, Some(0));
        assert_eq!(
            panel.panes.item_keystroke(&Keystroke::parse("escape")),
            TerminalKeyOutcome::Handled
        );
        assert!(panel.active_pane().unwrap().search.dismissed);
        let terminal = panel.active_terminal().unwrap().terminal();
        assert!(terminal.content().search_matches.is_empty());
    }

    #[test]
    fn closing_the_last_tab_closes_the_panel() {
        let mut panel = panel();
        panel.open(None);
        panel.open(None);
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        panel.render(region, true);
        let close_first = panel.panes.tab_close_id(0, 0);
        assert!(panel.click(close_first));
        assert!(!panel.sync(&|| None).closed_all);
        panel.render(region, true);
        assert!(panel.click(close_first));
        assert!(panel.is_empty());
        assert!(panel.sync(&|| None).closed_all);
        assert!(!panel.sync(&|| None).closed_all);
    }

    #[test]
    fn panes_split_and_empty_panes_leave_the_group() {
        let mut panel = panel();
        panel.open(None);
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        panel.render(region, true);
        assert!(panel.click(panel.panes.button_id(0, 1)));
        assert_eq!(panel.panes.group.leaf_count(), 2);
        assert_eq!(panel.panes.active, vec![1]);
        let (_, layout) = panel.render(region, true);
        let divider = panel.panes.divider_id(0);
        assert!(layout.dividers.iter().any(|d| d.id == divider));
        assert!(matches!(
            panel.divider_axis(divider),
            Some(DividerAxis::Horizontal)
        ));
        assert!(panel.drag_divider(divider, 600.0, 10.0));
        assert!(panel.click(panel.panes.button_id(1, 2)));
        assert_eq!(panel.panes.group.leaf_count(), 3);
        panel.render(region, true);
        assert!(panel.click(panel.panes.tab_close_id(1, 0)));
        assert_eq!(panel.panes.group.leaf_count(), 2);
        assert!(!panel.is_empty());
        assert!(!panel.sync(&|| None).closed_all);
    }

    #[test]
    fn dragging_a_tab_to_an_edge_splits_and_back_merges() {
        let mut panel = panel();
        panel.open(None);
        panel.open(None);
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        panel.render(region, true);
        let first_tab = panel.panes.tab_id(0, 0);
        assert!(panel.is_tab(first_tab));
        assert!(panel.begin_tab_drag(first_tab));
        assert!(panel.update_tab_drag(790.0, 250.0, None));
        assert!(panel.tab_drag_overlay().is_some());
        assert!(panel.drop_tab());
        assert_eq!(panel.panes.group.leaf_count(), 2);
        assert_eq!(panel.panes.active, vec![1]);
        panel.render(region, true);
        let right_tab = panel.panes.tab_id(1, 0);
        assert!(panel.begin_tab_drag(right_tab));
        assert!(panel.update_tab_drag(200.0, 250.0, None));
        assert!(panel.drop_tab());
        assert_eq!(panel.panes.group.leaf_count(), 1);
        assert_eq!(
            panel.panes.group.leaf_at(&[]).map(|p| p.open.len()),
            Some(2)
        );
    }

    #[test]
    fn pane_keys_split_move_focus_and_swap() {
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        let mut panel = panel();
        panel.open(None);
        panel.render(region, true);
        assert!(panel.pane_command(PaneCommand::Split(SplitDirection::Right)));
        assert_eq!(panel.panes.group.leaf_count(), 2);
        assert_eq!(panel.panes.active, vec![1]);
        panel.render(region, true);
        assert!(!panel.pane_command(PaneCommand::ActivatePane(SplitDirection::Right)));
        assert!(panel.pane_command(PaneCommand::ActivatePane(SplitDirection::Left)));
        assert_eq!(panel.panes.active, vec![0]);
        let left_id = panel.panes.group.leaf_at(&[0]).map(|pane| pane.id);
        assert!(panel.pane_command(PaneCommand::SwapPane(SplitDirection::Right)));
        assert_eq!(panel.panes.active, vec![1]);
        assert_eq!(panel.panes.group.leaf_at(&[1]).map(|pane| pane.id), left_id);
    }

    #[test]
    fn a_dragged_tab_moves_to_another_group_and_back() {
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        let mut source = panel();
        source.open(None);
        source.render(region, true);
        let mut target = panel();
        target.open(None);
        target.render(region, true);

        assert!(source.begin_tab_drag(source.panes.tab_id(0, 0)));
        let Some(dragged) = source.dragged_item() else {
            panic!("a tab should be dragging");
        };
        assert!(target.accepts_item(dragged));
        assert!(target.update_foreign_drop(790.0, 250.0, None, dragged));
        assert!(target.tab_drag_overlay().is_some());
        let Some(item) = source.take_dragged_item() else {
            panic!("dragged item should detach");
        };
        assert!(source.is_empty());
        target.accept_foreign_item(item);
        assert_eq!(target.panes.group.leaf_count(), 2);
        assert_eq!(target.panes.active, vec![1]);
        assert!(target.tab_drag_overlay().is_none());

        target.clear_foreign_drop();
        target.render(region, true);
        assert!(target.begin_tab_drag(target.panes.tab_id(1, 0)));
        let Some(item) = target.take_dragged_item() else {
            panic!("dragged item should detach");
        };
        assert_eq!(target.panes.group.leaf_count(), 1);
        source.accept_foreign_item(item);
        assert!(!source.is_empty());
    }

    struct Note;

    impl Item for Note {
        fn id(&self) -> Option<String> {
            Some("note.md".into())
        }
        fn title(&self) -> String {
            "note.md".into()
        }
        fn render(&mut self) -> Node {
            ui::div().into()
        }
        fn is_editable(&self) -> bool {
            true
        }
    }

    #[test]
    fn any_item_drops_into_the_panel_but_only_terminals_split_it() {
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        let mut panel = panel();
        panel.open(None);
        panel.render(region, true);
        assert!(panel.accepts_item(&Note));
        assert!(panel.update_foreign_drop(790.0, 250.0, None, &Note));
        panel.accept_foreign_item(Box::new(Note));
        assert_eq!(panel.panes.group.leaf_count(), 1);
        assert_eq!(
            panel
                .panes
                .active_item()
                .and_then(|item| item.id())
                .as_deref(),
            Some("note.md")
        );
        assert!(!panel.panes.active_wants_keystrokes());
        assert!(panel.panes.editor_focused());

        panel.render(region, true);
        let Some(shell) = panel.new_item(None) else {
            panic!("shell should start");
        };
        assert!(panel.update_foreign_drop(790.0, 250.0, None, shell.as_ref()));
        panel.accept_foreign_item(shell);
        assert_eq!(panel.panes.group.leaf_count(), 2);
    }

    #[test]
    fn the_only_tab_of_the_only_pane_cannot_split_itself() {
        let region = Rect::new(0.0, 0.0, 800.0, 400.0, Rgba::TRANSPARENT);
        let mut panel = panel();
        panel.open(None);
        panel.render(region, true);
        assert!(panel.begin_tab_drag(panel.panes.tab_id(0, 0)));
        assert!(panel.update_tab_drag(790.0, 250.0, None));
        assert!(panel.drop_tab());
        assert_eq!(panel.panes.group.leaf_count(), 1);
        assert!(!panel.is_empty());
    }

    #[test]
    fn saved_terminals_come_back_as_shells_in_their_directories() {
        let mut panel = panel();
        panel.open(Some(std::env::temp_dir()));
        panel.open(None);
        let saved = panel.save_panes();
        let mut back = super::TerminalPanel::new(std::env::temp_dir(), std::sync::Arc::new(|| {}));
        assert!(back.restore_panes(&saved));
        let count = back.panes.pane_at(&[]).map(|pane| pane.open.len());
        assert_eq!(count, Some(2));
        assert!(!back.is_empty());
    }

    #[test]
    fn agents_open_in_their_dock_and_leave_the_terminals_alone() {
        let mut app = ui::Application::new();
        let (handle, view) = app.open_raw_window(
            ui::WindowOptions {
                width: 1200.0,
                height: 800.0,
                scale: 2.0,
                ..Default::default()
            },
            |_| {
                let mut view = workspace::WorkspaceView::new(workspace::Layout::default());
                view.set_project(
                    Some(workspace::ProjectInfo {
                        name: "demo".into(),
                        ..Default::default()
                    }),
                    None,
                    Some(Box::new(panel())),
                    Some(Box::new(TerminalPanel::agent_dock(
                        std::env::temp_dir(),
                        std::sync::Arc::new(|| {}),
                    ))),
                );
                view
            },
        );
        view.update(app.app_mut(), |view, _| {
            view.open_agent_item("agent:demo", || {
                let terminal = Terminal::spawn(
                    TerminalOptions {
                        shell: Some(("/bin/sh".into(), vec!["-c".into(), "sleep 5".into()])),
                        ..TerminalOptions::default()
                    },
                    std::sync::Arc::new(|| {}),
                )
                .ok()?;
                Some(Box::new(TerminalItem::with_terminal(
                    7,
                    std::env::temp_dir(),
                    terminal,
                )) as Box<dyn Item>)
            })
        });
        let frame = app.draw(handle).expect("frame");
        assert!(
            view.read(app.app()).terminal_focused(),
            "keys go raw to the agent session"
        );
        let agent_hits: Vec<ui::Rect> = std::iter::once(&frame.base)
            .chain(frame.overlays.iter().map(|overlay| &overlay.painted))
            .flat_map(|painted| painted.hits.iter())
            .filter(|(_, id)| workspace::is_agent_id(*id))
            .map(|(rect, _)| *rect)
            .collect();
        assert!(!agent_hits.is_empty(), "agent tab laid out");
        assert!(
            agent_hits.iter().all(|rect| rect.x >= 600.0),
            "the agent sits in the right dock, not under the terminals"
        );

        let again = view.update(app.app_mut(), |view, _| {
            let mut built = false;
            view.open_agent_item("terminal:7", || {
                built = true;
                None
            });
            built
        });
        assert!(!again, "an open session is focused, not opened twice");
    }
}
