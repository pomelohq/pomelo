use std::path::PathBuf;

/// The window's own commands: what a key binding or the command palette can run outside the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    CommandPalette,
    FileFinder,
    ProjectSearch,
    OpenSettings,
    OpenKeymap,
    OpenProject,
    NewWorkspace,
    NewProject,
    SwitchWorkspace,
    CycleTheme,
    ToggleLeftDock,
    ToggleRightDock,
    ToggleBottomDock,
    FocusFiles,
    FocusGit,
    FocusServices,
    FocusDatabase,
    FocusPullRequests,
    ToggleAgent,
    ToggleTerminal,
    NewTerminal,
    CloseActiveItem,
    CloseAllItems,
    OpenInExternalEditor,
    ExportConfig,
    ImportConfig,
    MarkdownPreview,
    MarkdownPreviewToTheSide,
    OpenTicket,
    OpenProjectConfig,
    SetUpProjectWithAi,
    AddRepository,
    CloneMissingRepos,
    OpenAgentUsage,
    OpenDevRequests,
    OpenModuleStore,
    /// The tab at this 0-based position in the focused pane.
    ActivateTab(u8),
    ActivateLastTab,
    ActivatePreviousTab,
    ActivateNextTab,
}

impl Action {
    pub const ALL: [Action; 48] = [
        Action::CommandPalette,
        Action::FileFinder,
        Action::ProjectSearch,
        Action::OpenSettings,
        Action::OpenKeymap,
        Action::OpenProject,
        Action::NewWorkspace,
        Action::NewProject,
        Action::SwitchWorkspace,
        Action::CycleTheme,
        Action::ToggleLeftDock,
        Action::ToggleRightDock,
        Action::ToggleBottomDock,
        Action::FocusFiles,
        Action::FocusGit,
        Action::FocusServices,
        Action::FocusDatabase,
        Action::FocusPullRequests,
        Action::ToggleAgent,
        Action::ToggleTerminal,
        Action::NewTerminal,
        Action::CloseActiveItem,
        Action::CloseAllItems,
        Action::OpenInExternalEditor,
        Action::ExportConfig,
        Action::ImportConfig,
        Action::MarkdownPreview,
        Action::MarkdownPreviewToTheSide,
        Action::OpenTicket,
        Action::OpenProjectConfig,
        Action::SetUpProjectWithAi,
        Action::AddRepository,
        Action::CloneMissingRepos,
        Action::OpenAgentUsage,
        Action::OpenDevRequests,
        Action::OpenModuleStore,
        Action::ActivateTab(0),
        Action::ActivateTab(1),
        Action::ActivateTab(2),
        Action::ActivateTab(3),
        Action::ActivateTab(4),
        Action::ActivateTab(5),
        Action::ActivateTab(6),
        Action::ActivateTab(7),
        Action::ActivateTab(8),
        Action::ActivateLastTab,
        Action::ActivatePreviousTab,
        Action::ActivateNextTab,
    ];

    /// The name a keymap file binds, `namespace::Action`.
    pub fn name(self) -> &'static str {
        match self {
            Action::CommandPalette => "command_palette::Toggle",
            Action::FileFinder => "file_finder::Toggle",
            Action::ProjectSearch => "project_search::Deploy",
            Action::OpenSettings => "pomelo::OpenSettings",
            Action::OpenKeymap => "pomelo::OpenKeymap",
            Action::OpenProject => "workspace::Open",
            Action::NewWorkspace => "workspace::NewWorkspace",
            Action::NewProject => "workspace::NewProject",
            Action::SwitchWorkspace => "workspace::SwitchWorkspace",
            Action::CycleTheme => "theme::Cycle",
            Action::ToggleLeftDock => "workspace::ToggleLeftDock",
            Action::ToggleRightDock => "workspace::ToggleRightDock",
            Action::ToggleBottomDock => "workspace::ToggleBottomDock",
            Action::FocusFiles => "project_panel::ToggleFocus",
            Action::FocusGit => "git_panel::ToggleFocus",
            Action::FocusServices => "services_panel::ToggleFocus",
            Action::FocusDatabase => "database_panel::ToggleFocus",
            Action::FocusPullRequests => "git_panel::PullRequests",
            Action::ToggleAgent => "agent::ToggleFocus",
            Action::ToggleTerminal => "terminal_panel::ToggleFocus",
            Action::NewTerminal => "workspace::NewTerminal",
            Action::CloseActiveItem => "pane::CloseActiveItem",
            Action::CloseAllItems => "pane::CloseAllItems",
            Action::OpenInExternalEditor => "workspace::OpenInExternalEditor",
            Action::ExportConfig => "workspace::ExportConfig",
            Action::ImportConfig => "workspace::ImportConfig",
            Action::MarkdownPreview => "markdown::OpenPreview",
            Action::MarkdownPreviewToTheSide => "markdown::OpenPreviewToTheSide",
            Action::OpenTicket => "workspace::OpenTicket",
            Action::OpenProjectConfig => "pomelo::OpenProjectConfig",
            Action::SetUpProjectWithAi => "pomelo::SetUpProjectWithAi",
            Action::AddRepository => "pomelo::AddRepository",
            Action::CloneMissingRepos => "pomelo::CloneMissingRepos",
            Action::OpenAgentUsage => "pomelo::OpenAgentUsage",
            Action::OpenDevRequests => "pomelo::OpenDevRequests",
            Action::OpenModuleStore => "pomelo::OpenModuleStore",
            Action::ActivateTab(_) => "pane::ActivateItem",
            Action::ActivateLastTab => "pane::ActivateLastItem",
            Action::ActivatePreviousTab => "pane::ActivatePreviousItem",
            Action::ActivateNextTab => "pane::ActivateNextItem",
        }
    }

    /// What the palette and the keymap page call it.
    pub fn label(self) -> &'static str {
        match self {
            Action::CommandPalette => "Command Palette",
            Action::FileFinder => "Go to File",
            Action::ProjectSearch => "Find in Project",
            Action::OpenSettings => "Open Settings",
            Action::OpenKeymap => "Open Keymap",
            Action::OpenProject => "Open Project",
            Action::NewWorkspace => "New Workspace",
            Action::NewProject => "New Project",
            Action::SwitchWorkspace => "Switch Workspace",
            Action::CycleTheme => "Next Theme",
            Action::ToggleLeftDock => "Toggle Left Dock",
            Action::ToggleRightDock => "Toggle Right Dock",
            Action::ToggleBottomDock => "Toggle Bottom Dock",
            Action::FocusFiles => "Files",
            Action::FocusGit => "Git",
            Action::FocusServices => "Services",
            Action::FocusDatabase => "Database",
            Action::FocusPullRequests => "Pull Requests",
            Action::ToggleAgent => "Agent",
            Action::ToggleTerminal => "Terminal",
            Action::NewTerminal => "New Terminal",
            Action::CloseActiveItem => "Close Tab",
            Action::CloseAllItems => "Close All Tabs",
            Action::OpenInExternalEditor => "Open in External Editor",
            Action::ExportConfig => "Export Config",
            Action::ImportConfig => "Import Config",
            Action::MarkdownPreview => "Markdown Preview",
            Action::MarkdownPreviewToTheSide => "Markdown Preview to the Side",
            Action::OpenTicket => "Open Jira Ticket",
            Action::OpenProjectConfig => "Open Project Config",
            Action::SetUpProjectWithAi => "Set Up Project with AI",
            Action::AddRepository => "Add Repository",
            Action::CloneMissingRepos => "Clone Missing Repos into Main",
            Action::OpenAgentUsage => "Agent Usage",
            Action::OpenDevRequests => "Dev Requests",
            Action::OpenModuleStore => "node_modules Store",
            Action::ActivateTab(index) => match index {
                0 => "Go to Tab 1",
                1 => "Go to Tab 2",
                2 => "Go to Tab 3",
                3 => "Go to Tab 4",
                4 => "Go to Tab 5",
                5 => "Go to Tab 6",
                6 => "Go to Tab 7",
                7 => "Go to Tab 8",
                _ => "Go to Tab 9",
            },
            Action::ActivateLastTab => "Go to Last Tab",
            Action::ActivatePreviousTab => "Previous Tab",
            Action::ActivateNextTab => "Next Tab",
        }
    }

    /// Actions bound by name alone; `pane::ActivateItem` also needs its index (`["pane::ActivateItem", 0]`).
    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL
            .into_iter()
            .filter(|action| !matches!(action, Action::ActivateTab(_)))
            .find(|action| action.name() == name)
    }

    fn from_target(target: &serde_json::Value) -> Result<Option<Action>, String> {
        match target {
            serde_json::Value::Null => Ok(None),
            serde_json::Value::String(name) => Action::from_name(name)
                .map(Some)
                .ok_or_else(|| format!("unknown action \"{name}\"")),
            serde_json::Value::Array(parts) => match (parts.first(), parts.get(1)) {
                (Some(serde_json::Value::String(name)), Some(index))
                    if name == "pane::ActivateItem" =>
                {
                    index
                        .as_u64()
                        .filter(|index| *index < 9)
                        .map(|index| Some(Action::ActivateTab(index as u8)))
                        .ok_or_else(|| format!("\"{name}\" takes a tab index from 0 to 8"))
                }
                (Some(serde_json::Value::String(name)), _) => {
                    Err(format!("unknown action \"{name}\""))
                }
                _ => Err("needs an action name".into()),
            },
            _ => Err("needs an action name".into()),
        }
    }
}

/// One key press, as keymap files write it (`cmd-shift-p`, `ctrl-``, `enter`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Keystroke {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Lowercase key: a character, or a named key (`enter`, `escape`, `left`, `f1`...).
    pub key: String,
}

impl Keystroke {
    pub fn parse(text: &str) -> Option<Keystroke> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut stroke = Keystroke::default();
        // The key may itself be `-`, so peel modifiers off the front only.
        let mut rest = text;
        while let Some((modifier, after)) = rest.split_once('-') {
            if after.is_empty() {
                break;
            }
            match modifier {
                "cmd" | "super" | "command" => stroke.cmd = true,
                "secondary" if cfg!(target_os = "macos") => stroke.cmd = true,
                "secondary" => stroke.ctrl = true,
                "ctrl" | "control" => stroke.ctrl = true,
                "alt" | "option" => stroke.alt = true,
                "shift" => stroke.shift = true,
                _ => break,
            }
            rest = after;
        }
        stroke.key = rest.to_lowercase();
        Some(stroke)
    }

    /// Shift-typed symbols (`?`) name themselves; the shift is implied, as keymap files write them.
    fn normalized(&self) -> Keystroke {
        let mut stroke = self.clone();
        // With Command held, macOS reports the unshifted key (`/` for cmd-?), so a shifted US symbol is folded
        // into the character it types, the form bindings are written in.
        const SHIFTED: [(&str, &str); 21] = [
            ("1", "!"),
            ("2", "@"),
            ("3", "#"),
            ("4", "$"),
            ("5", "%"),
            ("6", "^"),
            ("7", "&"),
            ("8", "*"),
            ("9", "("),
            ("0", ")"),
            ("-", "_"),
            ("=", "+"),
            ("[", "{"),
            ("]", "}"),
            ("\\", "|"),
            (";", ":"),
            ("'", "\""),
            (",", "<"),
            (".", ">"),
            ("/", "?"),
            ("`", "~"),
        ];
        if stroke.shift {
            if let Some((_, typed)) = SHIFTED.iter().find(|(base, _)| *base == stroke.key) {
                stroke.key = (*typed).to_string();
            }
        }
        let symbol = stroke.key.chars().count() == 1
            && stroke
                .key
                .chars()
                .next()
                .is_some_and(|c| !c.is_alphanumeric());
        if symbol && "~!@#$%^&*()_+{}|:\"<>?".contains(stroke.key.as_str()) {
            stroke.shift = false;
        }
        stroke
    }

    /// For showing a binding, in the order keymap files are written: `ctrl-alt-cmd-shift-p`.
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.cmd {
            parts.push("cmd");
        }
        if self.shift {
            parts.push("shift");
        }
        parts.push(&self.key);
        parts.join("-")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyMatch {
    Action(Action),
    /// The press starts a longer binding; keep it and wait for the next one.
    Pending,
    None,
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    /// Later entries win, so the user's file (read after the defaults) overrides them; `None` unbinds.
    bindings: Vec<(Vec<Keystroke>, Option<Action>)>,
}

/// macOS binds window commands on Command; `ctrl-`` stays because `cmd-`` cycles the app's windows.
const MACOS_DEFAULTS: &[(&str, Action)] = &[
    ("cmd-shift-p", Action::CommandPalette),
    ("cmd-p", Action::FileFinder),
    ("cmd-shift-f", Action::ProjectSearch),
    ("cmd-,", Action::OpenSettings),
    ("cmd-k cmd-s", Action::OpenKeymap),
    ("cmd-o", Action::OpenProject),
    ("cmd-n", Action::NewWorkspace),
    ("cmd-shift-n", Action::NewProject),
    ("cmd-alt-o", Action::SwitchWorkspace),
    ("cmd-k cmd-t", Action::CycleTheme),
    ("cmd-b", Action::ToggleLeftDock),
    ("cmd-r", Action::ToggleRightDock),
    ("cmd-j", Action::ToggleBottomDock),
    ("cmd-shift-e", Action::FocusFiles),
    ("cmd-shift-c", Action::FocusGit),
    ("cmd-shift-s", Action::FocusServices),
    ("cmd-shift-d", Action::FocusDatabase),
    ("cmd-shift-r", Action::FocusPullRequests),
    ("cmd-?", Action::ToggleAgent),
    ("ctrl-`", Action::ToggleTerminal),
    ("cmd-t", Action::NewTerminal),
    ("cmd-w", Action::CloseActiveItem),
    ("cmd-alt-w", Action::CloseAllItems),
    ("cmd-shift-v", Action::MarkdownPreview),
    ("cmd-k v", Action::MarkdownPreviewToTheSide),
    ("cmd-shift-u", Action::OpenAgentUsage),
    ("cmd-1", Action::ActivateTab(0)),
    ("cmd-2", Action::ActivateTab(1)),
    ("cmd-3", Action::ActivateTab(2)),
    ("cmd-4", Action::ActivateTab(3)),
    ("cmd-5", Action::ActivateTab(4)),
    ("cmd-6", Action::ActivateTab(5)),
    ("cmd-7", Action::ActivateTab(6)),
    ("cmd-8", Action::ActivateTab(7)),
    ("cmd-9", Action::ActivateTab(8)),
    ("cmd-0", Action::ActivateLastTab),
    ("cmd-alt-left", Action::ActivatePreviousTab),
    ("cmd-alt-right", Action::ActivateNextTab),
    ("cmd-shift-[", Action::ActivatePreviousTab),
    ("cmd-shift-]", Action::ActivateNextTab),
];

/// Linux and Windows: the same commands on Control, with those platforms' tab keys.
const OTHER_DEFAULTS: &[(&str, Action)] = &[
    ("ctrl-shift-p", Action::CommandPalette),
    ("ctrl-p", Action::FileFinder),
    ("ctrl-shift-f", Action::ProjectSearch),
    ("ctrl-,", Action::OpenSettings),
    ("ctrl-k ctrl-s", Action::OpenKeymap),
    ("ctrl-o", Action::OpenProject),
    ("ctrl-n", Action::NewWorkspace),
    ("ctrl-shift-n", Action::NewProject),
    ("ctrl-alt-o", Action::SwitchWorkspace),
    ("ctrl-k ctrl-t", Action::CycleTheme),
    ("ctrl-b", Action::ToggleLeftDock),
    ("ctrl-r", Action::ToggleRightDock),
    ("ctrl-j", Action::ToggleBottomDock),
    ("ctrl-shift-e", Action::FocusFiles),
    ("ctrl-shift-c", Action::FocusGit),
    ("ctrl-shift-s", Action::FocusServices),
    ("ctrl-shift-d", Action::FocusDatabase),
    ("ctrl-shift-r", Action::FocusPullRequests),
    ("ctrl-?", Action::ToggleAgent),
    ("ctrl-`", Action::ToggleTerminal),
    ("ctrl-t", Action::NewTerminal),
    ("ctrl-w", Action::CloseActiveItem),
    ("ctrl-alt-w", Action::CloseAllItems),
    ("ctrl-shift-v", Action::MarkdownPreview),
    ("ctrl-k v", Action::MarkdownPreviewToTheSide),
    ("ctrl-shift-u", Action::OpenAgentUsage),
    ("ctrl-1", Action::ActivateTab(0)),
    ("ctrl-2", Action::ActivateTab(1)),
    ("ctrl-3", Action::ActivateTab(2)),
    ("ctrl-4", Action::ActivateTab(3)),
    ("ctrl-5", Action::ActivateTab(4)),
    ("ctrl-6", Action::ActivateTab(5)),
    ("ctrl-7", Action::ActivateTab(6)),
    ("ctrl-8", Action::ActivateTab(7)),
    ("ctrl-9", Action::ActivateTab(8)),
    ("ctrl-0", Action::ActivateLastTab),
    ("ctrl-pageup", Action::ActivatePreviousTab),
    ("ctrl-pagedown", Action::ActivateNextTab),
    ("ctrl-shift-[", Action::ActivatePreviousTab),
    ("ctrl-shift-]", Action::ActivateNextTab),
];

fn platform_defaults() -> &'static [(&'static str, Action)] {
    if cfg!(target_os = "macos") {
        MACOS_DEFAULTS
    } else {
        OTHER_DEFAULTS
    }
}

fn sequence(text: &str) -> Option<Vec<Keystroke>> {
    let strokes: Option<Vec<Keystroke>> = text
        .split_whitespace()
        .map(|part| Keystroke::parse(part).map(|stroke| stroke.normalized()))
        .collect();
    strokes.filter(|strokes| !strokes.is_empty())
}

impl Keymap {
    pub fn defaults() -> Keymap {
        Keymap {
            bindings: platform_defaults()
                .iter()
                .filter_map(|(keys, action)| Some((sequence(keys)?, Some(*action))))
                .collect(),
        }
    }

    pub fn user_file() -> Option<PathBuf> {
        Some(pom_paths::config_dir()?.join("keymap.json"))
    }

    /// The defaults with the user's keymap file applied; problems in it are returned, not fatal.
    pub fn load() -> (Keymap, Vec<String>) {
        let mut keymap = Keymap::defaults();
        let text = Keymap::user_file().and_then(|path| std::fs::read_to_string(path).ok());
        let problems = match text {
            Some(text) => keymap.apply_user(&text),
            None => Vec::new(),
        };
        (keymap, problems)
    }

    /// A keymap file in the reference's shape: `[{"context": "Workspace", "bindings": {"cmd-k": "name"}}]`,
    /// where `null` unbinds a key.
    pub fn apply_user(&mut self, text: &str) -> Vec<String> {
        let mut problems = Vec::new();
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => return vec![format!("keymap.json: {error}")],
        };
        let Some(sections) = value.as_array() else {
            return vec!["keymap.json: expected a list of sections".into()];
        };
        for section in sections {
            let context = section
                .get("context")
                .and_then(|context| context.as_str())
                .unwrap_or("Workspace");
            if context != "Workspace" {
                continue;
            }
            let Some(bindings) = section.get("bindings").and_then(|b| b.as_object()) else {
                continue;
            };
            for (keys, target) in bindings {
                let Some(strokes) = sequence(keys) else {
                    problems.push(format!("keymap.json: cannot read the keys \"{keys}\""));
                    continue;
                };
                let action = match Action::from_target(target) {
                    Ok(action) => action,
                    Err(problem) => {
                        problems.push(format!("keymap.json: \"{keys}\": {problem}"));
                        continue;
                    }
                };
                self.bindings.push((strokes, action));
            }
        }
        problems
    }

    /// What `pending` then `stroke` does.
    pub fn match_keys(&self, pending: &[Keystroke], stroke: &Keystroke) -> KeyMatch {
        let mut typed: Vec<Keystroke> = pending.to_vec();
        typed.push(stroke.normalized());
        if let Some((_, action)) = self.bindings.iter().rev().find(|(keys, _)| *keys == typed) {
            return action.map_or(KeyMatch::None, KeyMatch::Action);
        }
        let longer = self.bindings.iter().rev().any(|(keys, action)| {
            action.is_some() && keys.len() > typed.len() && keys[..typed.len()] == typed[..]
        });
        if longer {
            KeyMatch::Pending
        } else {
            KeyMatch::None
        }
    }

    /// The binding that currently runs `action`, as text (`cmd-k cmd-s`).
    pub fn binding_for(&self, action: Action) -> Option<String> {
        let mut rebound: Vec<&Vec<Keystroke>> = Vec::new();
        for (keys, bound) in self.bindings.iter().rev() {
            if rebound.contains(&keys) {
                continue;
            }
            if *bound == Some(action) {
                return Some(
                    keys.iter()
                        .map(Keystroke::text)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            rebound.push(keys);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(text: &str) -> Keystroke {
        Keystroke::parse(text).unwrap_or_default()
    }

    #[test]
    fn keystrokes_parse_like_keymap_files() {
        assert_eq!(
            stroke("cmd-shift-p"),
            Keystroke {
                cmd: true,
                shift: true,
                key: "p".into(),
                ..Default::default()
            }
        );
        assert_eq!(stroke("ctrl--").key, "-");
        assert_eq!(stroke("cmd-shift-p").text(), "cmd-shift-p");
        assert_eq!(stroke("enter").key, "enter");
    }

    #[test]
    fn cmd_question_mark_matches_however_macos_reports_it() {
        let keymap = Keymap::defaults();
        let agent = KeyMatch::Action(Action::ToggleAgent);
        assert_eq!(keymap.match_keys(&[], &stroke("cmd-shift-/")), agent);
        assert_eq!(keymap.match_keys(&[], &stroke("cmd-shift-?")), agent);
        assert_eq!(keymap.match_keys(&[], &stroke("cmd-?")), agent);
    }

    #[test]
    fn defaults_resolve_and_sequences_wait_for_their_second_key() {
        let keymap = Keymap::defaults();
        assert_eq!(
            keymap.match_keys(&[], &stroke("cmd-shift-p")),
            KeyMatch::Action(Action::CommandPalette)
        );
        assert_eq!(keymap.match_keys(&[], &stroke("cmd-k")), KeyMatch::Pending);
        assert_eq!(
            keymap.match_keys(&[stroke("cmd-k")], &stroke("cmd-s")),
            KeyMatch::Action(Action::OpenKeymap)
        );
        assert_eq!(keymap.match_keys(&[], &stroke("cmd-x")), KeyMatch::None);
        assert_eq!(
            keymap.match_keys(&[], &stroke("cmd-shift-?")),
            KeyMatch::Action(Action::ToggleAgent),
            "a shifted symbol matches its plain spelling"
        );
        assert_eq!(
            keymap.binding_for(Action::OpenKeymap).as_deref(),
            Some("cmd-k cmd-s")
        );
    }

    #[test]
    fn the_user_file_rebinds_unbinds_and_reports_mistakes() {
        let mut keymap = Keymap::defaults();
        let problems = keymap.apply_user(
            r#"[
                {"bindings": {"cmd-shift-g": "git_panel::ToggleFocus", "cmd-b": null,
                              "cmd-y": "nope::Nothing", "cmd-alt-3": ["pane::ActivateItem", 2],
                              "secondary-shift-x": "workspace::NewTerminal"}},
                {"context": "Editor", "bindings": {"cmd-j": "workspace::NewTerminal"}}
            ]"#,
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(
            keymap.match_keys(&[], &stroke("cmd-shift-g")),
            KeyMatch::Action(Action::FocusGit)
        );
        assert_eq!(keymap.match_keys(&[], &stroke("cmd-b")), KeyMatch::None);
        assert_eq!(
            keymap.match_keys(&[], &stroke("cmd-alt-3")),
            KeyMatch::Action(Action::ActivateTab(2))
        );
        let secondary = if cfg!(target_os = "macos") {
            "cmd-shift-x"
        } else {
            "ctrl-shift-x"
        };
        assert_eq!(
            keymap.match_keys(&[], &stroke(secondary)),
            KeyMatch::Action(Action::NewTerminal)
        );
        assert_eq!(keymap.binding_for(Action::ToggleLeftDock), None);
        assert_eq!(
            keymap.match_keys(&[], &stroke("cmd-j")),
            KeyMatch::Action(Action::ToggleBottomDock),
            "other contexts are not the window's"
        );
        assert!(!Keymap::defaults().apply_user("{").is_empty());
    }

    #[test]
    fn every_platform_binds_every_default_action_once() {
        for table in [MACOS_DEFAULTS, OTHER_DEFAULTS] {
            let mut keys: Vec<&str> = table.iter().map(|(keys, _)| *keys).collect();
            let before = keys.len();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), before, "a key is bound twice");
            assert!(table.iter().all(|(keys, _)| sequence(keys).is_some()));
        }
        let actions = |table: &[(&str, Action)]| {
            let mut actions: Vec<&str> = table.iter().map(|(_, action)| action.label()).collect();
            actions.sort();
            actions.dedup();
            actions
        };
        assert_eq!(actions(MACOS_DEFAULTS), actions(OTHER_DEFAULTS));
    }

    #[test]
    fn mac_defaults_put_window_commands_on_command() {
        let keymap = Keymap::defaults();
        if !cfg!(target_os = "macos") {
            return;
        }
        for (keys, action) in [
            ("cmd-shift-c", Action::FocusGit),
            ("cmd-shift-s", Action::FocusServices),
            ("cmd-shift-d", Action::FocusDatabase),
            ("cmd-shift-r", Action::FocusPullRequests),
            ("cmd-alt-o", Action::SwitchWorkspace),
            ("cmd-1", Action::ActivateTab(0)),
            ("cmd-9", Action::ActivateTab(8)),
            ("cmd-0", Action::ActivateLastTab),
        ] {
            assert_eq!(
                keymap.match_keys(&[], &stroke(keys)),
                KeyMatch::Action(action),
                "{keys}"
            );
        }
    }
}
