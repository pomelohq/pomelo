//! Cmd+Shift+P: a modal listing every editor command by name, fuzzy-filtered as you type, with its key
//! binding. Commands run from here float to the top next time, and Up on the first row recalls past queries.

use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};

use editor::transform::{LineTransform, TextTransform};
use ui::{div, icon, label, theme, IconKind, LabelSize, Node};
use workspace::EditKey;

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
}

/// Click targets inside the palette, as offsets from the view's palette id base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteClick {
    Row(usize),
    Run,
}

impl PaletteClick {
    const RUN: u64 = 0;

    pub fn offset(self) -> u64 {
        match self {
            PaletteClick::Run => Self::RUN,
            PaletteClick::Row(index) => index as u64 + 1,
        }
    }

    pub fn from_offset(offset: u64) -> Self {
        match offset {
            Self::RUN => PaletteClick::Run,
            n => PaletteClick::Row((n - 1) as usize),
        }
    }
}

use PaletteAction::{Key, Lines, Text};

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
    ("editor::Backtab", Key(EditKey::Outdent), &["shift-tab"]),
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
    binding: &'static [&'static str],
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
    selected: usize,
    scroll_top: usize,
    /// Scrolled distance not yet worth a whole row.
    scroll_remainder: f32,
    pub scrollbar: crate::list_scrollbar::ScrollbarReveal,
    /// The click id under the pointer.
    pub hovered: Option<u64>,
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
    pub fn new(memory: &PaletteMemory) -> Self {
        let mut commands: Vec<Command> = COMMANDS
            .iter()
            .map(|&(action_name, action, binding)| {
                let name = humanize_action_name(action_name);
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
        };
        palette.update_matches();
        palette
    }

    pub fn update_matches(&mut self) {
        let names: Vec<&str> = self.commands.iter().map(|c| c.name.as_str()).collect();
        let mut matches = fuzzy_match(&names, &normalize_action_query(&self.field.text()));
        let used_count = self.used_count;
        matches.sort_by_key(|m| {
            if m.candidate < used_count {
                m.candidate
            } else {
                usize::MAX
            }
        });
        self.matches = matches;
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
        let query = self.field.text();
        if !query.is_empty() {
            memory.history.add(query);
        }
        *memory.usage.entry(command.name.clone()).or_default() += 1;
        Some(command.action)
    }

    pub fn scroll_by(&mut self, dy: f32) -> bool {
        let max_top = self.matches.len().saturating_sub(Self::visible_rows());
        let moved = scroll_rows(
            &mut self.scroll_top,
            &mut self.scroll_remainder,
            dy,
            row_height() * ui::ui_text_scale(),
            max_top,
        );
        if moved {
            self.scrollbar.reveal();
        }
        moved
    }

    fn visible_rows() -> usize {
        ((MAX_RESULTS_HEIGHT - 8.0) / row_height()).floor().max(1.0) as usize
    }

    fn scroll_to_selected(&mut self) {
        let visible = Self::visible_rows();
        let before = self.scroll_top;
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        } else if self.selected >= self.scroll_top + visible {
            self.scroll_top = self.selected + 1 - visible;
        }
        let max_top = self.matches.len().saturating_sub(visible);
        self.scroll_top = self.scroll_top.min(max_top);
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
                PLACEHOLDER,
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
                        div().row().flex(1.0).px(6.0).py(4.0).child(
                            label("No matches")
                                .label_size(LabelSize::Default)
                                .color(colors.text_muted),
                        ),
                    ),
                )
                .into()
        } else {
            let visible = Self::visible_rows();
            let end = (self.scroll_top + visible).min(self.matches.len());
            let scrollbar = crate::list_scrollbar::render(
                WIDTH - 1.0,
                visible as f32 * row_height() + 8.0,
                self.scroll_top,
                visible,
                self.matches.len(),
                self.scrollbar.opacity(),
            );
            div()
                .col()
                .py(4.0)
                .children(scrollbar)
                .children((self.scroll_top..end).map(|row| self.render_row(row, id_base)))
                .into()
        };
        let footer = div()
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
            ));
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
        if self.hovered == Some(id_base + PaletteClick::Row(row).offset()) {
            item = item.bg(colors.ghost_element_hover);
        } else if row == self.selected {
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
                        .child(highlighted_label(&command.name, &found.positions))
                        .child(keys),
                ),
            )
            .into()
    }
}

/// A row's height in design px: a label line plus the item padding.
fn row_height() -> f32 {
    LabelSize::Default.px() * 1.4 + 10.0
}

/// The name with matched chars in the accent color.
fn highlighted_label(text: &str, positions: &[usize]) -> Node {
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
            row = row.child(
                label(std::mem::take(&mut run))
                    .label_size(LabelSize::Default)
                    .color(color),
            );
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
        row = row.child(label(run).label_size(LabelSize::Default).color(color));
    }
    row.into()
}

/// Split `cmd-shift-k` into its modifiers and key; the key may itself be `-` (`ctrl--`).
fn parse_keystroke(keystroke: &str) -> (Vec<&str>, &str) {
    let mut modifiers = Vec::new();
    let mut rest = keystroke;
    while let Some((head, tail)) = rest.split_once('-') {
        if tail.is_empty() || !matches!(head, "ctrl" | "alt" | "cmd" | "shift") {
            break;
        }
        modifiers.push(head);
        rest = tail;
    }
    (modifiers, rest)
}

fn key_icon(key: &str) -> Option<IconKind> {
    match key {
        "left" => Some(IconKind::KeyArrowLeft),
        "right" => Some(IconKind::KeyArrowRight),
        "up" => Some(IconKind::ArrowUp),
        "down" => Some(IconKind::ArrowDown),
        "backspace" | "delete" => Some(IconKind::Backspace),
        "enter" => Some(IconKind::Return),
        "tab" => Some(IconKind::Tab),
        _ => None,
    }
}

fn key_label(key: &str) -> String {
    match key {
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        key => {
            let mut chars = key.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

/// One keystroke as modifier glyphs (control, option, command, shift, in that order) then the key.
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

pub(crate) fn render_keystroke(keystroke: &str, size: f32) -> Node {
    let muted = theme().text_muted;
    let (modifiers, key) = parse_keystroke(keystroke);
    let glyph = |kind: IconKind| -> Node {
        div()
            .row()
            .items_center()
            .justify_center()
            .w_px(size)
            .h_px(size)
            .child(icon(kind).size(size).color(muted))
            .into()
    };
    let mut row = div().row().items_center();
    for (name, kind) in [
        ("ctrl", IconKind::Control),
        ("alt", IconKind::Option),
        ("cmd", IconKind::Command),
        ("shift", IconKind::Shift),
    ] {
        if modifiers.contains(&name) {
            row = row.child(glyph(kind));
        }
    }
    if let Some(kind) = key_icon(key) {
        return row.child(glyph(kind)).into();
    }
    let text = key_label(key);
    let key_box = div().row().items_center().h_px(size);
    let key_box = if text.chars().count() == 1 {
        key_box.w_px(size).justify_center()
    } else {
        key_box.px(2.0)
    };
    row.child(key_box.child(label(text).size(size).color(muted)))
        .into()
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
    fn parses_keystrokes() {
        assert_eq!(parse_keystroke("cmd-shift-k"), (vec!["cmd", "shift"], "k"));
        assert_eq!(parse_keystroke("ctrl--"), (vec!["ctrl"], "-"));
        assert_eq!(
            parse_keystroke("ctrl-shift--"),
            (vec!["ctrl", "shift"], "-")
        );
        assert_eq!(parse_keystroke("pageup"), (vec![], "pageup"));
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
}
