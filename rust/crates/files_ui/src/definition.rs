//! Going to where a symbol is defined: F12 and its variants at the caret, and a held command key turning the
//! symbol under the pointer into a link (underlined in the link color) that a click follows.

use std::ops::Range;

use lsp::{DefinitionKind, DefinitionTarget};
use ui::{theme, Rect, Rgba};

use crate::{FileItem, EDIT_LINE_H, TAB_COLS};

pub(crate) struct PendingDefinition {
    offset: usize,
    kind: DefinitionKind,
    request: Option<u64>,
}

pub(crate) struct LinkState {
    trigger: usize,
    kind: DefinitionKind,
    symbol_range: Option<Range<usize>>,
    targets: Vec<DefinitionTarget>,
    request: Option<u64>,
    sent: bool,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl FileItem {
    pub(crate) fn go_to_definition(&mut self, kind: DefinitionKind) {
        let Some(caret) = self.buffer.as_ref().map(|b| b.newest().head()) else {
            return;
        };
        self.definition_request = Some(PendingDefinition {
            offset: caret,
            kind,
            request: None,
        });
    }

    fn surrounding_word(&self, offset: usize) -> Option<Range<usize>> {
        let b = self.buffer.as_ref()?;
        let (mut start, mut end) = (offset, offset);
        while start > 0 && is_word_char(b.rope.char(start - 1)) {
            start -= 1;
        }
        while end < b.rope.len_chars() && is_word_char(b.rope.char(end)) {
            end += 1;
        }
        (start < end).then_some(start..end)
    }

    /// With the command key held over text: ask where the symbol at `offset` leads, unless the link already
    /// covers it. Shift asks for its type instead.
    pub(crate) fn show_link_definition(&mut self, offset: usize, type_definition: bool) {
        let kind = if type_definition {
            DefinitionKind::TypeDefinition
        } else {
            DefinitionKind::Definition
        };
        if let Some(link) = self.link.as_ref() {
            let covers = link
                .symbol_range
                .as_ref()
                .is_some_and(|range| range.start <= offset && offset <= range.end);
            if link.kind == kind && (link.trigger == offset || covers) {
                return;
            }
        }
        self.link = Some(LinkState {
            trigger: offset,
            kind,
            symbol_range: None,
            targets: Vec::new(),
            request: None,
            sent: false,
        });
    }

    pub(crate) fn hide_hovered_link(&mut self) -> bool {
        self.link.take().is_some()
    }

    /// A definition request waiting to be sent: the caret's first, then the link's.
    pub(crate) fn definition_request_due(&self) -> Option<(usize, DefinitionKind)> {
        if let Some(pending) = self
            .definition_request
            .as_ref()
            .filter(|p| p.request.is_none())
        {
            return Some((pending.offset, pending.kind));
        }
        let link = self.link.as_ref().filter(|link| !link.sent)?;
        Some((link.trigger, link.kind))
    }

    pub(crate) fn definition_requested(&mut self, token: Option<u64>) {
        if let Some(pending) = self
            .definition_request
            .as_mut()
            .filter(|p| p.request.is_none())
        {
            match token {
                Some(token) => pending.request = Some(token),
                None => self.definition_request = None,
            }
            return;
        }
        if let Some(link) = self.link.as_mut() {
            link.sent = true;
            link.request = token;
        }
    }

    /// Targets other than the one the offset is already on, which going there would not move from.
    fn excluding_here(&self, targets: &[DefinitionTarget], offset: usize) -> Vec<DefinitionTarget> {
        let here = self.root.join(&self.path);
        targets
            .iter()
            .filter(|target| {
                let (true, Some(b)) = (target.path == here, self.buffer.as_ref()) else {
                    return true;
                };
                let start = lsp::position_to_char(&b.rope, target.range.start);
                let end = lsp::position_to_char(&b.rope, target.range.end);
                !(start <= offset && offset <= end)
            })
            .cloned()
            .collect()
    }

    /// Take the server's answer: targets to go to for a caret request, or the hovered link's targets and
    /// the symbol range to underline.
    pub(crate) fn definitions_answered(
        &mut self,
        response: &lsp::DefinitionsResponse,
    ) -> Option<Vec<DefinitionTarget>> {
        if let Some(pending) = self
            .definition_request
            .as_ref()
            .filter(|p| p.request == Some(response.request))
        {
            let offset = pending.offset;
            self.definition_request = None;
            let targets = self.excluding_here(&response.targets, offset);
            return (!targets.is_empty()).then_some(targets);
        }
        let origin = response.origin.clone().map(|range| {
            let mut range = range;
            if let Some(b) = self.buffer.as_ref() {
                for batch in b.edits_since(response.synced.buffer_version) {
                    range.start =
                        editor::buffer::map_offset(batch, range.start, editor::buffer::Bias::Left);
                    range.end =
                        editor::buffer::map_offset(batch, range.end, editor::buffer::Bias::Right);
                }
            }
            range
        });
        let trigger = self.link.as_ref()?.trigger;
        let fallback = self.surrounding_word(trigger);
        let link = self
            .link
            .as_mut()
            .filter(|link| link.request == Some(response.request))?;
        link.targets = response.targets.clone();
        link.symbol_range = if link.targets.is_empty() {
            origin
        } else {
            origin.or(fallback)
        };
        None
    }

    /// The link to underline: the hovered symbol, once the server says it leads somewhere.
    pub(crate) fn link_range(&self) -> Option<Range<usize>> {
        let link = self.link.as_ref()?;
        if link.targets.is_empty() {
            return None;
        }
        link.symbol_range.clone()
    }

    /// A command-click at `local`: the hovered link's targets when it has them, else a request for the
    /// clicked symbol's definition, the caret placed there.
    pub(crate) fn definition_click(
        &mut self,
        local_x: f32,
        local_y: f32,
    ) -> Option<Vec<DefinitionTarget>> {
        let offset = self.hover_offset(local_x, local_y);
        if let (Some(link), Some(offset)) = (self.link.take(), offset) {
            if !link.targets.is_empty() {
                let targets = self.excluding_here(&link.targets, offset);
                return (!targets.is_empty()).then_some(targets);
            }
        }
        let offset = offset?;
        if let Some(b) = self.buffer.as_mut() {
            b.place_cursor(offset);
        }
        let kind = if ui::modifiers().shift {
            DefinitionKind::TypeDefinition
        } else {
            DefinitionKind::Definition
        };
        self.definition_request = Some(PendingDefinition {
            offset,
            kind,
            request: None,
        });
        None
    }

    /// The caret's distance below the top of the view, to show a jump's target at the same height.
    pub(crate) fn caret_top(&self) -> Option<f32> {
        let b = self.buffer.as_ref()?;
        let (row, _) = self.position(b.newest().head());
        Some(row as f32 * EDIT_LINE_H - self.scroll_y)
    }

    /// Select a target range (just its start when it spans lines) and scroll it to `caret_top`.
    pub(crate) fn select_target_range(
        &mut self,
        range: lsp::lsp_types::Range,
        caret_top: Option<f32>,
    ) {
        self.refresh();
        self.ensure_visible();
        let Some(b) = self.buffer.as_mut() else {
            return;
        };
        let start = lsp::position_to_char(&b.rope, range.start);
        let end = if range.start.line == range.end.line {
            lsp::position_to_char(&b.rope, range.end)
        } else {
            start
        };
        b.place_cursor(start);
        if end > start {
            b.extend_cursor(end);
        }
        let (row, _) = self.position(start);
        match caret_top {
            Some(top) => self.set_scroll_y(row as f32 * EDIT_LINE_H - top),
            None => {
                let line = self
                    .buffer
                    .as_ref()
                    .map_or(0, |b| b.rope.char_to_line(start));
                self.scroll_line_to_center(line);
            }
        }
        self.ensure_cursor_visible();
    }

    /// A thin underline in the link color under the link, row by row.
    pub(crate) fn link_underline_rects(
        &self,
        content: Rect,
        first: usize,
        last: usize,
    ) -> Vec<Rect> {
        let Some(range) = self.link_range() else {
            return Vec::new();
        };
        let gw = crate::gutter_width(self.line_count());
        let (start_row, start_x) = self.position(range.start);
        let (end_row, end_x) = self.position(range.end);
        let mut rects = Vec::new();
        for row in start_row.max(first)..=end_row.min(last.saturating_sub(1)) {
            let left = if row == start_row { start_x } else { 0.0 };
            let right = if row == end_row {
                end_x
            } else {
                self.row_width(row)
            };
            if right > left {
                rects.push(Rect::new(
                    content.x + gw + left - self.scroll_x,
                    content.y + row as f32 * EDIT_LINE_H - self.scroll_y
                        + crate::DIAGNOSTIC_UNDERLINE_TOP,
                    right - left,
                    1.0,
                    theme().link_text_hover,
                ));
            }
        }
        rects
    }

    /// `segments` of buffer `line` with the link's chars in the link color.
    pub(crate) fn with_link_color(
        &self,
        line: usize,
        segments: Vec<(String, Rgba)>,
    ) -> Vec<(String, Rgba)> {
        let (Some(range), Some(b)) = (self.link_range(), self.buffer.as_ref()) else {
            return segments;
        };
        if line >= b.rope.len_lines() {
            return segments;
        }
        let line_start = b.rope.line_to_char(line);
        let line_end = line_start + b.line_len(line);
        if range.end <= line_start || range.start >= line_end {
            return segments;
        }
        let slice = b.rope.line(line);
        let from =
            editor::display::to_display(slice, range.start.max(line_start) - line_start, TAB_COLS)
                .0;
        let to =
            editor::display::to_display(slice, range.end.min(line_end) - line_start, TAB_COLS).0;
        let color = theme().link_text_hover;
        let mut recolored: Vec<(String, Rgba)> = Vec::new();
        let mut column = 0;
        for (text, segment_color) in segments {
            for c in text.chars() {
                let paint = if (from..to).contains(&column) {
                    color
                } else {
                    segment_color
                };
                match recolored.last_mut() {
                    Some((run, run_color)) if *run_color == paint => run.push(c),
                    _ => recolored.push((c.to_string(), paint)),
                }
                column += 1;
            }
        }
        recolored
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilesView;
    use lsp::lsp_types::{Position, Range as LspRange};
    use std::path::PathBuf;
    use workspace::pane_group::Member;
    use workspace::{EditKey, Item};

    fn item(text: &str) -> FileItem {
        let mut item = FileItem::new(PathBuf::from("/project"), "a.rs", Some(text.into()));
        item.set_body_height(10.0 * EDIT_LINE_H);
        item.ensure_visible();
        item
    }

    fn range(line: u32, start: u32, end: u32) -> LspRange {
        LspRange::new(Position::new(line, start), Position::new(line, end))
    }

    fn response(
        item: &FileItem,
        request: u64,
        origin: Option<Range<usize>>,
        targets: Vec<DefinitionTarget>,
    ) -> lsp::DefinitionsResponse {
        let b = item.buffer.as_ref().unwrap();
        lsp::DefinitionsResponse {
            request,
            origin,
            targets,
            synced: lsp::SyncedText {
                lsp_version: 0,
                buffer_version: b.version(),
                rope: b.rope.clone(),
            },
        }
    }

    #[test]
    fn f12_asks_at_the_caret_and_skips_targets_it_is_already_on() {
        let mut item = item("fn helper() {}\nfn main() { helper(); }\n");
        item.buffer.as_mut().unwrap().place_cursor(29);
        item.input_key(EditKey::GoToDefinition, false);
        assert_eq!(
            item.definition_request_due(),
            Some((29, DefinitionKind::Definition))
        );
        item.definition_requested(Some(4));
        let here = DefinitionTarget {
            path: PathBuf::from("/project/a.rs"),
            range: range(1, 12, 18),
        };
        let there = DefinitionTarget {
            path: PathBuf::from("/project/a.rs"),
            range: range(0, 3, 9),
        };
        let answer = response(&item, 4, None, vec![here, there.clone()]);
        assert_eq!(item.definitions_answered(&answer), Some(vec![there]));
        assert!(item.definition_request_due().is_none());
    }

    #[test]
    fn a_held_command_key_links_the_symbol_once_the_server_answers() {
        let mut item = item("fn helper() {}\nfn main() { helper(); }\n");
        item.show_link_definition(30, false);
        assert_eq!(
            item.definition_request_due(),
            Some((30, DefinitionKind::Definition))
        );
        item.definition_requested(Some(1));
        assert!(item.link_range().is_none());
        let target = DefinitionTarget {
            path: PathBuf::from("/project/a.rs"),
            range: range(0, 3, 9),
        };
        let answer = response(&item, 1, None, vec![target]);
        assert!(item.definitions_answered(&answer).is_none());
        assert_eq!(item.link_range(), Some(27..33));
        // Moving within the symbol keeps the link without asking again.
        item.show_link_definition(28, false);
        assert!(item.definition_request_due().is_none());
        let colors = item.with_link_color(
            1,
            vec![("fn main() { helper(); }".into(), Rgba::TRANSPARENT)],
        );
        assert_eq!(colors[1], ("helper".to_string(), theme().link_text_hover));
        let content = Rect::new(0.0, 0.0, 800.0, 240.0, Rgba::TRANSPARENT);
        assert_eq!(item.link_underline_rects(content, 0, 3).len(), 1);
        assert!(item.hide_hovered_link());
        assert!(item.link_range().is_none());
    }

    #[test]
    fn a_command_click_without_a_link_asks_at_the_click() {
        let mut item = item("fn helper() {}\nfn main() { helper(); }\n");
        let x = crate::gutter_width(item.line_count()) + 14.25 * crate::char_advance();
        let y = EDIT_LINE_H * 1.5;
        assert!(item.definition_click(x, y).is_none());
        assert_eq!(item.buffer.as_ref().unwrap().newest().head(), 29);
        assert_eq!(
            item.definition_request_due(),
            Some((29, DefinitionKind::Definition))
        );
    }

    #[test]
    fn going_to_a_target_opens_its_file_and_selects_it() {
        let root = std::env::temp_dir().join(format!("pomelo-definition-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), "fn main() { helper(); }\n").unwrap();
        std::fs::write(root.join("b.rs"), "\npub fn helper() {}\n").unwrap();
        let mut view = FilesView::new(root.clone());
        let mut first = FileItem::new(
            root.clone(),
            "a.rs",
            Some("fn main() { helper(); }\n".into()),
        );
        first.set_body_height(10.0 * EDIT_LINE_H);
        if let Member::Leaf(pane) = &mut view.panes.group {
            pane.open.push(Box::new(first));
            pane.active = Some(0);
        }
        let target = DefinitionTarget {
            path: root.join("b.rs"),
            range: range(1, 7, 13),
        };
        crate::open_definition(&root, &mut view.panes, &[target], Some(0.0));
        let active = view.panes.active.clone();
        let opened = view.go_to_line_item(&active).unwrap();
        assert_eq!(opened.path, "b.rs");
        let selection = opened.buffer.as_ref().unwrap().newest();
        assert_eq!((selection.start, selection.end), (8, 14));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_definition_found_in_another_group_opens_there() {
        let root =
            std::env::temp_dir().join(format!("pomelo-definition-other-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("b.rs"), "\npub fn helper() {}\n").unwrap();
        let mut view = FilesView::new(root.clone());
        let mut other = workspace::pane_group_view::PaneGroupView::new(
            workspace::pane_group_view::PaneGroupConfig {
                id_base: 0,
                show_nav: false,
                buttons: Vec::new(),
                max_panes: 2,
            },
        );
        let mut first = FileItem::new(root.clone(), "a.rs", Some("helper();\n".into()));
        first.navigation = Some((
            vec![DefinitionTarget {
                path: root.join("b.rs"),
                range: range(1, 7, 13),
            }],
            Some(0.0),
        ));
        if let Some(pane) = other.active_pane_mut() {
            pane.add_item(Box::new(first));
        }
        workspace::FunctionView::sync_items(&mut view, Some(&mut other));
        let opened = other
            .active_item()
            .and_then(|item| item.id())
            .unwrap_or_default();
        assert_eq!(opened, "b.rs");
        assert!(view.panes.active_item().is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
