//! Snapshot tests for every prompt template. A change here is a change to what is sent to LLM
//! providers: review the snapshot diff, then bump the template version.
#![allow(clippy::expect_used)] // test helpers

use application::llm::{Prompt, PromptBuilder, PromptContext, SchemaContextBuilder};
use domain::{
    Catalog, ChatMessage, Column, DataType, Dialect, ForeignKey, LimitStyle, QueryResult,
    ResultColumn, Schema, SchemaTree, Table, TableKind, Value,
};

fn schema() -> SchemaTree {
    let col = |n: &str, t: &str, nullable: bool| Column {
        name: n.into(),
        data_type: DataType::new(t),
        nullable,
        default: None,
    };
    let mut customers = Table::new("customers", TableKind::Table);
    customers.columns = vec![
        col("id", "integer", false),
        col("name", "text", false),
        col("email", "text", true),
    ];
    customers.primary_key = vec!["id".into()];
    let mut orders = Table::new("orders", TableKind::Table);
    orders.columns = vec![
        col("id", "integer", false),
        col("customer_id", "integer", false),
        col("total", "numeric(10,2)", true),
    ];
    orders.primary_key = vec!["id".into()];
    orders.foreign_keys.push(ForeignKey {
        name: "orders_customer_fk".into(),
        columns: vec!["customer_id".into()],
        referenced_schema: Some("public".into()),
        referenced_table: "customers".into(),
        referenced_columns: vec!["id".into()],
    });
    SchemaTree {
        catalogs: vec![Catalog {
            name: "shop".into(),
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![customers, orders],
            }],
        }],
    }
}

fn postgres() -> Dialect {
    Dialect {
        name: "postgres",
        limit_style: LimitStyle::Limit,
        ..Dialect::ANSI
    }
}

fn render(p: &Prompt) -> String {
    let mut out = format!(
        "template: {}\n\n--- system ---\n{}\n\n--- messages ---\n",
        p.template, p.system
    );
    for m in &p.messages {
        out.push_str(&format!("[{:?}]\n{}\n\n", m.role, m.content));
    }
    out
}

fn with_ctx<R>(rows: Option<&QueryResult>, f: impl FnOnce(&PromptContext<'_>) -> R) -> R {
    let tree = schema();
    let schema_ctx = SchemaContextBuilder::new(2000).build(&tree, "");
    let dialect = postgres();
    f(&PromptContext {
        dialect: &dialect,
        schema: &schema_ctx,
        sample_rows: rows,
    })
}

#[test]
fn generate_sql() {
    let p = with_ctx(None, |c| {
        PromptBuilder.generate_sql(c, "total revenue per customer", None)
    });
    insta::assert_snapshot!(render(&p));
}

#[test]
fn generate_sql_with_editor_content() {
    let p = with_ctx(None, |c| {
        PromptBuilder.generate_sql(
            c,
            "only the top 5",
            Some("SELECT customer_id, sum(total) FROM orders GROUP BY 1"),
        )
    });
    insta::assert_snapshot!(render(&p));
}

#[test]
fn explain_sql() {
    let p = with_ctx(None, |c| {
        PromptBuilder.explain_sql(
            c,
            "SELECT * FROM orders o JOIN customers c ON c.id = o.customer_id",
        )
    });
    insta::assert_snapshot!(render(&p));
}

#[test]
fn optimize_sql() {
    let p = with_ctx(None, |c| {
        PromptBuilder.optimize_sql(
            c,
            "SELECT * FROM orders WHERE customer_id IN (SELECT id FROM customers)",
        )
    });
    insta::assert_snapshot!(render(&p));
}

#[test]
fn fix_sql() {
    let p = with_ctx(None, |c| {
        PromptBuilder.fix_sql(
            c,
            "SELECT nam FROM customers",
            "column \"nam\" does not exist",
        )
    });
    insta::assert_snapshot!(render(&p));
}

#[test]
fn chat() {
    let history = [
        ChatMessage::user("what tables exist?"),
        ChatMessage::assistant("customers and orders"),
    ];
    let p = with_ctx(None, |c| {
        PromptBuilder.chat(c, &history, "how are they related?")
    });
    insta::assert_snapshot!(render(&p));
}

#[test]
fn generate_sql_with_opted_in_sample_rows() {
    let rows = QueryResult {
        columns: vec![
            ResultColumn {
                name: "id".into(),
                data_type: DataType::new("integer"),
            },
            ResultColumn {
                name: "name".into(),
                data_type: DataType::new("text"),
            },
        ],
        rows: vec![
            vec![Value::Int(1), Value::Text("Ada".into())],
            vec![Value::Int(2), Value::Null],
        ],
        ..QueryResult::empty()
    };
    let p = with_ctx(Some(&rows), |c| {
        PromptBuilder.generate_sql(c, "customers without a name", None)
    });
    insta::assert_snapshot!(render(&p));
}

#[test]
fn mysql_style_dialect_changes_quoting_and_limit_hint() {
    let tree = schema();
    let schema_ctx = SchemaContextBuilder::new(2000).build(&tree, "");
    let mysql = Dialect {
        name: "mysql",
        identifier_quote: '`',
        limit_style: LimitStyle::Limit,
        ..Dialect::ANSI
    };
    let ctx = PromptContext {
        dialect: &mysql,
        schema: &schema_ctx,
        sample_rows: None,
    };
    let p = PromptBuilder.explain_sql(&ctx, "SELECT 1");
    insta::assert_snapshot!(render(&p));
}

#[test]
fn empty_schema() {
    let schema_ctx = SchemaContextBuilder::new(2000).build(&SchemaTree::default(), "");
    let d = Dialect::ANSI;
    let ctx = PromptContext {
        dialect: &d,
        schema: &schema_ctx,
        sample_rows: None,
    };
    insta::assert_snapshot!(render(&PromptBuilder.chat(&ctx, &[], "hello")));
}
