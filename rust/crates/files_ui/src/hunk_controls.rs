//! The strip of buttons over an expanded change the caret is in: stage or unstage it, restore it, or step to the
//! next or previous change. It sits at the change's top-right and sticks to the top while the change scrolls past.

use git::DiffHunk;
use ui::{div, theme, IconKind, Node, Rect};

use crate::{edit_line_h, FileItem, HUNK_CONTROL_BASE};

const STAGE: u64 = 0;
const UNSTAGE: u64 = 1;
const RESTORE: u64 = 2;
const NEXT: u64 = 3;
const PREVIOUS: u64 = 4;
pub(crate) const CONTROL_COUNT: u64 = 5;

/// Keeps the strip clear of the editor's right edge and its scrollbar.
const RIGHT_MARGIN: f32 = 18.0;
const BUTTON_PAD_X: f32 = 4.0;
const BUTTON_GAP: f32 = 4.0;
const ARROW_W: f32 = 18.0;
const TEXT_SIZE: f32 = 11.0;

impl FileItem {
    /// The expanded change holding the newest caret, when its staging can change.
    fn hunk_under_caret(&self) -> Option<DiffHunk> {
        if self.branch_diff || self.read_only || self.scratch.is_some() || self.split_active {
            return None;
        }
        let buffer = self.buffer.as_ref()?;
        let line = buffer
            .rope
            .char_to_line(buffer.newest().head().min(buffer.rope.len_chars()));
        self.git
            .hunks()
            .iter()
            .find(|hunk| {
                let holds =
                    hunk.rows.contains(&line) || (hunk.rows.is_empty() && hunk.rows.start == line);
                holds && self.is_expanded(hunk)
            })
            .cloned()
    }

    pub(crate) fn hunk_controls(&self, content: Rect) -> Option<(Node, f32, f32)> {
        let hunk = self.hunk_under_caret()?;
        let created = self.git.bases()?.head.is_none();
        let scale = ui::ui_text_scale();
        let line_h = edit_line_h();
        let removed = if self.base.is_some() {
            hunk.base_rows.len()
        } else {
            0
        };
        let top_row = self.disp_of(hunk.rows.start).saturating_sub(removed);
        let bottom_row = if hunk.rows.end >= self.line_count() {
            self.disp_count()
        } else {
            self.disp_of(hunk.rows.end)
        };
        let top = content.y + top_row as f32 * line_h - self.scroll_y;
        let bottom = content.y + bottom_row.max(top_row + 1) as f32 * line_h - self.scroll_y;
        if bottom <= content.y || top >= content.y + self.body_h {
            return None;
        }
        let y = if top >= content.y {
            top
        } else {
            content.y.min(bottom - line_h)
        };

        let stage_id = if hunk.staged { UNSTAGE } else { STAGE };
        let verb = if hunk.staged { "Unstage" } else { "Stage" };
        let stage_text = match self.selected_change_lines(!hunk.staged) {
            Some((_, 1)) => format!("{verb} 1 line"),
            Some((_, count)) => format!("{verb} {count} lines"),
            None => format!("{verb} hunk"),
        };
        let mut buttons = vec![
            self.hunk_button(stage_id, &stage_text, !hunk.staged, true),
            self.hunk_button(RESTORE, "Restore", false, !created),
        ];
        let mut width = [stage_text.as_str(), "Restore"]
            .iter()
            .map(|text| ui::measure_text_width(text, TEXT_SIZE, false, 500) + 2.0 * BUTTON_PAD_X)
            .sum::<f32>();
        if self.git.hunks().len() > 1 {
            buttons.push(self.hunk_arrow(NEXT, IconKind::ArrowDown));
            buttons.push(self.hunk_arrow(PREVIOUS, IconKind::ArrowUp));
            width += 2.0 * ARROW_W;
        }
        width += BUTTON_GAP * (buttons.len() as f32 - 1.0) + 2.0 * BUTTON_GAP + 2.0;
        let x = content.x + content.w - RIGHT_MARGIN * scale - width * scale;
        if x < content.x {
            return None;
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
        for button in buttons {
            strip = strip.child(button);
        }
        Some((strip.into(), x, y))
    }

    fn hunk_button(&self, control: u64, text: &str, primary: bool, enabled: bool) -> Node {
        components::button(HUNK_CONTROL_BASE + control, text)
            .size(components::ButtonSize::XSmall)
            .kind(if primary {
                components::ButtonKind::Primary
            } else {
                components::ButtonKind::Secondary
            })
            .enabled(enabled)
            .hovered(self.hunk_control_hover)
            .into()
    }

    fn hunk_arrow(&self, control: u64, kind: IconKind) -> Node {
        components::icon_button(HUNK_CONTROL_BASE + control, kind)
            .size(components::ButtonSize::XSmall)
            .hovered(self.hunk_control_hover)
            .into()
    }

    /// A click on the strip; returns whether `id` was one of its controls.
    pub(crate) fn click_hunk_control(&mut self, id: u64) -> bool {
        let Some(control) = id.checked_sub(HUNK_CONTROL_BASE) else {
            return false;
        };
        if control >= CONTROL_COUNT {
            return false;
        }
        let Some(hunk) = self.hunk_under_caret() else {
            return true;
        };
        let stage = control == STAGE;
        if matches!(control, STAGE | UNSTAGE) {
            if let Some((rows, _)) = self.selected_change_lines(stage) {
                self.write_staged_lines(rows, stage);
                return true;
            }
        }
        match control {
            STAGE => self.write_staged(&[hunk], true),
            UNSTAGE => self.write_staged(&[hunk], false),
            RESTORE => self.restore_hunk_list(vec![hunk]),
            NEXT => self.go_to_hunk(true),
            _ => self.go_to_hunk(false),
        }
        true
    }
}
