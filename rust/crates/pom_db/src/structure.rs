//! A table's shape beyond its column list (nullability, defaults, indexes, the foreign keys that point at it) and
//! the SQL the table browser builds from what the user types or edits.

use crate::{quote_identifier, Table};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    /// `(table, column)` a foreign key on this column points at.
    pub references: Option<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub columns: String,
    pub unique: bool,
    pub primary: bool,
}

/// A foreign key elsewhere that points at this table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reference {
    pub schema: String,
    pub table: String,
    pub column: String,
    pub on_delete: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableStructure {
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<Index>,
    pub referenced_by: Vec<Reference>,
}

impl TableStructure {
    pub fn column(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns.iter().find(|column| column.name == name)
    }

    pub fn primary_key(&self) -> Vec<&str> {
        self.columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| column.name.as_str())
            .collect()
    }
}

pub fn quote_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// What a column's filter box holds, as a WHERE condition: `= x` matches exactly, `null` / `not null` test for
/// NULL, `!= x` excludes, anything else is a case-insensitive substring of the value as text.
pub fn filter_condition(column: &str, typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let name = quote_identifier(column);
    let lower = typed.to_ascii_lowercase();
    Some(if lower == "null" {
        format!("{name} IS NULL")
    } else if lower == "not null" {
        format!("{name} IS NOT NULL")
    } else if let Some(value) = typed.strip_prefix("!=") {
        format!("{name}::text <> {}", quote_literal(value.trim()))
    } else if let Some(value) = typed.strip_prefix('=') {
        format!("{name}::text = {}", quote_literal(value.trim()))
    } else {
        let escaped = typed
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!(
            "{name}::text ILIKE {}",
            quote_literal(&format!("%{escaped}%"))
        )
    })
}

/// The typed WHERE and every column filter, joined with AND.
pub fn combined_filter(filter: &str, conditions: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !filter.trim().is_empty() {
        parts.push(format!("({})", filter.trim()));
    }
    parts.extend(conditions.iter().cloned());
    parts.join(" AND ")
}

/// `UPDATE` of one cell, found by the row's primary key values.
pub fn update_statement(
    table: &Table,
    key: &[(String, Option<String>)],
    column: &str,
    value: Option<&str>,
) -> String {
    update_parts(table, key, column, value).join(" ")
}

/// The same statement over three lines (UPDATE, SET, WHERE), for reading it back.
pub fn update_statement_lines(
    table: &Table,
    key: &[(String, Option<String>)],
    column: &str,
    value: Option<&str>,
) -> String {
    update_parts(table, key, column, value).join("\n")
}

fn update_parts(
    table: &Table,
    key: &[(String, Option<String>)],
    column: &str,
    value: Option<&str>,
) -> [String; 3] {
    let set = match value {
        Some(value) => quote_literal(value),
        None => "NULL".into(),
    };
    let condition = key
        .iter()
        .map(|(name, value)| match value {
            Some(value) => format!("{} = {}", quote_identifier(name), quote_literal(value)),
            None => format!("{} IS NULL", quote_identifier(name)),
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    [
        format!("UPDATE {}", table.sql_name()),
        format!("SET {} = {set}", quote_identifier(column)),
        format!("WHERE {condition}"),
    ]
}

/// The row a foreign key value points at.
pub fn referenced_row_query(schema: &str, table: &str, column: &str, value: &str) -> String {
    format!(
        "SELECT * FROM {}.{} WHERE {}::text = {} LIMIT 1",
        quote_identifier(if schema.is_empty() { "public" } else { schema }),
        quote_identifier(table),
        quote_identifier(column),
        quote_literal(value)
    )
}

/// How many rows of another table point at this value.
pub fn reference_count_query(reference: &Reference, value: &str) -> String {
    format!(
        "SELECT count(*) FROM {}.{} WHERE {}::text = {}",
        quote_identifier(&reference.schema),
        quote_identifier(&reference.table),
        quote_identifier(&reference.column),
        quote_literal(value)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TableKind;

    #[test]
    fn a_reviewed_update_reads_over_three_lines() {
        let key = [("id".to_string(), Some("7".to_string()))];
        assert_eq!(
            update_statement_lines(&users(), &key, "role", Some("admin")),
            "UPDATE \"public\".\"users\"\nSET \"role\" = 'admin'\nWHERE \"id\" = '7'"
        );
    }

    fn users() -> Table {
        Table {
            schema: "public".into(),
            name: "users".into(),
            kind: TableKind::Table,
            count: None,
        }
    }

    #[test]
    fn filter_boxes_read_as_conditions() {
        assert_eq!(filter_condition("id", "  "), None);
        assert_eq!(
            filter_condition("id", "= 7").as_deref(),
            Some("\"id\"::text = '7'")
        );
        assert_eq!(
            filter_condition("role", "!= admin").as_deref(),
            Some("\"role\"::text <> 'admin'")
        );
        assert_eq!(
            filter_condition("seen", "NULL").as_deref(),
            Some("\"seen\" IS NULL")
        );
        assert_eq!(
            filter_condition("email", "o'k 50%").as_deref(),
            Some("\"email\"::text ILIKE '%o''k 50\\%%'")
        );
        assert_eq!(
            combined_filter("id > 1", &["\"a\" IS NULL".into()]),
            "(id > 1) AND \"a\" IS NULL"
        );
        assert_eq!(combined_filter(" ", &[]), "");
    }

    #[test]
    fn a_cell_update_finds_its_row_by_primary_key() {
        assert_eq!(
            update_statement(
                &users(),
                &[("id".into(), Some("7".into()))],
                "name",
                Some("O'Neil")
            ),
            "UPDATE \"public\".\"users\" SET \"name\" = 'O''Neil' WHERE \"id\" = '7'"
        );
        assert_eq!(
            update_statement(&users(), &[("id".into(), Some("7".into()))], "seen", None),
            "UPDATE \"public\".\"users\" SET \"seen\" = NULL WHERE \"id\" = '7'"
        );
    }
}
