//! The Ticket field of the new-workspace form: a Jira key, typed or picked from a floating list of tickets
//! (ranked mine-first, best match), with the list they come from: a board's sprint or backlog, or every
//! open ticket assigned to the user.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use pom_jira::{Board, SprintIssue};
use ui::{deferred, div, icon, label, theme, IconKind, LabelSize, Node, Rgba};
use workspace::{status_line, InputField, WINDOW_MODAL_BASE};

use crate::{TicketList, TicketSource};

pub(crate) const TICKET_FIELD: u64 = WINDOW_MODAL_BASE + 7;
pub(crate) const SOURCE: u64 = WINDOW_MODAL_BASE + 8;
pub(crate) const SOURCE_MENU_SURFACE: u64 = WINDOW_MODAL_BASE + 13;
pub(crate) const SUGGESTIONS_SURFACE: u64 = WINDOW_MODAL_BASE + 14;
pub(crate) const SUGGESTION_BASE: u64 = WINDOW_MODAL_BASE + 200;
pub(crate) const SOURCE_OPTION_BASE: u64 = WINDOW_MODAL_BASE + 1000;
pub(crate) const SOURCE_OPTION_END: u64 = WINDOW_MODAL_BASE + 1100;
const MENU_WIDTH: f32 = 260.0;
const SUGGESTIONS_SHOWN: usize = 8;
const FIELD_HEIGHT: f32 = 32.0;
const KEY_W: f32 = 86.0;
const STATUS_W: f32 = 110.0;
const MINE_W: f32 = 40.0;

enum Loaded {
    Boards(Result<Vec<Board>, String>),
    Issues(Result<Vec<SprintIssue>, String>),
}

/// One row of the source menu.
struct SourceOption {
    list: TicketList,
    name: String,
    hint: String,
    /// Drawn after a divider, apart from the sprints above it.
    grouped_below: bool,
}

pub(crate) struct TicketPicker {
    pub field: InputField,
    source: TicketSource,
    boards: Vec<Board>,
    /// The board the sprint and backlog come from.
    board: Option<i64>,
    list: Option<TicketList>,
    issues: Vec<SprintIssue>,
    /// Keys that already have a workspace, never suggested again.
    taken: Vec<String>,
    loading: Option<Receiver<Loaded>>,
    error: Option<String>,
    highlighted: usize,
    menu_open: bool,
    /// The ticket list floats under the field while it is focused, until Escape or a pick.
    list_open: bool,
}

impl TicketPicker {
    pub fn new(source: TicketSource, existing_branches: &[String]) -> TicketPicker {
        let mut picker = TicketPicker {
            field: InputField::new("Ticket", "PROJ-101"),
            board: source.board,
            source,
            boards: Vec::new(),
            list: None,
            issues: Vec::new(),
            taken: existing_branches
                .iter()
                .filter_map(|branch| pom_jira::key_for_branch(branch))
                .collect(),
            loading: None,
            error: None,
            highlighted: 0,
            menu_open: false,
            list_open: true,
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

    fn load_issues(&mut self) {
        let Some(list) = self.list else {
            return;
        };
        let issues = self.source.issues.clone();
        self.load(move || Loaded::Issues(issues(list)));
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
            Loaded::Boards(result) => {
                let boards = match result {
                    Ok(boards) => boards,
                    Err(error) => {
                        self.error = Some(error);
                        Vec::new()
                    }
                };
                if !self
                    .board
                    .is_some_and(|id| boards.iter().any(|board| board.id == id))
                {
                    self.board = boards.first().map(|board| board.id);
                }
                self.boards = boards;
                self.list = Some(match self.board {
                    Some(board) => TicketList::Sprint(board),
                    None => TicketList::Assigned,
                });
                self.load_issues();
            }
            Loaded::Issues(Ok(issues)) => {
                self.issues = issues;
                self.error = None;
            }
            Loaded::Issues(Err(error)) => self.error = Some(error),
        }
        true
    }

    pub fn busy(&self) -> bool {
        self.loading.is_some()
    }

    pub fn board(&self) -> Option<i64> {
        self.board
    }

    pub fn toggle_menu(&mut self) {
        self.menu_open = !self.menu_open && self.list.is_some() && self.loading.is_none();
    }

    pub fn close_menu(&mut self) {
        self.menu_open = false;
    }

    pub fn menu_open(&self) -> bool {
        self.menu_open
    }

    pub fn list_open(&self) -> bool {
        self.list_open
    }

    pub fn open_list(&mut self) {
        self.list_open = true;
    }

    pub fn close_list(&mut self) {
        self.list_open = false;
    }

    fn board_name(&self, id: i64) -> String {
        self.boards
            .iter()
            .find(|board| board.id == id)
            .map(|board| board.name.clone())
            .unwrap_or_default()
    }

    fn options(&self) -> Vec<SourceOption> {
        let mut options: Vec<SourceOption> = self
            .boards
            .iter()
            .map(|board| SourceOption {
                list: TicketList::Sprint(board.id),
                name: board.name.clone(),
                hint: "current sprint".into(),
                grouped_below: false,
            })
            .collect();
        let mut below = Vec::new();
        if let Some(board) = self.board {
            below.push(SourceOption {
                list: TicketList::Backlog(board),
                name: "Backlog".into(),
                hint: self.board_name(board),
                grouped_below: false,
            });
        }
        below.push(SourceOption {
            list: TicketList::Assigned,
            name: "Assigned to me".into(),
            hint: "any board".into(),
            grouped_below: false,
        });
        below[0].grouped_below = !options.is_empty();
        options.extend(below);
        options
    }

    pub fn pick_source(&mut self, index: usize) {
        self.menu_open = false;
        let Some(option) = self.options().into_iter().nth(index) else {
            return;
        };
        self.list_open = true;
        if Some(option.list) == self.list || self.loading.is_some() {
            return;
        }
        if let TicketList::Sprint(board) = option.list {
            self.board = Some(board);
        }
        self.list = Some(option.list);
        self.issues.clear();
        self.highlighted = 0;
        self.load_issues();
    }

    /// What the source button reads: the board of the sprint, Backlog, or Assigned to me.
    fn source_name(&self) -> Option<String> {
        Some(match self.list? {
            TicketList::Sprint(board) => self.board_name(board),
            TicketList::Backlog(_) => "Backlog".into(),
            TicketList::Assigned => "Assigned to me".into(),
        })
    }

    pub fn suggestions(&self) -> Vec<SprintIssue> {
        let only_mine = self.source.only_mine && self.list != Some(TicketList::Assigned);
        let mut ranked =
            pom_jira::rank_suggestions(&self.issues, &self.taken, &self.field.text(), only_mine);
        ranked.truncate(SUGGESTIONS_SHOWN);
        ranked
    }

    pub fn typed(&mut self) {
        self.highlighted = 0;
        self.list_open = true;
    }

    pub fn move_highlight(&mut self, down: bool) {
        self.list_open = true;
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

    /// The ticket Enter picks, while the list shows.
    pub fn highlighted(&self) -> Option<SprintIssue> {
        if !self.list_open {
            return None;
        }
        self.suggestions().into_iter().nth(self.highlighted)
    }

    pub fn suggestion(&self, index: usize) -> Option<SprintIssue> {
        self.suggestions().into_iter().nth(index)
    }

    /// The summary of the ticket typed in the field, when it is one of the listed ones.
    pub fn summary_of_typed(&self) -> Option<String> {
        let key = self.field.text().trim().to_uppercase();
        self.issues
            .iter()
            .find(|issue| issue.key == key)
            .map(|issue| issue.summary.clone())
    }

    /// The field, with the ticket list floating `width` wide under it while `focused`.
    pub fn render(&self, focused: bool, width: f32) -> Node {
        let colors = theme();
        let mut header = div().row().items_center().gap(6.0).child(
            label(self.field.label)
                .label_size(LabelSize::Small)
                .color(colors.text),
        );
        if let Some(name) = self.source_name() {
            let mut anchor = div().col().child(self.source_button(&name));
            if self.menu_open {
                anchor = anchor.child(self.source_menu());
            }
            header = header.child(anchor);
        }
        header = header.child(
            div().row().flex(1.0).items_center().child(
                label("optional; pick one or type a key")
                    .label_size(LabelSize::Small)
                    .color(colors.text_muted)
                    .truncate(),
            ),
        );
        let mut anchor = div()
            .col()
            .child(self.field.render_input(TICKET_FIELD, focused, false));
        if focused && self.list_open && !self.menu_open {
            if let Some(list) = self.suggestion_list(width) {
                anchor = anchor.child(
                    deferred(list)
                        .below_or_above(FIELD_HEIGHT, 4.0)
                        .snap_to_window()
                        .priority(1),
                );
            }
        }
        let mut column = div().col().gap(4.0).child(header).child(anchor);
        let source = self.source_name().unwrap_or_else(|| "tickets".into());
        if self.loading.is_some() {
            column = column.child(status_line(
                IconKind::RotateCw,
                colors.icon_muted,
                &format!("Loading {source}..."),
            ));
        } else if let Some(error) = &self.error {
            column = column.child(status_line(
                IconKind::Warning,
                colors.warning,
                &format!("Couldn't load {source} ({error}); type a ticket key instead"),
            ));
        }
        column.into()
    }

    fn suggestion_list(&self, width: f32) -> Option<Node> {
        let colors = theme();
        let suggestions = self.suggestions();
        let typed = self.field.text().trim().to_string();
        if suggestions.is_empty() && (typed.is_empty() || self.loading.is_some()) {
            return None;
        }
        let mut list = div()
            .col()
            .w_px(width)
            .p(4.0)
            .rounded(8.0)
            .border(1.0, colors.border)
            .bg(colors.elevated_surface_background)
            .on_click(SUGGESTIONS_SURFACE);
        if suggestions.is_empty() {
            let note = format!("No ticket matches; {typed} is used as the key");
            return Some(
                list.child(
                    div().row().h_px(28.0).px(8.0).items_center().child(
                        label(note)
                            .label_size(LabelSize::Small)
                            .color(colors.text_muted)
                            .truncate(),
                    ),
                )
                .into(),
            );
        }
        for (index, issue) in suggestions.iter().enumerate() {
            list = list.child(suggestion_row(
                issue,
                SUGGESTION_BASE + index as u64,
                index == self.highlighted,
            ));
        }
        Some(list.into())
    }

    fn source_button(&self, name: &str) -> Node {
        let colors = theme();
        let mut button =
            div()
                .row()
                .h_px(20.0)
                .px(6.0)
                .gap(4.0)
                .items_center()
                .rounded(4.0)
                .child(label(name.to_string()).label_size(LabelSize::Small).color(
                    if self.menu_open {
                        colors.text
                    } else {
                        colors.text_muted
                    },
                ))
                .child(
                    icon(IconKind::ChevronDown)
                        .size(10.0)
                        .color(colors.icon_muted),
                );
        if self.menu_open {
            button = button.bg(colors.ghost_element_hover);
        }
        if self.loading.is_none() {
            button = button.on_click(SOURCE);
        }
        button.into()
    }

    /// Every board's sprint, then the backlog and the user's own tickets; the current one is accented.
    fn source_menu(&self) -> Node {
        let colors = theme();
        let mut menu = div()
            .col()
            .w_px(MENU_WIDTH)
            .p(4.0)
            .rounded(8.0)
            .border(1.0, colors.border)
            .bg(colors.elevated_surface_background)
            .on_click(SOURCE_MENU_SURFACE);
        for (index, option) in self.options().into_iter().enumerate() {
            if option.grouped_below {
                menu = menu.child(
                    div()
                        .col()
                        .px(2.0)
                        .py(4.0)
                        .child(div().h_px(1.0).bg(colors.border_variant)),
                );
            }
            let current = Some(option.list) == self.list;
            menu = menu.child(
                div()
                    .row()
                    .h_px(26.0)
                    .px(8.0)
                    .gap(8.0)
                    .items_center()
                    .rounded(4.0)
                    .on_click(SOURCE_OPTION_BASE + index as u64)
                    .child(
                        div().row().flex(1.0).items_center().child(
                            label(option.name)
                                .label_size(LabelSize::Default)
                                .color(if current {
                                    colors.text_accent
                                } else {
                                    colors.text
                                })
                                .truncate(),
                        ),
                    )
                    .child(
                        label(option.hint)
                            .label_size(LabelSize::XSmall)
                            .color(colors.text_placeholder),
                    ),
            );
        }
        deferred(menu)
            .below_or_above(20.0, 4.0)
            .snap_to_window()
            .priority(2)
            .into()
    }
}

fn status_pill(issue: &SprintIssue) -> Node {
    let colors = theme();
    let (text, border) = match issue.category.as_str() {
        "indeterminate" => (colors.text_accent, colors.border_selected),
        "done" => (colors.success, colors.border_variant),
        _ => (colors.text_muted, colors.border_variant),
    };
    div()
        .row()
        .w_px(STATUS_W)
        .items_center()
        .child(
            div()
                .row()
                .h_px(18.0)
                .px(7.0)
                .items_center()
                .rounded(9.0)
                .border(1.0, border)
                .child(
                    label(issue.status.clone())
                        .label_size(LabelSize::XSmall)
                        .color(text)
                        .truncate(),
                ),
        )
        .into()
}

fn suggestion_row(issue: &SprintIssue, id: u64, highlighted: bool) -> Node {
    let colors = theme();
    div()
        .row()
        .items_center()
        .gap(10.0)
        .h_px(28.0)
        .px(8.0)
        .rounded(5.0)
        .on_click(id)
        .bg(if highlighted {
            colors.element_selected
        } else {
            Rgba::TRANSPARENT
        })
        .child(
            div().row().w_px(KEY_W).items_center().child(
                label(issue.key.clone())
                    .label_size(LabelSize::Small)
                    .mono()
                    .color(colors.text_accent)
                    .truncate(),
            ),
        )
        .child(
            div().row().flex(1.0).items_center().child(
                label(issue.summary.clone())
                    .label_size(LabelSize::Small)
                    .color(colors.text)
                    .truncate(),
            ),
        )
        .child(status_pill(issue))
        .child(
            div()
                .row()
                .w_px(MINE_W)
                .items_center()
                .child(if issue.mine {
                    label("Mine")
                        .label_size(LabelSize::XSmall)
                        .color(colors.text_placeholder)
                        .into()
                } else {
                    Node::from(div())
                }),
        )
        .into()
}
