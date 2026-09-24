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
}

impl Action {
    pub const ALL: [Action; 24] = [
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
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.name() == name)
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

const DEFAULTS: &[(&str, Action)] = &[
    ("cmd-shift-p", Action::CommandPalette),
    ("cmd-p", Action::FileFinder),
    ("cmd-shift-f", Action::ProjectSearch),
    ("cmd-,", Action::OpenSettings),
    ("cmd-k cmd-s", Action::OpenKeymap),
    ("cmd-o", Action::OpenProject),
    ("cmd-n", Action::NewWorkspace),
    ("cmd-shift-n", Action::NewProject),
    ("ctrl-shift-w", Action::SwitchWorkspace),
    ("cmd-k cmd-t", Action::CycleTheme),
    ("cmd-b", Action::ToggleLeftDock),
    ("cmd-r", Action::ToggleRightDock),
    ("cmd-j", Action::ToggleBottomDock),
    ("cmd-shift-e", Action::FocusFiles),
    ("ctrl-shift-g", Action::FocusGit),
    ("ctrl-shift-s", Action::FocusServices),
    ("ctrl-shift-d", Action::FocusDatabase),
    ("ctrl-shift-p", Action::FocusPullRequests),
    ("cmd-?", Action::ToggleAgent),
    ("ctrl-`", Action::ToggleTerminal),
    ("cmd-t", Action::NewTerminal),
    ("cmd-w", Action::CloseActiveItem),
    ("cmd-alt-w", Action::CloseAllItems),
];

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
            bindings: DEFAULTS
                .iter()
                .filter_map(|(keys, action)| Some((sequence(keys)?, Some(*action))))
                .collect(),
        }
    }

    pub fn user_file() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".config/pomelo/keymap.json"))
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
                let action = match target {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(name) => match Action::from_name(name) {
                        Some(action) => Some(action),
                        None => {
                            problems.push(format!("keymap.json: unknown action \"{name}\""));
                            continue;
                        }
                    },
                    _ => {
                        problems.push(format!("keymap.json: \"{keys}\" needs an action name"));
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
                              "cmd-y": "nope::Nothing"}},
                {"context": "Editor", "bindings": {"cmd-j": "workspace::NewTerminal"}}
            ]"#,
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(
            keymap.match_keys(&[], &stroke("cmd-shift-g")),
            KeyMatch::Action(Action::FocusGit)
        );
        assert_eq!(keymap.match_keys(&[], &stroke("cmd-b")), KeyMatch::None);
        assert_eq!(keymap.binding_for(Action::ToggleLeftDock), None);
        assert_eq!(
            keymap.match_keys(&[], &stroke("cmd-j")),
            KeyMatch::Action(Action::ToggleBottomDock),
            "other contexts are not the window's"
        );
        assert!(!Keymap::defaults().apply_user("{").is_empty());
    }
}
