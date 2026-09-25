//! A markdown file rendered beside (or instead of) its editor. The files view hands it the editor's text
//! whenever the buffer changes and where the cursor is; the preview parses again once edits pause and scrolls
//! to the block the cursor is in.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use markdown::{Markdown, MarkdownLayout, DOCUMENT_STYLE};
use terminal::Modifiers;
use ui::{div, theme, IconKind, Node, Rect, Rgba};
use workspace::{Item, ItemTick};

const REPARSE_DELAY: Duration = Duration::from_millis(200);
const PAD: f32 = 24.0;
const MAX_WIDTH: f32 = 860.0;
const LINK_BASE: u64 = 1;

pub(crate) fn preview_id(path: &str) -> String {
    format!("markdown-preview:{path}")
}

pub(crate) fn is_markdown(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".mdx")
}

pub(crate) struct MarkdownPreview {
    root: PathBuf,
    /// Relative to `root`, like the editor item it follows.
    pub(crate) path: String,
    name: String,
    markdown: Markdown,
    /// Buffer version the preview last took text from.
    pub(crate) version: Option<u64>,
    pending: Option<(Instant, String)>,
    /// Cursor byte offset in the source; scrolled to once when it moves to another block.
    cursor: Option<usize>,
    followed_block: Option<usize>,
    layout: Option<(u32, MarkdownLayout)>,
    scroll: f32,
    body_h: f32,
    hits: Vec<(Rect, u64)>,
    error: Option<String>,
}

impl MarkdownPreview {
    pub(crate) fn new(
        root: PathBuf,
        path: String,
        text: &str,
        version: Option<u64>,
    ) -> MarkdownPreview {
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        MarkdownPreview {
            root,
            path,
            name,
            markdown: Markdown::parse(text),
            version,
            pending: None,
            cursor: None,
            followed_block: None,
            layout: None,
            scroll: 0.0,
            body_h: 0.0,
            hits: Vec::new(),
            error: None,
        }
    }

    /// New editor text; parsed once no edit has come for `REPARSE_DELAY`.
    pub(crate) fn text_changed(&mut self, text: String, version: u64) {
        self.version = Some(version);
        self.pending = Some((Instant::now(), text));
    }

    pub(crate) fn cursor_moved(&mut self, byte: usize) {
        self.cursor = Some(byte);
    }

    fn scroll_to_block(&mut self, block: usize) {
        if let Some((_, layout)) = &self.layout {
            if let Some(offset) = layout.offset_of_block(block) {
                self.scroll = offset.min(self.max_scroll()).max(0.0);
            }
        }
    }

    fn max_scroll(&self) -> f32 {
        self.layout.as_ref().map_or(0.0, |(_, layout)| {
            (layout.height() + 2.0 * PAD - self.body_h).max(0.0)
        })
    }

    fn follow_cursor(&mut self) {
        let Some(block) = self
            .cursor
            .and_then(|byte| self.markdown.block_at_source(byte))
        else {
            return;
        };
        if self.followed_block == Some(block) {
            return;
        }
        self.followed_block = Some(block);
        self.scroll_to_block(block);
    }

    fn open_link(&mut self, url: &str) {
        if url.starts_with('#') {
            if let Some(block) = self.markdown.heading_block(url) {
                self.scroll_to_block(block);
            }
            return;
        }
        let target = if url.contains("://") || url.starts_with("mailto:") {
            url.to_string()
        } else {
            let base = self
                .root
                .join(&self.path)
                .parent()
                .map_or_else(|| self.root.clone(), PathBuf::from);
            base.join(url.split('#').next().unwrap_or(url))
                .to_string_lossy()
                .into_owned()
        };
        if let Err(error) = std::process::Command::new("open").arg(&target).spawn() {
            self.error = Some(format!("Failed to open {target}: {error}"));
        }
    }
}

impl Item for MarkdownPreview {
    fn id(&self) -> Option<String> {
        Some(preview_id(&self.path))
    }

    fn title(&self) -> String {
        format!("Preview {}", self.name)
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Eye)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        self.body_h = body.h / scale;
        let full = body.w / scale;
        let width = (full - 2.0 * PAD).clamp(120.0, MAX_WIDTH);
        let key = width.to_bits();
        if self.layout.as_ref().is_none_or(|(at, _)| *at != key) {
            self.layout = Some((key, self.markdown.layout(width, DOCUMENT_STYLE)));
            self.followed_block = None;
        }
        self.follow_cursor();
        self.scroll = self.scroll.min(self.max_scroll());
        let (_, layout) = self.layout.as_ref()?;
        let top = (self.scroll - PAD).max(0.0);
        let first = layout.line_at(top);
        let count = layout.lines_fitting(first, self.body_h + PAD * 2.0) + 1;
        let skipped = layout.height_of(0, first);
        let colors = theme();
        let mut content = div()
            .col()
            .w_px(full)
            .px(((full - width) / 2.0).max(PAD))
            .child(div().h_px(PAD + skipped));
        if self.markdown.is_empty() {
            content = content.child(ui::label("Nothing to preview").color(colors.text_muted));
        } else {
            content = content.child(layout.render_links(first, count, width, Some(LINK_BASE)));
        }
        if let Some(error) = &self.error {
            content = content.child(ui::label(error.clone()).color(colors.error));
        }
        let tree: Node = div()
            .col()
            .w_px(full)
            .bg(colors.editor_background)
            .child(content)
            .into();
        // The column starts `scroll` above the body; the pane clips what falls outside.
        let shifted = Rect::new(
            body.x,
            body.y - self.scroll * scale,
            body.w,
            body.h + self.scroll * scale,
            Rgba::TRANSPARENT,
        );
        let painted = ui::render(&tree, shifted);
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn pointer_down(&mut self, x: f32, y: f32, _click_count: u32, _modifiers: Modifiers) -> bool {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id);
        if let Some(id) = hit.filter(|id| *id >= LINK_BASE) {
            if let Some(url) = self
                .markdown
                .links()
                .get((id - LINK_BASE) as usize)
                .cloned()
            {
                self.open_link(&url);
            }
        }
        true
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let next = (self.scroll - delta_y).clamp(0.0, self.max_scroll());
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let due = self
            .pending
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= REPARSE_DELAY);
        if !due {
            return ItemTick::default();
        }
        if let Some((_, text)) = self.pending.take() {
            self.markdown = Markdown::parse(&text);
            self.layout = None;
        }
        ItemTick {
            changed: true,
            ..ItemTick::default()
        }
    }

    fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn painted_texts(preview: &mut MarkdownPreview) -> Vec<String> {
        preview
            .paint_body(Rect::new(0.0, 0.0, 700.0, 400.0, Rgba::TRANSPARENT), true)
            .map(|painted| painted.texts.into_iter().map(|text| text.text).collect())
            .unwrap_or_default()
    }

    #[test]
    fn edits_show_after_a_pause_and_the_cursor_brings_its_block_into_view() {
        let mut long = String::from("# Top\n\n");
        for index in 0..60 {
            long.push_str(&format!("paragraph {index}\n\n"));
        }
        long.push_str("## End\n\nlast words\n");
        let mut preview =
            MarkdownPreview::new(PathBuf::from("/tmp"), "README.md".into(), &long, Some(1));
        assert_eq!(preview.title(), "Preview README.md");
        assert!(painted_texts(&mut preview).contains(&"Top".to_string()));

        preview.text_changed(long.replace("last words", "new words"), 2);
        assert!(preview.is_busy());
        preview.pending = preview
            .pending
            .take()
            .map(|(_, text)| (Instant::now() - REPARSE_DELAY, text));
        assert!(preview.tick(&|| None).changed);

        preview.cursor_moved(long.find("last words").unwrap_or(0));
        let texts = painted_texts(&mut preview);
        assert!(texts.contains(&"new words".to_string()), "{texts:?}");
        assert!(preview.scroll > 0.0);
    }

    #[test]
    fn only_markdown_files_get_a_preview() {
        assert!(is_markdown("docs/README.md"));
        assert!(is_markdown("notes.MARKDOWN"));
        assert!(!is_markdown("src/main.rs"));
    }
}
