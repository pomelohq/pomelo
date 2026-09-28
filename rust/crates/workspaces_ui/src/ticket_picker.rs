//! The Ticket field of the new-workspace form: a Jira key, typed or picked from the active sprint's tickets
//! (ranked mine-first, best match), with the board to take the sprint from.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use pom_jira::{Board, SprintIssue};
use ui::{deferred, div, icon, label, theme, IconKind, LabelSize, Node, Rgba};
use workspace::{status_line, InputField, WINDOW_MODAL_BASE};

use crate::TicketSource;

pub(crate) const TICKET_FIELD: u64 = WINDOW_MODAL_BASE + 7;
pub(crate) const BOARD: u64 = WINDOW_MODAL_BASE + 8;
pub(crate) const BOARD_MENU_SURFACE: u64 = WINDOW_MODAL_BASE + 13;
pub(crate) const SUGGESTION_BASE: u64 = WINDOW_MODAL_BASE + 200;
pub(crate) const BOARD_OPTION_BASE: u64 = WINDOW_MODAL_BASE + 1000;
pub(crate) const BOARD_OPTION_END: u64 = WINDOW_MODAL_BASE + 1100;
const MENU_WIDTH: f32 = 240.0;
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
    board_menu_open: bool,
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
            board_menu_open: false,
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

    pub fn toggle_board_menu(&mut self) {
        self.board_menu_open = !self.board_menu_open && self.can_switch_board();
    }

    pub fn close_board_menu(&mut self) {
        self.board_menu_open = false;
    }

    pub fn board_menu_open(&self) -> bool {
        self.board_menu_open
    }

    fn can_switch_board(&self) -> bool {
        self.boards.len() > 1 && self.loading.is_none()
    }

    pub fn pick_board(&mut self, index: usize) {
        self.board_menu_open = false;
        let Some(board) = self.boards.get(index) else {
            return;
        };
        if Some(board.id) == self.board || self.loading.is_some() {
            return;
        }
        self.board = Some(board.id);
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
            let mut chip = div()
                .col()
                .child(board_chip(&board.name, self.can_switch_board()));
            if self.board_menu_open {
                chip = chip.child(self.board_menu());
            }
            header = header.child(chip);
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

impl TicketPicker {
    /// The boards to take the sprint from, dropped under the chip; the current one is checked.
    fn board_menu(&self) -> Node {
        let colors = theme();
        let mut list = div()
            .col()
            .w_px(MENU_WIDTH)
            .p(4.0)
            .rounded(8.0)
            .border(1.0, colors.border)
            .bg(colors.elevated_surface_background)
            .on_click(BOARD_MENU_SURFACE);
        for (index, board) in self.boards.iter().enumerate() {
            let current = Some(board.id) == self.board;
            let mut row = div()
                .row()
                .h_px(26.0)
                .px(8.0)
                .gap(6.0)
                .items_center()
                .rounded(4.0)
                .on_click(BOARD_OPTION_BASE + index as u64);
            row = row.child(div().row().w_px(14.0).items_center().child(if current {
                icon(IconKind::Check).size(14.0).color(colors.text).into()
            } else {
                Node::from(div())
            }));
            list = list.child(
                row.child(
                    div().row().flex(1.0).items_center().child(
                        label(board.name.clone())
                            .label_size(LabelSize::Default)
                            .color(colors.text)
                            .truncate(),
                    ),
                ),
            );
        }
        deferred(list)
            .below_or_above(20.0, 4.0)
            .snap_to_window()
            .priority(1)
            .into()
    }
}

/// The board the sprint comes from; clicking opens the list of boards when there are several.
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
