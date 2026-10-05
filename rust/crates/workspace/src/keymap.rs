use std::path::PathBuf;

/// The window's own commands: what a key binding or the command palette can run outside the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    CommandPalette,
    FileFinder,
    ProjectSearch,
    ProjectDiagnostics,
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
    SelectLanguage,
    /// The tab at this 0-based position in the focused pane.
    ActivateTab(u8),
    ActivateLastTab,
    ActivatePreviousTab,
    ActivateNextTab,
    TreeOpen,
    TreeRename,
    TreeNewFile,
    TreeNewDirectory,
    TreeCut,
    TreeCopy,
    TreePaste,
    TreeDuplicate,
    TreeCopyPath,
    TreeCopyRelativePath,
    TreeTrash,
    TreeDelete,
    TreeRevealInFinder,
    TreeCollapse,
    TreeExpand,
    TreeCollapseAll,
    TreeExpandAll,
    ToggleTabSwitcher,
    ToggleTabSwitcherLast,
    TabSwitcherCloseSelected,
    PreviousWorkspace,
    NextWorkspace,
    NextWorkspaceNeedingAttention,
    GitOpenEntry,
    GitToggleStaged,
    GitStageFile,
    GitUnstageFile,
    GitStageAll,
    GitUnstageAll,
    GitRestoreFile,
    GitCopyPath,
    GitCopyRelativePath,
    GitFocusCommitEditor,
    GitCollapse,
    GitExpand,
    GitFetch,
    GitPush,
    GitPull,
    GitChangesTab,
    GitRemoteTab,
    GitHistoryTab,
    ServicesOpen,
    ServicesToggleRunning,
    ServicesRestart,
    ServicesOpenInBrowser,
    ServicesLogs,
    ServicesCollapse,
    ServicesExpand,
    DatabaseOpen,
    DatabaseCollapse,
    DatabaseExpand,
    DatabaseCopyUrl,
    DatabaseNewConsole,
    NewSideAgent,
    AgentTakeOver,
    AgentAllow,
    AgentDeny,
    StopAgent,
    RunNotificationAction,
    DismissNotification,
    CloseOtherItems,
    CloseItemsToTheLeft,
    CloseItemsToTheRight,
    CloseCleanItems,
    FocusEditor,
    NextRegion,
    PreviousRegion,
    ToggleVimMode,
}

impl Action {
    pub const ALL: [Action; 118] = [
        Action::CommandPalette,
        Action::FileFinder,
        Action::ProjectSearch,
        Action::ProjectDiagnostics,
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
        Action::SelectLanguage,
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
        Action::TreeOpen,
        Action::TreeRename,
        Action::TreeNewFile,
        Action::TreeNewDirectory,
        Action::TreeCut,
        Action::TreeCopy,
        Action::TreePaste,
        Action::TreeDuplicate,
        Action::TreeCopyPath,
        Action::TreeCopyRelativePath,
        Action::TreeTrash,
        Action::TreeDelete,
        Action::TreeRevealInFinder,
        Action::TreeCollapse,
        Action::TreeExpand,
        Action::TreeCollapseAll,
        Action::TreeExpandAll,
        Action::ToggleTabSwitcher,
        Action::ToggleTabSwitcherLast,
        Action::TabSwitcherCloseSelected,
        Action::PreviousWorkspace,
        Action::NextWorkspace,
        Action::NextWorkspaceNeedingAttention,
        Action::GitOpenEntry,
        Action::GitToggleStaged,
        Action::GitStageFile,
        Action::GitUnstageFile,
        Action::GitStageAll,
        Action::GitUnstageAll,
        Action::GitRestoreFile,
        Action::GitCopyPath,
        Action::GitCopyRelativePath,
        Action::GitFocusCommitEditor,
        Action::GitCollapse,
        Action::GitExpand,
        Action::GitFetch,
        Action::GitPush,
        Action::GitPull,
        Action::GitChangesTab,
        Action::GitRemoteTab,
        Action::GitHistoryTab,
        Action::ServicesOpen,
        Action::ServicesToggleRunning,
        Action::ServicesRestart,
        Action::ServicesOpenInBrowser,
        Action::ServicesLogs,
        Action::ServicesCollapse,
        Action::ServicesExpand,
        Action::DatabaseOpen,
        Action::DatabaseCollapse,
        Action::DatabaseExpand,
        Action::DatabaseCopyUrl,
        Action::DatabaseNewConsole,
        Action::NewSideAgent,
        Action::AgentTakeOver,
        Action::AgentAllow,
        Action::AgentDeny,
        Action::StopAgent,
        Action::RunNotificationAction,
        Action::DismissNotification,
        Action::CloseOtherItems,
        Action::CloseItemsToTheLeft,
        Action::CloseItemsToTheRight,
        Action::CloseCleanItems,
        Action::FocusEditor,
        Action::NextRegion,
        Action::PreviousRegion,
        Action::ToggleVimMode,
    ];

    /// The name a keymap file binds, `namespace::Action`.
    pub fn name(self) -> &'static str {
        match self {
            Action::CommandPalette => "command_palette::Toggle",
            Action::FileFinder => "file_finder::Toggle",
            Action::ProjectSearch => "project_search::Deploy",
            Action::ProjectDiagnostics => "diagnostics::Deploy",
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
            Action::SelectLanguage => "language_selector::Toggle",
            Action::ActivateTab(_) => "pane::ActivateItem",
            Action::ActivateLastTab => "pane::ActivateLastItem",
            Action::ActivatePreviousTab => "pane::ActivatePreviousItem",
            Action::ActivateNextTab => "pane::ActivateNextItem",
            Action::TreeOpen => "project_panel::Open",
            Action::TreeRename => "project_panel::Rename",
            Action::TreeNewFile => "project_panel::NewFile",
            Action::TreeNewDirectory => "project_panel::NewDirectory",
            Action::TreeCut => "project_panel::Cut",
            Action::TreeCopy => "project_panel::Copy",
            Action::TreePaste => "project_panel::Paste",
            Action::TreeDuplicate => "project_panel::Duplicate",
            Action::TreeCopyPath => "project_panel::CopyPath",
            Action::TreeCopyRelativePath => "project_panel::CopyRelativePath",
            Action::TreeTrash => "project_panel::Trash",
            Action::TreeDelete => "project_panel::Delete",
            Action::TreeRevealInFinder => "project_panel::RevealInFileManager",
            Action::TreeCollapse => "project_panel::CollapseSelectedEntry",
            Action::TreeExpand => "project_panel::ExpandSelectedEntry",
            Action::TreeCollapseAll => "project_panel::CollapseAllEntries",
            Action::TreeExpandAll => "project_panel::ExpandAllEntries",
            Action::ToggleTabSwitcher => "tab_switcher::Toggle",
            Action::ToggleTabSwitcherLast => "tab_switcher::ToggleSelectLast",
            Action::TabSwitcherCloseSelected => "tab_switcher::CloseSelectedItem",
            Action::PreviousWorkspace => "workspace::ActivatePreviousWorkspace",
            Action::NextWorkspace => "workspace::ActivateNextWorkspace",
            Action::NextWorkspaceNeedingAttention => {
                "workspace::ActivateNextWorkspaceNeedingAttention"
            }
            Action::GitOpenEntry => "git_panel::OpenSelectedEntry",
            Action::GitToggleStaged => "git::ToggleStaged",
            Action::GitStageFile => "git::StageFile",
            Action::GitUnstageFile => "git::UnstageFile",
            Action::GitStageAll => "git::StageAll",
            Action::GitUnstageAll => "git::UnstageAll",
            Action::GitRestoreFile => "git::RestoreFile",
            Action::GitCopyPath => "git_panel::CopyPath",
            Action::GitCopyRelativePath => "git_panel::CopyRelativePath",
            Action::GitFocusCommitEditor => "git_panel::FocusEditor",
            Action::GitCollapse => "git_panel::CollapseSelectedEntry",
            Action::GitExpand => "git_panel::ExpandSelectedEntry",
            Action::GitFetch => "git::Fetch",
            Action::GitPush => "git::Push",
            Action::GitPull => "git::Pull",
            Action::GitChangesTab => "git_panel::ActivateChangesTab",
            Action::GitRemoteTab => "git_panel::ActivateRemoteTab",
            Action::GitHistoryTab => "git_panel::ActivateHistoryTab",
            Action::ServicesOpen => "services_panel::Open",
            Action::ServicesToggleRunning => "services_panel::ToggleRunning",
            Action::ServicesRestart => "services_panel::Restart",
            Action::ServicesOpenInBrowser => "services_panel::OpenInBrowser",
            Action::ServicesLogs => "services_panel::ViewLogs",
            Action::ServicesCollapse => "services_panel::CollapseSelectedEntry",
            Action::ServicesExpand => "services_panel::ExpandSelectedEntry",
            Action::DatabaseOpen => "database_panel::Open",
            Action::DatabaseCollapse => "database_panel::CollapseSelectedEntry",
            Action::DatabaseExpand => "database_panel::ExpandSelectedEntry",
            Action::DatabaseCopyUrl => "database_panel::CopyUrl",
            Action::DatabaseNewConsole => "database_panel::NewConsole",
            Action::NewSideAgent => "agent::NewSideAgent",
            Action::AgentTakeOver => "agent::TakeOver",
            Action::AgentAllow => "agent::Allow",
            Action::AgentDeny => "agent::Deny",
            Action::StopAgent => "agent::StopAgent",
            Action::RunNotificationAction => "notification::RunAction",
            Action::DismissNotification => "notification::Dismiss",
            Action::CloseOtherItems => "pane::CloseOtherItems",
            Action::CloseItemsToTheLeft => "pane::CloseItemsToTheLeft",
            Action::CloseItemsToTheRight => "pane::CloseItemsToTheRight",
            Action::CloseCleanItems => "pane::CloseCleanItems",
            Action::FocusEditor => "workspace::FocusCenter",
            Action::NextRegion => "workspace::ActivateNextRegion",
            Action::PreviousRegion => "workspace::ActivatePreviousRegion",
            Action::ToggleVimMode => "workspace::ToggleVimMode",
        }
    }

    /// What the palette and the keymap page call it.
    pub fn label(self) -> &'static str {
        match self {
            Action::CommandPalette => "Command Palette",
            Action::FileFinder => "Go to File",
            Action::ProjectSearch => "Find in Project",
            Action::ProjectDiagnostics => "Project Diagnostics",
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
            Action::SelectLanguage => "Select Language",
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
            Action::TreeOpen => "Files: Open",
            Action::TreeRename => "Files: Rename",
            Action::TreeNewFile => "Files: New File",
            Action::TreeNewDirectory => "Files: New Folder",
            Action::TreeCut => "Files: Cut",
            Action::TreeCopy => "Files: Copy",
            Action::TreePaste => "Files: Paste",
            Action::TreeDuplicate => "Files: Duplicate",
            Action::TreeCopyPath => "Files: Copy Path",
            Action::TreeCopyRelativePath => "Files: Copy Relative Path",
            Action::TreeTrash => "Files: Move to Trash",
            Action::TreeDelete => "Files: Delete",
            Action::TreeRevealInFinder => "Files: Reveal in Finder",
            Action::TreeCollapse => "Files: Collapse",
            Action::TreeExpand => "Files: Expand",
            Action::TreeCollapseAll => "Files: Collapse All",
            Action::TreeExpandAll => "Files: Expand All",
            Action::ToggleTabSwitcher => "Switch Tab",
            Action::ToggleTabSwitcherLast => "Switch Tab (Oldest First)",
            Action::TabSwitcherCloseSelected => "Tab Switcher: Close Selected Tab",
            Action::PreviousWorkspace => "Previous Workspace",
            Action::NextWorkspace => "Next Workspace",
            Action::NextWorkspaceNeedingAttention => "Next Workspace Waiting for You",
            Action::GitOpenEntry => "Git: Open",
            Action::GitToggleStaged => "Git: Toggle Staged",
            Action::GitStageFile => "Git: Stage File",
            Action::GitUnstageFile => "Git: Unstage File",
            Action::GitStageAll => "Git: Stage All",
            Action::GitUnstageAll => "Git: Unstage All",
            Action::GitRestoreFile => "Git: Discard Changes",
            Action::GitCopyPath => "Git: Copy Path",
            Action::GitCopyRelativePath => "Git: Copy Relative Path",
            Action::GitFocusCommitEditor => "Git: Focus Commit Message",
            Action::GitCollapse => "Git: Collapse",
            Action::GitExpand => "Git: Expand",
            Action::GitFetch => "Git: Fetch",
            Action::GitPush => "Git: Push",
            Action::GitPull => "Git: Pull",
            Action::GitChangesTab => "Git: Changes",
            Action::GitRemoteTab => "Git: Remote",
            Action::GitHistoryTab => "Git: History",
            Action::ServicesOpen => "Services: Open",
            Action::ServicesToggleRunning => "Services: Start or Stop",
            Action::ServicesRestart => "Services: Restart",
            Action::ServicesOpenInBrowser => "Services: Open in Browser",
            Action::ServicesLogs => "Services: View Logs",
            Action::ServicesCollapse => "Services: Collapse",
            Action::ServicesExpand => "Services: Expand",
            Action::DatabaseOpen => "Database: Open",
            Action::DatabaseCollapse => "Database: Collapse",
            Action::DatabaseExpand => "Database: Expand",
            Action::DatabaseCopyUrl => "Database: Copy Connection URL",
            Action::DatabaseNewConsole => "Database: New Console",
            Action::NewSideAgent => "Agent: New Side Agent",
            Action::AgentTakeOver => "Agent: Take Over",
            Action::AgentAllow => "Agent: Allow Pending Tool",
            Action::AgentDeny => "Agent: Deny Pending Tool",
            Action::StopAgent => "Agent: Stop",
            Action::RunNotificationAction => "Notification: Run Action",
            Action::DismissNotification => "Notification: Dismiss",
            Action::CloseOtherItems => "Close Other Tabs",
            Action::CloseItemsToTheLeft => "Close Tabs to the Left",
            Action::CloseItemsToTheRight => "Close Tabs to the Right",
            Action::CloseCleanItems => "Close Saved Tabs",
            Action::FocusEditor => "Focus the Editor",
            Action::NextRegion => "Focus Next Region",
            Action::PreviousRegion => "Focus Previous Region",
            Action::ToggleVimMode => "Toggle Vim Mode",
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

/// Where a binding applies: the whole window, or only while one part of it has the keyboard.
pub const WORKSPACE: &str = "Workspace";
/// The file tree has the keyboard.
pub const PROJECT_PANEL: &str = "ProjectPanel";
thread_local! {
    static KEY_HINTS: std::cell::RefCell<Vec<(Action, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Records the window's bindings so tooltips anywhere can name the keys of their action.
pub fn set_key_hints(bindings: &[(Action, String)]) {
    KEY_HINTS.with(|hints| *hints.borrow_mut() = bindings.to_vec());
}

/// `text` followed by the keys bound to `action`, when it has any.
pub fn with_key_hint(text: &str, action: Action) -> String {
    let keys = KEY_HINTS.with(|hints| {
        hints
            .borrow()
            .iter()
            .find(|(bound, _)| *bound == action)
            .map(|(_, keys)| keys.clone())
    });
    match keys {
        Some(keys) => format!("{text}  {keys}"),
        None => text.to_string(),
    }
}

/// The tab switcher is open.
pub const TAB_SWITCHER: &str = "TabSwitcher";
/// The Git panel's list has the keyboard.
pub const GIT_PANEL: &str = "GitPanel";
/// The Services panel's list has the keyboard.
pub const SERVICES_PANEL: &str = "ServicesPanel";
/// The Database panel's tree has the keyboard.
pub const DATABASE_PANEL: &str = "DatabasePanel";
/// The contexts a keymap file may bind; others (the editor's own) are not the window's.
pub const CONTEXTS: &[&str] = &[
    WORKSPACE,
    PROJECT_PANEL,
    TAB_SWITCHER,
    GIT_PANEL,
    SERVICES_PANEL,
    DATABASE_PANEL,
];

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    /// Later entries win, so the user's file (read after the defaults) overrides them; `None` unbinds.
    bindings: Vec<(&'static str, Vec<Keystroke>, Option<Action>)>,
}

/// macOS binds window commands on Command; `ctrl-`` stays because `cmd-`` cycles the app's windows.
pub const MACOS_DEFAULTS: &[(&str, Action)] = &[
    ("cmd-shift-p", Action::CommandPalette),
    ("cmd-p", Action::FileFinder),
    ("cmd-shift-f", Action::ProjectSearch),
    ("cmd-shift-m", Action::ProjectDiagnostics),
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
    ("cmd-k m", Action::SelectLanguage),
    ("cmd-k a", Action::NewSideAgent),
    ("cmd-k o", Action::AgentTakeOver),
    ("cmd-k y", Action::AgentAllow),
    ("cmd-k n", Action::AgentDeny),
    ("cmd-k .", Action::StopAgent),
    ("cmd-k enter", Action::RunNotificationAction),
    ("cmd-k escape", Action::DismissNotification),
    ("cmd-alt-t", Action::CloseOtherItems),
    ("cmd-k e", Action::CloseItemsToTheLeft),
    ("cmd-k t", Action::CloseItemsToTheRight),
    ("cmd-k u", Action::CloseCleanItems),
    ("cmd-escape", Action::FocusEditor),
    ("cmd-k tab", Action::NextRegion),
    ("cmd-k shift-tab", Action::PreviousRegion),
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
    ("ctrl-tab", Action::ToggleTabSwitcher),
    ("ctrl-shift-tab", Action::ToggleTabSwitcherLast),
    ("cmd-alt-up", Action::PreviousWorkspace),
    ("cmd-alt-down", Action::NextWorkspace),
    ("cmd-alt-a", Action::NextWorkspaceNeedingAttention),
];

/// Linux and Windows: the same commands on Control, with those platforms' tab keys.
pub const OTHER_DEFAULTS: &[(&str, Action)] = &[
    ("ctrl-shift-p", Action::CommandPalette),
    ("ctrl-p", Action::FileFinder),
    ("ctrl-shift-f", Action::ProjectSearch),
    ("ctrl-shift-m", Action::ProjectDiagnostics),
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
    ("ctrl-k m", Action::SelectLanguage),
    ("ctrl-k a", Action::NewSideAgent),
    ("ctrl-k o", Action::AgentTakeOver),
    ("ctrl-k y", Action::AgentAllow),
    ("ctrl-k n", Action::AgentDeny),
    ("ctrl-k .", Action::StopAgent),
    ("ctrl-k enter", Action::RunNotificationAction),
    ("ctrl-k escape", Action::DismissNotification),
    ("ctrl-alt-t", Action::CloseOtherItems),
    ("ctrl-k e", Action::CloseItemsToTheLeft),
    ("ctrl-k t", Action::CloseItemsToTheRight),
    ("ctrl-k u", Action::CloseCleanItems),
    ("ctrl-escape", Action::FocusEditor),
    ("ctrl-k tab", Action::NextRegion),
    ("ctrl-k shift-tab", Action::PreviousRegion),
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
    ("ctrl-tab", Action::ToggleTabSwitcher),
    ("ctrl-shift-tab", Action::ToggleTabSwitcherLast),
    ("ctrl-alt-up", Action::PreviousWorkspace),
    ("ctrl-alt-down", Action::NextWorkspace),
    ("ctrl-alt-a", Action::NextWorkspaceNeedingAttention),
];

/// Bindings that apply only while one part of the window has the keyboard, as `(context, keys, action)`.
pub const MACOS_CONTEXT_DEFAULTS: &[(&str, &str, Action)] = &[
    (PROJECT_PANEL, "space", Action::TreeOpen),
    (PROJECT_PANEL, "enter", Action::TreeRename),
    (PROJECT_PANEL, "f2", Action::TreeRename),
    (PROJECT_PANEL, "cmd-n", Action::TreeNewFile),
    (PROJECT_PANEL, "cmd-alt-n", Action::TreeNewDirectory),
    (PROJECT_PANEL, "cmd-x", Action::TreeCut),
    (PROJECT_PANEL, "cmd-c", Action::TreeCopy),
    (PROJECT_PANEL, "cmd-v", Action::TreePaste),
    (PROJECT_PANEL, "cmd-d", Action::TreeDuplicate),
    (PROJECT_PANEL, "cmd-alt-c", Action::TreeCopyPath),
    (
        PROJECT_PANEL,
        "cmd-alt-shift-c",
        Action::TreeCopyRelativePath,
    ),
    (PROJECT_PANEL, "backspace", Action::TreeTrash),
    (PROJECT_PANEL, "delete", Action::TreeTrash),
    (PROJECT_PANEL, "cmd-backspace", Action::TreeTrash),
    (PROJECT_PANEL, "cmd-alt-backspace", Action::TreeDelete),
    (PROJECT_PANEL, "cmd-delete", Action::TreeDelete),
    (PROJECT_PANEL, "cmd-alt-r", Action::TreeRevealInFinder),
    (PROJECT_PANEL, "left", Action::TreeCollapse),
    (PROJECT_PANEL, "right", Action::TreeExpand),
    (PROJECT_PANEL, "cmd-left", Action::TreeCollapseAll),
    (PROJECT_PANEL, "cmd-right", Action::TreeExpandAll),
    (
        TAB_SWITCHER,
        "ctrl-backspace",
        Action::TabSwitcherCloseSelected,
    ),
    (GIT_PANEL, "enter", Action::GitOpenEntry),
    (GIT_PANEL, "space", Action::GitToggleStaged),
    (GIT_PANEL, "cmd-alt-y", Action::GitToggleStaged),
    (GIT_PANEL, "cmd-y", Action::GitStageFile),
    (GIT_PANEL, "cmd-shift-y", Action::GitUnstageFile),
    (GIT_PANEL, "cmd-ctrl-y", Action::GitStageAll),
    (GIT_PANEL, "cmd-ctrl-shift-y", Action::GitUnstageAll),
    (GIT_PANEL, "backspace", Action::GitRestoreFile),
    (GIT_PANEL, "delete", Action::GitRestoreFile),
    (GIT_PANEL, "cmd-backspace", Action::GitRestoreFile),
    (GIT_PANEL, "cmd-delete", Action::GitRestoreFile),
    (GIT_PANEL, "cmd-alt-c", Action::GitCopyPath),
    (GIT_PANEL, "cmd-alt-shift-c", Action::GitCopyRelativePath),
    (GIT_PANEL, "tab", Action::GitFocusCommitEditor),
    (GIT_PANEL, "shift-tab", Action::GitFocusCommitEditor),
    (GIT_PANEL, "left", Action::GitCollapse),
    (GIT_PANEL, "right", Action::GitExpand),
    (GIT_PANEL, "ctrl-g ctrl-g", Action::GitFetch),
    (GIT_PANEL, "ctrl-g up", Action::GitPush),
    (GIT_PANEL, "ctrl-g down", Action::GitPull),
    (GIT_PANEL, "cmd-1", Action::GitChangesTab),
    (GIT_PANEL, "cmd-2", Action::GitRemoteTab),
    (GIT_PANEL, "cmd-3", Action::GitHistoryTab),
    (SERVICES_PANEL, "enter", Action::ServicesOpen),
    (SERVICES_PANEL, "s", Action::ServicesToggleRunning),
    (SERVICES_PANEL, "r", Action::ServicesRestart),
    (SERVICES_PANEL, "o", Action::ServicesOpenInBrowser),
    (SERVICES_PANEL, "l", Action::ServicesLogs),
    (SERVICES_PANEL, "left", Action::ServicesCollapse),
    (SERVICES_PANEL, "right", Action::ServicesExpand),
    (DATABASE_PANEL, "enter", Action::DatabaseOpen),
    (DATABASE_PANEL, "left", Action::DatabaseCollapse),
    (DATABASE_PANEL, "right", Action::DatabaseExpand),
    (DATABASE_PANEL, "cmd-alt-c", Action::DatabaseCopyUrl),
    (DATABASE_PANEL, "cmd-n", Action::DatabaseNewConsole),
];

pub const OTHER_CONTEXT_DEFAULTS: &[(&str, &str, Action)] = &[
    (PROJECT_PANEL, "space", Action::TreeOpen),
    (PROJECT_PANEL, "enter", Action::TreeRename),
    (PROJECT_PANEL, "f2", Action::TreeRename),
    (PROJECT_PANEL, "ctrl-n", Action::TreeNewFile),
    (PROJECT_PANEL, "ctrl-alt-n", Action::TreeNewDirectory),
    (PROJECT_PANEL, "ctrl-x", Action::TreeCut),
    (PROJECT_PANEL, "ctrl-c", Action::TreeCopy),
    (PROJECT_PANEL, "ctrl-v", Action::TreePaste),
    (PROJECT_PANEL, "ctrl-d", Action::TreeDuplicate),
    (PROJECT_PANEL, "ctrl-alt-c", Action::TreeCopyPath),
    (
        PROJECT_PANEL,
        "ctrl-alt-shift-c",
        Action::TreeCopyRelativePath,
    ),
    (PROJECT_PANEL, "backspace", Action::TreeTrash),
    (PROJECT_PANEL, "delete", Action::TreeTrash),
    (PROJECT_PANEL, "ctrl-backspace", Action::TreeTrash),
    (PROJECT_PANEL, "ctrl-alt-backspace", Action::TreeDelete),
    (PROJECT_PANEL, "ctrl-delete", Action::TreeDelete),
    (PROJECT_PANEL, "ctrl-alt-r", Action::TreeRevealInFinder),
    (PROJECT_PANEL, "left", Action::TreeCollapse),
    (PROJECT_PANEL, "right", Action::TreeExpand),
    (PROJECT_PANEL, "ctrl-left", Action::TreeCollapseAll),
    (PROJECT_PANEL, "ctrl-right", Action::TreeExpandAll),
    (
        TAB_SWITCHER,
        "ctrl-backspace",
        Action::TabSwitcherCloseSelected,
    ),
    (GIT_PANEL, "enter", Action::GitOpenEntry),
    (GIT_PANEL, "space", Action::GitToggleStaged),
    (GIT_PANEL, "ctrl-alt-y", Action::GitToggleStaged),
    (GIT_PANEL, "alt-y", Action::GitStageFile),
    (GIT_PANEL, "alt-shift-y", Action::GitUnstageFile),
    (GIT_PANEL, "ctrl-space", Action::GitStageAll),
    (GIT_PANEL, "ctrl-shift-space", Action::GitUnstageAll),
    (GIT_PANEL, "backspace", Action::GitRestoreFile),
    (GIT_PANEL, "delete", Action::GitRestoreFile),
    (GIT_PANEL, "ctrl-backspace", Action::GitRestoreFile),
    (GIT_PANEL, "ctrl-delete", Action::GitRestoreFile),
    (GIT_PANEL, "ctrl-alt-c", Action::GitCopyPath),
    (GIT_PANEL, "ctrl-alt-shift-c", Action::GitCopyRelativePath),
    (GIT_PANEL, "tab", Action::GitFocusCommitEditor),
    (GIT_PANEL, "shift-tab", Action::GitFocusCommitEditor),
    (GIT_PANEL, "left", Action::GitCollapse),
    (GIT_PANEL, "right", Action::GitExpand),
    (GIT_PANEL, "ctrl-g ctrl-g", Action::GitFetch),
    (GIT_PANEL, "ctrl-g up", Action::GitPush),
    (GIT_PANEL, "ctrl-g down", Action::GitPull),
    (GIT_PANEL, "ctrl-1", Action::GitChangesTab),
    (GIT_PANEL, "ctrl-2", Action::GitRemoteTab),
    (GIT_PANEL, "ctrl-3", Action::GitHistoryTab),
    (SERVICES_PANEL, "enter", Action::ServicesOpen),
    (SERVICES_PANEL, "s", Action::ServicesToggleRunning),
    (SERVICES_PANEL, "r", Action::ServicesRestart),
    (SERVICES_PANEL, "o", Action::ServicesOpenInBrowser),
    (SERVICES_PANEL, "l", Action::ServicesLogs),
    (SERVICES_PANEL, "left", Action::ServicesCollapse),
    (SERVICES_PANEL, "right", Action::ServicesExpand),
    (DATABASE_PANEL, "enter", Action::DatabaseOpen),
    (DATABASE_PANEL, "left", Action::DatabaseCollapse),
    (DATABASE_PANEL, "right", Action::DatabaseExpand),
    (DATABASE_PANEL, "ctrl-alt-c", Action::DatabaseCopyUrl),
    (DATABASE_PANEL, "ctrl-n", Action::DatabaseNewConsole),
];

fn platform_context_defaults() -> &'static [(&'static str, &'static str, Action)] {
    if cfg!(target_os = "macos") {
        MACOS_CONTEXT_DEFAULTS
    } else {
        OTHER_CONTEXT_DEFAULTS
    }
}

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
        let window = platform_defaults()
            .iter()
            .filter_map(|(keys, action)| Some((WORKSPACE, sequence(keys)?, Some(*action))));
        let contextual = platform_context_defaults()
            .iter()
            .filter_map(|(context, keys, action)| Some((*context, sequence(keys)?, Some(*action))));
        Keymap {
            bindings: window.chain(contextual).collect(),
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
            let named = section
                .get("context")
                .and_then(|context| context.as_str())
                .unwrap_or(WORKSPACE);
            let Some(context) = CONTEXTS.iter().find(|known| **known == named).copied() else {
                continue;
            };
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
                self.bindings.push((context, strokes, action));
            }
        }
        problems
    }

    /// What `pending` then `stroke` does in the window as a whole.
    pub fn match_keys(&self, pending: &[Keystroke], stroke: &Keystroke) -> KeyMatch {
        self.match_in(&[WORKSPACE], pending, stroke)
    }

    /// What `pending` then `stroke` does where `contexts` (broadest first) have the keyboard: the most specific
    /// context that binds the keys wins.
    pub fn match_in(
        &self,
        contexts: &[&str],
        pending: &[Keystroke],
        stroke: &Keystroke,
    ) -> KeyMatch {
        let mut typed: Vec<Keystroke> = pending.to_vec();
        typed.push(stroke.normalized());
        for context in contexts.iter().rev() {
            if let Some((_, _, action)) = self
                .bindings
                .iter()
                .rev()
                .find(|(bound, keys, _)| bound == context && *keys == typed)
            {
                return action.map_or(KeyMatch::None, KeyMatch::Action);
            }
        }
        let longer = self.bindings.iter().rev().any(|(bound, keys, action)| {
            contexts.contains(bound)
                && action.is_some()
                && keys.len() > typed.len()
                && keys[..typed.len()] == typed[..]
        });
        if longer {
            KeyMatch::Pending
        } else {
            KeyMatch::None
        }
    }

    /// The binding that currently runs `action`, as text (`cmd-k cmd-s`).
    pub fn binding_for(&self, action: Action) -> Option<String> {
        let mut rebound: Vec<(&str, &Vec<Keystroke>)> = Vec::new();
        for (context, keys, bound) in self.bindings.iter().rev() {
            if rebound.contains(&(*context, keys)) {
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
            rebound.push((*context, keys));
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
    fn a_panel_binding_wins_only_while_the_panel_has_the_keyboard() {
        let mut keymap = Keymap::defaults();
        let new_file = if cfg!(target_os = "macos") {
            "cmd-n"
        } else {
            "ctrl-n"
        };
        assert_eq!(
            keymap.match_keys(&[], &stroke(new_file)),
            KeyMatch::Action(Action::NewWorkspace)
        );
        assert_eq!(
            keymap.match_in(&[WORKSPACE, PROJECT_PANEL], &[], &stroke(new_file)),
            KeyMatch::Action(Action::TreeNewFile)
        );
        assert_eq!(
            keymap.match_in(&[WORKSPACE, PROJECT_PANEL], &[], &stroke("space")),
            KeyMatch::Action(Action::TreeOpen)
        );
        assert_eq!(keymap.match_keys(&[], &stroke("space")), KeyMatch::None);
        let problems = keymap.apply_user(
            r#"[{"context": "ProjectPanel", "bindings": {"f2": null, "cmd-shift-r": "project_panel::Rename"}}]"#,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            keymap.match_in(&[WORKSPACE, PROJECT_PANEL], &[], &stroke("f2")),
            KeyMatch::None
        );
        assert_eq!(
            keymap.match_in(&[WORKSPACE, PROJECT_PANEL], &[], &stroke("cmd-shift-r")),
            KeyMatch::Action(Action::TreeRename)
        );
    }

    #[test]
    fn every_context_binding_is_readable_and_not_doubled() {
        for table in [MACOS_CONTEXT_DEFAULTS, OTHER_CONTEXT_DEFAULTS] {
            let mut keys: Vec<(&str, &str)> = table
                .iter()
                .map(|(context, keys, _)| (*context, *keys))
                .collect();
            let before = keys.len();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), before, "a key is bound twice in one context");
            assert!(table
                .iter()
                .all(|(context, keys, _)| CONTEXTS.contains(context) && sequence(keys).is_some()));
        }
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

    #[test]
    fn tooltips_name_the_keys_of_their_action() {
        set_key_hints(&[(Action::GitFetch, "ctrl-g ctrl-g".into())]);
        assert_eq!(
            with_key_hint("Fetch All", Action::GitFetch),
            "Fetch All  ctrl-g ctrl-g"
        );
        assert_eq!(with_key_hint("Push", Action::GitPush), "Push", "unbound");
    }
}
