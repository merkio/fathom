//! Edits to rows of one table, as the user staged them in a result grid.
//!
//! Rows are identified by primary key values, never by position, so a change cannot hit another
//! row if the data moved. Adapters turn these into statements through the shared builder in
//! `application`; nothing here is SQL.

use crate::Value;

/// One staged change to one row.
#[derive(Debug, Clone, PartialEq)]
pub enum RowChange {
    Update {
        /// Primary key column and value pairs that identify the row.
        key: Vec<(String, Value)>,
        /// Column and new value pairs.
        set: Vec<(String, Value)>,
    },
    Delete {
        key: Vec<(String, Value)>,
    },
}

/// Changes for one table, applied together.
#[derive(Debug, Clone, PartialEq)]
pub struct TableChanges {
    pub schema: Option<String>,
    pub table: String,
    /// Declared type of each column as introspection reported it. Databases that need an
    /// explicit conversion for typed columns use it; others ignore it.
    pub column_types: Vec<(String, String)>,
    pub changes: Vec<RowChange>,
}

impl TableChanges {
    pub fn column_type(&self, column: &str) -> Option<&str> {
        self.column_types
            .iter()
            .find(|(c, _)| c == column)
            .map(|(_, t)| t.as_str())
    }
}

/// What a batch did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Statements that ran.
    pub applied: usize,
    /// Rows the database reports as changed. `None` when it cannot say (ClickHouse mutations).
    pub rows_affected: Option<u64>,
    /// All changes were applied atomically (false for databases without transactions).
    pub atomic: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_types_are_looked_up_by_name() {
        let t = TableChanges {
            schema: None,
            table: "t".into(),
            column_types: vec![("id".into(), "integer".into())],
            changes: vec![],
        };
        assert_eq!(t.column_type("id"), Some("integer"));
        assert_eq!(t.column_type("missing"), None);
    }
}
