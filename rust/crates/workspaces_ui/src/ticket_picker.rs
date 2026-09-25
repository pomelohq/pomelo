//! The Ticket field of the new-workspace form: a Jira key, typed or picked from the active sprint's tickets
//! (ranked mine-first, best match), with the board to take the sprint from.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use pom_jira::{Board, SprintIssue};
use ui::{div, icon, label, theme, IconKind, LabelSize, Node, Rgba};
use workspace::{status_line, InputField, WINDOW_MODAL_BASE};

use crate::TicketSource;

pub(crate) const TICKET_FIELD: u64 = WINDOW_MODAL_BASE + 7;
pub(crate) const BOARD: u64 = WINDOW_MODAL_BASE + 8;
pub(crate) const SUGGESTION_BASE: u64 = WINDOW_MODAL_BASE + 200;
const SUGGESTIONS_SHOWN: usize = 8;

enum Loaded {
    Boards(Result<Vec<Board>, String>),
    Sprint(Result<Vec<SprintIssue>, String>),
}

pub(crate) struct TicketPicker {
    pub field: InputField,
    source: TicketSource,
    boards: Vec<Board>,
    board: Option<i64>,
    issues: Vec<SprintIssue>,
    /// Keys that already have a workspace, never suggested again.
    taken: Vec<String>,
    loading: Option<Receiver<Loaded>>,
    error: Option<String>,
    highlighted: usize,
}

impl TicketPicker {
    pub fn new(source: TicketSource, existing_branches: &[String]) -> TicketPicker {
        let mut picker = TicketPicker {
            field: InputField::new("Ticket", "PROJ-101"),
            board: source.board,
            source,
            boards: Vec::new(),
            issues: Vec::new(),
            taken: existing_branches
                .iter()
                .filter_map(|branch| pom_jira::key_for_branch(branch))
                .collect(),
            loading: None,
            error: None,
            highlighted: 0,
        };
        let boards = picker.source.boards.clone();
        picker.load(move || Loaded::Boards(boards()));
        picker
    }

    fn load(&mut self, work: impl FnOnce() -> Loaded + Send + 'static) {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            if sender.send(work()).is_err() {
                eprintln!("workspaces: the form closed before Jira answered");
            }
        });
        self.loading = Some(receiver);
    }

    fn load_sprint(&mut self) {
        let Some(board) = self.board else {
            return;
        };
        let sprint = self.source.sprint.clone();
        self.load(move || Loaded::Sprint(sprint(board)));
    }

    /// Picks up what Jira sent; returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(receiver) = &self.loading else {
            return false;
        };
        let loaded = match receiver.try_recv() {
            Ok(loaded) => loaded,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.loading = None;
                return true;
            }
        };
        self.loading = None;
        match loaded {
            Loaded::Boards(Ok(boards)) => {
                if !self
                    .board
                    .is_some_and(|id| boards.iter().any(|board| board.id == id))
                {
                    self.board = boards.first().map(|board| board.id);
                }
                self.boards = boards;
                self.load_sprint();
            }
            Loaded::Sprint(Ok(issues)) => {
                self.issues = issues;
                self.error = None;
            }
            Loaded::Boards(Err(error)) | Loaded::Sprint(Err(error)) => self.error = Some(error),
        }
        true
    }

    pub fn busy(&self) -> bool {
        self.loading.is_some()
    }

    pub fn board(&self) -> Option<i64> {
        self.board
    }

    pub fn next_board(&mut self) {
        if self.boards.len() < 2 || self.loading.is_some() {
            return;
        }
        let at = self
            .boards
            .iter()
            .position(|board| Some(board.id) == self.board)
            .map_or(0, |at| (at + 1) % self.boards.len());
        self.board = Some(self.boards[at].id);
        self.issues.clear();
        self.load_sprint();
    }

    pub fn suggestions(&self) -> Vec<SprintIssue> {
        let mut ranked = pom_jira::rank_suggestions(
            &self.issues,
            &self.taken,
            &self.field.text(),
            self.source.only_mine,
        );
        ranked.truncate(SUGGESTIONS_SHOWN);
        ranked
    }

    pub fn typed(&mut self) {
        self.highlighted = 0;
    }

    pub fn move_highlight(&mut self, down: bool) {
        let count = self.suggestions().len();
        if count == 0 {
            return;
        }
        self.highlighted = if down {
            (self.highlighted + 1) % count
        } else {
            (self.highlighted + count - 1) % count
        };
    }

    pub fn highlighted(&self) -> Option<SprintIssue> {
        self.suggestions().into_iter().nth(self.highlighted)
    }

    pub fn suggestion(&self, index: usize) -> Option<SprintIssue> {
        self.suggestions().into_iter().nth(index)
    }

    /// The summary of the ticket typed in the field, when it is one of the sprint's.
    pub fn summary_of_typed(&self) -> Option<String> {
        let key = self.field.text().trim().to_uppercase();
        self.issues
            .iter()
            .find(|issue| issue.key == key)
            .map(|issue| issue.summary.clone())
    }

    pub fn render(&self, focused: bool) -> Node {
        let colors = theme();
        let mut header = div()
            .row()
            .items_center()
            .gap(6.0)
            .child(
                label(self.field.label)
                    .label_size(LabelSize::Small)
                    .color(colors.text),
            )
            .child(
                div().row().flex(1.0).items_center().child(
                    label("optional; pick from the sprint or type a key")
                        .label_size(LabelSize::Small)
                        .color(colors.text_muted)
                        .truncate(),
                ),
            );
        if let Some(board) = self
            .boards
            .iter()
            .find(|board| Some(board.id) == self.board)
        {
            header = header.child(board_chip(
                &board.name,
                self.boards.len() > 1 && self.loading.is_none(),
            ));
        }
        let mut column = div()
            .col()
            .gap(4.0)
            .child(header)
            .child(self.field.render_input(TICKET_FIELD, focused, false));
        if self.loading.is_some() {
            column = column.child(status_line(
                IconKind::RotateCw,
                colors.icon_muted,
                "Loading the sprint...",
            ));
        } else if let Some(error) = &self.error {
            column = column.child(status_line(
                IconKind::Warning,
                colors.warning,
                &format!("Couldn't load the sprint ({error}); type a ticket key instead"),
            ));
        }
        let suggestions = self.suggestions();
        if focused && !suggestions.is_empty() {
            let mut list = div()
                .col()
                .p(2.0)
                .rounded(6.0)
                .border(1.0, colors.border_variant)
                .bg(colors.editor_background);
            for (index, issue) in suggestions.iter().enumerate() {
                list = list.child(suggestion_row(
                    issue,
                    SUGGESTION_BASE + index as u64,
                    index == self.highlighted,
                ));
            }
            column = column.child(list);
        }
        column.into()
    }
}

/// The board the sprint comes from, cycled by clicking when there are several.
fn board_chip(name: &str, enabled: bool) -> Node {
    let colors = theme();
    let mut chip = div()
        .row()
        .h_px(20.0)
        .px(6.0)
        .gap(4.0)
        .items_center()
        .rounded(4.0)
        .child(
            label(name.to_string())
                .label_size(LabelSize::Small)
                .color(colors.text_muted),
        );
    if enabled {
        chip = chip.on_click(BOARD).child(
            icon(IconKind::ChevronUpDown)
                .size(11.0)
                .color(colors.icon_muted),
        );
    }
    chip.into()
}

fn suggestion_row(issue: &SprintIssue, id: u64, highlighted: bool) -> Node {
    let colors = theme();
    let mut row = div()
        .row()
        .items_center()
        .gap(8.0)
        .h_px(26.0)
        .px(6.0)
        .rounded(4.0)
        .on_click(id)
        .bg(if highlighted {
            colors.element_selected
        } else {
            Rgba::TRANSPARENT
        })
        .child(
            label(issue.key.clone())
                .label_size(LabelSize::Small)
                .mono()
                .color(colors.text),
        )
        .child(
            div().row().flex(1.0).items_center().child(
                label(issue.summary.clone())
                    .label_size(LabelSize::Small)
                    .color(colors.text)
                    .truncate(),
            ),
        );
    if issue.mine {
        row = row.child(icon(IconKind::Check).size(10.0).color(colors.text_accent));
    }
    row.child(
        label(issue.status.clone())
            .label_size(LabelSize::XSmall)
            .color(colors.text_muted),
    )
    .into()
}
