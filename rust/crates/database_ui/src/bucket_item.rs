//! A bucket's folder as a tab: its sub-folders (with how much each holds) and objects with their size and
//! time, a crumb to climb back up, a find field, Upload here, and the selected object previewed at the side.

use std::collections::HashMap;
use std::path::PathBuf;

use pom_db::object_storage::{format_size, Listing, ObjectEntry, PrefixStats};
use pom_db::Database;
use terminal::{Keystroke, Modifiers};
use ui::{div, icon, label, theme, IconKind, Node, Rect, Rgba};
use workspace::text_field::{FieldFont, TextField};
use workspace::{EditKey, Item, ItemTick, PanelRequest, TerminalKeyOutcome};

use crate::details::{self, Pane};
use crate::object_item::{object_item_id, readable_time, ObjectItem};
use crate::{DatabaseContext, Pending};

const TOOLBAR_H: f32 = 36.0;
const STATUS_H: f32 = 28.0;
const HEAD_H: f32 = 26.0;
const ROW_H: f32 = 28.0;
const FIELD_H: f32 = 24.0;
const SIDE_W: f32 = 340.0;
const SIDE_NEEDS: f32 = 640.0;
const PAGE: usize = 500;
const STATS_FOLDERS: usize = 40;

/// Below this the ids belong to the embedded preview.
const OWN_BASE: u64 = 100;
const FIND_FIELD: u64 = 100;
const REFRESH: u64 = 101;
const UPLOAD: u64 = 102;
const MORE: u64 = 103;
const CRUMB_BASE: u64 = 200;
const FOLDER_BASE: u64 = 1000;
const OBJECT_BASE: u64 = 100_000;
const OBJECT_END: u64 = 200_000;

const KIND: &str = "db-bucket";

pub struct BucketItem {
    context: DatabaseContext,
    database: Database,
    bucket: String,
    prefix: String,
    folders: Vec<String>,
    objects: Vec<ObjectEntry>,
    next: Option<String>,
    loading: Pending<Listing>,
    stats: HashMap<String, PrefixStats>,
    counting: Pending<Vec<(String, PrefixStats)>>,
    uploading: Pending<String>,
    find: TextField,
    finding: bool,
    focused: bool,
    selected: Option<String>,
    preview: Option<ObjectItem>,
    status: Option<(String, bool)>,
    list: Pane,
    side_area: Option<Rect>,
    hits: Vec<(Rect, u64)>,
    hovered: Option<u64>,
    requests: Vec<PanelRequest>,
}

/// `uploads/avatars/` -> `avatars/`.
fn folder_name(prefix: &str) -> String {
    let trimmed = prefix.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    format!("{name}/")
}

impl BucketItem {
    pub fn item_id(database: &Database, bucket: &str) -> String {
        format!("db-bucket:{}:{bucket}", database.name)
    }

    /// The bucket open at `prefix`, with `selected` (an object's key) chosen once the folder loads.
    pub fn new(
        context: DatabaseContext,
        database: Database,
        bucket: String,
        prefix: &str,
        selected: Option<String>,
    ) -> BucketItem {
        let mut find = TextField::default();
        find.set_font_size(12.0);
        let mut item = BucketItem {
            context,
            database,
            bucket,
            prefix: String::new(),
            folders: Vec::new(),
            objects: Vec::new(),
            next: None,
            loading: Pending::idle(),
            stats: HashMap::new(),
            counting: Pending::idle(),
            uploading: Pending::idle(),
            find,
            finding: false,
            focused: false,
            selected: None,
            preview: None,
            status: None,
            list: Pane::default(),
            side_area: None,
            hits: Vec::new(),
            hovered: None,
            requests: Vec::new(),
        };
        item.open(prefix);
        item.selected = selected;
        item
    }

    /// Shows `prefix` of the bucket (`""` is its top).
    pub fn open(&mut self, prefix: &str) {
        self.prefix = prefix.to_string();
        self.folders.clear();
        self.objects.clear();
        self.next = None;
        self.selected = None;
        self.preview = None;
        self.find.set_text("");
        self.list.scroll = 0.0;
        self.load(None);
    }

    fn load(&mut self, token: Option<String>) {
        let (name, bucket, prefix, transport) = (
            self.database.name.clone(),
            self.bucket.clone(),
            self.prefix.clone(),
            self.context.objects.clone(),
        );
        self.loading = self.context.run(move |connector| {
            connector.object_store(&name).list(
                transport.as_ref(),
                &bucket,
                &prefix,
                token.as_deref(),
                PAGE,
            )
        });
    }

    fn count_folders(&mut self) {
        let folders: Vec<String> = self
            .folders
            .iter()
            .filter(|folder| !self.stats.contains_key(*folder))
            .take(STATS_FOLDERS)
            .cloned()
            .collect();
        if folders.is_empty() {
            return;
        }
        let (name, bucket, transport) = (
            self.database.name.clone(),
            self.bucket.clone(),
            self.context.objects.clone(),
        );
        self.counting = self.context.run(move |connector| {
            let store = connector.object_store(&name);
            folders
                .into_iter()
                .map(|folder| {
                    store
                        .prefix_stats(transport.as_ref(), &bucket, &folder)
                        .map(|stats| (folder, stats))
                })
                .collect()
        });
    }

    fn take(&mut self, listing: Listing) {
        self.folders.extend(listing.prefixes);
        self.objects.extend(listing.objects);
        self.next = listing.next;
        if let Some(key) = self.selected.clone() {
            self.selected = None;
            self.select(&key);
        }
        self.count_folders();
    }

    fn select(&mut self, key: &str) {
        if self.selected.as_deref() == Some(key) {
            return;
        }
        let Some(object) = self
            .objects
            .iter()
            .find(|object| object.key == key)
            .cloned()
        else {
            self.selected = Some(key.to_string());
            return;
        };
        self.selected = Some(key.to_string());
        self.preview = Some(ObjectItem::new(
            self.context.clone(),
            self.database.clone(),
            self.bucket.clone(),
            object,
        ));
    }

    fn matches(&self, name: &str) -> bool {
        let find = self.find.text().to_lowercase();
        find.is_empty() || name.to_lowercase().contains(&find)
    }

    fn shown_folders(&self) -> Vec<(usize, &String)> {
        self.folders
            .iter()
            .enumerate()
            .filter(|(_, folder)| self.matches(&folder_name(folder)))
            .collect()
    }

    fn shown_objects(&self) -> Vec<(usize, &ObjectEntry)> {
        self.objects
            .iter()
            .enumerate()
            .filter(|(_, object)| self.matches(object.name()))
            .collect()
    }

    /// The crumb's parts: the bucket, then each folder down to this one, with the prefix each opens.
    fn crumbs(&self) -> Vec<(String, String)> {
        let mut parts = vec![(self.bucket.clone(), String::new())];
        let mut path = String::new();
        for part in self.prefix.split('/').filter(|part| !part.is_empty()) {
            path.push_str(part);
            path.push('/');
            parts.push((part.to_string(), path.clone()));
        }
        parts
    }

    fn upload(&mut self) {
        let files: Vec<PathBuf> = (self.context.choose_files)();
        if files.is_empty() {
            return;
        }
        let (name, bucket, prefix, transport) = (
            self.database.name.clone(),
            self.bucket.clone(),
            self.prefix.clone(),
            self.context.objects.clone(),
        );
        self.status = Some((format!("Uploading {} files...", files.len()), false));
        self.uploading = self.context.run(move |connector| {
            let store = connector.object_store(&name);
            for file in &files {
                let file_name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .ok_or_else(|| format!("{} has no file name", file.display()))?;
                store.upload(
                    transport.as_ref(),
                    &bucket,
                    &format!("{prefix}{file_name}"),
                    file,
                )?;
            }
            Ok(match files.len() {
                1 => format!("Uploaded 1 file to {bucket}/{prefix}"),
                count => format!("Uploaded {count} files to {bucket}/{prefix}"),
            })
        });
    }

    fn open_object_tab(&mut self, object: ObjectEntry) {
        let (context, database, bucket) = (
            self.context.clone(),
            self.database.clone(),
            self.bucket.clone(),
        );
        self.requests.push(PanelRequest::Reveal {
            id: object_item_id(&database, &bucket, &object.key),
            open: Box::new(move || {
                Some(Box::new(ObjectItem::new(context, database, bucket, object)))
            }),
        });
    }

    fn click(&mut self, id: u64, click_count: u32) {
        match id {
            FIND_FIELD => self.finding = true,
            REFRESH => {
                let prefix = self.prefix.clone();
                let selected = self.selected.clone();
                self.stats.clear();
                self.open(&prefix);
                self.selected = selected;
            }
            UPLOAD => self.upload(),
            MORE => {
                if let Some(token) = self.next.clone() {
                    self.load(Some(token));
                }
            }
            id if (CRUMB_BASE..FOLDER_BASE).contains(&id) => {
                if let Some((_, prefix)) = self.crumbs().get((id - CRUMB_BASE) as usize).cloned() {
                    self.open(&prefix);
                }
            }
            id if (FOLDER_BASE..OBJECT_BASE).contains(&id) => {
                if let Some(folder) = self.folders.get((id - FOLDER_BASE) as usize).cloned() {
                    self.open(&folder);
                }
            }
            id if (OBJECT_BASE..OBJECT_END).contains(&id) => {
                if let Some(object) = self.objects.get((id - OBJECT_BASE) as usize).cloned() {
                    self.select(&object.key);
                    if click_count >= 2 {
                        self.open_object_tab(object);
                    }
                }
            }
            _ => {}
        }
    }

    fn text_button(&self, id: u64, text: &str) -> Node {
        let colors = theme();
        let mut button = div()
            .row()
            .h_px(22.0)
            .px(7.0)
            .rounded(4.0)
            .items_center()
            .on_click(id)
            .child(label(text.to_string()).size(12.0).color(colors.text));
        if self.hovered == Some(id) {
            button = button.bg(colors.ghost_element_hover);
        }
        button.into()
    }

    fn toolbar(&self, width: f32) -> Node {
        let colors = theme();
        let wide = width > 760.0;
        let mut crumb = div().row().flex(1.0).gap(6.0).items_center().child(
            icon(IconKind::EngineMinio)
                .size(12.0)
                .color(colors.icon_muted),
        );
        if wide && !self.database.repo.is_empty() {
            crumb = crumb
                .child(
                    label(self.database.repo.clone())
                        .size(12.5)
                        .color(colors.text_muted),
                )
                .child(label("/").size(12.5).color(colors.text_placeholder));
        }
        if wide {
            crumb = crumb
                .child(
                    label(self.database.label.clone())
                        .size(12.5)
                        .color(colors.text_muted),
                )
                .child(label("/").size(12.5).color(colors.text_placeholder));
        }
        let crumbs = self.crumbs();
        let last = crumbs.len() - 1;
        for (index, (name, _)) in crumbs.into_iter().enumerate() {
            if index > 0 {
                crumb = crumb.child(label("/").size(12.5).color(colors.text_placeholder));
            }
            let id = CRUMB_BASE + index as u64;
            let text = label(name).size(12.5).truncate();
            let text = if index == last {
                text.medium().color(colors.text)
            } else if self.hovered == Some(id) {
                text.underline(colors.text).color(colors.text)
            } else {
                text.color(colors.text_muted)
            };
            crumb = crumb.child(div().row().items_center().on_click(id).child(text));
        }
        if wide && !self.context.branch.is_empty() {
            crumb = crumb
                .child(div().w_px(2.0))
                .child(label("on").size(12.0).color(colors.text_placeholder))
                .child(icon(IconKind::Branch).size(11.0).color(colors.text_accent))
                .child(
                    label(self.context.branch.clone())
                        .size(12.0)
                        .mono()
                        .color(colors.text_accent),
                );
        }
        let find_w = (width * 0.3).clamp(120.0, 220.0);
        let finding = self.finding && self.focused;
        div()
            .row()
            .h_px(TOOLBAR_H)
            .px(10.0)
            .gap(8.0)
            .items_center()
            .child(crumb)
            .child(
                div()
                    .row()
                    .w_px(find_w)
                    .h_px(FIELD_H)
                    .px(6.0)
                    .gap(6.0)
                    .items_center()
                    .rounded(4.0)
                    .bg(colors.editor_background)
                    .border(
                        1.0,
                        if finding {
                            colors.border_focused
                        } else {
                            colors.border_variant
                        },
                    )
                    .on_click(FIND_FIELD)
                    .child(icon(IconKind::Search).size(11.0).color(colors.icon_muted))
                    .child(self.find.render(
                        "name in this folder",
                        finding,
                        colors.text,
                        FIELD_H - 6.0,
                        FieldFont::Ui,
                    )),
            )
            .child(self.text_button(UPLOAD, "Upload here"))
            .child(
                div()
                    .row()
                    .w_px(22.0)
                    .h_px(22.0)
                    .rounded(4.0)
                    .items_center()
                    .justify_center()
                    .on_click(REFRESH)
                    .bg(if self.hovered == Some(REFRESH) {
                        colors.ghost_element_hover
                    } else {
                        Rgba::TRANSPARENT
                    })
                    .child(icon(IconKind::RotateCw).size(12.0).color(colors.icon_muted)),
            )
            .into()
    }

    fn line(
        &self,
        id: u64,
        glyph: Node,
        name: Node,
        size: String,
        modified: String,
        chosen: bool,
    ) -> Node {
        let colors = theme();
        let mut row = div()
            .row()
            .h_px(ROW_H)
            .px(12.0)
            .gap(10.0)
            .items_center()
            .on_click(id)
            .child(div().row().w_px(16.0).items_center().child(glyph))
            .child(div().row().flex(1.0).items_center().child(name))
            .child(
                div()
                    .row()
                    .w_px(110.0)
                    .items_center()
                    .child(label(size).size(12.0).color(colors.text_muted).truncate()),
            )
            .child(
                div().row().w_px(130.0).items_center().child(
                    label(modified)
                        .size(12.0)
                        .color(colors.text_placeholder)
                        .truncate(),
                ),
            );
        if chosen {
            row = row.bg(colors.element_selected);
        } else if self.hovered == Some(id) {
            row = row.bg(colors.ghost_element_hover);
        }
        row.into()
    }

    fn list_view(&self) -> Node {
        let colors = theme();
        let mut list = div().col();
        let (folders, objects) = (self.shown_folders(), self.shown_objects());
        if folders.is_empty() && objects.is_empty() {
            let text = if self.loading.busy() {
                "Loading...".to_string()
            } else if self.find.text().is_empty() {
                "This folder is empty".to_string()
            } else {
                format!("Nothing here is named like {}", self.find.text())
            };
            return list
                .child(
                    div()
                        .row()
                        .px(12.0)
                        .h_px(ROW_H)
                        .items_center()
                        .child(label(text).size(12.0).color(colors.text_muted)),
                )
                .into();
        }
        for (index, folder) in folders {
            let (size, count) = match self.stats.get(folder) {
                Some(stats) => {
                    let plus = if stats.capped { "+" } else { "" };
                    (
                        format_size(stats.bytes),
                        match stats.objects {
                            1 => "1 file".to_string(),
                            count => format!("{count}{plus} files"),
                        },
                    )
                }
                None => (String::new(), String::new()),
            };
            list = list.child(
                self.line(
                    FOLDER_BASE + index as u64,
                    icon(IconKind::Folder)
                        .size(14.0)
                        .color(colors.text_accent)
                        .into(),
                    label(folder_name(folder))
                        .size(12.5)
                        .color(colors.text)
                        .truncate()
                        .into(),
                    size,
                    count,
                    false,
                ),
            );
        }
        for (index, object) in objects {
            list = list.child(
                self.line(
                    OBJECT_BASE + index as u64,
                    icon(IconKind::File)
                        .size(13.0)
                        .color(colors.icon_muted)
                        .into(),
                    label(object.name().to_string())
                        .size(12.5)
                        .mono()
                        .color(colors.text)
                        .truncate()
                        .into(),
                    format_size(object.size),
                    readable_time(&object.modified),
                    self.selected.as_deref() == Some(object.key.as_str()),
                ),
            );
        }
        if self.next.is_some() {
            let text = if self.loading.busy() {
                "Loading..."
            } else {
                "Load more"
            };
            list = list.child(
                div()
                    .row()
                    .px(38.0)
                    .h_px(ROW_H)
                    .items_center()
                    .child(details::link(MORE, text, self.hovered)),
            );
        }
        list.into()
    }

    fn status_bar(&self) -> Node {
        let colors = theme();
        let count = self.folders.len() + self.objects.len();
        let plus = if self.next.is_some() { "+" } else { "" };
        let (status, error) = self.status.clone().unwrap_or_default();
        div()
            .row()
            .h_px(STATUS_H)
            .px(12.0)
            .gap(10.0)
            .items_center()
            .child(
                label(format!("{count}{plus} items"))
                    .size(12.0)
                    .color(colors.text_muted),
            )
            .child(
                div().row().flex(1.0).items_center().child(
                    label(status)
                        .size(12.0)
                        .color(if error {
                            colors.error
                        } else {
                            colors.text_muted
                        })
                        .truncate(),
                ),
            )
            .into()
    }

    fn hit(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| {
                x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
            })
            .map(|(_, id)| *id)
    }

    fn move_selection(&mut self, step: isize) {
        let keys: Vec<String> = self
            .shown_objects()
            .into_iter()
            .map(|(_, object)| object.key.clone())
            .collect();
        if keys.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| keys.iter().position(|key| key == selected));
        let next = match at {
            Some(at) => (at as isize + step).clamp(0, keys.len() as isize - 1) as usize,
            None => 0,
        };
        let key = keys[next].clone();
        self.select(&key);
    }

    /// Shows a listing as if the storage had answered (previews and tests).
    pub fn show(&mut self, listing: Listing, stats: Vec<(String, PrefixStats)>) {
        self.loading = Pending::idle();
        self.counting = Pending::idle();
        self.stats.extend(stats);
        let selected = self.selected.take();
        self.folders.extend(listing.prefixes);
        self.objects.extend(listing.objects);
        self.next = listing.next;
        if let Some(key) = selected {
            self.select(&key);
        }
    }

    pub fn preview_mut(&mut self) -> Option<&mut ObjectItem> {
        self.preview.as_mut()
    }
}

pub(crate) fn restore_bucket(
    context: &DatabaseContext,
    item: &workspace::persistence::SerializedItem,
) -> Option<Box<dyn Item>> {
    if item.kind != KIND {
        return None;
    }
    let name = item.data.get("database")?.as_str()?;
    let database = context
        .databases()
        .into_iter()
        .find(|database| database.name == name)?;
    let bucket = item.data.get("bucket")?.as_str()?.to_string();
    let prefix = item
        .data
        .get("prefix")
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default();
    Some(Box::new(BucketItem::new(
        context.clone(),
        database,
        bucket,
        prefix,
        None,
    )))
}

impl Item for BucketItem {
    fn id(&self) -> Option<String> {
        Some(Self::item_id(&self.database, &self.bucket))
    }

    fn title(&self) -> String {
        format!("{} [{}]", self.bucket, self.database.label)
    }

    fn tab_icon(&self) -> Option<IconKind> {
        Some(IconKind::Folder)
    }

    fn serialize(&self) -> Option<workspace::persistence::SerializedItem> {
        Some(workspace::persistence::SerializedItem {
            kind: KIND.into(),
            data: serde_json::json!({
                "database": self.database.name,
                "bucket": self.bucket,
                "prefix": self.prefix,
            }),
        })
    }

    fn render(&mut self) -> Node {
        div().into()
    }

    fn paint_body(&mut self, body: Rect, focused: bool) -> Option<ui::Painted> {
        // A tab that paints its own body is never told its focus any other way.
        self.focused = focused;
        let scale = ui::ui_text_scale();
        let colors = theme();
        let width = body.w / scale;
        let side_w = if width >= SIDE_NEEDS { SIDE_W } else { 0.0 };
        let main_w = width - side_w - if side_w > 0.0 { 1.0 } else { 0.0 };
        let height = body.h / scale;
        let list_h = (height - TOOLBAR_H - HEAD_H - STATUS_H - 3.0).max(0.0);
        let head = div()
            .row()
            .h_px(HEAD_H)
            .px(12.0)
            .gap(10.0)
            .items_center()
            .child(div().w_px(16.0))
            .child(
                div()
                    .row()
                    .flex(1.0)
                    .child(details::small("Name", colors.text_placeholder)),
            )
            .child(
                div()
                    .row()
                    .w_px(110.0)
                    .child(details::small("Size", colors.text_placeholder)),
            )
            .child(
                div()
                    .row()
                    .w_px(130.0)
                    .child(details::small("Modified", colors.text_placeholder)),
            );
        let main = div()
            .col()
            .w_px(main_w)
            .h_px(height)
            .child(self.toolbar(main_w))
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(head)
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(div().h_px(list_h))
            .child(div().h_px(1.0).bg(colors.border_variant))
            .child(self.status_bar());
        let mut frame = div()
            .row()
            .w_px(width)
            .h_px(height)
            .bg(colors.editor_background)
            .child(main);
        if side_w > 0.0 {
            frame = frame
                .child(div().w_px(1.0).h_px(height).bg(colors.border_variant))
                .child(div().w_px(side_w).h_px(height).bg(colors.panel_background));
        }
        let frame: Node = frame.into();
        let mut painted = ui::render(&frame, body);
        let list_area = Rect::new(
            body.x,
            body.y + (TOOLBAR_H + HEAD_H + 2.0) * scale,
            main_w * scale,
            list_h * scale,
            Rgba::TRANSPARENT,
        );
        let list = self.list_view();
        let mut list = self.list.paint(&list, list_area);
        details::clip_right(&mut list, body.x + main_w * scale);
        details::merge(&mut painted, list);
        self.side_area = None;
        if side_w > 0.0 {
            let area = Rect::new(
                body.x + (main_w + 1.0) * scale,
                body.y,
                side_w * scale,
                body.h,
                Rgba::TRANSPARENT,
            );
            let side: Node = match &self.preview {
                Some(preview) => preview.side(side_w, height),
                None => div()
                    .col()
                    .p(12.0)
                    .child(
                        label("Select a file to preview it.")
                            .size(12.0)
                            .color(colors.text_placeholder),
                    )
                    .into(),
            };
            let mut side = ui::render(&side, area);
            details::clip_right(&mut side, area.x + area.w - 2.0);
            details::merge(&mut painted, side);
            self.side_area = Some(area);
        }
        self.hits = painted.hits.clone();
        Some(painted)
    }

    fn wants_keystrokes(&self) -> bool {
        true
    }

    fn keystroke(&mut self, keystroke: &Keystroke) -> TerminalKeyOutcome {
        let modifiers = keystroke.modifiers;
        if modifiers.cmd && keystroke.key == "r" {
            self.click(REFRESH, 1);
            return TerminalKeyOutcome::Handled;
        }
        if self.finding {
            let edit = match keystroke.key.as_str() {
                "escape" | "enter" => {
                    self.finding = false;
                    return TerminalKeyOutcome::Handled;
                }
                "v" if modifiers.cmd => return TerminalKeyOutcome::Paste,
                "left" if modifiers.cmd => EditKey::Home,
                "right" if modifiers.cmd => EditKey::End,
                "left" => EditKey::Left,
                "right" => EditKey::Right,
                "backspace" if modifiers.alt => EditKey::DeleteWordLeft,
                "backspace" => EditKey::Backspace,
                "delete" => EditKey::Delete,
                "a" if modifiers.cmd => EditKey::SelectAll,
                _ => return TerminalKeyOutcome::Ignored,
            };
            self.find.key(edit, modifiers.shift);
            return TerminalKeyOutcome::Handled;
        }
        match keystroke.key.as_str() {
            "down" => self.move_selection(1),
            "up" => self.move_selection(-1),
            "enter" => {
                if let Some(object) = self
                    .selected
                    .as_ref()
                    .and_then(|key| self.objects.iter().find(|object| &object.key == key))
                    .cloned()
                {
                    self.open_object_tab(object);
                }
            }
            "backspace" if modifiers.cmd => {
                let crumbs = self.crumbs();
                if crumbs.len() > 1 {
                    let parent = crumbs[crumbs.len() - 2].1.clone();
                    self.open(&parent);
                }
            }
            _ => return TerminalKeyOutcome::Ignored,
        }
        TerminalKeyOutcome::Handled
    }

    fn input_text(&mut self, text: &str) {
        if self.finding {
            let typed: String = text.chars().filter(|c| !c.is_control()).collect();
            self.find.insert(&typed);
        }
    }

    fn paste(&mut self, text: &str, _slices: Option<&[workspace::ClipboardSlice]>) {
        self.input_text(text);
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn pointer_down(&mut self, x: f32, y: f32, click_count: u32, _modifiers: Modifiers) -> bool {
        match self.hit(x, y) {
            Some(id) if id < OWN_BASE => {
                if let Some(preview) = self.preview.as_mut() {
                    preview.press(id);
                }
            }
            Some(id) => {
                if id != FIND_FIELD {
                    self.finding = false;
                }
                self.click(id, click_count);
            }
            None => self.finding = false,
        }
        true
    }

    fn pointer_move(&mut self, x: f32, y: f32, _modifiers: Modifiers, _focused: bool) -> bool {
        let hovered = self.hit(x, y);
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        changed
    }

    fn pointer_scroll(&mut self, x: f32, y: f32, delta_y: f32, _modifiers: Modifiers) -> bool {
        let in_side = self.side_area.is_some_and(|area| {
            x >= area.x && x < area.x + area.w && y >= area.y && y < area.y + area.h
        });
        if in_side {
            return self
                .preview
                .as_mut()
                .is_some_and(|preview| preview.scroll_preview(delta_y));
        }
        self.list.scroll_by(delta_y)
    }

    fn tick(&mut self, clipboard: &dyn Fn() -> Option<String>) -> ItemTick {
        let mut changed = false;
        if let Some(listing) = self.loading.poll() {
            match listing {
                Ok(listing) => self.take(listing),
                Err(error) => self.status = Some((error, true)),
            }
            changed = true;
        }
        if let Some(counted) = self.counting.poll() {
            match counted {
                Ok(stats) => {
                    self.stats.extend(stats);
                    self.count_folders();
                }
                Err(error) => self.status = Some((error, true)),
            }
            changed = true;
        }
        if let Some(done) = self.uploading.poll() {
            self.status = Some(match done {
                Ok(message) => (message, false),
                Err(error) => (error, true),
            });
            let prefix = self.prefix.clone();
            let selected = self.selected.clone();
            self.open(&prefix);
            self.selected = selected;
            changed = true;
        }
        let mut clipboard_store = None;
        if let Some(preview) = self.preview.as_mut() {
            let tick = preview.tick(clipboard);
            changed |= tick.changed;
            clipboard_store = tick.clipboard_store;
            if preview.gone() {
                let key = preview.key().to_string();
                self.objects.retain(|object| object.key != key);
                self.preview = None;
                self.selected = None;
                self.status = Some((format!("Deleted {key}"), false));
                changed = true;
            }
        }
        ItemTick {
            changed,
            clipboard_store,
            ..ItemTick::default()
        }
    }

    fn is_busy(&self) -> bool {
        self.loading.busy()
            || self.counting.busy()
            || self.uploading.busy()
            || self
                .preview
                .as_ref()
                .is_some_and(|preview| preview.is_busy())
    }

    fn take_requests(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.requests)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(key: &str, size: u64) -> ObjectEntry {
        ObjectEntry {
            key: key.into(),
            size,
            ..ObjectEntry::default()
        }
    }

    fn item(prefix: &str, selected: Option<&str>) -> (crate::tests::TestContext, BucketItem) {
        let context = crate::tests::context();
        let database = context
            .context
            .databases()
            .into_iter()
            .next()
            .expect("a database");
        let item = BucketItem::new(
            context.context.clone(),
            database,
            "uploads".into(),
            prefix,
            selected.map(str::to_string),
        );
        (context, item)
    }

    #[test]
    fn the_crumb_climbs_back_up_and_find_narrows_the_folder() {
        let (_context, mut item) = item("avatars/2026/", None);
        let crumbs: Vec<String> = item.crumbs().into_iter().map(|(name, _)| name).collect();
        assert_eq!(crumbs, ["uploads", "avatars", "2026"]);
        assert_eq!(item.crumbs()[1].1, "avatars/");
        item.show(
            Listing {
                prefixes: vec!["avatars/2026/big/".into()],
                objects: vec![
                    object("avatars/2026/u1.png", 10),
                    object("avatars/2026/notes.txt", 5),
                ],
                next: None,
            },
            Vec::new(),
        );
        item.find.set_text("PNG");
        assert_eq!(item.shown_objects().len(), 1);
        assert!(item.shown_folders().is_empty());
        item.click(CRUMB_BASE + 1, 1);
        assert_eq!(item.prefix, "avatars/");
        assert!(item.objects.is_empty());
        assert_eq!(folder_name("uploads/avatars/"), "avatars/");
    }

    #[test]
    fn an_object_opened_from_the_tree_is_chosen_once_its_folder_loads() {
        let (_context, mut item) = item("exports/", Some("exports/orders.json"));
        item.show(
            Listing {
                prefixes: Vec::new(),
                objects: vec![object("exports/a.csv", 1), object("exports/orders.json", 2)],
                next: None,
            },
            Vec::new(),
        );
        assert_eq!(item.selected.as_deref(), Some("exports/orders.json"));
        assert!(item.preview.is_some());
        item.move_selection(-1);
        assert_eq!(item.selected.as_deref(), Some("exports/a.csv"));
        let saved = item.serialize().expect("saved");
        assert_eq!(saved.data["prefix"], "exports/");
    }
}
