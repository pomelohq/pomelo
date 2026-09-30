//! How file and image tabs save themselves between sessions and come back: a file keeps its path, the disk
//! time it was read at, its selections, scroll position and folds.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use workspace::persistence::SerializedItem;
use workspace::Item;

use crate::{edit_line_h, FileItem, ImageItem};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contents: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contents_file: Option<PathBuf>,
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

enum UnsavedJob {
    Write(PathBuf, ropey::Rope),
    Remove(PathBuf),
}

impl UnsavedJob {
    fn target(&self) -> &PathBuf {
        match self {
            UnsavedJob::Write(target, _) | UnsavedJob::Remove(target) => target,
        }
    }

    fn run(self) {
        let result = match self {
            UnsavedJob::Write(target, rope) => write_unsaved(&target, &rope),
            UnsavedJob::Remove(target) => match std::fs::remove_file(&target) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        if let Err(error) = result {
            eprintln!("unsaved text: {error}");
        }
    }
}

fn write_unsaved(target: &std::path::Path, rope: &ropey::Rope) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = target.with_extension("partial");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&partial)?);
    for chunk in rope.chunks() {
        out.write_all(chunk.as_bytes())?;
    }
    out.into_inner()
        .map_err(|error| error.into_error())?
        .sync_all()?;
    std::fs::rename(&partial, target)
}

fn unsaved_path(root: &std::path::Path, path: &str) -> Option<PathBuf> {
    let full = root.join(path);
    let hash = full
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        });
    Some(unsaved_dir()?.join(format!("{hash:016x}.txt")))
}

fn unsaved_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("pomelo-unsaved-{}", std::process::id())));
    }
    Some(pom_paths::config_dir()?.join("unsaved"))
}

#[derive(Default)]
struct UnsavedQueue {
    jobs: Vec<UnsavedJob>,
    busy: bool,
}

fn unsaved_queue() -> &'static (std::sync::Mutex<UnsavedQueue>, std::sync::Condvar) {
    static QUEUE: std::sync::OnceLock<(std::sync::Mutex<UnsavedQueue>, std::sync::Condvar)> =
        std::sync::OnceLock::new();
    QUEUE.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("unsaved-writer".into())
            .spawn(run_unsaved_writer);
        if let Err(error) = spawned {
            eprintln!("unsaved text writer: {error}");
        }
        Default::default()
    })
}

fn queue_unsaved(job: UnsavedJob) {
    let (queue, ready) = unsaved_queue();
    let Ok(mut queue) = queue.lock() else {
        return;
    };
    queue
        .jobs
        .retain(|waiting| waiting.target() != job.target());
    queue.jobs.push(job);
    ready.notify_all();
}

fn run_unsaved_writer() {
    let (queue, ready) = unsaved_queue();
    loop {
        let job = {
            let Ok(mut state) = queue.lock() else {
                return;
            };
            state.busy = false;
            ready.notify_all();
            loop {
                if !state.jobs.is_empty() {
                    break;
                }
                state = match ready.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }
            state.busy = true;
            state.jobs.remove(0)
        };
        job.run();
    }
}

pub fn flush_unsaved_writes(timeout: std::time::Duration) {
    let (queue, ready) = unsaved_queue();
    let deadline = std::time::Instant::now() + timeout;
    let Ok(mut state) = queue.lock() else {
        return;
    };
    while !state.jobs.is_empty() || state.busy {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return;
        }
        state = match ready.wait_timeout(state, left) {
            Ok((state, _)) => state,
            Err(_) => return,
        };
    }
}

fn epoch(time: std::time::SystemTime) -> Option<(u64, u32)> {
    let since = time.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((since.as_secs(), since.subsec_nanos()))
}

impl FileItem {
    fn queue_unsaved_contents(&self) -> Option<PathBuf> {
        let buffer = self.buffer.as_ref()?;
        let target = unsaved_path(&self.root, &self.path)?;
        let version = buffer.is_dirty().then(|| buffer.version());
        if self.unsaved_queued.get() != version {
            self.unsaved_queued.set(version);
            let job = match version {
                Some(_) => UnsavedJob::Write(target.clone(), buffer.rope.clone()),
                None => UnsavedJob::Remove(target.clone()),
            };
            queue_unsaved(job);
        }
        version.map(|_| target)
    }

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
            contents: None,
            contents_file: self.queue_unsaved_contents(),
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
            mtime,
            selections,
            scroll,
            folds,
            contents,
            contents_file,
        } = saved;
        let contents = contents.or_else(|| {
            contents_file.and_then(|file| {
                std::fs::read_to_string(&file)
                    .map_err(|error| eprintln!("unsaved text {}: {error}", file.display()))
                    .ok()
            })
        });
        let on_disk = root.join(&path).is_file();
        if !on_disk && contents.is_none() {
            return None;
        }
        let text = if on_disk {
            files::read(&root, &path).ok().and_then(|read| read.text)
        } else {
            Some(String::new())
        };
        let mut file = FileItem::new(root, &path, text);
        if let Some(contents) = contents {
            file.restore_unsaved(&contents, mtime);
        }
        file.restore_view(&selections, &scroll, &folds);
        Some(file)
    }

    /// Bring back text left unsaved last session. The tab keeps the disk time it was based on, so a file that
    /// changed on disk meanwhile shows as a conflict instead of being overwritten.
    fn restore_unsaved(&mut self, contents: &str, mtime: Option<(u64, u32)>) {
        let Some(buffer) = self.buffer.as_mut() else {
            return;
        };
        buffer.restore_unsaved(contents);
        self.saved_mtime = mtime
            .map(|(secs, nanos)| std::time::UNIX_EPOCH + std::time::Duration::new(secs, nanos));
        self.rows = None;
        self.refresh_disk_state();
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
        let text = if folds.is_empty() {
            String::new()
        } else {
            buffer.rope.to_string()
        };
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
        self.scroll_anchor_offset = scroll.offset_y.clamp(0.0, edit_line_h());
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
        let mut view = crate::FilesView::scanned(root.clone());
        view.open_file("main.rs");
        view.open_file("lib.rs");
        let item = view.panes.clone_active_of(&[]);
        view.panes
            .split(&[], workspace::pane_group::SplitDirection::Right, item);
        let Some(saved) = view.save_panes() else {
            panic!("the editor area should save its panes");
        };
        let mut back = crate::FilesView::scanned(root.clone());
        assert!(back.restore_panes(&saved, &mut |_| None));
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

    fn edited(root: &std::path::Path) -> SerializedItem {
        let mut file = opened(root);
        if let Some(buffer) = file.buffer.as_mut() {
            buffer.edit(vec![(0..0, "// draft\n".into())]);
        }
        let saved = file.saved_state().unwrap();
        flush_unsaved_writes(std::time::Duration::from_secs(5));
        saved
    }

    fn text_of(file: &FileItem) -> String {
        file.buffer
            .as_ref()
            .map(|buffer| buffer.rope.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn unsaved_changes_come_back_unsaved_and_not_undoable() {
        let root = temp_root("hot-exit");
        std::fs::write(root.join("main.rs"), SOURCE).unwrap();
        let saved = edited(&root);
        let mut back = FileItem::from_saved(&saved).unwrap();
        assert_eq!(text_of(&back), format!("// draft\n{SOURCE}"));
        assert!(back.is_dirty());
        assert!(!back.has_conflict());
        if let Some(buffer) = back.buffer.as_mut() {
            buffer.undo();
        }
        assert_eq!(text_of(&back), format!("// draft\n{SOURCE}"));
        let clean = opened(&root).saved_state().unwrap();
        let reopened = FileItem::from_saved(&clean).unwrap();
        assert!(!reopened.is_dirty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unsaved_changes_over_a_file_changed_on_disk_show_a_conflict() {
        let root = temp_root("hot-exit-conflict");
        std::fs::write(root.join("main.rs"), SOURCE).unwrap();
        let saved = edited(&root);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("main.rs"), "fn changed() {}\n").unwrap();
        let back = FileItem::from_saved(&saved).unwrap();
        assert!(back.has_conflict());
        assert_eq!(text_of(&back), format!("// draft\n{SOURCE}"));
        std::fs::remove_file(root.join("main.rs")).unwrap();
        let gone = FileItem::from_saved(&saved).unwrap();
        assert!(gone.is_dirty());
        assert_eq!(text_of(&gone), format!("// draft\n{SOURCE}"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
