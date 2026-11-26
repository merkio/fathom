//! Schema introspection through `pg_catalog`.

use std::collections::BTreeMap;

use application::ports::{DbError, IntrospectScope};
use domain::{Catalog, Column, DataType, ForeignKey, Index, Schema, SchemaTree, Table, TableKind};
use sqlx::postgres::PgConnection;
use sqlx::{AssertSqlSafe, Row};

use crate::errors::map_error;

/// Relations to list: tables, partitioned tables, views, materialized views. System schemas
/// are excluded; `$1` optionally restricts to one schema.
const RELATION_FILTER: &str = "c.relkind IN ('r', 'p', 'v', 'm')
    AND n.nspname NOT IN ('pg_catalog', 'information_schema')
    AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp\\_%'
    AND ($1::text IS NULL OR n.nspname = $1)";

// Only the static `RELATION_FILTER` is interpolated into the queries below, never user input.
type Key = (String, String); // (schema, table)

pub(crate) async fn load(
    conn: &mut PgConnection,
    scope: &IntrospectScope,
) -> Result<SchemaTree, DbError> {
    let schema_filter: Option<&str> = match scope {
        IntrospectScope::All => None,
        IntrospectScope::Schema(name) => Some(name.as_str()),
    };

    let catalog: String = sqlx::query("SELECT current_database()")
        .fetch_one(&mut *conn)
        .await
        .map_err(map_error)?
        .try_get(0)
        .map_err(map_error)?;

    let mut tables: BTreeMap<Key, Table> = BTreeMap::new();

    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT n.nspname::text, c.relname::text, c.relkind::text
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE {RELATION_FILTER} ORDER BY 1, 2"
    )))
    .bind(schema_filter)
    .fetch_all(&mut *conn)
    .await
    .map_err(map_error)?;
    for r in rows {
        let (schema, name, relkind): (String, String, String) = (
            r.try_get(0).map_err(map_error)?,
            r.try_get(1).map_err(map_error)?,
            r.try_get(2).map_err(map_error)?,
        );
        let kind = if relkind == "v" || relkind == "m" {
            TableKind::View
        } else {
            TableKind::Table
        };
        tables.insert((schema, name.clone()), Table::new(name, kind));
    }

    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT n.nspname::text, c.relname::text, a.attname::text,
                format_type(a.atttypid, a.atttypmod), NOT a.attnotnull,
                pg_get_expr(d.adbin, d.adrelid)
         FROM pg_attribute a
         JOIN pg_class c ON c.oid = a.attrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
         WHERE a.attnum > 0 AND NOT a.attisdropped AND {RELATION_FILTER}
         ORDER BY n.nspname, c.relname, a.attnum"
    )))
    .bind(schema_filter)
    .fetch_all(&mut *conn)
    .await
    .map_err(map_error)?;
    for r in rows {
        let key = (
            r.try_get(0).map_err(map_error)?,
            r.try_get(1).map_err(map_error)?,
        );
        if let Some(table) = tables.get_mut(&key) {
            table.columns.push(Column {
                name: r.try_get(2).map_err(map_error)?,
                data_type: DataType::new(r.try_get::<String, _>(3).map_err(map_error)?),
                nullable: r.try_get(4).map_err(map_error)?,
                default: r.try_get(5).map_err(map_error)?,
            });
        }
    }

    // Primary keys and foreign keys, with columns in constraint order.
    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT n.nspname::text, c.relname::text, con.conname::text, con.contype::text,
                ARRAY(SELECT a.attname::text
                      FROM unnest(con.conkey) WITH ORDINALITY k(attnum, ord)
                      JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
                      ORDER BY k.ord),
                rn.nspname::text, rc.relname::text,
                ARRAY(SELECT a.attname::text
                      FROM unnest(con.confkey) WITH ORDINALITY k(attnum, ord)
                      JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum
                      ORDER BY k.ord)
         FROM pg_constraint con
         JOIN pg_class c ON c.oid = con.conrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         LEFT JOIN pg_class rc ON rc.oid = con.confrelid
         LEFT JOIN pg_namespace rn ON rn.oid = rc.relnamespace
         WHERE con.contype IN ('p', 'f') AND {RELATION_FILTER}
         ORDER BY n.nspname, c.relname, con.conname"
    )))
    .bind(schema_filter)
    .fetch_all(&mut *conn)
    .await
    .map_err(map_error)?;
    for r in rows {
        let key = (
            r.try_get(0).map_err(map_error)?,
            r.try_get(1).map_err(map_error)?,
        );
        let Some(table) = tables.get_mut(&key) else {
            continue;
        };
        let columns: Vec<String> = r.try_get(4).map_err(map_error)?;
        match r.try_get::<String, _>(3).map_err(map_error)?.as_str() {
            "p" => table.primary_key = columns,
            _ => table.foreign_keys.push(ForeignKey {
                name: r.try_get(2).map_err(map_error)?,
                columns,
                referenced_schema: r.try_get(5).map_err(map_error)?,
                referenced_table: r
                    .try_get::<Option<String>, _>(6)
                    .map_err(map_error)?
                    .unwrap_or_default(),
                referenced_columns: r.try_get(7).map_err(map_error)?,
            }),
        }
    }

    // Secondary indexes (the primary key is reported separately).
    let rows = sqlx::query(AssertSqlSafe(format!(
        "SELECT n.nspname::text, c.relname::text, i.relname::text, ix.indisunique,
                ARRAY(SELECT a.attname::text
                      FROM unnest(ix.indkey::int2[]) WITH ORDINALITY k(attnum, ord)
                      JOIN pg_attribute a ON a.attrelid = ix.indrelid AND a.attnum = k.attnum
                      WHERE k.attnum > 0 ORDER BY k.ord)
         FROM pg_index ix
         JOIN pg_class i ON i.oid = ix.indexrelid
         JOIN pg_class c ON c.oid = ix.indrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE NOT ix.indisprimary AND {RELATION_FILTER}
         ORDER BY n.nspname, c.relname, i.relname"
    )))
    .bind(schema_filter)
    .fetch_all(&mut *conn)
    .await
    .map_err(map_error)?;
    for r in rows {
        let key = (
            r.try_get(0).map_err(map_error)?,
            r.try_get(1).map_err(map_error)?,
        );
        if let Some(table) = tables.get_mut(&key) {
            table.indexes.push(Index {
                name: r.try_get(2).map_err(map_error)?,
                unique: r.try_get(3).map_err(map_error)?,
                columns: r.try_get(4).map_err(map_error)?,
            });
        }
    }

    let mut schemas: BTreeMap<String, Vec<Table>> = BTreeMap::new();
    for ((schema, _), table) in tables {
        schemas.entry(schema).or_default().push(table);
    }
    Ok(SchemaTree {
        catalogs: vec![Catalog {
            name: catalog,
            schemas: schemas
                .into_iter()
                .map(|(name, tables)| Schema { name, tables })
                .collect(),
        }],
    })
}
