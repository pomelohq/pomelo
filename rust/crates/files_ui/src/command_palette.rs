//! Cmd+Shift+P: a modal listing every editor command by name, fuzzy-filtered as you type, with its key
//! binding. Commands run from here float to the top next time, and Up on the first row recalls past queries.

use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};

use editor::transform::{LineTransform, TextTransform};
use ui::{div, label, theme, LabelSize, Node};
use workspace::{render_keystroke, EditKey};

use crate::fuzzy::{fuzzy_match, Match};
use crate::text_field::{FieldFont, TextField, INPUT_FONT};

pub const WIDTH: f32 = 608.0;
const MAX_RESULTS_HEIGHT: f32 = 384.0;
const HEAD_HEIGHT: f32 = 36.0;
const PLACEHOLDER: &str = "Execute a command...";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    Key(EditKey),
    Text(TextTransform),
    Lines(LineTransform),
    /// A window command, by the id it was listed with.
    External(u64),
}

/// Click targets inside the palette, as offsets from the view's palette id base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteClick {
    Row(usize),
    Run,
    /// The workspace switcher's new-workspace button.
    New,
}

impl PaletteClick {
    const RUN: u64 = 0;
    const NEW: u64 = 1;

    pub fn offset(self) -> u64 {
        match self {
            PaletteClick::Run => Self::RUN,
            PaletteClick::New => Self::NEW,
            PaletteClick::Row(index) => index as u64 + 2,
        }
    }

    pub fn from_offset(offset: u64) -> Self {
        match offset {
            Self::RUN => PaletteClick::Run,
            Self::NEW => PaletteClick::New,
            n => PaletteClick::Row((n - 2) as usize),
        }
    }
}

use PaletteAction::{Key, Lines, Text};

/// The key `key` is bound to, as the palette lists it.
pub(crate) fn key_hint(key: EditKey) -> Option<&'static str> {
    COMMANDS
        .iter()
        .find(|(_, action, _)| *action == Key(key))
        .and_then(|(_, _, keys)| keys.first().copied())
}

// Bindings mirror the key map in the binary's event loop; keep both in sync.
const COMMANDS: &[(&str, PaletteAction, &[&str])] = &[
    ("editor::MoveLeft", Key(EditKey::Left), &["left"]),
    ("editor::MoveRight", Key(EditKey::Right), &["right"]),
    ("editor::MoveUp", Key(EditKey::Up), &["up"]),
    ("editor::MoveDown", Key(EditKey::Down), &["down"]),
    (
        "editor::MoveToPreviousWordStart",
        Key(EditKey::WordLeft),
        &["alt-left"],
    ),
    (
        "editor::MoveToNextWordEnd",
        Key(EditKey::WordRight),
        &["alt-right"],
    ),
    (
        "editor::MoveToPreviousSubwordStart",
        Key(EditKey::SubwordLeft),
        &["ctrl-alt-left"],
    ),
    (
        "editor::MoveToNextSubwordEnd",
        Key(EditKey::SubwordRight),
        &["ctrl-alt-right"],
    ),
    (
        "editor::MoveToBeginningOfLine",
        Key(EditKey::Home),
        &["cmd-left"],
    ),
    ("editor::MoveToEndOfLine", Key(EditKey::End), &["cmd-right"]),
    (
        "editor::MoveToBeginning",
        Key(EditKey::DocumentStart),
        &["cmd-up"],
    ),
    (
        "editor::MoveToEnd",
        Key(EditKey::DocumentEnd),
        &["cmd-down"],
    ),
    ("editor::MovePageUp", Key(EditKey::PageUp), &["pageup"]),
    (
        "editor::MovePageDown",
        Key(EditKey::PageDown),
        &["pagedown"],
    ),
    ("editor::Backspace", Key(EditKey::Backspace), &["backspace"]),
    ("editor::Delete", Key(EditKey::Delete), &["delete"]),
    (
        "editor::DeleteToPreviousWordStart",
        Key(EditKey::DeleteWordLeft),
        &["alt-backspace"],
    ),
    (
        "editor::DeleteToNextWordEnd",
        Key(EditKey::DeleteWordRight),
        &["alt-delete"],
    ),
    (
        "editor::DeleteToPreviousSubwordStart",
        Key(EditKey::DeleteSubwordLeft),
        &["ctrl-alt-backspace"],
    ),
    (
        "editor::DeleteToNextSubwordEnd",
        Key(EditKey::DeleteSubwordRight),
        &["ctrl-alt-delete"],
    ),
    (
        "editor::DeleteToBeginningOfLine",
        Key(EditKey::DeleteToLineStart),
        &["cmd-backspace"],
    ),
    (
        "editor::DeleteToEndOfLine",
        Key(EditKey::DeleteToLineEnd),
        &["cmd-delete"],
    ),
    ("editor::Newline", Key(EditKey::Enter), &["enter"]),
    ("editor::Tab", Key(EditKey::Tab), &["tab"]),
    ("editor::Backtab", Key(EditKey::Backtab), &["shift-tab"]),
    ("editor::Indent", Key(EditKey::Indent), &["cmd-]"]),
    ("editor::Outdent", Key(EditKey::Outdent), &["cmd-["]),
    (
        "editor::ToggleComments",
        Key(EditKey::ToggleComments),
        &["cmd-/"],
    ),
    (
        "editor::DeleteLine",
        Key(EditKey::DeleteLine),
        &["cmd-shift-k"],
    ),
    (
        "editor::DuplicateLineUp",
        Key(EditKey::DuplicateLineUp),
        &["alt-shift-up"],
    ),
    (
        "editor::DuplicateLineDown",
        Key(EditKey::DuplicateLineDown),
        &["alt-shift-down"],
    ),
    ("editor::MoveLineUp", Key(EditKey::MoveLineUp), &["alt-up"]),
    (
        "editor::MoveLineDown",
        Key(EditKey::MoveLineDown),
        &["alt-down"],
    ),
    ("editor::JoinLines", Key(EditKey::JoinLines), &["ctrl-j"]),
    ("editor::Transpose", Key(EditKey::Transpose), &["ctrl-t"]),
    ("editor::SelectAll", Key(EditKey::SelectAll), &["cmd-a"]),
    ("editor::SelectNext", Key(EditKey::SelectNext), &["cmd-d"]),
    (
        "editor::SelectAllMatches",
        Key(EditKey::SelectAllMatches),
        &["cmd-shift-l"],
    ),
    (
        "editor::AddSelectionAbove",
        Key(EditKey::AddCursorAbove),
        &["cmd-alt-up"],
    ),
    (
        "editor::AddSelectionBelow",
        Key(EditKey::AddCursorBelow),
        &["cmd-alt-down"],
    ),
    (
        "editor::SelectLargerSyntaxNode",
        Key(EditKey::SelectLargerSyntaxNode),
        &["ctrl-shift-right"],
    ),
    (
        "editor::SelectSmallerSyntaxNode",
        Key(EditKey::SelectSmallerSyntaxNode),
        &["ctrl-shift-left"],
    ),
    (
        "editor::MoveToEnclosingBracket",
        Key(EditKey::MoveToEnclosingBracket),
        &["ctrl-m"],
    ),
    (
        "editor::ToggleSelectedDiffHunks",
        Key(EditKey::ToggleSelectedDiffHunks),
        &["cmd-'"],
    ),
    (
        "editor::ExpandAllDiffHunks",
        Key(EditKey::ExpandAllDiffHunks),
        &["cmd-\""],
    ),
    (
        "editor::ShowCompletions",
        Key(EditKey::ShowCompletions),
        &["ctrl-space"],
    ),
    (
        "editor::ShowWordCompletions",
        Key(EditKey::ShowWordCompletions),
        &[],
    ),
    ("editor::Hover", Key(EditKey::Hover), &[]),
    ("editor::GoToHunk", Key(EditKey::GoToHunk), &["cmd-f8"]),
    (
        "editor::GoToDiagnostic",
        Key(EditKey::GoToDiagnostic),
        &["f8"],
    ),
    (
        "editor::GoToPreviousDiagnostic",
        Key(EditKey::GoToPreviousDiagnostic),
        &["shift-f8"],
    ),
    (
        "editor::GoToDefinition",
        Key(EditKey::GoToDefinition),
        &["f12"],
    ),
    (
        "editor::GoToDeclaration",
        Key(EditKey::GoToDeclaration),
        &["ctrl-f12"],
    ),
    (
        "editor::GoToTypeDefinition",
        Key(EditKey::GoToTypeDefinition),
        &["cmd-f12"],
    ),
    (
        "editor::GoToImplementation",
        Key(EditKey::GoToImplementation),
        &["shift-f12"],
    ),
    (
        "editor::GoToPreviousHunk",
        Key(EditKey::GoToPreviousHunk),
        &["cmd-shift-f8"],
    ),
    ("git::Restore", Key(EditKey::GitRestore), &["cmd-alt-z"]),
    (
        "git::ToggleStaged",
        Key(EditKey::ToggleStaged),
        &["cmd-alt-y"],
    ),
    ("git::StageAndNext", Key(EditKey::StageAndNext), &["cmd-y"]),
    (
        "git::UnstageAndNext",
        Key(EditKey::UnstageAndNext),
        &["cmd-shift-y"],
    ),
    ("editor::Undo", Key(EditKey::Undo), &["cmd-z"]),
    ("editor::Redo", Key(EditKey::Redo), &["cmd-shift-z"]),
    (
        "editor::ToggleSoftWrap",
        Key(EditKey::ToggleSoftWrap),
        &["cmd-k", "z"],
    ),
    (
        "outline::Toggle",
        Key(EditKey::ToggleOutline),
        &["cmd-shift-o"],
    ),
    (
        "workspace::NewCenterTerminal",
        Key(EditKey::NewCenterTerminal),
        &[],
    ),
    (
        "go_to_line::Toggle",
        Key(EditKey::ToggleGoToLine),
        &["ctrl-g"],
    ),
    ("pane::GoBack", Key(EditKey::GoBack), &["ctrl--"]),
    (
        "pane::GoForward",
        Key(EditKey::GoForward),
        &["ctrl-shift--"],
    ),
    (
        "buffer_search::Deploy",
        Key(EditKey::DeploySearch),
        &["cmd-f"],
    ),
    (
        "buffer_search::UseSelectionForFind",
        Key(EditKey::UseSelectionForFind),
        &["cmd-e"],
    ),
    (
        "search::ToggleReplace",
        Key(EditKey::ToggleSearchReplace),
        &["cmd-shift-h"],
    ),
    (
        "search::SelectNextMatch",
        Key(EditKey::SelectNextMatch),
        &["cmd-g"],
    ),
    (
        "search::SelectPreviousMatch",
        Key(EditKey::SelectPreviousMatch),
        &["cmd-shift-g"],
    ),
    (
        "search::SelectAllMatches",
        Key(EditKey::SelectAllMatchesInSearch),
        &["alt-enter"],
    ),
    (
        "search::ToggleCaseSensitive",
        Key(EditKey::ToggleSearchCaseSensitive),
        &["alt-cmd-c"],
    ),
    (
        "search::ToggleWholeWord",
        Key(EditKey::ToggleSearchWholeWord),
        &["alt-cmd-w"],
    ),
    (
        "search::ToggleRegex",
        Key(EditKey::ToggleSearchRegex),
        &["alt-cmd-x"],
    ),
    (
        "search::ReplaceAll",
        Key(EditKey::ReplaceAll),
        &["cmd-enter"],
    ),
    (
        "editor::ConvertToUpperCase",
        Text(TextTransform::UpperCase),
        &[],
    ),
    (
        "editor::ConvertToLowerCase",
        Text(TextTransform::LowerCase),
        &[],
    ),
    (
        "editor::ConvertToTitleCase",
        Text(TextTransform::TitleCase),
        &[],
    ),
    (
        "editor::ConvertToSnakeCase",
        Text(TextTransform::SnakeCase),
        &[],
    ),
    (
        "editor::ConvertToKebabCase",
        Text(TextTransform::KebabCase),
        &[],
    ),
    (
        "editor::ConvertToUpperCamelCase",
        Text(TextTransform::UpperCamelCase),
        &[],
    ),
    (
        "editor::ConvertToLowerCamelCase",
        Text(TextTransform::LowerCamelCase),
        &[],
    ),
    (
        "editor::ConvertToSentenceCase",
        Text(TextTransform::SentenceCase),
        &[],
    ),
    (
        "editor::ConvertToOppositeCase",
        Text(TextTransform::OppositeCase),
        &[],
    ),
    ("editor::ToggleCase", Text(TextTransform::ToggleCase), &[]),
    ("editor::ConvertToRot13", Text(TextTransform::Rot13), &[]),
    ("editor::ConvertToRot47", Text(TextTransform::Rot47), &[]),
    (
        "editor::SortLinesCaseSensitive",
        Lines(LineTransform::SortCaseSensitive),
        &[],
    ),
    (
        "editor::SortLinesCaseInsensitive",
        Lines(LineTransform::SortCaseInsensitive),
        &[],
    ),
    (
        "editor::SortLinesByLength",
        Lines(LineTransform::SortByLength),
        &[],
    ),
    (
        "editor::UniqueLinesCaseSensitive",
        Lines(LineTransform::UniqueCaseSensitive),
        &[],
    ),
    (
        "editor::UniqueLinesCaseInsensitive",
        Lines(LineTransform::UniqueCaseInsensitive),
        &[],
    ),
    ("editor::ReverseLines", Lines(LineTransform::Reverse), &[]),
];

/// `editor::ConvertToUpperCase` -> `editor: convert to upper case`; acronym runs stay together.
pub fn humanize_action_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut result = String::with_capacity(name.len() * 2);
    let push_space = |result: &mut String| {
        if !result.ends_with(' ') {
            result.push(' ');
        }
    };
    let mut index = 0;
    while let Some(&c) = chars.get(index) {
        if c == ':' {
            result.push(if result.ends_with(':') { ' ' } else { ':' });
            index += 1;
        } else if c == '_' {
            result.push(' ');
            index += 1;
        } else if c.is_uppercase() {
            let start = index;
            index += 1;
            while chars.get(index).is_some_and(|next| next.is_uppercase()) {
                index += 1;
            }
            let run = &chars[start..index];
            let splits_before_last =
                run.len() > 1 && chars.get(index).is_some_and(|next| next.is_lowercase());
            if run.len() > 1 {
                let acronym_end = run.len() - usize::from(splits_before_last);
                if acronym_end > 0 {
                    push_space(&mut result);
                    result.extend(&run[..acronym_end]);
                }
                if splits_before_last {
                    push_space(&mut result);
                    result.extend(run[acronym_end].to_lowercase());
                }
            } else {
                push_space(&mut result);
                result.extend(c.to_lowercase());
            }
        } else {
            result.push(c);
            index += 1;
        }
    }
    result
}

/// Trim, treat `_` as a space, and collapse repeated spaces and colons, so `editor::sort_lines` and
/// `editor: sort lines` find the same commands.
pub fn normalize_action_query(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut last = None;
    for c in input.trim().chars() {
        let c = if c == '_' { ' ' } else { c };
        match last {
            Some(':') if c == ':' => continue,
            Some(l) if char::is_whitespace(l) && c.is_whitespace() => continue,
            _ => last = Some(c),
        }
        result.push(c);
    }
    result
}

pub struct Command {
    pub name: String,
    pub action: PaletteAction,
    binding: Vec<String>,
    usage: Option<u32>,
}

/// Past queries, walked with Up/Down; only entries starting with what was typed before walking are visited.
#[derive(Default)]
pub struct QueryHistory {
    entries: VecDeque<String>,
    cursor: Option<usize>,
    prefix: Option<String>,
}

impl QueryHistory {
    fn add(&mut self, query: String) {
        self.entries.retain(|entry| entry != &query);
        self.entries.push_back(query);
        self.reset_cursor();
    }

    fn reset_cursor(&mut self) {
        self.cursor = None;
        self.prefix = None;
    }

    fn is_navigating(&self) -> bool {
        self.cursor.is_some()
    }

    /// Editing the query by hand ends a walk.
    fn validate_cursor(&mut self, current: &str) -> Option<usize> {
        if let Some(position) = self.cursor {
            if self.entries.get(position).map(String::as_str) != Some(current) {
                self.reset_cursor();
            }
        }
        self.cursor
    }

    fn previous(&mut self, current: &str) -> Option<String> {
        if self.validate_cursor(current).is_none() {
            self.prefix = Some(current.to_string());
        }
        let prefix = self.prefix.clone().unwrap_or_default();
        let start = self.cursor.unwrap_or(self.entries.len());
        let found = (0..start)
            .rev()
            .find(|&i| self.entries.get(i).is_some_and(|e| e.starts_with(&prefix)))?;
        self.cursor = Some(found);
        self.entries.get(found).cloned()
    }

    fn next(&mut self, current: &str) -> Option<String> {
        let selected = self.validate_cursor(current)?;
        let prefix = self.prefix.clone().unwrap_or_default();
        let found = (selected + 1..self.entries.len())
            .find(|&i| self.entries.get(i).is_some_and(|e| e.starts_with(&prefix)))?;
        self.cursor = Some(found);
        self.entries.get(found).cloned()
    }
}

/// What the palette remembers between openings: how often each command ran from it, and past queries.
#[derive(Default)]
pub struct PaletteMemory {
    usage: HashMap<String, u32>,
    history: QueryHistory,
}

pub struct CommandPalette {
    pub field: TextField,
    commands: Vec<Command>,
    /// Commands run from the palette before, which lead the list in `commands`.
    used_count: usize,
    matches: Vec<Match>,
    pub(crate) selected: usize,
    scroll_top: usize,
    /// Scrolled distance not yet worth a whole row.
    scroll_remainder: f32,
    pub scrollbar: crate::list_scrollbar::ScrollbarReveal,
    /// The click id under the pointer.
    pub hovered: Option<u64>,
    placeholder: &'static str,
    /// The query the matches were made for; a new one puts the selection back on the best match.
    matched_query: String,
    /// Set when the palette lists workspaces: one entry per command, in the same order.
    switcher: Option<workspace::WorkspaceSwitcher>,
}

/// Move a row list's first visible row by a wheel delta (positive = toward the top), carrying partial rows over
/// to the next delta; returns whether the first row changed.
pub(crate) fn scroll_rows(
    top: &mut usize,
    remainder: &mut f32,
    dy: f32,
    row_height: f32,
    max_top: usize,
) -> bool {
    *remainder -= dy;
    let rows = (*remainder / row_height).trunc();
    if rows == 0.0 {
        return false;
    }
    *remainder -= rows * row_height;
    let next = (*top as isize + rows as isize).clamp(0, max_top as isize) as usize;
    if next == 0 || next == max_top {
        *remainder = 0.0;
    }
    let moved = next != *top;
    *top = next;
    moved
}

impl CommandPalette {
    #[cfg(test)]
    pub fn new(memory: &PaletteMemory) -> Self {
        Self::with_commands(memory, &[], true)
    }

    /// The editor's commands (when an editor is there to take them) and the window's `extra` ones.
    pub fn with_commands(
        memory: &PaletteMemory,
        extra: &[workspace::ExtraCommand],
        editor: bool,
    ) -> Self {
        let editor_commands =
            COMMANDS
                .iter()
                .filter(|_| editor)
                .map(|&(action_name, action, binding)| {
                    (
                        humanize_action_name(action_name),
                        action,
                        binding.iter().map(|key| key.to_string()).collect(),
                    )
                });
        let window_commands = extra.iter().map(|command| {
            (
                command.name.clone(),
                PaletteAction::External(command.id),
                command.keys.clone(),
            )
        });
        let mut commands: Vec<Command> = editor_commands
            .chain(window_commands)
            .map(|(name, action, binding)| {
                let usage = memory.usage.get(&name).copied();
                Command {
                    name,
                    action,
                    binding,
                    usage,
                }
            })
            .collect();
        commands.sort_by(|a, b| {
            Reverse(a.usage)
                .cmp(&Reverse(b.usage))
                .then_with(|| a.name.cmp(&b.name))
        });
        let used_count = commands.iter().take_while(|c| c.usage.is_some()).count();
        let mut palette = Self {
            field: TextField::default(),
            commands,
            used_count,
            matches: Vec::new(),
            selected: 0,
            scroll_top: 0,
            scroll_remainder: 0.0,
            scrollbar: Default::default(),
            hovered: None,
            placeholder: PLACEHOLDER,
            matched_query: String::new(),
            switcher: None,
        };
        palette.update_matches();
        palette
    }

    /// The workspaces in the order given, matched on their name, ticket key and branch.
    pub fn workspace_switcher(switcher: workspace::WorkspaceSwitcher) -> Self {
        let commands = switcher
            .entries
            .iter()
            .map(|entry| {
                let key = workspace::ticket_key(&entry.row).unwrap_or_default();
                Command {
                    name: format!(
                        "{} {key} {}",
                        workspace::switcher_title(&entry.row),
                        entry.row.branch
                    ),
                    action: PaletteAction::External(entry.id),
                    binding: Vec::new(),
                    usage: None,
                }
            })
            .collect();
        let mut palette = Self {
            field: TextField::default(),
            commands,
            used_count: 0,
            matches: Vec::new(),
            selected: 0,
            scroll_top: 0,
            scroll_remainder: 0.0,
            scrollbar: Default::default(),
            hovered: None,
            placeholder: "Switch to workspace...",
            matched_query: String::new(),
            switcher: Some(switcher),
        };
        palette.update_matches();
        palette
    }

    /// The id the switcher's new-workspace button hands back.
    pub fn new_workspace_id(&self) -> Option<u64> {
        self.switcher.as_ref().map(|s| s.new_workspace.id)
    }

    pub fn with_placeholder(mut self, placeholder: &'static str) -> Self {
        self.placeholder = placeholder;
        self
    }

    pub fn update_matches(&mut self) {
        let names: Vec<&str> = self.commands.iter().map(|c| c.name.as_str()).collect();
        let query = self.field.text();
        let mut matches = fuzzy_match(&names, &normalize_action_query(&query));
        let used_count = self.used_count;
        matches.sort_by_key(|m| {
            if m.candidate < used_count {
                m.candidate
            } else {
                usize::MAX
            }
        });
        self.matches = matches;
        if query != self.matched_query {
            self.selected = 0;
            self.scroll_top = 0;
            self.scroll_remainder = 0.0;
            self.matched_query = query;
        }
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        self.scroll_to_selected();
    }

    #[cfg(test)]
    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    #[cfg(test)]
    pub fn command(&self, index: usize) -> Option<&Command> {
        self.commands.get(index)
    }

    fn set_query(&mut self, query: &str) {
        self.field.set_text(query);
        self.field.move_to_end();
        self.update_matches();
    }

    pub fn select_previous(&mut self, memory: &mut PaletteMemory) {
        if self.selected == 0 || memory.history.is_navigating() {
            if let Some(query) = memory.history.previous(&self.field.text()) {
                return self.set_query(&query);
            }
        }
        let count = self.matches.len();
        if count > 0 {
            self.selected = if self.selected == 0 {
                count - 1
            } else {
                self.selected - 1
            };
            self.scroll_to_selected();
        }
    }

    pub fn select_next(&mut self, memory: &mut PaletteMemory) {
        if memory.history.is_navigating() {
            let query = match memory.history.next(&self.field.text()) {
                Some(query) => query,
                None => {
                    let prefix = memory.history.prefix.take().unwrap_or_default();
                    memory.history.reset_cursor();
                    prefix
                }
            };
            return self.set_query(&query);
        }
        let count = self.matches.len();
        if count > 0 {
            self.selected = if self.selected + 1 == count {
                0
            } else {
                self.selected + 1
            };
            self.scroll_to_selected();
        }
    }

    pub fn select_row(&mut self, row: usize) {
        if row < self.matches.len() {
            self.selected = row;
        }
    }

    /// The chosen command's action, recording the run; `None` when nothing matches.
    pub fn confirm(&self, memory: &mut PaletteMemory) -> Option<PaletteAction> {
        let found = self.matches.get(self.selected)?;
        let command = self.commands.get(found.candidate)?;
        if self.switcher.is_some() {
            return Some(command.action);
        }
        let query = self.field.text();
        if !query.is_empty() {
            memory.history.add(query);
        }
        *memory.usage.entry(command.name.clone()).or_default() += 1;
        Some(command.action)
    }

    pub fn scroll_by(&mut self, dy: f32) -> bool {
        let max_top = self.max_top();
        let step = match self.switcher {
            Some(_) => SWITCHER_ROW_HEIGHT,
            None => row_height(),
        };
        let moved = scroll_rows(
            &mut self.scroll_top,
            &mut self.scroll_remainder,
            dy,
            step * ui::ui_text_scale(),
            max_top,
        );
        if moved {
            self.scrollbar.reveal();
        }
        moved
    }

    fn list_height(&self) -> f32 {
        match self.switcher {
            Some(_) => SWITCHER_LIST_HEIGHT,
            None => MAX_RESULTS_HEIGHT - 8.0,
        }
    }

    /// Section headers only make sense over the whole list, not over matches ranked by score.
    fn shows_sections(&self) -> bool {
        self.switcher.is_some() && self.matched_query.trim().is_empty()
    }

    fn header_of(&self, row: usize) -> Option<&'static str> {
        let switcher = self.switcher.as_ref().filter(|_| self.shows_sections())?;
        switcher
            .entries
            .get(self.matches.get(row)?.candidate)?
            .section
    }

    fn height_of(&self, row: usize) -> f32 {
        match self.switcher {
            Some(_) if self.header_of(row).is_some() => {
                SWITCHER_ROW_HEIGHT + SWITCHER_HEADER_HEIGHT
            }
            Some(_) => SWITCHER_ROW_HEIGHT,
            None => row_height(),
        }
    }

    /// One past the last row that fits when the list starts at `top`; at least one row shows.
    fn visible_end(&self, top: usize) -> usize {
        let mut used = 0.0;
        let mut end = top;
        while end < self.matches.len() {
            used += self.height_of(end);
            if used > self.list_height() + 0.5 && end > top {
                break;
            }
            end += 1;
        }
        end
    }

    fn max_top(&self) -> usize {
        let mut top = self.matches.len();
        while top > 0 && self.visible_end(top - 1) == self.matches.len() {
            top -= 1;
        }
        top
    }

    fn scroll_to_selected(&mut self) {
        let before = self.scroll_top;
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        }
        while self.selected >= self.visible_end(self.scroll_top) {
            self.scroll_top += 1;
        }
        self.scroll_top = self.scroll_top.min(self.max_top());
        if self.scroll_top != before {
            self.scrollbar.reveal();
        }
    }

    pub fn render(&self, id_base: u64) -> Node {
        let colors = theme();
        let head = div()
            .row()
            .items_center()
            .h_px(HEAD_HEIGHT)
            .px(10.0)
            .child(self.field.render(
                self.placeholder,
                true,
                colors.editor_foreground,
                INPUT_FONT * FieldFont::Ui.line_height(),
                FieldFont::Ui,
            ));
        let results: Node = if self.matches.is_empty() {
            div()
                .col()
                .py(8.0)
                .child(
                    div().row().px(4.0).child(
                        div().row().flex(1.0).items_center().px(6.0).py(4.0).child(
                            label("No matches")
                                .label_size(LabelSize::Default)
                                .color(colors.text_muted),
                        ),
                    ),
                )
                .into()
        } else {
            let end = self.visible_end(self.scroll_top);
            let shown: f32 = (self.scroll_top..end).map(|row| self.height_of(row)).sum();
            let scrollbar = crate::list_scrollbar::render(
                WIDTH - 1.0,
                shown + 8.0,
                self.scroll_top,
                end - self.scroll_top,
                self.matches.len(),
                self.scrollbar.opacity(),
            );
            let rows = (self.scroll_top..end).map(|row| match self.switcher {
                Some(_) => self.render_workspace_row(row, id_base),
                None => self.render_row(row, id_base),
            });
            div()
                .col()
                .py(4.0)
                .children(scrollbar)
                .children(rows)
                .into()
        };
        let footer = match &self.switcher {
            Some(switcher) => self.switcher_footer(switcher, id_base),
            None => div()
                .row()
                .justify_end()
                .items_center()
                .p(6.0)
                .gap(4.0)
                .child(footer_button(
                    id_base + PaletteClick::Run.offset(),
                    "Run",
                    "enter",
                    colors.text,
                    self.hovered == Some(id_base + PaletteClick::Run.offset()),
                ))
                .into(),
        };
        div()
            .col()
            .w_px(WIDTH)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .bg(colors.elevated_surface_background)
            .child(head)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(results)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(footer)
            .into()
    }

    fn render_row(&self, row: usize, id_base: u64) -> Node {
        let colors = theme();
        let Some((found, command)) = self
            .matches
            .get(row)
            .and_then(|m| Some((m, self.commands.get(m.candidate)?)))
        else {
            return div().into();
        };
        let mut item = div()
            .row()
            .flex(1.0)
            .px(6.0)
            .py(4.0)
            .rounded(4.0)
            .on_click(id_base + PaletteClick::Row(row).offset());
        if row == self.selected {
            item = item.bg(colors.element_selected);
        }
        let keys = div().row().items_center().gap(4.0).children(
            command
                .binding
                .iter()
                .map(|keystroke| render_keystroke(keystroke, LabelSize::Default.px())),
        );
        div()
            .row()
            .px(4.0)
            .child(
                item.child(
                    div()
                        .row()
                        .flex(1.0)
                        .py(1.0)
                        .items_center()
                        .justify_between()
                        .gap(8.0)
                        .child(highlighted_label(
                            &command.name,
                            &found.positions,
                            LabelSize::Default.px(),
                        ))
                        .child(keys),
                ),
            )
            .into()
    }
}

const SWITCHER_ROW_HEIGHT: f32 = 45.0;
const SWITCHER_HEADER_HEIGHT: f32 = 26.0;
const SWITCHER_LIST_HEIGHT: f32 = 420.0;
const SWITCHER_TILE: f32 = 32.0;
const SWITCHER_TITLE: f32 = 13.0;
/// The row's horizontal insets: the list's, the item's, then the gap after the tile.
const SWITCHER_TEXT_LEFT: f32 = 6.0 + 8.0 + SWITCHER_TILE + 10.0;
const SWITCHER_TEXT_RIGHT: f32 = 8.0 + 6.0;

impl CommandPalette {
    /// One workspace: the sidebar's tile, its name with the matched letters, and its status or its branch; the
    /// first of a section carries the section's header above it.
    fn render_workspace_row(&self, row: usize, id_base: u64) -> Node {
        let colors = theme();
        let Some((found, entry)) = self.switcher.as_ref().and_then(|switcher| {
            let found = self.matches.get(row)?;
            Some((found, switcher.entries.get(found.candidate)?))
        }) else {
            return div().into();
        };
        let current = self
            .switcher
            .as_ref()
            .is_some_and(|switcher| switcher.current == entry.row.index);
        let current_pill = current.then(|| {
            div()
                .row()
                .h_px(18.0)
                .px(7.0)
                .items_center()
                .rounded(9.0)
                .bg(with_alpha(colors.text_accent, 0.14))
                .child(
                    label("current")
                        .size(10.5)
                        .medium()
                        .color(colors.text_accent),
                )
        });
        let pill_w = if current {
            ui::measure_text_width("current", 10.5, false, 500) + 14.0 + 10.0
        } else {
            0.0
        };
        let title = workspace::switcher_title(&entry.row);
        let title_w = WIDTH - SWITCHER_TEXT_LEFT - SWITCHER_TEXT_RIGHT - pill_w;
        let waiting = entry.row.agent == Some(workspace::AgentDot::AwaitingInput);
        let line = workspace::switcher_status(&entry.row).unwrap_or_else(|| {
            label(entry.row.branch.clone())
                .size(11.0)
                .mono()
                .color(colors.text_placeholder)
                .truncate()
                .into()
        });
        let text = div()
            .col()
            .flex(1.0)
            .gap(3.0)
            .child(div().row().h_px(16.0).items_center().child(fitted_label(
                &title,
                &found.positions,
                title_w,
            )))
            .child(
                div()
                    .row()
                    .h_px(if waiting { 20.0 } else { 14.0 })
                    .items_center()
                    .child(line),
            );
        let mut item = div()
            .row()
            .flex(1.0)
            .h_px(44.0)
            .px(8.0)
            .gap(10.0)
            .items_center()
            .rounded(6.0)
            .on_click(id_base + PaletteClick::Row(row).offset())
            .child(workspace::switcher_tile(&entry.row, current))
            .child(text)
            .children(current_pill.map(Node::from));
        if row == self.selected {
            item = item.bg(colors.overlay_selected);
        }
        let mut column = div().col().px(6.0).pb(1.0);
        if let Some(section) = self.header_of(row) {
            column = column.child(
                div()
                    .row()
                    .h_px(SWITCHER_HEADER_HEIGHT)
                    .pt(10.0)
                    .pb(4.0)
                    .px(8.0)
                    .child(
                        label(section.to_uppercase())
                            .size(10.5)
                            .weight(600)
                            .color(colors.text_placeholder),
                    ),
            );
        }
        column.child(item).into()
    }

    fn switcher_footer(&self, switcher: &workspace::WorkspaceSwitcher, id_base: u64) -> Node {
        let colors = theme();
        let action = |click: Option<PaletteClick>, keys: &str, text: &str| {
            let mut button = div()
                .row()
                .h_px(22.0)
                .px(4.0)
                .gap(6.0)
                .items_center()
                .rounded(4.0)
                .child(
                    div()
                        .row()
                        .h_px(18.0)
                        .px(5.0)
                        .items_center()
                        .rounded(4.0)
                        .bg(if colors.appearance == ui::Appearance::Light {
                            colors.element_active
                        } else {
                            colors.element_hover
                        })
                        .child(render_keystroke(keys, 11.0)),
                )
                .child(label(text).size(12.0).color(colors.text_placeholder));
            if let Some(click) = click {
                let id = id_base + click.offset();
                button = button.on_click(id);
                if self.hovered == Some(id) {
                    button = button.bg(colors.ghost_element_hover);
                }
            }
            button
        };
        let mut footer = div()
            .row()
            .justify_end()
            .items_center()
            .px(8.0)
            .py(6.0)
            .gap(6.0)
            .bg(colors.panel_background)
            .child(action(Some(PaletteClick::Run), "enter", "Switch"));
        if let Some(keys) = switcher.new_workspace.keys.first() {
            footer = footer.child(action(Some(PaletteClick::New), keys, "New workspace"));
        }
        footer.child(action(None, "escape", "Close")).into()
    }
}

fn with_alpha(color: ui::Rgba, alpha: f32) -> ui::Rgba {
    ui::Rgba::new(color.r, color.g, color.b, alpha)
}

/// [`highlighted_label`] cut with a trailing "..." to fit `width`, since the runs cannot shrink one by one.
fn fitted_label(text: &str, positions: &[usize], width: f32) -> Node {
    let size = SWITCHER_TITLE;
    let measure =
        |candidate: &str| ui::measure_text_width(candidate, size, false, ui::ui_font_weight());
    if measure(text) <= width {
        return highlighted_label(text, positions, size);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 0 {
        let head: String = chars[..keep].iter().collect();
        if measure(&format!("{}...", head.trim_end())) <= width {
            break;
        }
        keep -= 1;
    }
    let head: String = chars[..keep].iter().collect();
    let head = head.trim_end();
    let kept = head.chars().count();
    let positions: Vec<usize> = positions.iter().copied().filter(|p| *p < kept).collect();
    div()
        .row()
        .child(highlighted_label(head, &positions, size))
        .child(label("...").size(size).color(theme().text))
        .into()
}

/// A row's height in design px: a label line plus the item padding.
fn row_height() -> f32 {
    LabelSize::Default.px() * 1.4 + 10.0
}

/// The name with matched chars in the accent color.
fn highlighted_label(text: &str, positions: &[usize], size: f32) -> Node {
    let colors = theme();
    let mut row = div().row();
    let mut run = String::new();
    let mut run_matched = false;
    for (index, c) in text.chars().enumerate() {
        let matched = positions.binary_search(&index).is_ok();
        if matched != run_matched && !run.is_empty() {
            let color = if run_matched {
                colors.text_accent
            } else {
                colors.text
            };
            row = row.child(label(std::mem::take(&mut run)).size(size).color(color));
        }
        run_matched = matched;
        run.push(c);
    }
    if !run.is_empty() {
        let color = if run_matched {
            colors.text_accent
        } else {
            colors.text
        };
        row = row.child(label(run).size(size).color(color));
    }
    row.into()
}

/// A picker footer button: its label then the key binding that runs it, at 12px.
pub(crate) fn footer_button(
    id: u64,
    text: &str,
    keystroke: &str,
    color: ui::Rgba,
    hovered: bool,
) -> Node {
    let mut button = div()
        .row()
        .items_center()
        .h_px(22.0)
        .px(4.0)
        .gap(4.0)
        .rounded(4.0)
        .on_click(id)
        .child(label(text).label_size(LabelSize::Default).color(color))
        .child(render_keystroke(keystroke, 12.0));
    if hovered {
        button = button.bg(theme().ghost_element_hover);
    }
    button.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_action_names() {
        assert_eq!(
            humanize_action_name("editor::ConvertToUpperCase"),
            "editor: convert to upper case"
        );
        assert_eq!(
            humanize_action_name("go_to_line::Toggle"),
            "go to line: toggle"
        );
        assert_eq!(
            humanize_action_name("editor::ConvertToRot13"),
            "editor: convert to rot13"
        );
        assert_eq!(humanize_action_name("zed::OpenURL"), "zed: open URL");
        assert_eq!(
            humanize_action_name("editor::GoToHTMLFile"),
            "editor: go to HTML file"
        );
    }

    #[test]
    fn normalizes_queries() {
        assert_eq!(
            normalize_action_query("  editor::sort_lines "),
            "editor:sort lines"
        );
        assert_eq!(normalize_action_query("a   b"), "a b");
    }

    #[test]
    fn used_commands_lead_and_history_recalls_queries() {
        let mut memory = PaletteMemory::default();
        let mut palette = CommandPalette::new(&memory);
        palette.field.insert("upper case");
        palette.update_matches();
        let action = palette.confirm(&mut memory);
        assert_eq!(action, Some(Text(TextTransform::UpperCase)));

        let mut palette = CommandPalette::new(&memory);
        let first = palette.matches()[0].candidate;
        assert_eq!(
            palette.command(first).map(|c| c.name.as_str()),
            Some("editor: convert to upper case")
        );
        palette.select_previous(&mut memory);
        assert_eq!(palette.field.text(), "upper case");
        palette.select_next(&mut memory);
        assert_eq!(palette.field.text(), "");
    }

    #[test]
    fn selection_wraps_and_clamps_to_matches() {
        let mut memory = PaletteMemory::default();
        let mut palette = CommandPalette::new(&memory);
        let count = palette.matches().len();
        palette.select_previous(&mut memory);
        assert_eq!(palette.selected(), count - 1);
        palette.select_next(&mut memory);
        assert_eq!(palette.selected(), 0);
        palette.field.insert("zzzzqqq");
        palette.update_matches();
        assert!(palette.matches().is_empty());
        assert_eq!(palette.confirm(&mut memory), None);
    }

    #[test]
    fn typing_puts_the_selection_back_on_the_best_match() {
        let mut memory = PaletteMemory::default();
        let mut palette = CommandPalette::new(&memory);
        palette.field.insert("e");
        palette.update_matches();
        palette.select_next(&mut memory);
        palette.select_next(&mut memory);
        assert_eq!(palette.selected(), 2);
        palette.update_matches();
        assert_eq!(palette.selected(), 2, "the same query keeps the selection");
        palette.field.insert("d");
        palette.update_matches();
        assert_eq!(palette.selected(), 0);
    }

    fn switcher(labels: &[&str]) -> workspace::WorkspaceSwitcher {
        let entries = labels
            .iter()
            .enumerate()
            .map(|(index, label)| workspace::SwitcherEntry {
                row: workspace::WorkspaceRow {
                    index,
                    label: label.to_string(),
                    branch: format!("{}-branch", label.replace(' ', "-")),
                    agent: None,
                    ticket: String::new(),
                    ticket_category: String::new(),
                    running: 0,
                    pr: None,
                    missing: Vec::new(),
                },
                section: (index == 0).then_some("Main"),
                id: 500 + index as u64,
            })
            .collect();
        workspace::WorkspaceSwitcher {
            entries,
            current: 0,
            new_workspace: workspace::ExtraCommand {
                name: "New workspace".into(),
                keys: vec!["cmd-n".into()],
                id: 7,
            },
        }
    }

    #[test]
    fn a_long_workspace_name_is_cut_inside_the_picker() {
        let long =
            "Rate limit the public API and every internal endpoint behind the gateway with a \
                    per-tenant budget";
        let palette = CommandPalette::workspace_switcher(switcher(&[long, long]));
        let painted = ui::render(
            &palette.render(0),
            ui::Rect::new(0.0, 0.0, WIDTH, 600.0, ui::Rgba::TRANSPARENT),
        );
        let titles: Vec<&ui::Text> = painted
            .texts
            .iter()
            .filter(|text| {
                long.starts_with(text.text.trim_end_matches("...")) || text.text == "..."
            })
            .collect();
        assert!(
            titles.iter().any(|text| text.text == "..."),
            "the name is cut"
        );
        for text in painted.texts.iter() {
            let right = text.x
                + ui::measure_text_width(&text.text, text.size, text.font, text.weight)
                    * text.scale;
            assert!(right <= WIDTH, "{:?} ends at {right}", text.text);
        }
    }

    #[test]
    fn the_switcher_keeps_its_order_and_hands_back_the_picked_workspace() {
        let mut memory = PaletteMemory::default();
        let mut palette = CommandPalette::workspace_switcher(switcher(&["zeta", "alpha", "mid"]));
        let order: Vec<usize> = palette.matches().iter().map(|m| m.candidate).collect();
        assert_eq!(order, [0, 1, 2], "no alphabetical or usage sort");
        palette.select_next(&mut memory);
        assert_eq!(
            palette.confirm(&mut memory),
            Some(PaletteAction::External(501))
        );
        palette.field.insert("mid-branch");
        palette.update_matches();
        assert_eq!(
            palette.confirm(&mut memory),
            Some(PaletteAction::External(502))
        );
        assert_eq!(palette.new_workspace_id(), Some(7));
        assert!(
            memory.usage.is_empty(),
            "switching leaves the command ranking alone"
        );
    }
}
