//! The right-click menu of each kind of row, and what the destructive items ask before they run.

use pom_db::{Database, Engine, TableKind};

use crate::tree::{prefix_name, readable, Row};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Header,
    Refresh,
    CollapseAll,
    Collapse,
    CopyUrl,
    OpenClient,
    NewConsole,
    CopyName,
    CopyFromMain,
    ResetDatabase,
    AskAboutSchema,
    OpenData,
    NewConsoleWithSelect,
    CopySelect,
    ShowDdl,
    Truncate,
    DropTable,
    AskAboutTable,
    FilterByColumn,
    DistinctValues,
    OpenConsole,
    RenameConsole,
    ChangeDatabase,
    MoveConsole(String),
    DeleteConsole,
    OpenKeys,
    CopyPattern,
    DeleteKeys,
    CopyPath,
    DeleteFolder,
    OpenObject,
    DownloadObject,
    CopyPresignedUrl,
    DeleteObject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub label: String,
    pub action: Action,
    pub danger: bool,
    pub disabled: bool,
    /// A separator above it.
    pub sep: bool,
}

/// What the menu needs to know beyond the row.
pub(crate) struct Facts<'a> {
    pub database: Option<&'a Database>,
    /// Main's copy of this database exists to copy from.
    pub has_main_copy: bool,
    /// The repo's first Postgres database, for its connection and `psql`.
    pub repo_database: Option<&'a Database>,
}

#[derive(Default)]
struct Builder {
    entries: Vec<Entry>,
    separate: bool,
}

impl Builder {
    fn push(mut self, label: String, action: Action, danger: bool, disabled: bool) -> Builder {
        let sep = self.separate && !self.entries.is_empty();
        self.separate = false;
        self.entries.push(Entry {
            label,
            action,
            danger,
            disabled,
            sep,
        });
        self
    }

    fn header(self, text: String) -> Builder {
        self.push(text, Action::Header, false, true)
    }

    fn item(self, label: &str, action: Action) -> Builder {
        self.push(label.into(), action, false, false)
    }

    fn danger(self, label: &str, action: Action) -> Builder {
        self.push(label.into(), action, true, false)
    }

    fn disabled_if(mut self, disabled: bool) -> Builder {
        if let Some(last) = self.entries.last_mut() {
            last.disabled = disabled;
        }
        self
    }

    /// The next item draws a line above it.
    fn sep(mut self) -> Builder {
        self.separate = true;
        self
    }

    fn finish(self) -> Vec<Entry> {
        self.entries
    }
}

fn client_label(engine: Engine) -> &'static str {
    match engine {
        Engine::Redis => "Open redis-cli in Terminal",
        _ => "Open psql in Terminal",
    }
}

/// The menu for `row`; empty when the row has none.
pub(crate) fn entries(row: &Row, facts: &Facts<'_>) -> Vec<Entry> {
    let menu = Builder::default();
    let menu = match row {
        Row::Repo { repo, other, .. } => {
            let postgres = facts.repo_database;
            let menu = menu
                .header(repo.clone())
                .item("Refresh", Action::Refresh)
                .item("Collapse All", Action::CollapseAll);
            if *other {
                menu
            } else {
                menu.sep()
                    .item("Copy Connection URL", Action::CopyUrl)
                    .disabled_if(postgres.is_none())
                    .item("Open psql in Terminal", Action::OpenClient)
                    .disabled_if(postgres.is_none())
            }
        }
        Row::Database { .. } => {
            let Some(database) = facts.database else {
                return Vec::new();
            };
            let menu = menu.header(readable(database));
            match database.engine {
                Engine::Postgres => menu
                    .item("New Console", Action::NewConsole)
                    .item("Refresh", Action::Refresh)
                    .sep()
                    .item("Copy Name", Action::CopyName)
                    .item("Copy Connection URL", Action::CopyUrl)
                    .item("Open psql in Terminal", Action::OpenClient)
                    .sep()
                    .item("Copy Data from Main...", Action::CopyFromMain)
                    .disabled_if(!facts.has_main_copy)
                    .danger("Reset Database...", Action::ResetDatabase)
                    .sep()
                    .item("Ask Claude about this schema", Action::AskAboutSchema),
                Engine::Redis => menu
                    .item("New Console", Action::NewConsole)
                    .item("Refresh", Action::Refresh)
                    .sep()
                    .item("Copy Name", Action::CopyName)
                    .item("Copy Connection URL", Action::CopyUrl)
                    .item(client_label(Engine::Redis), Action::OpenClient),
                Engine::Minio => menu
                    .item("Refresh", Action::Refresh)
                    .sep()
                    .item("Copy Name", Action::CopyName)
                    .item("Copy Connection URL", Action::CopyUrl),
                _ => menu.item("Copy Name", Action::CopyName),
            }
        }
        Row::Group { kind, .. } => menu
            .header(
                if *kind == TableKind::View {
                    "views"
                } else {
                    "tables"
                }
                .into(),
            )
            .item("Refresh", Action::Refresh)
            .item("Collapse", Action::Collapse),
        Row::Table { table, .. } => {
            let menu = menu
                .header(table.qualified())
                .item("Open Data", Action::OpenData)
                .item("New Console with SELECT", Action::NewConsoleWithSelect)
                .sep()
                .item("Copy Name", Action::CopyName)
                .item("Copy SELECT Statement", Action::CopySelect)
                .item("Show DDL", Action::ShowDdl)
                .sep();
            let menu = if table.kind == TableKind::View {
                menu
            } else {
                menu.danger("Truncate...", Action::Truncate)
            };
            menu.danger("Drop Table...", Action::DropTable)
                .sep()
                .item("Ask Claude about this table", Action::AskAboutTable)
        }
        Row::Column { table, column, .. } => menu
            .header(format!("{}.{}", table.qualified(), column.name))
            .item("Copy Name", Action::CopyName)
            .item("Filter Data by this Column", Action::FilterByColumn)
            .item("Show Distinct Values", Action::DistinctValues),
        Row::Console { console, .. } => menu
            .header(console.title.clone())
            .item("Open", Action::OpenConsole)
            .item("Rename", Action::RenameConsole)
            .item("Change Database...", Action::ChangeDatabase)
            .sep()
            .danger("Delete Console...", Action::DeleteConsole),
        Row::Keyspace { table, .. } => menu
            .header(format!("{}:*", table.name))
            .item("Open Keys", Action::OpenKeys)
            .item("Copy Pattern", Action::CopyPattern)
            .item(client_label(Engine::Redis), Action::OpenClient)
            .sep()
            .danger("Delete Matching Keys...", Action::DeleteKeys),
        Row::Bucket { bucket, .. } => menu
            .header(bucket.clone())
            .item("Copy Path", Action::CopyPath)
            .item("Refresh", Action::Refresh),
        Row::Prefix { bucket, prefix, .. } => menu
            .header(format!("{bucket}/{prefix}"))
            .item("Copy Path", Action::CopyPath)
            .item("Refresh", Action::Refresh)
            .sep()
            .danger("Delete Folder...", Action::DeleteFolder),
        Row::Object { bucket, object, .. } => menu
            .header(format!("{bucket}/{}", object.key))
            .item("Open", Action::OpenObject)
            .item("Download", Action::DownloadObject)
            .item("Copy Presigned URL", Action::CopyPresignedUrl)
            .item("Copy Path", Action::CopyPath)
            .sep()
            .danger("Delete...", Action::DeleteObject),
        Row::Section { .. } | Row::More { .. } | Row::Note { .. } | Row::Failure { .. } => {
            return Vec::new()
        }
    };
    menu.finish()
}

/// What a destructive item asks before it runs: the question, what is lost, and the button that goes ahead.
pub(crate) struct Confirmation {
    pub message: String,
    pub detail: String,
    pub button: &'static str,
}

pub(crate) fn confirmation(
    action: &Action,
    row: &Row,
    database: Option<&Database>,
) -> Option<Confirmation> {
    let place = database.map(readable).unwrap_or_default();
    let name = database
        .map(|database| database.name.clone())
        .unwrap_or_default();
    let confirm = |message: String, detail: String, button| {
        Some(Confirmation {
            message,
            detail,
            button,
        })
    };
    match (action, row) {
        (Action::ResetDatabase, _) => confirm(
            format!("Reset the {place} database?"),
            format!("{name} is dropped and created again, empty. Every row in it is lost."),
            "Reset",
        ),
        (Action::CopyFromMain, _) => confirm(
            format!("Copy main's data into {place}?"),
            format!(
                "Every table of {name} is replaced with a copy of main's database. Changes made in this \
                 workspace's database are lost."
            ),
            "Copy from Main",
        ),
        (Action::Truncate, Row::Table { table, .. }) => confirm(
            format!("Truncate {}?", table.qualified()),
            format!(
                "Every row of {} in {place} is deleted. The table stays.",
                table.qualified()
            ),
            "Truncate",
        ),
        (Action::DropTable, Row::Table { table, .. }) => confirm(
            format!("Drop {}?", table.qualified()),
            format!(
                "{} and all its rows are removed from {place}.",
                table.qualified()
            ),
            "Drop",
        ),
        (Action::DeleteConsole, Row::Console { console, .. }) => confirm(
            format!("Delete the console \"{}\"?", console.title),
            "Its SQL is removed and cannot be recovered.".into(),
            "Delete",
        ),
        (Action::DeleteKeys, Row::Keyspace { table, .. }) => confirm(
            format!("Delete every {}:* key?", table.name),
            format!(
                "All keys matching {}:* in {place} are deleted, about {}.",
                table.name,
                table.count.unwrap_or(0)
            ),
            "Delete",
        ),
        (Action::DeleteFolder, Row::Prefix { bucket, prefix, .. }) => confirm(
            format!("Delete the folder {}?", prefix_name(prefix)),
            format!("Every object under {bucket}/{prefix} is deleted."),
            "Delete",
        ),
        (Action::DeleteObject, Row::Object { bucket, object, .. }) => confirm(
            format!("Delete {}?", object.name()),
            format!(
                "{bucket}/{} is removed from the bucket and cannot be recovered.",
                object.key
            ),
            "Delete",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pom_db::object_storage::ObjectEntry;
    use pom_db::Console;

    use super::*;
    use crate::tree::tests::{database, shared, table};
    use crate::tree::Status;

    fn labels(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| {
                let mut text = entry.label.clone();
                if entry.sep {
                    text.insert_str(0, "| ");
                }
                if entry.danger {
                    text.push_str(" !");
                }
                if entry.disabled && entry.action != Action::Header {
                    text.push_str(" (off)");
                }
                text
            })
            .collect()
    }

    fn facts(database: Option<&Database>) -> Facts<'_> {
        Facts {
            database,
            has_main_copy: false,
            repo_database: database,
        }
    }

    fn database_row(index: usize) -> Row {
        Row::Database {
            index,
            repo: "api".into(),
            open: false,
            shared_with: Vec::new(),
            status: Status::Connected,
        }
    }

    #[test]
    fn each_kind_of_row_has_its_own_menu() {
        let dev = database("myproject_api", Engine::Postgres, "api", "dev");
        assert_eq!(
            labels(&entries(&database_row(0), &facts(Some(&dev)))),
            [
                "api > dev",
                "New Console",
                "Refresh",
                "| Copy Name",
                "Copy Connection URL",
                "Open psql in Terminal",
                "| Copy Data from Main... (off)",
                "Reset Database... !",
                "| Ask Claude about this schema",
            ]
        );
        let users = Row::Table {
            index: 0,
            repo: "api".into(),
            table: table("users", TableKind::Table, None),
            open: false,
            has_columns: true,
            hit: None,
        };
        assert_eq!(
            labels(&entries(&users, &facts(Some(&dev)))),
            [
                "users",
                "Open Data",
                "New Console with SELECT",
                "| Copy Name",
                "Copy SELECT Statement",
                "Show DDL",
                "| Truncate... !",
                "Drop Table... !",
                "| Ask Claude about this table",
            ]
        );
        let repo = Row::Repo {
            repo: "api".into(),
            other: false,
            open: true,
            engines: Vec::new(),
            failed: false,
        };
        assert_eq!(
            labels(&entries(&repo, &facts(None))),
            [
                "api",
                "Refresh",
                "Collapse All",
                "| Copy Connection URL (off)",
                "Open psql in Terminal (off)",
            ]
        );
        let console = Row::Console {
            console: Console {
                title: "query 1".into(),
                ..Console::default()
            },
            place: "api > dev".into(),
        };
        assert_eq!(
            labels(&entries(&console, &facts(None))),
            [
                "query 1",
                "Open",
                "Rename",
                "Change Database...",
                "| Delete Console... !"
            ]
        );
        let redis = shared("redis", Engine::Redis, &["api"]);
        let keys = Row::Keyspace {
            index: 1,
            repo: "api".into(),
            table: table("session", TableKind::Keyspace, Some(214)),
            hit: None,
        };
        assert_eq!(
            labels(&entries(&keys, &facts(Some(&redis)))),
            [
                "session:*",
                "Open Keys",
                "Copy Pattern",
                "Open redis-cli in Terminal",
                "| Delete Matching Keys... !"
            ]
        );
        let object = Row::Object {
            index: 2,
            repo: "api".into(),
            bucket: "files".into(),
            object: ObjectEntry {
                key: "uploads/a.png".into(),
                ..ObjectEntry::default()
            },
            depth: 4,
            hit: None,
        };
        assert_eq!(
            labels(&entries(&object, &facts(None))),
            [
                "files/uploads/a.png",
                "Open",
                "Download",
                "Copy Presigned URL",
                "Copy Path",
                "| Delete... !"
            ]
        );
        let note = Row::Note {
            depth: 2,
            text: "Loading...".into(),
            error: false,
        };
        assert!(entries(&note, &facts(None)).is_empty());
    }

    #[test]
    fn every_danger_item_names_what_it_loses() {
        let dev = database("myproject_api", Engine::Postgres, "api", "dev");
        let users = Row::Table {
            index: 0,
            repo: "api".into(),
            table: table("users", TableKind::Table, None),
            open: false,
            has_columns: true,
            hit: None,
        };
        for entry in entries(&users, &facts(Some(&dev)))
            .iter()
            .filter(|entry| entry.danger)
        {
            let asked = confirmation(&entry.action, &users, Some(&dev));
            assert!(
                asked
                    .as_ref()
                    .is_some_and(|asked| asked.detail.contains("users")),
                "{}",
                entry.label
            );
        }
        let reset = confirmation(&Action::ResetDatabase, &database_row(0), Some(&dev));
        assert!(
            reset.is_some_and(|asked| asked.message == "Reset the api > dev database?"
                && asked.detail.contains("myproject_api"))
        );
        assert!(confirmation(&Action::CopyName, &users, Some(&dev)).is_none());
    }
}
