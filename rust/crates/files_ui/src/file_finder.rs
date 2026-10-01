use std::sync::Arc;

use ui::{div, icon, label, material_icon, theme, IconKind, LabelSize, Node};
use workspace::text_field::{FieldFont, TextField, INPUT_FONT};

use crate::command_palette::scroll_rows;
use crate::fuzzy::{match_paths, CharBag};

pub const WIDTH: f32 = 544.0;
const MAX_RESULTS_HEIGHT: f32 = 384.0;
const HEAD_HEIGHT: f32 = 36.0;
const PLACEHOLDER: &str = "Search project files...";
const MAX_MATCHES: usize = 100;
pub const MAX_RECENT: usize = 20;

#[derive(Default)]
pub struct Candidates {
    pub paths: Vec<String>,
    bags: Vec<CharBag>,
}

impl Candidates {
    pub fn new(paths: Vec<String>) -> Candidates {
        let bags = paths
            .iter()
            .map(|path| CharBag::from(path.as_str()))
            .collect();
        Candidates { paths, bags }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    /// Char indices into `path` of the matched query chars.
    pub positions: Vec<usize>,
    pub recent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub row: Option<u32>,
    pub column: Option<u32>,
}

/// Splits `path:row:column` (either number optional); the path part is what gets matched.
pub fn parse_query(query: &str) -> Target {
    let trimmed = query.trim().trim_end_matches(':');
    let mut parts = trimmed.rsplitn(3, ':');
    let last = parts.next().unwrap_or_default();
    let middle = parts.next();
    let first = parts.next();
    let number = |text: &str| text.parse::<u32>().ok().filter(|value| *value > 0);
    let (path, row, column) = match (first, middle, number(last)) {
        (Some(path), Some(row), Some(column)) if number(row).is_some() => {
            (path.to_string(), number(row), Some(column))
        }
        (_, Some(_), Some(row)) => (
            trimmed[..trimmed.len() - last.len() - 1].to_string(),
            Some(row),
            None,
        ),
        _ => (trimmed.to_string(), None, None),
    };
    let path = path.strip_prefix("./").unwrap_or(&path).to_string();
    Target { path, row, column }
}

pub struct FileFinder {
    pub field: TextField,
    candidates: Arc<Candidates>,
    /// Recently opened files, newest first; the open file (if any) leads.
    recent: Vec<String>,
    active: Option<String>,
    pub include_ignored: bool,
    matches: Vec<Entry>,
    pub(crate) selected: usize,
    scroll_top: usize,
    scroll_remainder: f32,
    pub scrollbar: crate::list_scrollbar::ScrollbarReveal,
    pub hovered: Option<u64>,
}

impl FileFinder {
    pub fn new(
        candidates: Arc<Candidates>,
        recent: Vec<String>,
        active: Option<String>,
    ) -> FileFinder {
        let mut finder = FileFinder {
            field: TextField::default(),
            candidates,
            recent,
            active,
            include_ignored: false,
            matches: Vec::new(),
            selected: 0,
            scroll_top: 0,
            scroll_remainder: 0.0,
            scrollbar: Default::default(),
            hovered: None,
        };
        finder.update_matches();
        finder
    }

    pub fn set_candidates(&mut self, candidates: Arc<Candidates>) {
        self.candidates = candidates;
        self.update_matches();
    }

    pub fn update_matches(&mut self) {
        let target = parse_query(&self.field.text());
        let mut ordered: Vec<String> = self.active.iter().cloned().collect();
        ordered.extend(
            self.recent
                .iter()
                .filter(|path| Some(*path) != self.active.as_ref())
                .cloned(),
        );
        let entries: Vec<Entry> = if target.path.trim().is_empty() {
            ordered
                .into_iter()
                .map(|path| Entry {
                    path,
                    positions: Vec::new(),
                    recent: true,
                })
                .collect()
        } else {
            let recent_paths: Vec<&str> = ordered.iter().map(String::as_str).collect();
            let recent_bags: Vec<CharBag> = recent_paths
                .iter()
                .map(|path| CharBag::from(*path))
                .collect();
            // A recent file only counts when part of the match lands in its name, not just its folders.
            let mut entries: Vec<Entry> =
                match_paths(&recent_paths, &recent_bags, &target.path, MAX_MATCHES, None)
                    .into_iter()
                    .filter(|found| {
                        let name_start =
                            recent_paths[found.candidate].rfind('/').map_or(0, |slash| {
                                recent_paths[found.candidate][..=slash].chars().count()
                            });
                        found
                            .positions
                            .iter()
                            .any(|position| *position >= name_start)
                    })
                    .map(|found| Entry {
                        path: recent_paths[found.candidate].to_string(),
                        positions: found.positions,
                        recent: true,
                    })
                    .collect();
            let paths: Vec<&str> = self.candidates.paths.iter().map(String::as_str).collect();
            for found in match_paths(
                &paths,
                &self.candidates.bags,
                &target.path,
                MAX_MATCHES,
                self.active.as_deref(),
            ) {
                let path = paths[found.candidate];
                if entries.iter().all(|entry| entry.path != path) {
                    entries.push(Entry {
                        path: path.to_string(),
                        positions: found.positions,
                        recent: false,
                    });
                }
            }
            entries.truncate(MAX_MATCHES);
            entries
        };
        self.matches = entries;
        // The file already open would be a no-op pick, so a search starts on the next one.
        let skip_active = !target.path.trim().is_empty()
            && self.matches.len() > 1
            && self.matches.first().map(|entry| Some(&entry.path)) == Some(self.active.as_ref());
        self.selected = usize::from(skip_active);
        self.scroll_top = 0;
        self.scroll_to_selected();
    }

    #[cfg(test)]
    pub fn matches(&self) -> &[Entry] {
        &self.matches
    }

    pub fn select_previous(&mut self) {
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

    pub fn select_next(&mut self) {
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

    /// The picked file with the query's line and column; `None` when nothing matches.
    pub fn confirm(&self) -> Option<Target> {
        let entry = self.matches.get(self.selected)?;
        let target = parse_query(&self.field.text());
        Some(Target {
            path: entry.path.clone(),
            row: target.row,
            column: target.column,
        })
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
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        } else if self.selected >= self.scroll_top + visible {
            self.scroll_top = self.selected + 1 - visible;
        }
        self.scroll_top = self
            .scroll_top
            .min(self.matches.len().saturating_sub(visible));
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
        let visible = Self::visible_rows();
        let end = (self.scroll_top + visible).min(self.matches.len());
        let mut results = div().col().py(4.0);
        if self.matches.is_empty() {
            let text = if self.field.text().trim().is_empty() {
                "No recent files"
            } else {
                "No matches"
            };
            results = results.child(
                div().row().px(10.0).py(4.0).child(
                    label(text)
                        .label_size(LabelSize::Default)
                        .color(colors.text_muted),
                ),
            );
        } else {
            results = results
                .children(crate::list_scrollbar::render(
                    WIDTH - 1.0,
                    visible.min(self.matches.len()) as f32 * row_height() + 8.0,
                    self.scroll_top,
                    visible,
                    self.matches.len(),
                    self.scrollbar.opacity(),
                ))
                .children((self.scroll_top..end).map(|row| self.render_row(row, id_base)));
        }
        div()
            .col()
            .w_px(WIDTH)
            .rounded(8.0)
            .border(1.0, colors.border_variant)
            .bg(colors.elevated_surface_background)
            .child(head)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(results)
            .into()
    }

    fn render_row(&self, row: usize, id_base: u64) -> Node {
        let colors = theme();
        let Some(entry) = self.matches.get(row) else {
            return div().into();
        };
        let (folder, name) = match entry.path.rsplit_once('/') {
            Some((folder, name)) => (folder, name),
            None => ("", entry.path.as_str()),
        };
        let name_start = entry.path.chars().count() - name.chars().count();
        let name_positions: Vec<usize> = entry
            .positions
            .iter()
            .filter(|position| **position >= name_start)
            .map(|position| position - name_start)
            .collect();
        let folder_positions: Vec<usize> = entry
            .positions
            .iter()
            .copied()
            .filter(|position| *position < folder.chars().count())
            .collect();
        let mut item = div()
            .row()
            .flex(1.0)
            .px(6.0)
            .py(4.0)
            .gap(6.0)
            .items_center()
            .rounded(4.0)
            .on_click(id_base + row as u64);
        if row == self.selected {
            item = item.bg(colors.element_selected);
        } else if self.hovered == Some(id_base + row as u64) {
            item = item.bg(colors.ghost_element_hover);
        }
        let name_label = highlighted(name, &name_positions, colors.text, LabelSize::Default);
        let folder_budget = WIDTH - ROW_CHROME - ui::measure(&name_label).0;
        let (folder, folder_positions) = elide_folder(folder, &folder_positions, |text| {
            ui::measure(&label(text.to_string()).label_size(LabelSize::Small).into()).0
                <= folder_budget
        });
        let path_labels = div()
            .row()
            .flex(1.0)
            .gap(6.0)
            .items_center()
            .child(name_label)
            .child(div().row().flex(1.0).items_center().child(highlighted(
                &folder,
                &folder_positions,
                colors.text_muted,
                LabelSize::Small,
            )));
        let end: Node = if entry.recent {
            icon(IconKind::Clock)
                .size(12.0)
                .color(colors.icon_muted)
                .into()
        } else {
            div().w_px(12.0).into()
        };
        div()
            .row()
            .px(4.0)
            .child(
                item.child(material_icon(crate::file_icon(name)).size(14.0))
                    .child(path_labels)
                    .child(end),
            )
            .into()
    }
}

/// A row's padding, icons and gaps around the two labels: outer and item padding, the file icon, the recent
/// clock and the gaps between them.
const ROW_CHROME: f32 = 4.0 * 2.0 + 6.0 * 2.0 + 14.0 + 12.0 + 6.0 * 3.0;
const ELISION: &str = "...";

/// `folder` shortened until `fits`: first a run of middle folders without a match becomes `...` (the first and
/// last folders stay), then, if that is not enough, the front is cut. Match positions follow the new text.
fn elide_folder(
    folder: &str,
    positions: &[usize],
    fits: impl Fn(&str) -> bool,
) -> (String, Vec<usize>) {
    if fits(folder) {
        return (folder.to_string(), positions.to_vec());
    }
    let chars: Vec<char> = folder.chars().collect();
    let mut components: Vec<std::ops::Range<usize>> = Vec::new();
    let mut start = 0;
    for (index, c) in chars.iter().enumerate() {
        if *c == '/' {
            components.push(start..index);
            start = index + 1;
        }
    }
    components.push(start..chars.len());
    let has_match =
        |range: &std::ops::Range<usize>| positions.iter().any(|position| range.contains(position));
    let mut longest: Option<std::ops::Range<usize>> = None;
    let mut run = 0..0;
    for (index, range) in components.iter().enumerate() {
        let kept = index == 0 || index == components.len() - 1 || has_match(range);
        if kept {
            if longest.as_ref().is_none_or(|old| old.len() <= run.len()) {
                longest = Some(run.clone());
            }
            run = index + 1..index + 1;
        } else {
            run.end = index + 1;
        }
    }
    let replaced = |cut: std::ops::Range<usize>| -> (String, Vec<usize>) {
        let text: String = chars[..cut.start]
            .iter()
            .chain(ELISION.chars().collect::<Vec<_>>().iter())
            .chain(chars[cut.end..].iter())
            .collect();
        let shift = cut.len() as isize - ELISION.len() as isize;
        let moved = positions
            .iter()
            .filter(|position| !cut.contains(*position))
            .map(|&position| {
                if position >= cut.end {
                    (position as isize - shift) as usize
                } else {
                    position
                }
            })
            .collect();
        (text, moved)
    };
    let mut best = (folder.to_string(), positions.to_vec());
    if let Some(eligible) = longest.filter(|range| !range.is_empty()) {
        let middle = chars.len() / 2;
        let first = components[eligible.start].start;
        let last = components[eligible.end - 1].end;
        let from_end = first.abs_diff(middle) > last.abs_diff(middle);
        for count in 1..=eligible.len() {
            let dropped = if from_end {
                eligible.end - count..eligible.end
            } else {
                eligible.start..eligible.start + count
            };
            let cut = components[dropped.start].start..components[dropped.end - 1].end;
            best = replaced(cut);
            if fits(&best.0) {
                return best;
            }
        }
    }
    let (text, moved) = best;
    let kept: Vec<char> = text.chars().collect();
    for skip in 1..kept.len() {
        let tail: String = kept[skip..].iter().collect();
        let candidate = format!("{ELISION}{tail}");
        if fits(&candidate) || skip + 1 == kept.len() {
            let shifted = moved
                .iter()
                .filter(|position| **position >= skip)
                .map(|position| position - skip + ELISION.len())
                .collect();
            return (candidate, shifted);
        }
    }
    (text, moved)
}

fn row_height() -> f32 {
    LabelSize::Default.px() * 1.4 + 10.0
}

/// `text` with the chars at `positions` in the accent colour; long text is cut from the front.
pub(crate) fn highlighted(
    text: &str,
    positions: &[usize],
    color: ui::Rgba,
    size: LabelSize,
) -> Node {
    let accent = theme().text_accent;
    if positions.is_empty() {
        return label(text.to_string())
            .label_size(size)
            .color(color)
            .truncate_start()
            .into();
    }
    let mut row = div().row();
    let mut run = String::new();
    let mut run_matched = false;
    for (index, c) in text.chars().enumerate() {
        let matched = positions.binary_search(&index).is_ok();
        if matched != run_matched && !run.is_empty() {
            row = row.child(
                label(std::mem::take(&mut run))
                    .label_size(size)
                    .color(if run_matched { accent } else { color }),
            );
        }
        run_matched = matched;
        run.push(c);
    }
    if !run.is_empty() {
        row =
            row.child(
                label(run)
                    .label_size(size)
                    .color(if run_matched { accent } else { color }),
            );
    }
    row.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_folder_loses_its_middle_folders_before_its_front() {
        let fits = |limit: usize| move |text: &str| text.chars().count() <= limit;
        let folder = "api/spec/interactors/billing/invoice_form";
        assert_eq!(elide_folder(folder, &[], fits(100)).0, folder);
        let matched = folder.find("invoice").unwrap();
        let (text, positions) = elide_folder(folder, &[matched], fits(28));
        assert_eq!(text, "api/spec/.../invoice_form");
        assert_eq!(positions, [text.find("invoice").unwrap()]);
        let (text, positions) = elide_folder(folder, &[matched], fits(16));
        assert!(
            text.starts_with("...") && text.chars().count() <= 20,
            "{text}"
        );
        assert_eq!(positions, [text.find("invoice").unwrap()]);
    }

    #[test]
    fn queries_carry_a_line_and_column() {
        let target = |path: &str, row: Option<u32>, column: Option<u32>| Target {
            path: path.into(),
            row,
            column,
        };
        assert_eq!(
            parse_query("src/main.rs"),
            target("src/main.rs", None, None)
        );
        assert_eq!(parse_query("main.rs:12"), target("main.rs", Some(12), None));
        assert_eq!(
            parse_query("main.rs:12:5"),
            target("main.rs", Some(12), Some(5))
        );
        assert_eq!(
            parse_query("./main.rs:3:"),
            target("main.rs", Some(3), None)
        );
        assert_eq!(parse_query("main"), target("main", None, None));
    }

    fn finder() -> FileFinder {
        let candidates = Candidates::new(
            [
                "src/main.rs",
                "src/lib.rs",
                "docs/main-guide.md",
                "Cargo.toml",
            ]
            .map(str::to_string)
            .to_vec(),
        );
        FileFinder::new(
            Arc::new(candidates),
            vec!["docs/main-guide.md".into(), "src/lib.rs".into()],
            Some("src/lib.rs".into()),
        )
    }

    #[test]
    fn an_empty_query_lists_recent_files_with_the_open_one_first() {
        let finder = finder();
        let paths: Vec<&str> = finder
            .matches()
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths, ["src/lib.rs", "docs/main-guide.md"]);
        assert!(finder.matches().iter().all(|entry| entry.recent));
    }

    #[test]
    fn a_query_puts_recent_hits_first_and_confirms_with_its_line() {
        let mut finder = finder();
        finder.field.insert("main:40");
        finder.update_matches();
        let paths: Vec<&str> = finder
            .matches()
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths.first(), Some(&"docs/main-guide.md"));
        assert!(paths.contains(&"src/main.rs"));
        assert_eq!(
            paths
                .iter()
                .filter(|path| **path == "docs/main-guide.md")
                .count(),
            1
        );
        assert_eq!(
            finder.confirm(),
            Some(Target {
                path: "docs/main-guide.md".into(),
                row: Some(40),
                column: None
            })
        );
    }

    #[test]
    fn a_search_does_not_start_on_the_open_file() {
        let mut finder = finder();
        finder.field.insert("lib");
        finder.update_matches();
        assert_eq!(
            finder.matches().first().map(|entry| entry.path.as_str()),
            Some("src/lib.rs")
        );
        assert_eq!(finder.selected, 0, "only one match: nothing to skip to");
        finder.field.set_text("rs");
        finder.update_matches();
        assert_eq!(
            finder.matches().first().map(|entry| entry.path.as_str()),
            Some("src/lib.rs")
        );
        assert_eq!(finder.selected, 1);
    }
}
