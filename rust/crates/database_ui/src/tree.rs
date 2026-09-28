//! What the Database panel lists, as flat rows: the consoles, then each repo with its databases and the shared
//! services it uses, then the services no repo uses. The filter narrows tables and columns across everything
//! loaded and opens the branches that hold a hit.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pom_db::object_storage::{Listing, ObjectEntry, PrefixStats};
use pom_db::{Column, Console, Database, Engine, Schema, Table, TableKind};

use crate::failure::Failure;
use crate::Pending;

const SEPARATOR: char = '\u{1f}';
pub(crate) const OTHER_SERVICES: &str = "Other services";

pub(crate) enum Opened {
    Schema(Schema),
    Buckets(Vec<String>),
    Failed(Box<Failure>),
}

pub(crate) enum Loaded {
    Loading(Pending<Opened>),
    Schema(Schema),
    Buckets(Vec<String>),
    Failed(Box<Failure>),
}

/// One prefix of a bucket, as far as it was listed.
#[derive(Default)]
pub(crate) struct Folder {
    pub prefixes: Vec<String>,
    pub objects: Vec<ObjectEntry>,
    /// Where the next page starts, when there is one.
    pub next: Option<String>,
    pub loading: Option<Pending<Listing>>,
    pub error: Option<String>,
}

impl Folder {
    pub(crate) fn listed(listing: Listing) -> Folder {
        Folder {
            prefixes: listing.prefixes,
            objects: listing.objects,
            next: listing.next,
            loading: None,
            error: None,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.prefixes.is_empty() && self.objects.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Connected,
    Failed { warning: bool },
    NotConnected,
    Loading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Consoles,
    Databases,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Section {
        section: Section,
        count: Option<usize>,
        open: bool,
    },
    Console {
        console: Console,
        /// The database it runs on, readable ("api > dev").
        place: String,
    },
    Repo {
        repo: String,
        other: bool,
        open: bool,
        engines: Vec<Engine>,
        failed: bool,
    },
    Database {
        index: usize,
        repo: String,
        open: bool,
        shared_with: Vec<String>,
        status: Status,
    },
    Group {
        index: usize,
        repo: String,
        kind: TableKind,
        count: usize,
        open: bool,
    },
    Table {
        index: usize,
        repo: String,
        table: Table,
        open: bool,
        has_columns: bool,
        hit: Option<Range<usize>>,
    },
    Column {
        index: usize,
        repo: String,
        table: Table,
        column: Column,
        hit: Option<Range<usize>>,
    },
    Keyspace {
        index: usize,
        repo: String,
        table: Table,
        hit: Option<Range<usize>>,
    },
    Bucket {
        index: usize,
        repo: String,
        bucket: String,
        open: bool,
    },
    Prefix {
        index: usize,
        repo: String,
        bucket: String,
        prefix: String,
        depth: usize,
        open: bool,
        stats: Option<PrefixStats>,
        hit: Option<Range<usize>>,
    },
    Object {
        index: usize,
        repo: String,
        bucket: String,
        object: ObjectEntry,
        depth: usize,
        hit: Option<Range<usize>>,
    },
    More {
        index: usize,
        bucket: String,
        prefix: String,
        depth: usize,
        loading: bool,
    },
    Note {
        depth: usize,
        text: String,
        error: bool,
    },
    Failure {
        index: usize,
    },
}

impl Row {
    /// How far in the row sits: sections and repos at 0.
    pub(crate) fn depth(&self) -> usize {
        match self {
            Row::Section { .. } | Row::Repo { .. } => 0,
            Row::Console { .. } | Row::Database { .. } => 1,
            Row::Group { .. } | Row::Keyspace { .. } | Row::Bucket { .. } | Row::Failure { .. } => {
                2
            }
            Row::Table { .. } => 3,
            Row::Column { .. } => 4,
            Row::Prefix { depth, .. }
            | Row::Object { depth, .. }
            | Row::More { depth, .. }
            | Row::Note { depth, .. } => *depth,
        }
    }

    /// Stable identity across rebuilds, for the selection.
    pub(crate) fn key(&self) -> String {
        match self {
            Row::Section { section, .. } => format!("s{SEPARATOR}{section:?}"),
            Row::Console { console, .. } => format!("c{SEPARATOR}{}", console.id),
            Row::Repo { repo, .. } => repo_key(repo),
            Row::Database { index, repo, .. } => format!("d{SEPARATOR}{repo}{SEPARATOR}{index}"),
            Row::Group {
                index, repo, kind, ..
            } => format!("g{SEPARATOR}{repo}{SEPARATOR}{index}{SEPARATOR}{kind:?}"),
            Row::Table {
                index, repo, table, ..
            } => format!(
                "t{SEPARATOR}{repo}{SEPARATOR}{index}{SEPARATOR}{}",
                table.qualified()
            ),
            Row::Column {
                index,
                repo,
                table,
                column,
                ..
            } => format!(
                "k{SEPARATOR}{repo}{SEPARATOR}{index}{SEPARATOR}{}{SEPARATOR}{}",
                table.qualified(),
                column.name
            ),
            Row::Keyspace {
                index, repo, table, ..
            } => format!(
                "y{SEPARATOR}{repo}{SEPARATOR}{index}{SEPARATOR}{}",
                table.name
            ),
            Row::Bucket {
                index,
                repo,
                bucket,
                ..
            } => format!("b{SEPARATOR}{repo}{SEPARATOR}{index}{SEPARATOR}{bucket}"),
            Row::Prefix {
                index,
                repo,
                bucket,
                prefix,
                ..
            } => format!(
                "p{SEPARATOR}{repo}{SEPARATOR}{index}{SEPARATOR}{bucket}{SEPARATOR}{prefix}"
            ),
            Row::Object {
                index,
                repo,
                bucket,
                object,
                ..
            } => format!(
                "o{SEPARATOR}{repo}{SEPARATOR}{index}{SEPARATOR}{bucket}{SEPARATOR}{}",
                object.key
            ),
            Row::More {
                index,
                bucket,
                prefix,
                ..
            } => format!("m{SEPARATOR}{index}{SEPARATOR}{bucket}{SEPARATOR}{prefix}"),
            Row::Note { depth, text, .. } => format!("n{SEPARATOR}{depth}{SEPARATOR}{text}"),
            Row::Failure { index } => format!("f{SEPARATOR}{index}"),
        }
    }

    /// The database the row belongs to (its index in the list).
    pub(crate) fn database(&self) -> Option<usize> {
        match self {
            Row::Database { index, .. }
            | Row::Group { index, .. }
            | Row::Table { index, .. }
            | Row::Column { index, .. }
            | Row::Keyspace { index, .. }
            | Row::Bucket { index, .. }
            | Row::Prefix { index, .. }
            | Row::Object { index, .. }
            | Row::More { index, .. }
            | Row::Failure { index } => Some(*index),
            _ => None,
        }
    }
}

pub(crate) const CONSOLES_KEY: &str = "consoles";

pub(crate) fn repo_key(repo: &str) -> String {
    format!("r{SEPARATOR}{repo}")
}

pub(crate) fn database_key(repo: &str, name: &str) -> String {
    format!("d{SEPARATOR}{repo}{SEPARATOR}{name}")
}

pub(crate) fn group_key(repo: &str, name: &str, kind: TableKind) -> String {
    format!("g{SEPARATOR}{repo}{SEPARATOR}{name}{SEPARATOR}{kind:?}")
}

pub(crate) fn table_key(repo: &str, name: &str, table: &Table) -> String {
    format!(
        "t{SEPARATOR}{repo}{SEPARATOR}{name}{SEPARATOR}{}",
        table.qualified()
    )
}

pub(crate) fn bucket_key(repo: &str, name: &str, bucket: &str) -> String {
    format!("b{SEPARATOR}{repo}{SEPARATOR}{name}{SEPARATOR}{bucket}")
}

pub(crate) fn prefix_key(repo: &str, name: &str, bucket: &str, prefix: &str) -> String {
    format!("p{SEPARATOR}{repo}{SEPARATOR}{name}{SEPARATOR}{bucket}{SEPARATOR}{prefix}")
}

/// Where a listed prefix of a bucket is kept (shared by every repo that shows the service).
pub(crate) fn folder_key(name: &str, bucket: &str, prefix: &str) -> String {
    format!("{name}{SEPARATOR}{bucket}{SEPARATOR}{prefix}")
}

/// Where a key is open by default: repos, the consoles and a database's tables are; the rest is folded.
fn open_by_default(key: &str) -> bool {
    key == CONSOLES_KEY
        || key.starts_with(&format!("r{SEPARATOR}"))
        || (key.starts_with(&format!("g{SEPARATOR}")) && key.ends_with("Table"))
}

/// `1284` -> `1.3k`, `88210` -> `88k`.
pub(crate) fn format_count(count: usize) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 10_000 {
        format!("{:.0}k", count as f64 / 1000.0)
    } else if count >= 1000 {
        format!("{:.1}k", count as f64 / 1000.0)
    } else {
        count.to_string()
    }
}

/// Where the filter text sits in `text`, ignoring case.
pub(crate) fn find(needle: &str, text: &str) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths outside ASCII; only then does a hit fall back to "somewhere".
    if lower.len() != text.len() {
        return lower.contains(needle).then_some(0..text.len());
    }
    lower.find(needle).map(|start| start..start + needle.len())
}

/// The name shown for the database a console runs on: `api > dev`, or the service name for a shared one.
pub(crate) fn readable(database: &Database) -> String {
    if database.is_shared() {
        database.label.clone()
    } else {
        format!("{} > {}", database.repo, database.label)
    }
}

#[derive(Default)]
pub(crate) struct Model {
    pub databases: Vec<Database>,
    /// Repo aliases in config order.
    pub repos: Vec<String>,
    pub consoles: Vec<Console>,
    pub loaded: HashMap<String, Loaded>,
    pub folders: HashMap<String, Folder>,
    /// Totals of a listed prefix, by `folder_key`; `None` while counting or when counting failed.
    pub stats: HashMap<String, Option<PrefixStats>>,
    /// Keys opened from their default, and keys folded from theirs.
    pub opened: HashSet<String>,
    pub folded: HashSet<String>,
    pub filter: String,
}

struct Build<'a> {
    model: &'a Model,
    needle: String,
    rows: Vec<Row>,
}

impl Model {
    pub(crate) fn is_open(&self, key: &str) -> bool {
        if open_by_default(key) {
            !self.folded.contains(key)
        } else {
            self.opened.contains(key)
        }
    }

    pub(crate) fn set_open(&mut self, key: &str, open: bool) {
        let default = open_by_default(key);
        if open == default {
            self.opened.remove(key);
            self.folded.remove(key);
        } else if default {
            self.folded.insert(key.to_string());
        } else {
            self.opened.insert(key.to_string());
        }
    }

    pub(crate) fn toggle(&mut self, key: &str) {
        let open = self.is_open(key);
        self.set_open(key, !open);
    }

    /// Folds everything back to how it starts, repos included.
    pub(crate) fn collapse_all(&mut self) {
        self.opened.clear();
        self.folded.clear();
        for repo in self.repos.clone() {
            self.folded.insert(repo_key(&repo));
        }
        self.folded.insert(repo_key(OTHER_SERVICES));
    }

    pub(crate) fn database(&self, name: &str) -> Option<&Database> {
        self.databases.iter().find(|database| database.name == name)
    }

    pub(crate) fn place_of(&self, database_name: &str) -> String {
        self.database(database_name)
            .map_or_else(|| database_name.to_string(), readable)
    }

    pub(crate) fn status(&self, name: &str) -> Status {
        match self.loaded.get(name) {
            None => Status::NotConnected,
            Some(Loaded::Loading(_)) => Status::Loading,
            Some(Loaded::Failed(failure)) => Status::Failed {
                warning: failure.is_warning(),
            },
            Some(Loaded::Schema(_) | Loaded::Buckets(_)) => Status::Connected,
        }
    }

    pub(crate) fn schema(&self, name: &str) -> Option<&Schema> {
        match self.loaded.get(name) {
            Some(Loaded::Schema(schema)) => Some(schema),
            _ => None,
        }
    }

    /// The repos to list, each with the databases under it (its own, then the shared ones it uses), and the
    /// services no repo uses last.
    pub(crate) fn groups(&self) -> Vec<(String, bool, Vec<usize>)> {
        let mut repos = self.repos.clone();
        for database in &self.databases {
            let owners = if database.is_shared() {
                database.used_by.clone()
            } else {
                vec![database.repo.clone()]
            };
            for owner in owners {
                if !repos.contains(&owner) {
                    repos.push(owner);
                }
            }
        }
        let mut groups: Vec<(String, bool, Vec<usize>)> = Vec::new();
        for repo in repos {
            let own = self
                .databases
                .iter()
                .enumerate()
                .filter(|(_, database)| !database.is_shared() && database.repo == repo);
            let shared =
                self.databases.iter().enumerate().filter(|(_, database)| {
                    database.is_shared() && database.used_by.contains(&repo)
                });
            let indexes: Vec<usize> = own.chain(shared).map(|(index, _)| index).collect();
            if !indexes.is_empty() {
                groups.push((repo, false, indexes));
            }
        }
        let unused: Vec<usize> = self
            .databases
            .iter()
            .enumerate()
            .filter(|(_, database)| database.is_shared() && database.used_by.is_empty())
            .map(|(index, _)| index)
            .collect();
        if !unused.is_empty() {
            groups.push((OTHER_SERVICES.to_string(), true, unused));
        }
        groups
    }

    /// The rows to draw, and how many table, column, key and object names the filter found.
    pub(crate) fn rows(&self) -> (Vec<Row>, usize) {
        let mut build = Build {
            model: self,
            needle: self.filter.trim().to_lowercase(),
            rows: Vec::new(),
        };
        build.consoles();
        let found = build.found();
        build.databases();
        (build.rows, found)
    }
}

impl Build<'_> {
    fn filtering(&self) -> bool {
        !self.needle.is_empty()
    }

    fn hit(&self, text: &str) -> Option<Range<usize>> {
        find(&self.needle, text)
    }

    fn consoles(&mut self) {
        let model = self.model;
        let open = model.is_open(CONSOLES_KEY);
        self.rows.push(Row::Section {
            section: Section::Consoles,
            count: Some(model.consoles.len()),
            open,
        });
        if !open || self.filtering() {
            return;
        }
        for console in &model.consoles {
            self.rows.push(Row::Console {
                console: console.clone(),
                place: model.place_of(&console.database),
            });
        }
    }

    /// Names the filter finds, each database counted once however many repos list it.
    fn found(&self) -> usize {
        if !self.filtering() {
            return 0;
        }
        let model = self.model;
        let mut found = 0;
        for database in &model.databases {
            if let Some(schema) = model.schema(&database.name) {
                found += schema
                    .tables
                    .iter()
                    .filter(|table| self.hit(&table.qualified()).is_some())
                    .count();
                found += schema
                    .columns
                    .iter()
                    .filter(|column| self.hit(&column.name).is_some())
                    .count();
            }
        }
        found += model
            .folders
            .values()
            .flat_map(|folder| {
                folder
                    .prefixes
                    .iter()
                    .map(|prefix| prefix_name(prefix))
                    .chain(folder.objects.iter().map(ObjectEntry::name))
            })
            .filter(|name| self.hit(name).is_some())
            .count();
        found
    }

    fn table_hits(&self, schema: &Schema, table: &Table) -> bool {
        self.hit(&table.qualified()).is_some()
            || schema
                .columns_of(table)
                .any(|column| self.hit(&column.name).is_some())
    }

    fn folder_hits(&self, name: &str, bucket: &str, prefix: &str, depth: usize) -> bool {
        let Some(folder) = self.model.folders.get(&folder_key(name, bucket, prefix)) else {
            return false;
        };
        folder
            .objects
            .iter()
            .any(|object| self.hit(object.name()).is_some())
            || folder.prefixes.iter().any(|child| {
                self.hit(prefix_name(child)).is_some()
                    || (depth < 32 && self.folder_hits(name, bucket, child, depth + 1))
            })
    }

    fn database_hits(&self, database: &Database) -> bool {
        match self.model.loaded.get(&database.name) {
            Some(Loaded::Schema(schema)) => schema
                .tables
                .iter()
                .any(|table| self.table_hits(schema, table)),
            Some(Loaded::Buckets(buckets)) => buckets
                .iter()
                .any(|bucket| self.folder_hits(&database.name, bucket, "", 0)),
            _ => false,
        }
    }

    fn databases(&mut self) {
        let model = self.model;
        self.rows.push(Row::Section {
            section: Section::Databases,
            count: None,
            open: true,
        });
        for (repo, other, indexes) in model.groups() {
            let shown: Vec<usize> = indexes
                .iter()
                .copied()
                .filter(|index| {
                    !self.filtering()
                        || model
                            .databases
                            .get(*index)
                            .is_some_and(|database| self.database_hits(database))
                })
                .collect();
            if shown.is_empty() {
                continue;
            }
            let open = self.filtering() || model.is_open(&repo_key(&repo));
            let mut engines: Vec<Engine> = Vec::new();
            for index in &indexes {
                if let Some(database) = model.databases.get(*index) {
                    if !engines.contains(&database.engine) {
                        engines.push(database.engine);
                    }
                }
            }
            let failed = indexes.iter().any(|index| {
                model.databases.get(*index).is_some_and(|database| {
                    matches!(model.status(&database.name), Status::Failed { .. })
                })
            });
            self.rows.push(Row::Repo {
                repo: repo.clone(),
                other,
                open,
                engines,
                failed,
            });
            if !open {
                continue;
            }
            for index in shown {
                self.database(&repo, index);
            }
        }
    }

    fn database(&mut self, repo: &str, index: usize) {
        let model = self.model;
        let Some(database) = model.databases.get(index) else {
            return;
        };
        let status = model.status(&database.name);
        let open = model.is_open(&database_key(repo, &database.name))
            || (self.filtering() && status == Status::Connected);
        let shared_with = if database.is_shared() {
            database
                .used_by
                .iter()
                .filter(|user| user.as_str() != repo)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        self.rows.push(Row::Database {
            index,
            repo: repo.to_string(),
            open,
            shared_with,
            status,
        });
        if !open {
            return;
        }
        let note = |text: &str| Row::Note {
            depth: 2,
            text: text.to_string(),
            error: false,
        };
        if !database.engine.browsable() {
            self.rows.push(note(&format!(
                "The panel cannot browse {} yet",
                database.engine.title()
            )));
            return;
        }
        match model.loaded.get(&database.name) {
            None | Some(Loaded::Loading(_)) => self.rows.push(note("Connecting...")),
            Some(Loaded::Failed(_)) => self.rows.push(Row::Failure { index }),
            Some(Loaded::Schema(schema)) if database.engine == Engine::Redis => {
                self.keyspaces(repo, index, schema)
            }
            Some(Loaded::Schema(schema)) => self.schema(repo, index, &database.name, schema),
            Some(Loaded::Buckets(buckets)) => self.buckets(repo, index, &database.name, buckets),
        }
    }

    fn keyspaces(&mut self, repo: &str, index: usize, schema: &Schema) {
        let mut any = false;
        for table in &schema.tables {
            let hit = self.hit(&table.name);
            if self.filtering() && hit.is_none() {
                continue;
            }
            any = true;
            self.rows.push(Row::Keyspace {
                index,
                repo: repo.to_string(),
                table: table.clone(),
                hit,
            });
        }
        if !any {
            self.rows.push(Row::Note {
                depth: 2,
                text: "No keys".into(),
                error: false,
            });
        }
    }

    fn schema(&mut self, repo: &str, index: usize, name: &str, schema: &Schema) {
        if schema.tables.is_empty() {
            self.rows.push(Row::Note {
                depth: 2,
                text: "No tables".into(),
                error: false,
            });
            return;
        }
        for kind in [TableKind::Table, TableKind::View] {
            let tables: Vec<&Table> = schema
                .tables
                .iter()
                .filter(|table| table.kind == kind)
                .filter(|table| !self.filtering() || self.table_hits(schema, table))
                .collect();
            if tables.is_empty() {
                continue;
            }
            let open = self.filtering() || self.model.is_open(&group_key(repo, name, kind));
            self.rows.push(Row::Group {
                index,
                repo: repo.to_string(),
                kind,
                count: tables.len(),
                open,
            });
            if !open {
                continue;
            }
            for table in tables {
                self.table(repo, index, name, schema, table);
            }
        }
    }

    fn table(&mut self, repo: &str, index: usize, name: &str, schema: &Schema, table: &Table) {
        let columns: Vec<&Column> = schema.columns_of(table).collect();
        let column_hit = columns
            .iter()
            .any(|column| self.hit(&column.name).is_some());
        let open = self.model.is_open(&table_key(repo, name, table)) || column_hit;
        self.rows.push(Row::Table {
            index,
            repo: repo.to_string(),
            table: table.clone(),
            open,
            has_columns: !columns.is_empty(),
            hit: self.hit(&table.qualified()),
        });
        if !open {
            return;
        }
        for column in columns {
            let hit = self.hit(&column.name);
            if column_hit && hit.is_none() {
                continue;
            }
            self.rows.push(Row::Column {
                index,
                repo: repo.to_string(),
                table: table.clone(),
                column: column.clone(),
                hit,
            });
        }
    }

    fn buckets(&mut self, repo: &str, index: usize, name: &str, buckets: &[String]) {
        if buckets.is_empty() {
            self.rows.push(Row::Note {
                depth: 2,
                text: "No buckets".into(),
                error: false,
            });
            return;
        }
        for bucket in buckets {
            if self.filtering() && !self.folder_hits(name, bucket, "", 0) {
                continue;
            }
            let open = self.filtering() || self.model.is_open(&bucket_key(repo, name, bucket));
            self.rows.push(Row::Bucket {
                index,
                repo: repo.to_string(),
                bucket: bucket.clone(),
                open,
            });
            if open {
                self.folder(repo, index, name, bucket, "", 3);
            }
        }
    }

    fn folder(
        &mut self,
        repo: &str,
        index: usize,
        name: &str,
        bucket: &str,
        prefix: &str,
        depth: usize,
    ) {
        let model = self.model;
        let note = |text: String, error: bool| Row::Note { depth, text, error };
        let Some(folder) = model.folders.get(&folder_key(name, bucket, prefix)) else {
            self.rows.push(note("Loading...".into(), false));
            return;
        };
        if let Some(error) = &folder.error {
            self.rows.push(note(error.clone(), true));
        }
        if folder.is_empty() {
            if folder.loading.is_some() {
                self.rows.push(note("Loading...".into(), false));
            } else if folder.error.is_none() {
                self.rows.push(note("Empty".into(), false));
            }
            return;
        }
        for child in &folder.prefixes {
            let hit = self.hit(prefix_name(child));
            let inside = self.filtering() && self.folder_hits(name, bucket, child, 0);
            if self.filtering() && hit.is_none() && !inside {
                continue;
            }
            let open = inside || model.is_open(&prefix_key(repo, name, bucket, child));
            self.rows.push(Row::Prefix {
                index,
                repo: repo.to_string(),
                bucket: bucket.to_string(),
                prefix: child.clone(),
                depth,
                open,
                stats: model
                    .stats
                    .get(&folder_key(name, bucket, child))
                    .copied()
                    .flatten(),
                hit,
            });
            if open && depth < 32 {
                self.folder(repo, index, name, bucket, child, depth + 1);
            }
        }
        for object in &folder.objects {
            let hit = self.hit(object.name());
            if self.filtering() && hit.is_none() {
                continue;
            }
            self.rows.push(Row::Object {
                index,
                repo: repo.to_string(),
                bucket: bucket.to_string(),
                object: object.clone(),
                depth,
                hit,
            });
        }
        if folder.next.is_some() && !self.filtering() {
            self.rows.push(Row::More {
                index,
                bucket: bucket.to_string(),
                prefix: prefix.to_string(),
                depth,
                loading: folder.loading.is_some(),
            });
        }
    }
}

/// `uploads/avatars/` -> `avatars`.
pub(crate) fn prefix_name(prefix: &str) -> &str {
    prefix
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(prefix)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use pom_db::SHARED;

    pub(crate) fn database(name: &str, engine: Engine, repo: &str, label: &str) -> Database {
        Database {
            name: name.into(),
            engine,
            repo: repo.into(),
            label: label.into(),
            used_by: Vec::new(),
        }
    }

    pub(crate) fn shared(name: &str, engine: Engine, used_by: &[&str]) -> Database {
        Database {
            used_by: used_by.iter().map(|user| user.to_string()).collect(),
            ..database(name, engine, SHARED, name)
        }
    }

    pub(crate) fn table(name: &str, kind: TableKind, count: Option<usize>) -> Table {
        Table {
            schema: "public".into(),
            name: name.into(),
            kind,
            count,
        }
    }

    pub(crate) fn column(table: &str, name: &str, primary_key: bool) -> Column {
        Column {
            schema: "public".into(),
            table: table.into(),
            name: name.into(),
            data_type: "bigint".into(),
            primary_key,
            references: None,
        }
    }

    pub(crate) fn model() -> Model {
        Model {
            databases: vec![
                database("myproject_api", Engine::Postgres, "api", "dev"),
                database("myproject_api_test", Engine::Postgres, "api", "test"),
                database("myproject_web", Engine::Postgres, "web", "main"),
                shared("redis", Engine::Redis, &["api", "web"]),
                shared("files", Engine::Minio, &["api"]),
                shared("queue", Engine::Rabbitmq, &[]),
            ],
            repos: vec!["api".into(), "web".into()],
            ..Model::default()
        }
    }

    pub(crate) fn users_schema() -> Schema {
        Schema {
            tables: vec![
                table("users", TableKind::Table, Some(1284)),
                table("login_tokens", TableKind::Table, Some(37)),
                table("orders", TableKind::Table, None),
                table("active_users", TableKind::View, None),
            ],
            columns: vec![
                column("users", "id", true),
                column("users", "email", false),
                column("login_tokens", "id", true),
                Column {
                    references: Some("users".into()),
                    ..column("login_tokens", "user_id", false)
                },
                column("login_tokens", "expires_at", false),
                column("orders", "id", true),
            ],
        }
    }

    fn outline(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let indent = "  ".repeat(row.depth());
                match row {
                    Row::Section { section, .. } => format!("{section:?}"),
                    Row::Console { console, place } => {
                        format!("{indent}{} @ {place}", console.title)
                    }
                    Row::Repo { repo, .. } => format!("{indent}{repo}"),
                    Row::Database {
                        index, shared_with, ..
                    } => {
                        let mut text = format!("{indent}{}", model().databases[*index].label);
                        if !shared_with.is_empty() {
                            text.push_str(&format!(" (shared with {})", shared_with.join(", ")));
                        }
                        text
                    }
                    Row::Group { kind, count, .. } => format!("{indent}{kind:?} {count}"),
                    Row::Table { table, .. } => format!("{indent}{}", table.name),
                    Row::Column { column, .. } => format!("{indent}.{}", column.name),
                    Row::Keyspace { table, .. } => format!("{indent}{}:*", table.name),
                    Row::Bucket { bucket, .. } => format!("{indent}[{bucket}]"),
                    Row::Prefix { prefix, .. } => format!("{indent}{prefix}"),
                    Row::Object { object, .. } => format!("{indent}{}", object.key),
                    Row::More { .. } => format!("{indent}more"),
                    Row::Note { text, .. } => format!("{indent}{text}"),
                    Row::Failure { .. } => format!("{indent}failure"),
                }
            })
            .collect()
    }

    #[test]
    fn repos_list_their_databases_then_the_shared_services_they_use() {
        let model = model();
        let (rows, found) = model.rows();
        assert_eq!(found, 0);
        assert_eq!(
            outline(&rows),
            [
                "Consoles",
                "Databases",
                "api",
                "  dev",
                "  test",
                "  redis (shared with web)",
                "  files",
                "web",
                "  main",
                "  redis (shared with api)",
                "Other services",
                "  queue",
            ]
        );
        assert!(matches!(
            rows.last(),
            Some(Row::Database {
                status: Status::NotConnected,
                ..
            })
        ));
        assert!(matches!(
            &rows[2],
            Row::Repo { engines, other: false, .. }
                if *engines == [Engine::Postgres, Engine::Redis, Engine::Minio]
        ));
    }

    #[test]
    fn an_open_database_shows_tables_views_and_a_tables_columns() {
        let mut model = model();
        model
            .loaded
            .insert("myproject_api".into(), Loaded::Schema(users_schema()));
        model.set_open(&database_key("api", "myproject_api"), true);
        model.set_open(
            &table_key(
                "api",
                "myproject_api",
                &table("login_tokens", TableKind::Table, None),
            ),
            true,
        );
        let (rows, _) = model.rows();
        let outline = outline(&rows);
        assert_eq!(
            outline[3..11],
            [
                "  dev",
                "    Table 3",
                "      users",
                "      login_tokens",
                "        .id",
                "        .user_id",
                "        .expires_at",
                "      orders",
            ]
        );
        assert_eq!(outline[11], "    View 1");
        assert!(matches!(
            &rows[3],
            Row::Database {
                status: Status::Connected,
                ..
            }
        ));
    }

    #[test]
    fn the_filter_finds_tables_and_columns_and_opens_their_branches() {
        let mut model = model();
        model
            .loaded
            .insert("myproject_api".into(), Loaded::Schema(users_schema()));
        model.filter = " USER".into();
        let (rows, found) = model.rows();
        // users, active_users, and the column user_id.
        assert_eq!(found, 3);
        assert_eq!(
            outline(&rows),
            [
                "Consoles",
                "Databases",
                "api",
                "  dev",
                "    Table 2",
                "      users",
                "      login_tokens",
                "        .user_id",
                "    View 1",
                "      active_users",
            ]
        );
        let hits: Vec<Option<Range<usize>>> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Table { hit, .. } | Row::Column { hit, .. } => Some(hit.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(hits, [Some(0..4), None, Some(0..4), Some(7..11)]);
        model.filter = "nothing".into();
        let (rows, found) = model.rows();
        assert_eq!(found, 0);
        assert_eq!(outline(&rows), ["Consoles", "Databases"]);
    }

    #[test]
    fn a_bucket_lists_folders_objects_and_a_way_to_load_more() {
        let mut model = model();
        model
            .loaded
            .insert("files".into(), Loaded::Buckets(vec!["uploads".into()]));
        model.set_open(&database_key("api", "files"), true);
        model.set_open(&bucket_key("api", "files", "uploads"), true);
        let object = |key: &str, size: u64| ObjectEntry {
            key: key.into(),
            size,
            ..ObjectEntry::default()
        };
        model.folders.insert(
            folder_key("files", "uploads", ""),
            Folder::listed(Listing {
                prefixes: vec!["avatars/".into()],
                objects: vec![object("invoice.pdf", 96_000)],
                next: Some("token".into()),
            }),
        );
        model.stats.insert(
            folder_key("files", "uploads", "avatars/"),
            Some(PrefixStats {
                objects: 2940,
                bytes: 1_200_000_000,
                capped: false,
            }),
        );
        let (rows, _) = model.rows();
        let files = rows
            .iter()
            .position(|row| matches!(row, Row::Database { index: 4, .. }))
            .unwrap_or(0);
        assert_eq!(
            outline(&rows)[files..files + 5],
            [
                "  files",
                "    [uploads]",
                "      avatars/",
                "      invoice.pdf",
                "      more"
            ]
        );
        assert!(matches!(
            &rows[files + 2],
            Row::Prefix { stats: Some(stats), open: false, .. } if stats.objects == 2940
        ));
        model.set_open(&prefix_key("api", "files", "uploads", "avatars/"), true);
        let (rows, _) = model.rows();
        assert_eq!(outline(&rows)[files + 3], "        Loading...");
    }

    #[test]
    fn keys_and_counts_read_short() {
        assert_eq!(format_count(37), "37");
        assert_eq!(format_count(1284), "1.3k");
        assert_eq!(format_count(88_210), "88k");
        assert_eq!(format_count(2_500_000), "2.5M");
        assert_eq!(prefix_name("uploads/avatars/"), "avatars");
        assert_eq!(find("ex", "Index"), Some(3..5));
        let mut model = model();
        assert!(model.is_open(&repo_key("api")));
        model.toggle(&repo_key("api"));
        assert!(!model.is_open(&repo_key("api")));
        model.toggle(&database_key("api", "x"));
        assert!(model.is_open(&database_key("api", "x")));
        model.collapse_all();
        assert!(!model.is_open(&database_key("api", "x")) && !model.is_open(&repo_key("web")));
        assert_eq!(model.place_of("myproject_api"), "api > dev");
        assert_eq!(model.place_of("redis"), "redis");
    }
}
