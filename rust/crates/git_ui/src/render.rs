use git::working_copy::{Staging, Upstream};
use git::ChangeStatus;
use ui::{deferred, div, icon, label, theme, Corner, Div, IconKind, Node, Rgba};

use crate::commit_area::{self, RemoteKind};
use crate::rows::{self, Row};
use crate::scan::{RepoScan, Scan};
use crate::widgets::{
    arrow_count, avatar, checkbox, chevron, chip, chip_count, chip_icon, chip_text, diff_stat,
    ghost_button, icon_button, status_icon, text_width, tooltip, with_guides, ChipTone,
    SplitButton, INDENT, ROW_H, SEGMENT_H, TAB_BAR_H, TOOLBAR_H,
};
use crate::{
    Control, GitPanel, Scope, Tab, BACK, BRANCH_LINK, BRANCH_MENU, COMMIT, COMMIT_EDITOR,
    COMMIT_MENU, DRIFT_CHIP, FETCH_ALL, LAST_COMMIT, OPEN_ALL_DIFFS, PLAN, STAGE_ALL, STAGE_MENU,
    SYNC, SYNC_MENU, TAB_CHANGES, TAB_HISTORY, TAB_REMOTE, UNCOMMIT, VIEW_DIFF, VIEW_OPTIONS,
};

const BRANCH_ROW_H: f32 = 24.0;
const COMMIT_ROW_H: f32 = 28.0;
const LAST_COMMIT_H: f32 = 30.0;
const WHY_H: f32 = 20.0;
const FOOTER_PAD: f32 = 10.0;
const FOOTER_GAP: f32 = 8.0;
/// The list's side padding; cards sit this far in from the panel's edges.
const CARD_INSET: f32 = 6.0;
/// A card's own padding around its rows.
const CARD_PAD_X: f32 = 4.0;
const CARD_RADIUS: f32 = 10.0;

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
            .px(8.0)
            .gap(8.0)
            .rounded(6.0)
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
                .border(1.0, theme().text_accent)
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

    /// The "on <branch>" pill, cut to `max` wide when given (its branch truncates inside).
    fn on_branch(&self, scan: &Scan, repo: usize, max: Option<f32>) -> Option<Node> {
        let (branch, kept) = self.other_branch(scan, repo)?;
        let color = if kept {
            theme().text_muted
        } else {
            theme().warning
        };
        let mut pill = div().row();
        if let Some(max) = max {
            pill = pill.w_px(max);
        }
        Some(
            pill.h_px(18.0)
                .px(6.0)
                .rounded(9.0)
                .items_center()
                .bg(color.alpha(0.14))
                .child(
                    label(format!("on {branch}"))
                        .size(12.0)
                        .color(color)
                        .truncate(),
                )
                .into(),
        )
    }

    pub(crate) fn render_panel(&mut self, width: f32, height: f32) -> Node {
        let scan = self.current();
        self.rows = self.build_rows(&scan);
        self.settle_selection();
        let (footer, footer_h) = self.render_footer(width, &scan);
        let list_h = (height - TAB_BAR_H - TOOLBAR_H - footer_h).max(0.0);
        self.viewport_h = list_h;
        let max_scroll = (rows::content_height(&self.rows) - list_h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let mut top = 0.0;
        let mut first_top = None;
        let mut shown_h = 0.0;
        let mut shown = div().col().w_px(width);
        // Each card is one rounded surface drawn behind its rows: (repo, top, height) in list coordinates.
        let mut cards: Vec<(usize, f32, f32)> = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            let row_h = row.height();
            if let Some(repo) = row.card() {
                match cards.last_mut() {
                    Some((last, start, height))
                        if *last == repo && (*start + *height - top).abs() < 0.5 =>
                    {
                        *height += row_h
                    }
                    _ => cards.push((repo, top, row_h)),
                }
            }
            if top + row_h <= self.scroll {
                top += row_h;
                continue;
            }
            if top >= self.scroll + list_h {
                top += row_h;
                continue;
            }
            first_top.get_or_insert(top);
            shown = shown.child(self.render_row(index, row, &scan, width));
            shown_h += row_h;
            top += row_h;
        }
        let first = first_top.unwrap_or(0.0);
        let mut layer = div().col().w_px(width).h_px(shown_h);
        for (_, start, height) in cards {
            if start + height <= first || start >= first + shown_h {
                continue;
            }
            layer = layer.pin(
                Corner::TopLeft,
                CARD_INSET,
                start - first,
                width - 2.0 * CARD_INSET,
                height,
                div().rounded(CARD_RADIUS).bg(theme().editor_background),
            );
        }
        let shown = layer.pin(Corner::TopLeft, 0.0, 0.0, width, shown_h, shown);
        let offset = first - self.scroll;
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
        // A segmented control: the tabs share one track, the chosen one lifted onto it.
        let tab = |id: u64, tab: Tab, title: &str, count: usize, dot: bool| -> Node {
            let active = self.tab == tab;
            let mut cell = div()
                .row()
                .flex(1.0)
                .h_px(SEGMENT_H)
                .gap(6.0)
                .rounded(4.0)
                .items_center()
                .justify_center()
                .on_click(self.fixed(id));
            if active {
                cell = cell
                    .bg(colors.element_background)
                    .border(1.0, colors.border_variant);
            } else if self.hot(self.fixed(id)) {
                cell = cell.bg(colors.element_hover);
            }
            let mut text = label(title.to_string()).size(12.0).color(if active {
                colors.text
            } else {
                colors.text_muted
            });
            if active {
                text = text.medium();
            }
            cell = cell.child(text);
            if count > 0 {
                cell = cell.child(
                    div()
                        .row()
                        .h_px(16.0)
                        .px(5.0)
                        .rounded(8.0)
                        .items_center()
                        .bg(colors.element_hover)
                        .child(
                            label(count.to_string())
                                .size(10.5)
                                .mono()
                                .color(colors.text_muted),
                        ),
                );
            }
            if dot {
                cell = cell.child(div().w_px(6.0).h_px(6.0).rounded(3.0).bg(colors.error));
            }
            cell.into()
        };
        div()
            .row()
            .w_px(width)
            .h_px(TAB_BAR_H)
            .pt(8.0)
            .px(8.0)
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .p(2.0)
                    .gap(2.0)
                    .rounded(6.0)
                    .bg(colors.background)
                    .child(tab(TAB_CHANGES, Tab::Changes, "Changes", changes, false))
                    .child(tab(TAB_REMOTE, Tab::Remote, "Remote", differ, failing))
                    .child(tab(TAB_HISTORY, Tab::History, "History", 0, false)),
            )
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
            .px(8.0)
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
                    diff_button = diff_button.child(diff_stat(Some(added), Some(deleted), 11.5));
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
                    primary: false,
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
        if row.card().is_none() {
            return self.row_content(index, row, scan, width).w_px(width).into();
        }
        let inner_w = width - 2.0 * (CARD_INSET + CARD_PAD_X);
        div()
            .row()
            .w_px(width)
            .h_px(row.height())
            .pt(row.gap_above())
            .px(CARD_INSET + CARD_PAD_X)
            .child(self.row_content(index, row, scan, inner_w).w_px(inner_w))
            .into()
    }

    fn row_content(&self, index: usize, row: &Row, scan: &Scan, width: f32) -> Div {
        let colors = theme();
        match row {
            Row::CardPad { height, .. } => div().h_px(*height),
            Row::Message {
                text, indent, repo, ..
            } => div()
                .row()
                .h_px(if repo.is_some() { rows::NOTE_H } else { ROW_H })
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
            Row::Day(day) => div()
                .row()
                .h_px(rows::DAY_H)
                .pl(14.0)
                .pt(6.0)
                .items_center()
                .child(
                    label(day.to_uppercase())
                        .size(11.0)
                        .weight(600)
                        .color(colors.text_placeholder),
                ),
            Row::RepoHeader { repo, history } => {
                self.repo_header(index, *repo, *history, scan, width)
            }
            Row::Section { staged, count, .. } => {
                let (staging, tip) = if *staged {
                    (Staging::Staged, "Unstage everything in this repo")
                } else {
                    (Staging::Unstaged, "Stage everything in this repo")
                };
                let mut header = self.row_base(index, rows::SECTION_H).gap(7.0);
                header = if *count > 0 {
                    header.child(self.check(index, staging, tip))
                } else {
                    header.child(div().w_px(14.0))
                };
                header
                    .child(
                        label(if *staged { "Staged" } else { "Not staged" })
                            .size(12.0)
                            .medium()
                            .color(colors.text_muted),
                    )
                    .child(grow(
                        label(count.to_string())
                            .size(12.0)
                            .color(colors.text_placeholder),
                    ))
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
                let mut row =
                    with_guides(self.row_base(index, row.height()), depth.saturating_sub(1))
                        .gap(6.0)
                        .pl(8.0 + depth.saturating_sub(1) as f32 * INDENT)
                        .child(chevron(open));
                if let Some(staging) = staging {
                    let tip = match (scope, staging) {
                        (Scope::Staged, _) | (_, Staging::Staged) => "Unstage folder",
                        _ => "Stage folder",
                    };
                    row = row.child(self.check(index, *staging, tip));
                }
                row = row
                    .child(
                        icon(if open {
                            IconKind::FolderOpen
                        } else {
                            IconKind::Folder
                        })
                        .size(13.0)
                        .color(colors.icon_muted),
                    )
                    .child(grow(
                        label(text.clone())
                            .size(12.0)
                            .color(colors.text_muted)
                            .truncate(),
                    ));
                if let Some(left) = left_to_review {
                    let text = if *left == 0 {
                        "reviewed".to_string()
                    } else {
                        format!("{left} to review")
                    };
                    row = row.child(label(text).size(11.0).color(colors.text_placeholder));
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
                const BAR_W: f32 = 90.0;
                let fill_w = ((BAR_W - 4.0) * fraction.max(0.02)).min(BAR_W - 4.0);
                let bar = div()
                    .row()
                    .w_px(BAR_W)
                    .h_px(8.0)
                    .p(2.0)
                    .rounded(4.0)
                    .bg(colors.background)
                    .border(1.0, colors.border_variant)
                    .child(div().w_px(fill_w).h_px(4.0).rounded(2.0).bg(colors.info));
                div()
                    .row()
                    .h_px(rows::PROGRESS_H)
                    .px(14.0)
                    .gap(10.0)
                    .items_center()
                    .child(
                        label(format!("{reviewed} of {total} files reviewed"))
                            .size(12.0)
                            .color(colors.text_muted)
                            .truncate(),
                    )
                    .child(bar)
                    .child(div().row().flex(1.0))
                    .child(div().row().h_px(22.0).items_center().on_click(open).child(
                        label("Open all diffs").size(12.0).color(if self.hot(open) {
                            colors.text
                        } else {
                            colors.text_accent
                        }),
                    ))
            }
            Row::Card { repo } => self.card_header(index, *repo, scan),
            Row::PullRequest { repo, pull_request } => {
                self.pull_request_block(index, *repo, pull_request)
            }
            Row::CreatePullRequest { .. } => {
                let mut button = div()
                    .row()
                    .h_px(30.0)
                    .px(8.0)
                    .gap(6.0)
                    .rounded(6.0)
                    .items_center()
                    .border(1.0, colors.text_accent.alpha(0.4))
                    .on_click(self.id(index, Control::Main))
                    .child(icon(IconKind::Plus).size(12.0).color(colors.text_accent))
                    .child(grow(
                        label("Create Pull Request")
                            .size(12.0)
                            .color(colors.text_accent)
                            .truncate(),
                    ));
                if self.hovered_row() == Some(index) {
                    button = button.bg(colors.ghost_element_hover);
                }
                self.mark_keyboard_row(
                    index,
                    div().col().h_px(rows::CREATE_PR_H).px(2.0).child(button),
                )
            }
            Row::CardNote { text, .. } => div()
                .row()
                .h_px(rows::NOTE_H)
                .pl(8.0)
                .pr(8.0)
                .items_center()
                .child(grow(
                    label(text.clone())
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                )),
            Row::CommitGroup {
                incoming,
                count,
                published,
                ..
            } => {
                let (title, arrow) = match (incoming, published) {
                    (true, _) => ("On origin, not pulled", IconKind::ArrowDown),
                    (false, true) => ("Not pushed", IconKind::ArrowUp),
                    (false, false) => ("Not published", IconKind::ArrowUp),
                };
                self.row_base(index, rows::SECTION_H)
                    .gap(6.0)
                    .child(icon(arrow).size(11.0).color(colors.text_muted))
                    .child(label(title).size(12.0).medium().color(colors.text_muted))
                    .child(grow(
                        label(count.to_string())
                            .size(12.0)
                            .color(colors.text_placeholder),
                    ))
            }
            Row::FilesHeader {
                repo,
                added,
                deleted,
                count,
            } => {
                let base = self
                    .sources
                    .get(*repo)
                    .map(|source| source.default_branch.clone())
                    .unwrap_or_default();
                self.row_base(index, rows::SECTION_H)
                    .gap(6.0)
                    .child(icon(IconKind::FileGit).size(11.0).color(colors.text_muted))
                    .child(
                        label("Files changed on this branch")
                            .size(12.0)
                            .medium()
                            .color(colors.text_muted)
                            .truncate(),
                    )
                    .child(
                        label(format!("vs {base}"))
                            .size(12.0)
                            .color(colors.text_placeholder)
                            .truncate(),
                    )
                    .child(div().row().flex(1.0))
                    .child(diff_stat(Some(*added), Some(*deleted), 11.5))
                    .child(
                        label(count.to_string())
                            .size(12.0)
                            .color(colors.text_placeholder),
                    )
            }
            Row::Commit {
                repo,
                commit,
                incoming,
                with_repo,
                indent,
                compact,
                in_card,
            } => {
                let unpushed = !incoming
                    && scan
                        .repos
                        .get(*repo)
                        .is_some_and(|state| state.is_unpushed(&commit.sha));
                if *compact {
                    return self
                        .row_base(index, ROW_H)
                        .pl(*indent)
                        .child(avatar(&commit.author))
                        .child(grow(
                            label(commit.subject.clone())
                                .size(12.0)
                                .color(colors.text)
                                .truncate(),
                        ))
                        .child(
                            label(git::history::relative_time(commit.timestamp))
                                .size(12.0)
                                .color(colors.text_placeholder),
                        )
                        .child(
                            label(commit.short_sha.clone())
                                .size(11.5)
                                .mono()
                                .color(colors.text_placeholder),
                        );
                }
                // A ring for a commit only this checkout has, a filled dot once origin has it too.
                let dot = div().w_px(9.0).h_px(9.0).rounded(4.5);
                let dot = if unpushed {
                    dot.border(2.0, colors.text_accent)
                } else {
                    dot.bg(colors.text_accent)
                };
                let text = div()
                    .col()
                    .flex(1.0)
                    .gap(2.0)
                    .child(
                        div().row().child(
                            label(commit.subject.clone())
                                .size(12.0)
                                .color(colors.text)
                                .truncate(),
                        ),
                    )
                    .child(
                        div()
                            .row()
                            .gap(4.0)
                            .items_center()
                            .child(
                                label(format!(
                                    "{} - {} -",
                                    commit.author,
                                    git::history::relative_time(commit.timestamp)
                                ))
                                .size(12.0)
                                .color(colors.text_placeholder)
                                .truncate(),
                            )
                            .child(
                                label(commit.short_sha.clone())
                                    .size(11.5)
                                    .mono()
                                    .color(colors.text_placeholder),
                            ),
                    );
                let mut row = self
                    .row_base(index, rows::COMMIT_ROW_H)
                    .pl(*indent)
                    .gap(10.0)
                    .child(dot)
                    .child(text);
                if *with_repo {
                    row = row.child(
                        div()
                            .row()
                            .h_px(16.0)
                            .px(6.0)
                            .rounded(8.0)
                            .items_center()
                            .bg(colors.element_hover)
                            .child(
                                label(self.source_name_of(*repo))
                                    .size(10.5)
                                    .mono()
                                    .color(colors.text_muted),
                            ),
                    );
                }
                if unpushed {
                    row = row.child(icon(IconKind::ArrowUp).size(11.0).color(colors.warning));
                }
                if *in_card {
                    row
                } else {
                    div().row().px(6.0).child(row.flex(1.0))
                }
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

    fn repo_header(
        &self,
        index: usize,
        repo: usize,
        history: bool,
        scan: &Scan,
        width: f32,
    ) -> Div {
        const GAP: f32 = 7.0;
        let colors = theme();
        let name = self.source_name_of(repo);
        let open = !self.is_folded(&format!("{}:{name}", if history { "h" } else { "c" }));
        let state = scan.repos.get(repo);
        let mut leading: Vec<Node> = vec![chevron(open)];
        if !history {
            if let Some(state) = state {
                let staging = rows::repo_staging(state);
                let tip = if staging == Staging::Staged {
                    "Unstage everything in this repo"
                } else {
                    "Stage everything in this repo"
                };
                leading.push(self.check(index, staging, tip));
            }
        }
        let mut trailing: Vec<Node> = Vec::new();
        if let Some(state) = state {
            if history {
                let unpushed = state
                    .history
                    .iter()
                    .filter(|commit| state.is_unpushed(&commit.sha))
                    .count();
                let commits = state.history.len();
                trailing.push(arrow_count(
                    IconKind::ArrowUp,
                    unpushed,
                    colors.text_placeholder,
                ));
                trailing.push(
                    label(format!(
                        "{commits} commit{}",
                        if commits == 1 { "" } else { "s" }
                    ))
                    .size(12.0)
                    .color(colors.text_placeholder)
                    .into(),
                );
            } else {
                let (added, deleted) =
                    state
                        .uncommitted
                        .iter()
                        .fold((0, 0), |(added, deleted), file| {
                            (
                                added + file.added.unwrap_or(0),
                                deleted + file.deleted.unwrap_or(0),
                            )
                        });
                trailing.push(diff_stat(Some(added), Some(deleted), 11.5));
            }
        }
        let name_label = label(name)
            .size(12.0)
            .weight(600)
            .color(colors.text)
            .truncate();
        // The repo name has priority: the branch pill only gets what the name and the stats leave, and its
        // branch truncates inside it.
        let used = 12.0
            + leading
                .iter()
                .chain(trailing.iter())
                .map(|node| ui::measure(node).0 + GAP)
                .sum::<f32>()
            + ui::measure(&name_label.clone().into()).0
            + GAP;
        let mut middle = div().row().flex(1.0).items_center();
        if let Some(on_branch) = self.on_branch(scan, repo, None) {
            let room = width - used;
            if ui::measure(&on_branch).0 <= room {
                middle = middle.child(on_branch);
            } else if let Some(cut) = self
                .on_branch(scan, repo, Some(room))
                .filter(|_| room >= 40.0)
            {
                middle = middle.child(cut);
            }
        }
        let mut header = self.row_base(index, rows::REPO_HEADER_H).gap(GAP).px(6.0);
        for node in leading {
            header = header.child(node);
        }
        header = header.child(name_label).child(middle);
        for node in trailing {
            header = header.child(node);
        }
        header
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
        let working = matches!(scope, Scope::Staged | Scope::Unstaged | Scope::Working);
        let mut row = self.row_base(index, ROW_H);
        if !flat {
            row = with_guides(row, depth.saturating_sub(1))
                .pl(10.0 + depth.saturating_sub(1) as f32 * INDENT);
        } else if scope == Scope::Branch {
            // Lines up with the commit rows above, under their group's title.
            row = row.pl(22.0).pr(4.0);
        }
        let (folder, name) = match entry.path.rsplit_once('/') {
            Some((folder, name)) => (Some(folder.to_string()), name.to_string()),
            None => (None, entry.path.clone()),
        };
        // The staging box leads the row, where the eye starts reading, as it does in the file tree.
        if let (Some(staging), true) = (entry.staging, working) {
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
        let name_color = if reviewed {
            colors.text_placeholder
        } else if deleted {
            colors.text_muted
        } else {
            colors.text
        };
        let mut name_label = label(name.clone()).size(12.0).color(name_color).truncate();
        if deleted {
            name_label = name_label.strikethrough(colors.text_muted);
        }
        let mut path = div()
            .row()
            .flex(1.0)
            .gap(8.0)
            .items_center()
            .child(ui::material_icon(files_ui::file_icon(&name)).size(14.0))
            .child(name_label);
        if flat {
            if let Some(folder) = folder {
                path = path.child(
                    label(folder)
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate_start(),
                );
            }
        }
        row = row.child(path);
        if scope == Scope::Branch {
            let eye = self.id(index, Control::Eye);
            let mut button = div()
                .w_px(20.0)
                .h_px(20.0)
                .rounded(4.0)
                .items_center()
                .justify_center()
                .on_click(eye)
                .child(icon(IconKind::Eye).size(13.0).color(if reviewed {
                    colors.success
                } else if self.hot(eye) {
                    colors.icon_muted
                } else {
                    colors.text_placeholder
                }));
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
            // A branch file only says what is left to review; its stats live in the diff it opens.
            return row.child(button);
        }
        row.child(diff_stat(entry.added, entry.deleted, 11.5))
            .child(status_icon(entry.status))
    }

    fn card_header(&self, index: usize, repo: usize, scan: &Scan) -> Div {
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
        .size(12.0)
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
                parts.push(chip_count(IconKind::ArrowDown, behind, tone));
            }
            if ahead > 0 {
                parts.push(chip_count(IconKind::ArrowUp, ahead, tone));
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
        // The name keeps its room and the branch takes what is left; on a narrow panel the branch goes first,
        // then the name truncates. The chip and the menu always fit.
        let mut header = self
            .row_base(index, rows::CARD_HEADER_H)
            .pl(6.0)
            .pr(4.0)
            .gap(7.0)
            .child(chevron(open))
            .child(
                label(name)
                    .size(12.0)
                    .weight(600)
                    .color(colors.text)
                    .truncate(),
            )
            .child(grow(branch_label));
        for node in trailing {
            header = header.child(node);
        }
        header
    }

    fn pull_request_block(&self, index: usize, _repo: usize, pr: &pom_forge::PullRequest) -> Div {
        let colors = theme();
        let hovered = self.hovered_row() == Some(index);
        let (checks, checks_color) = match pr.checks.as_str() {
            "fail" => ("CI failed", colors.error),
            "pending" => ("Checks pending", colors.warning),
            "pass" => ("Checks passing", colors.success),
            _ => ("No checks", colors.text_placeholder),
        };
        let kind = if pr.state == "MERGED" {
            IconKind::Merged
        } else {
            IconKind::PullRequest
        };
        let tone = if pr.state == "MERGED" {
            workspace::PrSeverity::Merged.color()
        } else if pr.conflict {
            colors.error
        } else {
            checks_color
        };
        let review = match pr.review.as_str() {
            "approved" => "Approved",
            "changes" => "Changes requested",
            _ => "Review pending",
        };
        let dash = || label("-").size(12.0).color(colors.text_placeholder);
        let mut parts = Vec::new();
        if pr.conflict {
            parts.push(label("Merge conflict").size(12.0).color(colors.error));
        }
        parts.push(label(checks).size(12.0).color(checks_color));
        parts.push(label(review).size(12.0).color(colors.text));
        if pr.is_draft {
            parts.push(label("Draft").size(12.0).color(colors.text_placeholder));
        }
        // Nested from the tail so a narrow card truncates the last part first and earlier parts only once
        // everything after them is gone.
        let mut tail: Option<Div> = None;
        for part in parts.into_iter().rev() {
            let mut link = div().row().gap(6.0).items_center().child(part.truncate());
            if let Some(rest) = tail {
                link = link.child(dash()).child(rest.flex(1.0));
            }
            tail = Some(link);
        }
        let second = tail.unwrap_or_else(div).h_px(18.0);
        let text = div()
            .col()
            .flex(1.0)
            .gap(1.0)
            .child(
                div().row().child(
                    label(format!("#{} {}", pr.number, pr.title))
                        .size(12.0)
                        .medium()
                        .color(colors.text)
                        .truncate(),
                ),
            )
            .child(second);
        let mut card = div()
            .row()
            .h_px(rows::PULL_REQUEST_H - 1.0)
            .px(8.0)
            .gap(8.0)
            .rounded(6.0)
            .items_center()
            .bg(if hovered {
                colors.element_hover
            } else {
                colors.element_background
            })
            .on_click(self.id(index, Control::Main))
            .child(icon(kind).size(14.0).color(tone))
            .child(text);
        if hovered {
            let external = self.id(index, Control::External);
            let mut link = icon_button(external, IconKind::ArrowUpRight, self.hot(external), true);
            if self.hot(external) {
                link = tooltip(link, "Open in Browser", false);
            }
            card = card.child(link);
        }
        // The pull request sits on a raised card of its own inside the repo's card.
        self.mark_keyboard_row(
            index,
            div().col().h_px(rows::PULL_REQUEST_H).px(2.0).child(
                // A line under the card stands in for the drop shadow that lifts it off the repo's card.
                div()
                    .col()
                    .pb(1.0)
                    .rounded(6.0)
                    .bg(colors.border)
                    .child(card),
            ),
        )
    }

    fn commit_header(&self, scan: &Scan) -> Div {
        let colors = theme();
        let Some(opened) = &self.opened else {
            return div();
        };
        let (where_it_is, where_color) = if opened.incoming {
            ("on origin, not pulled yet", colors.warning)
        } else if scan
            .repos
            .get(opened.repo)
            .is_some_and(|state| state.is_unpushed(&opened.commit.sha))
        {
            ("not pushed yet", colors.warning)
        } else {
            ("pushed", colors.text_placeholder)
        };
        let card = div()
            .col()
            .p(8.0)
            .gap(4.0)
            .rounded(10.0)
            .bg(colors.editor_background)
            .child(
                div().row().child(
                    label(opened.commit.subject.clone())
                        .size(12.0)
                        .weight(600)
                        .color(colors.text)
                        .truncate(),
                ),
            )
            .child(
                div()
                    .row()
                    .gap(4.0)
                    .items_center()
                    .child(
                        label(format!(
                            "{} - {} -",
                            opened.commit.author,
                            git::history::relative_time(opened.commit.timestamp)
                        ))
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                    )
                    .child(
                        label(opened.commit.short_sha.clone())
                            .size(11.5)
                            .mono()
                            .color(colors.text_placeholder),
                    ),
            )
            .child(label(where_it_is).size(12.0).color(where_color));
        div()
            .col()
            .h_px(rows::COMMIT_HEADER_H)
            .px(6.0)
            .pb(6.0)
            .child(card)
    }

    /// The workspace branch (with how many repos are on it), a link on the right, and below it a chip naming
    /// the repos on another branch. Returns the rows and their height.
    fn branch_row(&self, width: f32, scan: &Scan, link: Option<String>) -> (Node, f32) {
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
            format!("+ {kept} other")
        } else {
            format!("in {} of {total}", total - drifted.len())
        };
        let pick = self.fixed(BRANCH_MENU);
        // A long branch keeps the row: it truncates inside the picker before the link on the right moves.
        let link_w = link
            .as_ref()
            .map_or(0.0, |text| text_width(text, 13.0, false) + 8.0 + 6.0);
        let picker_natural = 6.0 + 12.0 + 5.0 + text_width(&workspace_branch, 13.0, false) + 6.0;
        let mut picker = div().row();
        if picker_natural > width - link_w {
            picker = picker.w_px((width - link_w).max(40.0));
        }
        let mut picker = picker
            .h_px(BRANCH_ROW_H)
            .px(6.0)
            .gap(5.0)
            .rounded(4.0)
            .items_center()
            .on_click(pick)
            .child(icon(IconKind::Branch).size(12.0).color(colors.icon_muted))
            .child(
                label(workspace_branch)
                    .size(13.0)
                    .medium()
                    .color(colors.text)
                    .truncate(),
            );
        if self.hot(pick) {
            picker = tooltip(
                picker.bg(colors.ghost_element_hover),
                "The branch of every repo in this workspace",
                true,
            );
        }
        let mut row = div()
            .row()
            .w_px(width)
            .h_px(BRANCH_ROW_H)
            .gap(6.0)
            .items_center()
            .child(picker)
            .child(grow(
                label(count)
                    .size(13.0)
                    .color(colors.text_placeholder)
                    .truncate(),
            ));
        if let Some(text) = link {
            let link_id = self.fixed(BRANCH_LINK);
            let mut link = div()
                .row()
                .h_px(BRANCH_ROW_H)
                .px(4.0)
                .rounded(4.0)
                .items_center()
                .on_click(link_id)
                .child(label(text).size(13.0).color(colors.text_accent));
            if self.hot(link_id) {
                link = tooltip(
                    link.bg(colors.ghost_element_hover),
                    "Open the Remote tab",
                    true,
                );
            }
            row = row.child(link);
        }
        if drifted.is_empty() {
            return (row.into(), BRANCH_ROW_H);
        }
        let text = drifted.join(", ");
        let chip_id = self.fixed(DRIFT_CHIP);
        let natural = 7.0 + 11.0 + 4.0 + text_width(&text, 13.0, false) + 7.0;
        let mut chip = div()
            .row()
            .w_px(natural.min(width))
            .h_px(20.0)
            .px(7.0)
            .gap(4.0)
            .rounded(10.0)
            .items_center()
            .on_click(chip_id)
            .bg(colors
                .warning
                .alpha(if self.hot(chip_id) { 0.22 } else { 0.14 }))
            .child(icon(IconKind::Warning).size(11.0).color(colors.warning))
            .child(grow(
                label(text).size(13.0).color(colors.warning).truncate(),
            ));
        if self.hot(chip_id) {
            chip = tooltip(chip, "Some repos are on another branch", true);
        }
        let rows = div()
            .col()
            .w_px(width)
            .gap(FOOTER_GAP)
            .child(row)
            .child(div().row().h_px(BRANCH_ROW_H).items_center().child(chip));
        (rows.into(), 2.0 * BRANCH_ROW_H + FOOTER_GAP)
    }

    fn render_footer(&mut self, width: f32, scan: &Scan) -> (Node, f32) {
        let colors = theme();
        let inner = width - 2.0 * FOOTER_PAD;
        let (rows, height): (Vec<Node>, f32) = match self.tab {
            Tab::Changes => self.changes_footer(inner, scan),
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
                let summary = match self.commit.remote_progress() {
                    Some(progress) if self.commit.remote_running().is_some() => progress,
                    _ => summary,
                };
                let split = SplitButton {
                    main: self.fixed(SYNC),
                    menu: self.fixed(SYNC_MENU),
                    text: &action,
                    note: None,
                    enabled: differ > 0 && !self.commit.busy(),
                    hover: self.hover,
                    primary: true,
                };
                let mut main = split.main_half();
                if self.hot(self.fixed(SYNC)) && split.enabled {
                    main = tooltip(main, "Pull, then push every repo that differs", true);
                }
                let sync_row = div()
                    .row()
                    .w_px(inner)
                    .h_px(COMMIT_ROW_H)
                    .gap(8.0)
                    .items_center()
                    .child(grow(
                        label(summary)
                            .size(13.0)
                            .color(colors.text_muted)
                            .truncate(),
                    ))
                    .child(split.finish(main));
                let (branch, branch_h) = self.branch_row(inner, scan, None);
                (
                    vec![branch, sync_row.into()],
                    branch_h + FOOTER_GAP + COMMIT_ROW_H,
                )
            }
            Tab::History => {
                let unpushed: usize = scan.repos.iter().map(RepoScan::unpushed).sum();
                let link = (unpushed > 0).then(|| format!("{unpushed} not pushed"));
                let (branch, branch_h) = self.branch_row(inner, scan, link);
                (vec![branch], branch_h)
            }
        };
        let mut footer = div()
            .col()
            .w_px(width)
            .bg(colors.editor_background)
            .child(line(colors.border_variant));
        let mut body = div().col().w_px(width).p(FOOTER_PAD).gap(FOOTER_GAP);
        for row in rows {
            body = body.child(row);
        }
        footer = footer.child(body);
        (footer.into(), 1.0 + 2.0 * FOOTER_PAD + height)
    }

    /// The Changes footer's rows (branch, message, commit, why it can't, last commit) and their height.
    fn changes_footer(&mut self, width: f32, scan: &Scan) -> (Vec<Node>, f32) {
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
        let plan_node: Node = {
            let summary = if staged.is_empty() {
                "nothing staged".to_string()
            } else {
                plan.summary.clone()
            };
            let id = self.fixed(PLAN);
            let mut pick = div()
                .row()
                .h_px(22.0)
                .gap(4.0)
                .rounded(4.0)
                .items_center()
                .child(
                    label(summary)
                        .size(13.0)
                        .color(colors.text_accent)
                        .truncate(),
                )
                .child(
                    icon(IconKind::ChevronDown)
                        .size(10.0)
                        .color(colors.text_accent),
                );
            if !staged.is_empty() {
                pick = pick.on_click(id);
            }
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
        let note = (!options.is_empty()).then(|| format!("+{}", options.join(", ")));
        let split = SplitButton {
            main: self.fixed(COMMIT),
            menu: self.fixed(COMMIT_MENU),
            text: plan.title,
            note: note.as_deref(),
            enabled: plan.enabled,
            hover: self.hover,
            primary: true,
        };
        let mut main = split.main_half();
        if self.hot(self.fixed(COMMIT)) {
            main = tooltip(main, &plan.hint, true);
        }
        let commit_row = div()
            .row()
            .w_px(width)
            .h_px(COMMIT_ROW_H)
            .gap(8.0)
            .items_center()
            .child(grow(plan_node))
            .child(split.finish(main));
        let (branch, branch_h) = self.branch_row(width, scan, link);
        let mut rows = vec![branch, editor, commit_row.into()];
        let mut height =
            branch_h + FOOTER_GAP + self.commit.editor_height() + FOOTER_GAP + COMMIT_ROW_H;
        // Say why the button is off right under it, not only in its tooltip.
        if !plan.enabled && !plan.hint.is_empty() {
            rows.push(
                div()
                    .row()
                    .w_px(width)
                    .h_px(WHY_H)
                    .gap(6.0)
                    .items_center()
                    .child(icon(IconKind::Info).size(12.0).color(colors.icon_muted))
                    .child(grow(
                        label(plan.hint.clone())
                            .size(13.0)
                            .color(colors.text_placeholder)
                            .truncate(),
                    ))
                    .into(),
            );
            height += FOOTER_GAP + WHY_H;
        }
        if let Some((repo, commit)) = self.last_commit(scan) {
            let pushed = !Self::can_uncommit(scan, repo, &commit);
            let subject = self.fixed(LAST_COMMIT);
            let mut open = div()
                .row()
                .flex(1.0)
                .h_px(22.0)
                .rounded(4.0)
                .items_center()
                .on_click(subject)
                .child(grow(
                    label(format!(
                        "{} - {}",
                        commit.subject,
                        self.source_name_of(repo)
                    ))
                    .size(13.0)
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
                .col()
                .w_px(width)
                .child(line(colors.border_variant))
                .child(
                    div()
                        .row()
                        .w_px(width)
                        .h_px(LAST_COMMIT_H - 1.0)
                        .pt(5.0)
                        .gap(8.0)
                        .items_center()
                        .child(
                            label("Last commit")
                                .size(13.0)
                                .color(colors.text_placeholder),
                        )
                        .child(open)
                        .child(undo_button),
                );
            rows.push(last.into());
            height += FOOTER_GAP + LAST_COMMIT_H;
        }
        (rows, height)
    }
}
