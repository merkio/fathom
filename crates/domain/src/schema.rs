//! Schema model: `Catalog -> Schema -> Table/View -> Column`, plus indexes and foreign keys.

use serde::{Deserialize, Serialize};

/// Database-reported type name (for example `"varchar(255)"`), kept as text so every database fits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DataType(pub String);

impl DataType {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_schema: Option<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableKind {
    Table,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<Column>,
    pub primary_key: Vec<String>,
    pub indexes: Vec<Index>,
    pub foreign_keys: Vec<ForeignKey>,
}

impl Table {
    pub fn new(name: impl Into<String>, kind: TableKind) -> Self {
        Self {
            name: name.into(),
            kind,
            columns: Vec::new(),
            primary_key: Vec::new(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
        }
    }

    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<Table>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub name: String,
    pub schemas: Vec<Schema>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaTree {
    pub catalogs: Vec<Catalog>,
}

impl SchemaTree {
    /// Iterates over every table with the name of its schema.
    pub fn tables(&self) -> impl Iterator<Item = (&str, &Table)> {
        self.catalogs
            .iter()
            .flat_map(|c| &c.schemas)
            .flat_map(|s| s.tables.iter().map(move |t| (s.name.as_str(), t)))
    }

    /// Finds a table by name, case-insensitively, in any schema.
    pub fn find_table(&self, name: &str) -> Option<(&str, &Table)> {
        self.tables()
            .find(|(_, t)| t.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> SchemaTree {
        let mut users = Table::new("Users", TableKind::Table);
        users.columns.push(Column {
            name: "id".into(),
            data_type: DataType::new("integer"),
            nullable: false,
            default: None,
        });
        SchemaTree {
            catalogs: vec![Catalog {
                name: "app".into(),
                schemas: vec![Schema {
                    name: "public".into(),
                    tables: vec![users],
                }],
            }],
        }
    }

    #[test]
    fn find_table_is_case_insensitive_and_reports_schema() {
        let tree = tree();
        let (schema, table) = tree.find_table("users").expect("table exists");
        assert_eq!((schema, table.name.as_str()), ("public", "Users"));
        assert!(tree.find_table("missing").is_none());
    }

    #[test]
    fn column_lookup_is_exact() {
        let tree = tree();
        let (_, table) = tree.find_table("users").expect("table exists");
        assert!(table.column("id").is_some());
        assert!(table.column("ID").is_none());
    }

    #[test]
    fn empty_tree_has_no_tables() {
        assert_eq!(SchemaTree::default().tables().count(), 0);
    }
}
