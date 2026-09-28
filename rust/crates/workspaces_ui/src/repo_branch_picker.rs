//! The branch picker of a repo row in the create form: the workspace branch first, then the repo's local and
//! origin branches, narrowed as you type, and a row that creates the typed branch when nothing matches. Opening
//! it fetches origin in the background, so branches pushed since show up, tagged new.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Instant;

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use pom_workspace::BranchInfo;
use ui::{deferred, div, icon, label, theme, Div, IconKind, LabelSize, Node, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::WINDOW_MODAL_BASE;

pub(crate) const PICKER_QUERY: u64 = WINDOW_MODAL_BASE + 9;
pub(crate) const PICKER_REFRESH: u64 = WINDOW_MODAL_BASE + 10;
pub(crate) const PICKER_SURFACE: u64 = WINDOW_MODAL_BASE + 11;
pub(crate) const ENTRY_BASE: u64 = WINDOW_MODAL_BASE + 500;
pub(crate) const ENTRY_END: u64 = WINDOW_MODAL_BASE + 1000;

const PLACEHOLDER: &str = "Switch or type to create a branch...";
const WIDTH: f32 = 360.0;
const ROWS_SHOWN: usize = 8;
/// Height of the branch box the picker drops from, and the gap between them.
pub(crate) const TRIGGER_HEIGHT: f32 = 26.0;
const TRIGGER_GAP: f32 = 4.0;

pub type ListBranches = Arc<dyn Fn(&str) -> Result<RepoBranches, String> + Send + Sync>;
pub type FetchOrigin = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Where the form gets a repo's branches (read from its main checkout) and how it fetches its origin.
#[derive(Clone)]
pub struct BranchSource {
    pub list: ListBranches,
    pub fetch: FetchOrigin,
}

/// A repo's branches as the picker lists them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoBranches {
    /// The branch the main checkout is on, which a new branch starts from.
    pub base: String,
    pub branches: Vec<BranchInfo>,
}

impl RepoBranches {
    /// How the worktree gets `branch`: `chosen` is a branch picked for this repo, not the workspace's.
    pub fn obtained(&self, branch: &str, chosen: bool) -> String {
        let found = self.branches.iter().find(|info| info.name == branch);
        match found {
            Some(info) if !info.remote => "local branch".to_string(),
            Some(_) if chosen => "taken over from origin".to_string(),
            Some(_) => "exists on origin, tracks it".to_string(),
            None if self.base.is_empty() => "new".to_string(),
            None => format!("new, from {}", self.base),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    WorkspaceBranch,
    Branch(BranchInfo),
    Create(String),
}

struct Fetched {
    fetch: Result<(), String>,
    list: Result<RepoBranches, String>,
}

pub(crate) struct BranchPicker {
    pub repo: String,
    pub query: TextField,
    highlighted: usize,
    fetching: Option<Receiver<Fetched>>,
    fetched: Option<Result<Instant, String>>,
    /// Branches the last fetch brought in.
    fresh: HashSet<String>,
    pub error: Option<String>,
}

impl BranchPicker {
    pub fn open(repo: &str, source: Option<&BranchSource>) -> BranchPicker {
        let mut picker = BranchPicker {
            repo: repo.to_string(),
            query: TextField::default(),
            highlighted: 0,
            fetching: None,
            fetched: None,
            fresh: HashSet::new(),
            error: None,
        };
        if let Some(source) = source {
            picker.fetch(source);
        }
        picker
    }

    pub fn fetch(&mut self, source: &BranchSource) {
        if self.fetching.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let (source, repo) = (source.clone(), self.repo.clone());
        std::thread::spawn(move || {
            let fetch = (source.fetch)(&repo);
            let list = (source.list)(&repo);
            if sender.send(Fetched { fetch, list }).is_err() {
                eprintln!("workspaces: the branch picker closed before {repo} fetched");
            }
        });
        self.fetching = Some(receiver);
    }

    pub fn busy(&self) -> bool {
        self.fetching.is_some()
    }

    /// Takes in a finished fetch; returns whether anything changed.
    pub fn poll(&mut self, known: &mut HashMap<String, RepoBranches>) -> bool {
        let Some(receiver) = &self.fetching else {
            return false;
        };
        let fetched = match receiver.try_recv() {
            Ok(fetched) => fetched,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Fetched {
                fetch: Err("the fetch stopped".into()),
                list: Err(String::new()),
            },
        };
        self.fetching = None;
        self.fetched = Some(fetched.fetch.map(|()| Instant::now()));
        if let Ok(list) = fetched.list {
            if let Some(before) = known.get(&self.repo) {
                let seen: HashSet<(&str, bool)> = before
                    .branches
                    .iter()
                    .map(|info| (info.name.as_str(), info.remote))
                    .collect();
                self.fresh.extend(
                    list.branches
                        .iter()
                        .filter(|info| !seen.contains(&(info.name.as_str(), info.remote)))
                        .map(|info| info.name.clone()),
                );
            }
            known.insert(self.repo.clone(), list);
        }
        true
    }

    pub fn query_text(&self) -> String {
        self.query.text().trim().to_string()
    }

    /// The query changed: the highlight stays where it was when that row still exists.
    pub fn typed(&mut self, entries: usize) {
        self.error = None;
        self.highlighted = self.highlighted.min(entries.saturating_sub(1));
    }

    pub fn highlight(&mut self, index: usize) {
        self.highlighted = index;
    }

    pub fn move_highlight(&mut self, down: bool, entries: usize) {
        if entries == 0 {
            return;
        }
        self.highlighted = if down {
            (self.highlighted + 1) % entries
        } else {
            (self.highlighted + entries - 1) % entries
        };
    }

    pub fn highlighted(&self) -> usize {
        self.highlighted
    }

    pub fn entries(&self, workspace_branch: &str, branches: Option<&RepoBranches>) -> Vec<Entry> {
        let query = self.query_text();
        let listed: Vec<&BranchInfo> = branches
            .map(|known| known.branches.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|info| info.name != workspace_branch)
            .collect();
        let mut entries = Vec::new();
        if query.is_empty() {
            entries.push(Entry::WorkspaceBranch);
            entries.extend(listed.iter().map(|info| Entry::Branch((*info).clone())));
            return entries;
        }
        let mut names = vec![workspace_branch.to_string()];
        names.extend(listed.iter().map(|info| display_name(info)));
        for index in ranked(&names, &query) {
            entries.push(match index {
                0 => Entry::WorkspaceBranch,
                index => Entry::Branch(listed[index - 1].clone()),
            });
        }
        let exists = query == workspace_branch || listed.iter().any(|info| info.name == query);
        if !exists {
            entries.push(Entry::Create(query));
        }
        entries
    }

    /// The picker dropped from a repo row's branch box, floating above the form.
    pub fn render(
        &self,
        workspace_branch: &str,
        branches: Option<&RepoBranches>,
        choice: Option<&str>,
    ) -> Node {
        let colors = theme();
        let panel = self
            .panel(
                &format!("Branch for {}", self.repo),
                workspace_branch,
                branches,
                choice,
            )
            .w_px(WIDTH)
            .rounded(8.0)
            .border(1.0, colors.border)
            .bg(colors.elevated_surface_background)
            .on_click(PICKER_SURFACE);
        deferred(panel)
            .below_or_above(TRIGGER_HEIGHT, TRIGGER_GAP)
            .priority(1)
            .into()
    }

    /// The picker laid into a modal as its body, open for as long as the modal is.
    pub fn render_inline(
        &self,
        workspace_branch: &str,
        branches: Option<&RepoBranches>,
        choice: Option<&str>,
    ) -> Node {
        let divider = div().h_px(1.0).bg(theme().border_variant);
        div()
            .col()
            .child(divider)
            .child(
                self.panel(
                    &format!("Branches of {}", self.repo),
                    workspace_branch,
                    branches,
                    choice,
                )
                .on_click(PICKER_SURFACE),
            )
            .into()
    }

    fn panel(
        &self,
        title: &str,
        workspace_branch: &str,
        branches: Option<&RepoBranches>,
        choice: Option<&str>,
    ) -> Div {
        let colors = theme();
        let entries = self.entries(workspace_branch, branches);
        let base = branches
            .map(|known| known.base.as_str())
            .unwrap_or_default();
        let query_row = div()
            .row()
            .items_center()
            .gap(8.0)
            .h_px(36.0)
            .px(10.0)
            .on_click(PICKER_QUERY)
            .child(icon(IconKind::Search).size(13.0).color(colors.icon_muted))
            .child(
                self.query
                    .render(PLACEHOLDER, true, colors.text, 20.0, FieldFont::Ui),
            )
            .child(
                div()
                    .row()
                    .items_center()
                    .justify_center()
                    .w_px(22.0)
                    .h_px(22.0)
                    .rounded(4.0)
                    .on_click(PICKER_REFRESH)
                    .child(icon(IconKind::RotateCw).size(13.0).color(if self.busy() {
                        colors.text_disabled
                    } else {
                        colors.icon_muted
                    })),
            );
        let first = self
            .highlighted
            .saturating_sub(ROWS_SHOWN - 1)
            .min(entries.len().saturating_sub(ROWS_SHOWN));
        let mut list = div().col().p(4.0).child(
            div().row().px(8.0).pt(4.0).pb(2.0).child(
                label(title.to_string())
                    .label_size(LabelSize::XSmall)
                    .color(colors.text_placeholder),
            ),
        );
        for (index, entry) in entries.iter().enumerate().skip(first).take(ROWS_SHOWN) {
            list =
                list.child(self.entry_row(entry, index, workspace_branch, branches, base, choice));
        }
        let divider = || div().h_px(1.0).bg(colors.border_variant);
        div()
            .col()
            .child(query_row)
            .child(divider())
            .child(list)
            .child(divider())
            .child(self.footer())
    }

    fn footer(&self) -> Node {
        let colors = theme();
        let (kind, color, text) = match (&self.error, self.busy(), &self.fetched) {
            (Some(error), _, _) => (IconKind::Warning, colors.warning, error.clone()),
            (None, true, _) => (
                IconKind::RotateCw,
                colors.icon_muted,
                "Fetching origin...".to_string(),
            ),
            (None, false, Some(Ok(at))) => {
                let minutes = at.elapsed().as_secs() / 60;
                let when = match minutes {
                    0 => "just now".to_string(),
                    1 => "1 minute ago".to_string(),
                    minutes => format!("{minutes} minutes ago"),
                };
                (
                    IconKind::Check,
                    colors.icon_muted,
                    format!("Fetched {when}"),
                )
            }
            (None, false, Some(Err(error))) => (
                IconKind::Warning,
                colors.warning,
                format!("Could not fetch origin: {error}"),
            ),
            (None, false, None) => (IconKind::Branch, colors.icon_muted, String::new()),
        };
        div()
            .row()
            .items_center()
            .gap(6.0)
            .h_px(26.0)
            .px(10.0)
            .child(icon(kind).size(11.0).color(color))
            .child(
                div().row().flex(1.0).items_center().child(
                    label(text)
                        .label_size(LabelSize::XSmall)
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            )
            .into()
    }

    fn entry_row(
        &self,
        entry: &Entry,
        index: usize,
        workspace_branch: &str,
        branches: Option<&RepoBranches>,
        base: &str,
        choice: Option<&str>,
    ) -> Node {
        let colors = theme();
        let (kind, title, subtitle, checked, fresh) = match entry {
            Entry::WorkspaceBranch => {
                let how = branches
                    .map(|known| known.obtained(workspace_branch, false))
                    .unwrap_or_default();
                let subtitle = match how.is_empty() {
                    true => "the workspace's branch".to_string(),
                    false => format!("the workspace's branch - {how}"),
                };
                (
                    IconKind::Branch,
                    workspace_branch.to_string(),
                    subtitle,
                    choice.is_none(),
                    false,
                )
            }
            Entry::Branch(info) => {
                let subtitle = [&info.author, &info.relative_time, &info.subject]
                    .iter()
                    .filter(|part| !part.is_empty())
                    .map(|part| part.as_str())
                    .collect::<Vec<_>>()
                    .join(" - ");
                (
                    if info.remote {
                        IconKind::Server
                    } else {
                        IconKind::Branch
                    },
                    display_name(info),
                    subtitle,
                    choice == Some(info.name.as_str()),
                    self.fresh.contains(&info.name),
                )
            }
            Entry::Create(name) => (
                IconKind::Plus,
                format!("Create Branch: \"{name}\""),
                match base.is_empty() {
                    true => String::new(),
                    false => format!("Based off {base}"),
                },
                false,
                false,
            ),
        };
        let (kind, icon_color) = if checked {
            (IconKind::Check, colors.icon_accent)
        } else {
            (kind, colors.icon_muted)
        };
        let mut title_row = div().row().items_center().gap(6.0).child(
            div().row().flex(1.0).items_center().child(
                label(title)
                    .label_size(LabelSize::Small)
                    .mono()
                    .color(colors.text)
                    .truncate(),
            ),
        );
        if fresh {
            title_row = title_row.child(
                div()
                    .row()
                    .items_center()
                    .px(4.0)
                    .rounded(3.0)
                    .bg(colors.text_accent.alpha(0.14))
                    .child(
                        label("new")
                            .label_size(LabelSize::XSmall)
                            .color(colors.text_accent),
                    ),
            );
        }
        let mut text = div().col().flex(1.0).child(title_row);
        if !subtitle.is_empty() {
            text = text.child(
                div().row().items_center().child(
                    label(subtitle)
                        .label_size(LabelSize::XSmall)
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            );
        }
        div()
            .row()
            .gap(8.0)
            .px(8.0)
            .py(4.0)
            .rounded(5.0)
            .on_click(ENTRY_BASE + index as u64)
            .bg(if index == self.highlighted {
                colors.ghost_element_hover
            } else {
                Rgba::TRANSPARENT
            })
            .child(
                div()
                    .col()
                    .pt(2.0)
                    .child(icon(kind).size(14.0).color(icon_color)),
            )
            .child(text)
            .into()
    }
}

fn display_name(info: &BranchInfo) -> String {
    if info.remote {
        format!("origin/{}", info.name)
    } else {
        info.name.clone()
    }
}

/// Indices of the names that fuzzy-match `query`, best first; ties keep their order.
fn ranked(names: &[String], query: &str) -> Vec<usize> {
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buffer = Vec::new();
    let mut scored: Vec<(usize, u32)> = names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            pattern
                .score(Utf32Str::new(name, &mut buffer), &mut matcher)
                .map(|score| (index, score))
        })
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(index, _)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str, remote: bool) -> BranchInfo {
        BranchInfo {
            name: name.into(),
            remote,
            author: "Ana Lima".into(),
            relative_time: "2 hours ago".into(),
            subject: "Validate cart totals".into(),
        }
    }

    fn known() -> RepoBranches {
        RepoBranches {
            base: "main".into(),
            branches: vec![
                branch("main", false),
                branch("ana/checkout-api", true),
                branch("feat-login", true),
            ],
        }
    }

    #[test]
    fn the_workspace_branch_leads_and_the_typed_name_can_be_created() {
        let mut picker = BranchPicker::open("api", None);
        let known = known();
        assert_eq!(
            picker.entries("feat-login", Some(&known)),
            [
                Entry::WorkspaceBranch,
                Entry::Branch(branch("main", false)),
                Entry::Branch(branch("ana/checkout-api", true)),
            ]
        );
        picker.query.insert("chk");
        assert_eq!(
            picker.entries("feat-login", Some(&known)),
            [
                Entry::Branch(branch("ana/checkout-api", true)),
                Entry::Create("chk".into())
            ]
        );
        picker.query.set_text("main");
        assert_eq!(
            picker.entries("feat-login", Some(&known)),
            [Entry::Branch(branch("main", false))],
            "an existing name offers no create row"
        );
    }

    #[test]
    fn how_a_branch_is_obtained() {
        let known = known();
        assert_eq!(
            known.obtained("feat-login", false),
            "exists on origin, tracks it"
        );
        assert_eq!(
            known.obtained("ana/checkout-api", true),
            "taken over from origin"
        );
        assert_eq!(known.obtained("main", true), "local branch");
        assert_eq!(known.obtained("feat-new", false), "new, from main");
    }

    #[test]
    fn a_fetch_tags_the_branches_it_brought_in() {
        let source = BranchSource {
            list: Arc::new(|_| {
                let mut known = known();
                known.branches.push(branch("ben/checkout-copy", true));
                Ok(known)
            }),
            fetch: Arc::new(|_| Ok(())),
        };
        let mut known_branches = HashMap::from([("api".to_string(), known())]);
        let mut picker = BranchPicker::open("api", Some(&source));
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while !picker.poll(&mut known_branches) && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!picker.busy());
        assert_eq!(
            picker.fresh.iter().collect::<Vec<_>>(),
            [&"ben/checkout-copy".to_string()]
        );
        assert_eq!(known_branches["api"].branches.len(), 4);
    }
}
