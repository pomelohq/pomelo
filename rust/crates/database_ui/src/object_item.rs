//! An object of a bucket as a tab: what the storage says about it, a preview (an image, or the first 64 KB of
//! text), and Download, Copy URL, Copy Path and Delete.

use std::path::PathBuf;
use std::sync::Arc;

use pom_db::object_storage::{format_size, HttpTransport, ObjectEntry, ObjectRead};
use pom_db::Database;
use terminal::Modifiers;
use ui::{button, div, label, theme, ButtonStyle, IconKind, Node, Rect};
use workspace::{Item, ItemTick};

use crate::{DatabaseContext, Pending};

const TEXT_PREVIEW: u64 = 64 * 1024;
const IMAGE_PREVIEW_MAX: u64 = 20 * 1024 * 1024;
const PRESIGNED_SECONDS: u64 = 3600;
const PREVIEW_LINES: usize = 400;
const LINE_H: f32 = 18.0;
const PAD: f32 = 16.0;

const DOWNLOAD: u64 = 1;
const COPY_URL: u64 = 2;
const COPY_PATH: u64 = 3;
const DELETE: u64 = 4;
const CONFIRM_DELETE: u64 = 5;
const CANCEL_DELETE: u64 = 6;

enum Preview {
    Loading,
    Image { id: u64 },
    Text(Vec<String>),
    None(String),
}

pub(crate) fn object_item_id(database: &Database, bucket: &str, key: &str) -> String {
    format!("db-object:{}:{bucket}:{key}", database.name)
}

/// The preview a content type (or the key's extension) gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewKind {
    Image,
    Text,
    None,
}

pub(crate) fn preview_kind(content_type: &str, key: &str) -> PreviewKind {
    let extension = key
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let content_type = content_type.to_ascii_lowercase();
    if content_type.starts_with("image/") && !content_type.contains("svg")
        || matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
        )
    {
        PreviewKind::Image
    } else if content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("csv")
        || matches!(
            extension.as_str(),
            "json" | "csv" | "txt" | "log" | "md" | "yml" | "yaml" | "xml" | "html" | "sql" | "svg"
        )
    {
        PreviewKind::Text
    } else {
        PreviewKind::None
    }
}

pub struct ObjectItem {
    context: DatabaseContext,
    database: Database,
    bucket: String,
    object: ObjectEntry,
    content_type: String,
    preview: Preview,
    reading: Pending<ObjectRead>,
    working: Pending<String>,
    confirming: bool,
    deleted: bool,
    status: Option<(String, bool)>,
    clipboard: Option<String>,
    scroll: f32,
    hits: Vec<(Rect, u64)>,
}

impl ObjectItem {
    pub fn new(
        context: DatabaseContext,
        database: Database,
        bucket: String,
        object: ObjectEntry,
    ) -> ObjectItem {
        let mut item = ObjectItem {
            context,
            database,
            bucket,
            object,
            content_type: String::new(),
            preview: Preview::Loading,
            reading: Pending::idle(),
            working: Pending::idle(),
            confirming: false,
            deleted: false,
            status: None,
            clipboard: None,
            scroll: 0.0,
            hits: Vec::new(),
        };
        item.read();
        item
    }

    fn transport(&self) -> Arc<dyn HttpTransport> {
        self.context.objects.clone()
    }

    fn read(&mut self) {
        let limit = match preview_kind("", &self.object.key) {
            PreviewKind::Image if self.object.size <= IMAGE_PREVIEW_MAX => None,
            PreviewKind::Image | PreviewKind::None => Some(1),
            PreviewKind::Text => Some(TEXT_PREVIEW),
        };
        let limit = if self.object.size == 0 { None } else { limit };
        let (name, bucket, key, transport) = (
            self.database.name.clone(),
            self.bucket.clone(),
            self.object.key.clone(),
            self.transport(),
        );
        self.reading = self.context.run(move |connector| {
            connector
                .object_store(&name)
                .read(transport.as_ref(), &bucket, &key, limit)
        });
    }

    fn path(&self) -> String {
        format!("{}/{}", self.bucket, self.object.key)
    }

    fn show(&mut self, read: Result<ObjectRead, String>) {
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                self.preview = Preview::None(error);
                return;
            }
        };
        self.content_type = read.content_type.clone();
        self.preview = match preview_kind(&read.content_type, &self.object.key) {
            PreviewKind::Image if read.bytes.len() as u64 >= self.object.size.max(1) => {
                match image::load_from_memory(&read.bytes) {
                    Ok(picture) => {
                        let rgba = picture.to_rgba8();
                        let (width, height) = rgba.dimensions();
                        let id = image_id(&self.database.name, &self.path());
                        ui::set_image(id, width, height, rgba.into_raw());
                        Preview::Image { id }
                    }
                    Err(_) => Preview::None("This image could not be decoded.".into()),
                }
            }
            PreviewKind::Image => {
                Preview::None("Too large to preview here. Download it to open.".into())
            }
            PreviewKind::Text => {
                let text = String::from_utf8_lossy(&read.bytes);
                let mut lines: Vec<String> = text
                    .lines()
                    .take(PREVIEW_LINES)
                    .map(|line| line.replace('\t', "    "))
                    .collect();
                if read.size.is_some_and(|size| size > read.bytes.len() as u64) {
                    lines.push(format!(
                        "... the first {} of {} shown",
                        format_size(read.bytes.len() as u64),
                        format_size(read.size.unwrap_or_default())
                    ));
                }
                Preview::Text(lines)
            }
            PreviewKind::None => {
                Preview::None("No preview for this type. Download it to open.".into())
            }
        };
    }

    fn click(&mut self, id: u64) {
        match id {
            DOWNLOAD => self.download(),
            COPY_URL => {
                let url = self.context.run_now(|connector| {
                    connector.object_store(&self.database.name).presigned_url(
                        &self.bucket,
                        &self.object.key,
                        PRESIGNED_SECONDS,
                    )
                });
                if let Some(url) = url {
                    self.clipboard = Some(url);
                    self.status = Some(("Copied a link that works for 1 hour".into(), false));
                }
            }
            COPY_PATH => {
                self.clipboard = Some(self.path());
                self.status = Some((format!("Copied {}", self.path()), false));
            }
            DELETE => self.confirming = true,
            CANCEL_DELETE => self.confirming = false,
            CONFIRM_DELETE => {
                self.confirming = false;
                let (name, bucket, key, transport) = (
                    self.database.name.clone(),
                    self.bucket.clone(),
                    self.object.key.clone(),
                    self.transport(),
                );
                self.status = Some(("Deleting...".into(), false));
                self.deleted = true;
                self.working = self.context.run(move |connector| {
                    connector
                        .object_store(&name)
                        .delete(transport.as_ref(), &bucket, &key)?;
                    Ok(format!("Deleted {key}"))
                });
            }
            _ => {}
        }
    }

    fn download(&mut self) {
        if self.working.busy() {
            return;
        }
        let downloads = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Downloads");
        let name = self.object.name().replace(['/', '\\'], "_");
        let (stem, extension) = match name.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => {
                (stem.to_string(), format!(".{extension}"))
            }
            _ => (name.clone(), String::new()),
        };
        let mut path = downloads.join(&name);
        let mut copy = 1;
        while path.exists() {
            path = downloads.join(format!("{stem}-{copy}{extension}"));
            copy += 1;
        }
        let (database, bucket, key, transport) = (
            self.database.name.clone(),
            self.bucket.clone(),
            self.object.key.clone(),
            self.transport(),
        );
        self.status = Some(("Downloading...".into(), false));
        self.working = self.context.run(move |connector| {
            connector
                .object_store(&database)
                .download(transport.as_ref(), &bucket, &key, &path)?;
            Ok(format!("Downloaded {} to {}", name, path.display()))
        });
    }

    fn facts(&self) -> Vec<(&'static str, String)> {
        let mut facts = vec![
            ("path", self.path()),
            ("size", format_size(self.object.size)),
        ];
        if !self.content_type.is_empty() {
            facts.push(("content-type", self.content_type.clone()));
        }
        if !self.object.modified.is_empty() {
            facts.push(("modified", readable_time(&self.object.modified)));
        }
        if !self.object.etag.is_empty() {
            facts.push(("etag", self.object.etag.clone()));
        }
        facts
    }

    fn toolbar(&self) -> Node {
        let colors = theme();
        if self.confirming {
            return div()
                .row()
                .gap(8.0)
                .items_center()
                .child(
                    label(format!(
                        "Delete {} from the bucket? It cannot be recovered.",
                        self.path()
                    ))
                    .size(12.5)
                    .color(colors.error)
                    .truncate(),
                )
                .child(button(CONFIRM_DELETE, "Delete", ButtonStyle::Outlined))
                .child(button(CANCEL_DELETE, "Cancel", ButtonStyle::Subtle))
                .into();
        }
        let mut row = div().row().gap(8.0).items_center();
        if !self.deleted {
            row = row
                .child(button(DOWNLOAD, "Download", ButtonStyle::Outlined))
                .child(button(
                    COPY_URL,
                    "Copy URL (expires in 1 hour)",
                    ButtonStyle::Outlined,
                ))
                .child(button(COPY_PATH, "Copy Path", ButtonStyle::Outlined))
                .child(button(DELETE, "Delete...", ButtonStyle::Outlined));
        }
        row.into()
    }
}

impl ObjectItem {
    /// The object as a folder tab's side shows it: name, preview, a few facts, then its actions.
    pub(crate) fn side(&self, width: f32, height: f32) -> Node {
        let colors = theme();
        let inner = width - 2.0 * 12.0;
        let preview: Node = match &self.preview {
            Preview::Loading => label("Loading...")
                .size(12.0)
                .color(colors.text_muted)
                .into(),
            Preview::None(text) => label(text.clone())
                .size(12.0)
                .color(colors.text_placeholder)
                .wrap(inner)
                .into(),
            Preview::Image { id } => div()
                .col()
                .w_px(inner)
                .h_px((height * 0.4).clamp(120.0, 260.0))
                .rounded(6.0)
                .border(1.0, colors.border_variant)
                .image(*id)
                .into(),
            Preview::Text(lines) => {
                let first = (self.scroll / LINE_H) as usize;
                let mut text = div()
                    .col()
                    .w_px(inner)
                    .px(8.0)
                    .py(6.0)
                    .rounded(6.0)
                    .bg(colors.editor_background)
                    .border(1.0, colors.border_variant);
                for line in lines
                    .iter()
                    .skip(first)
                    .take(((height * 0.5) / LINE_H) as usize)
                {
                    text = text.child(
                        div().row().h_px(LINE_H).items_center().child(
                            label(line.clone())
                                .size(11.5)
                                .mono()
                                .color(colors.editor_foreground)
                                .truncate(),
                        ),
                    );
                }
                text.into()
            }
        };
        let mut facts = div().col().gap(2.0);
        for (name, value) in self.facts().into_iter().filter(|(name, _)| *name != "path") {
            facts = facts.child(
                div()
                    .row()
                    .h_px(22.0)
                    .gap(8.0)
                    .items_center()
                    .child(
                        div()
                            .w_px(90.0)
                            .child(label(name).size(12.0).color(colors.text_placeholder)),
                    )
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .child(label(value).size(12.0).mono().color(colors.text).truncate()),
                    ),
            );
        }
        let actions: Node = if self.confirming {
            div()
                .col()
                .gap(6.0)
                .child(
                    label("Delete it from the bucket? It cannot be recovered.")
                        .size(12.0)
                        .color(colors.error)
                        .wrap(inner),
                )
                .child(
                    div()
                        .row()
                        .gap(6.0)
                        .child(button(CONFIRM_DELETE, "Delete", ButtonStyle::Outlined))
                        .child(button(CANCEL_DELETE, "Cancel", ButtonStyle::Subtle)),
                )
                .into()
        } else if self.deleted {
            div().into()
        } else {
            div()
                .col()
                .gap(6.0)
                .child(
                    div()
                        .row()
                        .gap(6.0)
                        .child(button(DOWNLOAD, "Download", ButtonStyle::Outlined))
                        .child(button(COPY_URL, "Copy URL (1 hour)", ButtonStyle::Subtle)),
                )
                .child(
                    div()
                        .row()
                        .gap(6.0)
                        .child(button(COPY_PATH, "Copy Path", ButtonStyle::Subtle))
                        .child(button(DELETE, "Delete...", ButtonStyle::Outlined)),
                )
                .into()
        };
        let mut side = div()
            .col()
            .p(12.0)
            .gap(8.0)
            .child(
                label(self.object.name().to_string())
                    .size(12.5)
                    .mono()
                    .color(colors.text)
                    .truncate(),
            )
            .child(preview)
            .child(crate::details::section("Details"))
            .child(facts)
            .child(div().h_px(4.0))
            .child(actions);
        if let Some((text, error)) = &self.status {
            side = side.child(
                label(text.clone())
                    .size(12.0)
                    .color(if *error {
                        colors.error
                    } else {
                        colors.text_muted
                    })
                    .wrap(inner),
            );
        }
        side.into()
    }

    pub(crate) fn press(&mut self, id: u64) {
        self.click(id);
    }

    pub(crate) fn scroll_preview(&mut self, delta_y: f32) -> bool {
        self.pointer_scroll(0.0, 0.0, delta_y, Modifiers::default())
    }

    /// Deleted and the storage has confirmed it.
    pub(crate) fn gone(&self) -> bool {
        self.deleted && !self.working.busy()
    }

    pub(crate) fn key(&self) -> &str {
        &self.object.key
    }
}

/// `2026-09-27T18:42:00.000Z` -> `2026-09-27 18:42`.
pub(crate) fn readable_time(stamp: &str) -> String {
    let text = stamp.replace('T', " ");
    text.get(..16).map_or(text.clone(), str::to_string)
}

fn image_id(database: &str, path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ("db-object", database, path).hash(&mut hasher);
    hasher.finish() | (1 << 63)
}

impl Item for ObjectItem {
    fn id(&self) -> Option<String> {
        Some(object_item_id(
            &self.database,
            &self.bucket,
            &self.object.key,
        ))
    }

    fn title(&self) -> String {
        self.object.name().to_string()
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::File)
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, _focused: bool) -> Option<ui::Painted> {
        let scale = ui::ui_text_scale();
        let colors = theme();
        let width = body.w / scale;
        let mut facts = div().col().gap(4.0);
        for (name, value) in self.facts() {
            facts = facts.child(
                div()
                    .row()
                    .gap(12.0)
                    .items_center()
                    .child(
                        div()
                            .w_px(96.0)
                            .child(label(name).size(12.5).color(colors.text_placeholder)),
                    )
                    .child(
                        div()
                            .row()
                            .flex(1.0)
                            .child(label(value).size(12.0).mono().color(colors.text).truncate()),
                    ),
            );
        }
        let preview: Node = match &self.preview {
            Preview::Loading => label("Loading...")
                .size(12.5)
                .color(colors.text_muted)
                .into(),
            Preview::None(text) => label(text.clone())
                .size(12.5)
                .color(colors.text_placeholder)
                .wrap(width - 2.0 * PAD)
                .into(),
            Preview::Image { id } => div()
                .col()
                .w_px(width - 2.0 * PAD)
                .h_px((body.h / scale - 200.0).max(120.0))
                .image(*id)
                .into(),
            Preview::Text(lines) => {
                let first = (self.scroll / LINE_H) as usize;
                let visible = ((body.h / scale) / LINE_H) as usize + 1;
                let mut text = div()
                    .col()
                    .w_px(width - 2.0 * PAD)
                    .px(12.0)
                    .py(10.0)
                    .rounded(6.0)
                    .bg(colors.surface_background)
                    .border(1.0, colors.border_variant);
                for line in lines.iter().skip(first).take(visible) {
                    text = text.child(
                        div().row().h_px(LINE_H).items_center().child(
                            label(line.clone())
                                .size(12.0)
                                .mono()
                                .color(colors.editor_foreground)
                                .truncate(),
                        ),
                    );
                }
                text.into()
            }
        };
        let mut content = div()
            .col()
            .w_px(width)
            .h_px(body.h / scale)
            .p(PAD)
            .gap(14.0)
            .bg(colors.editor_background)
            .child(self.toolbar());
        if let Some((text, error)) = &self.status {
            content = content.child(label(text.clone()).size(12.0).color(if *error {
                colors.error
            } else {
                colors.text_muted
            }));
        }
        let tree: Node = content.child(facts).child(preview).into();
        let painted = ui::render(&tree, body);
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
        match hit {
            Some(id) => {
                self.click(id);
                true
            }
            None => false,
        }
    }

    fn pointer_scroll(&mut self, _x: f32, _y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let Preview::Text(lines) = &self.preview else {
            return false;
        };
        let max = (lines.len() as f32 * LINE_H - LINE_H * 4.0).max(0.0);
        let next = (self.scroll - delta_y).clamp(0.0, max);
        let moved = (next - self.scroll).abs() > 0.01;
        self.scroll = next;
        moved
    }

    fn tick(&mut self, _clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let mut changed = false;
        if let Some(read) = self.reading.poll() {
            self.show(read);
            changed = true;
        }
        if let Some(done) = self.working.poll() {
            self.status = Some(match done {
                Ok(message) => (message, false),
                Err(error) => {
                    self.deleted = false;
                    (error, true)
                }
            });
            changed = true;
        }
        let clipboard_store = self.clipboard.take();
        ItemTick {
            changed: changed || clipboard_store.is_some(),
            clipboard_store,
            close: false,
        }
    }

    fn is_busy(&self) -> bool {
        self.reading.busy() || self.working.busy()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_follow_the_type_then_the_extension() {
        assert_eq!(preview_kind("image/png", "a"), PreviewKind::Image);
        assert_eq!(preview_kind("", "u1024.JPG"), PreviewKind::Image);
        assert_eq!(preview_kind("application/json", "x"), PreviewKind::Text);
        assert_eq!(preview_kind("", "import.csv"), PreviewKind::Text);
        assert_eq!(preview_kind("image/svg+xml", "logo.svg"), PreviewKind::Text);
        assert_eq!(
            preview_kind("application/pdf", "invoice.pdf"),
            PreviewKind::None
        );
        assert_eq!(
            readable_time("2026-09-27T18:42:00.000Z"),
            "2026-09-27 18:42"
        );
    }
}
