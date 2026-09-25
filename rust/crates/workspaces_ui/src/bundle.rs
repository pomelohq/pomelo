use std::path::PathBuf;

use pom_bundle::Contents;
use ui::{div, label, theme, IconKind, LabelSize, Node};
use workspace::{
    checkbox, modal_button, modal_footer, modal_frame, modal_header, modal_section,
    outlined_button, status_line, EditKey, InputField, ModalResult, WindowModal, WINDOW_MODAL_BASE,
};

const WIDTH: f32 = 480.0;
const PREVIEW_WIDTH: f32 = 580.0;
const PREVIEW_LINES: usize = 14;

const CLOSE: u64 = WINDOW_MODAL_BASE + 1;
const CANCEL: u64 = WINDOW_MODAL_BASE + 2;
const CONFIRM: u64 = WINDOW_MODAL_BASE + 3;
const INCLUDE_SECRETS: u64 = WINDOW_MODAL_BASE + 4;
const PASSWORD_FIELD: u64 = WINDOW_MODAL_BASE + 5;
const CHOOSE_FILE: u64 = WINDOW_MODAL_BASE + 6;
const WRITE_CONFIG: u64 = WINDOW_MODAL_BASE + 7;
const STORE_SECRETS: u64 = WINDOW_MODAL_BASE + 8;
const ADAPT: u64 = WINDOW_MODAL_BASE + 9;

/// What the export form submits; a password means the secrets go in, sealed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportConfig {
    pub password: Option<String>,
}

pub struct ExportConfigModal {
    secret_count: usize,
    include_secrets: bool,
    password: InputField,
    result: Option<ModalResult>,
}

impl ExportConfigModal {
    pub fn new(secret_count: usize) -> ExportConfigModal {
        ExportConfigModal {
            secret_count,
            include_secrets: false,
            password: InputField::new("Password", "shared with your teammate separately").masked(),
            result: None,
        }
    }

    fn can_export(&self) -> bool {
        !self.include_secrets || !self.password.text().is_empty()
    }

    fn submit(&mut self) {
        if !self.can_export() {
            return;
        }
        let password = self.include_secrets.then(|| self.password.text());
        self.result = Some(ModalResult::Submitted(Box::new(ExportConfig { password })));
    }
}

impl WindowModal for ExportConfigModal {
    fn width(&self) -> f32 {
        WIDTH
    }

    fn render(&mut self) -> Node {
        let colors = theme();
        let mut section = modal_section(10.0);
        if self.secret_count > 0 {
            section = section.child(checkbox(
                INCLUDE_SECRETS,
                self.include_secrets,
                &format!("Include secrets ({})", self.secret_count),
            ));
        }
        section = if self.include_secrets {
            section
                .child(self.password.render(PASSWORD_FIELD, true, None, None))
                .child(status_line(
                    IconKind::Key,
                    colors.icon_muted,
                    "Secrets are sealed with AES-256-GCM into a .pombundle",
                ))
        } else {
            section.child(status_line(
                IconKind::File,
                colors.icon_muted,
                "Exports the merged config as plain YAML, without secrets",
            ))
        };
        let buttons = div()
            .row()
            .items_center()
            .gap(4.0)
            .child(modal_button(CANCEL, "Cancel", Some("escape"), true))
            .child(modal_button(
                CONFIRM,
                "Export...",
                Some("enter"),
                self.can_export(),
            ));
        modal_frame(WIDTH)
            .child(modal_header("Export Config", Some(CLOSE)))
            .child(section)
            .child(modal_footer(None, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            INCLUDE_SECRETS => self.include_secrets = !self.include_secrets,
            CONFIRM => self.submit(),
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.submit(),
            key if self.include_secrets => {
                self.password.field.key(key, shift);
            }
            _ => {}
        }
        true
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() || !self.include_secrets {
            return false;
        }
        self.password.field.insert(&typed);
        true
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

/// Opens the system file picker; returns the file chosen.
pub type FileChooser = Box<dyn Fn() -> Option<PathBuf>>;

/// What the import form submits. `adapt` hands the config to Claude to merge instead of replacing `pom.yml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportConfig {
    pub contents: Contents,
    pub write_config: bool,
    pub store_secrets: bool,
    pub adapt: bool,
}

enum Stage {
    Pick,
    Locked(Vec<u8>),
    Loaded(Contents),
}

pub struct ImportConfigModal {
    choose_file: FileChooser,
    file_name: String,
    stage: Stage,
    password: InputField,
    error: Option<String>,
    write_config: bool,
    store_secrets: bool,
    result: Option<ModalResult>,
}

impl ImportConfigModal {
    pub fn new(choose_file: FileChooser) -> ImportConfigModal {
        ImportConfigModal {
            choose_file,
            file_name: String::new(),
            stage: Stage::Pick,
            password: InputField::new("Password", "the bundle's password").masked(),
            error: None,
            write_config: true,
            store_secrets: true,
            result: None,
        }
    }

    fn choose(&mut self) {
        let Some(path) = (self.choose_file)() else {
            return;
        };
        self.file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.password.field.set_text("");
        match std::fs::read(&path) {
            Ok(data) if data.is_empty() => self.fail("the file is empty"),
            Ok(data) => self.load(data),
            Err(error) => self.fail(&format!("could not read {}: {error}", path.display())),
        }
    }

    fn fail(&mut self, message: &str) {
        self.stage = Stage::Pick;
        self.error = Some(message.to_string());
    }

    fn load(&mut self, data: Vec<u8>) {
        self.error = None;
        if pom_bundle::is_sealed(&data) {
            self.stage = Stage::Locked(data);
            return;
        }
        match pom_bundle::open(&data, "") {
            Ok(contents) => self.stage = Stage::Loaded(contents),
            Err(error) => self.fail(&error),
        }
    }

    fn unlock(&mut self) {
        let Stage::Locked(data) = &self.stage else {
            return;
        };
        match pom_bundle::open(data, &self.password.text()) {
            Ok(contents) => {
                self.error = None;
                self.stage = Stage::Loaded(contents);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn submit(&mut self, adapt: bool) {
        let Stage::Loaded(contents) = &self.stage else {
            return;
        };
        let applies = self.write_config || (self.store_secrets && !contents.secrets.is_empty());
        if !adapt && !applies {
            return;
        }
        self.result = Some(ModalResult::Submitted(Box::new(ImportConfig {
            contents: contents.clone(),
            write_config: self.write_config,
            store_secrets: self.store_secrets,
            adapt,
        })));
    }

    fn file_row(&self) -> Node {
        let colors = theme();
        let name = if self.file_name.is_empty() {
            "No file chosen".to_string()
        } else {
            self.file_name.clone()
        };
        let button = if self.file_name.is_empty() {
            "Choose File..."
        } else {
            "Change..."
        };
        div()
            .row()
            .items_center()
            .gap(8.0)
            .child(
                div().row().items_center().flex(1.0).child(
                    label(name)
                        .label_size(LabelSize::Small)
                        .mono()
                        .color(if self.file_name.is_empty() {
                            colors.text_muted
                        } else {
                            colors.text
                        })
                        .truncate(),
                ),
            )
            .child(outlined_button(
                CHOOSE_FILE,
                Some(IconKind::FolderOpen),
                button,
                true,
            ))
            .into()
    }

    fn preview(contents: &Contents) -> Node {
        let colors = theme();
        let lines: Vec<&str> = contents.config.lines().collect();
        let mut column = div()
            .col()
            .px(8.0)
            .py(6.0)
            .rounded(6.0)
            .bg(colors.editor_background)
            .border(1.0, colors.border_variant);
        for line in lines.iter().take(PREVIEW_LINES) {
            column = column.child(
                label(line.to_string())
                    .label_size(LabelSize::Small)
                    .mono()
                    .color(colors.text_muted)
                    .truncate(),
            );
        }
        if lines.len() > PREVIEW_LINES {
            column = column.child(
                label(format!("... {} more lines", lines.len() - PREVIEW_LINES))
                    .label_size(LabelSize::Small)
                    .color(colors.text_placeholder),
            );
        }
        column.into()
    }
}

impl WindowModal for ImportConfigModal {
    fn width(&self) -> f32 {
        match self.stage {
            Stage::Loaded(_) => PREVIEW_WIDTH,
            _ => WIDTH,
        }
    }

    fn render(&mut self) -> Node {
        let colors = theme();
        let width = self.width();
        let mut section = modal_section(10.0).child(self.file_row());
        let mut buttons = div().row().items_center().gap(4.0).child(modal_button(
            CANCEL,
            "Cancel",
            Some("escape"),
            true,
        ));
        let mut start = None;
        match &self.stage {
            Stage::Pick => {
                section = section.child(status_line(
                    IconKind::File,
                    colors.icon_muted,
                    "Pick a pom.yml or a .pombundle exported from another project",
                ));
            }
            Stage::Locked(_) => {
                section = section.child(self.password.render(
                    PASSWORD_FIELD,
                    true,
                    Some("sealed bundle"),
                    None,
                ));
                buttons = buttons.child(modal_button(
                    CONFIRM,
                    "Unlock",
                    Some("enter"),
                    !self.password.text().is_empty(),
                ));
            }
            Stage::Loaded(contents) => {
                section = section.child(Self::preview(contents));
                if !contents.secrets.is_empty() {
                    let names: Vec<&str> = contents.secrets.keys().map(String::as_str).collect();
                    section = section
                        .child(
                            label(format!("Secrets: {}", names.join(", ")))
                                .label_size(LabelSize::Small)
                                .color(colors.text_muted)
                                .truncate(),
                        )
                        .child(checkbox(
                            STORE_SECRETS,
                            self.store_secrets,
                            "Store these secrets in this project",
                        ));
                }
                section = section.child(checkbox(
                    WRITE_CONFIG,
                    self.write_config,
                    "Replace my pom.yml (kept as pom.yml.bak, split into pom.d)",
                ));
                start = Some(outlined_button(
                    ADAPT,
                    Some(IconKind::Sparkle),
                    "Adapt with Claude",
                    true,
                ));
                let applies =
                    self.write_config || (self.store_secrets && !contents.secrets.is_empty());
                buttons = buttons.child(modal_button(CONFIRM, "Apply", Some("enter"), applies));
            }
        }
        if let Some(error) = &self.error {
            section = section.child(
                label(error.clone())
                    .label_size(LabelSize::Small)
                    .color(colors.error),
            );
        }
        modal_frame(width)
            .child(modal_header("Import Config", Some(CLOSE)))
            .child(section)
            .child(modal_footer(start, buttons.into()))
            .into()
    }

    fn click(&mut self, id: u64) {
        match id {
            CLOSE | CANCEL => self.result = Some(ModalResult::Cancelled),
            CHOOSE_FILE => self.choose(),
            WRITE_CONFIG => self.write_config = !self.write_config,
            STORE_SECRETS => self.store_secrets = !self.store_secrets,
            ADAPT => self.submit(true),
            CONFIRM => match self.stage {
                Stage::Locked(_) => self.unlock(),
                _ => self.submit(false),
            },
            _ => {}
        }
    }

    fn key(&mut self, key: EditKey, shift: bool) -> bool {
        match key {
            EditKey::Escape => self.result = Some(ModalResult::Cancelled),
            EditKey::Enter => self.click(CONFIRM),
            key if matches!(self.stage, Stage::Locked(_)) => {
                self.password.field.key(key, shift);
            }
            _ => {}
        }
        true
    }

    fn text(&mut self, text: &str) -> bool {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() || !matches!(self.stage, Stage::Locked(_)) {
            return false;
        }
        self.password.field.insert(&typed);
        true
    }

    fn take_result(&mut self) -> Option<ModalResult> {
        self.result.take()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn submitted<T: Clone + 'static>(modal: &mut dyn WindowModal) -> Option<T> {
        match modal.take_result() {
            Some(ModalResult::Submitted(value)) => value.downcast_ref::<T>().cloned(),
            _ => None,
        }
    }

    #[test]
    fn export_needs_a_password_only_with_secrets() {
        let mut form = ExportConfigModal::new(2);
        form.key(EditKey::Enter, false);
        assert_eq!(
            submitted::<ExportConfig>(&mut form),
            Some(ExportConfig { password: None })
        );
        form.click(INCLUDE_SECRETS);
        form.key(EditKey::Enter, false);
        assert!(form.take_result().is_none());
        form.text("pw");
        let painted = ui::render(
            &form.render(),
            ui::Rect::new(0.0, 0.0, WIDTH, 800.0, ui::Rgba::TRANSPARENT),
        );
        let texts: Vec<String> = painted.texts.iter().map(|t| t.text.clone()).collect();
        assert!(texts.iter().any(|text| text == "**"), "{texts:?}");
        assert!(!texts.iter().any(|text| text == "pw"), "{texts:?}");
        form.key(EditKey::Enter, false);
        assert_eq!(
            submitted::<ExportConfig>(&mut form),
            Some(ExportConfig {
                password: Some("pw".into())
            })
        );
    }

    #[test]
    fn import_unlocks_a_sealed_bundle_then_applies_or_adapts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("shop.pombundle");
        let contents = Contents {
            config: "session: shop\n".into(),
            secrets: BTreeMap::from([("TOKEN".to_string(), "t".to_string())]),
        };
        std::fs::write(&file, pom_bundle::seal(&contents, "pw").expect("seal")).expect("write");
        let chosen = file.clone();
        let mut form = ImportConfigModal::new(Box::new(move || Some(chosen.clone())));
        form.click(CHOOSE_FILE);
        assert!(matches!(form.stage, Stage::Locked(_)));
        form.text("nope");
        form.key(EditKey::Enter, false);
        assert!(form
            .error
            .as_deref()
            .is_some_and(|error| error.contains("wrong password")));
        form.password.field.set_text("pw");
        form.key(EditKey::Enter, false);
        assert!(matches!(form.stage, Stage::Loaded(_)));

        form.click(WRITE_CONFIG);
        form.key(EditKey::Enter, false);
        assert_eq!(
            submitted::<ImportConfig>(&mut form),
            Some(ImportConfig {
                contents: contents.clone(),
                write_config: false,
                store_secrets: true,
                adapt: false,
            })
        );
        form.click(ADAPT);
        assert!(submitted::<ImportConfig>(&mut form).is_some_and(|import| import.adapt));
    }

    #[test]
    fn import_reports_an_unreadable_file() {
        let mut form =
            ImportConfigModal::new(Box::new(|| Some(PathBuf::from("/nonexistent/x.yml"))));
        form.click(CHOOSE_FILE);
        assert!(matches!(form.stage, Stage::Pick));
        assert!(form
            .error
            .as_deref()
            .is_some_and(|error| error.contains("could not read")));
    }
}
