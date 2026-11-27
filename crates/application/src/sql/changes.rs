//! Turns staged row changes into parameterised statements, and decides whether a query's result
//! can be edited at all.
//!
//! Values are always bound as parameters, never written into the SQL text. Every statement is
//! keyed by the full primary key, so it can only touch the row the user edited.

use domain::{Dialect, RowChange, SchemaTree, TableChanges, Value};
use sqlparser::ast::{Expr, GroupByExpr, SelectItem, SetExpr, Statement, TableFactor};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use crate::ports::DbError;

/// How a driver writes its parameter placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamStyle {
    /// `$1`, `$2`, ...
    Dollar,
    /// `?`
    Question,
}

#[derive(Debug, Clone, Copy)]
pub struct BuildOptions<'a> {
    pub dialect: &'a Dialect,
    pub style: ParamStyle,
    /// Bind every value as text and convert it with `CAST(.. AS <column type>)`. For databases
    /// that do not convert text parameters for typed columns on their own.
    pub casts: bool,
    /// Write `ALTER TABLE .. UPDATE/DELETE` (ClickHouse mutations) instead of plain statements.
    /// These run in the background and report no row count, so no single-row check applies.
    pub mutations: bool,
}

/// One statement with its parameters, in placeholder order.
#[derive(Debug, Clone, PartialEq)]
pub struct StatementPlan {
    pub sql: String,
    pub params: Vec<Value>,
    /// The statement must change exactly one row (it is keyed by the primary key); anything else
    /// means the data moved and the batch should be rolled back.
    pub one_row: bool,
}

fn fail(message: impl Into<String>) -> DbError {
    DbError::Query(message.into())
}

fn text_of(v: &Value) -> Result<String, DbError> {
    Ok(match v {
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Decimal(s) | Value::Text(s) | Value::Temporal(s) | Value::Json(s) => s.clone(),
        Value::Bytes(_) => return Err(fail("binary values cannot be edited here")),
        Value::Null => return Err(fail("internal: NULL has no text form")),
    })
}

struct Params<'a> {
    opts: BuildOptions<'a>,
    values: Vec<Value>,
}

impl Params<'_> {
    /// The SQL for one value: `NULL`, or a placeholder (wrapped in a cast when needed).
    fn push(
        &mut self,
        value: &Value,
        column: &str,
        changes: &TableChanges,
    ) -> Result<String, DbError> {
        if value.is_null() {
            return Ok("NULL".into());
        }
        let n = self.values.len() + 1;
        let placeholder = match self.opts.style {
            ParamStyle::Dollar => format!("${n}"),
            ParamStyle::Question => "?".to_owned(),
        };
        if self.opts.casts {
            let ty = changes
                .column_type(column)
                .ok_or_else(|| fail(format!("unknown column `{column}`")))?;
            self.values.push(Value::Text(text_of(value)?));
            let text_param = match self.opts.style {
                ParamStyle::Dollar => format!("{placeholder}::text"),
                ParamStyle::Question => placeholder,
            };
            Ok(format!("CAST({text_param} AS {ty})"))
        } else {
            self.values.push(value.clone());
            Ok(placeholder)
        }
    }
}

fn where_clause(
    key: &[(String, Value)],
    params: &mut Params<'_>,
    changes: &TableChanges,
) -> Result<String, DbError> {
    if key.is_empty() {
        return Err(fail("a row without a primary key cannot be changed"));
    }
    let mut parts = Vec::with_capacity(key.len());
    for (column, value) in key {
        if value.is_null() {
            return Err(fail(format!("primary key column `{column}` has no value")));
        }
        let p = params.push(value, column, changes)?;
        parts.push(format!(
            "{} = {p}",
            params.opts.dialect.quote_identifier(column)
        ));
    }
    Ok(parts.join(" AND "))
}

/// One statement per change, in order.
pub fn build(
    opts: BuildOptions<'_>,
    changes: &TableChanges,
) -> Result<Vec<StatementPlan>, DbError> {
    let d = opts.dialect;
    let table = match &changes.schema {
        Some(schema) => format!(
            "{}.{}",
            d.quote_identifier(schema),
            d.quote_identifier(&changes.table)
        ),
        None => d.quote_identifier(&changes.table),
    };
    let mut plans = Vec::with_capacity(changes.changes.len());
    for change in &changes.changes {
        // Each statement numbers its own placeholders from 1.
        let mut params = Params {
            opts,
            values: Vec::new(),
        };
        let sql = match change {
            RowChange::Update { key, set } => {
                if set.is_empty() {
                    return Err(fail("an update with no changed column"));
                }
                let mut assignments = Vec::with_capacity(set.len());
                for (column, value) in set {
                    let p = params.push(value, column, changes)?;
                    assignments.push(format!("{} = {p}", d.quote_identifier(column)));
                }
                let filter = where_clause(key, &mut params, changes)?;
                let assignments = assignments.join(", ");
                if opts.mutations {
                    format!("ALTER TABLE {table} UPDATE {assignments} WHERE {filter}")
                } else {
                    format!("UPDATE {table} SET {assignments} WHERE {filter}")
                }
            }
            RowChange::Delete { key } => {
                let filter = where_clause(key, &mut params, changes)?;
                if opts.mutations {
                    format!("ALTER TABLE {table} DELETE WHERE {filter}")
                } else {
                    format!("DELETE FROM {table} WHERE {filter}")
                }
            }
        };
        plans.push(StatementPlan {
            sql,
            params: params.values,
            one_row: !opts.mutations,
        });
    }
    Ok(plans)
}

/// The single table a query reads, when its result rows map one-to-one onto table rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableSource {
    pub schema: String,
    pub table: String,
    pub primary_key: Vec<String>,
}

/// `Some` when `sql` is a plain `SELECT` from one table that has a primary key, and the result
/// is guaranteed to carry every primary key column. Joins, grouping, `DISTINCT`, subqueries,
/// expressions and aliases make a result non-editable.
pub fn editable_source(sql: &str, tree: &SchemaTree) -> Option<EditableSource> {
    let statements = Parser::parse_sql(&GenericDialect {}, sql).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    if query.with.is_some() {
        return None;
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    let plain_group =
        matches!(&select.group_by, GroupByExpr::Expressions(e, m) if e.is_empty() && m.is_empty());
    if select.distinct.is_some() || !plain_group || select.having.is_some() || select.into.is_some()
    {
        return None;
    }
    let [from] = select.from.as_slice() else {
        return None;
    };
    if !from.joins.is_empty() {
        return None;
    }
    let TableFactor::Table { name, .. } = &from.relation else {
        return None;
    };
    let parts: Vec<String> = name
        .0
        .iter()
        .map(|p| p.as_ident().map(|i| i.value.clone()))
        .collect::<Option<_>>()?;
    let (schema_hint, table_name) = match parts.as_slice() {
        [t] => (None, t.clone()),
        [s, t] => (Some(s.clone()), t.clone()),
        _ => return None,
    };

    // Which columns the result carries: `*`, or plain column names without aliases.
    let mut selected: Vec<String> = Vec::new();
    let mut wildcard = false;
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(_) => wildcard = true,
            SelectItem::UnnamedExpr(Expr::Identifier(i)) => selected.push(i.value.clone()),
            SelectItem::UnnamedExpr(Expr::CompoundIdentifier(ids)) => {
                selected.push(ids.last()?.value.clone());
            }
            _ => return None,
        }
    }

    let (schema, table) = tree
        .catalogs
        .iter()
        .flat_map(|c| &c.schemas)
        .flat_map(|s| s.tables.iter().map(move |t| (s, t)))
        .find(|(s, t)| {
            t.name.eq_ignore_ascii_case(&table_name)
                && schema_hint
                    .as_ref()
                    .is_none_or(|h| s.name.eq_ignore_ascii_case(h))
        })?;
    if table.kind != domain::TableKind::Table || table.primary_key.is_empty() {
        return None;
    }
    let carries =
        |column: &str| wildcard || selected.iter().any(|s| s.eq_ignore_ascii_case(column));
    if !table.primary_key.iter().all(|k| carries(k)) {
        return None;
    }
    Some(EditableSource {
        schema: schema.name.clone(),
        table: table.name.clone(),
        primary_key: table.primary_key.clone(),
    })
}

#[cfg(test)]
mod tests {
    use domain::{Catalog, Column, DataType, Schema, Table, TableKind};

    use super::*;

    fn key(v: i64) -> Vec<(String, Value)> {
        vec![("id".into(), Value::Int(v))]
    }

    fn changes(list: Vec<RowChange>) -> TableChanges {
        TableChanges {
            schema: Some("public".into()),
            table: "customers".into(),
            column_types: vec![
                ("id".into(), "integer".into()),
                ("name".into(), "text".into()),
                ("plan".into(), "text".into()),
            ],
            changes: list,
        }
    }

    fn opts(style: ParamStyle, casts: bool) -> BuildOptions<'static> {
        BuildOptions {
            dialect: &Dialect::ANSI,
            style,
            casts,
            mutations: false,
        }
    }

    fn update() -> RowChange {
        RowChange::Update {
            key: key(1042),
            set: vec![
                ("plan".into(), Value::Text("Pro".into())),
                ("name".into(), Value::Null),
            ],
        }
    }

    #[test]
    fn updates_bind_values_and_key_with_dollar_placeholders() {
        let plans = build(opts(ParamStyle::Dollar, false), &changes(vec![update()])).unwrap();
        assert_eq!(
            plans[0].sql,
            r#"UPDATE "public"."customers" SET "plan" = $1, "name" = NULL WHERE "id" = $2"#
        );
        assert_eq!(
            plans[0].params,
            [Value::Text("Pro".into()), Value::Int(1042)]
        );
        assert!(plans[0].one_row);
    }

    #[test]
    fn question_mark_style_keeps_parameter_order() {
        let plans = build(opts(ParamStyle::Question, false), &changes(vec![update()])).unwrap();
        assert_eq!(
            plans[0].sql,
            r#"UPDATE "public"."customers" SET "plan" = ?, "name" = NULL WHERE "id" = ?"#
        );
    }

    #[test]
    fn deletes_are_keyed_by_the_whole_primary_key() {
        let delete = RowChange::Delete {
            key: vec![
                ("a".into(), Value::Int(1)),
                ("b".into(), Value::Text("x".into())),
            ],
        };
        let mut c = changes(vec![delete]);
        c.column_types
            .extend([("a".into(), "int".into()), ("b".into(), "text".into())]);
        let plans = build(opts(ParamStyle::Dollar, false), &c).unwrap();
        assert_eq!(
            plans[0].sql,
            r#"DELETE FROM "public"."customers" WHERE "a" = $1 AND "b" = $2"#
        );
    }

    #[test]
    fn cast_mode_binds_text_and_converts_to_the_column_type() {
        let plans = build(opts(ParamStyle::Dollar, true), &changes(vec![update()])).unwrap();
        assert_eq!(
            plans[0].sql,
            r#"UPDATE "public"."customers" SET "plan" = CAST($1::text AS text), "name" = NULL WHERE "id" = CAST($2::text AS integer)"#
        );
        assert_eq!(
            plans[0].params,
            [Value::Text("Pro".into()), Value::Text("1042".into())]
        );
        let q = build(opts(ParamStyle::Question, true), &changes(vec![update()])).unwrap();
        assert!(q[0].sql.contains("CAST(? AS integer)"));
    }

    #[test]
    fn mutation_syntax_for_clickhouse_style_databases() {
        let o = BuildOptions {
            mutations: true,
            ..opts(ParamStyle::Question, true)
        };
        let plans = build(
            o,
            &changes(vec![update(), RowChange::Delete { key: key(7) }]),
        )
        .unwrap();
        assert_eq!(
            plans[0].sql,
            r#"ALTER TABLE "public"."customers" UPDATE "plan" = CAST(? AS text), "name" = NULL WHERE "id" = CAST(? AS integer)"#
        );
        assert_eq!(
            plans[1].sql,
            r#"ALTER TABLE "public"."customers" DELETE WHERE "id" = CAST(? AS integer)"#
        );
        assert!(
            plans.iter().all(|p| !p.one_row),
            "mutations report no row count"
        );
    }

    #[test]
    fn every_statement_numbers_its_own_placeholders() {
        let plans = build(
            opts(ParamStyle::Dollar, false),
            &changes(vec![update(), update()]),
        )
        .unwrap();
        assert_eq!(plans[0].sql, plans[1].sql);
    }

    #[test]
    fn values_never_appear_in_the_sql_text() {
        let evil = RowChange::Update {
            key: key(1),
            set: vec![(
                "plan".into(),
                Value::Text("x'; DROP TABLE customers; --".into()),
            )],
        };
        let plans = build(opts(ParamStyle::Dollar, false), &changes(vec![evil])).unwrap();
        assert!(!plans[0].sql.contains("DROP"), "{}", plans[0].sql);
        assert_eq!(plans[0].params.len(), 2);
    }

    #[test]
    fn identifiers_are_quoted_and_escaped() {
        let mut c = changes(vec![RowChange::Delete { key: key(1) }]);
        c.table = "we\"ird".into();
        let plans = build(opts(ParamStyle::Dollar, false), &c).unwrap();
        assert!(plans[0].sql.contains(r#""we""ird""#));
    }

    #[test]
    fn unsafe_changes_are_refused() {
        let c = |ch| build(opts(ParamStyle::Dollar, false), &changes(vec![ch]));
        assert!(c(RowChange::Delete { key: vec![] }).is_err(), "no key");
        assert!(
            c(RowChange::Delete {
                key: vec![("id".into(), Value::Null)]
            })
            .is_err(),
            "null key"
        );
        assert!(
            c(RowChange::Update {
                key: key(1),
                set: vec![]
            })
            .is_err(),
            "empty set"
        );
        let binary = RowChange::Update {
            key: key(1),
            set: vec![("plan".into(), Value::Bytes(vec![1]))],
        };
        assert!(build(opts(ParamStyle::Dollar, true), &changes(vec![binary])).is_err());
        let unknown = RowChange::Update {
            key: key(1),
            set: vec![("ghost".into(), Value::Text("x".into()))],
        };
        assert!(build(opts(ParamStyle::Dollar, true), &changes(vec![unknown])).is_err());
    }

    // ---- editable_source ---------------------------------------------------------------------

    fn tree() -> SchemaTree {
        let col = |n: &str| Column {
            name: n.into(),
            data_type: DataType::new("text"),
            nullable: true,
            default: None,
        };
        let mut customers = Table::new("customers", TableKind::Table);
        customers.columns = vec![col("id"), col("name")];
        customers.primary_key = vec!["id".into()];
        let mut nopk = Table::new("logs", TableKind::Table);
        nopk.columns = vec![col("msg")];
        let mut view = Table::new("v", TableKind::View);
        view.primary_key = vec!["id".into()];
        SchemaTree {
            catalogs: vec![Catalog {
                name: "db".into(),
                schemas: vec![Schema {
                    name: "public".into(),
                    tables: vec![customers, nopk, view],
                }],
            }],
        }
    }

    fn source(sql: &str) -> Option<EditableSource> {
        editable_source(sql, &tree())
    }

    #[test]
    fn plain_selects_from_a_keyed_table_are_editable() {
        let s = source("SELECT * FROM public.customers WHERE id > 3 ORDER BY id LIMIT 10;");
        assert_eq!(
            s,
            Some(EditableSource {
                schema: "public".into(),
                table: "customers".into(),
                primary_key: vec!["id".into()]
            })
        );
        assert!(
            source("select id, name from CUSTOMERS").is_some(),
            "case-insensitive"
        );
        assert!(source("SELECT c.id, c.name FROM customers c").is_some());
    }

    #[test]
    fn results_that_do_not_map_to_rows_are_not_editable() {
        for sql in [
            "SELECT name FROM customers",            // key not in the result
            "SELECT id, name AS n FROM customers",   // alias
            "SELECT id, upper(name) FROM customers", // expression
            "SELECT DISTINCT id FROM customers",
            "SELECT id, count(*) FROM customers GROUP BY id",
            "SELECT * FROM customers a JOIN customers b ON a.id = b.id",
            "SELECT * FROM customers, logs",
            "SELECT * FROM (SELECT * FROM customers) t",
            "WITH c AS (SELECT * FROM customers) SELECT * FROM c",
            "SELECT * FROM customers UNION SELECT * FROM customers",
            "SELECT * FROM logs",            // no primary key
            "SELECT * FROM v",               // a view
            "SELECT * FROM missing",         // unknown table
            "SELECT * FROM other.customers", // wrong schema
            "UPDATE customers SET name = 'x'",
            "SELECT 1; SELECT * FROM customers",
            "not sql at all",
        ] {
            assert_eq!(source(sql), None, "{sql}");
        }
    }
}
