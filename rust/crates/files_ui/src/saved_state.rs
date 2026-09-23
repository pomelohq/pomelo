//! How file and image tabs save themselves between sessions and come back: a file keeps its path, the disk
//! time it was read at, its selections, scroll position and folds.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use workspace::persistence::SerializedItem;
use workspace::Item;

use crate::{FileItem, ImageItem, EDIT_LINE_H};

pub(crate) const FILE_KIND: &str = "file";
pub(crate) const IMAGE_KIND: &str = "image";
/// Characters kept from each end of a fold, to find it again when the file changed while the app was closed.
const FOLD_FINGERPRINT_CHARS: usize = 32;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct SavedFile {
    root: PathBuf,
    path: String,
    /// Seconds and nanoseconds since the epoch of the disk version the tab last read or saved.
    mtime: Option<(u64, u32)>,
    selections: Vec<(usize, usize)>,
    scroll: SavedScroll,
    folds: Vec<SavedFold>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct SavedScroll {
    top_row: usize,
    offset_y: f32,
    x: f32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct SavedFold {
    start: usize,
    end: usize,
    start_text: String,
    end_text: String,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct SavedImage {
    root: PathBuf,
    path: String,
}

fn epoch(time: std::time::SystemTime) -> Option<(u64, u32)> {
    let since = time.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((since.as_secs(), since.subsec_nanos()))
}

impl FileItem {
    pub(crate) fn saved_state(&self) -> Option<SerializedItem> {
        let buffer = self.buffer.as_ref()?;
        let rope = &buffer.rope;
        let folds = self
            .folds
            .ranges()
            .iter()
            .map(|fold| {
                let start_end = (fold.start + FOLD_FINGERPRINT_CHARS).min(fold.end);
                let end_start = fold
                    .end
                    .saturating_sub(FOLD_FINGERPRINT_CHARS)
                    .max(fold.start);
                SavedFold {
                    start: fold.start,
                    end: fold.end,
                    start_text: rope.slice(fold.start..start_end).to_string(),
                    end_text: rope.slice(end_start..fold.end).to_string(),
                }
            })
            .collect();
        let saved = SavedFile {
            root: self.root.clone(),
            path: self.path.clone(),
            mtime: self.saved_mtime.and_then(epoch),
            selections: buffer
                .selections()
                .iter()
                .map(|selection| (selection.start, selection.end))
                .collect(),
            scroll: SavedScroll {
                top_row: rope.char_to_line(self.scroll_anchor.min(rope.len_chars())),
                offset_y: self.scroll_anchor_offset,
                x: self.scroll_x,
            },
            folds,
        };
        Some(SerializedItem {
            kind: FILE_KIND.into(),
            data: serde_json::to_value(saved).ok()?,
        })
    }

    /// The tab a saved file state describes, or `None` when the file is gone.
    pub(crate) fn from_saved(item: &SerializedItem) -> Option<FileItem> {
        let saved: SavedFile = serde_json::from_value(item.data.clone()).ok()?;
        let SavedFile {
            root,
            path,
            selections,
            scroll,
            folds,
            ..
        } = saved;
        if !root.join(&path).is_file() {
            return None;
        }
        let text = files::read(&root, &path)
            .ok()
            .and_then(|contents| contents.text);
        let mut file = FileItem::new(root, &path, text);
        file.restore_view(&selections, &scroll, &folds);
        Some(file)
    }

    fn restore_view(
        &mut self,
        selections: &[(usize, usize)],
        scroll: &SavedScroll,
        folds: &[SavedFold],
    ) {
        let Some(buffer) = self.buffer.as_mut() else {
            return;
        };
        let len = buffer.rope.len_chars();
        let ranges: Vec<std::ops::Range<usize>> = selections
            .iter()
            .map(|&(start, end)| start.min(len)..end.min(len))
            .collect();
        if !ranges.is_empty() {
            buffer.select_ranges(&ranges);
        }
        let text = buffer.rope.to_string();
        let mut search_from = 0;
        let mut found = Vec::new();
        for fold in folds {
            let Some(range) = locate_fold(&buffer.rope, &text, fold, search_from) else {
                continue;
            };
            search_from = range.end;
            found.push(range);
        }
        let top_row = scroll
            .top_row
            .min(buffer.rope.len_lines().saturating_sub(1));
        self.scroll_anchor = buffer.rope.line_to_char(top_row);
        self.scroll_anchor_offset = scroll.offset_y.clamp(0.0, EDIT_LINE_H);
        self.scroll_x = scroll.x.max(0.0);
        if let Some(buffer) = self.buffer.as_ref() {
            for range in found {
                self.folds.fold(buffer, range);
            }
        }
        self.rows = None;
    }
}

/// Where a saved fold sits now: at its old offsets when the text there still matches, else the next place its
/// start text appears (from `search_from`, so repeated folds keep their order) followed by its end text.
fn locate_fold(
    rope: &ropey::Rope,
    text: &str,
    fold: &SavedFold,
    search_from: usize,
) -> Option<std::ops::Range<usize>> {
    let len = rope.len_chars();
    let matches_at = |at: usize, needle: &str| {
        let end = at + needle.chars().count();
        end <= len && rope.slice(at..end) == needle
    };
    let end_text_chars = fold.end_text.chars().count();
    if fold.start < len
        && matches_at(fold.start, &fold.start_text)
        && matches_at(fold.end.saturating_sub(end_text_chars), &fold.end_text)
    {
        return Some(fold.start..fold.end);
    }
    let find = |needle: &str, from: usize| -> Option<usize> {
        let byte_from = rope.try_char_to_byte(from.min(len)).ok()?;
        let found = text.get(byte_from..)?.find(needle)?;
        rope.try_byte_to_char(byte_from + found).ok()
    };
    let start = find(&fold.start_text, search_from)?;
    let end = if fold.start_text == fold.end_text {
        start + (fold.end - fold.start)
    } else {
        find(&fold.end_text, start + fold.start_text.chars().count())? + end_text_chars
    };
    (end > start && end <= len).then_some(start..end)
}

impl ImageItem {
    pub(crate) fn saved_state(&self) -> Option<SerializedItem> {
        let saved = SavedImage {
            root: self.root.clone(),
            path: self.path.clone(),
        };
        Some(SerializedItem {
            kind: IMAGE_KIND.into(),
            data: serde_json::to_value(saved).ok()?,
        })
    }

    pub(crate) fn from_saved(item: &SerializedItem) -> Option<ImageItem> {
        let saved: SavedImage = serde_json::from_value(item.data.clone()).ok()?;
        saved
            .root
            .join(&saved.path)
            .is_file()
            .then(|| ImageItem::new(&saved.root, &saved.path))
    }
}

/// Rebuild a file or image tab from its saved state.
pub(crate) fn restore_item(item: &SerializedItem) -> Option<Box<dyn Item>> {
    match item.kind.as_str() {
        FILE_KIND => FileItem::from_saved(item).map(|file| Box::new(file) as Box<dyn Item>),
        IMAGE_KIND => ImageItem::from_saved(item).map(|image| Box::new(image) as Box<dyn Item>),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pomelo-saved-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    const SOURCE: &str =
        "fn main() {\n    let a = 1;\n    let b = 2;\n}\n\nfn other() {\n    call();\n}\n";

    fn opened(root: &std::path::Path) -> FileItem {
        FileItem::new(root.to_path_buf(), "main.rs", Some(SOURCE.into()))
    }

    #[test]
    fn a_file_tab_keeps_its_selections_scroll_and_folds() {
        let root = temp_root("view");
        std::fs::write(root.join("main.rs"), SOURCE).unwrap();
        let mut file = opened(&root);
        if let Some(buffer) = file.buffer.as_mut() {
            buffer.select_ranges(&[3..7, 20..22]);
        }
        let fold = SOURCE.find("{\n    call").unwrap() + 1..SOURCE.len() - 2;
        if let Some(buffer) = file.buffer.as_ref() {
            file.folds.fold(buffer, fold.clone());
        }
        file.scroll_anchor = SOURCE.find("fn other").unwrap();
        file.scroll_anchor_offset = 6.0;
        let Some(saved) = file.saved_state() else {
            panic!("an open file should save its state");
        };
        let Some(back) = FileItem::from_saved(&saved) else {
            panic!("the saved file should come back");
        };
        let selections: Vec<(usize, usize)> = back
            .buffer
            .as_ref()
            .map(|buffer| {
                buffer
                    .selections()
                    .iter()
                    .map(|s| (s.start, s.end))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(selections, [(3, 7), (20, 22)]);
        assert_eq!(back.folds.ranges(), std::slice::from_ref(&fold));
        assert_eq!(back.scroll_anchor, file.scroll_anchor);
        assert_eq!(back.scroll_anchor_offset, 6.0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_fold_is_found_again_after_the_file_changed() {
        let root = temp_root("moved");
        std::fs::write(root.join("main.rs"), SOURCE).unwrap();
        let mut file = opened(&root);
        let start = SOURCE.find("{\n    call").unwrap() + 1;
        let fold = start..SOURCE.len() - 2;
        if let Some(buffer) = file.buffer.as_ref() {
            file.folds.fold(buffer, fold.clone());
        }
        let Some(saved) = file.saved_state() else {
            panic!("an open file should save its state");
        };
        let prefix = "// header\n";
        std::fs::write(root.join("main.rs"), format!("{prefix}{SOURCE}")).unwrap();
        let Some(back) = FileItem::from_saved(&saved) else {
            panic!("the saved file should come back");
        };
        let shift = prefix.chars().count();
        assert_eq!(
            back.folds.ranges(),
            std::slice::from_ref(&(fold.start + shift..fold.end + shift))
        );
        std::fs::remove_file(root.join("main.rs")).unwrap();
        assert!(FileItem::from_saved(&saved).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_editor_area_comes_back_with_its_tabs_and_splits() {
        use workspace::FunctionView;
        let root = temp_root("area");
        std::fs::write(root.join("main.rs"), SOURCE).unwrap();
        std::fs::write(root.join("lib.rs"), "pub fn lib() {}\n").unwrap();
        let mut view = crate::FilesView::new(root.clone());
        view.open_file("main.rs");
        view.open_file("lib.rs");
        let item = view.panes.clone_active_of(&[]);
        view.panes
            .split(&[], workspace::pane_group::SplitDirection::Right, item);
        let Some(saved) = view.save_panes() else {
            panic!("the editor area should save its panes");
        };
        let mut back = crate::FilesView::new(root.clone());
        assert!(back.restore_panes(&saved));
        assert_eq!(back.panes.group.leaf_count(), 2);
        assert_eq!(back.panes.active, vec![1]);
        let titles: Vec<String> = back
            .panes
            .pane_at(&[0])
            .map(|pane| pane.open.iter().map(|item| item.title()).collect())
            .unwrap_or_default();
        assert_eq!(titles, ["main.rs", "lib.rs"]);
        assert_eq!(back.save_panes(), Some(saved));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
