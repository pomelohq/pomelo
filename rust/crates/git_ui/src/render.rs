use git::working_copy::{Staging, Upstream};
use git::ChangeStatus;
use ui::{deferred, div, icon, label, theme, Corner, Div, IconKind, Node, Rgba};

use crate::commit_area::{self, RemoteKind};
use crate::rows::{self, Row};
use crate::scan::{RepoScan, Scan};
use crate::widgets::{
    arrow_count, avatar, checkbox, chevron, chip, chip_icon, chip_text, diff_stat, ghost_button,
    icon_button, status_icon, text_width, tooltip, with_guides, ChipTone, SplitButton, INDENT,
    ROW_H, TAB_BAR_H, TOOLBAR_H,
};
use crate::{
    Control, GitPanel, Scope, Tab, BACK, BRANCH_LINK, BRANCH_MENU, COMMIT, COMMIT_EDITOR,
    COMMIT_MENU, DRIFT_CHIP, FETCH_ALL, LAST_COMMIT, OPEN_ALL_DIFFS, PLAN, STAGE_ALL, STAGE_MENU,
    SYNC, SYNC_MENU, TAB_CHANGES, TAB_HISTORY, TAB_REMOTE, UNCOMMIT, VIEW_DIFF, VIEW_OPTIONS,
};

const BRANCH_ROW_H: f32 = 30.0;
const COMMIT_ROW_H: f32 = 34.0;
const LAST_COMMIT_H: f32 = 30.0;
const CARD_INSET: f32 = 6.0;

fn fill(row: Div) -> Div {
    row.child(div().row().flex(1.0))
}

fn grow(node: impl Into<Node>) -> Div {
    div().row().flex(1.0).items_center().child(node)
}

fn line(color: Rgba) -> Div {
    div().h_px(1.0).bg(color)
}

impl GitPanel {
    fn hovered_row(&self) -> Option<usize> {
        self.hover
            .and_then(|id| self.decode(id))
            .map(|(row, _)| row)
    }

    fn row_base(&self, index: usize, height: f32) -> Div {
        let mut row = div()
            .row()
            .h_px(height)
            .pl(10.0)
            .pr(4.0)
            .gap(6.0)
            .items_center()
            .on_click(self.id(index, Control::Main));
        if self.hovered_row() == Some(index) {
            row = row.bg(theme().ghost_element_hover);
        }
        self.mark_keyboard_row(index, row)
    }

    fn mark_keyboard_row(&self, index: usize, row: Div) -> Div {
        if self.keyboard_row() == Some(index) {
            row.bg(theme().element_selected)
                .border(1.0, theme().panel_focused_border)
        } else {
            row
        }
    }

    fn check(&self, index: usize, staging: Staging, tip: &str) -> Node {
        let id = self.id(index, Control::Check);
        let boxed = div().child(checkbox(id, staging, self.hot(id)));
        if self.hot(id) {
            let tip =
                workspace::keymap::with_key_hint(tip, workspace::keymap::Action::GitToggleStaged);
            tooltip(boxed, &tip, false).into()
        } else {
            boxed.into()
        }
    }

    fn on_branch(&self, scan: &Scan, repo: usize) -> Option<Node> {
        let (branch, kept) = self.other_branch(scan, repo)?;
        let color = if kept {
            theme().text_muted
        } else {
            theme().warning
        };
        Some(
            label(format!("on {branch}"))
                .size(11.0)
                .mono()
                .color(color)
                .truncate()
                .into(),
        )
    }

    pub(crate) fn render_panel(&mut self, width: f32, height: f32) -> Node {
        let scan = self.current();
        self.rows = self.build_rows(&scan);
        self.settle_selection();
        let (footer, footer_h) = self.render_footer(width, &scan);
        let list_h = (height - TAB_BAR_H - 1.0 - TOOLBAR_H - footer_h).max(0.0);
        self.viewport_h = list_h;
        let max_scroll = (rows::content_height(&self.rows) - list_h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let mut top = 0.0;
        let mut first_top = None;
        let mut shown_h = 0.0;
        let mut shown = div().col().w_px(width);
        for (index, row) in self.rows.iter().enumerate() {
            let row_h = row.height();
            if top + row_h <= self.scroll {
                top += row_h;
                continue;
            }
            if top >= self.scroll + list_h {
                break;
            }
            first_top.get_or_insert(top);
            shown = shown.child(self.render_row(index, row, &scan, width));
            shown_h += row_h;
            top += row_h;
        }
        let offset = first_top.unwrap_or(0.0) - self.scroll;
        // Rows scroll by the pixel, so the first and last can be cut; the overlay's clip cuts them.
        let viewport = div().col().w_px(width).h_px(list_h).pin(
            Corner::TopLeft,
            0.0,
            offset,
            width,
            shown_h,
            shown,
        );
        let list = div().col().w_px(width).h_px(list_h).pin(
            Corner::TopLeft,
            0.0,
            0.0,
            width,
            list_h,
            deferred(viewport).clip(),
        );
        div()
            .col()
            .w_px(width)
            .h_px(height)
            .child(self.render_tab_bar(width, &scan))
            .child(line(theme().border))
            .child(self.render_toolbar(width, &scan))
            .child(list)
            .child(footer)
            .into()
    }

    fn render_tab_bar(&self, width: f32, scan: &Scan) -> Node {
        let colors = theme();
        let changes: usize = scan.repos.iter().map(|repo| repo.status.len()).sum();
        let differ = scan
            .repos
            .iter()
            .filter(|repo| repo.differs_from_origin())
            .count();
        let failing = self.pull_requests.as_ref().is_some_and(|prs| {
            self.sources.iter().any(|source| {
                prs.for_checkout(&source.root)
                    .and_then(|(_, pr)| pr)
                    .is_some_and(|pr| pr.checks == "fail")
            })
        });
        let tab = |id: u64, tab: Tab, title: &str, count: usize, dot: bool| -> Node {
            let active = self.tab == tab;
            let mut cell = div()
                .row()
                .flex(1.0)
                .h_px(TAB_BAR_H)
                .gap(4.0)
                .items_center()
                .justify_center()
                .on_click(self.fixed(id));
            cell = if self.hot(self.fixed(id)) && !active {
                cell.bg(colors.element_hover)
            } else if active {
                cell.bg(colors.panel_background)
            } else {
                cell.bg(colors.editor_background.alpha(0.6))
            };
            cell = cell.child(label(title.to_string()).size(13.0).color(if active {
                colors.text
            } else {
                colors.text_muted
            }));
            if count > 0 {
                cell = cell.child(
                    label(format!("({count})"))
                        .size(12.0)
                        .color(colors.text_placeholder),
                );
            }
            if dot {
                cell = cell.child(div().w_px(6.0).h_px(6.0).rounded(3.0).bg(colors.error));
            }
            cell.into()
        };
        let divider = || -> Node {
            div()
                .w_px(1.0)
                .h_px(TAB_BAR_H)
                .bg(colors.border_variant)
                .into()
        };
        div()
            .row()
            .w_px(width)
            .h_px(TAB_BAR_H)
            .child(tab(TAB_CHANGES, Tab::Changes, "Changes", changes, false))
            .child(divider())
            .child(tab(TAB_REMOTE, Tab::Remote, "Remote", differ, failing))
            .child(divider())
            .child(tab(TAB_HISTORY, Tab::History, "History", 0, false))
            .into()
    }

    fn view_options(&self) -> Node {
        let id = self.fixed(VIEW_OPTIONS);
        let button = icon_button(id, IconKind::Filter, self.hot(id), true);
        if self.hot(id) {
            tooltip(button, "View Options", false).into()
        } else {
            button.into()
        }
    }

    fn render_toolbar(&self, width: f32, scan: &Scan) -> Node {
        let colors = theme();
        let bar = div()
            .row()
            .w_px(width)
            .h_px(TOOLBAR_H)
            .pl(4.0)
            .pr(8.0)
            .gap(4.0)
            .items_center();
        if let Some(opened) = &self.opened {
            let back = self.fixed(BACK);
            let repo = self
                .sources
                .get(opened.repo)
                .map(|source| source.name.clone())
                .unwrap_or_default();
            let back_label = match self.tab {
                Tab::Changes => "Changes",
                Tab::Remote => "Remote",
                Tab::History => "History",
            };
            return bar
                .child(ghost_button(
                    back,
                    Some(IconKind::ChevronLeft),
                    back_label,
                    self.hot(back),
                ))
                .child(grow(
                    label(format!("{} - {repo}", opened.commit.short_sha))
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                ))
                .into();
        }
        match self.tab {
            Tab::Changes => {
                let (added, deleted) = scan
                    .repos
                    .iter()
                    .flat_map(|repo| repo.uncommitted.iter())
                    .fold((0, 0), |(added, deleted), file| {
                        (
                            added + file.added.unwrap_or(0),
                            deleted + file.deleted.unwrap_or(0),
                        )
                    });
                let view_diff = self.fixed(VIEW_DIFF);
                let mut diff_button = ghost_button(
                    view_diff,
                    Some(IconKind::DiffUnified),
                    "View Diff",
                    self.hot(view_diff),
                );
                if added + deleted > 0 {
                    diff_button = diff_button.child(diff_stat(Some(added), Some(deleted), 12.0));
                }
                if self.hot(view_diff) {
                    diff_button =
                        tooltip(diff_button, "Every uncommitted change in one tab", false);
                }
                let everything = Self::everything_staged(scan);
                let split = SplitButton {
                    main: self.fixed(STAGE_ALL),
                    menu: self.fixed(STAGE_MENU),
                    text: if everything {
                        "Unstage All"
                    } else {
                        "Stage All"
                    },
                    note: None,
                    enabled: scan.repos.iter().any(|repo| !repo.status.is_empty()),
                    hover: self.hover,
                };
                let main = split.main_half();
                fill(bar.child(diff_button))
                    .child(self.view_options())
                    .child(split.finish(main))
                    .into()
            }
            Tab::Remote => {
                let fetch = self.fixed(FETCH_ALL);
                let fetching = self.commit.remote_running() == Some(RemoteKind::Fetch);
                let text = if fetching { "Fetching..." } else { "Fetch All" };
                let mut button = ghost_button(
                    fetch,
                    Some(IconKind::RotateCw),
                    text,
                    self.hot(fetch) || fetching,
                );
                if self.hot(fetch) {
                    let tip = workspace::keymap::with_key_hint(
                        "git fetch in every repo",
                        workspace::keymap::Action::GitFetch,
                    );
                    button = tooltip(button, &tip, false);
                }
                fill(bar.child(button)).child(self.view_options()).into()
            }
            Tab::History => {
                let count: usize = scan.repos.iter().map(|repo| repo.history.len()).sum();
                let base = self
                    .sources
                    .first()
                    .map(|source| source.default_branch.clone())
                    .unwrap_or_else(|| "main".into());
                let plural = if count == 1 { "" } else { "s" };
                bar.child(
                    grow(
                        label(format!(
                            "{count} commit{plural} since the branch left {base}"
                        ))
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                    )
                    .pl(6.0),
                )
                .child(self.view_options())
                .into()
            }
        }
    }

    fn render_row(&self, index: usize, row: &Row, scan: &Scan, width: f32) -> Node {
        let Some(repo) = row.card() else {
            return self.row_content(index, row, scan, width).w_px(width).into();
        };
        let inner_w = width - 2.0 * CARD_INSET;
        let edge = theme().border_variant;
        let first = matches!(row, Row::Card { .. });
        let last = self.rows.get(index + 1).and_then(Row::card) != Some(repo);
        let height = row.height();
        let mut inner = self
            .row_content(index, row, scan, inner_w)
            .w_px(inner_w)
            .pin_left_edge(0.0, 0.0, 1.0, div().bg(edge))
            .pin(Corner::TopRight, 0.0, 0.0, 1.0, height, div().bg(edge));
        if first {
            inner = inner.pin(Corner::TopLeft, 0.0, 0.0, inner_w, 1.0, div().bg(edge));
        }
        if last {
            inner = inner.pin(Corner::BottomLeft, 0.0, 0.0, inner_w, 1.0, div().bg(edge));
        }
        div()
            .row()
            .w_px(width)
            .h_px(height)
            .px(CARD_INSET)
            .child(inner)
            .into()
    }

    fn row_content(&self, index: usize, row: &Row, scan: &Scan, width: f32) -> Div {
        let colors = theme();
        match row {
            Row::Message { text, indent } => div()
                .row()
                .h_px(ROW_H)
                .pl(*indent)
                .pr(8.0)
                .items_center()
                .child(grow(
                    label(text.clone())
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                )),
            Row::Gap => div().h_px(rows::GAP_H),
            Row::Day(day) => div().row().h_px(rows::DAY_H).pl(10.0).items_center().child(
                label(day.to_string())
                    .size(11.0)
                    .color(colors.text_placeholder),
            ),
            Row::RepoHeader { repo, history } => self.repo_header(index, *repo, *history, scan),
            Row::Section {
                repo,
                staged,
                count,
            } => {
                let name = self.source_name_of(*repo);
                let open = !self.is_folded(&format!("s:{name}:{staged}"));
                let mut header = self
                    .row_base(index, ROW_H)
                    .child(chevron(open))
                    .child(grow(
                        label(if *staged { "Staged" } else { "Not staged" })
                            .size(12.0)
                            .color(colors.text_muted),
                    ))
                    .child(
                        label(count.to_string())
                            .size(11.0)
                            .color(colors.text_placeholder),
                    );
                if *count > 0 {
                    let (staging, tip) = if *staged {
                        (Staging::Staged, "Unstage everything in this repo")
                    } else {
                        (Staging::Unstaged, "Stage everything in this repo")
                    };
                    header = header.child(self.check(index, staging, tip));
                }
                header
            }
            Row::Directory {
                repo,
                scope,
                path,
                label: text,
                depth,
                staging,
                left_to_review,
            } => {
                let name = self.source_name_of(*repo);
                let open = !self.is_folded(&format!("d:{}:{name}:{path}", scope.key()));
                let mut row = with_guides(self.row_base(index, ROW_H), *depth)
                    .pl(10.0 + *depth as f32 * INDENT)
                    .child(chevron(open))
                    .child(
                        icon(if open {
                            IconKind::FolderOpen
                        } else {
                            IconKind::Folder
                        })
                        .size(14.0)
                        .color(colors.icon_muted),
                    )
                    .child(grow(
                        label(text.clone()).color(colors.text_muted).truncate(),
                    ));
                if let Some(left) = left_to_review {
                    let text = if *left == 0 {
                        "reviewed".to_string()
                    } else {
                        format!("{left} to review")
                    };
                    row = row.child(label(text).size(11.0).color(colors.text_placeholder));
                }
                if let Some(staging) = staging {
                    let tip = match (scope, staging) {
                        (Scope::Staged, _) | (_, Staging::Staged) => "Unstage folder",
                        _ => "Stage folder",
                    };
                    row = row.child(self.check(index, *staging, tip));
                }
                row
            }
            Row::File {
                repo,
                scope,
                entry,
                depth,
                flat,
            } => self.file_row(index, *repo, *scope, entry, *depth, *flat, scan),
            Row::Progress { reviewed, total } => {
                let open = self.fixed(OPEN_ALL_DIFFS);
                let fraction = if *total == 0 {
                    0.0
                } else {
                    *reviewed as f32 / *total as f32
                };
                let bar_w = (width - 20.0).max(0.0);
                div()
                    .col()
                    .h_px(rows::PROGRESS_H)
                    .px(10.0)
                    .pt(6.0)
                    .gap(4.0)
                    .child(
                        div()
                            .row()
                            .h_px(22.0)
                            .items_center()
                            .child(grow(
                                label(format!("{reviewed} of {total} files reviewed"))
                                    .size(12.0)
                                    .color(colors.text_muted)
                                    .truncate(),
                            ))
                            .child(ghost_button(open, None, "Open all diffs", self.hot(open))),
                    )
                    .child(
                        div()
                            .row()
                            .w_px(bar_w)
                            .h_px(3.0)
                            .rounded(2.0)
                            .bg(colors.border_variant)
                            .child(
                                div()
                                    .w_px(bar_w * fraction)
                                    .h_px(3.0)
                                    .rounded(2.0)
                                    .bg(colors.success),
                            ),
                    )
            }
            Row::Card { repo } => self.card_header(index, *repo, scan, width),
            Row::PullRequest { repo, pull_request } => {
                self.pull_request_block(index, *repo, pull_request)
            }
            Row::CreatePullRequest { .. } => self
                .row_base(index, ROW_H)
                .pl(26.0)
                .child(icon(IconKind::Plus).size(13.0).color(colors.text_accent))
                .child(grow(
                    label("Create Pull Request")
                        .size(12.0)
                        .color(colors.text_accent)
                        .truncate(),
                )),
            Row::CardNote { repo, text } => {
                let unpublished = scan
                    .repos
                    .get(*repo)
                    .is_some_and(|state| state.outgoing.is_none());
                let mut note = div()
                    .row()
                    .h_px(ROW_H)
                    .pl(26.0)
                    .pr(8.0)
                    .gap(6.0)
                    .items_center();
                if unpublished {
                    note = note.child(
                        icon(IconKind::Clock)
                            .size(13.0)
                            .color(colors.text_placeholder),
                    );
                }
                note.child(grow(
                    label(text.clone())
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                ))
            }
            Row::CommitGroup {
                repo,
                incoming,
                count,
                published,
            } => {
                let name = self.source_name_of(*repo);
                let open =
                    !self.is_folded(&format!("{}:{name}", if *incoming { "in" } else { "out" }));
                let (title, arrow, color) = match (incoming, published) {
                    (true, _) => ("On origin, not pulled", IconKind::ArrowDown, colors.warning),
                    (false, true) => ("Not pushed", IconKind::ArrowUp, colors.text_accent),
                    (false, false) => ("Not published", IconKind::ArrowUp, colors.text_accent),
                };
                self.row_base(index, ROW_H)
                    .child(chevron(open))
                    .child(grow(label(title).size(11.0).color(colors.text_placeholder)))
                    .child(arrow_count(arrow, *count, color))
            }
            Row::FilesHeader {
                repo,
                added,
                deleted,
                count,
            } => {
                let name = self.source_name_of(*repo);
                let open = !self.is_folded(&format!("rf:{name}"));
                let base = self
                    .sources
                    .get(*repo)
                    .map(|source| source.default_branch.clone())
                    .unwrap_or_default();
                self.row_base(index, ROW_H)
                    .child(chevron(open))
                    .child(grow(
                        label("Files changed on this branch")
                            .size(11.0)
                            .color(colors.text_placeholder)
                            .truncate(),
                    ))
                    .child(
                        label(format!("vs {base}"))
                            .size(11.0)
                            .color(colors.text_placeholder),
                    )
                    .child(diff_stat(Some(*added), Some(*deleted), 11.0))
                    .child(
                        label(count.to_string())
                            .size(11.0)
                            .color(colors.text_placeholder),
                    )
            }
            Row::Commit {
                repo,
                commit,
                incoming,
                with_repo,
                indent,
                ..
            } => {
                let unpushed = !incoming
                    && scan
                        .repos
                        .get(*repo)
                        .is_some_and(|state| state.is_unpushed(&commit.sha));
                let mut first = div().row().h_px(18.0).gap(6.0).items_center().child(grow(
                    label(commit.subject.clone()).color(colors.text).truncate(),
                ));
                if *with_repo {
                    first = first.child(
                        div()
                            .row()
                            .h_px(16.0)
                            .px(5.0)
                            .rounded(3.0)
                            .items_center()
                            .bg(colors.text.alpha(0.05))
                            .child(
                                label(self.source_name_of(*repo))
                                    .size(10.5)
                                    .color(colors.text_muted),
                            ),
                    );
                }
                if unpushed {
                    first = first.child(
                        div()
                            .w_px(16.0)
                            .h_px(16.0)
                            .rounded(3.0)
                            .items_center()
                            .justify_center()
                            .border(1.0, colors.border)
                            .bg(colors.element_background)
                            .child(icon(IconKind::ArrowUp).size(10.0).color(colors.text_accent)),
                    );
                }
                let second = div()
                    .row()
                    .h_px(16.0)
                    .gap(6.0)
                    .items_center()
                    .child(avatar(&commit.author))
                    .child(
                        label(commit.author.clone())
                            .size(11.5)
                            .color(colors.text_muted)
                            .truncate(),
                    )
                    .child(
                        label(git::history::relative_time(commit.timestamp))
                            .size(11.5)
                            .color(colors.text_placeholder),
                    )
                    .child(
                        label(commit.short_sha.clone())
                            .size(11.0)
                            .mono()
                            .color(colors.text_placeholder),
                    );
                let mut row = div()
                    .col()
                    .h_px(rows::COMMIT_ROW_H)
                    .pl(*indent)
                    .pr(8.0)
                    .pt(4.0)
                    .gap(2.0)
                    .on_click(self.id(index, Control::Main))
                    .child(first)
                    .child(second);
                if self.hovered_row() == Some(index) {
                    row = row.bg(colors.ghost_element_hover);
                }
                self.mark_keyboard_row(index, row)
            }
            Row::CommitHeader => self.commit_header(scan),
        }
    }

    fn source_name_of(&self, repo: usize) -> String {
        self.sources
            .get(repo)
            .map(|source| source.name.clone())
            .unwrap_or_default()
    }

    fn repo_header(&self, index: usize, repo: usize, history: bool, scan: &Scan) -> Div {
        let colors = theme();
        let name = self.source_name_of(repo);
        let open = !self.is_folded(&format!("{}:{name}", if history { "h" } else { "c" }));
        let mut header = self.row_base(index, ROW_H).child(chevron(open)).child(
            label(name)
                .size(13.0)
                .weight(600)
                .color(colors.text)
                .truncate(),
        );
        let mut middle = div().row().flex(1.0).items_center();
        if let Some(on_branch) = self.on_branch(scan, repo) {
            middle = middle.child(on_branch);
        }
        header = header.child(middle);
        let Some(state) = scan.repos.get(repo) else {
            return header;
        };
        if history {
            let unpushed = state
                .history
                .iter()
                .filter(|commit| state.is_unpushed(&commit.sha))
                .count();
            if unpushed > 0 {
                header = header.child(arrow_count(IconKind::ArrowUp, unpushed, colors.text_muted));
            }
            return header.child(
                label(state.history.len().to_string())
                    .size(11.0)
                    .color(colors.text_placeholder),
            );
        }
        let (added, deleted) = state
            .uncommitted
            .iter()
            .fold((0, 0), |(added, deleted), file| {
                (
                    added + file.added.unwrap_or(0),
                    deleted + file.deleted.unwrap_or(0),
                )
            });
        let staging = rows::repo_staging(state);
        let tip = if staging == Staging::Staged {
            "Unstage everything in this repo"
        } else {
            "Stage everything in this repo"
        };
        header
            .child(diff_stat(Some(added), Some(deleted), 12.0))
            .child(self.check(index, staging, tip))
    }

    #[allow(clippy::too_many_arguments)]
    fn file_row(
        &self,
        index: usize,
        repo: usize,
        scope: Scope,
        entry: &crate::FileEntry,
        depth: usize,
        flat: bool,
        scan: &Scan,
    ) -> Div {
        let colors = theme();
        let reviewed = scope == Scope::Branch && self.is_reviewed(scan, repo, &entry.path);
        let deleted = entry.status == ChangeStatus::Deleted;
        let mut row = self
            .row_base(index, ROW_H)
            .pl(10.0 + depth as f32 * INDENT + if flat { 0.0 } else { 18.0 });
        if !flat {
            row = with_guides(row, depth);
        }
        let (folder, name) = match entry.path.rsplit_once('/') {
            Some((folder, name)) => (Some(folder.to_string()), name.to_string()),
            None => (None, entry.path.clone()),
        };
        let name_color = if deleted {
            colors.text_disabled
        } else if reviewed {
            colors.text_muted
        } else {
            colors.text
        };
        let mut name_label = label(name).color(name_color).truncate();
        if deleted {
            name_label = name_label.strikethrough(colors.text_disabled);
        }
        let mut path = div()
            .row()
            .flex(1.0)
            .gap(6.0)
            .items_center()
            .child(name_label);
        if flat {
            if let Some(folder) = folder {
                path = path.child(
                    label(folder)
                        .size(12.0)
                        .color(if deleted {
                            colors.text_disabled
                        } else {
                            colors.text_muted
                        })
                        .truncate_start(),
                );
            }
        }
        row = row.child(status_icon(entry.status)).child(path);
        if scope == Scope::Branch {
            let eye = self.id(index, Control::Eye);
            let mut button = div()
                .w_px(20.0)
                .h_px(20.0)
                .rounded(4.0)
                .items_center()
                .justify_center()
                .on_click(eye)
                .child(
                    icon(IconKind::Eye)
                        .size(13.0)
                        .color(if reviewed || self.hot(eye) {
                            colors.icon_muted
                        } else {
                            colors.icon_muted.alpha(0.25)
                        }),
                );
            if self.hot(eye) {
                button = tooltip(
                    button.bg(colors.element_selected),
                    if reviewed {
                        "Mark as Not Reviewed"
                    } else {
                        "Mark as Reviewed"
                    },
                    false,
                );
            }
            row = row.child(button);
        }
        row = row.child(diff_stat(entry.added, entry.deleted, 12.0));
        if let (Some(staging), Scope::Staged | Scope::Unstaged | Scope::Working) =
            (entry.staging, scope)
        {
            let shown = match (scope, staging) {
                (Scope::Staged, Staging::Unstaged) => Staging::Staged,
                (Scope::Unstaged, Staging::Staged) => Staging::Unstaged,
                (_, staging) => staging,
            };
            let tip = match (scope, staging) {
                (Scope::Staged, _) | (Scope::Working, Staging::Staged) => "Unstage",
                _ => "Stage",
            };
            row = row.child(self.check(index, shown, tip));
        }
        row
    }

    fn card_header(&self, index: usize, repo: usize, scan: &Scan, width: f32) -> Div {
        let colors = theme();
        let name = self.source_name_of(repo);
        let open = !self.is_folded(&format!("r:{name}"));
        let state = scan.repos.get(repo);
        let drift = self.other_branch(scan, repo);
        let branch = state
            .and_then(RepoScan::branch_name)
            .map(str::to_string)
            .or_else(|| drift.as_ref().map(|(branch, _)| branch.clone()))
            .unwrap_or_default();
        let local_only =
            state.is_some_and(|state| state.outgoing.is_none() && state.branch_name().is_some());
        let branch_color = match drift {
            Some((_, false)) => colors.warning,
            _ => colors.text_placeholder,
        };
        let branch_label = label(if local_only {
            format!("{branch} - local only")
        } else {
            branch
        })
        .size(11.0)
        .mono()
        .color(branch_color)
        .truncate();
        let mut trailing: Vec<Node> = Vec::new();
        if let Some(action) = state
            .and_then(|state| state.head.as_ref())
            .and_then(commit_area::remote_action)
        {
            let (text, request, ahead, behind) = action;
            let chip_id = self.id(index, Control::Action);
            let (tone, tip) = match (text, &request) {
                ("Publish", _) => (ChipTone::Pull, "git push --set-upstream"),
                ("Push", _) => (ChipTone::Push, "git push"),
                ("Pull", _) => (ChipTone::Pull, "git pull"),
                ("Sync", _) => (ChipTone::Pull, "git pull, then git push"),
                _ => (ChipTone::Quiet, "git fetch"),
            };
            let mut parts = Vec::new();
            if text == "Publish" {
                parts.push(chip_icon(IconKind::ExpandUp, tone));
            }
            if behind > 0 {
                parts.push(chip_icon(IconKind::ArrowDown, tone));
                parts.push(chip_text(behind.to_string(), tone));
            }
            if ahead > 0 {
                parts.push(chip_icon(IconKind::ArrowUp, tone));
                parts.push(chip_text(ahead.to_string(), tone));
            }
            parts.push(chip_text(text.to_string(), tone));
            let button = div().child(chip(chip_id, tone, parts, self.hot(chip_id)));
            trailing.push(
                if self.hot(chip_id) {
                    tooltip(button, tip, false)
                } else {
                    button
                }
                .into(),
            );
        }
        let menu = self.id(index, Control::Menu);
        trailing.push(icon_button(menu, IconKind::ChevronDown, self.hot(menu), true).into());
        // A long repo name is cut to what the row leaves after the chevron, the action chip and the menu,
        // keeping room for a little of the branch; the chip and the menu always fit.
        const GAP: f32 = 6.0;
        let fixed = 8.0
            + 6.0
            + 12.0
            + GAP
            + trailing
                .iter()
                .map(|node| ui::measure(node).0 + GAP)
                .sum::<f32>();
        let room = (width - fixed).max(40.0);
        let name_w = ui::measure_text_width(&name, 13.0, false, 600) / ui::ui_text_scale();
        let branch_room = 48.0;
        let mut header = self
            .row_base(index, rows::CARD_HEADER_H)
            .pl(8.0)
            .pr(6.0)
            .child(chevron(open));
        header = if name_w + GAP + branch_room <= room {
            header
                .child(label(name).weight(600).color(colors.text))
                .child(grow(branch_label))
        } else {
            header
                .child(
                    div()
                        .row()
                        .w_px((room - GAP - branch_room).max(40.0).min(name_w))
                        .child(label(name).weight(600).color(colors.text).truncate()),
                )
                .child(grow(branch_label))
        };
        for node in trailing {
            header = header.child(node);
        }
        header
    }

    fn pull_request_block(&self, index: usize, _repo: usize, pr: &pom_forge::PullRequest) -> Div {
        let colors = theme();
        let hovered = self.hovered_row() == Some(index);
        let failing = pr.checks == "fail";
        let (kind, color) = if failing {
            (IconKind::XCircle, colors.error)
        } else if pr.state == "MERGED" {
            (IconKind::Merged, workspace::PrSeverity::Merged.color())
        } else {
            (IconKind::PullRequest, pr_color(pr))
        };
        let external = self.id(index, Control::External);
        let mut first = div()
            .row()
            .h_px(26.0)
            .pl(26.0)
            .pr(8.0)
            .gap(6.0)
            .items_center()
            .child(icon(kind).size(13.0).color(color))
            .child(grow(
                label(format!("#{} {}", pr.number, pr.title))
                    .size(12.0)
                    .color(colors.text)
                    .truncate(),
            ));
        if hovered {
            let mut link = icon_button(external, IconKind::ArrowUpRight, self.hot(external), true);
            if self.hot(external) {
                link = tooltip(link, "Open in Browser", false);
            }
            first = first.child(link);
        }
        let (checks, checks_color) = match pr.checks.as_str() {
            "fail" => ("CI failed", colors.error),
            "pending" => ("Checks pending", colors.warning),
            "pass" => ("Checks passing", colors.success),
            _ => ("No checks", colors.text_placeholder),
        };
        let (review, review_color) = match pr.review.as_str() {
            "approved" => ("Approved", colors.success),
            "changes" => ("Changes requested", colors.error),
            _ => ("Review pending", colors.text_placeholder),
        };
        let mut second = div().row().h_px(20.0).pl(26.0).gap(8.0).items_center();
        if pr.conflict {
            second = second.child(label("Merge conflict").size(12.0).color(colors.error));
        }
        second = second
            .child(label(checks).size(12.0).color(checks_color))
            .child(label(review).size(12.0).color(review_color));
        if pr.is_draft {
            second = second.child(label("Draft").size(12.0).color(colors.text_placeholder));
        }
        let mut block = div()
            .col()
            .h_px(rows::PULL_REQUEST_H)
            .on_click(self.id(index, Control::Main))
            .child(first)
            .child(second);
        if hovered {
            block = block.bg(colors.ghost_element_hover);
        }
        self.mark_keyboard_row(index, block)
    }

    fn commit_header(&self, scan: &Scan) -> Div {
        let colors = theme();
        let Some(opened) = &self.opened else {
            return div();
        };
        let where_it_is = if opened.incoming {
            "on origin, not pulled yet"
        } else if scan
            .repos
            .get(opened.repo)
            .is_some_and(|state| state.is_unpushed(&opened.commit.sha))
        {
            "not pushed yet"
        } else {
            "pushed"
        };
        div()
            .col()
            .h_px(rows::COMMIT_HEADER_H)
            .px(10.0)
            .pt(6.0)
            .gap(6.0)
            .child(
                div().row().h_px(20.0).items_center().child(grow(
                    label(opened.commit.subject.clone())
                        .size(14.0)
                        .color(colors.text)
                        .truncate(),
                )),
            )
            .child(
                div()
                    .row()
                    .h_px(16.0)
                    .gap(10.0)
                    .items_center()
                    .child(
                        label(opened.commit.author.clone())
                            .size(12.0)
                            .color(colors.text_muted)
                            .truncate(),
                    )
                    .child(
                        label(git::history::relative_time(opened.commit.timestamp))
                            .size(12.0)
                            .color(colors.text_muted),
                    )
                    .child(
                        label(opened.commit.short_sha.clone())
                            .size(11.0)
                            .mono()
                            .color(colors.text_placeholder),
                    )
                    .child(label(where_it_is).size(12.0).color(colors.text_muted)),
            )
    }

    fn branch_row(&self, width: f32, scan: &Scan, link: Option<String>) -> Node {
        let colors = theme();
        let workspace_branch = self.workspace_branch(scan);
        let total = self.sources.len();
        let mut drifted = Vec::new();
        let mut kept = 0;
        for index in 0..total {
            match self.other_branch(scan, index) {
                Some((_, true)) => kept += 1,
                Some((branch, false)) => {
                    drifted.push(format!("{} on {branch}", self.source_name_of(index)))
                }
                None => {}
            }
        }
        let count = if kept > 0 {
            format!(" + {kept} other")
        } else {
            format!(" in {} of {total}", total - drifted.len())
        };
        let pick = self.fixed(BRANCH_MENU);
        let mut picker = div()
            .row()
            .h_px(20.0)
            .px(4.0)
            .rounded(4.0)
            .items_center()
            .on_click(pick)
            .child(
                label(workspace_branch.clone())
                    .size(12.0)
                    .color(colors.text),
            )
            .child(
                label(count.clone())
                    .size(12.0)
                    .color(colors.text_placeholder),
            );
        if self.hot(pick) {
            picker = tooltip(
                picker.bg(colors.ghost_element_hover),
                "The branch of every repo in this workspace",
                true,
            );
        }
        let picker_w =
            8.0 + text_width(&workspace_branch, 12.0, false) + text_width(&count, 12.0, false);
        let link_id = self.fixed(BRANCH_LINK);
        let link_w = link
            .as_ref()
            .map_or(0.0, |text| 8.0 + text_width(text, 12.0, false));
        let mut row = div()
            .row()
            .w_px(width)
            .h_px(BRANCH_ROW_H)
            .px(8.0)
            .gap(4.0)
            .items_center()
            .child(icon(IconKind::Branch).size(14.0).color(colors.icon_muted))
            .child(picker);
        if !drifted.is_empty() {
            let text = drifted.join(", ");
            let room = (width - 16.0 - 14.0 - picker_w - link_w - 16.0).max(24.0);
            let natural = 6.0 + 11.0 + 3.0 + text_width(&text, 11.0, false) + 6.0;
            let chip_id = self.fixed(DRIFT_CHIP);
            let mut chip = div()
                .row()
                .w_px(natural.min(room))
                .h_px(18.0)
                .px(6.0)
                .gap(3.0)
                .rounded(4.0)
                .items_center()
                .on_click(chip_id)
                .bg(colors
                    .warning
                    .alpha(if self.hot(chip_id) { 0.2 } else { 0.12 }))
                .child(icon(IconKind::Warning).size(11.0).color(colors.warning))
                .child(grow(
                    label(text).size(11.0).color(colors.warning).truncate(),
                ));
            if self.hot(chip_id) {
                chip = tooltip(chip, "Some repos are on another branch", true);
            }
            row = row.child(chip);
        }
        row = fill(row);
        if let Some(text) = link {
            let mut link = div()
                .row()
                .h_px(20.0)
                .px(4.0)
                .rounded(4.0)
                .items_center()
                .on_click(link_id)
                .child(label(text).size(12.0).color(colors.text_accent));
            if self.hot(link_id) {
                link = tooltip(
                    link.bg(colors.ghost_element_hover),
                    "Open the Remote tab",
                    true,
                );
            }
            row = row.child(link);
        }
        row.into()
    }

    fn render_footer(&mut self, width: f32, scan: &Scan) -> (Node, f32) {
        let colors = theme();
        let footer = div()
            .col()
            .w_px(width)
            .bg(colors.panel_background)
            .child(line(colors.border));
        match self.tab {
            Tab::Changes => self.changes_footer(width, scan, footer),
            Tab::Remote => {
                let differ = scan
                    .repos
                    .iter()
                    .filter(|repo| repo.differs_from_origin())
                    .count();
                let plural = if differ == 1 { "" } else { "s" };
                let (summary, action) = if differ == 0 {
                    (
                        "Every repo matches origin".to_string(),
                        "All in sync".to_string(),
                    )
                } else {
                    (
                        format!(
                            "{differ} repo{plural} differ{} from origin",
                            if differ == 1 { "s" } else { "" }
                        ),
                        format!("Sync {differ} repo{plural}"),
                    )
                };
                let split = SplitButton {
                    main: self.fixed(SYNC),
                    menu: self.fixed(SYNC_MENU),
                    text: &action,
                    note: None,
                    enabled: differ > 0 && !self.commit.busy(),
                    hover: self.hover,
                };
                let mut main = split.main_half();
                if self.hot(self.fixed(SYNC)) && split.enabled {
                    main = tooltip(main, "Pull, then push every repo that differs", true);
                }
                let sync_row = div()
                    .row()
                    .w_px(width)
                    .h_px(BRANCH_ROW_H)
                    .px(8.0)
                    .gap(4.0)
                    .items_center()
                    .child(grow(
                        label(summary)
                            .size(12.0)
                            .color(colors.text_placeholder)
                            .truncate(),
                    ))
                    .child(split.finish(main));
                let node = footer
                    .child(self.branch_row(width, scan, None))
                    .child(line(colors.border_variant))
                    .child(sync_row);
                (node.into(), 1.0 + BRANCH_ROW_H + 1.0 + BRANCH_ROW_H)
            }
            Tab::History => {
                let unpushed: usize = scan.repos.iter().map(RepoScan::unpushed).sum();
                let link = (unpushed > 0).then(|| format!("{unpushed} not pushed"));
                let node = footer.child(self.branch_row(width, scan, link));
                (node.into(), 1.0 + BRANCH_ROW_H)
            }
        }
    }

    fn changes_footer(&mut self, width: f32, scan: &Scan, footer: Div) -> (Node, f32) {
        let colors = theme();
        let to_push = scan
            .repos
            .iter()
            .filter(|repo| {
                repo.branch_name().is_some()
                    && match repo.upstream() {
                        Upstream::None | Upstream::Gone => true,
                        Upstream::Tracked { ahead, .. } => ahead > 0,
                    }
            })
            .count();
        let link = (to_push > 0).then(|| format!("{to_push} to push"));
        let plan = self.plan(scan);
        let staged = self.commit_targets(scan);
        let placeholder = if staged.is_empty() {
            "Stage files to commit".to_string()
        } else {
            self.suggestion(&plan.targets, scan)
                .unwrap_or_else(|| "Enter commit message".into())
        };
        let editor = self
            .commit
            .render_editor(width, &placeholder, self.fixed(COMMIT_EDITOR));
        let plan_node: Node = if staged.is_empty() {
            label("nothing staged")
                .size(11.0)
                .color(colors.text_placeholder)
                .into()
        } else {
            let id = self.fixed(PLAN);
            let mut pick = div()
                .row()
                .h_px(20.0)
                .px(4.0)
                .gap(3.0)
                .rounded(4.0)
                .items_center()
                .on_click(id)
                .child(grow(
                    label(plan.summary.clone())
                        .size(11.0)
                        .color(colors.text_muted)
                        .truncate(),
                ))
                .child(
                    icon(IconKind::ChevronDown)
                        .size(9.0)
                        .color(colors.icon_muted),
                );
            if self.hot(id) {
                pick = tooltip(
                    pick.bg(colors.ghost_element_hover),
                    "Each repo gets its own commit with this message; choose which repos",
                    true,
                );
            }
            pick.into()
        };
        let options: Vec<&str> = [
            (self.commit.signoff, "signoff"),
            (self.commit.skip_hooks, "no hooks"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, text)| *text)
        .collect();
        let note = (!options.is_empty()).then(|| format!(" +{}", options.join(", ")));
        let split = SplitButton {
            main: self.fixed(COMMIT),
            menu: self.fixed(COMMIT_MENU),
            text: plan.title,
            note: note.as_deref(),
            enabled: plan.enabled,
            hover: self.hover,
        };
        let mut main = split.main_half();
        if self.hot(self.fixed(COMMIT)) || (!plan.enabled && self.hot(self.fixed(COMMIT))) {
            main = tooltip(main, &plan.hint, true);
        }
        let commit_button = split.finish(main);
        let button_w = 16.0
            + text_width(plan.title, 12.0, false)
            + note
                .as_deref()
                .map_or(0.0, |note| text_width(note, 11.0, false))
            + 22.0;
        let commit_row = div()
            .row()
            .w_px(width)
            .h_px(COMMIT_ROW_H)
            .px(6.0)
            .gap(4.0)
            .items_center()
            .bg(colors.editor_background)
            .child(
                div()
                    .row()
                    .w_px((width - 12.0 - 8.0 - button_w).max(40.0))
                    .items_center()
                    .child(plan_node),
            )
            .child(div().row().flex(1.0))
            .child(commit_button);
        let mut node = footer
            .child(self.branch_row(width, scan, link))
            .child(line(colors.border))
            .child(editor)
            .child(line(colors.border_variant))
            .child(commit_row);
        let mut height =
            1.0 + BRANCH_ROW_H + 1.0 + self.commit.editor_height() + 1.0 + COMMIT_ROW_H;
        if let Some((repo, commit)) = self.last_commit(scan) {
            let pushed = !Self::can_uncommit(scan, repo, &commit);
            let subject = self.fixed(LAST_COMMIT);
            let mut open = div()
                .row()
                .flex(1.0)
                .h_px(20.0)
                .px(4.0)
                .rounded(3.0)
                .items_center()
                .on_click(subject)
                .child(grow(
                    label(format!(
                        "{} - {}",
                        commit.subject,
                        self.source_name_of(repo)
                    ))
                    .size(12.0)
                    .color(if self.hot(subject) {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .truncate(),
                ));
            if self.hot(subject) {
                open = open.bg(colors.ghost_element_hover);
            }
            let undo = self.fixed(UNCOMMIT);
            let mut undo_button = icon_button(undo, IconKind::Undo, self.hot(undo), !pushed);
            if self.hot(undo) {
                let tip = if pushed {
                    "Already pushed: can't uncommit"
                } else {
                    "Uncommit (git reset HEAD^ --soft)"
                };
                undo_button = tooltip(undo_button, tip, true);
            }
            let last = div()
                .row()
                .w_px(width)
                .h_px(LAST_COMMIT_H)
                .pl(8.0)
                .pr(6.0)
                .gap(6.0)
                .items_center()
                .child(
                    label("Last commit")
                        .size(12.0)
                        .color(colors.text_placeholder),
                )
                .child(open)
                .child(undo_button);
            node = node.child(line(colors.border)).child(last);
            height += 1.0 + LAST_COMMIT_H;
        }
        (node.into(), height)
    }
}

fn pr_color(pr: &pom_forge::PullRequest) -> Rgba {
    let colors = theme();
    match (pr.state.as_str(), pr.is_draft) {
        ("MERGED", _) => workspace::PrSeverity::Merged.color(),
        ("CLOSED", _) => colors.error,
        (_, true) => colors.text_muted,
        _ if pr.conflict || pr.checks == "fail" || pr.review == "changes" => colors.error,
        _ if pr.checks == "pending" || pr.review == "review" => colors.warning,
        _ => colors.success,
    }
}
