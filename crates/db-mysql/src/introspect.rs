//! Schema introspection through `information_schema`.
//!
//! Text columns are cast to `CHAR`: MySQL 8 returns some `information_schema` text as binary,
//! which would not decode as a string.

use std::collections::BTreeMap;

use application::ports::{DbError, IntrospectScope};
use domain::{Catalog, Column, DataType, ForeignKey, Index, Schema, SchemaTree, Table, TableKind};
use sqlx::mysql::MySqlConnection;
use sqlx::{AssertSqlSafe, Row};

use crate::errors::map_error;

/// Schemas that belong to the server itself.
const SYSTEM_SCHEMAS: &str = "('mysql', 'information_schema', 'performance_schema', 'sys')";

type Key = (String, String); // (schema, table)

/// Restricts a query to user schemas, optionally to one (`?` bound twice by the caller).
fn schema_filter(column: &str) -> String {
    format!("{column} NOT IN {SYSTEM_SCHEMAS} AND (? IS NULL OR {column} = ?)")
}

fn s(r: &sqlx::mysql::MySqlRow, i: usize) -> Result<String, DbError> {
    r.try_get::<Option<String>, _>(i)
        .map(Option::unwrap_or_default)
        .map_err(map_error)
}

fn opt(r: &sqlx::mysql::MySqlRow, i: usize) -> Result<Option<String>, DbError> {
    r.try_get::<Option<String>, _>(i).map_err(map_error)
}

pub(crate) async fn load(
    conn: &mut MySqlConnection,
    scope: &IntrospectScope,
) -> Result<SchemaTree, DbError> {
    let filter: Option<&str> = match scope {
        IntrospectScope::All => None,
        IntrospectScope::Schema(name) => Some(name.as_str()),
    };
    let mut tables: BTreeMap<Key, Table> = BTreeMap::new();

    // Only static fragments are interpolated; every value is bound.
    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT CAST(TABLE_SCHEMA AS CHAR), CAST(TABLE_NAME AS CHAR), CAST(TABLE_TYPE AS CHAR)
         FROM information_schema.TABLES WHERE {} ORDER BY 1, 2",
        schema_filter("TABLE_SCHEMA")
    )))
    .bind(filter)
    .bind(filter)
    .fetch_all(&mut *conn)
    .await
    .map_err(map_error)?;
    for r in rows {
        let (schema, name, kind) = (s(&r, 0)?, s(&r, 1)?, s(&r, 2)?);
        let kind = if kind.contains("VIEW") {
            TableKind::View
        } else {
            TableKind::Table
        };
        tables.insert((schema, name.clone()), Table::new(name, kind));
    }

    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT CAST(TABLE_SCHEMA AS CHAR), CAST(TABLE_NAME AS CHAR), CAST(COLUMN_NAME AS CHAR),
                CAST(COLUMN_TYPE AS CHAR), CAST(IS_NULLABLE AS CHAR), CAST(COLUMN_DEFAULT AS CHAR)
         FROM information_schema.COLUMNS WHERE {}
         ORDER BY TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION",
        schema_filter("TABLE_SCHEMA")
    )))
    .bind(filter)
    .bind(filter)
    .fetch_all(&mut *conn)
    .await
    .map_err(map_error)?;
    for r in rows {
        if let Some(table) = tables.get_mut(&(s(&r, 0)?, s(&r, 1)?)) {
            table.columns.push(Column {
                name: s(&r, 2)?,
                data_type: DataType::new(s(&r, 3)?),
                nullable: s(&r, 4)? == "YES",
                default: opt(&r, 5)?,
            });
        }
    }

    // Primary keys and foreign keys, columns in constraint order.
    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT CAST(TABLE_SCHEMA AS CHAR), CAST(TABLE_NAME AS CHAR), CAST(CONSTRAINT_NAME AS CHAR),
                CAST(COLUMN_NAME AS CHAR), CAST(REFERENCED_TABLE_SCHEMA AS CHAR),
                CAST(REFERENCED_TABLE_NAME AS CHAR), CAST(REFERENCED_COLUMN_NAME AS CHAR)
         FROM information_schema.KEY_COLUMN_USAGE
         WHERE (CONSTRAINT_NAME = 'PRIMARY' OR REFERENCED_TABLE_NAME IS NOT NULL) AND {}
         ORDER BY TABLE_SCHEMA, TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION",
        schema_filter("TABLE_SCHEMA")
    )))
    .bind(filter)
    .bind(filter)
    .fetch_all(&mut *conn)
    .await
    .map_err(map_error)?;
    for r in rows {
        let Some(table) = tables.get_mut(&(s(&r, 0)?, s(&r, 1)?)) else {
            continue;
        };
        let (constraint, column) = (s(&r, 2)?, s(&r, 3)?);
        if constraint == "PRIMARY" {
            table.primary_key.push(column);
            continue;
        }
        let (ref_schema, ref_table, ref_column) = (opt(&r, 4)?, s(&r, 5)?, s(&r, 6)?);
        match table
            .foreign_keys
            .iter_mut()
            .find(|fk| fk.name == constraint)
        {
            Some(fk) => {
                fk.columns.push(column);
                fk.referenced_columns.push(ref_column);
            }
            None => table.foreign_keys.push(ForeignKey {
                name: constraint,
                columns: vec![column],
                referenced_schema: ref_schema,
                referenced_table: ref_table,
                referenced_columns: vec![ref_column],
            }),
        }
    }

    // Secondary indexes (the primary key is reported separately).
    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT CAST(TABLE_SCHEMA AS CHAR), CAST(TABLE_NAME AS CHAR), CAST(INDEX_NAME AS CHAR),
                CAST(COLUMN_NAME AS CHAR), CAST(NON_UNIQUE AS SIGNED)
         FROM information_schema.STATISTICS
         WHERE INDEX_NAME <> 'PRIMARY' AND {}
         ORDER BY TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
        schema_filter("TABLE_SCHEMA")
    )))
    .bind(filter)
    .bind(filter)
    .fetch_all(&mut *conn)
    .await
    .map_err(map_error)?;
    for r in rows {
        let Some(table) = tables.get_mut(&(s(&r, 0)?, s(&r, 1)?)) else {
            continue;
        };
        let (name, column) = (s(&r, 2)?, opt(&r, 3)?);
        let non_unique: i64 = r.try_get(4).map_err(map_error)?;
        // Functional (expression) indexes have no column name; they are skipped.
        let Some(column) = column else { continue };
        match table.indexes.iter_mut().find(|i| i.name == name) {
            Some(index) => index.columns.push(column),
            None => table.indexes.push(Index {
                name,
                columns: vec![column],
                unique: non_unique == 0,
            }),
        }
    }

    let mut schemas: BTreeMap<String, Vec<Table>> = BTreeMap::new();
    for ((schema, _), table) in tables {
        schemas.entry(schema).or_default().push(table);
    }
    Ok(SchemaTree {
        catalogs: vec![Catalog {
            name: "def".into(),
            schemas: schemas
                .into_iter()
                .map(|(name, tables)| Schema { name, tables })
                .collect(),
        }],
    })
}
