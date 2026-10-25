//! Merge conflicts in the open file: each side tinted, and over each conflict a strip to keep one side, the
//! other, or both, or to hand the file to a side agent to merge.

use git::conflict::{self, Conflict, Side};
use ui::{div, theme, IconKind, Node, Rect};
use workspace::{AgentFix, PanelRequest};

use crate::{edit_line_h, FileItem, CONFLICT_CONTROL_BASE};

const USE_OURS: u64 = 0;
const USE_THEIRS: u64 = 1;
const USE_BOTH: u64 = 2;
const ASK_AGENT: u64 = 3;
const PER_CONFLICT: u64 = 4;
/// Ids past this many conflicts would run into the next id range.
pub(crate) const MAX_CONFLICTS: u64 = 20_000;

const RIGHT_MARGIN: f32 = 18.0;
const BUTTON_PAD_X: f32 = 4.0;
const BUTTON_GAP: f32 = 4.0;
const TEXT_SIZE: f32 = 11.0;
const ASK_LABEL: &str = "Ask Claude to merge";

impl FileItem {
    /// The conflicts in the buffer, parsed again only after an edit.
    pub(crate) fn conflicts(&self) -> Vec<Conflict> {
        let Some(buffer) = self.buffer.as_ref() else {
            return Vec::new();
        };
        if self.read_only || self.scratch.is_some() || self.branch_diff {
            return Vec::new();
        }
        let version = buffer.version();
        let mut cache = self.conflict_cache.borrow_mut();
        if let Some((_, found)) = cache.as_ref().filter(|(seen, _)| *seen == version) {
            return found.clone();
        }
        let text = buffer.text();
        let found = if text.contains("<<<<<<<") {
            conflict::conflicts(&text)
        } else {
            Vec::new()
        };
        *cache = Some((version, found.clone()));
        found
    }

    /// Ours (with its marker) and theirs (with its marker) tinted apart; the base of a diff3 conflict stays plain.
    pub(crate) fn conflict_rects(&self, content: Rect, first: usize, last: usize) -> Vec<Rect> {
        let colors = theme();
        let row_y = |row: usize| content.y + row as f32 * edit_line_h() - self.scroll_y;
        let mut rects = Vec::new();
        let mut tint = |lines: std::ops::Range<usize>, color: ui::Rgba| {
            if lines.is_empty() {
                return;
            }
            let top = self.disp_of(lines.start);
            let bottom = self.last_row_of_line(lines.end - 1);
            if top >= last || bottom < first {
                return;
            }
            rects.push(Rect::new(
                content.x,
                row_y(top),
                content.w,
                (bottom + 1 - top) as f32 * edit_line_h(),
                color,
            ));
        };
        for found in self.conflicts() {
            let ours_end = found
                .base
                .as_ref()
                .map_or(found.ours.end, |base| base.start - 1);
            tint(found.rows.start..ours_end, colors.success.alpha(0.1));
            tint(
                found.theirs.start - 1..found.rows.end,
                colors.info.alpha(0.1),
            );
        }
        rects
    }

    pub(crate) fn conflict_controls(&self, content: Rect) -> Vec<(Node, f32, f32)> {
        let scale = ui::ui_text_scale();
        let line_h = edit_line_h();
        let mut strips = Vec::new();
        for (index, found) in self
            .conflicts()
            .iter()
            .enumerate()
            .take(MAX_CONFLICTS as usize)
        {
            let top = content.y + self.disp_of(found.rows.start) as f32 * line_h - self.scroll_y;
            let bottom = content.y
                + (self.last_row_of_line(found.rows.end - 1) + 1) as f32 * line_h
                - self.scroll_y;
            if bottom <= content.y || top >= content.y + self.body_h {
                continue;
            }
            let y = if top >= content.y {
                top
            } else {
                content.y.min(bottom - line_h)
            };
            let base = CONFLICT_CONTROL_BASE + index as u64 * PER_CONFLICT;
            let labels = [
                (USE_OURS, format!("Use {}", found.ours_branch)),
                (USE_THEIRS, format!("Use {}", found.theirs_branch)),
                (USE_BOTH, "Use Both".to_string()),
            ];
            let mut width = labels
                .iter()
                .map(|(_, text)| ui::measure_text_width(text, TEXT_SIZE, false, 500))
                .sum::<f32>()
                + ui::measure_text_width(ASK_LABEL, TEXT_SIZE, false, 500)
                + 11.0
                + 6.0
                + 4.0 * 2.0 * BUTTON_PAD_X
                + 5.0 * BUTTON_GAP
                + 2.0;
            width = width.ceil();
            let x = content.x + content.w - RIGHT_MARGIN * scale - width * scale;
            if x < content.x {
                continue;
            }
            let colors = theme();
            let mut strip = div()
                .row()
                .w_px(width)
                .h_px(line_h / scale)
                .px(BUTTON_GAP)
                .gap(BUTTON_GAP)
                .items_center()
                .rounded(6.0)
                .bg(colors.editor_background)
                .border(1.0, colors.border_variant);
            for (control, text) in labels {
                strip = strip.child(self.conflict_button(base + control, &text, false));
            }
            strip = strip.child(self.conflict_button(base + ASK_AGENT, ASK_LABEL, true));
            strips.push((strip.into(), x, y));
        }
        strips
    }

    fn conflict_button(&self, id: u64, text: &str, agent: bool) -> Node {
        let button = components::button(id, text)
            .size(components::ButtonSize::XSmall)
            .hovered(self.hunk_control_hover);
        if agent {
            button
                .kind(components::ButtonKind::Agent)
                .icon(Some(IconKind::Sparkle))
                .into()
        } else {
            button.into()
        }
    }

    pub(crate) fn is_conflict_control(id: u64) -> bool {
        (CONFLICT_CONTROL_BASE..CONFLICT_CONTROL_BASE + MAX_CONFLICTS * PER_CONFLICT).contains(&id)
    }

    /// A click on a conflict strip; returns whether `id` was one of its controls.
    pub(crate) fn click_conflict_control(&mut self, id: u64) -> bool {
        if !Self::is_conflict_control(id) {
            return false;
        }
        let offset = id - CONFLICT_CONTROL_BASE;
        let conflicts = self.conflicts();
        let Some(found) = conflicts.get((offset / PER_CONFLICT) as usize) else {
            return true;
        };
        let side = match offset % PER_CONFLICT {
            USE_OURS => Side::Ours,
            USE_THEIRS => Side::Theirs,
            USE_BOTH => Side::Both,
            _ => {
                self.ask_agent_to_merge();
                return true;
            }
        };
        self.resolve_conflict(found, side);
        true
    }

    fn resolve_conflict(&mut self, found: &Conflict, side: Side) {
        let Some(buffer) = self.buffer.as_mut() else {
            return;
        };
        let text = buffer.text();
        let line_start = |line: usize| {
            if line >= buffer.rope.len_lines() {
                buffer.rope.len_chars()
            } else {
                buffer.rope.line_to_char(line)
            }
        };
        let range = line_start(found.rows.start)..line_start(found.rows.end);
        let kept = conflict::resolution(&text, found, side);
        buffer.replace_ranges(vec![(range, kept)]);
        self.refresh();
        self.ensure_visible();
        self.ensure_cursor_visible();
    }

    fn ask_agent_to_merge(&mut self) {
        let prompt = conflict::merge_prompt(std::slice::from_ref(&self.path));
        self.panel_requests
            .push(PanelRequest::FixWithAgent(AgentFix {
                prompt,
                cwd: self.root.clone(),
            }));
    }
}
