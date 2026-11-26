//! Schema introspection through `sqlite_master` and the `PRAGMA` table functions.

use application::ports::{DbError, IntrospectScope};
use domain::{Catalog, Column, DataType, ForeignKey, Index, Schema, SchemaTree, Table, TableKind};
use rusqlite::Connection;

fn query_err(e: rusqlite::Error) -> DbError {
    DbError::Query(e.to_string())
}

/// Quotes a name for use inside a PRAGMA argument.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub(crate) fn load(conn: &Connection, scope: &IntrospectScope) -> Result<SchemaTree, DbError> {
    // SQLite has one schema per attached database; we expose the main one.
    if let IntrospectScope::Schema(name) = scope
        && !name.eq_ignore_ascii_case("main")
    {
        return Ok(SchemaTree::default());
    }

    let mut stmt = conn
        .prepare(
            "SELECT name, type FROM sqlite_master
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
             ORDER BY name",
        )
        .map_err(query_err)?;
    let relations: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(query_err)?
        .collect::<Result<_, _>>()
        .map_err(query_err)?;

    let mut tables = Vec::with_capacity(relations.len());
    for (name, kind) in relations {
        let kind = if kind == "view" {
            TableKind::View
        } else {
            TableKind::Table
        };
        let mut table = Table::new(name.clone(), kind);
        columns(conn, &name, &mut table)?;
        foreign_keys(conn, &name, &mut table)?;
        indexes(conn, &name, &mut table)?;
        tables.push(table);
    }
    Ok(SchemaTree {
        catalogs: vec![Catalog {
            name: "main".into(),
            schemas: vec![Schema {
                name: "main".into(),
                tables,
            }],
        }],
    })
}

fn columns(conn: &Connection, name: &str, table: &mut Table) -> Result<(), DbError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quoted(name)))
        .map_err(query_err)?;
    // cid, name, type, notnull, dflt_value, pk (1-based position within the key, 0 if none)
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(query_err)?;
    let mut key: Vec<(i64, String)> = Vec::new();
    for row in rows {
        let (col, ty, notnull, default, pk) = row.map_err(query_err)?;
        if pk > 0 {
            key.push((pk, col.clone()));
        }
        table.columns.push(Column {
            name: col,
            data_type: DataType::new(if ty.is_empty() {
                "any".to_owned()
            } else {
                ty.to_lowercase()
            }),
            nullable: notnull == 0 && pk == 0,
            default,
        });
    }
    key.sort();
    table.primary_key = key.into_iter().map(|(_, c)| c).collect();
    Ok(())
}

fn foreign_keys(conn: &Connection, name: &str, table: &mut Table) -> Result<(), DbError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA foreign_key_list({})", quoted(name)))
        .map_err(query_err)?;
    // id, seq, table, from, to, ...  (one row per column; rows with the same id form one key)
    let rows: Vec<(i64, String, String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(query_err)?
        .collect::<Result<_, _>>()
        .map_err(query_err)?;
    let mut keys: Vec<(i64, ForeignKey)> = Vec::new();
    for (id, target, from, to) in rows {
        if let Some((_, fk)) = keys.iter_mut().find(|(i, _)| *i == id) {
            fk.columns.push(from);
            fk.referenced_columns.extend(to);
        } else {
            keys.push((
                id,
                ForeignKey {
                    name: format!("fk_{name}_{id}"),
                    columns: vec![from],
                    referenced_schema: Some("main".into()),
                    referenced_table: target,
                    referenced_columns: to.into_iter().collect(),
                },
            ));
        }
    }
    table.foreign_keys = keys.into_iter().map(|(_, fk)| fk).collect();
    Ok(())
}

fn indexes(conn: &Connection, name: &str, table: &mut Table) -> Result<(), DbError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA index_list({})", quoted(name)))
        .map_err(query_err)?;
    // seq, name, unique, origin ('c' created, 'u' unique constraint, 'pk' primary key), partial
    let list: Vec<(String, bool, String)> = stmt
        .query_map([], |r| Ok((r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)))
        .map_err(query_err)?
        .collect::<Result<_, _>>()
        .map_err(query_err)?;
    for (index, unique, origin) in list {
        // The primary key is reported separately.
        if origin == "pk" {
            continue;
        }
        let mut info = conn
            .prepare(&format!("PRAGMA index_info({})", quoted(&index)))
            .map_err(query_err)?;
        let columns: Vec<String> = info
            .query_map([], |r| r.get::<_, Option<String>>(2))
            .map_err(query_err)?
            .filter_map(|c| c.transpose())
            .collect::<Result<_, _>>()
            .map_err(query_err)?;
        table.indexes.push(Index {
            name: index,
            columns,
            unique,
        });
    }
    Ok(())
}
