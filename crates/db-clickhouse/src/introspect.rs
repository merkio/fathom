//! Schema introspection from ClickHouse's `system` tables.
//!
//! A ClickHouse "database" is what the rest of the application calls a schema. There are no
//! foreign keys; the sorting key is the primary key, and data-skipping indexes are listed as
//! (non-unique) indexes.

use std::collections::BTreeMap;

use application::ports::{DbError, IntrospectScope};
use clickhouse::Client;
use domain::{Catalog, Column, DataType, Index, Schema, SchemaTree, Table, TableKind};
use serde_json::Value as Json;

use crate::error::map_error;
use crate::values::unwrap_type;

/// Databases that belong to the server itself.
const SYSTEM_DATABASES: &str = "'system', 'INFORMATION_SCHEMA', 'information_schema'";

/// Engines that are views rather than tables.
const VIEW_ENGINES: &[&str] = &["View", "MaterializedView", "LiveView", "WindowView"];

type Rows = Vec<Vec<Json>>;

async fn fetch(client: &Client, sql: &str, scope: Option<&str>) -> Result<Rows, DbError> {
    let mut query = client.query(sql);
    if let Some(database) = scope {
        query = query.bind(database);
    }
    let mut cursor = query.fetch_bytes("JSONCompact").map_err(map_error)?;
    let body = cursor.collect().await.map_err(map_error)?;
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let json: Json = serde_json::from_slice(&body)
        .map_err(|e| DbError::Query(format!("unreadable schema from the server: {e}")))?;
    Ok(json
        .get("data")
        .and_then(Json::as_array)
        .map(|rows| {
            rows.iter()
                .map(|r| r.as_array().cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default())
}

pub(crate) async fn load(client: &Client, scope: &IntrospectScope) -> Result<SchemaTree, DbError> {
    let (filter, bind) = match scope {
        IntrospectScope::All => (String::new(), None),
        IntrospectScope::Schema(name) => (" AND database = ?".to_owned(), Some(name.as_str())),
    };
    let tables = fetch(
        client,
        &format!(
            "SELECT database, name, engine FROM system.tables \
             WHERE database NOT IN ({SYSTEM_DATABASES}){filter} ORDER BY database, name"
        ),
        bind,
    )
    .await?;
    let columns = fetch(
        client,
        &format!(
            "SELECT database, table, name, type, default_kind, default_expression, is_in_primary_key \
             FROM system.columns WHERE database NOT IN ({SYSTEM_DATABASES}){filter} \
             ORDER BY database, table, position"
        ),
        bind,
    )
    .await?;
    let indexes = fetch(
        client,
        &format!(
            "SELECT database, table, name, expr FROM system.data_skipping_indices \
             WHERE database NOT IN ({SYSTEM_DATABASES}){filter} ORDER BY database, table, name"
        ),
        bind,
    )
    .await?;
    Ok(assemble(&tables, &columns, &indexes))
}

fn s(row: &[Json], i: usize) -> String {
    row.get(i)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The flag column may arrive as a number or a string depending on the server version.
fn flag(row: &[Json], i: usize) -> bool {
    match row.get(i) {
        Some(Json::Number(n)) => n.as_u64() == Some(1),
        Some(Json::String(t)) => t == "1",
        Some(Json::Bool(b)) => *b,
        _ => false,
    }
}

/// Splits an index expression such as `a, b` or `(a, b)` into column names. Expressions that are
/// not plain column lists (`lower(name)`) are returned as one entry, unchanged.
fn index_columns(expr: &str) -> Vec<String> {
    let inner = expr
        .strip_prefix('(')
        .and_then(|e| e.strip_suffix(')'))
        .unwrap_or(expr);
    if inner.contains('(') {
        return vec![inner.trim().to_owned()];
    }
    inner
        .split(',')
        .map(|c| c.trim().trim_matches('`').to_owned())
        .filter(|c| !c.is_empty())
        .collect()
}

pub(crate) fn assemble(tables: &Rows, columns: &Rows, indexes: &Rows) -> SchemaTree {
    // database -> table -> Table, in server order.
    let mut by_schema: BTreeMap<String, Vec<Table>> = BTreeMap::new();
    for row in tables {
        let (db, name, engine) = (s(row, 0), s(row, 1), s(row, 2));
        let kind = if VIEW_ENGINES.contains(&engine.as_str()) {
            TableKind::View
        } else {
            TableKind::Table
        };
        by_schema
            .entry(db)
            .or_default()
            .push(Table::new(name, kind));
    }
    for row in columns {
        let (db, table, name) = (s(row, 0), s(row, 1), s(row, 2));
        let Some(t) = by_schema
            .get_mut(&db)
            .and_then(|ts| ts.iter_mut().find(|t| t.name == table))
        else {
            continue;
        };
        let ty = s(row, 3);
        let (_, nullable) = unwrap_type(&ty);
        // `default_expression` is empty for columns without a default.
        let default = Some(s(row, 5)).filter(|e| !e.is_empty());
        if flag(row, 6) {
            t.primary_key.push(name.clone());
        }
        t.columns.push(Column {
            name,
            data_type: DataType::new(ty),
            nullable,
            default,
        });
    }
    for row in indexes {
        let (db, table, name, expr) = (s(row, 0), s(row, 1), s(row, 2), s(row, 3));
        if let Some(t) = by_schema
            .get_mut(&db)
            .and_then(|ts| ts.iter_mut().find(|t| t.name == table))
        {
            t.indexes.push(Index {
                name,
                columns: index_columns(&expr),
                unique: false,
            });
        }
    }
    SchemaTree {
        catalogs: vec![Catalog {
            name: "clickhouse".into(),
            schemas: by_schema
                .into_iter()
                .map(|(name, tables)| Schema { name, tables })
                .collect(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rows(v: Json) -> Rows {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|r| r.as_array().cloned().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn sample() -> SchemaTree {
        assemble(
            &rows(json!([
                ["shop", "orders", "MergeTree"],
                ["shop", "orders_v", "MaterializedView"],
                ["shop", "plain_v", "View"],
            ])),
            &rows(json!([
                ["shop", "orders", "id", "Int32", "", "", 1],
                [
                    "shop",
                    "orders",
                    "note",
                    "Nullable(String)",
                    "DEFAULT",
                    "'n/a'",
                    0
                ],
                [
                    "shop",
                    "orders",
                    "tag",
                    "LowCardinality(String)",
                    "",
                    "",
                    "0"
                ],
                ["gone", "x", "ignored", "Int32", "", "", 0],
            ])),
            &rows(json!([
                ["shop", "orders", "idx_note", "note"],
                ["shop", "orders", "idx_two", "(id, note)"],
                ["shop", "orders", "idx_fn", "lower(note)"],
            ])),
        )
    }

    #[test]
    fn columns_keys_defaults_and_nullability() {
        let tree = sample();
        let (schema, t) = tree
            .find_table("orders")
            .unwrap_or_else(|| panic!("orders"));
        assert_eq!(schema, "shop");
        assert_eq!(t.primary_key, ["id"]);
        let names: Vec<_> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "note", "tag"], "server order is kept");
        assert!(!t.columns[0].nullable && t.columns[1].nullable && !t.columns[2].nullable);
        assert_eq!(t.columns[1].default.as_deref(), Some("'n/a'"));
        assert_eq!(t.columns[0].default, None);
        assert!(t.foreign_keys.is_empty());
    }

    #[test]
    fn views_are_recognised_by_engine() {
        let tree = sample();
        let kind = |n: &str| tree.find_table(n).map(|(_, t)| t.kind);
        assert_eq!(kind("orders"), Some(TableKind::Table));
        assert_eq!(kind("orders_v"), Some(TableKind::View));
        assert_eq!(kind("plain_v"), Some(TableKind::View));
    }

    #[test]
    fn indexes_are_non_unique_and_split_into_columns() {
        let tree = sample();
        let (_, t) = tree
            .find_table("orders")
            .unwrap_or_else(|| panic!("orders"));
        let by = |n: &str| {
            t.indexes
                .iter()
                .find(|i| i.name == n)
                .map(|i| i.columns.clone())
        };
        assert_eq!(by("idx_note"), Some(vec!["note".to_owned()]));
        assert_eq!(
            by("idx_two"),
            Some(vec!["id".to_owned(), "note".to_owned()])
        );
        assert_eq!(by("idx_fn"), Some(vec!["lower(note)".to_owned()]));
        assert!(t.indexes.iter().all(|i| !i.unique));
    }

    #[test]
    fn rows_for_unknown_tables_are_ignored() {
        let tree = sample();
        assert_eq!(tree.tables().count(), 3);
    }

    #[test]
    fn index_columns_handles_quotes_and_parentheses() {
        assert_eq!(index_columns("`a`, b"), ["a", "b"]);
        assert_eq!(index_columns("(a, b)"), ["a", "b"]);
        assert_eq!(index_columns("  "), Vec::<String>::new());
    }

    #[test]
    fn empty_input_gives_an_empty_tree() {
        let tree = assemble(&Vec::new(), &Vec::new(), &Vec::new());
        assert_eq!(tree.tables().count(), 0);
    }
}
