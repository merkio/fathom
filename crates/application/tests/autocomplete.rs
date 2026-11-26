//! Table-driven tests for local autocomplete. `|` marks the cursor in the SQL under test.
#![allow(clippy::expect_used)] // test helpers

use application::sql::{
    CompletionRequest, Suggestion, SuggestionKind, SuggestionSource, changes_schema,
};
use application::usecases::CompleteSql;
use domain::{
    Catalog, Column, DataType, Dialect, ForeignKey, Schema, SchemaTree, Table, TableKind,
};

fn pg() -> Dialect {
    Dialect {
        name: "postgres-like",
        keywords: &[
            "SELECT",
            "FROM",
            "WHERE",
            "GROUP BY",
            "ORDER BY",
            "JOIN",
            "LEFT JOIN",
            "LIMIT",
        ],
        functions: &["COUNT", "SUM", "COALESCE"],
        ..Dialect::ANSI
    }
}

fn mysql() -> Dialect {
    Dialect {
        name: "mysql-like",
        identifier_quote: '`',
        quote_mixed_case: false,
        ..pg()
    }
}

fn col(name: &str, ty: &str) -> Column {
    Column {
        name: name.into(),
        data_type: DataType::new(ty),
        nullable: true,
        default: None,
    }
}

fn table(name: &str, cols: &[(&str, &str)]) -> Table {
    let mut t = Table::new(name, TableKind::Table);
    t.columns = cols.iter().map(|(n, ty)| col(n, ty)).collect();
    t
}

fn fk(cols: &[&str], table: &str, ref_cols: &[&str]) -> ForeignKey {
    ForeignKey {
        name: format!("fk_{table}"),
        columns: cols.iter().map(|s| (*s).to_owned()).collect(),
        referenced_schema: Some("public".into()),
        referenced_table: table.into(),
        referenced_columns: ref_cols.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn tree() -> SchemaTree {
    let customers = table(
        "customers",
        &[("id", "integer"), ("name", "text"), ("email", "text")],
    );
    let mut orders = table(
        "orders",
        &[
            ("id", "integer"),
            ("customer_id", "integer"),
            ("total", "numeric"),
            ("placed", "date"),
        ],
    );
    orders
        .foreign_keys
        .push(fk(&["customer_id"], "customers", &["id"]));
    let mut items = table(
        "order_items",
        &[("id", "integer"), ("order_id", "integer"), ("sku", "text")],
    );
    items
        .foreign_keys
        .push(fk(&["order_id"], "orders", &["id"]));
    let user_data = table("UserData", &[("id", "integer"), ("FirstName", "text")]);
    let events = table("events", &[("id", "integer"), ("payload", "jsonb")]);
    SchemaTree {
        catalogs: vec![Catalog {
            name: "app".into(),
            schemas: vec![
                Schema {
                    name: "public".into(),
                    tables: vec![customers, orders, items, user_data],
                },
                Schema {
                    name: "audit".into(),
                    tables: vec![events],
                },
            ],
        }],
    }
}

fn suggestions(
    dialect: &Dialect,
    schema: &SchemaTree,
    marked: &str,
    recent: &[&str],
) -> Vec<Suggestion> {
    let cursor = marked.find('|').expect("cursor marker");
    let sql = marked.replacen('|', "", 1);
    let recent: Vec<String> = recent.iter().map(|s| (*s).to_owned()).collect();
    CompleteSql.execute(&CompletionRequest {
        sql: &sql,
        cursor,
        schema,
        dialect,
        recent_queries: &recent,
    })
}

fn texts(dialect: &Dialect, marked: &str) -> Vec<String> {
    suggestions(dialect, &tree(), marked, &[])
        .into_iter()
        .map(|s| s.text)
        .collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn tables_after_from_and_join_and_into() {
    for sql in [
        "SELECT * FROM |",
        "SELECT * FROM customers c JOIN |",
        "INSERT INTO |",
        "UPDATE |",
    ] {
        let r = texts(&pg(), sql);
        for t in [
            "customers",
            "orders",
            "order_items",
            "\"UserData\"",
            "events",
        ] {
            assert!(r.contains(&t.to_owned()), "{sql}: missing {t} in {r:?}");
        }
        assert!(
            !r.iter().any(|t| t == "name" || t == "SELECT"),
            "{sql}: unexpected {r:?}"
        );
    }
}

#[test]
fn table_prefix_filters_case_insensitively() {
    assert_eq!(texts(&pg(), "SELECT * FROM cu|"), ["customers"]);
    assert_eq!(texts(&pg(), "SELECT * FROM CU|"), ["customers"]);
    assert_eq!(
        sorted(texts(&pg(), "SELECT * FROM ord|")),
        ["order_items", "orders"]
    );
}

#[test]
fn comma_in_from_list_expects_another_table() {
    let r = texts(&pg(), "SELECT * FROM customers, ord|");
    assert_eq!(sorted(r), ["order_items", "orders"]);
}

#[test]
fn join_prefers_tables_linked_by_foreign_key() {
    let r = texts(&pg(), "SELECT * FROM customers c JOIN |");
    assert_eq!(r[0], "orders", "{r:?}");
    let r = texts(&pg(), "SELECT * FROM orders o JOIN |");
    assert_eq!(
        sorted(r[..2].to_vec()),
        ["customers", "order_items"],
        "{r:?}"
    );
}

#[test]
fn join_conditions_come_from_foreign_keys_in_both_directions() {
    let r = texts(&pg(), "SELECT * FROM orders o JOIN customers c ON |");
    assert_eq!(r[0], "o.customer_id = c.id", "{r:?}");
    let r = texts(&pg(), "SELECT * FROM customers c JOIN orders o ON |");
    assert_eq!(r[0], "o.customer_id = c.id", "{r:?}");
    let first = &suggestions(
        &pg(),
        &tree(),
        "SELECT * FROM orders o JOIN customers c ON |",
        &[],
    )[0];
    assert_eq!(
        (first.kind, first.source),
        (SuggestionKind::Join, SuggestionSource::Local)
    );
}

#[test]
fn join_condition_without_aliases_uses_table_names_and_chains_multi_column_keys() {
    let r = texts(&pg(), "SELECT * FROM orders JOIN customers ON |");
    assert_eq!(r[0], "orders.customer_id = customers.id");

    let mut a = table("a", &[("x", "int"), ("y", "int")]);
    a.foreign_keys.push(fk(&["x", "y"], "b", &["x", "y"]));
    let b = table("b", &[("x", "int"), ("y", "int")]);
    let schema = SchemaTree {
        catalogs: vec![Catalog {
            name: "d".into(),
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![a, b],
            }],
        }],
    };
    let r = suggestions(&pg(), &schema, "SELECT * FROM a JOIN b ON |", &[]);
    assert_eq!(r[0].text, "a.x = b.x AND a.y = b.y");
}

#[test]
fn alias_dot_lists_that_tables_columns() {
    let expected = ["email", "id", "name"];
    for sql in [
        "SELECT c.| FROM customers c",
        "SELECT x.| FROM customers AS x",
        "SELECT * FROM customers c JOIN orders o ON o.customer_id = c.| ",
        "SELECT * FROM customers c WHERE c.|",
        "SELECT * FROM customers WHERE customers.|",
    ] {
        assert_eq!(sorted(texts(&pg(), sql)), expected, "{sql}");
    }
    assert_eq!(texts(&pg(), "SELECT c.na| FROM customers c"), ["name"]);
}

#[test]
fn unknown_qualifier_gives_nothing() {
    assert!(texts(&pg(), "SELECT zz.| FROM customers").is_empty());
}

#[test]
fn schema_qualification() {
    assert_eq!(texts(&pg(), "SELECT * FROM audit.|"), ["events"]);
    assert_eq!(texts(&pg(), "SELECT * FROM public.cu|"), ["customers"]);
    assert_eq!(
        texts(&pg(), "SELECT * FROM public.customers c WHERE c.i|"),
        ["id"]
    );
    assert_eq!(
        texts(&pg(), "SELECT public.customers.na| FROM public.customers"),
        ["name"]
    );
}

#[test]
fn schema_names_are_offered_when_typed() {
    let r = suggestions(&pg(), &tree(), "SELECT * FROM aud|", &[]);
    assert_eq!(
        r.iter()
            .map(|s| (s.text.as_str(), s.kind))
            .collect::<Vec<_>>(),
        [("audit.", SuggestionKind::Schema)]
    );
}

#[test]
fn ambiguous_table_names_are_schema_qualified() {
    let mut s = tree();
    s.catalogs[0].schemas[1]
        .tables
        .push(table("customers", &[("id", "integer")]));
    let r: Vec<String> = suggestions(&pg(), &s, "SELECT * FROM cust|", &[])
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert_eq!(sorted(r), ["audit.customers", "public.customers"]);
}

#[test]
fn columns_of_tables_in_scope_even_when_from_follows_the_cursor() {
    let r = texts(&pg(), "SELECT | FROM customers");
    assert_eq!(sorted(r[..3].to_vec()), ["email", "id", "name"], "{r:?}");
    assert!(
        r[3..].iter().all(|t| t.ends_with('(')),
        "functions follow columns: {r:?}"
    );
    assert_eq!(texts(&pg(), "SELECT na| FROM customers"), ["name"]);
    assert_eq!(texts(&pg(), "SELECT id, na| FROM customers"), ["name"]);
}

#[test]
fn columns_in_other_clauses() {
    assert_eq!(texts(&pg(), "SELECT * FROM customers WHERE na|"), ["name"]);
    assert_eq!(
        texts(&pg(), "SELECT * FROM customers WHERE id = 1 AND em|"),
        ["email"]
    );
    assert_eq!(
        texts(&pg(), "SELECT count(*) FROM orders GROUP BY pl|"),
        ["placed"]
    );
    assert_eq!(
        texts(&pg(), "SELECT * FROM orders ORDER BY tot|"),
        ["total"]
    );
    assert_eq!(texts(&pg(), "UPDATE customers SET em|"), ["email"]);
    assert_eq!(
        texts(&pg(), "SELECT * FROM customers WHERE LOWER(na|)"),
        ["name"]
    );
}

#[test]
fn columns_that_exist_in_several_tables_are_qualified() {
    let r = texts(&pg(), "SELECT i| FROM customers c JOIN orders o ON 1=1");
    assert_eq!(sorted(r), ["c.id", "o.id"]);
    let r = texts(&pg(), "SELECT tot| FROM customers c JOIN orders o ON 1=1");
    assert_eq!(r, ["total"]);
}

#[test]
fn only_the_current_statement_counts() {
    let r = texts(&pg(), "SELECT id FROM customers; SELECT cust| FROM orders");
    assert_eq!(r, ["customer_id"]);
    // `email` belongs to the previous statement's table only.
    let r = texts(&pg(), "SELECT id FROM customers; SELECT em| FROM orders");
    assert!(r.is_empty(), "previous statement leaked into scope: {r:?}");
    let r = texts(&pg(), "SELECT em| FROM orders; SELECT id FROM customers");
    assert!(r.is_empty(), "email is not in orders: {r:?}");
}

#[test]
fn subqueries_share_the_statements_scope() {
    let r = texts(
        &pg(),
        "SELECT * FROM orders WHERE customer_id IN (SELECT em| FROM customers)",
    );
    assert_eq!(r, ["email"]);
}

#[test]
fn keywords_after_a_complete_expression() {
    let r = texts(&pg(), "SELECT * FROM customers |");
    assert!(
        r.contains(&"WHERE".to_owned()) && r.contains(&"JOIN".to_owned()),
        "{r:?}"
    );
    assert_eq!(texts(&pg(), "SELECT * FROM customers WH|"), ["WHERE"]);
    assert_eq!(texts(&pg(), "select * from customers wh|"), ["where"]);
    assert_eq!(texts(&pg(), "SELECT * FROM customers gr|"), ["group by"]);
    assert_eq!(texts(&pg(), "SELECT id FR|"), ["FROM"]);
    assert_eq!(texts(&pg(), "sel|"), ["select"]);
}

#[test]
fn functions_complete_with_a_paren_and_follow_the_typed_case() {
    assert_eq!(texts(&pg(), "SELECT cou| FROM customers"), ["count("]);
    assert_eq!(texts(&pg(), "SELECT COU| FROM customers"), ["COUNT("]);
}

#[test]
fn nothing_inside_strings_comments_numbers_or_after_as() {
    for sql in [
        "SELECT 'cu|",
        "SELECT 'a|b' FROM customers",
        "-- cu|",
        "/* cu| */",
        "SELECT 12|",
        "SELECT id AS cu|",
    ] {
        assert!(texts(&pg(), sql).is_empty(), "{sql}");
    }
}

#[test]
fn postgres_quoting() {
    // Mixed case must be quoted to survive PostgreSQL's case folding.
    assert_eq!(texts(&pg(), "SELECT * FROM Us|"), ["\"UserData\""]);
    // Inside an open quote the name is inserted with its closing quote.
    assert_eq!(texts(&pg(), "SELECT * FROM \"User|"), ["UserData\""]);
    // With the closing quote already present it is not duplicated.
    assert_eq!(texts(&pg(), "SELECT * FROM \"User|\""), ["UserData"]);
    assert_eq!(
        texts(&pg(), "SELECT u.| FROM \"UserData\" u")
            .iter()
            .filter(|t| t.contains("First"))
            .count(),
        1
    );
    assert_eq!(
        texts(&pg(), "SELECT u.Fi| FROM \"UserData\" u"),
        ["\"FirstName\""]
    );
    assert_eq!(texts(&pg(), "SELECT * FROM \"user|"), ["UserData\""]);
}

#[test]
fn mysql_quoting() {
    assert_eq!(texts(&mysql(), "SELECT * FROM `cust|`"), ["customers"]);
    assert_eq!(texts(&mysql(), "SELECT * FROM `cust|"), ["customers`"]);
    // MySQL does not fold case, so mixed-case names need no quotes.
    assert_eq!(texts(&mysql(), "SELECT * FROM Us|"), ["UserData"]);
    assert_eq!(
        texts(&mysql(), "SELECT u.Fi| FROM `UserData` u"),
        ["FirstName"]
    );
    // Double quotes are strings in MySQL: no completion inside them.
    assert!(texts(&mysql(), "SELECT * FROM \"cu|").is_empty());
}

#[test]
fn names_with_special_characters_are_quoted() {
    let mut s = tree();
    s.catalogs[0].schemas[0]
        .tables
        .push(table("order items", &[("id", "integer")]));
    s.catalogs[0].schemas[0]
        .tables
        .push(table("2fast", &[("id", "integer")]));
    let r: Vec<String> = suggestions(&pg(), &s, "SELECT * FROM |", &[])
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert!(
        r.contains(&"\"order items\"".to_owned()) && r.contains(&"\"2fast\"".to_owned()),
        "{r:?}"
    );
}

#[test]
fn recently_used_names_rank_first_within_their_group() {
    let recent = [
        "select email from customers",
        "select email, name from customers",
    ];
    let r = suggestions(&pg(), &tree(), "SELECT | FROM customers", &recent);
    assert_eq!(r[0].text, "email");
    assert_eq!(r[1].text, "name");
    let r = suggestions(
        &pg(),
        &tree(),
        "SELECT * FROM |",
        &["select * from orders", "select * from orders"],
    );
    assert_eq!(r[0].text, "orders");
}

#[test]
fn ranges_are_byte_offsets_that_replace_the_typed_prefix() {
    let marked = "SELECT 'é' FROM cu|";
    let sql = marked.replace('|', "");
    let s = &suggestions(&pg(), &tree(), marked, &[])[0];
    assert_eq!(&sql[s.range.clone()], "cu");

    let marked = "SELECT * FROM \"User|\" WHERE 1=1";
    let sql = marked.replace('|', "");
    let s = &suggestions(&pg(), &tree(), marked, &[])[0];
    assert_eq!(&sql[s.range.clone()], "User");

    let marked = "SELECT c.| FROM customers c";
    let s = &suggestions(&pg(), &tree(), marked, &[])[0];
    assert_eq!(s.range, 9..9);
}

#[test]
fn details_describe_columns_and_tables() {
    let r = suggestions(&pg(), &tree(), "SELECT na| FROM customers", &[]);
    assert_eq!(r[0].detail.as_deref(), Some("text (customers)"));
    let r = suggestions(&pg(), &tree(), "SELECT * FROM cu|", &[]);
    assert_eq!(
        (r[0].kind, r[0].detail.as_deref()),
        (SuggestionKind::Table, Some("public"))
    );
}

#[test]
fn works_on_garbage_and_edge_positions() {
    for sql in [
        "|",
        "|SELECT",
        "SELECT|",
        "((((|",
        ")))|",
        "SELECT * FROM (|",
        ";|;",
        "SELECT ,,, | FROM",
    ] {
        let _ = texts(&pg(), sql); // must not panic
    }
    // Cursor past the end or inside a multibyte character is clamped, not a panic.
    let s = tree();
    for cursor in [0, 1, 2, 3, 99] {
        let _ = CompleteSql.execute(&CompletionRequest {
            sql: "é é é",
            cursor,
            schema: &s,
            dialect: &pg(),
            recent_queries: &[],
        });
    }
    assert!(
        texts(&pg(), "|")
            .iter()
            .all(|t| t.chars().all(|c| !c.is_lowercase()))
    );
}

#[test]
fn view_kinds_are_reported() {
    let mut s = tree();
    s.catalogs[0].schemas[0]
        .tables
        .push(Table::new("customer_names", TableKind::View));
    let r = suggestions(&pg(), &s, "SELECT * FROM customer_n|", &[]);
    assert_eq!(r[0].kind, SuggestionKind::View);
}

#[test]
fn detects_statements_that_change_the_schema() {
    let d = pg();
    for sql in [
        "DROP TABLE x",
        "create table t (id int)",
        "select 1; ALTER TABLE t ADD c int",
        "  truncate t",
    ] {
        assert!(changes_schema(sql, &d), "{sql}");
    }
    for sql in [
        "select 'drop table x'",
        "-- drop table x\nselect 1",
        "select * from create_log",
        "insert into t values (1)",
        "",
    ] {
        assert!(!changes_schema(sql, &d), "{sql}");
    }
}
